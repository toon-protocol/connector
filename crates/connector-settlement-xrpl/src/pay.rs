//! The paying half of the batch-settlement port over the ledger's own
//! `PayChannel` (ADR 0075 decisions 2, 3 and 8, ADR 0087, issue #1613): this
//! node as the **source** of a channel toward a counterparty.
//!
//! **One key, every seat that is this node's.** The `[settlement.xrpl.<id>]`
//! settlement key is the channel's source, signs every transaction here, is
//! the channel's `PublicKey` and so signs every voucher. `[signer]` never
//! appears (ADR 0075 decision 3).
//!
//! **Opening is at most once (ADR 0075 decision 8).** `prepare_open` signs a
//! `PaymentChannelCreate` at a fixed `Sequence` and `LastLedgerSequence` --
//! never with `CancelAfter` -- and derives the channel id offline, from the
//! source, the destination and that `Sequence`. The caller journals the signed
//! blob; `open_prepared` submits the *same* blob, so a retry after a crash at
//! any point lands the one channel and never a second: a `Sequence` is
//! consumed once. A channel the ledger already holds is adopted and nothing is
//! sent. A blob past its `LastLedgerSequence`, or whose `Sequence` something
//! else consumed, with no channel to show for it, never lands:
//! [`OpenLapsed`](BatchSettlementError::OpenLapsed). The channel is used only
//! once a *validated* ledger holds it.
//!
//! **One sequence allocator per account.** Every transaction this node sends
//! from the account takes `BatchSettlementBackend`'s `sequencer` lock, signs at
//! the ledger's next `Sequence` and holds the lock until the ledger validates
//! it or can never. A prepared open's `Sequence` is held aside, in process, so
//! no other transaction of this node takes it; a prepared open that is never
//! sent holds it only for [`RESERVATION_PATIENCE`].
//!
//! **Winding down is `tfClose`, then the delay.** The source's
//! `PaymentChannelClaim` with `tfClose` sets `Expiration` while something is
//! unlanded -- the receiver then has the `SettleDelay` to land its latest
//! voucher -- and deletes the channel at once when nothing is. After
//! `Expiration`, a source transaction on the channel deletes it and refunds
//! the source what the receiver did not land.
//!
//! **A channel that is gone is answered from the cache** (ADR 0075, "Update
//! (issue #1565)" rule 2): the ledger answers `entryNotFound` for a channel
//! that closed and one that never was, so this half keeps the last state it
//! read, `outbound_state` returns it, and `finish_withdrawal` is
//! `NoWithdrawalPending`.
//!
//! **Money is checked before it is signed.** An open or a top-up the
//! account cannot afford -- deposit, the owner reserve a new channel locks
//! (read live), and the fee, above the account's own reserve -- is refused
//! naming the shortfall, never left to a `tecUNFUNDED`. A fee above the
//! backend's `max_fee_drops` is refused, naming it.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use connector_chain_rpc::xrpl::TxFate;
use connector_settlement::batch::{
    BatchChannelState, BatchChannelStatus, BatchSettlementError, BatchSettlementPayer,
    ChannelPresentation, OpenedChannel, OutboundChannelRecord, OutboundChannelState, ReceiverTerms,
    Voucher, VoucherSigner,
};
use connector_settlement::ChannelId;
use connector_signer::{
    decode_xrpl_classic_address, xrpl_channel_id, xrpl_classic_address, XRPL_MAX_DROPS,
};
use serde_json::Value;

use crate::batch::{rpc_error, Channel, XrplBatchSettlement, CHAIN};
use crate::tx::{blob_hash, Create, Fund, SignedTx, SourceClose};

/// How long another transaction of this node waits for a prepared open to be
/// sent before it takes the open's `Sequence`, which lapses that open.
const RESERVATION_PATIENCE: Duration = Duration::from_secs(30);

