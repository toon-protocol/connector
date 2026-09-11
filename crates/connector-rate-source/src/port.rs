use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use connector_domain::{AssetId, Rate};
use thiserror::Error;

/// An operator's name for one market venue, opaque to everything above this
/// port -- a pool address for the Uniswap-v3-compatible reader (issue
/// #1293), a base58 account for a Solana one, an arbitrary label for
/// [`crate::InMemoryRateSource`]. Whichever implementation is asked is the
/// one that parses it.
///
/// A name, and only a name, because ADR 0071 decision 3 gives the operator
/// the naming: there is no pool discovery in this connector, since letting
/// it pick pools for itself invites a dust-liquidity decoy. A pool the
/// operator cannot name does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PoolId(pub String);

impl std::fmt::Display for PoolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One leg of a quote path: read `pool` over `window`, as the price of one
/// base unit of `base` denominated in base units of `quote`.
///
/// Every field is something only the operator can supply, and together they
/// are the whole question a source is ever asked:
///
/// - **which pool** -- [`pool`](Self::pool), named, never discovered;
/// - **which direction** -- [`base`](Self::base) to [`quote`](Self::quote),
///   named by the two token identities rather than by any venue's own token
///   ordering, so the same leg reads on either chain. A pool holds its pair
///   in both directions and a source answers in the one asked for: the
///   reverse leg ([`reversed`](QuoteLeg::reversed)) is a different question
///   with a different answer, never the same number handed back;
/// - **which window** -- [`window`](Self::window), the TWAP the operator set.
///   There is no spot read: the window is the manipulation resistance, and a
///   source that cannot serve the window asked for says so
///   ([`RateSourceError::WindowNotServed`]) rather than serving a different
///   one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteLeg {
    pub pool: PoolId,
    pub base: AssetId,
    pub quote: AssetId,
    pub window: Duration,
}

impl QuoteLeg {
    /// The same pool over the same window, read the other way round.
    pub fn reversed(&self) -> QuoteLeg {
        QuoteLeg {
            pool: self.pool.clone(),
            base: self.quote.clone(),
            quote: self.base.clone(),
            window: self.window,
        }
    }
}

/// A token's quote: a path of **one or two** legs ending at the node's
/// numeraire (ADR 0071 decision 3). One leg for a numeraire-quoted token;
/// two for a token whose only real venue is quoted in an intermediate --
/// ANYONE's is a WETH-quoted pool, so its quote is ANYONE/WETH then
/// WETH/numeraire (`docs/research/token-pair-price-sources.md`).
///
/// "One or two" is structural here rather than checked, and the legs are
/// checked to meet: a path whose first leg quotes a token its second leg is
/// not based in composes to a number about nothing, which is exactly the
/// silent wrongness ADR 0071 exists to make impossible. Whether the far end
/// is really the node's numeraire is the config's own boot refusal (issue
/// #1290), since only the config knows which token that is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotePath {
    first: QuoteLeg,
    second: Option<QuoteLeg>,
}

impl QuotePath {
    /// A one-leg path: the token is quoted against the numeraire directly.
    pub fn direct(leg: QuoteLeg) -> QuotePath {
        QuotePath {
            first: leg,
            second: None,
        }
    }

    /// A two-leg path through an intermediate: `first` quotes the token in
    /// the intermediate, `second` quotes that intermediate in the
    /// numeraire. Refused when the legs do not meet.
    pub fn through(first: QuoteLeg, second: QuoteLeg) -> Result<QuotePath, QuotePathError> {
        if first.quote != second.base {
            return Err(QuotePathError::LegsDoNotMeet {
                quoted: first.quote,
                based_in: second.base,
            });
        }
        Ok(QuotePath {
            first,
            second: Some(second),
        })
    }

    pub fn first(&self) -> &QuoteLeg {
        &self.first
    }

    pub fn second(&self) -> Option<&QuoteLeg> {
        self.second.as_ref()
    }

    pub fn legs(&self) -> impl Iterator<Item = &QuoteLeg> {
        std::iter::once(&self.first).chain(self.second.iter())
    }

    /// The token this path prices.
    pub fn base(&self) -> &AssetId {
        &self.first.base
    }

    /// The token it prices it in -- the numeraire end.
    pub fn quote(&self) -> &AssetId {
        match &self.second {
            Some(second) => &second.quote,
            None => &self.first.quote,
        }
    }
}

