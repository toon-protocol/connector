//! ADR 0087, a client pays a node over XRPL (#1615): a client opens its own
//! `PayChannel` to the node and pays for a write with a `CLM\0` voucher
//! through the real runtime, over a spawned `xrpld` (ADR 0084).
//!
//! What it holds: the app is told the chain and the network, the journal
//! records the voucher, a claim with no `network` or an unconfigured one is
//! refused by name, a voucher at or below the watermark or over the ceiling
//! is refused by the existing refusals, a client that lost its store
//! recovers the watermark from `POST /ilp/claim-state`, and
//! `POST /channels/:id/land` moves value and leaves the channel open -- and
//! reports a land that delivered nothing as a failure.
//!
//! A missing Docker fails CI and skips locally (`require_xrpld`).

mod support;

use std::io::Write;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use connector_chain_rpc::xrpl::{TxFate, TxWait, XrplRpc};
use connector_domain::{Fulfill, JournalEntry, Reject};
use connector_operator::signing::{keyid_hex, sign_request};
use connector_runtime::{FileJournal, Journal};
use connector_settlement_xrpl::test_support::{
    require_xrpld, LedgerCloser, Wallet, Xrpld, STANDALONE_NETWORK_ID,
};
use connector_signer::XrplSigner;
use ed25519_dalek::Keypair;
use rand::rngs::OsRng;
use serde_json::{json, Value};
use tower::ServiceExt;

use support::{paid_prepare, post_ilp, spawn_recording_app, CLIENT_EDGE_JOURNAL};

const ROUTE: &str = "g.toon.xrpl";
const PRICE: u64 = 100;
const NETWORK: &str = "xrpl:1000";
/// Just above the config's 901-second floor: the stand-alone clock only
/// moves when it is closed, so the last step closes ledgers until it runs out.
const SETTLE_DELAY: u64 = 1_000;
const TF_CLOSE: u64 = 0x0002_0000;
const WAIT: TxWait = TxWait {
    poll: Duration::from_millis(50),
    give_up_after: Duration::from_secs(30),
};

fn channel_bytes(id: &str) -> [u8; 32] {
    hex::decode(id).expect("hex").try_into().expect("32 bytes")
}

fn voucher(network: Option<&str>, channel: &str, drops: u64, signer: &XrplSigner) -> String {
    let signature = signer
        .sign_claim(&channel_bytes(channel), u128::from(drops))
        .expect("a claim");
    let mut claim = json!({
        "version": "1.0",
        "blockchain": "xrpl",
        "scheme": "batch-settlement",
        "messageId": format!("voucher-{drops}"),
        "timestamp": "2026-10-30T12:00:00Z",
        "senderId": "an-xrpl-client",
        "channelId": channel,
        "maxClaimableAmount": drops.to_string(),
        "signature": hex::encode(signature),
    });
    if let Some(network) = network {
        claim["network"] = json!(network);
    }
    claim.to_string()
}

struct World {
    chain: Xrpld,
    rpc: XrplRpc,
    payer: Wallet,
    payer_signer: XrplSigner,
    _closer: LedgerCloser,
}

impl World {
    async fn submit(&self, mut tx: Value, seed: &str) -> Value {
        tx["LastLedgerSequence"] =
            json!(self.rpc.fee().await.expect("fee").ledger_current_index + 40);
        let last = u32::try_from(tx["LastLedgerSequence"].as_u64().expect("last")).expect("u32");
        let submitted = self.rpc.sign_and_submit(tx, seed).await.expect("submit");
        assert_eq!(submitted.engine_result, "tesSUCCESS", "{submitted:?}");
        match self
            .rpc
            .wait_for_tx(&submitted.hash, last, WAIT)
            .await
            .expect("wait")
        {
            TxFate::Landed { meta, .. } => meta,
            other => panic!("the transaction did not land: {other:?}"),
        }
    }

    /// A channel the payer opens to `node`, as a client does, returning its id.
    async fn open_channel(&self, node: &str, drops: u64) -> String {
        let meta = self
            .submit(
                json!({
                    "TransactionType": "PaymentChannelCreate",
                    "Account": self.payer.address,
                    "Destination": node,
                    "Amount": drops.to_string(),
                    "SettleDelay": SETTLE_DELAY,
                    "PublicKey": hex::encode_upper(self.payer_signer.public_key()),
                }),
                &self.payer.seed,
            )
            .await;
        meta["AffectedNodes"]
            .as_array()
            .expect("affected nodes")
            .iter()
            .find_map(|node| {
                let created = &node["CreatedNode"];
                (created["LedgerEntryType"] == "PayChannel").then(|| {
                    created["LedgerIndex"]
                        .as_str()
                        .expect("index")
                        .to_lowercase()
                })
            })
            .expect("a channel was created")
    }

