//! ADR 0074 on Solana, end to end through a node built from a config file:
//! a client opens an x402 `batch-settlement` channel on solana-foundation's
//! `payment-channels` (the mainnet-beta binary, in a disposable validator's
//! genesis at its canonical id), sponsored by this node's settlement key,
//! pays for a write with a voucher through the client edge's real claim
//! gate, and the node -- restarted from its config and its `state_dir` --
//! lands that voucher on chain through the receive-only port.
//!
//! Its own test binary: `solana-test-validator` binds fixed ports, so one
//! validator per binary.

mod support;

use std::io::Write;
use std::str::FromStr;
use std::sync::Arc;

use connector_domain::{Fulfill, JournalEntry, Reject};
use connector_runtime::{FileJournal, Journal};
use connector_settlement::batch::{BatchChannelStatus, BatchSettlementBackend, Voucher};
use connector_settlement::ChannelId;
use connector_settlement_solana::batch::wire::PAYMENT_CHANNELS_PROGRAM_ID;
use connector_settlement_solana::test_support::{
    create_mint, fund, mint_to, require_solana_test_validator, BatchPayer, SolanaValidator,
    LOCAL_TEST_PROGRAM_ID,
};
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;

use support::{paid_prepare, post_ilp, solana_voucher, spawn_recording_app, CLIENT_EDGE_JOURNAL};

const ROUTE: &str = "g.toon.batch";
const PRICE: u64 = 100;
const DEPOSIT: u64 = 1_000;
const ONE_DAY: u32 = 86_400;

/// An EVM voucher on the wire, for a node that has not opted in on EVM to
/// refuse.
fn unaccepted_evm_voucher() -> String {
    serde_json::json!({
        "version": "1.0",
        "blockchain": "evm",
        "scheme": "batch-settlement",
        "messageId": "voucher-evm",
        "timestamp": "2026-09-25T12:00:00.000Z",
        "senderId": "x402-client",
        "channelId": format!("0x{}", "ab".repeat(32)),
        "maxClaimableAmount": "100",
        "signature": format!("0x{}", "01".repeat(65)),
    })
    .to_string()
}

#[tokio::test]
async fn a_solana_voucher_is_accepted_journaled_and_landed_after_a_restart() {
    if !require_solana_test_validator() {
        return;
    }

    let validator = SolanaValidator::spawn().await;
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let authority = solana_sdk::signature::Keypair::new();
    fund(&rpc, &authority.pubkey()).await;
    let mint = create_mint(&rpc, &authority, 6).await;

    // The node's `[settlement.solana]` key: its settlement identity, and the
    // sponsor every channel it admits names as payee and rent payer.
    let sponsor_seed = [0x5a; 32];
    let sponsor = solana_sdk::signer::keypair::keypair_from_seed(&sponsor_seed).expect("a keypair");
    fund(&rpc, &sponsor.pubkey()).await;

    let program = Pubkey::from_str(PAYMENT_CHANNELS_PROGRAM_ID).expect("base58");
    let payer = BatchPayer::new(&validator.rpc_url, program).await;
    mint_to(&rpc, &authority, &mint, &payer.payer.pubkey(), 1_000_000).await;

    let (app_addr, recorded) = spawn_recording_app().await;
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
    assert!(
        runtime.batch_settlement_solana.is_some(),
        "opted in on Solana"
    );
    assert!(runtime.batch_settlement_evm.is_none(), "and not on EVM");
    let app = connector_cli::router(&runtime, &config).expect("router");
    let receiver = runtime.signer.public_key().expect("the node's wrap key");

    // The client opens a channel this node sponsors. Co-signing an `open`
    // is the public sponsor endpoint's job (issue #1346); here the test
    // holds the sponsor key and signs as it will.
    let open = payer
        .admissible_open(&sponsor.pubkey(), &mint, DEPOSIT, ONE_DAY)
        .await;
    let channel = payer.open(&open, &sponsor).await.expect("open");
    let channel_id = ChannelId(channel.to_string());
    let key = format!("solana:{channel}");

    let response = post_ilp(
        &app,
        &solana_voucher(&channel.to_string(), PRICE, &payer.sign(&channel, PRICE)),
        paid_prepare(ROUTE, &receiver),
    )
    .await;
    Fulfill::decode(&response).unwrap_or_else(|_| {
        panic!(
            "the voucher paid for the write: {:?}",
            Reject::decode(&response)
        )
    });
    assert_eq!(
        recorded.lock().unwrap().len(),
        1,
        "the write reached the app"
    );

    let entries = FileJournal::open(state_dir.path().join(CLIENT_EDGE_JOURNAL))
        .expect("the client-edge journal")
        .read_all()
        .expect("readable");
    assert!(entries.iter().any(|entry| matches!(
        entry,
        JournalEntry::BatchChannelAdmitted { channel_id, .. } if *channel_id == key
    )));
    let (amount, signature) = entries
        .iter()
        .rev()
        .find_map(|entry| match entry {
            JournalEntry::InboundClaimAccepted {
                channel_id,
                cumulative_amount,
                signature,
                ..
            } if *channel_id == key => Some((*cumulative_amount, signature.clone())),
            _ => None,
        })
        .expect("the voucher is journaled");
    assert_eq!(amount, PRICE);

    // The node on EVM has not opted in, and says so.
    let refused = Reject::decode(
        &post_ilp(
            &app,
            &unaccepted_evm_voucher(),
            paid_prepare(ROUTE, &receiver),
        )
        .await,
    )
    .expect("a voucher on a chain this node has not opted in on is refused");
    assert!(
        refused.message.contains("batch-settlement"),
        "refused by name: {refused:?}"
    );

    // Restart, and land the journaled voucher through the port.
    drop(app);
    drop(runtime);
    let runtime = connector_cli::build(&config).await.expect("rebuild");
    let backend = runtime
        .batch_settlement_solana
        .clone()
        .expect("still opted in") as Arc<dyn BatchSettlementBackend>;
    let before = backend
        .channel_state(&channel_id)
        .await
        .expect("re-admitted at boot, from the journal");
    assert_eq!(before.status, BatchChannelStatus::Open);
    assert_eq!(before.landed, 0);

    let landed = backend
        .land(
            &channel_id,
            Voucher {
                cumulative_amount: u128::from(amount),
                signature,
            },
        )
        .await
        .expect("the journaled voucher lands");
    assert_eq!(landed.landed, u128::from(PRICE));
    assert_eq!(landed.status, BatchChannelStatus::Open);
}
