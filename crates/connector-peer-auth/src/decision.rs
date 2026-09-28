//! The decision itself (§1.2), and the operator event a failed assertion
//! owes (§1.6).

use std::collections::BTreeMap;

use crate::policy::PeerAuthPolicy;
use crate::role::SessionRole;

/// The name of the operator-visible event §1.6 requires, declared once so
/// a log line, a metric label and a test cannot each spell it differently.
pub const PEER_AUTH_REFUSED_EVENT: &str = "peer_auth_refused";

/// What this connector's **own record** of a channel says about a claim's
/// signature.
///
/// Computed by the carriage, out of `ClaimBook`'s `verify_signature`, and
/// handed here as a verdict rather than as key material: verifying a
/// secp256k1 or ed25519 signature needs the counterparty key the
/// `[[peer_channels]]` row configures, which lives in the runtime's claim
/// book, and this crate deliberately depends on no runtime at all (§1.3,
/// and [`crate::tests::the_decision_crate_cannot_name_a_transport`]). The
/// three variants are exactly `verify_signature`'s three outcomes, so
/// nothing is collapsed on the way here — §1.6 needs the two failures apart
/// to tell an operator which one they have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClaimVerification {
    /// The signature recovered to the counterparty key the channel's
    /// `[[peer_channels]]` row configures — never to anything the claim
    /// declares about itself.
    Verified,
    /// This connector holds no record of the channel to verify against.
    UnknownChannel,
    /// A record exists and the signature did not recover to its key.
    SignatureInvalid,
}

/// The claim an interaction presented, as the role decision sees it: which
/// channel it names, and what this connector's own record made of its
/// signature.
///
/// It carries those two facts and nothing else. That is not minimalism: it
/// is §1.3 made structural. A value of this type cannot carry the carriage
/// it arrived on, the port it hit, its source address or its TLS name, so
/// the decision that consumes it cannot weight one. It carries no nonce and
/// no amount either — what a claim is *worth* is `ClaimBook`'s question,
/// downstream of the role, and answering it here would put claim state in
/// the decision path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentedClaim<'a> {
    channel_id: &'a str,
    verification: ClaimVerification,
}

impl<'a> PresentedClaim<'a> {
    /// The claim on this frame: the channel it names, and this connector's
    /// verdict on its signature.
    #[must_use]
    pub fn new(channel_id: &'a str, verification: ClaimVerification) -> Self {
        PresentedClaim {
            channel_id,
            verification,
        }
    }

    /// The channel this claim *names*. Naming is all it does: nothing
    /// downstream may treat it as identifying a peering until
    /// [`decide_role`] has resolved it against `[[peer_channels]]`.
    #[must_use]
    pub fn channel_id(&self) -> &str {
        self.channel_id
    }

    /// What this connector's own record made of the signature.
    #[must_use]
    pub fn verification(&self) -> ClaimVerification {
        self.verification
    }
}

/// Which of §1.2's two requirements an assertion failed to meet.
///
/// Carried on the operator event because the two have completely different
/// fixes — a channel this node holds no record of and a signature that
/// recovers to the wrong key look identical from the outside, and "peering
/// configured, nothing peers, no error anywhere" is the symptom §1.6 exists
/// to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnmetRequirement {
    /// **P2** — the claim names a channel a `[[peer_channels]]` row
    /// configures, but this connector holds no record to verify it
    /// against: `verify_signature` answered
    /// [`ClaimVerification::UnknownChannel`]. Config and the claim book
    /// disagree, which is a wiring fault rather than a caller's, and it is
    /// exactly the shape that presents as a peering that never peers.
    ChannelBinding,
    /// **P3** — a record exists and the signature did not recover to the
    /// counterparty key that row configures
    /// ([`ClaimVerification::SignatureInvalid`]). A rotated key on one side
    /// only, or a claim signed by somebody else entirely.
    ///
    /// Also a voucher, or a peer-role challenge, on a channel whose voucher
    /// signer is bound to a peering, whose signature does not recover to
    /// that signer ([`VoucherVerification::SignatureInvalid`], ADR 0075
    /// decision 5): the same fix, a key that is not the one the chain
    /// records for the channel.
    ClaimSignature,
    /// A peer-role challenge (ADR 0075 decision 5) signed by a bound
    /// channel's voucher signer, but outside its window: `expires` has
    /// passed, or lies further ahead than this node accepts
    /// ([`VoucherVerification::Expired`]). The key is right and the clock or
    /// the challenge's lifetime is not -- a different fix from
    /// [`UnmetRequirement::ClaimSignature`]'s, so a different event.
    ChallengeExpiry,
}

