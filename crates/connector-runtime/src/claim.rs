//! The peer wire's claim types (ADR 0004, ADR 0005, ADR 0024), and what is
//! left of the peer book.
//!
//! Every claim a peering carries is an x402 **voucher** now (ADR 0075,
//! issues #1378-#1380): signed through `crate::OutboundChannels` on the
//! paying side and judged by the client edge's voucher admission on the
//! receiving side, against the channel's one watermark. What a peering used
//! to prove itself with -- a `toon-channel` claim on a `[[peer_channels]]`
//! channel, verified here against the row's counterparty key and judged
//! against this book's nonce watermark -- is deleted with its last caller
//! (#1380). Signing one was deleted before it (#1381).
//!
//! [`ClaimBook`] remains only as the **replay** of that history: a journal
//! an older build wrote still holds the `toon-channel` claims its peers
//! paid it with, and `GET /claims` still reports where each channel's
//! watermark stood. Nothing advances it. ADR 0075 decision 8 is what
//! eventually refuses such a journal at boot, by name, with the drain
//! procedure; until then it is read, never skipped.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use connector_domain::JournalEntry;
use connector_signer::Signature;

use crate::journal::{Journal, JournalError};
use crate::operator_view::ClaimView;

/// A `toon-channel` claim as it travels the peer wire
/// (peer-semantics-pre-868.md §3.5): a channel identifier, a nonce, a
/// cumulative amount, and a signature.
///
/// **No peering sends or judges one any more** (ADR 0075, issue #1380):
/// every peering pays with vouchers, and a `toon-channel` claim presented on
/// a peer carriage is refused by name (#1384). The type survives only for
/// `SettlementBackend`'s redemption port, deleted in #1385. Distinct from
/// `connector_settlement::Claim`, the on-chain redemption claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireClaim {
    pub channel_id: String,
    pub nonce: u64,
    pub cumulative_amount: u64,
    pub signature: ClaimSignature,
}

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

const EVM_SIGNATURE_LEN: usize = 65; // r(32) + s(32) + recovery_id(1)
const SOLANA_SIGNATURE_LEN: usize = 64; // ed25519 R(32) + S(32)

/// The scheme discriminator [`WireClaim::encode`] writes ahead of a
/// signature. Present so the in-process binary form stays decodable now
/// that a signature has two lengths (issue #732); neither carriage puts
/// these bytes on a wire (`connector_peer_btp::claim_json`'s own module
/// doc), so no deployed peer ever parses them.
const EVM_SCHEME: u8 = 0;
const SOLANA_SCHEME: u8 = 1;

impl WireClaim {
    /// Length-prefixed `channel_id` (so no two distinct tuples can ever
    /// collide on the same byte string) followed by `nonce`,
    /// `cumulative_amount`, a signature-scheme byte and the raw signature
    /// -- the ad hoc encoding for fields RFC-0027 has no concept of. ADR
    /// 0027 re-hosts this same byte string as a `payment-channel-claim`
    /// BTP protocolData entry or a `Payment-Channel-Claim` HTTP header;
    /// only the carriage moves.
    pub fn encode(&self) -> Vec<u8> {
        let channel_id_bytes = self.channel_id.as_bytes();
        let mut out =
            Vec::with_capacity(2 + channel_id_bytes.len() + 8 + 8 + 1 + EVM_SIGNATURE_LEN);
        out.extend_from_slice(&(channel_id_bytes.len() as u16).to_be_bytes());
        out.extend_from_slice(channel_id_bytes);
        out.extend_from_slice(&self.nonce.to_be_bytes());
        out.extend_from_slice(&self.cumulative_amount.to_be_bytes());
        match &self.signature {
            ClaimSignature::Evm(signature) => {
                out.push(EVM_SCHEME);
                out.extend_from_slice(&signature.r);
                out.extend_from_slice(&signature.s);
                out.push(signature.recovery_id);
            }
            ClaimSignature::Solana(signature) => {
                out.push(SOLANA_SCHEME);
                out.extend_from_slice(signature);
            }
        }
        out
    }

