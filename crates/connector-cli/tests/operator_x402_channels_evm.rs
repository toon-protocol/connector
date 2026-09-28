//! ADR 0075 decision 11 on EVM, end to end: two nodes built from config
//! files over one disposable `anvil` holding x402's real
//! `x402BatchSettlement` at its canonical address. Every step is a signed
//! operator write through the production router:
//!
//! 1. node A opens an outbound channel toward node B on the terms B's own
//!    self-description publishes (`POST /channels`), and tops it up
//!    (`POST /channels/:id/fund`);
//! 2. A signs a voucher on it, B's client edge accepts it for a paid write,
//!    and B lands it now (`POST /channels/:id/land`);
//! 3. A withdraws what B did not land -- starting, refused while the delay
//!    runs, finishing once it has (`POST /channels/:id/withdraw`);
//! 4. A restarts from its config and `state_dir` and still knows the channel
//!    and its signed watermark.

mod support;

use std::io::Write;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use connector_domain::{Fulfill, Reject};
use connector_operator::signing::{keyid_hex, sign_request};
use connector_runtime::BatchChannelError;
use connector_settlement::batch::{BatchSettlementError, ChannelPresentation};
use connector_settlement::ChannelId;
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{
    require_anvil, Anvil, COUNTERPARTY_PRIVATE_KEY, DEPLOYER_PRIVATE_KEY,
};
use ed25519_dalek::Keypair;
use ethers::signers::{LocalWallet, Signer};
use ethers::types::Address;
use rand::rngs::OsRng;
use tower::ServiceExt;

use support::{evm_voucher, paid_prepare, post_ilp, spawn_recording_app};

/// This binary's own base port for [`Anvil::spawn`], clear of every other
/// anvil binary's range.
const ANVIL_BASE_PORT: u16 = 22_700;

const ROUTE: &str = "g.toon.outbound";
const PRICE: u64 = 300;
const ONE_DAY: u64 = 86_400;
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

/// One node's files: its state, its keys, and the config naming them.
struct NodeFiles {
    state_dir: tempfile::TempDir,
    _signer_key: tempfile::NamedTempFile,
    _settlement_key: tempfile::NamedTempFile,
    config: connector_config::Config,
}

fn node_files(
    settlement_key_hex: &str,
    signer_seed: u8,
    rpc_url: &str,
    token: Address,
    operator: &Keypair,
    route: Option<&str>,
) -> NodeFiles {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
    signer_key.write_all(&[signer_seed; 32]).expect("write");
    let mut settlement_key = tempfile::NamedTempFile::new().expect("settlement key");
    settlement_key
        .write_all(settlement_key_hex.as_bytes())
        .expect("write");
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

[settlement.evm]
rpc_url = "{rpc_url}"
token_address = "{token:?}"
decimals = 6
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[settlement.evm.key]
key_file = "{settlement_key}"

{route}
"#,
        state_dir = state_dir.path().display(),
        signer_key = signer_key.path().display(),
        settlement_key = settlement_key.path().display(),
        write_key = keyid_hex(operator),
    ));
    NodeFiles {
        state_dir,
        _signer_key: signer_key,
        _settlement_key: settlement_key,
        config,
    }
}

/// An operator write, signed at a fresh `created` so a repeat is not a
/// replay.
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

