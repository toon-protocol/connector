//! One contract suite (ADR 0007): the definition of the [`RateSource`]
//! port, written so any implementation -- the in-memory one here, the
//! Uniswap-v3-compatible `observe()` TWAP reader in its own crate (issue
//! #1293), a Raydium CLMM one after it -- can be run against it, and is not
//! an implementation of the port until it passes unmodified.
//!
//! Gated behind the `test-util` feature (rather than `#[cfg(test)]` alone)
//! for the reason `connector-settlement`'s own suite already states: this
//! port's real implementations live in separate crates, and a suite hidden
//! behind `#[cfg(test)]` is invisible outside this crate's own test build,
//! so those crates could never hold their implementation to it. They add
//! this crate under `[dev-dependencies]` with `features = ["test-util"]`
//! and call [`assert_upholds_the_contract`] from their own tests instead.
//!
//! Four promises, one named case each:
//!
//! 1. [`a_quote_path_of_one_or_two_legs_composes`];
//! 2. [`a_window_the_source_cannot_serve_is_an_error`];
//! 3. [`no_observation_is_substituted_for_the_one_asked_for`];
//! 4. [`an_unreachable_source_is_an_error_not_a_stale_value`].
//!
//! They run in that order, and the fourth runs last for a reason: it takes
//! the source's data out of reach and nothing is asked of it afterwards.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::Duration;
use connector_domain::{AssetId, Rate};

use crate::port::{compose_rates, PoolId, QuoteLeg, QuotePath, RateSource, RateSourceError};

/// Everything a [`RateSource`] implementation hands the suite about itself,
/// beyond the source value, so the suite can exercise it without hardcoding
/// assumptions no real venue actually holds -- which pool is real, which
/// pair it holds, which windows its observations span.
pub struct ContractFixture {
    pub source: Arc<dyn RateSource>,
    /// A leg this source can answer: a pool it has, a pair that pool holds,
    /// a window it can serve.
    ///
    /// Its rate must **not** be 1:1. Every real pair's is something else --
    /// a decimals difference alone moves it (ADR 0071 decision 4) -- and a
    /// 1:1 rate would make the direction assertion in
    /// [`no_observation_is_substituted_for_the_one_asked_for`] vacuous,
    /// since a pool read the other way round would legitimately report the
    /// same number.
    pub leg: QuoteLeg,
    /// A second leg this source can answer, based in what
    /// [`leg`](Self::leg) quotes: the two are the ANYONE/WETH-then-WETH/USDC
    /// shape ADR 0071 decision 3 calls a two-pool quote, and the suite
    /// composes them into one path.
    ///
    /// The two rates must compose exactly -- their product must reduce into
    /// a `u64/u64` [`Rate`], which every realistic pair's does (see
    /// [`compose_rates`]) -- because the suite asserts the composed rate
    /// rather than a rounded neighbourhood of it.
    pub next_leg: QuoteLeg,
    /// A pool identifier this source can parse but does not have: a real
    /// address with no pool at it, rather than a plain ASCII label a
    /// chain-backed reader would refuse before ever looking.
    pub absent_pool: PoolId,
    /// A token identity this source can parse which [`leg`](Self::leg)'s
    /// pool does not hold -- any third token will do.
    pub asset_the_pool_does_not_hold: AssetId,
    /// A window shorter than this source can serve, for
    /// [`leg`](Self::leg)'s pool: below its observation cadence, below its
    /// block time, or simply zero. Only the implementation knows where its
    /// own floor is.
    pub window_shorter_than_the_source_can_serve: Duration,
    /// Take this source's data out of reach, the way an RPC outage does --
    /// a flag for the in-memory fake, dropping the forked chain for a
    /// reader that talks to one.
    ///
    /// Called **once, at the end of the suite**, and nothing is asked of
    /// the source afterwards that must succeed. It is an outage midway
    /// through a source's life rather than a source that was never
    /// reachable, because that is the only shape that proves the third
    /// promise: a source with a perfectly good last observation to hand
    /// back must report the outage instead of handing it back.
    pub make_unreachable: MakeUnreachableFn,
}

/// A boxed, `'static`, `Send` future -- the shape
/// [`ContractFixture::make_unreachable`] returns, since a plain `async fn`
/// cannot be named as a trait object field type.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The shape of [`ContractFixture::make_unreachable`], named so clippy's
/// `type_complexity` lint (and any reader) sees one name rather than the
/// spelled-out trait object at every use site.
pub type MakeUnreachableFn = Box<dyn Fn() -> BoxFuture<'static, ()> + Send>;

/// Run every assertion the [`RateSource`] port makes, against a freshly
/// built implementation from `build`. A conforming implementation passes
/// this function without modification -- that unmodified pass is what
/// "upholds the contract" means (ADR 0007).
pub async fn assert_upholds_the_contract<F, Fut>(build: F)
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ContractFixture>,
{
    let fixture = build().await;

    a_quote_path_of_one_or_two_legs_composes(&fixture).await;
    a_window_the_source_cannot_serve_is_an_error(&fixture).await;
    no_observation_is_substituted_for_the_one_asked_for(&fixture).await;
    // Last, and last for a reason: it leaves the source unreachable.
    an_unreachable_source_is_an_error_not_a_stale_value(&fixture).await;
}

