//! A durable local index of `TokenNetwork`'s own `ChannelOpened` /
//! `ChannelNewDeposit` / `ChannelSettled` logs (issue #661), so that
//! resolving a channel this index has caught up to is a `HashMap` probe
//! rather than an `eth_call`.
//!
//! `ChannelClosed` and `ChannelClosedByExpiry` are deliberately not in that
//! alphabet: both mark the *start* of a channel's challenge period
//! (`closeChannel` and `forceCloseExpiredChannel` each set
//! `ChannelState.Closed`), and `claimFromChannel` accepts `Opened` and
//! `Closed` alike, so neither event changes whether a claim is still
//! payable. The one transition that does -- `settleChannel`, the only path
//! out of `Closed` -- emits `ChannelSettled`, which this index does track.
//!
//! # What this is a fix for
//!
//! `#611` resolves a channel this connector has no declared record of by
//! reading the chain -- one `TokenNetwork.channels(id)` call plus one
//! `participants(id, counterparty)` call the deposit needs. `#613` is the
//! abuse of that: an anonymous sender can name channels that do not exist
//! and force one RPC read each, and every bound available at the resolving
//! layer (`connector_client_edge::lookup_budget::UnresolvableLookupBudget`)
//! is a rationing of that cost, not a removal of it. Subscribing to the
//! contract's own logs and keeping a local map removes the RPC cost from
//! the lookup entirely: a hit answers from memory, and a genuine miss falls
//! through to the existing chain-read path unchanged (see
//! `connector-cli`'s `runtime::IndexedEvmChannelSource`, which wires this
//! index in as a [`connector_client_edge::ClientChannelSource`] with exactly
//! that fallback -- kept out of this crate per ADR 0001, which locates every
//! construction decision in `connector-cli`).
//!
//! This module holds the index's state machine and its durable snapshot
//! format only. Actually querying a chain for logs and driving this index
//! from them lives in [`crate::channel_index_sync`], kept separate so the
//! state machine here can be tested without a chain at all.
//!
//! # EVM only -- Solana is out of scope
//!
//! `packages/solana-program/src/processor.rs` emits only free-text `msg!`
//! lines for a channel's lifecycle (`"Payment channel initialized"`,
//! `"Deposit of {} tokens recorded"`, `"Channel closed at timestamp {}"`,
//! `"Channel settled: A={}, B={}"`) -- no `emit!`, no structured event, no
//! anchor IDL event section. There is nothing indexable the way
//! `TokenNetwork.sol`'s real Solidity events are: building an equivalent for
//! Solana would mean `getSignaturesForAddress` polling plus parsing prose
//! log lines whose format is not a contract, a materially different (and
//! worse) mechanism with no format-stability guarantee. Solana channel
//! resolution therefore keeps its existing resolve + liveness-refresh +
//! [`connector_client_edge::lookup_budget::UnresolvableLookupBudget`] path,
//! unchanged, indefinitely -- this is a decision, not a gap to fill later.
//!
//! # Why a snapshot, not an append-only log
//!
//! Structurally the same call `docs/adr/0034-...md`'s `PeerRouteStore`
//! (`crates/connector-runtime/src/peer_route_store.rs`) already made for
//! issue #884's runtime peer/route table, and for the same reason: this
//! table supports removal-in-effect (a settled channel is marked terminal
//! in place, not appended over), which an append-only log the shape of
//! [`connector_domain::JournalEntry`] cannot express without a compaction
//! pass nothing else here needs. It is deliberately **not** a new
//! `JournalEntry` variant -- ADR 0033 already froze that alphabet at
//! "claims sent, claims received, fulfilments" and this index is neither: it
//! is a rebuildable-from-chain cache, not a money record. Every write goes
//! to a temp file beside the target path and is renamed over it, so a crash
//! mid-write leaves the previous, still-valid snapshot in place.
//!
//! # What a snapshot is about
//!
//! A snapshot records the chain id and the resolved `TokenNetwork` its rows
//! came from, and [`EvmChannelIndex::open`] resumes one only when both
//! still match and its checkpoint is at or above the configured
//! `channel_index_from_block` (issue #1282). This is not belt-and-braces:
//! a `state_dir` is a volume that outlives an `[settlement.evm]` edit, so
//! an unbound snapshot really does get resumed against a chain it says
//! nothing about -- a Base Sepolia checkpoint at block 46,256,398 resumed
//! against Base mainnet, ~4.5M blocks of unrelated history walked in
//! 2,000-block ranges, and five wrong-chain channels served out of this
//! table the whole time. [`ChannelIndexLookup::Terminal`] is the part that
//! makes that a wrong answer rather than a slow one, since it is served
//! with no chain read to correct it.
//!
//! A snapshot that fails either test is discarded whole and never
//! repaired: this index is rebuildable from chain, and rewriting a file
//! whose provenance is exactly what is in doubt would be inventing a
//! checkpoint rather than finding one.
//!
//! # Reorgs
//!
//! This index only ever applies a log once it is
//! `channel_index_confirmations` blocks deep (issue #661 decision point 4).
//! There is no unwind path, on purpose: nothing this index answers needs
//! head liveness. The case it serves is a buyer whose channel-open has been
//! sitting on chain for a while, so waiting out a confirmation depth costs
//! that buyer nothing; and a channel opened *inside* the window -- the only
//! case the delay could hurt -- is a [`ChannelIndexLookup::Miss`] here,
//! which falls through to the direct `eth_call` read and is served exactly
//! as fast as it is today.
//!
//! A channel *funded* inside the window is the case that sentence missed
//! (issue #1151): its `ChannelOpened` is applied and its
//! `ChannelNewDeposit` is not, so it is an
//! [`Active`](ChannelIndexLookup::Active) with a deposit of zero rather
//! than a `Miss`, and reading that zero as a collateral ceiling refuses a
//! claim the chain would honour. The zero is therefore the caller's cue to
//! read the chain -- see that variant's own doc.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use ethers::types::{Address, H256, U256};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Whether a channel this index holds a record for can still be paid on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedChannelStatus {
    Open,
    /// Seen `ChannelSettled`: `TokenNetwork.claimFromChannel` requires
    /// `Opened` or `Closed` (`TokenNetwork.sol:273`), so a claim against a
    /// settled channel can never be redeemed.
    Settled,
}

