//! `XrplBatchSettlement` against a real stand-alone `xrpld`, held to the
//! batch-settlement port's contract suite unmodified (ADR 0007, ADR 0087),
//! plus what the suite cannot ask of every chain: the XRPL-only admission
//! rules, `tfRenew`, a claim that answers `tesSUCCESS` and delivers nothing,
//! a deleted channel and the drop ceiling (issue #1612).
//!
//! Everything the node does not do itself, the test does as the client would:
//! `PaymentChannelCreate`, `PaymentChannelFund`, the source's `tfClose` and
//! `tfRenew`, and signing vouchers with the payer's key. The paying half is a
//! later ticket, so each is a raw XRPL transaction.

use std::sync::Arc;
use std::time::Duration;

use connector_chain_rpc::xrpl::{TxFate, TxWait, XrplRpc};
use connector_settlement::batch::contract::{
    assert_upholds_the_contract, BatchContractFixture, BoxFuture, ChannelTerms, OpenedChannel,
};
use connector_settlement::batch::{
    AdmissionRefusal, BatchChannelStatus, BatchSettlementBackend, BatchSettlementError,
    ChannelPresentation, Voucher, VoucherSigner,
};
use connector_settlement::ChannelId;
use connector_settlement_xrpl::test_support::{
    require_xrpld, LedgerCloser, Wallet, Xrpld, STANDALONE_NETWORK_ID,
};
use connector_settlement_xrpl::{RpcTransport, XrplBatchSettlement};
use connector_signer::{xrpl_family_seed, XrplAlgorithm, XrplSigner, XRPL_MAX_DROPS};
use serde_json::{json, Value};

/// Above the suite's 901-second floor, and short against nothing: the
/// chain's clock only moves when it is closed.
const MINIMUM_DELAY: u64 = 1_000;
const TF_CLOSE: u64 = 0x0002_0000;
const TF_RENEW: u64 = 0x0001_0000;
const WAIT: TxWait = TxWait {
    poll: Duration::from_millis(50),
    give_up_after: Duration::from_secs(30),
};

/// A chain, the node's backend and a payer who owns every channel.
struct World {
    chain: Xrpld,
    rpc: XrplRpc,
    payer: Wallet,
    payer_signer: XrplSigner,
    /// Someone who is not this node.
    stranger: Wallet,
    node: Wallet,
    backend: XrplBatchSettlement,
    _closer: LedgerCloser,
}

async fn world(min_settle_delay_secs: u64) -> World {
    let chain = Xrpld::spawn().await;
    let rpc = chain.rpc();
    let payer = chain.funded_wallet(&rpc, 100_000_000_000).await;
    let stranger = chain.funded_wallet(&rpc, 100_000_000).await;
    let node = chain.funded_wallet(&rpc, 100_000_000).await;
    let payer_signer = XrplSigner::from_family_seed(&payer.seed).expect("a family seed");
    let node_signer = XrplSigner::from_family_seed(&node.seed).expect("a family seed");
    assert_eq!(node_signer.classic_address(), node.address);

    // A backend waits for its transactions to validate, as it would on a
    // network, and nothing closes a stand-alone ledger but a call.
    let closer = chain.close_ledgers_every(Duration::from_millis(200));
    let backend = XrplBatchSettlement::connect(
        RpcTransport::direct(&chain.rpc_url).expect("rpc transport"),
        STANDALONE_NETWORK_ID,
        node_signer,
        min_settle_delay_secs,
    )
    .await
    .expect("connect to the stand-alone chain")
    .with_wait(WAIT);
    assert_eq!(backend.address(), node.address);

    World {
        chain,
        rpc,
        payer,
        payer_signer,
        stranger,
        node,
        backend,
        _closer: closer,
    }
}

