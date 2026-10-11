//! ADR 0075 decision 7 on EVM, end to end (issue #1381): a client earns
//! through a connector and is paid a **voucher** on an `x402BatchSettlement`
//! channel the connector opened toward it, delivered over the client's BTP
//! session, which the client lands on chain itself.
//!
//! Two nodes built from config files over one disposable `anvil` holding
//! x402's real contract at its canonical address. Node A is the connector.
//! Node B stands in for the client's keys and chain access: its settlement
//! key is the client's key, its outbound channel toward A is the channel the
//! client pays on, and its receiving half is how the client lands a payout.
//! The client's socket is this test's own BTP client.
//!
//! 1. The client opens its own channel toward A and pays A a voucher over
//!    its BTP session -- which is how A learns the key it is paid at;
//! 2. A's operator opens A's payout channel toward the terms the client
//!    publishes;
//! 3. A PREPARE to the client's address is fulfilled by the client's
//!    session, and A's payout voucher arrives over the same socket;
//! 4. the client lands it on chain, and the chain says it was paid;
//! 5. A restarts from its config and `state_dir`, and the next payout
//!    carries on above the journaled watermark.

mod payout_client;
mod support;

use std::io::Write;
use std::net::TcpListener;

use axum::Router;
use connector_runtime::receiver_terms;
use connector_settlement::batch::{BatchSettlementBackend, ChannelPresentation};
use connector_settlement::ChannelId;
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{
    require_anvil, Anvil, COUNTERPARTY_PRIVATE_KEY, DEPLOYER_PRIVATE_KEY,
};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::Address;

use payout_client::{client_session, earn, payout_of};
use support::evm_voucher;

/// This binary's own base port for [`Anvil::spawn`], clear of every other
/// anvil binary's range.
const ANVIL_BASE_PORT: u16 = 23_300;

const CLIENT_ADDRESS: &str = "g.toon.earner";
const FUNDED: u128 = 1_000_000;

fn hex32(text: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).expect("hex");
    }
    out
}

fn address_of(key: &str) -> Address {
    LocalWallet::from_bytes(&hex32(key)).expect("key").address()
}

struct NodeFiles {
    _state_dir: tempfile::TempDir,
    _signer_key: tempfile::NamedTempFile,
    _settlement_key: tempfile::NamedTempFile,
    config: connector_config::Config,
}

fn node_files(
    settlement_key_hex: &str,
    signer_seed: u8,
    rpc_url: &str,
    token: Address,
) -> NodeFiles {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
    signer_key.write_all(&[signer_seed; 32]).expect("write");
    let mut settlement_key = tempfile::NamedTempFile::new().expect("settlement key");
    settlement_key
        .write_all(settlement_key_hex.as_bytes())
        .expect("write");
    let config = support::load_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

# Where the client's voucher-carrying probe goes: a voucher riding a packet to
# a destination nothing serves is never looked at (issue #1446).
[[routes]]
prefix = "g.toon.nowhere"
handler_url = "http://localhost:4000/"
price = 1

[signer]
key_file = "{signer_key}"

[settlement.evm]
rpc_url = "{rpc_url}"
token_address = "{token:?}"
decimals = 6
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[settlement.evm.key]
key_file = "{settlement_key}"

"#,
        state_dir = state_dir.path().display(),
        signer_key = signer_key.path().display(),
        settlement_key = settlement_key.path().display(),
    ));
    NodeFiles {
        _state_dir: state_dir,
        _signer_key: signer_key,
        _settlement_key: settlement_key,
        config,
    }
}