impl IndexedChannelStatus {
    /// Whether a claim against a channel in this status can ever be
    /// redeemed. A merely closed (but not yet settled) channel still
    /// redeems during its challenge period (issue #574) -- this index does
    /// not track `ChannelClosed` *or* `ChannelClosedByExpiry` at all for
    /// exactly that reason: both set `ChannelState.Closed`, a state
    /// `claimFromChannel` accepts, so neither changes whether a claim is
    /// still payable (see the module doc).
    pub fn is_terminal(self) -> bool {
        matches!(self, IndexedChannelStatus::Settled)
    }
}

#[derive(Debug, Clone)]
struct IndexedChannel {
    participant1: Address,
    participant2: Address,
    /// Each participant's cumulative on-chain deposit, keyed by that
    /// participant's own address -- `ChannelNewDeposit.totalDeposit` is
    /// already the cumulative figure, not an increment, so applying one is
    /// an overwrite rather than an add.
    deposits: HashMap<Address, U256>,
    status: IndexedChannelStatus,
}

/// What asking this index about a channel reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelIndexLookup {
    /// This index holds no record of the channel at all -- because it has
    /// never opened, because it opened inside the confirmation window, or
    /// because this index has not caught up that far yet. Resolve it the
    /// existing way: a direct chain read, budgeted as before (issue #661
    /// decision point 5 -- a miss is never treated as "no such channel").
    Miss,
    /// A channel `own_address` is a participant of and that has not
    /// settled, and what its counterparty has deposited in total.
    ///
    /// A `deposit` of **zero** is the one value here that is not a fact
    /// (issue #1151): it is what this index reports both for a channel
    /// whose counterparty has genuinely put nothing in and for one whose
    /// `ChannelNewDeposit` this index has not applied yet, and no amount of
    /// waiting separates the two -- this index is permanently
    /// `channel_index_confirmations` blocks behind head, so a deposit made
    /// inside that window is invisible to it however old the channel is. A
    /// caller turning this into a collateral ceiling must ask the chain
    /// instead of believing the zero; see `connector-cli`'s
    /// `runtime::IndexedEvmChannelSource`, which does exactly that.
    Active {
        counterparty: Address,
        deposit: U256,
    },
    /// This index has seen the channel settle. Distinct from
    /// [`Miss`](Self::Miss): this is a known, definitive fact rather than an
    /// absence of information, and it is reported as such so a caller can
    /// refuse the claim distinguishably from an unknown channel, without a
    /// chain read.
    Terminal,
}

/// One decoded, not-yet-applied log this index's state machine knows how to
/// fold in. Chain-querying and decoding lives in
/// `crate::channel_index_sync`; this type is what crosses that seam so the
/// state machine here needs no chain client at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelIndexEvent {
    Opened {
        channel_id: [u8; 32],
        participant1: Address,
        participant2: Address,
    },
    NewDeposit {
        channel_id: [u8; 32],
        participant: Address,
        total_deposit: U256,
    },
    Settled {
        channel_id: [u8; 32],
    },
}

/// A [`ChannelIndexEvent`] tagged with where it sits in the chain's own
/// order, so a batch spanning more than one event type (and, at a range
/// boundary, more than one block) is applied in the order the chain itself
/// produced it -- a `ChannelSettled` in the same block as a late
/// `ChannelNewDeposit` must not be applied before the deposit it follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderedChannelIndexEvent {
    pub block_number: u64,
    pub log_index: u64,
    pub event: ChannelIndexEvent,
}

/// What a channel index's contents are *about*: the chain, and the
/// `TokenNetwork` on it, whose logs produced every row in the table (issue
/// #1282).
///
/// This travels into the durable snapshot and back out of it, because a
/// snapshot that does not say what it indexes cannot be told apart from
/// another deployment's -- and a state volume outlives an
/// `[settlement.evm]` cutover, so one really does arrive holding the wrong
/// chain's channels at the wrong chain's block height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedContract {
    /// The live chain id the settlement backend proved by asking the chain,
    /// never one inferred from the shape of an RPC URL.
    pub chain_id: u64,
    /// The **resolved** `TokenNetwork` -- what the configured registry
    /// answers for the configured token, not the registry itself. A
    /// registry that re-points its `TokenNetwork` therefore invalidates
    /// this index too, which is correct: the old contract's logs describe
    /// channels the new one has never heard of.
    pub token_network: Address,
}

/// Why a snapshot found on disk was not taken as a checkpoint (issue
/// #1282). `None` from [`EvmChannelIndex::rejected_snapshot`] means either
/// that the snapshot was taken or that there was no snapshot to take.
///
/// Every variant means the same thing operationally -- the index starts
/// empty, from the configured `channel_index_from_block`, and re-backfills
/// -- and differs only in what an operator should be told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectedSnapshot {
    /// Written before issue #1282 and so recording neither chain id nor
    /// contract. Deliberately not an error: an upgrade must not require an
    /// operator to hand-edit a state volume, and the index is rebuildable
    /// from chain anyway.
    Unbound,
    /// The snapshot's blocks are numbered in a different chain -- the
    /// cutover this was found on.
    ChainId { recorded: u64, configured: u64 },
    /// Same chain, different contract. Fires on a registry re-pointing its
    /// `TokenNetwork` as surely as on a cutover, because the address bound
    /// in is the *resolved* one; the rows are as wrong either way.
    TokenNetwork {
        recorded: Address,
        configured: Address,
    },
    /// The operator has asserted, with `channel_index_from_block`, that the
    /// contract did not exist before that block; a checkpoint below it is
    /// stale by construction whatever identity it records.
    BeforeStartBlock { checkpoint: u64, from_block: u64 },
}

