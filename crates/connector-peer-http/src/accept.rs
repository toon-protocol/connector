//! **Accept**: an inbound peer HTTP request (`peer-carriage-spec.md` §1,
//! §3, §6, §7.2).
//!
//! # An interaction is one request (§1.1)
//!
//! Which is now barely a difference at all. Role is a property of the
//! **frame** on either carriage (§1.5, as amended by #868): each arrival
//! stands on the claim it carries, and there is nothing session-lived on
//! either side for a role to outlive. What HTTP still has that BTP does not
//! is that a request is answered exactly once and always, which is what
//! bounds the claim ack structurally (§6.3).
//!
//! What is called rather than re-derived:
//!
//! * [`connector_peer_btp::role_gate::decide`] joins §1.2's P2/P3 rule to
//!   the claim book's verdict on a signature, and is the same call the BTP
//!   carriage makes, so §0.1's one pipeline cannot admit over one carriage
//!   what it refuses over the other. Its
//!   [`RoleDecision`](connector_peer_auth::RoleDecision) carries the role
//!   and the `peer_auth_refused` event together, so the silent downgrade
//!   cannot ship without the loud event;
//! * the claim, its verdict and the per-relation watermark ledger are
//!   `connector-peer-btp`'s ([`AcceptedClaims`]), because §2.5/I6 make them
//!   per **peering relation**, never per carriage -- a peering with two paths
//!   is still one relation, and a second ledger would be a double-spend
//!   surface.
//!
//! # This is not the client edge's `POST /ilp`, and must never become it
//!
//! The pipeline below the port is shared; **the admission is not**. ADR 0026
//! was written to dissolve exactly this blur, and the devnet incident §1.9
//! names -- `toon-sandbox` admitting an anonymous BTP session with
//! `success:true mode:"no-auth"` and then treating it as a quasi-peer -- is
//! what happens when the two audiences meet in one handler. Here a
//! client-role request reaches no peer handling at all: its claim is not
//! judged, no watermark moves, nothing is appended to the peer claim ledger,
//! and no `Toon-Claim-Ack` is emitted. Falling a client-role request
//! through to `connector-client-edge` instead of
//! answering it `F02` is the bring-up wiring of issue #678; what §1 requires
//! of *this* module is that role is decided by the request's verified claim
//! and that a client can never reach peer handling, and that holds either
//! way.
//!
//! # `Toon-Peer-Auth` is ignored, not refused
//!
//! ADR 0060 deleted the `{peerId, secret}` credential that header carried.
//! A request still setting one is read exactly as one that does not -- no
//! `400`, no log line, no branch -- so the two ends of a peering may be
//! upgraded in either order without the peering going dark mid-flight.
//!
//! # The accept-only side (§6.4)
//!
//! On HTTP an accept-only side cannot originate, so it is structurally a
//! **payee**: it can never forward a packet to that peer, and it cannot
//! prompt a payer that has simply stopped sending the way a live BTP
//! session's liveness can. Before ADR 0031/ADR 0033 (issue #882) this was
//! bounded by a configured `ceiling`, the accept-only side's only real
//! bound (`ConfigError::AcceptOnlyPeerWithoutCeiling`); that requirement is
//! retired along with the credit window it protected, since every peer
//! PREPARE now carries its own covering claim regardless of which side can
//! originate.
//!
//! What this module still owns is §6.4's prompt: a payee that cannot
//! originate MAY ask a payer to flush, with [`FlushHints`]. It is a hint and
//! only a hint -- it creates no obligation, and a payer that ignores every
//! one of them is not in violation of the specification.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::{Arc, RwLock};

