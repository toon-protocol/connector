//! **Accept**: an inbound peer BTP session, from its websocket upgrade to
//! its close (`peer-carriage-spec.md` §1, §3, §6, §7.1).
//!
//! # Role is a property of the frame, not of the session (§1.5)
//!
//! This session binds nothing. Until ADR 0060 it bound a role once, at the
//! `auth` frame, from a `{peerId, secret}` shared secret, and every later
//! frame on the socket rode that one decision. The secret is deleted, and
//! §1.5 inverted with it: **each frame stands on the claim it carries**,
//! and a frame carrying no claim that satisfies P2 and P3 is a client frame
//! however many peer frames preceded it here. That is strictly narrower
//! than the rule it replaces -- a session could previously prove itself
//! once and then send anything.
//!
//! What follows from it:
//!
//! * **A claim ingested as a client claim stays a client claim.** There is
//!   no history to rewrite: the role answers what *this* frame is, and
//!   §1.8's namespace disjointness is what keeps that safe.
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
//! **inline** -- decoding, the role decision, and above all claim
//! admission -- so claims on one session are judged strictly sequentially
//! in arrival order and cannot race each other into `nonce_not_advancing`.
//! Only the post-admission tail (routing, the downstream round trip,
//! writing the RESPONSE) overlaps, bounded by the same per-session
//! in-flight window `btp_session_window` sets. Losing that is the measured
//! ~125--150 events/s admission wall.
//!
//! Consequently RESPONSEs may leave in a different order than the MESSAGEs
//! that provoked them; `requestId` is the correlation, and a peer must not
//! infer which claim an ack answers from position (§7.1).
//!
//! # The client-role path is inert, on purpose
//!
//! §1.9's named regression is testable as: a client-role interaction moves
//! no peer watermark, appends nothing to the peer claim ledger, and gets no
//! `claim-ack`. Here a client-role session reaches none of the peer
//! pipeline at all -- its packets are
//! answered `F02` and its claims are not judged. Composing this carriage
//! onto the *shared* client listener, so that a client-role session falls
//! through to `connector-client-edge` instead, is the bring-up wiring of
//! issue #678; what §1 requires of this crate is that role is decided by
//! the frame's verified claim and that a client can never reach peer
//! handling, and that holds either way.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, RwLock};

use connector_btp::{
    decode_frame, encode_error, encode_response, reply, BtpDecodeError, BtpSessionHandle,
    OutboundRequests, ProtocolData, SessionGone, BTP_ERROR, BTP_MESSAGE, BTP_RESPONSE,
    BTP_TRANSFER,
};
use connector_domain::{Fulfill, PacketResponse, Prepare, Reject, RejectCode};
use connector_peer_auth::{
    claim_ack_to_emit, PeerAuthPolicy, PeerAuthRefusal, PeerAuthRefusalLog, SessionRole,
};
use connector_runtime::{ClaimAckOutcome, Connector, WireClaim};
use tokio::sync::mpsc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::price_gate::{self, ClaimEnforcementPolicy, PaymentRequired};
use crate::{ack, claim_json, fields, role_gate};

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
    /// mandatory authentication**. Role is *still* decided by P2 and P3 --
    /// the listener is defence in depth and MUST NOT become the decider,
    /// so §1.3 holds in full either way. What changes is only what happens
    /// to a frame that fails: on a dedicated listener it is refused
    /// outright (ERROR, then close) rather than downgraded to client, and
    /// that is safe *only* because such a listener serves no clients --
    /// there is no client to downgrade to and no oracle to leak.
    ///
    /// `false` (the default) is the shared-listener reading: a frame whose
    /// claim does not verify is an ordinary client frame, per §1.6's "MUST
    /// NOT refuse it for the assertion alone".
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

