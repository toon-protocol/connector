//! **A config-declared peering on x402 channels, paying both ways** (ADR
//! 0075 decisions 4, 5, 6 and 9; issue #1380), against a real chain.
//!
//! Two config-driven nodes -- built by `connector_cli::build` and served by
//! `connector_cli::router`, the production boot path -- peer through their
//! config files alone: each names the other in `[[peers]]`, binds the
//! other's settlement address as the voucher signer in `[[peer_channels]]`,
//! and names **its own** outbound x402 channel toward the other in
//! `[[pay_channels]]`. A peering is two one-way channels, and neither node
//! pastes the other's channel id anywhere.
//!
//! The channels themselves are opened the way an operator opens one before
//! writing the row: `POST /channels` on each node, with the other's
//! published terms, under the `state_dir` the configured node then boots
//! from. Nothing here hands a node a pre-opened channel or a settlement
//! backend.
//!
//! The claims this file holds:
//!
//! 1. **The peering forwards and is paid in both directions**, over both
//!    carriages: every forward carries a voucher on the payer's own outbound
//!    channel, the payee journals it against the channel's one watermark,
//!    and each voucher advances it by exactly what was forwarded (the price;
//!    the operator's own packet pays its node no fee -- ADR 0061, #1466).
//! 2. **A payer whose outbound journal lost its latest vouchers recovers
//!    from the next hop's `POST /ilp/claim-state`** (decision 6): restored
//!    from a backup taken right after the open, it asks the payee where the
//!    channel stands before it signs again, and its next voucher advances
//!    past the payee's watermark -- the packet is delivered rather than
//!    refused as not advancing.
//! 3. **No `TokenNetwork` channel is opened for any of it** -- asserted
//!    against the chain.

mod support;

use std::io::Write;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::routing::post;
use axum::Router;
use chrono::{Duration as ChronoDuration, Utc};
use connector_domain::{EnvelopeRequest, EnvelopeResponse, Fulfill, Prepare, Reject};
use connector_operator::test_support::sign_request;
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

/// This test binary's own base port for [`Anvil::spawn`], clear of every
/// other binary's range.
const ANVIL_BASE_PORT: u16 = 23_700;
/// What each node's app route charges, and so what every covering voucher
/// must advance the payee's watermark by.
const APP_PRICE: u64 = 1_000;
/// The fee each node's peering with the other retains per packet (ADR
/// 0061). Non-zero so "advanced by exactly the forwarded amount" measures
/// the fee rather than the request.
const PEER_FEE: u64 = 50;
/// What a client of one node pays for a route to the other: the app's price
/// plus the peering's fee.
const ROUTE_PRICE: u64 = APP_PRICE + PEER_FEE;
/// What a packet originated at one node carries. An operator's own packet
/// pays its node no fee (ADR 0061, #1466), so all of it is forwarded and
/// this is exactly what reaches the other node.
const AMOUNT: u64 = APP_PRICE;
/// Each node's opening deposit on its own outbound channel.
const DEPOSIT: u128 = 10_000;
/// Each node's settlement account's USDC.
const FUNDED: u128 = 1_000_000;

static NEXT_CREATED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(5_000);

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

