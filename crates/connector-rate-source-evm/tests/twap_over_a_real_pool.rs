//! Reading a TWAP off a real chain (issue #1293, ADR 0071 decisions 3 and 6):
//! what a quote path composes to, what a pool that cannot serve a window says
//! instead, and why growing observation cardinality is the difference between
//! the two.
//!
//! Tier 3 (ADR 0007): chain behaviour is the subject, so each test forks its
//! own disposable `anvil`, compiles `contracts/OracleMockPool.sol` with a real
//! `forge`, deploys it, and drives it through a scripted timeline of swaps.
//! Never the `docker-compose` containers -- nothing under `crates/` dials
//! `localhost:8545`. A missing chain binary fails loudly under `CI` and skips
//! locally, and never reports a pass in `0.00s`.

mod support;

use chrono::Duration;
use connector_domain::{AssetId, Rate};
use connector_rate_source::{
    compose_rates, PoolId, QuoteLeg, QuotePath, RateSource, RateSourceError,
};
use connector_rate_source_evm::tick_math::{rate_at_tick, PoolDirection};
use connector_rate_source_evm::UniswapV3RateSource;
use connector_settlement_evm::test_support::require_anvil;

const ANVIL_BASE_PORT: u16 = 19_400;

/// 1 base unit of ANYONE in base units of WETH, at the mean tick the fixture's
/// timeline produces. Both tokens are 18 decimals, so this is also the human
/// price: about one three-hundred-thousandth of an ether.
///
/// A known-good figure, not a figure this code produced: it comes from
/// v3-core's `TickMath` algorithm worked in exact integer arithmetic outside
/// this crate, and `src/tick_math.rs`'s own unit tests hold the Rust port to
/// the same two numbers without a chain in sight.
fn anyone_in_weth() -> Rate {
    Rate::new(30_734_378_297_187, 9_223_372_036_854_775_808).expect("a rate")
}

/// 1 base unit of WETH in base units of USDC: about 3001 USDC to the ether,
/// once the 18-against-6 decimals gap is read out of the ratio.
fn weth_in_usdc() -> Rate {
    Rate::new(17_179_869_184, 5_724_706_407_302_616_293).expect("a rate")
}

/// The two composed: about one hundredth of a USDC per ANYONE.
fn anyone_in_usdc() -> Rate {
    Rate::new(184_468, 18_446_744_039_687_437_403).expect("a rate")
}

fn window() -> Duration {
    Duration::seconds(support::WINDOW_SECONDS)
}

fn anyone_weth_leg(pool: &PoolId) -> QuoteLeg {
    QuoteLeg {
        pool: pool.clone(),
        base: AssetId::evm(support::ANYONE),
        quote: AssetId::evm(support::WETH),
        window: window(),
    }
}

fn weth_usdc_leg(pool: &PoolId) -> QuoteLeg {
    QuoteLeg {
        pool: pool.clone(),
        base: AssetId::evm(support::WETH),
        quote: AssetId::evm(support::USDC),
        window: window(),
    }
}

/// A one-leg quote and a two-leg quote both resolve against the numeraire, and
/// the two-leg composition lands on a figure computed outside this crate.
///
/// The ANYONE/WETH pool is swapped to a second tick partway through the window
/// on purpose, and the window's mean tick is **neither** of the two ticks the
/// pool ever held. That is the assertion that a spot read cannot pass: a
/// reader that looked at the pool's current price would answer with the second
/// tick's rate, and this test would name it.
#[tokio::test]
async fn a_two_leg_quote_composes_to_a_known_good_rate() {
    if !require_anvil() || !support::require_forge() {
        return;
    }

    let venue = support::Venue::stand_up(ANVIL_BASE_PORT).await;
    let source = UniswapV3RateSource::connect(&venue.rpc_url).expect("a reader");

    let first = source
        .observe(&anyone_weth_leg(&venue.anyone_weth))
        .await
        .expect("the ANYONE/WETH pool serves the window");
    assert_eq!(
        first.rate,
        anyone_in_weth(),
        "the mean tick over the window is {}, and that tick's price is a known figure",
        support::ANYONE_WETH_MEAN_TICK
    );
    assert_eq!(
        first.observed_at, venue.head,
        "an observation is dated by the venue's own clock, not the poller's"
    );

    for spot in [
        support::ANYONE_WETH_TICK_BEFORE,
        support::ANYONE_WETH_TICK_AFTER,
    ] {
        assert_ne!(
            first.rate,
            rate_at_tick(spot, PoolDirection::Token1PerToken0).expect("a rate"),
            "the window's mean is not the price at tick {spot}: a TWAP over a pool that moved \
             is not either end of the move, and a reader that answered with one would be \
             reading a spot price"
        );
    }

    // The second leg is read against the pool's *other* direction: USDC is
    // that pool's `token0`, and the leg asks for WETH in USDC.
    let second = source
        .observe(&weth_usdc_leg(&venue.usdc_weth))
        .await
        .expect("the USDC/WETH pool serves the window");
    assert_eq!(second.rate, weth_in_usdc());

    // One leg: the path's rate is the leg's own, with nothing applied. The
    // spread and the guards are the rate table's job (ADR 0071 decision 5).
    let direct = source
        .quote(&QuotePath::direct(weth_usdc_leg(&venue.usdc_weth)))
        .await
        .expect("a one-leg path");
    assert_eq!(direct.rate, weth_in_usdc());

    // Two legs: ANYONE priced in WETH, WETH priced in USDC, composed into
    // ANYONE priced in the numeraire -- ADR 0071 decision 3's two-pool quote.
    let path = QuotePath::through(
        anyone_weth_leg(&venue.anyone_weth),
        weth_usdc_leg(&venue.usdc_weth),
    )
    .expect("the legs meet at WETH");
    let composed = source.quote(&path).await.expect("a two-leg path");

    assert_eq!(
        composed.rate,
        anyone_in_usdc(),
        "ANYONE against the numeraire, through WETH"
    );
    assert_eq!(
        composed.rate,
        compose_rates(first.rate, second.rate).expect("the legs compose"),
        "a path's rate is its legs' rates composed, and nothing else"
    );
    assert_eq!(composed.path.base(), &AssetId::evm(support::ANYONE));
    assert_eq!(composed.path.quote(), &AssetId::evm(support::USDC));
}

