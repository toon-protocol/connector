//! **Role** (`peer-carriage-spec.md` §1.2), for both carriages.
//!
//! One function, called from three places — the BTP peer session, the
//! ILP-over-HTTP peer handler, and the client edge's front door where a
//! shared listener decides whether an arrival is peer handling's at all.
//! Shared for the same reason [`crate::price_gate::payment_required`] is:
//! §0.1's one pipeline must not admit over one carriage what it refuses
//! over the other, and a rule written twice is a rule that drifts.
//!
//! What lives here is only the **join**. §1.2's rule itself is
//! [`connector_peer_auth::decide_role`]'s and
//! [`connector_peer_auth::decide_voucher_role`]'s, which see a verdict and
//! the binding it resolves to and nothing else (§1.3). This is the bridge
//! between them and the two places a verdict comes from, and it exists so
//! neither side has to grow a dependency on the other.
//!
//! # Two proofs, until #1380
//!
//! ADR 0075 decision 5 makes the peer role's proof **a voucher on an x402
//! channel bound to that peer**, or -- for a packet that moves no value --
//! **the voucher claim-state challenge** signed by such a channel's voucher
//! signer. Both are resolved by the receiving half ([`VoucherEvidence`]):
//! the channel is found and its signer read from the chain, never from the
//! evidence, and that signer is looked up in this node's runtime bindings
//! ([`Connector::voucher_signer_peer`]).
//!
//! The `toon-channel` claim still proves the peer role too, verified against
//! the counterparty key `[[peer_channels]]` configures
//! ([`connector_runtime::ClaimBook`]): config-declared peerings still prove
//! themselves that way, and #1380 deletes it once the last one has moved.
//!
//! # Judging a peer's voucher (ADR 0075 decision 6, issue #1378)
//!
//! Once a voucher has decided the role, it is judged as payment by the same
//! receiving half ([`VoucherEvidence::judge_peer_voucher`]): admitted by the
//! rules a client's is, held to the channel's **one** amount watermark
//! whichever role its vouchers arrive under (`peer-carriage-spec.md` §1.8),
//! and journaled. Its verdict rides back in the `claim-ack` exactly as a
//! `toon-channel` claim's does, and [`crate::price_gate`] measures coverage
//! by the advance it made ([`judge_voucher`]).

use async_trait::async_trait;
use connector_btp::{BtpFrame, BTP_MESSAGE, CLAIM_PROTOCOL, PEER_CHALLENGE_PROTOCOL};
use connector_domain::client_claim::ClientClaim;
use connector_domain::{Watermark, VOUCHER_WATERMARK_NONCE};
use connector_peer_auth::SessionRole;
use connector_peer_auth::{
    decide_role, decide_voucher_role, ClaimVerification, PeerAuthPolicy, PresentedClaim,
    PresentedVoucher, RoleDecision, VoucherVerification,
};
use connector_runtime::{ClaimAckOutcome, ClaimRejectReason, Connector, VoucherSigner, WireClaim};

use crate::challenge_json::PeerRoleChallenge;
use crate::claim_json::PresentedPeerClaim;

/// The longest ahead of this node's clock a peer-role challenge's `expires`
/// may lie, in seconds (ADR 0075: "`expires` is to be kept short", a bound
/// "the implementing ticket's to fix").
///
/// Within `expires` a challenge is a bearer proof for zero-value traffic: it
/// names one channel, so only that channel's receiver can use it, but that
/// receiver can replay it until it lapses. Five minutes is long enough to
/// absorb clock skew between two operators' hosts and a dialer that signs
/// one challenge per few packets rather than one per packet, and short
/// enough that a captured challenge is stale before anyone could act on it
/// at a scale that matters -- it moves no value either way. A challenge
/// signed further ahead than this is refused as if it had expired, so a
/// peer cannot mint one long-lived proof and skip the bound.
pub const MAX_PEER_CHALLENGE_LIFETIME_SECS: u64 = 300;

