//! A Mina daemon and archive node as a test sees them: a real HTTP server on
//! a real socket answering GraphQL by operation name, with the answers
//! zk_toon recorded from a real vault on its proofs-on lightnet (zk_toon#67).
//!
//! It is a fake, not a mock (ADR 0007): it holds answers and asserts no call
//! sequence. [`StandIn::calls`] exists for the tests whose subject is
//! traffic itself (which socket a dial reached).

#![allow(dead_code)] // each test binary uses a part of this

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use connector_chain_rpc::graphql::GraphqlRpc;
use connector_chain_rpc::{Route, RpcTransport, Timeouts};
use serde_json::{json, Value};

/// zk_toon's recording: the daemon's and archive's answers for a vault, and
/// the request proofs made over its tree.
pub const RECORDED_VAULT: &str = include_str!("../../fixtures/recorded-vault.json");
pub const REQUEST_PROOFS: &str = include_str!("../../fixtures/request-proofs.json");

pub fn recorded() -> Value {
    serde_json::from_str(RECORDED_VAULT).expect("the recording")
}

pub fn proofs() -> Value {
    serde_json::from_str(REQUEST_PROOFS).expect("the request proofs")
}

/// The tip's slot and its block's time. **Constructed**, not recorded:
/// zk_toon's recording asked the daemon for the slot only, and the anchor
/// needs the block's timestamp beside it (`utcDate`, ms since the epoch).
/// The slot is the recording's own tip.
pub const ANCHOR_SLOT: u64 = 1114;
pub const ANCHOR_TIME_MS: u64 = 1_793_243_264_164;

pub fn slot_anchor_answer() -> Value {
    json!({ "data": { "bestChain": [{ "protocolState": {
        "consensusState": { "slotSinceGenesis": ANCHOR_SLOT.to_string() },
        "blockchainState": { "utcDate": ANCHOR_TIME_MS.to_string() },
    } }] } })
}

/// What the stand-in does with one request.
#[derive(Clone)]
pub enum Reply {
    /// HTTP 200 with this body.
    Json(Value),
    /// A bare status, the way a CDN or a rate limiter answers.
    Status(u16),
    /// Never answer.
    Hang,
    /// Wait, then do the inner reply.
    Slow(Duration, Box<Reply>),
}

type Script = dyn Fn(&str) -> Reply + Send + Sync;

pub struct StandIn {
    addr: SocketAddr,
    calls: Arc<Mutex<Vec<String>>>,
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

#[derive(Clone)]
struct Shared {
    script: Arc<Script>,
    calls: Arc<Mutex<Vec<String>>>,
}

async fn serve(State(shared): State<Shared>, body: Bytes) -> Response {
    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let operation = request["operationName"].as_str().unwrap_or("").to_string();
    shared.calls.lock().unwrap().push(operation.clone());
    let mut reply = (shared.script)(&operation);
    loop {
        match reply {
            Reply::Json(value) => return Json(value).into_response(),
            Reply::Status(code) => {
                return (StatusCode::from_u16(code).unwrap(), "no").into_response()
            }
            Reply::Hang => std::future::pending::<()>().await,
            Reply::Slow(wait, inner) => {
                tokio::time::sleep(wait).await;
                reply = *inner;
            }
        }
    }
}

impl StandIn {
    pub async fn spawn(script: impl Fn(&str) -> Reply + Send + Sync + 'static) -> StandIn {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let shared = Shared {
            script: Arc::new(script),
            calls: Arc::clone(&calls),
        };
        let app = Router::new().route("/", post(serve)).with_state(shared);
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

    /// A daemon that answers the recorded vault (and the constructed anchor).
    pub async fn daemon() -> StandIn {
        let state = recorded()["vaultState"].clone();
        StandIn::spawn(move |operation| match operation {
            "VaultState" => Reply::Json(state.clone()),
            "SlotAnchor" => Reply::Json(slot_anchor_answer()),
            other => panic!("the daemon was asked for {other}"),
        })
        .await
    }

    /// An archive that answers the recorded actions.
    pub async fn archive() -> StandIn {
        let actions = recorded()["actions"].clone();
        StandIn::spawn(move |operation| match operation {
            "VaultActions" => Reply::Json(actions.clone()),
            other => panic!("the archive was asked for {other}"),
        })
        .await
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Every operation this server was asked for, in order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

/// Bounds short enough that a test watches them fire.
pub const SHORT: Timeouts = Timeouts {
    connect: Duration::from_millis(500),
    request: Duration::from_millis(500),
    pool_idle: Duration::from_millis(500),
};

pub fn direct(url: &str, timeouts: Timeouts) -> GraphqlRpc {
    GraphqlRpc::new(RpcTransport::new(url, Route::Direct, timeouts).expect("a transport"))
}

/// One verifier for the whole test binary: building the index is seconds of
/// CPU, and every test would otherwise pay it.
pub async fn verifier() -> std::sync::Arc<connector_zk_verifier::Verifier> {
    static VERIFIER: tokio::sync::OnceCell<std::sync::Arc<connector_zk_verifier::Verifier>> =
        tokio::sync::OnceCell::const_new();
    VERIFIER
        .get_or_init(|| async {
            let hash = connector_zk_note_mina::carried_key_hash().expect("the carried key");
            connector_zk_note_mina::request_verifier(&hash)
                .await
                .expect("a verifier under the carried key")
        })
        .await
        .clone()
}

pub const VAULT: &str = "B62qnKkNiS3r6vR6n7mFawPZKG6KhmWeutUbToDXEecSpnqWkptkhoU";

/// The adapter over a daemon and an archive, both direct.
pub async fn vault_over(
    daemon: &StandIn,
    archive: &StandIn,
    timeouts: Timeouts,
) -> connector_zk_note_mina::MinaZkNoteVault {
    connector_zk_note_mina::MinaZkNoteVault::new(
        VAULT.to_string(),
        direct(&daemon.url(), timeouts),
        direct(&archive.url(), timeouts),
        verifier().await,
    )
}
