use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use connector_domain::{AssetId, Rate};

use crate::port::{LegObservation, PoolId, QuoteLeg, RateSource, RateSourceError};

/// What one named pool holds, as [`InMemoryRateSource`] stands in for it.
///
/// Shaped after what a real pool actually constrains a reader by, so the
/// fake refuses the same things a chain would rather than being agreeable:
/// one pair, read in either direction, averaged over any window between the
/// two its observations can span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolContents {
    pub base: AssetId,
    pub quote: AssetId,
    /// How many base units of [`quote`](Self::quote) one base unit of
    /// [`base`](Self::base) buys. Reported for any window this pool can
    /// serve; the reverse direction reports its inverse.
    pub rate: Rate,
    /// The instant this pool dates its newest observation at -- its own
    /// clock, which is what [`LegObservation::observed_at`] carries and
    /// what a `ttl` guard is read against.
    pub observed_at: DateTime<Utc>,
    /// The shortest window this pool's observations can span: below it
    /// there is nothing to average, and a source that answered anyway would
    /// be interpolating. A real pool's floor is its observation cadence
    /// (Raydium CLMM writes one at most every 15 seconds).
    pub shortest_window: Duration,
    /// The longest window its history reaches back over: above it the
    /// observations simply are not there. A real pool's ceiling is its
    /// oracle array (a Uniswap v3 pool whose cardinality has not been grown
    /// reverts `OLD`; Raydium's ring is a fixed 100 slots, roughly 25
    /// minutes).
    pub longest_window: Duration,
}

/// The in-memory [`RateSource`]: pools live only in this process and
/// nothing is ever read off a chain. This is the fake this workspace's own
/// tests use -- the poller (issue #1294) is demoable against it -- and the
/// first implementation to pass the contract suite in [`crate::contract`]
/// (ADR 0007), proving the port's shape is satisfiable before any reader
/// exists.
///
/// A fake, not a stub (ADR 0007): it answers from pools it was told about,
/// by the same rules a chain would enforce, and asserts nothing about how
/// it is called.
pub struct InMemoryRateSource {
    pools: Mutex<HashMap<PoolId, PoolContents>>,
    reachable: AtomicBool,
}

impl Default for InMemoryRateSource {
    fn default() -> Self {
        InMemoryRateSource::new()
    }
}

impl InMemoryRateSource {
    pub fn new() -> Self {
        InMemoryRateSource {
            pools: Mutex::new(HashMap::new()),
            reachable: AtomicBool::new(true),
        }
    }

    /// Record what a named pool holds, as an operator naming a real pool
    /// would point this source at one. Replaces whatever that name held
    /// before, which is how a refreshed observation arrives.
    pub fn insert_pool(&self, pool: PoolId, contents: PoolContents) {
        self.pools().insert(pool, contents);
    }

    /// Take this source's data out of reach, as an RPC outage does: every
    /// read afterwards is [`RateSourceError::Unreachable`], including reads
    /// of pools this source answered a moment ago. The last observation is
    /// deliberately *not* served in its place -- that is the behaviour ADR
    /// 0071 calls being drained politely, and the contract suite's third
    /// promise exists to catch it.
    pub fn cut_off(&self) {
        self.reachable.store(false, Ordering::SeqCst);
    }

    /// End that outage: reads answer again, from the pools this source
    /// still holds.
    pub fn reconnect(&self) {
        self.reachable.store(true, Ordering::SeqCst);
    }

    fn pools(&self) -> MutexGuard<'_, HashMap<PoolId, PoolContents>> {
        self.pools.lock().expect("InMemoryRateSource lock poisoned")
    }
}