/// What the receiving half made of a voucher or a challenge: the channel's
/// voucher signer **as the chain records it**, and whether the signature
/// recovered to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoucherCheck {
    /// The channel was found and the signature recovered to its signer.
    Verified(VoucherSigner),
    /// The channel was found and the signature did not recover to its
    /// signer -- including a challenge presented as a voucher, or a voucher
    /// as a challenge, since neither message verifies as the other.
    SignatureInvalid(VoucherSigner),
    /// No channel this node admits: unknown, unreadable, a config that does
    /// not hash to the id, a chain this node has not opted in on, or a
    /// lookup that failed or was budgeted. There is no chain-recorded
    /// signer, so nothing to resolve a binding from.
    Unresolved,
}

/// What judging a peer's voucher as payment produced (ADR 0075 decision
/// 6): the verdict the `claim-ack` carries, and the channel's watermark
/// just before it -- what coverage is measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerVoucherVerdict {
    pub ack: ClaimAckOutcome,
    /// The channel's accepted cumulative amount before this voucher was
    /// judged, zero for a channel nothing was accepted on yet.
    pub prior: u64,
}

/// The receiving half, as the peer carriages ask it (ADR 0075 decisions 3,
/// 5 and 6).
///
/// For the **role** ([`Self::check_voucher`], [`Self::check_challenge`]):
/// resolve the x402 channel a voucher or a challenge names, read its voucher
/// signer from what the backend admitted, and check the signature. Nothing
/// is admitted, advanced or journaled -- §1.5 fixes role before any
/// watermark moves.
///
/// For the **payment**, downstream of a role already decided `peer`
/// ([`Self::judge_peer_voucher`]): admit, advance and journal, by exactly
/// the rules a client's voucher is judged by, against the channel's one
/// watermark (§1.8). `connector-client-edge`'s claim gate is the
/// implementation, because it holds the batch-settlement backends, the
/// journaled configs, and that watermark.
#[async_trait]
pub trait VoucherEvidence: Send + Sync {
    /// A voucher (`ClientClaim::EvmVoucher` or `ClientClaim::SolanaVoucher`).
    async fn check_voucher(&self, voucher: &ClientClaim) -> VoucherCheck;

    /// A peer-role challenge. Its `expires` is the role gate's to judge,
    /// not this method's.
    async fn check_challenge(&self, challenge: &PeerRoleChallenge) -> VoucherCheck;

    /// Judge a voucher that proved the peer role as payment: accepted and
    /// durably journaled, or refused naming why. A voucher the channel's
    /// watermark already holds, resent byte-identically, is accepted and
    /// advances nothing.
    async fn judge_peer_voucher(&self, voucher: &ClientClaim) -> PeerVoucherVerdict;
}

/// What a frame paid, as judged below the role: the claim ack, the
/// cumulative amount the judged evidence names, and the channel's
/// watermark just before it -- the three figures
/// [`crate::price_gate::payment_required`] measures coverage by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Judged {
    pub ack: ClaimAckOutcome,
    pub claimed: Option<u64>,
    pub prior: Option<Watermark>,
}

/// Judge a peer frame's voucher as payment (ADR 0075 decision 6), or
/// `None` when there is none to judge here: no voucher, a frame that is not
/// a peer's (a client's voucher is the client edge's to judge, never this
/// pipeline's, §1.7), or a carriage built without the receiving half.
pub async fn judge_voucher(
    role: &SessionRole,
    vouchers: Option<&dyn VoucherEvidence>,
    voucher: Option<&ClientClaim>,
) -> Option<Judged> {
    let (Some(_), Some(voucher), Some(vouchers)) = (role.peer_id(), voucher, vouchers) else {
        return None;
    };
    let verdict = vouchers.judge_peer_voucher(voucher).await;
    Some(Judged {
        ack: verdict.ack,
        claimed: Some(voucher.transferred_amount()),
        prior: Some(Watermark {
            nonce: VOUCHER_WATERMARK_NONCE,
            cumulative_amount: verdict.prior,
        }),
    })
}

