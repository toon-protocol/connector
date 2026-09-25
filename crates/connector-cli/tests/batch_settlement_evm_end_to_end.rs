//! ADR 0074 on EVM, end to end through a node built from a config file: a
//! client opens an x402 `batch-settlement` channel on the real
//! `x402BatchSettlement` (Base Sepolia's bytecode at its canonical address on
//! a disposable `anvil`), pays for a write with vouchers through the client
//! edge's real claim gate, and the node -- restarted from nothing but its
//! config and its `state_dir` -- lands the latest voucher on chain through
//! the receive-only port.
//!
//! Nothing here builds a backend and hands it to the gate: the
//! `[settlement.evm.batch_settlement]` table is the only opt-in this test
//! writes, and `connector_cli::build` / `router` are what turn it into a
//! gate that accepts vouchers and a port that lands them.

mod support;

use std::io::Write;

use connector_domain::{Fulfill, JournalEntry, Reject};
use connector_runtime::{FileJournal, Journal};
use connector_settlement::batch::{
    BatchChannelStatus, BatchSettlementBackend, EvmChannelConfig, Voucher,
};
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};
use connector_settlement_evm::EvmSettlementBackend;
use ethers::signers::{LocalWallet, Signer};

use support::{evm_voucher, paid_prepare, post_ilp, spawn_recording_app, CLIENT_EDGE_JOURNAL};

/// This binary's own base port for [`Anvil::spawn`], clear of every other
/// anvil binary's range.
const ANVIL_BASE_PORT: u16 = 22_300;

const ROUTE: &str = "g.toon.batch";
const PRICE: u64 = 100;
const DEPOSIT: u128 = 1_000;
const ONE_DAY: u64 = 86_400;

/// A Solana voucher on the wire, for a node that has not opted in on
/// Solana to refuse.
fn unaccepted_solana_voucher() -> String {
    serde_json::json!({
        "version": "1.0",
        "blockchain": "solana",
        "scheme": "batch-settlement",
        "messageId": "voucher-solana",
        "timestamp": "2026-09-25T12:00:00Z",
        "senderId": "x402-client",
        "channelId": bs58::encode([0xc3; 32]).into_string(),
        "maxClaimableAmount": "100",
        "expiresAt": 0,
        "signature": bs58::encode([0x01; 64]).into_string(),
    })
    .to_string()
}

