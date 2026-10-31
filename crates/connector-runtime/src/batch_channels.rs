//! This node's x402 `batch-settlement` channels as the operator surface
//! drives them (ADR 0075 decisions 8 and 11): the **outbound** ones it pays
//! on, which [`OutboundChannels`] opens, funds, withdraws from and signs on
//! and journals, and the **inbound** ones it is paid on, which
//! [`BatchChannels`] reads and lands the latest held voucher on.
//!
//! # The outbound journal (ADR 0075 decision 8)
//!
//! Nothing on chain can rebuild an outbound channel: on EVM the contract
//! keeps a `ChannelConfig` only as a hash, and on Solana the channel's
//! address is a PDA over a random salt and the slot its `open` was built
//! at. So [`OutboundChannels`] keeps its own journal, a file of its own
//! beside the claim books', and writes to it in this order:
//!
//! 1. `OutboundChannelOpening`, carrying the settlement port's
//!    [`OutboundChannelRecord`] as version 2, naming the network it is on
//!    (ADR 0086 decision 10), **before** the opening transaction is sent;
//!    a version-1 record an earlier build wrote is journaled again as
//!    version 2 at boot, once [`OutboundChannels::restore`] proves its
//!    network;
//! 2. `OutboundChannelOpened` once the chain holds the channel, or
//!    `OutboundChannelAbandoned` once it provably never will (a Solana
//!    `open` whose blockhash expired);
//! 3. `OutboundVoucherSigned` for every voucher signed, **before** the
//!    voucher is handed to anyone; and `OutboundWatermarkSet`, which a replay
//!    sets rather than folds by max, when the receiver reports less than was
//!    signed after a forward ended in a reject (issue #1446).
//!
//! What each crash leaves, and what the next attempt does:
//!
//! | crash between                      | journal holds | the chain holds | next boot / retry                                                    |
//! | ---------------------------------- | ------------- | --------------- | -------------------------------------------------------------------- |
//! | journaling and sending             | opening       | nothing         | a retried open to the same receiver sends **this** record            |
//! | sending and confirmation           | opening       | maybe           | boot adopts it if landed; a retry re-sends a form that lands once    |
//! | confirmation and `opened`          | opening       | the channel     | boot restores it and journals `opened`; a retry adopts it            |
//! | signing and `voucher_signed`       | the old mark  | --              | the voucher was never handed out, so the old watermark is honest     |
//!
//! "A form that lands once": on EVM the opening deposit's collector nonce is
//! derived from the config's salt, so the same authorisation is spent at
//! most once; on Solana the journaled bytes are the payer-signed `open`
//! itself, and a signature lands once. A record therefore opens one channel
//! with one opening deposit however many times it is sent -- the settlement
//! port's contract suite holds all three implementations to that.
//!
//! A restart restores every channel with the watermark the journal ends on
//! -- the highest voucher, unless a later `OutboundWatermarkSet` lowered it
//! -- never below what the chain shows landed. A node that lost this journal entirely restores
//! its watermark from the receiver's `POST /ilp/claim-state` instead, which
//! is the peering's to wire (issue #1378).

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard};

use connector_domain::x402::X402BatchSettlementTerms;
use connector_domain::{AssetChain, JournalEntry, Network};
use connector_settlement::batch::{
    BatchChannelState, BatchChannelStatus, BatchSettlementBackend, BatchSettlementError,
    BatchSettlementPayer, ChannelPresentation, EvmReceiverTerms, HeldVouchers, Landed,
    OutboundChannelRecord, OutboundChannelState, ReceiverTerms, SolanaReceiverTerms, Voucher,
    VoucherSigner, XrplReceiverTerms,
};
use connector_settlement::ChannelId;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::journal::{Journal, JournalError};
use crate::operator_view::{ClaimBookKind, ClaimDirection, ClaimScheme, ClaimView};
use crate::SettlementChain;

/// The `scheme` every row here carries: an x402 voucher's, as the wire names it.
const BATCH_SETTLEMENT: &str = "batch-settlement";

/// Which way value moves on a channel, as this node sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelDirection {
    /// This node is the receiver: the counterparty's vouchers pay it.
    Inbound,
    /// This node is the payer: its vouchers pay the counterparty.
    Outbound,
}

/// Where a batch-settlement channel stands, in the operator's spelling: the
/// chain's own status ([`BatchChannelStatus`]), and the two states only this
/// node can know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BatchChannelViewStatus {
    /// Outbound only: journaled, and not yet known to be on chain. A
    /// retried `POST /channels` toward the same receiver resumes it.
    Opening,
    Open,
    Withdrawing,
    Closing,
    Sealed,
    /// The chain could not be read, or no longer holds the channel;
    /// `detail` says which.
    Unreadable,
}

impl From<BatchChannelStatus> for BatchChannelViewStatus {
    fn from(status: BatchChannelStatus) -> Self {
        match status {
            BatchChannelStatus::Open => BatchChannelViewStatus::Open,
            BatchChannelStatus::Withdrawing => BatchChannelViewStatus::Withdrawing,
            BatchChannelStatus::Closing => BatchChannelViewStatus::Closing,
            BatchChannelStatus::Sealed => BatchChannelViewStatus::Sealed,
        }
    }
}

/// An x402 `batch-settlement` channel as `GET /channels` reports it (ADR
/// 0075 decision 11): its direction, collateral, watermark and status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchChannelView {
    /// EVM: `0x` and 64 lowercase hex. Solana: the channel account, base58.
    pub id: String,
    /// Always `"batch-settlement"`: what tells this row from a
    /// `toon-channel` one in the same list.
    pub scheme: String,
    /// `"evm"` or `"solana"`.
    pub chain: String,
    /// The CAIP-2 network the channel is on (ADR 0086), and so where its
    /// collateral sits. Absent only for a channel no configured network
    /// reproduces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    pub direction: ChannelDirection,
    pub status: BatchChannelViewStatus,
    /// Outbound: the receiver this node pays. Inbound: the voucher signer
    /// that pays this node. `0x` hex on EVM, base58 on Solana.
    pub counterparty: String,
    /// What still backs a new voucher above `landed`, read from the chain
    /// now. Absent while opening, or when the chain could not be read.
    pub collateral: Option<u128>,
    /// What the receiver has landed on chain.
    pub landed: Option<u128>,
    /// Outbound: the highest amount this node has signed a voucher for.
    /// Inbound: the highest voucher it has accepted.
    pub watermark: u128,
    /// Why the status is `unreadable`, when it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// What `POST /channels/:id/land` answers: the channel as
/// [`BatchChannelView`] reports it, plus the transaction that landed the
/// voucher. A separate type so that `GET /channels` keeps its shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LandedChannel {
    #[serde(flatten)]
    pub view: BatchChannelView,
    /// EVM: the transaction hash, `0x` and 64 lowercase hex. Solana: the
    /// transaction signature, base58.
    pub transaction: String,
}

/// What a failed batch-settlement channel operation reports.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BatchChannelError {
    /// This node has no x402 batch-settlement backend on that chain.
    #[error("this node has no x402 batch-settlement backend on {0}")]
    NoBackend(SettlementChain),
    /// This node pays on several networks of that chain, and the open did
    /// not say which (ADR 0086): a channel is never opened on a guessed one.
    #[error(
        "this node pays on several {0} networks, and an open on that chain names the one it \
         is on"
    )]
    NetworkAmbiguous(SettlementChain),
    /// Not an outbound channel this node opened (for fund, withdraw), or
    /// not a channel it holds a voucher on (for land).
    #[error("'{0}' is not an x402 batch-settlement channel of this node's")]
    UnknownChannel(String),
    /// The channel's open is journaled and not yet known to have landed.
    #[error(
        "channel '{0}' is still opening: its opening transaction has not been seen on chain; \
         POST /channels toward the same receiver again to resume it"
    )]
    StillOpening(String),
    /// Nothing to land: no voucher is held on the channel.
    #[error("no voucher is held on channel '{0}' to land")]
    NoVoucherHeld(String),
    /// The counterparty's published terms cannot be opened on.
    #[error("the counterparty's terms cannot be opened on: {0}")]
    InvalidTerms(String),
    #[error(transparent)]
    Settlement(#[from] BatchSettlementError),
    /// The journal could not be written, so nothing further was done: an
    /// open was not sent, a voucher was not handed out.
    #[error("the outbound channel journal could not be written: {0}")]
    Journal(String),
    /// A controller's deposit limit refused the deposit (ADR 0078), before
    /// anything was journaled or sent. Carries the reason, which names it.
    #[error("{0}")]
    DepositRefused(String),
}

/// Why [`OutboundChannels::restore`] refused to replay a journal. Each is a
/// refusal to start: a channel this node cannot place is one holding its
/// deposit that it would otherwise forget.
#[derive(Debug, Error)]
pub enum OutboundRestoreError {
    #[error(transparent)]
    Journal(#[from] JournalError),
    /// ADR 0086 decision 10: the journal holds an outbound channel on no
    /// network this node settles on -- a version-1 record no configured
    /// table's domain reproduces (`network: None`), or a version-2 record
    /// naming a network no table is for. The node still holds value there,
    /// and its operator removed the network.
    #[error(
        "the outbound channel {channel_id} is on {}, and this node has no settlement table \
         for it: the node still holds value in that channel. Restore the table for the \
         network it was opened on, at least until the channel is withdrawn (ADR 0086 \
         decision 10)",
        match network {
            Some(network) => format!("{network}"),
            None => "no network this node settles on (no configured table's EIP-712 domain \
                     reproduces its channel id)".to_string(),
        }
    )]
    OnNoNetwork {
        /// The journal key, `evm:0x…` or `solana:…`.
        channel_id: String,
        network: Option<Network>,
    },
    /// ADR 0086 decision 10: a version-1 record that more than one
    /// configured table's domain reproduces. State is never assigned to a
    /// network by guess.
    #[error(
        "the outbound channel {channel_id} is reproduced by more than one settlement table \
         ({}), so which network it is on cannot be proved: state is never assigned to a \
         network by guess (ADR 0086 decision 10)",
        networks.iter().map(|network| network.to_string()).collect::<Vec<_>>().join(", ")
    )]
    OnSeveralNetworks {
        channel_id: String,
        networks: Vec<Network>,
    },
}

/// Where a deposit is counted against a controller's deposit limit (ADR 0078
/// decision 4). Asked once per deposit that commits value: when an open is
/// about to be journaled as a new channel -- never for a repeat that finds or
/// resumes one -- and before every top-up, which is an increment each time.
/// An `Err` is the reason the deposit is refused; the channel write then
/// journals and sends nothing.
pub trait DepositGate: Send + Sync {
    fn count(&self, amount: u128) -> Result<(), String>;
}

impl From<JournalError> for BatchChannelError {
    fn from(error: JournalError) -> Self {
        BatchChannelError::Journal(error.to_string())
    }
}

/// What `POST /channels/:id/withdraw` did (ADR 0075 decision 11): the one
/// lever starts a withdrawal, and finishes it when it is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WithdrawStep {
    /// EVM `initiateWithdraw`, Solana `request_close`: the receiver now has
    /// the delay in which to land its latest voucher.
    Started,
    /// EVM `finalizeWithdraw`, Solana `distribute`: what the receiver had
    /// not landed is back in this node's settlement account.
    Finished,
    /// An open channel holding nothing this node could take back.
    NothingToWithdraw,
}