/// **A quote path of one or two legs composes.** A token is quoted against
/// the numeraire directly or through one intermediate (ADR 0071 decision
/// 3), and the path's answer is its legs' answers composed -- not a fresh
/// number arrived at some other way, and not one leg's standing in for
/// both.
///
/// The freshness composes too, downwards: a path is only as fresh as its
/// stalest leg, since a busy WETH/USDC pool must not vouch for a thin one.
pub async fn a_quote_path_of_one_or_two_legs_composes(fixture: &ContractFixture) {
    let source = &fixture.source;

    let first = source
        .observe(&fixture.leg)
        .await
        .expect("the source answers the fixture's own leg");
    let second = source
        .observe(&fixture.next_leg)
        .await
        .expect("the source answers the fixture's onward leg");

    // One leg: the path's rate is that leg's rate, unchanged. Nothing is
    // applied here -- the spread and the guards are the rate table's job
    // (ADR 0071 decision 5), and a source that pre-applied one would be
    // quoting a price nobody asked it for.
    let direct = QuotePath::direct(fixture.leg.clone());
    let observed = source
        .quote(&direct)
        .await
        .expect("the source answers a one-leg path");
    assert_eq!(
        observed.path, direct,
        "a quote must answer the path it was asked about"
    );
    assert_eq!(
        observed.rate, first.rate,
        "a one-leg path's rate is its leg's own rate"
    );
    assert_eq!(
        observed.observed_at, first.observed_at,
        "a one-leg path is as fresh as its leg, no fresher"
    );

    // Two legs: base to intermediate to numeraire, composed.
    let path = QuotePath::through(fixture.leg.clone(), fixture.next_leg.clone())
        .expect("the fixture's legs meet: the first's quote is the second's base");
    let observed = source
        .quote(&path)
        .await
        .expect("the source answers a two-leg path");
    assert_eq!(
        observed.path, path,
        "a quote must answer the path it was asked about"
    );
    assert_eq!(
        observed.rate,
        compose_rates(first.rate, second.rate).expect("the fixture's legs compose exactly"),
        "a two-leg path's rate is its legs' rates composed"
    );
    assert_eq!(
        observed.observed_at,
        first.observed_at.min(second.observed_at),
        "a composed quote is only as fresh as its stalest leg"
    );
    assert_eq!(
        observed.path.quote(),
        &fixture.next_leg.quote,
        "a two-leg path ends where its second leg does"
    );
}

/// **A window shorter than the source can serve is an error, not a guess.**
/// The window is what the operator set the manipulation resistance to (ADR
/// 0071 decision 3: TWAP only, over an operator-set window, no spot read),
/// so a source that quietly served a window it could manage instead would
/// be substituting its own guard for the operator's -- and a flash-loaned
/// pool is exactly what the difference costs.
///
/// Asserted through both entry points, so a path cannot be the way around
/// a refusal a leg would have given.
pub async fn a_window_the_source_cannot_serve_is_an_error(fixture: &ContractFixture) {
    let too_short = QuoteLeg {
        window: fixture.window_shorter_than_the_source_can_serve,
        ..fixture.leg.clone()
    };

    let err = fixture
        .source
        .observe(&too_short)
        .await
        .expect_err("a window this source cannot serve must not be answered");
    assert!(
        matches!(err, RateSourceError::WindowNotServed { .. }),
        "a window this source cannot serve is WindowNotServed, not {err}"
    );

    let err = fixture
        .source
        .quote(&QuotePath::direct(too_short))
        .await
        .expect_err("a path over that window must not be answered either");
    assert!(
        matches!(err, RateSourceError::WindowNotServed { .. }),
        "a path over a window this source cannot serve is WindowNotServed, not {err}"
    );
}