    /// Decode one [`WireClaim`] from the front of `bytes`, returning it
    /// alongside how many bytes it consumed so a caller can decode
    /// whatever follows (a `WireClaim` never appears alone on the wire --
    /// it always rides a PREPARE or stands as the whole of a FLUSH).
    pub fn decode(bytes: &[u8]) -> Option<(WireClaim, usize)> {
        let channel_id_len = u16::from_be_bytes(bytes.get(0..2)?.try_into().ok()?) as usize;
        let mut offset = 2;
        let channel_id =
            String::from_utf8(bytes.get(offset..offset + channel_id_len)?.to_vec()).ok()?;
        offset += channel_id_len;
        let nonce = u64::from_be_bytes(bytes.get(offset..offset + 8)?.try_into().ok()?);
        offset += 8;
        let cumulative_amount = u64::from_be_bytes(bytes.get(offset..offset + 8)?.try_into().ok()?);
        offset += 8;
        let scheme = *bytes.get(offset)?;
        offset += 1;
        let signature = match scheme {
            EVM_SCHEME => {
                let raw: [u8; EVM_SIGNATURE_LEN] = bytes
                    .get(offset..offset + EVM_SIGNATURE_LEN)?
                    .try_into()
                    .ok()?;
                offset += EVM_SIGNATURE_LEN;
                ClaimSignature::Evm(Signature::from_bytes(&raw)?)
            }
            SOLANA_SCHEME => {
                let raw: [u8; SOLANA_SIGNATURE_LEN] = bytes
                    .get(offset..offset + SOLANA_SIGNATURE_LEN)?
                    .try_into()
                    .ok()?;
                offset += SOLANA_SIGNATURE_LEN;
                ClaimSignature::Solana(raw)
            }
            _ => return None,
        };
        Some((
            WireClaim {
                channel_id,
                nonce,
                cumulative_amount,
                signature,
            },
            offset,
        ))
    }
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

/// A peer claim's signature, discriminated by the scheme its chain
/// actually uses (issue #732). The two are different lengths over
/// different messages verified by different primitives, and the peer semantics
/// keeps them apart for the whole of their travel rather than flattening
/// both into one opaque byte string -- a 64-byte ed25519 signature stuffed
/// into a 65-byte `r ‖ s ‖ v` slot is a claim this connector could no
/// longer tell you how to check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimSignature {
    /// secp256k1 `r ‖ s ‖ v` over ADR 0024's EIP-712 `BalanceProof`
    /// digest.
    Evm(Signature),
    /// ed25519 over
    /// `connector_signer::solana_balance_proof_message`'s 96 bytes.
    Solana([u8; 64]),
}

impl ClaimSignature {
    /// The signature's own bytes, in the length its scheme defines -- 65
    /// for EVM, 64 for Solana. This is what a journal entry records and
    /// what a settlement backend is later handed; nothing pads one to the
    /// other's width.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            ClaimSignature::Evm(signature) => signature.to_bytes().to_vec(),
            ClaimSignature::Solana(signature) => signature.to_vec(),
        }
    }
}

impl From<Signature> for ClaimSignature {
    fn from(signature: Signature) -> ClaimSignature {
        ClaimSignature::Evm(signature)
    }
}

/// Where a `toon-channel` channel's watermark stood when an older build's
/// journal last recorded it: its nonce and cumulative amount, as history.
/// Not `connector_domain`'s voucher watermark, and nothing is judged against
/// it -- the nonce rules are deleted (ADR 0075 decision 8, issue #1384).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReplayedWatermark {
    nonce: u64,
    cumulative_amount: u64,
}

/// The replay of a peer claim journal an older build wrote (ADR 0005):
/// each channel's `toon-channel` watermark as the journal last recorded it,
/// for `GET /claims`. Nothing appends to it and nothing judges a claim
/// against it (see the module doc).
pub struct ClaimBook {
    /// `channel_id` -> the highest nonce/amount the journal recorded on it.
    inbound_watermarks: Arc<RwLock<HashMap<String, ReplayedWatermark>>>,
}