impl UnmetRequirement {
    /// The requirement's name in §1.2, for the operator event.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            UnmetRequirement::ChannelBinding => "P2",
            UnmetRequirement::ClaimSignature => "P3",
            UnmetRequirement::ChallengeExpiry => "P3-expires",
        }
    }
}

/// An interaction named a configured peer channel and did not prove it
/// (§1.6).
///
/// This is not a refusal *on the wire*. §1.6 forbids that: refusing would
/// make the check an oracle for which peerings this connector has
/// configured. The interaction is admitted, as a client, and this value is
/// what an operator sees instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAuthRefusal {
    peer_id: String,
    unmet: UnmetRequirement,
}

impl PeerAuthRefusal {
    /// The **configured** peer id whose channel was named.
    ///
    /// It comes from the `[[peer_channels]]` row, never from the
    /// interaction — the claim names a channel and config names the
    /// peering, so an attacker-chosen string never reaches this log line.
    /// A claim naming a channel this connector binds to no peering
    /// produces no refusal to carry one (see [`decide_role`]).
    #[must_use]
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// Which of P2/P3 failed.
    #[must_use]
    pub fn unmet(&self) -> UnmetRequirement {
        self.unmet
    }
}

/// The verdict: a role, and the operator event it owes.
///
/// The two travel together because §1.6 requires both halves of the same
/// outcome — the downgrade *and* the event. Returning only a role would
/// let a carriage implement the silent half and forget the loud one, which
/// is the failure §1.6 describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleDecision {
    role: SessionRole,
    refusal: Option<PeerAuthRefusal>,
}

impl RoleDecision {
    /// The decided role. Never `Peer` when [`RoleDecision::refusal`] is
    /// `Some` — a refusal *is* the downgrade.
    #[must_use]
    pub fn role(&self) -> &SessionRole {
        &self.role
    }

    /// The `peer_auth_refused` event this decision owes an operator, if
    /// any. Feed it to a [`PeerAuthRefusalLog`] rather than emitting it
    /// directly: §1.6 requires the event be rate-limited.
    #[must_use]
    pub fn refusal(&self) -> Option<&PeerAuthRefusal> {
        self.refusal.as_ref()
    }

    /// Split the verdict, for a carriage that binds the role and reports
    /// the event on different paths.
    #[must_use]
    pub fn into_parts(self) -> (SessionRole, Option<PeerAuthRefusal>) {
        (self.role, self.refusal)
    }

    fn client() -> Self {
        RoleDecision {
            role: SessionRole::Client,
            refusal: None,
        }
    }

    fn refused(peer_id: &str, unmet: UnmetRequirement) -> Self {
        RoleDecision {
            role: SessionRole::Client,
            refusal: Some(PeerAuthRefusal {
                peer_id: peer_id.to_string(),
                unmet,
            }),
        }
    }

    fn peer(peer_id: &str) -> Self {
        RoleDecision {
            role: SessionRole::peer(peer_id),
            refusal: None,
        }
    }
}