impl World {
    /// Submit `tx_json` signed by `seed`, wait for it to validate and return
    /// its metadata. The result must be `tesSUCCESS`.
    async fn send(&self, tx_json: Value, seed: &str) -> Value {
        let last = self.rpc.fee().await.expect("fee").ledger_current_index + 40;
        let mut tx_json = tx_json;
        tx_json["LastLedgerSequence"] = json!(last);
        let submitted = self
            .rpc
            .sign_and_submit(tx_json, seed)
            .await
            .expect("submit");
        assert_eq!(submitted.engine_result, "tesSUCCESS", "{submitted:?}");
        match self
            .rpc
            .wait_for_tx(&submitted.hash, last, WAIT)
            .await
            .expect("wait")
        {
            TxFate::Landed {
                transaction_result,
                meta,
                ..
            } => {
                assert_eq!(transaction_result, "tesSUCCESS", "{meta}");
                meta
            }
            other => panic!("the transaction did not land: {other:?}"),
        }
    }

    /// `PaymentChannelCreate` from the payer; the channel's id, in the
    /// port's spelling.
    async fn create(
        &self,
        destination: &str,
        drops: u64,
        settle_delay: u64,
        extra: Value,
    ) -> ChannelId {
        let mut tx = json!({
            "TransactionType": "PaymentChannelCreate",
            "Account": self.payer.address,
            "Destination": destination,
            "Amount": drops.to_string(),
            "SettleDelay": settle_delay,
            "PublicKey": hex::encode_upper(self.payer_signer.public_key()),
        });
        for (name, value) in extra.as_object().expect("extra fields") {
            tx[name] = value.clone();
        }
        let meta = self.send(tx, &self.payer.seed).await;
        let index = meta["AffectedNodes"]
            .as_array()
            .expect("affected nodes")
            .iter()
            .find_map(|node| {
                let created = &node["CreatedNode"];
                (created["LedgerEntryType"] == "PayChannel")
                    .then(|| created["LedgerIndex"].as_str().expect("index").to_string())
            })
            .expect("the transaction created a channel");
        ChannelId(index.to_ascii_lowercase())
    }

    /// A channel this node would admit: paid to it, at least the minimum
    /// delay, no `Expiration` or `CancelAfter`.
    async fn admissible(&self, drops: u64) -> OpenedChannel {
        let channel = self
            .create(&self.node.address, drops, MINIMUM_DELAY, json!({}))
            .await;
        opened(channel, &self.payer_signer)
    }

    async fn fund(&self, channel: &ChannelId, drops: u64) {
        self.send(
            json!({
                "TransactionType": "PaymentChannelFund",
                "Account": self.payer.address,
                "Channel": channel.0,
                "Amount": drops.to_string(),
            }),
            &self.payer.seed,
        )
        .await;
    }

    /// The payer's `PaymentChannelClaim` with `flags`: `tfClose` begins its
    /// exit (or ends the channel when nothing is left unlanded), `tfRenew`
    /// takes the exit back.
    async fn payer_claims(&self, channel: &ChannelId, flags: u64) {
        self.send(
            json!({
                "TransactionType": "PaymentChannelClaim",
                "Account": self.payer.address,
                "Channel": channel.0,
                "Flags": flags,
            }),
            &self.payer.seed,
        )
        .await;
    }

    fn voucher(&self, channel: &ChannelId, drops: u128) -> Voucher {
        Voucher {
            cumulative_amount: drops,
            signature: sign(&self.payer_signer, channel, drops),
        }
    }
}

fn id_bytes(channel: &ChannelId) -> [u8; 32] {
    hex::decode(&channel.0)
        .expect("hex")
        .try_into()
        .expect("32 bytes")
}

fn sign(signer: &XrplSigner, channel: &ChannelId, drops: u128) -> Vec<u8> {
    signer
        .sign_claim(&id_bytes(channel), drops)
        .expect("a claim under the ceiling")
}

fn presentation(channel: &ChannelId) -> ChannelPresentation {
    ChannelPresentation::Xrpl {
        channel: channel.clone(),
    }
}