fn address_of(key: &str) -> Address {
    LocalWallet::from_bytes(&hex32(key)).expect("key").address()
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

fn sealed_prepare(
    destination: &str,
    amount: u64,
    receiver: &PublicKeyBytes,
) -> (Prepare, [u8; 32]) {
    let plaintext = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: b"over the configured peering".to_vec(),
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

/// One chain: anvil, x402 placed, a FiatToken USDC, and both settlement
/// accounts holding [`FUNDED`] USDC.
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
/// restarts here, and how it comes back with a new config file.
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
    signer_key: tempfile::NamedTempFile,
    settlement_key_file: tempfile::NamedTempFile,
    carriage: Carriage,
    app: String,
    runtime: Option<connector_cli::Runtime>,
    socket: Option<Swappable>,
}

impl Node {
    /// Node `name`, settling as `settlement_key`, on `listener`'s address,
    /// with a priced app route under `g.example.<name>.app`, serving and not
    /// yet peered.
    async fn boot(
        name: &'static str,
        settlement_key: &'static str,
        signer_seed: u8,
        listener: std::net::TcpListener,
        carriage: Carriage,
        app: &str,
        chain: &Chain,
    ) -> Node {
        let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
        signer_key.write_all(&[signer_seed; 32]).expect("write");
        let mut settlement_key_file = tempfile::NamedTempFile::new().expect("settlement key");
        settlement_key_file
            .write_all(settlement_key.as_bytes())
            .expect("write");
        let mut node = Node {
            name,
            settlement_key,
            addr: listener.local_addr().expect("the node's address"),
            operator: Keypair::generate(&mut OsRng),
            state_dir: tempfile::tempdir().expect("state dir"),
            signer_key,
            settlement_key_file,
            carriage,
            app: app.to_string(),
            runtime: None,
            socket: None,
        };
        let config = support::load_config(&node.config_text(chain, ""));
        let runtime = connector_cli::build(&config).await.expect("build the node");
        let router = connector_cli::router(&runtime, &config).expect("the node's router");
        node.runtime = Some(runtime);
        node.socket = Some(Swappable::serve(listener, router));
        node
    }

    /// This node's config file: the node itself, and `peering`.
    fn config_text(&self, chain: &Chain, peering: &str) -> String {
        let (expose, btp_endpoint) = match self.carriage {
            Carriage::Http => ("http", String::new()),
            Carriage::Btp => (
                "btp",
                format!("btp_endpoint = \"ws://{}/ilp/btp\"", self.addr),
            ),
        };
        format!(
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
{peering}
[settlement.evm]
rpc_url = "{rpc_url}"
token_address = "{token:?}"
decimals = 6
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[settlement.evm.key]
key_file = "{settlement_key}"

"#,
            name = self.name,
            addr = self.addr,
            app = self.app,
            state_dir = self.state_dir.path().display(),
            signer_key = self.signer_key.path().display(),
            settlement_key = self.settlement_key_file.path().display(),
            write_key = write_key_hex(&self.operator),
            rpc_url = chain.anvil.rpc_url,
            token = chain.token,
        )
    }

    /// The rows that make `other` this node's peering, paying on `channel`
    /// -- this node's own outbound channel toward it.
    fn peering_with(&self, other: &Node, channel: &str) -> String {
        let endpoint = match other.carriage {
            Carriage::Http => format!("http://{}/ilp", other.addr),
            Carriage::Btp => format!("ws://{}/ilp/btp", other.addr),
        };
        format!(
            r#"
[[routes]]
prefix = "g.example.{other}"
peer_id = "{other}"
price = {ROUTE_PRICE}

[[peers]]
id = "{other}"
endpoint = "{endpoint}"
fee = {PEER_FEE}

[[peer_channels]]
peer_id = "{other}"
voucher_signer = "{signer}"

[[pay_channels]]
peer_id = "{other}"
outbound_channel = "{channel}"
client_edge_url = "http://{other_addr}/ilp"
"#,
            other = other.name,
            signer = spelled(other.settlement_address()),
            other_addr = other.addr,
        )
    }

    /// Restart from `config_text`, through the production boot path, over
    /// the same `state_dir`, on the same socket.
    async fn restart_with(&mut self, config_text: &str) {
        let config = support::load_config(config_text);
        let runtime = connector_cli::build(&config)
            .await
            .expect("rebuild the node from its peering config");
        self.socket
            .as_ref()
            .expect("serving")
            .swap(connector_cli::router(&runtime, &config).expect("the node's router"));
        self.runtime = Some(runtime);
    }

    fn runtime(&self) -> &connector_cli::Runtime {
        self.runtime.as_ref().expect("built")
    }

    fn router(&self) -> Router {
        self.socket.as_ref().expect("serving").router()
    }

    fn settlement_address(&self) -> Address {
        address_of(self.settlement_key)
    }

    fn identity(&self) -> PublicKeyBytes {
        self.runtime()
            .signer
            .public_key()
            .expect("a local signer has a public key")
    }

    fn outbound_journal(&self) -> std::path::PathBuf {
        self.state_dir.path().join("outbound-channels.log")
    }

    /// `POST /channels`: open this node's own outbound channel toward
    /// `other`, on the terms `other` publishes (ADR 0075 decision 11).
    async fn open_channel_toward(&self, other: &Node) -> String {
        let (status, described) = answer(
            &other.router(),
            Request::get("/ilp").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{described}");
        let terms = described["batchSettlements"][0].clone();
        let (status, opened) = answer(
            &self.router(),
            signed(
                &self.operator,
                Method::POST,
                "/channels",
                serde_json::to_vec(&serde_json::json!({ "terms": terms, "deposit": DEPOSIT }))
                    .expect("json"),
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{} POST /channels: {opened}",
            self.name
        );
        assert_eq!(opened["direction"], "outbound");
        opened["id"].as_str().expect("an id").to_string()
    }

    /// Originate one packet over `POST /packets`, sealed to `payee`.
    async fn originate(&self, payee: &Node, amount: u64) -> Result<Vec<u8>, Reject> {
        let (prepare, shared_secret) = sealed_prepare(
            &format!("g.example.{}.app", payee.name),
            amount,
            &payee.identity(),
        );
        let response = self
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
                let opened = open_response(&shared_secret, &fulfill.data).expect("open");
                let envelope = EnvelopeResponse::decode(&opened).expect("an envelope");
                assert_eq!(envelope.status, 200);
                Ok(envelope.body)
            }
            Err(_) => Err(Reject::decode(&bytes).expect("a FULFILL or a REJECT")),
        }
    }

    /// The highest voucher this node accepted on inbound channel `id`, as
    /// `GET /channels` reports it.
    async fn inbound_watermark(&self, id: &str) -> u64 {
        let (status, channels) = answer(&self.router(), bearer_get("/channels")).await;
        assert_eq!(status, StatusCode::OK, "{channels}");
        channels
            .as_array()
            .expect("a list")
            .iter()
            .find(|row| row["id"] == id && row["direction"] == "inbound")
            .map_or(0, |row| row["watermark"].as_u64().expect("a watermark"))
    }

    /// Every voucher this node's client-edge journal recorded on `id`, in
    /// order: a peer's vouchers are journaled beside a client's, against
    /// the channel's one watermark (`peer-carriage-spec.md` §1.8).
    fn journaled(&self, id: &str) -> Vec<u64> {
        std::fs::read_to_string(self.state_dir.path().join(support::CLIENT_EDGE_JOURNAL))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with("inbound_claim_accepted\t"))
            .filter_map(|line| {
                let fields: Vec<&str> = line.split('\t').collect();
                (fields[1] == format!("evm:{id}")).then(|| fields[3].parse().expect("an amount"))
            })
            .collect()
    }
}

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