use connector_btp::{
    ACCUMULATED_COST_HEADER, CLAIM_ACK_HEADER, CLAIM_HEADER, FLUSH_REQUESTED_HEADER,
    PAYMENT_REQUIRED_HEADER,
};
use connector_domain::{Fulfill, PacketResponse, Prepare, Reject, RejectCode};
use connector_peer_auth::{
    claim_ack_to_emit, Capability, PeerAuthPolicy, PeerAuthRefusal, PeerAuthRefusalLog, SessionRole,
};
use connector_peer_btp::claim_json::{self};
use connector_peer_btp::price_gate::{self, ClaimEnforcementPolicy, PaymentRequired};
use connector_peer_btp::role_gate;
use connector_peer_btp::AcceptedClaims;
use connector_runtime::{ClaimAckOutcome, Connector, WireClaim};

use crate::headers::{self, PeerRequest, PeerResponse};

/// How this connector accepts peer requests.
#[derive(Debug, Clone, Copy, Default)]
pub struct PeerHttpPolicy {
    /// §1.10's bounded escape hatch: a **dedicated peer listener with
    /// mandatory authentication**. Role is *still* decided by P2 and P3 --
    /// the listener is defence in depth and MUST NOT become the decider, so
    /// §1.3 holds in full either way. What changes is only what happens to a
    /// request that fails: on a dedicated listener it is refused outright
    /// (`401`) rather than downgraded to client, and that is safe *only*
    /// because such a listener serves no clients -- there is no client to
    /// downgrade to and no oracle to leak.
    ///
    /// `false` (the default) is the shared-listener reading: a request whose
    /// claim does not verify is an ordinary client request, per §1.6's "MUST
    /// NOT refuse it for the assertion alone".
    pub mandatory_auth: bool,
}

/// §6.4's flush prompt, from the side that cannot originate.
///
/// A payee that cannot dial has no way to prompt a payer that has simply
/// stopped sending, and unlike BTP it has no live session to read liveness
/// from. `Toon-Flush-Requested` is the one thing it can do about that, and
/// the specification is emphatic about how little it means: it creates no
/// obligation, and a payer that ignores it is conforming.
///
/// A hint is *drained* when it is emitted. Repeating it on every response
/// until the claim arrived would name a channel many times in one exchange
/// for no gain; whoever knows a claim is still owed re-requests it.
#[derive(Debug, Default)]
pub struct FlushHints {
    by_peer: RwLock<HashMap<String, HashSet<String>>>,
}

impl FlushHints {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask `peer_id` to flush its pending claim on `channel_id`, on the next
    /// response this connector sends it.
    pub fn request(&self, peer_id: &str, channel_id: &str) {
        self.by_peer
            .write()
            .expect("flush hints lock poisoned")
            .entry(peer_id.to_string())
            .or_default()
            .insert(claim_json::canonical_evm_channel_id(channel_id));
    }

    /// The channels to name on a response to `peer_id`, in a stable order,
    /// removing them: **a payee SHOULD NOT name the same channel more than
    /// once in one response** (§6.4).
    #[must_use]
    pub fn take(&self, peer_id: &str) -> Vec<String> {
        let mut by_peer = self.by_peer.write().expect("flush hints lock poisoned");
        let Some(channels) = by_peer.remove(peer_id) else {
            return Vec::new();
        };
        let mut channels: Vec<String> = channels.into_iter().collect();
        channels.sort();
        channels
    }
}

/// Everything an inbound peer request needs: the one pipeline below the
/// port, the role policy, the per-relation ledger, the rate-limited
/// `peer_auth_refused` log, and §6.4's prompt.
pub struct PeerHttpState {
    connector: Arc<Connector>,
    auth: Arc<PeerAuthPolicy>,
    accepted: Arc<AcceptedClaims>,
    enforcement: Arc<ClaimEnforcementPolicy>,
    hints: Arc<FlushHints>,
    refusals: Mutex<PeerAuthRefusalLog>,
    policy: PeerHttpPolicy,
}

