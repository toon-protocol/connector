//! The one transaction this node signs as a channel's destination: a
//! `PaymentChannelClaim` (ADR 0087 decisions 1 and 9).
//!
//! A node holds a key and the ledger takes a **blob**, so the transaction is
//! serialised here, in the ledger's canonical binary form, rather than handed
//! to a node's `sign` (which a production node does not offer). Only the
//! fields this transaction carries are encoded, in the canonical order
//! (type code, then field code).

use connector_signer::{sha512_half, XrplSigner};

/// `"STX\0"`: what a transaction's signature is over, before the fields.
const SIGNING_PREFIX: [u8; 4] = [0x53, 0x54, 0x58, 0x00];
/// `"TXN\0"`: what a transaction's id is the hash of, before the blob.
const ID_PREFIX: [u8; 4] = [0x54, 0x58, 0x4E, 0x00];
/// `PaymentChannelClaim`'s transaction type.
const PAYMENT_CHANNEL_CLAIM: u16 = 15;
/// `tfFullyCanonicalSig`: universal, and what every signature here is.
const FULLY_CANONICAL_SIG: u32 = 0x8000_0000;

/// A destination's `PaymentChannelClaim` that lands a voucher: `Balance` and
/// `Amount` both name the voucher's cumulative amount, and the claim
/// signature and channel key are the voucher's. It never sets `tfClose`
/// (ADR 0087 decision 8: the ordinary land leaves the channel open) or
/// `tfRenew` (only a channel's source may renew).
pub struct LandClaim {
    /// This node's settlement account.
    pub account: [u8; 20],
    pub channel: [u8; 32],
    /// The voucher's cumulative amount, in drops.
    pub drops: u64,
    /// The voucher's signature, as the ledger verifies it.
    pub claim_signature: Vec<u8>,
    /// The channel's `PublicKey`.
    pub channel_key: [u8; 33],
    pub fee_drops: u64,
    pub sequence: u32,
    pub last_ledger_sequence: u32,
}

/// A transaction signed and ready to submit.
pub struct SignedTx {
    /// The blob `submit` takes, uppercase hex.
    pub blob_hex: String,
    /// The transaction's id, uppercase hex: known before it is submitted, so
    /// a lost answer is resolved by looking the hash up.
    pub hash: String,
}

impl LandClaim {
    /// Sign with the account's key (`SHA512Half` and DER for secp256k1, the
    /// raw message for Ed25519, over `"STX\0"` and the fields).
    pub fn sign(&self, signer: &XrplSigner) -> SignedTx {
        let signing_key = signer.public_key();
        let mut message = SIGNING_PREFIX.to_vec();
        message.extend(self.fields(&signing_key, None));
        let signature = signer.sign(&message);
        let blob = self.fields(&signing_key, Some(&signature));
        let mut id_input = ID_PREFIX.to_vec();
        id_input.extend(&blob);
        SignedTx {
            blob_hex: hex::encode_upper(&blob),
            hash: hex::encode_upper(sha512_half(&id_input)),
        }
    }

    /// The canonical serialisation. `signature` is `Some` for the blob that
    /// is submitted, which carries `TxnSignature` and the voucher's own
    /// `Signature`; it is `None` for what is signed, which carries neither:
    /// both are `notSigning` fields in the ledger's definitions.
    fn fields(&self, signing_key: &[u8; 33], signature: Option<&[u8]>) -> Vec<u8> {
        let mut out = Vec::with_capacity(320);
        // UInt16 TransactionType (1, 2)
        out.push(0x12);
        out.extend(PAYMENT_CHANNEL_CLAIM.to_be_bytes());
        // UInt32 Flags (2, 2), Sequence (2, 4), LastLedgerSequence (2, 27)
        out.push(0x22);
        out.extend(FULLY_CANONICAL_SIG.to_be_bytes());
        out.push(0x24);
        out.extend(self.sequence.to_be_bytes());
        out.extend([0x20, 0x1B]);
        out.extend(self.last_ledger_sequence.to_be_bytes());
        // Hash256 Channel (5, 22)
        out.extend([0x50, 0x16]);
        out.extend(self.channel);
        // Amount Amount (6, 1), Balance (6, 2), Fee (6, 8)
        out.push(0x61);
        out.extend(xrp_amount(self.drops));
        out.push(0x62);
        out.extend(xrp_amount(self.drops));
        out.push(0x68);
        out.extend(xrp_amount(self.fee_drops));
        // Blob PublicKey (7, 1), SigningPubKey (7, 3), TxnSignature (7, 4),
        // Signature (7, 6)
        out.push(0x71);
        blob(&mut out, &self.channel_key);
        out.push(0x73);
        blob(&mut out, signing_key);
        if let Some(signature) = signature {
            out.push(0x74);
            blob(&mut out, signature);
            out.push(0x76);
            blob(&mut out, &self.claim_signature);
        }
        // AccountID Account (8, 1)
        out.push(0x81);
        blob(&mut out, &self.account);
        out
    }
}

