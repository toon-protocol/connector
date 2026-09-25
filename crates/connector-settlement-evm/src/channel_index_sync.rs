//! Drives an [`EvmChannelIndex`] from a real `TokenNetwork`'s own logs
//! (issue #661): backfill in bounded `eth_getLogs` ranges up to
//! `chain_head - confirmations`, then poll for more as the chain advances.
//! The state machine [`EvmChannelIndex`] applies is chain-agnostic and
//! tested without a chain at all -- this module is the one place that
//! actually queries and decodes logs, kept separate for exactly that
//! reason.
//!
//! No `eth_subscribe`: this workspace standardizes on HTTP JSON-RPC over the
//! settlement table's one `RpcTransport` (ADR 0073), and this syncer follows
//! that rather than introducing the only WS-transport consumer in the
//! codebase for one feature. It takes the **same** transport the backend
//! does, so it shares the backend's bounds, its refusal retries and, when
//! the table is proxied, its circuit: a syncer left direct would be the
//! whole leak ADR 0073 decision 2 exists to close.

use std::sync::Arc;
use std::time::Duration;

use connector_chain_rpc::evm::EvmRpc;
use connector_chain_rpc::RpcTransport;
use ethers::contract::{ContractError, EthEvent};
use ethers::providers::{Middleware, Provider, ProviderError};
use ethers::types::{Address, ValueOrArray};

use crate::bindings::token_network::{
    ChannelNewDepositFilter, ChannelOpenedFilter, ChannelSettledFilter,
    TokenNetwork as TokenNetworkContract,
};
use crate::channel_index::{
    ChannelIndexEvent, EvmChannelIndex, EvmChannelIndexError, OrderedChannelIndexEvent,
};

/// Widest single `eth_getLogs` range this syncer asks for in one request
/// (issue #661 decision point 3: "backfill in bounded block ranges" --
/// `eth_getLogs` has provider-imposed range caps). Comfortably inside a
/// public provider's typical cap (e.g. Alchemy's free-tier 2,000-block
/// limit) so this never needs a per-deployment tuning knob.
const MAX_BLOCK_RANGE: u64 = 2_000;