impl PeerHttpState {
    /// `accepted` is deliberately shared with whatever other carriage serves
    /// the same peerings (§2.5, I6): one peering relation has one set of
    /// watermarks however many paths it has, and giving each carriage its own
    /// would let one claim advance two independent watermarks. `enforcement`
    /// (issue #883, child B6) is shared for the same reason `auth` is: one
    /// peering has one migration state, whichever carriage it rides.
    #[must_use]
    pub fn new(
        connector: Arc<Connector>,
        auth: Arc<PeerAuthPolicy>,
        accepted: Arc<AcceptedClaims>,
        enforcement: Arc<ClaimEnforcementPolicy>,
        hints: Arc<FlushHints>,
        policy: PeerHttpPolicy,
    ) -> Self {
        PeerHttpState {
            connector,
            auth,
            accepted,
            enforcement,
            hints,
            refusals: Mutex::new(PeerAuthRefusalLog::default()),
            policy,
        }
    }

    /// §6.4: ask `peer_id` to flush `channel_id` on the next response.
    pub fn request_flush(&self, peer_id: &str, channel_id: &str) {
        self.hints.request(peer_id, channel_id);
    }

    /// Answer one peer request.
    ///
    /// **Role is decided before anything else happens** (§1.5): before a
    /// claim is decoded, before a watermark is consulted, before a packet is
    /// routed, and before any fee or journal accounting.
    pub async fn handle(&self, request: PeerRequest) -> PeerResponse {
        // §1.5's smuggling defence, counted before anything is parsed: more
        // than one claim header on one request is refused, not resolved --
        // never the first, never the last, never a concatenation. `400`,
        // with no ILP body.
        if request.headers.get_all(CLAIM_HEADER).len() > 1 {
            return PeerResponse::refused(400);
        }

        // **Role, from this request's own claim** (§1.2, §1.5): decoded and
        // verified before anything is judged, routed, charged or journaled.
        // Decoded once, here, and reused for the price-coverage check
        // further down.
        let claim = claim_on(&request);
        let (role, refusal) =
            role_gate::decide(&self.connector, &self.auth, claim.as_ref()).into_parts();
        self.report_refusal(refusal.as_ref());

        // §1.10: on a dedicated peer listener a failure is refused outright
        // rather than downgraded, because such a listener serves no clients.
        if self.policy.mandatory_auth && !role.is_peer() {
            return PeerResponse::refused(401);
        }

        // The watermark is read *before* `judge_claim` below may advance
        // it, so that check judges the claim's own advance past the
        // watermark it rode in on, not the one it just became (issue #880).
        //
        // It is read from the book that is about to judge the claim --
        // `ClaimBook`'s own durable inbound watermark, keyed by channel as
        // that book keys it -- and never from `AcceptedClaims`, which is
        // in-memory and per-process. Reading the per-process record made
        // coverage disagree with the judgement across a restart: the book
        // replays its journal and the record does not, so the first priced
        // peer PREPARE after a restart was credited with its claim's whole
        // cumulative amount as new payment (issue #1104).
        let prior_watermark = role
            .peer_id()
            .and(claim.as_ref())
            .and_then(|claim| self.connector.peer_channel_watermark(&claim.channel_id));

        let ack = self.judge_claim(&role, claim.as_ref());

        // FLUSH (§3): a POST with an **empty ILP body** plus the claim
        // header. The ack rides the response that already answers it -- HTTP
        // always answers, which is what bounds the ack structurally (§6.3).
        if request.body.is_empty() {
            return self.finish(&role, PeerResponse::ok(Vec::new()), ack);
        }

        let Some(peer_id) = role.peer_id().map(str::to_string) else {
            // A client-role packet reaches no peer handling at all: no
            // watermark, no ledger, no ack (§1.7, §1.9).
            return self.finish(
                &role,
                packet_response(PacketResponse::Reject(Reject {
                    code: RejectCode::f02_unreachable(),
                    triggered_by: String::new(),
                    message: "no peer route for this interaction".to_string(),
                    data: Vec::new(),
                    accumulated_cost: 0,
                })),
                ClaimAckOutcome::NotSent,
            );
        };

        let prepare = match Prepare::decode(&request.body) {
            Ok(prepare) => prepare,
            // §6.2: `4xx` is for a request there is no ILP answer to, which
            // an undecodable packet is. It is not a claim verdict and never
            // becomes one.
            Err(error) => {
                tracing::warn!(peer_id, %error, "peer POSTed an undecodable ILP packet");
                return PeerResponse::refused(400);
            }
        };

        // Issue #880 (owner decision #868) and ADR 0042: a peer PREPARE
        // carries a covering claim -- the route's `price` where this
        // connector terminates, the packet's own `amount` where it forwards
        // -- or it is refused with the client edge's own x402 greeting. The
        // decision is `connector_peer_btp::price_gate`'s, shared with the
        // BTP carriage so §0.1's one pipeline cannot admit over one carriage
        // what it refuses over the other; what is this carriage's is only
        // the response the refusal is shaped into.
        if let Some(refusal) = price_gate::payment_required(
            &self.connector,
            &peer_id,
            &prepare,
            ack,
            claim.as_ref(),
            prior_watermark,
            self.enforcement.mode(&peer_id),
        ) {
            return self.finish(&role, payment_required_response(refusal), ack);
        }

        // The one pipeline below the port (§0.1): a peer PREPARE that
        // arrived over HTTP is indistinguishable here from one that arrived
        // over BTP. `handle_peer_prepare` is handed no claim -- this
        // request's was judged above, before anything was routed -- and IS
        // handed the peering it arrived over, which is the incoming half of
        // ADR 0071's denomination boundary (issue #1295): the request is
        // signature-authenticated, so this carriage knows whose unit the
        // amount on it is denominated in.
        let (response, _) = self
            .connector
            .handle_peer_prepare(Some(&peer_id), prepare, None)
            .await;
        self.finish(&role, packet_response(response), ack)
    }