fn opened(channel: ChannelId, signer: &XrplSigner) -> OpenedChannel {
    OpenedChannel {
        presentation: presentation(&channel),
        voucher_signer: VoucherSigner::Xrpl(signer.public_key()),
    }
}

fn refusal(result: Result<impl std::fmt::Debug, BatchSettlementError>) -> AdmissionRefusal {
    match result {
        Err(BatchSettlementError::NotAdmissible { refusal, .. }) => refusal,
        other => panic!("expected an admission refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn xrpl_batch_settlement_upholds_the_contract() {
    if !require_xrpld() {
        return;
    }
    let world = Arc::new(world(MINIMUM_DELAY).await);

    let open = {
        let world = Arc::clone(&world);
        Box::new(move |terms: ChannelTerms| {
            let world = Arc::clone(&world);
            let boxed: BoxFuture<'static, OpenedChannel> = Box::pin(async move {
                assert!(
                    terms.in_settled_token,
                    "a channel holds XRP only: the fixture declares it holds no other token"
                );
                let destination = if terms.pays_this_node {
                    &world.node.address
                } else {
                    &world.stranger.address
                };
                let channel = world
                    .create(
                        destination,
                        u64::try_from(terms.deposit).expect("the suite's deposits fit"),
                        terms.delay_secs,
                        json!({}),
                    )
                    .await;
                opened(channel, &world.payer_signer)
            });
            boxed
        })
    };
    let deposit = {
        let world = Arc::clone(&world);
        Box::new(move |channel: &ChannelId, amount: u128| {
            let world = Arc::clone(&world);
            let channel = channel.clone();
            let boxed: BoxFuture<'static, ()> = Box::pin(async move {
                world
                    .fund(&channel, u64::try_from(amount).expect("fits"))
                    .await;
            });
            boxed
        })
    };
    let begin_exit = {
        let world = Arc::clone(&world);
        Box::new(move |channel: &ChannelId| {
            let world = Arc::clone(&world);
            let channel = channel.clone();
            let boxed: BoxFuture<'static, ()> = Box::pin(async move {
                world.payer_claims(&channel, TF_CLOSE).await;
            });
            boxed
        })
    };
    let sign_voucher = {
        let signer = XrplSigner::from_family_seed(&world.payer.seed).expect("seed");
        Box::new(move |channel: &ChannelId, amount: u128| sign(&signer, channel, amount))
    };
    let fixture = BatchContractFixture {
        backend: Arc::new(XrplBackendHandle(Arc::clone(&world))),
        minimum_delay_secs: MINIMUM_DELAY,
        open,
        deposit,
        begin_exit,
        sign: sign_voucher,
        unopened: presentation(&ChannelId("c4".repeat(32))),
        holds_other_tokens: false,
    };

    assert_upholds_the_contract(|| async move { fixture }).await;
}

/// Lets the suite hold the backend through the `Arc<World>` the fixture's
/// closures also hold.
struct XrplBackendHandle(Arc<World>);

