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
//!    [`OutboundChannelRecord`], **before** the opening transaction is sent;
//! 2. `OutboundChannelOpened` once the chain holds the channel, or
//!    `OutboundChannelAbandoned` once it provably never will (a Solana
//!    `open` whose blockhash expired);
//! 3. `OutboundVoucherSigned` for every voucher signed, **before** the
//!    voucher is handed to anyone.
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
//! A restart restores every channel with the highest voucher journaled on
//! it, never below what the chain shows landed, so the signed watermark
//! never goes backwards. A node that lost this journal entirely restores
//! its watermark from the receiver's `POST /ilp/claim-state` instead, which
//! is the peering's to wire (issue #1378).

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard};

use connector_domain::x402::X402BatchSettlementTerms;
use connector_domain::JournalEntry;
use connector_settlement::batch::{
    BatchChannelState, BatchChannelStatus, BatchSettlementBackend, BatchSettlementError,
    BatchSettlementPayer, ChannelPresentation, EvmReceiverTerms, HeldVouchers,
    OutboundChannelRecord, OutboundChannelState, ReceiverTerms, SolanaReceiverTerms, Voucher,
    VoucherSigner,
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

/// What a failed batch-settlement channel operation reports.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BatchChannelError {
    /// This node has no x402 batch-settlement backend on that chain.
    #[error("this node has no x402 batch-settlement backend on {0}")]
    NoBackend(SettlementChain),
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
/// `solana:…`, as the client edge keys its own.
fn journal_key(chain: SettlementChain, channel: &ChannelId) -> String {
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
    }
}

fn spell_signer(signer: &VoucherSigner) -> String {
    match signer {
        VoucherSigner::Evm(address) => hex(address),
        VoucherSigner::Solana(key) => bs58::encode(key).into_string(),
    }
}

/// The channels this node pays on (ADR 0075 decisions 2, 8 and 11): the
/// settlement port's paying half on each chain, and the journal that lets
/// them survive a restart. See the module doc for the journal's order and
/// what each crash leaves.
pub struct OutboundChannels {
    journal: Arc<dyn Journal>,
    payers: Vec<(SettlementChain, Arc<dyn BatchSettlementPayer>)>,
    channels: Mutex<BTreeMap<String, Tracked>>,
    /// One open at a time, so two concurrent opens toward one receiver
    /// cannot both miss the other's pending record.
    opening: tokio::sync::Mutex<()>,
}