    /// §1.6's loud half. A claim naming a configured peer channel that
    /// fails P2 or P3 is an *assertion*; the request is a client request and
    /// is not refused for the assertion alone -- refusing would make the
    /// check an oracle for which peerings this connector has configured --
    /// but a silent downgrade would present to an operator as "peering
    /// configured, nothing peers, no error anywhere". The rate-limited event
    /// is what stops that.
    fn report_refusal(&self, refusal: Option<&PeerAuthRefusal>) {
        let Some(refusal) = refusal else {
            return;
        };
        let report = self
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
                "a peer channel's claim did not verify; the request is a client request"
            );
        }
    }

    /// A claim's verdict, or [`ClaimAckOutcome::NotSent`] when there was no
    /// readable claim to judge or the request was a client's.
    fn judge_claim(&self, role: &SessionRole, claim: Option<&WireClaim>) -> ClaimAckOutcome {
        // §1.5: a client's claim is not judged here at all -- the peer
        // namespace is not reachable from a client interaction (§1.8). A
        // request whose claim did not verify *is* a client request, so this
        // is also what keeps a bad signature from touching a watermark.
        let (Some(peer_id), Some(claim)) = (role.peer_id(), claim) else {
            return ClaimAckOutcome::NotSent;
        };

        // §6.3's idempotent re-ack, checked **before** the claim reaches the
        // book: a byte-identical retransmission at the current watermark is
        // `accepted`, and nothing is advanced or recorded. A payee that
        // answered it `nonce_not_advancing` would wedge the peering
        // permanently, since the payer's only honest retransmission would be
        // refused forever and minting a higher nonce for the same cumulative
        // is explicitly forbidden.
        if self.accepted.is_at_watermark(peer_id, claim) {
            return ClaimAckOutcome::Accepted;
        }

        let ack = self.connector.handle_peer_claim(claim.clone());
        if ack == ClaimAckOutcome::Accepted {
            self.accepted.record(peer_id, claim);
        }
        ack
    }

    /// The §3 fields that ride *every* answer: the claim ack (§6.1) and the
    /// flush prompt (§6.4), both gated on role.
    fn finish(
        &self,
        role: &SessionRole,
        mut response: PeerResponse,
        ack: ClaimAckOutcome,
    ) -> PeerResponse {
        // §1.7: a connector MUST NOT emit a `Toon-Claim-Ack` on a client
        // interaction, and §6.2 forbids one on a response answering a
        // request that carried no claim. Both are this one call.
        if let Some(value) = claim_ack_to_emit(role, headers::claim_ack_header_value(ack)) {
            response.headers.push(CLAIM_ACK_HEADER, value);
        }
        // §6.4: never on a response to a client interaction -- a client is
        // never treated as a peering relation for flush purposes (§1.7).
        if let Some(peer_id) = role
            .grants(Capability::CountTowardPeeringExposure)
            .then(|| role.peer_id())
            .flatten()
        {
            for channel_id in self.hints.take(peer_id) {
                response.headers.push(FLUSH_REQUESTED_HEADER, channel_id);
            }
        }
        response
    }
}

