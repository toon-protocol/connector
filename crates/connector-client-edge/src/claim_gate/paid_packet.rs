//! A paid packet's lifecycle at the client edge, in one place (#1719, spec
//! #1687): admission hands the carriage **one handle** for the packet a claim
//! pays for, and one shared pipeline drives it.
//!
//! The claim gate decides whether a claim is good. Everything after that
//! decision used to be a sequence each carriage wrote out by hand: wait for
//! durability, record the accepted claim and note its time, route, roll back
//! an uncarried forward, release the in-flight hold. The handle owns those
//! steps now, so a carriage cannot make them in the wrong order:
//!
//! - [`admit`] runs the refuse-before-admitting check
//!   ([`crate::claim_must_not_be_admitted`]) and only then the gate, and
//!   returns an [`AdmittedPacket`]: accepted, not yet durable.
//! - [`AdmittedPacket::durable`] waits for the journal, at the same point as
//!   before (ADR 0005, journal before service), then records the accepted
//!   claim and notes its time. Only then is there a [`PaidPacket`] to route.
//! - [`PaidPacket::finish`] takes the packet's [`Outcome`]. *Not carried*
//!   rolls the claim's watermark back (ADR 0028, #1012); every outcome then
//!   releases the zk edge's in-flight hold (ADR 0080 decision 4).
//! - **A handle dropped without an outcome counts as not carried.** Its
//!   rollback is made synchronously in `Drop`, so a pipeline that panics or
//!   is cancelled from inside never leaves value counted or a hold held, and
//!   the same voucher is good again.
//! - **A client going away is not such a cancellation.** Once admitted, a
//!   packet is carried in its own task, so it always reaches an outcome and
//!   is charged by that outcome: a client cannot be refunded for work the
//!   app or the next hop did by disconnecting before the answer arrived.
//!
//! [`carry`] is the pipeline both carriages share: packet, claim, price and
//! role in; response out. Each carriage keeps only its framing. It is split
//! at [`admit`] and [`carry_admitted`] because the BTP carriage admits a
//! session's claims inline, in arrival order, and finishes them in a
//! windowed task of its own (#1720). The HTTP carriage calls [`carry`],
//! which admits on the request's future and carries in a task of its own.
//! Either way the post-admission half runs detached from the client, so a
//! dropped request or a closed socket charges by the outcome.
//!
//! A claim that pays for no packet -- one standing alone on a BTP session, a
//! probe's, a peer's voucher judged below the role -- is admitted to the
//! same handle and finished with [`AdmittedPacket::accepted`]: its
//! acceptance is its outcome.
//!
//! This module is a child of the claim gate on purpose: the steps it drives
//! (the gate's admission tuple, the durability wait, noting a claim's time,
//! rollback) are private to the gate, so no carriage can call them in
//! another order. The gate's own interface is admission
//! ([`ClientClaimGate::ingest`] and the handles here) and read-only queries.

use std::borrow::Borrow;
use std::sync::Arc;

use connector_domain::client_claim::{ClientClaim, ClientEdgeClaim};
use connector_domain::{PacketResponse, Prepare};
use connector_peer_auth::SessionRole;
use connector_runtime::Connector;

use super::{ClaimIngestRejection, ClientClaimGate, DurabilityTicket, InFlightForward};
use crate::ClientEdgeState;

/// How a paid packet ended, as the carriage that carried it reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The packet was fulfilled: the claim stays counted.
    Fulfilled,
    /// The packet was carried and rejected where it ended -- an app's own
    /// answer, which consumed real work (ADR 0020): the claim stays counted.
    Rejected,
    /// The packet was never carried: the claim is rolled back. What a
    /// forwarded route's terminal reject means (ADR 0028, #1012), and what a
    /// handle dropped without an outcome counts as.
    NotCarried,
}

impl Outcome {
    /// The outcome of `response` on a route that is `forwarded` or not. A
    /// forwarded route charges on the forward's FULFILL and never on its
    /// REJECT (ADR 0028), so any reject there is *not carried*; a reject at a
    /// route this connector terminates is the app's answer.
    pub(crate) fn of(response: &PacketResponse, forwarded: bool) -> Outcome {
        match response {
            PacketResponse::Fulfill(_) => Outcome::Fulfilled,
            PacketResponse::Reject(_) if forwarded => Outcome::NotCarried,
            PacketResponse::Reject(_) => Outcome::Rejected,
        }
    }
}