    async fn payer_begins_to_leave(&self, channel: &str) {
        self.submit(
            json!({
                "TransactionType": "PaymentChannelClaim",
                "Account": self.payer.address,
                "Channel": channel,
                "Flags": TF_CLOSE,
            }),
            &self.payer.seed,
        )
        .await;
    }
}

async fn world() -> World {
    let chain = Xrpld::spawn().await;
    let rpc = chain.rpc();
    let closer = chain.close_ledgers_every(Duration::from_millis(200));
    let payer = chain.funded_wallet(&rpc, 100_000_000_000).await;
    let payer_signer = XrplSigner::from_family_seed(&payer.seed).expect("a family seed");
    World {
        chain,
        rpc,
        payer,
        payer_signer,
        _closer: closer,
    }
}

async fn answer(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("an answer");
    let status = response.status();
    let bytes = hyper::body::to_bytes(response.into_body())
        .await
        .expect("the body");
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, body)
}

async fn operator_write(
    app: &Router,
    operator: &Keypair,
    created: u64,
    path: &str,
) -> (StatusCode, Value) {
    let (sig_input, sig, digest) =
        sign_request(operator, "POST", path, &[], created, Some(9_999_999_999));
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("signature-input", sig_input)
        .header("signature", sig)
        .header("content-digest", digest)
        .body(Body::empty())
        .expect("a request");
    answer(app, request).await
}

async fn read(app: &Router, path: &str) -> Value {
    let request = Request::builder()
        .uri(path)
        .header(header::AUTHORIZATION, "Bearer operator-secret")
        .body(Body::empty())
        .expect("a request");
    let (status, body) = answer(app, request).await;
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    body
}

