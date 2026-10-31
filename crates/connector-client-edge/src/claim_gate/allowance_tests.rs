//! The allowance half of the gate's own tests: what needs `roll_back`, which
//! is the crate's, not the gate's public surface.

use super::*;
use crate::zk_note::InMemoryZkNoteVault;
use chrono::TimeZone;
use connector_runtime::{FileJournal, TestClock};
use connector_zk_note::note::balance::commit;
use connector_zk_note::note::policy::TIERS;
use connector_zk_note::request::{json_proof_for, Point, RequestPublic};
use connector_zk_note::session::{sign_allowance_claim, signature_to_hex};
use connector_zk_note::{Keypair, Scalar, SecKey};

fn keypair(seed: u64) -> Keypair {
    Keypair::try_from(SecKey::new(Scalar::from(seed))).expect("a keypair")
}

fn edge(dir: &std::path::Path, vault: Arc<InMemoryZkNoteVault>, clock: Arc<dyn Clock>) -> ZkEdge {
    ZkEdge {
        vault,
        evidence: EvidenceFile::open(&dir.join("evidence.bin")).unwrap(),
        state_signer: connector_signer::ZkStateSigner::from_base58(
            &SecKey::new(Scalar::from(500u64)).to_base58(),
        )
        .unwrap(),
        tiers: Tiers::new([TIERS[1]]).unwrap(),
        allowance_lifetime: Duration::from_secs(3600),
        open_budget_cap: TIERS[1],
        subsidy: None,
        clock,
    }
}

/// A zk edge over a real journal, with one allowance granted to `session`.
async fn edge_with_allowance(
    dir: &std::path::Path,
    session: &Keypair,
) -> (ClientClaimGate, AllowanceId) {
    let state = keypair(500).public.into_compressed();
    let clock = Arc::new(TestClock::new(
        chrono::Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap(),
    ));
    let now = u64::try_from(clock.now().timestamp_millis()).unwrap();
    let vault = Arc::new(InMemoryZkNoteVault::new());
    let journal = Arc::new(FileJournal::open(dir.join("claims.log")).unwrap());
    let gate = ClientClaimGate::restore(journal)
        .unwrap()
        .with_zk_edge(edge(dir, Arc::clone(&vault), clock))
        .unwrap();
    let session_key = session.public.into_compressed();
    let expires = now + 1_000;
    let nullifier = Field::from(77u64);
    let commitment = commit(Scalar::from(5_000_000_000u64), Scalar::from(17u64));
    let public = RequestPublic {
        vault_x: state.x,
        vault_is_odd: state.is_odd,
        root: Field::from(1u64),
        request_time: now,
        tier: TIERS[1],
        nullifier,
        tag: expected_tag(nullifier, &state, &session_key, TIERS[1], expires),
        commitment: Point {
            x: commitment.x,
            y: commitment.y,
        },
        subsidised: false,
        period: 0,
        period_nullifier: Field::from(0u64),
    };
    let json = json_proof_for(&public, "p");
    vault.hold_root(public.root);
    vault.accept(&RequestProof::parse(&json).unwrap());
    let grant = gate
        .admit_allowance(AllowanceRequest {
            proof: json,
            session_key,
            expires_at_ms: expires,
        })
        .await
        .expect("granted");
    (gate, grant.allowance_id)
}

fn claim(session: &Keypair, id: AllowanceId, amount: u64) -> String {
    let signature = signature_to_hex(&sign_allowance_claim(session, id.as_bytes(), amount));
    format!(
        r#"{{"version":"1.0","scheme":"zk-note","allowanceId":"{id}","cumulativeAmount":"{amount}","signature":"{signature}"}}"#
    )
}

fn at(amount: u128) -> Option<Watermark> {
    Some(Watermark {
        cumulative_amount: amount,
    })
}

