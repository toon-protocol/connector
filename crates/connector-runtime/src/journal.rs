//! Durable persistence of money state (ADR 0005, issue #424): the [`Journal`]
//! port and its two implementations. Per the ADR, only what is signed or
//! irreversible is ever written here -- [`connector_domain::JournalEntry`]
//! is the exact alphabet, and everything else (balances, exposure) stays a
//! [`connector_domain::Projection`] recomputed from it, never stored
//! directly. No ledger port, no database: [`FileJournal`] is a single
//! append-only file, matching "no separate accounting database exists" in
//! the issue's own acceptance criteria.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use connector_domain::JournalEntry;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum JournalError {
    #[error("journal I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("corrupt journal entry: {0}")]
    Corrupt(String),
}

/// Durable storage for the sequence of [`JournalEntry`] values ADR 0005
/// requires persisted. `append` MUST return only once `entry` is durable --
/// callers rely on that to write the journal before considering value moved
/// (ADR 0005's "Consequences").
pub trait Journal: Send + Sync {
    fn append(&self, entry: &JournalEntry) -> Result<(), JournalError>;

    /// Append `entries` in order, durably, as one operation -- the group
    /// commit issue #686 amortizes the client edge's per-claim fsync over.
    /// The contract is `append`'s, batched: when this returns `Ok`, every
    /// entry in the batch is durable, in the order given, with nothing
    /// interleaved between them from any concurrent append. An `Err` makes
    /// no promise about any entry in the batch -- a caller must treat the
    /// whole batch as not durably recorded, exactly as it would treat one
    /// failed `append`.
    ///
    /// The default is a per-entry loop, correct for any implementation --
    /// [`FileJournal`] overrides it to one write and one fsync, which is
    /// the entire point.
    fn append_batch(&self, entries: &[JournalEntry]) -> Result<(), JournalError> {
        for entry in entries {
            self.append(entry)?;
        }
        Ok(())
    }

    /// Every entry ever appended, in the order they were written -- what a
    /// node folds into a [`connector_domain::Projection`] and replays into
    /// `ClaimBook` state on start.
    fn read_all(&self) -> Result<Vec<JournalEntry>, JournalError>;
}

/// An in-process, non-durable [`Journal`] (ADR 0007's fake, not a mock --
/// real behavior, just not backed by a disk). This is `ClaimBook`'s default:
/// a node that never configures a real journal keeps working exactly as it
/// did before issue #424, just as a node with no settlement backend keeps
/// working with `settlement: None`. Nothing here survives a restart.
#[derive(Default)]
pub struct InMemoryJournal {
    entries: Mutex<Vec<JournalEntry>>,
}

impl InMemoryJournal {
    pub fn new() -> InMemoryJournal {
        InMemoryJournal::default()
    }
}

impl Journal for InMemoryJournal {
    fn append(&self, entry: &JournalEntry) -> Result<(), JournalError> {
        self.entries
            .lock()
            .expect("in-memory journal lock poisoned")
            .push(entry.clone());
        Ok(())
    }

    fn read_all(&self) -> Result<Vec<JournalEntry>, JournalError> {
        Ok(self
            .entries
            .lock()
            .expect("in-memory journal lock poisoned")
            .clone())
    }
}

/// Lowercase hex, no `0x` prefix -- the journal line's own encoding for a
/// signature, which (unlike `channel_id`/`peer_id`) is arbitrary bytes that
/// could otherwise contain a tab or newline.
fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// One line of the journal's on-disk encoding: a type tag followed by its
/// fields, tab-separated -- deliberately not `serde_json` or a binary
/// format: every field here is a `String` or `u64`, none can themselves
/// contain a tab or newline (`channel_id`/`peer_id` are connector-assigned
/// identifiers, not untrusted wire input), so this is the simplest format
/// that round-trips exactly, human-readable in place, matching the
/// wire-level manual-encoding style already used throughout this crate
/// (`WireClaim::encode`, `Frame`) rather than pulling in a new dependency
/// for it.
fn encode_line(entry: &JournalEntry) -> String {
    match entry {
        JournalEntry::OutboundClaimSigned {
            peer_id,
            channel_id,
            nonce,
            cumulative_amount,
        } => {
            format!("outbound_claim_signed\t{peer_id}\t{channel_id}\t{nonce}\t{cumulative_amount}")
        }
        JournalEntry::InboundClaimAccepted {
            channel_id,
            nonce,
            cumulative_amount,
            signature,
        } => format!(
            "inbound_claim_accepted\t{channel_id}\t{nonce}\t{cumulative_amount}\t{}",
            encode_hex(signature)
        ),
        JournalEntry::InboundFulfillmentRecorded { channel_id, amount } => {
            format!("inbound_fulfillment_recorded\t{channel_id}\t{amount}")
        }
        JournalEntry::InboundClaimWatermarkReset { channel_id } => {
            format!("inbound_claim_watermark_reset\t{channel_id}")
        }
        JournalEntry::InboundClaimRolledBack {
            channel_id,
            nonce,
            cumulative_amount,
        } => format!("inbound_claim_rolled_back\t{channel_id}\t{nonce}\t{cumulative_amount}"),
        JournalEntry::BatchChannelAdmitted {
            channel_id,
            presentation,
        } => format!(
            "batch_channel_admitted\t{channel_id}\t{}",
            encode_hex(presentation)
        ),
    }
}

