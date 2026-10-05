//! The packet history (ADR 0077): the most recent packets this connector
//! handled, held in memory for its operator to watch.
//!
//! Bounded, lossy and gone on restart. It is never a record and nothing that
//! decides a packet's fate reads it -- the packet path only ever *writes*, and
//! does so without awaiting and without taking a lock a reader can hold
//! (ADR 0015): [`PacketHistory::record`] is a `try_send` to a collector task,
//! and a row that cannot be sent at once is dropped and counted. Nothing here
//! touches the disk or the log.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Serialize, Serializer};
use tokio::sync::mpsc;

/// The longest `message` a row keeps, in bytes.
const MESSAGE_LIMIT: usize = 256;

/// How many rows may wait for the collector before the next is dropped.
#[cfg(not(test))]
const QUEUE_BOUND: usize = 1024;
/// Small under test, so filling it takes a few packets and not a thousand.
#[cfg(test)]
const QUEUE_BOUND: usize = 8;

/// Which way a packet went through this connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Ended at one of this connector's apps.
    Delivered,
    /// Arrived from one side and left toward a peer.
    Forwarded,
    /// Originated through `POST /packets`.
    Sent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Fulfilled,
    Rejected,
}

/// One packet this connector handled. Units are not converted and not named:
/// `amount` is in the arriving leg's unit, `fee` in the outgoing peering's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PacketRow {
    #[serde(serialize_with = "serialize_time")]
    pub time: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    pub destination: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_peer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_peer: Option<String>,
    pub amount: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee: Option<u64>,
    pub outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn serialize_time<S: Serializer>(time: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&time.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// Cut `message` to [`MESSAGE_LIMIT`] bytes, on a character boundary.
pub(crate) fn clip_message(message: &str) -> String {
    if message.len() <= MESSAGE_LIMIT {
        return message.to_string();
    }
    let mut end = MESSAGE_LIMIT;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_string()
}

/// What `GET /packets` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PacketHistoryView {
    pub enabled: bool,
    pub capacity: usize,
    pub dropped: u64,
    /// Newest first.
    pub packets: Vec<PacketRow>,
}

impl PacketHistoryView {
    /// The answer of a node that keeps no history.
    pub fn off() -> PacketHistoryView {
        PacketHistoryView {
            enabled: false,
            capacity: 0,
            dropped: 0,
            packets: Vec::new(),
        }
    }
}

pub(crate) struct PacketHistory {
    capacity: usize,
    sender: mpsc::Sender<PacketRow>,
    ring: Arc<Mutex<VecDeque<PacketRow>>>,
    dropped: AtomicU64,
}

impl PacketHistory {
    /// Start a history of `capacity` rows (non-zero) and its collector task.
    /// Must be called inside a tokio runtime.
    pub(crate) fn spawn(capacity: usize) -> PacketHistory {
        let (sender, mut receiver) = mpsc::channel::<PacketRow>(QUEUE_BOUND);
        let ring = Arc::new(Mutex::new(VecDeque::with_capacity(capacity.min(4096))));
        let collector = Arc::clone(&ring);
        tokio::spawn(async move {
            while let Some(row) = receiver.recv().await {
                let mut ring = collector.lock().unwrap_or_else(|e| e.into_inner());
                if ring.len() == capacity {
                    ring.pop_back();
                }
                ring.push_front(row);
            }
        });
        PacketHistory {
            capacity,
            sender,
            ring,
            dropped: AtomicU64::new(0),
        }
    }

    /// Hand a row to the collector. Never awaits and never blocks; a row that
    /// cannot be handed over at once is dropped and counted.
    pub(crate) fn record(&self, row: PacketRow) {
        if self.sender.try_send(row).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn view(&self, limit: Option<usize>) -> PacketHistoryView {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        let take = limit.unwrap_or(usize::MAX);
        PacketHistoryView {
            enabled: true,
            capacity: self.capacity,
            dropped: self.dropped.load(Ordering::Relaxed),
            packets: ring.iter().take(take).cloned().collect(),
        }
    }

    #[cfg(test)]
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Stall the collector, so the queue fills and further rows drop, until
    /// the returned guard is dropped. The ring's lock is held on a thread of
    /// its own, so a test never holds it across an await.
    #[cfg(test)]
    pub(crate) fn stall_collector(&self) -> Stall {
        let ring = Arc::clone(&self.ring);
        let (held, wait_held) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let _ring = ring.lock().unwrap_or_else(|e| e.into_inner());
            held.send(()).unwrap();
            let _ = released.recv();
        });
        wait_held.recv().unwrap();
        Stall {
            release: Some(release),
            thread: Some(thread),
        }
    }

    #[cfg(test)]
    pub(crate) const QUEUE_BOUND: usize = QUEUE_BOUND;
}

/// A stalled collector (see [`PacketHistory::stall_collector`]); releases on drop.
#[cfg(test)]
pub(crate) struct Stall {
    release: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(test)]
impl Drop for Stall {
    fn drop(&mut self) {
        drop(self.release.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_cut_on_a_character_boundary() {
        let long = "é".repeat(200); // 400 bytes, every char two bytes
        let clipped = clip_message(&long);
        assert_eq!(clipped.len(), 256);
        let odd = format!("a{}", "é".repeat(200)); // boundary falls mid-char
        let clipped = clip_message(&odd);
        assert_eq!(clipped.len(), 255);
        assert_eq!(clip_message("short"), "short");
    }
}
