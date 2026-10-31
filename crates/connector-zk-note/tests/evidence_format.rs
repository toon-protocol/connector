//! The evidence file's bytes, spelled out byte by byte as the connector wrote them before it
//! encoded with zk_toon's `zk-evidence` (#1513's stand-in, written to zk_toon#29's brief). Every
//! expected value is from that brief, not recomputed by the code under test, so these tests are
//! the promise that an evidence file a zk edge has already written still reads back under
//! `zk-evidence` -- the format is zk_toon's (ADR 0080 decision 12), and zk_toon's own vectors are
//! replayed through the connector's file writer in `connector-client-edge`.

use connector_zk_note::evidence::{
    evidence_hash, frame, scan, write_header, Close, DecodeError, FileError, Grant, Record,
};
use connector_zk_note::Field;
use sha2::{Digest, Sha256};

fn grant() -> Grant {
    Grant {
        proof: br#"{"publicInput":[]}"#.to_vec(),
        allowance_id: vec![0xa1, 0xa2],
        session_key_x: Field::from(0x0102u64),
        session_key_is_odd: true,
        expires_at: 0x0807_0605_0403_0201,
        granted_at: 5,
    }
}

/// CRC-32C (Castagnoli, reflected polynomial `0x82F63B78`), bit by bit: the framing's checksum,
/// computed independently of the code under test.
fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0x82F6_3B78 & mask);
        }
    }
    !crc
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
    assert_eq!(write_header(), b"TOONZKEV\x01\x00".to_vec());
}

#[test]
fn records_are_checksummed_with_crc32c() {
    // The Castagnoli check value.
    assert_eq!(crc32c(b"123456789"), 0xe306_9283);
}

#[test]
fn a_grant_record_encodes_as_the_brief_spells_it_and_reads_back() {
    let record = Record::Grant(grant());
    let payload = record.encode().expect("encodes");
    assert_eq!(payload, grant_payload());
    assert_eq!(Record::decode(&payload), Ok(record));
}

#[test]
fn a_close_record_encodes_as_the_brief_spells_it_and_reads_back() {
    let record = Record::Close(Close {
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
    let payload = record.encode().expect("encodes");
    assert_eq!(payload, expected);
    assert_eq!(Record::decode(&payload), Ok(record));
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

fn payloads(scan: &connector_zk_note::evidence::Scan<'_>) -> Vec<Vec<u8>> {
    scan.records.iter().map(|r| r.payload.to_vec()).collect()
}

fn file_of(payloads: &[Vec<u8>]) -> Vec<u8> {
    let mut file = write_header();
    for p in payloads {
        file.extend_from_slice(&frame(p));
    }
    file
}

#[test]
fn a_file_reads_back_exactly_its_records() {
    let a = grant_payload();
    let b = Record::Close(Close {
        nullifier: Field::from(1u64),
        allowance_id: vec![1],
        charge: 0,
    })
    .encode()
    .unwrap();
    let file = file_of(&[a.clone(), b.clone()]);
    let read = scan(&file).expect("reads");
    assert_eq!(payloads(&read), vec![a, b]);
    assert_eq!(read.valid_len, file.len());
    assert!(!read.torn_tail);
}

#[test]
fn a_torn_final_record_is_ignored_and_the_end_is_before_it() {
    let a = grant_payload();
    let whole = file_of(std::slice::from_ref(&a));
    for cut in 1..frame(&a).len() {
        let mut file = file_of(&[a.clone(), a.clone()]);
        file.truncate(whole.len() + cut);
        let read = scan(&file).expect("a torn tail is not corruption");
        assert_eq!(payloads(&read), vec![a.clone()], "cut at {cut}");
        assert_eq!(read.valid_len, whole.len());
        assert!(read.torn_tail);
    }
    // A final record whose CRC fails is torn too.
    let mut file = file_of(&[a.clone(), a.clone()]);
    let last = file.len() - 1;
    file[last] ^= 0xff;
    let read = scan(&file).expect("torn");
    assert_eq!(payloads(&read), vec![a]);
    assert!(read.torn_tail);
}

#[test]
fn a_crc_failure_before_the_end_is_corruption() {
    let a = grant_payload();
    let mut file = file_of(&[a.clone(), a]);
    file[write_header().len() + 4] ^= 0xff;
    assert!(matches!(scan(&file), Err(FileError::Corrupt { .. })));
}

#[test]
fn an_unknown_version_or_magic_is_refused() {
    let mut file = write_header();
    file[8] = 2;
    assert_eq!(scan(&file), Err(FileError::UnsupportedVersion(2)));
    let mut file = write_header();
    file[0] = b'X';
    assert_eq!(scan(&file), Err(FileError::BadMagic));
}

/// A half-written header is zk-evidence's `TruncatedHeader`; the connector's file writer treats
/// it as a torn start and writes the header again (`evidence_file.rs`).
#[test]
fn an_empty_or_half_written_header_is_truncated() {
    for len in 0..write_header().len() {
        assert_eq!(
            scan(&write_header()[..len]),
            Err(FileError::TruncatedHeader)
        );
    }
}

#[test]
fn a_non_canonical_field_element_is_refused() {
    let mut payload = vec![2u8];
    payload.extend_from_slice(&[0xff; 32]);
    payload.extend_from_slice(&[1, 0xbb]);
    payload.extend_from_slice(&0u64.to_le_bytes());
    assert_eq!(
        Record::decode(&payload),
        Err(DecodeError::NonCanonicalField)
    );
}

#[test]
fn an_allowance_id_over_64_bytes_is_refused() {
    let mut long = grant();
    long.allowance_id = vec![0; 65];
    assert_eq!(
        Record::Grant(long).encode(),
        Err(DecodeError::AllowanceIdTooLong(65))
    );
}
