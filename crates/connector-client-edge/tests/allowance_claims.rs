//! A zk edge's allowances and allowance claims, at the `ClientClaimGate`
//! seam (ADR 0080, issue #1522), over the in-memory vault and a real journal
//! and evidence file.
//!
//! The vault is a fake, not a mock: it holds a vault's facts and asserts no
//! call sequence. It counts its reads, so a test can show that a refusal the
//! gate should make for nothing really asked the vault nothing.

use std::sync::Arc;

use chrono::{Duration as TimeDelta, TimeZone, Utc};
use connector_client_edge::zk_note::{InMemoryZkNoteVault, ZkNoteVault};
use connector_client_edge::{
    AllowanceClose, AllowanceCloseRefusal, AllowanceGrant, AllowanceRefusal, AllowanceRequest,
    ClaimIngestRejection, ClientClaimGate, EvidenceFile, SignedSuccessor, Subsidy, SuccessorStatus,
    ZkEdge, EVIDENCE_FILE,
};
use connector_domain::client_claim::{AllowanceId, ClientEdgeClaim};
use connector_domain::JournalEntry;
use connector_runtime::{Clock, FileJournal, Journal, TestClock};
use connector_signer::ZkStateSigner;
use connector_zk_note::evidence::{evidence_hash, scan, Record};
use connector_zk_note::note::balance::{commit, Commitment};
use connector_zk_note::note::policy::{subsidy_period, SUBSIDY_PERIOD_MS, TIERS};
use connector_zk_note::note::state::verify_state;
use connector_zk_note::request::{
    expected_tag, json_proof_for, Point, RequestProof, RequestPublic,
};
use connector_zk_note::session::{
    sign_allowance_claim, sign_allowance_close, signature_to_be_bytes, signature_to_hex,
};
use connector_zk_note::tiers::Tiers;
use connector_zk_note::{field_to_le_bytes, CompressedPubKey, Field, Keypair, Scalar, SecKey};

const HOUR_MS: u64 = 3_600_000;

/// What every test payer's note holds before its allowance, nanomina.
const NOTE_BALANCE: u64 = 5_000_000_000;

fn keypair(seed: u64) -> Keypair {
    Keypair::try_from(SecKey::new(Scalar::from(seed))).expect("a keypair")
}

/// The zk edge's state key as the node holds it: `connector-signer`'s,
/// read from the text of a key file.
fn state_signer(seed: u64) -> ZkStateSigner {
    ZkStateSigner::from_base58(&SecKey::new(Scalar::from(seed)).to_base58()).expect("a state key")
}

/// The zk edge under test: its state key, its clock, its vault, and the
/// directory its journal and evidence file live in.
struct Edge {
    state: Keypair,
    clock: Arc<TestClock>,
    vault: Arc<InMemoryZkNoteVault>,
    dir: tempfile::TempDir,
    cap: u64,
    /// The `subsidy_budget` per period; zero offers no subsidy.
    subsidy_budget: u64,
}

impl Edge {
    fn new() -> Edge {
        Edge::with_cap(10 * TIERS[1])
    }

    fn with_cap(cap: u64) -> Edge {
        Edge {
            state: keypair(1_000),
            clock: Arc::new(TestClock::new(
                Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap(),
            )),
            vault: Arc::new(InMemoryZkNoteVault::new()),
            dir: tempfile::tempdir().expect("state dir"),
            cap,
            subsidy_budget: 0,
        }
    }

    /// An edge that pays `budget` of subsidy per period.
    fn with_subsidy(budget: u64) -> Edge {
        Edge {
            subsidy_budget: budget,
            ..Edge::new()
        }
    }

    /// When the vault was created: ten whole subsidy periods and a day
    /// before the clock starts, so the first period this edge is in is 10.
    fn vault_creation_ms(&self) -> u64 {
        let start = u64::try_from(
            Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0)
                .unwrap()
                .timestamp_millis(),
        )
        .unwrap();
        start - 10 * SUBSIDY_PERIOD_MS - 86_400_000
    }

    /// The subsidy period the clock is in, as zk_toon counts it.
    fn period(&self) -> u64 {
        subsidy_period(self.vault_creation_ms(), self.now_ms()).expect("after the vault")
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.clock.now().timestamp_millis()).expect("after 1970")
    }

    fn state_key(&self) -> CompressedPubKey {
        self.state.public.into_compressed()
    }

    fn journal(&self) -> Arc<FileJournal> {
        Arc::new(
            FileJournal::open(self.dir.path().join("client-edge-claims.log")).expect("journal"),
        )
    }

    /// The gate, as the node would build it at boot: replayed from the
    /// journal, then given its zk edge.
    fn gate(&self) -> ClientClaimGate {
        self.gate_over(self.journal())
    }

    /// [`Self::gate`], over a journal of the test's choosing.
    fn gate_over(&self, journal: Arc<dyn Journal>) -> ClientClaimGate {
        ClientClaimGate::restore(journal)
            .expect("replays")
            .with_zk_edge(ZkEdge {
                vault: Arc::clone(&self.vault) as Arc<dyn ZkNoteVault>,
                evidence: EvidenceFile::open(&self.dir.path().join(EVIDENCE_FILE))
                    .expect("evidence"),
                state_signer: state_signer(1_000),
                tiers: Tiers::new([TIERS[0], TIERS[1]]).expect("tiers"),
                allowance_lifetime: std::time::Duration::from_millis(HOUR_MS),
                open_budget_cap: self.cap,
                subsidy: (self.subsidy_budget > 0).then(|| Subsidy {
                    budget: self.subsidy_budget,
                    vault_creation_ms: self.vault_creation_ms(),
                }),
                clock: Arc::clone(&self.clock) as Arc<dyn Clock>,
            })
            .expect("the evidence holds every grant")
    }

    fn entries(&self) -> Vec<JournalEntry> {
        self.journal().read_all().expect("journal")
    }

    fn evidence(&self) -> Vec<Vec<u8>> {
        let bytes = std::fs::read(self.dir.path().join(EVIDENCE_FILE)).expect("evidence");
        scan(&bytes)
            .expect("evidence")
            .records
            .iter()
            .map(|record| record.payload.to_vec())
            .collect()
    }
}

/// A payer opening one allowance: what it proves, and the session key it
/// will sign claims with.
#[derive(Clone)]
struct Payer {
    session: Keypair,
    nullifier: Field,
    root: Field,
    tier: u64,
    expires_at_ms: u64,
    request_time: u64,
    subsidised: bool,
    period: u64,
    period_nullifier: Field,
    /// The state key the proof is about; the edge's own unless spoiled.
    vault: CompressedPubKey,
    /// What the tag is bound to, if not this payer's own terms.
    tag_override: Option<Field>,
    proof_bytes: &'static str,
    /// The note's balance before this allowance, and the proof's
    /// re-randomised commitment to it.
    balance: u64,
    blinding: u64,
    commitment: Commitment,
}

impl Payer {
    fn new(edge: &Edge, seed: u64) -> Payer {
        Payer {
            session: keypair(seed),
            nullifier: Field::from(seed * 7 + 3),
            root: Field::from(5_000u64),
            tier: TIERS[1],
            expires_at_ms: edge.now_ms() + HOUR_MS / 2,
            request_time: edge.now_ms(),
            subsidised: false,
            period: 0,
            period_nullifier: Field::from(0u64),
            vault: edge.state_key(),
            tag_override: None,
            proof_bytes: "proof",
            balance: NOTE_BALANCE,
            blinding: seed + 101,
            commitment: commit(Scalar::from(NOTE_BALANCE), Scalar::from(seed + 101)),
        }
    }

    fn public(&self) -> RequestPublic {
        let session = self.session.public.into_compressed();
        RequestPublic {
            vault_x: self.vault.x,
            vault_is_odd: self.vault.is_odd,
            root: self.root,
            request_time: self.request_time,
            tier: self.tier,
            nullifier: self.nullifier,
            tag: self.tag_override.unwrap_or_else(|| {
                expected_tag(
                    self.nullifier,
                    &self.vault,
                    &session,
                    self.tier,
                    self.expires_at_ms,
                )
            }),
            commitment: Point {
                x: self.commitment.x,
                y: self.commitment.y,
            },
            subsidised: self.subsidised,
            period: self.period,
            period_nullifier: self.period_nullifier,
        }
    }

    /// This payer's request as a subsidised one, for the period the clock is
    /// in: the smallest tier, and a per-period nullifier of its own.
    fn subsidised(self, edge: &Edge) -> Payer {
        Payer {
            tier: TIERS[0],
            subsidised: true,
            period: edge.period(),
            period_nullifier: self.nullifier + Field::from(1_000_000u64),
            ..self
        }
    }