#[tokio::test]
async fn a_config_declared_peering_pays_both_ways_on_x402_over_http() {
    a_config_declared_peering_pays_both_ways(Carriage::Http, 0).await;
}

#[tokio::test]
async fn a_config_declared_peering_pays_both_ways_on_x402_over_btp() {
    a_config_declared_peering_pays_both_ways(Carriage::Btp, 10).await;
}

async fn a_config_declared_peering_pays_both_ways(carriage: Carriage, offset: u16) {
    // `require_anvil`, not a bare availability check: it panics when `CI`
    // is set and skips only on a developer machine without Foundry.
    if !require_anvil() {
        return;
    }
    let chain = Chain::spawn(offset).await;
    let app = spawn_app();
    let a_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind A");
    let b_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind B");
    let mut a = Node::boot(
        "nodea",
        COUNTERPARTY_PRIVATE_KEY,
        0x0a,
        a_listener,
        carriage,
        &app,
        &chain,
    )
    .await;
    let mut b = Node::boot(
        "nodeb",
        DEPLOYER_PRIVATE_KEY,
        0x0b,
        b_listener,
        carriage,
        &app,
        &chain,
    )
    .await;

    // ── Each operator opens its own outbound channel, then writes the rows ─
    let a_to_b = a.open_channel_toward(&b).await;
    let b_to_a = b.open_channel_toward(&a).await;
    assert_ne!(a_to_b, b_to_a, "a peering is two channels, one each way");
    // What a backup of B's outbound journal taken right after the open
    // holds: the channel, and no voucher signed on it.
    let b_backup = std::fs::read(b.outbound_journal()).expect("B's outbound journal");
    let a_config = a.config_text(&chain, &a.peering_with(&b, &a_to_b));
    let b_config = b.config_text(&chain, &b.peering_with(&a, &b_to_a));
    a.restart_with(&a_config).await;
    b.restart_with(&b_config).await;

    // ── B pays A, twice: each forward carries a voucher on B's channel ───
    for crossing in 1..=2u64 {
        let delivered = b
            .originate(&a, AMOUNT)
            .await
            .unwrap_or_else(|reject| panic!("B→A crossing {crossing}: {reject:?}"));
        assert_eq!(delivered, b"delivered");
        assert_eq!(
            a.inbound_watermark(&b_to_a).await,
            crossing * APP_PRICE,
            "A's watermark on B's channel advanced by exactly what B forwarded: the packet \
             carried {AMOUNT}, B charged its own operator's packet no fee, and {APP_PRICE} \
             reached A"
        );
    }
    assert_eq!(a.journaled(&b_to_a), vec![APP_PRICE, 2 * APP_PRICE]);

    // ── A pays B, on A's own channel ─────────────────────────────────────
    a.originate(&b, AMOUNT)
        .await
        .unwrap_or_else(|reject| panic!("A→B: {reject:?}"));
    assert_eq!(b.inbound_watermark(&a_to_b).await, APP_PRICE);
    assert_eq!(b.journaled(&a_to_b), vec![APP_PRICE]);

    // Each funded only its own channel, on x402, and nothing on
    // `TokenNetwork`.
    for (channel, deposit) in [(&a_to_b, DEPOSIT), (&b_to_a, DEPOSIT)] {
        assert_eq!(
            chain.x402.channel(&ChannelId(channel.clone())).await.0,
            deposit
        );
    }

    // ── B's journal loses its vouchers: claim-state recovers it ──────────
    // Over HTTP, where each request reaches whichever process holds the
    // socket; a BTP session dialled before a restart would still reach the
    // old process here, which the swappable socket cannot sever.
    if let Carriage::Http = carriage {
        std::fs::write(b.outbound_journal(), &b_backup).expect("restore B's backup");
        b.restart_with(&b_config).await;
        let restored = b
            .runtime()
            .outbound_channels
            .as_ref()
            .expect("B pays on x402")
            .signed(&b_to_a)
            .expect("B's channel is journaled");
        assert!(
            restored < u128::from(2 * APP_PRICE),
            "the restored journal is behind A's watermark: {restored}"
        );
        b.originate(&a, AMOUNT)
            .await
            .unwrap_or_else(|reject| panic!("B→A after B lost its journal: {reject:?}"));
        assert_eq!(
            a.inbound_watermark(&b_to_a).await,
            3 * APP_PRICE,
            "B asked A where its channel stands and signed past it, rather than a voucher A \
             refuses as not advancing"
        );
        assert_eq!(
            a.journaled(&b_to_a),
            vec![APP_PRICE, 2 * APP_PRICE, 3 * APP_PRICE]
        );
    }
}