/// Everything one frame presents that could prove the peer role.
///
/// A carriage refuses a frame carrying both a claim and a challenge before
/// building one of these (§1.5: duplicated authentication material is
/// refused, never resolved), so at most one of the two decides.
#[derive(Debug, Clone, Default)]
pub struct FrameEvidence {
    /// The claim slot, decoded: a `toon-channel` claim or a voucher.
    pub claim: Option<PresentedPeerClaim>,
    /// The `peer-role-challenge` slot, decoded.
    pub challenge: Option<PeerRoleChallenge>,
    /// Whether the frame's packet is a PREPARE whose `amount` is zero. A
    /// challenge proves the role only for such a packet (ADR 0075 decision
    /// 5); on anything else it proves nothing.
    pub moves_no_value: bool,
}

impl FrameEvidence {
    /// The voucher, if that is what the claim slot held: judged below the
    /// role by the receiving half ([`judge_voucher`]).
    #[must_use]
    pub fn voucher(&self) -> Option<&ClientClaim> {
        match &self.claim {
            Some(PresentedPeerClaim::Voucher(voucher)) => Some(voucher),
            Some(PresentedPeerClaim::Channel(_)) | None => None,
        }
    }

    /// The `toon-channel` claim, if that is what the claim slot held: the
    /// one kind of evidence `ClaimBook` judges below the role. A voucher is
    /// judged by the receiving half instead ([`judge_voucher`]).
    #[must_use]
    pub fn into_channel_claim(self) -> Option<WireClaim> {
        match self.claim {
            Some(PresentedPeerClaim::Channel(claim)) => Some(claim),
            Some(PresentedPeerClaim::Voucher(_)) | None => None,
        }
    }
}

/// §1.5's smuggling defence, over both evidence slots: which duplicated
/// authentication material a frame or request carried. Refused, never
/// resolved -- "which one did we check?" has no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmbiguousEvidence {
    /// More than one claim.
    Claims,
    /// More than one peer-role challenge.
    Challenges,
    /// A claim beside a challenge (ADR 0075 decision 5).
    ClaimAndChallenge,
}

impl AmbiguousEvidence {
    /// The one rule, from how many of each slot were presented. Counted
    /// before anything is parsed, so an undecodable second entry cannot be
    /// discarded to leave one unambiguous one standing.
    ///
    /// # Errors
    ///
    /// The ambiguity, when there is one.
    pub fn check(claims: usize, challenges: usize) -> Result<(), AmbiguousEvidence> {
        match (claims, challenges) {
            (claims, _) if claims > 1 => Err(AmbiguousEvidence::Claims),
            (_, challenges) if challenges > 1 => Err(AmbiguousEvidence::Challenges),
            (1, 1) => Err(AmbiguousEvidence::ClaimAndChallenge),
            _ => Ok(()),
        }
    }

    /// What a BTP ERROR frame says about it.
    #[must_use]
    pub fn message(self) -> &'static [u8] {
        match self {
            AmbiguousEvidence::Claims => b"more than one claim entry on one frame",
            AmbiguousEvidence::Challenges => {
                b"more than one peer-role challenge entry on one frame"
            }
            AmbiguousEvidence::ClaimAndChallenge => {
                b"a claim and a peer-role challenge on one frame"
            }
        }
    }
}

/// Everything a BTP frame presents, decoded -- the one reading both the
/// peer session and the client edge's front door make of a frame.
///
/// # Errors
///
/// [`AmbiguousEvidence`] when the frame carries duplicated evidence (§1.5).
pub fn btp_evidence(frame: &BtpFrame) -> Result<FrameEvidence, AmbiguousEvidence> {
    let count = |name: &str| {
        frame
            .protocol_data
            .iter()
            .filter(|entry| entry.name == name)
            .count()
    };
    AmbiguousEvidence::check(count(CLAIM_PROTOCOL), count(PEER_CHALLENGE_PROTOCOL))?;
    let slot = |name: &str| {
        frame
            .protocol_data
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.data.as_slice())
    };
    Ok(FrameEvidence {
        claim: slot(CLAIM_PROTOCOL).and_then(decode_claim),
        challenge: slot(PEER_CHALLENGE_PROTOCOL).and_then(decode_challenge),
        moves_no_value: frame.frame_type == BTP_MESSAGE && moves_no_value(&frame.ilp_packet),
    })
}