    fn proof(&self) -> RequestProof {
        RequestProof::parse(&json_proof_for(&self.public(), self.proof_bytes)).expect("a proof")
    }

    fn request(&self) -> AllowanceRequest {
        AllowanceRequest {
            proof: json_proof_for(&self.public(), self.proof_bytes),
            session_key: self.session.public.into_compressed(),
            expires_at_ms: self.expires_at_ms,
        }
    }

    /// Make this payer's proof one the vault will accept: its root held, its
    /// proof verifying.
    fn provable(self, edge: &Edge) -> Payer {
        edge.vault.hold_root(self.root);
        edge.vault.accept(&self.proof());
        self
    }

    fn claim(&self, id: AllowanceId, amount: u64) -> String {
        claim_signed_by(&self.session, id, amount)
    }
}

fn claim_signed_by(key: &Keypair, id: AllowanceId, amount: u64) -> String {
    let signature = signature_to_hex(&sign_allowance_claim(key, id.as_bytes(), amount));
    format!(
        r#"{{"version":"1.0","scheme":"zk-note","allowanceId":"{id}","cumulativeAmount":"{amount}","signature":"{signature}"}}"#
    )
}

async fn grant(gate: &ClientClaimGate, payer: &Payer) -> AllowanceGrant {
    gate.admit_allowance(payer.request())
        .await
        .expect("granted")
}

// -- Admitting an allowance --

#[tokio::test]
async fn an_allowance_is_granted_and_paid_with_allowance_claims() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 1).provable(&edge);

    let granted = grant(&gate, &payer).await;
    assert_eq!(granted.tier, TIERS[1]);
    assert_eq!(granted.expires_at_ms, payer.expires_at_ms);

    let first = gate
        .ingest(&payer.claim(granted.allowance_id, 100), 100)
        .await
        .expect("a first claim");
    assert!(matches!(first, ClientEdgeClaim::Allowance(_)));
    assert_eq!(first.claim_key(), granted.allowance_id.claim_key());
    gate.ingest(&payer.claim(granted.allowance_id, 250), 150)
        .await
        .expect("a second claim, covering its packet");
}

#[tokio::test]
async fn a_grant_is_evidence_before_it_is_an_answer() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 2).provable(&edge);
    let granted = grant(&gate, &payer).await;

    // The evidence file holds the proof verbatim, and the allowance's terms.
    let records = edge.evidence();
    assert_eq!(records.len(), 1);
    let Record::Grant(record) = Record::decode(&records[0]).expect("a record") else {
        panic!("a grant record");
    };
    assert_eq!(record.proof, payer.request().proof);
    assert_eq!(
        record.allowance_id,
        granted.allowance_id.as_bytes().to_vec()
    );
    let session_key = payer.session.public.into_compressed();
    assert_eq!(record.session_key_x, session_key.x);
    assert_eq!(record.session_key_is_odd, session_key.is_odd);
    assert_eq!(record.expires_at, payer.expires_at_ms);
    assert_eq!(record.granted_at, edge.now_ms());

    // The journal names that record by its hash, with the spent nullifier
    // and the terms -- and never the proof.
    let grants: Vec<_> = edge
        .entries()
        .into_iter()
        .filter_map(|entry| match entry {
            JournalEntry::AllowanceGranted {
                allowance_id,
                nullifier,
                evidence_hash: hash,
                tier,
                expires_at_ms,
                ..
            } => Some((allowance_id, nullifier, hash, tier, expires_at_ms)),
            _ => None,
        })
        .collect();
    assert_eq!(
        grants,
        vec![(
            granted.allowance_id.to_string(),
            field_to_le_bytes(payer.nullifier),
            evidence_hash(&records[0]),
            TIERS[1],
            payer.expires_at_ms,
        )]
    );
}

/// A case name, the request it spoils, and the refusal it must earn.
type RefusalCase = (
    &'static str,
    AllowanceRequest,
    fn(&AllowanceRefusal) -> bool,
);

/// Every refusal the gate can make without the vault, it makes without the
/// vault -- and writes nothing.
#[tokio::test]
async fn each_cheap_refusal_is_named_and_asks_the_vault_nothing() {
    let edge = Edge::new();
    let gate = edge.gate();
    let now = edge.now_ms();
    let base = Payer::new(&edge, 3).provable(&edge);

    let cases: Vec<RefusalCase> = vec![
        (
            "not a JsonProof",
            AllowanceRequest {
                proof: b"{}".to_vec(),
                ..base.request()
            },
            |r| matches!(r, AllowanceRefusal::Malformed(_)),
        ),
        (
            "a tier this edge does not publish",
            Payer {
                tier: TIERS[2],
                ..base.clone()
            }
            .request(),
            |r| matches!(r, AllowanceRefusal::TierNotOffered { .. }),
        ),
        (
            "an expiry past the allowance lifetime",
            Payer {
                expires_at_ms: now + HOUR_MS + 1,
                ..base.clone()
            }
            .request(),
            |r| matches!(r, AllowanceRefusal::ExpiryOutsideLifetime { .. }),
        ),
        (
            "an expiry already past",
            Payer {
                expires_at_ms: now,
                ..base.clone()
            }
            .request(),
            |r| matches!(r, AllowanceRefusal::ExpiryOutsideLifetime { .. }),
        ),
        (
            "another zk edge's state key",
            Payer {
                vault: keypair(77).public.into_compressed(),
                ..base.clone()
            }
            .request(),
            |r| matches!(r, AllowanceRefusal::ForeignStateKey),
        ),
        (
            "a request time more than ten minutes back",
            Payer {
                request_time: now - 600_001,
                ..base.clone()
            }
            .request(),
            |r| matches!(r, AllowanceRefusal::RequestTimeOutsideWindow { .. }),
        ),
        (
            "a request time more than a minute ahead",
            Payer {
                request_time: now + 60_001,
                ..base.clone()
            }
            .request(),
            |r| matches!(r, AllowanceRefusal::RequestTimeOutsideWindow { .. }),
        ),
        (
            "a subsidised proof at an edge with no subsidy budget",
            Payer {
                subsidised: true,
                tier: TIERS[0],
                ..base.clone()
            }
            .request(),
            |r| matches!(r, AllowanceRefusal::SubsidyBudgetClosed),
        ),
        (
            "a period on an unsubsidised proof",
            Payer {
                period: 3,
                ..base.clone()
            }
            .request(),
            |r| matches!(r, AllowanceRefusal::Malformed(_)),
        ),
        (
            "a per-period nullifier on an unsubsidised proof",
            Payer {
                period_nullifier: Field::from(3u64),
                ..base.clone()
            }
            .request(),
            |r| matches!(r, AllowanceRefusal::Malformed(_)),
        ),
        (
            "a tag bound to another session key",
            AllowanceRequest {
                session_key: keypair(78).public.into_compressed(),
                ..base.request()
            },
            |r| matches!(r, AllowanceRefusal::TagDoesNotBind),
        ),
        (
            "a tag bound to another expiry",
            AllowanceRequest {
                expires_at_ms: base.expires_at_ms - 1,
                ..base.request()
            },
            |r| matches!(r, AllowanceRefusal::TagDoesNotBind),
        ),
        (
            "a tag bound to nothing of this edge's",
            Payer {
                tag_override: Some(Field::from(42u64)),
                ..base.clone()
            }
            .request(),
            |r| matches!(r, AllowanceRefusal::TagDoesNotBind),
        ),
    ];

    for (case, request, expected) in cases {
        let refusal = gate.admit_allowance(request).await.expect_err(case);
        assert!(expected(&refusal), "{case}: {refusal:?}");
        assert_eq!(edge.vault.reads(), 0, "{case}: the vault was asked");
    }
    assert!(edge.evidence().is_empty(), "nothing was written");
    assert!(edge.entries().is_empty(), "nothing was journaled");

    // And the request every case spoiled is fine.
    grant(&gate, &base).await;
}

#[tokio::test]
async fn a_replayed_proof_is_refused_without_asking_the_vault() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 4).provable(&edge);
    grant(&gate, &payer).await;
    let reads = edge.vault.reads();

    assert_eq!(
        gate.admit_allowance(payer.request()).await.unwrap_err(),
        AllowanceRefusal::NullifierSpent
    );
    assert_eq!(edge.vault.reads(), reads);
    assert_eq!(edge.evidence().len(), 1);
}

