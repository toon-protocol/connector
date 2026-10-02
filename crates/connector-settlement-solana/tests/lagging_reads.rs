//! A read that follows a transaction this node sent and saw confirm must not
//! be answered from a slot before the one that transaction landed in.
//!
//! Behind a load-balanced endpoint a node that has not reached the slot
//! answers with the channel as it was. A single validator cannot lag itself,
//! so a `FakeRpc` stands in front of one and answers the first read after a
//! confirmed send as a lagging node would: with `-32016`, with the account
//! from before the transaction, or with no account at all.
//!
//! Two nodes, each behind its own fake endpoint: a payer (the outbound half:
//! `open_prepared`, `top_up`, `start_withdrawal`) and a receiver (`land` and
//! the sponsor's own admit), joined by a real HTTP sponsor endpoint.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use connector_chain_rpc::{FakeRpc, RpcCall, RpcReply};
use connector_settlement::batch::{
    BatchChannelStatus, BatchSettlementBackend, BatchSettlementError, BatchSettlementPayer,
    OutboundChannelRecord, ReceiverTerms, SolanaReceiverTerms,
};
use connector_settlement::ChannelId;
use connector_settlement_solana::batch::SolanaBatchSettlement;
use connector_settlement_solana::test_support::{
    create_mint, fund, mint_to, require_solana_test_validator, SolanaValidator,
};
use connector_settlement_solana::RpcTransport;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_api::custom_error::JSON_RPC_SERVER_ERROR_MIN_CONTEXT_SLOT_NOT_REACHED;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::transaction::VersionedTransaction;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const ONE_DAY: u64 = 86_400;
const DEPOSIT: u128 = 1_000;

/// What the fake does with `getAccountInfo` once a send has gone through.
#[derive(Default)]
struct Plan {
    /// Only reads of this account are answered by the plan.
    address: String,
    armed: bool,
    sent: bool,
    once: VecDeque<RpcReply>,
    always: Option<RpcReply>,
}

#[derive(Clone, Default)]
struct Lag(Arc<Mutex<Plan>>);

impl Lag {
    /// From the next `sendTransaction` on, answer reads with `once` (each
    /// once, in order), then with `always` if set, then truthfully.
    fn arm(&self, address: &Pubkey, once: Vec<RpcReply>, always: Option<RpcReply>) {
        *self.0.lock().unwrap() = Plan {
            address: address.to_string(),
            armed: true,
            sent: false,
            once: once.into(),
            always,
        };
    }

    /// As if a send had just gone through: for the sponsor's answer, which
    /// is HTTP and never touches the payer's RPC endpoint.
    fn begin(&self, address: &Pubkey) {
        let mut plan = self.0.lock().unwrap();
        plan.address = address.to_string();
        plan.sent = true;
    }

    /// Back to answering truthfully.
    fn heal(&self) {
        *self.0.lock().unwrap() = Plan::default();
    }

    fn script(&self) -> impl Fn(&RpcCall) -> RpcReply + Send + Sync + 'static {
        let plan = Arc::clone(&self.0);
        move |call| {
            let mut plan = plan.lock().unwrap();
            match call.method.as_str() {
                "sendTransaction" => {
                    if plan.armed {
                        plan.sent = true;
                    }
                    RpcReply::Forward
                }
                "getAccountInfo" if plan.sent && call.params[0] == plan.address.as_str() => {
                    if let Some(reply) = plan.once.pop_front() {
                        reply
                    } else {
                        plan.always.clone().unwrap_or(RpcReply::Forward)
                    }
                }
                _ => RpcReply::Forward,
            }
        }
    }
}

fn not_reached() -> RpcReply {
    RpcReply::Error {
        code: JSON_RPC_SERVER_ERROR_MIN_CONTEXT_SLOT_NOT_REACHED,
        message: "Minimum context slot has not been reached".to_string(),
    }
}