/// A claim slot's raw JSON, decoded; `None`, with a warning, when it does
/// not decode. An undecodable claim is *not acknowledged* (§6.3) rather than
/// rejected, so the payer's claim stays pending and its retransmission is
/// read the same way.
#[must_use]
pub fn decode_claim(raw: &[u8]) -> Option<PresentedPeerClaim> {
    crate::claim_json::parse_presented(raw)
        .inspect_err(|error| {
            // No peer id to name: the claim *is* what would have named one,
            // and it did not decode.
            tracing::warn!(%error, "peer claim could not be decoded; not acknowledged");
        })
        .ok()
}

/// The role of one frame, from the `toon-channel` claim that frame carries.
///
/// `claim` is `None` for a frame carrying none, which is a client frame:
/// under owner decision #868 a peer PREPARE with no covering claim is not
/// admitted at all, so there is no claimless peer frame for anything else
/// to carry the role on.
///
/// **Nothing is accepted, advanced or journaled here.** The claim is
/// verified against this node's own record of its channel and no more;
/// judging it — the watermark, the ledger, the ack — is
/// `Connector::handle_peer_claim`'s, downstream of the role, exactly as
/// §1.5 requires ("role MUST still be fixed before the packet is routed,
/// before a fee is taken … and before any watermark is advanced or anything
/// is journaled").
#[must_use]
pub fn decide(
    connector: &Connector,
    policy: &PeerAuthPolicy,
    claim: Option<&WireClaim>,
) -> RoleDecision {
    let presented = claim.map(|claim| {
        let verification = match connector.verify_peer_claim(claim) {
            Ok(()) => ClaimVerification::Verified,
            Err(ClaimRejectReason::UnknownChannel) => ClaimVerification::UnknownChannel,
            // `verify_signature` answers only those two, and a third would
            // be a signature this node could not vouch for either way --
            // which is `SignatureInvalid`'s meaning, not `Verified`'s.
            Err(_) => ClaimVerification::SignatureInvalid,
        };
        PresentedClaim::new(&claim.channel_id, verification)
    });
    decide_role(presented, policy)
}

/// The role of one frame, from everything it presents (§1.2 as amended by
/// ADR 0075 decision 5): a `toon-channel` claim, a voucher, or -- for a
/// packet that moves no value -- a peer-role challenge.
///
/// `vouchers` is `None` on a carriage built without the receiving half, on
/// which a voucher or a challenge proves nothing and the frame is a client's.
/// A node with no voucher signer bound at all
/// ([`Connector::has_voucher_bindings`]) is answered the same way without
/// asking the receiving half anything, which is what every client voucher
/// costs such a node: nothing.
///
/// Nothing is admitted, advanced or journaled here, on any branch.
pub async fn decide_frame(
    connector: &Connector,
    policy: &PeerAuthPolicy,
    vouchers: Option<&dyn VoucherEvidence>,
    evidence: &FrameEvidence,
) -> RoleDecision {
    let vouchers = vouchers.filter(|_| connector.has_voucher_bindings());
    match &evidence.claim {
        Some(PresentedPeerClaim::Channel(claim)) => decide(connector, policy, Some(claim)),
        Some(PresentedPeerClaim::Voucher(voucher)) => {
            let Some(vouchers) = vouchers else {
                return decide_voucher_role(None);
            };
            let check = vouchers.check_voucher(voucher).await;
            decide_voucher_role(presented(connector, check, true).as_ref().map(as_presented))
        }
        None => match (&evidence.challenge, vouchers) {
            (Some(challenge), Some(vouchers)) if evidence.moves_no_value => {
                let check = vouchers.check_challenge(challenge).await;
                let in_window = challenge_in_window(challenge.expires(), connector.now_unix());
                decide_voucher_role(
                    presented(connector, check, in_window)
                        .as_ref()
                        .map(as_presented),
                )
            }
            _ => decide(connector, policy, None),
        },
    }
}

