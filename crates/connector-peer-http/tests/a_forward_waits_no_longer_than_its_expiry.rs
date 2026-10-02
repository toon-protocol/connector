//! PF-26 on the ILP-over-HTTP carriage: the wait for a peer's answer ends at
//! the packet's outgoing expiry when that is sooner than the peering's answer
//! timeout, and is the answer timeout's otherwise.
//!
//! The one fake is the socket (a client that never answers), as in
//! `peer_carriage_http.rs`; the bound under test is `HttpPeerTransport`'s own.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use connector_domain::{PacketResponse, Prepare};
use connector_peer_http::dial::{HttpDialError, PeerHttpClient, PeerRelation};
use connector_peer_http::headers::{PeerRequest, PeerResponse};
use connector_peer_http::HttpPeerTransport;
use connector_runtime::{PeerForward, PeerTransport};
use url::Url;

const PEER_ID: &str = "peer-b";

/// A peer that takes the request and never answers it.
struct Silent;

#[async_trait]
impl PeerHttpClient for Silent {
    async fn post(
        &self,
        _endpoint: &Url,
        _request: PeerRequest,
    ) -> Result<PeerResponse, HttpDialError> {
        std::future::pending().await
    }
}

fn transport() -> HttpPeerTransport {
    let transport = HttpPeerTransport::new(Arc::new(Silent));
    transport.add_peer(PeerRelation::new(
        PEER_ID,
        Url::parse("https://peer.example:443/ilp").unwrap(),
        Duration::from_secs(30),
    ));
    transport
}

fn prepare() -> Prepare {
    Prepare {
        amount: 100,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: "g.example.app".to_string(),
        data: Vec::new(),
    }
}

#[tokio::test(start_paused = true)]
async fn a_wait_that_ends_at_the_expiry_is_reported_as_such() {
    let started = tokio::time::Instant::now();

    let answer = transport()
        .forward_within(PEER_ID, prepare(), None, Duration::from_secs(5))
        .await;

    assert!(answer.ran_out_at_expiry);
    assert!(!answer.reached_peer);
    assert_eq!(started.elapsed(), Duration::from_secs(5));
}

#[tokio::test(start_paused = true)]
async fn a_wait_that_ends_at_the_answer_timeout_is_t01_as_ever() {
    let started = tokio::time::Instant::now();

    let PeerForward {
        response,
        ran_out_at_expiry,
        ..
    } = transport()
        .forward_within(PEER_ID, prepare(), None, Duration::from_secs(120))
        .await;

    assert!(!ran_out_at_expiry);
    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "T01"),
        other => panic!("expected the transport's own T01, got {other:?}"),
    }
    assert_eq!(started.elapsed(), Duration::from_secs(30));
}

#[tokio::test(start_paused = true)]
async fn a_forward_with_no_bound_waits_for_the_answer_timeout() {
    let answer = transport().forward(PEER_ID, prepare(), None).await;
    assert!(!answer.ran_out_at_expiry);
}