/// What a durable acceptance is recorded as, beyond the journal: the step
/// the handle takes once the claim is durable and before anything is
/// served for it.
enum Record {
    /// A claim on a client's interaction that pays for a packet or stands
    /// alone on a client's session: its channel is recognised, which makes
    /// the sender eligible to probe (issue #548), and its time noted for the
    /// claim-state endpoint (issue #693).
    Recognised(Arc<Connector>),
    /// A probe's claim, which identifies rather than pays: its time is noted
    /// (issue #693), but it never makes a channel one this connector
    /// recognises (ADR 0011's first condition).
    ClaimTime,
    /// A peer's voucher, judged below the role (#1378), or a bare
    /// [`ClientClaimGate::ingest`]: journaled, and nothing else.
    JournalOnly,
}

impl Record {
    fn record(&self, gate: &ClientClaimGate, channel_key: &str) {
        match self {
            Record::Recognised(connector) => {
                connector.recognize_channel(channel_key);
                gate.note_claim_time(channel_key, crate::now_unix());
            }
            Record::ClaimTime => gate.note_claim_time(channel_key, crate::now_unix()),
            Record::JournalOnly => {}
        }
    }
}

/// A packet whose claim the gate has accepted but whose acceptance is not yet
/// durable: no service may be rendered for it until [`Self::durable`]
/// resolves. Dropped before then, its claim is rolled back.
///
/// `G` is how the handle holds the gate: shared (`Arc`) for a packet carried
/// in a task of its own, borrowed for an admission finished where it was
/// made (a peer's voucher, [`ClientClaimGate::ingest`]).
#[must_use = "dropping it rolls the claim back"]
pub(crate) struct AdmittedPacket<G: Borrow<ClientClaimGate> = Arc<ClientClaimGate>> {
    durability: DurabilityTicket,
    record: Record,
    packet: PaidPacket<G>,
}

impl<G: Borrow<ClientClaimGate>> AdmittedPacket<G> {
    fn new(
        gate: G,
        (claim, durability, in_flight): (ClientEdgeClaim, DurabilityTicket, InFlightForward),
        record: Record,
    ) -> AdmittedPacket<G> {
        AdmittedPacket {
            durability,
            record,
            packet: PaidPacket {
                gate,
                claim,
                settled: false,
                _in_flight: in_flight,
            },
        }
    }

    /// Wait for the acceptance's batch fsync, then record the accepted claim
    /// -- for a client's packet, the sender's channel is recognised (issue
    /// #548) and its time noted for the claim-state endpoint (issue #693). A
    /// batch that could not be made durable is
    /// [`ClaimIngestRejection::NotDurable`]; the committer has already
    /// unwound it, so there is nothing to roll back and the same claim
    /// resubmitted is still good.
    pub(crate) async fn durable(self) -> Result<PaidPacket<G>, ClaimIngestRejection> {
        let AdmittedPacket {
            durability,
            record,
            mut packet,
        } = self;
        if let Err(rejection) = durability.durable().await {
            packet.settled = true;
            return Err(rejection);
        }
        record.record(packet.gate.borrow(), &packet.channel_key());
        Ok(packet)
    }

    /// Finish a claim that pays for no packet -- a standalone claim on a BTP
    /// session, a probe's, a peer's voucher -- whose acceptance is itself its
    /// outcome: the claim stays counted, and the answer is the claim once
    /// durable and recorded.
    ///
    /// The outcome is given **before** the returned future first runs, so a
    /// caller that goes away while it waits for durability undoes nothing:
    /// the acceptance is already enqueued, and the committer journals it
    /// either way, as it always has for these claims.
    pub(crate) fn accepted(
        self,
    ) -> impl std::future::Future<Output = Result<ClientEdgeClaim, ClaimIngestRejection>> {
        let AdmittedPacket {
            durability,
            record,
            mut packet,
        } = self;
        packet.settled = true;
        async move {
            durability.durable().await?;
            record.record(packet.gate.borrow(), &packet.channel_key());
            Ok(packet.claim.clone())
        }
    }
}

/// The handle for one paid packet whose claim is durably accepted. The
/// carriage routes the packet and gives this its [`Outcome`]; it does the
/// rest. Dropped without one, it counts as [`Outcome::NotCarried`].
#[must_use = "dropping it counts the packet as not carried"]
pub(crate) struct PaidPacket<G: Borrow<ClientClaimGate> = Arc<ClientClaimGate>> {
    gate: G,
    claim: ClientEdgeClaim,
    /// Whether an outcome has been given -- or there is otherwise nothing
    /// left for `Drop` to undo.
    settled: bool,
    /// Held until the packet has resolved, past any rollback: an allowance
    /// is charged only once none of its forwards is in flight (ADR 0080
    /// decision 4). Inert for a voucher. A field, so it is released after
    /// `Drop::drop`'s rollback has run.
    _in_flight: InFlightForward,
}

impl<G: Borrow<ClientClaimGate>> PaidPacket<G> {
    /// The claim that paid for this packet.
    pub(crate) fn claim(&self) -> &ClientEdgeClaim {
        &self.claim
    }