/// **The decision** (§1.2): `peer` if and only if P2 and P3 both hold,
/// `client` otherwise.
///
/// ```text
/// (the frame's verified claim, the configured channel bindings) -> role
/// ```
///
/// Those are the only two inputs, and that is the security property. There
/// is no third parameter for the carriage, the listener, the port, the
/// bind address, the source address, the TLS SNI name, a client
/// certificate, the `btp` subprotocol, an endpoint from `[[peers]]`, the
/// shape of what was sent, or anything this or another interaction did
/// earlier — §1.3's list, absent by construction rather than by
/// convention. A caller who wanted to weight one would have to change this
/// signature, and changing it is a reviewable act in a way that adding a
/// branch is not.
///
/// There is no bearer credential in that list either, and there is not
/// meant to be: ADR 0060 deleted the `{peerId, secret}` shared secret
/// outright, and did not replace it — not renamed, not demoted to a label,
/// not kept as an optional discriminator. A signature over ADR 0024's
/// balance proof proves control of the key the channel was actually opened
/// against, which is strictly stronger than possession of a string both
/// operators wrote into their own config files, and it is present on every
/// packet rather than once per session.
///
/// It is a free function rather than a method for the same reason: a
/// method on a session, a listener or a connection would have a `self`
/// with fields, and every one of those fields is something §1.3 forbids
/// consulting.
///
/// # Branches
///
/// | The frame's claim | Outcome |
/// | ----------------- | ------- |
/// | none | `client`, no event — the ordinary client interaction |
/// | on a channel no `[[peer_channels]]` row binds | `client`, no event (see below) |
/// | on a bound channel, no record to verify against | `client` + `peer_auth_refused` (P2) |
/// | on a bound channel, signature does not recover | `client` + `peer_auth_refused` (P3) |
/// | on a bound channel, signature verifies | `peer`, as that row's relation |
///
/// A claim on an **unbound** channel produces no event, and that is §1.6
/// read literally: an assertion is one that names a configured peering and
/// fails. The reason it matters here is concrete rather than pedantic —
/// every ordinary client covers every packet with a claim of its own, on a
/// `[[client_channels]]` channel this policy does not hold. Emitting on an
/// unbound channel would fire `peer_auth_refused` on essentially every
/// client packet, which is both noise and a log-volume lever any anonymous
/// caller could pull. The cost is real and worth stating: a peering that
/// pays from a channel its counterparty never configured presents as an
/// ordinary client with nothing logged. A configured channel whose claims
/// do not verify — the far likelier mistake, and the one §1.6 names — is
/// loud.
#[must_use]
pub fn decide_role(claim: Option<PresentedClaim<'_>>, policy: &PeerAuthPolicy) -> RoleDecision {
    // No claim: a client, and not an event. Under owner decision #868 a
    // peer PREPARE carrying no covering claim is not admitted at all -- it
    // gets the same 402 greeting the client edge gives -- so there is no
    // claimless peer frame left for anything else to carry the role.
    let Some(claim) = claim else {
        return RoleDecision::client();
    };

    // P2. The claim names a channel, the channel names at most one
    // `[[peer_channels]]` row, and that row names exactly one peering
    // (`PeerChannelDuplicate`, `ChannelInBothNamespaces`). One channel,
    // one row, one relation -- nothing for a caller to resolve.
    let Some(binding) = policy.binding(claim.channel_id()) else {
        return RoleDecision::client();
    };

    match claim.verification() {
        // P3. The peer id is config's, not the interaction's: the claim
        // named a channel, and config named the relation.
        ClaimVerification::Verified => RoleDecision::peer(&binding.peer_id),
        // Config binds the channel and the claim book has no record of it.
        // The two are built from the same table, so this is a wiring fault
        // -- and it is silent everywhere else, which is the whole reason
        // §1.6 exists.
        ClaimVerification::UnknownChannel => {
            RoleDecision::refused(&binding.peer_id, UnmetRequirement::ChannelBinding)
        }
        ClaimVerification::SignatureInvalid => {
            RoleDecision::refused(&binding.peer_id, UnmetRequirement::ClaimSignature)
        }
    }
}

/// What this connector's receiving half made of a **voucher**, or of a
/// **peer-role challenge**, presented as evidence of the peer role (ADR 0075
/// decision 5, issue #1377).
///
/// Computed by the carriage, out of the x402 channel the evidence names, and
/// handed here as a verdict for the reason [`ClaimVerification`] is: the
/// signer a voucher is checked against is the one the **chain** records for
/// its channel (EVM `payerAuthorizer`, Solana `authorized_signer`), read by
/// a settlement backend this crate does not and must not depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VoucherVerification {
    /// The signature recovered to the channel's chain-recorded voucher
    /// signer, and -- for a challenge -- `expires` is inside this node's
    /// window.
    Verified,
    /// The channel was found and the signature did not recover to its
    /// voucher signer. This is also what a challenge presented in a
    /// voucher's place, or a voucher in a challenge's, becomes: the two sign
    /// different messages (`connector_signer`'s separation tests), so
    /// neither ever verifies as the other.
    SignatureInvalid,
    /// A challenge whose signature verified and whose `expires` is outside
    /// this node's window. Never a voucher's verdict: a voucher does not
    /// expire (`expiresAt` is zero, ADR 0074 decision 3).
    Expired,
}

