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
//! * [`connector_peer_btp::role_gate::decide_frame`] joins §1.2's P2/P3 rule to
//!   the receiving half's verdict on a voucher or a peer-role challenge
//!   (ADR 0075 decision 5, #1380), and is the same call the BTP carriage
//!   makes, so §0.1's one pipeline cannot admit over one carriage what it
//!   refuses over the other. Its
//!   [`RoleDecision`](connector_peer_auth::RoleDecision) carries the role
//!   and the `peer_auth_refused` event together, so the silent downgrade
//!   cannot ship without the loud event;
//! * a peer's voucher is judged as payment by that same receiving half
//!   ([`connector_peer_btp::role_gate::VoucherEvidence`]), against the
//!   x402 channel's **one** amount watermark, because §2.5/I6 make it per
//!   **channel**, never per carriage -- a peering with two paths is still
//!   one channel, and a second ledger would be a double-spend surface.
//!
//! # This is not the client edge's `POST /ilp`, and must never become it
//!
//! The pipeline below the port is shared; **the admission is not**. ADR 0026
//! was written to dissolve exactly this blur, and the devnet incident §1.9
//! names -- `toon-sandbox` admitting an anonymous BTP session with
//! `success:true mode:"no-auth"` and then treating it as a quasi-peer -- is
//! what happens when the two audiences meet in one handler. Here a
//! client-role request reaches no peer handling at all: its claim is not
//! judged here, no watermark moves, nothing is journaled as a peer's, and
//! no `Toon-Claim-Ack` is emitted. Falling a client-role request
//! through to `connector-client-edge` instead of
//! answering it `F02` is the bring-up wiring of issue #678; what §1 requires
//! of *this* module is that role is decided by the request's verified
//! voucher or challenge and that a client can never reach peer handling, and that holds either
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
//! §6.4's prompt -- `Toon-Flush-Requested`, a payee asking a payer to flush
//! a pending `toon-channel` claim -- is gone with the peer claim it named:
//! since ADR 0075 (#1380) a peer pays with a voucher that rides the PREPARE
//! it covers, so there is nothing left pending to prompt for.

use std::sync::Arc;
use std::sync::Mutex;

use connector_btp::{
    ACCUMULATED_COST_HEADER, CLAIM_ACK_HEADER, CLAIM_HEADER, PAYMENT_REQUIRED_HEADER,
    PEER_CHALLENGE_HEADER,
};
use connector_domain::{Fulfill, PacketResponse, Prepare, Reject, RejectCode};
use connector_peer_auth::{claim_ack_to_emit, PeerAuthRefusal, PeerAuthRefusalLog, SessionRole};
use connector_peer_btp::price_gate::{self, ClaimEnforcementPolicy, PaymentRequired};
use connector_peer_btp::role_gate::{self, FrameEvidence, RefusedEvidence, VoucherEvidence};
use connector_runtime::{ClaimAckOutcome, Connector};

use crate::headers::{self, PeerRequest, PeerResponse};

/// How this connector accepts peer requests.
#[derive(Debug, Clone, Copy, Default)]
pub struct PeerHttpPolicy {
    /// §1.10's bounded escape hatch: a **dedicated peer listener with
    /// mandatory authentication**. Role is *still* decided by the request's
    /// evidence --
    /// the listener is defence in depth and MUST NOT become the decider, so
    /// §1.3 holds in full either way. What changes is only what happens to a
    /// request that fails: on a dedicated listener it is refused outright
    /// (`401`) rather than downgraded to client, and that is safe *only*
    /// because such a listener serves no clients -- there is no client to
    /// downgrade to and no oracle to leak.
    ///
    /// `false` (the default) is the shared-listener reading: a request whose
    /// evidence does not verify is an ordinary client request, per §1.6's
    /// "MUST NOT refuse it for the assertion alone".
    pub mandatory_auth: bool,
}

/// Everything an inbound peer request needs: the one pipeline below the
/// port, the price-gate enforcement policy, the receiving half vouchers are
/// judged by, and the rate-limited `peer_auth_refused` log.
///
/// There is no flush prompt any more. `Toon-Flush-Requested` asked a payer
/// to FLUSH a pending `toon-channel` claim; no peering sends one since ADR
/// 0075 (#1380), because a voucher rides the PREPARE it covers.
pub struct PeerHttpState {
    connector: Arc<Connector>,
    enforcement: Arc<ClaimEnforcementPolicy>,
    refusals: Mutex<PeerAuthRefusalLog>,
    policy: PeerHttpPolicy,
    /// The receiving half a voucher or a peer-role challenge is resolved
    /// through (ADR 0075 decision 5). `None` -- [`Self::new`]'s default --
    /// and neither proves the peer role on this carriage.
    vouchers: Option<Arc<dyn VoucherEvidence>>,
}