#[derive(Debug, Error)]
pub enum EvmChannelIndexError {
    #[error("channel index I/O error at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("corrupt channel index at {path}: {source}")]
    Corrupt {
        path: PathBuf,
        source: serde_json::Error,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredDeposit {
    participant: String,
    /// Decimal, not hex -- `U256` does not fit a JSON number and a decimal
    /// string is what an operator inspecting the file with `jq` expects a
    /// token amount to look like.
    deposit: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum StoredStatus {
    Open,
    Settled,
}

impl From<IndexedChannelStatus> for StoredStatus {
    fn from(status: IndexedChannelStatus) -> StoredStatus {
        match status {
            IndexedChannelStatus::Open => StoredStatus::Open,
            IndexedChannelStatus::Settled => StoredStatus::Settled,
        }
    }
}

impl From<StoredStatus> for IndexedChannelStatus {
    fn from(status: StoredStatus) -> IndexedChannelStatus {
        match status {
            StoredStatus::Open => IndexedChannelStatus::Open,
            StoredStatus::Settled => IndexedChannelStatus::Settled,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredChannel {
    channel_id: String,
    participant1: String,
    participant2: String,
    #[serde(default)]
    deposits: Vec<StoredDeposit>,
    status: StoredStatus,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Snapshot {
    /// `None` (or the key absent) means "never applied a block" -- kept
    /// distinct from `Some(0)` ("caught up through block 0 and nothing
    /// more") so a fresh index and one legitimately caught up to genesis
    /// are not the same state. Genesis never carries a `TokenNetwork` log
    /// in practice, but the syncer's own resume arithmetic
    /// (`last_indexed_block + 1`) needs the distinction to avoid re-querying
    /// block 0 forever when a confirmation depth larger than chain height
    /// pins `confirmed_head` at `0`.
    #[serde(default)]
    last_indexed_block: Option<u64>,
    /// The chain this snapshot's blocks are numbered in (issue #1282).
    /// `None` on a file written before that change -- read as "this
    /// snapshot does not say what it indexes", never as a match.
    #[serde(default)]
    chain_id: Option<u64>,
    /// The resolved `TokenNetwork` whose logs produced this table (issue
    /// #1282), in the same `{:#x}` spelling every other address in this
    /// file uses. `None` on a pre-#1282 file, read the same way
    /// `chain_id`'s `None` is.
    #[serde(default)]
    token_network: Option<String>,
    #[serde(default)]
    channels: Vec<StoredChannel>,
}

fn format_address(address: Address) -> String {
    format!("{address:#x}")
}

fn parse_address(value: &str, path: &Path) -> Result<Address, EvmChannelIndexError> {
    value
        .parse::<Address>()
        .map_err(|source| EvmChannelIndexError::Corrupt {
            path: path.to_path_buf(),
            source: serde::de::Error::custom(format!("invalid address '{value}': {source}")),
        })
}

/// The same `{:#x}` idiom [`format_address`] uses -- `H256`'s `LowerHex`
/// prints all 32 bytes (its `Display` abbreviates, so `{:#x}` rather than
/// `{}` is load-bearing here).
fn format_channel_id(id: [u8; 32]) -> String {
    format!("{:#x}", H256::from(id))
}

fn parse_channel_id(value: &str, path: &Path) -> Result<[u8; 32], EvmChannelIndexError> {
    let hex_digits = value.strip_prefix("0x").unwrap_or(value);
    if hex_digits.len() != 64 || !hex_digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(EvmChannelIndexError::Corrupt {
            path: path.to_path_buf(),
            source: serde::de::Error::custom(format!(
                "invalid channel id '{value}': not 32 bytes of hex"
            )),
        });
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex_digits[i * 2..i * 2 + 2], 16).map_err(|source| {
            EvmChannelIndexError::Corrupt {
                path: path.to_path_buf(),
                source: serde::de::Error::custom(format!("invalid channel id '{value}': {source}")),
            }
        })?;
    }
    Ok(out)
}

#[derive(Debug)]
struct IndexState {
    last_indexed_block: Option<u64>,
    channels: HashMap<[u8; 32], IndexedChannel>,
}

/// The local channel index: an in-memory table of every `TokenNetwork`
/// channel this connector has observed through its own logs, plus (when a
/// `state_dir` is configured) a durable snapshot that survives a restart.
///
/// A node with no `state_dir` still indexes in memory for the life of the
/// process -- it still saves every RPC call within a run -- but re-backfills
/// from `from_block` on every restart, the same in-memory-only degrade issue
/// #884's runtime peer/route table already established for a `state_dir`-less
/// node (ADR 0034).
#[derive(Debug)]
pub struct EvmChannelIndex {
    path: Option<PathBuf>,
    indexed_contract: IndexedContract,
    rejected: Option<RejectedSnapshot>,
    state: RwLock<IndexState>,
}

impl EvmChannelIndex {
    /// No checkpoint and no channels, writing through to `path` from the
    /// first [`Self::apply`] on. The four ways to start from nothing --
    /// no `state_dir` at all, a snapshot file not written yet, one
    /// truncated to empty, and one refused as another deployment's
    /// (`rejected`) -- differ only in that `path` and that reason.
    fn empty(
        path: Option<PathBuf>,
        indexed_contract: IndexedContract,
        rejected: Option<RejectedSnapshot>,
    ) -> Self {
        EvmChannelIndex {
            path,
            indexed_contract,
            rejected,
            state: RwLock::new(IndexState {
                last_indexed_block: None,
                channels: HashMap::new(),
            }),
        }
    }

    /// Open the durable snapshot at `path` (or start empty if it does not
    /// exist yet), falling back to an in-memory-only index when `path` is
    /// `None`.
    ///
    /// A snapshot is taken as a checkpoint only when it is *this* index's
    /// (issue #1282). It was not always so: this call used to take the
    /// checkpoint whenever one existed, on the reasoning that "the
    /// checkpoint -- once it exists -- is always the more accurate figure
    /// to resume from". That is false across a cutover. A state volume
    /// outlives an `[settlement.evm]` edit, so the file can hold another
    /// chain's channels numbered in another chain's blocks, and this index
    /// answers a [`ChannelIndexLookup::Terminal`] with no chain read at all
    /// -- a wrong answer rather than a slow one. So the snapshot must agree
    /// with `indexed_contract` on both the chain id and the `TokenNetwork`, and its
    /// checkpoint must be at or above `from_block`, which is the operator's
    /// own assertion of where that contract begins. A snapshot failing
    /// either test is discarded whole -- table and checkpoint alike -- and
    /// this index starts from `from_block` as a cold one would, with the
    /// reason available from [`Self::rejected_snapshot`]. Nothing is
    /// repaired or migrated: the rule is "do not trust it", and the index
    /// is rebuildable from chain.
    pub fn open(
        path: Option<&Path>,
        indexed_contract: IndexedContract,
        from_block: u64,
    ) -> Result<Self, EvmChannelIndexError> {
        let Some(path) = path else {
            return Ok(EvmChannelIndex::empty(None, indexed_contract, None));
        };
        if !path.exists() {
            return Ok(EvmChannelIndex::empty(
                Some(path.to_path_buf()),
                indexed_contract,
                None,
            ));
        }
        let text = fs::read_to_string(path).map_err(|source| EvmChannelIndexError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if text.trim().is_empty() {
            return Ok(EvmChannelIndex::empty(
                Some(path.to_path_buf()),
                indexed_contract,
                None,
            ));
        }
        let snapshot: Snapshot =
            serde_json::from_str(&text).map_err(|source| EvmChannelIndexError::Corrupt {
                path: path.to_path_buf(),
                source,
            })?;
        if let Some(rejected) = reject_snapshot(&snapshot, indexed_contract, from_block, path)? {
            report_rejected_snapshot(path, rejected, indexed_contract, from_block);
            return Ok(EvmChannelIndex::empty(
                Some(path.to_path_buf()),
                indexed_contract,
                Some(rejected),
            ));
        }
        let mut channels = HashMap::new();
        for stored in snapshot.channels {
            let channel_id = parse_channel_id(&stored.channel_id, path)?;
            let participant1 = parse_address(&stored.participant1, path)?;
            let participant2 = parse_address(&stored.participant2, path)?;
            let mut deposits = HashMap::new();
            for deposit in stored.deposits {
                let participant = parse_address(&deposit.participant, path)?;
                let amount = U256::from_dec_str(&deposit.deposit).map_err(|source| {
                    EvmChannelIndexError::Corrupt {
                        path: path.to_path_buf(),
                        source: serde::de::Error::custom(format!(
                            "invalid deposit '{}': {source}",
                            deposit.deposit
                        )),
                    }
                })?;
                deposits.insert(participant, amount);
            }
            channels.insert(
                channel_id,
                IndexedChannel {
                    participant1,
                    participant2,
                    deposits,
                    status: stored.status.into(),
                },
            );
        }
        Ok(EvmChannelIndex {
            path: Some(path.to_path_buf()),
            indexed_contract,
            rejected: None,
            state: RwLock::new(IndexState {
                last_indexed_block: snapshot.last_indexed_block,
                channels,
            }),
        })
    }

    /// Why the snapshot on disk was not taken as this index's checkpoint,
    /// or `None` when it was taken (or there was none to take) -- the same
    /// verdict [`Self::open`]'s `warn!` renders in prose, kept as a value
    /// so a test can say *which* rule fired rather than grepping a log
    /// line. The operator-facing channel is that `warn!`; this is on no
    /// operator surface.
    pub fn rejected_snapshot(&self) -> Option<RejectedSnapshot> {
        self.rejected
    }

    /// The last block this index has fully applied. `None` for a fresh
    /// index with no checkpoint -- the caller (`channel_index_sync`) treats
    /// that as "start from `channel_index_from_block`", never as "block 0
    /// is caught up".
    pub fn last_indexed_block(&self) -> Option<u64> {
        self.state
            .read()
            .expect("channel index lock poisoned")
            .last_indexed_block
    }

    /// Resolve `channel_id` from this index alone -- no chain read, however
    /// the answer comes out. `own_address` is this connector's own signing
    /// address, so an [`ChannelIndexLookup::Active`] answer can name the
    /// *other* participant as the counterparty; a channel that does not
    /// include `own_address` at all is reported as [`ChannelIndexLookup::Miss`]
    /// -- it is real, but it is not a channel this connector can be paid on,
    /// and the existing chain-read fallback already encodes that exact rule
    /// (`EvmSettlementBackend::channel_counterparty`), so deferring to it
    /// here keeps the two paths agreeing rather than each inventing the
    /// check separately.
    pub fn lookup(&self, channel_id: &[u8; 32], own_address: Address) -> ChannelIndexLookup {
        let state = self.state.read().expect("channel index lock poisoned");
        let Some(channel) = state.channels.get(channel_id) else {
            return ChannelIndexLookup::Miss;
        };
        let counterparty = if channel.participant1 == own_address {
            channel.participant2
        } else if channel.participant2 == own_address {
            channel.participant1
        } else {
            return ChannelIndexLookup::Miss;
        };
        if channel.status.is_terminal() {
            return ChannelIndexLookup::Terminal;
        }
        let deposit = channel
            .deposits
            .get(&counterparty)
            .copied()
            .unwrap_or_default();
        ChannelIndexLookup::Active {
            counterparty,
            deposit,
        }
    }

    /// Fold `events` into this index and advance its checkpoint to
    /// `up_to_block`, persisting the result if a `state_dir` is configured.
    ///
    /// `events` need not already be sorted -- this applies them in
    /// `(block_number, log_index)` order regardless of what order the
    /// caller's several per-event-type queries returned them in, which
    /// matters when two different event types land in the same block range
    /// (e.g. a `ChannelNewDeposit` immediately followed by a `ChannelSettled`
    /// for the same channel).
    ///
    /// An event for a channel this index has not seen `Opened` for yet (a
    /// `NewDeposit`/`Settled` with no matching `Opened` in
    /// this or an earlier batch) is dropped rather than applied -- it cannot
    /// happen against a correctly-ordered, complete log stream, since
    /// `TokenNetwork` itself refuses every one of those calls before a
    /// channel exists, so this is defense against a malformed batch rather
    /// than a case this index expects to hit.
    pub fn apply(
        &self,
        mut events: Vec<OrderedChannelIndexEvent>,
        up_to_block: u64,
    ) -> Result<(), EvmChannelIndexError> {
        events.sort_by_key(|event| (event.block_number, event.log_index));
        let mut state = self.state.write().expect("channel index lock poisoned");
        for ordered in events {
            match ordered.event {
                ChannelIndexEvent::Opened {
                    channel_id,
                    participant1,
                    participant2,
                } => {
                    state.channels.insert(
                        channel_id,
                        IndexedChannel {
                            participant1,
                            participant2,
                            deposits: HashMap::new(),
                            status: IndexedChannelStatus::Open,
                        },
                    );
                }
                ChannelIndexEvent::NewDeposit {
                    channel_id,
                    participant,
                    total_deposit,
                } => {
                    if let Some(channel) = state.channels.get_mut(&channel_id) {
                        channel.deposits.insert(participant, total_deposit);
                    }
                }
                ChannelIndexEvent::Settled { channel_id } => {
                    if let Some(channel) = state.channels.get_mut(&channel_id) {
                        channel.status = IndexedChannelStatus::Settled;
                    }
                }
            }
        }
        state.last_indexed_block = Some(up_to_block);

        let Some(path) = &self.path else {
            return Ok(());
        };
        let snapshot = snapshot_of(&state, self.indexed_contract);
        // Dropped before any I/O: persisting must not hold up a lookup
        // racing in on the read side.
        drop(state);
        persist_snapshot(path, &snapshot)
    }
}

/// `state` in the durable snapshot's own shape. Channels, and each
/// channel's deposits, are sorted by their string key so two runs over the
/// same state produce byte-identical files -- a `HashMap`'s iteration order
/// is not stable across runs, and an operator diffing this file (or reading
/// it under `jq`) should see the table change only when the table changed.
fn snapshot_of(state: &IndexState, indexed_contract: IndexedContract) -> Snapshot {
    let mut channels: Vec<StoredChannel> = state
        .channels
        .iter()
        .map(|(channel_id, channel)| {
            let mut deposits: Vec<StoredDeposit> = channel
                .deposits
                .iter()
                .map(|(participant, amount)| StoredDeposit {
                    participant: format_address(*participant),
                    deposit: amount.to_string(),
                })
                .collect();
            deposits.sort_by(|a, b| a.participant.cmp(&b.participant));
            StoredChannel {
                channel_id: format_channel_id(*channel_id),
                participant1: format_address(channel.participant1),
                participant2: format_address(channel.participant2),
                deposits,
                status: channel.status.into(),
            }
        })
        .collect();
    channels.sort_by(|a, b| a.channel_id.cmp(&b.channel_id));
    Snapshot {
        last_indexed_block: state.last_indexed_block,
        chain_id: Some(indexed_contract.chain_id),
        token_network: Some(format_address(indexed_contract.token_network)),
        channels,
    }
}

/// Rule 1 of issue #1282, in one place and with no I/O: does this snapshot
/// say it is about the chain and contract this index is about? A snapshot
/// that says nothing -- either field absent, which is the pre-#1282 file --
/// says "no".
///
/// A recorded address that does not parse is [`EvmChannelIndexError::Corrupt`]
/// rather than a mismatch, the same answer this file's channel addresses
/// have always given: an unreadable snapshot is a different failure from a
/// readable one belonging to somebody else, and only the second is
/// something a restart resolves.
fn reject_snapshot(
    snapshot: &Snapshot,
    indexed_contract: IndexedContract,
    from_block: u64,
    path: &Path,
) -> Result<Option<RejectedSnapshot>, EvmChannelIndexError> {
    let (Some(chain_id), Some(token_network)) =
        (snapshot.chain_id, snapshot.token_network.as_ref())
    else {
        return Ok(Some(RejectedSnapshot::Unbound));
    };
    if chain_id != indexed_contract.chain_id {
        return Ok(Some(RejectedSnapshot::ChainId {
            recorded: chain_id,
            configured: indexed_contract.chain_id,
        }));
    }
    let token_network = parse_address(token_network, path)?;
    if token_network != indexed_contract.token_network {
        return Ok(Some(RejectedSnapshot::TokenNetwork {
            recorded: token_network,
            configured: indexed_contract.token_network,
        }));
    }
    // Rule 2, checked last only because a mismatched identity is the more
    // useful thing to tell an operator when both are wrong. It stands on
    // its own: a snapshot can name this very chain and this very contract
    // and still hold a checkpoint from before the contract existed.
    if let Some(checkpoint) = snapshot.last_indexed_block {
        if checkpoint < from_block {
            return Ok(Some(RejectedSnapshot::BeforeStartBlock {
                checkpoint,
                from_block,
            }));
        }
    }
    Ok(None)
}

/// The operator-facing half of a rejection. Deliberately one `warn!` per
/// node start rather than a silent re-backfill: the symptom an operator
/// sees otherwise is a syncer walking millions of blocks for no reason,
/// which looks like a slow RPC rather than a discarded index.
fn report_rejected_snapshot(
    path: &Path,
    rejected: RejectedSnapshot,
    indexed_contract: IndexedContract,
    from_block: u64,
) {
    let reason = match rejected {
        RejectedSnapshot::Unbound => "it records no chain id or TokenNetwork at all (it was \
             written before the connector recorded either), so it cannot be shown to be this \
             deployment's"
            .to_string(),
        RejectedSnapshot::ChainId {
            recorded,
            configured,
        } => format!(
            "it was written against chain id {recorded} and this node now settles on chain id \
             {configured}"
        ),
        RejectedSnapshot::TokenNetwork {
            recorded,
            configured,
        } => format!(
            "it was written against TokenNetwork {recorded:#x} and this node's configured \
             registry now resolves TokenNetwork {configured:#x} -- a registry re-pointing its \
             TokenNetwork does this as surely as a chain cutover does"
        ),
        RejectedSnapshot::BeforeStartBlock {
            checkpoint,
            from_block,
        } => format!(
            "its checkpoint is block {checkpoint}, below the configured \
             channel_index_from_block of {from_block}, so it predates the contract this node \
             indexes"
        ),
    };
    tracing::warn!(
        path = %path.display(),
        chain_id = indexed_contract.chain_id,
        token_network = %format_address(indexed_contract.token_network),
        channel_index_from_block = from_block,
        "discarding the local EVM channel index rather than resuming it: {reason}. This node \
         is re-backfilling from channel_index_from_block; channel lookups fall through to \
         direct chain reads until it catches up. Nothing is lost -- this index is rebuildable \
         from chain -- but the old file is not repaired, only ignored (issue #1282)"
    );
}

fn persist_snapshot(path: &Path, snapshot: &Snapshot) -> Result<(), EvmChannelIndexError> {
    let text = serde_json::to_string_pretty(snapshot)
        .expect("a channel index snapshot always serializes to JSON");
    let tmp_path = path.with_extension("json.tmp");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| EvmChannelIndexError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut file = fs::File::create(&tmp_path).map_err(|source| EvmChannelIndexError::Io {
        path: tmp_path.clone(),
        source,
    })?;
    file.write_all(text.as_bytes())
        .map_err(|source| EvmChannelIndexError::Io {
            path: tmp_path.clone(),
            source,
        })?;
    file.sync_all().map_err(|source| EvmChannelIndexError::Io {
        path: tmp_path.clone(),
        source,
    })?;
    fs::rename(&tmp_path, path).map_err(|source| EvmChannelIndexError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(byte: u8) -> Address {
        Address::from([byte; 20])
    }

    fn channel_id(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn indexed_contract(chain_id: u64, token_network: Address) -> IndexedContract {
        IndexedContract {
            chain_id,
            token_network,
        }
    }

    fn opened(block: u64, id: [u8; 32], p1: Address, p2: Address) -> OrderedChannelIndexEvent {
        OrderedChannelIndexEvent {
            block_number: block,
            log_index: 0,
            event: ChannelIndexEvent::Opened {
                channel_id: id,
                participant1: p1,
                participant2: p2,
            },
        }
    }

    #[test]
    fn a_channel_this_index_has_never_seen_is_a_miss() {
        let index =
            EvmChannelIndex::open(None, indexed_contract(31_337, address(0xCC)), 0).expect("open");
        assert_eq!(
            index.lookup(&channel_id(1), address(0xAA)),
            ChannelIndexLookup::Miss
        );
    }

    #[test]
    fn an_opened_channel_resolves_active_with_a_zero_deposit_before_any_deposit_event() {
        let index =
            EvmChannelIndex::open(None, indexed_contract(31_337, address(0xCC)), 0).expect("open");
        let own = address(0xAA);
        let counterparty = address(0xBB);
        index
            .apply(vec![opened(10, channel_id(1), own, counterparty)], 10)
            .expect("apply");

        assert_eq!(
            index.lookup(&channel_id(1), own),
            ChannelIndexLookup::Active {
                counterparty,
                deposit: U256::zero(),
            }
        );
        assert_eq!(index.last_indexed_block(), Some(10));
    }

    #[test]
    fn a_channel_not_naming_own_address_is_a_miss_even_though_it_exists() {
        let index =
            EvmChannelIndex::open(None, indexed_contract(31_337, address(0xCC)), 0).expect("open");
        index
            .apply(
                vec![opened(10, channel_id(1), address(0x01), address(0x02))],
                10,
            )
            .expect("apply");
        assert_eq!(
            index.lookup(&channel_id(1), address(0xAA)),
            ChannelIndexLookup::Miss
        );
    }

    #[test]
    fn a_new_deposit_raises_the_reported_ceiling_for_the_depositing_participant() {
        let index =
            EvmChannelIndex::open(None, indexed_contract(31_337, address(0xCC)), 0).expect("open");
        let own = address(0xAA);
        let counterparty = address(0xBB);
        index
            .apply(vec![opened(10, channel_id(1), own, counterparty)], 10)
            .expect("apply");
        index
            .apply(
                vec![OrderedChannelIndexEvent {
                    block_number: 11,
                    log_index: 0,
                    event: ChannelIndexEvent::NewDeposit {
                        channel_id: channel_id(1),
                        participant: counterparty,
                        total_deposit: U256::from(500u64),
                    },
                }],
                11,
            )
            .expect("apply");

        assert_eq!(
            index.lookup(&channel_id(1), own),
            ChannelIndexLookup::Active {
                counterparty,
                deposit: U256::from(500u64),
            }
        );
    }

    #[test]
    fn a_deposit_by_this_node_itself_does_not_change_the_counterpartys_reported_deposit() {
        let index =
            EvmChannelIndex::open(None, indexed_contract(31_337, address(0xCC)), 0).expect("open");
        let own = address(0xAA);
        let counterparty = address(0xBB);
        index
            .apply(vec![opened(10, channel_id(1), own, counterparty)], 10)
            .expect("apply");
        index
            .apply(
                vec![OrderedChannelIndexEvent {
                    block_number: 11,
                    log_index: 0,
                    event: ChannelIndexEvent::NewDeposit {
                        channel_id: channel_id(1),
                        participant: own,
                        total_deposit: U256::from(999u64),
                    },
                }],
                11,
            )
            .expect("apply");

        assert_eq!(
            index.lookup(&channel_id(1), own),
            ChannelIndexLookup::Active {
                counterparty,
                deposit: U256::zero(),
            }
        );
    }

    #[test]
    fn a_settled_channel_is_reported_terminal_not_active_and_not_miss() {
        let index =
            EvmChannelIndex::open(None, indexed_contract(31_337, address(0xCC)), 0).expect("open");
        let own = address(0xAA);
        let counterparty = address(0xBB);
        index
            .apply(vec![opened(10, channel_id(1), own, counterparty)], 10)
            .expect("apply");
        index
            .apply(
                vec![OrderedChannelIndexEvent {
                    block_number: 20,
                    log_index: 0,
                    event: ChannelIndexEvent::Settled {
                        channel_id: channel_id(1),
                    },
                }],
                20,
            )
            .expect("apply");

        assert_eq!(
            index.lookup(&channel_id(1), own),
            ChannelIndexLookup::Terminal
        );
    }

    /// Events are applied in chain order regardless of the order they are
    /// handed in, so a deposit followed by a settlement in the same batch
    /// cannot be applied as settlement-then-deposit and end up reporting an
    /// active channel that is actually terminal.
    #[test]
    fn events_out_of_order_in_the_batch_are_applied_in_block_order() {
        let index =
            EvmChannelIndex::open(None, indexed_contract(31_337, address(0xCC)), 0).expect("open");
        let own = address(0xAA);
        let counterparty = address(0xBB);
        index
            .apply(
                vec![
                    OrderedChannelIndexEvent {
                        block_number: 12,
                        log_index: 0,
                        event: ChannelIndexEvent::Settled {
                            channel_id: channel_id(1),
                        },
                    },
                    opened(10, channel_id(1), own, counterparty),
                    OrderedChannelIndexEvent {
                        block_number: 11,
                        log_index: 0,
                        event: ChannelIndexEvent::NewDeposit {
                            channel_id: channel_id(1),
                            participant: counterparty,
                            total_deposit: U256::from(500u64),
                        },
                    },
                ],
                12,
            )
            .expect("apply");

        assert_eq!(
            index.lookup(&channel_id(1), own),
            ChannelIndexLookup::Terminal
        );
    }

    #[test]
    fn a_deposit_or_settlement_with_no_prior_opened_event_is_dropped_not_applied() {
        let index =
            EvmChannelIndex::open(None, indexed_contract(31_337, address(0xCC)), 0).expect("open");
        index
            .apply(
                vec![OrderedChannelIndexEvent {
                    block_number: 5,
                    log_index: 0,
                    event: ChannelIndexEvent::NewDeposit {
                        channel_id: channel_id(1),
                        participant: address(0xBB),
                        total_deposit: U256::from(500u64),
                    },
                }],
                5,
            )
            .expect("apply");

        assert_eq!(
            index.lookup(&channel_id(1), address(0xAA)),
            ChannelIndexLookup::Miss
        );
    }

    #[test]
    fn opening_a_path_that_does_not_exist_yet_starts_with_no_checkpoint_and_an_empty_table() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("evm-channel-index.json");
        let index = EvmChannelIndex::open(Some(&path), indexed_contract(31_337, address(0xCC)), 0)
            .expect("open");
        assert_eq!(index.last_indexed_block(), None);
        assert_eq!(
            index.lookup(&channel_id(1), address(0xAA)),
            ChannelIndexLookup::Miss
        );
    }

    #[test]
    fn a_persisted_index_is_read_back_identically_after_a_restart() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("evm-channel-index.json");
        let own = address(0xAA);
        let counterparty = address(0xBB);
        {
            let index =
                EvmChannelIndex::open(Some(&path), indexed_contract(31_337, address(0xCC)), 0)
                    .expect("open");
            index
                .apply(vec![opened(10, channel_id(1), own, counterparty)], 10)
                .expect("apply");
            index
                .apply(
                    vec![OrderedChannelIndexEvent {
                        block_number: 11,
                        log_index: 0,
                        event: ChannelIndexEvent::NewDeposit {
                            channel_id: channel_id(1),
                            participant: counterparty,
                            total_deposit: U256::from(500u64),
                        },
                    }],
                    11,
                )
                .expect("apply");
        }

        let reopened =
            EvmChannelIndex::open(Some(&path), indexed_contract(31_337, address(0xCC)), 0)
                .expect("re-open");
        assert_eq!(reopened.last_indexed_block(), Some(11));
        assert_eq!(
            reopened.lookup(&channel_id(1), own),
            ChannelIndexLookup::Active {
                counterparty,
                deposit: U256::from(500u64),
            }
        );
    }

    #[test]
    fn a_restart_resumes_from_the_checkpoint_rather_than_rescanning() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("evm-channel-index.json");
        {
            let index =
                EvmChannelIndex::open(Some(&path), indexed_contract(31_337, address(0xCC)), 0)
                    .expect("open");
            index
                .apply(
                    vec![opened(500, channel_id(1), address(0xAA), address(0xBB))],
                    500,
                )
                .expect("apply");
        }
        let reopened =
            EvmChannelIndex::open(Some(&path), indexed_contract(31_337, address(0xCC)), 0)
                .expect("re-open");
        assert_eq!(reopened.last_indexed_block(), Some(500));
    }

    #[test]
    fn an_empty_snapshot_file_reads_back_as_an_empty_index() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("evm-channel-index.json");
        fs::write(&path, "").expect("write empty file");

        let index = EvmChannelIndex::open(Some(&path), indexed_contract(31_337, address(0xCC)), 0)
            .expect("open");
        assert_eq!(index.last_indexed_block(), None);
    }

    #[test]
    fn corrupt_json_is_a_named_error_not_a_silent_empty_index() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("evm-channel-index.json");
        fs::write(&path, "{not json").expect("write garbage");

        let error = EvmChannelIndex::open(Some(&path), indexed_contract(31_337, address(0xCC)), 0)
            .expect_err("garbage must not open");
        assert!(matches!(error, EvmChannelIndexError::Corrupt { .. }));
    }

    #[test]
    fn persisting_again_overwrites_the_snapshot_rather_than_appending() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("evm-channel-index.json");
        let index = EvmChannelIndex::open(Some(&path), indexed_contract(31_337, address(0xCC)), 0)
            .expect("open");
        index
            .apply(
                vec![opened(1, channel_id(1), address(0xAA), address(0xBB))],
                1,
            )
            .expect("first apply");
        index
            .apply(
                vec![OrderedChannelIndexEvent {
                    block_number: 2,
                    log_index: 0,
                    event: ChannelIndexEvent::Settled {
                        channel_id: channel_id(1),
                    },
                }],
                2,
            )
            .expect("second apply");

        let reopened =
            EvmChannelIndex::open(Some(&path), indexed_contract(31_337, address(0xCC)), 0)
                .expect("re-open");
        assert_eq!(reopened.last_indexed_block(), Some(2));
        assert_eq!(
            reopened.lookup(&channel_id(1), address(0xAA)),
            ChannelIndexLookup::Terminal
        );
    }

    /// Issue #1282: a state volume carried from one chain to another holds a
    /// snapshot of the old chain's channels at the old chain's block height.
    /// Resuming it is how a Base Sepolia checkpoint at block 46,256,398 came
    /// to be resumed against Base mainnet.
    #[test]
    fn a_snapshot_recording_a_different_chain_id_is_not_a_checkpoint() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("evm-channel-index.json");
        let token_network = address(0xCC);
        {
            let index =
                EvmChannelIndex::open(Some(&path), indexed_contract(84_532, token_network), 0)
                    .expect("open");
            index
                .apply(
                    vec![opened(
                        46_256_398,
                        channel_id(1),
                        address(0xAA),
                        address(0xBB),
                    )],
                    46_256_398,
                )
                .expect("apply");
        }

        let reopened =
            EvmChannelIndex::open(Some(&path), indexed_contract(8_453, token_network), 0)
                .expect("re-open");

        assert_eq!(reopened.last_indexed_block(), None);
        assert_eq!(
            reopened.lookup(&channel_id(1), address(0xAA)),
            ChannelIndexLookup::Miss
        );
        assert_eq!(
            reopened.rejected_snapshot(),
            Some(RejectedSnapshot::ChainId {
                recorded: 84_532,
                configured: 8_453,
            })
        );
    }

    /// The same rule on the other half of the identity. The registry is
    /// what the config names and the `TokenNetwork` is what the registry
    /// resolves, so this fires on a registry re-pointing its token network
    /// as well as on a chain cutover -- both leave a table of channels the
    /// contract now being indexed has never heard of.
    #[test]
    fn a_snapshot_recording_a_different_token_network_is_not_a_checkpoint() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("evm-channel-index.json");
        let was = address(0xC1);
        let now = address(0xC2);
        {
            let index =
                EvmChannelIndex::open(Some(&path), indexed_contract(8_453, was), 0).expect("open");
            index
                .apply(
                    vec![opened(500, channel_id(1), address(0xAA), address(0xBB))],
                    500,
                )
                .expect("apply");
        }

        let reopened =
            EvmChannelIndex::open(Some(&path), indexed_contract(8_453, now), 0).expect("re-open");

        assert_eq!(reopened.last_indexed_block(), None);
        assert_eq!(
            reopened.lookup(&channel_id(1), address(0xAA)),
            ChannelIndexLookup::Miss
        );
        assert_eq!(
            reopened.rejected_snapshot(),
            Some(RejectedSnapshot::TokenNetwork {
                recorded: was,
                configured: now,
            })
        );
    }