/// A voucher or a peer-role challenge, as the role decision sees it: the
/// peering its channel's voucher signer is bound to, if any, and what this
/// node made of its signature.
///
/// `bound_peer` is resolved by the carriage from the signer the **chain**
/// records for the channel and a runtime binding this node holds
/// (`connector_runtime::Connector::voucher_signer_peer`) -- never from
/// anything the evidence declares about itself. A channel this node could
/// not resolve has no chain-recorded signer to resolve a binding from, so
/// it arrives here as unbound.
///
/// Like [`PresentedClaim`] it carries nothing a carriage could weight: no
/// amount, no carriage, no address (§1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentedVoucher<'a> {
    bound_peer: Option<&'a str>,
    verification: VoucherVerification,
}

impl<'a> PresentedVoucher<'a> {
    /// A voucher or challenge whose channel's voucher signer is bound to
    /// `bound_peer` (or to nothing), with this connector's verdict on it.
    #[must_use]
    pub fn new(bound_peer: Option<&'a str>, verification: VoucherVerification) -> Self {
        PresentedVoucher {
            bound_peer,
            verification,
        }
    }

    /// The peering the channel's voucher signer is bound to, if any.
    #[must_use]
    pub fn bound_peer(&self) -> Option<&str> {
        self.bound_peer
    }

    /// What this connector made of the signature and the window.
    #[must_use]
    pub fn verification(&self) -> VoucherVerification {
        self.verification
    }
}

/// **The decision for x402 evidence** (§1.2 as amended by ADR 0075 decision
/// 5): `peer` if and only if the frame carries a voucher, or -- for a packet
/// that moves no value -- a claim-state challenge, whose channel's voucher
/// signer is bound to a peering and whose signature verifies against that
/// signer; `client` otherwise.
///
/// Whether a challenge may count at all (the packet moves no value) is the
/// carriage's to establish before it presents one: this function sees only
/// the evidence and its verdict, as [`decide_role`] does.
///
/// # Branches
///
/// | The frame's evidence | Outcome |
/// | -------------------- | ------- |
/// | none | `client`, no event |
/// | on a channel whose signer is bound to no peering | `client`, no event |
/// | bound, signature verifies (and a challenge is in window) | `peer`, as the bound relation |
/// | bound, signature does not recover to the signer | `client` + `peer_auth_refused` (P3) |
/// | bound, a challenge outside its window | `client` + `peer_auth_refused` (P3-expires) |
///
/// An unbound channel is silent for [`decide_role`]'s reason: every client
/// paying with a voucher presents one, so an event there would fire on
/// every client packet.
#[must_use]
pub fn decide_voucher_role(voucher: Option<PresentedVoucher<'_>>) -> RoleDecision {
    let Some(voucher) = voucher else {
        return RoleDecision::client();
    };
    let Some(peer_id) = voucher.bound_peer() else {
        return RoleDecision::client();
    };
    match voucher.verification() {
        VoucherVerification::Verified => RoleDecision::peer(peer_id),
        VoucherVerification::SignatureInvalid => {
            RoleDecision::refused(peer_id, UnmetRequirement::ClaimSignature)
        }
        VoucherVerification::Expired => {
            RoleDecision::refused(peer_id, UnmetRequirement::ChallengeExpiry)
        }
    }
}

/// The event a [`PeerAuthRefusal`] becomes once rate limiting has had its
/// say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAuthRefusalReport {
    /// Always [`PEER_AUTH_REFUSED_EVENT`]; carried so a caller can emit
    /// the report without reaching for the constant separately.
    pub event: &'static str,
    /// The configured peer id whose channel was named.
    pub peer_id: String,
    /// Which of P2/P3 failed.
    pub unmet: UnmetRequirement,
    /// How many identical refusals were suppressed since the last report
    /// for this peer id and requirement. A peering whose claims do not
    /// verify keeps sending; the count is what keeps "still wrong, 4 000
    /// times" from costing 4 000 log lines while still saying it is still
    /// wrong.
    pub suppressed: u64,
}

/// The rate limit §1.6 requires on `peer_auth_refused`.
///
/// One report per (peer id, unmet requirement) per window, carrying the
/// number suppressed since the last one. Rate limiting is *not* folded
/// into [`decide_role`]: a limiter has state and a notion of time, and the
/// decision must have neither. This owns both, and takes `now_ms` as an
/// argument rather than reading a clock — so its whole behaviour is
/// testable by advancing a `u64`, with no fake clock and no sleeping test
/// (ADR 0007).
///
/// Its key space is bounded by configuration: a refusal only ever names a
/// **configured** peer id, so an anonymous caller cannot grow this map by
/// naming channels of its choosing.
#[derive(Debug, Clone)]
pub struct PeerAuthRefusalLog {
    window_ms: u64,
    windows: BTreeMap<(String, UnmetRequirement), Window>,
}

