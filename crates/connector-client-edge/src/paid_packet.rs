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
//! at [`admit`] and [`carry_admitted`] because the BTP carriage must admit a
//! session's claims inline, in arrival order, and finish them in a windowed
//! task -- the split it moves onto in #1720. The HTTP carriage calls
//! [`carry`], which admits on the request's future and carries in a task of
//! its own.

use std::sync::Arc;

use connector_domain::client_claim::ClientEdgeClaim;
use connector_domain::{PacketResponse, Prepare};
use connector_peer_auth::SessionRole;
use connector_runtime::Connector;

use crate::claim_gate::{ClaimIngestRejection, DurabilityTicket, InFlightForward};
use crate::{ClientClaimGate, ClientEdgeState};

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

/// A packet whose claim the gate has accepted but whose acceptance is not yet
/// durable: no service may be rendered for it until [`Self::durable`]
/// resolves. Dropped before then, its claim is rolled back.
#[must_use = "dropping it rolls the claim back"]
pub(crate) struct AdmittedPacket {
    durability: DurabilityTicket,
    connector: Arc<Connector>,
    packet: PaidPacket,
}

impl AdmittedPacket {
    /// Wait for the acceptance's batch fsync, then record the accepted claim
    /// -- the sender's channel is recognised (issue #548) and its time noted
    /// for the claim-state endpoint (issue #693). A batch that could not be
    /// made durable is [`ClaimIngestRejection::NotDurable`]; the committer
    /// has already unwound it, so there is nothing to roll back and the same
    /// claim resubmitted is still good.
    pub(crate) async fn durable(self) -> Result<PaidPacket, ClaimIngestRejection> {
        let AdmittedPacket {
            durability,
            connector,
            mut packet,
        } = self;
        if let Err(rejection) = durability.durable().await {
            packet.settled = true;
            return Err(rejection);
        }
        let channel_key = packet.channel_key();
        connector.recognize_channel(&channel_key);
        packet.gate.note_claim_time(&channel_key, crate::now_unix());
        Ok(packet)
    }
}

/// The handle for one paid packet whose claim is durably accepted. The
/// carriage routes the packet and gives this its [`Outcome`]; it does the
/// rest. Dropped without one, it counts as [`Outcome::NotCarried`].
#[must_use = "dropping it counts the packet as not carried"]
pub(crate) struct PaidPacket {
    gate: Arc<ClientClaimGate>,
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

impl PaidPacket {
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
                .roll_back(&channel_key, self.claim.transferred_amount())
                .await
            {
                log_failed_rollback(&channel_key, &error);
            }
        }
    }
}

impl Drop for PaidPacket {
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
    let (claim, durability, in_flight) = state.claim_gate.admit(&claim_json, charge, role).await?;
    Ok(Some(AdmittedPacket {
        durability,
        connector: Arc::clone(&state.connector),
        packet: PaidPacket {
            gate: Arc::clone(&state.claim_gate),
            claim,
            settled: false,
            _in_flight: in_flight,
        },
    }))
}

/// The pipeline's order-insensitive half: wait for `admitted` to be durable,
/// route `packet`, and finish the handle with the outcome. A claim whose
/// acceptance could not be made durable is refused, and its packet is never
/// routed.
pub(crate) async fn carry_admitted(
    state: &ClientEdgeState,
    packet: Packet,
    admitted: Option<AdmittedPacket>,
) -> Result<Carried, ClaimIngestRejection> {
    let paid = match admitted {
        Some(admitted) => Some(admitted.durable().await?),
        None => None,
    };
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
    let carrying = tokio::spawn(async move { carry_admitted(&state, packet, admitted).await });
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