/// One outbound channel, as the journal and this process know it.
#[derive(Debug, Clone)]
struct Tracked {
    chain: SettlementChain,
    /// The network whose payer the channel is on (ADR 0086): proved from
    /// the record at restore, or the one it was opened on. `None` for a
    /// record no configured network reproduces, which no payer serves.
    network: Option<Network>,
    record: OutboundChannelRecord,
    opened: bool,
    /// The highest voucher journaled on it.
    signed: u128,
    /// Whether this process's payer knows the channel yet. False after a
    /// boot whose restore could not reach the chain; the first operation
    /// on the channel restores it then.
    restored: bool,
}

/// The canonical key a channel is journaled under: `evm:0x…` or
/// `solana:…`, as the client edge keys its own. `pub(crate)` because
/// `Connector::peer_asset` (issue #1382) builds the identical key for a
/// runtime x402 peering's own channel, to read its token off
/// [`connector_config::ClientChannelAssets`] the same way a client
/// channel's is -- one key format, not two that could drift apart.
pub(crate) fn journal_key(chain: SettlementChain, channel: &ChannelId) -> String {
    format!("{}:{}", chain.name(), channel.0)
}

/// The id an operator names a channel by, in the port's spelling: an EVM id
/// in any case, with or without `0x`, is `0x` and lowercase hex.
fn canonical_id(id: &str) -> String {
    let hex = id.strip_prefix("0x").unwrap_or(id);
    if hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        format!("0x{}", hex.to_ascii_lowercase())
    } else {
        id.to_string()
    }
}

fn chain_of(name: &str) -> SettlementChain {
    SettlementChain::from_str(name).expect("the settlement port names only evm and solana")
}

/// The network among `networks` that the channel `presentation` is on (ADR
/// 0086 decisions 9 and 10): the one candidate of its chain when there is
/// one, and otherwise, on EVM, the one whose `x402BatchSettlement` domain
/// reproduces the channel id from its config -- an EVM channel id binds
/// its chain, so at most one does. `None` when no network of its chain is
/// given, or (EVM) when none or several reproduce it.
pub fn network_of_presentation<'a>(
    presentation: &ChannelPresentation,
    networks: impl IntoIterator<Item = &'a Network>,
) -> Option<Network> {
    let family = match presentation {
        ChannelPresentation::Evm { .. } => AssetChain::Evm,
        ChannelPresentation::Solana { .. } => AssetChain::Solana,
        ChannelPresentation::Xrpl { .. } => AssetChain::Xrpl,
    };
    let candidates: Vec<&Network> = networks
        .into_iter()
        .filter(|network| network.chain() == family)
        .collect();
    if let [only] = candidates.as_slice() {
        return Some((*only).clone());
    }
    if family != AssetChain::Evm {
        return None;
    }
    match <[Network; 1]>::try_from(networks_proving(presentation, candidates)) {
        Ok([network]) => Some(network),
        Err(_) => None,
    }
}

/// The networks among `networks` that prove the channel `presentation` is
/// theirs: on EVM each whose `x402BatchSettlement` domain reproduces the
/// channel id from its config, and on Solana each Solana network, of which
/// a node has at most one. No shortcut for a single candidate: one network
/// configured is not proof that a channel is on it.
fn networks_proving<'a>(
    presentation: &ChannelPresentation,
    networks: impl IntoIterator<Item = &'a Network>,
) -> Vec<Network> {
    let candidates = networks.into_iter();
    let (channel, config) = match presentation {
        ChannelPresentation::Solana { .. } => {
            return candidates
                .filter(|network| network.chain() == AssetChain::Solana)
                .cloned()
                .collect()
        }
        ChannelPresentation::Xrpl { .. } => {
            return candidates
                .filter(|network| network.chain() == AssetChain::Xrpl)
                .cloned()
                .collect()
        }
        ChannelPresentation::Evm { channel, config } => (channel, config),
    };
    let config = connector_signer::BatchChannelConfig {
        payer: config.payer,
        payer_authorizer: config.payer_authorizer,
        receiver: config.receiver,
        receiver_authorizer: config.receiver_authorizer,
        token: config.token,
        withdraw_delay: config.withdraw_delay,
        salt: config.salt,
    };
    candidates
        .filter(|network| {
            network.eip155_chain_id().is_some_and(|chain_id| {
                let id = connector_signer::evm_batch_channel_id(
                    &connector_signer::BatchSettlementDomain::x402(chain_id),
                    &config,
                );
                canonical_id(&channel.0) == hex(&id)
            })
        })
        .cloned()
        .collect()
}

/// Place every replayed channel on one of `payers`' networks (ADR 0086
/// decision 10), and answer the version-2 rewrite of each version-1 record
/// placed by proof -- in key order, so the rewrite is the same entries
/// however often it is done. Refuses, by name, a channel on no configured
/// network or (version 1) proved on several. A version-2 Solana record is
/// placed on the one Solana network whatever genesis it names: a local
/// cluster is reset to a fresh genesis on every start.
fn place_on_networks(
    channels: &mut BTreeMap<String, Tracked>,
    payers: &[(Network, Arc<dyn BatchSettlementPayer>)],
) -> Result<Vec<JournalEntry>, OutboundRestoreError> {
    let configured = || payers.iter().map(|(network, _)| network);
    let mut upgrades = Vec::new();
    for tracked in channels.values_mut() {
        let channel_id = journal_key(tracked.chain, tracked.record.channel());
        match tracked.network.clone() {
            Some(journaled) => {
                let placed = match journaled.chain() {
                    AssetChain::Evm => configured().find(|network| **network == journaled),
                    AssetChain::Solana => {
                        configured().find(|network| network.chain() == AssetChain::Solana)
                    }
                    // No payer settles on Mina: a zk edge's vault is not a
                    // channel this node pays from.
                    AssetChain::Mina => None,
                    // An XRPL network is placed on its exact payer, if the
                    // node has one; a record naming another is refused by
                    // name below (ADR 0087).
                    AssetChain::Xrpl => configured().find(|network| **network == journaled),
                };
                let Some(placed) = placed else {
                    return Err(OutboundRestoreError::OnNoNetwork {
                        channel_id,
                        network: Some(journaled),
                    });
                };
                tracked.network = Some(placed.clone());
            }
            None => {
                let proving = networks_proving(&tracked.record.presentation(), configured());
                let network = match <[Network; 1]>::try_from(proving) {
                    Ok([network]) => network,
                    Err(proving) if proving.is_empty() => {
                        return Err(OutboundRestoreError::OnNoNetwork {
                            channel_id,
                            network: None,
                        })
                    }
                    Err(networks) => {
                        return Err(OutboundRestoreError::OnSeveralNetworks {
                            channel_id,
                            networks,
                        })
                    }
                };
                upgrades.push(JournalEntry::OutboundChannelOpening {
                    channel_id,
                    record: tracked.record.encode_on(&network.to_string()),
                });
                tracked.network = Some(network);
            }
        }
    }
    Ok(upgrades)
}

