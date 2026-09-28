//! ADR 0075 decision 7 on Solana, end to end (issue #1381): a client earns
//! through a connector and is paid a **voucher** on a `payment-channels`
//! channel the connector opened toward it -- through the client's own
//! sponsor endpoint, so the client holds the `payee` and `rent_payer` seats
//! (ADR 0075 decision 3) -- delivered over the client's BTP session, which
//! the client lands on chain itself.
//!
//! Two nodes built from config files over one disposable validator holding
//! solana-foundation's `payment-channels` at `CHNLx…`. Node A is the
//! connector. Node B stands in for the client's keys, chain access and
//! sponsor endpoint: its settlement key is the client's key, its outbound
//! channel toward A is the channel the client pays on, and its receiving
//! half is how the client lands a payout. The client's socket is this
//! test's own BTP client. The steps are `client_payout_evm.rs`'s.
//!
//! Its own test binary: `solana-test-validator` binds fixed ports.

mod payout_client;
mod support;

use std::io::Write;
use std::net::TcpListener;

use axum::Router;
use connector_runtime::receiver_terms;
use connector_settlement::batch::BatchSettlementBackend;
use connector_settlement_solana::test_support::{
    create_mint, fund, mint_to, require_solana_test_validator, SolanaValidator,
};
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;
use url::Url;

use payout_client::{client_session, earn, payout_of};
use support::solana_voucher;

const CLIENT_ADDRESS: &str = "g.toon.earner";
const FUNDED: u64 = 1_000_000;

struct NodeFiles {
    _state_dir: tempfile::TempDir,
    _signer_key: tempfile::NamedTempFile,
    _settlement_key: tempfile::NamedTempFile,
    config: connector_config::Config,
}

fn node_files(seed: [u8; 32], rpc_url: &str, mint: &Pubkey) -> NodeFiles {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
    signer_key.write_all(&[seed[0] ^ 0xff; 32]).expect("write");
    let mut settlement_key = tempfile::NamedTempFile::new().expect("settlement key");
    settlement_key.write_all(&seed).expect("write");
    let config = support::load_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{signer_key}"

[settlement.solana]
rpc_url = "{rpc_url}"
token_address = "{mint}"
decimals = 6
min_sponsored_deposit = 1

[settlement.solana.key]
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

/// Serve `app` on a listener of its own, answering at the returned address.
fn serve_on(listener: TcpListener, app: Router) {
    tokio::spawn(async move {
        axum::Server::from_tcp(listener)
            .expect("serve from the listener")
            .serve(app.into_make_service())
            .await
            .expect("the node serves");
    });
}

fn serve(app: Router) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("its address");
    serve_on(listener, app);
    addr
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_is_paid_a_solana_voucher_it_lands_on_chain_itself() {
    if !require_solana_test_validator() {
        return;
    }

    let validator = SolanaValidator::spawn().await;
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let authority = solana_sdk::signature::Keypair::new();
    fund(&rpc, &authority.pubkey()).await;
    let mint = create_mint(&rpc, &authority, 6).await;

    let (a_seed, client_seed) = ([0x6a; 32], [0x6b; 32]);
    let a_key = solana_sdk::signer::keypair::keypair_from_seed(&a_seed).expect("a keypair");
    let client_key =
        solana_sdk::signer::keypair::keypair_from_seed(&client_seed).expect("a keypair");
    for key in [&a_key, &client_key] {
        fund(&rpc, &key.pubkey()).await;
        mint_to(&rpc, &authority, &mint, &key.pubkey(), FUNDED).await;
    }

    let a = node_files(a_seed, &validator.rpc_url, &mint);
    let b = node_files(client_seed, &validator.rpc_url, &mint);
    let a_runtime = connector_cli::build(&a.config).await.expect("build A");
    let a_addr = serve(connector_cli::router(&a_runtime, &a.config).expect("A's router"));
    let client = connector_cli::build(&b.config)
        .await
        .expect("build the client");
    let client_addr = serve(connector_cli::router(&client, &b.config).expect("client router"));

    // -- 1. The client's own channel toward A, through A's sponsor --
    let client_outbound = client.outbound_channels.clone().expect("x402");
    let a_url = Url::parse(&format!("http://{a_addr}/ilp")).unwrap();
    let a_terms = receiver_terms(&a_runtime.batch_settlements[0], Some(&a_url)).expect("A's terms");
    let (client_channel, _) = client_outbound
        .open(a_terms, 1_000)
        .await
        .expect("the client opens its channel toward A");
    let client_channel = client_channel.on_chain.id;
    let voucher_for = |signature: Vec<u8>, amount: u64| {
        let signature: [u8; 64] = signature.try_into().expect("64 bytes");
        solana_voucher(&client_channel.0, amount, &signature)
    };
    let signed = client_outbound
        .sign_voucher(&client_channel.0, 100)
        .await
        .expect("the client signs");
    let voucher = voucher_for(signed.signature, 100);

    // -- 2. A's operator opens the payout channel, through the client's
    //       own sponsor endpoint --
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
    let (_, presentation, first) = payout_of(&transfer);
    assert_eq!(presentation.channel(), &payout_channel);
    assert_eq!(first.cumulative_amount, 300);
    let transfer = earn(a_addr, &mut session, CLIENT_ADDRESS, 200, 2).await;
    let (_, _, second) = payout_of(&transfer);
    assert_eq!(second.cumulative_amount, 500, "vouchers are cumulative");

    // -- 4. The client lands its latest payout itself --
    let lander = client
        .batch_settlement_solana
        .clone()
        .expect("x402 on Solana");
    lander
        .admit(presentation.clone())
        .await
        .expect("the payout channel pays the client");
    let landed = lander
        .land(presentation.channel(), second)
        .await
        .expect("the client lands the payout voucher");
    assert_eq!(landed.landed, 500);

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
        .expect("the post-restart voucher lands above the last");
    assert_eq!(landed.landed, 600);
}
