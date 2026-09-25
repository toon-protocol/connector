//! ADR 0074 decision 5 on Solana, through a node built from a config file
//! (issue #1344): a client pays over an x402 `batch-settlement` channel this
//! node sponsors, then calls `request_close`. From then on only this node,
//! as `payee`, can land a voucher, and only before the grace period ends.
//! The running node's own watcher, started by `router` and nothing else,
//! finds the Closing channel, lands its latest voucher with
//! `settle_and_seal` inside the grace period, and then distributes it.
//!
//! Nothing here calls `land` or a watcher: the test pays, closes and waits,
//! as a client would.
//!
//! The voucher is the channel's whole deposit, so `distribute` makes exactly
//! one payout: two or more go out as an SPL Token `Batch` CPI, which the
//! SPL Token a local validator ships refuses (see the settlement crate's
//! `batch_watch` test). Its own test binary: `solana-test-validator` binds
//! fixed ports, so one validator per binary.

mod support;

use std::io::Write;
use std::str::FromStr;
use std::time::Duration;

use connector_domain::{Fulfill, Reject};
use connector_settlement_solana::batch::wire::{self, ChannelStatus, PAYMENT_CHANNELS_PROGRAM_ID};
use connector_settlement_solana::test_support::{
    create_mint, fund, mint_to, require_solana_test_validator, BatchPayer, SolanaValidator,
    LOCAL_TEST_PROGRAM_ID,
};
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;

use support::{paid_prepare, post_ilp, solana_voucher, spawn_recording_app};

const ROUTE: &str = "g.toon.batch";
const PRICE: u64 = 100;
const DEPOSIT: u64 = 2 * PRICE;
const ONE_DAY: u32 = 86_400;

async fn read(rpc: &RpcClient, channel: &Pubkey) -> Option<wire::ChannelAccount> {
    rpc.get_account_with_commitment(channel, CommitmentConfig::confirmed())
        .await
        .expect("read")
        .value
        .and_then(|account| wire::ChannelAccount::parse(&account.data))
}

async fn token_balance(rpc: &RpcClient, owner: &Pubkey, mint: &Pubkey) -> u64 {
    let ata = spl_associated_token_account::get_associated_token_address(owner, mint);
    match rpc.get_token_account_balance(&ata).await {
        Ok(balance) => balance.amount.parse().expect("an amount"),
        Err(_) => 0,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_close_begun_by_the_payer_is_answered_by_the_node_sealing_its_latest_voucher() {
    if !require_solana_test_validator() {
        return;
    }
    let validator = SolanaValidator::spawn().await;
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let authority = solana_sdk::signature::Keypair::new();
    fund(&rpc, &authority.pubkey()).await;
    let mint = create_mint(&rpc, &authority, 6).await;
    let sponsor_seed = [0x5b; 32];
    let sponsor = solana_sdk::signer::keypair::keypair_from_seed(&sponsor_seed).expect("a keypair");
    fund(&rpc, &sponsor.pubkey()).await;
    let program = Pubkey::from_str(PAYMENT_CHANNELS_PROGRAM_ID).expect("base58");
    let payer = BatchPayer::new(&validator.rpc_url, program).await;
    mint_to(&rpc, &authority, &mint, &payer.payer.pubkey(), 1_000_000).await;

    let (app_addr, _recorded) = spawn_recording_app().await;
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
    signer_key.write_all(&[0x07; 32]).expect("write");
    let mut settlement_key = tempfile::NamedTempFile::new().expect("settlement key");
    settlement_key.write_all(&sponsor_seed).expect("write");
    let config = support::load_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{signer_key}"

[settlement.solana]
rpc_url = "{rpc_url}"
program_id = "{LOCAL_TEST_PROGRAM_ID}"
token_address = "{mint}"
decimals = 6

[settlement.solana.key]
key_file = "{settlement_key}"

[settlement.solana.batch_settlement]
min_sponsored_deposit = 1

[[routes]]
prefix = "{ROUTE}"
handler_url = "http://{app_addr}"
price = {PRICE}
"#,
        state_dir = state_dir.path().display(),
        signer_key = signer_key.path().display(),
        settlement_key = settlement_key.path().display(),
        rpc_url = validator.rpc_url,
    ));
    let runtime = connector_cli::build(&config).await.expect("build");
    let app = connector_cli::router(&runtime, &config).expect("router");
    let receiver = runtime.signer.public_key().expect("the node's wrap key");

    // A channel this node sponsors, co-signed here as the sponsor endpoint
    // (issue #1346) will.
    let open = payer
        .admissible_open(&sponsor.pubkey(), &mint, DEPOSIT, ONE_DAY)
        .await;
    let channel = payer.open(&open, &sponsor).await.expect("open");

    // Two paid writes: the node holds a voucher for the whole deposit.
    for amount in [PRICE, 2 * PRICE] {
        let response = post_ilp(
            &app,
            &solana_voucher(&channel.to_string(), amount, &payer.sign(&channel, amount)),
            paid_prepare(ROUTE, &receiver),
        )
        .await;
        Fulfill::decode(&response)
            .unwrap_or_else(|_| panic!("paid for {amount}: {:?}", Reject::decode(&response)));
    }
    let before = read(&rpc, &channel).await.expect("the channel");
    assert_eq!((before.status, before.settled), (ChannelStatus::Open, 0));

    // The payer asks to close. Only this node can land a voucher now.
    payer.request_close(&channel).await.expect("request_close");
    let closing = read(&rpc, &channel).await.expect("the channel");
    assert_eq!(closing.status, ChannelStatus::Closing);
    let deadline = closing.closure_started_at + i64::from(closing.grace_period);

    // The node answers on its own.
    let mut sealed = None;
    for _ in 0..600 {
        match read(&rpc, &channel).await {
            Some(account) if account.status != ChannelStatus::Closing => {
                sealed = Some(account);
                break;
            }
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    let sealed = sealed.expect("the running node sealed the Closing channel");
    assert_eq!(
        sealed.settled, DEPOSIT,
        "sealed with the latest voucher landed"
    );
    let now = rpc
        .get_block_time(rpc.get_slot().await.expect("slot"))
        .await
        .expect("block time");
    assert!(now < deadline, "inside the grace period");

    // And pays it out to this node on a later pass.
    let mut paid = 0;
    for _ in 0..600 {
        paid = token_balance(&rpc, &sponsor.pubkey(), &mint).await;
        if paid == DEPOSIT {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        paid, DEPOSIT,
        "distributed to this node's receiving account"
    );
    assert_eq!(
        read(&rpc, &channel).await.map(|account| account.status),
        Some(ChannelStatus::Distributed),
        "left for reclaim once its slot window passes"
    );
}
