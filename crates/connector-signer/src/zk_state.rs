//! The zk edge's state key (ADR 0080 decisions 2, 4 and 12): the Mina key a
//! zk edge signs each closed allowance's successor state with, read from
//! `[zk_note] state_key_file`.
//!
//! [`ZkStateSigner`] is the only thing in the workspace that reads, holds or
//! uses it. It exposes the public half -- what the greeting publishes and
//! every request proof must be about -- and one operation, signing a
//! zk_toon state message. The successor is composed by
//! `connector_zk_note::successor`, and its signature is zk_toon's
//! `sign_state` (chunked nonce, devnet domain); nothing here derives either.

use std::fmt;
use std::path::{Path, PathBuf};

use connector_zk_note::note::state::{sign_state, StateMessage};
use connector_zk_note::{CompressedPubKey, Keypair, SecKey, Signature};

/// Why a state key file could not be read as a state key.
#[derive(Debug, thiserror::Error)]
pub enum ZkStateKeyError {
    #[error("the zk-note state key file {} could not be read: {source}", path.display())]
    Unreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the zk-note state key file {} is not a base58 Mina secret key", path.display())]
    NotAMinaKey { path: PathBuf },
}

/// A zk edge's state key, able only to say its public half and to sign a
/// state message.
pub struct ZkStateSigner {
    keypair: Keypair,
}

impl ZkStateSigner {
    /// Read the state key from `path`: a Mina secret key in base58 (`EK…`),
    /// as Mina's tooling and o1js's `PrivateKey.toBase58` export it, with
    /// surrounding whitespace ignored.
    ///
    /// # Errors
    /// [`ZkStateKeyError`] for a file that cannot be read or holds no Mina
    /// secret key.
    pub fn from_key_file(path: &Path) -> Result<ZkStateSigner, ZkStateKeyError> {
        let text = std::fs::read_to_string(path).map_err(|source| ZkStateKeyError::Unreadable {
            path: path.to_path_buf(),
            source,
        })?;
        ZkStateSigner::from_base58(&text).ok_or_else(|| ZkStateKeyError::NotAMinaKey {
            path: path.to_path_buf(),
        })
    }

    /// The state key a key file's text holds, `None` for anything that is
    /// not a base58 Mina secret key. [`Self::from_key_file`] without the
    /// file, for a test or a fixture that already has the text.
    #[must_use]
    pub fn from_base58(text: &str) -> Option<ZkStateSigner> {
        let secret = SecKey::from_base58(text.trim()).ok()?;
        Keypair::try_from(secret)
            .ok()
            .map(|keypair| ZkStateSigner { keypair })
    }

    /// The public half: what the greeting publishes as the zk edge's state
    /// key, and whose x is the `edge` every binding tag is computed with.
    #[must_use]
    pub fn public_key(&self) -> CompressedPubKey {
        self.keypair.public.into_compressed()
    }

    /// Sign a state message: zk_toon's `sign_state`.
    #[must_use]
    pub fn sign(&self, message: &StateMessage) -> Signature {
        sign_state(&self.keypair, message)
    }
}

impl fmt::Debug for ZkStateSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZkStateSigner")
            .field("public_key", &self.public_key().into_address())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_zk_note::note::balance::commit;
    use connector_zk_note::note::state::verify_state;
    use connector_zk_note::{Field, Scalar};

    fn secret(seed: u64) -> SecKey {
        SecKey::new(Scalar::from(seed))
    }

    #[test]
    fn a_state_key_file_is_a_base58_mina_secret_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zk-state.key");
        std::fs::write(&path, format!("  {}\n", secret(42).to_base58())).unwrap();
        let signer = ZkStateSigner::from_key_file(&path).expect("a state key");
        let expected = Keypair::try_from(secret(42)).unwrap().public;
        assert_eq!(signer.public_key(), expected.into_compressed());

        std::fs::write(&path, "not a key").unwrap();
        assert!(matches!(
            ZkStateSigner::from_key_file(&path),
            Err(ZkStateKeyError::NotAMinaKey { .. })
        ));
        assert!(matches!(
            ZkStateSigner::from_key_file(&dir.path().join("missing")),
            Err(ZkStateKeyError::Unreadable { .. })
        ));
        assert!(ZkStateSigner::from_base58("").is_none());
    }

    #[test]
    fn a_signed_state_verifies_under_the_public_key_and_no_other() {
        let signer = ZkStateSigner::from_base58(&secret(7).to_base58()).unwrap();
        let message = StateMessage::new(
            &signer.public_key(),
            commit(Scalar::from(10u64), Scalar::from(3u64)),
            Field::from(5u64),
        );
        let signature = signer.sign(&message);
        let public = Keypair::try_from(secret(7)).unwrap().public;
        assert!(verify_state(&signature, &public, &message));
        let other = Keypair::try_from(secret(8)).unwrap().public;
        assert!(!verify_state(&signature, &other, &message));
    }

    #[test]
    fn debug_shows_the_public_half_only() {
        let key = secret(11);
        let signer = ZkStateSigner::from_base58(&key.to_base58()).unwrap();
        let shown = format!("{signer:?}");
        assert!(
            shown.contains(&signer.public_key().into_address()),
            "{shown}"
        );
        assert!(!shown.contains(&key.to_base58()), "{shown}");
    }
}