/// A nullifier spent by one allowance cannot open another, whatever else the
/// second request changes -- and a restart forgets none of them.
#[tokio::test]
async fn a_nullifier_is_spent_once_across_allowances_and_restarts() {
    let edge = Edge::new();
    let first = Payer::new(&edge, 5).provable(&edge);
    grant(&edge.gate(), &first).await;

    let second = Payer {
        session: keypair(6),
        proof_bytes: "another proof",
        ..first.clone()
    }
    .provable(&edge);
    let restarted = edge.gate();
    assert_eq!(
        restarted
            .admit_allowance(second.request())
            .await
            .unwrap_err(),
        AllowanceRefusal::NullifierSpent
    );
}

/// The edge's journal on a disk that refuses the next `failures` appends,
/// and then recovers: a full disk, freed.
struct FullDisk {
    inner: Arc<FileJournal>,
    failures: std::sync::atomic::AtomicUsize,
}

impl Journal for FullDisk {
    fn append(&self, entry: &JournalEntry) -> Result<(), connector_runtime::JournalError> {
        use std::sync::atomic::Ordering;
        let left = self.failures.load(Ordering::SeqCst);
        if left > 0 {
            self.failures.store(left - 1, Ordering::SeqCst);
            return Err(connector_runtime::JournalError::Io(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "disk full",
            )));
        }
        self.inner.append(entry)
    }

    fn read_all(&self) -> Result<Vec<JournalEntry>, connector_runtime::JournalError> {
        self.inner.read_all()
    }
}

/// ADR 0080 decision 11: a grant whose journal entry fails is refused, and
/// leaves no grant record behind -- so the retry writes the only grant for
/// its nullifier, and the challenger never reads a grant the edge forgot.
#[tokio::test]
async fn a_grant_the_journal_refuses_leaves_no_evidence_and_a_retry_writes_one() {
    let edge = Edge::new();
    let gate = edge.gate_over(Arc::new(FullDisk {
        inner: edge.journal(),
        failures: std::sync::atomic::AtomicUsize::new(1),
    }));
    let payer = Payer::new(&edge, 8).provable(&edge);

    assert_eq!(
        gate.admit_allowance(payer.request()).await.unwrap_err(),
        AllowanceRefusal::NotDurable
    );
    assert!(edge.evidence().is_empty(), "the failed grant left a record");
    assert!(edge.entries().is_empty());

    let granted = grant(&gate, &payer).await;
    let records = edge.evidence();
    assert_eq!(records.len(), 1, "one grant for one nullifier");
    let Record::Grant(record) = Record::decode(&records[0]).expect("a record") else {
        panic!("a grant record");
    };
    assert_eq!(
        record.allowance_id,
        granted.allowance_id.as_bytes().to_vec()
    );
    drop(gate);
    // And the edge boots on what it left.
    edge.gate();
}

#[tokio::test]
async fn a_nullifier_spent_on_chain_is_refused() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 7).provable(&edge);
    edge.vault.spend_on_chain(payer.nullifier);
    assert_eq!(
        gate.admit_allowance(payer.request()).await.unwrap_err(),
        AllowanceRefusal::NullifierSpent
    );
    assert_eq!(edge.vault.verifications(), 0, "refused before the proof");
    assert!(edge.entries().is_empty());
}

#[tokio::test]
async fn an_unknown_root_is_refused_before_the_proof() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 8);
    edge.vault.accept(&payer.proof());
    assert_eq!(
        gate.admit_allowance(payer.request()).await.unwrap_err(),
        AllowanceRefusal::UnknownRoot
    );
    assert_eq!(edge.vault.verifications(), 0);
}

/// A proof whose public inputs all check out, and which does not verify:
/// a forged or mis-keyed proof. Nothing is spent, so the real proof for the
/// same note still opens.
#[tokio::test]
async fn a_proof_that_does_not_verify_is_refused_and_spends_nothing() {
    let edge = Edge::new();
    let gate = edge.gate();
    let real = Payer::new(&edge, 9).provable(&edge);
    let forged = Payer {
        proof_bytes: "forged",
        ..real.clone()
    };
    assert_eq!(
        gate.admit_allowance(forged.request()).await.unwrap_err(),
        AllowanceRefusal::ProofInvalid
    );
    assert!(edge.entries().is_empty());
    assert!(edge.evidence().is_empty());
    grant(&gate, &real).await;
}

#[tokio::test]
async fn a_vault_that_cannot_be_read_refuses_the_allowance_by_name() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 10).provable(&edge);
    edge.vault.go_down();
    assert!(matches!(
        gate.admit_allowance(payer.request()).await.unwrap_err(),
        AllowanceRefusal::VaultUnavailable(_)
    ));
    assert!(edge.entries().is_empty());
}

/// ADR 0080 decision 5: the budgets of open allowances may not sum past the
/// cap, and the refusal is free.
#[tokio::test]
async fn the_open_budget_cap_refuses_an_allowance_that_would_exceed_it() {
    let edge = Edge::with_cap(TIERS[1] + TIERS[0]);
    let gate = edge.gate();
    grant(&gate, &Payer::new(&edge, 11).provable(&edge)).await;
    let small = Payer {
        tier: TIERS[0],
        ..Payer::new(&edge, 12)
    }
    .provable(&edge);
    grant(&gate, &small).await;

    let over = Payer {
        tier: TIERS[0],
        ..Payer::new(&edge, 13)
    }
    .provable(&edge);
    let reads = edge.vault.reads();
    assert_eq!(
        gate.admit_allowance(over.request()).await.unwrap_err(),
        AllowanceRefusal::OpenBudgetExceeded {
            open: TIERS[1] + TIERS[0],
            tier: TIERS[0],
            cap: TIERS[1] + TIERS[0],
        }
    );
    assert_eq!(edge.vault.reads(), reads);

    // The open budget is restored from the journal too.
    assert!(matches!(
        edge.gate()
            .admit_allowance(over.request())
            .await
            .unwrap_err(),
        AllowanceRefusal::OpenBudgetExceeded { .. }
    ));
}

/// Two requests spending one nullifier at once: exactly one is granted.
#[tokio::test]
async fn concurrent_requests_for_one_nullifier_open_one_allowance() {
    let edge = Edge::new();
    let gate = edge.gate();
    let a = Payer::new(&edge, 14).provable(&edge);
    let b = Payer {
        session: keypair(15),
        proof_bytes: "b",
        ..a.clone()
    }
    .provable(&edge);
    let (ra, rb) = tokio::join!(
        gate.admit_allowance(a.request()),
        gate.admit_allowance(b.request())
    );
    assert_eq!([ra.is_ok(), rb.is_ok()].iter().filter(|ok| **ok).count(), 1);
    assert_eq!(edge.evidence().len(), 1);
}

#[tokio::test]
async fn a_refusal_never_names_the_nullifier_or_the_session_key() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 16).provable(&edge);
    grant(&gate, &payer).await;
    let secret_words = [
        connector_zk_note::note::field_to_string(payer.nullifier),
        connector_zk_note::note::field_to_string(payer.session.public.into_compressed().x),
        payer.session.public.into_address(),
    ];
    let refusals = [
        gate.admit_allowance(payer.request()).await.unwrap_err(),
        gate.admit_allowance(
            Payer {
                tag_override: Some(Field::from(1u64)),
                nullifier: Field::from(999u64),
                ..payer.clone()
            }
            .request(),
        )
        .await
        .unwrap_err(),
    ];
    for refusal in refusals {
        let message = refusal.to_string();
        for word in &secret_words {
            assert!(!message.contains(word.as_str()), "{message}");
        }
    }
}

#[tokio::test]
async fn a_node_that_is_no_zk_edge_opens_no_allowance() {
    let edge = Edge::new();
    let gate = ClientClaimGate::restore(edge.journal()).expect("replays");
    let payer = Payer::new(&edge, 17);
    assert_eq!(
        gate.admit_allowance(payer.request()).await.unwrap_err(),
        AllowanceRefusal::NotAZkEdge
    );
    // And its claim is an unknown scheme, exactly as before ADR 0080.
    let id = AllowanceId::from_bytes([1; 16]);
    assert_eq!(
        gate.ingest(&payer.claim(id, 1), 0).await.unwrap_err(),
        ClaimIngestRejection::Malformed(
            "claim is structurally invalid: unsupported claim scheme 'zk-note' (expected \
             'batch-settlement')"
                .to_string()
        )
    );
}

// -- Admitting allowance claims --