/// What the validator answers to `getAccountInfo` for `address` right now,
/// kept to be replayed later as the answer of a node that is behind.
async fn snapshot(validator: &SolanaValidator, address: &Pubkey) -> RpcReply {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "getAccountInfo",
        "params": [address.to_string(), {"encoding": "base64", "commitment": "confirmed"}],
    });
    let answer: serde_json::Value = reqwest::Client::new()
        .post(&validator.rpc_url)
        .json(&body)
        .send()
        .await
        .expect("the validator answers")
        .json()
        .await
        .expect("JSON");
    RpcReply::Result(answer["result"].clone())
}

fn seed_of(keypair: &Keypair) -> [u8; 32] {
    keypair.to_bytes()[..32].try_into().expect("seed")
}

struct Sponsor {
    url: String,
    posts: Arc<Mutex<usize>>,
    /// Answer the post with a refusal after the open has landed.
    refuse_after_landing: Arc<Mutex<bool>>,
}

/// The sponsor endpoint ADR 0074 describes, in front of `receiver`: vet,
/// co-sign, submit, admit, answer `{"channelId"}`. `payer_lag.begin()` runs
/// as it answers, so the payer's next read is the first one after it.
async fn sponsor_endpoint(receiver: Arc<SolanaBatchSettlement>, payer_lag: Lag) -> Sponsor {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}/open", listener.local_addr().unwrap());
    let posts = Arc::new(Mutex::new(0usize));
    let refuse_after_landing = Arc::new(Mutex::new(false));
    let (counted, refusing) = (Arc::clone(&posts), Arc::clone(&refuse_after_landing));
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let (receiver, payer_lag) = (Arc::clone(&receiver), payer_lag.clone());
            let (counted, refusing) = (Arc::clone(&counted), Arc::clone(&refusing));
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut chunk = [0u8; 8192];
                let body = loop {
                    let read = socket.read(&mut chunk).await.unwrap_or(0);
                    request.extend_from_slice(&chunk[..read]);
                    let text = String::from_utf8_lossy(&request).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let length = head
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if body.len() >= length || read == 0 {
                            break body.to_string();
                        }
                    } else if read == 0 {
                        return;
                    }
                };
                *counted.lock().unwrap() += 1;
                let transaction = serde_json::from_str::<serde_json::Value>(&body).unwrap()
                    ["transaction"]
                    .as_str()
                    .unwrap()
                    .to_string();
                let vetted = receiver.vet_sponsored_open(&transaction).expect("vetted");
                let channel = vetted.channel;
                let (status, answer) = match receiver.sponsor_vetted(vetted).await {
                    Ok(_) if *refusing.lock().unwrap() => (
                        "500 Internal Server Error",
                        serde_json::json!({"error": "ChainUnavailable", "detail": "no"}),
                    ),
                    Ok(open) => (
                        "200 OK",
                        serde_json::json!({ "channelId": open.channel.to_string() }),
                    ),
                    Err(refusal) => (
                        "422 Unprocessable Entity",
                        serde_json::json!({"error": refusal.name(), "detail": "refused"}),
                    ),
                };
                payer_lag.begin(&channel);
                let text = answer.to_string();
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: \
                     {}\r\nconnection: close\r\n\r\n{text}",
                    text.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    Sponsor {
        url,
        posts,
        refuse_after_landing,
    }
}

struct World {
    min_grace_period_secs: u64,
    validator: SolanaValidator,
    rpc: RpcClient,
    mint: Pubkey,
    payer: Arc<SolanaBatchSettlement>,
    payer_fake: FakeRpc,
    payer_lag: Lag,
    payer_sends: SendPlan,
    receiver: Arc<SolanaBatchSettlement>,
    receiver_fake: FakeRpc,
    receiver_lag: Lag,
    sponsor: Sponsor,
}

async fn world() -> World {
    world_admitting(ONE_DAY).await
}

