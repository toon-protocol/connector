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
//! ([`connector_runtime::ClaimBook`]): every peering in the tree still
//! proves itself that way, and #1380 deletes it once the last one has moved.

use async_trait::async_trait;
use connector_domain::client_claim::ClientClaim;
use connector_peer_auth::{
    decide_role, decide_voucher_role, ClaimVerification, PeerAuthPolicy, PresentedClaim,
    PresentedVoucher, RoleDecision, VoucherVerification,
};
use connector_runtime::{ClaimRejectReason, Connector, VoucherSigner, WireClaim};

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

/// The receiving half, as the role gate asks it (ADR 0075 decisions 3 and
/// 5): resolve the x402 channel a voucher or a challenge names, read its
/// voucher signer from what the backend admitted, and check the signature.
///
/// **Nothing is admitted, advanced or journaled.** Like
/// [`Connector::verify_peer_claim`] this is verification only: §1.5 fixes
/// role before any watermark moves. `connector-client-edge`'s claim gate is
/// the implementation, because it holds the batch-settlement backends and
/// the journaled configs an EVM voucher without one is resolved from.
#[async_trait]
pub trait VoucherEvidence: Send + Sync {
    /// A voucher (`ClientClaim::EvmVoucher` or `ClientClaim::SolanaVoucher`).
    async fn check_voucher(&self, voucher: &ClientClaim) -> VoucherCheck;

    /// A peer-role challenge. Its `expires` is the role gate's to judge,
    /// not this method's.
    async fn check_challenge(&self, challenge: &PeerRoleChallenge) -> VoucherCheck;
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
