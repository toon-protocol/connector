//! The zk edge's port to its Mina vault (ADR 0080, issue #1522): the
//! [`ZkNoteVault`], its in-memory fake, and -- behind `test-util` -- the
//! contract suite every implementation must uphold.
//!
//! Three operations, all reads: verify a request proof against the
//! configured verification key, say whether a root is one the vault has
//! held, and say whether a nullifier is already used on chain. The gate asks
//! them last, after every refusal it can make for nothing (ADR 0080 decision
//! 3), and a failed read refuses that one allowance by name, never the node
//! (decision 9).
//!
//! The fake is not a mock: it holds the facts a vault owns -- its roots, the
//! nullifiers spent on chain, which proofs verify -- and asserts no call
//! sequence. It counts each operation, so a test can show that a refusal the
//! gate should make for free really asked the vault nothing. The Mina
//! adapter (#1514) runs [`contract::assert_upholds_the_contract`] against
//! committed fixtures o1js produced.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use connector_zk_note::request::RequestProof;
use connector_zk_note::{field_to_le_bytes, Field};

/// Why the vault could not answer. Refuses the one allowance that asked;
/// the node keeps serving.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the Mina vault could not be read: {0}")]
pub struct ZkNoteVaultUnavailable(pub String);

/// The zk edge's view of its Mina vault.
#[async_trait]
pub trait ZkNoteVault: Send + Sync + std::fmt::Debug {
    /// Whether `proof` verifies against the configured verification key,
    /// over the public input it carries.
    async fn verify_request_proof(
        &self,
        proof: &RequestProof,
    ) -> Result<bool, ZkNoteVaultUnavailable>;

    /// Whether `root` is a root the vault has held.
    async fn root_known(&self, root: Field) -> Result<bool, ZkNoteVaultUnavailable>;

    /// Whether `nullifier` is already used on chain.
    async fn nullifier_used(&self, nullifier: Field) -> Result<bool, ZkNoteVaultUnavailable>;
}

/// An in-memory [`ZkNoteVault`]: the roots it has held, the nullifiers
/// spent on chain, and the proofs that verify, by their `JsonProof` bytes.
#[derive(Debug, Default)]
pub struct InMemoryZkNoteVault {
    roots: Mutex<HashSet<[u8; 32]>>,
    used: Mutex<HashSet<[u8; 32]>>,
    valid: Mutex<HashSet<Vec<u8>>>,
    unavailable: AtomicBool,
    root_reads: AtomicUsize,
    nullifier_reads: AtomicUsize,
    verifications: AtomicUsize,
}

impl InMemoryZkNoteVault {
    /// A vault with no root, no spent nullifier and no valid proof.
    pub fn new() -> InMemoryZkNoteVault {
        InMemoryZkNoteVault::default()
    }

    /// The vault has held `root`.
    pub fn hold_root(&self, root: Field) {
        self.roots
            .lock()
            .expect("vault lock")
            .insert(field_to_le_bytes(root));
    }

    /// `nullifier` is spent on chain.
    pub fn spend_on_chain(&self, nullifier: Field) {
        self.used
            .lock()
            .expect("vault lock")
            .insert(field_to_le_bytes(nullifier));
    }

    /// `proof` verifies: o1js produced it over its public input, under the
    /// configured key.
    pub fn accept(&self, proof: &RequestProof) {
        self.valid
            .lock()
            .expect("vault lock")
            .insert(proof.json().to_vec());
    }

    /// Every read fails from now on, as an unreachable endpoint would.
    pub fn go_down(&self) {
        self.unavailable.store(true, Ordering::SeqCst);
    }

    /// How many times the vault has been asked anything.
    pub fn reads(&self) -> usize {
        self.root_reads() + self.nullifier_reads() + self.verifications()
    }

    /// How many root reads.
    pub fn root_reads(&self) -> usize {
        self.root_reads.load(Ordering::SeqCst)
    }

    /// How many on-chain nullifier reads.
    pub fn nullifier_reads(&self) -> usize {
        self.nullifier_reads.load(Ordering::SeqCst)
    }

    /// How many proof verifications.
    pub fn verifications(&self) -> usize {
        self.verifications.load(Ordering::SeqCst)
    }