#[tokio::test]
async fn an_evm_voucher_is_accepted_journaled_and_landed_after_a_restart() {
    if !require_anvil() {
        return;
    }

    // The chain: x402's contracts, an ERC-3009 USDC, and a TokenNetwork
    // registry over it for the node's `[settlement.evm]` table.
    let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
    let mut x402 = X402Chain::place(&anvil.rpc_url).await;
    let token = x402.deploy_fiat_token().await;
    let settlement = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("a TokenNetwork over the FiatToken");
    let registry = settlement.registry_address();
    let node = settlement.own_address().to_fixed_bytes();
    // Everything the deployer key sends from here on, the node sends.
    drop(settlement);

    // The client: a funding key and the session key that signs vouchers.
    let payer = LocalWallet::from_bytes(&[0x61; 32]).expect("key");
    let session = LocalWallet::from_bytes(&[0x62; 32]).expect("key");
    x402.fund_gas(payer.address()).await;
    x402.mint(token, payer.address(), 1_000_000).await;

    let (app_addr, recorded) = spawn_recording_app().await;
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
    signer_key.write_all(&[0x07; 32]).expect("write");
    let mut settlement_key = tempfile::NamedTempFile::new().expect("settlement key");
    settlement_key
        .write_all(DEPLOYER_PRIVATE_KEY.as_bytes())
        .expect("write");
    let config = support::load_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{signer_key}"

[settlement.evm]
rpc_url = "{rpc_url}"
contract_address = "{registry:?}"
token_address = "{token:?}"
decimals = 6

[settlement.evm.key]
key_file = "{settlement_key}"

[settlement.evm.batch_settlement]
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[[routes]]
prefix = "{ROUTE}"
handler_url = "http://{app_addr}"
price = {PRICE}
"#,
        state_dir = state_dir.path().display(),
        signer_key = signer_key.path().display(),
        settlement_key = settlement_key.path().display(),
        rpc_url = anvil.rpc_url,
    ));

    let runtime = connector_cli::build(&config).await.expect("build");
    assert!(runtime.batch_settlement_evm.is_some(), "opted in on EVM");
    assert!(
        runtime.batch_settlement_solana.is_none(),
        "and not on Solana"
    );
    let app = connector_cli::router(&runtime, &config).expect("router");
    let receiver = runtime.signer.public_key().expect("the node's wrap key");

    // The client opens a channel that pays this node, through the
    // ERC-3009 collector a facilitator relays for it.
    let mut salt = [0u8; 32];
    salt[31] = 1;
    let channel_config = EvmChannelConfig {
        payer: payer.address().to_fixed_bytes(),
        payer_authorizer: session.address().to_fixed_bytes(),
        receiver: node,
        receiver_authorizer: node,
        token: token.to_fixed_bytes(),
        withdraw_delay: ONE_DAY,
        salt,
    };
    x402.deposit(&payer, &channel_config, DEPOSIT, 1).await;
    let channel = x402.channel_id(&channel_config);
    let key = format!("evm:{}", channel.0);

    // A paid write, and then another whose voucher names only its channel.
    for (amount, config) in [(PRICE, Some(&channel_config)), (2 * PRICE, None)] {
        let signature = x402.sign_voucher(&session, &channel, u128::from(amount));
        let response = post_ilp(
            &app,
            &evm_voucher(&channel, amount, &signature, config),
            paid_prepare(ROUTE, &receiver),
        )
        .await;
        Fulfill::decode(&response).unwrap_or_else(|_| {
            panic!(
                "the voucher for {amount} paid for the write: {:?}",
                Reject::decode(&response)
            )
        });
    }
    assert_eq!(
        recorded.lock().unwrap().len(),
        2,
        "both writes reached the app"
    );

    // Journaled: the channel, with the config that restores it, and each
    // voucher's signed bytes and watermark.
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
        .expect("the latest voucher is journaled");
    assert_eq!(amount, 2 * PRICE);

    // The node on Solana has not opted in, and says so.
    let refused = Reject::decode(
        &post_ilp(
            &app,
            &unaccepted_solana_voucher(),
            paid_prepare(ROUTE, &receiver),
        )
        .await,
    )
    .expect("a voucher on a chain this node has not opted in on is refused");
    assert!(
        refused.message.contains("batch-settlement"),
        "refused by name: {refused:?}"
    );

    // Restart: a new process, from the same config and state_dir. Nothing
    // was ever landed, and the chain never gives the config back -- the
    // journal is how the port can land this channel at all.
    drop(app);
    drop(runtime);
    let runtime = connector_cli::build(&config).await.expect("rebuild");
    let backend = runtime
        .batch_settlement_evm
        .clone()
        .expect("still opted in") as std::sync::Arc<dyn BatchSettlementBackend>;
    let before = backend
        .channel_state(&channel)
        .await
        .expect("restored at boot, from the journal");
    assert_eq!(before.status, BatchChannelStatus::Open);
    assert_eq!(before.landed, 0);

    let landed = backend
        .land(
            &channel,
            Voucher {
                cumulative_amount: u128::from(amount),
                signature,
            },
        )
        .await
        .expect("the journaled voucher lands");
    assert_eq!(landed.landed, u128::from(2 * PRICE));
    assert_eq!(
        x402.channel(&channel).await,
        (DEPOSIT, u128::from(2 * PRICE)),
        "totalClaimed on chain is the voucher's amount"
    );

    // And the restarted gate still takes a voucher that names only its
    // channel.
    let app = connector_cli::router(&runtime, &config).expect("router");
    let signature = x402.sign_voucher(&session, &channel, u128::from(3 * PRICE));
    Fulfill::decode(
        &post_ilp(
            &app,
            &evm_voucher(&channel, 3 * PRICE, &signature, None),
            paid_prepare(ROUTE, &receiver),
        )
        .await,
    )
    .expect("paid after the restart");
}