#[tokio::test]
async fn stale_over_budget_and_wrongly_signed_claims_are_refused() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 18).provable(&edge);
    let id = grant(&gate, &payer).await.allowance_id;
    gate.ingest(&payer.claim(id, 1_000), 0)
        .await
        .expect("first");

    assert_eq!(
        gate.ingest(&payer.claim(id, 1_000), 0)
            .await
            .map(|claim| claim.transferred_amount()),
        Ok(1_000),
        "a byte-identical resend at no charge is a retransmission"
    );
    assert_eq!(
        gate.ingest(&payer.claim(id, 900), 0).await.unwrap_err(),
        ClaimIngestRejection::AmountNotAdvancing
    );
    assert_eq!(
        gate.ingest(&payer.claim(id, 1_050), 100).await.unwrap_err(),
        ClaimIngestRejection::Underpayment {
            advanced: 50,
            price: 100
        }
    );
    assert_eq!(
        gate.ingest(&payer.claim(id, TIERS[1] + 1), 0)
            .await
            .unwrap_err(),
        ClaimIngestRejection::OverBudget {
            claimed: u128::from(TIERS[1] + 1),
            budget: TIERS[1],
        }
    );
    assert_eq!(
        gate.ingest(&claim_signed_by(&keypair(99), id, 2_000), 0)
            .await
            .unwrap_err(),
        ClaimIngestRejection::SignatureInvalid
    );
    // Its budget, exactly, is fine.
    gate.ingest(&payer.claim(id, TIERS[1]), 0)
        .await
        .expect("a claim at the budget");
}

#[tokio::test]
async fn a_claim_on_an_allowance_this_edge_never_granted_is_refused() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 19);
    assert_eq!(
        gate.ingest(&payer.claim(AllowanceId::from_bytes([7; 16]), 1), 0)
            .await
            .unwrap_err(),
        ClaimIngestRejection::UnknownAllowance
    );
}

#[tokio::test]
async fn a_claim_after_the_allowance_expires_is_refused() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 20).provable(&edge);
    let id = grant(&gate, &payer).await.allowance_id;
    edge.clock.advance(TimeDelta::milliseconds(
        i64::try_from(payer.expires_at_ms - edge.now_ms()).unwrap(),
    ));
    assert_eq!(
        gate.ingest(&payer.claim(id, 1), 0).await.unwrap_err(),
        ClaimIngestRejection::AllowanceExpired
    );
}

#[tokio::test]
async fn open_allowances_and_their_watermarks_survive_a_restart() {
    let edge = Edge::new();
    let payer = Payer::new(&edge, 21).provable(&edge);
    let id = {
        let gate = edge.gate();
        let id = grant(&gate, &payer).await.allowance_id;
        gate.ingest(&payer.claim(id, 500), 0).await.expect("before");
        id
    };
    let restarted = edge.gate();
    assert_eq!(
        restarted
            .ingest(&payer.claim(id, 400), 0)
            .await
            .unwrap_err(),
        ClaimIngestRejection::AmountNotAdvancing
    );
    restarted
        .ingest(&payer.claim(id, 600), 100)
        .await
        .expect("after");
}

#[tokio::test]
async fn a_mina_claim_is_still_refused_by_name_at_a_zk_edge() {
    let edge = Edge::new();
    let gate = edge.gate();
    let mina = r#"{"version":"1.0","scheme":"zk-note","blockchain":"mina","allowanceId":"00","cumulativeAmount":"1","signature":"00"}"#;
    assert_eq!(
        gate.ingest(mina, 0).await.unwrap_err(),
        ClaimIngestRejection::Mina
    );
}

// -- Closing an allowance, and its successor (ADR 0080 decision 4, #1523) --

impl Payer {
    /// A close request signed by this payer's session key.
    fn close(&self, id: AllowanceId) -> AllowanceClose {
        close_signed_by(&self.session, id)
    }

    /// Check a successor the way the payer's wallet does (ADR 0080
    /// decision 2): signed by the zk edge's state key, and the old balance
    /// less exactly `charge`, under the same blinding.
    fn check(&self, edge: &Edge, successor: &SignedSuccessor, charge: u64) {
        assert_eq!(
            successor.charge, charge,
            "the charge is the final watermark"
        );
        assert_eq!(
            successor.commitment,
            commit(
                Scalar::from(self.balance - charge),
                Scalar::from(self.blinding)
            ),
            "the successor is the old balance less the charge"
        );
        assert_eq!(
            successor.anchor,
            connector_zk_note::note::next_anchor(self.nullifier)
        );
        assert!(verify_state(
            &successor.signature,
            &edge.state.public,
            &successor.message()
        ));
    }
}

fn close_signed_by(key: &Keypair, id: AllowanceId) -> AllowanceClose {
    AllowanceClose {
        allowance_id: id,
        signature: signature_to_be_bytes(&sign_allowance_close(key, id.as_bytes())),
    }
}

fn signed(status: SuccessorStatus) -> SignedSuccessor {
    match status {
        SuccessorStatus::Signed(successor) => *successor,
        SuccessorStatus::Pending => panic!("expected a signed successor, got pending"),
    }
}

impl Edge {
    /// Every charge the journal holds, by allowance.
    fn charges(&self) -> Vec<(String, u64)> {
        self.entries()
            .into_iter()
            .filter_map(|entry| match entry {
                JournalEntry::AllowanceClosed {
                    allowance_id,
                    charge,
                } => Some((allowance_id, charge)),
                _ => None,
            })
            .collect()
    }

    fn successors(&self) -> usize {
        self.entries()
            .iter()
            .filter(|entry| matches!(entry, JournalEntry::AllowanceSuccessorSigned { .. }))
            .count()
    }

    fn advance_ms(&self, ms: u64) {
        self.clock
            .advance(TimeDelta::milliseconds(i64::try_from(ms).unwrap()));
    }
}

#[tokio::test]
async fn an_open_allowances_successor_is_pending_and_an_unknown_one_is_refused() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 30).provable(&edge);
    let id = grant(&gate, &payer).await.allowance_id;
    gate.ingest(&payer.claim(id, 100), 100).await.unwrap();
    assert_eq!(gate.successor(id), Ok(SuccessorStatus::Pending));
    assert_eq!(
        gate.successor(AllowanceId::from_bytes([9; 16])),
        Err(AllowanceCloseRefusal::UnknownAllowance)
    );
    // A fetch only reads: nothing was closed or journaled by asking.
    assert!(edge.charges().is_empty());
    gate.ingest(&payer.claim(id, 200), 100)
        .await
        .expect("still open");
}

#[tokio::test]
async fn a_close_waits_for_every_forward_in_flight_before_charging() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 31).provable(&edge);
    let id = grant(&gate, &payer).await.allowance_id;
    gate.ingest(&payer.claim(id, 100), 100).await.unwrap();
    let (_, in_flight) = gate
        .ingest_forward(&payer.claim(id, 250), 150)
        .await
        .expect("admitted and forwarded");

    // The close stops new claims at once, but charges nothing yet.
    assert_eq!(
        gate.close_allowance(payer.close(id)),
        Ok(SuccessorStatus::Pending)
    );
    assert_eq!(
        gate.ingest(&payer.claim(id, 300), 50).await.unwrap_err(),
        ClaimIngestRejection::AllowanceClosed
    );
    assert_eq!(gate.successor(id), Ok(SuccessorStatus::Pending));
    assert!(
        edge.charges().is_empty(),
        "no charge while a forward is in flight"
    );

    // The forward resolves: the charge is the final watermark, once.
    drop(in_flight);
    let successor = signed(gate.successor(id).expect("known"));
    payer.check(&edge, &successor, 250);
    assert_eq!(edge.charges(), vec![(id.to_string(), 250)]);
    assert_eq!(edge.successors(), 1);
}

#[tokio::test]
async fn the_charge_is_written_once_and_the_successor_is_idempotent() {
    let edge = Edge::new();
    let payer = Payer::new(&edge, 32).provable(&edge);
    let (id, first) = {
        let gate = edge.gate();
        let id = grant(&gate, &payer).await.allowance_id;
        gate.ingest(&payer.claim(id, 100), 100).await.unwrap();
        gate.ingest(&payer.claim(id, 400), 100).await.unwrap();
        let first = signed(gate.close_allowance(payer.close(id)).expect("closed"));
        payer.check(&edge, &first, 400);
        // Asking again -- to close, or for the successor -- changes nothing.
        assert_eq!(
            gate.close_allowance(payer.close(id)),
            Ok(SuccessorStatus::Signed(Box::new(first.clone())))
        );
        assert_eq!(
            gate.successor(id),
            Ok(SuccessorStatus::Signed(Box::new(first.clone())))
        );
        (id, first)
    };
    // Nor does a restart: the same successor, from the journal.
    let restarted = edge.gate();
    assert_eq!(
        restarted.successor(id),
        Ok(SuccessorStatus::Signed(Box::new(first)))
    );
    assert_eq!(edge.charges(), vec![(id.to_string(), 400)]);
    assert_eq!(edge.successors(), 1);

    // The evidence file holds the close record the challenger reads: the
    // spent nullifier, the allowance and the charge.
    let closes: Vec<_> = edge
        .evidence()
        .iter()
        .filter_map(|payload| match Record::decode(payload).unwrap() {
            Record::Close(close) => Some((close.nullifier, close.allowance_id, close.charge)),
            Record::Grant(_) => None,
        })
        .collect();
    assert_eq!(closes, vec![(payer.nullifier, id.as_bytes().to_vec(), 400)]);
}