    fn available(&self) -> Result<(), ZkNoteVaultUnavailable> {
        if self.unavailable.load(Ordering::SeqCst) {
            Err(ZkNoteVaultUnavailable("the in-memory vault is down".into()))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl ZkNoteVault for InMemoryZkNoteVault {
    async fn verify_request_proof(
        &self,
        proof: &RequestProof,
    ) -> Result<bool, ZkNoteVaultUnavailable> {
        self.verifications.fetch_add(1, Ordering::SeqCst);
        self.available()?;
        Ok(self
            .valid
            .lock()
            .expect("vault lock")
            .contains(proof.json()))
    }

    async fn root_known(&self, root: Field) -> Result<bool, ZkNoteVaultUnavailable> {
        self.root_reads.fetch_add(1, Ordering::SeqCst);
        self.available()?;
        Ok(self
            .roots
            .lock()
            .expect("vault lock")
            .contains(&field_to_le_bytes(root)))
    }

    async fn nullifier_used(&self, nullifier: Field) -> Result<bool, ZkNoteVaultUnavailable> {
        self.nullifier_reads.fetch_add(1, Ordering::SeqCst);
        self.available()?;
        Ok(self
            .used
            .lock()
            .expect("vault lock")
            .contains(&field_to_le_bytes(nullifier)))
    }
}

/// The contract every [`ZkNoteVault`] upholds, run against the fake here and
/// against the Mina adapter in #1514 (ADR 0007: a fake that upholds a
/// port's contract suite is a legitimate test subject).
#[cfg(any(test, feature = "test-util"))]
pub mod contract {
    use std::sync::Arc;

    use super::*;

    /// What a suite run needs from the vault under test: facts it holds,
    /// and facts it does not.
    pub struct ZkNoteVaultFixture {
        pub vault: Arc<dyn ZkNoteVault>,
        /// A root the vault has held.
        pub known_root: Field,
        /// A root it never held.
        pub unknown_root: Field,
        /// A nullifier spent on chain.
        pub used_nullifier: Field,
        /// A nullifier never spent.
        pub unused_nullifier: Field,
        /// A request proof that verifies under the configured key.
        pub valid_proof: RequestProof,
        /// One that does not: its proof bytes altered, or made under
        /// another key.
        pub invalid_proof: RequestProof,
    }

    /// Assert every rule of the port against `fixture`.
    ///
    /// # Panics
    /// On the first rule the implementation breaks.
    pub async fn assert_upholds_the_contract(fixture: ZkNoteVaultFixture) {
        let vault = &fixture.vault;
        for _ in 0..2 {
            assert_eq!(
                vault.root_known(fixture.known_root).await,
                Ok(true),
                "a root the vault held is known, every time it is asked"
            );
            assert_eq!(
                vault.root_known(fixture.unknown_root).await,
                Ok(false),
                "a root the vault never held is not"
            );
            assert_eq!(
                vault.nullifier_used(fixture.used_nullifier).await,
                Ok(true),
                "a nullifier spent on chain is used"
            );
            assert_eq!(
                vault.nullifier_used(fixture.unused_nullifier).await,
                Ok(false),
                "a nullifier never spent is not"
            );
            assert_eq!(
                vault.verify_request_proof(&fixture.valid_proof).await,
                Ok(true),
                "a proof made under the configured key verifies"
            );
            assert_eq!(
                vault.verify_request_proof(&fixture.invalid_proof).await,
                Ok(false),
                "a proof not made under it does not"
            );
        }
        // Verifying is a read: it spends nothing.
        assert_eq!(
            vault
                .nullifier_used(fixture.valid_proof.public().nullifier)
                .await,
            vault
                .nullifier_used(fixture.valid_proof.public().nullifier)
                .await,
            "asking twice changes nothing"
        );
        assert_eq!(
            vault.nullifier_used(fixture.unused_nullifier).await,
            Ok(false),
            "verifying a proof does not spend its nullifier"
        );
    }

    /// A fixture over the in-memory fake.
    pub fn in_memory_fixture(valid: RequestProof, invalid: RequestProof) -> ZkNoteVaultFixture {
        let vault = InMemoryZkNoteVault::new();
        let (known, unknown) = (Field::from(11u64), Field::from(12u64));
        let (used, unused) = (Field::from(21u64), Field::from(22u64));
        vault.hold_root(known);
        vault.spend_on_chain(used);
        vault.accept(&valid);
        ZkNoteVaultFixture {
            vault: Arc::new(vault),
            known_root: known,
            unknown_root: unknown,
            used_nullifier: used,
            unused_nullifier: unused,
            valid_proof: valid,
            invalid_proof: invalid,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use connector_zk_note::note::balance::generator_g;
        use connector_zk_note::request::{json_proof_for, RequestPublic};
        use connector_zk_note::CompressedPubKey;

        fn proof(bytes: &str) -> RequestProof {
            let public = RequestPublic {
                vault: CompressedPubKey {
                    x: Field::from(1u64),
                    is_odd: false,
                },
                root: Field::from(11u64),
                request_time: 1,
                tier: 10_000_000,
                nullifier: Field::from(99u64),
                tag: Field::from(3u64),
                commitment: generator_g(),
                subsidised: false,
                period: Field::from(0u64),
                period_nullifier: Field::from(0u64),
            };
            RequestProof::parse(&json_proof_for(&public, bytes)).expect("a proof")
        }

        #[tokio::test]
        async fn the_in_memory_vault_upholds_the_contract() {
            assert_upholds_the_contract(in_memory_fixture(proof("valid"), proof("forged"))).await;
        }
    }
}