/// What the paying half remembers about one channel it opened, for the
/// process lifetime.
#[derive(Debug, Clone)]
pub(crate) struct Outbound {
    /// The highest cumulative amount signed: the next voucher must exceed it.
    signed: u128,
    /// The highest cumulative amount the channel backs, as of this node's own
    /// last open, top-up or exit: its `Amount` while the payer has not begun
    /// to leave, what was delivered once it has. Only this node moves it,
    /// which is what lets a voucher be signed without reading the ledger.
    backed: u128,
    /// The channel as last read, kept because a closed channel leaves nothing
    /// to read.
    last: BatchChannelState,
    /// The channel as it stood once it was gone from the ledger.
    finished: Option<BatchChannelState>,
}

/// One transaction's outcome: its hash, its result code and its metadata.
pub(crate) struct Sent {
    pub(crate) hash: String,
    pub(crate) result: String,
    pub(crate) meta: Value,
}

impl XrplBatchSettlement {
    /// This node's settlement key as the ledger names a voucher signer.
    fn settlement_key(&self) -> VoucherSigner {
        VoucherSigner::Xrpl(self.signer.public_key())
    }

    /// This node's settlement key: the channel key of every channel it opens.
    pub fn settlement_public_key(&self) -> [u8; 33] {
        self.signer.public_key()
    }

    fn caip2(&self) -> String {
        format!("xrpl:{}", self.network_id)
    }

