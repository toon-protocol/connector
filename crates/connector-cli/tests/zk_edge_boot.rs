//! A node with a `[zk_note]` table boots as a zk edge (ADR 0080 decision 15,
//! issue #1514): against zk_toon's recorded vault served over real HTTP, it
//! reads before it serves, publishes its terms, runs the allowance sweep, and
//! refuses to start -- by name -- on a key it does not carry or a vault it
//! cannot read.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use connector_cli::{build, router, RuntimeError};
use connector_config::Config;
use serde_json::{json, Value};
use tower::ServiceExt;

const RECORDED_VAULT: &str =
    include_str!("../../connector-zk-note-mina/fixtures/recorded-vault.json");

/// A Mina endpoint on a real socket, answering GraphQL by operation name.
struct StandIn {
    addr: SocketAddr,
    calls: Arc<Mutex<Vec<String>>>,
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

impl StandIn {
    async fn spawn(answers: impl Fn(&str) -> Option<Value> + Send + Sync + 'static) -> StandIn {
        type Shared = (
            Arc<dyn Fn(&str) -> Option<Value> + Send + Sync>,
            Arc<Mutex<Vec<String>>>,
        );
        async fn serve(State((answers, calls)): State<Shared>, body: Bytes) -> Json<Value> {
            let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let operation = request["operationName"].as_str().unwrap_or("").to_string();
            calls.lock().unwrap().push(operation.clone());
            Json(answers(&operation).unwrap_or_else(|| json!({ "errors": [{ "message": "no" }] })))
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/", post(serve))
            .with_state((Arc::new(answers) as _, Arc::clone(&calls)));
        let server = axum::Server::bind(&SocketAddr::from(([127, 0, 0, 1], 0)))
            .serve(app.into_make_service());
        let addr = server.local_addr();
        let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = server
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await;
        });
        StandIn {
            addr,
            calls,
            _shutdown: shutdown,
        }
    }

    fn url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

fn recorded() -> Value {
    serde_json::from_str(RECORDED_VAULT).expect("the recording")
}

/// The recorded daemon: the vault's account, and a constructed anchor (the
/// recording asked for the slot only, so the block's time is made up).
async fn daemon() -> StandIn {
    let state = recorded()["vaultState"].clone();
    StandIn::spawn(move |operation| match operation {
        "VaultState" => Some(state.clone()),
        "SlotAnchor" => Some(json!({ "data": { "bestChain": [{ "protocolState": {
            "consensusState": { "slotSinceGenesis": "1114" },
            "blockchainState": { "utcDate": chrono::Utc::now().timestamp_millis().to_string() },
        } }] } })),
        _ => None,
    })
    .await
}

struct Fixture {
    config: Config,
    _dir: tempfile::TempDir,
}

fn request_key_hash() -> String {
    connector_zk_note_mina::carried_key_hash().expect("the carried key")
}

fn fixture(graphql_url: &str, archive_url: &str, key_hash: &str) -> Fixture {
    let dir = tempfile::tempdir().expect("temp dir");
    let signer_key = dir.path().join("signer.key");
    std::fs::write(&signer_key, [7u8; 32]).expect("signer key");
    let state_key = dir.path().join("zk-state.key");
    std::fs::write(
        &state_key,
        connector_zk_note::SecKey::new(connector_zk_note::Scalar::from(9u64)).to_base58(),
    )
    .expect("state key");
    let vault = recorded()["vault"].as_str().unwrap().to_string();
    let text = format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state}"

[signer]
key_file = "{signer}"

[zk_note]
vault = "{vault}"
network = "mina:devnet"
graphql_url = "{graphql_url}"
archive_url = "{archive_url}"
state_key_file = "{state_key}"
verification_key_hash = "{key_hash}"
tiers = [100000000]
allowance_lifetime_secs = 3600
open_budget_cap = 1000000000
"#,
        state = dir.path().join("state").display(),
        signer = signer_key.display(),
        state_key = state_key.display(),
    );
    let path = dir.path().join("connector.toml");
    std::fs::write(&path, text).expect("config file");
    let config = Config::load(&path).expect("a valid zk edge config");
    Fixture { config, _dir: dir }
}

// A current-thread runtime, so no leftover connection task can end between the
// two counts and cancel out the sweep's +1.
#[tokio::test(flavor = "current_thread")]
async fn a_zk_edge_boots_reads_its_vault_publishes_its_terms_and_runs_the_sweep() {
    let (daemon, archive) = (daemon().await, StandIn::spawn(|_| None).await);
    let fixture = fixture(&daemon.url(), &archive.url(), &request_key_hash());
    let runtime = build(&fixture.config)
        .await
        .expect("a zk edge boots against a readable vault");
    assert!(
        daemon.calls().contains(&"SlotAnchor".to_string()),
        "boot reads the slot anchor: {:?}",
        daemon.calls()
    );
    assert!(runtime.zk_edge.is_some());

    // The allowance sweep is a task for the life of the node, and a node with
    // no zk edge starts none. Counted across `router()` alone: the tasks
    // boot's own reads leave alive are not the sweep.
    let alive_before = tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks();
    let app = router(&runtime, &fixture.config).expect("the node's router");
    let alive_after = tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks();
    assert!(
        alive_after > alive_before,
        "the sweep is spawned beside the router ({alive_before} -> {alive_after})"
    );

    // The terms: the self-description names the vault, the key hash and the
    // endpoint, the greeting's source.
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/ilp")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    let description: Value = serde_json::from_slice(&body).expect("a self-description");
    let zk = &description["zkNote"];
    assert_eq!(zk["payTo"], recorded()["vault"]);
    assert_eq!(zk["verificationKeyHash"], request_key_hash());
    assert_eq!(zk["allowanceEndpoint"], "/ilp/zk-note/allowances");
}

