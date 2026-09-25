//! The EVM half of ADR 0074 decision 5 (issue #1344): the
//! `WithdrawInitiated` watcher and the periodic claim-then-settle sweep over
//! every x402 `batch-settlement` channel this node holds a voucher on.
//!
//! # Why a watcher
//!
//! Only a `totalClaimed` recorded on chain protects this node. A voucher it
//! accepted but never claimed does not: once a payer's `initiateWithdraw`
//! delay passes, `finalizeWithdraw` takes everything above `totalClaimed`.
//! So on a `WithdrawInitiated` for a channel this node holds a voucher on,
//! [`EvmBatchWatcher::watch_once`] claims that voucher at once, and re-reads
//! the channel. There is no collateral cache to drop: the backend answers
//! every `channel_state` from the chain and the client edge's claim gate asks
//! it again for every voucher, because collateral **falls** when a
//! withdrawal is initiated (`client-edge-spec.md` §1.3 step 5 does not hold
//! here). The re-read is what tells the log what the withdrawal left.
//!
//! # Why a sweep
//!
//! [`EvmBatchWatcher::sweep_once`] claims every held voucher above what the
//! chain has recorded, many channels to one `claim`, and then moves what is
//! claimed to this node's address with `settle(receiver, token)`. It is also
//! what catches a withdrawal initiated while the node was down: the watcher
//! only reads logs from the block it started at, and the sweep runs as soon
//! as the node does.
//!
//! # Policy and I/O
//!
//! What to claim and whether to settle are [`claim_target`] and
//! [`settle_due`], pure functions with no chain and no clock. Everything else
//! here is the thin shell that reads the chain, calls them and sends what
//! they decide. Every read and write goes through the backend's own client
//! and sender, so the table's one `RpcTransport` (ADR 0073) and the
//! settlement key's one nonce sequence.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use connector_settlement::batch::{
    BatchSettlementBackend, BatchSettlementError, ChannelPresentation, EvmChannelConfig,
    HeldVoucher, HeldVouchers,
};
use connector_settlement::ChannelId;
use connector_signer::{evm_voucher_signer, verify_evm_voucher};
use ethers::abi::{AbiDecode, AbiEncode};
use ethers::middleware::Middleware;
use ethers::types::Bytes;

use crate::batch_settlement::{
    backend_error, chain_config, parse_id, signer_config, EvmBatchSettlementBackend,
};
use crate::bindings::x402_batch_settlement::{
    ChannelsCall, ChannelsReturn, Voucher as X402Voucher, VoucherClaim,
};
use crate::channel_id::format_channel_id;
use crate::send::confirm;

/// How often the watcher reads new `WithdrawInitiated` logs. The same
/// cadence the channel-index syncer polls the same endpoint at
/// ([`crate::DEFAULT_POLL_INTERVAL`]): a withdrawal's delay is at least this
/// node's published minimum, a day by default (ADR 0074 decision 5), so what
/// the interval buys is margin, not correctness.
pub const WITHDRAWAL_WATCH_INTERVAL: Duration = Duration::from_secs(5);

/// How often the sweep claims every held voucher and settles. Ten minutes
/// keeps what a withdrawal could race to at most ten minutes of vouchers
/// even with the watcher down, while "claims are constant, settlement is
/// rare" (ADR 0004, 0005) still holds: one `claim` for every channel, and
/// one `settle`, per sweep that has anything to do.
pub const BATCH_SWEEP_INTERVAL: Duration = Duration::from_secs(600);

/// Widest `eth_getLogs` range asked for at once; the channel-index syncer's
/// bound, for the same provider caps.
const MAX_BLOCK_RANGE: u64 = 2_000;

/// Most channels one `multicall` read asks about: a node that has held
/// vouchers on many channels still reads them in bounded `eth_call`s.
const MAX_READS_PER_MULTICALL: usize = 100;

/// Most rows one `claim` carries. Each row verifies a signature and writes
/// two slots, so this keeps a sweep's transaction well inside a block
/// however many channels it covers.
const MAX_CLAIM_ROWS: usize = 32;

/// The `totalClaimed` to claim for a voucher of `voucher` on a channel whose
/// chain state is `balance` and `total_claimed`, or `None` if claiming would
/// record nothing.
///
/// The voucher's own amount when the channel still holds it; the whole
/// balance when it no longer does -- a withdrawal finalized first, which is
/// the race the watcher exists to win, and a row claiming less than its
/// voucher's `maxClaimableAmount` is one the contract accepts. A row above
/// `balance` would revert the whole batch (`ClaimExceedsBalance`), and one at
/// or below `total_claimed` is a no-op the contract skips, so neither is
/// sent.
pub fn claim_target(voucher: u128, balance: u128, total_claimed: u128) -> Option<u128> {
    let target = voucher.min(balance);
    (target > total_claimed).then_some(target)
}