/// How long a caught-up syncer waits before checking chain head again, and
/// the FIRST interval a failed attempt waits before retrying (it backs off
/// from there -- see [`EvmChannelIndexSyncer::run`]).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Ceiling on the retry backoff of a syncer that keeps failing. A minute is
/// short enough that a syncer notices an endpoint coming back well inside
/// the window a channel lookup would otherwise spend on direct chain reads,
/// and long enough that a permanently misconfigured node is neither
/// hammering that endpoint nor filling its own log.
const MAX_RETRY_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum ChannelIndexSyncError {
    #[error("channel index sync could not read the chain: {0}")]
    Provider(#[from] ProviderError),
    #[error("channel index sync could not read a TokenNetwork log: {0}")]
    Decode(String),
    #[error(transparent)]
    Index(#[from] EvmChannelIndexError),
}

/// Reads `TokenNetwork`'s own `ChannelOpened`/`ChannelNewDeposit`/
/// `ChannelSettled` logs (the close events are deliberately not indexed --
/// see [`crate::channel_index`]'s module doc) and folds them into an
/// [`EvmChannelIndex`]. Holds no signing key and sends no transaction -- a
/// plain provider over the table's transport, never
/// [`crate::EvmSettlementBackend`]'s sender, since this syncer only ever
/// reads.
pub struct EvmChannelIndexSyncer {
    contract: TokenNetworkContract<Provider<EvmRpc>>,
    confirmations: u64,
    from_block: u64,
}

impl EvmChannelIndexSyncer {
    /// `confirmations` must be at least 1 -- `connector-config`'s
    /// `EvmSettlementConfig::channel_index_confirmations` already refuses a
    /// depth of `0` at config load time, so the `max(1)` below is a second,
    /// defensive check rather than the primary one.
    pub fn new(
        transport: &RpcTransport,
        contract_address: Address,
        confirmations: u64,
        from_block: u64,
    ) -> Self {
        let provider = EvmRpc::provider(transport.clone());
        let contract = TokenNetworkContract::new(contract_address, Arc::new(provider));
        EvmChannelIndexSyncer {
            contract,
            confirmations: confirmations.max(1),
            from_block,
        }
    }

    /// One backfill/poll step: apply everything between this index's
    /// checkpoint and `chain_head - confirmations`, in at most
    /// [`MAX_BLOCK_RANGE`] blocks. Returns how many blocks were newly
    /// applied -- `0` means either there is nothing new deep enough yet, or
    /// the provider's own head has not advanced past the confirmation
    /// window. Never blocks past one bounded range, so a syncer far behind
    /// catches up in several short calls rather than one long one holding a
    /// lookup racing in on `index` for an unbounded time.
    pub async fn sync_once(&self, index: &EvmChannelIndex) -> Result<u64, ChannelIndexSyncError> {
        let head = self.contract.client().get_block_number().await?.as_u64();
        let confirmed_head = head.saturating_sub(self.confirmations);
        let start = match index.last_indexed_block() {
            Some(checkpoint) => checkpoint + 1,
            None => self.from_block,
        };
        if start > confirmed_head {
            return Ok(0);
        }
        let end = start
            .saturating_add(MAX_BLOCK_RANGE - 1)
            .min(confirmed_head);
        let events = self.query_range(start, end).await?;
        index.apply(events, end)?;
        Ok(end - start + 1)
    }

    /// Every log this index folds in, over `from..=to`. One `eth_getLogs`
    /// per event type -- the three topics have nothing in common to filter
    /// on in a single query -- gathered unordered, since
    /// [`EvmChannelIndex::apply`] sorts the whole batch into chain order
    /// before applying any of it.
    async fn query_range(
        &self,
        from: u64,
        to: u64,
    ) -> Result<Vec<OrderedChannelIndexEvent>, ChannelIndexSyncError> {
        let mut events = Vec::new();
        self.collect_logs(from, to, &mut events, |log: ChannelOpenedFilter| {
            ChannelIndexEvent::Opened {
                channel_id: log.channel_id,
                participant1: log.participant_1,
                participant2: log.participant_2,
            }
        })
        .await?;
        self.collect_logs(from, to, &mut events, |log: ChannelNewDepositFilter| {
            ChannelIndexEvent::NewDeposit {
                channel_id: log.channel_id,
                participant: log.participant,
                total_deposit: log.total_deposit,
            }
        })
        .await?;
        self.collect_logs(from, to, &mut events, |log: ChannelSettledFilter| {
            ChannelIndexEvent::Settled {
                channel_id: log.channel_id,
            }
        })
        .await?;
        Ok(events)
    }

    /// One event type's logs over `from..=to`, decoded by `into` and tagged
    /// with the `(block_number, log_index)` position the chain gave them.
    ///
    /// The `address` filter is set EXPLICITLY, and that is the whole point
    /// of it being written out here. `Contract::event::<D>()` builds
    /// `D::new(Filter::new(), client)` (ethers-contract 2.0.14,
    /// `src/contract.rs:314`) -- a bare filter carrying only the event's
    /// topic0 and whatever block range is chained onto it. Its two
    /// siblings, `event_with_filter` and `event_for_name`, both
    /// `.address(self.address)` on the way through; `event` is the one that
    /// does not, so the `eth_getLogs` this method sent named no contract at
    /// all and asked for every `ChannelOpened`-shaped log on the chain.
    ///
    /// An unrestricted `eth_getLogs` is a request many public RPC providers
    /// refuse outright rather than serve. The devnet relay and store boxes
    /// both point `[settlement.evm].rpc_url` at
    /// `https://base-sepolia-rpc.publicnode.com`, which answers
    /// `-32701 Please specify an address in your request` -- so
    /// [`Self::sync_once`] failed on its very first range, the index never
    /// took a checkpoint, and every channel lookup on both boxes fell back
    /// to a direct chain read for the life of the process (verified
    /// 2026-08-14 against `connector:rust-sha-415531a`: the retry warning
    /// below was ~99.99% of 100,000 lines of connector output). Scoping the
    /// query to the `TokenNetwork` this syncer was built for is also simply
    /// correct -- it is the only contract whose logs this index folds in --
    /// and it makes the query cheaper on providers that would have served
    /// the wide one.
    async fn collect_logs<E, F>(
        &self,
        from: u64,
        to: u64,
        events: &mut Vec<OrderedChannelIndexEvent>,
        into: F,
    ) -> Result<(), ChannelIndexSyncError>
    where
        E: EthEvent,
        F: Fn(E) -> ChannelIndexEvent,
    {
        let logs = self
            .contract
            .event::<E>()
            .address(ValueOrArray::Value(self.contract.address()))
            .from_block(from)
            .to_block(to)
            .query_with_meta()
            .await
            .map_err(decode_error)?;
        events.extend(
            logs.into_iter()
                .map(|(log, meta)| OrderedChannelIndexEvent {
                    block_number: meta.block_number.as_u64(),
                    log_index: meta.log_index.as_u64(),
                    event: into(log),
                }),
        );
        Ok(())
    }

    /// Backfill-then-poll forever: never returns except by being dropped.
    /// Meant to be `tokio::spawn`'d, never `.await`'d on the startup path --
    /// issue #661's own acceptance criterion is that a cold-start backfill
    /// must not block the node from serving traffic.
    ///
    /// A failed attempt (an unreachable RPC endpoint, say) is retried; it
    /// never panics and never stops the loop, since a channel this index
    /// cannot currently reach still has the existing direct chain-read
    /// fallback to lean on.
    ///
    /// # Why the retry does not simply log every attempt
    ///
    /// It used to, at `warn`, every `poll_interval`. That is fine for a
    /// blip and ruinous for a failure that does not clear: the devnet boxes
    /// spent months emitting the same warning every 5 seconds -- 99.99% of
    /// 100,000 lines of connector output on the relay box -- which is not
    /// "noisy", it is an operator unable to see anything else the process
    /// says. A permanently failing subsystem must stay diagnosable without
    /// becoming the log.
    ///
    /// So the volume follows the INFORMATION, not the attempt count:
    ///
    ///   * the first failure of a run is `warn` -- an operator watching at
    ///     the default `info` level sees it, exactly as before;
    ///   * a repeat of the SAME failure is `debug` -- nothing new was
    ///     learned, and `RUST_LOG=...=debug` still gets every attempt for
    ///     anyone actually debugging one;
    ///   * a DIFFERENT failure is `warn` again. The error text changing is
    ///     a state change ("...specify an address" becoming a timeout is
    ///     the difference between a misconfigured query and a dead
    ///     endpoint), and those are the lines worth waking up for;
    ///   * recovery is `warn` too, not `info`. It is the line that clears
    ///     the warning above it, so it has to be visible wherever that one
    ///     was -- an operator filtering to `warn` must never see a
    ///     permanent-looking failure and miss that it resolved.
    ///
    /// The retry interval backs off alongside it, `poll_interval` doubling
    /// up to [`MAX_RETRY_INTERVAL`], so a wedged syncer also stops hammering
    /// an endpoint that is refusing it. Backoff resets the moment a sync
    /// succeeds, so a caught-up syncer still polls at `poll_interval`.
    pub async fn run(self, index: Arc<EvmChannelIndex>, poll_interval: Duration) {
        let mut retry = RetrySchedule::new(poll_interval);
        loop {
            match self.sync_once(&index).await {
                Ok(progressed) => {
                    if retry.recovered() {
                        tracing::warn!(
                            "channel index sync recovered and is advancing again; affected \
                             channels are served from the index once it catches up"
                        );
                    }
                    // Progress was made and there may be more behind it --
                    // loop again immediately rather than waiting out a poll
                    // interval per bounded range while catching up.
                    if progressed == 0 {
                        tokio::time::sleep(poll_interval).await;
                    }
                }
                Err(error) => {
                    let failure = error.to_string();
                    let (report, wait) = retry.failed(&failure);
                    match report {
                        FailureReport::Loudly => tracing::warn!(
                            error = %failure,
                            "channel index sync failed to advance; affected channels fall back to \
                             a direct chain read until this recovers. Repeats of this same failure \
                             are logged at debug -- raise the level for this target to see every \
                             attempt"
                        ),
                        FailureReport::Quietly => tracing::debug!(
                            error = %failure,
                            "channel index sync still failing to advance"
                        ),
                    }
                    tokio::time::sleep(wait).await;
                }
            }
        }
    }
}

/// Whether a failed attempt says something new.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureReport {
    /// The first failure of a run, or one whose text differs from the last
    /// reported -- `warn`.
    Loudly,
    /// A repeat of the failure already reported -- `debug`.
    Quietly,
}

/// [`EvmChannelIndexSyncer::run`]'s retry rule, held apart from the loop
/// that applies it so it can be exercised without a chain, a clock or a log
/// subscriber -- see that method's doc comment for the reasoning the rule
/// encodes.
#[derive(Debug)]
struct RetrySchedule {
    poll_interval: Duration,
    /// `None` while the syncer is healthy; the text of the last failure
    /// REPORTED at `warn` while it is not -- so a repeat can be told from a
    /// change without keeping a count of either.
    reported_failure: Option<String>,
    backoff: Duration,
}

impl RetrySchedule {
    fn new(poll_interval: Duration) -> RetrySchedule {
        RetrySchedule {
            poll_interval,
            reported_failure: None,
            backoff: poll_interval,
        }
    }

    /// Record a failed attempt: how it should be logged, and how long to
    /// wait before the next one.
    fn failed(&mut self, failure: &str) -> (FailureReport, Duration) {
        if self.reported_failure.as_deref() == Some(failure) {
            let wait = self.backoff;
            self.backoff = (self.backoff * 2).min(MAX_RETRY_INTERVAL);
            return (FailureReport::Quietly, wait);
        }
        self.reported_failure = Some(failure.to_string());
        // A first or changed failure is a fresh problem: it retries at the
        // poll interval rather than inheriting the pace whatever came
        // before it had already backed off to.
        self.backoff = (self.poll_interval * 2).min(MAX_RETRY_INTERVAL);
        (FailureReport::Loudly, self.poll_interval)
    }

    /// Record a successful attempt. `true` exactly once per run of
    /// failures -- on the success that ends one -- so the recovery line is
    /// logged as often as the failure line it clears, and no more.
    ///
    /// The backoff needs no reset here: clearing `reported_failure` makes
    /// the next failure a fresh one, and [`Self::failed`] restarts the
    /// interval for those.
    fn recovered(&mut self) -> bool {
        self.reported_failure.take().is_some()
    }
}

fn decode_error<M: Middleware>(error: ContractError<M>) -> ChannelIndexSyncError {
    ChannelIndexSyncError::Decode(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLL: Duration = Duration::from_secs(5);

    /// The devnet boxes' shape: one failure that never clears. The operator
    /// gets one `warn`, and the retry stops hammering the endpoint that is
    /// refusing it.
    #[test]
    fn a_failure_that_never_clears_is_reported_once_and_backs_off_to_the_ceiling() {
        let mut retry = RetrySchedule::new(POLL);
        let refusal = "channel index sync could not read a TokenNetwork log: (code: -32701, \
                       message: Please specify an address in your request)";

        assert_eq!(retry.failed(refusal), (FailureReport::Loudly, POLL));
        assert_eq!(
            retry.failed(refusal),
            (FailureReport::Quietly, Duration::from_secs(10))
        );
        assert_eq!(
            retry.failed(refusal),
            (FailureReport::Quietly, Duration::from_secs(20))
        );

        for _ in 0..20 {
            let (report, wait) = retry.failed(refusal);
            assert_eq!(report, FailureReport::Quietly);
            assert!(
                wait <= MAX_RETRY_INTERVAL,
                "the backoff must never grow past its ceiling: {wait:?}"
            );
        }
        assert_eq!(retry.failed(refusal).1, MAX_RETRY_INTERVAL);
    }

    /// A state change stays visible: the text changing is the difference
    /// between a misconfigured query and a dead endpoint, and an operator
    /// filtered to `warn` has to see it.
    #[test]
    fn a_different_failure_is_reported_again_and_restarts_the_backoff() {
        let mut retry = RetrySchedule::new(POLL);
        assert_eq!(retry.failed("specify an address").0, FailureReport::Loudly);
        assert_eq!(retry.failed("specify an address").0, FailureReport::Quietly);
        assert_eq!(retry.failed("specify an address").0, FailureReport::Quietly);

        assert_eq!(
            retry.failed("connection refused"),
            (FailureReport::Loudly, POLL),
            "a fresh problem retries at the poll interval rather than inheriting the pace \
             the previous one had backed off to"
        );
        assert_eq!(
            retry.failed("connection refused"),
            (FailureReport::Quietly, Duration::from_secs(10))
        );
    }

    /// Recovery is reported exactly once per run of failures, and never
    /// when nothing was failing -- a healthy syncer says nothing at all.
    #[test]
    fn recovery_is_reported_once_and_only_after_a_failure() {
        let mut retry = RetrySchedule::new(POLL);
        assert!(!retry.recovered(), "a healthy syncer reports no recovery");

        retry.failed("specify an address");
        assert!(retry.recovered());
        assert!(!retry.recovered(), "and does not repeat it");

        // The failure that recurs after a recovery is a fresh one again:
        // reported at `warn`, and retried at the poll interval rather than
        // resuming where the previous run of failures left off.
        assert_eq!(
            retry.failed("specify an address"),
            (FailureReport::Loudly, POLL)
        );
    }
}
