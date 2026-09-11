//! The background poller that keeps the rate table fresh, off the forwarding
//! path ([ADR 0071](../../../docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)
//! decision 6, issue #1294).
//!
//! One [`RatePoller`] per token that declared a `quote` path: it walks that
//! token's operator-named pools through a [`RateSource`], composes what they
//! answer into the token's price in the numeraire, and writes it into the
//! [`SharedRateTable`] a forward reads. A packet never waits on any of this.
//! A slow RPC costs a refresh its latency; it costs no packet anything.
//!
//! # What the poller decides, and what it does not
//!
//! Very little is decided here, deliberately. The three guards are the
//! table's ([`connector_domain::rate_table`]), and this module only carries
//! their verdicts to the log:
//!
//! * **`max_move`** -- a refresh jumping past the bound is refused by
//!   [`RateTable::refresh`](connector_domain::RateTable::refresh), which
//!   leaves the previous value in place *with its own observation instant*,
//!   so it goes on ageing towards its `ttl` exactly as it would have. The poller logs the refusal and keeps
//!   polling: the next reading inside the bound is taken, and if none comes
//!   the pair ages out on its own.
//! * **`ttl`** -- staleness is not something the poller does. It is a
//!   question the *lookup* answers from the instant the observation carries,
//!   so a source that stops answering needs no notification, no timer and no
//!   eviction: the pair simply stops being live one `ttl` after its last
//!   good reading, and every route across it refuses until a fresh one
//!   lands. That is ADR 0071's "staleness is an outage, on purpose", and the
//!   poller's part in it is to write nothing rather than to write something
//!   old.
//! * **`spread`** -- never seen here at all. The table stores mids; the
//!   lookup widens them.
//!
//! An observation is dated by the **source's** clock
//! ([`QuoteObservation::observed_at`](connector_rate_source::QuoteObservation::observed_at)),
//! never by the poller's. On a pool
//! whose observations are written by swaps, a quiet hour ages the newest
//! reading while every poll of it keeps succeeding -- and it is that
//! instant, not the poll's, that the `ttl` guard has to be read against.
//! This module therefore holds no [`crate::Clock`]: it has nothing to ask
//! one.
//!
//! # Cadence
//!
//! Each poller runs at a cadence derived from its own pair's `ttl` -- see
//! [`poll_interval`]. There is no `poll_interval` config key and should not
//! be one: ADR 0009 makes the config the immutable declaration of the
//! *source*, `ttl` is already the operator's statement of how old a rate may
//! get, and a second key could only ever disagree with it.
//!
//! # Nothing here writes a file
//!
//! A refresh is an observation, which ADR 0071 decision 6 puts "on the same
//! footing as a chain balance": it lives in the table for the life of the
//! process and nowhere else. The poller opens no journal, names no path and
//! has no `state_dir` -- the config the operator deployed is read once at
//! boot and never rewritten, and a restart starts from no observation at
//! all, which is the correct state to start from.

use std::sync::Arc;
use std::time::Duration;

use chrono::TimeDelta;
use connector_config::DenominationConfig;
use connector_domain::{AssetId, Refresh, Ttl};
use connector_rate_source::{PoolId, QuoteLeg, QuotePath, QuotePathError, RateSource};
use thiserror::Error;

use crate::rate_table::SharedRateTable;

/// How many refreshes a pair gets inside one `ttl` (see [`poll_interval`]).
///
/// Three, so a pair survives two consecutive failed readings -- a dropped
/// RPC connection and its retry -- before it ages out and takes its routes
/// down. Two would make every single failure a coin flip against the
/// deadline; many more would poll a chain harder than an operator who wrote
/// a `ttl` could reasonably expect.
const REFRESHES_PER_TTL: i32 = 3;