/// Whether `ilp_packet` is a PREPARE that moves no value -- the only packet
/// a peer-role challenge proves the role for (ADR 0075 decision 5). Anything
/// else, an undecodable packet or none at all included, moves value as far
/// as the role is concerned, and a challenge on it proves nothing.
#[must_use]
pub fn moves_no_value(ilp_packet: &[u8]) -> bool {
    connector_domain::Prepare::decode(ilp_packet).is_ok_and(|prepare| prepare.amount == 0)
}

/// A peer-role challenge's raw JSON, decoded; `None`, with a warning, when
/// it does not decode. An unreadable challenge proves nothing and is not
/// refused for it, as an unreadable claim is not: the frame is simply not
/// a peer's.
#[must_use]
pub fn decode_challenge(raw: &[u8]) -> Option<PeerRoleChallenge> {
    crate::challenge_json::parse(raw)
        .inspect_err(|error| {
            tracing::warn!(%error, "peer-role challenge could not be decoded; it proves nothing");
        })
        .ok()
}

/// Whether a challenge expiring at `expires` is inside this node's window at
/// `now`: not yet passed, and no further ahead than
/// [`MAX_PEER_CHALLENGE_LIFETIME_SECS`].
#[must_use]
pub fn challenge_in_window(expires: u64, now: u64) -> bool {
    expires > now && expires - now <= MAX_PEER_CHALLENGE_LIFETIME_SECS
}

/// A check, as the peering its signer is bound to and a verdict. `None` for
/// a channel the receiving half could not resolve.
fn presented(
    connector: &Connector,
    check: VoucherCheck,
    in_window: bool,
) -> Option<(Option<String>, VoucherVerification)> {
    let (signer, verification) = match check {
        VoucherCheck::Verified(signer) if in_window => (signer, VoucherVerification::Verified),
        VoucherCheck::Verified(signer) => (signer, VoucherVerification::Expired),
        VoucherCheck::SignatureInvalid(signer) => (signer, VoucherVerification::SignatureInvalid),
        VoucherCheck::Unresolved => return None,
    };
    Some((connector.voucher_signer_peer(&signer), verification))
}

fn as_presented(
    (bound_peer, verification): &(Option<String>, VoucherVerification),
) -> PresentedVoucher<'_> {
    PresentedVoucher::new(bound_peer.as_deref(), *verification)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambiguous_evidence_is_counted_across_both_slots() {
        assert_eq!(AmbiguousEvidence::check(0, 0), Ok(()));
        assert_eq!(AmbiguousEvidence::check(1, 0), Ok(()));
        assert_eq!(AmbiguousEvidence::check(0, 1), Ok(()));
        assert_eq!(
            AmbiguousEvidence::check(2, 0),
            Err(AmbiguousEvidence::Claims)
        );
        assert_eq!(
            AmbiguousEvidence::check(0, 2),
            Err(AmbiguousEvidence::Challenges)
        );
        assert_eq!(
            AmbiguousEvidence::check(1, 1),
            Err(AmbiguousEvidence::ClaimAndChallenge)
        );
    }

    #[test]
    fn a_challenge_is_in_window_only_until_it_expires_and_no_further_ahead_than_the_bound() {
        let now = 1_800_000_000;
        assert!(challenge_in_window(now + 1, now));
        assert!(challenge_in_window(
            now + MAX_PEER_CHALLENGE_LIFETIME_SECS,
            now
        ));
        assert!(!challenge_in_window(now, now), "expires is exclusive");
        assert!(!challenge_in_window(now - 1, now));
        assert!(!challenge_in_window(
            now + MAX_PEER_CHALLENGE_LIFETIME_SECS + 1,
            now
        ));
    }
}
