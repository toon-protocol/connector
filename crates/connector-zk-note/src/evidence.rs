//! The evidence file's bytes: the append-only record of every allowance granted and how each
//! closed, which zk_toon's challenger reads (ADR 0080 decisions 11 and 12).
//!
//! The format is zk_toon's, encoded by its `zk-evidence` crate (zk_toon#29, zk_toon ADR 0004) and
//! re-exported here, so the connector writes exactly the bytes the challenger reads and owns none
//! of them. What the connector owns is putting them on disk durably:
//! `connector-client-edge`'s evidence file.
//!
//! - **File:** `TOONZKEV`, a `u16` LE version (`1`), then records.
//! - **Framing:** `u32 LE payload length ‖ payload ‖ CRC32C(payload)`. A short or CRC-bad record
//!   at the end is a torn write, ignored by a reader and cut off by a writer; a CRC failure
//!   anywhere else is corruption.
//! - **Evidence hash:** SHA-256 of the payload alone. The journal stores it.

pub use zk_evidence::{
    evidence_hash, frame, read_header, scan, write_header, Close, DecodeError, FileError, Framed,
    Grant, Record, Scan, MAGIC, MAX_ALLOWANCE_ID, VERSION,
};

/// A refusal to read an evidence file, in words for a log: `zk-evidence` gives its errors no
/// `Display`.
#[must_use]
pub fn file_error_message(error: &FileError) -> String {
    match error {
        FileError::TruncatedHeader => "its header is incomplete".into(),
        FileError::BadMagic => "it does not start with the evidence file's magic".into(),
        FileError::UnsupportedVersion(version) => {
            format!("it is format version {version}; this build reads only version {VERSION}")
        }
        FileError::Corrupt { offset } => format!("it is corrupt at byte {offset}"),
    }
}

/// A refusal to encode or decode a record, in words for a log.
#[must_use]
pub fn decode_error_message(error: &DecodeError) -> String {
    match error {
        DecodeError::Truncated => "the record ends before its last field".into(),
        DecodeError::TrailingBytes => "bytes follow the record's last field".into(),
        DecodeError::UnknownKind(kind) => format!("the record is of unknown kind {kind}"),
        DecodeError::NonCanonicalField => "the record holds a non-canonical field element".into(),
        DecodeError::BadFlag => "the record holds a flag that is neither 0 nor 1".into(),
        DecodeError::AllowanceIdTooLong(len) => {
            format!("an allowance id of {len} bytes is longer than {MAX_ALLOWANCE_ID}")
        }
        DecodeError::BadProof(why) => format!("the record's request proof is unreadable: {why}"),
    }
}