/// A world whose receiver admits a `grace_period` of at least
/// `min_grace_period_secs`.
async fn world_admitting(min_grace_period_secs: u64) -> World {
    let validator = SolanaValidator::spawn().await;
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let authority = Keypair::new();
    fund(&rpc, &authority.pubkey()).await;
    let mint = create_mint(&rpc, &authority, 6).await;

    let (payer_key, receiver_key) = (Keypair::new(), Keypair::new());
    fund(&rpc, &payer_key.pubkey()).await;
    fund(&rpc, &receiver_key.pubkey()).await;
    mint_to(&rpc, &authority, &mint, &payer_key.pubkey(), 1_000_000).await;

    let (payer_lag, receiver_lag) = (Lag::default(), Lag::default());
    let payer_sends = SendPlan::new();
    let (reads, sends) = (payer_lag.script(), payer_sends.script());
    let payer_fake =
        FakeRpc::spawn_in_front_of(&validator.rpc_url, move |call| match sends(call) {
            RpcReply::Forward => reads(call),
            answer => answer,
        })
        .await;
    let receiver_fake = FakeRpc::spawn_in_front_of(&validator.rpc_url, receiver_lag.script()).await;
    let connect = |fake: &FakeRpc, key: &Keypair| {
        let transport = RpcTransport::direct(&fake.url()).expect("transport");
        let seed = seed_of(key);
        async move {
            Arc::new(
                SolanaBatchSettlement::connect(
                    &transport,
                    &seed,
                    mint,
                    6,
                    min_grace_period_secs,
                    1,
                )
                .await
                .expect("connect"),
            )
        }
    };
    let payer = connect(&payer_fake, &payer_key).await;
    let receiver = connect(&receiver_fake, &receiver_key).await;
    assert_eq!(payer.settlement_key(), payer_key.pubkey());
    let sponsor = sponsor_endpoint(Arc::clone(&receiver), payer_lag.clone()).await;
    World {
        min_grace_period_secs,
        validator,
        rpc,
        mint,
        payer,
        payer_fake,
        payer_lag,
        payer_sends,
        receiver,
        receiver_fake,
        receiver_lag,
        sponsor,
    }
}

impl World {
    fn terms(&self) -> ReceiverTerms {
        ReceiverTerms::Solana(SolanaReceiverTerms {
            sponsor: self.receiver.sponsor().to_bytes(),
            receiver: self.receiver.receiver().to_bytes(),
            mint: self.mint.to_bytes(),
            min_grace_period_secs: self.min_grace_period_secs,
            min_deposit: 1,
            sponsor_endpoint: self.sponsor.url.clone(),
        })
    }

    async fn record(&self) -> (OutboundChannelRecord, ChannelId, Pubkey) {
        let record = self
            .payer
            .prepare_open(self.terms(), DEPOSIT)
            .await
            .expect("prepare");
        let OutboundChannelRecord::Solana { channel, .. } = &record else {
            panic!("a Solana record");
        };
        let channel = channel.clone();
        let address = channel.0.parse().expect("an address");
        (record, channel, address)
    }

    /// An open channel, opened over healthy endpoints.
    async fn open(&self) -> (ChannelId, Pubkey) {
        let (record, channel, address) = self.record().await;
        self.payer.open_prepared(&record).await.expect("open");
        self.payer_lag.heal();
        self.receiver_lag.heal();
        (channel, address)
    }
}

/// The slot the `n`th-from-last `sendTransaction` to `fake` landed in, and
/// the `minContextSlot` of the first `getAccountInfo` after it.
async fn slot_and_floor_of_send(
    rpc: &RpcClient,
    fake: &FakeRpc,
    from_end: usize,
) -> (u64, Option<u64>) {
    use base64::Engine as _;
    let calls = fake.calls();
    let sends: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.method == "sendTransaction")
        .map(|(index, _)| index)
        .collect();
    let at = sends[sends.len() - 1 - from_end];
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(calls[at].params[0].as_str().expect("base64 transaction"))
        .expect("base64");
    let transaction: VersionedTransaction = bincode::deserialize(&bytes).expect("a transaction");
    let status = rpc
        .get_signature_statuses(&[transaction.signatures[0]])
        .await
        .expect("statuses")
        .value
        .remove(0)
        .expect("the transaction landed");
    let floor = calls[at + 1..]
        .iter()
        .find(|call| call.method == "getAccountInfo")
        .and_then(|call| call.params[1]["minContextSlot"].as_u64());
    (status.slot, floor)
}

