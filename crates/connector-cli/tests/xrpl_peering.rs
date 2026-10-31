//! ADR 0087, a peering over XRPL (#1616): two config-driven nodes -- built by
//! `connector_cli::build`, served by `connector_cli::router` -- each given one
//! authenticated write, `POST /peers`, naming the other's URL, over one
//! spawned `xrpld` (ADR 0084).
//!
//! Nothing here hands a node a pre-opened channel or a sponsor: an XRPL
//! channel is opened by its own payer's submission (there is no sponsor
//! endpoint on XRPL), so what the tests read back is the ledger's own
//! `PayChannel` entries.
//!
//! What it holds:
//!
//! 1. A peering is two `PayChannel`s on one network, one opened by each side,
//!    each paying the other's account under the payer's own key, and packets
//!    cross it more than once, the payee's claim journal holding the vouchers.
//! 2. `POST /channels/:id/fund` raises the collateral, and one the account
//!    cannot afford is refused naming the shortfall.
//! 3. A payer that lost its outbound journal's vouchers recovers its watermark
//!    from the peer's `POST /ilp/claim-state`.
//! 4. `POST /channels/:id/withdraw` twice -- before and after `Expiration` --
//!    ends the channel and refunds the payer.
//!
//! A missing Docker fails CI and skips locally (`require_xrpld`).

mod support;

use std::io::Write;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::routing::post;
use axum::Router;
use chrono::{Duration as ChronoDuration, Utc};
use connector_chain_rpc::xrpl::XrplRpc;
use connector_domain::{EnvelopeRequest, EnvelopeResponse, Fulfill, Prepare, Reject};
use connector_operator::test_support::sign_request;
use connector_settlement_xrpl::test_support::{
    require_xrpld, LedgerCloser, Wallet, Xrpld, STANDALONE_NETWORK_ID,
};
use connector_signer::giftwrap::{open_response, seal_request};
use connector_signer::{PublicKeyBytes, XrplSigner};
use ed25519_dalek::Keypair;
use rand::rngs::OsRng;
use serde_json::{json, Value};
use tower::ServiceExt;

const NETWORK: &str = "xrpl:1000";
/// What each node's own app route charges, and so what every covering
/// voucher must advance the payee's watermark by.
const APP_PRICE: u64 = 1_000;
/// The fee each node's peering with the other retains per packet (ADR 0061).
const PEER_FEE: u64 = 50;
const ROUTE_PRICE: u64 = APP_PRICE + PEER_FEE;
const AMOUNT: u64 = APP_PRICE;
/// The opening deposit each node puts behind its own outbound channel, in
/// drops.
const DEPOSIT: u64 = 20_000;
/// Just above the config's 901-second floor: the stand-alone clock only moves
/// when it is closed, so the exit closes ledgers until it runs out.
const SETTLE_DELAY: u64 = 1_000;
/// Each node's settlement account, funded from genesis.
const FUNDED: u64 = 100_000_000;

static NEXT_CREATED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(3_000);

fn signed(keypair: &Keypair, method: Method, path: &str, body: Vec<u8>) -> Request<Body> {
    let created = NEXT_CREATED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (sig_input, sig, digest) = sign_request(
        keypair,
        method.as_str(),
        path,
        &body,
        created,
        Some(9_999_999_999),
    );
    Request::builder()
        .method(method)
        .uri(path)
        .header("signature-input", sig_input)
        .header("signature", sig)
        .header("content-digest", digest)
        .body(Body::from(body))
        .unwrap()
}

fn bearer_get(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header("authorization", "Bearer operator-secret")
        .body(Body::empty())
        .unwrap()
}

fn write_key_hex(keypair: &Keypair) -> String {
    keypair
        .public
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn answer(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.expect("an answer");
    let status = response.status();
    let bytes = hyper::body::to_bytes(response.into_body())
        .await
        .expect("the body");
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, body)
}

/// A PREPARE to `destination` carrying `amount`, sealed to `receiver`
/// (ADR 0018), with the secret its answer is opened with.
fn sealed_prepare(
    destination: &str,
    amount: u64,
    receiver: &PublicKeyBytes,
    body: &[u8],
) -> (Prepare, [u8; 32]) {
    let plaintext = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: body.to_vec(),
    }
    .encode();
    let (data, shared_secret) = seal_request(&plaintext, receiver).expect("seal");
    (
        Prepare {
            amount,
            expires_at: Utc::now() + ChronoDuration::minutes(5),
            greeting: false,
            destination: destination.to_string(),
            data,
        },
        shared_secret,
    )
}

