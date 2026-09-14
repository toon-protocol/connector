//! **A Solana peering established from a URL, against a real validator**
//! (ADR 0058, issue #1233): the Solana twin of `peering_from_a_url.rs`
//! and of `connector-client-edge`'s
//! `runtime_peering_can_pay_the_forward_it_accepted.rs`, in one file
//! because both halves need the same two nodes.
//!
//! Nothing here injects a settlement backend, hands a node a pre-opened
//! channel, or serves a hand-written self-description. Two config-driven
//! `connector_cli::run` nodes settle on one spawned
//! `solana-test-validator` running the real `packages/solana-program`
//! artifact; node A is served on a real socket so that node B's
//! `POST /peers` reads A's **own** self-description -- endpoints, edge
//! identity, settlement address, program id, mint -- exactly as a
//! counterparty on devnet would.
//!
//! What the Solana leg does differently, and what each test holds:
//!
//! 1. **The channel is a node-submitted `InitializeChannel`.** There is no
//!    chain CLI that can build one; `establish_peering` is the submitter,
//!    and the account it reports is read back off the chain under the
//!    configured program with both participants' settlement keys.
//! 2. **Funding is a strictly-by-signer `Deposit` increment** (#1118).
//!    `POST /channels/:id/fund` moves the caller's *own* tokens, and a
//!    second call adds to the deposit rather than restating it -- the
//!    opposite of the EVM leg's absolute `setTotalDeposit`.
//! 3. **A claim binds the program** (ADR 0053). The payee resolves the
//!    channel from its own `[settlement.solana]` program and verifies the
//!    balance proof under that program id; a claim signed for any other
//!    deployment would not verify. The payee's accepted claim is read
//!    back under the `solana:` namespace from its client book.
//! 4. **A restart rehydrates a payable hop** (#1217/#1230): after the
//!    payer is booted again from the same config and `state_dir`, the
//!    durable runtime-peer row replays into an outbound CLIENT hop and a
//!    further packet still fulfils.
//!
//! # Why node A publishes a BTP endpoint too
//!
//! `[node]` requires both endpoints today (issue #1220 / PR #1232 will
//! make each conditional on `peer_expose`), so a real node A has to
//! publish a `btp_endpoint`, and a node B that allows plaintext dials BTP
//! first when both are published (`peering.rs`, "BTP first where both are
//! published"). Node A therefore exposes **both** carriages on the one
//! socket it serves, which is valid under today's loader and under
//! #1232's required-iff-exposed rule alike, and the peering below rides
//! BTP while the covering claim's `POST /ilp/claim-state` ask rides the
//! `httpEndpoint`, as it always does. The chain semantics under test do
//! not depend on which carriage carried the packet.

use std::io::Write;
use std::net::SocketAddr;
use std::str::FromStr;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use chrono::{Duration as ChronoDuration, Utc};
use ed25519_dalek::Keypair;
use rand::rngs::OsRng;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer as SolanaSigner;
use tower::ServiceExt;

use connector_domain::{EnvelopeRequest, EnvelopeResponse, Fulfill, Prepare, Reject};
use connector_operator::test_support::sign_request;
use connector_runtime::PeerView;
use connector_settlement::{ChannelId, SettlementBackend};
use connector_settlement_solana::test_support::{
    fund as fund_sol, require_solana_test_validator, SolanaValidator, LOCAL_TEST_PROGRAM_ID,
};
use connector_settlement_solana::SolanaSettlementBackend;
use connector_signer::giftwrap::{open_response, seal_request};
use connector_signer::{verify_solana_balance_proof, LocalSigner, PublicKeyBytes, Signer};

/// What node A's app route charges, and so what every covering claim
/// below advances the payee's watermark by.
const APP_PRICE: u64 = 1_000;
/// What node B collateralises the channel with, in mint base units. Enough
/// for the three crossings of the payment proof and then some.
const FIRST_DEPOSIT: u128 = 1_500;
const TOP_UP: u128 = 500;
/// Real mock USDC of each node's own, minted by the fixture's mint
/// authority: `fund` is a self-deposit (#1118), so a node with an empty
/// ATA cannot collateralise a channel at all.
const TOKENS_PER_NODE: u64 = 10_000;
const PEER_ID: &str = "apex-a";
const A_PREFIX: &str = "g.example.a";
const A_APP_PREFIX: &str = "g.example.a.app";

