//! **Accept**: an inbound peer BTP session, from its websocket upgrade to
//! its close (`peer-carriage-spec.md` §1, §3, §6, §7.1).
//!
//! # Role is a property of the frame, not of the session (§1.5)
//!
//! This session binds nothing. Until ADR 0060 it bound a role once, at the
//! `auth` frame, from a `{peerId, secret}` shared secret, and every later
//! frame on the socket rode that one decision. The secret is deleted, and
//! §1.5 inverted with it: **each frame stands on the evidence it carries**
//! -- a voucher from a bound signer's channel, or the peer-role challenge on
//! a packet that moves no value (ADR 0075 decision 5) -- and a frame
//! carrying neither is a client frame however many peer frames preceded it
//! here. A `toon-channel` claim is never such evidence (#1380).
//!
//! What follows from it:
//!
//! * **A voucher ingested as a client's stays a client's.** There is no
//!   history to rewrite: the role answers what *this* frame is, and the
//!   channel's one watermark (§1.8) is what keeps that safe.
//! * **An `auth` entry means nothing here.** A receiver ignores one rather
//!   than answering an ERROR (ADR 0060), so the two ends of a peering may
//!   be upgraded in either order without going dark mid-flight. A MESSAGE
//!   carrying nothing but an `auth` entry is answered with the same empty
//!   RESPONSE it always was -- through the ordinary claimless-frame path,
//!   not through a branch that knows the entry's name.
//!
//! # Ordering (§7.1)
//!
//! Deliberately the client edge's shape, reusing its mechanism rather than
//! a peer-specific one: the session task runs everything order-sensitive
//! **inline** -- decoding, the role decision, and above all voucher
//! admission -- so vouchers on one session are judged strictly sequentially
//! in arrival order and cannot race each other into `amount_not_advancing`.
//! Only the post-admission tail (routing, the downstream round trip,
//! writing the RESPONSE) overlaps, bounded by the same per-session
//! in-flight window `btp_session_window` sets. Losing that is the measured
//! ~125--150 events/s admission wall.
//!
//! Consequently RESPONSEs may leave in a different order than the MESSAGEs
//! that provoked them; `requestId` is the correlation, and a peer must not
//! infer which voucher an ack answers from position (§7.1).
//!
//! # The client-role path is inert, on purpose
//!
//! §1.9's named regression is testable as: a client-role interaction is
//! judged as no peering's payment and gets no `claim-ack`. Here a
//! client-role session reaches none of the peer pipeline at all -- its
//! packets are answered `F02` and its vouchers are not judged. Composing
//! this carriage onto the *shared* client listener, so that a client-role
//! session falls through to `connector-client-edge` instead, is the
//! bring-up wiring of issue #678; what §1 requires of this crate is that
//! role is decided by the frame's verified evidence and that a client can
//! never reach peer handling, and that holds either way.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use connector_btp::{
    decode_frame, encode_error, encode_response, reply, BtpDecodeError, BtpSessionHandle,
    OutboundRequests, ProtocolData, SessionGone, BTP_ERROR, BTP_MESSAGE, BTP_RESPONSE,
    BTP_TRANSFER,
};
use connector_domain::client_claim::ClientClaim;
use connector_domain::{Fulfill, PacketResponse, Prepare, Reject, RejectCode};
use connector_peer_auth::{claim_ack_to_emit, PeerAuthRefusal, PeerAuthRefusalLog, SessionRole};
use connector_runtime::{ClaimAckOutcome, Connector};
use tokio::sync::mpsc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::price_gate::{self, ClaimEnforcementPolicy, PaymentRequired};
use crate::role_gate::VoucherEvidence;
use crate::{ack, fields, role_gate};

/// How many completed replies may queue for the socket's writer before a
/// finishing frame waits its turn -- burst smoothing between out-of-order
/// completions and the one socket, not admission control (that is the
/// in-flight window's job). The client edge's own figure, for the same
/// reason.
pub const REPLY_QUEUE_DEPTH: usize = 32;

/// The per-session concurrency bound (§7.1), matching the client edge's
/// `btp_session_window` default.
pub const DEFAULT_PEER_SESSION_WINDOW: u32 = 16;

