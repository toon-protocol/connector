//! The watcher: this node's answer to a payer's exit (ADR 0087 decision 8,
//! issue #1617).
//!
//! **A payer that starts an exit is answered at once.** The moment a channel
//! this node is the destination of carries an `Expiration`, the watcher sends
//! one destination `PaymentChannelClaim` with `tfClose`: the latest held
//! voucher is landed, the payer is refunded the rest and the ledger entry is
//! deleted, like Solana's `settle_and_seal`, before the delay can run out on
//! the node. A channel with nothing worth landing is closed with `tfClose`
//! alone, so the payer's refund is not held and nobody's reserve stays locked.
//!
//! **The accepted cost** is the payer's: one that began an exit and then
//! wanted to `tfRenew` loses the chance once this node has seen it.
//!
//! The channels it looks at are those the claim gate holds a voucher on and
//! those the backend has admitted or restored. A channel the ledger no longer
//! holds reads as `ChannelNotFound`, which is the end of it: it is remembered
//! as done and never read again.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use connector_chain_rpc::{RefusalLog, Refused};
use connector_settlement::batch::{
    BatchSettlementError, ChannelPresentation, HeldVouchers, Voucher,
};
use connector_settlement::ChannelId;
use connector_signer::{verify_xrpl_claim, XRPL_MAX_DROPS};

use crate::batch::{Channel, XrplBatchSettlement};
use crate::tx::{LandClaim, SourceClose};

impl XrplBatchSettlement {
    /// Close `channel` with `tfClose` in one destination
    /// `PaymentChannelClaim`, landing `voucher` first when it is worth
    /// landing: genuine under the channel's key, above what is delivered and
    /// within the deposit. A voucher that is not is left out, and the close
    /// stands alone. Returns the drops delivered by the transaction.
    async fn close_with(
        &self,
        id: &ChannelId,
        id_bytes: [u8; 32],
        read: &Channel,
        voucher: Option<&Voucher>,
    ) -> Result<u64, BatchSettlementError> {
        let key = read.key.ok_or_else(|| {
            BatchSettlementError::Backend(format!(
                "channel {id} has a PublicKey that is not a 33-byte channel key"
            ))
        })?;
        let landable = voucher.and_then(|voucher| {
            let drops = u64::try_from(voucher.cumulative_amount)
                .ok()
                .filter(|drops| *drops <= XRPL_MAX_DROPS)?;
            let worth = drops > read.balance && drops <= read.amount;
            let genuine = verify_xrpl_claim(
                &key,
                &id_bytes,
                voucher.cumulative_amount,
                &voucher.signature,
            );
            if worth && !genuine {
                tracing::warn!(
                    channel = %id,
                    amount = drops,
                    "a held XRPL voucher is not its channel key's; closing without landing it"
                );
            }
            (worth && genuine).then(|| (drops, voucher.signature.clone()))
        });
        let account = self.signer.account_id();
        let _turn = self.sequence_turn().await;
        let sent = self
            .send(
                |sequence, fee_drops, last_ledger_sequence| match &landable {
                    Some((drops, signature)) => LandClaim {
                        account,
                        channel: id_bytes,
                        drops: *drops,
                        claim_signature: signature.clone(),
                        channel_key: key,
                        close: true,
                        fee_drops,
                        sequence,
                        last_ledger_sequence,
                    }
                    .sign(&self.signer),
                    None => SourceClose {
                        account,
                        channel: id_bytes,
                        fee_drops,
                        sequence,
                        last_ledger_sequence,
                    }
                    .sign(&self.signer),
                },
            )
            .await?;
        if sent.result != "tesSUCCESS" {
            return Err(BatchSettlementError::Backend(format!(
                "the closing PaymentChannelClaim {} landed as {}",
                sent.hash, sent.result
            )));
        }
        match self.read(id).await {
            Err(BatchSettlementError::ChannelNotFound(_)) => {}
            Ok(_) => {
                return Err(BatchSettlementError::Backend(format!(
                    "the closing PaymentChannelClaim {} answered tesSUCCESS but channel {id} \
                     is still on the ledger",
                    sent.hash
                )))
            }
            Err(other) => return Err(other),
        }
        Ok(landable.map_or(0, |(drops, _)| drops.saturating_sub(read.balance)))
    }
}

