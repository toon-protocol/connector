//! ADR 0075 decision 11 on Solana, end to end: two nodes built from config
//! files over one disposable validator holding solana-foundation's
//! `payment-channels` at `CHNLx…`. Node B serves its client edge on a real
//! socket, so node A's open reaches the chain the only way it can -- through
//! B's own sponsor endpoint, which keeps the `payee` seat (ADR 0074
//! decision 9). Every step is a signed operator write through the
//! production router:
//!
//! 1. A opens an outbound channel toward B on the terms B publishes, and
//!    tops it up;
//! 2. A signs a voucher, B accepts it for a paid write and lands it now;
//! 3. A restarts from its config and `state_dir`, still knows the channel
//!    and its signed watermark, and signs on above it;
//! 4. A requests a close, B's latest voucher seals it, and A distributes,
//!    taking back what B did not land.
//!
//! Its own test binary: `solana-test-validator` binds fixed ports.

mod support;

use std::io::Write;
use std::net::TcpListener;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use connector_domain::{Fulfill, Reject};
use connector_operator::signing::{keyid_hex, sign_request};
use connector_runtime::BatchChannelError;
use connector_settlement::batch::BatchSettlementError;
use connector_settlement_solana::test_support::{
    create_mint, fund, mint_to, require_solana_test_validator, SolanaValidator,
    LOCAL_TEST_PROGRAM_ID,
};
use ed25519_dalek::Keypair;
use rand::rngs::OsRng;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;
use tower::ServiceExt;

use support::{paid_prepare, post_ilp, solana_voucher, spawn_recording_app};

const ROUTE: &str = "g.toon.outbound";
const PRICE: u64 = 300;
const FUNDED: u64 = 1_000_000;

struct NodeFiles {
    _state_dir: tempfile::TempDir,
    _signer_key: tempfile::NamedTempFile,
    _settlement_key: tempfile::NamedTempFile,
    config: connector_config::Config,
}

fn node_files(
    seed: [u8; 32],
    rpc_url: &str,
    mint: &Pubkey,
    operator: &Keypair,
    route: Option<&str>,
) -> NodeFiles {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
    signer_key.write_all(&[seed[0] ^ 0xff; 32]).expect("write");
    let mut settlement_key = tempfile::NamedTempFile::new().expect("settlement key");
    settlement_key.write_all(&seed).expect("write");
    let route = route
        .map(|app| {
            format!(
                "[[routes]]\nprefix = \"{ROUTE}\"\nhandler_url = \"http://{app}\"\nprice = {PRICE}\n"
            )
        })
        .unwrap_or_default();
    let config = support::load_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{signer_key}"

[operator]
bearer_token = "operator-secret"
write_keys = ["{write_key}"]

[settlement.solana]
rpc_url = "{rpc_url}"
token_address = "{mint}"
decimals = 6
min_sponsored_deposit = 1

[settlement.solana.key]
key_file = "{settlement_key}"

{route}
"#,
        state_dir = state_dir.path().display(),
        signer_key = signer_key.path().display(),
        settlement_key = settlement_key.path().display(),
        write_key = keyid_hex(operator),
    ));
    NodeFiles {
        _state_dir: state_dir,
        _signer_key: signer_key,
        _settlement_key: settlement_key,
        config,
    }
}

async fn write(
    app: &Router,
    operator: &Keypair,
    created: u64,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let body = body
        .map(|body| serde_json::to_vec(&body).expect("json"))
        .unwrap_or_default();
    let (sig_input, sig, digest) =
        sign_request(operator, "POST", path, &body, created, Some(9_999_999_999));
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("signature-input", sig_input)
        .header("signature", sig)
        .header("content-digest", digest)
        .body(Body::from(body))
        .expect("a request");
    answer(app, request).await
}

async fn read(app: &Router, path: &str) -> serde_json::Value {
    let request = Request::builder()
        .uri(path)
        .header(header::AUTHORIZATION, "Bearer operator-secret")
        .body(Body::empty())
        .expect("a request");
    let (status, body) = answer(app, request).await;
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    body
}