/// A distinct `created` per signed request: the operator surface rejects a
/// replayed signature (ADR 0008's #1067 amendment), and two of the writes
/// below are byte-identical on purpose.
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

async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    hyper::body::to_bytes(response.into_body())
        .await
        .unwrap()
        .to_vec()
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(response).await).expect("a JSON body")
}

/// One node's key material: a raw 32-byte seed on disk, read as both its
/// `[signer]` key (secp256k1, the edge identity a payload is sealed to)
/// and its `[settlement.solana]` key (ed25519, the channel participant).
/// Two curves, one file -- the same shape `settlement_lifecycle.rs` uses.
struct NodeKeys {
    seed: [u8; 32],
    key_file: tempfile::NamedTempFile,
    /// The RFC 9421 write key for this node's operator surface.
    write_key: Keypair,
}

impl NodeKeys {
    fn new(seed: [u8; 32]) -> Self {
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(&seed)
            .expect("write raw 32-byte key file");
        Self {
            seed,
            key_file,
            write_key: Keypair::generate(&mut OsRng),
        }
    }

    fn settlement_pubkey(&self) -> Pubkey {
        solana_sdk::signer::keypair::keypair_from_seed(&self.seed)
            .expect("derive keypair")
            .pubkey()
    }

    fn edge_identity(&self) -> PublicKeyBytes {
        LocalSigner::from_secret_bytes("edge", self.seed)
            .expect("a raw seed is a valid secp256k1 secret")
            .public_key()
            .expect("a local signer has a public key")
    }