/// An app answering every write with 200 "delivered".
fn spawn_app() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind app");
    let addr = listener.local_addr().expect("app addr");
    let app = Router::new().route("/", post(|| async { "delivered" }));
    tokio::spawn(async move {
        let _ = axum::Server::from_tcp(listener)
            .expect("serve the app")
            .serve(app.into_make_service())
            .await;
    });
    addr.to_string()
}

/// A router behind a socket that can be swapped for another: how a node
/// **restarts** here.
#[derive(Clone)]
struct Swappable(Arc<Mutex<Router>>);

impl Swappable {
    fn serve(listener: std::net::TcpListener, router: Router) -> Swappable {
        let swappable = Swappable(Arc::new(Mutex::new(router)));
        let inner = swappable.clone();
        let service = tower::service_fn(move |request: Request<Body>| {
            let router = inner.0.lock().expect("router lock").clone();
            async move { router.oneshot(request).await }
        });
        tokio::spawn(async move {
            let _ = axum::Server::from_tcp(listener)
                .expect("serve the bound listener")
                .serve(tower::make::Shared::new(service))
                .await;
        });
        swappable
    }

    fn router(&self) -> Router {
        self.0.lock().expect("router lock").clone()
    }

    fn swap(&self, router: Router) {
        *self.0.lock().expect("router lock") = router;
    }
}

/// One config-driven node on a real socket.
struct Node {
    name: &'static str,
    wallet: Wallet,
    addr: SocketAddr,
    operator: Keypair,
    state_dir: tempfile::TempDir,
    _signer_key: tempfile::NamedTempFile,
    _settlement_key: tempfile::NamedTempFile,
    config: connector_config::Config,
    runtime: connector_cli::Runtime,
    socket: Swappable,
}

impl Node {
    async fn boot(name: &'static str, seed: u8, rpc_url: &str, wallet: Wallet, app: &str) -> Node {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the node");
        let addr = listener.local_addr().expect("the node's address");
        let operator = Keypair::generate(&mut OsRng);
        let state_dir = tempfile::tempdir().expect("state dir");
        let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
        signer_key.write_all(&[seed; 32]).expect("write");
        let mut settlement_key = tempfile::NamedTempFile::new().expect("settlement key");
        settlement_key
            .write_all(wallet.seed.as_bytes())
            .expect("write");
        let config = support::load_config(&format!(
            r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"
peer_allow_plaintext_endpoints = true
peer_expose = "http"

[node]
addresses     = ["g.example.{name}"]
http_endpoint = "http://{addr}/ilp"

[signer]
key_file = "{signer_key}"

[operator]
bearer_token = "operator-secret"
write_keys = ["{write_key}"]

[[routes]]
prefix = "g.example.{name}.app"
handler_url = "http://{app}/"
price = {APP_PRICE}

[settlement.xrpl.{STANDALONE_NETWORK_ID}]
rpc_url = "{rpc_url}"
min_settle_delay_secs = {SETTLE_DELAY}
redemption_safety_secs = 90

[settlement.xrpl.{STANDALONE_NETWORK_ID}.key]
key_file = "{settlement_key}"
"#,
            state_dir = state_dir.path().display(),
            signer_key = signer_key.path().display(),
            settlement_key = settlement_key.path().display(),
            write_key = write_key_hex(&operator),
        ));
        let runtime = connector_cli::build(&config).await.expect("build the node");
        let router = connector_cli::router(&runtime, &config).expect("the node's router");
        let socket = Swappable::serve(listener, router);
        Node {
            name,
            wallet,
            addr,
            operator,
            state_dir,
            _signer_key: signer_key,
            _settlement_key: settlement_key,
            config,
            runtime,
            socket,
        }
    }

    async fn restart(&mut self) {
        self.runtime.state_dir_lock.take();
        self.runtime = connector_cli::build(&self.config)
            .await
            .expect("rebuild the node");
        self.socket
            .swap(connector_cli::router(&self.runtime, &self.config).expect("the node's router"));
    }

    fn url(&self) -> String {
        format!("http://{}/ilp", self.addr)
    }

    fn outbound_journal(&self) -> std::path::PathBuf {
        self.state_dir.path().join("outbound-channels.log")
    }

    fn channel_key(&self) -> [u8; 33] {
        XrplSigner::from_family_seed(&self.wallet.seed)
            .expect("a family seed")
            .public_key()
    }

