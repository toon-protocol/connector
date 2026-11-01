//! ADR 0075 decision 7 on XRPL, end to end (#1616): a client earns through a
//! connector and is paid a **voucher** on a `PayChannel` the connector opened
//! toward the client's account, delivered over the client's BTP session,
//! which the client lands on the ledger itself.
//!
//! Two nodes built from config files over one spawned `xrpld` (ADR 0084).
//! Node A is the connector. Node B stands in for the client's key and chain
//! access: its settlement account is the client's account, its outbound
//! channel toward A is the channel the client pays on, and its receiving half
//! is how the client lands a payout. The steps are `client_payout_solana.rs`'s.
//!
//! A missing Docker fails CI and skips locally (`require_xrpld`).

mod payout_client;
mod support;

use std::io::Write;
use std::net::TcpListener;
use std::time::Duration;

use axum::Router;
use connector_domain::Network;
use connector_runtime::receiver_terms;
use connector_settlement::batch::BatchSettlementBackend;
use connector_settlement_xrpl::test_support::{require_xrpld, Wallet, STANDALONE_NETWORK_ID};
use url::Url;

use payout_client::{client_session, earn, payout_of};

const CLIENT_ADDRESS: &str = "g.toon.earner";
const NETWORK: &str = "xrpl:1000";
const FUNDED: u64 = 100_000_000;
const SETTLE_DELAY: u64 = 1_000;

struct NodeFiles {
    _state_dir: tempfile::TempDir,
    _signer_key: tempfile::NamedTempFile,
    _settlement_key: tempfile::NamedTempFile,
    config: connector_config::Config,
}

fn node_files(signer_seed: u8, wallet: &Wallet, rpc_url: &str) -> NodeFiles {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
    signer_key.write_all(&[signer_seed; 32]).expect("write");
    let mut settlement_key = tempfile::NamedTempFile::new().expect("settlement key");
    settlement_key
        .write_all(wallet.seed.as_bytes())
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

[settlement.xrpl.{STANDALONE_NETWORK_ID}]
rpc_url = "{rpc_url}"
min_settle_delay_secs = {SETTLE_DELAY}
redemption_safety_secs = 90

[settlement.xrpl.{STANDALONE_NETWORK_ID}.key]
key_file = "{settlement_key}"

[[tokens]]
asset = "xrpl:{STANDALONE_NETWORK_ID}/slip44:144"
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

/// A client voucher claim, as the client edge reads it.
fn xrpl_voucher(channel: &str, amount: u64, signature: &[u8]) -> String {
    serde_json::json!({
        "version": "1.0",
        "blockchain": "xrpl",
        "network": NETWORK,
        "scheme": "batch-settlement",
        "messageId": format!("voucher-{amount}"),
        "timestamp": "2026-10-30T12:00:00Z",
        "senderId": "an-xrpl-client",
        "channelId": channel,
        "maxClaimableAmount": amount.to_string(),
        "signature": hex::encode(signature),
    })
    .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_is_paid_an_xrpl_voucher_it_lands_on_the_ledger_itself() {
    if !require_xrpld() {
        return;
    }
    let chain = connector_settlement_xrpl::test_support::Xrpld::spawn().await;
    let rpc = chain.rpc();
    let _closer = chain.close_ledgers_every(Duration::from_millis(200));
    let a_wallet = chain.funded_wallet(&rpc, FUNDED).await;
    let client_wallet = chain.funded_wallet(&rpc, FUNDED).await;

    let a = node_files(0x6a, &a_wallet, &chain.rpc_url);
    let b = node_files(0x6b, &client_wallet, &chain.rpc_url);
    let a_runtime = connector_cli::build(&a.config).await.expect("build A");
    let a_addr = serve(connector_cli::router(&a_runtime, &a.config).expect("A's router"));
    let client = connector_cli::build(&b.config)
        .await
        .expect("build the client");
    let client_addr = serve(connector_cli::router(&client, &b.config).expect("client router"));

    // -- 1. The client's own channel toward A --
    let client_outbound = client.outbound_channels.clone().expect("x402");
    let a_url = Url::parse(&format!("http://{a_addr}/ilp")).unwrap();
    let a_terms = receiver_terms(&a_runtime.batch_settlements[0], Some(&a_url)).expect("A's terms");
    let (client_channel, _) = client_outbound
        .open(a_terms, 1_000)
        .await
        .expect("the client opens its channel toward A");
    let client_channel = client_channel.on_chain.id;
    let voucher_for =
        |signature: Vec<u8>, amount: u64| xrpl_voucher(&client_channel.0, amount, &signature);
    let signed = client_outbound
        .sign_voucher(&client_channel.0, 100)
        .await
        .expect("the client signs");
    let voucher = voucher_for(signed.signature, 100);

    // -- 2. A's operator opens the payout channel toward the client's account --
    let client_url = Url::parse(&format!("http://{client_addr}/ilp")).unwrap();
    let client_terms = receiver_terms(&client.batch_settlements[0], Some(&client_url))
        .expect("the client's terms");
    let (payout_channel, _) = a_runtime
        .outbound_channels
        .clone()
        .expect("x402")
        .open(client_terms, 5_000)
        .await
        .expect("A opens its payout channel toward the client");
    let payout_channel = payout_channel.on_chain.id;

    // -- 3. The client earns, and is paid over its session --
    let mut session = client_session(a_addr, CLIENT_ADDRESS, &voucher).await;
    let transfer = earn(a_addr, &mut session, CLIENT_ADDRESS, 300, 1).await;
    let (json, presentation, first) = payout_of(&transfer);
    assert_eq!(json["blockchain"], "xrpl", "{json}");
    assert_eq!(json["network"], NETWORK, "{json}");
    assert_eq!(presentation.channel(), &payout_channel);
    assert_eq!(first.cumulative_amount, 300);
    let transfer = earn(a_addr, &mut session, CLIENT_ADDRESS, 200, 2).await;
    let (_, _, second) = payout_of(&transfer);
    assert_eq!(second.cumulative_amount, 500, "vouchers are cumulative");

    // -- 4. The client lands its latest payout itself: a valid voucher --
    let lander = client
        .batch_settlement_xrpl
        .get(&NETWORK.parse::<Network>().expect("a network"))
        .cloned()
        .expect("x402 on XRPL");
    lander
        .admit(presentation.clone())
        .await
        .expect("the payout channel pays the client");
    let landed = lander
        .land(presentation.channel(), second)
        .await
        .expect("the client lands the payout voucher")
        .state;
    assert_eq!(landed.landed, 500);
    let on_chain = rpc
        .call(
            "ledger_entry",
            serde_json::json!({ "index": payout_channel.0 }),
        )
        .await
        .expect("the payout channel is on the ledger");
    assert_eq!(on_chain["node"]["Balance"], "500", "{on_chain}");
    assert_eq!(
        on_chain["node"]["Destination"],
        client_wallet.address.as_str(),
        "{on_chain}"
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
    let voucher = voucher_for(signed.signature, 200);
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
        .expect("the post-restart voucher lands above the last")
        .state;
    assert_eq!(landed.landed, 600);
}