fn decode_line(line: &str) -> Result<JournalEntry, JournalError> {
    let fields: Vec<&str> = line.split('\t').collect();
    let corrupt = || JournalError::Corrupt(line.to_string());
    let parse_u64 = |s: &str| s.parse::<u64>().map_err(|_| corrupt());
    match fields.as_slice() {
        ["outbound_claim_signed", peer_id, channel_id, nonce, cumulative_amount] => {
            Ok(JournalEntry::OutboundClaimSigned {
                peer_id: peer_id.to_string(),
                channel_id: channel_id.to_string(),
                nonce: parse_u64(nonce)?,
                cumulative_amount: parse_u64(cumulative_amount)?,
            })
        }
        ["inbound_claim_accepted", channel_id, nonce, cumulative_amount, signature] => {
            Ok(JournalEntry::InboundClaimAccepted {
                channel_id: channel_id.to_string(),
                nonce: parse_u64(nonce)?,
                cumulative_amount: parse_u64(cumulative_amount)?,
                signature: decode_hex(signature).ok_or_else(corrupt)?,
            })
        }
        ["inbound_fulfillment_recorded", channel_id, amount] => {
            Ok(JournalEntry::InboundFulfillmentRecorded {
                channel_id: channel_id.to_string(),
                amount: parse_u64(amount)?,
            })
        }
        ["inbound_claim_watermark_reset", channel_id] => {
            Ok(JournalEntry::InboundClaimWatermarkReset {
                channel_id: channel_id.to_string(),
            })
        }
        ["inbound_claim_rolled_back", channel_id, nonce, cumulative_amount] => {
            Ok(JournalEntry::InboundClaimRolledBack {
                channel_id: channel_id.to_string(),
                nonce: parse_u64(nonce)?,
                cumulative_amount: parse_u64(cumulative_amount)?,
            })
        }
        ["batch_channel_admitted", channel_id, presentation] => {
            Ok(JournalEntry::BatchChannelAdmitted {
                channel_id: channel_id.to_string(),
                presentation: decode_hex(presentation).ok_or_else(corrupt)?,
            })
        }
        _ => Err(corrupt()),
    }
}

/// A [`Journal`] backed by a single append-only file -- one line per entry,
/// `fsync`'d before `append` returns so a crash immediately after cannot
/// lose it (ADR 0005: "the journal being written before value is considered
/// moved").
pub struct FileJournal {
    path: PathBuf,
    file: Mutex<File>,
}

impl FileJournal {
    /// Open `path` for appending, creating it if it does not exist yet --
    /// the file itself is this node's entire durable money state, per ADR
    /// 0005 ("no separate accounting database exists").
    pub fn open(path: impl AsRef<Path>) -> Result<FileJournal, JournalError> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)?;
        Ok(FileJournal {
            path,
            file: Mutex::new(file),
        })
    }
}

impl Journal for FileJournal {
    fn append(&self, entry: &JournalEntry) -> Result<(), JournalError> {
        let mut file = self.file.lock().expect("file journal lock poisoned");
        writeln!(file, "{}", encode_line(entry))?;
        file.sync_data()?;
        Ok(())
    }