/// **No observation is ever silently substituted for one that was asked
/// for.** Every answer carries its own question back, and every question
/// this source cannot answer is refused rather than answered with a
/// near-enough one: a pool it does not have, a pair the named pool does not
/// hold, a direction it was not asked to read in. Each substitution would
/// produce a perfectly plausible-looking number for the wrong pair, which
/// is the one failure ADR 0071 exists to make structurally impossible.
pub async fn no_observation_is_substituted_for_the_one_asked_for(fixture: &ContractFixture) {
    let source = &fixture.source;

    let observed = source
        .observe(&fixture.leg)
        .await
        .expect("the source answers the fixture's own leg");
    assert_eq!(
        observed.leg, fixture.leg,
        "an observation must carry back the leg it answers, entire"
    );

    // A pool this source does not have: refused, never answered with one
    // it does have.
    let absent = QuoteLeg {
        pool: fixture.absent_pool.clone(),
        ..fixture.leg.clone()
    };
    let err = source
        .observe(&absent)
        .await
        .expect_err("a pool this source does not have must not be answered");
    assert!(
        matches!(err, RateSourceError::PoolNotFound(_)),
        "an unnamed pool is PoolNotFound, not {err}"
    );

    // The right pool, a pair it does not hold: refused, never answered with
    // the pair it does hold.
    let wrong_pair = QuoteLeg {
        base: fixture.asset_the_pool_does_not_hold.clone(),
        ..fixture.leg.clone()
    };
    let err = source
        .observe(&wrong_pair)
        .await
        .expect_err("a pair this pool does not hold must not be answered");
    assert!(
        matches!(err, RateSourceError::PairNotInPool { .. }),
        "a pair the named pool does not hold is PairNotInPool, not {err}"
    );

    // Direction is part of the question. A pool holds its pair both ways
    // and a source answers in the one asked for; the reverse reading is a
    // different number, on the other side of one.
    let one = Rate::new(1, 1).expect("1/1 is a rate");
    assert_ne!(
        observed.rate, one,
        "the fixture's leg must not be a 1:1 pair, or reading it backwards proves nothing"
    );
    let reversed = fixture.leg.reversed();
    let back = source
        .observe(&reversed)
        .await
        .expect("a pool reads in both directions");
    assert_eq!(
        back.leg, reversed,
        "an observation must carry back the leg it answers, direction included"
    );
    assert_eq!(
        observed.rate > one,
        back.rate < one,
        "reading a pool the other way round must not answer with the same number"
    );
}

/// **An unreachable source is an error, not a stale value.** The source
/// answered this very leg a moment ago and still holds that answer; once
/// its data is out of reach it must say so rather than hand the answer back
/// again.
///
/// Staleness is an outage on purpose (ADR 0071): a pair whose source has
/// gone away reaches its `ttl` and takes its routes down loudly, because
/// the alternative -- quietly dealing on the last known price -- is how a
/// dealer is drained politely.
pub async fn an_unreachable_source_is_an_error_not_a_stale_value(fixture: &ContractFixture) {
    let source = &fixture.source;
    let path = QuotePath::direct(fixture.leg.clone());

    let before = source
        .quote(&path)
        .await
        .expect("the source answers before the outage");

    (fixture.make_unreachable)().await;

    let err = source
        .observe(&fixture.leg)
        .await
        .expect_err("an unreachable source must not answer");
    assert!(
        matches!(err, RateSourceError::Unreachable(_)),
        "an unreachable source reports it rather than repeating {}: got {err}",
        before.rate
    );

    let err = source
        .quote(&path)
        .await
        .expect_err("an unreachable source must not answer a path either");
    assert!(
        matches!(err, RateSourceError::Unreachable(_)),
        "an unreachable source reports it rather than repeating {}: got {err}",
        before.rate
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{DateTime, Utc};

    use crate::in_memory::{InMemoryRateSource, PoolContents};

    fn anyone() -> AssetId {
        AssetId::evm("0x0000000000000000000000000000000000009876")
    }

    fn weth() -> AssetId {
        AssetId::evm("0x4200000000000000000000000000000000000006")
    }

    fn usdc() -> AssetId {
        AssetId::evm("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913")
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).expect("a timestamp")
    }

    #[tokio::test]
    async fn in_memory_rate_source_upholds_the_contract() {
        assert_upholds_the_contract(|| async {
            let source = Arc::new(InMemoryRateSource::new());
            let window = Duration::seconds(300);

            // The ANYONE/WETH-then-WETH/USDC shape ADR 0071 decision 3
            // calls a two-pool quote: a thin token quoted in an
            // intermediate, the intermediate quoted in the numeraire.
            source.insert_pool(
                PoolId("anyone-weth".to_string()),
                PoolContents {
                    base: anyone(),
                    quote: weth(),
                    rate: Rate::new(1, 300_000).expect("a rate"),
                    observed_at: at(1_757_499_700),
                    shortest_window: Duration::seconds(60),
                    longest_window: Duration::seconds(1_800),
                },
            );
            source.insert_pool(
                PoolId("weth-usdc".to_string()),
                PoolContents {
                    base: weth(),
                    quote: usdc(),
                    rate: Rate::new(3_000_000_000, 1_000_000_000_000_000_000).expect("a rate"),
                    observed_at: at(1_757_500_000),
                    shortest_window: Duration::seconds(60),
                    longest_window: Duration::seconds(1_800),
                },
            );

            ContractFixture {
                source: Arc::clone(&source) as Arc<dyn RateSource>,
                leg: QuoteLeg {
                    pool: PoolId("anyone-weth".to_string()),
                    base: anyone(),
                    quote: weth(),
                    window,
                },
                next_leg: QuoteLeg {
                    pool: PoolId("weth-usdc".to_string()),
                    base: weth(),
                    quote: usdc(),
                    window,
                },
                absent_pool: PoolId("dai-usdc".to_string()),
                asset_the_pool_does_not_hold: usdc(),
                window_shorter_than_the_source_can_serve: Duration::seconds(30),
                make_unreachable: {
                    let source = Arc::clone(&source);
                    Box::new(move || {
                        let source = Arc::clone(&source);
                        Box::pin(async move { source.cut_off() })
                    })
                },
            }
        })
        .await;
    }
}