/// An XRP amount: 64 bits, the top bit clear for "not an issued currency",
/// the next set for "positive", and the drops below.
fn xrp_amount(drops: u64) -> [u8; 8] {
    debug_assert!(drops < 1 << 62, "XRP's supply is 10^17 drops");
    (drops | 0x4000_0000_0000_0000).to_be_bytes()
}

/// A length-prefixed blob. Every blob here is at most 72 bytes, inside the
/// single-byte prefix's 192.
fn blob(out: &mut Vec<u8>, bytes: &[u8]) {
    assert!(bytes.len() <= 192, "a blob here is a key or a signature");
    out.push(bytes.len() as u8);
    out.extend(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim() -> LandClaim {
        LandClaim {
            account: [0x11; 20],
            channel: [0x22; 32],
            drops: 1_000,
            claim_signature: vec![0x33; 70],
            channel_key: [0x02; 33],
            fee_drops: 10,
            sequence: 7,
            last_ledger_sequence: 99,
        }
    }

    #[test]
    fn fields_are_in_canonical_order_and_the_signature_is_not_signed() {
        let claim = claim();
        let unsigned = claim.fields(&[0x44; 33], None);
        let signed = claim.fields(&[0x44; 33], Some(&[0x55; 71]));
        // Type, flags, sequence, last ledger, channel, then the amounts.
        assert_eq!(&unsigned[..3], &[0x12, 0x00, 0x0F]);
        assert_eq!(&unsigned[3..8], &[0x22, 0x80, 0x00, 0x00, 0x00]);
        assert_eq!(&unsigned[8..13], &[0x24, 0, 0, 0, 7]);
        assert_eq!(&unsigned[13..19], &[0x20, 0x1B, 0, 0, 0, 99]);
        assert_eq!(&unsigned[19..21], &[0x50, 0x16]);
        assert_eq!(unsigned[53], 0x61);
        // The amount: positive XRP, 1000 drops.
        assert_eq!(&unsigned[54..62], &[0x40, 0, 0, 0, 0, 0, 0x03, 0xE8]);
        // The blob adds `TxnSignature` and the voucher's `Signature`; what is
        // signed carries neither.
        assert_eq!(signed.len(), unsigned.len() + 2 + 71 + 2 + 70);
        assert!(signed.windows(2).any(|w| w == [0x74, 71]));
        assert!(!unsigned.windows(2).any(|w| w == [0x74, 71]));
    }

    #[test]
    fn a_signed_claim_names_its_own_hash() {
        let signer = XrplSigner::from_family_seed("snoPBrXtMeMyMHUVTgbuqAfg1SUTb").expect("seed");
        let mut claim = claim();
        claim.account = signer.account_id();
        let signed = claim.sign(&signer);
        assert_eq!(signed.hash.len(), 64);
        let blob = hex::decode(&signed.blob_hex).expect("hex");
        let mut input = ID_PREFIX.to_vec();
        input.extend(blob);
        assert_eq!(signed.hash, hex::encode_upper(sha512_half(&input)));
    }
}