    /// One buffered write and one `fsync` for the whole batch -- the fsync
    /// is the per-entry cost issue #686 exists to amortize, and batching
    /// `write` calls beside it keeps a batch's lines contiguous under the
    /// one lock hold.
    fn append_batch(&self, entries: &[JournalEntry]) -> Result<(), JournalError> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut lines = String::new();
        for entry in entries {
            lines.push_str(&encode_line(entry));
            lines.push('\n');
        }
        let mut file = self.file.lock().expect("file journal lock poisoned");
        file.write_all(lines.as_bytes())?;
        file.sync_data()?;
        Ok(())
    }

    fn read_all(&self) -> Result<Vec<JournalEntry>, JournalError> {
        let file = File::open(&self.path)?;
        BufReader::new(file)
            .lines()
            .map(|line| decode_line(&line?))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<JournalEntry> {
        vec![
            JournalEntry::OutboundClaimSigned {
                peer_id: "peer-b".to_string(),
                channel_id: "channel-a".to_string(),
                nonce: 1,
                cumulative_amount: 100,
            },
            JournalEntry::InboundClaimAccepted {
                channel_id: "channel-c".to_string(),
                nonce: 3,
                cumulative_amount: 250,
                signature: vec![0xde, 0xad, 0xbe, 0xef],
            },
            JournalEntry::InboundFulfillmentRecorded {
                channel_id: "channel-c".to_string(),
                amount: 25,
            },
            JournalEntry::InboundClaimWatermarkReset {
                channel_id: "solana:channel-c".to_string(),
            },
            JournalEntry::InboundClaimRolledBack {
                channel_id: "channel-c".to_string(),
                nonce: 2,
                cumulative_amount: 150,
            },
            JournalEntry::BatchChannelAdmitted {
                channel_id: "evm:0xabcd".to_string(),
                presentation: vec![0x01, 0x02, 0x03],
            },
            // A Solana channel's presentation is empty, and must still
            // round-trip rather than lose its trailing field.
            JournalEntry::BatchChannelAdmitted {
                channel_id: "solana:channel-d".to_string(),
                presentation: Vec::new(),
            },
        ]
    }

    #[test]
    fn an_in_memory_journal_reads_back_everything_appended_in_order() {
        let journal = InMemoryJournal::new();
        for entry in sample_entries() {
            journal.append(&entry).unwrap();
        }

        assert_eq!(journal.read_all().unwrap(), sample_entries());
    }

    #[test]
    fn a_file_journal_reads_back_everything_appended_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let journal = FileJournal::open(dir.path().join("journal.log")).unwrap();
        for entry in sample_entries() {
            journal.append(&entry).unwrap();
        }

        assert_eq!(journal.read_all().unwrap(), sample_entries());
    }

    #[test]
    fn a_file_journal_reopened_on_the_same_path_still_has_every_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.log");
        {
            let journal = FileJournal::open(&path).unwrap();
            for entry in sample_entries() {
                journal.append(&entry).unwrap();
            }
        }

        // A fresh handle on the same path, standing in for a restart: the
        // entries a prior process instance wrote are still there.
        let reopened = FileJournal::open(&path).unwrap();
        assert_eq!(reopened.read_all().unwrap(), sample_entries());
    }

    #[test]
    fn a_file_journal_batch_reads_back_in_order_beside_single_appends() {
        let dir = tempfile::tempdir().unwrap();
        let journal = FileJournal::open(dir.path().join("journal.log")).unwrap();
        let entries = sample_entries();
        journal.append(&entries[0]).unwrap();
        journal.append_batch(&entries[1..]).unwrap();

        assert_eq!(journal.read_all().unwrap(), entries);
    }

    #[test]
    fn an_empty_batch_appends_nothing_and_syncs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let journal = FileJournal::open(dir.path().join("journal.log")).unwrap();
        journal.append_batch(&[]).unwrap();

        assert!(journal.read_all().unwrap().is_empty());
    }

    #[test]
    fn the_default_batch_is_the_per_entry_loop() {
        let journal = InMemoryJournal::new();
        journal.append_batch(&sample_entries()).unwrap();

        assert_eq!(journal.read_all().unwrap(), sample_entries());
    }

    #[test]
    fn a_file_journal_starts_empty_for_a_path_that_does_not_exist_yet() {
        let dir = tempfile::tempdir().unwrap();
        let journal = FileJournal::open(dir.path().join("fresh.log")).unwrap();

        assert!(journal.read_all().unwrap().is_empty());
    }

    #[test]
    fn every_entry_kind_round_trips_through_the_line_encoding() {
        for entry in sample_entries() {
            assert_eq!(decode_line(&encode_line(&entry)).unwrap(), entry);
        }
    }
}