/// The answer to a PREPARE: **the body answers the packet, the header
/// answers the claim, and the status is `200` regardless of the claim's
/// verdict** (§6.2). A rejected claim never becomes a non-`200`, and never
/// changes the packet's own outcome, its `accumulatedCost` or its fee
/// accounting -- and a fulfilled packet can carry a `rejected` ack, which is
/// the property whose loss would silently destroy ADR 0024's semantics.
fn packet_response(response: PacketResponse) -> PeerResponse {
    match response {
        PacketResponse::Fulfill(fulfill) => {
            // §5.2: `Toon-Accumulated-Cost` rides **only** a REJECT. It is
            // never emitted beside a FULFILL.
            PeerResponse::ok(Fulfill::encode(&fulfill))
        }
        PacketResponse::Reject(reject) => {
            let mut response = PeerResponse::ok(reject.encode());
            // §5.2: always emitted on a REJECT, even at zero, so "absent"
            // never has to carry meaning in the direction that matters.
            response
                .headers
                .push(ACCUMULATED_COST_HEADER, reject.accumulated_cost.to_string());
            response
        }
    }
}

/// [`price_gate::payment_required`]'s refusal, HTTP-shaped: the greeting
/// rides a header rather than the client edge's own real `402`, because
/// this carriage keeps status `200` regardless of the packet's verdict,
/// unchanged by this issue (§6.2). The REJECT itself is shaped by
/// [`packet_response`], so §5.2's accumulated cost is not spelled twice.
fn payment_required_response(refusal: PaymentRequired) -> PeerResponse {
    let mut response = packet_response(PacketResponse::Reject(refusal.reject));
    response.headers.push(
        PAYMENT_REQUIRED_HEADER,
        headers::payment_required_header_value(&refusal.terms),
    );
    response
}

/// A monotonic-enough millisecond reading for the refusal log's rate limit.
/// Wall clock is fine: the log's own contract says a reading that goes
/// backwards closes the window early rather than suppressing forever.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The claim a peer request carries, decoded -- exposed so a caller that
/// wants to know what a request *would* be judged on, or what role it
/// proves, does not have to re-derive the header layer. Judging it is
/// [`PeerHttpState::handle`]'s.
///
/// `None` when the request carries no claim header at all, and also when it
/// carries one this connector could not read: an undecodable claim is *not
/// acknowledged* (§6.3) rather than rejected, so the payer's claim stays
/// pending and its retransmission is read the same way instead of being
/// recorded as a verdict that was never reached.
#[must_use]
pub fn claim_on(request: &PeerRequest) -> Option<WireClaim> {
    match headers::claim_json(&request.headers)? {
        Ok(raw) => claim_json::parse(&raw)
            .inspect_err(|error| {
                // No peer id to name: the claim *is* what would have named
                // one, and it did not decode.
                tracing::warn!(%error, "peer claim could not be decoded; not acknowledged");
            })
            .ok(),
        Err(_) => {
            tracing::warn!("peer claim header is not base64; not acknowledged");
            None
        }
    }
}