/// What `settle(receiver, token)` would move to this node, or `None` when it
/// would move nothing: the receiver's `totalClaimed` above its
/// `totalSettled`.
pub fn settle_due(total_claimed: u128, total_settled: u128) -> Option<u128> {
    let due = total_claimed.saturating_sub(total_settled);
    (due > 0).then_some(due)
}

/// One row a claim landed: the channel and the `totalClaimed` it now has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    pub channel: ChannelId,
    pub total_claimed: u128,
}

/// What a sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub claimed: Vec<Claimed>,
    /// What `settle` moved to this node's address.
    pub settled: u128,
}

impl EvmBatchSettlementBackend {
    /// The chain's current block.
    pub async fn block_number(&self) -> Result<u64, BatchSettlementError> {
        self.client
            .get_block_number()
            .await
            .map(|block| block.as_u64())
            .map_err(backend_error)
    }

    /// Every channel a `WithdrawInitiated` in blocks `from..=to` names,
    /// once each, read in bounded ranges.
    pub async fn withdrawals_initiated(
        &self,
        from: u64,
        to: u64,
    ) -> Result<Vec<ChannelId>, BatchSettlementError> {
        let mut channels = Vec::new();
        let mut start = from;
        while start <= to {
            let end = start.saturating_add(MAX_BLOCK_RANGE - 1).min(to);
            let logs = self
                .contract
                .withdraw_initiated_filter()
                .from_block(start)
                .to_block(end)
                .query()
                .await
                .map_err(backend_error)?;
            for log in logs {
                let channel = format_channel_id(log.channel_id);
                if !channels.contains(&channel) {
                    channels.push(channel);
                }
            }
            start = end + 1;
        }
        Ok(channels)
    }

    /// Claim every one of `held` that would record more than the chain
    /// has, in as few `claim`s as [`MAX_CLAIM_ROWS`] allows. Returns the rows
    /// landed.
    ///
    /// A voucher is claimed only on a channel this backend admits -- one it
    /// has not admitted in this process is admitted first, from the
    /// voucher's own presentation -- and only when its signature is the
    /// channel's signer's: a bad row would revert every other row with it.
    /// Either failure skips that voucher, logged, and claims the rest.
    pub async fn claim_held(
        &self,
        held: &[HeldVoucher],
    ) -> Result<Vec<Claimed>, BatchSettlementError> {
        let mut candidates: Vec<(ChannelId, [u8; 32], EvmChannelConfig, u128, Bytes)> = Vec::new();
        for entry in held {
            let ChannelPresentation::Evm { channel, .. } = &entry.presentation else {
                continue;
            };
            let Some(config) = self.config_for(entry).await else {
                continue;
            };
            let Some(id) = parse_id(channel) else {
                continue;
            };
            let amount = entry.voucher.cumulative_amount;
            let signature: Option<[u8; 65]> = entry.voucher.signature.as_slice().try_into().ok();
            let signer = evm_voucher_signer(&signer_config(&config));
            let Some(signature) = signature.filter(|signature| {
                verify_evm_voucher(&self.domain(), &id, amount, signature, &signer)
            }) else {
                tracing::warn!(
                    %channel,
                    amount,
                    "a held batch-settlement voucher is not its channel's signer's; not claiming it"
                );
                continue;
            };
            candidates.push((
                format_channel_id(id),
                id,
                config,
                amount,
                Bytes::from(signature.to_vec()),
            ));
        }
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<[u8; 32]> = candidates.iter().map(|(_, id, ..)| *id).collect();
        let on_chain = self.read_channels(&ids).await?;
        let mut rows = Vec::new();
        let mut claimed = Vec::new();
        for ((channel, _, config, amount, signature), (balance, total_claimed)) in
            candidates.into_iter().zip(on_chain)
        {
            let Some(target) = claim_target(amount, balance, total_claimed) else {
                continue;
            };
            if target < amount {
                tracing::warn!(
                    %channel,
                    voucher = amount,
                    balance,
                    "a batch-settlement channel holds less than its latest voucher; claiming \
                     what it still holds"
                );
            }
            rows.push(VoucherClaim {
                voucher: X402Voucher {
                    channel: chain_config(&config),
                    max_claimable_amount: amount,
                },
                signature,
                total_claimed: target,
            });
            claimed.push(Claimed {
                channel,
                total_claimed: target,
            });
        }

        let mut landed = Vec::new();
        for (rows, claimed) in rows
            .chunks(MAX_CLAIM_ROWS)
            .zip(claimed.chunks(MAX_CLAIM_ROWS))
        {
            let hash = self
                .sender
                .send(self.contract.claim(rows.to_vec()).tx)
                .await
                .map_err(backend_error)?;
            confirm(&self.client, hash, self.confirm)
                .await
                .map_err(backend_error)?;
            landed.extend_from_slice(claimed);
        }
        Ok(landed)
    }