/// The one way a [`QuotePath`] can be ill-formed. Separate from
/// [`RateSourceError`] on purpose: this is a path that was never a question,
/// caught where it is built, rather than a source failing to answer one.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QuotePathError {
    #[error("a quote path's first leg quotes {quoted} but its second is based in {based_in}")]
    LegsDoNotMeet { quoted: AssetId, based_in: AssetId },
}

/// What a source read for one [`QuoteLeg`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegObservation {
    /// The leg this answers, echoed back. The echo is the port's guard
    /// against a substituted observation: a source that served a shorter
    /// window than it was asked for, or read the pool the other way round,
    /// or answered about a pool the caller did not name, cannot also echo
    /// the question it was actually asked.
    pub leg: QuoteLeg,
    /// How many base units of [`QuoteLeg::quote`] one base unit of
    /// [`QuoteLeg::base`] bought, averaged over [`QuoteLeg::window`]. The
    /// decimals difference between the two tokens is already folded into
    /// the ratio (ADR 0071 decision 4) -- scale is not price, and nothing
    /// downstream applies a second one.
    pub rate: Rate,
    /// When the **source itself** dates this observation: the end of the
    /// window as the venue's own clock recorded it, never the reader's wall
    /// clock. The difference is load-bearing on a pool whose observations
    /// are written by swaps -- a quiet Raydium CLMM pool's newest
    /// observation ages while every poll of it keeps succeeding
    /// (`docs/research/token-pair-price-sources.md`), and it is this
    /// instant, not the poll's, that the `ttl` guard (ADR 0071 decision 5)
    /// must be read against.
    pub observed_at: DateTime<Utc>,
}

/// What a source read for a whole [`QuotePath`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteObservation {
    /// The path this answers, echoed back -- for the reason
    /// [`LegObservation::leg`] is.
    pub path: QuotePath,
    /// The path's legs composed: how many base units of
    /// [`QuotePath::quote`] one base unit of [`QuotePath::base`] bought.
    pub rate: Rate,
    /// The **oldest** of the legs' [`LegObservation::observed_at`]: a
    /// composed quote is only as fresh as its stalest leg, and dating it
    /// any later would let a busy WETH/USDC pool vouch for the freshness of
    /// a thin one.
    pub observed_at: DateTime<Utc>,
}

/// Errors a [`RateSource`] implementation reports.
///
/// Every variant is a refusal, and that is the point: ADR 0071's absence
/// rule ("no declared rate, no conversion, no forward") only holds if a
/// source that cannot answer says so. A source never guesses, never
/// interpolates a window it does not have, and never hands back the last
/// number it happened to know.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RateSourceError {
    /// This source has no pool by that name: an address with no pool at it,
    /// a pool without the interface this source reads (ADR 0071 decision 6
    /// excludes Uniswap v4 core pools by name -- their oracles are optional
    /// per-pool hooks that cannot be assumed present), or a name this
    /// source cannot parse at all.
    #[error("no pool named '{0}' at this rate source")]
    PoolNotFound(PoolId),

    /// The named pool exists but does not hold the named pair, in either
    /// direction. Refused rather than answered with the pair the pool
    /// *does* hold -- an operator who names their ANYONE/WETH pool for an
    /// ANYONE/USDC leg has made an error worth hearing about, and the WETH
    /// number would be a perfectly plausible-looking wrong one.
    #[error("pool '{pool}' does not hold the pair {base}/{quote}")]
    PairNotInPool {
        pool: PoolId,
        base: AssetId,
        quote: AssetId,
    },

    /// The pool cannot serve that window, at either end: shorter than its
    /// observations can span (Raydium CLMM writes one at most every 15
    /// seconds), or longer than its history reaches back (a Uniswap v3 pool
    /// whose observation cardinality has not been grown reverts `OLD`, and
    /// a freshly initialised one serves no window at all). An error, never
    /// a nearby window served quietly in its place: the window is what the
    /// operator set the manipulation resistance to, so substituting one is
    /// substituting their guard.
    #[error("pool '{pool}' cannot serve a {}s TWAP window", .window.num_seconds())]
    WindowNotServed { pool: PoolId, window: Duration },

    /// The source could not be reached -- an RPC timeout, a refused
    /// connection, a malformed response. Reported, never papered over with
    /// the last observation this source made: staleness is an outage on
    /// purpose (ADR 0071), and a pair whose source has gone away must reach
    /// its `ttl` and take its routes down loudly rather than keep dealing
    /// on a price that stopped moving.
    #[error("rate source unreachable: {0}")]
    Unreachable(String),

    /// The composed rate does not fit a `u64/u64` [`Rate`] even after
    /// reduction -- see [`compose_rates`]. Reached only by a path whose two
    /// legs multiply out past `1/u64::MAX` or `u64::MAX/1`, which no real
    /// token pair does; refused rather than saturated, because a saturated
    /// rate is a made-up price.
    #[error("the composed rate {numerator}/{denominator} is not a u64 rational")]
    Unrepresentable { numerator: u128, denominator: u128 },
}