/// The floor under [`poll_interval`], whatever `ttl` is: one second.
///
/// A `ttl` of a few seconds is a test's, not an operator's, and the floor is
/// here so that such a value produces a brisk poller rather than a hot loop
/// against an RPC endpoint. Nothing above it is clamped -- a long `ttl` gets
/// the long cadence it asked for.
const MINIMUM_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// How often a pair whose rate may be `ttl` old is refreshed:
/// `ttl / REFRESHES_PER_TTL`, never below `MINIMUM_POLL_INTERVAL` (one second).
///
/// Derived rather than configured, for the reason this module's header
/// gives: `ttl` is already the operator's statement of how old a rate may
/// get, and the cadence is only ever a consequence of it.
pub fn poll_interval(ttl: Ttl) -> Duration {
    (ttl.as_time_delta() / REFRESHES_PER_TTL)
        .to_std()
        .unwrap_or(MINIMUM_POLL_INTERVAL)
        .max(MINIMUM_POLL_INTERVAL)
}

/// Why a declared quote path cannot be polled.
///
/// Every variant is something `Config::load` already refuses by name (ADR
/// 0071 decision 3: one or two legs, on the token's own settlement chain,
/// ending at the numeraire), so this is the second lock on the same door.
/// It exists as an error rather than an `expect` because the price of being
/// wrong about that is a node that panics mid-boot, and because a poller
/// silently skipping a pair would leave the operator reading a config that
/// says the pair is priced while every forward across it refuses.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QuotePathUnusable {
    /// A path with no leg at all -- nothing to read, and no rate that could
    /// come of reading it.
    #[error("the quote path for {token} names no pool")]
    NoLegs { token: AssetId },

    /// More legs than ADR 0071 decision 3 allows. A third leg is not a
    /// longer path, it is a path nobody has thought about.
    #[error("the quote path for {token} names {legs} pools; at most two compose")]
    TooManyLegs { token: AssetId, legs: usize },

    /// The path prices something other than the token whose row it would be
    /// written to.
    #[error("the quote path for {token} prices {prices} instead")]
    DoesNotPriceTheToken { token: AssetId, prices: AssetId },

    /// The path ends somewhere other than this node's numeraire, so what it
    /// composes to is not a quote at all.
    #[error("the quote path for {token} ends at {ends_at}, not at the numeraire {numeraire}")]
    DoesNotEndAtNumeraire {
        token: AssetId,
        numeraire: AssetId,
        ends_at: AssetId,
    },

    /// Two legs that do not meet: the second reads a pair the first does not
    /// hand it.
    #[error("the quote path for {token} does not compose: {source}")]
    LegsDoNotMeet {
        token: AssetId,
        source: QuotePathError,
    },

    /// A TWAP window no span type can hold. Unreachable from a file a human
    /// wrote; refused rather than truncated, because a truncated window is
    /// somebody else's manipulation resistance.
    #[error(
        "the quote path for {token} asks for a TWAP window of {window:?}, which is not a span"
    )]
    WindowOutOfRange { token: AssetId, window: Duration },
}

/// One quoted token's background poller: the pair `(token, numeraire)`, the
/// path its price is read over, and the cadence that pair's `ttl` implies.
///
/// One per token rather than one sweeping every token, because the cadence
/// is a property of the pair: a thin token tightened to a short `ttl` should
/// not drag a deep one's polling up with it, and a long-`ttl` pair should
/// not be polled at a short-`ttl` pair's rate. Independent tasks also mean a
/// source that hangs on one pool delays only the pair that named it.
pub struct RatePoller {
    table: SharedRateTable,
    source: Arc<dyn RateSource>,
    token: AssetId,
    numeraire: AssetId,
    path: QuotePath,
    cadence: Duration,
}

impl RatePoller {
    /// One poller per `[[tokens]]` row that declared a `quote` path, in the
    /// order declared.
    ///
    /// A node whose tokens declare no quote path gets an empty `Vec` and
    /// starts nothing -- the static-rows-only configuration ADR 0071
    /// decision 3 leaves the operator to tend by hand, and every node that
    /// predates the record.
    pub fn for_config(
        table: &SharedRateTable,
        source: Arc<dyn RateSource>,
        denomination: &DenominationConfig,
    ) -> Result<Vec<RatePoller>, QuotePathUnusable> {
        denomination
            .quoted_tokens()
            .map(|(token, declared)| {
                let path = port_quote_path(token, declared)?;
                RatePoller::new(table, Arc::clone(&source), token.clone(), path)
            })
            .collect()
    }

