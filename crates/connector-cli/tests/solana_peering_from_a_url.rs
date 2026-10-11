//! **A Solana peering established from a URL, against a real validator**
//! (ADR 0058, as ADR 0075 decisions 3 and 4 amend it; issues #1233, #1379):
//! the Solana twin of `peering_from_a_url.rs`.
//!
//! Nothing here injects a settlement backend, hands a node a pre-opened
//! channel, or serves a hand-written self-description. Two config-driven
//! nodes -- built by `connector_cli::build` and served by
//! `connector_cli::router`, the production boot path -- each on a real
//! socket, settle on one spawned `solana-test-validator` holding
//! solana-foundation's `payment-channels` at `CHNLx…` in genesis. Each is
//! given one authenticated write, `POST /peers { id, url, fee,
//! max_packet_amount, deposit }`, naming the other's URL.
//!
//! The claims this file exists to hold:
//!
//! 1. **A peering is two one-way `payment-channels` channels, one opened by
//!    each side.** `POST /peers` opens and funds only this node's outbound
//!    channel, and never a channel of TOON's own payment-channel program --
//!    asserted against the chain.
//! 2. **Each receiver is the `payee` and `rent_payer` of its inbound
//!    channel**, because the open went through its own sponsor endpoint --
//!    read off the channel account, never off either node's answer.
//! 3. **The other half is admitted, not configured**: each node binds the
//!    peer's channel by the `authorized_signer` the peer's self-description
//!    publishes (`voucherSigners`); nobody exchanges a channel id.
//! 4. **Every forwarded PREPARE carries a voucher, both ways, over both
//!    carriages**, each payee journals it and its verdict rides back in the
//!    ack; a packet that moves no value carries the peer-role challenge.
//! 5. **The peering survives a restart of either node**, with both
//!    watermarks restored.
//! 6. **After the payer's `request_close`, the receiver still lands its
//!    latest voucher with `settle_and_seal`** -- the seat the sponsor
//!    endpoint kept it -- and the payer distributes and takes back only the
//!    rest.

mod support;

use std::io::Write;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::routing::post;
use axum::Router;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::{Duration as ChronoDuration, Utc};
use connector_domain::{EnvelopeRequest, EnvelopeResponse, Fulfill, Prepare, Reject};
use connector_operator::test_support::sign_request;
use connector_runtime::PeerView;
use connector_settlement::batch::{ChannelPresentation, Voucher};
use connector_settlement_solana::batch::wire::{
    ChannelAccount, ChannelStatus, PAYMENT_CHANNELS_PROGRAM_ID,
};
use connector_settlement_solana::test_support::{
    create_mint, fund, mint_to, require_solana_test_validator, SolanaValidator,
};
use connector_signer::giftwrap::{open_response, seal_request};
use connector_signer::PublicKeyBytes;
use ed25519_dalek::Keypair;
use rand::rngs::OsRng;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer as _;
use solana_sdk::signer::keypair::keypair_from_seed;
use tower::ServiceExt;

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
const DEPOSIT: u64 = 10_000;
/// Each node's settlement key's mock USDC, minted by the fixture.
const FUNDED: u64 = 1_000_000;

/// A distinct `created` per signed request: the operator surface rejects a
/// replayed signature (ADR 0008's #1067 amendment).
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

/// One validator for the whole test, with `payment-channels` at `CHNLx…`, a
/// fresh 6-decimal mint, and both nodes' settlement keys holding SOL and
/// [`FUNDED`] tokens.
struct Chain {
    validator: SolanaValidator,
    mint: Pubkey,
}

impl Chain {
    async fn spawn(seeds: &[[u8; 32]]) -> Chain {
        let validator = SolanaValidator::spawn().await;
        let rpc = rpc(&validator.rpc_url);
        let authority = solana_sdk::signature::Keypair::new();
        fund(&rpc, &authority.pubkey()).await;
        let mint = create_mint(&rpc, &authority, 6).await;
        for seed in seeds {
            let key = keypair_from_seed(seed).expect("a keypair").pubkey();
            fund(&rpc, &key).await;
            mint_to(&rpc, &authority, &mint, &key, FUNDED).await;
        }
        Chain { validator, mint }
    }

    fn rpc(&self) -> RpcClient {
        rpc(&self.validator.rpc_url)
    }

    /// The `payment-channels` channel account at `id`, read off the chain.
    async fn channel(&self, id: &str) -> ChannelAccount {
        let address = Pubkey::from_str(id).expect("a base58 channel account");
        let account = self
            .rpc()
            .get_account(&address)
            .await
            .expect("the channel account exists on chain");
        assert_eq!(
            account.owner.to_string(),
            PAYMENT_CHANNELS_PROGRAM_ID,
            "the channel is a payment-channels (x402) channel"
        );
        ChannelAccount::parse(&account.data).expect("a payment-channels channel account")
    }

