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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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

    /// When the vault was created, in ms since the epoch: the origin of
    /// zk_toon's subsidy periods (ADR 0080 decision 14). Read once, at boot,
    /// and only by a node with a non-zero `subsidy_budget`.
    async fn creation_time_ms(&self) -> Result<u64, ZkNoteVaultUnavailable>;
}

/// An in-memory [`ZkNoteVault`]: the roots it has held, the nullifiers
/// spent on chain, and the proofs that verify, by their `JsonProof` bytes.
#[derive(Debug, Default)]
pub struct InMemoryZkNoteVault {
    roots: Mutex<HashSet<[u8; 32]>>,
    used: Mutex<HashSet<[u8; 32]>>,
    valid: Mutex<HashSet<Vec<u8>>>,
    created_at_ms: AtomicU64,
    unavailable: AtomicBool,
    creation_reads: AtomicUsize,
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

    /// The vault was created at `created_at_ms` (ms since the epoch).
    pub fn created_at(&self, created_at_ms: u64) {
        self.created_at_ms.store(created_at_ms, Ordering::SeqCst);
    }

    /// How many times the vault's creation time has been read.
    pub fn creation_reads(&self) -> usize {
        self.creation_reads.load(Ordering::SeqCst)
    }

    /// Every read fails from now on, as an unreachable endpoint would.
    pub fn go_down(&self) {
        self.unavailable.store(true, Ordering::SeqCst);
    }

    /// How many times the vault has been asked anything.
    pub fn reads(&self) -> usize {
        self.root_reads() + self.nullifier_reads() + self.verifications() + self.creation_reads()
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

    async fn creation_time_ms(&self) -> Result<u64, ZkNoteVaultUnavailable> {
        self.creation_reads.fetch_add(1, Ordering::SeqCst);
        self.available()?;
        Ok(self.created_at_ms.load(Ordering::SeqCst))
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
        /// A subsidised request proof that verifies under the configured key.
        pub valid_subsidised_proof: RequestProof,
        /// Proofs made from `valid_subsidised_proof` by altering what it says
        /// about its subsidy -- its flag, its period, its per-period
        /// nullifier -- and nothing else. None verifies.
        pub subsidy_altered_proofs: Vec<RequestProof>,
        /// When the vault was created, ms since the epoch.
        pub creation_time_ms: u64,
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
        // The subsidised flag, the period and the per-period nullifier are
        // public inputs the proof is verified over: read as the proof states
        // them, and a proof whose claim about its subsidy was altered is not
        // the proof that was made.
        let subsidised = fixture.valid_subsidised_proof.public();
        assert!(
            subsidised.subsidised,
            "the fixture's subsidised proof states that it is"
        );
        assert_eq!(
            vault
                .verify_request_proof(&fixture.valid_subsidised_proof)
                .await,
            Ok(true),
            "a subsidised proof made under the configured key verifies"
        );
        assert!(
            !fixture.subsidy_altered_proofs.is_empty(),
            "a fixture must alter the subsidy at least once"
        );
        for altered in &fixture.subsidy_altered_proofs {
            let (was, now) = (subsidised, altered.public());
            assert!(
                was.subsidised != now.subsidised
                    || was.period != now.period
                    || was.period_nullifier != now.period_nullifier,
                "an altered proof alters the subsidy"
            );
            assert_eq!(
                vault.verify_request_proof(altered).await,
                Ok(false),
                "a proof whose subsidised flag, period or per-period nullifier was altered \
                 does not verify"
            );
        }
        assert_eq!(
            vault.creation_time_ms().await,
            Ok(fixture.creation_time_ms),
            "the vault says when it was created"
        );
        assert_eq!(
            vault.creation_time_ms().await,
            Ok(fixture.creation_time_ms),
            "and says the same every time"
        );
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
    pub fn in_memory_fixture(
        valid: RequestProof,
        invalid: RequestProof,
        valid_subsidised: RequestProof,
        subsidy_altered: Vec<RequestProof>,
    ) -> ZkNoteVaultFixture {
        const CREATED_AT_MS: u64 = 1_700_000_000_000;
        let vault = InMemoryZkNoteVault::new();
        vault.created_at(CREATED_AT_MS);
        vault.accept(&valid_subsidised);
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
            valid_subsidised_proof: valid_subsidised,
            subsidy_altered_proofs: subsidy_altered,
            creation_time_ms: CREATED_AT_MS,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use connector_zk_note::note::balance::generator_g;
        use connector_zk_note::request::{json_proof_for, Point, RequestPublic};

        fn proof(bytes: &str) -> RequestProof {
            subsidy_proof(bytes, false, 0, 0)
        }

        fn subsidy_proof(
            bytes: &str,
            subsidised: bool,
            period: u64,
            period_nullifier: u64,
        ) -> RequestProof {
            let public = RequestPublic {
                vault_x: Field::from(1u64),
                vault_is_odd: false,
                root: Field::from(11u64),
                request_time: 1,
                tier: 10_000_000,
                nullifier: Field::from(99u64),
                tag: Field::from(3u64),
                commitment: Point {
                    x: generator_g().x,
                    y: generator_g().y,
                },
                subsidised,
                period,
                period_nullifier: Field::from(period_nullifier),
            };
            RequestProof::parse(&json_proof_for(&public, bytes)).expect("a proof")
        }

        #[tokio::test]
        async fn the_in_memory_vault_upholds_the_contract() {
            assert_upholds_the_contract(in_memory_fixture(
                proof("valid"),
                proof("forged"),
                subsidy_proof("subsidised", true, 3, 77),
                vec![
                    subsidy_proof("subsidised", false, 3, 77),
                    subsidy_proof("subsidised", true, 4, 77),
                    subsidy_proof("subsidised", true, 3, 78),
                ],
            ))
            .await;
        }
    }
}