impl PeerHttpState {
    /// `enforcement` (issue #883, child B6) is deliberately shared with
    /// whatever other carriage serves the same peerings: one peering has one
    /// migration state, whichever carriage it rides.
    #[must_use]
    pub fn new(
        connector: Arc<Connector>,
        enforcement: Arc<ClaimEnforcementPolicy>,
        policy: PeerHttpPolicy,
    ) -> Self {
        PeerHttpState {
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

    /// Answer one peer request.
    ///
    /// **Role is decided before anything else happens** (§1.5): before a
    /// voucher is judged, before a watermark is consulted, before a packet
    /// is routed, and before any fee or journal accounting.
    pub async fn handle(&self, request: PeerRequest) -> PeerResponse {
        // §1.5's smuggling defence, counted before anything is parsed: more
        // than one claim header on one request is refused, not resolved --
        // never the first, never the last, never a concatenation. `400`,
        // with no ILP body.
        //
        // The same for the peer-role challenge (ADR 0075 decision 5), and a
        // claim beside a challenge is refused too: two pieces of
        // authentication material, and "which did we check?" has no answer.
        //
        // A `toon-channel` claim is refused `400` too, and by name: ADR 0075
        // retired the scheme (issue #1384), and a straggling peer is told so
        // in the body rather than silently read as presenting nothing.
        let evidence = match evidence_on(&request) {
            Ok(evidence) => evidence,
            Err(refused @ RefusedEvidence::ToonChannelClaim) => {
                return PeerResponse::refused_naming(400, &refused.message());
            }
            Err(_) => return PeerResponse::refused(400),
        };

        // **Role, from this request's own evidence** (§1.2, §1.5): decoded
        // and verified before anything is judged, routed, charged or
        // journaled. Decoded once, here, and reused for the price-coverage
        // check further down.
        let (role, refusal) =
            role_gate::decide_frame(&self.connector, self.vouchers.as_deref(), &evidence)
                .await
                .into_parts();
        self.report_refusal(refusal.as_ref());
        // A voucher is judged by the receiving half, below the role (ADR
        // 0075 decision 6). A `toon-channel` claim is judged by nothing here:
        // it proved nothing, and a peering never pays with one (#1380).
        let voucher = evidence.voucher().cloned();

        // §1.10: on a dedicated peer listener a failure is refused outright
        // rather than downgraded, because such a listener serves no clients.
        if self.policy.mandatory_auth && !role.is_peer() {
            return PeerResponse::refused(401);
        }

        // The voucher is judged **before** anything is routed, and its
        // prior watermark is what the price-coverage check below measures its
        // own advance from -- read from the durable book that judges it,
        // never from a per-process record (issue #1104's rule).
        let (ack, claimed, prior_watermark) =
            match role_gate::judge_voucher(&role, self.vouchers.as_deref(), voucher.as_ref()).await
            {
                Some(judged) => (judged.ack, judged.claimed, judged.prior),
                None => (ClaimAckOutcome::NotSent, None, None),
            };

        // A POST with an **empty ILP body**: once the `toon-channel` FLUSH
        // (§3). A voucher standing alone is still answered, its ack riding
        // the response -- HTTP always answers, which is what bounds the ack
        // structurally (§6.3).
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
            claimed,
            prior_watermark,
            self.enforcement.mode(&peer_id),
        ) {
            return self.finish(&role, payment_required_response(refusal), ack);
        }

        // The one pipeline below the port (§0.1): a peer PREPARE that
        // arrived over HTTP is indistinguishable here from one that arrived
        // over BTP. `handle_peer_prepare` is handed no voucher -- this
        // request's was judged above, before anything was routed -- and IS
        // handed the peering it arrived over, which is the incoming half of
        // ADR 0071's denomination boundary (issue #1295): the request is
        // signature-authenticated, so this carriage knows whose unit the
        // amount on it is denominated in.
        let response = self
            .connector
            .handle_peer_prepare(Some(&peer_id), prepare)
            .await;
        self.finish(&role, packet_response(response), ack)
    }

    /// §1.6's loud half. A voucher or challenge from a bound signer's channel
    /// that does not verify is an *assertion*; the request is a client
    /// request and is not refused for the assertion alone -- refusing would make the
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
                "a peer channel's voucher did not verify; the request is a client request"
            );
        }
    }

    /// The §3 field that rides *every* answer: the claim ack (§6.1), gated
    /// on role.
    fn finish(
        &self,
        role: &SessionRole,
        mut response: PeerResponse,
        ack: ClaimAckOutcome,
    ) -> PeerResponse {
        // §1.7: a connector MUST NOT emit a `Toon-Claim-Ack` on a client
        // interaction, and §6.2 forbids one on a response answering a
        // request that carried no voucher. Both are this one call.
        if let Some(value) = claim_ack_to_emit(role, headers::claim_ack_header_value(ack)) {
            response.headers.push(CLAIM_ACK_HEADER, value);
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

/// Everything a peer request presents that could prove the peer role (ADR
/// 0075 decision 5): its claim header -- a voucher -- its peer-role challenge
/// header, and whether its body is a PREPARE that moves no value.
///
/// # Errors
///
/// [`RefusedEvidence`], which the caller refuses `400`: §1.5's ambiguity --
/// more than one claim header, more than one challenge header, or a claim
/// beside a challenge -- or a `toon-channel` claim (ADR 0075 decision 8). An
/// otherwise unreadable claim or challenge is neither; it proves nothing,
/// and the request is judged as if it were absent.
pub fn evidence_on(request: &PeerRequest) -> Result<FrameEvidence, RefusedEvidence> {
    RefusedEvidence::check(
        request.headers.get_all(CLAIM_HEADER).len(),
        request.headers.get_all(PEER_CHALLENGE_HEADER).len(),
    )?;
    let claim = match headers::claim_json(&request.headers) {
        None => None,
        Some(Ok(raw)) => role_gate::decode_claim(&raw)?,
        Some(Err(_)) => {
            tracing::warn!("peer claim header is not base64; not acknowledged");
            None
        }
    };
    let challenge = match headers::peer_challenge_json(&request.headers) {
        None => None,
        Some(Ok(raw)) => role_gate::decode_challenge(&raw),
        Some(Err(_)) => {
            tracing::warn!("peer-role challenge header is not base64; it proves nothing");
            None
        }
    };
    Ok(FrameEvidence {
        claim,
        challenge,
        moves_no_value: role_gate::moves_no_value(&request.body),
    })
}
