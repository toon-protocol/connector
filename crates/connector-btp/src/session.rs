//! Session framing: the writer channel a completed frame is queued on, the
//! session-scoped outbound `requestId` allocator, the demux table that
//! correlates the answer to a request this connector originated, and the
//! handle a carriage hands out for originating one.
//!
//! Nothing here reads the socket. Which frames arrive in what order, and
//! what is order-sensitive about them, belongs to the carriage that owns the
//! read loop -- see `connector-client-edge`'s `btp` module for the client
//! edge's ordering contract (issue #688).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::frame::{encode_message, encode_transfer, BtpFrame, ProtocolData};

/// The session's send half is gone -- the writer task exited, so no reply
/// can ever be delivered again and the session loop should end.
#[derive(Debug)]
pub struct SessionGone;

/// Queue one reply frame for the writer task.
pub async fn reply(replies: &mpsc::Sender<Vec<u8>>, frame: Vec<u8>) -> Result<(), SessionGone> {
    replies.send(frame).await.map_err(|_| SessionGone)
}

// ─── server-originated MESSAGE/TRANSFER (issue #697, RFC-0023's symmetric
// grammar) ───
//
// The deployed dialect has the server only ever answer -- it "never
// originates a requestId" (the client edge's original framing, still true of
// every id `decode_frame` reads from an inbound frame there). RFC-23 says the
// two sides "play identical roles" after auth, so this connector needs its own
// outbound id space and a way to correlate the RESPONSE/ERROR that answers
// one of its own requests. The two namespaces (client-originated,
// server-originated) never collide *by meaning* even if a value repeats:
// each side tracks only the requestIds *it* is waiting on, and a RESPONSE is
// addressed to whichever side sent the request it answers -- there is no
// shared "one pending-map for the whole socket" the way a naive reading of
// "requestId correlates" might suggest. What RFC-23's uniqueness rule
// ("care must be taken so that duplicate IDs are never in-flight at the
// same time") binds is this connector's own outbound ids against each
// other, which is exactly what [`OutboundRequests::reserve`] guarantees.
//
// The client edge originates nothing today -- the session registry that will
// hold a [`BtpSessionHandle`] per authenticated counterparty and decide *when*
// to push a payout MESSAGE or settle a TRANSFER is toon-meta#262's work, and
// the peer carriage of ADR 0027 (issue #676) is the other caller. This module
// ships the mechanics -- allocate, send, correlate -- proven by its own tests.

/// How long a server-originated request waits for its RESPONSE/ERROR before
/// giving up and freeing its requestId. Generous relative to any downstream
/// round-trip a carriage otherwise waits on (the claim journal's fsync,
/// app delivery) because the other side of this wait is a websocket peer
/// that may be doing real work before it answers, not a local call.
pub const OUTBOUND_ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// One session's outbound requestId space and the RESPONSE/ERROR each
/// pending id is still waiting for. `next_id` is a plain incrementing
/// counter (RFC-23 permits sequential ids explicitly) rather than random,
/// since uniqueness here only has to hold against this session's *own*
/// in-flight requests, which `pending` tracks directly. A carriage
/// constructs one of these per session so [`resolve`](Self::resolve)
/// -- the inbound-correlation half -- is live even before anything calls
/// [`reserve`](Self::reserve) to originate a request.
pub struct OutboundRequests {
    next_id: AtomicU32,
    pending: Mutex<HashMap<u32, oneshot::Sender<BtpFrame>>>,
}

impl Default for OutboundRequests {
    fn default() -> Self {
        Self::new()
    }
}