#[tokio::test]
async fn a_read_after_a_confirmed_transaction_names_the_slot_it_landed_in() {
    if !require_solana_test_validator() {
        return;
    }
    let w = world().await;
    let (channel, _) = w.open().await;
    w.payer.top_up(&channel, 500).await.expect("top up");
    let voucher = w.payer.sign_voucher(&channel, 700).await.expect("voucher");
    w.receiver.land(&channel, voucher).await.expect("land");
    w.payer.start_withdrawal(&channel).await.expect("close");

    // The receiver sent the sponsored open, then settled; the payer sent
    // whatever it prefunded, then topped up, then asked to close.
    for (rpc_fake, from_end, what) in [
        (&w.receiver_fake, 1, "the sponsored open"),
        (&w.receiver_fake, 0, "settle"),
        (&w.payer_fake, 1, "top_up"),
        (&w.payer_fake, 0, "request_close"),
    ] {
        let (slot, floor) = slot_and_floor_of_send(&w.rpc, rpc_fake, from_end).await;
        assert_eq!(
            floor,
            Some(slot),
            "the read after {what} must carry the slot it landed in"
        );
    }
}

#[tokio::test]
async fn a_top_up_read_answered_minimum_context_slot_not_reached_is_repeated() {
    if !require_solana_test_validator() {
        return;
    }
    let w = world().await;
    let (channel, address) = w.open().await;
    w.payer_lag.arm(&address, vec![not_reached()], None);
    let state = w.payer.top_up(&channel, 500).await.expect("top up");
    assert_eq!(state.on_chain.collateral, DEPOSIT + 500);
    w.payer
        .sign_voucher(&channel, DEPOSIT + 500)
        .await
        .expect("a voucher the increment covers is signed");
}

#[tokio::test]
async fn a_top_up_read_answered_from_before_it_is_repeated() {
    if !require_solana_test_validator() {
        return;
    }
    let w = world().await;
    let (channel, address) = w.open().await;
    let before = snapshot(&w.validator, &address).await;
    w.payer_lag.arm(&address, vec![before], None);
    let state = w.payer.top_up(&channel, 500).await.expect("top up");
    assert_eq!(state.on_chain.collateral, DEPOSIT + 500);
    w.payer
        .sign_voucher(&channel, DEPOSIT + 500)
        .await
        .expect("a voucher the increment covers is signed");
}