/// How this connector accepts peer sessions.
#[derive(Debug, Clone, Copy)]
pub struct PeerAcceptPolicy {
    /// §1.10's bounded escape hatch: a **dedicated peer listener with
    /// mandatory authentication**. Role is *still* decided by the frame's
    /// evidence --
    /// the listener is defence in depth and MUST NOT become the decider,
    /// so §1.3 holds in full either way. What changes is only what happens
    /// to a frame that fails: on a dedicated listener it is refused
    /// outright (ERROR, then close) rather than downgraded to client, and
    /// that is safe *only* because such a listener serves no clients --
    /// there is no client to downgrade to and no oracle to leak.
    ///
    /// `false` (the default) is the shared-listener reading: a frame whose
    /// evidence does not verify is an ordinary client frame, per §1.6's
    /// "MUST NOT refuse it for the assertion alone".
    pub mandatory_auth: bool,
    /// How many frames may be past admission and not yet answered (§7.1).
    pub session_window: NonZeroU32,
}

impl Default for PeerAcceptPolicy {
    fn default() -> Self {
        PeerAcceptPolicy {
            mandatory_auth: false,
            session_window: NonZeroU32::new(DEFAULT_PEER_SESSION_WINDOW)
                .expect("the default window is non-zero"),
        }
    }
}

/// Everything a peer session needs that outlives it: the one pipeline
/// below the port, the price-gate enforcement policy (issue #883, child
/// B6), the receiving half vouchers are judged by, and the rate-limited
/// `peer_auth_refused` log.
pub struct PeerCarriageState {
    connector: Arc<Connector>,
    enforcement: Arc<ClaimEnforcementPolicy>,
    refusals: Mutex<PeerAuthRefusalLog>,
    policy: PeerAcceptPolicy,
    /// The receiving half a voucher or a peer-role challenge is resolved
    /// through (ADR 0075 decision 5). `None` -- [`Self::new`]'s default --
    /// and neither proves the peer role on this carriage.
    vouchers: Option<Arc<dyn VoucherEvidence>>,
}

impl PeerCarriageState {
    #[must_use]
    pub fn new(
        connector: Arc<Connector>,
        enforcement: Arc<ClaimEnforcementPolicy>,
        policy: PeerAcceptPolicy,
    ) -> Self {
        PeerCarriageState {
            connector,
            enforcement,
            refusals: Mutex::new(PeerAuthRefusalLog::default()),
            policy,
            vouchers: None,
        }
    }

    /// Resolve vouchers and peer-role challenges through `vouchers`, so that
    /// either proves the peer role on a channel bound to a peering (ADR 0075
    /// decision 5).
    #[must_use]
    pub fn with_voucher_evidence(mut self, vouchers: Arc<dyn VoucherEvidence>) -> Self {
        self.vouchers = Some(vouchers);
        self
    }
}

/// One inbound peer session: its outbound `requestId` space (§2.3 -- a BTP
/// session is symmetric once established, so either side may originate on
/// it) and its in-flight window.
///
/// It holds no role. Role is a property of the frame (§1.5), decided from
/// the evidence each frame carries, so there is nothing session-lived left
/// to keep here.
pub struct PeerSession {
    state: Arc<PeerCarriageState>,
    outbound: Arc<OutboundRequests>,
    replies: mpsc::Sender<Vec<u8>>,
    window: Arc<Semaphore>,
}

/// Why a session ended.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionEnd {
    /// The peer closed, or the frame source ran dry.
    Closed,
    /// The socket's send half is gone; nothing further could be answered.
    Gone,
    /// §1.10: a dedicated peer listener refused a frame that did not prove
    /// the peer role, and closed.
    Refused,
}

impl PeerSession {
    #[must_use]
    pub fn new(state: Arc<PeerCarriageState>, replies: mpsc::Sender<Vec<u8>>) -> Self {
        Self::with_outbound(state, replies, Arc::new(OutboundRequests::new()))
    }

    /// A session over an `outbound` table somebody else already holds --
    /// what a **dialed** session needs (§2.3): one socket, one read loop,
    /// and both halves of RFC-23's symmetric grammar on it. The dialing
    /// side reserves request ids through its [`BtpSessionHandle`] while
    /// this session resolves the answers and serves whatever the far side
    /// originates, and a second correlation table would mean answers
    /// resolving against the wrong one.
    #[must_use]
    pub fn with_outbound(
        state: Arc<PeerCarriageState>,
        replies: mpsc::Sender<Vec<u8>>,
        outbound: Arc<OutboundRequests>,
    ) -> Self {
        let window = Arc::new(Semaphore::new(state.policy.session_window.get() as usize));
        PeerSession {
            state,
            outbound,
            replies,
            window,
        }
    }