    fn identity(&self) -> PublicKeyBytes {
        self.runtime
            .signer
            .public_key()
            .expect("a local signer has a public key")
    }

    async fn write_answer(&self, method: Method, path: &str, body: Value) -> (StatusCode, Value) {
        answer(
            &self.socket.router(),
            signed(
                &self.operator,
                method,
                path,
                serde_json::to_vec(&body).expect("json"),
            ),
        )
        .await
    }

    async fn write(&self, method: Method, path: &str, body: Value) -> Value {
        let (status, answer) = self.write_answer(method, path, body).await;
        assert_eq!(status, StatusCode::OK, "{} {path}: {answer}", self.name);
        answer
    }

    async fn read(&self, path: &str) -> Value {
        let (status, answer) = answer(&self.socket.router(), bearer_get(path)).await;
        assert_eq!(status, StatusCode::OK, "{} {path}: {answer}", self.name);
        answer
    }

    fn peer_body(other: &Node, fee: u64, deposit: Option<u64>) -> Value {
        let mut body = json!({
            "id": other.name,
            "url": other.url(),
            "fee": fee,
            "max_packet_amount": 5_000,
        });
        if let Some(deposit) = deposit {
            body["deposit"] = json!(deposit);
        }
        body
    }

    async fn peer_with(&self, other: &Node, fee: u64) -> Value {
        self.write(
            Method::POST,
            "/peers",
            Node::peer_body(other, fee, Some(DEPOSIT)),
        )
        .await
    }

    async fn route_to(&self, other: &Node) {
        self.write(
            Method::POST,
            "/routes/peers",
            json!({
                "prefix": format!("g.example.{}", other.name),
                "peer_id": other.name,
                "price": ROUTE_PRICE,
            }),
        )
        .await;
    }