#[tokio::test]
async fn a_start_withdrawal_read_answered_from_before_it_is_repeated() {
    if !require_solana_test_validator() {
        return;
    }
    let w = world().await;
    let (channel, address) = w.open().await;
    let before = snapshot(&w.validator, &address).await;
    w.payer_lag.arm(&address, vec![before], None);
    let state = w.payer.start_withdrawal(&channel).await.expect("close");
    assert_eq!(state.on_chain.status, BatchChannelStatus::Closing);
    let refused = w.payer.sign_voucher(&channel, 1).await;
    assert!(
        matches!(refused, Err(BatchSettlementError::VoucherUnbacked { .. })),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_land_read_answered_from_before_it_is_repeated() {
    if !require_solana_test_validator() {
        return;
    }
    let w = world().await;
    let (channel, address) = w.open().await;
    let before = snapshot(&w.validator, &address).await;
    let voucher = w.payer.sign_voucher(&channel, 400).await.expect("voucher");
    w.receiver_lag.arm(&address, vec![before], None);
    let state = w.receiver.land(&channel, voucher).await.expect("land");
    assert_eq!(state.landed, 400);
}

#[tokio::test]
async fn the_sponsors_own_admit_is_repeated_when_the_open_is_not_visible_yet() {
    if !require_solana_test_validator() {
        return;
    }
    let w = world().await;
    let (record, channel, address) = w.record().await;
    let before = snapshot(&w.validator, &address).await;
    w.receiver_lag.arm(&address, vec![before], None);
    w.payer
        .open_prepared(&record)
        .await
        .expect("a sponsored open");
    w.receiver
        .channel_state(&channel)
        .await
        .expect("the channel was admitted");
}

#[tokio::test]
async fn the_payers_read_after_the_sponsors_answer_is_repeated_while_no_account_shows() {
    if !require_solana_test_validator() {
        return;
    }
    let w = world().await;
    let (record, _, address) = w.record().await;
    let before = snapshot(&w.validator, &address).await;
    w.payer_lag.arm(&address, vec![before], None);
    w.payer.open_prepared(&record).await.expect("open");
}

#[tokio::test]
async fn a_failed_post_for_an_open_that_landed_keeps_the_record() {
    if !require_solana_test_validator() {
        return;
    }
    let w = world().await;
    let (record, channel, address) = w.record().await;
    let before = snapshot(&w.validator, &address).await;
    *w.sponsor.refuse_after_landing.lock().unwrap() = true;
    w.payer_lag.arm(&address, vec![before], None);
    let error = w
        .payer
        .open_prepared(&record)
        .await
        .expect_err("the post failed");
    assert!(error.to_string().contains("exists on chain"), "{error}");
    w.payer_lag.heal();
    w.payer
        .outbound_state(&channel)
        .await
        .expect("the record of a channel that holds the deposit is kept");
}

#[tokio::test]
async fn an_endpoint_that_never_reaches_the_slot_after_start_withdrawal_fails_and_backs_nothing() {
    if !require_solana_test_validator() {
        return;
    }
    let w = world().await;
    let (channel, address) = w.open().await;
    w.payer_lag.arm(&address, vec![], Some(not_reached()));
    let error = w
        .payer
        .start_withdrawal(&channel)
        .await
        .expect_err("the result could not be read");
    assert!(error.to_string().contains("confirmed"), "{error}");
    for amount in [1, DEPOSIT] {
        let refused = w.payer.sign_voucher(&channel, amount).await;
        assert!(
            matches!(refused, Err(BatchSettlementError::VoucherUnbacked { .. })),
            "{refused:?}"
        );
    }
}

#[tokio::test]
async fn an_endpoint_that_never_reaches_the_slot_after_top_up_leaves_what_the_channel_backs() {
    if !require_solana_test_validator() {
        return;
    }
    let w = world().await;
    let (channel, address) = w.open().await;
    w.payer_lag.arm(&address, vec![], Some(not_reached()));
    let error = w
        .payer
        .top_up(&channel, 500)
        .await
        .expect_err("the result could not be read");
    assert!(error.to_string().contains("confirmed"), "{error}");
    w.payer_lag.heal();
    let refused = w.payer.sign_voucher(&channel, DEPOSIT + 1).await;
    assert!(
        matches!(refused, Err(BatchSettlementError::VoucherUnbacked { .. })),
        "{refused:?}"
    );
    w.payer
        .sign_voucher(&channel, DEPOSIT)
        .await
        .expect("the figure from before the top-up");
}

#[tokio::test]
async fn a_payer_that_never_sees_the_account_keeps_the_channel_and_adopts_it_later() {
    if !require_solana_test_validator() {
        return;
    }
    let w = world().await;
    let (record, channel, address) = w.record().await;
    let before = snapshot(&w.validator, &address).await;
    w.payer_lag.arm(&address, vec![], Some(before));
    let error = w
        .payer
        .open_prepared(&record)
        .await
        .expect_err("no account showed");
    let text = error.to_string();
    assert!(
        text.contains("keeps the channel") && text.contains("opening the same record again"),
        "{text}"
    );
    w.payer_lag.heal();
    let posts = *w.sponsor.posts.lock().unwrap();
    w.payer.open_prepared(&record).await.expect("adopted");
    assert_eq!(*w.sponsor.posts.lock().unwrap(), posts, "no second post");
    w.payer.outbound_state(&channel).await.expect("recorded");
}

/// What the payer's endpoint does with the `sendTransaction`s that follow
/// the `seal` of a `finish_withdrawal`: the first send after arming is the
/// `seal` and is always forwarded.
#[derive(Clone, Copy, PartialEq)]
enum Behind {
    Healthy,
    Once,
    Always,
}

/// Which `sendTransaction`s the payer's endpoint has seen since it was armed.
#[derive(Default)]
struct Armed {
    mode: Option<Behind>,
    sends: usize,
    turned_away: bool,
}

#[derive(Clone)]
struct SendPlan(Arc<Mutex<Armed>>);

impl SendPlan {
    fn new() -> SendPlan {
        SendPlan(Arc::default())
    }

    fn arm(&self, mode: Behind) {
        *self.0.lock().unwrap() = Armed {
            mode: Some(mode),
            ..Armed::default()
        };
    }

    fn script(&self) -> impl Fn(&RpcCall) -> RpcReply + Send + Sync + 'static {
        let plan = Arc::clone(&self.0);
        move |call| {
            if call.method != "sendTransaction" {
                return RpcReply::Forward;
            }
            let mut plan = plan.lock().unwrap();
            let Some(mode) = plan.mode else {
                return RpcReply::Forward;
            };
            plan.sends += 1;
            match (plan.sends, mode) {
                (1, _) | (_, Behind::Healthy) => RpcReply::Forward,
                (_, Behind::Always) => not_reached(),
                (_, Behind::Once) if !plan.turned_away => {
                    plan.turned_away = true;
                    not_reached()
                }
                (_, Behind::Once) => RpcReply::Forward,
            }
        }
    }
}

async fn token_balance(rpc: &RpcClient, owner: &Pubkey, mint: &Pubkey) -> u64 {
    let ata = spl_associated_token_account::get_associated_token_address(owner, mint);
    match rpc.get_token_account_balance(&ata).await {
        Ok(balance) => balance.amount.parse().expect("an amount"),
        Err(_) => 0,
    }
}

fn sends_to(fake: &FakeRpc) -> Vec<RpcCall> {
    fake.calls()
        .into_iter()
        .filter(|call| call.method == "sendTransaction")
        .collect()
}

impl World {
    /// A channel whose withdrawal is pending and whose grace period (one
    /// second) has run, with the payer's balance before it is finished.
    async fn closing(&self) -> (ChannelId, u64) {
        let (channel, _) = self.open().await;
        self.payer.start_withdrawal(&channel).await.expect("close");
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        (channel, self.payer_tokens().await)
    }

    async fn payer_tokens(&self) -> u64 {
        token_balance(&self.rpc, &self.payer.settlement_key(), &self.mint).await
    }

    /// The slot a send the payer's endpoint was given landed in.
    async fn slot_of(&self, call: &RpcCall) -> u64 {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(call.params[0].as_str().expect("base64 transaction"))
            .expect("base64");
        let transaction: VersionedTransaction =
            bincode::deserialize(&bytes).expect("a transaction");
        self.rpc
            .get_signature_statuses(&[transaction.signatures[0]])
            .await
            .expect("statuses")
            .value
            .remove(0)
            .expect("the transaction landed")
            .slot
    }
}

#[tokio::test]
async fn the_distribute_after_a_confirmed_seal_names_the_slot_the_seal_landed_in() {
    if !require_solana_test_validator() {
        return;
    }
    let w = world_admitting(1).await;
    let (channel, before) = w.closing().await;
    let first = sends_to(&w.payer_fake).len();
    w.payer_sends.arm(Behind::Healthy);
    w.payer.finish_withdrawal(&channel).await.expect("finish");
    assert_eq!(w.payer_tokens().await - before, DEPOSIT as u64);

    let sends = sends_to(&w.payer_fake)[first..].to_vec();
    assert_eq!(sends.len(), 2, "a seal, then a distribute");
    let seal_slot = w.slot_of(&sends[0]).await;
    assert!(sends[0].params[1]["minContextSlot"].is_null());
    assert_eq!(sends[1].params[1]["minContextSlot"], seal_slot);
    assert_eq!(sends[1].params[1]["skipPreflight"], false);
}

#[tokio::test]
async fn a_distribute_send_answered_minimum_context_slot_not_reached_once_is_repeated() {
    if !require_solana_test_validator() {
        return;
    }
    let w = world_admitting(1).await;
    let (channel, before) = w.closing().await;
    let first = sends_to(&w.payer_fake).len();
    w.payer_sends.arm(Behind::Once);
    let state = w.payer.finish_withdrawal(&channel).await.expect("finish");
    assert_eq!(state.on_chain.status, BatchChannelStatus::Sealed);
    assert_eq!(w.payer_tokens().await - before, DEPOSIT as u64);

    let sends = sends_to(&w.payer_fake)[first..].to_vec();
    assert_eq!(
        sends.len(),
        3,
        "a seal, a distribute turned away, the same again"
    );
    assert_eq!(
        sends[1].params[0], sends[2].params[0],
        "the same signed bytes"
    );
    assert_eq!(sends[1].params[1], sends[2].params[1]);
}

#[tokio::test]
async fn a_node_that_never_reaches_the_seals_slot_fails_naming_it_and_tries_one_treasury_owner() {
    if !require_solana_test_validator() {
        return;
    }
    let w = world_admitting(1).await;
    let (channel, before) = w.closing().await;
    let first = sends_to(&w.payer_fake).len();
    w.payer_sends.arm(Behind::Always);
    let started = std::time::Instant::now();
    let error = w
        .payer
        .finish_withdrawal(&channel)
        .await
        .expect_err("the node never caught up")
        .to_string();
    assert!(started.elapsed() < std::time::Duration::from_secs(60));
    assert!(error.contains("seal confirmed"), "{error}");
    assert!(error.contains("behind"), "{error}");
    assert!(error.contains("retrying is safe"), "{error}");
    assert!(!error.contains("simulation"), "{error}");

    let sends = sends_to(&w.payer_fake)[first..].to_vec();
    assert!(sends.len() > 2, "the distribute was repeated");
    let mut owners: Vec<_> = sends[1..]
        .iter()
        .map(|call| call.params[0].clone())
        .collect();
    owners.dedup();
    assert_eq!(
        owners.len(),
        1,
        "one treasury owner, one signed transaction"
    );
    assert_eq!(w.payer_tokens().await, before, "nothing was paid out");

    // The seal landed, so a repeat over a healthy endpoint distributes the
    // channel it finds Sealed, with no slot to name.
    w.payer_sends.arm(Behind::Healthy);
    let first = sends_to(&w.payer_fake).len();
    w.payer.finish_withdrawal(&channel).await.expect("finish");
    assert_eq!(w.payer_tokens().await - before, DEPOSIT as u64);
    let sends = sends_to(&w.payer_fake)[first..].to_vec();
    assert!(sends[0].params[1]["minContextSlot"].is_null());
}

#[tokio::test]
async fn a_validator_answers_a_send_naming_a_slot_it_has_not_reached_and_does_not_land_it() {
    if !require_solana_test_validator() {
        return;
    }
    use solana_rpc_client_api::config::RpcSendTransactionConfig;
    use solana_sdk::transaction::Transaction;
    let validator = SolanaValidator::spawn().await;
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let payer = Keypair::new();
    fund(&rpc, &payer.pubkey()).await;
    let transaction = Transaction::new_signed_with_payer(
        &[solana_sdk::system_instruction::transfer(
            &payer.pubkey(),
            &Keypair::new().pubkey(),
            1,
        )],
        Some(&payer.pubkey()),
        &[&payer],
        rpc.get_latest_blockhash().await.expect("blockhash"),
    );
    let slot = rpc.get_slot().await.expect("slot");
    let error = rpc
        .send_transaction_with_config(
            &transaction,
            RpcSendTransactionConfig {
                skip_preflight: false,
                min_context_slot: Some(slot + 1_000_000),
                ..RpcSendTransactionConfig::default()
            },
        )
        .await
        .expect_err("the validator has not reached that slot");
    assert!(
        connector_chain_rpc::solana::is_min_context_slot_not_reached(&error),
        "{error}"
    );
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let status = rpc
        .get_signature_statuses(&[transaction.signatures[0]])
        .await
        .expect("statuses")
        .value
        .remove(0);
    assert!(status.is_none(), "it was never broadcast: {status:?}");
}