    /// One poller for one token, against the pair and guards `table` already
    /// holds: the numeraire every quote is taken against, and the `ttl` this
    /// pair's cadence comes from.
    pub fn new(
        table: &SharedRateTable,
        source: Arc<dyn RateSource>,
        token: AssetId,
        path: QuotePath,
    ) -> Result<RatePoller, QuotePathUnusable> {
        let held = table.read();
        let numeraire = held.numeraire().clone();
        if path.base() != &token {
            return Err(QuotePathUnusable::DoesNotPriceTheToken {
                token,
                prices: path.base().clone(),
            });
        }
        if path.quote() != &numeraire {
            return Err(QuotePathUnusable::DoesNotEndAtNumeraire {
                ends_at: path.quote().clone(),
                token,
                numeraire,
            });
        }
        let cadence = poll_interval(held.guards(&token, &numeraire).ttl());
        Ok(RatePoller {
            table: table.clone(),
            source,
            token,
            numeraire,
            path,
            cadence,
        })
    }

    /// The token this poller prices.
    pub fn token(&self) -> &AssetId {
        &self.token
    }

    /// How often [`run`](RatePoller::run) reads the path.
    pub fn cadence(&self) -> Duration {
        self.cadence
    }

    /// Read the path once and write what it says into the table.
    ///
    /// Never fails: every outcome is either a written row or a log line, and
    /// there is nothing a caller could usefully do about the difference. A
    /// source that does not answer leaves the pair exactly as it was, ageing
    /// against its own `ttl` -- deliberately, since the alternative to "no
    /// new observation" is dealing on an old price, which is how a dealer is
    /// drained politely.
    pub async fn refresh_once(&self) {
        let observed = match self.source.quote(&self.path).await {
            Ok(observed) => observed,
            Err(error) => {
                tracing::warn!(
                    token = %self.token,
                    numeraire = %self.numeraire,
                    %error,
                    "rate source did not answer; this pair keeps its last observation and ages \
                     towards its ttl"
                );
                return;
            }
        };

        let verdict = self.table.write(|table| {
            table.refresh(
                self.token.clone(),
                self.numeraire.clone(),
                observed.rate,
                observed.observed_at,
            )
        });

        match verdict {
            Refresh::Accepted => tracing::debug!(
                token = %self.token,
                numeraire = %self.numeraire,
                rate = %observed.rate,
                observed_at = %observed.observed_at,
                "refreshed a declared quote"
            ),
            Refresh::OutsideMaxMove { previous } => tracing::warn!(
                token = %self.token,
                numeraire = %self.numeraire,
                offered = %observed.rate,
                previous = %previous,
                max_move = %self
                    .table
                    .read()
                    .guards(&self.token, &self.numeraire)
                    .max_move(),
                "refusing a refresh outside max_move as probable manipulation; the previous value \
                 stays in force and goes on ageing against its own ttl, so a human has until then \
                 to look"
            ),
            Refresh::Declared { standing } => tracing::warn!(
                token = %self.token,
                numeraire = %self.numeraire,
                observed = %observed.rate,
                standing = %standing,
                "this pair runs a static [[rates]] row, so the observation is discarded; a quote \
                 path and a declared rate for one pair is a configuration saying two things"
            ),
        }
    }

    /// Refresh forever, at this pair's [`cadence`](RatePoller::cadence).
    ///
    /// Reads once immediately -- `tokio`'s first tick is now -- so a node
    /// that has just booted prices its pairs as soon as its source can
    /// answer rather than one cadence later. Never returns, so it is
    /// spawned rather than awaited: startup does not wait on a chain, in the
    /// same shape `EvmChannelIndexSyncer::run` already runs in.
    pub async fn run(self) {
        let mut ticks = tokio::time::interval(self.cadence);
        // A tick missed because a read took longer than the cadence is a
        // slow source, not a debt: catching up by firing the backlog back to
        // back would hammer exactly the endpoint that is already struggling.
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            self.refresh_once().await;
        }
    }
}

