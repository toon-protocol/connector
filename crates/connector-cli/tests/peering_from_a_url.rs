//! **A peering established from a URL, against a real chain** (ADR 0058,
//! as ADR 0075 decision 4 amends it; issues #1160, #1378).
//!
//! Nothing here injects a settlement backend or hands a node a pre-opened
//! channel. Config-driven nodes -- built by `connector_cli::build` and
//! served by `connector_cli::router`, the production boot path -- are given
//! one authenticated write each, `POST /peers { id, url, fee,
//! max_packet_amount, deposit }`, pointed at a real self-description on a
//! real socket, over one disposable `anvil` holding x402's real
//! `x402BatchSettlement` at its canonical address and a Circle FiatToken as
//! USDC.
//!
//! The claims this file exists to hold:
//!
//! 1. **A peering is two one-way x402 channels, one opened by each side.**
//!    `POST /peers` opens and funds only this node's outbound channel, and
//!    never a `TokenNetwork` channel -- asserted against the chain.
//! 2. **The other half is admitted, not configured.** Each node binds the
//!    peer's channel toward it by the voucher signer the peer's
//!    self-description publishes; nobody exchanges a channel id.
//! 3. **Every forwarded PREPARE carries a voucher, both ways, over both
//!    carriages**, each payee journals it, and its verdict rides back in the
//!    ack. A packet that moves no value carries the peer-role challenge
//!    instead and is still attributed to the peering.
//! 4. **The peering survives a restart of either node**, with both
//!    watermarks restored.
//! 5. **The endpoint is safely retryable**: a repeat finds this node's own
//!    channel rather than opening a second, and says which branch it took.
//! 6. **Trust-on-first-use.** Whatever the URL serves is who the peering is
//!    with.

mod support;

use std::io::Write;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::{Duration as ChronoDuration, Utc};
use connector_domain::x402::{X402BatchSettlementEvmTerms, X402BatchSettlementTerms};
use connector_domain::{
    EdgeIdentity, EnvelopeRequest, EnvelopeResponse, Fulfill, NodeFacts, NodeSelfDescription,
    Prepare, Reject, VoucherSignerFact,
};
use connector_operator::test_support::sign_request;
use connector_runtime::PeerView;
use connector_settlement::batch::{ChannelPresentation, Voucher};
use connector_settlement::ChannelId;
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{
    require_anvil, Anvil, COUNTERPARTY_PRIVATE_KEY, DEPLOYER_PRIVATE_KEY,
};
use connector_signer::giftwrap::{open_response, seal_request};
use connector_signer::PublicKeyBytes;
use ed25519_dalek::Keypair;
use ethers::signers::{LocalWallet, Signer as _};
use ethers::types::Address;
use rand::rngs::OsRng;
use tower::ServiceExt;

/// This test binary's own base port for [`Anvil::spawn`]. Every other test
/// binary that spawns one has its own base, so binaries running
/// concurrently under `cargo test --workspace` never contend for a port.
const ANVIL_BASE_PORT: u16 = 19_000;

/// What each node's own app route charges, and so what every covering
/// voucher must advance the payee's watermark by.
const APP_PRICE: u64 = 1_000;
/// The fee each node's peering with the other retains per packet (ADR
/// 0061). Non-zero on purpose: it is what makes "advanced by exactly the
/// forwarded amount" a measurement of the fee rather than of the request.
const PEER_FEE: u64 = 50;
/// What a client of one node pays for a route to the other: the app's price
/// plus the peering's fee (ADR 0010, ADR 0028).
const ROUTE_PRICE: u64 = APP_PRICE + PEER_FEE;
/// What a packet originated at one node carries. An operator's own packet
/// pays its node no fee (ADR 0061, #1466), so all of it is forwarded and
/// this is exactly what reaches the other node.
const AMOUNT: u64 = APP_PRICE;
/// The opening deposit each node puts behind its own outbound channel.
const DEPOSIT: u128 = 10_000;
/// Each node's settlement account's USDC.
const FUNDED: u128 = 1_000_000;

/// A distinct `created` per signed request: the operator surface rejects a
/// replayed signature (ADR 0008's #1067 amendment).
static NEXT_CREATED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(2_000);

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

fn hex32(text: &str) -> [u8; 32] {
    let text = text.trim_start_matches("0x");
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).expect("hex");
    }
    out
}