impl Default for ClaimBook {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaimBook {
    #[must_use]
    pub fn new() -> ClaimBook {
        ClaimBook {
            inbound_watermarks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Replay `journal` into this book's watermarks. Takes `&mut self`:
    /// called only while a `Connector` is still being built.
    ///
    /// A journal written before ADR 0075 (issue #1381) may still carry
    /// `OutboundClaimSigned` entries from the retired payout ledger; they
    /// replay as nothing.
    pub fn set_journal(&mut self, journal: Arc<dyn Journal>) -> Result<(), JournalError> {
        let entries = journal.read_all()?;
        self.inbound_watermarks = Arc::new(RwLock::new(Self::rebuild_from(&entries)));
        Ok(())
    }

    fn rebuild_from(entries: &[JournalEntry]) -> HashMap<String, ReplayedWatermark> {
        let mut inbound_watermarks: HashMap<String, ReplayedWatermark> = HashMap::new();
        for entry in entries {
            if let JournalEntry::InboundClaimAccepted {
                channel_id,
                nonce,
                cumulative_amount,
                ..
            } = entry
            {
                inbound_watermarks.insert(
                    channel_id.clone(),
                    ReplayedWatermark {
                        nonce: *nonce,
                        cumulative_amount: *cumulative_amount,
                    },
                );
            }
        }
        inbound_watermarks
    }

    /// Every channel's replayed watermark, for the operator surface's
    /// read-only inspection interface (issue #420). `peer_id` is `None`:
    /// the journal recorded only the channel.
    pub fn views(&self) -> Vec<ClaimView> {
        self.inbound_watermarks
            .read()
            .expect("inbound watermarks lock poisoned")
            .iter()
            .map(|(channel_id, watermark)| ClaimView {
                peer_id: None,
                channel_id: channel_id.clone(),
                direction: crate::operator_view::ClaimDirection::Inbound,
                nonce: watermark.nonce,
                cumulative_amount: watermark.cumulative_amount,
                pending: false,
                book: crate::operator_view::ClaimBookKind::Peer,
                scheme: crate::operator_view::ClaimScheme::ToonChannel,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileJournal;

    #[test]
    fn a_wire_claim_round_trips_through_encode_and_decode() {
        let claim = WireClaim {
            channel_id: format!("0x{:064x}", 1),
            nonce: 7,
            cumulative_amount: 900,
            signature: ClaimSignature::Evm(Signature {
                r: [1u8; 32],
                s: [2u8; 32],
                recovery_id: 1,
            }),
        };
        let mut bytes = claim.encode();
        bytes.extend_from_slice(b"trailing");

        let (decoded, consumed) = WireClaim::decode(&bytes).unwrap();
        assert_eq!(decoded, claim);
        assert_eq!(&bytes[consumed..], b"trailing");
    }

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

    /// A peer claim journal an older build wrote is replayed rather than
    /// skipped (ADR 0075 decision 8: a skipped entry is a claim somebody
    /// could still redeem that this node has forgotten it accepted), and
    /// `GET /claims` still shows where each channel stood.
    #[test]
    fn a_journal_an_older_build_wrote_is_replayed_into_the_view() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.log");
        let journal = FileJournal::open(&path).unwrap();
        journal
            .append(&JournalEntry::OutboundClaimSigned {
                peer_id: "peer-b".to_string(),
                channel_id: "0x01".to_string(),
                nonce: 2,
                cumulative_amount: 150,
            })
            .unwrap();
        for (nonce, amount) in [(1, 40), (2, 90)] {
            journal
                .append(&JournalEntry::InboundClaimAccepted {
                    channel_id: "0x02".to_string(),
                    nonce,
                    cumulative_amount: amount,
                    signature: vec![0; 65],
                })
                .unwrap();
        }

        let mut book = ClaimBook::new();
        book.set_journal(Arc::new(FileJournal::open(&path).unwrap()))
            .unwrap();

        let views = book.views();
        assert_eq!(views.len(), 1, "an outbound entry replays as nothing");
        assert_eq!(views[0].channel_id, "0x02");
        assert_eq!(views[0].cumulative_amount, 90);
        assert_eq!(views[0].nonce, 2);
    }
}