/// What a peering relation remembers between frames, and **across
/// connections** (§2.5): the claim currently at each channel's watermark,
/// and the channel this relation is known to identify itself by.
///
/// Per *relation*, never per carriage and never per connection -- that is
/// §2.5's rule, and maintaining it per-connection would be a double-spend
/// surface, since the same claim would advance two independent watermarks.
/// One of these is shared by every session from every peer.
///
/// # The idempotent re-ack (§6.3)
///
/// A lost ack and a lost claim are indistinguishable at the payer, so a
/// payer that was not acknowledged retransmits its latest pending claim --
/// byte-identical, if nothing has changed. A payee that answered such a
/// retransmission `nonce_not_advancing` would wedge the peering
/// permanently: the payer's only honest retransmission would be refused
/// forever, and minting a higher nonce for the same cumulative is
/// explicitly forbidden.
///
/// So: a claim whose `(channel, nonce, cumulative, signature)` is
/// byte-identical to the one already at the watermark is answered
/// `accepted`, and **nothing is advanced or recorded** -- there is nothing
/// to advance, the exposure covered is identical. A claim at the same
/// nonce differing in *any* other field is a different claim and is
/// refused `nonce_not_advancing`, exactly as §3.2's strictly-advancing rule
/// requires.
///
/// This record is in-memory and per-process. A restart loses it, and the
/// first retransmission after one is refused `nonce_not_advancing` until
/// the payer's next fulfilment produces a genuinely newer claim --
/// recovering it belongs with the claim journal's own durability (ADR
/// 0005), not with a carriage.
///
/// # What it is *not*: the money baseline
///
/// That cost is tolerable for the re-ack above, which is a per-relation,
/// per-process question, and is **not** tolerable for anything arithmetic.
/// The price-coverage gate (issue #880) read its prior watermark from here
/// and so measured a claim's advance against a record that a restart had
/// zeroed while `ClaimBook` had replayed its journal: the first priced peer
/// PREPARE after a payee restart was credited with its claim's whole
/// cumulative amount as new payment (issue #1104). Coverage now reads
/// [`connector_runtime::Connector::peer_channel_watermark`] -- the book
/// that actually judges the claim -- and this record answers the re-ack
/// alone.
#[derive(Debug, Default)]
pub struct AcceptedClaims {
    /// `(peer id, canonical channel id)` → the claim at that watermark.
    at_watermark: RwLock<HashMap<(String, String), WireClaim>>,
}

impl AcceptedClaims {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `claim` is byte-identical to the one already at this
    /// relation's watermark for that channel (§6.3).
    #[must_use]
    pub fn is_at_watermark(&self, peer_id: &str, claim: &WireClaim) -> bool {
        self.at_watermark
            .read()
            .expect("accepted claims lock poisoned")
            .get(&(peer_id.to_string(), claim.channel_id.clone()))
            == Some(claim)
    }

    /// Record `claim` as the one now at this relation's watermark.
    pub fn record(&self, peer_id: &str, claim: &WireClaim) {
        self.at_watermark
            .write()
            .expect("accepted claims lock poisoned")
            .insert(
                (peer_id.to_string(), claim.channel_id.clone()),
                claim.clone(),
            );
    }

    /// This relation's watermark for `channel_id` (domain
    /// [`connector_domain::Watermark`]) *as this process has seen it*,
    /// `None` if no claim has been recorded for it since this process
    /// started.
    ///
    /// **Never a baseline for money arithmetic.** `None` here does not mean
    /// "nothing has ever been claimed on this channel", only "not since
    /// this process started", and treating the two as the same is issue
    /// #1104. Anything measuring a claim's *advance* -- the price-coverage
    /// gate above all -- reads
    /// [`connector_runtime::Connector::peer_channel_watermark`], which is
    /// [`connector_runtime::ClaimBook`]'s own durable figure and the one
    /// the claim is about to be judged against. What this answers is what
    /// this record is for: what one relation has observed in one process.
    #[must_use]
    pub fn watermark(
        &self,
        peer_id: &str,
        channel_id: &str,
    ) -> Option<connector_domain::Watermark> {
        self.at_watermark
            .read()
            .expect("accepted claims lock poisoned")
            .get(&(peer_id.to_string(), channel_id.to_string()))
            .map(|claim| connector_domain::Watermark {
                nonce: claim.nonce,
                cumulative_amount: claim.cumulative_amount,
            })
    }
}

/// Everything a peer session needs that outlives it: the one pipeline
/// below the port, the role policy, the per-relation ledger, the price-gate
/// enforcement policy (issue #883, child B6), and the rate-limited
/// `peer_auth_refused` log.
pub struct PeerCarriageState {
    connector: Arc<Connector>,
    auth: Arc<PeerAuthPolicy>,
    accepted: Arc<AcceptedClaims>,
    enforcement: Arc<ClaimEnforcementPolicy>,
    refusals: Mutex<PeerAuthRefusalLog>,
    policy: PeerAcceptPolicy,
}