async fn answer(app: &Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let response = app.clone().oneshot(request).await.expect("an answer");
    let status = response.status();
    let bytes = hyper::body::to_bytes(response.into_body())
        .await
        .expect("the body");
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&bytes).into()));
    (status, body)
}

fn row<'a>(list: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    list.as_array()
        .expect("a list")
        .iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("{id} is listed in {list}"))
}

async fn token_balance(rpc: &RpcClient, owner: &Pubkey, mint: &Pubkey) -> u64 {
    let ata = spl_associated_token_account::get_associated_token_address(owner, mint);
    match rpc.get_token_account_balance(&ata).await {
        Ok(balance) => balance.amount.parse().expect("an amount"),
        Err(_) => 0,
    }
}

/// Serve `app` on a socket of its own, answering at the returned address.
fn serve(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("its address");
    tokio::spawn(async move {
        axum::Server::from_tcp(listener)
            .expect("serve from the listener")
            .serve(app.into_make_service())
            .await
            .expect("the node serves");
    });
    addr.to_string()
}

#[tokio::test]
async fn an_outbound_channel_is_opened_funded_landed_and_withdrawn_over_the_operator_surface() {
    if !require_solana_test_validator() {
        return;
    }

    let validator = SolanaValidator::spawn().await;
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let authority = solana_sdk::signature::Keypair::new();
    fund(&rpc, &authority.pubkey()).await;
    let mint = create_mint(&rpc, &authority, 6).await;

    // Each node's `[settlement.solana]` key, holding SOL. A holds the
    // tokens it deposits; B holds a receiving account, which its sponsor
    // checks exists before it floats a channel's rent.
    let (a_seed, b_seed) = ([0x5a; 32], [0x5b; 32]);
    let a_key = solana_sdk::signer::keypair::keypair_from_seed(&a_seed).expect("a keypair");
    let b_key = solana_sdk::signer::keypair::keypair_from_seed(&b_seed).expect("a keypair");
    for key in [&a_key, &b_key] {
        fund(&rpc, &key.pubkey()).await;
    }
    mint_to(&rpc, &authority, &mint, &a_key.pubkey(), FUNDED).await;
    mint_to(&rpc, &authority, &mint, &b_key.pubkey(), 0).await;

    let operator = Keypair::generate(&mut OsRng);
    let (app_addr, recorded) = spawn_recording_app().await;
    let a = node_files(a_seed, &validator.rpc_url, &mint, &operator, None);
    let b = node_files(
        b_seed,
        &validator.rpc_url,
        &mint,
        &operator,
        Some(&app_addr),
    );
    let a_runtime = connector_cli::build(&a.config).await.expect("build A");
    let a_app = connector_cli::router(&a_runtime, &a.config).expect("A's router");
    let b_runtime = connector_cli::build(&b.config).await.expect("build B");
    let b_app = connector_cli::router(&b_runtime, &b.config).expect("B's router");
    let b_addr = serve(b_app.clone());

    // -- Open toward B, through B's own sponsor endpoint --
    let terms = serde_json::to_value(&b_runtime.batch_settlements[0]).expect("B's terms");
    assert_eq!(
        terms["sponsorEndpoint"],
        connector_cli::SPONSOR_PATH,
        "B publishes its sponsor endpoint as a path, resolved against its URL"
    );
    let (status, opened) = write(
        &a_app,
        &operator,
        1_000,
        "/channels",
        Some(serde_json::json!({
            "terms": terms,
            "deposit": 1_000,
            "url": format!("http://{b_addr}/ilp"),
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    assert_eq!(opened["direction"], "outbound");
    assert_eq!(opened["collateral"], 1_000);
    let id = opened["id"].as_str().expect("an id").to_string();
    assert_eq!(
        token_balance(&rpc, &a_key.pubkey(), &mint).await,
        FUNDED - 1_000
    );

    let (status, funded) = write(
        &a_app,
        &operator,
        1_001,
        &format!("/channels/{id}/fund"),
        Some(serde_json::json!({ "amount": 500 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{funded}");
    assert_eq!(funded["collateral"], 1_500);

    // -- A pays B; B lands it now, on an open channel --
    let receiver = b_runtime.signer.public_key().expect("B's wrap key");
    let pay = |outbound: std::sync::Arc<connector_runtime::OutboundChannels>, amount: u64| {
        let (b_app, id, receiver) = (b_app.clone(), id.clone(), receiver);
        async move {
            let voucher = outbound
                .sign_voucher(&id, u128::from(amount))
                .await
                .expect("A signs on its channel");
            let signature: [u8; 64] = voucher.signature.try_into().expect("64 bytes");
            let response = post_ilp(
                &b_app,
                &solana_voucher(&id, amount, &signature),
                paid_prepare(ROUTE, &receiver),
            )
            .await;
            Fulfill::decode(&response).unwrap_or_else(|_| {
                panic!(
                    "A's voucher for {amount} paid for B's write: {:?}",
                    Reject::decode(&response)
                )
            });
        }
    };
    pay(a_runtime.outbound_channels.clone().expect("x402"), PRICE).await;
    let listed = read(&b_app, "/channels").await;
    assert_eq!(row(&listed, &id)["direction"], "inbound");
    assert_eq!(row(&listed, &id)["watermark"], PRICE);

    let land = format!("/channels/{id}/land");
    let (status, landed) = write(&b_app, &operator, 2_000, &land, None).await;
    assert_eq!(status, StatusCode::OK, "{landed}");
    assert_eq!(landed["landed"], PRICE);
    assert_eq!(landed["status"], "open");

    // -- A restarts, and still knows its channel and what it signed --
    drop(a_app);
    drop(a_runtime);
    let a_runtime = connector_cli::build(&a.config).await.expect("rebuild A");
    let a_app = connector_cli::router(&a_runtime, &a.config).expect("A's router");
    let outbound = a_runtime.outbound_channels.clone().expect("x402");
    let restored = read(&a_app, "/channels").await;
    assert_eq!(row(&restored, &id)["watermark"], PRICE);
    assert_eq!(row(&restored, &id)["landed"], PRICE);
    assert_eq!(
        outbound
            .sign_voucher(&id, u128::from(PRICE))
            .await
            .unwrap_err(),
        BatchChannelError::Settlement(BatchSettlementError::VoucherNotAdvancing {
            amount: u128::from(PRICE),
            signed: u128::from(PRICE),
        }),
        "the signed watermark survives the restart"
    );
    pay(outbound, 2 * PRICE).await;
    assert_eq!(recorded.lock().unwrap().len(), 2);

    // -- A winds down: a close, sealed by B's latest voucher, distributed --
    let withdraw = format!("/channels/{id}/withdraw");
    let (status, started) = write(&a_app, &operator, 1_002, &withdraw, None).await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert_eq!(started["step"], "started");
    assert_eq!(started["status"], "closing");
    // B lands what it holds with `settle_and_seal`. Its own Closing watcher
    // races this write for the same voucher, so either may be the one that
    // seals; what must hold is that the channel is sealed at B's latest.
    let (status, sealed) = write(&b_app, &operator, 2_001, &land, None).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CONFLICT,
        "{status}: {sealed}"
    );
    let listed = read(&b_app, "/channels").await;
    assert_eq!(row(&listed, &id)["status"], "sealed");
    assert_eq!(row(&listed, &id)["landed"], 2 * PRICE);

    let (status, finished) = write(&a_app, &operator, 1_003, &withdraw, None).await;
    assert_eq!(status, StatusCode::OK, "{finished}");
    assert_eq!(finished["step"], "finished");
    assert_eq!(
        token_balance(&rpc, &a_key.pubkey(), &mint).await,
        FUNDED - 2 * PRICE,
        "A paid exactly what B landed"
    );
    assert_eq!(token_balance(&rpc, &b_key.pubkey(), &mint).await, 2 * PRICE);
}