#[tokio::test(flavor = "multi_thread")]
async fn another_key_hash_refuses_boot_by_name_before_anything_is_dialled() {
    let (daemon, archive) = (daemon().await, StandIn::spawn(|_| None).await);
    let fixture = fixture(&daemon.url(), &archive.url(), "12345");
    let Err(error) = build(&fixture.config).await else {
        panic!("a key this build does not carry must not boot");
    };
    assert!(matches!(error, RuntimeError::ZkNoteKey(_)), "{error}");
    let message = error.to_string();
    assert!(message.contains("verification_key_hash"), "{message}");
    assert!(message.contains("12345"), "{message}");
    assert!(daemon.calls().is_empty(), "nothing was dialled");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_vault_that_cannot_be_read_at_boot_refuses_boot_by_name() {
    // A daemon that answers nothing it is asked.
    let (daemon, archive) = (
        StandIn::spawn(|_| None).await,
        StandIn::spawn(|_| None).await,
    );
    let fixture = fixture(&daemon.url(), &archive.url(), &request_key_hash());
    let Err(error) = build(&fixture.config).await else {
        panic!("a zk edge with no slot anchor must not boot");
    };
    assert!(
        matches!(error, RuntimeError::ZkNoteSlotAnchor(_)),
        "{error}"
    );
    assert!(error.to_string().contains("slot anchor"), "{error}");
    assert!(
        daemon.calls().len() > 1,
        "the read is retried a bounded number of times: {:?}",
        daemon.calls()
    );
}

/// POST an allowance request that binds this node's state key, over a note
/// with `nullifier` and a root the recorded vault holds (or does not).
async fn open_allowance(
    app: &Router,
    root: connector_zk_note::Field,
    nullifier: connector_zk_note::Field,
) -> (u16, Value) {
    use connector_zk_note::note::balance::commit;
    use connector_zk_note::note::policy::TIERS;
    use connector_zk_note::request::{expected_tag, json_proof_for, point, RequestPublic};
    use connector_zk_note::{Keypair, Scalar, SecKey};

    let state_key =
        connector_signer::ZkStateSigner::from_base58(&SecKey::new(Scalar::from(9u64)).to_base58())
            .expect("the fixture's state key")
            .public_key();
    let session_key = Keypair::try_from(SecKey::new(Scalar::from(11u64)))
        .expect("a keypair")
        .public
        .into_compressed();
    let expires_at_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap() + 1_800_000;
    let tier = TIERS[1];
    let public = RequestPublic {
        vault_x: state_key.x,
        vault_is_odd: state_key.is_odd,
        root,
        // The anchor is the recording's tip slot, read at boot, so the
        // current slot is within a slot of it.
        request_time: 1114,
        tier,
        nullifier,
        tag: expected_tag(nullifier, &state_key, &session_key, tier, expires_at_ms),
        commitment: point(commit(Scalar::from(4_000_000_000u64), Scalar::from(777u64))),
        subsidised: false,
        period: 0,
        period_nullifier: connector_zk_note::Field::from(0u64),
    };
    let proof: Value = serde_json::from_slice(&json_proof_for(&public, "not a proof")).unwrap();
    let body = json!({
        "proof": proof,
        "sessionKey": session_key.into_address(),
        "expiresAt": expires_at_ms.to_string(),
    });
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/ilp/zk-note/allowances")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn each_failed_read_refuses_one_allowance_by_name_and_the_node_keeps_serving() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let rec = recorded();
    let field =
        |value: &Value| -> connector_zk_note::Field { value.as_str().unwrap().parse().unwrap() };
    let note_root = field(&rec["vaultState"]["data"]["account"]["zkappState"][2]);
    let absent_root = field(&rec["named"]["absentRoot"]);
    let consumed = field(&rec["named"]["consumedNullifier"]);
    let absent = field(&rec["named"]["absentNullifier"]);

    let archive_down = Arc::new(AtomicBool::new(false));
    let down = Arc::clone(&archive_down);
    let actions = rec["actions"].clone();
    let archive =
        StandIn::spawn(move |_| (!down.load(Ordering::SeqCst)).then(|| actions.clone())).await;
    let daemon = daemon().await;
    let fixture = fixture(&daemon.url(), &archive.url(), &request_key_hash());
    let runtime = build(&fixture.config).await.expect("boots");
    let app = router(&runtime, &fixture.config).expect("router");

    // A root the vault never held, by name.
    let (status, body) = open_allowance(&app, absent_root, absent).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (422, Some("unknown_root"))
    );

    // A nullifier an escape spent on chain, by name: read from the archive.
    let (status, body) = open_allowance(&app, note_root, consumed).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (422, Some("nullifier_spent"))
    );

    // A proof the verifier refuses, by name.
    let (status, body) = open_allowance(&app, note_root, absent).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (422, Some("proof_invalid"))
    );

    // The archive goes down: that one allowance is refused as unavailable...
    archive_down.store(true, Ordering::SeqCst);
    let (status, body) = open_allowance(&app, note_root, absent).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (503, Some("vault_unavailable")),
        "{body}"
    );
    // ...and the node, which never stopped serving, serves the next one when
    // it is back.
    archive_down.store(false, Ordering::SeqCst);
    let (status, body) = open_allowance(&app, note_root, absent).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (422, Some("proof_invalid"))
    );
}
