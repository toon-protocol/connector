//! Issue #1240, at the socket: a dialed BTP session must **stop reporting
//! itself usable** the moment the connection under it goes away.
//!
//! Everything else in this crate proves the carriage's behaviour without a
//! socket, because [`connector_peer_btp::dial::PeerDialer`] is a port. This
//! one test is the exception it needs, and it is the exception because the
//! defect lived precisely in the half a port hides: `ws`'s dialed session
//! splits the websocket, and its two halves fail independently. A peer that
//! restarts closes the connection, which the read half sees at once and the
//! write half learns only on its next write -- so the writer sat waiting on
//! a channel that was still open over a socket that was not, the dial side
//! read that channel as a live session, wrote the next PREPARE into it and
//! then waited out the answer timeout for a RESPONSE no read loop was left
//! to deliver.
//!
//! What is asserted here is the repair, in the terms a caller has: the
//! session says it is gone, and a send on it fails **at once** rather than
//! hanging until `OUTBOUND_ANSWER_TIMEOUT`.
//!
//! # Why `ws://` is the faithful scheme for this
//!
//! The live occurrences were on `wss://`, and this dials `ws://`. The
//! scheme selects nothing but the TLS layer `connect_async` puts under the
//! websocket: `dial` splits whatever stream comes back and spawns the same
//! two tasks over it either way, and the defect is entirely in how those
//! two tasks end. A `wss://` arm would also have to be dialed against a
//! certificate this crate's client could trust, which it cannot be -- the
//! `tokio-tungstenite` build here is pinned to webpki roots, so there is no
//! seam through which a test CA could be offered. There is no `wss://`
//! coverage anywhere in the workspace; closing that gap needs a root-store
//! seam and is not this issue's to open.

use std::time::Duration;

use connector_btp::{BtpSessionHandle, OriginateError};
use connector_peer_btp::dial::PeerDialer;
use connector_peer_btp::TungsteniteDialer;
use tokio::net::TcpListener;
use url::Url;

/// How long a test is willing to wait for something that must happen
/// promptly. Comfortably shorter than `OUTBOUND_ANSWER_TIMEOUT`, which is
/// what the unrepaired code waited out instead.
const PROMPTLY: Duration = Duration::from_secs(3);

/// Accepts exactly one websocket, then closes it on demand -- a payee whose
/// connector goes down under a peering that is already up.
async fn a_peer_that_restarts() -> (Url, tokio::sync::oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
    let endpoint = Url::parse(&format!(
        "ws://{}/ilp/btp",
        listener.local_addr().expect("bound")
    ))
    .expect("a well-formed endpoint");
    let (restart, restarted) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("the dial arrives");
        let socket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("the websocket handshake completes");
        let _ = restarted.await;
        // The process went away: the socket goes with it, unannounced.
        drop(socket);
    });
    (endpoint, restart)
}

async fn wait_until_gone(handle: &BtpSessionHandle) -> bool {
    let deadline = tokio::time::Instant::now() + PROMPTLY;
    while tokio::time::Instant::now() < deadline {
        if handle.is_gone() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[tokio::test]
async fn a_dialed_session_reports_itself_gone_once_its_socket_is() {
    let (endpoint, restart) = a_peer_that_restarts().await;
    let handle = TungsteniteDialer::new()
        .dial("peer-b", &endpoint)
        .await
        .expect("the peer is listening");
    assert!(
        !handle.is_gone(),
        "a session that just dialed a live peer is not gone"
    );

    drop(restart);

    assert!(
        wait_until_gone(&handle).await,
        "the socket died and the session did not notice: this is #1240, and the \
         next packet is refused T01 on a peering nobody redialled"
    );
}

/// The same death, as the send path meets it. `SessionGone` is the answer
/// that says the frame was never written -- which is what makes redialling
/// and sending once more the same packet rather than a second one.
#[tokio::test]
async fn a_send_on_a_session_whose_socket_died_fails_at_once_rather_than_waiting_out_the_answer() {
    let (endpoint, restart) = a_peer_that_restarts().await;
    let handle = TungsteniteDialer::new()
        .dial("peer-b", &endpoint)
        .await
        .expect("the peer is listening");
    drop(restart);
    assert!(wait_until_gone(&handle).await, "the socket died");

    let sent = tokio::time::timeout(PROMPTLY, handle.send_message(&[], b"not a packet"))
        .await
        .expect("a send on a dead session answers without waiting out the answer timeout");

    assert!(
        matches!(sent, Err(OriginateError::SessionGone)),
        "expected SessionGone, got {sent:?}"
    );
}
