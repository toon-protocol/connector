//! What this connector pays a client back (ADR 0075 decision 7, issue
//! #1381): a **voucher** on an x402 `batch-settlement` channel this node
//! opened toward the client with the paying half of the settlement port.
//! `ClientClaimGate` (`crate::claim_gate`) handles the opposite direction, a
//! client paying this connector.
//!
//! # A connector→client channel, and no netting
//!
//! An x402 channel moves value one way, so a payout rides a channel of its
//! own: this node is its payer, and the client is its receiver. The channel
//! is one of this node's ordinary outbound channels
//! ([`connector_runtime::OutboundChannels`]): opened and funded by the
//! operator's signed `POST /channels` toward the terms the client publishes
//! (on Solana, through the client's own sponsor endpoint, so the client
//! holds the `payee` and `rent_payer` seats -- ADR 0075 decision 3), and
//! journaled, so its signed watermark survives a restart (decision 8).
//! Nothing here opens or tops up a channel: funding stays an operator write
//! (decision 11).
//!
//! ADR 0026's #700 netting is gone: a payout no longer raises what the
//! client may spend on its own inbound channel, and nothing the client pays
//! in funds its payouts. The two channels are independent.
//!
//! # Which channel pays which client
//!
//! A client is paid on a channel whose receiver is its **payee key** -- the
//! voucher signer of the channel it pays this node on (EVM
//! `payerAuthorizer`, Solana `authorized_signer`), which the claim gate has
//! verified a signature from (`ClientClaimGate::record_session_payee`). The
//! payout channel is found by that key among this node's opened outbound
//! channels ([`connector_runtime::OutboundChannels::opened_toward`]), a
//! lookup rather than a derivation (ADR 0075 decision 4).
//!
//! A voucher is landable only by its channel's receiver, so a payout
//! delivered to the wrong socket pays nobody else: the worst a mistaken
//! association can do is pay the right client's channel for work a
//! different session did.
//!
//! # What is signed, and by which key
//!
//! Each chain's settlement key signs every voucher on that chain (ADR 0075
//! decision 3): on EVM `payerAuthorizer == payer`, on Solana the payer is
//! `authorized_signer`. `[signer]` signs none. A payout's voucher is
//! cumulative, so a later one carries everything an earlier undelivered
//! one did.
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use connector_runtime::{BatchChannelError, OutboundChannels};
use connector_settlement::batch::{
    BatchSettlementError, ChannelPresentation, Voucher, VoucherSigner,
};

/// A payout voucher: what a payout TRANSFER carries to the client, and
/// everything the client needs to land it on chain itself -- the channel,
/// on EVM with the `ChannelConfig` `claim` needs, and the signed cumulative
/// amount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayoutVoucher {
    /// The client this voucher pays: the receiver of its channel.
    pub payee: VoucherSigner,
    /// The channel, as its receiver is shown it.
    pub presentation: ChannelPresentation,
    /// The signed cumulative amount.
    pub voucher: Voucher,
}

impl PayoutVoucher {
    /// The channel's id in its chain's spelling.
    pub fn channel_id(&self) -> &str {
        &self.presentation.channel().0
    }

    /// The cumulative amount the voucher signs.
    pub fn cumulative_amount(&self) -> u128 {
        self.voucher.cumulative_amount
    }
}

/// This connector's payouts to clients, signed as vouchers on its outbound
/// channels toward them. See the module doc.
pub struct ClientPayoutLedger {
    outbound: Arc<OutboundChannels>,
    /// One payout signed at a time: the next voucher's amount is read from
    /// the channel's watermark, and two payouts reading it at once would
    /// sign the same step twice.
    signing: tokio::sync::Mutex<()>,
    /// Which `(payee, job_id)` pairs [`Self::record_payout_once`] has
    /// already paid (issue #770's AC3). The channel's own watermark
    /// advances on every voucher, so it cannot tell a genuine second job
    /// from a retried first one; this can.
    ///
    /// `job_id` must be something the **payer** fixes, never something the
    /// party being paid supplies (issue #1269 / ADR 0069): every caller
    /// derives it from the PREPARE it is about to hand the payee, before the
    /// payee answers. In memory only: the durable, money-bearing fact is the
    /// channel's journaled watermark, never this set.
    credited_jobs: Mutex<HashSet<(VoucherSigner, [u8; 32])>>,
    /// The latest voucher signed on each payout channel that the client has
    /// not yet acknowledged, by channel id: what a resend carries.
    pending: Mutex<HashMap<String, PayoutVoucher>>,
}