    fn write_key_hex(&self) -> String {
        self.write_key
            .public
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// The chain both nodes settle on, and the fixture that funds them: a
/// `deploy()`-built backend purely as the mock mint's authority. Nothing
/// it can do that a real node cannot is used by either node -- both are
/// built from config files, by `connect()`.
struct Chain {
    validator: SolanaValidator,
    program_id: Pubkey,
    token_mint: Pubkey,
}

impl Chain {
    async fn spawn(nodes: &[&NodeKeys]) -> Self {
        let validator = SolanaValidator::spawn().await;
        let program_id =
            Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
        let mint_authority = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
            .await
            .expect("bind to the genesis-loaded payment-channel program");
        let token_mint = mint_authority.token_mint();
        let rpc = RpcClient::new_with_commitment(
            validator.rpc_url.clone(),
            CommitmentConfig::confirmed(),
        );
        for node in nodes {
            let pubkey = node.settlement_pubkey();
            fund_sol(&rpc, &pubkey).await;
            mint_authority
                .test_mint_tokens_to(&pubkey, TOKENS_PER_NODE)
                .await
                .expect("give the node real tokens of its own");
        }
        Self {
            validator,
            program_id,
            token_mint,
        }
    }

    fn rpc(&self) -> RpcClient {
        RpcClient::new_with_commitment(
            self.validator.rpc_url.clone(),
            CommitmentConfig::confirmed(),
        )
    }

    async fn token_balance(&self, owner: &Pubkey) -> u64 {
        let ata =
            spl_associated_token_account::get_associated_token_address(owner, &self.token_mint);
        self.rpc()
            .get_token_account_balance(&ata)
            .await
            .expect("read the SPL balance")
            .amount
            .parse::<u64>()
            .expect("an SPL balance is an integer of base units")
    }

    /// A second backend under `keys`'s own identity, to read the chain
    /// back with -- never the node under test's own answer.
    async fn reader(&self, keys: &NodeKeys) -> SolanaSettlementBackend {
        SolanaSettlementBackend::connect(
            &self.validator.rpc_url,
            &keys.seed,
            self.program_id,
            self.token_mint,
            6,
        )
        .await
        .expect("a reader backend under the node's own settlement key")
    }

    fn settlement_toml(&self, keys: &NodeKeys) -> String {
        format!(
            r#"
[settlement.solana]
rpc_url = "{rpc_url}"
program_id = "{program_id}"
token_address = "{token_mint}"
decimals = 6

[settlement.solana.key]
key_file = "{key_path}"
"#,
            rpc_url = self.validator.rpc_url,
            program_id = self.program_id,
            token_mint = self.token_mint,
            key_path = keys.key_file.path().display(),
        )
    }
}

/// Boot a config-driven node and hand back its merged router.
async fn boot(config_path: &std::path::Path) -> Router {
    let command = connector_cli::run(&["connector".to_string(), config_path.display().to_string()])
        .await
        .expect("run: a config-driven node with Solana settlement");
    let connector_cli::Command::Serve(node) = command else {
        panic!("a config path must produce a servable node");
    };
    node.router
}

/// Serve `router` on an already-bound listener, so its port could be
/// written into a config file before the node behind it existed.
fn serve(listener: std::net::TcpListener, router: Router) {
    tokio::spawn(async move {
        let _ = axum::Server::from_tcp(listener)
            .expect("serve the bound listener")
            .serve(router.into_make_service())
            .await;
    });
}

/// Node A: the counterparty, served on a real socket. It exposes both
/// peer carriages on that socket (see the file header for why both), and
/// terminates one priced app route at a tiny real HTTP app this test also
/// serves, so a packet forwarded to it can actually be delivered.
struct NodeA {
    addr: SocketAddr,
    router: Router,
    _config: tempfile::NamedTempFile,
    state_dir: tempfile::TempDir,
}

impl NodeA {
    async fn boot(chain: &Chain, keys: &NodeKeys) -> Self {
        // The app: answers every POST with 200 "delivered".
        let app_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind app");
        let app_addr = app_listener.local_addr().expect("app addr");
        serve(
            app_listener,
            Router::new().route("/", axum::routing::post(|| async { "delivered" })),
        );

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind node A");
        let addr = listener.local_addr().expect("node A addr");
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut config = tempfile::NamedTempFile::new().expect("temp config file");
        write!(
            config,
            r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"
peer_allow_plaintext_endpoints = true
peer_expose = "both"

[node]
addresses = ["{A_PREFIX}"]
http_endpoint = "http://{addr}/ilp"
btp_endpoint = "ws://{addr}/ilp/btp"

[signer]
key_file = "{key_path}"

[operator]
bearer_token = "operator-secret"
write_keys = ["{write_key_hex}"]

[[routes]]
prefix = "{A_APP_PREFIX}"
handler_url = "http://{app_addr}/"
price = {APP_PRICE}
{settlement}"#,
            state_dir = state_dir.path().display(),
            key_path = keys.key_file.path().display(),
            write_key_hex = keys.write_key_hex(),
            settlement = chain.settlement_toml(keys),
        )
        .expect("write config file");

        let router = boot(config.path()).await;
        serve(listener, router.clone());
        Self {
            addr,
            router,
            _config: config,
            state_dir,
        }
    }

    fn self_description_url(&self) -> String {
        format!("http://{}/ilp", self.addr)
    }

    /// The last claim the payee accepted at its client edge, straight
    /// off its own durable journal (`client-edge-claims.log`, ADR 0005):
    /// `(channel key, nonce, cumulative, signature)`. The signature is what
    /// `GET /claims` does not carry, and what an on-chain redemption would
    /// submit.
    fn last_journalled_client_claim(&self) -> (String, u64, u64, Vec<u8>) {
        let journal = std::fs::read_to_string(self.state_dir.path().join("client-edge-claims.log"))
            .expect("the payee keeps a client-edge claim journal in its state_dir");
        let line = journal
            .lines()
            .rfind(|line| line.starts_with("inbound_claim_accepted\t"))
            .expect("at least one accepted claim is journalled");
        let fields: Vec<&str> = line.split('\t').collect();
        let signature = (0..fields[4].len() / 2)
            .map(|i| u8::from_str_radix(&fields[4][i * 2..i * 2 + 2], 16).expect("hex"))
            .collect();
        (
            fields[1].to_string(),
            fields[2].parse().expect("a nonce"),
            fields[3].parse().expect("an amount"),
            signature,
        )
    }

    /// The payee's client book, read over its operator surface: every
    /// `(channel key, nonce, cumulative)` accepted at the client edge.
    async fn client_claims(&self) -> Vec<(String, u64, u64)> {
        let response = self
            .router
            .clone()
            .oneshot(bearer_get("/claims"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response)
            .await
            .as_array()
            .expect("GET /claims answers a list")
            .iter()
            .filter(|row| row["book"] == "client")
            .map(|row| {
                (
                    row["channel_id"]
                        .as_str()
                        .expect("a channel id")
                        .to_string(),
                    row["nonce"].as_u64().expect("a nonce"),
                    row["cumulative_amount"].as_u64().expect("an amount"),
                )
            })
            .collect()
    }
}

/// Node B: the node under test, the one that establishes the peering. It
/// is driven through its router directly -- nothing dials *it* -- and its
/// config file and `state_dir` outlive any one boot, so the payment proof
/// can boot it a second time from the same durable state.
struct NodeB {
    config: tempfile::NamedTempFile,
    state_dir: tempfile::TempDir,
}

impl NodeB {
    fn write(chain: &Chain, keys: &NodeKeys) -> Self {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        // No `[[peers]]` table and no `[node]`: every peering this node
        // holds is established over the operator surface, which is the
        // whole of what ADR 0058 adds. `peer_allow_plaintext_endpoints`
        // is the same opt-in every `local/` topology takes.
        let mut config = tempfile::NamedTempFile::new().expect("temp config file");
        write!(
            config,
            r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"
peer_allow_plaintext_endpoints = true

[signer]
key_file = "{key_path}"

[operator]
bearer_token = "operator-secret"
write_keys = ["{write_key_hex}"]
{settlement}"#,
            state_dir = state_dir.path().display(),
            key_path = keys.key_file.path().display(),
            write_key_hex = keys.write_key_hex(),
            settlement = chain.settlement_toml(keys),
        )
        .expect("write config file");
        Self { config, state_dir }
    }

    async fn boot(&self) -> Router {
        boot(self.config.path()).await
    }

    /// The durable row `POST /peers` wrote for [`PEER_ID`], straight off
    /// this node's `state_dir` -- what a restart rehydrates from.
    fn durable_peering(&self) -> serde_json::Value {
        let table = std::fs::read_to_string(self.state_dir.path().join("runtime-peers.json"))
            .expect("the runtime peer/route table lives in state_dir");
        let snapshot: serde_json::Value = serde_json::from_str(&table).expect("a JSON table");
        snapshot["peers"]
            .as_array()
            .expect("a peers list")
            .iter()
            .find(|row| row["id"] == PEER_ID)
            .cloned()
            .expect("the peering has a durable row")
    }
}

fn peer_body(url: &str, chain: Option<&str>) -> Vec<u8> {
    let mut body = serde_json::json!({
        "id": PEER_ID,
        "url": url,
        "fee": 0,
        "max_packet_amount": 5_000,
    });
    if let Some(chain) = chain {
        body["chain"] = serde_json::Value::String(chain.to_string());
    }
    serde_json::to_vec(&body).unwrap()
}

/// `POST /peers` on node B, at node A's real self-description.
async fn establish(
    b: &Router,
    keys: &NodeKeys,
    a: &NodeA,
    chain: Option<&str>,
) -> serde_json::Value {
    let response = b
        .clone()
        .oneshot(signed(
            &keys.write_key,
            Method::POST,
            "/peers",
            peer_body(&a.self_description_url(), chain),
        ))
        .await
        .unwrap();
    let status = response.status();
    let body = body_bytes(response).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "POST /peers: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("a JSON body")
}

/// `POST /channels/:id/fund` on node B: a self-deposit of `amount`, and
/// the channel as the node reads it back afterwards.
async fn fund(b: &Router, keys: &NodeKeys, channel: &str, amount: u128) -> serde_json::Value {
    let body = serde_json::to_vec(&serde_json::json!({ "amount": amount })).unwrap();
    let response = b
        .clone()
        .oneshot(signed(
            &keys.write_key,
            Method::POST,
            &format!("/channels/{channel}/fund"),
            body,
        ))
        .await
        .unwrap();
    let status = response.status();
    let body = body_bytes(response).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "POST /channels/:id/fund: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("a JSON body")
}

/// `GET /routes/peers` on node B: the peer-forwarding routing table as the
/// operator surface reports it, config-file and runtime rows alike.
async fn peer_routes(b: &Router) -> serde_json::Value {
    let response = b
        .clone()
        .oneshot(bearer_get("/routes/peers"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

/// The one row [`route_through_peering`] lands, as `GET /routes/peers`
/// must report it: the posted prefix, peer and price, tagged `runtime`.
fn expected_routing_table() -> serde_json::Value {
    serde_json::json!([{
        "prefix": A_PREFIX,
        "peer_id": PEER_ID,
        "price": APP_PRICE,
        "source": "runtime",
    }])
}

/// `POST /routes/peers` on node B: forward `A_PREFIX` to the peering.
async fn route_through_peering(b: &Router, keys: &NodeKeys) {
    assert_eq!(
        peer_routes(b).await,
        serde_json::json!([]),
        "a node with no `[[routes]]` peer form and no runtime writes forwards nothing"
    );
    let body = serde_json::to_vec(&serde_json::json!({
        "prefix": A_PREFIX,
        "peer_id": PEER_ID,
        "price": APP_PRICE,
    }))
    .unwrap();
    let response = b
        .clone()
        .oneshot(signed(&keys.write_key, Method::POST, "/routes/peers", body))
        .await
        .unwrap();
    let status = response.status();
    let body = body_bytes(response).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "POST /routes/peers must accept a route through a peering that can pay (#1217): {}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
        expected_routing_table()[0],
        "the write answers with the row it landed"
    );
    assert_eq!(
        peer_routes(b).await,
        expected_routing_table(),
        "GET /routes/peers is the routing table: exactly the posted row, tagged `runtime`"
    );
}

/// Originate one packet over node B's operator surface, addressed to node
/// A's app route and gift-wrapped to A's edge identity (ADR 0018), and
/// require the app's own answer back out of the FULFILL.
async fn originate_and_expect_fulfil(
    b: &Router,
    keys: &NodeKeys,
    payee_identity: &PublicKeyBytes,
    body: &[u8],
) {
    let plaintext = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: body.to_vec(),
    }
    .encode();
    let (data, shared_secret) = seal_request(&plaintext, payee_identity).expect("seal");
    let prepare = Prepare {
        amount: APP_PRICE,
        expires_at: Utc::now() + ChronoDuration::minutes(5),
        greeting: false,
        destination: A_APP_PREFIX.to_string(),
        data,
    };
    let response = b
        .clone()
        .oneshot(signed(
            &keys.write_key,
            Method::POST,
            "/packets",
            prepare.encode(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body_bytes(response).await;
    let fulfill = match Fulfill::decode(&bytes) {
        Ok(fulfill) => fulfill,
        Err(_) => {
            let reject = Reject::decode(&bytes).expect("a packet answer is a FULFILL or a REJECT");
            panic!(
                "expected a fulfil -- issue #1217's bug answers a T00 naming a missing \
                 '[[pay_channels]]' row here instead: {:?} {} (from {})",
                reject.code, reject.message, reject.triggered_by
            );
        }
    };
    let opened = open_response(&shared_secret, &fulfill.data).expect("open the sealed response");
    let envelope = EnvelopeResponse::decode(&opened).expect("decode envelope response");
    assert_eq!(envelope.status, 200);
    assert_eq!(envelope.body, b"delivered");
}

/// The channel half (issue #1233, test 1): one operator write derives and
/// opens the channel on chain, a repeat finds it, funding is an
/// increment, and a route through the peering is accepted.
#[tokio::test]
async fn one_operator_write_establishes_a_solana_peering_and_funding_it_is_an_increment() {
    // `require_solana_test_validator`, not a bare availability check: it
    // panics when `CI` is set and skips only on a developer machine
    // without the Solana CLI and SBF toolchain.
    if !require_solana_test_validator() {
        return;
    }

    let keys_a = NodeKeys::new([41u8; 32]);
    let keys_b = NodeKeys::new([42u8; 32]);
    assert_ne!(keys_a.settlement_pubkey(), keys_b.settlement_pubkey());
    let chain = Chain::spawn(&[&keys_a, &keys_b]).await;
    let a = NodeA::boot(&chain, &keys_a).await;
    let b = NodeB::write(&chain, &keys_b);
    let router_b = b.boot().await;

    // ── The write ────────────────────────────────────────────────────────
    // No `chain` in the request: the two nodes share exactly one, so the
    // write resolves it rather than refusing `AmbiguousChain`.
    let established = establish(&router_b, &keys_b, &a, None).await;
    assert_eq!(established["id"], PEER_ID);
    assert_eq!(established["source"], "runtime");
    assert_eq!(established["fee"], 0);
    assert_eq!(established["max_packet_amount"], 5_000);
    assert_eq!(established["channel"]["status"], "created");
    assert_eq!(established["channel"]["chain"], "solana");
    let channel = established["channel"]["id"]
        .as_str()
        .expect("the channel's account")
        .to_string();
    let channel_pubkey = Pubkey::from_str(&channel).expect("a Solana channel id is base58");

    // ── The account exists on chain, under the configured program ──────
    let account = chain
        .rpc()
        .get_account(&channel_pubkey)
        .await
        .expect("the channel account the write reported exists on chain");
    assert_eq!(
        account.owner, chain.program_id,
        "the channel account is owned by the configured payment-channel program"
    );

    // ...and it is the two SETTLEMENT keys' channel, read back through a
    // backend under A's own key -- never the node under test's answer.
    let reader = chain.reader(&keys_a).await;
    let counterparty = reader
        .channel_counterparty(channel_pubkey)
        .await
        .expect("read the channel back off the chain")
        .expect("the channel exists and A is a participant");
    assert_eq!(
        counterparty,
        keys_b.settlement_pubkey(),
        "seen from A's side, the other participant is B's SETTLEMENT key -- never B's \
         secp256k1 edge identity, which the deployed program could not hold as a participant"
    );
    // ...and it is the PDA ADR 0059's derivation names for this pair.
    let derived = reader
        .live_channel_with(keys_b.settlement_pubkey().to_bytes().to_vec())
        .await
        .expect("ask the chain whether this pair has a channel")
        .expect("it does");
    assert_eq!(derived.0, channel);

    // ── Repeating the identical request finds it ────────────────────────
    let repeated = establish(&router_b, &keys_b, &a, None).await;
    assert_eq!(
        repeated["channel"]["status"], "found",
        "a repeat must land on the channel the first attempt opened"
    );
    assert_eq!(repeated["channel"]["id"], channel.as_str());

    // Naming the chain explicitly selects Solana and lands on the same
    // channel: the `"chain"` request field is honoured on the Solana arm,
    // not only parsed.
    let named = establish(&router_b, &keys_b, &a, Some("solana")).await;
    assert_eq!(named["channel"]["status"], "found");
    assert_eq!(named["channel"]["chain"], "solana");
    assert_eq!(named["channel"]["id"], channel.as_str());

    // ── The peering is one row, readable back ───────────────────────────
    let response = router_b
        .clone()
        .oneshot(bearer_get("/peers"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let peers: Vec<PeerView> = serde_json::from_slice(&body_bytes(response).await).unwrap();
    assert_eq!(peers.len(), 1, "three writes, one peering: {peers:?}");
    assert_eq!(peers[0].id, PEER_ID);

    // ...and its durable row carries everything a restart needs to rebuild
    // the Solana hop: the carriage endpoint (BTP, see the file header), the
    // client edge the covering claim's ask goes to, and the binding with
    // the program id ADR 0053 signs into every claim on the channel.
    let row = b.durable_peering();
    assert!(
        row["endpoint"]
            .as_str()
            .is_some_and(|endpoint| endpoint.starts_with("ws://")),
        "with both endpoints published, the peering dials BTP first: {row}"
    );
    assert_eq!(row["client_edge_url"], a.self_description_url());
    assert_eq!(
        row["channels"],
        serde_json::json!([{
            "chain": "solana",
            "channel_account": channel,
            "counterparty_key": keys_a.settlement_pubkey().to_string(),
            "program_id": chain.program_id.to_string(),
        }])
    );

    // ── Funding is a self-deposit, and a second fund is an increment ────
    let b_pubkey = keys_b.settlement_pubkey();
    assert_eq!(chain.token_balance(&b_pubkey).await, TOKENS_PER_NODE);

    let funded = fund(&router_b, &keys_b, &channel, FIRST_DEPOSIT).await;
    assert_eq!(funded["own_deposited"], FIRST_DEPOSIT as u64);
    assert_eq!(
        funded["deposited"], 0,
        "a self-deposit credits B's own side; A has deposited nothing"
    );
    assert_eq!(
        chain.token_balance(&b_pubkey).await,
        TOKENS_PER_NODE - FIRST_DEPOSIT as u64,
        "the deposit left B's own token account for the vault"
    );

    // Read the deposit back off the chain first, the way `local/keys.sh`'s
    // solana-channels stage does, then top up by the shortfall.
    let on_chain = reader
        .channel_state(&ChannelId(channel.clone()))
        .await
        .expect("read the channel state");
    assert_eq!(
        on_chain.counterparty_deposited, FIRST_DEPOSIT,
        "from A's side, B's deposit is the counterparty's"
    );

    let topped_up = fund(&router_b, &keys_b, &channel, TOP_UP).await;
    assert_eq!(
        topped_up["own_deposited"],
        (FIRST_DEPOSIT + TOP_UP) as u64,
        "the second fund adds the increment to the deposit; it does not restate it -- \
         the opposite of the EVM leg's absolute setTotalDeposit"
    );
    assert_eq!(
        chain.token_balance(&b_pubkey).await,
        TOKENS_PER_NODE - (FIRST_DEPOSIT + TOP_UP) as u64,
    );
    let on_chain = reader
        .channel_state(&ChannelId(channel.clone()))
        .await
        .expect("read the channel state again");
    assert_eq!(on_chain.counterparty_deposited, FIRST_DEPOSIT + TOP_UP);

    // ── A route through it is a second, separate write ──────────────────
    // Accepted because `establish_peering` registered the CLIENT-role hop
    // (#1217/#1230): the guard tests that hop, and a Solana hop needs the
    // `[settlement.solana]` signer this node has.
    route_through_peering(&router_b, &keys_b).await;
}

/// The payment half (issue #1233, test 2): a packet originated over the
/// peering fulfils, the payee's client book advances under the Solana
/// namespace with a claim verified against the payee's own program (ADR
/// 0053), and a payer booted again from its durable row still pays.
#[tokio::test]
async fn a_solana_runtime_peering_can_pay_the_forward_it_accepted_and_still_can_after_a_restart() {
    if !require_solana_test_validator() {
        return;
    }

    let keys_a = NodeKeys::new([43u8; 32]);
    let keys_b = NodeKeys::new([44u8; 32]);
    let chain = Chain::spawn(&[&keys_a, &keys_b]).await;
    let a = NodeA::boot(&chain, &keys_a).await;
    let b = NodeB::write(&chain, &keys_b);
    let router_b = b.boot().await;
    let payee_identity = keys_a.edge_identity();

    // ── Establish, collateralise, route ─────────────────────────────────
    let established = establish(&router_b, &keys_b, &a, None).await;
    assert_eq!(established["channel"]["status"], "created");
    assert_eq!(established["channel"]["chain"], "solana");
    let channel = established["channel"]["id"]
        .as_str()
        .expect("the channel's account")
        .to_string();
    let funded = fund(&router_b, &keys_b, &channel, 3 * APP_PRICE as u128).await;
    assert_eq!(funded["own_deposited"], 3 * APP_PRICE);
    route_through_peering(&router_b, &keys_b).await;

    // The payee resolves this channel from chain under its own program and
    // has accepted nothing on it yet.
    let channel_key = format!("solana:{channel}");
    assert!(
        a.client_claims().await.is_empty(),
        "nothing has been paid over the peering yet"
    );

    // ── First crossing: the peering can actually pay ────────────────────
    originate_and_expect_fulfil(&router_b, &keys_b, &payee_identity, b"first crossing").await;
    assert_eq!(
        a.client_claims().await,
        vec![(channel_key.clone(), 1, APP_PRICE)],
        "the payee's client book must show B's claim under the Solana namespace, advanced by \
         the forward -- verified against the program the payee's own [settlement.solana] \
         names (ADR 0053), since that is the only program its channel source resolves under"
    );

    // ── Second crossing: genuinely covered, not stuck replaying ─────────
    originate_and_expect_fulfil(&router_b, &keys_b, &payee_identity, b"second crossing").await;
    assert_eq!(
        a.client_claims().await,
        vec![(channel_key.clone(), 2, 2 * APP_PRICE)],
        "each crossing must advance the payee's watermark by what it forwards"
    );

    // ── A restart rehydrates a payable hop, not a name ───────────────────
    // The same config file and the same `state_dir`: the runtime-peer row,
    // the route and the outbound client ledger all come back from disk
    // through the production boot path, not through any test seam.
    drop(router_b);
    let router_b = b.boot().await;

    let response = router_b
        .clone()
        .oneshot(bearer_get("/peers"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let peers: Vec<PeerView> = serde_json::from_slice(&body_bytes(response).await).unwrap();
    assert_eq!(peers.len(), 1, "the peering itself survived the restart");
    assert_eq!(peers[0].id, PEER_ID);
    assert_eq!(
        peer_routes(&router_b).await,
        expected_routing_table(),
        "the routing table survives the restart with the row still tagged `runtime`"
    );

    originate_and_expect_fulfil(&router_b, &keys_b, &payee_identity, b"after a restart").await;
    assert_eq!(
        a.client_claims().await,
        vec![(channel_key.clone(), 3, 3 * APP_PRICE)],
        "the same channel's watermark keeps advancing after the payer's restart -- a restart \
         must not turn a payable peering back into an accept-only one"
    );

    // ── The claim binds the program (ADR 0053) ──────────────────────────
    // The signature the payee journalled is a balance proof over the
    // payee's own program id, the channel account, the nonce and the
    // amount, signed by B's SETTLEMENT key (never its edge identity). Read
    // the journal rather than the operator view, because the signature is
    // what an on-chain redemption submits, and check it both ways: it
    // verifies under the configured program and under no other.
    let (journalled_key, nonce, cumulative, signature) = a.last_journalled_client_claim();
    assert_eq!(journalled_key, channel_key);
    assert_eq!((nonce, cumulative), (3, 3 * APP_PRICE));
    let channel_account = Pubkey::from_str(&channel)
        .expect("a Solana channel id is base58")
        .to_bytes();
    let payer_key = keys_b.settlement_pubkey().to_bytes();
    assert!(
        verify_solana_balance_proof(
            &chain.program_id.to_bytes(),
            &channel_account,
            nonce,
            cumulative,
            &signature,
            &payer_key,
        ),
        "the accepted claim is a balance proof under the payee's configured program, signed by \
         the payer's settlement key"
    );
    assert!(
        !verify_solana_balance_proof(
            &[0x11u8; 32],
            &channel_account,
            nonce,
            cumulative,
            &signature,
            &payer_key,
        ),
        "the same signature verifies under no other program: a claim signed for one deployment \
         cannot be replayed against another (ADR 0053)"
    );
    assert!(
        !verify_solana_balance_proof(
            &chain.program_id.to_bytes(),
            &channel_account,
            nonce,
            cumulative,
            &signature,
            &keys_a.settlement_pubkey().to_bytes(),
        ),
        "and it is the payer's signature, not the payee's own"
    );
}
