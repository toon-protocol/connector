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
    AllowanceGrant, AllowanceRefusal, AllowanceRequest, ClaimIngestRejection, ClientClaimGate,
    EvidenceFile, ZkEdge, EVIDENCE_FILE,
};
use connector_domain::client_claim::{AllowanceId, ClientEdgeClaim};
use connector_domain::JournalEntry;
use connector_runtime::{Clock, FileJournal, Journal, TestClock};
use connector_zk_note::evidence::{evidence_hash, scan, Record};
use connector_zk_note::note::balance::generator_g;
use connector_zk_note::note::policy::TIERS;
use connector_zk_note::request::{expected_tag, json_proof_for, RequestProof, RequestPublic};
use connector_zk_note::session::{sign_allowance_claim, signature_to_hex};
use connector_zk_note::tiers::Tiers;
use connector_zk_note::{field_to_le_bytes, CompressedPubKey, Field, Keypair, Scalar, SecKey};

const HOUR_MS: u64 = 3_600_000;

fn keypair(seed: u64) -> Keypair {
    Keypair::try_from(SecKey::new(Scalar::from(seed))).expect("a keypair")
}

/// The zk edge under test: its state key, its clock, its vault, and the
/// directory its journal and evidence file live in.
struct Edge {
    state: Keypair,
    clock: Arc<TestClock>,
    vault: Arc<InMemoryZkNoteVault>,
    dir: tempfile::TempDir,
    cap: u64,
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
        }
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
        ClientClaimGate::restore(self.journal())
            .expect("replays")
            .with_zk_edge(ZkEdge {
                vault: Arc::clone(&self.vault) as Arc<dyn ZkNoteVault>,
                evidence: EvidenceFile::open(&self.dir.path().join(EVIDENCE_FILE))
                    .expect("evidence"),
                state_key: self.state_key(),
                tiers: Tiers::new([TIERS[0], TIERS[1]]).expect("tiers"),
                allowance_lifetime: std::time::Duration::from_millis(HOUR_MS),
                open_budget_cap: self.cap,
                clock: Arc::clone(&self.clock) as Arc<dyn Clock>,
            })
            .expect("the evidence holds every grant")
    }

    fn entries(&self) -> Vec<JournalEntry> {
        self.journal().read_all().expect("journal")
    }

    fn evidence(&self) -> Vec<Vec<u8>> {
        scan(&std::fs::read(self.dir.path().join(EVIDENCE_FILE)).expect("evidence"))
            .expect("evidence")
            .records
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
    period: Field,
    period_nullifier: Field,
    /// The state key the proof is about; the edge's own unless spoiled.
    vault: CompressedPubKey,
    /// What the tag is bound to, if not this payer's own terms.
    tag_override: Option<Field>,
    proof_bytes: &'static str,
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
            period: Field::from(0u64),
            period_nullifier: Field::from(0u64),
            vault: edge.state_key(),
            tag_override: None,
            proof_bytes: "proof",
        }
    }

    fn public(&self) -> RequestPublic {
        let session = self.session.public.into_compressed();
        RequestPublic {
            vault: self.vault.clone(),
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
            commitment: generator_g(),
            subsidised: self.subsidised,
            period: self.period,
            period_nullifier: self.period_nullifier,
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
    assert_eq!(first.channel_key(), granted.allowance_id.channel_key());
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
    let Record::Grant(record) = Record::from_payload(&records[0]).expect("a record") else {
        panic!("a grant record");
    };
    assert_eq!(record.proof_json, payer.request().proof);
    assert_eq!(
        record.allowance_id,
        granted.allowance_id.as_bytes().to_vec()
    );
    assert_eq!(record.session_key, payer.session.public.into_compressed());
    assert_eq!(record.expiry, payer.expires_at_ms);
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
            "a subsidised proof",
            Payer {
                subsidised: true,
                ..base.clone()
            }
            .request(),
            |r| matches!(r, AllowanceRefusal::Subsidised),
        ),
        (
            "a period",
            Payer {
                period: Field::from(3u64),
                ..base.clone()
            }
            .request(),
            |r| matches!(r, AllowanceRefusal::Subsidised),
        ),
        (
            "a per-period nullifier",
            Payer {
                period_nullifier: Field::from(3u64),
                ..base.clone()
            }
            .request(),
            |r| matches!(r, AllowanceRefusal::Subsidised),
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