#[derive(Debug, Clone)]
struct Window {
    opened_at_ms: u64,
    suppressed: u64,
}

impl Default for PeerAuthRefusalLog {
    fn default() -> Self {
        PeerAuthRefusalLog::new(PeerAuthRefusalLog::DEFAULT_WINDOW_MS)
    }
}

impl PeerAuthRefusalLog {
    /// One report per minute per (peer id, requirement).
    ///
    /// Long enough that a peering retrying every second does not fill a
    /// log, short enough that an operator who fixes a channel row sees the
    /// refusals stop while they are still watching.
    pub const DEFAULT_WINDOW_MS: u64 = 60_000;

    /// A log with an explicit window.
    #[must_use]
    pub fn new(window_ms: u64) -> Self {
        PeerAuthRefusalLog {
            window_ms,
            windows: BTreeMap::new(),
        }
    }

    /// Record a refusal, and say whether it should be reported now.
    ///
    /// The first refusal for a (peer id, requirement) always reports:
    /// suppressing the first one would recreate the silence §1.6 exists to
    /// break. Subsequent ones inside the window are counted and returned
    /// on the next report.
    ///
    /// `now_ms` is any monotonic millisecond reading the caller already
    /// has. A reading that goes backwards (it should not, but a caller can
    /// pass anything) closes the window early rather than suppressing
    /// forever, because failing loud is the right direction for this
    /// event.
    pub fn observe(
        &mut self,
        refusal: &PeerAuthRefusal,
        now_ms: u64,
    ) -> Option<PeerAuthRefusalReport> {
        let key = (refusal.peer_id().to_string(), refusal.unmet());
        let report = |suppressed| {
            Some(PeerAuthRefusalReport {
                event: PEER_AUTH_REFUSED_EVENT,
                peer_id: refusal.peer_id().to_string(),
                unmet: refusal.unmet(),
                suppressed,
            })
        };

        match self.windows.get_mut(&key) {
            None => {
                self.windows.insert(
                    key,
                    Window {
                        opened_at_ms: now_ms,
                        suppressed: 0,
                    },
                );
                report(0)
            }
            Some(window) => {
                let elapsed = now_ms.saturating_sub(window.opened_at_ms);
                if elapsed >= self.window_ms || now_ms < window.opened_at_ms {
                    let suppressed = window.suppressed;
                    window.opened_at_ms = now_ms;
                    window.suppressed = 0;
                    report(suppressed)
                } else {
                    window.suppressed = window.suppressed.saturating_add(1);
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHANNEL: &str = "0xd1d2d3";
    const CLIENT_CHANNEL: &str = "0xc1c2c3";

    /// A connector with one fully configured peering: a `[[peers]]` entry
    /// and a `[[peer_channels]]` row binding one channel to it. The only
    /// shape that can produce a `peer`.
    fn policy_with_a_bound_peer() -> PeerAuthPolicy {
        PeerAuthPolicy::new(vec!["store-box"], vec![(CHANNEL, "store-box")])
    }

    fn presented(channel_id: &str, verification: ClaimVerification) -> PresentedClaim<'_> {
        PresentedClaim::new(channel_id, verification)
    }

    #[test]
    fn a_verified_claim_on_a_bound_channel_is_a_peer() {
        let policy = policy_with_a_bound_peer();

        let decision = decide_role(
            Some(presented(CHANNEL, ClaimVerification::Verified)),
            &policy,
        );

        assert_eq!(decision.role(), &SessionRole::peer("store-box"));
        assert_eq!(decision.refusal(), None);
    }

    // ---------------------------------------------------------------
    // §1.9, the named regression. `toon-sandbox` admitted an anonymous
    // BTP session with `btp_auth … success:true mode:"no-auth"` and then
    // treated it as a quasi-peer. Each case below is one of the four the
    // spec names, asserted at the decision. The carriages owe the same
    // four end-to-end, over their own frames (issues #727 and #728).
    // ---------------------------------------------------------------

    /// §1.9(1): no claim at all — the anonymous session itself.
    #[test]
    fn named_regression_no_claim_is_a_client() {
        let decision = decide_role(None, &policy_with_a_bound_peer());

        assert_eq!(decision.role(), &SessionRole::Client);
        assert_eq!(decision.refusal(), None);
    }

    /// §1.9(2): a claim on a channel this connector binds to no peering —
    /// every ordinary client's claim, and the reason this branch must be
    /// silent.
    #[test]
    fn named_regression_a_claim_on_an_unbound_channel_is_a_client() {
        for verification in [
            ClaimVerification::Verified,
            ClaimVerification::UnknownChannel,
            ClaimVerification::SignatureInvalid,
        ] {
            let decision = decide_role(
                Some(presented(CLIENT_CHANNEL, verification)),
                &policy_with_a_bound_peer(),
            );

            assert_eq!(decision.role(), &SessionRole::Client);
            assert_eq!(
                decision.refusal(),
                None,
                "a claim on an unbound channel must not fire peer_auth_refused: every client \
                 covers every packet with a claim of its own, so emitting here is both noise \
                 and a log-volume lever an anonymous caller can pull"
            );
        }
    }

    /// §1.9(3): a claim on a configured peer channel whose signature does
    /// not recover to the counterparty key that row configures. P3 alone
    /// failing — and loud, because it is a real peering that is not
    /// peering.
    #[test]
    fn named_regression_a_signature_that_does_not_verify_is_a_client() {
        let decision = decide_role(
            Some(presented(CHANNEL, ClaimVerification::SignatureInvalid)),
            &policy_with_a_bound_peer(),
        );

        assert_eq!(decision.role(), &SessionRole::Client);
        assert_eq!(
            decision.refusal().map(PeerAuthRefusal::unmet),
            Some(UnmetRequirement::ClaimSignature)
        );
    }

    /// §1.9(4): P2 alone failing — config binds the channel and the claim
    /// book holds no record of it, so there is nothing to verify against.
    #[test]
    fn named_regression_a_bound_channel_with_no_record_is_a_client() {
        let decision = decide_role(
            Some(presented(CHANNEL, ClaimVerification::UnknownChannel)),
            &policy_with_a_bound_peer(),
        );

        assert_eq!(decision.role(), &SessionRole::Client);
        assert_eq!(
            decision.refusal().map(PeerAuthRefusal::unmet),
            Some(UnmetRequirement::ChannelBinding)
        );
    }

    /// A `[[peer_channels]]` row naming a peer no `[[peers]]` entry
    /// configures binds nothing, so even a verified claim on it takes no
    /// role. `Config::load` refuses that shape
    /// (`ConfigError::PeerChannelOrphaned`); this is the second lock.
    #[test]
    fn a_channel_bound_to_no_configured_peering_is_a_client() {
        let policy = PeerAuthPolicy::new(Vec::<&str>::new(), vec![(CHANNEL, "store-box")]);

        let decision = decide_role(
            Some(presented(CHANNEL, ClaimVerification::Verified)),
            &policy,
        );

        assert_eq!(decision.role(), &SessionRole::Client);
        assert_eq!(decision.refusal(), None);
    }

    /// A connector that configures no peerings at all — the fleet's
    /// ordinary shape today. Nothing can be a peer, whatever it presents.
    #[test]
    fn an_empty_policy_admits_no_peer() {
        let policy = PeerAuthPolicy::default();

        let decision = decide_role(
            Some(presented(CHANNEL, ClaimVerification::Verified)),
            &policy,
        );

        assert_eq!(decision.role(), &SessionRole::Client);
        assert_eq!(decision.refusal(), None);
    }

    /// The structural invariant behind §1.6: a refusal *is* a downgrade.
    /// No branch may return one beside a `peer` role.
    #[test]
    fn a_refusal_never_accompanies_a_peer_role() {
        let policy = PeerAuthPolicy::new(
            vec!["bound-box", "unbound-box"],
            vec![(CHANNEL, "bound-box"), ("0xbeef", "ghost-box")],
        );

        for channel in [CHANNEL, "0xbeef", CLIENT_CHANNEL] {
            for verification in [
                ClaimVerification::Verified,
                ClaimVerification::UnknownChannel,
                ClaimVerification::SignatureInvalid,
            ] {
                let decision = decide_role(Some(presented(channel, verification)), &policy);

                if decision.refusal().is_some() {
                    assert_eq!(
                        decision.role(),
                        &SessionRole::Client,
                        "{channel}/{verification:?}: a refusal must accompany a downgrade"
                    );
                }
            }
        }
    }

    /// The same inputs always give the same verdict. The decision reads no
    /// clock, no socket and no counter, so there is nothing for a second
    /// call to differ on — and a future branch that reached for one would
    /// have to break this.
    #[test]
    fn the_decision_is_a_pure_function_of_its_two_arguments() {
        let policy = policy_with_a_bound_peer();
        let claim = presented(CHANNEL, ClaimVerification::SignatureInvalid);

        assert_eq!(
            decide_role(Some(claim), &policy),
            decide_role(Some(claim), &policy)
        );
    }

    /// §1.3, as far as the type system can put it: the only things a
    /// presented claim carries are a channel id and a verification
    /// verdict, so two interactions differing in *anything else* --
    /// carriage, port, source address, SNI, client certificate,
    /// subprotocol, history -- are the same input and get the same answer,
    /// because there is no way to express the difference.
    #[test]
    fn a_presented_claim_carries_nothing_a_carriage_could_weight() {
        let policy = policy_with_a_bound_peer();

        // The two carriages decode their own bytes -- raw JSON on BTP,
        // base64 in a header on HTTP -- into one `WireClaim`, and this is
        // all that survives of it here.
        let over_btp = presented(CHANNEL, ClaimVerification::Verified);
        let over_http = presented(CHANNEL, ClaimVerification::Verified);

        assert_eq!(
            decide_role(Some(over_btp), &policy),
            decide_role(Some(over_http), &policy)
        );
    }

    // ---------------------------------------------------------------
    // ADR 0075 decision 5: a voucher, or a challenge for a packet that
    // moves no value, from a bound channel's voucher signer.
    // ---------------------------------------------------------------

    fn voucher(
        bound_peer: Option<&str>,
        verification: VoucherVerification,
    ) -> PresentedVoucher<'_> {
        PresentedVoucher::new(bound_peer, verification)
    }

    #[test]
    fn a_verified_voucher_on_a_bound_channel_is_that_peer() {
        let decision = decide_voucher_role(Some(voucher(
            Some("store-box"),
            VoucherVerification::Verified,
        )));

        assert_eq!(decision.role(), &SessionRole::peer("store-box"));
        assert_eq!(decision.refusal(), None);
    }

    /// A voucher on a channel bound to no peering is an ordinary client's,
    /// whatever its signature does: silent, for `decide_role`'s reason.
    #[test]
    fn a_voucher_on_an_unbound_channel_is_a_client_and_no_event() {
        for verification in [
            VoucherVerification::Verified,
            VoucherVerification::SignatureInvalid,
            VoucherVerification::Expired,
        ] {
            let decision = decide_voucher_role(Some(voucher(None, verification)));

            assert_eq!(decision.role(), &SessionRole::Client);
            assert_eq!(decision.refusal(), None, "{verification:?}");
        }
        assert_eq!(decide_voucher_role(None).role(), &SessionRole::Client);
    }

    #[test]
    fn a_bound_channels_bad_signature_or_expired_challenge_is_a_loud_client() {
        for (verification, unmet) in [
            (
                VoucherVerification::SignatureInvalid,
                UnmetRequirement::ClaimSignature,
            ),
            (
                VoucherVerification::Expired,
                UnmetRequirement::ChallengeExpiry,
            ),
        ] {
            let decision = decide_voucher_role(Some(voucher(Some("store-box"), verification)));

            assert_eq!(decision.role(), &SessionRole::Client);
            assert_eq!(
                decision
                    .refusal()
                    .map(|refusal| (refusal.peer_id(), refusal.unmet())),
                Some(("store-box", unmet))
            );
        }
    }

    #[test]
    fn a_requirement_names_itself_as_the_spec_does() {
        assert_eq!(UnmetRequirement::ChannelBinding.name(), "P2");
        assert_eq!(UnmetRequirement::ClaimSignature.name(), "P3");
        assert_eq!(UnmetRequirement::ChallengeExpiry.name(), "P3-expires");
        assert_eq!(PEER_AUTH_REFUSED_EVENT, "peer_auth_refused");
    }

    #[test]
    fn the_refused_peer_id_is_the_configured_one() {
        let decision = decide_role(
            Some(presented(CHANNEL, ClaimVerification::SignatureInvalid)),
            &policy_with_a_bound_peer(),
        );

        assert_eq!(
            decision.refusal().map(PeerAuthRefusal::peer_id),
            Some("store-box")
        );
    }

    #[test]
    fn a_decision_can_be_split_into_its_role_and_its_event() {
        let (role, refusal) = decide_role(
            Some(presented(CHANNEL, ClaimVerification::SignatureInvalid)),
            &policy_with_a_bound_peer(),
        )
        .into_parts();

        assert_eq!(role, SessionRole::Client);
        assert_eq!(
            refusal.map(|r| r.unmet()),
            Some(UnmetRequirement::ClaimSignature)
        );
    }

    // ---------------------------------------------------------------
    // The rate limit (§1.6).
    // ---------------------------------------------------------------

    fn refusal(peer_id: &str, unmet: UnmetRequirement) -> PeerAuthRefusal {
        PeerAuthRefusal {
            peer_id: peer_id.to_string(),
            unmet,
        }
    }

    #[test]
    fn the_first_refusal_always_reports() {
        let mut log = PeerAuthRefusalLog::default();

        let report = log
            .observe(&refusal("store-box", UnmetRequirement::ClaimSignature), 0)
            .expect("first refusal reports");

        assert_eq!(report.event, PEER_AUTH_REFUSED_EVENT);
        assert_eq!(report.peer_id, "store-box");
        assert_eq!(report.unmet, UnmetRequirement::ClaimSignature);
        assert_eq!(report.suppressed, 0);
    }

    #[test]
    fn refusals_inside_the_window_are_counted_and_reported_on_the_next_one() {
        let mut log = PeerAuthRefusalLog::new(60_000);
        let refusal = refusal("store-box", UnmetRequirement::ClaimSignature);

        assert!(log.observe(&refusal, 0).is_some());
        for tick in 1..=5 {
            assert!(log.observe(&refusal, tick * 1_000).is_none());
        }

        let report = log
            .observe(&refusal, 60_000)
            .expect("the window has closed");

        assert_eq!(report.suppressed, 5);
    }

    /// Two different mistakes on two different peerings are two different
    /// operator problems, and one must not silence the other. The two
    /// `verify_signature` outcomes ride the same limiter and keep their own
    /// windows, so an unknown channel does not hide a bad signature.
    #[test]
    fn the_window_is_per_peer_and_per_requirement() {
        let mut log = PeerAuthRefusalLog::new(60_000);

        assert!(log
            .observe(&refusal("store-box", UnmetRequirement::ClaimSignature), 0)
            .is_some());
        assert!(log
            .observe(&refusal("store-box", UnmetRequirement::ChannelBinding), 0)
            .is_some());
        assert!(log
            .observe(&refusal("relay-box", UnmetRequirement::ClaimSignature), 0)
            .is_some());
    }

    /// A reading that goes backwards must not suppress forever. Failing
    /// loud is the right direction for an event whose whole purpose is to
    /// break a silence.
    #[test]
    fn a_clock_that_goes_backwards_reopens_the_window() {
        let mut log = PeerAuthRefusalLog::new(60_000);
        let refusal = refusal("store-box", UnmetRequirement::ClaimSignature);

        assert!(log.observe(&refusal, 10_000).is_some());
        assert!(log.observe(&refusal, 10_001).is_none());
        assert!(log.observe(&refusal, 5_000).is_some());
    }

    /// End to end: a claim that does not verify downgrades silently on the
    /// wire and loudly to the operator. This is the pairing §1.6 asks for,
    /// and the reason the two halves live in one returned value.
    #[test]
    fn an_asserted_role_downgrades_silently_and_reports_loudly() {
        let policy = policy_with_a_bound_peer();
        let mut log = PeerAuthRefusalLog::default();

        for (verification, expected) in [
            (ClaimVerification::SignatureInvalid, "P3"),
            (ClaimVerification::UnknownChannel, "P2"),
        ] {
            let decision = decide_role(Some(presented(CHANNEL, verification)), &policy);
            let report = decision
                .refusal()
                .and_then(|refusal| log.observe(refusal, 0));

            assert_eq!(
                decision.role(),
                &SessionRole::Client,
                "refusing on the wire would make the check a peering oracle (§1.6)"
            );
            assert_eq!(
                report.map(|report| (report.event, report.unmet.name())),
                Some((PEER_AUTH_REFUSED_EVENT, expected)),
                "a peering whose claims do not verify must not present as 'peering \
                 configured, nothing peers, no error anywhere' (§1.6)"
            );
        }
    }
}
