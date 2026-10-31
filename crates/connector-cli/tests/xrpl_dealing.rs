//! ADR 0087 decision 6 and ADR 0071, dealing over XRPL (#1616): a node whose
//! one peering holds USDC on anvil and whose other holds XRP on a spawned
//! `xrpld` converts at the rate it declares, because a channel holds XRP only
//! and XRPL has no rate source -- the rate is a static `[[rates]]` row with a
//! spread.
//!
//! Three config-driven nodes: A settles on anvil, C on `xrpld`, and B on both
//! and **deals**. A pays B in USDC base units; B pays C in drops.
//!
//! * a USDC→XRP forward credits C the **converted** amount -- the declared
//!   rate less the spread, less the peering's flat fee in the outgoing unit
//!   -- and not the amount A sent;
//! * a forward the other way, XRP→USDC, has no `[[rates]]` row and is
//!   refused rather than passed through at par.
//!
//! A forward between two XRPL networks is a forward between two assets
//! (ADR 0087 decision 6.5): the stand-alone harness serves one network id
//! only, so that is held in `connector-config`'s own tests instead.
//!
//! A missing Docker or anvil fails CI and skips locally.

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
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{
    require_anvil, Anvil, COUNTERPARTY_PRIVATE_KEY, DEPLOYER_PRIVATE_KEY,
};
use connector_settlement_xrpl::test_support::{
    require_xrpld, LedgerCloser, Wallet, Xrpld, STANDALONE_NETWORK_ID,
};
use connector_signer::giftwrap::{open_response, seal_request};
use connector_signer::PublicKeyBytes;
use ed25519_dalek::Keypair;
use rand::rngs::OsRng;
use serde_json::{json, Value};
use tower::ServiceExt;

const NETWORK: &str = "xrpl:1000";
/// This test binary's own base port for [`Anvil::spawn`], clear of every
/// other binary's range.
const ANVIL_BASE_PORT: u16 = 25_700;
/// What A's packet carries into B, in USDC base units, and what B's route
/// toward C charges: the INCOMING unit (ADR 0071 decision 1).
const AMOUNT: u64 = 10_000;
/// B's declared rate, USDC base unit -> drop: one micro-USDC buys two drops.
const RATE: (u64, u64) = (2, 1);
/// B's spread, applied against the sender: 2/1 at 1/100 deals at 198/100.
const SPREAD: (u64, u64) = (1, 100);
/// B's flat fee on its peering with C, in the OUTGOING unit, drops (ADR 0071
/// decision 1 amends ADR 0061 in exactly that clause).
const B_C_FEE: u64 = 300;
/// What reaches C per crossing: `floor(10_000 * 198 / 100) - 300`.
const CONVERTED: u64 = AMOUNT * (RATE.0 * (SPREAD.1 - SPREAD.0)) / (RATE.1 * SPREAD.1) - B_C_FEE;
/// The USDC opening deposit of an EVM channel, and the drops of an XRPL one.
const USDC_DEPOSIT: u64 = 100_000;
const DROPS_DEPOSIT: u64 = 1_000_000;
const SETTLE_DELAY: u64 = 1_000;
const XRP_FUNDED: u64 = 100_000_000;
const USDC_FUNDED: u128 = 1_000_000;
const MAX_PACKET: u64 = 1_000_000;

static NEXT_CREATED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(7_000);

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
}

fn hex32(text: &str) -> [u8; 32] {
    let text = text.trim_start_matches("0x");
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).expect("hex");
    }
    out
}

fn address_of(key: &str) -> ethers::types::Address {
    use ethers::signers::Signer as _;
    ethers::signers::LocalWallet::from_bytes(&hex32(key))
        .expect("key")
        .address()
}

/// What a node settles on: anvil's USDC, XRP on `xrpld`, or both.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Settles {
    Evm,
    Xrpl,
    Both,
}

/// Everything the three nodes share: one anvil with x402 placed and a
/// FiatToken USDC, one `xrpld`, and the funded accounts.
struct Chains {
    anvil: Anvil,
    token: ethers::types::Address,
    xrpld: Xrpld,
    rpc: XrplRpc,
    /// An app answering every write with 200 "delivered", on its own socket.
    app: String,
    _closer: LedgerCloser,
}

impl Chains {
    async fn spawn() -> Chains {
        let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
        let mut x402 = X402Chain::place(&anvil.rpc_url).await;
        let token = x402.deploy_fiat_token().await;
        for key in [DEPLOYER_PRIVATE_KEY, COUNTERPARTY_PRIVATE_KEY] {
            x402.mint(token, address_of(key), USDC_FUNDED).await;
        }
        let xrpld = Xrpld::spawn().await;
        let rpc = xrpld.rpc();
        let closer = xrpld.close_ledgers_every(Duration::from_millis(300));
        Chains {
            anvil,
            token,
            xrpld,
            rpc,
            app: spawn_app(),
            _closer: closer,
        }
    }
}