    /// The handle this session hands out for **originating** a MESSAGE or
    /// TRANSFER on it. §2.3: on BTP a session is symmetric once
    /// established, so the side that *accepted* it can originate too --
    /// which is the whole of the difference between the carriages, and why
    /// BTP has no `Toon-Flush-Requested` analogue and needs none (§6.4).
    #[must_use]
    pub fn handle(&self) -> BtpSessionHandle {
        BtpSessionHandle::new(self.replies.clone(), Arc::clone(&self.outbound))
    }

    /// Read frames until the peer closes or the socket dies.
    pub async fn run(mut self, mut frames: mpsc::Receiver<Vec<u8>>) -> SessionEnd {
        while let Some(bytes) = frames.recv().await {
            match self.handle_frame(&bytes).await {
                Ok(None) => {}
                Ok(Some(end)) => return end,
                Err(SessionGone) => return SessionEnd::Gone,
            }
        }
        SessionEnd::Closed
    }

    /// Process one frame. `Ok(None)` continues the session; `Ok(Some(_))`
    /// ends it deliberately.
    pub async fn handle_frame(
        &mut self,
        frame_bytes: &[u8],
    ) -> Result<Option<SessionEnd>, SessionGone> {
        let frame = match decode_frame(frame_bytes) {
            Ok(frame) => frame,
            // No readable `requestId`, so no ERROR can correlate.
            Err(BtpDecodeError::TooShort) => return Ok(None),
            Err(BtpDecodeError::Malformed { request_id, reason }) => {
                // ERROR stays reserved for undecodable frames (§6.2).
                self.send(encode_error(
                    request_id,
                    "F00",
                    "NotAcceptedError",
                    reason.as_bytes(),
                ))
                .await?;
                return Ok(None);
            }
        };

        // The answer to a request *this* side originated on this session
        // (§7.3): correlate and stop. One this connector never originated
        // resolves to nothing and is dropped, which is ordinary.
        if frame.frame_type == BTP_RESPONSE || frame.frame_type == BTP_ERROR {
            self.outbound.resolve(frame);
            return Ok(None);
        }

        // A frame type this grammar does not have. Ignored rather than
        // errored: the carriage stays additively extensible (§3). An `auth`
        // entry riding one of the two it does have is ignored the same way
        // and for the same reason -- ADR 0060 deleted the credential, and a
        // receiver that answered `400`/ERROR to an arriving one would make
        // the two ends of a peering un-upgradable in either order.
        if frame.frame_type != BTP_TRANSFER && frame.frame_type != BTP_MESSAGE {
            return Ok(None);
        }

        // §1.5's smuggling defence, counted before anything is parsed:
        // more than one claim entry on one frame is refused, not resolved
        // -- never the first, never the last, never a concatenation. The
        // same holds for the peer-role challenge (ADR 0075 decision 5), and
        // for a claim beside a challenge: two pieces of authentication
        // material, and "which one did we check?" has no answer.
        let evidence = match role_gate::btp_evidence(&frame) {
            Ok(evidence) => evidence,
            Err(ambiguous) => {
                self.send(encode_error(
                    frame.request_id,
                    "F00",
                    "NotAcceptedError",
                    &ambiguous.message(),
                ))
                .await?;
                return Ok(None);
            }
        };

        // **Role, from this frame's own evidence** (§1.2, §1.5): decoded and
        // verified before anything is judged, routed, charged or journaled,
        // and re-decided on every frame because a voucher or a challenge
        // proves the frame it rides on and no other.
        let (role, refusal) = role_gate::decide_frame(
            &self.state.connector,
            self.state.vouchers.as_deref(),
            &evidence,
        )
        .await
        .into_parts();
        self.report_refusal(refusal.as_ref());
        let voucher = evidence.voucher().cloned();

        // §1.10: on a dedicated peer listener a failure is refused
        // outright rather than downgraded, because such a listener serves
        // no clients -- there is no client to downgrade to and no oracle to
        // leak.
        if self.state.policy.mandatory_auth && !role.is_peer() {
            self.send(encode_error(
                frame.request_id,
                "F00",
                "NotAcceptedError",
                b"this listener serves peers only",
            ))
            .await?;
            return Ok(Some(SessionEnd::Refused));
        }

        match frame.frame_type {
            // A TRANSFER was the `toon-channel` FLUSH (§3): a claim standing
            // alone. No peering sends one any more (ADR 0075, #1380) -- a
            // voucher rides the PREPARE it covers -- so there is nothing to
            // judge, and RFC-0023's "answer every request" is kept with an
            // empty RESPONSE that acknowledges nothing.
            BTP_TRANSFER => {
                self.send(encode_response(frame.request_id, &[], &[]))
                    .await?;
                Ok(None)
            }
            _ => {
                self.handle_message(frame.request_id, &role, voucher.as_ref(), &frame.ilp_packet)
                    .await?;
                Ok(None)
            }
        }
    }