impl OutboundRequests {
    pub fn new() -> Self {
        Self {
            // Start at 1: every existing test and the deployed client both
            // treat 0 as an ordinary id, but starting there is needless
            // overlap with the low ids a client's own counter is likely to
            // pick, and this session's ids are free to start anywhere.
            next_id: AtomicU32::new(1),
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Allocate a requestId with nothing else currently pending under it,
    /// and the receiver its eventual RESPONSE/ERROR arrives on. Skipping a
    /// colliding id (rather than trusting the wraparound never happens)
    /// is what makes the RFC's uniqueness property hold even after `u32`
    /// wraps on a long-lived session.
    pub fn reserve(&self) -> (u32, oneshot::Receiver<BtpFrame>) {
        let (tx, rx) = oneshot::channel();
        let mut pending = self.pending.lock().expect("not poisoned");
        let mut id = self.next_id.fetch_add(1, Ordering::Relaxed);
        while pending.contains_key(&id) {
            id = self.next_id.fetch_add(1, Ordering::Relaxed);
        }
        pending.insert(id, tx);
        (id, rx)
    }

    /// An inbound RESPONSE/ERROR frame arrived; if its `requestId` names a
    /// request this session originated and is still waiting on, deliver it
    /// there. Returns whether it did -- a `false` here is ordinary: it is
    /// what every RESPONSE/ERROR the client edge receives resolves to today,
    /// since nothing there originates a request for one to answer.
    pub fn resolve(&self, frame: BtpFrame) -> bool {
        let sender = self
            .pending
            .lock()
            .expect("not poisoned")
            .remove(&frame.request_id);
        match sender {
            Some(sender) => {
                let _ = sender.send(frame);
                true
            }
            None => false,
        }
    }

    /// Free a reservation nobody will ever resolve -- the wait timed out.
    pub fn cancel(&self, request_id: u32) {
        self.pending
            .lock()
            .expect("not poisoned")
            .remove(&request_id);
    }
}

/// Why a server-originated request went unanswered.
#[derive(Debug, PartialEq, Eq)]
pub enum OriginateError {
    /// The socket's send half is gone; the request was never written.
    SessionGone,
    /// The request was written but no RESPONSE/ERROR arrived within
    /// [`OUTBOUND_ANSWER_TIMEOUT`].
    Timeout,
}

/// A handle a session hands out for originating a MESSAGE or TRANSFER on
/// it -- the RFC-23 half the deployed dialect never had. Cloning shares the
/// same underlying session (the writer channel and the correlation table), so
/// more than one caller can hold one at once.
#[derive(Clone)]
pub struct BtpSessionHandle {
    replies: mpsc::Sender<Vec<u8>>,
    outbound: Arc<OutboundRequests>,
}

impl BtpSessionHandle {
    pub fn new(replies: mpsc::Sender<Vec<u8>>, outbound: Arc<OutboundRequests>) -> Self {
        Self { replies, outbound }
    }

    /// Write `frame_bytes` (already encoded under the id `rx` was reserved
    /// for) and wait for the RESPONSE/ERROR that answers it, freeing the
    /// reservation on every exit path -- sent-and-answered frees it in
    /// [`OutboundRequests::resolve`], everything else frees it here.
    async fn await_answer(
        &self,
        request_id: u32,
        rx: oneshot::Receiver<BtpFrame>,
        frame_bytes: Vec<u8>,
    ) -> Result<BtpFrame, OriginateError> {
        if reply(&self.replies, frame_bytes).await.is_err() {
            self.outbound.cancel(request_id);
            return Err(OriginateError::SessionGone);
        }
        match tokio::time::timeout(OUTBOUND_ANSWER_TIMEOUT, rx).await {
            Ok(Ok(frame)) => Ok(frame),
            Ok(Err(_)) => Err(OriginateError::SessionGone),
            Err(_) => {
                self.outbound.cancel(request_id);
                Err(OriginateError::Timeout)
            }
        }
    }

    /// Whether this session is dead: its writer is gone, so nothing more
    /// can be written on it and nothing more can ever answer.
    ///
    /// The counterpart of [`OriginateError::SessionGone`], readable
    /// *before* a caller commits a frame to it. A carriage that caches a
    /// session across packets -- the peer carriage's dial side does -- needs
    /// that: a peer's restart leaves a handle behind whose session died with
    /// the socket, and answering `T01` off it rather than redialling is
    /// issue #1240. It is a one-way door, `false` until the writer exits and
    /// `true` forever after, so a `false` may always be stale by the time it
    /// is read; the send's own `SessionGone` is what settles it.
    ///
    /// This is only as truthful as the carriage that owns the socket: a
    /// writer task that outlives its read loop reports a live session over
    /// a dead socket. `connector-peer-btp`'s `ws` module ties the two
    /// together for exactly this reason.
    #[must_use]
    pub fn is_gone(&self) -> bool {
        self.replies.is_closed()
    }

    /// Originate a MESSAGE, allocating its requestId, and wait for the
    /// RESPONSE/ERROR it provokes.
    pub async fn send_message(
        &self,
        protocol_data: &[ProtocolData],
        ilp_packet: &[u8],
    ) -> Result<BtpFrame, OriginateError> {
        let (request_id, rx) = self.outbound.reserve();
        let frame_bytes = encode_message(request_id, protocol_data, ilp_packet);
        self.await_answer(request_id, rx, frame_bytes).await
    }

    /// Originate a TRANSFER, allocating its requestId, and wait for the
    /// RESPONSE/ERROR it provokes.
    pub async fn send_transfer(
        &self,
        amount: u64,
        protocol_data: &[ProtocolData],
    ) -> Result<BtpFrame, OriginateError> {
        let (request_id, rx) = self.outbound.reserve();
        let frame_bytes = encode_transfer(request_id, amount, protocol_data);
        self.await_answer(request_id, rx, frame_bytes).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{
        decode_frame, BTP_ERROR, BTP_MESSAGE, BTP_RESPONSE, BTP_TRANSFER, CONTENT_TYPE_TEXT,
    };

    // ─── issue #697: RFC-0023's symmetric grammar ───

    #[test]
    fn outbound_requests_never_hands_out_an_id_still_pending() {
        let outbound = OutboundRequests::new();
        let (first, _rx1) = outbound.reserve();
        let (second, _rx2) = outbound.reserve();
        assert_ne!(first, second);
        // Force a collision: rewind the counter to `first` and reserve
        // again -- the still-pending entry for `first` must be skipped.
        outbound.next_id.store(first, Ordering::Relaxed);
        let (third, _rx3) = outbound.reserve();
        assert_ne!(
            third, first,
            "an id with a pending receiver is never reused"
        );
        assert_ne!(third, second);
    }

    #[test]
    fn resolve_delivers_the_frame_to_the_reservations_receiver() {
        let outbound = OutboundRequests::new();
        let (id, mut rx) = outbound.reserve();
        let answer = BtpFrame {
            frame_type: BTP_RESPONSE,
            request_id: id,
            amount: None,
            protocol_data: Vec::new(),
            ilp_packet: b"fulfilled".to_vec(),
        };
        assert!(outbound.resolve(answer));
        let delivered = rx.try_recv().expect("the receiver got the frame");
        assert_eq!(delivered.ilp_packet, b"fulfilled".to_vec());
    }

    #[test]
    fn resolve_of_an_id_nothing_is_waiting_on_is_a_harmless_no_op() {
        // Every RESPONSE/ERROR the client edge receives today, absent a
        // caller of `BtpSessionHandle` -- must not panic or affect
        // anything else pending.
        let outbound = OutboundRequests::new();
        let (real_id, mut rx) = outbound.reserve();
        let stray = BtpFrame {
            frame_type: BTP_ERROR,
            request_id: real_id.wrapping_add(1),
            amount: None,
            protocol_data: Vec::new(),
            ilp_packet: Vec::new(),
        };
        assert!(!outbound.resolve(stray));
        assert!(
            rx.try_recv().is_err(),
            "the real reservation is untouched by an unrelated id"
        );
    }

    #[test]
    fn cancel_frees_the_id_so_a_late_resolve_is_a_no_op() {
        let outbound = OutboundRequests::new();
        let (id, _rx) = outbound.reserve();
        outbound.cancel(id);
        let late = BtpFrame {
            frame_type: BTP_RESPONSE,
            request_id: id,
            amount: None,
            protocol_data: Vec::new(),
            ilp_packet: Vec::new(),
        };
        assert!(
            !outbound.resolve(late),
            "a cancelled reservation answers nothing"
        );
    }

    /// End-to-end through the real production types -- `BtpSessionHandle`
    /// encodes and writes a MESSAGE, a stand-in "peer" reads the encoded
    /// bytes off the same `replies` channel a session's writer task reads
    /// from, decodes them with the same `decode_frame` a real session uses,
    /// and answers by calling `OutboundRequests::resolve` exactly as a
    /// carriage's RESPONSE/ERROR branch does on a real inbound frame. The
    /// only thing not exercised here is the websocket transport itself,
    /// which `connector-client-edge`'s `tests/btp_session.rs` already covers
    /// for the inbound direction.
    #[tokio::test]
    async fn an_originated_message_is_answered_through_resolve() {
        let (replies, mut reply_rx) = mpsc::channel::<Vec<u8>>(1);
        let outbound = Arc::new(OutboundRequests::new());
        let handle = BtpSessionHandle::new(replies, Arc::clone(&outbound));

        let peer = tokio::spawn(async move {
            let sent = reply_rx.recv().await.expect("the MESSAGE was written");
            let decoded = decode_frame(&sent).expect("the connector's own encoder");
            assert_eq!(decoded.frame_type, BTP_MESSAGE);
            outbound.resolve(BtpFrame {
                frame_type: BTP_RESPONSE,
                request_id: decoded.request_id,
                amount: None,
                protocol_data: Vec::new(),
                ilp_packet: b"peer answered".to_vec(),
            });
        });

        let pd = vec![ProtocolData {
            name: "payout-notice".to_string(),
            content_type: CONTENT_TYPE_TEXT,
            data: b"increment 3".to_vec(),
        }];
        let answer = handle
            .send_message(&pd, &[])
            .await
            .expect("the peer answered before the timeout");
        assert_eq!(answer.ilp_packet, b"peer answered".to_vec());
        peer.await.expect("the peer task");
    }

    /// The TRANSFER analogue of the MESSAGE round trip above: the amount
    /// rides the wire, the peer answers, and the originator's `await`
    /// resolves with that answer.
    #[tokio::test]
    async fn an_originated_transfer_is_answered_through_resolve() {
        let (replies, mut reply_rx) = mpsc::channel::<Vec<u8>>(1);
        let outbound = Arc::new(OutboundRequests::new());
        let handle = BtpSessionHandle::new(replies, Arc::clone(&outbound));

        let peer = tokio::spawn(async move {
            let sent = reply_rx.recv().await.expect("the TRANSFER was written");
            let decoded = decode_frame(&sent).expect("the connector's own encoder");
            assert_eq!(decoded.frame_type, BTP_TRANSFER);
            assert_eq!(decoded.amount, Some(500_000));
            outbound.resolve(BtpFrame {
                frame_type: BTP_RESPONSE,
                request_id: decoded.request_id,
                amount: None,
                protocol_data: Vec::new(),
                ilp_packet: Vec::new(),
            });
        });

        let answer = handle
            .send_transfer(500_000, &[])
            .await
            .expect("the peer answered before the timeout");
        assert!(answer.ilp_packet.is_empty());
        peer.await.expect("the peer task");
    }

    /// The socket's send half is gone before the request could even be
    /// written -- `OriginateError::SessionGone`, and the reservation is
    /// freed rather than leaked.
    #[tokio::test]
    async fn originating_on_a_dead_session_reports_session_gone_and_frees_the_id() {
        let (replies, reply_rx) = mpsc::channel::<Vec<u8>>(1);
        drop(reply_rx);
        let outbound = Arc::new(OutboundRequests::new());
        let handle = BtpSessionHandle::new(replies, Arc::clone(&outbound));

        let error = handle
            .send_message(&[], &[])
            .await
            .expect_err("nothing could ever read the write");
        assert_eq!(error, OriginateError::SessionGone);
    }
}