impl ClientPayoutLedger {
    /// A ledger that signs on `outbound`'s channels.
    pub fn new(outbound: Arc<OutboundChannels>) -> ClientPayoutLedger {
        ClientPayoutLedger {
            outbound,
            signing: tokio::sync::Mutex::new(()),
            credited_jobs: Mutex::new(HashSet::new()),
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Pay `payee` `amount` more for the job `job_id`, as a voucher on one
    /// of this node's opened outbound channels toward it, and hold that
    /// voucher pending until the client acknowledges it. The caller has
    /// already subtracted any fee.
    ///
    /// A no-op -- nothing signed, `None` -- when this exact
    /// `(payee, job_id)` pair has already been paid (issue #770's AC3), so a
    /// retransmitted FULFILL of one job pays once. The check-and-mark is
    /// atomic under one lock. If nothing could be signed -- no channel is
    /// open toward `payee`, or none backs the new amount -- the mark is
    /// released, and the failure is logged by name: nothing was paid, so a
    /// later attempt for the same job must not find it "already done".
    pub async fn record_payout_once(
        &self,
        payee: VoucherSigner,
        job_id: &[u8; 32],
        amount: u64,
    ) -> Option<PayoutVoucher> {
        let key = (payee, *job_id);
        if !self
            .credited_jobs
            .lock()
            .expect("credited jobs lock poisoned")
            .insert(key)
        {
            return None;
        }
        let paid = self.sign_payout(payee, u128::from(amount)).await;
        if paid.is_none() {
            self.credited_jobs
                .lock()
                .expect("credited jobs lock poisoned")
                .remove(&key);
        }
        paid
    }

    /// Sign `amount` more to `payee` on the first opened channel toward it
    /// that backs the new cumulative amount.
    async fn sign_payout(&self, payee: VoucherSigner, amount: u128) -> Option<PayoutVoucher> {
        let _signing = self.signing.lock().await;
        let channels = self.outbound.opened_toward(&payee);
        if channels.is_empty() {
            tracing::warn!(
                payee = ?payee,
                amount,
                "not paying a client: this node has no open x402 channel toward it; an operator \
                 opens one with POST /channels on the client's published terms (ADR 0075 \
                 decision 7)"
            );
            return None;
        }
        for id in channels {
            // Read before signing: once a voucher is signed it is journaled,
            // and a payout that then failed would be paid again on retry.
            let (Some(signed), Some(presentation)) =
                (self.outbound.signed(&id), self.outbound.presentation(&id))
            else {
                continue;
            };
            let voucher = match self.outbound.sign_voucher(&id, signed + amount).await {
                // The journaled mark lagged the one the payer restored from
                // the chain: sign above that instead.
                Err(BatchChannelError::Settlement(BatchSettlementError::VoucherNotAdvancing {
                    signed,
                    ..
                })) => self.outbound.sign_voucher(&id, signed + amount).await,
                other => other,
            };
            match voucher {
                Ok(voucher) => {
                    let payout = PayoutVoucher {
                        payee,
                        presentation,
                        voucher,
                    };
                    self.pending
                        .lock()
                        .expect("pending payouts lock poisoned")
                        .insert(payout.channel_id().to_string(), payout.clone());
                    return Some(payout);
                }
                Err(error) => tracing::warn!(
                    channel = %id,
                    %error,
                    amount,
                    "a payout channel could not carry this payout; trying the next one toward \
                     the same client"
                ),
            }
        }
        tracing::warn!(
            payee = ?payee,
            amount,
            "not paying a client: no open x402 channel toward it backs this payout; an operator \
             tops one up with POST /channels/:id/fund"
        );
        None
    }

    /// Every voucher signed toward `payee` that it has not acknowledged,
    /// one per channel: what the next TRANSFER to that client carries.
    pub fn pending_for(&self, payee: &VoucherSigner) -> Vec<PayoutVoucher> {
        let mut pending: Vec<PayoutVoucher> = self
            .pending
            .lock()
            .expect("pending payouts lock poisoned")
            .values()
            .filter(|payout| payout.payee == *payee)
            .cloned()
            .collect();
        pending.sort_by(|a, b| a.channel_id().cmp(b.channel_id()));
        pending
    }

    /// Everything this node has signed toward `payee`, summed over its
    /// channels toward it: what it owes the client in all, delivered or
    /// not, as the journal records it.
    pub fn signed_toward(&self, payee: &VoucherSigner) -> u128 {
        self.outbound
            .opened_toward(payee)
            .iter()
            .filter_map(|id| self.outbound.signed(id))
            .sum()
    }

    /// The client acknowledged the voucher for `cumulative_amount` on
    /// `channel_id`. Clears it -- but only if it is still the one pending: a
    /// fresher payout may have superseded it while the acknowledgement was
    /// in flight, and that one must not be cleared.
    pub fn acknowledge(&self, channel_id: &str, cumulative_amount: u128) {
        let mut pending = self.pending.lock().expect("pending payouts lock poisoned");
        if pending
            .get(channel_id)
            .is_some_and(|payout| payout.cumulative_amount() == cumulative_amount)
        {
            pending.remove(channel_id);
        }
    }
}

/// A payout ledger over a fake EVM-shaped chain on which the connector
/// (`0x01`, funded) has opened a channel of 10,000 toward `payee`: the
/// payout channel an operator's `POST /channels` would have opened.
#[cfg(test)]
pub(crate) async fn test_ledger_paying(payee: [u8; 20]) -> Arc<ClientPayoutLedger> {
    use connector_runtime::{InMemoryJournal, SettlementChain};
    use connector_settlement::batch::{
        BatchSettlementPayer, EvmReceiverTerms, InMemoryBatchChain, InMemoryBatchSettlement,
        PayerExit, ReceiverTerms,
    };

    let chain = InMemoryBatchChain::new(PayerExit::Withdrawal);
    let payer = InMemoryBatchSettlement::on(chain, 0x01, 86_400);
    payer.fund(100_000);
    let outbound = OutboundChannels::restore(
        Arc::new(InMemoryJournal::new()),
        vec![(
            SettlementChain::Evm,
            Arc::new(payer) as Arc<dyn BatchSettlementPayer>,
        )],
    )
    .await
    .expect("an empty journal replays");
    outbound
        .open(
            ReceiverTerms::Evm(EvmReceiverTerms {
                receiver: payee,
                token: [0x70; 20],
                min_withdraw_delay_secs: 86_400,
            }),
            10_000,
        )
        .await
        .expect("the payout channel opens");
    Arc::new(ClientPayoutLedger::new(Arc::new(outbound)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_runtime::{InMemoryJournal, Journal, SettlementChain};
    use connector_settlement::batch::{
        BatchSettlementBackend, BatchSettlementPayer, EvmReceiverTerms, InMemoryBatchChain,
        InMemoryBatchSettlement, PayerExit, ReceiverTerms,
    };

    const ONE_DAY: u64 = 86_400;
    /// The fake's settled token.
    const TOKEN: u8 = 0x70;
    /// The client's payee key. Any key the fake does not reserve.
    const CLIENT: u8 = 0x02;

    fn payee() -> VoucherSigner {
        VoucherSigner::Evm([CLIENT; 20])
    }

    fn client_terms() -> ReceiverTerms {
        ReceiverTerms::Evm(EvmReceiverTerms {
            receiver: [CLIENT; 20],
            token: [TOKEN; 20],
            min_withdraw_delay_secs: ONE_DAY,
        })
    }

    /// A connector (`0x01`) funded with 10,000 on an EVM-shaped fake chain,
    /// its outbound channels over `journal`, and the client's receiving
    /// half on the same chain.
    async fn world(
        journal: Arc<dyn Journal>,
    ) -> (
        Arc<OutboundChannels>,
        Arc<InMemoryBatchChain>,
        InMemoryBatchSettlement,
    ) {
        let chain = InMemoryBatchChain::new(PayerExit::Withdrawal);
        let payer = InMemoryBatchSettlement::on(Arc::clone(&chain), 0x01, ONE_DAY);
        payer.fund(10_000);
        let outbound = OutboundChannels::restore(
            journal,
            vec![(
                SettlementChain::Evm,
                Arc::new(payer) as Arc<dyn BatchSettlementPayer>,
            )],
        )
        .await
        .expect("replays");
        let client = InMemoryBatchSettlement::on(Arc::clone(&chain), CLIENT, ONE_DAY);
        (Arc::new(outbound), chain, client)
    }

    #[tokio::test]
    async fn with_no_channel_toward_the_client_nothing_is_signed_and_the_job_is_not_spent() {
        let (outbound, _, _) = world(Arc::new(InMemoryJournal::new())).await;
        let ledger = ClientPayoutLedger::new(Arc::clone(&outbound));
        assert!(ledger
            .record_payout_once(payee(), &[7; 32], 500)
            .await
            .is_none());

        outbound.open(client_terms(), 1_000).await.expect("open");
        let paid = ledger
            .record_payout_once(payee(), &[7; 32], 500)
            .await
            .expect("the same job pays once a channel exists");
        assert_eq!(paid.cumulative_amount(), 500);
    }

    #[tokio::test]
    async fn payouts_are_cumulative_vouchers_the_client_can_land_itself() {
        let (outbound, _, client) = world(Arc::new(InMemoryJournal::new())).await;
        outbound.open(client_terms(), 1_000).await.expect("open");
        let ledger = ClientPayoutLedger::new(Arc::clone(&outbound));

        let first = ledger
            .record_payout_once(payee(), &[1; 32], 300)
            .await
            .expect("paid");
        let second = ledger
            .record_payout_once(payee(), &[2; 32], 200)
            .await
            .expect("paid");
        assert_eq!(first.cumulative_amount(), 300);
        assert_eq!(second.cumulative_amount(), 500);
        assert_eq!(first.channel_id(), second.channel_id());

        client
            .admit(second.presentation.clone())
            .await
            .expect("the channel pays the client");
        let landed = client
            .land(second.presentation.channel(), second.voucher.clone())
            .await
            .expect("the client lands its latest payout");
        assert_eq!(landed.landed, 500);
    }

    #[tokio::test]
    async fn a_retried_job_is_paid_once() {
        let (outbound, _, _) = world(Arc::new(InMemoryJournal::new())).await;
        outbound.open(client_terms(), 1_000).await.expect("open");
        let ledger = ClientPayoutLedger::new(outbound);

        ledger
            .record_payout_once(payee(), &[7; 32], 500)
            .await
            .expect("paid");
        assert!(ledger
            .record_payout_once(payee(), &[7; 32], 500)
            .await
            .is_none());
        let next = ledger
            .record_payout_once(payee(), &[8; 32], 100)
            .await
            .expect("another job pays");
        assert_eq!(
            next.cumulative_amount(),
            600,
            "exactly one credit for job 7"
        );
    }

    #[tokio::test]
    async fn a_payout_above_the_channels_collateral_is_not_signed() {
        let (outbound, _, _) = world(Arc::new(InMemoryJournal::new())).await;
        outbound.open(client_terms(), 1_000).await.expect("open");
        let ledger = ClientPayoutLedger::new(outbound);
        assert!(ledger
            .record_payout_once(payee(), &[7; 32], 1_001)
            .await
            .is_none());
        assert!(ledger.pending_for(&payee()).is_empty());
    }

    #[tokio::test]
    async fn a_payout_to_another_client_is_not_signed_on_this_ones_channel() {
        let (outbound, _, _) = world(Arc::new(InMemoryJournal::new())).await;
        outbound.open(client_terms(), 1_000).await.expect("open");
        let ledger = ClientPayoutLedger::new(outbound);
        assert!(ledger
            .record_payout_once(VoucherSigner::Evm([0x03; 20]), &[7; 32], 100)
            .await
            .is_none());
        assert!(ledger
            .record_payout_once(VoucherSigner::Solana([CLIENT; 32]), &[7; 32], 100)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn the_latest_voucher_is_pending_until_the_client_acknowledges_it() {
        let (outbound, _, _) = world(Arc::new(InMemoryJournal::new())).await;
        outbound.open(client_terms(), 1_000).await.expect("open");
        let ledger = ClientPayoutLedger::new(outbound);
        let first = ledger
            .record_payout_once(payee(), &[1; 32], 100)
            .await
            .expect("paid");
        let second = ledger
            .record_payout_once(payee(), &[2; 32], 100)
            .await
            .expect("paid");
        assert_eq!(ledger.pending_for(&payee()), vec![second.clone()]);

        ledger.acknowledge(first.channel_id(), first.cumulative_amount());
        assert_eq!(
            ledger.pending_for(&payee()),
            vec![second.clone()],
            "a stale acknowledgement leaves the fresher voucher pending"
        );
        ledger.acknowledge(second.channel_id(), second.cumulative_amount());
        assert!(ledger.pending_for(&payee()).is_empty());
    }

    /// The acceptance criterion: the payout watermark survives a restart,
    /// because the voucher is journaled on the outbound channel before it
    /// is handed out.
    #[tokio::test]
    async fn the_payout_watermark_survives_a_restart() {
        let journal: Arc<dyn Journal> = Arc::new(InMemoryJournal::new());
        let chain = InMemoryBatchChain::new(PayerExit::Withdrawal);
        let payer = || -> Arc<dyn BatchSettlementPayer> {
            Arc::new(InMemoryBatchSettlement::on(
                Arc::clone(&chain),
                0x01,
                ONE_DAY,
            ))
        };
        InMemoryBatchSettlement::on(Arc::clone(&chain), 0x01, ONE_DAY).fund(10_000);
        {
            let outbound = Arc::new(
                OutboundChannels::restore(
                    Arc::clone(&journal),
                    vec![(SettlementChain::Evm, payer())],
                )
                .await
                .expect("replays"),
            );
            outbound.open(client_terms(), 1_000).await.expect("open");
            ClientPayoutLedger::new(outbound)
                .record_payout_once(payee(), &[1; 32], 300)
                .await
                .expect("paid");
        }

        let restarted = Arc::new(
            OutboundChannels::restore(journal, vec![(SettlementChain::Evm, payer())])
                .await
                .expect("replays"),
        );
        let paid = ClientPayoutLedger::new(restarted)
            .record_payout_once(payee(), &[2; 32], 100)
            .await
            .expect("paid after the restart");
        assert_eq!(
            paid.cumulative_amount(),
            400,
            "the voucher after a restart carries on from the journaled watermark"
        );
    }
}
