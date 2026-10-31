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