#[async_trait::async_trait]
impl BatchSettlementBackend for XrplBackendHandle {
    async fn admit(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<connector_settlement::batch::BatchChannelState, BatchSettlementError> {
        self.0.backend.admit(presentation).await
    }
    async fn restore(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<connector_settlement::batch::BatchChannelState, BatchSettlementError> {
        self.0.backend.restore(presentation).await
    }
    async fn channel_state(
        &self,
        channel: &ChannelId,
    ) -> Result<connector_settlement::batch::BatchChannelState, BatchSettlementError> {
        self.0.backend.channel_state(channel).await
    }
    async fn land(
        &self,
        channel: &ChannelId,
        voucher: Voucher,
    ) -> Result<connector_settlement::batch::Landed, BatchSettlementError> {
        self.0.backend.land(channel, voucher).await
    }
}

/// Each rule ADR 0087 decision 9 fixes refuses by its own name, on real
/// channels: a `CancelAfter` of any value (the new variant), a `Destination`
/// that is not this node, a `SettleDelay` below the minimum, and an
/// `Expiration` already set.
#[tokio::test]
async fn each_admission_rule_refuses_by_name() {
    if !require_xrpld() {
        return;
    }
    let world = world(MINIMUM_DELAY).await;
    let node = world.node.address.clone();

    // A `CancelAfter`, far off as it is, is still an immutable deadline.
    let close_time = world
        .rpc
        .call("ledger", json!({ "ledger_index": "validated" }))
        .await
        .expect("ledger")["ledger"]["close_time"]
        .as_u64()
        .expect("close_time");
    let cancelable = world
        .create(
            &node,
            1_000,
            MINIMUM_DELAY,
            json!({ "CancelAfter": close_time + 100_000_000 }),
        )
        .await;
    assert_eq!(
        refusal(world.backend.admit(presentation(&cancelable)).await),
        AdmissionRefusal::CancelAfter
    );

    let elsewhere = world
        .create(&world.stranger.address, 1_000, MINIMUM_DELAY, json!({}))
        .await;
    assert_eq!(
        refusal(world.backend.admit(presentation(&elsewhere)).await),
        AdmissionRefusal::NotPayableToThisNode {
            field: "Destination"
        }
    );

    let short = world
        .create(&node, 1_000, MINIMUM_DELAY - 1, json!({}))
        .await;
    assert_eq!(
        refusal(world.backend.admit(presentation(&short)).await),
        AdmissionRefusal::DelayBelowMinimum {
            delay_secs: MINIMUM_DELAY - 1,
            minimum_secs: MINIMUM_DELAY,
        }
    );

    let leaving = world.create(&node, 1_000, MINIMUM_DELAY, json!({})).await;
    world.payer_claims(&leaving, TF_CLOSE).await;
    assert_eq!(
        refusal(world.backend.admit(presentation(&leaving)).await),
        AdmissionRefusal::NotOpen,
        "an Expiration already set means the payer has begun to leave"
    );

    // None of them was admitted by having been looked at.
    assert_eq!(
        world.backend.channel_state(&cancelable).await.unwrap_err(),
        BatchSettlementError::ChannelNotAdmitted(cancelable.clone())
    );

    // A channel this node already holds a voucher on is restored without
    // judging any of that, and still lands.
    let state = world
        .backend
        .restore(presentation(&cancelable))
        .await
        .expect("restore judges no admission rule");
    assert_eq!(state.status, BatchChannelStatus::Open);
    let landed = world
        .backend
        .land(&cancelable, world.voucher(&cancelable, 100))
        .await
        .expect("a voucher held on a restored CancelAfter channel lands");
    assert_eq!(landed.state.landed, 100);
    assert_eq!(landed.transaction.len(), 64);

    // And something that is not a channel at all is not found.
    assert!(matches!(
        world
            .backend
            .admit(ChannelPresentation::Xrpl {
                channel: ChannelId("not hex".to_string()),
            })
            .await,
        Err(BatchSettlementError::ChannelNotFound(_))
    ));
}

/// A payer's exit is a delay it can take back: `tfRenew` clears
/// `Expiration`, and the channel reads `Open` again and accepts vouchers
/// (ADR 0087 decision 10).
#[tokio::test]
async fn a_renewed_channel_reads_open_again_and_accepts_vouchers() {
    if !require_xrpld() {
        return;
    }
    let world = world(MINIMUM_DELAY).await;
    let channel = world.admissible(1_000).await.presentation.channel().clone();
    world
        .backend
        .admit(presentation(&channel))
        .await
        .expect("admit");

    world.payer_claims(&channel, TF_CLOSE).await;
    let leaving = world.backend.channel_state(&channel).await.expect("state");
    assert_eq!(leaving.status, BatchChannelStatus::Closing);
    assert_eq!(leaving.collateral, 0);

    world.payer_claims(&channel, TF_RENEW).await;
    let renewed = world.backend.channel_state(&channel).await.expect("state");
    assert_eq!(renewed.status, BatchChannelStatus::Open);
    assert_eq!(renewed.collateral, 1_000);
    assert_eq!(
        refusal_free(world.backend.admit(presentation(&channel)).await),
        renewed,
        "a renewed channel is admissible again"
    );

    let landed = world
        .backend
        .land(&channel, world.voucher(&channel, 400))
        .await
        .expect("a renewed channel accepts a voucher");
    assert_eq!(landed.state.landed, 400);
    assert_eq!(landed.state.collateral, 600);
    assert_eq!(landed.state.status, BatchChannelStatus::Open);
}

fn refusal_free<T: std::fmt::Debug>(result: Result<T, BatchSettlementError>) -> T {
    result.unwrap_or_else(|e| panic!("expected success, got {e:?}"))
}

/// A claim on an expired channel answers `tesSUCCESS`, deletes the channel
/// and delivers nothing: that is a failure, not a landing, and the channel
/// then reads `ChannelNotFound` with no copy kept.
#[tokio::test]
async fn a_land_on_an_expired_channel_that_delivers_nothing_is_a_failure() {
    if !require_xrpld() {
        return;
    }
    // A short delay, so the stand-alone clock can run it out.
    const SHORT: u64 = 5;
    let world = world(SHORT).await;
    let channel = world
        .create(&world.node.address, 1_000, SHORT, json!({}))
        .await;
    world
        .backend
        .admit(presentation(&channel))
        .await
        .expect("admit");

    world.payer_claims(&channel, TF_CLOSE).await;
    world.chain.let_close_time_pass(&world.rpc, SHORT + 2).await;

    // Still there, until something touches it.
    assert_eq!(
        world
            .backend
            .channel_state(&channel)
            .await
            .expect("the expired channel is not yet deleted")
            .status,
        BatchChannelStatus::Closing
    );
    let err = world
        .backend
        .land(&channel, world.voucher(&channel, 300))
        .await
        .unwrap_err();
    let BatchSettlementError::Backend(message) = &err else {
        panic!("expected a backend failure, got {err:?}");
    };
    assert!(
        message.contains("delivered nothing"),
        "tesSUCCESS alone is not a landing: {message}"
    );

    // The claim deleted the channel and refunded the payer.
    assert_eq!(
        world.backend.channel_state(&channel).await.unwrap_err(),
        BatchSettlementError::ChannelNotFound(channel.clone())
    );
}

/// A channel that ends is deleted by the ledger and reads `ChannelNotFound`.
#[tokio::test]
async fn a_deleted_channel_answers_channel_not_found() {
    if !require_xrpld() {
        return;
    }
    let world = world(MINIMUM_DELAY).await;
    let channel = world.admissible(1_000).await.presentation.channel().clone();
    world
        .backend
        .admit(presentation(&channel))
        .await
        .expect("admit");
    // Everything landed: the payer's `tfClose` has nothing to wait for and
    // ends the channel at once.
    let landed = world
        .backend
        .land(&channel, world.voucher(&channel, 1_000))
        .await
        .expect("land the whole deposit");
    assert_eq!(landed.state.collateral, 0);
    assert_eq!(landed.state.status, BatchChannelStatus::Open);
    world.payer_claims(&channel, TF_CLOSE).await;

    assert_eq!(
        world.backend.channel_state(&channel).await.unwrap_err(),
        BatchSettlementError::ChannelNotFound(channel.clone())
    );
    assert_eq!(
        world
            .backend
            .restore(presentation(&channel))
            .await
            .unwrap_err(),
        BatchSettlementError::ChannelNotFound(channel.clone())
    );
}

/// A voucher above the drop ceiling is `AmountExceedsChain` before anything
/// is read or sent, whatever the integer width, and one at the ceiling is
/// not (it is merely unbacked).
#[tokio::test]
async fn an_over_ceiling_voucher_is_amount_exceeds_chain() {
    if !require_xrpld() {
        return;
    }
    let world = world(MINIMUM_DELAY).await;
    let channel = world.admissible(1_000).await.presentation.channel().clone();
    world
        .backend
        .admit(presentation(&channel))
        .await
        .expect("admit");

    for amount in [
        u128::from(XRPL_MAX_DROPS) + 1,
        u128::from(u64::MAX) + 1,
        u128::MAX,
    ] {
        let err = world
            .backend
            .land(
                &channel,
                Voucher {
                    cumulative_amount: amount,
                    // No claim can be signed above the ceiling.
                    signature: vec![0x30; 70],
                },
            )
            .await
            .unwrap_err();
        assert_eq!(
            err,
            BatchSettlementError::AmountExceedsChain {
                amount,
                max: XRPL_MAX_DROPS,
            }
        );
    }
    assert_eq!(
        world
            .backend
            .land(
                &channel,
                world.voucher(&channel, u128::from(XRPL_MAX_DROPS))
            )
            .await
            .unwrap_err(),
        BatchSettlementError::VoucherExceedsDeposit {
            amount: u128::from(XRPL_MAX_DROPS),
            deposited: 1_000,
        },
        "at the ceiling is a width the chain can name"
    );
}

/// A voucher the channel's key did not sign is refused before anything is
/// sent, so it costs the node no fee.
#[tokio::test]
async fn a_voucher_not_signed_by_the_channel_key_costs_nothing() {
    if !require_xrpld() {
        return;
    }
    let world = world(MINIMUM_DELAY).await;
    let channel = world.admissible(1_000).await.presentation.channel().clone();
    world
        .backend
        .admit(presentation(&channel))
        .await
        .expect("admit");
    let impostor = XrplSigner::from_family_seed(&world.stranger.seed).expect("seed");
    let before = world
        .rpc
        .account_info(&world.node.address)
        .await
        .unwrap()
        .unwrap();

    let err = world
        .backend
        .land(
            &channel,
            Voucher {
                cumulative_amount: 100,
                signature: sign(&impostor, &channel, 100),
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, BatchSettlementError::InvalidVoucherSignature(_)),
        "{err:?}"
    );
    let after = world
        .rpc
        .account_info(&world.node.address)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after, before, "nothing was sent: no fee, no sequence used");
}

/// A presentation for another chain is not this backend's.
#[tokio::test]
async fn a_presentation_for_another_chain_is_the_wrong_chain() {
    if !require_xrpld() {
        return;
    }
    let world = world(MINIMUM_DELAY).await;
    let err = world
        .backend
        .admit(ChannelPresentation::Solana {
            channel: ChannelId("11111111111111111111111111111111".to_string()),
        })
        .await
        .unwrap_err();
    assert_eq!(
        err,
        BatchSettlementError::WrongChain {
            presented: "solana",
            backend: "xrpl",
        }
    );
}

/// Boot refuses, by name, a table whose key names another network than the
/// endpoint serves (ADR 0087 decision 5) and an account the ledger lacks.
#[tokio::test]
async fn connect_refuses_a_wrong_network_and_a_missing_account() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let rpc = chain.rpc();
    let node = chain.funded_wallet(&rpc, 100_000_000).await;
    let signer = || XrplSigner::from_family_seed(&node.seed).expect("seed");
    let transport = || RpcTransport::direct(&chain.rpc_url).expect("rpc transport");