    /// `settle(receiver, token)` for this node's own address and token, when
    /// it would move anything. Returns what it moved.
    pub async fn settle_claimed(&self) -> Result<u128, BatchSettlementError> {
        let (total_claimed, total_settled) = self
            .contract
            .receivers(self.own_address, self.token)
            .call()
            .await
            .map_err(backend_error)?;
        let Some(due) = settle_due(total_claimed, total_settled) else {
            return Ok(0);
        };
        let hash = self
            .sender
            .send(self.contract.settle(self.own_address, self.token).tx)
            .await
            .map_err(backend_error)?;
        confirm(&self.client, hash, self.confirm)
            .await
            .map_err(backend_error)?;
        Ok(due)
    }

    /// The config `entry`'s channel was admitted under, admitting it now if
    /// this process has not. `None`, logged, when it cannot be.
    async fn config_for(&self, entry: &HeldVoucher) -> Option<EvmChannelConfig> {
        let channel = entry.presentation.channel();
        if let Some(config) = self.admitted_config(channel) {
            return Some(config);
        }
        match self.admit(entry.presentation.clone()).await {
            Ok(state) => self.admitted_config(&state.id),
            Err(error) => {
                tracing::warn!(
                    %channel,
                    %error,
                    "could not admit a batch-settlement channel this node holds a voucher on; \
                     not claiming it this time"
                );
                None
            }
        }
    }

    /// `(balance, totalClaimed)` for each of `ids`, in order, through the
    /// contract's own `multicall` of `channels(id)`, at most
    /// [`MAX_READS_PER_MULTICALL`] to one `eth_call`. Each answer is one
    /// block's reading of its channel, which is all a row needs.
    async fn read_channels(
        &self,
        ids: &[[u8; 32]],
    ) -> Result<Vec<(u128, u128)>, BatchSettlementError> {
        let mut read = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(MAX_READS_PER_MULTICALL) {
            let calls = chunk
                .iter()
                .map(|id| Bytes::from(ChannelsCall { channel_id: *id }.encode()))
                .collect();
            let answers = self
                .contract
                .multicall(calls)
                .call()
                .await
                .map_err(backend_error)?;
            if answers.len() != chunk.len() {
                return Err(BatchSettlementError::Backend(format!(
                    "x402BatchSettlement's multicall answered {} results for {} calls",
                    answers.len(),
                    chunk.len()
                )));
            }
            for answer in &answers {
                let channel = ChannelsReturn::decode(answer).map_err(backend_error)?;
                read.push((channel.balance, channel.total_claimed));
            }
        }
        Ok(read)
    }
}

/// The watcher and the sweep over one [`EvmBatchSettlementBackend`], reading
/// the vouchers to land from `held` (the client edge's claim gate, in a
/// running node). [`run`](Self::run) is what the runtime spawns; the two
/// steps are public so a test can drive them one at a time.
pub struct EvmBatchWatcher {
    backend: Arc<EvmBatchSettlementBackend>,
    held: Arc<dyn HeldVouchers>,
    /// The last block whose logs have been read; `None` until the first
    /// watch, which starts from the chain's head.
    cursor: Option<u64>,
}

impl EvmBatchWatcher {
    pub fn new(backend: Arc<EvmBatchSettlementBackend>, held: Arc<dyn HeldVouchers>) -> Self {
        EvmBatchWatcher {
            backend,
            held,
            cursor: None,
        }
    }