#[tokio::test]
async fn an_allowance_closed_with_no_claim_charges_nothing() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 33).provable(&edge);
    let id = grant(&gate, &payer).await.allowance_id;
    let successor = signed(gate.close_allowance(payer.close(id)).expect("closed"));
    payer.check(&edge, &successor, 0);
}

#[tokio::test]
async fn a_close_not_signed_by_the_session_key_is_refused_and_closes_nothing() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 34).provable(&edge);
    let id = grant(&gate, &payer).await.allowance_id;
    assert_eq!(
        gate.close_allowance(close_signed_by(&keypair(77_777), id)),
        Err(AllowanceCloseRefusal::SignatureInvalid)
    );
    // A claim's signature is not a close's.
    assert_eq!(
        gate.close_allowance(AllowanceClose {
            allowance_id: id,
            signature: signature_to_be_bytes(&sign_allowance_claim(
                &payer.session,
                id.as_bytes(),
                0
            )),
        }),
        Err(AllowanceCloseRefusal::SignatureInvalid)
    );
    assert_eq!(
        gate.close_allowance(close_signed_by(
            &payer.session,
            AllowanceId::from_bytes([3; 16])
        )),
        Err(AllowanceCloseRefusal::UnknownAllowance)
    );
    gate.ingest(&payer.claim(id, 100), 100)
        .await
        .expect("still open");
}

#[tokio::test]
async fn reaching_the_budget_closes_the_allowance() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 35).provable(&edge);
    let id = grant(&gate, &payer).await.allowance_id;
    let (_, in_flight) = gate
        .ingest_forward(&payer.claim(id, TIERS[1]), 100)
        .await
        .expect("a claim at the budget");
    assert_eq!(gate.successor(id), Ok(SuccessorStatus::Pending));
    drop(in_flight);
    payer.check(&edge, &signed(gate.successor(id).unwrap()), TIERS[1]);
    assert_eq!(
        gate.ingest(&payer.claim(id, TIERS[1]), 0)
            .await
            .unwrap_err(),
        ClaimIngestRejection::AllowanceClosed
    );
}

#[tokio::test]
async fn the_sweep_closes_an_expired_allowance_and_then_its_claims_are_refused() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 36).provable(&edge);
    let id = grant(&gate, &payer).await.allowance_id;
    gate.ingest(&payer.claim(id, 300), 100).await.unwrap();

    gate.sweep_allowances();
    assert_eq!(
        gate.successor(id),
        Ok(SuccessorStatus::Pending),
        "not yet expired"
    );

    edge.advance_ms(payer.expires_at_ms - edge.now_ms());
    assert_eq!(
        gate.ingest(&payer.claim(id, 400), 100).await.unwrap_err(),
        ClaimIngestRejection::AllowanceExpired
    );
    gate.sweep_allowances();
    payer.check(&edge, &signed(gate.successor(id).unwrap()), 300);
    assert_eq!(
        gate.ingest(&payer.claim(id, 400), 100).await.unwrap_err(),
        ClaimIngestRejection::AllowanceClosed
    );
}

#[tokio::test]
async fn the_sweep_waits_for_an_expired_allowances_forward_in_flight() {
    let edge = Edge::new();
    let gate = edge.gate();
    let payer = Payer::new(&edge, 37).provable(&edge);
    let id = grant(&gate, &payer).await.allowance_id;
    let (_, in_flight) = gate
        .ingest_forward(&payer.claim(id, 300), 100)
        .await
        .unwrap();
    edge.advance_ms(payer.expires_at_ms - edge.now_ms());
    gate.sweep_allowances();
    assert_eq!(gate.successor(id), Ok(SuccessorStatus::Pending));
    drop(in_flight);
    payer.check(&edge, &signed(gate.successor(id).unwrap()), 300);
}

#[tokio::test]
async fn the_boot_sweep_closes_what_expired_while_the_node_was_down() {
    let edge = Edge::new();
    let payer = Payer::new(&edge, 38).provable(&edge);
    let still_open = Payer {
        expires_at_ms: edge.now_ms() + HOUR_MS,
        ..Payer::new(&edge, 39)
    }
    .provable(&edge);
    let (expiring, open) = {
        let gate = edge.gate();
        let expiring = grant(&gate, &payer).await.allowance_id;
        let open = grant(&gate, &still_open).await.allowance_id;
        gate.ingest(&payer.claim(expiring, 700), 100).await.unwrap();
        (expiring, open)
    };
    edge.advance_ms(payer.expires_at_ms - edge.now_ms());

    // Replayed, then swept, before the gate serves anything.
    let gate = edge.gate();
    payer.check(&edge, &signed(gate.successor(expiring).unwrap()), 700);
    assert_eq!(gate.successor(open), Ok(SuccessorStatus::Pending));
    assert_eq!(edge.charges(), vec![(expiring.to_string(), 700)]);
}

/// ADR 0080 decision 4: a close is final. A node that stops after closing
/// an allowance -- on its session key's request, on reaching its budget, or
/// at its expiry -- but before its forwards in flight resolved, boots with
/// it closing, refuses its claims, and finishes the close in the boot
/// sweep: Δ written, the successor signed.
#[tokio::test]
async fn a_close_survives_a_restart_and_the_boot_sweep_finishes_it() {
    let edge = Edge::new();
    let by_request = Payer::new(&edge, 43).provable(&edge);
    let by_budget = Payer::new(&edge, 44).provable(&edge);
    let by_expiry = Payer {
        expires_at_ms: edge.now_ms() + 1_000,
        ..Payer::new(&edge, 45)
    }
    .provable(&edge);
    let (requested, budgeted, expired) = {
        let gate = edge.gate();
        let requested = grant(&gate, &by_request).await.allowance_id;
        let budgeted = grant(&gate, &by_budget).await.allowance_id;
        let expired = grant(&gate, &by_expiry).await.allowance_id;
        let (_, a) = gate
            .ingest_forward(&by_request.claim(requested, 300), 100)
            .await
            .unwrap();
        let (_, b) = gate
            .ingest_forward(&by_budget.claim(budgeted, TIERS[1]), 100)
            .await
            .unwrap();
        let (_, c) = gate
            .ingest_forward(&by_expiry.claim(expired, 200), 100)
            .await
            .unwrap();
        assert_eq!(
            gate.close_allowance(by_request.close(requested)),
            Ok(SuccessorStatus::Pending)
        );
        edge.advance_ms(1_000);
        gate.sweep_allowances();
        // The node stops with all three forwards still in flight.
        std::mem::forget((a, b, c));
        assert!(edge.charges().is_empty());
        (requested, budgeted, expired)
    };
    let gate = edge.gate();
    by_request.check(&edge, &signed(gate.successor(requested).unwrap()), 300);
    by_budget.check(&edge, &signed(gate.successor(budgeted).unwrap()), TIERS[1]);
    by_expiry.check(&edge, &signed(gate.successor(expired).unwrap()), 200);
    assert_eq!(
        gate.ingest(&by_request.claim(requested, 400), 100)
            .await
            .unwrap_err(),
        ClaimIngestRejection::AllowanceClosed
    );
    let mut charges = edge.charges();
    charges.sort();
    let mut expected = vec![
        (requested.to_string(), 300),
        (budgeted.to_string(), TIERS[1]),
        (expired.to_string(), 200),
    ];
    expected.sort();
    assert_eq!(charges, expected);
}

/// A signed close the journal refuses is refused as not durable -- the
/// payer retries -- but the allowance takes no more claims meanwhile.
#[tokio::test]
async fn a_close_the_journal_refuses_is_not_durable_and_still_stops_claims() {
    use std::sync::atomic::Ordering;
    let edge = Edge::new();
    let disk = Arc::new(FullDisk {
        inner: edge.journal(),
        failures: std::sync::atomic::AtomicUsize::new(0),
    });
    let gate = edge.gate_over(Arc::clone(&disk) as Arc<dyn Journal>);
    let payer = Payer::new(&edge, 47).provable(&edge);
    let id = grant(&gate, &payer).await.allowance_id;
    gate.ingest(&payer.claim(id, 100), 100).await.unwrap();

    disk.failures.store(1, Ordering::SeqCst);
    assert_eq!(
        gate.close_allowance(payer.close(id)),
        Err(AllowanceCloseRefusal::NotDurable)
    );
    assert_eq!(
        gate.ingest(&payer.claim(id, 200), 100).await.unwrap_err(),
        ClaimIngestRejection::AllowanceClosed
    );
    payer.check(
        &edge,
        &signed(gate.close_allowance(payer.close(id)).unwrap()),
        100,
    );
}