/// `bytes` as `0x` and lowercase hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + 2 * bytes.len());
    out.push_str("0x");
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// A key or address in its chain's own spelling.
fn spell(chain: SettlementChain, bytes: &[u8]) -> String {
    match chain {
        SettlementChain::Evm => hex(bytes),
        SettlementChain::Solana => bs58::encode(bytes).into_string(),
        // A receiver is spelled as its classic address, whose bytes the
        // record keeps as UTF-8.
        SettlementChain::Xrpl => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn spell_signer(signer: &VoucherSigner) -> String {
    match signer {
        VoucherSigner::Evm(address) => hex(address),
        VoucherSigner::Solana(key) => bs58::encode(key).into_string(),
        // The channel key as ADR 0087 spells a voucher signer: 33 bytes in
        // bare lowercase hex, which `[[peer_channels]] voucher_signer` reads.
        VoucherSigner::Xrpl(key) => key.iter().map(|byte| format!("{byte:02x}")).collect(),
    }
}

/// The channels this node pays on (ADR 0075 decisions 2, 8 and 11): the
/// settlement port's paying half on each chain, and the journal that lets
/// them survive a restart. See the module doc for the journal's order and
/// what each crash leaves.
pub struct OutboundChannels {
    journal: Arc<dyn Journal>,
    /// The paying half of each network this node settles on (ADR 0086):
    /// one per EVM table, and the one Solana table.
    payers: Vec<(Network, Arc<dyn BatchSettlementPayer>)>,
    channels: Mutex<BTreeMap<String, Tracked>>,
    /// One open at a time, so two concurrent opens toward one receiver
    /// cannot both miss the other's pending record.
    opening: tokio::sync::Mutex<()>,
}

impl OutboundChannels {
    /// Replay `journal` and restore every channel it names to its
    /// network's payer. A channel whose chain cannot be read now is kept,
    /// and restored on first use; one journaled as opening that the chain
    /// holds is journaled as opened. Fails on a journal that cannot be read
    /// or a record that cannot be decoded -- a record this node cannot read
    /// is a channel it cannot find, and starting without it would strand
    /// whatever is in it -- and on a channel it cannot place on one of
    /// `payers`' networks, for the same reason.
    ///
    /// **Version-1 records are upgraded here, by proof** (ADR 0086 decision
    /// 10). A record written before ADR 0086 names no network: its channel
    /// id is recomputed under every configured EVM network's x402 domain,
    /// and the one that reproduces it is its network (a Solana record's is
    /// the one Solana network). The record is then journaled again, as
    /// version 2 under its unchanged key, which a later replay reads in
    /// place of the first. None reproducing it, or several, is refused by
    /// name before anything is written. Every rewrite is the same entry
    /// whenever it is written, and a record already rewritten is version 2,
    /// so a restart after a crash part-way through ends on the same journal.
    pub async fn restore(
        journal: Arc<dyn Journal>,
        payers: Vec<(Network, Arc<dyn BatchSettlementPayer>)>,
    ) -> Result<OutboundChannels, OutboundRestoreError> {
        let mut channels: BTreeMap<String, Tracked> = BTreeMap::new();
        let mut by_key: BTreeMap<String, String> = BTreeMap::new();
        for entry in journal.read_all()? {
            match entry {
                JournalEntry::OutboundChannelOpening { channel_id, record } => {
                    let unreadable = || {
                        JournalError::Corrupt(format!(
                            "outbound channel '{channel_id}' has a record this build cannot read"
                        ))
                    };
                    let journaled =
                        OutboundChannelRecord::decode(&record).ok_or_else(unreadable)?;
                    // Unassigned until proved, below, for a version-1 record.
                    let network = journaled
                        .network
                        .map(|network| network.parse::<Network>())
                        .transpose()
                        .map_err(|_| unreadable())?;
                    let record = journaled.record;
                    let id = record.channel().0.clone();
                    by_key.insert(channel_id, id.clone());
                    // The same channel journaled again is its record
                    // upgraded: what replay has folded into it so far stays.
                    if let Some(tracked) = channels.get_mut(&id) {
                        tracked.network = network;
                        tracked.record = record;
                        continue;
                    }
                    channels.insert(
                        id,
                        Tracked {
                            chain: chain_of(record.chain()),
                            network,
                            record,
                            opened: false,
                            signed: 0,
                            restored: false,
                        },
                    );
                }
                JournalEntry::OutboundChannelOpened { channel_id } => {
                    if let Some(tracked) =
                        by_key.get(&channel_id).and_then(|id| channels.get_mut(id))
                    {
                        tracked.opened = true;
                    }
                }
                JournalEntry::OutboundChannelAbandoned { channel_id } => {
                    if let Some(id) = by_key.remove(&channel_id) {
                        channels.remove(&id);
                    }
                }
                JournalEntry::OutboundVoucherSigned {
                    channel_id,
                    cumulative_amount,
                } => {
                    if let Some(tracked) =
                        by_key.get(&channel_id).and_then(|id| channels.get_mut(id))
                    {
                        tracked.signed = tracked.signed.max(cumulative_amount);
                    }
                }
                // Overwrites, where the voucher above folds by max: it
                // exists to move a watermark down (issue #1446).
                JournalEntry::OutboundWatermarkSet {
                    channel_id,
                    cumulative_amount,
                } => {
                    if let Some(tracked) =
                        by_key.get(&channel_id).and_then(|id| channels.get_mut(id))
                    {
                        tracked.signed = cumulative_amount;
                    }
                }
                // Another book's entry kinds never reach this file.
                _ => {}
            }
        }
        let upgrades = place_on_networks(&mut channels, &payers)?;
        journal.append_batch(&upgrades)?;
        let outbound = OutboundChannels {
            journal,
            payers,
            channels: Mutex::new(channels),
            opening: tokio::sync::Mutex::new(()),
        };
        for id in outbound.ids() {
            if let Err(error) = outbound.restore_one(&id).await {
                tracing::warn!(
                    channel = %id,
                    %error,
                    "an outbound channel the journal holds could not be restored now; it is \
                     restored on first use"
                );
            }
        }
        Ok(outbound)
    }

    fn channels(&self) -> MutexGuard<'_, BTreeMap<String, Tracked>> {
        self.channels
            .lock()
            .expect("outbound channels lock poisoned")
    }

    fn ids(&self) -> Vec<String> {
        self.channels().keys().cloned().collect()
    }

    /// The key the channel an operator names `id` is tracked under. An EVM
    /// id is keyed `0x` and lowercase, but an XRPL id -- 64 hex digits too --
    /// is keyed bare and lowercase, its port's spelling, so a 64-hex id in
    /// any spelling is tried both ways.
    fn key(&self, id: &str) -> String {
        let canonical = canonical_id(id);
        let channels = self.channels();
        if channels.contains_key(&canonical) {
            return canonical;
        }
        match canonical.strip_prefix("0x") {
            Some(bare) if channels.contains_key(bare) => bare.to_string(),
            _ => canonical,
        }
    }

    fn tracked(&self, id: &str) -> Result<Tracked, BatchChannelError> {
        self.channels()
            .get(id)
            .cloned()
            .ok_or_else(|| BatchChannelError::UnknownChannel(id.to_string()))
    }

    /// The payer of the network `tracked` is on.
    fn payer(
        &self,
        tracked: &Tracked,
    ) -> Result<&Arc<dyn BatchSettlementPayer>, BatchChannelError> {
        tracked
            .network
            .as_ref()
            .and_then(|network| self.payer_on(network))
            .ok_or(BatchChannelError::NoBackend(tracked.chain))
    }

    fn payer_on(&self, network: &Network) -> Option<&Arc<dyn BatchSettlementPayer>> {
        self.payers
            .iter()
            .find(|(configured, _)| configured == network)
            .map(|(_, payer)| payer)
    }

    /// The network an outbound channel this node journaled is on (ADR
    /// 0086): what every voucher and challenge signed on it names.
    pub fn network(&self, id: &str) -> Option<Network> {
        let id = self.key(id);
        self.channels()
            .get(&id)
            .and_then(|tracked| tracked.network.clone())
    }

    /// Every network this node pays on, in the order its payers were given.
    pub fn networks(&self) -> Vec<Network> {
        self.payers
            .iter()
            .map(|(network, _)| network.clone())
            .collect()
    }

    fn append(&self, entry: JournalEntry) -> Result<(), BatchChannelError> {
        self.journal.append(&entry).map_err(BatchChannelError::from)
    }

    /// Hand the channel `id` to its payer with its journaled watermark, and
    /// journal an open the chain turns out to hold. A channel still opening
    /// that the chain does not hold stays opening.
    async fn restore_one(&self, id: &str) -> Result<OutboundChannelState, BatchChannelError> {
        let tracked = self.tracked(id)?;
        let payer = self.payer(&tracked)?;
        let state = match payer
            .restore_outbound(&tracked.record, tracked.signed)
            .await
        {
            Err(BatchSettlementError::ChannelNotFound(_)) if !tracked.opened => {
                return Err(BatchChannelError::StillOpening(id.to_string()))
            }
            other => other?,
        };
        if !tracked.opened {
            self.append(JournalEntry::OutboundChannelOpened {
                channel_id: journal_key(tracked.chain, tracked.record.channel()),
            })?;
        }
        if let Some(tracked) = self.channels().get_mut(id) {
            tracked.opened = true;
            tracked.restored = true;
            // The payer restores the larger of the journaled mark and what
            // the chain shows landed, so this copy never lags it.
            tracked.signed = tracked.signed.max(state.signed);
        }
        Ok(state)
    }

    /// The payer of the open channel `id`, restoring it first if this
    /// process has not.
    async fn ready(
        &self,
        id: &str,
    ) -> Result<(ChannelId, Arc<dyn BatchSettlementPayer>), BatchChannelError> {
        let id = self.key(id);
        let tracked = self.tracked(&id)?;
        if !tracked.restored {
            self.restore_one(&id).await?;
        }
        Ok((
            tracked.record.channel().clone(),
            Arc::clone(self.payer(&tracked)?),
        ))
    }

    /// Open an outbound channel toward the counterparty that published
    /// `terms`, with an opening deposit of `deposit` (ADR 0075 decision
    /// 11), and say whether it resumed an earlier open.
    ///
    /// **A retried open finds the journaled channel.** If an open toward the
    /// same receiver, on the same chain and for the same deposit, is journaled and not yet known to be
    /// on chain -- a crash, a lost confirmation, a sponsor that did not
    /// answer -- this resumes **that** record rather
    /// than building a second channel. Once no such record is pending, an
    /// open is a new channel: several toward one receiver are legal (ADR
    /// 0075 decision 4).
    pub async fn open(
        &self,
        terms: ReceiverTerms,
        deposit: u128,
    ) -> Result<(OutboundChannelState, bool), BatchChannelError> {
        self.open_with(terms, deposit, None).await
    }

    /// [`OutboundChannels::open`], counting a deposit that journals a new
    /// channel with `gate` first. A resumed open counts nothing.
    pub async fn open_with(
        &self,
        terms: ReceiverTerms,
        deposit: u128,
        gate: Option<&dyn DepositGate>,
    ) -> Result<(OutboundChannelState, bool), BatchChannelError> {
        let chain = chain_of(terms.chain());
        let mut on_chain = self
            .payers
            .iter()
            .filter(|(network, _)| network.chain() == chain.asset_chain());
        let network = match (on_chain.next(), on_chain.next()) {
            (Some((network, _)), None) => network.clone(),
            (None, _) => return Err(BatchChannelError::NoBackend(chain)),
            (Some(_), Some(_)) => return Err(BatchChannelError::NetworkAmbiguous(chain)),
        };
        self.open_on(&network, terms, deposit, gate).await
    }

    /// [`OutboundChannels::open_with`] on `network`, one of the networks this
    /// node pays on (ADR 0086): the channel is opened through that network's
    /// payer, and every voucher on it names it.
    pub async fn open_on(
        &self,
        network: &Network,
        terms: ReceiverTerms,
        deposit: u128,
        gate: Option<&dyn DepositGate>,
    ) -> Result<(OutboundChannelState, bool), BatchChannelError> {
        let chain = chain_of(terms.chain());
        if network.chain() != chain.asset_chain() {
            return Err(BatchChannelError::InvalidTerms(format!(
                "they are {} terms, and the open names {network}",
                chain.name()
            )));
        }
        let payer = Arc::clone(
            self.payer_on(network)
                .ok_or(BatchChannelError::NoBackend(chain))?,
        );
        let _opening = self.opening.lock().await;
        let receiver = match &terms {
            ReceiverTerms::Evm(evm) => evm.receiver.to_vec(),
            ReceiverTerms::Solana(solana) => solana.receiver.to_vec(),
            ReceiverTerms::Xrpl(xrpl) => xrpl.receiver.as_bytes().to_vec(),
        };
        // The same open: same chain, same receiver, same deposit. A request
        // that differs is a different open, so a stale pending record --
        // one whose terms the counterparty has since changed, say -- never
        // traps every later open toward that receiver.
        let pending = self
            .channels()
            .values()
            .find(|tracked| {
                tracked.network.as_ref() == Some(network)
                    && !tracked.opened
                    && tracked.record.receiver() == receiver
                    && tracked.record.deposit() == deposit
            })
            .map(|tracked| tracked.record.clone());
        let (mut record, mut resumed) = match pending {
            Some(record) => (record, true),
            None => (
                self.prepare(&payer, network, chain, terms.clone(), deposit, gate)
                    .await?,
                false,
            ),
        };
        loop {
            match payer.open_prepared(&record).await {
                Ok(_) => break,
                // Provably never opened: forget it, and if it was an older
                // attempt being resumed, open this request's own.
                Err(BatchSettlementError::OpenLapsed(channel)) => {
                    self.append(JournalEntry::OutboundChannelAbandoned {
                        channel_id: journal_key(chain, &channel),
                    })?;
                    self.channels().remove(&channel.0);
                    if !resumed {
                        return Err(BatchSettlementError::OpenLapsed(channel).into());
                    }
                    record = self
                        .prepare(&payer, network, chain, terms.clone(), deposit, gate)
                        .await?;
                    resumed = false;
                }
                Err(error) => return Err(error.into()),
            }
        }
        let id = record.channel().0.clone();
        let state = self.restore_one(&id).await?;
        Ok((state, resumed))
    }

    /// Build a channel and journal it before anything is sent.
    async fn prepare(
        &self,
        payer: &Arc<dyn BatchSettlementPayer>,
        network: &Network,
        chain: SettlementChain,
        terms: ReceiverTerms,
        deposit: u128,
        gate: Option<&dyn DepositGate>,
    ) -> Result<OutboundChannelRecord, BatchChannelError> {
        // Counted as the channel is journaled and before the open is built,
        // so a refused deposit is neither signed nor sent; a deposit whose
        // transaction then fails is not given back.
        if let Some(gate) = gate {
            gate.count(deposit)
                .map_err(BatchChannelError::DepositRefused)?;
        }
        let record = payer.prepare_open(terms, deposit).await?;
        self.append(JournalEntry::OutboundChannelOpening {
            channel_id: journal_key(chain, record.channel()),
            record: record.encode_on(&network.to_string()),
        })?;
        self.channels().insert(
            record.channel().0.clone(),
            Tracked {
                chain,
                network: Some(network.clone()),
                record: record.clone(),
                opened: false,
                signed: 0,
                restored: false,
            },
        );
        Ok(record)
    }

    /// Add `increment` to an outbound channel's deposit: an increment,
    /// never a total (ADR 0075 decision 11).
    pub async fn top_up(
        &self,
        id: &str,
        increment: u128,
    ) -> Result<OutboundChannelState, BatchChannelError> {
        self.top_up_with(id, increment, None).await
    }

    /// [`OutboundChannels::top_up`], counting `increment` with `gate` before
    /// it is sent -- every time, because it is an increment.
    pub async fn top_up_with(
        &self,
        id: &str,
        increment: u128,
        gate: Option<&dyn DepositGate>,
    ) -> Result<OutboundChannelState, BatchChannelError> {
        let (channel, payer) = self.ready(id).await?;
        if let Some(gate) = gate {
            gate.count(increment)
                .map_err(BatchChannelError::DepositRefused)?;
        }
        Ok(payer.top_up(&channel, increment).await?)
    }

    /// Start a withdrawal (EVM) or request a close (Solana) on an outbound
    /// channel, or finish the one started once it is due (ADR 0075
    /// decision 11): one lever, whose next step the chain decides.
    pub async fn withdraw(
        &self,
        id: &str,
    ) -> Result<(WithdrawStep, OutboundChannelState), BatchChannelError> {
        let (channel, payer) = self.ready(id).await?;
        let state = payer.outbound_state(&channel).await?;
        match state.on_chain.status {
            BatchChannelStatus::Open if state.on_chain.collateral == 0 => {
                Ok((WithdrawStep::NothingToWithdraw, state))
            }
            BatchChannelStatus::Open => Ok((
                WithdrawStep::Started,
                payer.start_withdrawal(&channel).await?,
            )),
            BatchChannelStatus::Withdrawing
            | BatchChannelStatus::Closing
            | BatchChannelStatus::Sealed => Ok((
                WithdrawStep::Finished,
                payer.finish_withdrawal(&channel).await?,
            )),
        }
    }

    /// Sign a voucher for `cumulative_amount` on an outbound channel, and
    /// journal it before returning it: a voucher this returns is one a
    /// restart remembers signing. If the journal cannot be written the
    /// voucher is never handed out, so the watermark a restart restores is
    /// still an honest one.
    ///
    /// The seam the packet path signs through: a runtime EVM peering's
    /// every forward since #1378 (`Connector::cover_forward`), and
    /// `[[pay_channels]]` and client payouts once #1380 and #1381 land.
    pub async fn sign_voucher(
        &self,
        id: &str,
        cumulative_amount: u128,
    ) -> Result<Voucher, BatchChannelError> {
        let (channel, payer) = self.ready(id).await?;
        let voucher = payer.sign_voucher(&channel, cumulative_amount).await?;
        let chain = self.tracked(&channel.0)?.chain;
        self.append(JournalEntry::OutboundVoucherSigned {
            channel_id: journal_key(chain, &channel),
            cumulative_amount,
        })?;
        if let Some(tracked) = self.channels().get_mut(&channel.0) {
            tracked.signed = tracked.signed.max(cumulative_amount);
        }
        Ok(voucher)
    }

    /// Sign the voucher claim-state challenge for an outbound channel,
    /// valid until `expires` (ADR 0075 decisions 5 and 6): what this node
    /// asks the receiver's `POST /ilp/claim-state` with, and what proves the
    /// peer role on a packet that moves no value. Nothing is journaled: a
    /// challenge moves no value and advances no watermark.
    pub async fn sign_challenge(
        &self,
        id: &str,
        expires: u64,
    ) -> Result<Vec<u8>, BatchChannelError> {
        let (channel, payer) = self.ready(id).await?;
        Ok(payer.sign_claim_state_challenge(&channel, expires).await?)
    }

    /// A **live** outbound channel on `chain` toward `receiver` (its raw
    /// address or key), if this node has one: "is there a live channel with
    /// this peer?" as a lookup of this node's own channels rather than a
    /// derivation (ADR 0075 decision 4). Live is read from the chain now: a
    /// channel whose withdrawal or close has started backs nothing new, so a
    /// peering re-established after one is given a fresh channel rather than
    /// the one being wound down. Several are legal; the live one with the
    /// highest signed watermark answers, so a repeat finds the channel the
    /// peering has been paying on. A channel whose chain cannot be read now
    /// is not live, and an error is never mistaken for an absence -- it
    /// answers `Err`.
    ///
    /// Narrows [`Self::opened_toward`], the journal's own "channels toward
    /// this party", to the ones on `network` the chain still shows open: one
    /// settlement key may receive on several networks (ADR 0086 decision
    /// 5), and a channel on one never pays a peering on another.
    pub async fn live_toward(
        &self,
        receiver: &VoucherSigner,
        network: &Network,
    ) -> Result<Option<String>, BatchChannelError> {
        let mut candidates: Vec<(String, u128)> = self
            .opened_toward(receiver)
            .into_iter()
            .filter(|id| self.network(id).as_ref() == Some(network))
            .map(|id| {
                let signed = self.signed(&id).unwrap_or(0);
                (id, signed)
            })
            .collect();
        candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.1));
        for (id, _) in candidates {
            let (channel, payer) = self.ready(&id).await?;
            if payer.outbound_state(&channel).await?.on_chain.status == BatchChannelStatus::Open {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    /// Set an outbound channel's signed watermark to `amount`, the figure
    /// the receiver reports holding a voucher for (ADR 0075 decision 6: the
    /// receiver's `POST /ilp/claim-state` is the watermark authority, on
    /// restore and after a reject, issue #1446). Higher than this node's
    /// own, a node that lost its journal would otherwise sign a voucher that
    /// fails to advance; lower, a voucher the receiver never admitted paid
    /// for a packet it never carried, and the next voucher is signed above
    /// the receiver's figure rather than above that one. Never below what
    /// the chain shows landed, which the payer enforces. Returns the
    /// watermark now standing.
    ///
    /// Raising is journaled as a signed voucher before it is believed,
    /// because it is one: the receiver holds a voucher at `amount` only if
    /// this node's key signed it. Lowering is journaled as an
    /// `OutboundWatermarkSet`, which a replay sets rather than folds by max.
    ///
    /// The caller decides that lowering is safe -- that no voucher has been
    /// signed on the channel since the rejected one -- and holds the
    /// channel's signing lock across the decision and this call.
    pub async fn set_watermark(&self, id: &str, amount: u128) -> Result<u128, BatchChannelError> {
        let id = self.key(id);
        let tracked = self.tracked(&id)?;
        if amount == tracked.signed {
            return Ok(tracked.signed);
        }
        let payer = Arc::clone(self.payer(&tracked)?);
        let channel_id = journal_key(tracked.chain, tracked.record.channel());
        self.append(if amount > tracked.signed {
            JournalEntry::OutboundVoucherSigned {
                channel_id,
                cumulative_amount: amount,
            }
        } else {
            JournalEntry::OutboundWatermarkSet {
                channel_id,
                cumulative_amount: amount,
            }
        })?;
        let state = payer.restore_outbound(&tracked.record, amount).await?;
        if let Some(tracked) = self.channels().get_mut(&id) {
            tracked.signed = state.signed;
            tracked.restored = true;
        }
        Ok(state.signed)
    }

    /// The channel as its receiver is shown it -- on EVM with the config its
    /// first voucher carries.
    pub fn presentation(&self, id: &str) -> Option<ChannelPresentation> {
        let id = self.key(id);
        self.channels()
            .get(&id)
            .map(|tracked| tracked.record.presentation())
    }

    /// Whether `id` names an outbound channel this node journaled.
    pub fn knows(&self, id: &str) -> bool {
        let id = self.key(id);
        self.channels().contains_key(&id)
    }

    /// Every opened outbound channel that pays `receiver` -- EVM `receiver`,
    /// Solana the one distribution recipient -- on `receiver`'s chain, in a
    /// stable order: "is there a channel toward this party?" as a lookup of
    /// this node's own journal, not a derivation (ADR 0075 decision 4).
    /// Several are legal. A channel still opening is not listed: nothing
    /// can be signed on it yet.
    ///
    /// What a client payout is signed on (ADR 0075 decision 7, issue
    /// #1381): the channel the operator opened toward the client.
    pub fn opened_toward(&self, receiver: &VoucherSigner) -> Vec<String> {
        let (chain, receiver) = match receiver {
            VoucherSigner::Evm(address) => (SettlementChain::Evm, address.to_vec()),
            VoucherSigner::Solana(key) => (SettlementChain::Solana, key.to_vec()),
            // A channel's receiver is kept as the account id its
            // `Destination` names, which the one key behind it derives.
            VoucherSigner::Xrpl(key) => (
                SettlementChain::Xrpl,
                connector_signer::xrpl_account_id(key).to_vec(),
            ),
        };
        self.channels()
            .iter()
            .filter(|(_, tracked)| {
                tracked.opened && tracked.chain == chain && tracked.record.receiver() == receiver
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// The highest amount this node has journaled a voucher for on the
    /// outbound channel `id`: the watermark the next voucher it signs there
    /// must exceed. `None` for a channel it did not open.
    pub fn signed(&self, id: &str) -> Option<u128> {
        let id = self.key(id);
        self.channels().get(&id).map(|tracked| tracked.signed)
    }

    /// Every outbound channel, read from its chain now.
    pub async fn views(&self) -> Vec<BatchChannelView> {
        let mut views = Vec::new();
        for id in self.ids() {
            let Ok(tracked) = self.tracked(&id) else {
                continue;
            };
            let counterparty = spell(tracked.chain, &tracked.record.receiver());
            let base = BatchChannelView {
                id: id.clone(),
                scheme: BATCH_SETTLEMENT.to_string(),
                chain: tracked.chain.name().to_string(),
                network: tracked.network.as_ref().map(ToString::to_string),
                direction: ChannelDirection::Outbound,
                status: BatchChannelViewStatus::Opening,
                counterparty,
                collateral: None,
                landed: None,
                watermark: tracked.signed,
                detail: None,
            };
            // An open still pending may have landed since boot: ask the
            // chain, which journals it opened if it has.
            if !tracked.opened && self.restore_one(&id).await.is_err() {
                views.push(base);
                continue;
            }
            let read = match self.ready(&id).await {
                Ok((channel, payer)) => payer
                    .outbound_state(&channel)
                    .await
                    .map_err(BatchChannelError::from),
                Err(error) => Err(error),
            };
            views.push(match read {
                Ok(state) => BatchChannelView {
                    status: state.on_chain.status.into(),
                    collateral: Some(state.on_chain.collateral),
                    landed: Some(state.on_chain.landed),
                    watermark: state.signed,
                    ..base
                },
                Err(error) => BatchChannelView {
                    status: BatchChannelViewStatus::Unreadable,
                    detail: Some(error.to_string()),
                    ..base
                },
            });
        }
        views
    }

    /// The highest voucher signed on each outbound channel, as
    /// `GET /claims` rows.
    pub fn claims(&self) -> Vec<ClaimView> {
        self.channels()
            .values()
            .filter(|tracked| tracked.signed > 0)
            .map(|tracked| ClaimView {
                peer_id: None,
                channel_id: journal_key(tracked.chain, tracked.record.channel()),
                direction: ClaimDirection::Outbound,
                nonce: 0,
                cumulative_amount: tracked.signed,
                pending: false,
                book: ClaimBookKind::Outbound,
                scheme: ClaimScheme::BatchSettlement,
            })
            .collect()
    }
}

/// Every x402 `batch-settlement` channel of this node's, both ways, as the
/// operator surface reads and drives them (ADR 0075 decision 11).
pub struct BatchChannels {
    outbound: Arc<OutboundChannels>,
    /// The receiving half of each network this node settles on (ADR 0086),
    /// keyed by its CAIP-2 network as the greeting publishes it: a
    /// counterparty's terms must name one of them.
    receivers: Vec<(Network, Arc<dyn BatchSettlementBackend>)>,
    /// The latest voucher held on each inbound channel: the client edge's
    /// claim gate, which is where vouchers are accepted and journaled.
    held: Arc<dyn HeldVouchers>,
}

impl BatchChannels {
    pub fn new(
        outbound: Arc<OutboundChannels>,
        receivers: Vec<(Network, Arc<dyn BatchSettlementBackend>)>,
        held: Arc<dyn HeldVouchers>,
    ) -> BatchChannels {
        BatchChannels {
            outbound,
            receivers,
            held,
        }
    }

    pub fn outbound(&self) -> &Arc<OutboundChannels> {
        &self.outbound
    }

    /// The network the inbound channel `presentation` is on, as an operator
    /// reads it: the one of this node's networks whose domain reproduces it.
    fn network_of(&self, presentation: &ChannelPresentation) -> Option<String> {
        // What recorded the channel knows beats what the presentation can
        // say: an XRPL channel id binds no network.
        self.held.network_of(presentation.channel()).or_else(|| {
            network_of_presentation(presentation, self.receivers.iter().map(|(n, _)| n))
                .map(|network| network.to_string())
        })
    }

    /// The receiving half of the network the inbound channel
    /// `presentation` is on.
    fn receiver(
        &self,
        presentation: &ChannelPresentation,
    ) -> Result<&Arc<dyn BatchSettlementBackend>, BatchChannelError> {
        let network = self
            .held
            .network_of(presentation.channel())
            .and_then(|network| network.parse::<Network>().ok())
            .or_else(|| {
                network_of_presentation(presentation, self.receivers.iter().map(|(n, _)| n))
            });
        self.receivers
            .iter()
            .find(|(configured, _)| Some(configured) == network.as_ref())
            .map(|(_, backend)| backend)
            .ok_or(BatchChannelError::NoBackend(chain_of(presentation.chain())))
    }

    /// Open an outbound channel toward the counterparty that published
    /// `terms` in its self-description's `batchSettlements`, resolving a
    /// relative Solana `sponsorEndpoint` against `counterparty_url`.
    pub async fn open(
        &self,
        terms: &X402BatchSettlementTerms,
        counterparty_url: Option<&Url>,
        deposit: u128,
    ) -> Result<(BatchChannelView, bool), BatchChannelError> {
        self.open_with(terms, counterparty_url, deposit, None).await
    }

    /// [`BatchChannels::open`], counting a deposit that journals a new
    /// channel with `gate`.
    pub async fn open_with(
        &self,
        terms: &X402BatchSettlementTerms,
        counterparty_url: Option<&Url>,
        deposit: u128,
        gate: Option<&dyn DepositGate>,
    ) -> Result<(BatchChannelView, bool), BatchChannelError> {
        let (chain, network) = match terms {
            X402BatchSettlementTerms::Evm(evm) => (SettlementChain::Evm, &evm.network),
            X402BatchSettlementTerms::Solana(solana) => (SettlementChain::Solana, &solana.network),
            X402BatchSettlementTerms::Xrpl(xrpl) => (SettlementChain::Xrpl, &xrpl.network),
        };
        // ADR 0086: the counterparty's network must be one this node settles
        // on, and the channel is opened on exactly that one.
        let ours: Vec<&Network> = self
            .outbound
            .payers
            .iter()
            .map(|(network, _)| network)
            .filter(|own| own.chain() == chain.asset_chain())
            .collect();
        let on = ours
            .iter()
            .find(|own| own.to_string() == *network)
            .map(|own| (*own).clone());
        let on = match (on, ours.is_empty()) {
            (Some(on), _) => on,
            (None, true) => return Err(BatchChannelError::NoBackend(chain)),
            (None, false) => {
                return Err(BatchChannelError::InvalidTerms(format!(
                    "the counterparty settles on {network}, and this node's {} backends on {}",
                    chain.name(),
                    ours.iter()
                        .map(|own| own.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )))
            }
        };
        let terms =
            receiver_terms(terms, counterparty_url).map_err(BatchChannelError::InvalidTerms)?;
        let (state, resumed) = self.outbound.open_on(&on, terms, deposit, gate).await?;
        Ok((self.outbound.view_of(&state), resumed))
    }

    /// Every inbound channel this node holds a voucher on, read from its
    /// chain now.
    pub async fn inbound_views(&self) -> Vec<BatchChannelView> {
        let mut views = Vec::new();
        for held in self.held.held_vouchers() {
            let chain = chain_of(held.presentation.chain());
            let id = held.presentation.channel().0.clone();
            let base = BatchChannelView {
                id,
                scheme: BATCH_SETTLEMENT.to_string(),
                chain: chain.name().to_string(),
                network: self.network_of(&held.presentation),
                direction: ChannelDirection::Inbound,
                status: BatchChannelViewStatus::Unreadable,
                counterparty: String::new(),
                collateral: None,
                landed: None,
                watermark: held.voucher.cumulative_amount,
                detail: None,
            };
            let state = match self.receiver(&held.presentation) {
                Ok(backend) => read_inbound(backend.as_ref(), &held.presentation).await,
                Err(error) => Err(error),
            };
            views.push(match state {
                Ok(state) => BatchChannelView {
                    status: state.status.into(),
                    counterparty: spell_signer(&state.voucher_signer),
                    collateral: Some(state.collateral),
                    landed: Some(state.landed),
                    ..base
                },
                Err(error) => BatchChannelView {
                    detail: Some(error.to_string()),
                    ..base
                },
            });
        }
        views
    }

    /// Every batch-settlement channel, inbound then outbound.
    pub async fn views(&self) -> Vec<BatchChannelView> {
        let mut views = self.inbound_views().await;
        views.extend(self.outbound.views().await);
        views
    }

    /// Land the latest voucher held on the inbound channel `id` now (ADR
    /// 0075 decision 11): the manual lever beside the watchers and sweeps,
    /// for planned maintenance.
    pub async fn land(&self, id: &str) -> Result<LandedChannel, BatchChannelError> {
        let id = canonical_id(id);
        let held = self
            .held
            .held_vouchers()
            .into_iter()
            .find(|held| canonical_id(&held.presentation.channel().0) == id)
            .ok_or_else(|| BatchChannelError::NoVoucherHeld(id.clone()))?;
        let chain = chain_of(held.presentation.chain());
        let backend = self.receiver(&held.presentation)?;
        read_inbound(backend.as_ref(), &held.presentation).await?;
        let Landed { state, transaction } = backend
            .land(held.presentation.channel(), held.voucher.clone())
            .await?;
        let view = BatchChannelView {
            id,
            scheme: BATCH_SETTLEMENT.to_string(),
            chain: chain.name().to_string(),
            network: self.network_of(&held.presentation),
            direction: ChannelDirection::Inbound,
            status: state.status.into(),
            counterparty: spell_signer(&state.voucher_signer),
            collateral: Some(state.collateral),
            landed: Some(state.landed),
            watermark: held.voucher.cumulative_amount,
            detail: None,
        };
        Ok(LandedChannel { view, transaction })
    }
}

/// An inbound channel's state, restoring it to its backend first if this
/// process has not: a held voucher is landable whatever today's admission
/// rules say (ADR 0074 decision 5).
async fn read_inbound(
    backend: &dyn BatchSettlementBackend,
    presentation: &ChannelPresentation,
) -> Result<BatchChannelState, BatchChannelError> {
    match backend.channel_state(presentation.channel()).await {
        Err(BatchSettlementError::ChannelNotAdmitted(_)) => {
            Ok(backend.restore(presentation.clone()).await?)
        }
        other => Ok(other?),
    }
}

impl OutboundChannels {
    /// The outbound channel `state` reads, as `GET /channels` reports it:
    /// what each write on it answers with.
    pub fn view_of(&self, state: &OutboundChannelState) -> BatchChannelView {
        let (chain, counterparty) = match self.tracked(&state.on_chain.id.0) {
            Ok(tracked) => (
                tracked.chain,
                spell(tracked.chain, &tracked.record.receiver()),
            ),
            Err(_) => match state.on_chain.voucher_signer {
                VoucherSigner::Evm(_) => (SettlementChain::Evm, String::new()),
                VoucherSigner::Solana(_) => (SettlementChain::Solana, String::new()),
                VoucherSigner::Xrpl(_) => (SettlementChain::Xrpl, String::new()),
            },
        };
        let network = self
            .network(&state.on_chain.id.0)
            .map(|network| network.to_string());
        BatchChannelView {
            id: state.on_chain.id.0.clone(),
            scheme: BATCH_SETTLEMENT.to_string(),
            chain: chain.name().to_string(),
            network,
            direction: ChannelDirection::Outbound,
            status: state.on_chain.status.into(),
            counterparty,
            collateral: Some(state.on_chain.collateral),
            landed: Some(state.on_chain.landed),
            watermark: state.signed,
            detail: None,
        }
    }
}

/// The counterparty's published `batchSettlements` entry (ADR 0075
/// decision 10) as the settlement port's [`ReceiverTerms`]. A Solana
/// `sponsorEndpoint` published as a path is resolved against
/// `counterparty_url`, the URL the operator named the counterparty by.
pub fn receiver_terms(
    terms: &X402BatchSettlementTerms,
    counterparty_url: Option<&Url>,
) -> Result<ReceiverTerms, String> {
    match terms {
        X402BatchSettlementTerms::Evm(evm) => {
            let receiver = evm_address(&evm.pay_to, "payTo")?;
            // This node names the receiver in both receiving seats (ADR
            // 0075 decision 3), which is what the counterparty admits.
            if evm_address(&evm.receiver_authorizer, "receiverAuthorizer")? != receiver {
                return Err(
                    "its receiverAuthorizer is not its payTo, and a channel this node opens \
                     names the receiver in both seats (ADR 0075 decision 3)"
                        .to_string(),
                );
            }
            Ok(ReceiverTerms::Evm(EvmReceiverTerms {
                receiver,
                token: evm_address(&evm.asset, "asset")?,
                min_withdraw_delay_secs: evm.min_withdraw_delay_secs,
            }))
        }
        X402BatchSettlementTerms::Solana(solana) => {
            let sponsor_endpoint = match Url::parse(&solana.sponsor_endpoint) {
                Ok(absolute) => absolute,
                Err(_) => counterparty_url
                    .ok_or_else(|| {
                        format!(
                            "its sponsorEndpoint '{}' is a path, and resolving it needs the \
                             counterparty's URL in `url`",
                            solana.sponsor_endpoint
                        )
                    })?
                    .join(&solana.sponsor_endpoint)
                    .map_err(|error| format!("its sponsorEndpoint does not resolve: {error}"))?,
            };
            Ok(ReceiverTerms::Solana(SolanaReceiverTerms {
                sponsor: solana_key(&solana.fee_payer, "feePayer")?,
                receiver: solana_key(&solana.pay_to, "payTo")?,
                mint: solana_key(&solana.asset, "asset")?,
                min_grace_period_secs: solana.min_grace_period_secs,
                min_deposit: solana.min_deposit.parse().map_err(|_| {
                    format!("its minDeposit '{}' is not an amount", solana.min_deposit)
                })?,
                sponsor_endpoint: sponsor_endpoint.to_string(),
            }))
        }
        X402BatchSettlementTerms::Xrpl(xrpl) => {
            // A binding this build was not written to is refused, not
            // guessed at: a later `xrpl-paychan` version may sign other bytes
            // (ADR 0087 decision 1).
            if xrpl.binding != connector_domain::x402::XRPL_PAYCHAN_BINDING {
                return Err(format!(
                    "its binding is '{}', and this node speaks only '{}'",
                    xrpl.binding,
                    connector_domain::x402::XRPL_PAYCHAN_BINDING
                ));
            }
            connector_signer::decode_xrpl_classic_address(&xrpl.pay_to).map_err(|error| {
                format!(
                    "its payTo '{}' is not a classic address: {error}",
                    xrpl.pay_to
                )
            })?;
            Ok(ReceiverTerms::Xrpl(XrplReceiverTerms {
                receiver: xrpl.pay_to.clone(),
                min_settle_delay_secs: xrpl.min_settle_delay_secs,
            }))
        }
    }
}

fn evm_address(text: &str, field: &str) -> Result<[u8; 20], String> {
    let digits = text.strip_prefix("0x").unwrap_or(text);
    let bytes = (0..digits.len())
        .step_by(2)
        .map(|i| {
            digits
                .get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        })
        .collect::<Option<Vec<u8>>>();
    bytes
        .and_then(|bytes| <[u8; 20]>::try_from(bytes).ok())
        .ok_or_else(|| format!("its {field} '{text}' is not an EVM address"))
}

fn solana_key(text: &str, field: &str) -> Result<[u8; 32], String> {
    bs58::decode(text)
        .into_vec()
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| format!("its {field} '{text}' is not a Solana key"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{FileJournal, InMemoryJournal};
    use connector_domain::x402::{X402BatchSettlementEvmTerms, X402BatchSettlementSolanaTerms};
    use connector_settlement::batch::{
        HeldVoucher, InMemoryBatchChain, InMemoryBatchSettlement, PayerExit,
    };
    use std::sync::RwLock;

    const ONE_DAY: u64 = 86_400;

    /// A journal that durably records nothing: every append fails, as a
    /// full disk would.
    struct FullDisk;

    impl Journal for FullDisk {
        fn append(&self, _entry: &JournalEntry) -> Result<(), JournalError> {
            Err(JournalError::Io(std::io::Error::other(
                "no space left on device",
            )))
        }

        fn read_all(&self) -> Result<Vec<JournalEntry>, JournalError> {
            Ok(Vec::new())
        }
    }

    fn chain_name(exit: PayerExit) -> SettlementChain {
        match exit {
            PayerExit::Withdrawal => SettlementChain::Evm,
            PayerExit::Close => SettlementChain::Solana,
        }
    }

    /// The one network of the in-memory chain an exit belongs to.
    fn exit_network(exit: PayerExit) -> Network {
        match exit {
            PayerExit::Withdrawal => "eip155:8453",
            PayerExit::Close => "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1",
        }
        .parse()
        .expect("a CAIP-2 network")
    }

    /// One chain, the paying node `0x01` funded with 10,000, and the
    /// counterparty `0x02` it pays.
    struct World {
        exit: PayerExit,
        chain: Arc<InMemoryBatchChain>,
        receiver: Arc<InMemoryBatchSettlement>,
    }

    impl World {
        fn new(exit: PayerExit) -> World {
            let chain = InMemoryBatchChain::new(exit);
            let payer = InMemoryBatchSettlement::on(Arc::clone(&chain), 0x01, ONE_DAY);
            payer.fund(10_000);
            let receiver = Arc::new(
                InMemoryBatchSettlement::on(Arc::clone(&chain), 0x02, ONE_DAY)
                    .with_min_sponsored_deposit(500),
            );
            World {
                exit,
                chain,
                receiver,
            }
        }

        /// The paying node's port as a freshly booted process has it:
        /// nothing remembered but the chain.
        fn payer(&self) -> Arc<InMemoryBatchSettlement> {
            Arc::new(InMemoryBatchSettlement::on(
                Arc::clone(&self.chain),
                0x01,
                ONE_DAY,
            ))
        }

        /// A node booting over `journal`.
        async fn boot(&self, journal: Arc<dyn Journal>) -> OutboundChannels {
            let payer = self.payer() as Arc<dyn BatchSettlementPayer>;
            OutboundChannels::restore(journal, vec![(exit_network(self.exit), payer)])
                .await
                .expect("the journal replays")
        }

        fn balance(&self) -> u128 {
            self.payer().balance()
        }
    }

    fn only_view(views: Vec<BatchChannelView>) -> BatchChannelView {
        let [view] = <[BatchChannelView; 1]>::try_from(views).expect("exactly one channel");
        view
    }

    /// ADR 0075 decision 8: the record is journaled before anything is
    /// sent, so a journal that cannot record it sends nothing at all.
    #[tokio::test]
    async fn an_open_the_journal_cannot_record_is_never_sent() {
        for exit in [PayerExit::Withdrawal, PayerExit::Close] {
            let world = World::new(exit);
            let outbound = world.boot(Arc::new(FullDisk)).await;
            let error = outbound
                .open(world.receiver.published_terms(), 1_000)
                .await
                .unwrap_err();
            assert!(matches!(error, BatchChannelError::Journal(_)), "{error:?}");
            assert_eq!(world.balance(), 10_000, "nothing was deposited");
            assert!(outbound.views().await.is_empty());
        }
    }

    /// The journal's order: the record, then that it opened.
    #[tokio::test]
    async fn an_open_is_journaled_as_opening_then_opened() {
        let world = World::new(PayerExit::Withdrawal);
        let journal = Arc::new(InMemoryJournal::new());
        let outbound = world.boot(Arc::clone(&journal) as Arc<dyn Journal>).await;
        let (state, resumed) = outbound
            .open(world.receiver.published_terms(), 1_000)
            .await
            .expect("open");
        assert!(!resumed);
        let key = journal_key(SettlementChain::Evm, &state.on_chain.id);
        let entries = journal.read_all().unwrap();
        let [JournalEntry::OutboundChannelOpening { channel_id, record }, JournalEntry::OutboundChannelOpened { channel_id: opened }] =
            entries.as_slice()
        else {
            panic!("expected opening then opened, got {entries:?}");
        };
        assert_eq!(channel_id, &key);
        assert_eq!(opened, &key);
        assert_eq!(
            OutboundChannelRecord::decode(record)
                .expect("a record")
                .record
                .channel(),
            &state.on_chain.id
        );
    }

    /// The acceptance criterion: after a restart the node still knows its
    /// outbound channels and can sign on them, and its signed watermark does
    /// not go backwards. A file journal, reopened, is the restart.
    #[tokio::test]
    async fn after_a_restart_the_node_knows_its_channels_and_signs_only_above_its_watermark() {
        for exit in [PayerExit::Withdrawal, PayerExit::Close] {
            let world = World::new(exit);
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("outbound-channels.log");
            let id = {
                let outbound = world
                    .boot(Arc::new(FileJournal::open(&path).unwrap()))
                    .await;
                let (state, _) = outbound
                    .open(world.receiver.published_terms(), 1_000)
                    .await
                    .expect("open");
                let id = state.on_chain.id.0.clone();
                outbound.sign_voucher(&id, 300).await.expect("sign");
                id
            };

            let outbound = world
                .boot(Arc::new(FileJournal::open(&path).unwrap()))
                .await;
            let view = only_view(outbound.views().await);
            assert_eq!(view.id, id);
            assert_eq!(view.direction, ChannelDirection::Outbound);
            assert_eq!(view.status, BatchChannelViewStatus::Open);
            assert_eq!(view.collateral, Some(1_000));
            assert_eq!(view.watermark, 300);
            assert_eq!(
                outbound.sign_voucher(&id, 300).await.unwrap_err(),
                BatchChannelError::Settlement(BatchSettlementError::VoucherNotAdvancing {
                    amount: 300,
                    signed: 300,
                }),
                "a restart never lets the watermark go backwards"
            );
            let voucher = outbound.sign_voucher(&id, 301).await.expect("sign above");
            assert_eq!(voucher.cumulative_amount, 301);
            let topped = outbound.top_up(&id, 500).await.expect("top up");
            assert_eq!(topped.on_chain.collateral, 1_500);
            assert_eq!(outbound.claims().len(), 1);
            assert_eq!(outbound.claims()[0].cumulative_amount, 301);
        }
    }

    /// Issue #1446: a watermark lowered to the receiver's figure is durable.
    /// The voucher entries fold by max, so without an entry that overwrites
    /// on replay a restart would sign from the old, higher figure again --
    /// and the chain's landed figure still bounds it from below.
    #[tokio::test]
    async fn a_lowered_watermark_survives_a_journal_replay() {
        for exit in [PayerExit::Withdrawal, PayerExit::Close] {
            let world = World::new(exit);
            let journal = Arc::new(InMemoryJournal::new());
            let outbound = world.boot(Arc::clone(&journal) as Arc<dyn Journal>).await;
            let (state, _) = outbound
                .open(world.receiver.published_terms(), 1_000)
                .await
                .expect("open");
            let id = state.on_chain.id.0.clone();
            outbound.sign_voucher(&id, 100).await.expect("sign");
            outbound.sign_voucher(&id, 300).await.expect("sign");

            assert_eq!(outbound.set_watermark(&id, 100).await.unwrap(), 100);
            assert_eq!(outbound.signed(&id), Some(100));

            let restarted = world.boot(Arc::clone(&journal) as Arc<dyn Journal>).await;
            assert_eq!(restarted.signed(&id), Some(100), "replay sets, not max");
            assert_eq!(
                restarted
                    .sign_voucher(&id, 150)
                    .await
                    .unwrap()
                    .cumulative_amount,
                150,
                "the next voucher is signed above the lowered figure"
            );
            // A raise is still a raise.
            assert_eq!(restarted.set_watermark(&id, 500).await.unwrap(), 500);
        }
    }

    /// Crash between journaling and sending: the node restarts holding a
    /// record nothing on chain matches. The channel is listed as opening,
    /// and a retried open toward the same receiver sends **that** record --
    /// one channel, one deposit -- rather than building a second.
    #[tokio::test]
    async fn a_retried_open_resumes_the_journaled_channel_rather_than_opening_another() {
        for exit in [PayerExit::Withdrawal, PayerExit::Close] {
            let world = World::new(exit);
            let journal = Arc::new(InMemoryJournal::new());
            let crashed = world.payer();
            let record = crashed
                .prepare_open(world.receiver.published_terms(), 1_000)
                .await
                .expect("prepare");
            journal
                .append(&JournalEntry::OutboundChannelOpening {
                    channel_id: journal_key(chain_name(exit), record.channel()),
                    record: record.encode_on(&exit_network(world.exit).to_string()),
                })
                .unwrap();

            let outbound = world.boot(Arc::clone(&journal) as Arc<dyn Journal>).await;
            let view = only_view(outbound.views().await);
            assert_eq!(view.status, BatchChannelViewStatus::Opening);
            assert_eq!(
                outbound.top_up(&view.id, 1).await.unwrap_err(),
                BatchChannelError::StillOpening(view.id.clone())
            );

            let (state, resumed) = outbound
                .open(world.receiver.published_terms(), 1_000)
                .await
                .expect("the retried open");
            assert!(resumed, "the retry resumed the journaled open");
            assert_eq!(&state.on_chain.id, record.channel());
            assert_eq!(world.balance(), 9_000, "one channel, one deposit");

            // With nothing pending, the next open is a channel of its own.
            let (second, resumed) = outbound
                .open(world.receiver.published_terms(), 1_000)
                .await
                .expect("a second open");
            assert!(!resumed);
            assert_ne!(second.on_chain.id, state.on_chain.id);
            assert_eq!(outbound.views().await.len(), 2);
        }
    }

    /// A gate that adds up what it is asked to count.
    #[derive(Default)]
    struct Counted(std::sync::Mutex<u128>);

    impl DepositGate for Counted {
        fn count(&self, amount: u128) -> Result<(), String> {
            *self.0.lock().unwrap() += amount;
            Ok(())
        }
    }

    /// ADR 0078 decision 4: a retried open that resumes its journaled
    /// channel counts nothing against a deposit limit; the next open, a
    /// channel of its own, counts its deposit.
    #[tokio::test]
    async fn a_resumed_open_counts_nothing_and_a_new_one_counts_its_deposit() {
        let world = World::new(PayerExit::Withdrawal);
        let journal = Arc::new(InMemoryJournal::new());
        let record = world
            .payer()
            .prepare_open(world.receiver.published_terms(), 1_000)
            .await
            .expect("prepare");
        journal
            .append(&JournalEntry::OutboundChannelOpening {
                channel_id: journal_key(chain_name(PayerExit::Withdrawal), record.channel()),
                record: record.encode_on(&exit_network(world.exit).to_string()),
            })
            .unwrap();
        let outbound = world.boot(Arc::clone(&journal) as Arc<dyn Journal>).await;
        let gate = Counted::default();

        let (_, resumed) = outbound
            .open_with(world.receiver.published_terms(), 1_000, Some(&gate))
            .await
            .expect("the retried open");
        assert!(resumed);
        assert_eq!(*gate.0.lock().unwrap(), 0, "a resumed open counts nothing");

        let (_, resumed) = outbound
            .open_with(world.receiver.published_terms(), 1_000, Some(&gate))
            .await
            .expect("a second open");
        assert!(!resumed);
        assert_eq!(*gate.0.lock().unwrap(), 1_000);
    }

    /// A pending open resumes only for the same open: a request with another
    /// deposit is another channel, so a stale record never traps every
    /// later open toward that receiver.
    #[tokio::test]
    async fn only_the_same_open_resumes_a_pending_one() {
        let world = World::new(PayerExit::Withdrawal);
        let journal = Arc::new(InMemoryJournal::new());
        let record = world
            .payer()
            .prepare_open(world.receiver.published_terms(), 1_000)
            .await
            .expect("prepare");
        journal
            .append(&JournalEntry::OutboundChannelOpening {
                channel_id: journal_key(SettlementChain::Evm, record.channel()),
                record: record.encode_on(&exit_network(world.exit).to_string()),
            })
            .unwrap();
        let outbound = world.boot(Arc::clone(&journal) as Arc<dyn Journal>).await;
        let (state, resumed) = outbound
            .open(world.receiver.published_terms(), 2_000)
            .await
            .expect("another open");
        assert!(!resumed);
        assert_ne!(&state.on_chain.id, record.channel());
        assert_eq!(world.balance(), 8_000);
    }

    /// Crash after the open landed and before `opened` was journaled: the
    /// boot finds it on chain, adopts it and journals that it opened.
    #[tokio::test]
    async fn a_boot_adopts_an_open_that_landed_before_the_crash() {
        let world = World::new(PayerExit::Close);
        let journal = Arc::new(InMemoryJournal::new());
        let crashed = world.payer();
        let record = crashed
            .prepare_open(world.receiver.published_terms(), 1_000)
            .await
            .expect("prepare");
        let key = journal_key(SettlementChain::Solana, record.channel());
        journal
            .append(&JournalEntry::OutboundChannelOpening {
                channel_id: key.clone(),
                record: record.encode_on(&exit_network(world.exit).to_string()),
            })
            .unwrap();
        crashed.open_prepared(&record).await.expect("it landed");

        let outbound = world.boot(Arc::clone(&journal) as Arc<dyn Journal>).await;
        assert_eq!(
            only_view(outbound.views().await).status,
            BatchChannelViewStatus::Open
        );
        assert_eq!(
            journal.read_all().unwrap().last(),
            Some(&JournalEntry::OutboundChannelOpened { channel_id: key })
        );
        assert_eq!(world.balance(), 9_000);
    }

    /// Solana: a journaled open whose blockhash expired with nothing on
    /// chain never will land. A retry journals it abandoned and opens this
    /// request's own channel.
    #[tokio::test]
    async fn a_lapsed_open_is_abandoned_and_a_fresh_one_opened() {
        let world = World::new(PayerExit::Close);
        let journal = Arc::new(InMemoryJournal::new());
        let record = world
            .payer()
            .prepare_open(world.receiver.published_terms(), 1_000)
            .await
            .expect("prepare");
        let key = journal_key(SettlementChain::Solana, record.channel());
        journal
            .append(&JournalEntry::OutboundChannelOpening {
                channel_id: key.clone(),
                record: record.encode_on(&exit_network(world.exit).to_string()),
            })
            .unwrap();
        world.chain.advance_time(ONE_DAY);

        let outbound = world.boot(Arc::clone(&journal) as Arc<dyn Journal>).await;
        let (state, resumed) = outbound
            .open(world.receiver.published_terms(), 1_000)
            .await
            .expect("a fresh open");
        assert!(!resumed);
        assert_ne!(&state.on_chain.id, record.channel());
        assert_eq!(world.balance(), 9_000);
        assert!(journal
            .read_all()
            .unwrap()
            .contains(&JournalEntry::OutboundChannelAbandoned { channel_id: key }));
        assert_eq!(outbound.views().await.len(), 1, "the lapsed one is gone");
    }

    /// `POST /channels/:id/withdraw`'s one lever: it starts, refuses to
    /// finish early by name, and finishes once the delay has run.
    #[tokio::test]
    async fn withdrawing_starts_then_finishes_once_due() {
        for exit in [PayerExit::Withdrawal, PayerExit::Close] {
            let world = World::new(exit);
            let outbound = world.boot(Arc::new(InMemoryJournal::new())).await;
            let (state, _) = outbound
                .open(world.receiver.published_terms(), 1_000)
                .await
                .expect("open");
            let id = state.on_chain.id.0;
            let (step, state) = outbound.withdraw(&id).await.expect("start");
            assert_eq!(step, WithdrawStep::Started);
            assert_eq!(state.on_chain.collateral, 0);
            assert!(matches!(
                outbound.withdraw(&id).await.unwrap_err(),
                BatchChannelError::Settlement(BatchSettlementError::WithdrawalNotDue { .. })
            ));
            world.chain.advance_time(ONE_DAY);
            let (step, _) = outbound.withdraw(&id).await.expect("finish");
            assert_eq!(step, WithdrawStep::Finished);
            assert_eq!(world.balance(), 10_000, "all of it came back");
        }
    }

    #[tokio::test]
    async fn a_channel_this_node_did_not_open_is_not_its_to_drive() {
        let world = World::new(PayerExit::Withdrawal);
        let outbound = world.boot(Arc::new(InMemoryJournal::new())).await;
        let id = format!("0x{}", "ab".repeat(32));
        assert_eq!(
            outbound.top_up(&id, 1).await.unwrap_err(),
            BatchChannelError::UnknownChannel(id.clone())
        );
        assert_eq!(
            outbound.withdraw(&id).await.unwrap_err(),
            BatchChannelError::UnknownChannel(id)
        );
    }

    /// `POST /channels/:id/land`: the latest voucher held on an inbound
    /// channel lands now, and landing it again is refused by name.
    #[tokio::test]
    async fn the_latest_held_voucher_lands_on_an_inbound_channel() {
        for exit in [PayerExit::Withdrawal, PayerExit::Close] {
            let world = World::new(exit);
            let payer = world.payer();
            let opened = payer
                .open(world.receiver.published_terms(), 1_000)
                .await
                .expect("the counterparty opens toward this node");
            let channel = opened.presentation.channel().clone();
            let voucher = payer.sign_voucher(&channel, 300).await.expect("sign");
            let held = Arc::new(RwLock::new(vec![HeldVoucher {
                presentation: opened.presentation.clone(),
                voucher,
            }]));
            let channels = BatchChannels::new(
                Arc::new(world.boot(Arc::new(InMemoryJournal::new())).await),
                vec![(
                    exit_network(exit),
                    Arc::clone(&world.receiver) as Arc<dyn BatchSettlementBackend>,
                )],
                held,
            );

            let view = only_view(channels.inbound_views().await);
            assert_eq!(view.direction, ChannelDirection::Inbound);
            assert_eq!(view.watermark, 300);
            assert_eq!(view.landed, Some(0));

            let landed = channels.land(&channel.0).await.expect("land");
            assert_eq!(landed.view.landed, Some(300));
            assert!(!landed.transaction.is_empty());
            assert_eq!(
                channels.land(&channel.0).await.unwrap_err(),
                BatchChannelError::Settlement(BatchSettlementError::StaleVoucher {
                    amount: 300,
                    landed: 300,
                })
            );
            assert_eq!(
                channels.land("not-a-channel").await.unwrap_err(),
                BatchChannelError::NoVoucherHeld("not-a-channel".to_string())
            );
        }
    }

    fn evm_terms() -> X402BatchSettlementEvmTerms {
        X402BatchSettlementEvmTerms {
            network: "eip155:84532".to_string(),
            asset: format!("0x{}", "70".repeat(20)),
            pay_to: format!("0x{}", "02".repeat(20)),
            receiver_authorizer: format!("0x{}", "02".repeat(20)),
            min_withdraw_delay_secs: ONE_DAY,
            name: "USDC".to_string(),
            version: "2".to_string(),
            asset_transfer_method: Default::default(),
            facilitator: None,
        }
    }

    fn solana_terms(sponsor_endpoint: &str) -> X402BatchSettlementSolanaTerms {
        X402BatchSettlementSolanaTerms {
            network: "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1".to_string(),
            asset: bs58::encode([0x70; 32]).into_string(),
            pay_to: bs58::encode([0x02; 32]).into_string(),
            fee_payer: bs58::encode([0x03; 32]).into_string(),
            min_grace_period_secs: ONE_DAY,
            min_deposit: "500".to_string(),
            token_program: "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".to_string(),
            sponsor_endpoint: sponsor_endpoint.to_string(),
        }
    }

    #[test]
    fn published_terms_read_as_the_ports_receiver_terms() {
        assert_eq!(
            receiver_terms(&X402BatchSettlementTerms::Evm(evm_terms()), None),
            Ok(ReceiverTerms::Evm(EvmReceiverTerms {
                receiver: [0x02; 20],
                token: [0x70; 20],
                min_withdraw_delay_secs: ONE_DAY,
            }))
        );
        let url = Url::parse("https://peer.example/ilp").unwrap();
        let ReceiverTerms::Solana(solana) = receiver_terms(
            &X402BatchSettlementTerms::Solana(solana_terms("/ilp/batch-settlement/solana/open")),
            Some(&url),
        )
        .expect("terms") else {
            panic!("Solana terms");
        };
        assert_eq!(
            solana.sponsor_endpoint, "https://peer.example/ilp/batch-settlement/solana/open",
            "a published path resolves against the counterparty's URL"
        );
        assert_eq!(solana.sponsor, [0x03; 32]);
        assert_eq!(solana.receiver, [0x02; 32]);
        assert_eq!(solana.min_deposit, 500);
    }

    #[test]
    fn terms_this_node_cannot_open_on_are_refused_by_what_is_wrong() {
        let delegated = X402BatchSettlementEvmTerms {
            receiver_authorizer: format!("0x{}", "09".repeat(20)),
            ..evm_terms()
        };
        assert!(
            receiver_terms(&X402BatchSettlementTerms::Evm(delegated), None)
                .unwrap_err()
                .contains("receiverAuthorizer")
        );
        assert!(receiver_terms(
            &X402BatchSettlementTerms::Solana(solana_terms("/ilp/batch-settlement/solana/open")),
            None,
        )
        .unwrap_err()
        .contains("needs the counterparty's URL"));
    }

    #[tokio::test]
    async fn terms_on_another_network_are_refused_before_anything_is_built() {
        let world = World::new(PayerExit::Withdrawal);
        let journal = Arc::new(InMemoryJournal::new());
        let channels = BatchChannels::new(
            Arc::new(world.boot(Arc::clone(&journal) as Arc<dyn Journal>).await),
            Vec::new(),
            Arc::new(RwLock::new(Vec::new())),
        );
        let error = channels
            .open(&X402BatchSettlementTerms::Evm(evm_terms()), None, 1_000)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, BatchChannelError::InvalidTerms(reason) if reason.contains("eip155:84532")),
            "{error:?}"
        );
        assert!(journal.read_all().unwrap().is_empty());
    }

    /// ADR 0086 decision 10: version-1 outbound records, as a build from
    /// before it journaled them, upgraded at boot by proof.
    mod record_upgrade {
        use super::*;

        const BASE: &str = "eip155:31337";
        const OTHER: &str = "eip155:31338";
        const SOLANA: &str = "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1";

        fn net(text: &str) -> Network {
            text.parse().expect("a CAIP-2 network")
        }

        /// An EVM record whose channel id is the one x402's contract on
        /// `chain_id` derives from its config: proof of its network.
        fn evm_record_on(chain_id: u64, salt: u8) -> OutboundChannelRecord {
            let config = connector_settlement::batch::EvmChannelConfig {
                payer: [1; 20],
                payer_authorizer: [1; 20],
                receiver: [2; 20],
                receiver_authorizer: [2; 20],
                token: [3; 20],
                withdraw_delay: ONE_DAY,
                salt: [salt; 32],
            };
            let id = connector_signer::evm_batch_channel_id(
                &connector_signer::BatchSettlementDomain::x402(chain_id),
                &connector_signer::BatchChannelConfig {
                    payer: config.payer,
                    payer_authorizer: config.payer_authorizer,
                    receiver: config.receiver,
                    receiver_authorizer: config.receiver_authorizer,
                    token: config.token,
                    withdraw_delay: config.withdraw_delay,
                    salt: config.salt,
                },
            );
            OutboundChannelRecord::Evm {
                channel: ChannelId(hex(&id)),
                config,
                deposit: 1_000,
            }
        }

        fn solana_record() -> OutboundChannelRecord {
            OutboundChannelRecord::Solana {
                channel: ChannelId("9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin".to_string()),
                receiver: [5; 32],
                sponsor_endpoint: "https://peer.example/open".to_string(),
                deposit: 1_000,
                transaction: vec![6; 64],
                last_valid_block_height: 1,
            }
        }

        fn key(record: &OutboundChannelRecord) -> String {
            journal_key(chain_of(record.chain()), record.channel())
        }

        /// A journal as a pre-0086 build left it: each record opened, and
        /// a voucher signed on it.
        fn version_1_journal(records: &[&OutboundChannelRecord]) -> Arc<InMemoryJournal> {
            let journal = Arc::new(InMemoryJournal::new());
            for record in records {
                for entry in [
                    JournalEntry::OutboundChannelOpening {
                        channel_id: key(record),
                        record: record.encode_version_1(),
                    },
                    JournalEntry::OutboundChannelOpened {
                        channel_id: key(record),
                    },
                    JournalEntry::OutboundVoucherSigned {
                        channel_id: key(record),
                        cumulative_amount: 40,
                    },
                ] {
                    journal.append(&entry).unwrap();
                }
            }
            journal
        }

        /// One payer per network named, each its chain's in-memory fake.
        fn payers(networks: &[&str]) -> Vec<(Network, Arc<dyn BatchSettlementPayer>)> {
            networks
                .iter()
                .map(|network| {
                    let network = net(network);
                    let exit = match network.chain() {
                        AssetChain::Evm => PayerExit::Withdrawal,
                        AssetChain::Solana => PayerExit::Close,
                        AssetChain::Mina => unreachable!("no payer settles on Mina"),
                        AssetChain::Xrpl => unreachable!("no payer settles on XRPL yet"),
                    };
                    let payer =
                        InMemoryBatchSettlement::on(InMemoryBatchChain::new(exit), 0x01, ONE_DAY);
                    (network, Arc::new(payer) as Arc<dyn BatchSettlementPayer>)
                })
                .collect()
        }

        async fn boot(
            journal: &Arc<InMemoryJournal>,
            networks: &[&str],
        ) -> Result<OutboundChannels, OutboundRestoreError> {
            OutboundChannels::restore(Arc::clone(journal) as Arc<dyn Journal>, payers(networks))
                .await
        }

        #[tokio::test]
        async fn a_version_1_record_is_rewritten_as_version_2_on_the_network_that_reproduces_it() {
            let record = evm_record_on(31_338, 9);
            let journal = version_1_journal(&[&record]);
            let before = journal.read_all().unwrap();

            let outbound = boot(&journal, &[BASE, OTHER]).await.expect("it boots");
            let id = &record.channel().0;
            assert_eq!(outbound.network(id), Some(net(OTHER)));
            assert_eq!(outbound.signed(id), Some(40), "its watermark kept");

            let after = journal.read_all().unwrap();
            assert_eq!(after[..before.len()], before[..], "appended, never edited");
            assert_eq!(
                after[before.len()..],
                [JournalEntry::OutboundChannelOpening {
                    channel_id: format!("evm:{id}"),
                    record: record.encode_on(OTHER),
                }],
                "the record, under its unchanged key, as version 2 on 31338"
            );
        }

        /// A table's removal never strands value silently: one EVM table
        /// left is not proof, so the record is refused, naming the channel.
        #[tokio::test]
        async fn a_version_1_record_no_network_reproduces_is_refused_by_name() {
            let record = evm_record_on(31_338, 9);
            let journal = version_1_journal(&[&record]);
            let before = journal.read_all().unwrap();

            let Err(error) = boot(&journal, &[BASE]).await else {
                panic!("booted holding a channel on no network it settles on");
            };
            let key = key(&record);
            assert!(
                matches!(&error, OutboundRestoreError::OnNoNetwork { channel_id, network: None }
                    if *channel_id == key),
                "{error:?}"
            );
            assert!(error.to_string().contains(&key), "{error}");
            assert_eq!(journal.read_all().unwrap(), before, "nothing rewritten");
        }

        #[tokio::test]
        async fn a_version_1_record_several_networks_reproduce_is_refused_by_name() {
            let record = evm_record_on(31_338, 9);
            let journal = version_1_journal(&[&record]);

            let Err(error) = boot(&journal, &[BASE, OTHER, OTHER]).await else {
                panic!("assigned a channel to one of several networks by guess");
            };
            let key = key(&record);
            assert!(
                matches!(&error, OutboundRestoreError::OnSeveralNetworks { channel_id, networks }
                    if *channel_id == key && *networks == vec![net(OTHER), net(OTHER)]),
                "{error:?}"
            );
            assert!(error.to_string().contains(&key), "{error}");
        }

        /// A crash after the first rewrite and before the second: the
        /// restart finishes the upgrade, and the journal ends exactly as an
        /// uninterrupted one did. A further restart writes nothing.
        #[tokio::test]
        async fn the_upgrade_is_idempotent_across_a_crash_mid_upgrade() {
            let on_base = evm_record_on(31_337, 1);
            let on_other = evm_record_on(31_338, 2);
            let records = [&on_base, &on_other];

            let uninterrupted = version_1_journal(&records);
            boot(&uninterrupted, &[BASE, OTHER]).await.expect("boots");
            let finished = uninterrupted.read_all().unwrap();
            let upgrades = &finished[records.len() * 3..];
            assert_eq!(upgrades.len(), 2, "{finished:?}");

            let crashed = version_1_journal(&records);
            crashed.append(&upgrades[0]).unwrap();
            boot(&crashed, &[BASE, OTHER]).await.expect("boots");
            assert_eq!(crashed.read_all().unwrap(), finished);

            let outbound = boot(&crashed, &[BASE, OTHER]).await.expect("boots");
            assert_eq!(crashed.read_all().unwrap(), finished, "nothing more");
            assert_eq!(outbound.network(&on_base.channel().0), Some(net(BASE)));
            assert_eq!(outbound.network(&on_other.channel().0), Some(net(OTHER)));
        }

        #[tokio::test]
        async fn a_solana_version_1_record_is_assigned_the_one_solana_network() {
            let record = solana_record();
            let journal = version_1_journal(&[&record]);

            let outbound = boot(&journal, &[BASE, SOLANA]).await.expect("boots");
            assert_eq!(outbound.network(&record.channel().0), Some(net(SOLANA)));
            assert_eq!(
                journal.read_all().unwrap().last(),
                Some(&JournalEntry::OutboundChannelOpening {
                    channel_id: key(&record),
                    record: record.encode_on(SOLANA),
                })
            );
        }

        /// A record the journal abandoned holds nothing: it is neither
        /// refused nor rewritten.
        #[tokio::test]
        async fn an_abandoned_version_1_record_is_neither_refused_nor_rewritten() {
            let record = evm_record_on(31_338, 9);
            let journal = Arc::new(InMemoryJournal::new());
            for entry in [
                JournalEntry::OutboundChannelOpening {
                    channel_id: key(&record),
                    record: record.encode_version_1(),
                },
                JournalEntry::OutboundChannelAbandoned {
                    channel_id: key(&record),
                },
            ] {
                journal.append(&entry).unwrap();
            }
            let before = journal.read_all().unwrap();
            boot(&journal, &[BASE]).await.expect("boots");
            assert_eq!(journal.read_all().unwrap(), before);
        }

        /// A version-2 record on a network whose table is gone holds value
        /// there too: refused, naming the channel and its network.
        #[tokio::test]
        async fn a_version_2_record_on_a_network_with_no_table_is_refused_by_name() {
            let record = evm_record_on(31_338, 9);
            let journal = Arc::new(InMemoryJournal::new());
            journal
                .append(&JournalEntry::OutboundChannelOpening {
                    channel_id: key(&record),
                    record: record.encode_on(OTHER),
                })
                .unwrap();
            let Err(error) = boot(&journal, &[BASE]).await else {
                panic!("booted holding a channel on a network it no longer settles on");
            };
            assert!(
                matches!(&error, OutboundRestoreError::OnNoNetwork { network: Some(network), .. }
                    if *network == net(OTHER)),
                "{error:?}"
            );
            let message = error.to_string();
            assert!(
                message.contains(&key(&record)) && message.contains(OTHER),
                "{message}"
            );
        }
    }
}