    /// Read the `WithdrawInitiated` logs since the last watch and, for every
    /// channel they name that this node holds a voucher on, claim that
    /// voucher now and re-read the channel. Returns the rows claimed.
    ///
    /// The first watch reads nothing: it starts the cursor at the chain's
    /// head. A withdrawal begun before then is the sweep's, which runs as
    /// soon as the watcher does.
    pub async fn watch_once(&mut self) -> Result<Vec<Claimed>, BatchSettlementError> {
        let head = self.backend.block_number().await?;
        let Some(cursor) = self.cursor else {
            self.cursor = Some(head);
            return Ok(Vec::new());
        };
        if head <= cursor {
            return Ok(Vec::new());
        }
        let withdrawing = self.backend.withdrawals_initiated(cursor + 1, head).await?;
        let held: HashMap<ChannelId, HeldVoucher> = self
            .held
            .held_vouchers()
            .into_iter()
            .filter_map(|entry| {
                let ChannelPresentation::Evm { channel, .. } = &entry.presentation else {
                    return None;
                };
                Some((format_channel_id(parse_id(channel)?), entry))
            })
            .collect();
        let at_risk: Vec<HeldVoucher> = withdrawing
            .iter()
            .filter_map(|channel| held.get(channel).cloned())
            .collect();
        let claimed = if at_risk.is_empty() {
            Vec::new()
        } else {
            self.backend.claim_held(&at_risk).await?
        };
        // Only now is the cursor moved: a claim that failed is retried from
        // the same logs on the next watch.
        self.cursor = Some(head);
        for entry in &at_risk {
            let channel = entry.presentation.channel();
            match self.backend.channel_state(channel).await {
                Ok(state) => tracing::info!(
                    %channel,
                    landed = state.landed,
                    collateral = state.collateral,
                    "a batch-settlement payer began a withdrawal; claimed its latest voucher and \
                     re-read what the channel still backs"
                ),
                Err(error) => tracing::warn!(
                    %channel,
                    %error,
                    "a batch-settlement payer began a withdrawal; could not re-read the channel"
                ),
            }
        }
        Ok(claimed)
    }

    /// Claim every held voucher the chain has not recorded, then settle what
    /// is claimed to this node's address.
    pub async fn sweep_once(&self) -> Result<SweepReport, BatchSettlementError> {
        let claimed = self.backend.claim_held(&self.held.held_vouchers()).await?;
        let settled = self.backend.settle_claimed().await?;
        Ok(SweepReport { claimed, settled })
    }

    /// Watch every `watch_every` and sweep every `sweep_every`, both from
    /// now, for the life of the process. A step that fails is logged and
    /// retried on its next tick; nothing here ever returns.
    pub async fn run(mut self, watch_every: Duration, sweep_every: Duration) {
        let mut watch = tokio::time::interval(watch_every);
        let mut sweep = tokio::time::interval(sweep_every);
        watch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = watch.tick() => {
                    if let Err(error) = self.watch_once().await {
                        tracing::warn!(%error, "batch-settlement withdrawal watch failed; retrying");
                    }
                }
                _ = sweep.tick() => {
                    match self.sweep_once().await {
                        Ok(report) if !report.claimed.is_empty() || report.settled > 0 => {
                            tracing::info!(
                                channels = report.claimed.len(),
                                settled = report.settled,
                                "batch-settlement sweep claimed and settled"
                            );
                        }
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!(%error, "batch-settlement sweep failed; retrying");
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// A claim never records more than the voucher signs or the channel
        /// holds, and is sent only when it records something new.
        #[test]
        fn a_claim_target_is_backed_signed_and_new(
            voucher in 0u128..1_000_000,
            balance in 0u128..1_000_000,
            total_claimed in 0u128..1_000_000,
        ) {
            match claim_target(voucher, balance, total_claimed) {
                Some(target) => {
                    prop_assert!(target <= voucher);
                    prop_assert!(target <= balance);
                    prop_assert!(target > total_claimed);
                }
                None => prop_assert!(voucher.min(balance) <= total_claimed),
            }
        }

        /// While the channel still holds it, the voucher is claimed whole.
        #[test]
        fn a_backed_voucher_is_claimed_whole(
            total_claimed in 0u128..1_000,
            above in 1u128..1_000,
            spare in 0u128..1_000,
        ) {
            let voucher = total_claimed + above;
            prop_assert_eq!(
                claim_target(voucher, voucher + spare, total_claimed),
                Some(voucher)
            );
        }

        #[test]
        fn settle_moves_exactly_what_is_claimed_and_unsettled(
            total_settled in 0u128..1_000_000,
            more in 0u128..1_000_000,
        ) {
            let due = settle_due(total_settled + more, total_settled);
            prop_assert_eq!(due, (more > 0).then_some(more));
        }
    }

    /// A withdrawal that finalized first leaves less than the voucher; what
    /// is left is still claimed rather than nothing.
    #[test]
    fn a_drained_channel_is_claimed_to_its_balance() {
        assert_eq!(claim_target(500, 300, 100), Some(300));
        assert_eq!(claim_target(500, 100, 100), None);
    }
}