    /// The canonical key of the channel (or allowance) the claim paid on.
    pub(crate) fn channel_key(&self) -> String {
        self.claim.claim_key()
    }

    /// Finish the packet with `outcome`. Not carried rolls the claim back,
    /// durably, before this resolves (#1012); then the in-flight hold is
    /// released.
    pub(crate) async fn finish(mut self, outcome: Outcome) {
        self.settled = true;
        if outcome == Outcome::NotCarried {
            let channel_key = self.channel_key();
            if let Err(error) = self
                .gate
                .borrow()
                .roll_back(&channel_key, self.claim.transferred_amount())
                .await
            {
                log_failed_rollback(&channel_key, &error);
            }
        }
    }
}

impl<G: Borrow<ClientClaimGate>> Drop for PaidPacket<G> {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        // Not carried. The restore and the journal enqueue happen here, under
        // the gate's lock; the fsync is the committer's, which unwinds and
        // logs a batch that fails.
        let channel_key = self.channel_key();
        if let Err(error) = self
            .gate
            .borrow()
            .begin_roll_back(&channel_key, self.claim.transferred_amount())
        {
            log_failed_rollback(&channel_key, &error);
        }
    }
}

fn log_failed_rollback(channel_key: &str, error: &ClaimIngestRejection) {
    tracing::error!(
        channel = channel_key,
        error = %error.message(),
        "a packet this claim paid for was not carried, but this claim's watermark could not \
         be durably rolled back -- the client remains charged for a packet this connector \
         never carried"
    );
}

/// What a paid packet's pipeline needs to know about the packet itself.
pub(crate) struct Packet {
    pub(crate) prepare: Prepare,
    /// What this connector charges for this packet (ADR 0065).
    pub(crate) charge: u64,
    /// Whether the one route lookup (issue #701) matched a forwarded route.
    pub(crate) forwarded: bool,
}

/// What the pipeline answers a carriage with.
pub(crate) struct Carried {
    pub(crate) response: PacketResponse,
    /// The claim's self-declared signer, when a voucher paid for the packet
    /// -- whether the carriage may use it as an identity is its own framing's
    /// rule (a wrapped claim's is never one).
    pub(crate) voucher_signer: Option<String>,
}

/// Admit `claim` for `prepare` at `charge`, on an interaction of `role`:
/// the pipeline's order-sensitive half.
///
/// `claim` decodes the carriage's own framing into the claim's JSON, and is
/// called only once the packet is known to be one this connector may admit a
/// claim for ([`crate::claim_must_not_be_admitted`], issue #869 and #1446),
/// so a claim on a packet going nowhere is never decoded or spent. `Ok(None)`:
/// no claim was presented, or it was left unadmitted.
pub(crate) async fn admit<F>(
    state: &ClientEdgeState,
    prepare: &Prepare,
    claim: Option<F>,
    charge: u64,
    role: &SessionRole,
) -> Result<Option<AdmittedPacket>, ClaimIngestRejection>
where
    F: FnOnce() -> Result<String, ClaimIngestRejection>,
{
    let Some(claim) = claim else {
        return Ok(None);
    };
    if crate::claim_must_not_be_admitted(state, prepare) {
        return Ok(None);
    }
    let claim_json = claim()?;
    let admitted = state.claim_gate.admit(&claim_json, charge, role).await?;
    Ok(Some(AdmittedPacket::new(
        Arc::clone(&state.claim_gate),
        admitted,
        Record::Recognised(Arc::clone(&state.connector)),
    )))
}

/// Admit `claim_json` standing alone on a client's BTP session (§1.9 step
/// 4): against price 0 -- identify-not-pay -- and recorded as a packet's
/// claim is, since the session it rides is a client's. It pays for no
/// packet: finish it with [`AdmittedPacket::accepted`].
pub(crate) async fn admit_session_claim(
    state: &ClientEdgeState,
    claim_json: &str,
) -> Result<AdmittedPacket, ClaimIngestRejection> {
    let admitted = state
        .claim_gate
        .admit(claim_json, 0, &SessionRole::Client)
        .await?;
    Ok(AdmittedPacket::new(
        Arc::clone(&state.claim_gate),
        admitted,
        Record::Recognised(Arc::clone(&state.connector)),
    ))
}

/// Admit a probe's `claim_json` (`POST /ilp/probe`, ADR 0011): against price
/// 0, since it identifies rather than pays, and with its time noted but its
/// channel not recognised. It pays for no packet: finish it with
/// [`AdmittedPacket::accepted`].
pub(crate) async fn admit_probe_claim<'g>(
    gate: &'g ClientClaimGate,
    claim_json: &str,
) -> Result<AdmittedPacket<&'g ClientClaimGate>, ClaimIngestRejection> {
    let admitted = gate.admit(claim_json, 0, &SessionRole::Client).await?;
    Ok(AdmittedPacket::new(gate, admitted, Record::ClaimTime))
}