fn serve(app: Router) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("its address");
    tokio::spawn(async move {
        axum::Server::from_tcp(listener)
            .expect("serve from the listener")
            .serve(app.into_make_service())
            .await
            .expect("the node serves");
    });
    addr
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_is_paid_an_evm_voucher_it_lands_on_chain_itself() {
    if !require_anvil() {
        return;
    }

    let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
    let mut x402 = X402Chain::place(&anvil.rpc_url).await;
    let token = x402.deploy_fiat_token().await;
    let connector_key = address_of(DEPLOYER_PRIVATE_KEY);
    let client_key = address_of(COUNTERPARTY_PRIVATE_KEY);
    x402.mint(token, connector_key, FUNDED).await;
    x402.mint(token, client_key, FUNDED).await;

    let a = node_files(DEPLOYER_PRIVATE_KEY, 0x0a, &anvil.rpc_url, token);
    let b = node_files(COUNTERPARTY_PRIVATE_KEY, 0x0b, &anvil.rpc_url, token);
    let a_runtime = connector_cli::build(&a.config).await.expect("build A");
    let a_addr = serve(connector_cli::router(&a_runtime, &a.config).expect("A's router"));
    let client = connector_cli::build(&b.config)
        .await
        .expect("build the client");
    let _client_app = connector_cli::router(&client, &b.config).expect("the client's router");

    // -- 1. The client's own channel toward A, and its voucher on it --
    let client_outbound = client.outbound_channels.clone().expect("x402");
    let a_terms = receiver_terms(&a_runtime.batch_settlements[0], None).expect("A's terms");
    let (client_channel, _) = client_outbound
        .open(a_terms, 1_000)
        .await
        .expect("the client opens its channel toward A");
    let client_channel = client_channel.on_chain.id;
    let Some(ChannelPresentation::Evm { config, .. }) =
        client_outbound.presentation(&client_channel.0)
    else {
        panic!("an EVM channel presents its config");
    };
    let signed = client_outbound
        .sign_voucher(&client_channel.0, 100)
        .await
        .expect("the client signs");
    let voucher = evm_voucher(&client_channel, 100, &signed.signature, Some(&config));

    // -- 2. A's operator opens the payout channel toward the client --
    let client_terms =
        receiver_terms(&client.batch_settlements[0], None).expect("the client's terms");
    let (payout_channel, _) = a_runtime
        .outbound_channels
        .clone()
        .expect("x402")
        .open(client_terms, 5_000)
        .await
        .expect("A opens its payout channel toward the client");
    let payout_channel: ChannelId = payout_channel.on_chain.id;

    // -- 3. The client earns, and is paid over its session --
    let mut session = client_session(a_addr, CLIENT_ADDRESS, &voucher).await;
    let transfer = earn(a_addr, &mut session, CLIENT_ADDRESS, 300, 1).await;
    let (_, presentation, first) = payout_of(&transfer);
    assert_eq!(presentation.channel(), &payout_channel);
    assert_eq!(first.cumulative_amount, 300);
    let transfer = earn(a_addr, &mut session, CLIENT_ADDRESS, 200, 2).await;
    let (_, _, second) = payout_of(&transfer);
    assert_eq!(second.cumulative_amount, 500, "vouchers are cumulative");

    // -- 4. The client lands its latest payout itself --
    let lander = client.batch_settlement_evm.clone().expect("x402 on EVM");
    lander
        .admit(presentation.clone())
        .await
        .expect("the payout channel pays the client");
    let landed = lander
        .land(presentation.channel(), second)
        .await
        .expect("the client lands the payout voucher");
    assert_eq!(landed.landed, 500);
    assert_eq!(
        x402.channel(&payout_channel).await,
        (5_000, 500),
        "totalClaimed on chain is the payout the client landed"
    );

    // -- 5. A restarts; the payout watermark comes back from its journal --
    drop(session);
    drop(a_runtime);
    let a_runtime = connector_cli::build(&a.config).await.expect("rebuild A");
    let a_addr = serve(connector_cli::router(&a_runtime, &a.config).expect("A's router"));
    let signed = client_outbound
        .sign_voucher(&client_channel.0, 200)
        .await
        .expect("the client signs again");
    let voucher = evm_voucher(&client_channel, 200, &signed.signature, None);
    let mut session = client_session(a_addr, CLIENT_ADDRESS, &voucher).await;
    let transfer = earn(a_addr, &mut session, CLIENT_ADDRESS, 100, 3).await;
    let (_, presentation, third) = payout_of(&transfer);
    assert_eq!(presentation.channel(), &payout_channel);
    assert_eq!(
        third.cumulative_amount, 600,
        "the payout watermark survived A's restart"
    );
    let landed = lander
        .land(presentation.channel(), third)
        .await
        .expect("the post-restart voucher lands above the last");
    assert_eq!(landed.landed, 600);
    assert_eq!(x402.channel(&payout_channel).await, (5_000, 600));
}
