//! What the batch-settlement end-to-end tests (ADR 0074) share: a config
//! loaded through the one loader the binary uses, a real app on its own
//! socket, and a sealed, voucher-carrying packet posted to a built node's
//! client edge.
//!
//! Not every test binary uses every helper, so dead code is allowed here
//! rather than in each caller.

#![allow(dead_code)]

use std::io::Write;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::Router;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use connector_config::Config;
use connector_domain::{EnvelopeRequest, Prepare};
use connector_signer::giftwrap::seal_request;
use connector_signer::PublicKeyBytes;
use tower::ServiceExt;

/// The client edge's journal under `state_dir`: the file a node's accepted
/// vouchers are recorded in.
pub const CLIENT_EDGE_JOURNAL: &str = "client-edge-claims.log";

/// Load `text` as a node's config file.
pub fn load_config(text: &str) -> Config {
    let mut file = tempfile::NamedTempFile::new().expect("temp config file");
    file.write_all(text.as_bytes()).expect("write config");
    Config::load(file.path()).expect("the config loads")
}

/// An app on its own socket, recording the body of every write it is sent.
pub async fn spawn_recording_app() -> (String, Arc<Mutex<Vec<Bytes>>>) {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new().route(
        "/",
        post({
            let recorded = Arc::clone(&recorded);
            move |body: Bytes| async move {
                recorded.lock().unwrap().push(body);
                StatusCode::OK
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the app");
    let addr = listener.local_addr().expect("the app's address");
    tokio::spawn(async move {
        axum::Server::from_tcp(listener)
            .expect("serve from the listener")
            .serve(router.into_make_service())
            .await
            .expect("the app serves");
    });
    (addr.to_string(), recorded)
}

/// A write to `destination`, sealed to the terminating node's `receiver`
/// key (ADR 0018).
pub fn paid_prepare(destination: &str, receiver: &PublicKeyBytes) -> Prepare {
    let envelope = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: b"a paid write".to_vec(),
    }
    .encode();
    let (data, _secret) = seal_request(&envelope, receiver).expect("seal");
    Prepare {
        amount: 0,
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
        greeting: false,
        destination: destination.to_string(),
        data,
    }
}

/// `POST /ilp` with `claim` in the claim header, returning the OER packet
/// the edge answers with.
pub async fn post_ilp(app: &Router, claim: &str, prepare: Prepare) -> Bytes {
    let request = Request::builder()
        .method("POST")
        .uri("/ilp")
        .header("ilp-payment-channel-claim", BASE64.encode(claim.as_bytes()))
        .body(Body::from(prepare.encode()))
        .expect("a request");
    let response = app.clone().oneshot(request).await.expect("an answer");
    assert_eq!(response.status(), StatusCode::OK);
    hyper::body::to_bytes(response.into_body())
        .await
        .expect("the body")
}