/// The rate-source port (ADR 0071 decision 6): read a token's price off a
/// market the operator named, over a window the operator set, without the
/// caller knowing which chain or which venue that is.
///
/// One method is required -- [`observe`](RateSource::observe), which reads
/// a single leg. [`quote`](RateSource::quote) walks a path's legs and
/// composes them, and is provided rather than required because every source
/// composes identically; an implementation whose venue can read a whole
/// path in one round trip (a batched `eth_call`, say) may override it, and
/// the contract suite is what holds such an override to the same answer.
///
/// The four promises the suite in [`crate::contract`] defines, and the
/// whole of what a source promises:
///
/// 1. a quote path of one or two legs **composes**;
/// 2. a window shorter than the source can serve is an **error**, not a
///    guess;
/// 3. an unreachable source is an **error**, not a stale value;
/// 4. **no observation is ever silently substituted** for one that was
///    asked for.
///
/// Every method is asynchronous and fallible because a real implementation
/// reads a chain over RPC. Nothing on the forwarding path calls one: a
/// background poller (issue #1294) does, and a packet only ever reads the
/// table that poller writes.
#[async_trait]
pub trait RateSource: Send + Sync {
    /// Read one leg: the mean price of `leg.base` in `leg.quote`, over
    /// `leg.window`, from `leg.pool`.
    ///
    /// The answer carries the leg back ([`LegObservation::leg`]) and must
    /// carry back the one it was asked. A source that can serve the pool
    /// but not this window, direction or pair returns the error saying so;
    /// it never answers with an observation the caller did not ask for.
    async fn observe(&self, leg: &QuoteLeg) -> Result<LegObservation, RateSourceError>;

    /// Read a whole quote path: every leg, composed into one rate from the
    /// path's base to the numeraire at its far end.
    ///
    /// The composition is [`compose_rates`], and the freshness is the
    /// oldest leg's. A leg that cannot be read fails the whole path --
    /// there is no partial quote, because half a composition is a rate
    /// between the wrong two tokens.
    async fn quote(&self, path: &QuotePath) -> Result<QuoteObservation, RateSourceError> {
        let first = self.observe(path.first()).await?;
        let Some(second_leg) = path.second() else {
            return Ok(QuoteObservation {
                path: path.clone(),
                rate: first.rate,
                observed_at: first.observed_at,
            });
        };
        let second = self.observe(second_leg).await?;
        Ok(QuoteObservation {
            path: path.clone(),
            rate: compose_rates(first.rate, second.rate)?,
            observed_at: first.observed_at.min(second.observed_at),
        })
    }
}

/// Compose two legs' rates: base-to-intermediate, then
/// intermediate-to-numeraire, into base-to-numeraire. The port's own
/// definition of "the legs compose", so that every implementation -- and
/// the suite that holds them to it -- means the same arithmetic by it.
///
/// Exact wherever the product reduces into `u64/u64`, which is where every
/// realistic pair lands: a rate folds a decimals difference into the ratio
/// (ADR 0071 decision 4), and even 18-against-6 decimals leaves both terms
/// twelve orders of magnitude inside the ceiling. Past that the two terms
/// are divided down by one common integer -- the ratio is preserved to
/// within a part in `u64::MAX`, and both terms are floored, which is not a
/// favour to either side of the trade. Dealing margin is the `spread`
/// guard's job (ADR 0071 decision 5), not a rounding rule's, and no float
/// touches any of it.
pub fn compose_rates(first: Rate, second: Rate) -> Result<Rate, RateSourceError> {
    let numerator = u128::from(first.numerator()) * u128::from(second.numerator());
    let denominator = u128::from(first.denominator()) * u128::from(second.denominator());

    let divisor = greatest_common_divisor(numerator, denominator);
    let (mut reduced_numerator, mut reduced_denominator) =
        (numerator / divisor, denominator / divisor);

    const CEILING: u128 = u64::MAX as u128;
    if reduced_numerator > CEILING || reduced_denominator > CEILING {
        let scale = reduced_numerator.max(reduced_denominator).div_ceil(CEILING);
        reduced_numerator /= scale;
        reduced_denominator /= scale;
    }

    let unrepresentable = || RateSourceError::Unrepresentable {
        numerator,
        denominator,
    };
    let reduced_numerator = u64::try_from(reduced_numerator).map_err(|_| unrepresentable())?;
    let reduced_denominator = u64::try_from(reduced_denominator).map_err(|_| unrepresentable())?;
    // `Rate::new` refuses a zero term at either end (issue #1288), which is
    // exactly what a rate divided down past representability becomes.
    Rate::new(reduced_numerator, reduced_denominator).map_err(|_| unrepresentable())
}

