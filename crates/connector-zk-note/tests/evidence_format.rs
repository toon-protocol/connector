//! The evidence file's bytes, pinned against zk_toon#29's brief (the format is zk_toon's, ADR 0080
//! decision 12). Every expected value below is spelled out from the brief, not recomputed by the
//! code under test. When zk_toon's `zk-evidence` crate and its `test-vectors/zk-evidence.json`
//! land, this file is replaced by a replay of those vectors.

use connector_zk_note::evidence::{
    crc32c, evidence_hash, frame, header, scan, CloseRecord, EvidenceError, GrantRecord, Record,
};
use connector_zk_note::{CompressedPubKey, Field};
use sha2::{Digest, Sha256};

fn grant() -> GrantRecord {
    GrantRecord {
        proof_json: br#"{"publicInput":[]}"#.to_vec(),
        allowance_id: vec![0xa1, 0xa2],
        session_key: CompressedPubKey {
            x: Field::from(0x0102u64),
            is_odd: true,
        },
        expiry: 0x0807_0605_0403_0201,
        granted_at: 5,
    }
}

/// The grant above, byte by byte from the brief: kind `1`, the `JsonProof` with a `u32` LE
/// length, the allowance id with a `u8` length, the session key's x (32 bytes LE) and its
/// y-is-odd byte, the expiry and the grant time as `u64` LE.
fn grant_payload() -> Vec<u8> {
    let json = br#"{"publicInput":[]}"#;
    let mut p = vec![1u8];
    p.extend_from_slice(&(json.len() as u32).to_le_bytes());
    p.extend_from_slice(json);
    p.extend_from_slice(&[2, 0xa1, 0xa2]);
    let mut x = [0u8; 32];
    x[0] = 0x02;
    x[1] = 0x01;
    p.extend_from_slice(&x);
    p.push(1);
    p.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    p.extend_from_slice(&[5, 0, 0, 0, 0, 0, 0, 0]);
    p
}

#[test]
fn the_header_is_the_magic_and_version_one() {
    assert_eq!(header(), *b"TOONZKEV\x01\x00");
}

#[test]
fn records_are_checksummed_with_crc32c() {
    // The Castagnoli check value.
    assert_eq!(crc32c(b"123456789"), 0xe306_9283);
}

#[test]
fn a_grant_record_encodes_as_the_brief_spells_it_and_reads_back() {
    let record = Record::Grant(grant());
    let payload = record.to_payload().expect("encodes");
    assert_eq!(payload, grant_payload());
    assert_eq!(Record::from_payload(&payload), Ok(record));
}

#[test]
fn a_close_record_encodes_as_the_brief_spells_it_and_reads_back() {
    let record = Record::Close(CloseRecord {
        nullifier: Field::from(9u64),
        allowance_id: vec![0xbb],
        charge: 300,
    });
    let mut nullifier = [0u8; 32];
    nullifier[0] = 9;
    let mut expected = vec![2u8];
    expected.extend_from_slice(&nullifier);
    expected.extend_from_slice(&[1, 0xbb]);
    expected.extend_from_slice(&300u64.to_le_bytes());
    let payload = record.to_payload().expect("encodes");
    assert_eq!(payload, expected);
    assert_eq!(Record::from_payload(&payload), Ok(record));
}

#[test]
fn a_record_is_framed_by_length_and_crc_and_hashed_over_its_payload_only() {
    let payload = grant_payload();
    let mut expected = (payload.len() as u32).to_le_bytes().to_vec();
    expected.extend_from_slice(&payload);
    expected.extend_from_slice(&crc32c(&payload).to_le_bytes());
    assert_eq!(frame(&payload), expected);
    let digest: [u8; 32] = Sha256::digest(&payload).into();
    assert_eq!(evidence_hash(&payload), digest);
}

fn file_of(payloads: &[Vec<u8>]) -> Vec<u8> {
    let mut file = header().to_vec();
    for p in payloads {
        file.extend_from_slice(&frame(p));
    }
    file
}

#[test]
fn a_file_reads_back_exactly_its_records() {
    let a = grant_payload();
    let b = Record::Close(CloseRecord {
        nullifier: Field::from(1u64),
        allowance_id: vec![1],
        charge: 0,
    })
    .to_payload()
    .unwrap();
    let file = file_of(&[a.clone(), b.clone()]);
    let read = scan(&file).expect("reads");
    assert_eq!(read.records, vec![a, b]);
    assert_eq!(read.end, file.len());
    assert!(!read.torn);
}

#[test]
fn a_torn_final_record_is_ignored_and_the_end_is_before_it() {
    let a = grant_payload();
    let whole = file_of(std::slice::from_ref(&a));
    for cut in 1..frame(&a).len() {
        let mut file = file_of(&[a.clone(), a.clone()]);
        file.truncate(whole.len() + cut);
        let read = scan(&file).expect("a torn tail is not corruption");
        assert_eq!(read.records, vec![a.clone()], "cut at {cut}");
        assert_eq!(read.end, whole.len());
        assert!(read.torn);
    }
    // A final record whose CRC fails is torn too.
    let mut file = file_of(&[a.clone(), a.clone()]);
    let last = file.len() - 1;
    file[last] ^= 0xff;
    let read = scan(&file).expect("torn");
    assert_eq!(read.records, vec![a]);
    assert!(read.torn);
}

#[test]
fn a_crc_failure_before_the_end_is_corruption() {
    let a = grant_payload();
    let mut file = file_of(&[a.clone(), a]);
    file[header().len() + 4] ^= 0xff;
    assert!(matches!(scan(&file), Err(EvidenceError::Corrupt { .. })));
}

#[test]
fn an_unknown_version_or_magic_is_refused() {
    let mut file = header().to_vec();
    file[8] = 2;
    assert_eq!(scan(&file), Err(EvidenceError::UnknownVersion(2)));
    let mut file = header().to_vec();
    file[0] = b'X';
    assert_eq!(scan(&file), Err(EvidenceError::BadMagic));
}

#[test]
fn an_empty_or_half_written_header_is_a_torn_start() {
    for len in 0..header().len() {
        let read = scan(&header()[..len]).expect("torn header");
        assert!(read.records.is_empty());
        assert_eq!(read.end, 0);
    }
}

#[test]
fn a_non_canonical_field_element_is_refused() {
    let mut payload = vec![2u8];
    payload.extend_from_slice(&[0xff; 32]);
    payload.extend_from_slice(&[1, 0xbb]);
    payload.extend_from_slice(&0u64.to_le_bytes());
    assert_eq!(
        Record::from_payload(&payload),
        Err(EvidenceError::NonCanonicalField)
    );
}

#[test]
fn an_allowance_id_over_64_bytes_is_refused() {
    let mut long = grant();
    long.allowance_id = vec![0; 65];
    assert_eq!(
        Record::Grant(long).to_payload(),
        Err(EvidenceError::AllowanceIdTooLong(65))
    );
}