fn wallet_of(key: &str) -> LocalWallet {
    LocalWallet::from_bytes(&hex32(key)).expect("key")
}

fn address_of(key: &str) -> Address {
    wallet_of(key).address()
}

fn spelled(address: Address) -> String {
    format!("{address:#x}")
}

fn write_key_hex(keypair: &Keypair) -> String {
    keypair
        .public
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn answer(router: &Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let response = router.clone().oneshot(request).await.expect("an answer");
    let status = response.status();
    let bytes = hyper::body::to_bytes(response.into_body())
        .await
        .expect("the body");
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&bytes).into()));
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

/// One chain for the whole test: anvil, x402 placed, a FiatToken USDC, and
/// both nodes' settlement accounts holding [`FUNDED`] USDC.
struct Chain {
    anvil: Anvil,
    x402: X402Chain,
    token: Address,
}

impl Chain {
    async fn spawn(offset: u16) -> Chain {
        let anvil = Anvil::spawn(ANVIL_BASE_PORT + offset).await;
        let mut x402 = X402Chain::place(&anvil.rpc_url).await;
        let token = x402.deploy_fiat_token().await;
        for key in [DEPLOYER_PRIVATE_KEY, COUNTERPARTY_PRIVATE_KEY] {
            x402.mint(token, address_of(key), FUNDED).await;
        }
        Chain { anvil, x402, token }
    }
}

/// A router behind a socket that can be swapped for another: how a node
/// **restarts** here. A new runtime is built from the same config file and
/// `state_dir` through the production boot path and takes over the socket
/// the peer's durable row names; the old one stops being reached.
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

/// Which carriage the two nodes peer over: each exposes and publishes
/// that one, and `POST /peers` dials whatever the other publishes.
#[derive(Clone, Copy, Debug)]
enum Carriage {
    Http,
    Btp,
}

/// One config-driven node on a real socket: its key files, its state, its
/// config, and its router behind a swappable socket.
struct Node {
    name: &'static str,
    settlement_key: &'static str,
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
    /// Node `name` settling as `settlement_key`, answering to
    /// `g.example.<name>`, with a priced app route under it and a free one
    /// **pinned to the carriage it does not peer over**: a client arriving
    /// over the peering's carriage is refused that route (ADR 0072), and a
    /// peer is not held to a client's pin -- which is how a zero-value
    /// packet's attribution is observable below.
    async fn boot(
        name: &'static str,
        settlement_key: &'static str,
        signer_seed: u8,
        chain: &Chain,
        carriage: Carriage,
        app: &str,
    ) -> Node {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the node");
        let addr = listener.local_addr().expect("the node's address");
        let operator = Keypair::generate(&mut OsRng);
        let state_dir = tempfile::tempdir().expect("state dir");
        let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
        signer_key.write_all(&[signer_seed; 32]).expect("write");
        let mut settlement_key_file = tempfile::NamedTempFile::new().expect("settlement key");
        settlement_key_file
            .write_all(settlement_key.as_bytes())
            .expect("write");
        let (expose, btp_endpoint, pinned) = match carriage {
            Carriage::Http => ("http", String::new(), "btp"),
            Carriage::Btp => (
                "btp",
                format!("btp_endpoint = \"ws://{addr}/ilp/btp\""),
                "http",
            ),
        };
        let config = support::load_config(&format!(
            r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"
peer_allow_plaintext_endpoints = true
peer_expose = "{expose}"

[node]
addresses     = ["g.example.{name}"]
http_endpoint = "http://{addr}/ilp"
{btp_endpoint}

[signer]
key_file = "{signer_key}"

[operator]
bearer_token = "operator-secret"
write_keys = ["{write_key}"]

[[routes]]
prefix = "g.example.{name}.app"
handler_url = "http://{app}/"
price = {APP_PRICE}

[[routes]]
prefix = "g.example.{name}.pinned"
handler_url = "http://{app}/pinned"
price = 0
transport = "{pinned}"

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
            settlement_key = settlement_key_file.path().display(),
            write_key = write_key_hex(&operator),
            rpc_url = chain.anvil.rpc_url,
            token = chain.token,
        ));
        let runtime = connector_cli::build(&config).await.expect("build the node");
        let router = connector_cli::router(&runtime, &config).expect("the node's router");
        let socket = Swappable::serve(listener, router);
        Node {
            name,
            settlement_key,
            addr,
            operator,
            state_dir,
            _signer_key: signer_key,
            _settlement_key: settlement_key_file,
            config,
            runtime,
            socket,
        }
    }

