//! The decision itself (§1.2), and the operator event a failed assertion
//! owes (§1.6).

use std::collections::BTreeMap;

use crate::role::SessionRole;

/// The name of the operator-visible event §1.6 requires, declared once so
/// a log line, a metric label and a test cannot each spell it differently.
pub const PEER_AUTH_REFUSED_EVENT: &str = "peer_auth_refused";

/// Which of §1.2's requirements an assertion failed to meet.
///
/// Carried on the operator event because each has a different fix, and
/// "peering configured, nothing peers, no error anywhere" is the symptom
/// §1.6 exists to prevent.
///
/// There is no P2 any more (ADR 0075, issue #1380). It was a `toon-channel`
/// claim naming a `[[peer_channels]]` channel this node held no record of --
/// a wiring fault between config and the retired claim book. A voucher's
/// channel is read from the chain, so there is no second record to
/// disagree with the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnmetRequirement {
    /// **P3** — a voucher, or a peer-role challenge, on a channel whose
    /// voucher signer is bound to a peering, whose signature does not
    /// recover to that signer ([`VoucherVerification::SignatureInvalid`],
    /// ADR 0075 decision 5): a key that is not the one the chain records for
    /// the channel.
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
    /// It comes from this node's own voucher-signer binding, never from the
    /// interaction — the evidence names a channel, the chain names its
    /// signer, and the binding names the peering, so an attacker-chosen
    /// string never reaches this log line. Evidence whose signer is bound
    /// to no peering produces no refusal to carry one (see
    /// [`decide_voucher_role`]).
    #[must_use]
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// Which requirement failed.
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

/// What this connector's receiving half made of a **voucher**, or of a
/// **peer-role challenge**, presented as evidence of the peer role (ADR 0075
/// decision 5, issue #1377).
///
/// Computed by the carriage, out of the x402 channel the evidence names, and
/// handed here as a verdict: the signer a voucher is checked against is the
/// one the **chain** records for its channel (EVM `payerAuthorizer`, Solana
/// `authorized_signer`), read by a settlement backend this crate does not
/// and must not depend on (§1.3, and
/// `crate::tests::the_decision_crate_cannot_name_a_transport`).
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
/// It carries nothing a carriage could weight: no amount, no carriage, no
/// address (§1.3). What a voucher is *worth* is the receiving half's
/// question, downstream of the role.
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
/// the evidence and its verdict.
///
/// Those are the only inputs, and that is the security property. There is
/// no parameter for the carriage, the listener, the port, the bind address,
/// the source address, the TLS SNI name, a client certificate, the `btp`
/// subprotocol, an endpoint from `[[peers]]`, the shape of what was sent,
/// or anything this or another interaction did earlier — §1.3's list,
/// absent by construction rather than by convention. It is a free function
/// for the same reason: a method on a session would have a `self` with
/// fields, and every one of those fields is something §1.3 forbids
/// consulting.
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
/// An unbound channel produces no event, and that is §1.6 read literally: an
/// assertion is one that names a configured peering and fails. Every client
/// paying with a voucher presents one, so an event there would fire on
/// every client packet -- both noise and a log-volume lever any anonymous
/// caller could pull.
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
    /// Which requirement failed.
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
/// into [`decide_voucher_role`]: a limiter has state and a notion of time, and the
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

    // ---------------------------------------------------------------
    // §1.9, the named regression. `toon-sandbox` admitted an anonymous
    // BTP session with `btp_auth … success:true mode:"no-auth"` and then
    // treated it as a quasi-peer. Each case below is one of the four the
    // spec names, asserted at the decision. The carriages owe the same
    // four end-to-end, over their own frames (issues #727 and #728).
    // ---------------------------------------------------------------

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
    /// whatever its signature does: silent (§1.6).
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

    /// A voucher whose signature does not verify on a bound channel
    /// downgrades silently on the wire and loudly to the operator: the
    /// pairing §1.6 asks for, and the reason the two halves live in one
    /// returned value.
    #[test]
    fn an_asserted_role_downgrades_silently_and_reports_loudly() {
        let mut log = PeerAuthRefusalLog::default();

        for (verification, expected) in [
            (VoucherVerification::SignatureInvalid, "P3"),
            (VoucherVerification::Expired, "P3-expires"),
        ] {
            let decision = decide_voucher_role(Some(voucher(Some("store-box"), verification)));
            let (role, refusal) = decision.into_parts();
            let report = refusal.as_ref().and_then(|refusal| log.observe(refusal, 0));

            assert_eq!(
                role,
                SessionRole::Client,
                "refusing on the wire would make the check a peering oracle (§1.6)"
            );
            assert_eq!(
                refusal.as_ref().map(PeerAuthRefusal::peer_id),
                Some("store-box"),
                "the refused peer id is the bound one"
            );
            assert_eq!(
                report.map(|report| (report.event, report.unmet.name())),
                Some((PEER_AUTH_REFUSED_EVENT, expected))
            );
        }
    }

    #[test]
    fn a_requirement_names_itself_as_the_spec_does() {
        assert_eq!(UnmetRequirement::ClaimSignature.name(), "P3");
        assert_eq!(UnmetRequirement::ChallengeExpiry.name(), "P3-expires");
        assert_eq!(PEER_AUTH_REFUSED_EVENT, "peer_auth_refused");
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
    /// refusals ride the same limiter and keep their own
    /// windows, so an expired challenge does not hide a bad signature.
    #[test]
    fn the_window_is_per_peer_and_per_requirement() {
        let mut log = PeerAuthRefusalLog::new(60_000);

        assert!(log
            .observe(&refusal("store-box", UnmetRequirement::ClaimSignature), 0)
            .is_some());
        assert!(log
            .observe(&refusal("store-box", UnmetRequirement::ChallengeExpiry), 0)
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
}