    /// The file every node running before issue #1282 left in its state
    /// volume, written out here in the exact shape that code produced --
    /// `last_indexed_block` and `channels`, and nothing else. Upgrading must
    /// not be a manual state-volume edit, so this is unmatched rather than
    /// corrupt: the node starts, re-backfills, and says why.
    #[test]
    fn a_snapshot_written_before_the_identity_was_recorded_is_unmatched_not_an_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("evm-channel-index.json");
        fs::write(
            &path,
            r#"{
  "last_indexed_block": 46256398,
  "channels": [
    {
      "channel_id": "0x0101010101010101010101010101010101010101010101010101010101010101",
      "participant1": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "participant2": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "deposits": [],
      "status": "settled"
    }
  ]
}"#,
        )
        .expect("write a pre-#1282 snapshot");

        let index = EvmChannelIndex::open(
            Some(&path),
            indexed_contract(8_453, address(0xCC)),
            50_745_815,
        )
        .expect("a pre-#1282 snapshot must open, not error");

        assert_eq!(index.last_indexed_block(), None);
        assert_eq!(index.rejected_snapshot(), Some(RejectedSnapshot::Unbound));
        // And the settled channel it held is gone with it -- a `Terminal`
        // is answered from this table with no chain read at all, so keeping
        // the table while dropping the checkpoint would keep exactly the
        // wrong half.
        assert_eq!(
            index.lookup(&channel_id(1), address(0xAA)),
            ChannelIndexLookup::Miss
        );
    }

    /// Rule 2, and it holds independently of rule 1: this snapshot names
    /// the right chain and the right contract, and is still wrong, because
    /// `channel_index_from_block` is the operator asserting that the
    /// contract did not exist before that block.
    #[test]
    fn a_checkpoint_below_the_configured_start_block_is_discarded_even_when_the_identity_matches() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("evm-channel-index.json");
        let token_network = address(0xCC);
        {
            let index =
                EvmChannelIndex::open(Some(&path), indexed_contract(8_453, token_network), 0)
                    .expect("open");
            index
                .apply(
                    vec![opened(
                        46_256_398,
                        channel_id(1),
                        address(0xAA),
                        address(0xBB),
                    )],
                    46_256_398,
                )
                .expect("apply");
        }

        let reopened = EvmChannelIndex::open(
            Some(&path),
            indexed_contract(8_453, token_network),
            50_745_815,
        )
        .expect("re-open");

        assert_eq!(reopened.last_indexed_block(), None);
        assert_eq!(
            reopened.rejected_snapshot(),
            Some(RejectedSnapshot::BeforeStartBlock {
                checkpoint: 46_256_398,
                from_block: 50_745_815,
            })
        );
    }

    /// The boundary the rule above is drawn at: the start block itself is
    /// "at or above", not below, so a node that has indexed exactly its
    /// configured first block keeps its checkpoint.
    #[test]
    fn a_checkpoint_exactly_at_the_configured_start_block_is_still_a_checkpoint() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("evm-channel-index.json");
        let token_network = address(0xCC);
        {
            let index =
                EvmChannelIndex::open(Some(&path), indexed_contract(8_453, token_network), 0)
                    .expect("open");
            index
                .apply(
                    vec![opened(
                        50_745_815,
                        channel_id(1),
                        address(0xAA),
                        address(0xBB),
                    )],
                    50_745_815,
                )
                .expect("apply");
        }

        let reopened = EvmChannelIndex::open(
            Some(&path),
            indexed_contract(8_453, token_network),
            50_745_815,
        )
        .expect("re-open");

        assert_eq!(reopened.last_indexed_block(), Some(50_745_815));
        assert_eq!(reopened.rejected_snapshot(), None);
    }

    /// What every rule above is read back out of. Asserted against the
    /// file's own JSON rather than only through a re-open, because this
    /// file is a surface in its own right -- the module doc's operator
    /// reading it under `jq` should be able to see which deployment it
    /// belongs to.
    #[test]
    fn the_snapshot_written_after_a_sync_records_the_chain_id_and_the_token_network() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("evm-channel-index.json");
        let index = EvmChannelIndex::open(
            Some(&path),
            indexed_contract(
                8_453,
                "0xc24a18F19F5c5B2d41B5D1Ecb4B0Ec4C0b5c4B3a"
                    .parse()
                    .unwrap(),
            ),
            0,
        )
        .expect("open");
        index
            .apply(
                vec![opened(
                    50_745_900,
                    channel_id(1),
                    address(0xAA),
                    address(0xBB),
                )],
                50_745_900,
            )
            .expect("apply");

        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read back")).expect("parse");
        assert_eq!(written["chain_id"], serde_json::json!(8_453));
        assert_eq!(
            written["token_network"],
            serde_json::json!("0xc24a18f19f5c5b2d41b5d1ecb4b0ec4c0b5c4b3a")
        );
        assert_eq!(written["last_indexed_block"], serde_json::json!(50_745_900));
    }

    #[test]
    fn a_node_with_no_state_dir_still_indexes_in_memory_for_the_life_of_the_process() {
        let index =
            EvmChannelIndex::open(None, indexed_contract(31_337, address(0xCC)), 0).expect("open");
        let own = address(0xAA);
        let counterparty = address(0xBB);
        index
            .apply(vec![opened(10, channel_id(1), own, counterparty)], 10)
            .expect("apply");
        assert_eq!(
            index.lookup(&channel_id(1), own),
            ChannelIndexLookup::Active {
                counterparty,
                deposit: U256::zero(),
            }
        );
    }
}