impl PeerCarriageState {
    #[must_use]
    pub fn new(
        connector: Arc<Connector>,
        auth: Arc<PeerAuthPolicy>,
        accepted: Arc<AcceptedClaims>,
        enforcement: Arc<ClaimEnforcementPolicy>,
        policy: PeerAcceptPolicy,
    ) -> Self {
        PeerCarriageState {
            connector,
            auth,
            accepted,
            enforcement,
            refusals: Mutex::new(PeerAuthRefusalLog::default()),
            policy,
        }
    }
}

/// One inbound peer session: its outbound `requestId` space (§2.3 -- a BTP
/// session is symmetric once established, so either side may originate on
/// it) and its in-flight window.
///
/// It holds no role. Role is a property of the frame (§1.5), decided from
/// the claim each frame carries, so there is nothing session-lived left to
/// keep here.
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
    /// §1.10: a dedicated peer listener refused a frame that failed P2 or
    /// P3, and closed.
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
        // -- never the first, never the last, never a concatenation.
        let raw = match claim_json::present_from_protocol_data(&frame.protocol_data) {
            Ok(raw) => raw,
            Err(_) => {
                self.send(encode_error(
                    frame.request_id,
                    "F00",
                    "NotAcceptedError",
                    b"more than one claim entry on one frame",
                ))
                .await?;
                return Ok(None);
            }
        };

        // **Role, from this frame's own claim** (§1.2, §1.5): decoded and
        // verified before anything is judged, routed, charged or journaled,
        // and re-decided on every frame because a claim proves the frame it
        // rides on and no other.
        let claim = raw.and_then(|raw| self.decode_claim(raw));
        let (role, refusal) =
            role_gate::decide(&self.state.connector, &self.state.auth, claim.as_ref()).into_parts();
        self.report_refusal(refusal.as_ref());

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
            // FLUSH (§3): a TRANSFER whose `amount` is the claim's new
            // cumulative, carrying the claim and **no** `ilpPacket`.
            BTP_TRANSFER => {
                self.handle_flush(frame.request_id, frame.amount, &role, claim)
                    .await?;
                Ok(None)
            }
            _ => {
                self.handle_message(frame.request_id, &role, claim, &frame.ilp_packet)
                    .await?;
                Ok(None)
            }
        }
    }

    /// The claim this frame carries, decoded (§4). `None` when the frame
    /// carries no claim entry at all, and also when it carries one this
    /// connector could not read -- an undecodable claim is *not
    /// acknowledged* (§6.3) rather than rejected, so the payer's claim
    /// stays pending and its retransmission is read the same way instead of
    /// being recorded as a verdict that was never reached.
    fn decode_claim(&self, raw: &[u8]) -> Option<WireClaim> {
        match claim_json::parse(raw) {
            Ok(claim) => Some(claim),
            Err(error) => {
                // No peer id to name: the claim *is* what would have named
                // one, and it did not decode.
                tracing::warn!(%error, "peer claim could not be decoded; not acknowledged");
                None
            }
        }
    }

    /// §1.6's loud half. A claim naming a configured peer channel that
    /// fails P2 or P3 is an *assertion*; the frame is a client frame and is
    /// not refused for the assertion alone -- refusing would make the check
    /// an oracle for which peerings this connector has configured -- but a
    /// silent downgrade would present to an operator as "peering
    /// configured, nothing peers, no error anywhere". The rate-limited
    /// event is what stops that.
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
                "a peer channel's claim did not verify; the frame is a client frame"
            );
        }
    }

    async fn send(&self, frame: Vec<u8>) -> Result<(), SessionGone> {
        reply(&self.replies, frame).await
    }

    /// FLUSH (§3.3, §3): a TRANSFER carrying the claim alone.
    async fn handle_flush(
        &mut self,
        request_id: u32,
        amount: Option<u64>,
        role: &SessionRole,
        claim: Option<WireClaim>,
    ) -> Result<(), SessionGone> {
        let ack = self.judge_claim(role, claim.as_ref());
        if let (Some(amount), Some(claim)) = (amount, claim.as_ref()) {
            if amount != claim.cumulative_amount {
                // §10.2 item 13 pins the equality. The claim is the truth
                // (ADR 0005) and is judged either way; a disagreeing
                // `amount` is a peer bug worth naming, not a second
                // opinion to reconcile.
                tracing::warn!(
                    transfer_amount = amount,
                    claim_cumulative = claim.cumulative_amount,
                    "peer FLUSH amount disagrees with its claim's cumulative"
                );
            }
        }
        let entries: Vec<ProtocolData> = self.claim_ack_entry(role, ack).into_iter().collect();
        // §6.1: the ack rides the RESPONSE that already answers the
        // claim-bearing TRANSFER. RFC-0023 requires the responder answer
        // every request, which is exactly what bounds the ack structurally.
        self.send(encode_response(request_id, &entries, &[])).await
    }

    /// A MESSAGE: a PREPARE, or a claim standing alone on one.
    async fn handle_message(
        &mut self,
        request_id: u32,
        role: &SessionRole,
        claim: Option<WireClaim>,
        ilp_packet: &[u8],
    ) -> Result<(), SessionGone> {
        // Peeked before `judge_claim` below may advance this channel's
        // watermark, so the price-coverage check further down judges the
        // claim's own advance past the watermark it rode in on, not the one
        // it just became (issue #880).
        //
        // It is read from the book that is about to judge the claim --
        // `ClaimBook`'s own durable inbound watermark, keyed by channel as
        // that book keys it -- and never from `AcceptedClaims`, which is
        // in-memory and per-process. Reading the per-process record made
        // coverage disagree with the judgement across a restart: the book
        // replays its journal and the record does not, so the first priced
        // peer PREPARE after a restart was credited with its claim's whole
        // cumulative amount as new payment (issue #1104).
        // The peer role gates the read (§1.5 does not read a client's
        // claim at all) and is no part of it: a channel's watermark is a
        // property of the channel, which is how `ClaimBook` keys it.
        let prior_watermark = role.peer_id().and(claim.as_ref()).and_then(|claim| {
            self.state
                .connector
                .peer_channel_watermark(&claim.channel_id)
        });

        // Claims are judged **inline, in arrival order** (§7.1) -- before
        // the packet is even decoded, and before anything is spawned.
        let ack = self.judge_claim(role, claim.as_ref());

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
            claim.as_ref(),
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
            // `handle_peer_prepare` is handed no claim -- this frame's was
            // judged inline above, in order. It IS handed the peering the
            // frame arrived over, which is the incoming half of ADR 0071's
            // denomination boundary (issue #1295): the session is
            // authenticated, so this carriage knows who is on the other end
            // of it and a forward out of this packet can be priced.
            let (response, _) = state
                .connector
                .handle_peer_prepare(Some(&peer_id), prepare, None)
                .await;
            let frame = encode_packet_response(&role, request_id, response, ack);
            let _ = reply(&replies, frame).await;
        });
        Ok(())
    }

    /// A claim's verdict (§6.1), or [`ClaimAckOutcome::NotSent`] when
    /// there was no readable claim to judge or the frame was a client's.
    fn judge_claim(&self, role: &SessionRole, claim: Option<&WireClaim>) -> ClaimAckOutcome {
        // §1.5: a client's claim is not judged here at all -- the peer
        // namespace is not reachable from a client frame (§1.8). A frame
        // whose claim did not verify *is* a client frame, so this is also
        // what keeps a bad signature from touching a watermark.
        let (Some(peer_id), Some(claim)) = (role.peer_id(), claim) else {
            return ClaimAckOutcome::NotSent;
        };

        // §6.3's idempotent re-ack, checked **before** the claim reaches
        // the book: a byte-identical retransmission at the current
        // watermark is `accepted`, and nothing is advanced or recorded.
        if self.state.accepted.is_at_watermark(peer_id, claim) {
            return ClaimAckOutcome::Accepted;
        }

        let ack = self.state.connector.handle_peer_claim(claim.clone());
        if ack == ClaimAckOutcome::Accepted {
            self.state.accepted.record(peer_id, claim);
        }
        ack
    }

    /// §1.7: a connector MUST NOT emit a `claim-ack` on a client
    /// interaction, and §6.2 forbids one on a response answering a frame
    /// that carried no claim. Both are one call.
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