#[tokio::test]
async fn a_closed_allowance_no_longer_counts_against_the_open_budget() {
    let edge = Edge::with_cap(TIERS[1]);
    let gate = edge.gate();
    let first = Payer::new(&edge, 40).provable(&edge);
    let second = Payer::new(&edge, 41).provable(&edge);
    let id = grant(&gate, &first).await.allowance_id;
    assert!(matches!(
        gate.admit_allowance(second.request()).await.unwrap_err(),
        AllowanceRefusal::OpenBudgetExceeded { .. }
    ));
    signed(gate.close_allowance(first.close(id)).unwrap());
    grant(&gate, &second).await;
    // And after a restart, from the journal: only the second is open.
    let third = Payer::new(&edge, 42).provable(&edge);
    assert!(matches!(
        edge.gate().admit_allowance(third.request()).await.unwrap_err(),
        AllowanceRefusal::OpenBudgetExceeded { open, .. } if open == TIERS[1]
    ));
}

// -- The two HTTP endpoints (ADR 0080 decision 8, #1523) --

mod endpoints {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use connector_client_edge::{zk_note_router, ZK_NOTE_ALLOWANCES_PATH};
    use tower::ServiceExt;

    async fn call(app: axum::Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = app.oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = hyper::body::to_bytes(response.into_body())
            .await
            .expect("body");
        (status, serde_json::from_slice(&bytes).expect("JSON"))
    }

    fn post(body: String) -> Request<Body> {
        Request::post(ZK_NOTE_ALLOWANCES_PATH)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .expect("request")
    }

    fn get_successor(id: &str) -> Request<Body> {
        Request::get(format!("{ZK_NOTE_ALLOWANCES_PATH}/{id}/successor"))
            .body(Body::empty())
            .expect("request")
    }

    fn open_body(payer: &Payer) -> String {
        let request = payer.request();
        let proof: serde_json::Value = serde_json::from_slice(&request.proof).unwrap();
        serde_json::json!({
            "proof": proof,
            "sessionKey": request.session_key.into_address(),
            "expiresAt": request.expires_at_ms.to_string(),
        })
        .to_string()
    }

    fn close_body(payer: &Payer, id: &str) -> String {
        let id: AllowanceId = id.parse().unwrap();
        serde_json::json!({
            "allowanceId": id.to_string(),
            "signature": signature_to_hex(&sign_allowance_close(&payer.session, id.as_bytes())),
        })
        .to_string()
    }

    #[tokio::test]
    async fn an_allowance_opens_and_closes_over_http_and_its_successor_is_idempotent() {
        let edge = Edge::new();
        let gate = Arc::new(edge.gate());
        let app = zk_note_router(Arc::clone(&gate));
        let payer = Payer::new(&edge, 50).provable(&edge);

        let (status, opened) = call(app.clone(), post(open_body(&payer))).await;
        assert_eq!(status, StatusCode::OK, "{opened}");
        let id = opened["allowanceId"].as_str().expect("an id").to_string();
        assert_eq!(opened["tier"], TIERS[1].to_string());
        assert_eq!(opened["expiresAt"], payer.expires_at_ms.to_string());

        let (status, pending) = call(app.clone(), get_successor(&id)).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(
            pending,
            serde_json::json!({"allowanceId": id, "status": "pending"})
        );

        let allowance: AllowanceId = id.parse().unwrap();
        gate.ingest(&payer.claim(allowance, 300), 100)
            .await
            .unwrap();
        let (status, closed) = call(app.clone(), post(close_body(&payer, &id))).await;
        assert_eq!(status, StatusCode::OK, "{closed}");
        assert_eq!(closed["status"], "signed");
        assert_eq!(closed["charge"], "300");
        let successor = signed(gate.successor(allowance).unwrap());
        payer.check(&edge, &successor, 300);
        assert_eq!(
            closed["anchor"],
            connector_zk_note::note::field_to_string(successor.anchor)
        );
        assert_eq!(
            closed["commitment"]["x"],
            connector_zk_note::note::field_to_string(successor.commitment.x)
        );
        assert_eq!(
            closed["signature"]["r"],
            connector_zk_note::note::field_to_string(successor.signature.rx)
        );

        let (status, again) = call(app.clone(), get_successor(&id)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(again, closed, "every fetch returns the same signed state");
    }

    #[tokio::test]
    async fn each_refusal_is_named() {
        let edge = Edge::new();
        let app = zk_note_router(Arc::new(edge.gate()));
        let payer = Payer::new(&edge, 51).provable(&edge);

        let (status, body) = call(app.clone(), post("{}".to_string())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "request_malformed");

        let unoffered = Payer {
            tier: TIERS[2],
            ..payer.clone()
        };
        let (status, body) = call(app.clone(), post(open_body(&unoffered))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"], "tier_not_offered");

        let unknown = "00000000000000000000000000000000";
        let (status, body) = call(app.clone(), get_successor(unknown)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "unknown_allowance");
        let (status, body) = call(app.clone(), get_successor("not-an-id")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "request_malformed");

        let (_, opened) = call(app.clone(), post(open_body(&payer))).await;
        let id = opened["allowanceId"].as_str().unwrap();
        let stranger = Payer::new(&edge, 52);
        let (status, body) = call(app.clone(), post(close_body(&stranger, id))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"], "signature_invalid");
    }

    #[tokio::test]
    async fn a_node_that_is_no_zk_edge_refuses_both_endpoints_by_name() {
        let journal = Arc::new(connector_runtime::InMemoryJournal::new());
        let app = zk_note_router(Arc::new(ClientClaimGate::restore(journal).unwrap()));
        let (status, body) = call(app.clone(), post("{}".to_string())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "zk_note_not_offered");
        let (status, body) = call(app, get_successor("00000000000000000000000000000000")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "zk_note_not_offered");
    }
}

// -- An allowance claim over the client BTP carriage (client-edge-spec.md
// §1.9), in the `payment-channel-claim` protocolData a voucher rides in --

mod btp {
    use super::*;
    use connector_config::StaticRoute;
    use connector_domain::{EnvelopeRequest, EnvelopeResponse, Fulfill, Prepare, Reject};
    use connector_runtime::{AppOutcome, Connector, FakeAppClient, InProcessPeerTransport};
    use connector_signer::{LocalSigner, PublicKeyBytes, Signer};
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    const PRICE: u64 = 100;
    const BTP_RESPONSE: u8 = 1;
    const BTP_MESSAGE: u8 = 6;

    /// A MESSAGE frame, as `@toon-protocol/client` writes one.
    fn btp_message(request_id: u32, protocol_data: &[(&str, &[u8])], ilp: &[u8]) -> Vec<u8> {
        let mut out = vec![BTP_MESSAGE];
        out.extend_from_slice(&request_id.to_be_bytes());
        out.push(u8::try_from(protocol_data.len()).unwrap());
        for (name, data) in protocol_data {
            out.push(u8::try_from(name.len()).unwrap());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&1u16.to_be_bytes());
            out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
            out.extend_from_slice(data);
        }
        out.extend_from_slice(&u32::try_from(ilp.len()).unwrap().to_be_bytes());
        out.extend_from_slice(ilp);
        out
    }

    /// A RESPONSE frame's type, request id and ILP packet.
    fn parse_response(buf: &[u8]) -> (u8, u32, Vec<u8>) {
        let be32 = |at: usize| u32::from_be_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]);
        let frame_type = buf[0];
        let request_id = be32(1);
        let mut pos = 5;
        let count = usize::from(buf[pos]);
        pos += 1;
        for _ in 0..count {
            pos += 1 + usize::from(buf[pos]) + 2;
            pos += 4 + be32(pos) as usize;
        }
        let len = be32(pos) as usize;
        (frame_type, request_id, buf[pos + 4..pos + 4 + len].to_vec())
    }

    fn sealed_prepare(receiver: &PublicKeyBytes) -> Prepare {
        let envelope = EnvelopeRequest {
            method: "POST".to_string(),
            target: "/".to_string(),
            headers: vec![],
            body: b"paid by an allowance".to_vec(),
        }
        .encode();
        let (data, _) =
            connector_signer::giftwrap::seal_request(&envelope, receiver).expect("seal");
        Prepare {
            amount: PRICE,
            expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
            greeting: false,
            destination: "g.test.app".to_string(),
            data,
        }
    }