#[async_trait]
impl RateSource for InMemoryRateSource {
    async fn observe(&self, leg: &QuoteLeg) -> Result<LegObservation, RateSourceError> {
        if !self.reachable.load(Ordering::SeqCst) {
            return Err(RateSourceError::Unreachable(
                "the in-memory rate source has been cut off".to_string(),
            ));
        }

        let pools = self.pools();
        let contents = pools
            .get(&leg.pool)
            .ok_or_else(|| RateSourceError::PoolNotFound(leg.pool.clone()))?;

        // A pool holds its pair in both directions; which one was asked for
        // is the leg's own base and quote, and the other direction is the
        // inverse rather than the same number handed back.
        let rate = if leg.base == contents.base && leg.quote == contents.quote {
            contents.rate
        } else if leg.base == contents.quote && leg.quote == contents.base {
            contents.rate.inverted()
        } else {
            return Err(RateSourceError::PairNotInPool {
                pool: leg.pool.clone(),
                base: leg.base.clone(),
                quote: leg.quote.clone(),
            });
        };

        if leg.window < contents.shortest_window || leg.window > contents.longest_window {
            return Err(RateSourceError::WindowNotServed {
                pool: leg.pool.clone(),
                window: leg.window,
            });
        }

        Ok(LegObservation {
            leg: leg.clone(),
            rate,
            observed_at: contents.observed_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::port::QuotePath;

    fn usdc() -> AssetId {
        AssetId::evm("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913")
    }

    fn weth() -> AssetId {
        AssetId::evm("0x4200000000000000000000000000000000000006")
    }

    fn pool() -> PoolId {
        PoolId("weth-usdc".to_string())
    }

    fn weth_usdc() -> PoolContents {
        PoolContents {
            base: weth(),
            quote: usdc(),
            // One WETH (18 decimals) buys 3,000 USDC (6 decimals): the
            // decimals difference is already in the ratio.
            rate: Rate::new(3_000_000_000, 1_000_000_000_000_000_000).expect("a rate"),
            observed_at: DateTime::from_timestamp(1_757_500_000, 0).expect("a timestamp"),
            shortest_window: Duration::seconds(60),
            longest_window: Duration::seconds(1_800),
        }
    }

    fn leg(window: Duration) -> QuoteLeg {
        QuoteLeg {
            pool: pool(),
            base: weth(),
            quote: usdc(),
            window,
        }
    }

    fn source() -> InMemoryRateSource {
        let source = InMemoryRateSource::new();
        source.insert_pool(pool(), weth_usdc());
        source
    }

    #[tokio::test]
    async fn a_pool_reads_in_both_directions() {
        let source = source();
        let there = source.observe(&leg(Duration::seconds(300))).await.unwrap();
        let back = source
            .observe(&leg(Duration::seconds(300)).reversed())
            .await
            .unwrap();
        assert_eq!(there.rate, weth_usdc().rate);
        assert_eq!(back.rate, weth_usdc().rate.inverted());
        assert_eq!(back.leg, leg(Duration::seconds(300)).reversed());
    }

    #[tokio::test]
    async fn a_window_outside_what_the_pool_spans_is_refused_at_both_ends() {
        let source = source();
        for window in [Duration::seconds(30), Duration::seconds(3_600)] {
            let err = source.observe(&leg(window)).await.unwrap_err();
            assert_eq!(
                err,
                RateSourceError::WindowNotServed {
                    pool: pool(),
                    window,
                }
            );
        }
    }

    #[tokio::test]
    async fn a_pair_the_pool_does_not_hold_is_refused_rather_than_answered() {
        let source = source();
        let dai = AssetId::evm("0x50c5725949a6f0c72e6c4a641f24049a917db0cb");
        let err = source
            .observe(&QuoteLeg {
                base: dai.clone(),
                ..leg(Duration::seconds(300))
            })
            .await
            .unwrap_err();
        assert_eq!(
            err,
            RateSourceError::PairNotInPool {
                pool: pool(),
                base: dai,
                quote: usdc(),
            }
        );
    }

    #[tokio::test]
    async fn an_outage_is_reported_rather_than_answered_from_memory() {
        let source = source();
        let path = QuotePath::direct(leg(Duration::seconds(300)));
        source.quote(&path).await.expect("answered while reachable");

        source.cut_off();
        let err = source.quote(&path).await.unwrap_err();
        assert!(matches!(err, RateSourceError::Unreachable(_)), "{err}");

        source.reconnect();
        source
            .quote(&path)
            .await
            .expect("answered once reconnected");
    }
}
