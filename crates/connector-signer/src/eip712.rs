//! The EIP-712 primitives the voucher and claim-state-challenge digests are
//! built from (`crate::voucher_signature`, `crate::claim_state_challenge`):
//! Keccak-256, the ABI word encodings, and recovering the EVM address that
//! signed a digest.
//!
//! TOON's own claim -- the EIP-712 `BalanceProof` over `TokenNetwork`
//! (ADR 0024) and the 96-byte `TOON-BALPROOF-V2` Solana message (ADR 0053)
//! -- and its verification are deleted with TOON's channels (ADR 0075,
//! issue #1385). Every claim is an x402 voucher.
//!
//! Recovery fails closed: malformed or truncated signature bytes recover no
//! address, never a panic.

use libsecp256k1::{Message, RecoveryId, Signature as RawSignature};
use sha3::{Digest, Keccak256};

use crate::address::derive_evm_address;
use crate::signer::PublicKeyBytes;
use crate::Address;

pub(crate) fn keccak256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// A `uint64` as its big-endian ABI word.
pub(crate) fn word_u64_be(value: u64) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[24..].copy_from_slice(&value.to_be_bytes());
    word
}

/// An `address` as its left-padded ABI word.
pub(crate) fn word_address(address: &Address) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(address);
    word
}

/// The EVM address whose key produced `signature` (65 bytes, `r || s || v`,
/// `v` in either `{0, 1}` or `{27, 28}`) over `digest`, or `None` for bytes
/// that are not such a signature.
pub(crate) fn recover_evm_signer(digest: &[u8; 32], signature: &[u8]) -> Option<Address> {
    if signature.len() != 65 {
        return None;
    }
    let mut rs = [0u8; 64];
    rs.copy_from_slice(&signature[..64]);
    let raw_signature = RawSignature::parse_standard(&rs).ok()?;
    let v = signature[64];
    let recovery_byte = if v >= 27 { v - 27 } else { v };
    let recovery_id = RecoveryId::parse(recovery_byte).ok()?;
    let message = Message::parse(digest);
    let recovered_key = libsecp256k1::recover(&message, &raw_signature, &recovery_id).ok()?;
    let public_key_bytes: PublicKeyBytes = recovered_key.serialize();
    Some(derive_evm_address(&public_key_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use libsecp256k1::{PublicKey, SecretKey};
    use rand::rngs::OsRng;

    fn signed(digest: &[u8; 32]) -> (Vec<u8>, Address) {
        let secret = SecretKey::random(&mut OsRng);
        let address = derive_evm_address(&PublicKey::from_secret_key(&secret).serialize());
        let (signature, recovery_id) = libsecp256k1::sign(&Message::parse(digest), &secret);
        let mut bytes = signature.serialize().to_vec();
        let recovery_byte: u8 = recovery_id.into();
        bytes.push(recovery_byte + 27);
        (bytes, address)
    }

    #[test]
    fn a_signature_recovers_to_its_signers_address_in_either_v_convention() {
        let digest = keccak256(b"a digest");
        let (mut signature, address) = signed(&digest);
        assert_eq!(recover_evm_signer(&digest, &signature), Some(address));
        signature[64] -= 27;
        assert_eq!(recover_evm_signer(&digest, &signature), Some(address));
    }

    #[test]
    fn malformed_signature_bytes_recover_nothing_rather_than_panicking() {
        let digest = keccak256(b"a digest");
        let (signature, _) = signed(&digest);
        assert_eq!(recover_evm_signer(&digest, &[]), None);
        assert_eq!(recover_evm_signer(&digest, &signature[..64]), None);
        let mut bad_v = signature.clone();
        bad_v[64] = 99;
        assert_eq!(recover_evm_signer(&digest, &bad_v), None);
    }

    #[test]
    fn abi_words_are_right_aligned() {
        assert_eq!(word_u64_be(1)[31], 1);
        assert_eq!(word_u64_be(1)[..31], [0u8; 31]);
        assert_eq!(word_address(&[0xab; 20])[..12], [0u8; 12]);
        assert_eq!(word_address(&[0xab; 20])[12..], [0xab; 20]);
    }
}