impl std::fmt::Debug for RatePoller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RatePoller")
            .field("token", &self.token)
            .field("numeraire", &self.numeraire)
            .field("path", &self.path)
            .field("cadence", &self.cadence)
            .finish_non_exhaustive()
    }
}

/// The declared path as the port asks for it: each leg based in what the
/// previous one quoted, starting at the token and ending -- config having
/// already checked it -- at the numeraire.
///
/// The two vocabularies are deliberately not the same type.
/// `connector_config`'s leg is what the operator wrote (a pool, a token to
/// quote into, a window); the port's is a question to ask a venue (a pool, a
/// *pair*, a window). Threading the base through here is the whole of the
/// difference, and doing it in one place is why [`QuotePath::through`] can
/// only fail on a path this function did not build.
fn port_quote_path(
    token: &AssetId,
    declared: &connector_config::QuotePath,
) -> Result<QuotePath, QuotePathUnusable> {
    let mut base = token.clone();
    let mut legs = Vec::with_capacity(declared.legs().len());
    for leg in declared.legs() {
        let window = TimeDelta::from_std(leg.twap_window()).map_err(|_| {
            QuotePathUnusable::WindowOutOfRange {
                token: token.clone(),
                window: leg.twap_window(),
            }
        })?;
        legs.push(QuoteLeg {
            pool: PoolId(leg.pool().to_string()),
            base: std::mem::replace(&mut base, leg.quote_token().clone()),
            quote: leg.quote_token().clone(),
            window,
        });
    }

    let mut legs = legs.into_iter();
    let (Some(first), second) = (legs.next(), legs.next()) else {
        return Err(QuotePathUnusable::NoLegs {
            token: token.clone(),
        });
    };
    if legs.next().is_some() {
        return Err(QuotePathUnusable::TooManyLegs {
            token: token.clone(),
            legs: declared.legs().len(),
        });
    }
    match second {
        None => Ok(QuotePath::direct(first)),
        Some(second) => {
            QuotePath::through(first, second).map_err(|source| QuotePathUnusable::LegsDoNotMeet {
                token: token.clone(),
                source,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeZone, Utc};
    use connector_config::Config;
    use connector_domain::{Guards, MaxMove, Rate, RateLookup, RateTable, Spread};
    use connector_rate_source::{InMemoryRateSource, PoolContents};
    use std::io::Write;

    /// USDC on Base: the numeraire, 6 decimals.
    const USDC: &str = "evm:0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
    /// ANYONE on Base: the dealt token, 18 decimals, WETH-quoted.
    const ANYONE: &str = "evm:0x9ff58f4ffb29fa2266ab25e75e2a8b3503311656";
    /// WETH on Base: an intermediate, never a token this node deals.
    const WETH: &str = "evm:0x4200000000000000000000000000000000000006";
    const POOL_ANYONE_WETH: &str = "0x1111111111111111111111111111111111111111";
    const POOL_WETH_USDC: &str = "0x2222222222222222222222222222222222222222";

    fn asset(text: &str) -> AssetId {
        text.parse::<AssetId>().expect("a declared asset")
    }

    fn rate(numerator: u64, denominator: u64) -> Rate {
        Rate::new(numerator, denominator).expect("a rate")
    }

    /// `seconds` after this test module's fixed start. Seconds, because
    /// every assertion here is about a `ttl` measured in them.
    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap() + TimeDelta::seconds(seconds)
    }

    /// A table dealing against USDC, with a two-minute `ttl` and a 5%
    /// `max_move`, dealing at the mid so that an assertion about a rate is
    /// about the observation rather than about the spread arithmetic
    /// `connector-domain` already has its own tests for.
    fn table_with(ttl_seconds: i64) -> SharedRateTable {
        SharedRateTable::new(RateTable::new(
            asset(USDC),
            Guards::new(
                Spread::none(),
                Ttl::new(TimeDelta::seconds(ttl_seconds)).expect("a positive ttl"),
                MaxMove::fraction(5, 100).expect("a max_move"),
            ),
        ))
    }

    /// A source holding the one pool ANYONE's price is read from, at `rate`
    /// dated `observed_at`.
    fn source_quoting(anyone_in_usdc: Rate, observed_at: DateTime<Utc>) -> Arc<InMemoryRateSource> {
        let source = Arc::new(InMemoryRateSource::new());
        source.insert_pool(
            PoolId(POOL_ANYONE_WETH.to_string()),
            PoolContents {
                base: asset(ANYONE),
                quote: asset(USDC),
                rate: anyone_in_usdc,
                observed_at,
                shortest_window: TimeDelta::seconds(60),
                longest_window: TimeDelta::seconds(3600),
            },
        );
        source
    }

    /// The one-leg path that reads that pool.
    fn direct_path() -> QuotePath {
        QuotePath::direct(QuoteLeg {
            pool: PoolId(POOL_ANYONE_WETH.to_string()),
            base: asset(ANYONE),
            quote: asset(USDC),
            window: TimeDelta::seconds(900),
        })
    }

    fn poller(table: &SharedRateTable, source: Arc<InMemoryRateSource>) -> RatePoller {
        RatePoller::new(table, source, asset(ANYONE), direct_path()).expect("a usable quote path")
    }

    #[tokio::test]
    async fn a_declared_quote_path_lands_a_fresh_rate_in_the_table() {
        let table = table_with(120);
        let source = source_quoting(rate(4, 1_000_000_000_000), at(0));

        poller(&table, source).refresh_once().await;

        let looked_up = table.lookup(&asset(ANYONE), &asset(USDC), at(60));
        assert!(looked_up.is_live(), "{looked_up:?}");
        assert_eq!(looked_up.rate(), Some(rate(4, 1_000_000_000_000)));
    }

    /// The cadence half of the same criterion: left running, the poller goes
    /// on reading, and a price that moves in the pool moves in the table
    /// without anybody calling anything.
    ///
    /// `start_paused` means tokio's timer advances the moment every task is
    /// idle, so this asserts the *schedule* rather than spending the `ttl`
    /// in real seconds.
    #[tokio::test(start_paused = true)]
    async fn the_poller_goes_on_refreshing_on_its_own_cadence() {
        let table = table_with(30);
        let source = source_quoting(rate(4, 1_000_000_000_000), at(0));
        let poller = poller(&table, Arc::clone(&source));
        let cadence = poller.cadence();
        assert_eq!(cadence, Duration::from_secs(10), "ttl / REFRESHES_PER_TTL");

        let running = tokio::spawn(poller.run());
        tokio::time::sleep(cadence / 2).await;
        assert_eq!(
            table.lookup(&asset(ANYONE), &asset(USDC), at(10)).rate(),
            Some(rate(4, 1_000_000_000_000)),
            "the first tick is immediate, so a booted node prices its pairs at once"
        );

        // The pool moves, within `max_move` of where it was.
        source.insert_pool(
            PoolId(POOL_ANYONE_WETH.to_string()),
            PoolContents {
                base: asset(ANYONE),
                quote: asset(USDC),
                rate: rate(41, 10_000_000_000_000),
                observed_at: at(5),
                shortest_window: TimeDelta::seconds(60),
                longest_window: TimeDelta::seconds(3600),
            },
        );
        tokio::time::sleep(cadence).await;

        assert_eq!(
            table.lookup(&asset(ANYONE), &asset(USDC), at(20)).rate(),
            Some(rate(41, 10_000_000_000_000)),
            "the next tick took the new reading, with nothing asking it to"
        );
        running.abort();
    }

    /// ADR 0071's "staleness is an outage, on purpose": a source that stops
    /// answering serves no last-good value past the `ttl` the operator set.
    #[tokio::test]
    async fn a_source_that_stops_answering_leaves_the_pair_to_go_stale() {
        let table = table_with(120);
        let source = source_quoting(rate(4, 1_000_000_000_000), at(0));
        let poller = poller(&table, Arc::clone(&source));
        poller.refresh_once().await;

        source.cut_off();
        poller.refresh_once().await;
        poller.refresh_once().await;

        assert!(
            table.lookup(&asset(ANYONE), &asset(USDC), at(60)).is_live(),
            "inside the ttl the last observation still deals"
        );
        let aged = table.lookup(&asset(ANYONE), &asset(USDC), at(180));
        assert!(
            matches!(aged, RateLookup::Stale { .. }),
            "past it the pair is down, not dealing on the price before the outage: {aged:?}"
        );
        assert_eq!(aged.rate(), None);
    }

    /// The `max_move` half of decision 5: the jump is refused, and what the
    /// pair held goes on ageing against its own `ttl` rather than being
    /// replaced or dropped.
    #[tokio::test]
    async fn a_refresh_outside_max_move_is_refused_and_the_previous_value_goes_on_ageing() {
        let table = table_with(120);
        let source = source_quoting(rate(4, 1_000_000_000_000), at(0));
        let poller = poller(&table, Arc::clone(&source));
        poller.refresh_once().await;

        // A flash-loaned pool: twice the price, far outside 5%, dated after
        // the reading it would replace.
        source.insert_pool(
            PoolId(POOL_ANYONE_WETH.to_string()),
            PoolContents {
                base: asset(ANYONE),
                quote: asset(USDC),
                rate: rate(8, 1_000_000_000_000),
                observed_at: at(60),
                shortest_window: TimeDelta::seconds(60),
                longest_window: TimeDelta::seconds(3600),
            },
        );
        poller.refresh_once().await;

        let held = table.lookup(&asset(ANYONE), &asset(USDC), at(60));
        assert_eq!(
            held.rate(),
            Some(rate(4, 1_000_000_000_000)),
            "the previous value stays in force"
        );
        assert_eq!(
            held.refused().map(|refused| refused.offered),
            Some(rate(8, 1_000_000_000_000)),
            "and the refusal is on the answer an operator already reads"
        );
        assert!(
            matches!(
                table.lookup(&asset(ANYONE), &asset(USDC), at(180)),
                RateLookup::Stale { .. }
            ),
            "ageing against its own ttl, which a refused refresh did not reset"
        );
    }

    #[test]
    fn a_short_ttl_still_polls_no_faster_than_the_floor() {
        assert_eq!(
            poll_interval(Ttl::new(TimeDelta::seconds(2)).expect("a positive ttl")),
            MINIMUM_POLL_INTERVAL
        );
        assert_eq!(
            poll_interval(Ttl::new(TimeDelta::seconds(300)).expect("a positive ttl")),
            Duration::from_secs(100)
        );
    }

    #[tokio::test]
    async fn a_quote_path_ending_anywhere_but_the_numeraire_is_refused() {
        let table = table_with(120);
        let refused = RatePoller::new(
            &table,
            Arc::new(InMemoryRateSource::new()),
            asset(ANYONE),
            QuotePath::direct(QuoteLeg {
                pool: PoolId(POOL_ANYONE_WETH.to_string()),
                base: asset(ANYONE),
                quote: asset(WETH),
                window: TimeDelta::seconds(900),
            }),
        );

        assert_eq!(
            refused.err(),
            Some(QuotePathUnusable::DoesNotEndAtNumeraire {
                token: asset(ANYONE),
                numeraire: asset(USDC),
                ends_at: asset(WETH),
            })
        );
    }

    // ---- from a real config file -------------------------------------
    //
    // The boot sequence itself: a file on disk, `Config::load`, the table
    // built from what it declares and a poller per declared quote path.
    // These live here rather than beside `SharedRateTable::from_config` so
    // that one fixture serves both halves of the same boot.

    /// A node dealing ANYONE against USDC: USDC the numeraire, ANYONE quoted
    /// through the two pools ADR 0071 decision 3 uses as its own example, a
    /// static row for the Solana-side pair that cannot self-source, and a
    /// `[rate_guards]` table over the lot.
    ///
    /// Returns the loaded config and the directory it was loaded from, which
    /// [`no_file_is_written_or_rewritten_by_a_refresh`] then watches.
    fn dealing_node() -> (Config, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp config dir");
        let key_file = dir.path().join("signer.key");
        std::fs::write(&key_file, [7u8; 32]).expect("write a raw 32-byte key");
        let state_dir = dir.path().join("state");
        std::fs::create_dir(&state_dir).expect("create a state dir");

        let config_file = dir.path().join("connector.toml");
        let mut file = std::fs::File::create(&config_file).expect("create the config file");
        write!(
            file,
            r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0xe7f1725e7734ce288f8367e1bb143e90bb3f0512"
token_address = "{usdc}"
decimals = 6

[settlement.evm.key]
key_file = "{key_file}"

[[tokens]]
asset = "evm:{usdc}"
numeraire = true

[[tokens]]
asset = "solana:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"

[[tokens]]
asset = "evm:{anyone}"
quote = [
  {{ pool = "{pool_anyone_weth}", quote_token = "evm:{weth}", twap_window_secs = 1800 }},
  {{ pool = "{pool_weth_usdc}", quote_token = "evm:{usdc}", twap_window_secs = 900 }},
]

[[rates]]
from = "solana:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
to = "evm:{anyone}"
rate = {{ numerator = 4000000000000, denominator = 1 }}

[[rates]]
from = "evm:{anyone}"
to = "evm:{usdc}"
ttl_secs = 60

[rate_guards]
spread = {{ numerator = 30, denominator = 10000 }}
ttl_secs = 300
max_move = {{ numerator = 5, denominator = 100 }}
"#,
            state_dir = state_dir.display(),
            key_file = key_file.display(),
            usdc = USDC.trim_start_matches("evm:"),
            anyone = ANYONE.trim_start_matches("evm:"),
            weth = WETH.trim_start_matches("evm:"),
            pool_anyone_weth = POOL_ANYONE_WETH,
            pool_weth_usdc = POOL_WETH_USDC,
        )
        .expect("write the config file");

        let config = Config::load(&config_file).expect("load the config");
        (config, dir)
    }

    /// The source that node's two-leg path reads: ANYONE priced in WETH,
    /// WETH priced in USDC, composing to ANYONE in USDC.
    fn two_leg_source() -> Arc<InMemoryRateSource> {
        let source = Arc::new(InMemoryRateSource::new());
        source.insert_pool(
            PoolId(POOL_ANYONE_WETH.to_string()),
            PoolContents {
                base: asset(ANYONE),
                quote: asset(WETH),
                rate: rate(1, 4000),
                observed_at: at(0),
                shortest_window: TimeDelta::seconds(60),
                longest_window: TimeDelta::seconds(3600),
            },
        );
        source.insert_pool(
            PoolId(POOL_WETH_USDC.to_string()),
            PoolContents {
                base: asset(WETH),
                quote: asset(USDC),
                rate: rate(3, 1_000_000_000),
                observed_at: at(0),
                shortest_window: TimeDelta::seconds(60),
                longest_window: TimeDelta::seconds(3600),
            },
        );
        source
    }

    #[test]
    fn the_table_is_built_from_what_the_file_declares() {
        let (config, _dir) = dealing_node();
        let table =
            SharedRateTable::from_config(config.denomination()).expect("a node that deals tokens");

        let held = table.read();
        assert_eq!(held.numeraire(), &asset(USDC));
        assert_eq!(
            held.lookup(
                &asset("solana:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"),
                &asset(ANYONE),
                at(0)
            )
            .rate(),
            // The declared row is a mid, and `[rate_guards]`'s 30/10000
            // spread comes off it on the way out.
            Some(rate(3_988_000_000_000, 1)),
            "the static [[rates]] row is in the table at boot"
        );
        assert_eq!(
            held.guards(&asset(ANYONE), &asset(USDC)).ttl(),
            Ttl::new(TimeDelta::seconds(60)).expect("a positive ttl"),
            "and the rate-less [[rates]] row tightened that pair's ttl over the node default"
        );
        assert_eq!(
            held.lookup(&asset(ANYONE), &asset(USDC), at(0)),
            RateLookup::NotDeclared,
            "a quoted pair holds nothing until its poller's first reading"
        );
    }

    #[tokio::test]
    async fn a_poller_per_declared_quote_path_prices_that_token() {
        let (config, _dir) = dealing_node();
        let table = SharedRateTable::from_config(config.denomination()).expect("a dealing node");
        let pollers = RatePoller::for_config(&table, two_leg_source(), config.denomination())
            .expect("usable quote paths");

        assert_eq!(pollers.len(), 1, "one token declared a quote path");
        assert_eq!(pollers[0].token(), &asset(ANYONE));
        assert_eq!(
            pollers[0].cadence(),
            Duration::from_secs(20),
            "the pair's own tightened 60s ttl, not the node's 300s default"
        );
        pollers[0].refresh_once().await;

        assert_eq!(
            table
                .read()
                .lookup(&asset(ANYONE), &asset(USDC), at(30))
                .rate(),
            // 1/4000 ANYONE per WETH composed with 3/10^12 WETH per USDC
            // base unit, less the node's 30/10000 spread.
            Some(rate(2991, 4_000_000_000_000_000)),
            "the two legs composed, and the spread came off the mid on the way out"
        );
    }

    /// A node that declares no token has no numeraire, so it has no table,
    /// no poller and no change in behaviour at all -- the easy path, and the
    /// one every node that predates ADR 0071 takes.
    #[test]
    fn a_node_that_declares_no_tokens_has_no_table_and_no_poller() {
        let dir = tempfile::tempdir().expect("temp config dir");
        let key_file = dir.path().join("signer.key");
        std::fs::write(&key_file, [7u8; 32]).expect("write a raw 32-byte key");
        let config_file = dir.path().join("connector.toml");
        std::fs::write(
            &config_file,
            format!(
                "client_edge_addr = \"127.0.0.1:0\"\n\n[signer]\nkey_file = \"{}\"\n",
                key_file.display()
            ),
        )
        .expect("write the config file");
        let config = Config::load(&config_file).expect("load the config");

        assert!(!config.denomination().declares_tokens());
        assert!(SharedRateTable::from_config(config.denomination()).is_none());
    }

    /// ADR 0009 is not disturbed: the config declares the *source*, and what
    /// a refresh produces is runtime state on the same footing as a chain
    /// balance. Nothing a poller does touches the file the operator
    /// deployed, or leaves anything of its own beside it.
    #[tokio::test]
    async fn no_file_is_written_or_rewritten_by_a_refresh() {
        let (config, dir) = dealing_node();
        let before = snapshot(dir.path());

        let table = SharedRateTable::from_config(config.denomination()).expect("a dealing node");
        let pollers = RatePoller::for_config(&table, two_leg_source(), config.denomination())
            .expect("usable quote paths");
        for poller in &pollers {
            poller.refresh_once().await;
            poller.refresh_once().await;
        }

        assert!(
            table
                .lookup(&asset(ANYONE), &asset(USDC), at(30))
                .rate()
                .is_some(),
            "the refreshes really happened"
        );
        assert_eq!(
            snapshot(dir.path()),
            before,
            "a refresh wrote no file, rewrote none, and left nothing behind"
        );
    }

    /// Every file under `root`, by path and by bytes -- so a rewrite that
    /// happened to preserve a length, or a file dropped into the state
    /// directory, is as visible as a change to the config itself.
    fn snapshot(root: &std::path::Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
        let mut found = std::collections::BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).expect("read a directory") {
                let entry = entry.expect("a directory entry");
                if entry.file_type().expect("a file type").is_dir() {
                    pending.push(entry.path());
                } else {
                    let bytes = std::fs::read(entry.path()).expect("read a file");
                    found.insert(entry.path(), bytes);
                }
            }
        }
        found
    }
}
