//! PF-26 on the BTP carriage: the wait for a peer's answer ends at the
//! packet's outgoing expiry when that is sooner than the peering's answer
//! timeout, and is the answer timeout's otherwise -- the same rule
//! `connector-peer-http`'s twin of this file holds the other carriage to.
//!
//! The one fake is the socket: a dialer whose session swallows every frame
//! and answers none, so the bound under test is `BtpPeerTransport`'s own.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use connector_btp::{BtpSessionHandle, OutboundRequests};
use connector_domain::{PacketResponse, Prepare};
use connector_peer_btp::dial::{DialError, PeerDialer, PeerRelation};
use connector_peer_btp::BtpPeerTransport;
use connector_runtime::{PeerForward, PeerTransport};
use tokio::sync::mpsc;
use url::Url;

const PEER_ID: &str = "peer-b";

/// A peer that reads every frame and answers none.
struct SilentDialer {
    dials: AtomicUsize,
}

#[async_trait]
impl PeerDialer for SilentDialer {
    async fn dial(&self, _peer_id: &str, _endpoint: &Url) -> Result<BtpSessionHandle, DialError> {
        self.dials.fetch_add(1, Ordering::SeqCst);
        let (to_peer, mut to_peer_rx) = mpsc::channel::<Vec<u8>>(32);
        tokio::spawn(async move { while to_peer_rx.recv().await.is_some() {} });
        Ok(BtpSessionHandle::new(
            to_peer,
            Arc::new(OutboundRequests::new()),
        ))
    }
}

fn transport() -> (BtpPeerTransport, Arc<SilentDialer>) {
    let dialer = Arc::new(SilentDialer {
        dials: AtomicUsize::new(0),
    });
    let transport = BtpPeerTransport::new(Arc::clone(&dialer) as Arc<dyn PeerDialer>);
    transport.add_peer(PeerRelation::new(
        PEER_ID,
        Url::parse("wss://peer.example/btp").unwrap(),
        Duration::from_secs(30),
    ));
    (transport, dialer)
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
    let (transport, dialer) = transport();
    let started = tokio::time::Instant::now();

    let answer = transport
        .forward_within(PEER_ID, prepare(), None, Duration::from_secs(5))
        .await;

    assert!(answer.ran_out_at_expiry);
    assert!(!answer.reached_peer);
    assert_eq!(started.elapsed(), Duration::from_secs(5));
    assert_eq!(dialer.dials.load(Ordering::SeqCst), 1, "never retried");

    // The session is left as a timeout leaves it: forgotten, so the next
    // frame dials afresh.
    transport
        .forward_within(PEER_ID, prepare(), None, Duration::from_secs(5))
        .await;
    assert_eq!(dialer.dials.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn a_wait_that_ends_at_the_answer_timeout_is_t01_as_ever() {
    let (transport, _dialer) = transport();
    let started = tokio::time::Instant::now();

    let PeerForward {
        response,
        ran_out_at_expiry,
        ..
    } = transport
        .forward_within(PEER_ID, prepare(), None, Duration::from_secs(120))
        .await;

    assert!(!ran_out_at_expiry);
    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "T01"),
        other => panic!("expected the transport's own T01, got {other:?}"),
    }
    assert_eq!(started.elapsed(), Duration::from_secs(30));
}