impl OutboundChannels {
    /// Replay `journal` and restore every channel it names to its chain's
    /// payer. A channel whose chain cannot be read now is kept, and
    /// restored on first use; one journaled as opening that the chain holds
    /// is journaled as opened. Fails only on a journal that cannot be read
    /// or a record that cannot be decoded: a record this node cannot read
    /// is a channel it cannot find, and starting without it would strand
    /// whatever is in it.
    pub async fn restore(
        journal: Arc<dyn Journal>,
        payers: Vec<(SettlementChain, Arc<dyn BatchSettlementPayer>)>,
    ) -> Result<OutboundChannels, JournalError> {
        let mut channels: BTreeMap<String, Tracked> = BTreeMap::new();
        let mut by_key: BTreeMap<String, String> = BTreeMap::new();
        for entry in journal.read_all()? {
            match entry {
                JournalEntry::OutboundChannelOpening { channel_id, record } => {
                    let record = OutboundChannelRecord::decode(&record).ok_or_else(|| {
                        JournalError::Corrupt(format!(
                            "outbound channel '{channel_id}' has a record this build cannot read"
                        ))
                    })?;
                    let id = record.channel().0.clone();
                    by_key.insert(channel_id, id.clone());
                    channels.insert(
                        id,
                        Tracked {
                            chain: chain_of(record.chain()),
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
                // Another book's entry kinds never reach this file.
                _ => {}
            }
        }
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

    fn tracked(&self, id: &str) -> Result<Tracked, BatchChannelError> {
        self.channels()
            .get(id)
            .cloned()
            .ok_or_else(|| BatchChannelError::UnknownChannel(id.to_string()))
    }

    fn payer(
        &self,
        chain: SettlementChain,
    ) -> Result<&Arc<dyn BatchSettlementPayer>, BatchChannelError> {
        self.payers
            .iter()
            .find(|(configured, _)| *configured == chain)
            .map(|(_, payer)| payer)
            .ok_or(BatchChannelError::NoBackend(chain))
    }

    fn append(&self, entry: JournalEntry) -> Result<(), BatchChannelError> {
        self.journal.append(&entry).map_err(BatchChannelError::from)
    }

    /// Hand the channel `id` to its payer with its journaled watermark, and
    /// journal an open the chain turns out to hold. A channel still opening
    /// that the chain does not hold stays opening.
    async fn restore_one(&self, id: &str) -> Result<OutboundChannelState, BatchChannelError> {
        let tracked = self.tracked(id)?;
        let payer = self.payer(tracked.chain)?;
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
        let id = canonical_id(id);
        let tracked = self.tracked(&id)?;
        if !tracked.restored {
            self.restore_one(&id).await?;
        }
        Ok((
            tracked.record.channel().clone(),
            Arc::clone(self.payer(tracked.chain)?),
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
        let chain = chain_of(terms.chain());
        let payer = Arc::clone(self.payer(chain)?);
        let _opening = self.opening.lock().await;
        let receiver = match &terms {
            ReceiverTerms::Evm(evm) => evm.receiver.to_vec(),
            ReceiverTerms::Solana(solana) => solana.receiver.to_vec(),
        };
        // The same open: same chain, same receiver, same deposit. A request
        // that differs is a different open, so a stale pending record --
        // one whose terms the counterparty has since changed, say -- never
        // traps every later open toward that receiver.
        let pending = self
            .channels()
            .values()
            .find(|tracked| {
                tracked.chain == chain
                    && !tracked.opened
                    && tracked.record.receiver() == receiver
                    && tracked.record.deposit() == deposit
            })
            .map(|tracked| tracked.record.clone());
        let (mut record, mut resumed) = match pending {
            Some(record) => (record, true),
            None => (
                self.prepare(&payer, chain, terms.clone(), deposit).await?,
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
                    record = self.prepare(&payer, chain, terms.clone(), deposit).await?;
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
        chain: SettlementChain,
        terms: ReceiverTerms,
        deposit: u128,
    ) -> Result<OutboundChannelRecord, BatchChannelError> {
        let record = payer.prepare_open(terms, deposit).await?;
        self.append(JournalEntry::OutboundChannelOpening {
            channel_id: journal_key(chain, record.channel()),
            record: record.encode(),
        })?;
        self.channels().insert(
            record.channel().0.clone(),
            Tracked {
                chain,
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
        let (channel, payer) = self.ready(id).await?;
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
    /// this party", to the ones the chain still shows open.
    pub async fn live_toward(
        &self,
        receiver: &VoucherSigner,
    ) -> Result<Option<String>, BatchChannelError> {
        let mut candidates: Vec<(String, u128)> = self
            .opened_toward(receiver)
            .into_iter()
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

    /// Raise an outbound channel's signed watermark to `amount`, where the
    /// receiver reports holding a voucher that high (ADR 0075 decision 6:
    /// the receiver's `POST /ilp/claim-state` is the watermark authority on
    /// restore). A node that lost its journal would otherwise sign a voucher
    /// that fails to advance; one that did not already stands at least this
    /// high, and nothing changes.
    ///
    /// Journaled as a signed voucher before it is believed, because it is
    /// one: the receiver holds a voucher at `amount` only if this node's key
    /// signed it.
    pub async fn raise_watermark(&self, id: &str, amount: u128) -> Result<u128, BatchChannelError> {
        let id = canonical_id(id);
        let tracked = self.tracked(&id)?;
        if amount <= tracked.signed {
            return Ok(tracked.signed);
        }
        let payer = Arc::clone(self.payer(tracked.chain)?);
        self.append(JournalEntry::OutboundVoucherSigned {
            channel_id: journal_key(tracked.chain, tracked.record.channel()),
            cumulative_amount: amount,
        })?;
        payer.restore_outbound(&tracked.record, amount).await?;
        if let Some(tracked) = self.channels().get_mut(&id) {
            tracked.signed = tracked.signed.max(amount);
            tracked.restored = true;
        }
        Ok(amount)
    }

    /// The channel as its receiver is shown it -- on EVM with the config its
    /// first voucher carries.
    pub fn presentation(&self, id: &str) -> Option<ChannelPresentation> {
        self.channels()
            .get(&canonical_id(id))
            .map(|tracked| tracked.record.presentation())
    }

    /// Whether `id` names an outbound channel this node journaled.
    pub fn knows(&self, id: &str) -> bool {
        self.channels().contains_key(&canonical_id(id))
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
        self.channels()
            .get(&canonical_id(id))
            .map(|tracked| tracked.signed)
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
                cumulative_amount: u64::try_from(tracked.signed).unwrap_or(u64::MAX),
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
    /// The receiving half on each chain.
    receivers: Vec<(SettlementChain, Arc<dyn BatchSettlementBackend>)>,
    /// The latest voucher held on each inbound channel: the client edge's
    /// claim gate, which is where vouchers are accepted and journaled.
    held: Arc<dyn HeldVouchers>,
    /// This node's own CAIP-2 network on each chain, as its greeting
    /// publishes it: a counterparty's terms must name the same one.
    networks: Vec<(SettlementChain, String)>,
}

impl BatchChannels {
    pub fn new(
        outbound: Arc<OutboundChannels>,
        receivers: Vec<(SettlementChain, Arc<dyn BatchSettlementBackend>)>,
        held: Arc<dyn HeldVouchers>,
        networks: Vec<(SettlementChain, String)>,
    ) -> BatchChannels {
        BatchChannels {
            outbound,
            receivers,
            held,
            networks,
        }
    }

    pub fn outbound(&self) -> &Arc<OutboundChannels> {
        &self.outbound
    }

    fn receiver(
        &self,
        chain: SettlementChain,
    ) -> Result<&Arc<dyn BatchSettlementBackend>, BatchChannelError> {
        self.receivers
            .iter()
            .find(|(configured, _)| *configured == chain)
            .map(|(_, backend)| backend)
            .ok_or(BatchChannelError::NoBackend(chain))
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
        let (chain, network) = match terms {
            X402BatchSettlementTerms::Evm(evm) => (SettlementChain::Evm, &evm.network),
            X402BatchSettlementTerms::Solana(solana) => (SettlementChain::Solana, &solana.network),
        };
        if let Some((_, own)) = self.networks.iter().find(|(c, _)| *c == chain) {
            if own != network {
                return Err(BatchChannelError::InvalidTerms(format!(
                    "the counterparty settles on {network}, and this node's {} backend on {own}",
                    chain.name()
                )));
            }
        }
        let terms =
            receiver_terms(terms, counterparty_url).map_err(BatchChannelError::InvalidTerms)?;
        let (state, resumed) = self.outbound.open(terms, deposit).await?;
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
                direction: ChannelDirection::Inbound,
                status: BatchChannelViewStatus::Unreadable,
                counterparty: String::new(),
                collateral: None,
                landed: None,
                watermark: held.voucher.cumulative_amount,
                detail: None,
            };
            let state = match self.receiver(chain) {
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
    pub async fn land(&self, id: &str) -> Result<BatchChannelView, BatchChannelError> {
        let id = canonical_id(id);
        let held = self
            .held
            .held_vouchers()
            .into_iter()
            .find(|held| held.presentation.channel().0 == id)
            .ok_or_else(|| BatchChannelError::NoVoucherHeld(id.clone()))?;
        let chain = chain_of(held.presentation.chain());
        let backend = self.receiver(chain)?;
        read_inbound(backend.as_ref(), &held.presentation).await?;
        let state = backend
            .land(held.presentation.channel(), held.voucher.clone())
            .await?;
        Ok(BatchChannelView {
            id,
            scheme: BATCH_SETTLEMENT.to_string(),
            chain: chain.name().to_string(),
            direction: ChannelDirection::Inbound,
            status: state.status.into(),
            counterparty: spell_signer(&state.voucher_signer),
            collateral: Some(state.collateral),
            landed: Some(state.landed),
            watermark: held.voucher.cumulative_amount,
            detail: None,
        })
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
            },
        };
        BatchChannelView {
            id: state.on_chain.id.0.clone(),
            scheme: BATCH_SETTLEMENT.to_string(),
            chain: chain.name().to_string(),
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
            OutboundChannels::restore(journal, vec![(chain_name(self.exit), payer)])
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
                    record: record.encode(),
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
                record: record.encode(),
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
                record: record.encode(),
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
                record: record.encode(),
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
            let chain = chain_name(exit);
            let channels = BatchChannels::new(
                Arc::new(world.boot(Arc::new(InMemoryJournal::new())).await),
                vec![(
                    chain,
                    Arc::clone(&world.receiver) as Arc<dyn BatchSettlementBackend>,
                )],
                held,
                Vec::new(),
            );

            let view = only_view(channels.inbound_views().await);
            assert_eq!(view.direction, ChannelDirection::Inbound);
            assert_eq!(view.watermark, 300);
            assert_eq!(view.landed, Some(0));

            let landed = channels.land(&channel.0).await.expect("land");
            assert_eq!(landed.landed, Some(300));
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
            vec![(SettlementChain::Evm, "eip155:8453".to_string())],
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
}
