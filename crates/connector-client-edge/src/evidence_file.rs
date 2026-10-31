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
//!
//! An append that fails without a crash -- a write cut short, an fsync that
//! errors -- is cut back the same way before it returns, so neither the next
//! append nor the next boot ever sees it. A record can also be appended
//! *pending* ([`EvidenceFile::append_pending`]): durable, but cut back unless
//! kept, for a grant whose journal entry might still fail.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use connector_zk_note::evidence::{
    evidence_hash, file_error_message, frame, scan, write_header, FileError,
};

/// The file's name under `state_dir`.
pub const EVIDENCE_FILE: &str = "zk-note-evidence.bin";

/// Why the evidence file could not be opened or written.
#[derive(Debug, thiserror::Error)]
pub enum EvidenceFileError {
    #[error("the zk-note evidence file could not be read or written: {0}")]
    Io(#[from] std::io::Error),
    #[error("the zk-note evidence file is unreadable: {}", file_error_message(.0))]
    Format(FileError),
}

/// What the evidence file is written through: a [`File`], or in this
/// module's tests a disk that fails on demand.
trait Storage: Write + Send {
    fn sync_data(&mut self) -> std::io::Result<()>;
    fn set_len(&mut self, len: u64) -> std::io::Result<()>;
    fn seek_to(&mut self, offset: u64) -> std::io::Result<()>;
}

impl Storage for File {
    fn sync_data(&mut self) -> std::io::Result<()> {
        File::sync_data(self)
    }
    fn set_len(&mut self, len: u64) -> std::io::Result<()> {
        File::set_len(self, len)
    }
    fn seek_to(&mut self, offset: u64) -> std::io::Result<()> {
        self.seek(SeekFrom::Start(offset)).map(|_| ())
    }
}

/// The file and the offset its last whole record ends at.
struct Tail {
    storage: Box<dyn Storage>,
    /// Where the last whole, durable record ends: where the next is written.
    end: u64,
    /// Whether bytes past [`Self::end`] may be on disk -- a failed append
    /// whose cut-back itself failed -- so the next append cuts first.
    dirty: bool,
}

impl Tail {
    /// Cut the file back to [`Self::end`], durably.
    fn cut_back(&mut self) -> std::io::Result<()> {
        self.storage.set_len(self.end)?;
        self.storage.seek_to(self.end)?;
        self.storage.sync_data()?;
        self.dirty = false;
        Ok(())
    }

    /// Write one framed record at [`Self::end`] and fsync it, cutting back
    /// whatever a failure left. Returns the offset it started at.
    fn write_record(&mut self, payload: &[u8]) -> Result<u64, EvidenceFileError> {
        if self.dirty {
            self.cut_back()?;
        }
        let start = self.end;
        let framed = frame(payload);
        let written = self
            .storage
            .write_all(&framed)
            .and_then(|()| self.storage.sync_data());
        if let Err(error) = written {
            self.dirty = true;
            if let Err(cut) = self.cut_back() {
                tracing::error!(
                    error = %cut,
                    "a failed zk-note evidence append could not be cut back; the next append retries"
                );
            }
            return Err(error.into());
        }
        self.end = start + framed.len() as u64;
        Ok(start)
    }
}

/// An open evidence file.
pub struct EvidenceFile {
    tail: Mutex<Tail>,
    hashes: Mutex<HashSet<[u8; 32]>>,
}

impl std::fmt::Debug for EvidenceFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvidenceFile").finish_non_exhaustive()
    }
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
        let header = write_header();
        // An empty or half-written header is a torn start, not a foreign file:
        // the crash came before the header was whole, so it is written again.
        // Shorter than a header and not its prefix is a foreign file, which
        // `zk-evidence` would call merely truncated.
        let (valid_len, hashes) = if bytes.len() < header.len() {
            if !header.starts_with(&bytes) {
                return Err(EvidenceFileError::Format(FileError::BadMagic));
            }
            (0, HashSet::new())
        } else {
            let read = scan(&bytes).map_err(EvidenceFileError::Format)?;
            let hashes = read
                .records
                .iter()
                .map(|record| evidence_hash(record.payload))
                .collect();
            (read.valid_len, hashes)
        };
        if valid_len < bytes.len() {
            file.set_len(valid_len as u64)?;
        }
        file.seek(SeekFrom::Start(valid_len as u64))?;
        let mut end = valid_len as u64;
        if valid_len == 0 {
            file.write_all(&header)?;
            end = header.len() as u64;
        }
        file.sync_all()?;
        Ok(EvidenceFile::over(Box::new(file), end, hashes))
    }

    fn over(storage: Box<dyn Storage>, end: u64, hashes: HashSet<[u8; 32]>) -> EvidenceFile {
        EvidenceFile {
            tail: Mutex::new(Tail {
                storage,
                end,
                dirty: false,
            }),
            hashes: Mutex::new(hashes),
        }
    }

    /// Append one record's payload and fsync it, returning its evidence
    /// hash. Nothing that depends on the record may be answered before this
    /// returns.
    ///
    /// # Errors
    /// [`EvidenceFileError::Io`] when the write or the fsync fails; the
    /// record is then not evidence -- the file is cut back to the record
    /// before it -- and the grant it was for must be refused.
    pub fn append(&self, payload: &[u8]) -> Result<[u8; 32], EvidenceFileError> {
        Ok(self.append_pending(payload)?.keep())
    }

    /// [`Self::append`], held pending: the record is durable, but the file
    /// stays locked until it is [kept](PendingRecord::keep), and is cut back
    /// to the record before it if the [`PendingRecord`] is dropped instead.
    /// For a record that is evidence only once something else is durable
    /// too: a grant's, until its journal entry is.
    ///
    /// # Errors
    /// As [`Self::append`].
    pub fn append_pending(&self, payload: &[u8]) -> Result<PendingRecord<'_>, EvidenceFileError> {
        let mut tail = self.tail.lock().expect("evidence file lock poisoned");
        let start = tail.write_record(payload)?;
        Ok(PendingRecord {
            file: self,
            tail: Some(tail),
            start,
            hash: evidence_hash(payload),
        })
    }

    /// Whether a record with this evidence hash is in the file.
    pub fn holds(&self, hash: &[u8; 32]) -> bool {
        self.hashes
            .lock()
            .expect("evidence hashes lock poisoned")
            .contains(hash)
    }
}