/// One channel the watcher closed, and the drops its voucher delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closed {
    pub channel: ChannelId,
    pub delivered: u64,
}

/// The watcher over one [`XrplBatchSettlement`], reading the vouchers to land
/// from `held` (the client edge's claim gate, in a running node).
/// [`run`](Self::run) is what the runtime spawns; [`tick`](Self::tick) is one
/// pass, public so a test can drive it.
pub struct XrplBatchWatcher {
    backend: Arc<XrplBatchSettlement>,
    held: Arc<dyn HeldVouchers>,
    /// Channels the ledger no longer holds: never read again.
    gone: Mutex<HashSet<String>>,
    /// Whether the endpoint is refusing, so [`run`](Self::run) says so once.
    refusals: RefusalLog,
}

impl XrplBatchWatcher {
    pub fn new(backend: Arc<XrplBatchSettlement>, held: Arc<dyn HeldVouchers>) -> Self {
        XrplBatchWatcher {
            backend,
            held,
            gone: Mutex::new(HashSet::new()),
            refusals: RefusalLog::new(),
        }
    }

    /// One pass over every inbound channel: close the ones whose payer has
    /// begun to leave. A channel without `Expiration` is left untouched; one
    /// whose close failed is logged and tried again next pass.
    pub async fn tick(&self) -> Result<Vec<Closed>, BatchSettlementError> {
        let mut vouchers = std::collections::HashMap::new();
        for entry in self.held.held_vouchers() {
            if let ChannelPresentation::Xrpl { channel } = &entry.presentation {
                if let Ok((id, _)) = XrplBatchSettlement::parse(channel) {
                    vouchers.insert(id.0, entry.voucher);
                }
            }
        }
        let mut channels: Vec<String> = vouchers.keys().cloned().collect();
        for admitted in self.backend.admitted_channels() {
            if !vouchers.contains_key(&admitted.0) {
                channels.push(admitted.0);
            }
        }
        channels.sort();

        let mut closed = Vec::new();
        for name in channels {
            if self.gone.lock().expect("gone").contains(&name) {
                continue;
            }
            let id = ChannelId(name.clone());
            let Ok((_, id_bytes)) = XrplBatchSettlement::parse(&id) else {
                continue;
            };
            let read = match self.backend.read(&id).await {
                Ok(read) => read,
                Err(BatchSettlementError::ChannelNotFound(_)) => {
                    self.gone.lock().expect("gone").insert(name);
                    continue;
                }
                Err(error) => {
                    self.report(&error, &id, "reading the channel");
                    continue;
                }
            };
            if read.expiration.is_none() || read.destination != self.backend.address() {
                continue;
            }
            match self
                .backend
                .close_with(&id, id_bytes, &read, vouchers.get(&name))
                .await
            {
                Ok(delivered) => {
                    tracing::info!(
                        channel = %id,
                        delivered,
                        "an XRPL payer began to exit; landed its latest voucher with tfClose"
                    );
                    self.gone.lock().expect("gone").insert(name);
                    closed.push(Closed {
                        channel: id,
                        delivered,
                    });
                }
                Err(error) => self.report(&error, &id, "closing the channel"),
            }
        }
        Ok(closed)
    }

    /// Log `error`, unless it is a refusal, which [`RefusalLog`] says once.
    fn report(&self, error: &BatchSettlementError, channel: &ChannelId, doing: &str) {
        match error {
            BatchSettlementError::Refused { endpoint, status } => {
                self.refusals.refused(&Refused {
                    endpoint: endpoint.clone(),
                    status: *status,
                });
            }
            _ => {
                tracing::warn!(%channel, %error, "XRPL watcher failed while {doing}; retrying next pass")
            }
        }
    }

    /// Pass every `every`, for the life of the process. A pass that fails is
    /// logged and retried on the next; nothing here ever returns.
    pub async fn run(self, every: Duration) {
        let mut watch = tokio::time::interval(every);
        watch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            watch.tick().await;
            self.refusals.begin_pass();
            match self.tick().await {
                Ok(_) => self.refusals.end_pass(),
                Err(error) => tracing::warn!(%error, "XRPL watch failed; retrying"),
            }
        }
    }
}
