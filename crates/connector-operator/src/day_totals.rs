//! The running totals of a controller's write key (ADR 0078 decision 8): what
//! each restricted key has counted against its limits, per UTC day, journaled
//! under `state_dir` so that a restart, or removing an entry and adding it
//! back, does not hand the key a fresh day.
//!
//! The journal is one JSON object per line, each naming a [`CountKind`] so the
//! deposit limit can count into the same file as a second kind. It is replayed
//! at boot into totals by key, kind and day. A count is appended and
//! `fsync`'d **before** [`DayTotals::count_spending`] returns, and the check
//! and the append share one lock, so two writes arriving together cannot both
//! pass a check that only one of them fits.

use std::collections::HashMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use connector_config::SpendingLimit;
use connector_runtime::Clock;
use serde::{Deserialize, Serialize};

/// The file under `state_dir` the totals are journaled in.
pub const DAY_TOTALS_JOURNAL: &str = "controller-day-totals.journal";

/// What a count is against. Only the spending limit counts today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CountKind {
    Spending,
}

#[derive(Debug, Serialize, Deserialize)]
struct CountRecord {
    kind: CountKind,
    /// The key, 64 hex.
    key: String,
    /// The UTC day, `YYYY-MM-DD`.
    day: String,
    amount: u64,
}

/// Why a count was not made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpendRefusal {
    /// The packet's amount is above the per-packet figure.
    OverPacket { amount: u64, limit: u64 },
    /// The packet would take the day's total above the per-day figure.
    OverDay {
        amount: u64,
        counted: u64,
        limit: u64,
    },
    /// The count could not be appended, so the packet is refused.
    Unrecorded(String),
}

impl fmt::Display for SpendRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpendRefusal::OverPacket { amount, limit } => write!(
                f,
                "spending limit: a packet of {amount} is above this key's per-packet figure of {limit} (ADR 0078)"
            ),
            SpendRefusal::OverDay {
                amount,
                counted,
                limit,
            } => write!(
                f,
                "spending limit: a packet of {amount} would take this key's total for the UTC day from {counted} above its per-day figure of {limit} (ADR 0078)"
            ),
            SpendRefusal::Unrecorded(error) => write!(
                f,
                "spending limit: this key's count could not be journaled ({error}), so the packet is refused (ADR 0078)"
            ),
        }
    }
}

/// Why the journal could not be opened.
#[derive(Debug)]
pub enum DayTotalsError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Corrupt {
        path: PathBuf,
        line: usize,
        message: String,
    },
}

impl fmt::Display for DayTotalsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DayTotalsError::Io { path, source } => write!(
                f,
                "the controller day-totals journal {} is unusable: {source}",
                path.display()
            ),
            DayTotalsError::Corrupt {
                path,
                line,
                message,
            } => write!(
                f,
                "the controller day-totals journal {} is corrupt at line {line}: {message}; \
                 refusing to start rather than let a key's day reset",
                path.display()
            ),
        }
    }
}

impl std::error::Error for DayTotalsError {}

struct Inner {
    file: File,
    totals: HashMap<(CountKind, String, String), u64>,
}

/// Per-key, per-UTC-day running totals, durable across restarts.
pub struct DayTotals {
    clock: Arc<dyn Clock>,
    inner: Mutex<Inner>,
}

impl DayTotals {
    /// Open (creating if absent) the journal at `path` and replay it.
    pub fn open(path: &Path, clock: Arc<dyn Clock>) -> Result<DayTotals, DayTotalsError> {
        let io_error = |source| DayTotalsError::Io {
            path: path.to_path_buf(),
            source,
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path)
            .map_err(io_error)?;
        let mut text = String::new();
        file.read_to_string(&mut text).map_err(io_error)?;
        let mut totals = HashMap::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record: CountRecord =
                serde_json::from_str(line).map_err(|error| DayTotalsError::Corrupt {
                    path: path.to_path_buf(),
                    line: index + 1,
                    message: error.to_string(),
                })?;
            let total = totals
                .entry((record.kind, record.key, record.day))
                .or_insert(0u64);
            *total = total.saturating_add(record.amount);
        }
        Ok(DayTotals {
            clock,
            inner: Mutex::new(Inner { file, totals }),
        })
    }

    fn today(&self) -> String {
        self.clock.now().format("%Y-%m-%d").to_string()
    }

    /// What `key` has counted against its spending limit today.
    pub fn spent_today(&self, key: &str) -> u64 {
        let day = self.today();
        let inner = self.inner.lock().expect("day totals lock poisoned");
        inner
            .totals
            .get(&(CountKind::Spending, key.to_string(), day))
            .copied()
            .unwrap_or(0)
    }

    /// Check `amount` against `limit` and, if it fits, journal it and add it
    /// to the key's total for the current UTC day. Nothing moves on a refusal.
    pub fn count_spending(
        &self,
        key: &str,
        amount: u64,
        limit: SpendingLimit,
    ) -> Result<(), SpendRefusal> {
        if amount > limit.per_write {
            return Err(SpendRefusal::OverPacket {
                amount,
                limit: limit.per_write,
            });
        }
        let day = self.today();
        let mut inner = self.inner.lock().expect("day totals lock poisoned");
        let slot = (CountKind::Spending, key.to_string(), day.clone());
        let counted = inner.totals.get(&slot).copied().unwrap_or(0);
        match counted.checked_add(amount) {
            Some(total) if total <= limit.per_day => {}
            _ => {
                return Err(SpendRefusal::OverDay {
                    amount,
                    counted,
                    limit: limit.per_day,
                })
            }
        }
        let record = CountRecord {
            kind: CountKind::Spending,
            key: key.to_string(),
            day,
            amount,
        };
        // One `write_all`, so the line and its newline reach the file together.
        let mut line = serde_json::to_string(&record).expect("a count record serialises");
        line.push('\n');
        inner
            .file
            .write_all(line.as_bytes())
            .and_then(|()| inner.file.sync_data())
            .map_err(|error| SpendRefusal::Unrecorded(error.to_string()))?;
        inner.totals.insert(slot, counted + amount);
        Ok(())
    }
}
