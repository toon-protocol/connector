//! zk_toon's `test-vectors/zk-evidence.json`, replayed through the zk edge's evidence file
//! (ADR 0080 decisions 11 and 12): the records it holds, encoded by `zk-evidence` and written
//! by [`EvidenceFile`], are byte for byte the file zk_toon's challenger reads, and a file it
//! calls torn or corrupt is cut back or refused at open exactly as it says.
//!
//! zk_toon exports no constant for this file, so a copy is committed under `tests/fixtures/`
//! and its SHA-256 is pinned: refresh it, from the same zk_toon commit as `connector-zk-note`'s
//! pin, whenever that pin moves.

use connector_client_edge::{EvidenceFile, EvidenceFileError};
use connector_zk_note::evidence::{Close, Grant, Record};
use connector_zk_note::Field;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::str::FromStr;

const VECTORS: &str = include_str!("fixtures/zk-evidence.json");

/// SHA-256 of `test-vectors/zk-evidence.json` at zk_toon `0d6b10c3`.
const VECTORS_SHA256: &str = "a6d56a9437cc6bd8095bab8e3e3252249bc09b5f8253c01f74e63fdd92e06872";

fn vectors() -> Value {
    serde_json::from_str(VECTORS).expect("zk_toon's evidence vectors are JSON")
}

fn s(v: &Value) -> &str {
    v.as_str().expect("a string")
}

fn u(v: &Value) -> u64 {
    s(v).parse().expect("a u64")
}

fn bytes(v: &Value) -> Vec<u8> {
    hex::decode(s(v)).expect("hex")
}

fn record(e: &Value) -> Record {
    match s(&e["kind"]) {
        "grant" => Record::Grant(Grant {
            proof: s(&e["proof"]).as_bytes().to_vec(),
            allowance_id: bytes(&e["allowanceId"]),
            session_key_x: Field::from_str(s(&e["sessionKeyX"])).expect("a field"),
            session_key_is_odd: e["sessionKeyIsOdd"].as_bool().expect("a bool"),
            expires_at: u(&e["expiresAt"]),
            granted_at: u(&e["grantedAt"]),
        }),
        "close" => Record::Close(Close {
            nullifier: Field::from_str(s(&e["nullifier"])).expect("a field"),
            allowance_id: bytes(&e["allowanceId"]),
            charge: u(&e["charge"]),
        }),
        other => panic!("an evidence record of kind {other}"),
    }
}

fn path(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join(connector_client_edge::EVIDENCE_FILE)
}

#[test]
fn the_vectors_are_the_ones_this_pin_was_reviewed_against() {
    assert_eq!(
        hex::encode(Sha256::digest(VECTORS.as_bytes())),
        VECTORS_SHA256
    );
}

#[test]
fn the_zk_edge_writes_byte_for_byte_the_file_zk_toon_reads() {
    let v = vectors();
    let dir = tempfile::tempdir().unwrap();
    let file = EvidenceFile::open(&path(&dir)).expect("a new evidence file");
    let records = v["file"]["records"].as_array().expect("records");
    assert!(records.len() > 1);
    for e in records {
        let payload = record(e).encode().expect("encodes");
        assert_eq!(hex::encode(&payload), s(&e["payloadHex"]));
        let hash = file.append(&payload).expect("appended");
        assert_eq!(hex::encode(hash), s(&e["evidenceHash"]));
        assert!(file.holds(&hash));
    }
    drop(file);
    assert_eq!(
        hex::encode(std::fs::read(path(&dir)).unwrap()),
        s(&v["file"]["hex"])
    );
}

#[test]
fn every_record_zk_toon_pins_encodes_to_its_bytes() {
    let v = vectors();
    for name in ["grant", "grantMaxAllowanceId", "close"] {
        let e = &v[name];
        let payload = record(e).encode().expect("encodes");
        assert_eq!(hex::encode(&payload), s(&e["payloadHex"]), "{name}");
        assert_eq!(Record::decode(&payload), Ok(record(e)), "{name}");
    }
}

#[test]
fn a_file_zk_toon_calls_whole_reopens_holding_every_record() {
    let v = vectors();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(path(&dir), bytes(&v["file"]["hex"])).unwrap();
    let file = EvidenceFile::open(&path(&dir)).expect("opens");
    for e in v["file"]["records"].as_array().unwrap() {
        let hash: [u8; 32] = bytes(&e["evidenceHash"]).try_into().unwrap();
        assert!(file.holds(&hash));
    }
}

#[test]
fn a_file_zk_toon_calls_torn_is_cut_back_to_its_valid_length() {
    let v = vectors();
    let torn = &v["tornTail"];
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(path(&dir), bytes(&torn["hex"])).unwrap();
    let file = EvidenceFile::open(&path(&dir)).expect("a torn tail is not corruption");
    let kept = torn["records"].as_array().unwrap();
    for e in kept {
        let hash: [u8; 32] = bytes(&e["evidenceHash"]).try_into().unwrap();
        assert!(file.holds(&hash));
    }
    drop(file);
    let valid_len = usize::try_from(torn["validLen"].as_u64().unwrap()).unwrap();
    assert_eq!(
        std::fs::metadata(path(&dir)).unwrap().len() as usize,
        valid_len
    );
}

#[test]
fn a_file_zk_toon_calls_corrupt_or_foreign_is_refused_at_open() {
    let v = vectors();
    for name in ["corruptMidFile", "badMagic", "unknownVersion"] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(&dir), bytes(&v[name]["hex"])).unwrap();
        assert!(
            matches!(
                EvidenceFile::open(&path(&dir)),
                Err(EvidenceFileError::Format(_))
            ),
            "{name}"
        );
    }
}