    type Session = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn answer(session: &mut Session) -> (u8, u32, Vec<u8>) {
        loop {
            let frame = session.next().await.expect("open").expect("a frame");
            if let WsMessage::Binary(bytes) = frame {
                return parse_response(&bytes);
            }
        }
    }

    async fn send(session: &mut Session, frame: Vec<u8>) {
        session.send(WsMessage::Binary(frame)).await.expect("sent");
    }

    /// ADR 0080 decision 2: an allowance claim pays for a packet over BTP
    /// exactly as over HTTP -- admitted, its watermark advanced under the
    /// allowance's key, the packet fulfilled -- and the same claim again
    /// buys nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_allowance_claim_pays_for_a_packet_over_btp() {
        let edge = Edge::new();
        let gate = edge.gate();
        let payer = Payer::new(&edge, 60).provable(&edge);
        let id = grant(&gate, &payer).await.allowance_id;

        let route = StaticRoute::new_priced("g.test.app", "http://localhost:4000", PRICE).unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(
            route.handler_url(),
            AppOutcome::Answered {
                response: EnvelopeResponse {
                    status: 200,
                    headers: vec![],
                    body: b"app said yes".to_vec(),
                },
            },
        );
        let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("allowance-btp-test"));
        let connector = Arc::new(
            Connector::new(
                vec![route],
                vec![],
                app_client,
                Arc::new(InProcessPeerTransport::new()),
                Arc::clone(&edge.clock) as Arc<dyn Clock>,
            )
            .with_identity_signer(signer.clone()),
        );
        let app = connector_client_edge::router_with_gate(connector, signer.clone(), None, gate);
        let server =
            axum::Server::bind(&"127.0.0.1:0".parse().unwrap()).serve(app.into_make_service());
        let addr = server.local_addr();
        tokio::spawn(server);
        let (mut session, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ilp/btp"))
            .await
            .expect("upgraded");

        send(
            &mut session,
            btp_message(1, &[("auth", br#"{"peerId":"p","secret":""}"#)], &[]),
        )
        .await;
        assert_eq!(answer(&mut session).await.0, BTP_RESPONSE);

        let receiver = signer.public_key().unwrap();
        let claim = payer.claim(id, PRICE);
        send(
            &mut session,
            btp_message(
                2,
                &[("payment-channel-claim", claim.as_bytes())],
                &sealed_prepare(&receiver).encode(),
            ),
        )
        .await;
        let (frame_type, request_id, packet) = answer(&mut session).await;
        assert_eq!((frame_type, request_id), (BTP_RESPONSE, 2));
        Fulfill::decode(&packet).unwrap_or_else(|_| {
            let reject = Reject::decode(&packet).expect("an OER packet");
            panic!("refused {} {:?}", reject.code.as_str(), reject.message)
        });
        let accepted: Vec<_> = edge
            .entries()
            .into_iter()
            .filter_map(|entry| match entry {
                JournalEntry::InboundClaimAccepted {
                    channel_id,
                    cumulative_amount,
                    ..
                } => Some((channel_id, cumulative_amount)),
                _ => None,
            })
            .collect();
        assert_eq!(
            accepted,
            vec![(id.claim_key(), u128::from(PRICE))],
            "the allowance's watermark is journaled under its key"
        );

        // Resent, it advances nothing and pays for nothing.
        send(
            &mut session,
            btp_message(
                3,
                &[("payment-channel-claim", claim.as_bytes())],
                &sealed_prepare(&receiver).encode(),
            ),
        )
        .await;
        let (_, _, packet) = answer(&mut session).await;
        let reject = Reject::decode(&packet).expect("a REJECT");
        assert_eq!(reject.code.as_str(), "F03");
    }
}

// -- Subsidised allowances (ADR 0080 decision 14, #1581) --

/// Every journaled charge Δ, in order: a subsidised allowance's is the
/// budget's, not the note's.
fn subsidised_payer(edge: &Edge, seed: u64) -> Payer {
    Payer::new(edge, seed).subsidised(edge).provable(edge)
}

#[tokio::test]
async fn a_subsidised_allowance_is_granted_and_charges_the_note_nothing() {
    let edge = Edge::with_subsidy(3 * TIERS[0]);
    let gate = edge.gate();
    let payer = subsidised_payer(&edge, 50);

    let granted = grant(&gate, &payer).await;
    assert_eq!(granted.tier, TIERS[0]);
    let id = granted.allowance_id;
    gate.ingest(&payer.claim(id, 100), 100).await.unwrap();
    gate.ingest(&payer.claim(id, 400), 300).await.unwrap();

    // The successor carries the old balance unchanged: the note is charged 0.
    let successor = signed(gate.close_allowance(payer.close(id)).expect("closed"));
    payer.check(&edge, &successor, 0);
    assert_eq!(successor.commitment, payer.commitment);
    // Δ is journaled as the budget's.
    assert_eq!(edge.charges(), vec![(id.to_string(), 400)]);
    // The grant journals its period and per-period nullifier.
    let subsidy = edge.entries().into_iter().find_map(|entry| match entry {
        JournalEntry::AllowanceGranted { subsidy, .. } => subsidy,
        _ => None,
    });
    let subsidy = subsidy.expect("a subsidised grant journals its subsidy");
    assert_eq!(subsidy.period, edge.period());
    assert_eq!(
        subsidy.period_nullifier,
        field_to_le_bytes(payer.period_nullifier)
    );
    // And so does a restart.
    let restarted = edge.gate();
    assert_eq!(
        restarted.successor(id),
        Ok(SuccessorStatus::Signed(Box::new(successor)))
    );
    // The close record the challenger reads charges the note nothing.
    let charges: Vec<u64> = edge
        .evidence()
        .iter()
        .filter_map(|payload| match Record::decode(payload).unwrap() {
            Record::Close(close) => Some(close.charge),
            Record::Grant(_) => None,
        })
        .collect();
    assert_eq!(charges, vec![0]);
}

#[tokio::test]
async fn each_subsidy_refusal_has_its_own_name_and_writes_nothing() {
    let edge = Edge::with_subsidy(TIERS[0]);
    let gate = edge.gate();
    let period = edge.period();
    let base = |seed: u64| Payer::new(&edge, seed).subsidised(&edge);

    // A budget of exactly one tier: the first grant closes it.
    let first = base(60).provable(&edge);
    grant(&gate, &first).await;
    let (evidence, entries) = (edge.evidence().len(), edge.entries().len());

    let cases: Vec<(&str, Payer, &str)> = vec![
        (
            "a tier above the smallest",
            Payer {
                tier: TIERS[1],
                ..base(61)
            },
            "subsidy_tier_not_smallest",
        ),
        (
            "the previous period",
            Payer {
                period: period - 1,
                ..base(62)
            },
            "subsidy_period_not_current",
        ),
        (
            "the next period",
            Payer {
                period: period + 1,
                ..base(63)
            },
            "subsidy_period_not_current",
        ),
        ("a closed budget", base(64), "subsidy_budget_closed"),
        (
            "a reused per-period nullifier, while the budget is closed",
            Payer {
                period_nullifier: first.period_nullifier,
                ..base(65)
            },
            "subsidy_budget_closed",
        ),
    ];
    for (case, payer, name) in cases {
        let payer = payer.provable(&edge);
        let refusal = gate.admit_allowance(payer.request()).await.expect_err(case);
        assert_eq!(refusal.name(), name, "{case}: {refusal}");
    }
    assert_eq!(edge.evidence().len(), evidence, "nothing was written");
    assert_eq!(edge.entries().len(), entries, "nothing was journaled");
}

/// zk_toon binds a subsidised proof's period to its own `requestTime`
/// (`RequestPublic::check_subsidy_period`), which the circuit cannot: it
/// takes the vault's creation time as a private input. Just past a period
/// boundary a fresh request can still be from the last period; a proof that
/// claims the edge's current period for it was proven with another creation
/// time, and is refused as not current.
#[tokio::test]
async fn a_subsidy_period_must_be_the_request_times_not_just_the_edges() {
    let edge = Edge::with_subsidy(10 * TIERS[0]);
    let creation = edge.vault_creation_ms();
    // One minute into the next period, by the edge's clock.
    let boundary = creation + (edge.period() + 1) * SUBSIDY_PERIOD_MS;
    edge.clock.advance(TimeDelta::milliseconds(
        i64::try_from(boundary + 60_000 - edge.now_ms()).unwrap(),
    ));
    let current = edge.period();
    // Five minutes ago, inside the freshness window, and in the last period.
    let request_time = edge.now_ms() - 5 * 60_000;
    assert_eq!(subsidy_period(creation, request_time), Some(current - 1));

    let forged = Payer {
        request_time,
        period: current,
        ..Payer::new(&edge, 80).subsidised(&edge)
    }
    .provable(&edge);
    let gate = edge.gate();
    let (evidence, entries) = (edge.evidence().len(), edge.entries().len());
    let refusal = gate
        .admit_allowance(forged.request())
        .await
        .expect_err("a period its request time is not in");
    assert_eq!(refusal.name(), "subsidy_period_not_current", "{refusal}");
    assert_eq!(edge.evidence().len(), evidence, "nothing was written");
    assert_eq!(edge.entries().len(), entries, "nothing was journaled");

    // The same request, proven for the period its request time is in, is
    // the edge's last period, not its current one: refused too.
    let last = Payer {
        request_time,
        period: current - 1,
        ..Payer::new(&edge, 81).subsidised(&edge)
    }
    .provable(&edge);
    let refusal = gate.admit_allowance(last.request()).await.unwrap_err();
    assert_eq!(refusal.name(), "subsidy_period_not_current", "{refusal}");

    // A request made in the current period is granted.
    grant(
        &gate,
        &Payer::new(&edge, 82).subsidised(&edge).provable(&edge),
    )
    .await;
}

#[tokio::test]
async fn a_per_period_nullifier_is_seen_once_even_after_a_restart() {
    let edge = Edge::with_subsidy(10 * TIERS[0]);
    let first = subsidised_payer(&edge, 70);
    grant(&edge.gate(), &first).await;

    // Another note, the same per-period nullifier: refused, before and
    // after a restart, by the per-period rule and not the state's.
    let reused = Payer {
        period_nullifier: first.period_nullifier,
        ..Payer::new(&edge, 71).subsidised(&edge)
    }
    .provable(&edge);
    let (evidence, entries) = (edge.evidence().len(), edge.entries().len());
    for gate in [edge.gate(), edge.gate()] {
        let refusal = gate.admit_allowance(reused.request()).await.unwrap_err();
        assert_eq!(refusal, AllowanceRefusal::SubsidyNullifierSpent);
        assert_eq!(refusal.name(), "subsidy_nullifier_spent");
    }
    assert_eq!(edge.evidence().len(), evidence);
    assert_eq!(edge.entries().len(), entries);
}

#[tokio::test]
async fn reservations_cannot_overspend_the_budget_and_a_remainder_reopens_it() {
    // One and a half tiers: one reservation fits, two do not.
    let edge = Edge::with_subsidy(TIERS[0] + TIERS[0] / 2);
    let gate = edge.gate();
    let (a, b) = (subsidised_payer(&edge, 80), subsidised_payer(&edge, 81));
    assert_eq!(gate.subsidy_open(), Some(true));
    let id = grant(&gate, &a).await.allowance_id;
    assert_eq!(gate.subsidy_open(), Some(false));
    assert_eq!(
        gate.admit_allowance(b.request()).await.unwrap_err(),
        AllowanceRefusal::SubsidyBudgetClosed
    );

    // The first closes having used a quarter of its tier: its remainder
    // reopens the budget in the same period.
    gate.ingest(&a.claim(id, TIERS[0] / 4), TIERS[0] / 4)
        .await
        .unwrap();
    gate.close_allowance(a.close(id)).expect("closed");
    assert_eq!(gate.subsidy_open(), Some(true));
    grant(&gate, &b).await;
    assert_eq!(gate.subsidy_open(), Some(false));

    // None of it is forgotten by a restart.
    let restarted = edge.gate();
    assert_eq!(restarted.subsidy_open(), Some(false));
}

#[tokio::test]
async fn a_remainder_released_after_its_period_ended_does_not_raise_the_next_budget() {
    let edge = Edge::with_subsidy(TIERS[0]);
    let gate = edge.gate();
    let old = subsidised_payer(&edge, 90);
    grant(&gate, &old).await;
    assert_eq!(gate.subsidy_open(), Some(false));

    // The period ends with the allowance unclosed; the sweep closes it, and
    // its whole reservation returns to a period that is over.
    edge.advance_ms(SUBSIDY_PERIOD_MS);
    gate.sweep_allowances();
    assert_eq!(edge.charges().len(), 1);

    // The new period has the budget, and no more than the budget.
    assert_eq!(gate.subsidy_open(), Some(true));
    grant(&gate, &subsidised_payer(&edge, 91)).await;
    assert_eq!(gate.subsidy_open(), Some(false));
    assert_eq!(
        gate.admit_allowance(subsidised_payer(&edge, 92).request())
            .await
            .unwrap_err(),
        AllowanceRefusal::SubsidyBudgetClosed
    );
    // The old period's proof is refused by its period, whatever its budget.
    let stale = Payer {
        request_time: edge.now_ms(),
        expires_at_ms: edge.now_ms() + HOUR_MS / 2,
        ..old
    }
    .provable(&edge);
    assert_eq!(
        gate.admit_allowance(stale.request())
            .await
            .unwrap_err()
            .name(),
        "subsidy_period_not_current"
    );
}

#[tokio::test]
async fn a_subsidised_allowance_counts_toward_the_open_budget_cap() {
    let edge = Edge {
        subsidy_budget: 5 * TIERS[0],
        ..Edge::with_cap(TIERS[0])
    };
    let gate = edge.gate();
    grant(&gate, &subsidised_payer(&edge, 100)).await;
    let refusal = gate
        .admit_allowance(subsidised_payer(&edge, 101).request())
        .await
        .unwrap_err();
    assert!(
        matches!(refusal, AllowanceRefusal::OpenBudgetExceeded { .. }),
        "{refusal:?}"
    );
}

#[tokio::test]
async fn the_greeting_reads_open_and_closed_and_says_nothing_without_a_budget() {
    assert_eq!(Edge::new().gate().subsidy_open(), None);
    let edge = Edge::with_subsidy(TIERS[0]);
    let gate = edge.gate();
    assert_eq!(gate.subsidy_open(), Some(true));
    let payer = subsidised_payer(&edge, 110);
    let id = grant(&gate, &payer).await.allowance_id;
    assert_eq!(gate.subsidy_open(), Some(false));
    gate.close_allowance(payer.close(id)).expect("closed");
    assert_eq!(
        gate.subsidy_open(),
        Some(true),
        "nothing used, all returned"
    );
    // A new period reopens a budget that was spent.
    grant(&gate, &subsidised_payer(&edge, 111)).await;
    assert_eq!(gate.subsidy_open(), Some(false));
    edge.advance_ms(SUBSIDY_PERIOD_MS);
    assert_eq!(gate.subsidy_open(), Some(true));
}

#[tokio::test]
async fn the_self_description_counts_the_last_finished_period_only() {
    assert_eq!(Edge::new().gate().subsidised_last_period(), None);
    let edge = Edge::with_subsidy(5 * TIERS[0]);
    let gate = edge.gate();
    grant(&gate, &subsidised_payer(&edge, 120)).await;
    grant(&gate, &subsidised_payer(&edge, 121)).await;
    // Two are granted in the current period, which is not finished.
    assert_eq!(gate.subsidised_last_period(), Some(0));
    edge.advance_ms(SUBSIDY_PERIOD_MS);
    assert_eq!(gate.subsidised_last_period(), Some(2));
    grant(&gate, &subsidised_payer(&edge, 122)).await;
    assert_eq!(
        gate.subsidised_last_period(),
        Some(2),
        "the new period's grant is not counted until it ends"
    );
    // The count is rebuilt from the journal.
    assert_eq!(edge.gate().subsidised_last_period(), Some(2));
    edge.advance_ms(SUBSIDY_PERIOD_MS);
    assert_eq!(gate.subsidised_last_period(), Some(1));
}

#[tokio::test]
async fn the_vaults_creation_time_is_read_once_and_only_for_a_budget() {
    let vault = InMemoryZkNoteVault::new();
    vault.created_at(1_234);
    assert_eq!(Subsidy::read(&vault, 0).await, Ok(None));
    assert_eq!(vault.creation_reads(), 0, "a zero budget reads nothing");
    assert_eq!(
        Subsidy::read(&vault, 9).await,
        Ok(Some(Subsidy {
            budget: 9,
            vault_creation_ms: 1_234
        }))
    );
    assert_eq!(vault.creation_reads(), 1);
    vault.go_down();
    assert!(Subsidy::read(&vault, 9).await.is_err(), "boot fails");
    assert_eq!(Subsidy::read(&vault, 0).await, Ok(None));
}