    /// §1.6's loud half. A voucher or challenge from a bound signer's channel
    /// that does not verify is an *assertion*; the frame is a client frame
    /// and is not refused for the assertion alone -- refusing would make the
    /// check an oracle for which peerings this connector has configured --
    /// but a silent downgrade would present to an operator as "peering
    /// configured, nothing peers, no error anywhere". The rate-limited event
    /// is what stops that.
    fn report_refusal(&self, refusal: Option<&PeerAuthRefusal>) {
        let Some(refusal) = refusal else {
            return;
        };
        let report = self
            .state
            .refusals
            .lock()
            .expect("peer auth refusal log poisoned")
            .observe(refusal, now_ms());
        if let Some(report) = report {
            tracing::warn!(
                event = report.event,
                peer_id = %report.peer_id,
                unmet = report.unmet.name(),
                suppressed = report.suppressed,
                "a peer channel's voucher did not verify; the frame is a client frame"
            );
        }
    }

    async fn send(&self, frame: Vec<u8>) -> Result<(), SessionGone> {
        reply(&self.replies, frame).await
    }

    /// A MESSAGE: a PREPARE, or a voucher standing alone on one.
    async fn handle_message(
        &mut self,
        request_id: u32,
        role: &SessionRole,
        voucher: Option<&ClientClaim>,
        ilp_packet: &[u8],
    ) -> Result<(), SessionGone> {
        // A peer's voucher (ADR 0075 decision 6): judged by the receiving
        // half, against the channel's one watermark, **inline and in
        // arrival order** (§7.1) -- before the packet is even decoded, and
        // before anything is spawned. Its prior watermark is what the
        // price-coverage check below measures the voucher's own advance
        // from, read from the same durable book that judges it (issue
        // #1104's rule).
        let judged_voucher =
            role_gate::judge_voucher(role, self.state.vouchers.as_deref(), voucher).await;

        let (ack, claimed, prior_watermark) = match judged_voucher {
            Some(judged) => (judged.ack, judged.claimed, judged.prior),
            None => (ClaimAckOutcome::NotSent, None, None),
        };

        if ilp_packet.is_empty() {
            let entries: Vec<ProtocolData> = self.claim_ack_entry(role, ack).into_iter().collect();
            return self.send(encode_response(request_id, &entries, &[])).await;
        }

        let Some(peer_id) = role.peer_id().map(str::to_string) else {
            // A client-role packet reaches no peer handling at all: no
            // watermark, no ledger, no ack (§1.7, §1.9).
            return self
                .send(self.reject_response(
                    role,
                    request_id,
                    Reject {
                        code: RejectCode::f02_unreachable(),
                        triggered_by: String::new(),
                        message: "no peer route for this interaction".to_string(),
                        data: Vec::new(),
                        accumulated_cost: 0,
                    },
                    ClaimAckOutcome::NotSent,
                ))
                .await;
        };

        let prepare = match Prepare::decode(ilp_packet) {
            Ok(prepare) => prepare,
            Err(error) => {
                // The HTTP carriage's 400: a transport-level answer, not
                // an ILP-level one, so an ERROR rather than a REJECT.
                return self
                    .send(encode_error(
                        request_id,
                        "F00",
                        "NotAcceptedError",
                        error.to_string().as_bytes(),
                    ))
                    .await;
            }
        };

        // Issue #880 (owner decision #868) and ADR 0042: a peer PREPARE
        // carries a covering claim -- the route's `price` where this
        // connector terminates, the packet's own `amount` where it forwards
        // -- or it is refused with the client edge's own x402 greeting. The
        // decision is `price_gate`'s, shared with the HTTP carriage so
        // §0.1's one pipeline cannot admit over one carriage what it
        // refuses over the other; what is this carriage's is only the frame
        // the refusal is shaped into.
        if let Some(refusal) = price_gate::payment_required(
            &self.state.connector,
            &peer_id,
            &prepare,
            ack,
            claimed,
            prior_watermark,
            self.state.enforcement.mode(&peer_id),
        ) {
            return self
                .send(self.payment_required_response(role, request_id, refusal, ack))
                .await;
        }

        let permit = window_slot(&self.window).await;
        let state = Arc::clone(&self.state);
        let replies = self.replies.clone();
        let role = role.clone();
        tokio::spawn(async move {
            let _slot = permit;
            // Everything past admission, overlapping up to the window
            // (§7.1): routing and the downstream round trip.
            // `handle_peer_prepare` is handed no voucher -- this frame's was
            // judged inline above, in order. It IS handed the peering the
            // frame arrived over, which is the incoming half of ADR 0071's
            // denomination boundary (issue #1295): the session is
            // authenticated, so this carriage knows who is on the other end
            // of it and a forward out of this packet can be priced.
            let response = state
                .connector
                .handle_peer_prepare(Some(&peer_id), prepare)
                .await;
            let frame = encode_packet_response(&role, request_id, response, ack);
            let _ = reply(&replies, frame).await;
        });
        Ok(())
    }