async fn claim_state(app: &Router, entry: Value) -> Value {
    let request = Request::builder()
        .method("POST")
        .uri("/ilp/claim-state")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "channels": [entry] }).to_string()))
        .expect("a request");
    let (status, body) = answer(app, request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["channels"][0].clone()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

#[tokio::test]
async fn a_client_opened_xrpl_channel_pays_a_node_and_the_node_lands_it() {
    if !require_xrpld() {
        return;
    }
    let world = world().await;
    let node = world.chain.funded_wallet(&world.rpc, 100_000_000).await;
    let operator = Keypair::generate(&mut OsRng);
    let (app_addr, recorded) = spawn_recording_app().await;
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
    signer_key.write_all(&[0x07; 32]).expect("write");
    let mut node_key = tempfile::NamedTempFile::new().expect("node key");
    node_key.write_all(node.seed.as_bytes()).expect("write");
    let config = support::load_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{signer_key}"

[operator]
bearer_token = "operator-secret"
write_keys = ["{write_key}"]

[settlement.xrpl.{STANDALONE_NETWORK_ID}]
rpc_url = "{rpc_url}"
min_settle_delay_secs = {SETTLE_DELAY}
redemption_safety_secs = 90

[settlement.xrpl.{STANDALONE_NETWORK_ID}.key]
key_file = "{node_key}"

[[routes]]
prefix = "{ROUTE}"
handler_url = "http://{app_addr}"
price = {PRICE}
"#,
        state_dir = state_dir.path().display(),
        signer_key = signer_key.path().display(),
        write_key = keyid_hex(&operator),
        rpc_url = world.chain.rpc_url,
        node_key = node_key.path().display(),
    ));
    let runtime = connector_cli::build(&config).await.expect("build");
    let app = connector_cli::router(&runtime, &config).expect("router");
    let receiver = runtime.signer.public_key().expect("the node's wrap key");

    let channel = world.open_channel(&node.address, 5_000).await;
    let key = format!("xrpl:{channel}");
    let pay = |json: String| {
        let app = app.clone();
        async move { post_ilp(&app, &json, paid_prepare(ROUTE, &receiver)).await }
    };
    let refusal = |bytes: axum::body::Bytes| {
        Reject::decode(&bytes)
            .expect("the voucher is refused")
            .message
    };

    // A client-opened channel pays for a request.
    let signer = &world.payer_signer;
    let answered = pay(voucher(Some(NETWORK), &channel, PRICE, signer)).await;
    Fulfill::decode(&answered)
        .unwrap_or_else(|_| panic!("the voucher paid: {:?}", Reject::decode(&answered)));
    {
        let recorded = recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1, "the write reached the app");
        let write = &recorded[0];
        assert_eq!(write.header("x-toon-chain"), Some("xrpl"));
        assert_eq!(write.header("x-toon-network"), Some(NETWORK));
        assert_eq!(write.header("x-toon-payer"), Some(key.as_str()));
        assert_eq!(write.header("x-toon-amount"), Some("100"));
    }
    let entries = FileJournal::open(state_dir.path().join(CLIENT_EDGE_JOURNAL))
        .expect("the client-edge journal")
        .read_all()
        .expect("readable");
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            JournalEntry::InboundClaimAccepted { channel_id, cumulative_amount, .. }
                if *channel_id == key && *cumulative_amount == u128::from(PRICE)
        )),
        "the voucher is journaled"
    );

    // No `network`, and a network with no table, are refused by name.
    let message = refusal(pay(voucher(None, &channel, 2 * PRICE, signer)).await);
    assert!(message.contains("'network'"), "{message}");
    let message = refusal(pay(voucher(Some("xrpl:0"), &channel, 2 * PRICE, signer)).await);
    assert!(
        message.contains("xrpl:0") && message.contains("settles"),
        "{message}"
    );

    // At the watermark, and over the ceiling (the channel holds 5000 drops).
    let at = refusal(pay(voucher(Some(NETWORK), &channel, PRICE, signer)).await);
    assert!(at.contains("advances value by 0"), "{at}");
    let over = refusal(pay(voucher(Some(NETWORK), &channel, 5_001, signer)).await);
    assert!(over.contains("could never be landed"), "{over}");

    // A client that lost its store restores the watermark from the node.
    let expires = now() + 300;
    let proof = |signer: &XrplSigner, network: &str| {
        json!({
            "blockchain": "xrpl",
            "scheme": "batch-settlement",
            "network": network,
            "channelId": channel,
            "expires": expires,
            "signature": hex::encode(
                signer
                    .sign_claim_state_challenge(network, &channel_bytes(&channel), expires)
                    .expect("a challenge"),
            ),
        })
    };
    let state = claim_state(&app, proof(signer, NETWORK)).await;
    assert_eq!(state["ok"], true, "{state}");
    assert_eq!(state["network"], NETWORK, "{state}");
    assert_eq!(state["cumulativeClaimed"], "100", "{state}");
    assert_eq!(state["maxCumulative"], "5000", "{state}");
    assert_eq!(state["available"], "4900", "{state}");
    // Not the channel key: unverified, learning nothing.
    let stranger = XrplSigner::from_family_seed(&node.seed).expect("a seed");
    let state = claim_state(&app, proof(&stranger, NETWORK)).await;
    assert_eq!(state["ok"], false, "{state}");
    assert_eq!(state["error"], "unverified", "{state}");
    // A claim signature is not a challenge signature.
    let mut replayed = proof(signer, NETWORK);
    replayed["signature"] = json!(hex::encode(
        signer
            .sign_claim(&channel_bytes(&channel), 100)
            .expect("a claim")
    ));
    assert_eq!(claim_state(&app, replayed).await["ok"], false);
    // An unconfigured network is refused by name.
    let state = claim_state(&app, proof(signer, "xrpl:0")).await;
    assert_eq!(state["error"], "network-not-settled", "{state}");

    // Pay on, then land: value moves on chain and the channel stays open.
    let answered = pay(voucher(Some(NETWORK), &channel, 3 * PRICE, signer)).await;
    Fulfill::decode(&answered).expect("a second voucher pays");
    let land = format!("/channels/{channel}/land");
    let (status, landed) = operator_write(&app, &operator, 1_000, &land).await;
    assert_eq!(status, StatusCode::OK, "{landed}");
    assert_eq!(landed["landed"], 3 * PRICE, "{landed}");
    assert_eq!(landed["status"], "open", "{landed}");
    let on_chain = world
        .rpc
        .call("ledger_entry", json!({ "index": channel }))
        .await
        .expect("the channel is still on the ledger");
    assert_eq!(on_chain["node"]["Balance"], "300", "{on_chain}");
    let listed = read(&app, "/channels").await;
    let view = listed
        .as_array()
        .expect("a list")
        .iter()
        .find(|view| view["id"] == channel.as_str())
        .unwrap_or_else(|| panic!("listed: {listed}"));
    assert_eq!(view["landed"], 300, "{view}");
    assert_eq!(view["status"], "open", "{view}");

    // The payer begins to leave and the settle delay runs out: a land is
    // accepted by the ledger and delivers nothing, which is a failure.
    let answered = pay(voucher(Some(NETWORK), &channel, 4 * PRICE, signer)).await;
    Fulfill::decode(&answered).expect("a third voucher pays");
    world.payer_begins_to_leave(&channel).await;
    world
        .chain
        .let_close_time_pass(&world.rpc, SETTLE_DELAY + 2)
        .await;
    let (status, failed) = operator_write(&app, &operator, 1_001, &land).await;
    assert!(!status.is_success(), "{status}: {failed}");
    assert!(
        failed.to_string().contains("delivered nothing"),
        "a land that delivered nothing is reported as one: {failed}"
    );
}