    let err = XrplBatchSettlement::connect(transport(), 1, signer(), MINIMUM_DELAY)
        .await
        .err()
        .expect("a wrong network id is refused");
    let BatchSettlementError::Backend(message) = err else {
        panic!("expected a backend refusal");
    };
    assert!(
        message.contains("network 1000") && message.contains("network 1"),
        "{message}"
    );

    let (_, seed) = rpc.wallet_propose().await.expect("wallet_propose");
    let unfunded = XrplSigner::from_family_seed(&seed).expect("seed");
    let address = unfunded.classic_address();
    let err =
        XrplBatchSettlement::connect(transport(), STANDALONE_NETWORK_ID, unfunded, MINIMUM_DELAY)
            .await
            .err()
            .expect("a missing account is refused");
    let BatchSettlementError::Backend(message) = err else {
        panic!("expected a backend refusal");
    };
    assert!(message.contains(&address), "{message}");
}

/// Ed25519 keys work end to end: the node's account lands with an
/// Ed25519-signed transaction, and the channel's `PublicKey` is an
/// `0xED`-prefixed Ed25519 key whose raw-message signature the ledger
/// verifies as well as the backend does.
#[tokio::test]
async fn an_ed25519_node_lands_an_ed25519_payers_voucher() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let rpc = chain.rpc();
    // `wallet_propose` writes an Ed25519 account's seed without its `sEd`
    // prefix, so the seeds are built here, from entropy the chain never sees.
    let (chain_ref, rpc_ref) = (&chain, &rpc);
    let ed25519 = |entropy: u8| async move {
        let (chain, rpc) = (chain_ref, rpc_ref);
        let seed = xrpl_family_seed(XrplAlgorithm::Ed25519, &[entropy; 16]);
        assert!(seed.starts_with("sEd"), "{seed}");
        let address = XrplSigner::from_family_seed(&seed)
            .expect("seed")
            .classic_address();
        chain.fund(rpc, &address, 100_000_000_000).await;
        Wallet { address, seed }
    };
    let (node, payer) = (ed25519(0xA1).await, ed25519(0xB2).await);
    let node_signer = XrplSigner::from_family_seed(&node.seed).expect("seed");
    let payer_signer = XrplSigner::from_family_seed(&payer.seed).expect("seed");
    assert_eq!(node_signer.classic_address(), node.address);
    assert_eq!(payer_signer.public_key()[0], 0xED);

    let _closer = chain.close_ledgers_every(Duration::from_millis(200));
    let backend = XrplBatchSettlement::connect(
        RpcTransport::direct(&chain.rpc_url).expect("rpc transport"),
        STANDALONE_NETWORK_ID,
        node_signer,
        MINIMUM_DELAY,
    )
    .await
    .expect("connect")
    .with_wait(WAIT);

    let submitted = rpc
        .sign_and_submit(
            json!({
                "TransactionType": "PaymentChannelCreate",
                "Account": payer.address,
                "Destination": node.address,
                "Amount": "1000",
                "SettleDelay": MINIMUM_DELAY,
                "PublicKey": hex::encode_upper(payer_signer.public_key()),
            }),
            &payer.seed,
        )
        .await
        .expect("submit");
    assert_eq!(submitted.engine_result, "tesSUCCESS", "{submitted:?}");
    let TxFate::Landed { meta, .. } = rpc
        .wait_for_tx(&submitted.hash, u32::MAX, WAIT)
        .await
        .expect("wait")
    else {
        panic!("the channel was not created");
    };
    let index = meta["AffectedNodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .find_map(|node| {
            let created = &node["CreatedNode"];
            (created["LedgerEntryType"] == "PayChannel")
                .then(|| created["LedgerIndex"].as_str())
                .flatten()
        })
        .expect("a created channel");
    let channel = ChannelId(index.to_ascii_lowercase());

    let state = backend
        .admit(presentation(&channel))
        .await
        .expect("an Ed25519-keyed channel is admitted");
    assert_eq!(
        state.voucher_signer,
        VoucherSigner::Xrpl(payer_signer.public_key())
    );
    let landed = backend
        .land(
            &channel,
            Voucher {
                cumulative_amount: 250,
                signature: sign(&payer_signer, &channel, 250),
            },
        )
        .await
        .expect("an Ed25519 voucher lands under an Ed25519-signed claim");
    assert_eq!(landed.state.landed, 250);
    assert_eq!(landed.state.collateral, 750);
}