fn greatest_common_divisor(a: u128, b: u128) -> u128 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(token: &str) -> AssetId {
        AssetId::solana(token)
    }

    fn leg(base: &str, quote: &str) -> QuoteLeg {
        QuoteLeg {
            pool: PoolId(format!("{base}-{quote}")),
            base: asset(base),
            quote: asset(quote),
            window: Duration::seconds(300),
        }
    }

    #[test]
    fn a_path_composes_only_where_its_legs_meet() {
        let path = QuotePath::through(leg("anyone", "weth"), leg("weth", "usdc"))
            .expect("legs that meet at WETH");
        assert_eq!(path.base(), &asset("anyone"));
        assert_eq!(path.quote(), &asset("usdc"));
        assert_eq!(path.legs().count(), 2);

        let err = QuotePath::through(leg("anyone", "weth"), leg("usdc", "dai")).unwrap_err();
        assert_eq!(
            err,
            QuotePathError::LegsDoNotMeet {
                quoted: asset("weth"),
                based_in: asset("usdc"),
            }
        );
    }

    #[test]
    fn a_one_leg_path_names_its_own_ends() {
        let path = QuotePath::direct(leg("anyone", "usdc"));
        assert_eq!(path.base(), &asset("anyone"));
        assert_eq!(path.quote(), &asset("usdc"));
        assert_eq!(path.second(), None);
    }

    #[test]
    fn composing_multiplies_and_reduces() {
        let composed = compose_rates(
            Rate::new(1, 300_000).expect("a rate"),
            Rate::new(3_000_000_000, 1_000_000_000_000_000_000).expect("a rate"),
        )
        .expect("a composable pair");
        // 1/300000 x 3/1000000000 = 1/100000000000000.
        assert_eq!(composed, Rate::new(1, 100_000_000_000_000).expect("a rate"));
    }

    #[test]
    fn a_product_past_u64_is_scaled_rather_than_wrapped() {
        // Both terms multiply out past u64::MAX and share no factor, so the
        // pair is divided down: the ratio survives, the spelling does not.
        let big = Rate::new(u64::MAX - 2, u64::MAX - 1).expect("a rate");
        let composed = compose_rates(big, big).expect("still representable");
        assert!(composed.numerator() < composed.denominator());
        // Within a part in 10^18 of the true square of a ratio that is
        // itself within a part in 10^19 of one.
        assert!(
            u128::from(composed.denominator()) - u128::from(composed.numerator())
                < u128::from(composed.denominator()) / 1_000_000_000_000_000_000,
        );
    }

    #[test]
    fn a_rate_that_divides_down_to_nothing_is_refused() {
        let tiny = Rate::new(1, u64::MAX).expect("a rate");
        let err = compose_rates(tiny, tiny).unwrap_err();
        assert_eq!(
            err,
            RateSourceError::Unrepresentable {
                numerator: 1,
                denominator: u128::from(u64::MAX) * u128::from(u64::MAX),
            }
        );
    }

    #[test]
    fn a_leg_reads_the_other_way_round() {
        let there = leg("anyone", "usdc");
        let back = there.reversed();
        assert_eq!(back.pool, there.pool);
        assert_eq!(back.base, there.quote);
        assert_eq!(back.quote, there.base);
        assert_eq!(back.window, there.window);
        assert_eq!(back.reversed(), there);
    }
}