    async fn token_balance(&self, owner: &Pubkey) -> u64 {
        let ata = spl_associated_token_account::get_associated_token_address(owner, &self.mint);
        match self.rpc().get_token_account_balance(&ata).await {
            Ok(balance) => balance.amount.parse().expect("an amount"),
            Err(_) => 0,
        }
    }
}

fn rpc(url: &str) -> RpcClient {
    RpcClient::new_with_commitment(url.to_string(), CommitmentConfig::confirmed())
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

/// Which carriage the two nodes peer over: each exposes and publishes that
/// one, and `POST /peers` dials whatever the other publishes.
#[derive(Clone, Copy, Debug)]
enum Carriage {
    Http,
    Btp,
}

/// One config-driven node on a real socket: its key files, its state, its
/// config, and its router behind a swappable socket.
struct Node {
    name: &'static str,
    seed: [u8; 32],
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
    /// Node `name` settling as `seed`, answering to `g.example.<name>`,
    /// with a priced app route under it and a free one **pinned to the
    /// carriage it does not peer over**: a client arriving over the
    /// peering's carriage is refused that route (ADR 0072), and a peer is
    /// not held to a client's pin -- which is how a zero-value packet's
    /// attribution is observable below.
    async fn boot(
        name: &'static str,
        seed: [u8; 32],
        chain: &Chain,
        carriage: Carriage,
        app: &str,
    ) -> Node {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the node");
        let addr = listener.local_addr().expect("the node's address");
        let operator = Keypair::generate(&mut OsRng);
        let state_dir = tempfile::tempdir().expect("state dir");
        let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
        signer_key.write_all(&[seed[0] ^ 0xff; 32]).expect("write");
        let mut settlement_key = tempfile::NamedTempFile::new().expect("settlement key");
        settlement_key.write_all(&seed).expect("write");
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
            write_key = write_key_hex(&operator),
            rpc_url = chain.validator.rpc_url,
            mint = chain.mint,
        ));
        let runtime = connector_cli::build(&config).await.expect("build the node");
        let router = connector_cli::router(&runtime, &config).expect("the node's router");
        let socket = Swappable::serve(listener, router);
        Node {
            name,
            seed,
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

    /// This node's `[settlement.solana]` key: payer, `authorized_signer`
    /// and sponsor, all one key (ADR 0075 decision 3).
    fn settlement_key(&self) -> Pubkey {
        keypair_from_seed(&self.seed).expect("a keypair").pubkey()
    }

    /// The key a payload for this node is sealed to (ADR 0018).
    fn identity(&self) -> PublicKeyBytes {
        self.runtime
            .signer
            .public_key()
            .expect("a local signer has a public key")
    }

    async fn write_answer(
        &self,
        method: Method,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
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

    async fn write(
        &self,
        method: Method,
        path: &str,
        body: serde_json::Value,
    ) -> serde_json::Value {
        let (status, answer) = self.write_answer(method, path, body).await;
        assert_eq!(status, StatusCode::OK, "{} {path}: {answer}", self.name);
        answer
    }

    async fn read(&self, path: &str) -> serde_json::Value {
        let (status, answer) = answer(&self.socket.router(), bearer_get(path)).await;
        assert_eq!(status, StatusCode::OK, "{} {path}: {answer}", self.name);
        answer
    }

    fn peer_body(other: &Node, fee: u64, deposit: Option<u64>) -> serde_json::Value {
        let mut body = serde_json::json!({
            "id": other.name,
            "url": other.url(),
            "fee": fee,
            "max_packet_amount": 5_000,
        });
        if let Some(deposit) = deposit {
            body["deposit"] = serde_json::json!(deposit);
        }
        body
    }

    /// `POST /peers` naming `other`, at `fee`, with this node's own opening
    /// deposit.
    async fn peer_with(&self, other: &Node, fee: u64) -> serde_json::Value {
        self.write(
            Method::POST,
            "/peers",
            Node::peer_body(other, fee, Some(DEPOSIT)),
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

    /// Channel `id`'s row in this node's `GET /channels`, in `direction`.
    async fn channel_row(&self, id: &str, direction: &str) -> Option<serde_json::Value> {
        self.read("/channels")
            .await
            .as_array()
            .expect("a list")
            .iter()
            .find(|row| row["id"] == id && row["direction"] == direction)
            .cloned()
    }

    /// The highest voucher this node accepted on inbound channel `id`, as
    /// `GET /channels` reports it; zero for one it has accepted none on.
    async fn inbound_watermark(&self, id: &str) -> u64 {
        self.channel_row(id, "inbound")
            .await
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
                (fields[1] == format!("solana:{id}")).then(|| fields[3].parse().expect("an amount"))
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
async fn two_nodes_peer_over_two_solana_channels_over_http() {
    two_nodes_peer_over_two_solana_channels(Carriage::Http, [0x61; 32], [0x62; 32]).await;
}

#[tokio::test]
async fn two_nodes_peer_over_two_solana_channels_over_btp() {
    two_nodes_peer_over_two_solana_channels(Carriage::Btp, [0x63; 32], [0x64; 32]).await;
}

async fn two_nodes_peer_over_two_solana_channels(
    carriage: Carriage,
    a_seed: [u8; 32],
    b_seed: [u8; 32],
) {
    // `require_solana_test_validator`, not a bare availability check: it
    // panics when `CI` is set and skips only on a developer machine
    // without the Solana CLI and SBF toolchain.
    if !require_solana_test_validator() {
        return;
    }
    let chain = Chain::spawn(&[a_seed, b_seed]).await;
    let app = spawn_app();
    let mut a = Node::boot("nodea", a_seed, &chain, carriage, &app).await;
    let mut b = Node::boot("nodeb", b_seed, &chain, carriage, &app).await;

    // ── Each self-description publishes its node's voucher signer ───────
    // (ADR 0075 decision 10): the Solana settlement key, which every
    // channel the node opens names as `authorized_signer`.
    let described = a.read("/ilp").await;
    let network = described["batchSettlements"][0]["network"]
        .as_str()
        .expect("a Solana x402 network")
        .to_string();
    assert!(network.starts_with("solana:"), "{network}");
    assert_eq!(
        described["voucherSigners"],
        serde_json::json!([{
            "network": network,
            "signer": a.settlement_key().to_string(),
        }]),
        "{described}"
    );

    // ── No channel yet and no deposit named: refused before any spend ───
    let (status, refused) = b
        .write_answer(Method::POST, "/peers", Node::peer_body(&a, PEER_FEE, None))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert!(refused.to_string().contains("deposit"), "{refused}");
    assert_eq!(chain.token_balance(&b.settlement_key()).await, FUNDED);

    // ── Each node writes `POST /peers` naming the other ─────────────────
    let b_established = b.peer_with(&a, PEER_FEE).await;
    let a_established = a.peer_with(&b, PEER_FEE).await;
    for established in [&a_established, &b_established] {
        assert_eq!(established["channel"]["status"], "created");
        assert_eq!(established["channel"]["chain"], "solana");
        assert_eq!(established["fee"], PEER_FEE);
    }
    let b_to_a = channel_of(&b_established);
    let a_to_b = channel_of(&a_established);
    assert_ne!(b_to_a, a_to_b, "a peering is two channels, one each way");

    // Each is a payment-channels channel its payer opened and funded, and
    // its RECEIVER holds the `payee` and `rent_payer` seats: the open went
    // through the receiver's own sponsor endpoint (ADR 0075 decision 3).
    for (id, payer, receiver) in [(&b_to_a, &b, &a), (&a_to_b, &a, &b)] {
        let account = chain.channel(id).await;
        assert_eq!(account.payer, payer.settlement_key(), "{id}: payer");
        assert_eq!(
            account.authorized_signer,
            payer.settlement_key(),
            "{id}: the payer's settlement key signs its vouchers"
        );
        assert_eq!(account.payee, receiver.settlement_key(), "{id}: payee");
        assert_eq!(
            account.rent_payer,
            receiver.settlement_key(),
            "{id}: rent_payer"
        );
        assert_eq!(account.deposit, DEPOSIT, "{id}: deposit");
        assert_eq!(account.mint, chain.mint, "{id}: mint");
    }
    for node in [&a, &b] {
        assert_eq!(
            chain.token_balance(&node.settlement_key()).await,
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
    // peer-role challenge -- Ed25519 by B's `authorized_signer` -- can have
    // made it one (ADR 0075 decision 5).
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
        the_ack_carries_each_vouchers_verdict(&a, &b, &b_to_a).await;
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
        chain.channel(&b_to_a).await.deposit,
        DEPOSIT,
        "no repeat deposited anything more"
    );

    // ── B asks to close; A still lands its latest voucher ───────────────
    // The seat the sponsor endpoint kept A: as `payee`, A may
    // `settle_and_seal` its latest voucher inside the grace period B's
    // `request_close` starts (ADR 0074 decision 5).
    the_receiver_lands_its_latest_voucher_after_the_payers_close(&chain, &a, &b, &b_to_a).await;
}

/// §6.1 over ILP-over-HTTP: the ack answers the voucher, independently of
/// the packet. B's next voucher on its channel toward A is accepted; the
/// same bytes again are accepted and advance nothing (§6.3's resend), so the
/// packet they ride is refused as uncovered; and a voucher below the
/// channel's watermark is refused `amount_not_advancing`.
async fn the_ack_carries_each_vouchers_verdict(a: &Node, b: &Node, b_to_a: &str) {
    let outbound = b.runtime.outbound_channels.clone().expect("B pays on x402");
    let presentation = outbound.presentation(b_to_a).expect("B's channel");
    assert!(matches!(presentation, ChannelPresentation::Solana { .. }));
    let stale = outbound.signed(b_to_a).expect("B's channel");
    let voucher = outbound
        .sign_voucher(b_to_a, stale + u128::from(APP_PRICE))
        .await
        .expect("sign");
    let as_json = |voucher: &Voucher| {
        connector_runtime::voucher_json(
            &presentation,
            voucher,
            &b.settlement_key().to_string(),
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

    // A genuine voucher by B's settlement key -- the 50-byte message, with
    // `expires_at` zero -- below the channel's one watermark.
    let account = Pubkey::from_str(b_to_a).expect("base58").to_bytes();
    let message =
        connector_signer::solana_voucher_message(&account, u64::try_from(stale).expect("fits"), 0);
    let below = Voucher {
        cumulative_amount: stale,
        signature: keypair_from_seed(&b.seed)
            .expect("B's key")
            .sign_message(&message)
            .as_ref()
            .to_vec(),
    };
    let (ack, delivered) = send(as_json(&below), b"stale").await;
    assert!(
        ack.contains("\"rejected\"") && ack.contains("amount_not_advancing"),
        "{ack}"
    );
    assert!(!delivered);
}

/// ADR 0075 decision 3's reason for the sponsor endpoint: B, the payer,
/// requests the close of its channel toward A; A, holding the `payee` seat,
/// lands its latest voucher with `settle_and_seal`; B then distributes and
/// takes back exactly what A did not land.
///
/// B pays once more first, so that A holds a voucher above what the chain
/// has settled when the close starts: A's Open sweep lands only on its
/// first pass after boot and then every ten minutes, so a restarted A may
/// already have landed everything before this point, and then there would
/// be nothing left for `settle_and_seal` to land. A's Closing watcher and
/// the `POST /land` below race for the seal; either may win, and what must
/// hold is that the channel is sealed at A's latest voucher.
async fn the_receiver_lands_its_latest_voucher_after_the_payers_close(
    chain: &Chain,
    a: &Node,
    b: &Node,
    b_to_a: &str,
) {
    b.originate(a, "app", AMOUNT, b"the last crossing before the close")
        .await
        .unwrap_or_else(|reject| panic!("B→A before the close: {reject:?}"));
    let latest = a.inbound_watermark(b_to_a).await;
    let before = chain.channel(b_to_a).await;
    assert!(
        before.settled < latest,
        "A holds a voucher at {latest} the chain has not settled (settled: {})",
        before.settled
    );

    let withdraw = format!("/channels/{b_to_a}/withdraw");
    let started = b
        .write(Method::POST, &withdraw, serde_json::json!({}))
        .await;
    assert_eq!(started["step"], "started");
    assert_eq!(started["status"], "closing");

    let (status, landed) = a
        .write_answer(
            Method::POST,
            &format!("/channels/{b_to_a}/land"),
            serde_json::json!({}),
        )
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CONFLICT,
        "{status}: {landed}"
    );
    // Sealed by whichever of the two landed it; the watcher passes every
    // ten seconds, so a minute is ample.
    let mut sealed = chain.channel(b_to_a).await;
    for _ in 0..60 {
        if sealed.status == ChannelStatus::Sealed {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        sealed = chain.channel(b_to_a).await;
    }
    assert_eq!(sealed.status, ChannelStatus::Sealed, "{sealed:?}");
    assert_eq!(
        sealed.settled, latest,
        "A landed its latest voucher after B asked to close"
    );
    let row = a
        .channel_row(b_to_a, "inbound")
        .await
        .expect("A still lists its inbound channel");
    assert_eq!(row["status"], "sealed", "{row}");
    assert_eq!(row["landed"], latest, "{row}");

    let a_before = chain.token_balance(&a.settlement_key()).await;
    let b_before = chain.token_balance(&b.settlement_key()).await;
    let finished = b
        .write(Method::POST, &withdraw, serde_json::json!({}))
        .await;
    assert_eq!(finished["step"], "finished");
    assert_eq!(
        chain.token_balance(&a.settlement_key()).await,
        a_before + (latest - sealed.payout_watermark),
        "A was paid what it landed and had not yet been paid"
    );
    assert_eq!(
        chain.token_balance(&b.settlement_key()).await,
        b_before + (DEPOSIT - latest),
        "B took back exactly the rest of its deposit"
    );
}