#[tokio::test]
async fn an_outbound_channel_is_opened_funded_landed_and_withdrawn_over_the_operator_surface() {
    if !require_anvil() {
        return;
    }

    let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
    let mut x402 = X402Chain::place(&anvil.rpc_url).await;
    let token = x402.deploy_fiat_token().await;
    let payer = address_of(DEPLOYER_PRIVATE_KEY);
    x402.mint(token, payer, FUNDED).await;

    let operator = Keypair::generate(&mut OsRng);
    let (app_addr, recorded) = spawn_recording_app().await;
    let a = node_files(
        DEPLOYER_PRIVATE_KEY,
        0x0a,
        &anvil.rpc_url,
        token,
        &operator,
        None,
    );
    let b = node_files(
        COUNTERPARTY_PRIVATE_KEY,
        0x0b,
        &anvil.rpc_url,
        token,
        &operator,
        Some(&app_addr),
    );
    let a_runtime = connector_cli::build(&a.config).await.expect("build A");
    let a_app = connector_cli::router(&a_runtime, &a.config).expect("A's router");
    let b_runtime = connector_cli::build(&b.config).await.expect("build B");
    let b_app = connector_cli::router(&b_runtime, &b.config).expect("B's router");

    // -- Open, toward B, on the terms B publishes (ADR 0075 decision 10) --
    let terms = serde_json::to_value(&b_runtime.batch_settlements[0]).expect("B's terms");
    let (status, opened) = write(
        &a_app,
        &operator,
        1_000,
        "/channels",
        Some(serde_json::json!({ "terms": terms, "deposit": 1_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    assert_eq!(opened["direction"], "outbound");
    assert_eq!(opened["collateral"], 1_000);
    assert_eq!(opened["resumed"], false);
    let id = opened["id"].as_str().expect("an id").to_string();
    assert_eq!(x402.balance_of(token, payer).await, FUNDED - 1_000);

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
    assert_eq!(x402.channel(&ChannelId(id.clone())).await, (1_500, 0));

    // -- A pays B with a voucher; B lands it now --
    let outbound = a_runtime.outbound_channels.clone().expect("A pays on x402");
    let voucher = outbound
        .sign_voucher(&id, u128::from(PRICE))
        .await
        .expect("A signs on its channel");
    let Some(ChannelPresentation::Evm { config, .. }) = outbound.presentation(&id) else {
        panic!("an EVM channel presents its config");
    };
    let receiver = b_runtime.signer.public_key().expect("B's wrap key");
    let response = post_ilp(
        &b_app,
        &evm_voucher(
            &ChannelId(id.clone()),
            PRICE,
            &voucher.signature,
            Some(&config),
        ),
        paid_prepare(ROUTE, &receiver),
    )
    .await;
    Fulfill::decode(&response).unwrap_or_else(|_| {
        panic!(
            "A's voucher paid for B's write: {:?}",
            Reject::decode(&response)
        )
    });
    assert_eq!(recorded.lock().unwrap().len(), 1);

    let inbound = read(&b_app, "/channels").await;
    let listed = row(&inbound, &id);
    assert_eq!(listed["direction"], "inbound");
    assert_eq!(listed["watermark"], PRICE);
    assert_eq!(listed["landed"], 0);

    let land = format!("/channels/{id}/land");
    let (status, landed) = write(&b_app, &operator, 2_000, &land, None).await;
    assert_eq!(status, StatusCode::OK, "{landed}");
    assert_eq!(landed["landed"], PRICE);
    assert_eq!(
        x402.channel(&ChannelId(id.clone())).await,
        (1_500, u128::from(PRICE)),
        "totalClaimed on chain is the voucher's amount"
    );
    let (status, _) = write(&b_app, &operator, 2_001, &land, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "a voucher lands once");

    let claims = read(&a_app, "/claims").await;
    let signed = claims
        .as_array()
        .expect("a list")
        .iter()
        .find(|claim| claim["channel_id"] == format!("evm:{id}"))
        .expect("A's signed voucher is listed");
    assert_eq!(signed["direction"], "outbound");
    assert_eq!(signed["scheme"], "batch-settlement");
    assert_eq!(signed["cumulative_amount"], PRICE);

    // -- A withdraws what B did not land --
    let withdraw = format!("/channels/{id}/withdraw");
    let (status, started) = write(&a_app, &operator, 1_002, &withdraw, None).await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert_eq!(started["step"], "started");
    assert_eq!(started["status"], "withdrawing");
    let (status, early) = write(&a_app, &operator, 1_003, &withdraw, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{early}");
    x402.advance_time(ONE_DAY).await;
    let (status, finished) = write(&a_app, &operator, 1_004, &withdraw, None).await;
    assert_eq!(status, StatusCode::OK, "{finished}");
    assert_eq!(finished["step"], "finished");
    assert_eq!(
        x402.balance_of(token, payer).await,
        FUNDED - u128::from(PRICE),
        "A paid exactly what B landed"
    );

    // -- A restarts, and still knows its channel and what it signed --
    drop(a_app);
    drop(outbound);
    drop(a_runtime);
    let a_runtime = connector_cli::build(&a.config).await.expect("rebuild A");
    let a_app = connector_cli::router(&a_runtime, &a.config).expect("A's router");
    let listed = read(&a_app, "/channels").await;
    let restored = row(&listed, &id);
    assert_eq!(restored["direction"], "outbound");
    assert_eq!(restored["watermark"], PRICE);
    assert_eq!(restored["landed"], PRICE);
    assert_eq!(
        a_runtime
            .outbound_channels
            .clone()
            .expect("A pays on x402")
            .sign_voucher(&id, u128::from(PRICE))
            .await
            .unwrap_err(),
        BatchChannelError::Settlement(BatchSettlementError::VoucherNotAdvancing {
            amount: u128::from(PRICE),
            signed: u128::from(PRICE),
        }),
        "the signed watermark survives the restart"
    );
    assert!(a.state_dir.path().join("outbound-channels.log").exists());
}