/// ADR 0080 decision 2: a reject from the next hop rolls an allowance's
/// watermark back exactly as a voucher's, durably -- so the payer is not
/// charged for a packet that was never carried, before or after a restart.
#[tokio::test]
async fn a_reject_rolls_an_allowance_claim_back_as_it_does_a_voucher() {
    let dir = tempfile::tempdir().unwrap();
    let session = keypair(9);
    let (gate, id) = edge_with_allowance(dir.path(), &session).await;
    let key = id.claim_key();
    gate.ingest(&claim(&session, id, 100), 100).await.unwrap();
    gate.ingest(&claim(&session, id, 200), 100).await.unwrap();

    gate.roll_back(&key, 200).await.expect("rolled back");
    assert_eq!(gate.watermark(&key), at(100));
    // The same claim is good again, for the packet that is carried.
    gate.ingest(&claim(&session, id, 200), 100)
        .await
        .expect("the rolled-back claim is good again");
    gate.roll_back(&key, 200).await.unwrap();
    drop(gate);

    // The journal agrees, and boots: an allowance's watermark is no retired
    // channel's.
    let journal = Arc::new(FileJournal::open(dir.path().join("claims.log")).unwrap());
    assert_eq!(
        crate::batch_settlement::first_toon_channel_entry(&journal.read_all().unwrap()),
        None
    );
    let restarted = ClientClaimGate::restore(journal).unwrap();
    assert_eq!(restarted.watermark(&key), at(100));
}

#[tokio::test]
async fn rolling_back_an_allowances_first_claim_empties_it() {
    let dir = tempfile::tempdir().unwrap();
    let session = keypair(10);
    let (gate, id) = edge_with_allowance(dir.path(), &session).await;
    gate.ingest(&claim(&session, id, 100), 100).await.unwrap();
    gate.roll_back(&id.claim_key(), 100).await.unwrap();
    assert_eq!(gate.watermark(&id.claim_key()), None);
}

#[tokio::test]
async fn a_lost_evidence_file_refuses_the_zk_edge_at_boot() {
    let dir = tempfile::tempdir().unwrap();
    let (gate, id) = edge_with_allowance(dir.path(), &keypair(9)).await;
    drop(gate);
    std::fs::remove_file(dir.path().join("evidence.bin")).unwrap();
    let journal = Arc::new(FileJournal::open(dir.path().join("claims.log")).unwrap());
    let error = ClientClaimGate::restore(journal)
        .unwrap()
        .with_zk_edge(edge(
            dir.path(),
            Arc::new(InMemoryZkNoteVault::new()),
            Arc::new(connector_runtime::SystemClock),
        ))
        .err()
        .expect("refused");
    assert_eq!(error.allowance_id, id.to_string());
}

fn close(session: &Keypair, id: AllowanceId) -> AllowanceClose {
    AllowanceClose {
        allowance_id: id,
        signature: connector_zk_note::session::signature_to_be_bytes(
            &connector_zk_note::session::sign_allowance_close(session, id.as_bytes()),
        ),
    }
}

/// ADR 0080 decisions 2 and 4: a close waits for forwards in flight, and a
/// forward the next hop rejected while the close waited is rolled back
/// before the charge is read -- so Δ is the final watermark, and the payer
/// pays nothing for the packet that was never carried.
#[tokio::test]
async fn a_forward_rejected_while_the_close_waits_is_not_charged() {
    let dir = tempfile::tempdir().unwrap();
    let session = keypair(11);
    let (gate, id) = edge_with_allowance(dir.path(), &session).await;
    let (_, carried) = gate
        .ingest_forward(&claim(&session, id, 100), 100)
        .await
        .unwrap();
    let (_, rejected) = gate
        .ingest_forward(&claim(&session, id, 250), 150)
        .await
        .unwrap();
    assert_eq!(
        gate.close_allowance(close(&session, id)),
        Ok(SuccessorStatus::Pending)
    );

    // The next hop rejects the second packet: rolled back, then resolved.
    gate.roll_back(&id.claim_key(), 250).await.unwrap();
    drop(rejected);
    assert_eq!(gate.successor(id), Ok(SuccessorStatus::Pending));
    drop(carried);
    let SuccessorStatus::Signed(successor) = gate.successor(id).unwrap() else {
        panic!("signed once nothing is in flight");
    };
    assert_eq!(successor.charge, 100);

    // A restart charges it the same, once.
    drop(gate);
    let journal = Arc::new(FileJournal::open(dir.path().join("claims.log")).unwrap());
    let charges: Vec<_> = journal
        .read_all()
        .unwrap()
        .into_iter()
        .filter(|entry| matches!(entry, JournalEntry::AllowanceClosed { .. }))
        .collect();
    assert_eq!(
        charges,
        vec![JournalEntry::AllowanceClosed {
            allowance_id: id.to_string(),
            charge: 100
        }]
    );
}
