//! The zk edge's evidence file (ADR 0080 decision 11): append-only, under
//! `state_dir`, one record per allowance granted, fsynced before the grant
//! is answered. Its bytes are zk_toon's ([`connector_zk_note::evidence`]),
//! because zk_toon's challenger reads it; this module only puts them on disk
//! and gets them back.
//!
//! Opening recovers from a torn final write -- a crash mid-append -- by
//! cutting the file back to its last whole record, as the format requires a
//! writer to. A CRC failure anywhere else is corruption and refuses the open:
//! a zk edge that cannot read its evidence cannot defend its vault.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

use connector_zk_note::evidence::{evidence_hash, frame, header, scan, EvidenceError};

/// The file's name under `state_dir`.
pub const EVIDENCE_FILE: &str = "zk-note-evidence.bin";

/// Why the evidence file could not be opened or written.
#[derive(Debug, thiserror::Error)]
pub enum EvidenceFileError {
    #[error("the zk-note evidence file could not be read or written: {0}")]
    Io(#[from] std::io::Error),
    #[error("the zk-note evidence file is unreadable: {0}")]
    Format(#[from] EvidenceError),
}

/// An open evidence file.
#[derive(Debug)]
pub struct EvidenceFile {
    file: Mutex<File>,
    hashes: Mutex<HashSet<[u8; 32]>>,
}

impl EvidenceFile {
    /// Open (or create) the evidence file at `path`, cutting off a torn
    /// final record, and writing the header if there is none.
    ///
    /// # Errors
    /// [`EvidenceFileError`] for an unreadable file or one corrupt before
    /// its end.
    pub fn open(path: &Path) -> Result<EvidenceFile, EvidenceFileError> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let read = scan(&bytes)?;
        if read.end < bytes.len() {
            file.set_len(read.end as u64)?;
        }
        file.seek(SeekFrom::Start(read.end as u64))?;
        if read.end == 0 {
            file.write_all(&header())?;
        }
        file.sync_all()?;
        Ok(EvidenceFile {
            file: Mutex::new(file),
            hashes: Mutex::new(read.records.iter().map(|p| evidence_hash(p)).collect()),
        })
    }

    /// Append one record's payload and fsync it, returning its evidence
    /// hash. Nothing that depends on the record may be answered before this
    /// returns.
    ///
    /// # Errors
    /// [`EvidenceFileError::Io`] when the write or the fsync fails; the
    /// record is then not evidence, and the grant it was for must be
    /// refused.
    pub fn append(&self, payload: &[u8]) -> Result<[u8; 32], EvidenceFileError> {
        let mut file = self.file.lock().expect("evidence file lock poisoned");
        file.write_all(&frame(payload))?;
        file.sync_data()?;
        let hash = evidence_hash(payload);
        self.hashes
            .lock()
            .expect("evidence hashes lock poisoned")
            .insert(hash);
        Ok(hash)
    }

    /// Whether a record with this evidence hash is in the file.
    pub fn holds(&self, hash: &[u8; 32]) -> bool {
        self.hashes
            .lock()
            .expect("evidence hashes lock poisoned")
            .contains(hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_zk_note::evidence::scan;

    #[test]
    fn a_new_file_gets_its_header_and_appends_whole_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(EVIDENCE_FILE);
        let file = EvidenceFile::open(&path).unwrap();
        let hash = file.append(b"\x02payload").unwrap();
        assert!(file.holds(&hash));
        let read = scan(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(read.records, vec![b"\x02payload".to_vec()]);
        drop(file);
        let reopened = EvidenceFile::open(&path).unwrap();
        assert!(reopened.holds(&hash));
    }

    #[test]
    fn a_torn_tail_is_cut_before_the_next_append() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(EVIDENCE_FILE);
        EvidenceFile::open(&path).unwrap().append(b"one").unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(&[9, 0, 0, 0, b't']);
        std::fs::write(&path, &bytes).unwrap();
        EvidenceFile::open(&path).unwrap().append(b"two").unwrap();
        let read = scan(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(read.records, vec![b"one".to_vec(), b"two".to_vec()]);
        assert!(!read.torn);
    }

    #[test]
    fn corruption_before_the_end_refuses_the_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(EVIDENCE_FILE);
        let file = EvidenceFile::open(&path).unwrap();
        file.append(b"one").unwrap();
        file.append(b"two").unwrap();
        drop(file);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[header().len() + 4] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            EvidenceFile::open(&path),
            Err(EvidenceFileError::Format(EvidenceError::Corrupt { .. }))
        ));
    }
}
