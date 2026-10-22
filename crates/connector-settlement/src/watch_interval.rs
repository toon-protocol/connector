//! How often a batch-settlement watcher looks at the chain, derived from the
//! delay its node publishes rather than fixed (no I/O, no clock).
//!
//! A watcher exists to act inside a delay the counterparty cannot shorten
//! below the node's published minimum: Solana's grace period, EVM's
//! withdraw delay (ADR 0074 decision 5). Watching ninety times inside the
//! shortest delay any admitted channel can carry is the safety case; any
//! faster only spends the operator's RPC tier.

use std::time::Duration;

/// Passes the watch keeps inside the shortest delay an admitted channel can
/// carry.
pub const WATCH_PASSES_PER_MINIMUM_DELAY: u64 = 90;

/// The fastest a watch runs: x402's 900 second minimum delay over
/// [`WATCH_PASSES_PER_MINIMUM_DELAY`].
pub const MIN_WATCH_INTERVAL: Duration = Duration::from_secs(10);

/// The slowest a watch runs. It bounds how long a Sealed channel waits to be
/// distributed, a Distributed one to be reclaimed, or a withdrawing EVM
/// channel to be claimed.
pub const MAX_WATCH_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// The watch interval for a node whose shortest admitted delay is
/// `min_delay_secs`: that delay divided by
/// [`WATCH_PASSES_PER_MINIMUM_DELAY`], clamped to
/// [`MIN_WATCH_INTERVAL`]..=[`MAX_WATCH_INTERVAL`].
pub fn watch_interval(min_delay_secs: u64) -> Duration {
    Duration::from_secs(min_delay_secs / WATCH_PASSES_PER_MINIMUM_DELAY)
        .clamp(MIN_WATCH_INTERVAL, MAX_WATCH_INTERVAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const ONE_DAY: u64 = 86_400;
    /// The shortest delay config admits on either chain.
    const X402_MINIMUM_DELAY: u64 = 900;

    #[test]
    fn the_floor_delay_watches_every_ten_seconds() {
        assert_eq!(watch_interval(900), Duration::from_secs(10));
    }

    #[test]
    fn the_default_one_day_delay_watches_every_fifteen_minutes() {
        assert_eq!(watch_interval(ONE_DAY), MAX_WATCH_INTERVAL);
    }

    #[test]
    fn the_floor_is_the_x402_minimum_delay_over_the_passes() {
        assert_eq!(
            MIN_WATCH_INTERVAL.as_secs() * WATCH_PASSES_PER_MINIMUM_DELAY,
            X402_MINIMUM_DELAY
        );
    }

    // Uniform over all of `u64` almost every draw lands far above the
    // ceiling's 81 000 seconds, so each property also samples the region
    // where the division, not a clamp, decides the interval.
    fn delay() -> impl Strategy<Value = u64> {
        prop_oneof![0u64..=200_000, any::<u64>()]
    }

    fn admitted_delay() -> impl Strategy<Value = u64> {
        prop_oneof![X402_MINIMUM_DELAY..=200_000, X402_MINIMUM_DELAY..=u64::MAX]
    }

    proptest! {
        #[test]
        fn always_within_the_bounds(min in delay()) {
            let interval = watch_interval(min);
            prop_assert!(interval >= MIN_WATCH_INTERVAL && interval <= MAX_WATCH_INTERVAL);
        }

        #[test]
        fn ninety_passes_fit_unless_clamped_at_the_ceiling(min in admitted_delay()) {
            let interval = watch_interval(min);
            if interval < MAX_WATCH_INTERVAL {
                prop_assert!(min / interval.as_secs() >= WATCH_PASSES_PER_MINIMUM_DELAY);
            }
        }
    }
}