    async fn originate(&self, payee: &Node, amount: u64, body: &[u8]) -> Result<Vec<u8>, Reject> {
        let (prepare, shared_secret) = sealed_prepare(
            &format!("g.example.{}.app", payee.name),
            amount,
            &payee.identity(),
            body,
        );
        let response = self
            .socket
            .router()
            .oneshot(signed(
                &self.operator,
                Method::POST,
                "/packets",
                prepare.encode(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        match Fulfill::decode(&bytes) {
            Ok(fulfill) => {
                let opened =
                    open_response(&shared_secret, &fulfill.data).expect("open the response");
                let envelope = EnvelopeResponse::decode(&opened).expect("an envelope");
                assert_eq!(envelope.status, 200);
                Ok(envelope.body)
            }
            Err(_) => Err(Reject::decode(&bytes).expect("a FULFILL or a REJECT")),
        }
    }

    async fn channel_row(&self, id: &str, direction: &str) -> Option<Value> {
        self.read("/channels")
            .await
            .as_array()
            .expect("a list")
            .iter()
            .find(|row| row["id"] == id && row["direction"] == direction)
            .cloned()
    }

    async fn inbound_watermark(&self, id: &str) -> u64 {
        self.channel_row(id, "inbound")
            .await
            .map_or(0, |row| row["watermark"].as_u64().expect("a watermark"))
    }

    /// Every voucher this node's client-edge journal recorded on channel
    /// `id`, in order.
    fn journaled(&self, id: &str) -> Vec<u64> {
        let journal =
            std::fs::read_to_string(self.state_dir.path().join(support::CLIENT_EDGE_JOURNAL))
                .unwrap_or_default();
        journal
            .lines()
            .filter(|line| line.starts_with("inbound_claim_accepted\t"))
            .filter_map(|line| {
                let fields: Vec<&str> = line.split('\t').collect();
                (fields[1] == format!("xrpl:{id}")).then(|| fields[3].parse().expect("an amount"))
            })
            .collect()
    }
}

fn channel_of(established: &Value) -> String {
    established["channel"]["id"]
        .as_str()
        .expect("the channel's id")
        .to_string()
}

/// A channel's ledger entry, `None` once the ledger no longer holds it.
async fn ledger_channel(rpc: &XrplRpc, id: &str) -> Option<Value> {
    rpc.call("ledger_entry", json!({ "index": id }))
        .await
        .ok()
        .map(|entry| entry["node"].clone())
}

async fn balance(rpc: &XrplRpc, address: &str) -> u64 {
    rpc.account_info(address)
        .await
        .expect("account_info")
        .expect("the account exists")
        .balance_drops
}

struct World {
    chain: Xrpld,
    rpc: XrplRpc,
    a: Node,
    b: Node,
    _closer: LedgerCloser,
}

async fn world() -> World {
    let chain = Xrpld::spawn().await;
    let rpc = chain.rpc();
    let closer = chain.close_ledgers_every(Duration::from_millis(300));
    let app = spawn_app();
    let a_wallet = chain.funded_wallet(&rpc, FUNDED).await;
    let b_wallet = chain.funded_wallet(&rpc, FUNDED).await;
    let a = Node::boot("nodea", 0x61, &chain.rpc_url, a_wallet, &app).await;
    let b = Node::boot("nodeb", 0x62, &chain.rpc_url, b_wallet, &app).await;
    World {
        chain,
        rpc,
        a,
        b,
        _closer: closer,
    }
}

#[tokio::test]
async fn two_nodes_peer_over_two_xrpl_channels() {
    if !require_xrpld() {
        return;
    }
    let World {
        chain,
        rpc,
        a,
        mut b,
        _closer,
    } = world().await;

    // ── Each self-description publishes its node's channel key ──────────
    let described = a.read("/ilp").await;
    assert_eq!(described["batchSettlements"][0]["network"], NETWORK);
    assert_eq!(
        described["voucherSigners"],
        json!([{ "network": NETWORK, "signer": hex::encode(a.channel_key()) }]),
        "{described}"
    );

    // ── No channel yet and no deposit named: refused before any spend ───
    let (status, refused) = b
        .write_answer(Method::POST, "/peers", Node::peer_body(&a, PEER_FEE, None))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert!(refused.to_string().contains("deposit"), "{refused}");

    // ── Each node writes `POST /peers` naming the other ─────────────────
    let b_established = b.peer_with(&a, PEER_FEE).await;
    let a_established = a.peer_with(&b, PEER_FEE).await;
    for established in [&a_established, &b_established] {
        assert_eq!(established["channel"]["status"], "created");
        assert_eq!(established["channel"]["chain"], "xrpl");
        assert_eq!(established["channel"]["network"], NETWORK);
        assert_eq!(established["fee"], PEER_FEE);
    }
    let b_to_a = channel_of(&b_established);
    let a_to_b = channel_of(&a_established);
    assert_ne!(b_to_a, a_to_b, "a peering is two channels, one each way");

    // Each is a `PayChannel` its payer opened and funded on one network,
    // paying the other's account under the payer's own key.
    for (id, payer, receiver) in [(&b_to_a, &b, &a), (&a_to_b, &a, &b)] {
        let entry = ledger_channel(&rpc, id)
            .await
            .unwrap_or_else(|| panic!("{id} is on the ledger"));
        assert_eq!(entry["LedgerEntryType"], "PayChannel", "{id}");
        assert_eq!(entry["Account"], payer.wallet.address.as_str(), "{id}");
        assert_eq!(
            entry["Destination"],
            receiver.wallet.address.as_str(),
            "{id}"
        );
        assert_eq!(entry["Amount"], DEPOSIT.to_string(), "{id}");
        assert_eq!(
            entry["PublicKey"],
            hex::encode_upper(payer.channel_key()),
            "{id}: the payer's settlement key signs its vouchers"
        );
        assert!(entry.get("CancelAfter").is_none(), "{id}: no CancelAfter");
    }
    b.route_to(&a).await;
    a.route_to(&b).await;

    // ── B pays A, twice: each forward carries a voucher ─────────────────
    for (crossing, body) in [(1, b"first".as_slice()), (2, b"second".as_slice())] {
        let delivered = b
            .originate(&a, AMOUNT, body)
            .await
            .unwrap_or_else(|reject| panic!("B→A crossing {crossing}: {reject:?}"));
        assert_eq!(delivered, b"delivered");
        assert_eq!(
            a.inbound_watermark(&b_to_a).await,
            crossing * APP_PRICE,
            "A's watermark on B's channel advanced by exactly what reached A"
        );
    }
    assert_eq!(a.journaled(&b_to_a), vec![APP_PRICE, 2 * APP_PRICE]);

    // ── A pays B, on its own channel ─────────────────────────────────────
    a.originate(&b, AMOUNT, b"back the other way")
        .await
        .unwrap_or_else(|reject| panic!("A→B: {reject:?}"));
    assert_eq!(b.inbound_watermark(&a_to_b).await, APP_PRICE);
    assert_eq!(b.journaled(&a_to_b), vec![APP_PRICE]);

    // ── A repeat of the write finds its channel ─────────────────────────
    let repeated = b.peer_with(&a, PEER_FEE).await;
    assert_eq!(repeated["channel"]["status"], "found");
    assert_eq!(repeated["channel"]["id"], b_to_a.as_str());

    // ── /fund raises the collateral; an unaffordable one names the gap ──
    let fund = format!("/channels/{b_to_a}/fund");
    let funded = b
        .write(Method::POST, &fund, json!({ "amount": 5_000 }))
        .await;
    assert_eq!(funded["collateral"], DEPOSIT + 5_000, "{funded}");
    let on_chain = ledger_channel(&rpc, &b_to_a).await.expect("the channel");
    assert_eq!(on_chain["Amount"], (DEPOSIT + 5_000).to_string());
    let (status, refused) = b
        .write_answer(Method::POST, &fund, json!({ "amount": 10 * FUNDED }))
        .await;
    assert!(!status.is_success(), "{status}: {refused}");
    assert!(
        refused.to_string().contains("drops short"),
        "the refusal names the shortfall: {refused}"
    );
    assert_eq!(
        ledger_channel(&rpc, &b_to_a).await.expect("the channel")["Amount"],
        (DEPOSIT + 5_000).to_string(),
        "a refused fund moved nothing"
    );

    // ── B's journal loses its vouchers: claim-state recovers it ─────────
    let b_before = std::fs::read(b.outbound_journal()).expect("B's outbound journal");
    b.originate(&a, AMOUNT, b"a third")
        .await
        .unwrap_or_else(|reject| panic!("B→A third: {reject:?}"));
    assert_eq!(a.inbound_watermark(&b_to_a).await, 3 * APP_PRICE);
    std::fs::write(b.outbound_journal(), &b_before).expect("B's journal goes back");
    b.restart().await;
    b.originate(&a, AMOUNT, b"after B lost its journal")
        .await
        .unwrap_or_else(|reject| panic!("B→A after B lost its journal: {reject:?}"));
    assert_eq!(
        a.inbound_watermark(&b_to_a).await,
        4 * APP_PRICE,
        "B asked A where its channel stands and signed past it"
    );
    assert_eq!(
        a.journaled(&b_to_a),
        vec![APP_PRICE, 2 * APP_PRICE, 3 * APP_PRICE, 4 * APP_PRICE]
    );

    // ── /withdraw, twice: tfClose, then the refund after Expiration ─────
    // On a channel toward a bare account, so nothing answers the exit before
    // the delay runs out: a peer node's own watcher lands its voucher with
    // `tfClose` the moment it sees `Expiration` (ADR 0087 decision 8), which
    // is held in `connector-settlement-xrpl`'s watch tests and ends a
    // channel before this payer's second call.
    let bystander = chain.funded_wallet(&rpc, FUNDED).await;
    let mut terms = serde_json::to_value(&a.runtime.batch_settlements[0]).expect("A's terms");
    terms["payTo"] = json!(bystander.address);
    let opened = b
        .write(
            Method::POST,
            "/channels",
            json!({ "terms": terms, "deposit": DEPOSIT }),
        )
        .await;
    assert_eq!(opened["direction"], "outbound", "{opened}");
    assert_eq!(opened["collateral"], DEPOSIT, "{opened}");
    let id = opened["id"].as_str().expect("an id").to_string();
    let before = balance(&rpc, &b.wallet.address).await;
    let withdraw = format!("/channels/{id}/withdraw");
    let started = b.write(Method::POST, &withdraw, json!({})).await;
    assert_eq!(started["step"], "started", "{started}");
    assert_eq!(started["status"], "closing", "{started}");
    let entry = ledger_channel(&rpc, &id)
        .await
        .expect("the channel stays until the delay runs out");
    assert!(entry.get("Expiration").is_some(), "{entry}");
    chain.let_close_time_pass(&rpc, SETTLE_DELAY + 2).await;
    let finished = b.write(Method::POST, &withdraw, json!({})).await;
    assert_eq!(finished["step"], "finished", "{finished}");
    assert!(
        ledger_channel(&rpc, &id).await.is_none(),
        "the channel is gone"
    );
    let refunded = balance(&rpc, &b.wallet.address).await - before;
    assert!(
        refunded <= DEPOSIT && refunded > DEPOSIT - 1_000,
        "the payer is refunded its deposit less the two fees: {refunded}"
    );
}