    /// Restart from the same config file and `state_dir`, through the
    /// production boot path, on the same socket.
    async fn restart(&mut self) {
        self.runtime = connector_cli::build(&self.config)
            .await
            .expect("rebuild the node");
        self.socket
            .swap(connector_cli::router(&self.runtime, &self.config).expect("the node's router"));
    }

    fn url(&self) -> String {
        format!("http://{}/ilp", self.addr)
    }

    fn settlement_address(&self) -> Address {
        address_of(self.settlement_key)
    }

    /// The key a payload for this node is sealed to (ADR 0018).
    fn identity(&self) -> PublicKeyBytes {
        self.runtime
            .signer
            .public_key()
            .expect("a local signer has a public key")
    }

    async fn write(
        &self,
        method: Method,
        path: &str,
        body: serde_json::Value,
    ) -> serde_json::Value {
        let (status, answer) = answer(
            &self.socket.router(),
            signed(
                &self.operator,
                method,
                path,
                serde_json::to_vec(&body).expect("json"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{} {path}: {answer}", self.name);
        answer
    }

    async fn read(&self, path: &str) -> serde_json::Value {
        let (status, answer) = answer(&self.socket.router(), bearer_get(path)).await;
        assert_eq!(status, StatusCode::OK, "{} {path}: {answer}", self.name);
        answer
    }

    /// `POST /peers` naming `other`, at `fee`, with this node's own opening
    /// deposit.
    async fn peer_with(&self, other: &Node, fee: u64) -> serde_json::Value {
        self.write(
            Method::POST,
            "/peers",
            serde_json::json!({
                "id": other.name,
                "url": other.url(),
                "fee": fee,
                "max_packet_amount": 5_000,
                "deposit": DEPOSIT,
            }),
        )
        .await
    }

    /// Route everything under `other`'s addresses to the peering with it.
    async fn route_to(&self, other: &Node) {
        self.write(
            Method::POST,
            "/routes/peers",
            serde_json::json!({
                "prefix": format!("g.example.{}", other.name),
                "peer_id": other.name,
                "price": ROUTE_PRICE,
            }),
        )
        .await;
    }

    /// Originate one packet over `POST /packets`, sealed to `payee`: the
    /// app's own answer, or the reject.
    async fn originate(
        &self,
        payee: &Node,
        route: &str,
        amount: u64,
        body: &[u8],
    ) -> Result<Vec<u8>, Reject> {
        let (prepare, shared_secret) = sealed_prepare(
            &format!("g.example.{}.{route}", payee.name),
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

    /// The highest voucher this node accepted on inbound channel `id`, as
    /// `GET /channels` reports it; zero for one it has accepted none on.
    async fn inbound_watermark(&self, id: &str) -> u64 {
        let channels = self.read("/channels").await;
        channels
            .as_array()
            .expect("a list")
            .iter()
            .find(|row| row["id"] == id && row["direction"] == "inbound")
            .map_or(0, |row| row["watermark"].as_u64().expect("a watermark"))
    }

    /// Every voucher this node's client-edge journal recorded on channel
    /// `id`, in order: the durable record a restart replays and a landing
    /// submits (ADR 0005). A voucher channel keeps one watermark whichever
    /// role its vouchers arrive under (`peer-carriage-spec.md` §1.8), so a
    /// peer's are journaled beside a client's.
    fn journaled(&self, id: &str) -> Vec<u64> {
        let journal =
            std::fs::read_to_string(self.state_dir.path().join(support::CLIENT_EDGE_JOURNAL))
                .unwrap_or_default();
        journal
            .lines()
            .filter(|line| line.starts_with("inbound_claim_accepted\t"))
            .filter_map(|line| {
                let fields: Vec<&str> = line.split('\t').collect();
                (fields[1] == format!("evm:{id}")).then(|| fields[3].parse().expect("an amount"))
            })
            .collect()
    }
}

fn channel_of(established: &serde_json::Value) -> String {
    established["channel"]["id"]
        .as_str()
        .expect("the channel's id")
        .to_string()
}

/// An app answering every write with 200 "delivered".
fn spawn_app() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind app");
    let addr = listener.local_addr().expect("app addr");
    let app = Router::new()
        .route("/", post(|| async { "delivered" }))
        .route("/pinned", post(|| async { "delivered" }));
    tokio::spawn(async move {
        let _ = axum::Server::from_tcp(listener)
            .expect("serve the app")
            .serve(app.into_make_service())
            .await;
    });
    addr.to_string()
}

// ─────────────────────────────────────────────────────────────────────────
// Seam 2: two nodes, each `POST /peers` the other.
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn two_nodes_peer_over_two_x402_channels_over_http() {
    two_nodes_peer_over_two_x402_channels(Carriage::Http, 40).await;
}

#[tokio::test]
async fn two_nodes_peer_over_two_x402_channels_over_btp() {
    two_nodes_peer_over_two_x402_channels(Carriage::Btp, 50).await;
}

async fn two_nodes_peer_over_two_x402_channels(carriage: Carriage, offset: u16) {
    // `require_anvil`, not a bare availability check: it panics when `CI`
    // is set and skips only on a developer machine without Foundry.
    if !require_anvil() {
        return;
    }
    let chain = Chain::spawn(offset).await;
    let app = spawn_app();
    let mut a = Node::boot(
        "nodea",
        COUNTERPARTY_PRIVATE_KEY,
        0x0a,
        &chain,
        carriage,
        &app,
    )
    .await;
    let mut b = Node::boot("nodeb", DEPLOYER_PRIVATE_KEY, 0x0b, &chain, carriage, &app).await;

    // ── Each self-description publishes its node's voucher signer ───────
    // (ADR 0075 decision 10): the EVM settlement address, which every
    // channel the node opens names as `payerAuthorizer`.
    let described = a.read("/ilp").await;
    assert_eq!(
        described["voucherSigners"],
        serde_json::json!([{
            "network": format!("eip155:{}", chain.x402.chain_id()),
            "signer": spelled(a.settlement_address()),
        }]),
        "{described}"
    );

    // ── Each node writes `POST /peers` naming the other ─────────────────
    let b_established = b.peer_with(&a, PEER_FEE).await;
    let a_established = a.peer_with(&b, PEER_FEE).await;
    for established in [&a_established, &b_established] {
        assert_eq!(established["channel"]["status"], "created");
        assert_eq!(established["channel"]["chain"], "evm");
        assert_eq!(established["fee"], PEER_FEE);
    }
    let b_to_a = channel_of(&b_established);
    let a_to_b = channel_of(&a_established);
    assert_ne!(b_to_a, a_to_b, "a peering is two channels, one each way");
    // Each opened and funded only its own, on x402, and nothing on
    // `TokenNetwork` (ADR 0075 decision 4).
    assert_eq!(
        chain.x402.channel(&ChannelId(b_to_a.clone())).await,
        (DEPOSIT, 0)
    );
    assert_eq!(
        chain.x402.channel(&ChannelId(a_to_b.clone())).await,
        (DEPOSIT, 0)
    );
    for node in [&a, &b] {
        assert_eq!(
            chain
                .x402
                .balance_of(chain.token, node.settlement_address())
                .await,
            FUNDED - DEPOSIT,
            "{} funded its own channel and nothing else",
            node.name
        );
    }
    b.route_to(&a).await;
    a.route_to(&b).await;

    // ── B pays A, twice: each forward carries a voucher ─────────────────
    for (crossing, body) in [(1, b"first".as_slice()), (2, b"second".as_slice())] {
        let delivered = b
            .originate(&a, "app", AMOUNT, body)
            .await
            .unwrap_or_else(|reject| panic!("B→A crossing {crossing}: {reject:?}"));
        assert_eq!(delivered, b"delivered");
        assert_eq!(
            a.inbound_watermark(&b_to_a).await,
            crossing * APP_PRICE,
            "A's watermark on B's channel advanced by exactly what B forwarded: the packet \
             carried {AMOUNT}, B charged its own operator's packet no fee (ADR 0061, #1466), and {APP_PRICE} reached A"
        );
    }
    assert_eq!(a.journaled(&b_to_a), vec![APP_PRICE, 2 * APP_PRICE]);

    // ── A pays B, on its own channel ─────────────────────────────────────
    a.originate(&b, "app", AMOUNT, b"back the other way")
        .await
        .unwrap_or_else(|reject| panic!("A→B: {reject:?}"));
    assert_eq!(b.inbound_watermark(&a_to_b).await, APP_PRICE);
    assert_eq!(b.journaled(&a_to_b), vec![APP_PRICE]);

    // ── A zero-value packet carries the challenge, and no voucher ───────
    // B reprices its peering to no fee -- a repeat of the write, which finds
    // its channel and opens nothing. (An originated packet pays no fee, so
    // this no longer decides whether the packet is refused.)
    let repriced = b.peer_with(&a, 0).await;
    assert_eq!(repriced["channel"]["status"], "found");
    assert_eq!(repriced["channel"]["id"], b_to_a.as_str());
    if let Carriage::Http = carriage {
        // Control: the same zero-value packet from a client, over HTTP, is
        // refused the route pinned to BTP.
        let (prepare, _) =
            sealed_prepare("g.example.nodea.pinned", 0, &a.identity(), b"from a client");
        let response = a
            .socket
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ilp")
                    .body(Body::from(prepare.encode()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        assert!(
            Fulfill::decode(&bytes).is_err(),
            "control: a client is refused the pinned route over this carriage"
        );
    }
    // Delivered over the carriage the route is NOT pinned to: only a peer
    // is not held to that pin, and with no voucher on the packet only the
    // peer-role challenge can have made it one (ADR 0075 decision 5).
    b.originate(&a, "pinned", 0, b"no value")
        .await
        .unwrap_or_else(|reject| {
            panic!("a zero-value packet is attributed to the peering by the challenge: {reject:?}")
        });
    assert_eq!(
        a.journaled(&b_to_a),
        vec![APP_PRICE, 2 * APP_PRICE],
        "a zero-value packet carries no voucher and journals nothing"
    );
    b.peer_with(&a, PEER_FEE).await;

    // ── Each voucher's verdict rides back in the ack ────────────────────
    if let Carriage::Http = carriage {
        the_ack_carries_each_vouchers_verdict(&chain, &a, &b, &b_to_a).await;
    }
    let settled = a.inbound_watermark(&b_to_a).await;

    // ── The payer restarts, and pays on from its restored watermark ─────
    b.restart().await;
    b.originate(&a, "app", AMOUNT, b"after the payer restarted")
        .await
        .unwrap_or_else(|reject| panic!("B→A after B restarted: {reject:?}"));
    assert_eq!(a.inbound_watermark(&b_to_a).await, settled + APP_PRICE);

    // ── The payee restarts, and judges from its restored watermark ──────
    // Over HTTP, where each request reaches whichever process holds the
    // socket; a BTP session dialled before a restart would still reach the
    // old one here, which the swappable socket cannot sever.
    if let Carriage::Http = carriage {
        a.restart().await;
        b.originate(&a, "app", AMOUNT, b"after the payee restarted")
            .await
            .unwrap_or_else(|reject| panic!("B→A after A restarted: {reject:?}"));
        assert_eq!(a.inbound_watermark(&b_to_a).await, settled + 2 * APP_PRICE);
        // ...and, restarted, pays too: its outbound watermark came back with
        // its channel.
        a.originate(&b, "app", AMOUNT, b"from the restarted node")
            .await
            .unwrap_or_else(|reject| panic!("A→B after A restarted: {reject:?}"));
        assert_eq!(b.inbound_watermark(&a_to_b).await, 2 * APP_PRICE);
    }

    // ── One peering, whatever the retries ───────────────────────────────
    let peers: Vec<PeerView> = serde_json::from_value(b.read("/peers").await).unwrap();
    assert_eq!(peers.len(), 1, "{peers:?}");
    assert_eq!(peers[0].fee, PEER_FEE);
    assert_eq!(
        chain.x402.channel(&ChannelId(b_to_a)).await.0,
        DEPOSIT,
        "no repeat deposited anything more"
    );
}

/// §6.1 over ILP-over-HTTP: the ack answers the voucher, independently of
/// the packet. B's next voucher on its channel toward A is accepted; the
/// same bytes again are accepted and advance nothing (§6.3's resend), so the
/// packet they ride is refused as uncovered; and a voucher below the
/// channel's watermark is refused `amount_not_advancing`.
async fn the_ack_carries_each_vouchers_verdict(chain: &Chain, a: &Node, b: &Node, b_to_a: &str) {
    let outbound = b.runtime.outbound_channels.clone().expect("B pays on x402");
    let presentation = outbound.presentation(b_to_a).expect("B's channel");
    assert!(matches!(presentation, ChannelPresentation::Evm { .. }));
    let stale = outbound.signed(b_to_a).expect("B's channel");
    let voucher = outbound
        .sign_voucher(b_to_a, stale + u128::from(APP_PRICE))
        .await
        .expect("sign");
    let as_json = |voucher: &Voucher| {
        connector_runtime::voucher_json(
            &presentation,
            voucher,
            &spelled(b.settlement_address()),
            "2026-09-28T00:00:00.000Z",
        )
    };
    let send = |json: String, body: &'static [u8]| {
        let (prepare, _) = sealed_prepare("g.example.nodea.app", APP_PRICE, &a.identity(), body);
        let router = a.socket.router();
        async move {
            let response = router
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/ilp")
                        .header("ilp-payment-channel-claim", BASE64.encode(json))
                        .body(Body::from(prepare.encode()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let ack = response
                .headers()
                .get("toon-claim-ack")
                .map(|value| {
                    String::from_utf8(BASE64.decode(value.as_bytes()).expect("base64"))
                        .expect("utf-8")
                })
                .expect("a peer's voucher is acknowledged");
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            (ack, Fulfill::decode(&bytes).is_ok())
        }
    };

    let (ack, delivered) = send(as_json(&voucher), b"acked").await;
    assert!(ack.contains("\"accepted\""), "{ack}");
    assert!(delivered, "a fresh voucher covering the price delivers");

    let (ack, delivered) = send(as_json(&voucher), b"resent").await;
    assert!(
        ack.contains("\"accepted\""),
        "a byte-identical resend is accepted: {ack}"
    );
    assert!(!delivered, "...and pays for nothing new");

    // A genuine voucher by B's key, below the channel's one watermark.
    let below = Voucher {
        cumulative_amount: stale,
        signature: chain.x402.sign_voucher(
            &wallet_of(b.settlement_key),
            &ChannelId(b_to_a.to_string()),
            stale,
        ),
    };
    let (ack, delivered) = send(as_json(&below), b"stale").await;
    assert!(
        ack.contains("\"rejected\"") && ack.contains("amount_not_advancing"),
        "{ack}"
    );
    assert!(!delivered);
}

// ─────────────────────────────────────────────────────────────────────────
// The write itself, against a served document.
// ─────────────────────────────────────────────────────────────────────────

/// A **real** self-description on a **real** socket, exactly as ADR 0050
/// says a connector answers a `GET` on its own URL with: x402 terms naming
/// `receiver`, and `receiver` as the voucher signer. Nothing signs it and
/// nothing vouches for it: whoever answers the URL the operator named is who
/// the peering is with.
fn serve_self_description(chain: &Chain, receiver: Address) -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let network = format!("eip155:{}", chain.x402.chain_id());
    let document = NodeSelfDescription::describe(
        &NodeFacts {
            ilp_addresses: vec!["g.example.counterparty".to_string()],
            http_endpoint: Some(format!("http://{addr}/ilp")),
            btp_endpoint: None,
            peer_carriages: vec!["http".to_string()],
            batch_settlements: vec![X402BatchSettlementTerms::Evm(X402BatchSettlementEvmTerms {
                network: network.clone(),
                asset: spelled(chain.token),
                pay_to: spelled(receiver),
                receiver_authorizer: spelled(receiver),
                min_withdraw_delay_secs: 86_400,
                name: "USDC".to_string(),
                version: "2".to_string(),
                asset_transfer_method: Default::default(),
                facilitator: None,
            })],
            voucher_signers: vec![VoucherSignerFact {
                network,
                signer: spelled(receiver),
            }],
        },
        // A secp256k1 edge identity, deliberately a different value from
        // the settlement address above: the two are not interchangeable.
        Some(EdgeIdentity {
            key_id: "counterparty-edge-key".to_string(),
            public_key: "0x04".to_string() + &"cd".repeat(64),
        }),
        Vec::new(),
        None,
    );
    let app = Router::new().route(
        "/ilp",
        get(move || {
            let document = document.clone();
            async move { Json(document) }
        }),
    );
    tokio::spawn(async move {
        let _ = axum::Server::from_tcp(listener)
            .expect("serve the bound listener")
            .serve(app.into_make_service())
            .await;
    });
    addr
}

/// **One operator write opens this node's own channel, and repeating it
/// finds that channel** (ADR 0075 decisions 4 and 8). The endpoint spends
/// gas, so a repeat must be a success rather than a second channel -- and
/// the answer says which branch it took.
#[tokio::test]
async fn one_operator_write_opens_this_nodes_own_channel_and_repeating_it_finds_it() {
    if !require_anvil() {
        return;
    }
    let chain = Chain::spawn(0).await;
    let app = spawn_app();
    let node = Node::boot(
        "nodeb",
        DEPLOYER_PRIVATE_KEY,
        0x0b,
        &chain,
        Carriage::Http,
        &app,
    )
    .await;
    let counterparty = address_of(COUNTERPARTY_PRIVATE_KEY);
    let served = serve_self_description(&chain, counterparty);
    let body = serde_json::json!({
        "id": "apex-relay-2",
        "url": format!("http://{served}/ilp"),
        "fee": 100,
        "max_packet_amount": 5_000,
    });

    // No channel toward the counterparty yet, and no deposit named: refused
    // by name, before anything is spent.
    let (status, refused) = answer(
        &node.socket.router(),
        signed(
            &node.operator,
            Method::POST,
            "/peers",
            serde_json::to_vec(&body).unwrap(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert!(refused.to_string().contains("deposit"), "{refused}");

    let mut with_deposit = body.clone();
    with_deposit["deposit"] = serde_json::json!(DEPOSIT);
    let established = node
        .write(Method::POST, "/peers", with_deposit.clone())
        .await;
    assert_eq!(established["id"], "apex-relay-2");
    assert_eq!(established["source"], "runtime");
    assert_eq!(established["fee"], 100);
    assert_eq!(established["max_packet_amount"], 5_000);
    assert_eq!(established["channel"]["status"], "created");
    assert_eq!(established["channel"]["chain"], "evm");
    let channel_id = channel_of(&established);

    // The channel is this node's, toward the published receiver, on x402 --
    // read off the chain and off this node's own channel list.
    assert_eq!(
        chain.x402.channel(&ChannelId(channel_id.clone())).await,
        (DEPOSIT, 0)
    );
    let channels = node.read("/channels").await;
    let row = channels
        .as_array()
        .expect("a list")
        .iter()
        .find(|row| row["id"] == channel_id.as_str())
        .expect("the channel is listed");
    assert_eq!(row["direction"], "outbound");
    assert_eq!(row["counterparty"], spelled(counterparty));

    // Repeating the identical request finds it, and deposits nothing more.
    let repeated = node.write(Method::POST, "/peers", with_deposit).await;
    assert_eq!(repeated["channel"]["status"], "found");
    assert_eq!(repeated["channel"]["id"], channel_id.as_str());
    assert_eq!(
        chain.x402.channel(&ChannelId(channel_id)).await,
        (DEPOSIT, 0)
    );
    assert_eq!(
        chain
            .x402
            .balance_of(chain.token, node.settlement_address())
            .await,
        FUNDED - DEPOSIT
    );

    let peers: Vec<PeerView> = serde_json::from_value(node.read("/peers").await).unwrap();
    assert_eq!(peers.len(), 1, "one write, one peering: {peers:?}");

    // A route through it is a second, separate write, accepted because the
    // peering has a channel to pay from (ADR 0042's load rule, at runtime).
    node.write(
        Method::POST,
        "/routes/peers",
        serde_json::json!({
            "prefix": "g.example.counterparty",
            "peer_id": "apex-relay-2",
            "price": 1_100,
        }),
    )
    .await;
}

/// The document is taken as served, and nothing in the request is compared
/// against it: a document publishing a **stranger** -- an address nobody
/// here holds a key for -- gets a channel opened toward that stranger. That
/// is trust-on-first-use (ADR 0058, unchanged by ADR 0075 decision 4), and
/// it is asserted so a later change that quietly adds a pin fails a test
/// rather than passing one.
#[tokio::test]
async fn whatever_the_url_serves_is_who_the_peering_is_with() {
    if !require_anvil() {
        return;
    }
    let chain = Chain::spawn(10).await;
    let app = spawn_app();
    let node = Node::boot(
        "nodeb",
        DEPLOYER_PRIVATE_KEY,
        0x0b,
        &chain,
        Carriage::Http,
        &app,
    )
    .await;
    let stranger = Address::from([0x5a; 20]);
    let served = serve_self_description(&chain, stranger);
    let established = node
        .write(
            Method::POST,
            "/peers",
            serde_json::json!({
                "id": "whoever-answers",
                "url": format!("http://{served}/ilp"),
                "deposit": DEPOSIT,
            }),
        )
        .await;
    assert_eq!(established["channel"]["status"], "created");
    let channel_id = channel_of(&established);

    let channels = node.read("/channels").await;
    let row = channels
        .as_array()
        .expect("a list")
        .iter()
        .find(|row| row["id"] == channel_id.as_str())
        .expect("the channel is listed");
    assert_eq!(
        row["counterparty"],
        spelled(stranger),
        "the receiver is whoever the URL said, and the operator's vetting of that URL is the \
         whole of the assurance"
    );
    assert_ne!(
        row["counterparty"],
        spelled(node.settlement_address()),
        "and never this node itself"
    );
}

/// Issue #1220's case, on x402: an HTTP-only node -- `peer_expose =
/// "http"`, `[node] http_endpoint` set, no `btp_endpoint` -- publishes a
/// self-description a stranger can dial, with its voucher signer in it (ADR
/// 0075 decision 10), and a counterparty's `POST /peers` against it lands
/// over that one carriage.
#[tokio::test]
async fn an_http_only_nodes_self_description_is_dialable_and_a_counterparty_peers_with_it() {
    if !require_anvil() {
        return;
    }
    let chain = Chain::spawn(20).await;
    let app = spawn_app();
    let a = Node::boot(
        "nodea",
        COUNTERPARTY_PRIVATE_KEY,
        0x0a,
        &chain,
        Carriage::Http,
        &app,
    )
    .await;
    let b = Node::boot(
        "nodeb",
        DEPLOYER_PRIVATE_KEY,
        0x0b,
        &chain,
        Carriage::Http,
        &app,
    )
    .await;

    let document = a.read("/ilp").await;
    assert_eq!(document["httpEndpoint"], serde_json::json!(a.url()));
    assert!(
        document.get("btpEndpoint").is_none(),
        "an HTTP-only node must publish no btpEndpoint at all, not a null one: {document}"
    );
    assert_eq!(document["peerCarriages"], serde_json::json!(["http"]));
    assert_eq!(
        document["voucherSigners"][0]["signer"],
        spelled(a.settlement_address()),
        "{document}"
    );

    let established = b.peer_with(&a, PEER_FEE).await;
    assert_eq!(established["id"], "nodea");
    assert_eq!(established["channel"]["status"], "created");
    assert_eq!(established["channel"]["chain"], "evm");
}

/// The near-miss ADR 0050 gives a name to: `POST /peers` takes a
/// connector's self-description URL, not its origin. No chain is needed --
/// `establish_peering` fails while fetching the self-description, before it
/// ever reaches settlement -- so this asserts only the 502 and its hint
/// (issue #1219).
#[tokio::test]
async fn an_origin_without_ilp_answers_502_naming_the_fix() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::Server::from_tcp(listener)
            .expect("serve the bound listener")
            .serve(Router::new().into_make_service())
            .await;
    });

    let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
    key_file
        .write_all(DEPLOYER_PRIVATE_KEY.as_bytes())
        .expect("write key file");
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let keypair = Keypair::generate(&mut OsRng);
    let config = support::load_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"
peer_allow_plaintext_endpoints = true

[signer]
key_file = "{key_path}"

[operator]
bearer_token = "operator-secret"
write_keys = ["{write_key_hex}"]
"#,
        state_dir = state_dir.path().display(),
        key_path = key_file.path().display(),
        write_key_hex = write_key_hex(&keypair),
    ));
    let runtime = connector_cli::build(&config)
        .await
        .expect("a node with no settlement backend at all");
    let router = connector_cli::router(&runtime, &config).expect("router");

    let body = serde_json::to_vec(&serde_json::json!({
        "id": "near-miss",
        "url": format!("http://{addr}"),
        "fee": 100,
        "max_packet_amount": 5_000,
    }))
    .unwrap();
    let response = router
        .oneshot(signed(&keypair, Method::POST, "/peers", body))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::BAD_GATEWAY,
        "the counterparty's host answered 404, which is the counterparty's problem"
    );
    let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
    let message = String::from_utf8(bytes.to_vec()).expect("a UTF-8 error body");
    assert!(
        message.contains("/ilp"),
        "the 502 must name the fix -- POST /peers takes the self-description URL: {message}"
    );
}