/// Everything this reader refuses rather than guesses at, against the chain
/// that would have let it guess.
#[tokio::test]
async fn a_pool_that_cannot_answer_the_question_asked_says_so() {
    if !require_anvil() || !support::require_forge() {
        return;
    }

    let venue = support::Venue::stand_up(ANVIL_BASE_PORT + 10).await;
    let source = UniswapV3RateSource::connect(&venue.rpc_url).expect("a reader");

    // A window longer than the pool's whole history. v3's oracle reverts
    // `OLD`; this reader reports the window it could not serve rather than
    // quietly serving the longest one it could.
    let error = source
        .observe(&QuoteLeg {
            window: Duration::seconds(3_600),
            ..anyone_weth_leg(&venue.anyone_weth)
        })
        .await
        .expect_err("an hour of history does not exist on a chain minutes old");
    assert!(
        matches!(
            &error,
            RateSourceError::WindowNotServed { pool, window }
                if *pool == venue.anyone_weth && window.num_seconds() == 3_600
        ),
        "got {error}"
    );

    // A pool whose only observation is younger than the window's far edge:
    // the same refusal, for the same reason, on a pool that is otherwise
    // perfectly readable.
    let error = source
        .observe(&anyone_weth_leg(&venue.too_young))
        .await
        .expect_err("a pool younger than the window cannot serve it");
    assert!(
        matches!(error, RateSourceError::WindowNotServed { .. }),
        "got {error}"
    );

    // An address with no code at it. The `eth_call` succeeds and returns
    // nothing, which is not a pool and must not be read as a price.
    let absent = PoolId(support::NO_POOL_HERE.to_string());
    let error = source
        .observe(&anyone_weth_leg(&absent))
        .await
        .expect_err("there is no pool at that address");
    assert_eq!(error, RateSourceError::PoolNotFound(absent));

    // The right pool, a pair it does not hold. Refused rather than answered
    // with the pair it does hold, which would be a plausible-looking number
    // for the wrong two tokens.
    let error = source
        .observe(&QuoteLeg {
            base: AssetId::evm(support::USDC),
            ..anyone_weth_leg(&venue.anyone_weth)
        })
        .await
        .expect_err("the ANYONE/WETH pool holds no USDC");
    assert!(
        matches!(error, RateSourceError::PairNotInPool { .. }),
        "got {error}"
    );
}

/// **Growing observation cardinality is what makes a window servable.**
///
/// A pool is created with an array of one observation, and every swap
/// overwrites it -- so a pool that is being traded holds no history at all
/// until somebody pays for the slots
/// (`docs/research/token-pair-price-sources.md`). Naming a pool is therefore
/// not enough, and this is the operator prerequisite the runbook owns: the
/// same read, over the same window, against the same pool, refuses before the
/// growth and answers after it.
#[tokio::test]
async fn growing_observation_cardinality_is_what_makes_a_window_servable() {
    if !require_anvil() || !support::require_forge() {
        return;
    }

    let mut pool = support::UngrownPool::stand_up(ANVIL_BASE_PORT + 20).await;
    let source = UniswapV3RateSource::connect(&pool.rpc_url).expect("a reader");
    let leg = QuoteLeg {
        pool: pool.pool.clone(),
        base: AssetId::evm(support::ANYONE),
        quote: AssetId::evm(support::WETH),
        window: Duration::seconds(60),
    };

    let error = source
        .observe(&leg)
        .await
        .expect_err("one observation slot, overwritten by every swap, is no history");
    assert!(
        matches!(error, RateSourceError::WindowNotServed { .. }),
        "got {error}"
    );

    pool.grow_and_keep_swapping().await;

    let observation = source
        .observe(&leg)
        .await
        .expect("the same window, once the pool has slots to remember it in");
    assert_eq!(
        observation.rate,
        rate_at_tick(
            support::ANYONE_WETH_MEAN_TICK,
            PoolDirection::Token1PerToken0
        )
        .expect("a rate"),
        "the pool held one tick throughout, so the window's mean is that tick"
    );
    assert_eq!(observation.leg, leg, "an answer carries its question back");
}