/// Admit a peer's voucher, judged below the role (#1378): with no price of
/// its own, since coverage is the peer carriage's to measure, against the
/// channel's one watermark and journal (`peer-carriage-spec.md` §1.8). It
/// pays for no packet the gate sees: finish it with
/// [`AdmittedPacket::accepted`].
pub(crate) async fn admit_peer_voucher(
    gate: &ClientClaimGate,
    voucher: ClientClaim,
) -> Result<AdmittedPacket<&ClientClaimGate>, ClaimIngestRejection> {
    let admitted = gate
        .admit_claim(ClientEdgeClaim::Voucher(Box::new(voucher)), 0)
        .await?;
    Ok(AdmittedPacket::new(gate, admitted, Record::JournalOnly))
}

/// Admit `claim_json` against `price` and finish it as accepted: what
/// [`ClientClaimGate::ingest`] is.
pub(super) async fn ingest(
    gate: &ClientClaimGate,
    claim_json: &str,
    price: u64,
) -> Result<ClientEdgeClaim, ClaimIngestRejection> {
    let admitted = gate.admit(claim_json, price, &SessionRole::Client).await?;
    AdmittedPacket::new(gate, admitted, Record::JournalOnly)
        .accepted()
        .await
}

/// The pipeline's order-insensitive half: wait for `admitted` to be durable,
/// route `packet`, and finish the handle with the outcome. A claim whose
/// acceptance could not be made durable is refused, and its packet is never
/// routed.
///
/// `on_accepted` is the carriage's own step for a durably accepted claim,
/// run before the packet is routed: the BTP carriage learns who its session
/// is paid as from it (issue #1396); HTTP has none.
pub(crate) async fn carry_admitted(
    state: &ClientEdgeState,
    packet: Packet,
    admitted: Option<AdmittedPacket>,
    on_accepted: impl FnOnce(&ClientEdgeClaim),
) -> Result<Carried, ClaimIngestRejection> {
    let paid = match admitted {
        Some(admitted) => Some(admitted.durable().await?),
        None => None,
    };
    if let Some(paid) = &paid {
        on_accepted(paid.claim());
    }
    let channel_key = paid.as_ref().map(PaidPacket::channel_key);
    let voucher_signer = paid
        .as_ref()
        .and_then(|paid| paid.claim().voucher())
        .map(|voucher| voucher.signer().to_string());
    // Issue #736: the configured routes first, then whatever client session
    // is bound to this destination.
    let response =
        crate::session_route::route_prepare(state, packet.prepare, channel_key.as_deref()).await;
    if let Some(paid) = paid {
        paid.finish(Outcome::of(&response, packet.forwarded)).await;
    }
    Ok(Carried {
        response,
        voucher_signer,
    })
}

/// The shared pipeline: admit `claim` for `packet` on an interaction of
/// `role`, carry it, and answer with the response. An `Err` is a claim
/// refusal, which the carriage frames as its own refusal response.
///
/// **Once admitted, the packet is carried in its own task**, so the
/// caller going away cannot cancel it: a client that disconnects from
/// `POST /ilp` mid-flight still has its packet reach an outcome, and is
/// charged by that outcome -- a forward the next hop fulfils stays counted,
/// as it would have had the client waited. Admission itself stays on the
/// caller's future: its last suspension point comes before anything is
/// advanced, so a caller dropped during admission leaves nothing counted,
/// and nothing is held. A handle dropped without an outcome inside the task
/// -- the task panicked, or the runtime shut it down -- still rolls back.
pub(crate) async fn carry<F>(
    state: &Arc<ClientEdgeState>,
    packet: Packet,
    claim: Option<F>,
    role: &SessionRole,
) -> Result<Carried, ClaimIngestRejection>
where
    F: FnOnce() -> Result<String, ClaimIngestRejection>,
{
    let admitted = admit(state, &packet.prepare, claim, packet.charge, role).await?;
    let state = Arc::clone(state);
    let carrying =
        tokio::spawn(async move { carry_admitted(&state, packet, admitted, |_| {}).await });
    match carrying.await {
        Ok(carried) => carried,
        // A panic in the pipeline is this request's panic, as it was before
        // the pipeline had a task of its own; the handle it dropped has
        // already rolled back.
        Err(error) => match error.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            // Cancelled only by the runtime shutting down: the handle rolled
            // back, so the packet was not carried and nothing is charged.
            Err(_) => Err(ClaimIngestRejection::NotDurable),
        },
    }
}

#[cfg(test)]
mod tests;