    fn outbound_map(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, Outbound>> {
        self.outbound.lock().expect("XrplBatchSettlement outbound")
    }

    /// The channel `channel` names and what this node remembers of it, or
    /// [`NotOutbound`](BatchSettlementError::NotOutbound).
    fn outbound_entry(
        &self,
        channel: &ChannelId,
    ) -> Result<(ChannelId, [u8; 32], Outbound), BatchSettlementError> {
        let (id, bytes) =
            Self::parse(channel).map_err(|_| BatchSettlementError::NotOutbound(channel.clone()))?;
        let entry = self
            .outbound_map()
            .get(&id.0)
            .cloned()
            .ok_or_else(|| BatchSettlementError::NotOutbound(channel.clone()))?;
        Ok((id, bytes, entry))
    }

    fn update(&self, id: &ChannelId, change: impl FnOnce(&mut Outbound)) {
        if let Some(entry) = self.outbound_map().get_mut(&id.0) {
            change(entry);
        }
    }

    /// The channel in a validated ledger, or `None` once it is gone.
    async fn read_channel(&self, id: &ChannelId) -> Result<Option<Channel>, BatchSettlementError> {
        match self.read(id).await {
            Ok(channel) => Ok(Some(channel)),
            Err(BatchSettlementError::ChannelNotFound(_)) => Ok(None),
            Err(other) => Err(other),
        }
    }

    /// Record what the ledger shows of a channel this node opened: its state
    /// if it is there (and the cache of it), or, once it is gone, the last
    /// state this node read with nothing left backing a voucher.
    fn observe(
        &self,
        id: &ChannelId,
        channel: Option<&Channel>,
    ) -> Result<(BatchChannelState, bool), BatchSettlementError> {
        match channel {
            Some(channel) => {
                let state = Self::state_of(id, channel)?;
                self.update(id, |entry| entry.last = state.clone());
                Ok((state, true))
            }
            None => {
                let mut map = self.outbound_map();
                let entry = map
                    .get_mut(&id.0)
                    .ok_or_else(|| BatchSettlementError::NotOutbound(id.clone()))?;
                let finished = entry.finished.clone().unwrap_or_else(|| BatchChannelState {
                    status: BatchChannelStatus::Closing,
                    collateral: 0,
                    ..entry.last.clone()
                });
                entry.finished = Some(finished.clone());
                entry.backed = finished.landed;
                Ok((finished, false))
            }
        }
    }

    /// The account's drops spendable above its reserve, with `new_objects`
    /// more owned objects, against `drops` plus the fee; refused naming the
    /// shortfall (ADR 0087 decision 6).
    async fn require_funds(
        &self,
        drops: u64,
        new_objects: u32,
    ) -> Result<(), BatchSettlementError> {
        let account = self
            .rpc
            .account_info(&self.address)
            .await
            .map_err(rpc_error)?
            .ok_or_else(|| {
                BatchSettlementError::Backend(format!(
                    "the settlement account {} does not exist",
                    self.address
                ))
            })?;
        let reserve = self.rpc.reserve().await.map_err(rpc_error)?;
        let (fee, _) = self.fee().await?;
        let locked = u128::from(reserve.base_drops)
            + u128::from(reserve.owner_drops) * u128::from(account.owner_count + new_objects);
        let spendable = u128::from(account.balance_drops).saturating_sub(locked);
        let required = u128::from(drops) + u128::from(fee);
        if required > spendable {
            return Err(BatchSettlementError::Underfunded {
                required,
                spendable,
                shortfall: required - spendable,
            });
        }
        Ok(())
    }

    /// The fee to sign with and the ledger the open ledger will become;
    /// refused above `max_fee_drops`.
    async fn fee(&self) -> Result<(u64, u32), BatchSettlementError> {
        let fee = self.rpc.fee().await.map_err(rpc_error)?;
        let drops = fee.open_ledger_fee_drops.max(fee.base_fee_drops);
        if drops > self.max_fee_drops {
            return Err(BatchSettlementError::Backend(format!(
                "the ledger's fee of {drops} drops is above max_fee_drops of {}",
                self.max_fee_drops
            )));
        }
        Ok((drops, fee.ledger_current_index))
    }

    /// Take the sequencer for a transaction signed at the account's next
    /// `Sequence`. While that `Sequence` belongs to a prepared open that has
    /// not been sent, the sequencer is let go -- the open needs it to be sent
    /// -- for up to [`RESERVATION_PATIENCE`]. The reservation is judged under
    /// the lock, so no open prepared in between is overtaken unawaited.
    pub(crate) async fn sequence_turn(&self) -> tokio::sync::MutexGuard<'_, ()> {
        let deadline = Instant::now() + RESERVATION_PATIENCE;
        loop {
            let turn = self.sequencer.lock().await;
            let Ok(Some(account)) = self.rpc.account_info(&self.address).await else {
                return turn;
            };
            let held = {
                let mut reserved = self.reserved.lock().expect("reserved");
                reserved.retain(|sequence| *sequence >= account.sequence);
                reserved.contains(&account.sequence)
            };
            if !held || Instant::now() >= deadline {
                return turn;
            }
            drop(turn);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Sign at the account's next `Sequence` with `build`, submit and wait
    /// for the ledger's verdict. The caller holds the sequencer.
    pub(crate) async fn send(
        &self,
        build: impl FnOnce(u32, u64, u32) -> SignedTx,
    ) -> Result<Sent, BatchSettlementError> {
        let account = self
            .rpc
            .account_info(&self.address)
            .await
            .map_err(rpc_error)?
            .ok_or_else(|| {
                BatchSettlementError::Backend(format!(
                    "the settlement account {} does not exist",
                    self.address
                ))
            })?;
        let (fee, ledger) = self.fee().await?;
        let last_ledger_sequence = ledger + crate::batch::LAST_LEDGER_WINDOW;
        let signed = build(account.sequence, fee, last_ledger_sequence);
        self.submit_and_wait(&signed.blob_hex, &signed.hash, last_ledger_sequence)
            .await?
            .ok_or_else(|| {
                BatchSettlementError::Backend(format!(
                    "transaction {} did not land by ledger {last_ledger_sequence} and never \
                     can: nothing was spent",
                    signed.hash
                ))
            })
    }

    /// `None` when the ledger is known not to hold the transaction and never
    /// can: past its `LastLedgerSequence`.
    async fn submit_and_wait(
        &self,
        blob_hex: &str,
        hash: &str,
        last_ledger_sequence: u32,
    ) -> Result<Option<Sent>, BatchSettlementError> {
        let submitted = self.rpc.submit(blob_hex).await.map_err(rpc_error)?;
        // A definitive refusal at submission never lands; anything else is
        // settled by the bounded wait.
        // (`tefALREADY` is the ledger saying it holds this very blob already: a
        // retry of a send whose answer was lost, settled by the wait below.)
        if ["tem", "tef", "tel"]
            .iter()
            .any(|class| submitted.engine_result.starts_with(class))
            && submitted.engine_result != "tefALREADY"
        {
            return Err(BatchSettlementError::Backend(format!(
                "the ledger refused transaction {hash}: {}",
                submitted.engine_result
            )));
        }
        match self
            .rpc
            .wait_for_tx(hash, last_ledger_sequence, self.wait)
            .await
            .map_err(rpc_error)?
        {
            TxFate::KnownNotLanded { .. } => Ok(None),
            TxFate::Landed {
                transaction_result,
                meta,
                ..
            } => Ok(Some(Sent {
                hash: hash.to_string(),
                result: transaction_result,
                meta,
            })),
        }
    }

    /// Build an open at the account's next `Sequence`, held aside, and sign
    /// it. The caller holds the sequencer.
    async fn prepare_locked(
        &self,
        terms: ReceiverTerms,
        deposit: u128,
    ) -> Result<OutboundChannelRecord, BatchSettlementError> {
        let ReceiverTerms::Xrpl(terms) = terms else {
            return Err(BatchSettlementError::WrongChain {
                presented: terms.chain(),
                backend: CHAIN,
            });
        };
        let drops = u64::try_from(deposit)
            .ok()
            .filter(|drops| *drops <= XRPL_MAX_DROPS)
            .ok_or(BatchSettlementError::AmountExceedsChain {
                amount: deposit,
                max: XRPL_MAX_DROPS,
            })?;
        if drops == 0 {
            return Err(BatchSettlementError::Backend(
                "a channel is opened with a deposit above zero".to_string(),
            ));
        }
        let destination = decode_xrpl_classic_address(&terms.receiver).map_err(|_| {
            BatchSettlementError::Backend(format!(
                "the counterparty's address {} is not a classic address",
                terms.receiver
            ))
        })?;
        let settle_delay = u32::try_from(terms.min_settle_delay_secs).map_err(|_| {
            BatchSettlementError::Backend(format!(
                "a SettleDelay of {}s does not fit the ledger's 32 bits",
                terms.min_settle_delay_secs
            ))
        })?;
        self.require_funds(drops, 1).await?;

        let account = self
            .rpc
            .account_info(&self.address)
            .await
            .map_err(rpc_error)?
            .ok_or_else(|| {
                BatchSettlementError::Backend(format!(
                    "the settlement account {} does not exist",
                    self.address
                ))
            })?;
        let (fee, ledger) = self.fee().await?;
        let sequence = {
            let mut reserved = self.reserved.lock().expect("reserved");
            reserved.retain(|sequence| *sequence >= account.sequence);
            let sequence = reserved
                .last()
                .map_or(account.sequence, |last| (last + 1).max(account.sequence));
            reserved.insert(sequence);
            sequence
        };
        let last_ledger_sequence = ledger + self.open_window;
        let source = self.signer.account_id();
        let signed = Create {
            account: source,
            destination,
            drops,
            settle_delay_secs: settle_delay,
            fee_drops: fee,
            sequence,
            last_ledger_sequence,
        }
        .sign(&self.signer);
        Ok(OutboundChannelRecord::Xrpl {
            channel: ChannelId(hex::encode(xrpl_channel_id(
                &source,
                &destination,
                sequence,
            ))),
            receiver: destination,
            settle_delay_secs: terms.min_settle_delay_secs,
            deposit,
            sequence,
            last_ledger_sequence,
            transaction: signed.blob,
        })
    }

    /// Send the open `record` names, or find the channel it made. The caller
    /// holds the sequencer. The record's reservation is released once the
    /// open has landed or can never.
    async fn open_locked(
        &self,
        record: &OutboundChannelRecord,
    ) -> Result<OpenedChannel, BatchSettlementError> {
        let OutboundChannelRecord::Xrpl { sequence, .. } = record else {
            return Err(BatchSettlementError::WrongChain {
                presented: record.chain(),
                backend: CHAIN,
            });
        };
        let result = self.open_record(record).await;
        if matches!(result, Ok(_) | Err(BatchSettlementError::OpenLapsed(_))) {
            self.reserved.lock().expect("reserved").remove(sequence);
        }
        result
    }

    async fn open_record(
        &self,
        record: &OutboundChannelRecord,
    ) -> Result<OpenedChannel, BatchSettlementError> {
        let OutboundChannelRecord::Xrpl {
            channel,
            receiver,
            deposit,
            sequence,
            last_ledger_sequence,
            transaction,
            ..
        } = record
        else {
            unreachable!("open_locked matched an XRPL record");
        };
        let (id, _) = Self::parse(channel)
            .map_err(|_| BatchSettlementError::ChannelNotFound(channel.clone()))?;
        // The validated index is read *before* the channel, so a channel the
        // open made inside it is seen by the read that follows.
        let validated = self
            .rpc
            .server_info()
            .await
            .map_err(rpc_error)?
            .validated_ledger;
        if let Some(held) = self.read_channel(&id).await? {
            return self.adopt(record, &id, &held);
        }
        let account = self
            .rpc
            .account_info(&self.address)
            .await
            .map_err(rpc_error)?
            .ok_or_else(|| {
                BatchSettlementError::Backend(format!(
                    "the settlement account {} does not exist",
                    self.address
                ))
            })?;
        // The channel is absent but its `Sequence` is spent: either the open
        // has just landed, or something else took the number and the open
        // can never. Read once more to tell which.
        if account.sequence > *sequence {
            return match self.read_channel(&id).await? {
                Some(held) => self.adopt(record, &id, &held),
                None => Err(BatchSettlementError::OpenLapsed(channel.clone())),
            };
        }
        if validated.is_some_and(|validated| validated > *last_ledger_sequence) {
            return Err(BatchSettlementError::OpenLapsed(channel.clone()));
        }
        let drops =
            u64::try_from(*deposit).map_err(|_| BatchSettlementError::AmountExceedsChain {
                amount: *deposit,
                max: XRPL_MAX_DROPS,
            })?;
        self.require_funds(drops, 1).await?;
        debug_assert_eq!(
            xrpl_channel_id(&self.signer.account_id(), receiver, *sequence),
            <[u8; 32]>::try_from(hex::decode(&id.0).expect("hex")).expect("32 bytes")
        );

        let hash = blob_hash(transaction);
        // A blob the ledger is known never to take is a lapsed open.
        let sent = match self
            .submit_and_wait(
                &hex::encode_upper(transaction),
                &hash,
                *last_ledger_sequence,
            )
            .await
        {
            Ok(sent) => sent.ok_or_else(|| BatchSettlementError::OpenLapsed(channel.clone()))?,
            // A refusal such as `tefPAST_SEQ` may be this very blob, sent
            // before a crash, landing between the read above and the
            // submission: the channel it made is adopted.
            Err(error) => match self.read_channel(&id).await? {
                Some(held) => return self.adopt(record, &id, &held),
                None => return Err(error),
            },
        };
        if sent.result != "tesSUCCESS" {
            return Err(BatchSettlementError::Backend(format!(
                "the PaymentChannelCreate {} landed as {} and opened nothing; its record can \
                 never open a channel",
                sent.hash, sent.result
            )));
        }
        // Used only once a validated ledger holds it.
        let held = self
            .read_channel(&id)
            .await?
            .ok_or_else(|| BatchSettlementError::ChannelNotFound(id.clone()))?;
        self.adopt(record, &id, &held)
    }

    /// Take the channel the ledger holds as this node's own, if it is.
    fn adopt(
        &self,
        record: &OutboundChannelRecord,
        id: &ChannelId,
        held: &Channel,
    ) -> Result<OpenedChannel, BatchSettlementError> {
        let OutboundChannelRecord::Xrpl { receiver, .. } = record else {
            unreachable!("open_locked matched an XRPL record");
        };
        if held.account != self.address
            || held.key != Some(self.signer.public_key())
            || held.destination != xrpl_classic_address(receiver)
        {
            return Err(BatchSettlementError::NotOutbound(id.clone()));
        }
        let state = Self::state_of(id, held)?;
        self.outbound_map()
            .entry(id.0.clone())
            .and_modify(|entry| entry.last = state.clone())
            .or_insert(Outbound {
                signed: 0,
                backed: backed_by(held),
                last: state,
                finished: None,
            });
        Ok(OpenedChannel {
            presentation: ChannelPresentation::Xrpl {
                channel: id.clone(),
            },
            voucher_signer: self.settlement_key(),
        })
    }

    /// The state of an outbound channel with the figures this node keeps.
    fn outbound_state_of(&self, id: &ChannelId, state: BatchChannelState) -> OutboundChannelState {
        let signed = self
            .outbound_map()
            .get(&id.0)
            .map_or(0, |entry| entry.signed);
        OutboundChannelState {
            on_chain: state,
            signed,
        }
    }

    /// A source's `PaymentChannelClaim` with `tfClose` on `id`.
    async fn close(&self, id_bytes: [u8; 32]) -> Result<Sent, BatchSettlementError> {
        let _sequencer = self.sequence_turn().await;
        let account = self.signer.account_id();
        self.send(|sequence, fee_drops, last_ledger_sequence| {
            SourceClose {
                account,
                channel: id_bytes,
                fee_drops,
                sequence,
                last_ledger_sequence,
            }
            .sign(&self.signer)
        })
        .await
    }
}

/// What a channel backs a new voucher with: its whole `Amount` while its
/// payer has not begun to leave, and what was delivered once it has.
fn backed_by(channel: &Channel) -> u128 {
    if channel.expiration.is_some() {
        u128::from(channel.balance)
    } else {
        u128::from(channel.amount)
    }
}

#[async_trait]
impl BatchSettlementPayer for XrplBatchSettlement {
    async fn prepare_open(
        &self,
        terms: ReceiverTerms,
        deposit: u128,
    ) -> Result<OutboundChannelRecord, BatchSettlementError> {
        // A preparation sends nothing: it takes the `Sequence` after any
        // already held aside, so it need not wait for them.
        let _sequencer = self.sequencer.lock().await;
        self.prepare_locked(terms, deposit).await
    }

    async fn open_prepared(
        &self,
        record: &OutboundChannelRecord,
    ) -> Result<OpenedChannel, BatchSettlementError> {
        let _sequencer = self.sequencer.lock().await;
        self.open_locked(record).await
    }

    /// Prepared and sent under one hold of the sequencer, so no other
    /// transaction of this node can take the open's `Sequence` between them.
    async fn open(
        &self,
        terms: ReceiverTerms,
        deposit: u128,
    ) -> Result<OpenedChannel, BatchSettlementError> {
        let _sequencer = self.sequence_turn().await;
        let record = self.prepare_locked(terms, deposit).await?;
        self.open_locked(&record).await
    }

    async fn restore_outbound(
        &self,
        record: &OutboundChannelRecord,
        signed: u128,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let OutboundChannelRecord::Xrpl { channel, .. } = record else {
            return Err(BatchSettlementError::WrongChain {
                presented: record.chain(),
                backend: CHAIN,
            });
        };
        let (id, _) = Self::parse(channel)
            .map_err(|_| BatchSettlementError::ChannelNotFound(channel.clone()))?;
        let held = self.read(&id).await?;
        // The channel key must be this node's settlement key, or no voucher
        // signed for it verifies.
        if held.account != self.address || held.key != Some(self.signer.public_key()) {
            return Err(BatchSettlementError::NotOutbound(id));
        }
        let state = Self::state_of(&id, &held)?;
        // The ledger is a lower bound on what this node signed (ADR 0075
        // decision 6): the caller's figure, never below what is delivered.
        let signed = signed.max(state.landed);
        self.outbound_map().insert(
            id.0.clone(),
            Outbound {
                signed,
                backed: backed_by(&held),
                last: state.clone(),
                finished: None,
            },
        );
        Ok(OutboundChannelState {
            on_chain: state,
            signed,
        })
    }

    async fn top_up(
        &self,
        channel: &ChannelId,
        increment: u128,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let (id, id_bytes, entry) = self.outbound_entry(channel)?;
        if entry.finished.is_some() {
            return Err(BatchSettlementError::ChannelSealed(id));
        }
        let drops = u64::try_from(increment)
            .ok()
            .filter(|drops| *drops <= XRPL_MAX_DROPS)
            .ok_or(BatchSettlementError::AmountExceedsChain {
                amount: increment,
                max: XRPL_MAX_DROPS,
            })?;
        let Some(held) = self.read_channel(&id).await? else {
            self.observe(&id, None)?;
            return Err(BatchSettlementError::ChannelSealed(id));
        };
        self.observe(&id, Some(&held))?;
        if held.expiration.is_some() {
            // A payer that has begun to leave backs nothing new.
            return Err(BatchSettlementError::ChannelSealed(id));
        }
        let _sequencer = self.sequence_turn().await;
        self.require_funds(drops, 0).await?;
        let account = self.signer.account_id();
        let sent = self
            .send(|sequence, fee_drops, last_ledger_sequence| {
                Fund {
                    account,
                    channel: id_bytes,
                    drops,
                    fee_drops,
                    sequence,
                    last_ledger_sequence,
                }
                .sign(&self.signer)
            })
            .await?;
        if sent.result != "tesSUCCESS" {
            return Err(BatchSettlementError::Backend(format!(
                "the PaymentChannelFund {} landed as {} and funded nothing",
                sent.hash, sent.result
            )));
        }
        let held = self.read_channel(&id).await?;
        let (state, present) = self.observe(&id, held.as_ref())?;
        if !present {
            return Err(BatchSettlementError::ChannelSealed(id));
        }
        if let Some(held) = &held {
            self.update(&id, |entry| entry.backed = backed_by(held));
        }
        Ok(self.outbound_state_of(&id, state))
    }

    async fn sign_voucher(
        &self,
        channel: &ChannelId,
        cumulative_amount: u128,
    ) -> Result<Voucher, BatchSettlementError> {
        let (id, id_bytes, entry) = self.outbound_entry(channel)?;
        if entry.finished.is_some() {
            return Err(BatchSettlementError::ChannelSealed(id));
        }
        if cumulative_amount > u128::from(XRPL_MAX_DROPS) {
            return Err(BatchSettlementError::AmountExceedsChain {
                amount: cumulative_amount,
                max: XRPL_MAX_DROPS,
            });
        }
        if cumulative_amount <= entry.signed {
            return Err(BatchSettlementError::VoucherNotAdvancing {
                amount: cumulative_amount,
                signed: entry.signed,
            });
        }
        if cumulative_amount > entry.backed {
            return Err(BatchSettlementError::VoucherUnbacked {
                amount: cumulative_amount,
                backed: entry.backed,
            });
        }
        let signature = self
            .signer
            .sign_claim(&id_bytes, cumulative_amount)
            .map_err(|error| BatchSettlementError::InvalidVoucherSignature(error.to_string()))?;
        self.update(&id, |entry| entry.signed = cumulative_amount);
        Ok(Voucher {
            cumulative_amount,
            signature,
        })
    }

    async fn start_withdrawal(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let (id, id_bytes, entry) = self.outbound_entry(channel)?;
        if entry.finished.is_some() {
            return Err(BatchSettlementError::ChannelSealed(id));
        }
        let Some(held) = self.read_channel(&id).await? else {
            self.observe(&id, None)?;
            return Err(BatchSettlementError::ChannelSealed(id));
        };
        if held.expiration.is_some() {
            // Already leaving: nothing to send.
            self.update(&id, |entry| entry.backed = backed_by(&held));
            let (state, _) = self.observe(&id, Some(&held))?;
            return Ok(self.outbound_state_of(&id, state));
        }
        // What the ledger shows now is the last this node knows of it, should
        // the close delete it.
        self.observe(&id, Some(&held))?;
        let sent = self.close(id_bytes).await?;
        let after = self.read_channel(&id).await?;
        if sent.result != "tesSUCCESS" && after.is_some() {
            return Err(BatchSettlementError::Backend(format!(
                "the PaymentChannelClaim {} landed as {} and began no withdrawal",
                sent.hash, sent.result
            )));
        }
        // A channel with nothing unlanded is deleted by the close itself.
        match &after {
            Some(after) => self.update(&id, |entry| entry.backed = backed_by(after)),
            None => self.update(&id, |entry| entry.backed = entry.last.landed),
        }
        let (state, _) = self.observe(&id, after.as_ref())?;
        Ok(self.outbound_state_of(&id, state))
    }

    async fn finish_withdrawal(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let (id, id_bytes, entry) = self.outbound_entry(channel)?;
        if entry.finished.is_some() {
            return Err(BatchSettlementError::NoWithdrawalPending(id));
        }
        let Some(held) = self.read_channel(&id).await? else {
            self.observe(&id, None)?;
            return Err(BatchSettlementError::NoWithdrawalPending(id));
        };
        let Some(expiration) = held.expiration else {
            return Err(BatchSettlementError::NoWithdrawalPending(id));
        };
        self.update(&id, |entry| {
            entry.last = Self::state_of(&id, &held).unwrap_or_else(|_| entry.last.clone())
        });
        let now = self.rpc.validated_close_time().await.map_err(rpc_error)?;
        if now < expiration {
            return Err(BatchSettlementError::WithdrawalNotDue {
                channel: id,
                remaining_secs: expiration - now,
            });
        }
        let sent = self.close(id_bytes).await?;
        let after = self.read_channel(&id).await?;
        if after.is_some() {
            return Err(BatchSettlementError::Backend(format!(
                "the PaymentChannelClaim {} landed as {} and the channel was not deleted",
                sent.hash, sent.result
            )));
        }
        let (state, _) = self.observe(&id, None)?;
        Ok(self.outbound_state_of(&id, state))
    }

    async fn outbound_state(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let (id, _, entry) = self.outbound_entry(channel)?;
        if let Some(finished) = entry.finished {
            return Ok(self.outbound_state_of(&id, finished));
        }
        let held = self.read_channel(&id).await?;
        let (state, _) = self.observe(&id, held.as_ref())?;
        Ok(self.outbound_state_of(&id, state))
    }

    async fn sign_claim_state_challenge(
        &self,
        channel: &ChannelId,
        expires: u64,
    ) -> Result<Vec<u8>, BatchSettlementError> {
        let (_, id_bytes, _) = self.outbound_entry(channel)?;
        self.signer
            .sign_claim_state_challenge(&self.caip2(), &id_bytes, expires)
            .map_err(|error| BatchSettlementError::Backend(error.to_string()))
    }
}