    /// §1.7: a connector MUST NOT emit a `claim-ack` on a client
    /// interaction, and §6.2 forbids one on a response answering a frame
    /// that carried no voucher. Both are one call.
    fn claim_ack_entry(&self, role: &SessionRole, ack: ClaimAckOutcome) -> Option<ProtocolData> {
        claim_ack_to_emit(role, ack::protocol_data(ack))
    }

    fn reject_response(
        &self,
        role: &SessionRole,
        request_id: u32,
        reject: Reject,
        ack: ClaimAckOutcome,
    ) -> Vec<u8> {
        encode_packet_response(role, request_id, PacketResponse::Reject(reject), ack)
    }

    /// [`price_gate::payment_required`]'s refusal, BTP-shaped: `F06` plus
    /// the greeting as protocolData, exactly like the client edge's own BTP
    /// carriage answers a claimless request (`connector-client-edge`'s
    /// `btp` module), since BTP cannot answer HTTP `402`. The claim ack
    /// still rides this same RESPONSE (§6.1) -- the packet's own refusal
    /// and the claim's verdict are independent (§6.2).
    fn payment_required_response(
        &self,
        role: &SessionRole,
        request_id: u32,
        refusal: PaymentRequired,
        ack: ClaimAckOutcome,
    ) -> Vec<u8> {
        let mut entries = vec![
            fields::accumulated_cost_protocol_data(refusal.reject.accumulated_cost),
            fields::payment_required_protocol_data(refusal.terms),
        ];
        entries.extend(self.claim_ack_entry(role, ack));
        encode_response(request_id, &entries, &refusal.reject.encode())
    }
}

/// The RESPONSE answering a PREPARE: **two independent answers on one
/// frame** (§6.2). `ilpPacket` answers the packet; the `claim-ack` entry
/// answers the claim. A rejected claim never becomes an ERROR frame and
/// never changes the packet's own outcome, its `accumulatedCost` or its fee
/// accounting -- and a fulfilled packet can carry a `rejected` ack, which is
/// the property whose loss would silently destroy ADR 0024's semantics.
fn encode_packet_response(
    role: &SessionRole,
    request_id: u32,
    response: PacketResponse,
    ack: ClaimAckOutcome,
) -> Vec<u8> {
    let ack_entry = claim_ack_to_emit(role, ack::protocol_data(ack));
    match response {
        PacketResponse::Fulfill(fulfill) => {
            // §5.2: `toon-accumulated-cost` rides **only** a REJECT. It is
            // never emitted beside a FULFILL.
            let entries: Vec<ProtocolData> = ack_entry.into_iter().collect();
            encode_response(request_id, &entries, &Fulfill::encode(&fulfill))
        }
        PacketResponse::Reject(reject) => {
            // §5.2: always emitted on a REJECT, even at zero, so "absent"
            // never has to carry meaning in the direction that matters.
            let mut entries = vec![fields::accumulated_cost_protocol_data(
                reject.accumulated_cost,
            )];
            entries.extend(ack_entry);
            encode_response(request_id, &entries, &reject.encode())
        }
    }
}

async fn window_slot(window: &Arc<Semaphore>) -> OwnedSemaphorePermit {
    Arc::clone(window)
        .acquire_owned()
        .await
        .expect("the session window semaphore is never closed")
}

/// A monotonic-enough millisecond reading for the refusal log's rate
/// limit. Wall clock is fine: the log's own contract says a reading that
/// goes backwards closes the window early rather than suppressing forever.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