/// A record written and fsynced, and not yet evidence: kept by
/// [`Self::keep`], cut back out of the file if dropped.
#[must_use = "a pending record is cut back out of the file unless kept"]
pub struct PendingRecord<'a> {
    file: &'a EvidenceFile,
    tail: Option<MutexGuard<'a, Tail>>,
    start: u64,
    hash: [u8; 32],
}

impl PendingRecord<'_> {
    /// The record's evidence hash.
    pub fn hash(&self) -> [u8; 32] {
        self.hash
    }

    /// Keep the record: it is evidence from here on.
    pub fn keep(mut self) -> [u8; 32] {
        self.tail.take();
        self.file
            .hashes
            .lock()
            .expect("evidence hashes lock poisoned")
            .insert(self.hash);
        self.hash
    }
}

impl Drop for PendingRecord<'_> {
    fn drop(&mut self) {
        let Some(mut tail) = self.tail.take() else {
            return;
        };
        // Nothing was appended after it: the lock has been held since.
        tail.end = self.start;
        tail.dirty = true;
        if let Err(error) = tail.cut_back() {
            tracing::error!(
                %error,
                "a rescinded zk-note evidence record could not be cut back; the next append retries"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_zk_note::evidence::scan;

    /// Every whole record's payload in the file at `path`, as zk_toon reads it.
    fn records(path: &Path) -> Vec<Vec<u8>> {
        let bytes = std::fs::read(path).unwrap();
        scan(&bytes)
            .unwrap()
            .records
            .iter()
            .map(|record| record.payload.to_vec())
            .collect()
    }

    #[test]
    fn a_new_file_gets_its_header_and_appends_whole_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(EVIDENCE_FILE);
        let file = EvidenceFile::open(&path).unwrap();
        let hash = file.append(b"\x02payload").unwrap();
        assert!(file.holds(&hash));
        assert_eq!(records(&path), vec![b"\x02payload".to_vec()]);
        drop(file);
        let reopened = EvidenceFile::open(&path).unwrap();
        assert!(reopened.holds(&hash));
    }

    /// A crash before the header was whole leaves a prefix of it: a torn
    /// start, which open writes again, never a foreign file.
    #[test]
    fn an_empty_or_half_written_header_is_written_again() {
        let header = write_header();
        for len in 0..header.len() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(EVIDENCE_FILE);
            std::fs::write(&path, &header[..len]).unwrap();
            let file = EvidenceFile::open(&path).expect("a torn start");
            file.append(b"one").unwrap();
            assert_eq!(records(&path), vec![b"one".to_vec()], "cut at {len}");
        }
    }

    #[test]
    fn a_foreign_file_shorter_than_a_header_is_refused_as_foreign() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(EVIDENCE_FILE);
        std::fs::write(&path, b"XY").unwrap();
        assert!(matches!(
            EvidenceFile::open(&path),
            Err(EvidenceFileError::Format(FileError::BadMagic))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"XY", "left as found");
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
        assert_eq!(records(&path), vec![b"one".to_vec(), b"two".to_vec()]);
        assert!(!scan(&std::fs::read(&path).unwrap()).unwrap().torn_tail);
    }

    /// One fault, then a healthy disk: a write that takes `tear_after`
    /// bytes and fails, or an fsync that fails once.
    enum Fault {
        TearAfter(usize),
        Fsync,
    }

    struct FailingDisk {
        file: File,
        fault: Option<Fault>,
    }

    impl Write for FailingDisk {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            match self.fault {
                Some(Fault::TearAfter(0)) => {
                    self.fault = None;
                    Err(std::io::Error::new(
                        std::io::ErrorKind::StorageFull,
                        "disk full",
                    ))
                }
                Some(Fault::TearAfter(ref mut left)) => {
                    let n = buf.len().min(*left);
                    *left -= n;
                    self.file.write(&buf[..n])
                }
                _ => self.file.write(buf),
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.file.flush()
        }
    }

    impl Storage for FailingDisk {
        fn sync_data(&mut self) -> std::io::Result<()> {
            if matches!(self.fault, Some(Fault::Fsync)) {
                self.fault = None;
                return Err(std::io::Error::other("fsync failed"));
            }
            self.file.sync_data()
        }
        fn set_len(&mut self, len: u64) -> std::io::Result<()> {
            self.file.set_len(len)
        }
        fn seek_to(&mut self, offset: u64) -> std::io::Result<()> {
            self.file.seek(SeekFrom::Start(offset)).map(|_| ())
        }
    }

    /// The evidence file at `path`, as `EvidenceFile::open` leaves it, over
    /// a disk that faults once.
    fn faulting(path: &Path, fault: Fault) -> EvidenceFile {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let end = file.seek(SeekFrom::End(0)).unwrap();
        EvidenceFile::over(
            Box::new(FailingDisk {
                file,
                fault: Some(fault),
            }),
            end,
            HashSet::new(),
        )
    }

    /// A write that fails partway leaves no torn record: the file is cut
    /// back to its last whole record before the append returns, so the next
    /// append through the same handle, and the next boot, see a clean file.
    #[test]
    fn a_write_that_fails_partway_is_cut_back_to_the_last_whole_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(EVIDENCE_FILE);
        EvidenceFile::open(&path).unwrap().append(b"one").unwrap();
        let before = std::fs::read(&path).unwrap();

        let file = faulting(&path, Fault::TearAfter(5));
        assert!(file.append(b"a record the disk tears").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        file.append(b"two").unwrap();
        assert_eq!(records(&path), vec![b"one".to_vec(), b"two".to_vec()]);
        assert!(!scan(&std::fs::read(&path).unwrap()).unwrap().torn_tail);
    }

    /// A record whose fsync fails was never promised: it is cut, not held,
    /// and not there after a restart.
    #[test]
    fn a_record_whose_fsync_fails_is_cut_and_not_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(EVIDENCE_FILE);
        EvidenceFile::open(&path).unwrap().append(b"one").unwrap();
        let before = std::fs::read(&path).unwrap();

        let file = faulting(&path, Fault::Fsync);
        assert!(file.append(b"unsynced").is_err());
        assert!(!file.holds(&evidence_hash(b"unsynced")));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(file);
        let reopened = EvidenceFile::open(&path).unwrap();
        assert!(!reopened.holds(&evidence_hash(b"unsynced")));
        reopened.append(b"two").unwrap();
        assert_eq!(records(&path), vec![b"one".to_vec(), b"two".to_vec()]);
    }

    /// A pending record dropped -- its grant could not be journaled --
    /// leaves the file as it was; one kept is evidence.
    #[test]
    fn a_dropped_pending_record_leaves_the_file_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(EVIDENCE_FILE);
        let file = EvidenceFile::open(&path).unwrap();
        file.append(b"one").unwrap();
        let before = std::fs::read(&path).unwrap();

        let pending = file.append_pending(b"grant").unwrap();
        assert_eq!(pending.hash(), evidence_hash(b"grant"));
        drop(pending);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!file.holds(&evidence_hash(b"grant")));

        let kept = file.append_pending(b"grant").unwrap().keep();
        assert!(file.holds(&kept));
        assert_eq!(records(&path), vec![b"one".to_vec(), b"grant".to_vec()]);
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
        bytes[write_header().len() + 4] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            EvidenceFile::open(&path),
            Err(EvidenceFileError::Format(FileError::Corrupt { .. }))
        ));
    }
}
