//! The peer wire's claim types (ADR 0004, ADR 0005): what covers a PREPARE
//! this node sends a peer, and the acknowledgement a peer answers it with.
//!
//! Every claim a peering carries is an x402 **voucher** (ADR 0075, issues
//! #1378-#1380): signed through `crate::OutboundChannels` on the paying
//! side and judged by the client edge's voucher admission on the receiving
//! side, against the channel's one watermark. The `toon-channel` claim, its
//! wire encoding and the peer book that replayed an older build's journal of
//! them are deleted (issue #1385); such a journal is refused at boot, by
//! name (ADR 0075 decision 8).

/// What covers a PREPARE this node sends a peer (ADR 0042: a packet
/// carries its claim), as the [`crate::PeerTransport`] port hands it to a
/// carriage.
///
/// An x402 **voucher** (ADR 0075 decision 6), rendered -- the client edge's
/// own voucher JSON (`client-edge-spec.md` §1.3), exactly the bytes the
/// receiving half parses. A packet that moves no value carries no voucher
/// (ADR 0075 decision 5) and, where it needs the peer role, the voucher
/// claim-state **challenge** instead: a separate slot on both carriages
/// (`peer-carriage-spec.md` §1.4), because a challenge is not a claim and
/// moves nothing. There is no `toon-channel` variant: no peering covers a
/// forward with one since #1380.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Covering {
    /// An x402 `batch-settlement` voucher's JSON: the claim slot, verbatim.
    Voucher(String),
    /// A voucher claim-state challenge's JSON: the peer-role challenge
    /// slot, verbatim.
    Challenge(String),
}

/// Why a claim was rejected (peer-semantics-pre-868.md §3.4's CLAIM_ACK reasons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimRejectReason {
    SignatureInvalid,
    NonceNotAdvancing,
    AmountNotAdvancing,
    UnknownChannel,
}

impl ClaimRejectReason {
    fn to_wire(self) -> u8 {
        match self {
            ClaimRejectReason::SignatureInvalid => 0,
            ClaimRejectReason::NonceNotAdvancing => 1,
            ClaimRejectReason::AmountNotAdvancing => 2,
            ClaimRejectReason::UnknownChannel => 3,
        }
    }

    fn from_wire(byte: u8) -> Option<ClaimRejectReason> {
        match byte {
            0 => Some(ClaimRejectReason::SignatureInvalid),
            1 => Some(ClaimRejectReason::NonceNotAdvancing),
            2 => Some(ClaimRejectReason::AmountNotAdvancing),
            3 => Some(ClaimRejectReason::UnknownChannel),
            _ => None,
        }
    }
}

/// The outcome of sending a claim (peer-semantics-pre-868.md §3.4): [`ClaimAckOutcome::NotSent`]
/// when no claim rode this frame at all, distinct from a claim that rode it
/// and was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimAckOutcome {
    NotSent,
    Accepted,
    Rejected(ClaimRejectReason),
}

impl ClaimAckOutcome {
    /// Encode the CLAIM_ACK answering a claim that was sent. Never called
    /// for [`ClaimAckOutcome::NotSent`] -- there is nothing to acknowledge,
    /// so no CLAIM_ACK frame is sent at all (the caller checks this first).
    pub fn encode(&self) -> Vec<u8> {
        match self {
            ClaimAckOutcome::Accepted => vec![0],
            ClaimAckOutcome::Rejected(reason) => vec![1, reason.to_wire()],
            ClaimAckOutcome::NotSent => vec![],
        }
    }

    pub fn decode(bytes: &[u8]) -> Option<ClaimAckOutcome> {
        match bytes.first()? {
            0 => Some(ClaimAckOutcome::Accepted),
            1 => Some(ClaimAckOutcome::Rejected(ClaimRejectReason::from_wire(
                *bytes.get(1)?,
            )?)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claim_ack_round_trips_through_encode_and_decode() {
        for outcome in [
            ClaimAckOutcome::Accepted,
            ClaimAckOutcome::Rejected(ClaimRejectReason::SignatureInvalid),
            ClaimAckOutcome::Rejected(ClaimRejectReason::NonceNotAdvancing),
            ClaimAckOutcome::Rejected(ClaimRejectReason::AmountNotAdvancing),
            ClaimAckOutcome::Rejected(ClaimRejectReason::UnknownChannel),
        ] {
            let bytes = outcome.encode();
            assert_eq!(ClaimAckOutcome::decode(&bytes), Some(outcome));
        }
    }
}