/// One config-driven node on a real socket.
struct Node {
    name: &'static str,
    addr: SocketAddr,
    operator: Keypair,
    _state_dir: tempfile::TempDir,
    _signer_key: tempfile::NamedTempFile,
    _evm_key: tempfile::NamedTempFile,
    _xrpl_key: tempfile::NamedTempFile,
    runtime: connector_cli::Runtime,
    socket: Swappable,
}

impl Node {
    /// Node `name`, answering to `g.example.<name>`, settling as `settles`
    /// says. `dealing` is B's `[[tokens]]`, `[[rates]]` and `[rate_guards]`.
    async fn boot(
        name: &'static str,
        signer_seed: u8,
        settles: Settles,
        evm_key: &str,
        wallet: Option<&Wallet>,
        chains: &Chains,
        dealing: &str,
    ) -> Node {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the node");
        let addr = listener.local_addr().expect("the node's address");
        let operator = Keypair::generate(&mut OsRng);
        let state_dir = tempfile::tempdir().expect("state dir");
        let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
        signer_key.write_all(&[signer_seed; 32]).expect("write");
        let mut evm_file = tempfile::NamedTempFile::new().expect("evm key");
        evm_file.write_all(evm_key.as_bytes()).expect("write");
        let mut xrpl_file = tempfile::NamedTempFile::new().expect("xrpl key");
        if let Some(wallet) = wallet {
            xrpl_file.write_all(wallet.seed.as_bytes()).expect("write");
        }
        let mut settlement = String::new();
        if settles != Settles::Xrpl {
            settlement.push_str(&format!(
                r#"
[settlement.eip155.31337]
rpc_url = "{rpc_url}"
token_address = "{token:?}"
decimals = 6
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[settlement.eip155.31337.key]
key_file = "{evm_key}"
"#,
                rpc_url = chains.anvil.rpc_url,
                token = chains.token,
                evm_key = evm_file.path().display(),
            ));
        }
        if settles != Settles::Evm {
            settlement.push_str(&format!(
                r#"
[settlement.xrpl.{STANDALONE_NETWORK_ID}]
rpc_url = "{rpc_url}"
min_settle_delay_secs = {SETTLE_DELAY}
redemption_safety_secs = 90

[settlement.xrpl.{STANDALONE_NETWORK_ID}.key]
key_file = "{xrpl_key}"
"#,
                rpc_url = chains.xrpld.rpc_url,
                xrpl_key = xrpl_file.path().display(),
            ));
        }
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
price = 0
{dealing}
{settlement}
"#,
            state_dir = state_dir.path().display(),
            signer_key = signer_key.path().display(),
            write_key = write_key_hex(&operator),
            app = chains.app,
        ));
        let runtime = connector_cli::build(&config).await.expect("build the node");
        let router = connector_cli::router(&runtime, &config).expect("the node's router");
        let socket = Swappable::serve(listener, router);
        Node {
            name,
            addr,
            operator,
            _state_dir: state_dir,
            _signer_key: signer_key,
            _evm_key: evm_file,
            _xrpl_key: xrpl_file,
            runtime,
            socket,
        }
    }

    fn url(&self) -> String {
        format!("http://{}/ilp", self.addr)
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

    /// `POST /peers` naming `other`, at `fee`, opening this node's channel
    /// with `deposit` (USDC base units on EVM, drops on XRPL) on `network`.
    async fn peer_with(&self, other: &Node, fee: u64, network: &str, deposit: u64) -> Value {
        self.write(
            Method::POST,
            "/peers",
            json!({
                "id": other.name,
                "url": other.url(),
                "fee": fee,
                "max_packet_amount": MAX_PACKET,
                "network": network,
                "deposit": deposit,
            }),
        )
        .await
    }

    async fn route_to(&self, other: &Node, prefix: &str, price: u64) {
        self.write(
            Method::POST,
            "/routes/peers",
            json!({ "prefix": prefix, "peer_id": other.name, "price": price }),
        )
        .await;
    }

    /// A packet to `payee`'s app, sealed to `payee` (ADR 0018).
    async fn originate(&self, payee: &Node, amount: u64) -> Result<Vec<u8>, Reject> {
        let destination = format!("g.example.{}.app", payee.name);
        let (prepare, shared_secret) =
            sealed_prepare(&destination, amount, &payee.identity(), b"deal");
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

    async fn inbound_watermark(&self, id: &str) -> u64 {
        self.read("/channels")
            .await
            .as_array()
            .expect("a list")
            .iter()
            .find(|row| row["id"] == id && row["direction"] == "inbound")
            .map_or(0, |row| row["watermark"].as_u64().expect("a watermark"))
    }
}

fn channel_of(established: &Value) -> String {
    established["channel"]["id"]
        .as_str()
        .expect("the channel's id")
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_usdc_to_xrp_forward_credits_the_converted_amount_at_the_declared_rate() {
    if !require_anvil() || !require_xrpld() {
        return;
    }
    let chains = Chains::spawn().await;
    let b_wallet = chains.xrpld.funded_wallet(&chains.rpc, XRP_FUNDED).await;
    let c_wallet = chains.xrpld.funded_wallet(&chains.rpc, XRP_FUNDED).await;
    let usdc = format!("{:#x}", chains.token);
    let dealing = format!(
        r#"
[[tokens]]
asset = "eip155:31337/erc20:{usdc}"
numeraire = true

[[tokens]]
asset = "xrpl:{STANDALONE_NETWORK_ID}/slip44:144"

[[rates]]
from = "eip155:31337/erc20:{usdc}"
to = "xrpl:{STANDALONE_NETWORK_ID}/slip44:144"
rate = {{ numerator = {rn}, denominator = {rd} }}

[rate_guards]
spread = {{ numerator = {sn}, denominator = {sd} }}
ttl_secs = 300
max_move = {{ numerator = 5, denominator = 100 }}
"#,
        rn = RATE.0,
        rd = RATE.1,
        sn = SPREAD.0,
        sd = SPREAD.1,
    );
    let a = Node::boot(
        "nodea",
        0x61,
        Settles::Evm,
        DEPLOYER_PRIVATE_KEY,
        None,
        &chains,
        "",
    )
    .await;
    let b = Node::boot(
        "nodeb",
        0x62,
        Settles::Both,
        COUNTERPARTY_PRIVATE_KEY,
        Some(&b_wallet),
        &chains,
        &dealing,
    )
    .await;
    let c = Node::boot(
        "nodec",
        0x63,
        Settles::Xrpl,
        "",
        Some(&c_wallet),
        &chains,
        "",
    )
    .await;

    // ── The peerings: A–B over USDC, B–C over XRP ───────────────────────
    // The payee first, so the payer's first voucher already arrives on a
    // bound channel.
    let evm = "eip155:31337";
    let b_to_a = channel_of(&b.peer_with(&a, 0, evm, USDC_DEPOSIT).await);
    let a_to_b = channel_of(&a.peer_with(&b, 0, evm, USDC_DEPOSIT).await);
    c.peer_with(&b, 0, NETWORK, DROPS_DEPOSIT).await;
    let b_established = b.peer_with(&c, B_C_FEE, NETWORK, DROPS_DEPOSIT).await;
    assert_eq!(b_established["channel"]["chain"], "xrpl");
    let b_to_c = channel_of(&b_established);

    // B forwards everything for C, at its incoming unit's price.
    a.route_to(&b, "g.example.nodec", AMOUNT).await;
    b.route_to(&c, "g.example.nodec", AMOUNT).await;

    // ── A pays in USDC, twice; C is credited drops at the declared rate ─
    for crossing in 1..=2u64 {
        a.originate(&c, AMOUNT)
            .await
            .unwrap_or_else(|reject| panic!("A→C crossing {crossing}: {reject:?}"));
        assert_eq!(
            c.inbound_watermark(&b_to_c).await,
            crossing * CONVERTED,
            "C's watermark advanced by the CONVERTED amount: {AMOUNT} base units at \
             {}/{} less a {}/{} spread, less the {B_C_FEE}-drop fee -- not the amount A sent",
            RATE.0,
            RATE.1,
            SPREAD.0,
            SPREAD.1
        );
    }
    assert_eq!(
        b.inbound_watermark(&a_to_b).await,
        2 * AMOUNT,
        "B was paid in USDC what A sent"
    );

    // ── The other direction has no `[[rates]]` row: refused, not passed ─
    // C pays B in XRP toward A. There is no XRP -> USDC row, so there is no
    // forward (ADR 0071 decision 2), and A is credited nothing.
    c.route_to(&b, "g.example.nodea", 100).await;
    b.route_to(&a, "g.example.nodea", 100).await;
    let credited = a.inbound_watermark(&b_to_a).await;
    let refused = c.originate(&a, 100).await;
    let reject = refused.expect_err("a forward with no matching [[rates]] row is refused");
    assert!(
        reject.message.contains("declares no rate"),
        "refused for want of a rate, not for another reason: {reject:?}"
    );
    assert_eq!(
        a.inbound_watermark(&b_to_a).await,
        credited,
        "nothing reached A"
    );
}
