//! The ed25519 signer port (issue #742): the Solana counterpart of
//! [`crate::Signer`]. A Solana message is signed whole with an ed25519 key,
//! never recovered like a secp256k1 signature -- kept as its own trait
//! rather than folded into [`crate::Signer`], so the two chains are not
//! merged behind one abstraction (issue #732).
//!
//! Only [`LocalEd25519Signer`] exists today, holding key material directly
//! in process memory. There is no KMS-backed implementation yet -- matching
//! how a Solana peer channel has no config surface to load one from either;
//! wiring either into `connector-config`/`connector-cli` is out of this
//! issue's scope, which is signing capability, not deployment.

use ed25519_dalek::{Keypair, PublicKey, SecretKey, Signer as DalekSigner};
use rand::rngs::OsRng;

use crate::error::SignerError;

/// Sign a Solana message -- a voucher, or a claim-state challenge -- with
/// an ed25519 key. The counterpart of [`crate::Signer::sign`], over a
/// message rather than a digest: the message is already the full bytes
/// ed25519 signs, with no separate hashing step the way an EIP-712 digest
/// has one.
pub trait Ed25519Signer: Send + Sync {
    /// The raw 32-byte public key of the currently active key.
    fn public_key(&self) -> [u8; 32];

    /// Sign `message` with the currently active key.
    ///
    /// A slice, because each message layout this key signs opens with its
    /// own domain tag and length, so none can be mistaken for another.
    fn sign(&self, message: &[u8]) -> [u8; 64];
}

/// A [`Ed25519Signer`] that holds an ed25519 key pair directly in process
/// memory -- the Solana counterpart of [`crate::LocalSigner`].
pub struct LocalEd25519Signer {
    keypair: Keypair,
}

impl LocalEd25519Signer {
    /// Generate a fresh key pair.
    #[must_use]
    pub fn generate() -> Self {
        let mut rng = OsRng;
        LocalEd25519Signer {
            keypair: Keypair::generate(&mut rng),
        }
    }

    /// Load an existing 32-byte seed rather than generating one.
    pub fn from_secret_bytes(seed: [u8; 32]) -> Result<Self, SignerError> {
        let secret = SecretKey::from_bytes(&seed).map_err(|_| SignerError::InvalidKey)?;
        let public = PublicKey::from(&secret);
        Ok(LocalEd25519Signer {
            keypair: Keypair { secret, public },
        })
    }
}

impl Ed25519Signer for LocalEd25519Signer {
    fn public_key(&self) -> [u8; 32] {
        self.keypair.public.to_bytes()
    }

    fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.keypair.sign(message).to_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_signers_have_distinct_keys() {
        let a = LocalEd25519Signer::generate();
        let b = LocalEd25519Signer::generate();
        assert_ne!(a.public_key(), b.public_key());
    }

    #[test]
    fn from_secret_bytes_is_deterministic() {
        let a = LocalEd25519Signer::from_secret_bytes([7u8; 32]).unwrap();
        let b = LocalEd25519Signer::from_secret_bytes([7u8; 32]).unwrap();
        assert_eq!(a.public_key(), b.public_key());
    }

    #[test]
    fn a_signed_message_verifies_against_its_signers_own_public_key_only() {
        use ed25519_dalek::Verifier;
        let signer = LocalEd25519Signer::from_secret_bytes([3u8; 32]).unwrap();
        let other = LocalEd25519Signer::from_secret_bytes([4u8; 32]).unwrap();
        let message = b"a message";
        let signature = ed25519_dalek::Signature::from_bytes(&signer.sign(message)).unwrap();

        let own = PublicKey::from_bytes(&signer.public_key()).unwrap();
        assert!(own.verify(message, &signature).is_ok());
        let theirs = PublicKey::from_bytes(&other.public_key()).unwrap();
        assert!(theirs.verify(message, &signature).is_err());
    }
}
