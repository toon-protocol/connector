//! The table a forwarding path reads to cross a denomination boundary, and the
//! three guards that decide what it answers
//! ([ADR 0071](../../../docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)
//! decision 5, issue #1289).
//!
//! A [`RateTable`] maps an **ordered** pair of [`AssetId`]s to the rate this
//! connector has declared between them, and answers a [`lookup`](RateTable::lookup)
//! with either that rate or **the reason it has none**. The two reasons are
//! separate answers, because they are separate facts about the path: a pair
//! this node has declared nothing for will never work and the sender should
//! route elsewhere, while a pair whose observation aged out is a live route
//! having a bad minute and is worth retrying. Collapsing them into one refusal
//! would tell every sender the wrong thing half the time.
//!
//! # The table has no clock
//!
//! This crate has no I/O and no clock (ADR 0001), so every freshness answer
//! here is a function of an instant the caller passes in -- the same
//! [`DateTime<Utc>`] `connector_runtime`'s `Clock::now` hands out, so the
//! poller that writes a refresh (issue #1294) and the operator read that
//! renders a state (issue #1297) hand this module the value they already
//! hold. Nothing in here reads wall time, and a test moves time by choosing a
//! different argument rather than by sleeping.
//!
//! # The three guards
//!
//! Each is declared per ordered pair over a node-level default
//! ([`Guards`] with a [`GuardOverride`] on top), so a thin pair can be
//! tightened without restating policy for every pair the node deals.
//!
//! * [`Spread`] -- the operator's dealing margin, applied against the sender.
//!   The table stores **mids**; the lookup applies the spread. Direction is
//!   the trade, so `X -> Y` and `Y -> X` are taken from the same mid and are
//!   not each other's reciprocal: a round trip loses the spread twice, which
//!   is exactly what a dealer is paid for.
//! * [`Ttl`] -- a rate not refreshed within it is dead, and the pair refuses
//!   until a fresh one lands. It never trades on the last price before the
//!   outage; staleness is an outage on purpose (ADR 0071's consequences say so
//!   in as many words), because the alternative is how a dealer is drained
//!   politely.
//! * [`MaxMove`] -- a refresh jumping past the bound is refused as probable
//!   manipulation, and the **previous value serves out its own ttl** rather
//!   than being replaced or dropped. Expiring the old value on one bad
//!   observation would let a single manipulated read take the pair down, which
//!   is the outage the guard exists to prevent.
//!
//! A `max_move` refusal is therefore *not* a lookup answer: the pair goes on
//! trading at the value it held. It is instead an annotation the same
//! [`RateLookup`] carries ([`RefusedRefresh`]), so the operator read that has
//! to tell "refused by `max_move`" from "simply aged out" reads it off the
//! answer the forwarding path already gets, rather than off a second surface
//! that could disagree with the first.
//!
//! # Composition
//!
//! Rates are declared per token against **one numeraire** (ADR 0071 decision
//! 3), so a cross rate is the composition `X -> Y = (X/numeraire) /
//! (Y/numeraire)`, which [`Rate::inverted`] expresses without a second
//! arithmetic. A token's quote *is* an ordered pair -- `(token, numeraire)` --
//! so the table needs one kind of row, not two, and a leg's own guards are the
//! guards of the pair it is.
//!
//! Every composed and spread-adjusted rate is rounded **down**, the
//! connector's way, exactly as [`crate::amount_after_rate_and_fee`] rounds the
//! conversion it is handed. The rounding can only ever understate what the
//! sender receives, never overstate it, which is what keeps the covering claim
//! the connector mints on the outgoing leg sufficient.
//!
//! # What this module will not invent
//!
//! There is no identity rate here, for `rate.rs`'s reason: ADR 0071 decision 2
//! makes silent conversion structurally impossible by making **absence** the
//! refusal, and a table that answered a same-asset pair with `1/1` would be
//! handing out the 1:1 rate nobody declared. A same-asset pair has no rate row
//! *by definition* and forwards unconverted at the flat fee; asking this table
//! about one gets [`RateLookup::NotDeclared`], and the forwarding path is
//! expected not to ask.
//!
//! A row for `X -> Y` is likewise not a row for `Y -> X`. An ordered pair is
//! ordered: the spread is taken against the sender in whichever direction the
//! packet runs, and an operator who declared one direction has declared one
//! direction.

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, TimeDelta, Utc};
use thiserror::Error;

use crate::asset::AssetId;
use crate::rate::Rate;

/// The part of a mid this connector keeps when it deals, as a fraction
/// strictly below one.
///
/// Applied **against the sender**: a mid of `m` under a spread of `n/d` deals
/// at `m * (d - n) / d`, so less of the outgoing token leaves per unit of the
/// incoming one.
///
/// A spread is a separate earnings stream from the flat fee, and a
/// non-dealing hop still earns only the fee (ADR 0071 decision 8): this
/// widens the *price*, and [`crate::amount_after_rate_and_fee`] subtracts the
/// fee afterwards. It is spelled as a **fraction** rather than as a
/// percentage or a basis point for the reason ADR 0010 deleted the
/// basis-point fee: there are no floats on this path, and an operator who
/// deals at half a basis point writes `1/20000` rather than watching it round
/// to zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Spread {
    numerator: u64,
    denominator: u64,
}

impl Spread {
    /// Deal at the mid. A real declaration, unlike a 1:1 *rate*: a node may
    /// choose to earn only its flat fee on a crossing, and saying so costs
    /// nothing and hides nothing.
    pub const fn none() -> Spread {
        Spread {
            numerator: 0,
            denominator: 1,
        }
    }

    /// `numerator` parts in `denominator` kept from every mid.
    ///
    /// Refuses a zero denominator, and refuses a fraction at or above one: a
    /// spread of the whole mid leaves nothing to forward, and [`Rate`] has no
    /// zero numerator to express it with anyway (a rate that buys nothing is
    /// one a reject cannot convert a cost back through).
    pub fn fraction(numerator: u64, denominator: u64) -> Result<Spread, GuardError> {
        if denominator == 0 {
            return Err(GuardError::ZeroSpreadDenominator { numerator });
        }
        if numerator >= denominator {
            return Err(GuardError::SpreadAtOrAboveOne {
                numerator,
                denominator,
            });
        }
        Ok(Spread {
            numerator,
            denominator,
        })
    }

    /// The kept part of the fraction.
    pub const fn numerator(&self) -> u64 {
        self.numerator
    }

    /// The whole the fraction is taken from. Never zero.
    pub const fn denominator(&self) -> u64 {
        self.denominator
    }

    /// Whether this connector deals at the mid on this pair.
    pub const fn is_none(&self) -> bool {
        self.numerator == 0
    }

    /// `mid` less this spread, rounded **down** -- the connector's way, the
    /// way every other figure on the value path rounds.
    ///
    /// `None` when the answer is smaller than the smallest rate a [`Rate`] can
    /// spell. That takes a mid already below roughly `2^-64`, which no real
    /// pair of tokens reaches, and the honest answer there is the one
    /// [`Rate::new`] gives a zero numerator: a value that cannot be inverted
    /// is not a rate, and rounding *up* to the smallest one that can would be
    /// the first figure on this path to round the sender's way.
    pub fn applied_to(&self, mid: Rate) -> Option<Rate> {
        if self.is_none() {
            return Some(mid);
        }
        rate_at_most(
            u128::from(mid.numerator()) * u128::from(self.denominator - self.numerator),
            u128::from(mid.denominator()) * u128::from(self.denominator),
        )
    }
}

impl fmt::Display for Spread {
    /// `1/200` -- both halves, for [`Rate`]'s reason: the pair is the
    /// declaration, and one number would be a number in nothing.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

/// How far one refresh may move a pair's rate before this connector treats it
/// as probable manipulation rather than as a market.
///
/// A fraction of the value in force, symmetric in both directions: a bound of
/// `n/d` admits a refreshed rate `r` against a standing rate `p` exactly when
/// `|r - p| <= (n/d) * p`.
///
/// A zero numerator is allowed and means *pinned* -- only the same value is
/// ever accepted -- which is a coherent thing for an operator to declare about
/// a par pair. A fraction at or above one is allowed too, and is simply a wide
/// bound; ADR 0071 gives the operator the knob and does not tell them where to
/// set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MaxMove {
    numerator: u64,
    denominator: u64,
}

impl MaxMove {
    /// A bound of `numerator` parts in `denominator` of the value in force.
    /// Refuses only a zero denominator, which is not a fraction.
    pub fn fraction(numerator: u64, denominator: u64) -> Result<MaxMove, GuardError> {
        if denominator == 0 {
            return Err(GuardError::ZeroMaxMoveDenominator { numerator });
        }
        Ok(MaxMove {
            numerator,
            denominator,
        })
    }

    /// How much of the standing value the bound allows.
    pub const fn numerator(&self) -> u64 {
        self.numerator
    }

    /// The whole the bound is a fraction of. Never zero.
    pub const fn denominator(&self) -> u64 {
        self.denominator
    }

    /// Whether a refresh to `refreshed` is inside this bound of `previous`.
    ///
    /// `|refreshed - previous| * denominator <= numerator * previous`, which
    /// cross-multiplied over `refreshed.denominator * previous.denominator *
    /// self.denominator` -- all three positive by construction -- is
    /// `|rn*pd - pn*rd| * d <= n * pn * rd`. Both sides need 192 bits, one
    /// more than `u128` holds, so they are compared as `(high, low)` pairs
    /// rather than approximated; see [`wide_mul`].
    pub fn admits(&self, previous: Rate, refreshed: Rate) -> bool {
        let refreshed_side = u128::from(refreshed.numerator()) * u128::from(previous.denominator());
        let previous_side = u128::from(previous.numerator()) * u128::from(refreshed.denominator());
        let moved = refreshed_side.abs_diff(previous_side);
        let allowed = u128::from(self.numerator) * u128::from(previous.numerator());

        wide_mul(moved, self.denominator) <= wide_mul(allowed, refreshed.denominator())
    }
}

impl fmt::Display for MaxMove {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

/// How long an observation stays alive. Strictly positive: a ttl of no time at
/// all is a pair that is dead the instant it is written, which is a
/// misconfiguration rather than a policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ttl(TimeDelta);

impl Ttl {
    /// Refuses a zero or negative span.
    pub fn new(within: TimeDelta) -> Result<Ttl, GuardError> {
        if within <= TimeDelta::zero() {
            return Err(GuardError::NonPositiveTtl { ttl: within });
        }
        Ok(Ttl(within))
    }

    /// The span itself, for arithmetic against an instant.
    pub const fn as_time_delta(&self) -> TimeDelta {
        self.0
    }
}

impl fmt::Display for Ttl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The three guards in force for one ordered pair: a complete set, which is
/// what a node declares once as its default and what
/// [`RateTable::guards`] answers for any pair after the pair's own
/// [`GuardOverride`] has been laid over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guards {
    spread: Spread,
    ttl: Ttl,
    max_move: MaxMove,
}

impl Guards {
    /// All three, already validated -- each guard's own constructor is where a
    /// bad value is refused, so assembling three good ones cannot fail.
    pub const fn new(spread: Spread, ttl: Ttl, max_move: MaxMove) -> Guards {
        Guards {
            spread,
            ttl,
            max_move,
        }
    }

    /// The dealing margin taken against the sender.
    pub const fn spread(&self) -> Spread {
        self.spread
    }

    /// How long an observation on this pair stays alive.
    pub const fn ttl(&self) -> Ttl {
        self.ttl
    }

    /// How far one refresh may move this pair's rate.
    pub const fn max_move(&self) -> MaxMove {
        self.max_move
    }

    /// These guards with whatever `over` names replaced, and everything it
    /// leaves unset kept. Field by field, deliberately: an operator tightening
    /// one pair's `ttl` has not thereby dropped the node's `max_move`.
    pub fn overridden_by(&self, over: &GuardOverride) -> Guards {
        Guards {
            spread: over.spread.unwrap_or(self.spread),
            ttl: over.ttl.unwrap_or(self.ttl),
            max_move: over.max_move.unwrap_or(self.max_move),
        }
    }
}

/// What one ordered pair says differently from the node default. Every field
/// unset -- [`Default`] -- is a pair that takes the node's policy whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GuardOverride {
    /// This pair's own dealing margin.
    pub spread: Option<Spread>,
    /// This pair's own freshness window.
    pub ttl: Option<Ttl>,
    /// This pair's own manipulation bound.
    pub max_move: Option<MaxMove>,
}

/// Where a live rate came from, which is what decides whether it can die.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// A static row an operator tends by hand -- ADR 0009's immutable
    /// declaration, in force for the process lifetime. Nothing refreshes it,
    /// so `ttl` has nothing to measure and never kills it. A declared row that
    /// expired would take a pair down with no way to bring it back short of a
    /// redeploy, which is the opposite of what the guard is for.
    Declared,
    /// An observation the poller wrote at this instant, ageing against its
    /// pair's [`Ttl`].
    Observed(DateTime<Utc>),
}

/// A refresh [`MaxMove`] turned away, kept so an operator can tell a guarded
/// pair from a quiet one.
///
/// Orthogonal to whether the pair is live: the refusal leaves the previous
/// value in place *and ageing*, so a pair carrying one of these may be trading
/// normally, or may since have aged out on its own `ttl`. Both readings matter
/// and neither replaces the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusedRefresh {
    /// The rate the refresh offered, which the table did not take.
    pub offered: Rate,
    /// When it was offered.
    pub at: DateTime<Utc>,
}

/// What the table answers about one ordered pair as of one instant.
///
/// Three answers, and the two refusals are separate on purpose: ADR 0071
/// decision 2 refuses a cross-token forward with no declared rate, and the
/// sender's next move differs between "this node does not deal this pair" and
/// "this node deals it and is currently blind". The forwarding path (issue
/// #1295) turns them into two reject codes, and the operator read (issue
/// #1297) renders the same three answers plus [`refused`](Self::refused).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLookup {
    /// In force. `rate` is what a forward converts at *right now*: composed
    /// through the numeraire where it had to be, with this pair's spread
    /// already taken, rounded down at every step.
    Live {
        /// The rate to convert at, spread applied.
        rate: Rate,
        /// Whether it was declared or observed, and when it was last observed.
        freshness: Freshness,
        /// A refresh `max_move` turned away since the last accepted one.
        refused: Option<RefusedRefresh>,
    },
    /// Declared, but the observation behind it aged past its `ttl` -- or, for
    /// a composed pair, at least one of its legs did. The pair refuses until a
    /// fresh observation lands, and never trades on the last price before the
    /// outage.
    Stale {
        /// When the stalest thing behind this answer was last observed.
        observed_at: DateTime<Utc>,
        /// A refresh `max_move` turned away, if this answer came from a single
        /// row. Always `None` for a composed pair; the refusal is visible on
        /// the leg it happened to.
        refused: Option<RefusedRefresh>,
    },
    /// No rate is declared for this ordered pair, and none is composable from
    /// the numeraire. Includes a same-asset pair, which has no rate row by
    /// definition and needs none.
    NotDeclared,
}

impl RateLookup {
    /// The rate to convert at, or `None` for either refusal. A convenience for
    /// a caller that has already decided what to do about a refusal.
    pub const fn rate(&self) -> Option<Rate> {
        match self {
            RateLookup::Live { rate, .. } => Some(*rate),
            RateLookup::Stale { .. } | RateLookup::NotDeclared => None,
        }
    }

    /// Whether a forward may cross this pair as of the instant asked about.
    pub const fn is_live(&self) -> bool {
        matches!(self, RateLookup::Live { .. })
    }

    /// The refresh `max_move` turned away, if the table is holding one for
    /// this pair.
    pub const fn refused(&self) -> Option<RefusedRefresh> {
        match self {
            RateLookup::Live { refused, .. } | RateLookup::Stale { refused, .. } => *refused,
            RateLookup::NotDeclared => None,
        }
    }
}

/// What became of one call to [`RateTable::refresh`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// Written. The pair now holds this rate, observed at the instant given,
    /// and any earlier [`RefusedRefresh`] is cleared -- the pair is healthy
    /// again and an operator looking at it should see that.
    Accepted,
    /// Outside `max_move`. The previous value stays in place and goes on
    /// ageing against its own `ttl`, which is the whole point of the guard:
    /// one manipulated observation must not become an immediate outage.
    OutsideMaxMove {
        /// The value that stays in force.
        previous: Rate,
    },
    /// The pair runs a static row an operator tends by hand. Nothing observed
    /// replaces a declaration (ADR 0009: what changes at runtime is an
    /// observation, never a declaration), so this is a refusal and not a
    /// silent no-op -- a poller quoting a pair the operator has pinned is a
    /// misconfiguration worth a log line.
    Declared {
        /// The declared value that stays in force.
        standing: Rate,
    },
}

/// The rates this connector has declared, and the guards they are read under.
///
/// Holds **mids**: what the lookup answers has the pair's spread taken off it,
/// so the same stored value serves both the price a forward converts at and
/// the comparison `max_move` makes against the next refresh. A table that
/// stored post-spread rates would compare a widened price against a widened
/// price and call a spread change a market move.
///
/// A node that declares no tokens has no numeraire and therefore no table at
/// all; the runtime holds an `Option<RateTable>` and a node without one
/// forwards exactly as it does today.
#[derive(Debug, Clone)]
pub struct RateTable {
    numeraire: AssetId,
    defaults: Guards,
    /// `from -> to -> row`, nested rather than keyed by a `(AssetId, AssetId)`
    /// tuple so the forwarding path can look a pair up from two borrows
    /// without cloning either.
    rows: BTreeMap<AssetId, BTreeMap<AssetId, Row>>,
    overrides: BTreeMap<AssetId, BTreeMap<AssetId, GuardOverride>>,
}

/// One ordered pair's entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Row {
    held: Held,
    refused: Option<RefusedRefresh>,
}

/// The rate a row holds, and how it got there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    Declared(Rate),
    Observed { rate: Rate, at: DateTime<Utc> },
}

impl RateTable {
    /// An empty table dealing against `numeraire` under `defaults`. Every pair
    /// answers [`RateLookup::NotDeclared`] until a row is put in it.
    pub fn new(numeraire: AssetId, defaults: Guards) -> RateTable {
        RateTable {
            numeraire,
            defaults,
            rows: BTreeMap::new(),
            overrides: BTreeMap::new(),
        }
    }

    /// The one token every quote is taken against. Mixing numeraires is
    /// refused at boot (ADR 0071 decision 3), so a table has exactly one.
    pub fn numeraire(&self) -> &AssetId {
        &self.numeraire
    }

    /// The node-level guards, before any pair's own override.
    pub const fn defaults(&self) -> Guards {
        self.defaults
    }

    /// Tighten one ordered pair. A later call replaces an earlier one whole,
    /// rather than merging into it: the override *is* what this pair says
    /// differently, and two half-overrides for one pair would be two
    /// declarations of one fact.
    pub fn set_guards(&mut self, from: AssetId, to: AssetId, over: GuardOverride) {
        self.overrides.entry(from).or_default().insert(to, over);
    }

    /// The guards in force for one ordered pair: the node defaults with this
    /// pair's override laid over them.
    pub fn guards(&self, from: &AssetId, to: &AssetId) -> Guards {
        match self.overrides.get(from).and_then(|tos| tos.get(to)) {
            Some(over) => self.defaults.overridden_by(over),
            None => self.defaults,
        }
    }

    /// Put a **static row** in the table: a rate the operator tends by hand,
    /// for a pair that cannot self-source, which also overrides composition
    /// through the numeraire (ADR 0071 decision 3).
    ///
    /// It never goes stale and nothing observed replaces it. Replaces whatever
    /// the pair held, including a refusal: a declaration is the operator
    /// speaking, and the guard's job was to hold a line until they did.
    pub fn declare(&mut self, from: AssetId, to: AssetId, rate: Rate) {
        self.rows.entry(from).or_default().insert(
            to,
            Row {
                held: Held::Declared(rate),
                refused: None,
            },
        );
    }

    /// Write an observation for one ordered pair, as of `at`, under that
    /// pair's `max_move`.
    ///
    /// The first observation for a pair has nothing to move away from and is
    /// always taken. A later one is taken only if it is inside the bound; if
    /// it is not, the previous value stays *and keeps its own observation
    /// instant*, so it goes on ageing towards its `ttl` exactly as it would
    /// have. That is the difference between a guard and an outage: a refused
    /// refresh buys a human time to look, it does not itself take the pair
    /// down.
    pub fn refresh(
        &mut self,
        from: AssetId,
        to: AssetId,
        rate: Rate,
        at: DateTime<Utc>,
    ) -> Refresh {
        let max_move = self.guards(&from, &to).max_move();
        let tos = self.rows.entry(from).or_default();

        let Some(row) = tos.get_mut(&to) else {
            // Nothing to have moved away from.
            tos.insert(
                to,
                Row {
                    held: Held::Observed { rate, at },
                    refused: None,
                },
            );
            return Refresh::Accepted;
        };

        match row.held {
            Held::Declared(standing) => Refresh::Declared { standing },
            Held::Observed { rate: previous, .. } => {
                if max_move.admits(previous, rate) {
                    row.held = Held::Observed { rate, at };
                    row.refused = None;
                    Refresh::Accepted
                } else {
                    row.refused = Some(RefusedRefresh { offered: rate, at });
                    Refresh::OutsideMaxMove { previous }
                }
            }
        }
    }

    /// Every ordered pair the table holds a row for, in a stable order.
    ///
    /// These are the rows an operator declared or a poller wrote -- token
    /// quotes (`(token, numeraire)`) and static pair rows. The cross pairs
    /// composable from them are not listed, because a composition is derived
    /// and a refusal is not: a `max_move` refusal happens to a *row*, and an
    /// operator asking which pair took the node down wants the leg it happened
    /// to rather than the same news repeated across every cross pair that
    /// leans on it.
    pub fn declared_pairs(&self) -> impl Iterator<Item = (&AssetId, &AssetId)> {
        self.rows
            .iter()
            .flat_map(|(from, tos)| tos.keys().map(move |to| (from, to)))
    }

    /// What this connector converts `from -> to` at as of `now`, or why it
    /// will not.
    ///
    /// A row for the ordered pair is the whole answer. Failing that, the pair
    /// composes through the numeraire -- `X -> Y = (X/numeraire) /
    /// (Y/numeraire)` -- and is live only while **both** legs are, each
    /// against its own pair's `ttl`. The spread taken is the `from -> to`
    /// pair's own, applied once on top of the composition rather than once per
    /// leg: a cross rate is one trade, not two.
    pub fn lookup(&self, from: &AssetId, to: &AssetId, now: DateTime<Utc>) -> RateLookup {
        let guards = self.guards(from, to);

        if let Some(row) = self.row(from, to) {
            return read_row(row, guards, now);
        }

        // A same-asset pair has no rate row by definition (ADR 0071 decision
        // 2) and this table will not invent the 1:1 that would let one
        // through.
        if from == to {
            return RateLookup::NotDeclared;
        }

        let (mid, freshness) = match (self.leg(from, now), self.leg(to, now)) {
            // A leg with no row at all: the pair is not composable, and saying
            // "stale" about the other leg would tell the sender to retry
            // something that will never work.
            (Leg::Missing, _) | (_, Leg::Missing) => return RateLookup::NotDeclared,
            // Staleness on either leg is staleness for the pair, reported
            // against whichever observation is oldest.
            (Leg::Stale(left), Leg::Stale(right)) => {
                return RateLookup::Stale {
                    observed_at: left.min(right),
                    refused: None,
                }
            }
            (Leg::Stale(observed_at), _) | (_, Leg::Stale(observed_at)) => {
                return RateLookup::Stale {
                    observed_at,
                    refused: None,
                }
            }
            // Both sides the numeraire is the same asset twice, which the
            // check above already answered; the arm is here so this match
            // needs no catch-all to hide a case behind.
            (Leg::Numeraire, Leg::Numeraire) => return RateLookup::NotDeclared,
            // `from` is the numeraire, so the composition is the other leg
            // read backwards: numeraire -> Y is the inverse of Y -> numeraire.
            (Leg::Numeraire, Leg::Quoted { rate, freshness }) => (rate.inverted(), freshness),
            // `to` is the numeraire, which means a row for this pair *is* the
            // quote and the direct lookup above would have found it. There is
            // nothing left to compose from.
            (Leg::Quoted { .. }, Leg::Numeraire) => return RateLookup::NotDeclared,
            (
                Leg::Quoted {
                    rate: quoted_from,
                    freshness: left,
                },
                Leg::Quoted {
                    rate: quoted_to,
                    freshness: right,
                },
            ) => {
                let inverted = quoted_to.inverted();
                let composed = rate_at_most(
                    u128::from(quoted_from.numerator()) * u128::from(inverted.numerator()),
                    u128::from(quoted_from.denominator()) * u128::from(inverted.denominator()),
                );
                match composed {
                    Some(composed) => (composed, older(left, right)),
                    None => return RateLookup::NotDeclared,
                }
            }
        };

        match guards.spread().applied_to(mid) {
            Some(rate) => RateLookup::Live {
                rate,
                freshness,
                refused: None,
            },
            None => RateLookup::NotDeclared,
        }
    }

    fn row(&self, from: &AssetId, to: &AssetId) -> Option<&Row> {
        self.rows.get(from).and_then(|tos| tos.get(to))
    }

    /// One side of a composition: the row quoting `token` against the
    /// numeraire, read as of `now` under that row's own guards.
    fn leg(&self, token: &AssetId, now: DateTime<Utc>) -> Leg {
        if token == &self.numeraire {
            return Leg::Numeraire;
        }
        let Some(row) = self.row(token, &self.numeraire) else {
            return Leg::Missing;
        };
        match row.held {
            Held::Declared(rate) => Leg::Quoted {
                rate,
                freshness: Freshness::Declared,
            },
            Held::Observed { rate, at } => {
                let ttl = self.guards(token, &self.numeraire).ttl();
                if is_stale(at, ttl, now) {
                    Leg::Stale(at)
                } else {
                    Leg::Quoted {
                        rate,
                        freshness: Freshness::Observed(at),
                    }
                }
            }
        }
    }
}

/// One leg of a cross rate, as of an instant.
#[derive(Debug, Clone, Copy)]
enum Leg {
    /// The token *is* the numeraire. Not a rate, and not one this module
    /// invents: it is the absence of a conversion on that side.
    Numeraire,
    Quoted {
        rate: Rate,
        freshness: Freshness,
    },
    Stale(DateTime<Utc>),
    Missing,
}

/// A single row's answer: the same three, with the spread applied and the
/// row's own refusal carried through.
fn read_row(row: &Row, guards: Guards, now: DateTime<Utc>) -> RateLookup {
    let (mid, freshness) = match row.held {
        Held::Declared(rate) => (rate, Freshness::Declared),
        Held::Observed { rate, at } => {
            if is_stale(at, guards.ttl(), now) {
                return RateLookup::Stale {
                    observed_at: at,
                    refused: row.refused,
                };
            }
            (rate, Freshness::Observed(at))
        }
    };

    match guards.spread().applied_to(mid) {
        Some(rate) => RateLookup::Live {
            rate,
            freshness,
            refused: row.refused,
        },
        None => RateLookup::NotDeclared,
    }
}

/// Whether an observation taken at `observed_at` has aged past `ttl` as of
/// `now`.
///
/// The boundary is [`crate::is_expired`]'s, and for the same reason: a rate is
/// alive strictly *inside* its window, so an observation exactly one `ttl` old
/// is already dead. Choosing the other boundary would make the guard one
/// instant weaker than the operator wrote, at every pair, forever.
fn is_stale(observed_at: DateTime<Utc>, ttl: Ttl, now: DateTime<Utc>) -> bool {
    now.signed_duration_since(observed_at) >= ttl.as_time_delta()
}

/// Which of two legs will expire first, and so which one a composed pair
/// reports as its freshness. A declared leg never expires, so an observed one
/// always wins over it.
fn older(left: Freshness, right: Freshness) -> Freshness {
    match (left, right) {
        (Freshness::Declared, other) | (other, Freshness::Declared) => other,
        (Freshness::Observed(left), Freshness::Observed(right)) => {
            Freshness::Observed(left.min(right))
        }
    }
}

/// A [`Rate`] that is **never more than** the exact ratio
/// `numerator / denominator`.
///
/// Exact whenever the reduced terms fit two `u64`s, which is every composition
/// of rates an operator would actually declare. When they do not -- two rates
/// folding a `10^12` decimals gap into each other reach `10^26` in the
/// numerator alone -- the answer is the largest rate still under the exact
/// one: `u64::MAX / 1` when the value itself is past what a [`Rate`] can hold,
/// and otherwise the terms shifted down together, the numerator rounding
/// **down** and the denominator **up**, so the value can only fall. That keeps
/// ADR 0071 decision 4's promise at the one place it is easy to lose: a
/// composition that rounded up would deliver more on the outgoing leg than the
/// connector's own covering claim was minted for.
///
/// `None` when the value has fallen below the smallest rate `Rate` can spell,
/// which is [`Rate::new`]'s zero-numerator refusal reached by another road.
fn rate_at_most(numerator: u128, denominator: u128) -> Option<Rate> {
    const CEILING: u128 = u64::MAX as u128;

    if numerator == 0 || denominator == 0 {
        return None;
    }
    let divisor = gcd(numerator, denominator);
    let (mut numerator, mut denominator) = (numerator / divisor, denominator / divisor);

    // A value at or past the largest rate there is. Clamping to `u64::MAX / 1`
    // is still a rate under the exact one, and it is the closest such rate;
    // shifting instead would answer something needlessly far below it.
    if numerator / denominator >= CEILING {
        return Rate::new(u64::MAX, 1).ok();
    }

    while numerator > CEILING || denominator > CEILING {
        // At least one bit, since the larger half is past `u64`'s range.
        let shift = numerator.max(denominator).ilog2() - 63;
        numerator >>= shift;
        denominator = denominator.div_ceil(1u128 << shift);
        if numerator == 0 {
            return None;
        }
    }

    Rate::new(
        u64::try_from(numerator).ok()?,
        u64::try_from(denominator).ok()?,
    )
    .ok()
}

/// `wide * narrow` as a `(high, low)` pair of `u128`s, so that two such
/// products can be compared exactly.
///
/// [`MaxMove::admits`] multiplies a `u128` difference by a `u64` bound on both
/// sides, which needs 192 bits. `u128` is the widest integer Rust has, and
/// saturating or approximating here would make the guard admit or refuse a
/// move on the strength of an arithmetic artefact rather than of the
/// operator's bound. Tuples compare lexicographically, which is exactly the
/// comparison these two limbs want.
fn wide_mul(wide: u128, narrow: u64) -> (u128, u128) {
    let narrow = u128::from(narrow);
    let high_half = wide >> 64;
    let low_half = wide & u128::from(u64::MAX);

    let low_product = low_half * narrow;
    let high_product = high_half * narrow;

    let (low, carried) = low_product.overflowing_add(high_product << 64);
    let high = (high_product >> 64) + u128::from(carried);
    (high, low)
}

/// Euclid, on `u128`. `rate.rs` has the same three lines over `u64`; a
/// composition's intermediates are `u128`, and neither module exports its
/// arithmetic to the other.
fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

/// What can be wrong with a declared guard. Each variant names the value that
/// was written, so a boot refusal points at the row.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GuardError {
    /// A fraction with no whole to be a fraction of.
    #[error("a spread is a fraction, and '{numerator}/0' is not one")]
    ZeroSpreadDenominator { numerator: u64 },

    /// A spread of the whole mid, or more, leaves nothing to forward.
    #[error(
        "a spread is the part of a mid this connector keeps, so it is less than the whole of it, \
         and '{numerator}/{denominator}' is not -- a pair dealt at or above its own mid forwards \
         nothing"
    )]
    SpreadAtOrAboveOne { numerator: u64, denominator: u64 },

    /// A fraction with no whole to be a fraction of.
    #[error("a max_move is a fraction, and '{numerator}/0' is not one")]
    ZeroMaxMoveDenominator { numerator: u64 },

    /// A window that is over before it starts.
    #[error(
        "a ttl is how long an observed rate stays alive, and '{ttl}' is no time at all -- a pair \
         under it would be stale the instant its rate was written"
    )]
    NonPositiveTtl { ttl: TimeDelta },
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use proptest::prelude::*;

    /// One 6-decimals USDC base unit in 18-decimals ANYONE base units, at four
    /// ANYONE to the USDC -- the decimals gap and the market price folded into
    /// one ratio, as `fee.rs` and `rate.rs` both use it.
    const USDC_TO_ANYONE: u64 = 4_000_000_000_000;

    fn usdc() -> AssetId {
        AssetId::evm("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48")
    }

    fn anyone() -> AssetId {
        AssetId::evm("0x1111111111111111111111111111111111111111")
    }

    fn weth() -> AssetId {
        AssetId::evm("0x2222222222222222222222222222222222222222")
    }

    fn rate(numerator: u64, denominator: u64) -> Rate {
        Rate::new(numerator, denominator).expect("a rate with two non-zero halves")
    }

    fn ttl(seconds: i64) -> Ttl {
        Ttl::new(TimeDelta::seconds(seconds)).expect("a positive span")
    }

    fn max_move(numerator: u64, denominator: u64) -> MaxMove {
        MaxMove::fraction(numerator, denominator).expect("a fraction with a whole")
    }

    fn spread(numerator: u64, denominator: u64) -> Spread {
        Spread::fraction(numerator, denominator).expect("a fraction below one")
    }

    /// No spread, a minute of life, a tenth of movement allowed.
    fn defaults() -> Guards {
        Guards::new(Spread::none(), ttl(60), max_move(1, 10))
    }

    /// `second` seconds after a fixed instant. Added rather than spelled into
    /// the timestamp, so a test may reach past a minute without the calendar
    /// having an opinion about it.
    fn at(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap() + TimeDelta::seconds(i64::from(second))
    }

    /// A table dealing against USDC, with ANYONE quoted at one observation.
    fn quoted(mid: Rate, observed: DateTime<Utc>) -> RateTable {
        let mut table = RateTable::new(usdc(), defaults());
        table.refresh(anyone(), usdc(), mid, observed);
        table
    }

    #[test]
    fn an_undeclared_pair_is_not_declared() {
        let table = RateTable::new(usdc(), defaults());
        assert_eq!(
            table.lookup(&usdc(), &anyone(), at(0)),
            RateLookup::NotDeclared
        );
    }

    #[test]
    fn a_rate_past_its_ttl_is_stale_rather_than_undeclared() {
        // The whole point of two refusals: the same pair answers differently
        // depending on whether this node deals it at all.
        let table = quoted(rate(1, USDC_TO_ANYONE), at(0));

        assert!(table.lookup(&anyone(), &usdc(), at(59)).is_live());
        assert_eq!(
            table.lookup(&anyone(), &usdc(), at(60)),
            RateLookup::Stale {
                observed_at: at(0),
                refused: None,
            }
        );
        assert_eq!(
            table.lookup(&weth(), &usdc(), at(59)),
            RateLookup::NotDeclared
        );
    }

    #[test]
    fn the_ttl_boundary_is_the_expiry_boundary() {
        // Alive strictly inside the window, exactly as a packet fulfils
        // strictly before its deadline.
        let table = quoted(rate(3, 2), at(0));
        assert!(table.lookup(&anyone(), &usdc(), at(59)).is_live());
        assert!(!table.lookup(&anyone(), &usdc(), at(60)).is_live());
    }

    #[test]
    fn a_refresh_outside_max_move_leaves_the_previous_value_ageing() {
        // The acceptance criterion this module exists for: a bad observation
        // must not become an immediate outage, and must not buy the old value
        // a new lease either.
        let mut table = quoted(rate(100, 1), at(0));

        let verdict = table.refresh(anyone(), usdc(), rate(500, 1), at(30));
        assert_eq!(
            verdict,
            Refresh::OutsideMaxMove {
                previous: rate(100, 1)
            }
        );

        // Still trading, at the old value, with the old observation instant.
        assert_eq!(
            table.lookup(&anyone(), &usdc(), at(30)).rate(),
            Some(rate(100, 1))
        );
        assert!(matches!(
            table.lookup(&anyone(), &usdc(), at(30)),
            RateLookup::Live {
                freshness: Freshness::Observed(observed),
                ..
            } if observed == at(0)
        ));

        // And ageing against its own ttl, from when it was observed rather
        // than from when the bad refresh arrived.
        assert!(!table.lookup(&anyone(), &usdc(), at(60)).is_live());
    }

    #[test]
    fn a_refused_refresh_is_visible_while_the_pair_still_trades() {
        // The second reader (issue #1297) sees the refusal on the same answer
        // the forwarding path reads, so the two can never disagree.
        let mut table = quoted(rate(100, 1), at(0));
        table.refresh(anyone(), usdc(), rate(500, 1), at(30));

        let answer = table.lookup(&anyone(), &usdc(), at(30));
        assert!(answer.is_live());
        assert_eq!(
            answer.refused(),
            Some(RefusedRefresh {
                offered: rate(500, 1),
                at: at(30),
            })
        );
    }

    #[test]
    fn a_pair_refused_by_max_move_is_distinguishable_from_one_that_aged_out() {
        let mut guarded = quoted(rate(100, 1), at(0));
        guarded.refresh(anyone(), usdc(), rate(500, 1), at(30));
        let quiet = quoted(rate(100, 1), at(0));

        // Both are stale at the same instant, and only one of them was
        // refused.
        assert_eq!(
            guarded.lookup(&anyone(), &usdc(), at(90)),
            RateLookup::Stale {
                observed_at: at(0),
                refused: Some(RefusedRefresh {
                    offered: rate(500, 1),
                    at: at(30),
                }),
            }
        );
        assert_eq!(
            quiet.lookup(&anyone(), &usdc(), at(90)),
            RateLookup::Stale {
                observed_at: at(0),
                refused: None,
            }
        );
    }

    #[test]
    fn an_accepted_refresh_clears_the_refusal() {
        let mut table = quoted(rate(100, 1), at(0));
        table.refresh(anyone(), usdc(), rate(500, 1), at(10));
        assert_eq!(
            table.refresh(anyone(), usdc(), rate(105, 1), at(20)),
            Refresh::Accepted
        );

        let answer = table.lookup(&anyone(), &usdc(), at(20));
        assert_eq!(answer.rate(), Some(rate(105, 1)));
        assert_eq!(answer.refused(), None);
    }

    #[test]
    fn a_first_observation_has_nothing_to_move_away_from() {
        let mut table =
            RateTable::new(usdc(), Guards::new(Spread::none(), ttl(60), max_move(0, 1)));
        // A pinned pair still takes its first value.
        assert_eq!(
            table.refresh(anyone(), usdc(), rate(7, 3), at(0)),
            Refresh::Accepted
        );
        assert_eq!(
            table.refresh(anyone(), usdc(), rate(8, 3), at(1)),
            Refresh::OutsideMaxMove {
                previous: rate(7, 3)
            }
        );
        // ... and the same value again, which has moved by nothing.
        assert_eq!(
            table.refresh(anyone(), usdc(), rate(7, 3), at(2)),
            Refresh::Accepted
        );
    }

    #[test]
    fn the_spread_makes_the_two_directions_different_prices() {
        // One mid, read both ways, with the margin taken against whoever is
        // sending. The two effective rates are therefore not reciprocals: a
        // round trip loses the spread twice, which is what the dealer earns.
        let mut table = RateTable::new(
            usdc(),
            Guards::new(spread(1, 100), ttl(60), max_move(1, 10)),
        );
        table.declare(anyone(), usdc(), rate(1, 4));
        table.declare(usdc(), anyone(), rate(4, 1));

        let out = table
            .lookup(&usdc(), &anyone(), at(0))
            .rate()
            .expect("declared");
        let back = table
            .lookup(&anyone(), &usdc(), at(0))
            .rate()
            .expect("declared");

        assert_eq!(out, rate(4 * 99, 100));
        assert_eq!(back, rate(99, 400));
        // A unit of USDC out and back comes home short.
        let round_trip = u128::from(out.numerator()) * u128::from(back.numerator());
        let whole = u128::from(out.denominator()) * u128::from(back.denominator());
        assert!(round_trip < whole, "{round_trip} should be under {whole}");
    }

    #[test]
    fn a_per_pair_guard_overrides_the_node_default() {
        let mut table = quoted(rate(100, 1), at(0));
        table.set_guards(
            anyone(),
            usdc(),
            GuardOverride {
                ttl: Some(ttl(10)),
                ..GuardOverride::default()
            },
        );

        // Ten seconds, not the node's sixty.
        assert!(table.lookup(&anyone(), &usdc(), at(9)).is_live());
        assert!(!table.lookup(&anyone(), &usdc(), at(10)).is_live());

        // And the two guards the override said nothing about are still the
        // node's.
        let guards = table.guards(&anyone(), &usdc());
        assert_eq!(guards.spread(), Spread::none());
        assert_eq!(guards.max_move(), max_move(1, 10));
    }

    #[test]
    fn an_unset_pair_falls_back_to_the_node_default() {
        let mut table = quoted(rate(100, 1), at(0));
        table.set_guards(
            weth(),
            usdc(),
            GuardOverride {
                ttl: Some(ttl(1)),
                ..GuardOverride::default()
            },
        );
        assert_eq!(table.guards(&anyone(), &usdc()), defaults());
        assert!(table.lookup(&anyone(), &usdc(), at(59)).is_live());
    }

    #[test]
    fn a_cross_rate_composes_through_the_numeraire() {
        // USDC is the numeraire. ANYONE quotes at a quarter of a USDC per
        // token and WETH at three thousand, both in base units of their own,
        // so ANYONE -> WETH is 1/12000 of a WETH per ANYONE.
        let mut table = RateTable::new(usdc(), defaults());
        table.refresh(anyone(), usdc(), rate(1, 4), at(0));
        table.refresh(weth(), usdc(), rate(3000, 1), at(0));

        assert_eq!(
            table.lookup(&anyone(), &weth(), at(0)).rate(),
            Some(rate(1, 12_000))
        );
        assert_eq!(
            table.lookup(&weth(), &anyone(), at(0)).rate(),
            Some(rate(12_000, 1))
        );
    }

    #[test]
    fn the_numeraire_side_of_a_cross_rate_is_the_quote_read_backwards() {
        let mut table = RateTable::new(usdc(), defaults());
        table.refresh(anyone(), usdc(), rate(1, 4), at(0));

        // ANYONE -> USDC is the row itself.
        assert_eq!(
            table.lookup(&anyone(), &usdc(), at(0)).rate(),
            Some(rate(1, 4))
        );
        // USDC -> ANYONE is that row inverted; no second row is declared.
        assert_eq!(
            table.lookup(&usdc(), &anyone(), at(0)).rate(),
            Some(rate(4, 1))
        );
    }

    #[test]
    fn a_stale_leg_takes_the_whole_cross_pair_down() {
        let mut table = RateTable::new(usdc(), defaults());
        table.refresh(anyone(), usdc(), rate(1, 4), at(0));
        table.refresh(weth(), usdc(), rate(3000, 1), at(30));

        // Both fresh.
        assert!(table.lookup(&anyone(), &weth(), at(30)).is_live());
        // ANYONE's leg has aged out; WETH's has not.
        assert_eq!(
            table.lookup(&anyone(), &weth(), at(60)),
            RateLookup::Stale {
                observed_at: at(0),
                refused: None,
            }
        );
        assert!(table.lookup(&weth(), &usdc(), at(60)).is_live());
    }

    #[test]
    fn a_composed_pair_reports_the_stalest_leg_as_its_freshness() {
        let mut table = RateTable::new(usdc(), defaults());
        table.refresh(anyone(), usdc(), rate(1, 4), at(0));
        table.refresh(weth(), usdc(), rate(3000, 1), at(30));

        assert!(matches!(
            table.lookup(&anyone(), &weth(), at(30)),
            RateLookup::Live {
                freshness: Freshness::Observed(observed),
                ..
            } if observed == at(0)
        ));
    }

    #[test]
    fn a_missing_leg_is_not_declared_rather_than_stale() {
        let mut table = RateTable::new(usdc(), defaults());
        table.refresh(anyone(), usdc(), rate(1, 4), at(0));
        assert_eq!(
            table.lookup(&anyone(), &weth(), at(0)),
            RateLookup::NotDeclared
        );
    }

    #[test]
    fn a_same_asset_pair_has_no_rate_and_gets_no_invented_one() {
        // ADR 0071 decision 2: a same-asset pair has no rate row by
        // definition and forwards unconverted. This table never hands out the
        // 1:1 that would let a real crossing through unconverted.
        let table = quoted(rate(1, 4), at(0));
        assert_eq!(
            table.lookup(&anyone(), &anyone(), at(0)),
            RateLookup::NotDeclared
        );
        assert_eq!(
            table.lookup(&usdc(), &usdc(), at(0)),
            RateLookup::NotDeclared
        );
    }

    #[test]
    fn an_ordered_pair_is_ordered() {
        // A static row for one direction is not a declaration about the other,
        // and nothing composes it into one.
        let mut table = RateTable::new(anyone(), defaults());
        table.declare(usdc(), weth(), rate(1, 3000));
        assert_eq!(
            table.lookup(&usdc(), &weth(), at(0)).rate(),
            Some(rate(1, 3000))
        );
        assert_eq!(
            table.lookup(&weth(), &usdc(), at(0)),
            RateLookup::NotDeclared
        );
    }

    #[test]
    fn a_static_row_never_goes_stale() {
        let mut table = RateTable::new(usdc(), defaults());
        table.declare(anyone(), usdc(), rate(1, 4));

        let answer = table.lookup(&anyone(), &usdc(), at(0));
        assert!(matches!(
            answer,
            RateLookup::Live {
                freshness: Freshness::Declared,
                ..
            }
        ));
        // A century later, still in force: nothing refreshes a declaration, so
        // `ttl` has nothing to measure.
        let much_later = Utc.with_ymd_and_hms(2130, 1, 1, 0, 0, 0).unwrap();
        assert!(table.lookup(&anyone(), &usdc(), much_later).is_live());
    }

    #[test]
    fn nothing_observed_replaces_a_declaration() {
        let mut table = RateTable::new(usdc(), defaults());
        table.declare(anyone(), usdc(), rate(1, 4));
        assert_eq!(
            table.refresh(anyone(), usdc(), rate(1, 4), at(0)),
            Refresh::Declared {
                standing: rate(1, 4)
            }
        );
        assert_eq!(
            table.lookup(&anyone(), &usdc(), at(0)).rate(),
            Some(rate(1, 4))
        );
    }

    #[test]
    fn a_static_row_overrides_composition() {
        // ADR 0071 decision 3: the row an operator tends by hand is also the
        // per-pair override.
        let mut table = RateTable::new(usdc(), defaults());
        table.refresh(anyone(), usdc(), rate(1, 4), at(0));
        table.refresh(weth(), usdc(), rate(3000, 1), at(0));
        table.declare(anyone(), weth(), rate(1, 10_000));

        assert_eq!(
            table.lookup(&anyone(), &weth(), at(0)).rate(),
            Some(rate(1, 10_000))
        );
    }

    #[test]
    fn a_declared_pair_lists_itself_for_an_operator() {
        let mut table = RateTable::new(usdc(), defaults());
        table.refresh(anyone(), usdc(), rate(1, 4), at(0));
        table.declare(weth(), usdc(), rate(3000, 1));

        let listed: Vec<_> = table
            .declared_pairs()
            .map(|(from, to)| (from.to_string(), to.to_string()))
            .collect();
        assert_eq!(
            listed,
            vec![
                (anyone().to_string(), usdc().to_string()),
                (weth().to_string(), usdc().to_string()),
            ]
        );
    }

    #[test]
    fn a_table_with_no_rows_lists_nothing() {
        let table = RateTable::new(usdc(), defaults());
        assert_eq!(table.declared_pairs().count(), 0);
    }

    #[test]
    fn a_spread_is_refused_by_name_when_it_is_not_a_fraction() {
        assert_eq!(
            Spread::fraction(1, 0).expect_err("no whole"),
            GuardError::ZeroSpreadDenominator { numerator: 1 }
        );
        assert_eq!(
            Spread::fraction(3, 2).expect_err("more than the mid"),
            GuardError::SpreadAtOrAboveOne {
                numerator: 3,
                denominator: 2
            }
        );
        assert_eq!(
            Spread::fraction(2, 2).expect_err("the whole mid"),
            GuardError::SpreadAtOrAboveOne {
                numerator: 2,
                denominator: 2
            }
        );
    }

    #[test]
    fn a_max_move_is_refused_by_name_when_it_is_not_a_fraction() {
        assert_eq!(
            MaxMove::fraction(1, 0).expect_err("no whole"),
            GuardError::ZeroMaxMoveDenominator { numerator: 1 }
        );
        // A wide bound is a policy, not an error.
        assert!(MaxMove::fraction(5, 1).is_ok());
    }

    #[test]
    fn a_ttl_of_no_time_at_all_is_refused_by_name() {
        assert_eq!(
            Ttl::new(TimeDelta::zero()).expect_err("no window"),
            GuardError::NonPositiveTtl {
                ttl: TimeDelta::zero()
            }
        );
        assert!(Ttl::new(TimeDelta::seconds(-1)).is_err());
        assert!(Ttl::new(TimeDelta::seconds(1)).is_ok());
    }

    #[test]
    fn every_guard_refusal_names_the_guard() {
        for error in [
            GuardError::ZeroSpreadDenominator { numerator: 1 },
            GuardError::SpreadAtOrAboveOne {
                numerator: 3,
                denominator: 2,
            },
        ] {
            assert!(error.to_string().contains("spread"), "got: {error}");
        }
        assert!(GuardError::ZeroMaxMoveDenominator { numerator: 1 }
            .to_string()
            .contains("max_move"));
        assert!(GuardError::NonPositiveTtl {
            ttl: TimeDelta::zero()
        }
        .to_string()
        .contains("ttl"));
    }

    #[test]
    fn max_move_admits_the_move_it_says_it_does() {
        let tenth = max_move(1, 10);
        assert!(tenth.admits(rate(100, 1), rate(110, 1)));
        assert!(tenth.admits(rate(100, 1), rate(90, 1)));
        assert!(!tenth.admits(rate(100, 1), rate(111, 1)));
        assert!(!tenth.admits(rate(100, 1), rate(89, 1)));
    }

    #[test]
    fn max_move_is_exact_at_the_widest_terms_a_rate_can_hold() {
        // The comparison needs 192 bits, and a saturating one would answer
        // this wrong. A move of exactly nothing is admitted by a pinned bound
        // however large the terms are.
        let pinned = max_move(0, 1);
        let extreme = rate(u64::MAX, u64::MAX - 1);
        assert!(pinned.admits(extreme, extreme));
        assert!(!pinned.admits(extreme, rate(u64::MAX, u64::MAX - 2)));
        assert!(max_move(1, 2).admits(extreme, rate(u64::MAX, u64::MAX - 2)));

        // And the pair that actually needs the extra limb: the difference
        // alone fills a `u128`, so both sides of the comparison run past it.
        assert!(!max_move(1, 100).admits(rate(u64::MAX, 1), rate(1, u64::MAX)));
        assert!(max_move(u64::MAX, 1).admits(rate(u64::MAX, 1), rate(1, u64::MAX)));
    }

    #[test]
    fn a_composition_that_will_not_fit_rounds_down_rather_than_up() {
        // Two rates each folding a 10^12 decimals gap: the exact product's
        // numerator is 10^24, well past a `u64`.
        let mut table = RateTable::new(usdc(), defaults());
        table.refresh(anyone(), usdc(), rate(1_000_000_000_000, 7), at(0));
        table.refresh(weth(), usdc(), rate(7, 1_000_000_000_000), at(0));

        let composed = table
            .lookup(&anyone(), &weth(), at(0))
            .rate()
            .expect("composed");
        // Exact value is 10^24 / 49; the answer never exceeds it.
        let exact_numerator = 1_000_000_000_000u128 * 1_000_000_000_000u128;
        let exact_denominator = 49u128;
        assert!(
            u128::from(composed.numerator()) * exact_denominator
                <= exact_numerator * u128::from(composed.denominator()),
            "{composed} exceeded the exact composition"
        );
    }

    proptest! {
        #[test]
        fn a_spread_never_pays_the_sender_more_than_the_mid(
            numerator in 1u64..,
            denominator in 1u64..,
            kept in 0u64..1000,
        ) {
            let mid = rate(numerator, denominator);
            let spread = Spread::fraction(kept, 1000).expect("a fraction below one");
            if let Some(dealt) = spread.applied_to(mid) {
                prop_assert!(dealt <= mid, "{dealt} should not exceed {mid}");
            }
        }

        #[test]
        fn a_zero_spread_is_the_mid_exactly(
            numerator in 1u64..,
            denominator in 1u64..,
        ) {
            let mid = rate(numerator, denominator);
            prop_assert_eq!(Spread::none().applied_to(mid), Some(mid));
        }

        #[test]
        fn max_move_is_symmetric_about_the_value_in_force(
            previous in 1u64..1_000_000,
            moved in 0u64..2_000_000,
            bound in 0u64..100,
        ) {
            // The bound is a fraction of `previous`, so a move of the same
            // size up and down is admitted or refused alike.
            let guard = max_move(bound, 100);
            let previous_rate = rate(previous, 1);
            let up = rate(previous + moved, 1);
            prop_assume!(previous > moved);
            let down = rate(previous - moved, 1);
            prop_assert_eq!(guard.admits(previous_rate, up), guard.admits(previous_rate, down));
        }

        #[test]
        fn max_move_agrees_with_the_arithmetic_it_stands_for(
            previous in 1u64..100_000,
            refreshed in 1u64..100_000,
            bound in 0u64..50,
        ) {
            // Over integer rates the condition is plain: the guard must admit
            // exactly the moves inside `bound/100` of the value in force.
            let guard = max_move(bound, 100);
            let expected =
                u128::from(previous.abs_diff(refreshed)) * 100 <= u128::from(bound) * u128::from(previous);
            prop_assert_eq!(guard.admits(rate(previous, 1), rate(refreshed, 1)), expected);
        }

        #[test]
        fn a_refused_refresh_never_changes_what_the_table_answers(
            first in 1u64..1_000_000,
            second in 1u64..1_000_000,
            when in 0u32..59,
        ) {
            let mut table = quoted(rate(first, 1), at(0));
            let before = table.lookup(&anyone(), &usdc(), at(when));
            let verdict = table.refresh(anyone(), usdc(), rate(second, 1), at(when));
            let after = table.lookup(&anyone(), &usdc(), at(when));

            match verdict {
                Refresh::OutsideMaxMove { previous } => {
                    prop_assert_eq!(previous, rate(first, 1));
                    prop_assert_eq!(before.rate(), after.rate());
                    prop_assert!(after.refused().is_some());
                }
                Refresh::Accepted => {
                    prop_assert_eq!(after.rate(), Some(rate(second, 1)));
                    prop_assert!(after.refused().is_none());
                }
                Refresh::Declared { .. } => prop_assert!(false, "nothing was declared"),
            }
        }

        #[test]
        fn a_lookup_is_total_over_any_instant(
            numerator in 1u64..,
            denominator in 1u64..,
            seconds in -1_000_000i64..1_000_000,
        ) {
            // No panic anywhere between a clock that has run backwards and one
            // that has run far ahead: the only contract the packet path needs.
            let table = quoted(rate(numerator, denominator), at(0));
            let when = at(0) + TimeDelta::seconds(seconds);
            let _ = table.lookup(&anyone(), &usdc(), when);
            let _ = table.lookup(&usdc(), &anyone(), when);
            let _ = table.lookup(&weth(), &anyone(), when);
        }

        #[test]
        fn a_composition_never_rounds_the_senders_way(
            left_numerator in 1u64..u32::MAX as u64,
            left_denominator in 1u64..u32::MAX as u64,
            right_numerator in 1u64..u32::MAX as u64,
            right_denominator in 1u64..u32::MAX as u64,
            kept in 0u64..1000,
        ) {
            // Whatever the table answers for a cross pair, it is never more
            // than the exact `(X/numeraire) / (Y/numeraire)` less the spread.
            // A rate that rounded up would deliver more on the outgoing leg
            // than the covering claim was minted for.
            let mut table = RateTable::new(
                usdc(),
                Guards::new(
                    Spread::fraction(kept, 1000).expect("a fraction below one"),
                    ttl(60),
                    max_move(1, 10),
                ),
            );
            table.refresh(anyone(), usdc(), rate(left_numerator, left_denominator), at(0));
            table.refresh(weth(), usdc(), rate(right_numerator, right_denominator), at(0));

            if let Some(composed) = table.lookup(&anyone(), &weth(), at(0)).rate() {
                // exact = (ln/ld) * (rd/rn) * (1000 - kept)/1000
                let exact_numerator = u128::from(left_numerator)
                    * u128::from(right_denominator)
                    * u128::from(1000 - kept);
                let exact_denominator =
                    u128::from(left_denominator) * u128::from(right_numerator) * 1000;
                // Cross-multiplied through the same 192-bit multiply the
                // guard uses, because the exact terms here outgrow a `u128`.
                prop_assert!(
                    wide_mul(exact_denominator, composed.numerator())
                        <= wide_mul(exact_numerator, composed.denominator()),
                    "{composed} exceeds the exact composition"
                );
            }
        }

        #[test]
        fn a_cross_pair_is_live_exactly_while_both_its_legs_are(
            left_observed in 0u32..40,
            right_observed in 0u32..40,
            now in 0u32..120,
        ) {
            let mut table = RateTable::new(usdc(), defaults());
            table.refresh(anyone(), usdc(), rate(1, 4), at(left_observed));
            table.refresh(weth(), usdc(), rate(3000, 1), at(right_observed));

            let both_live = table.lookup(&anyone(), &usdc(), at(now)).is_live()
                && table.lookup(&weth(), &usdc(), at(now)).is_live();
            prop_assert_eq!(table.lookup(&anyone(), &weth(), at(now)).is_live(), both_live);
        }

        #[test]
        fn an_override_replaces_only_what_it_names(
            spread_numerator in 0u64..999,
            ttl_seconds in 1i64..10_000,
            move_numerator in 0u64..1000,
        ) {
            let mut table = RateTable::new(usdc(), defaults());
            table.set_guards(
                anyone(),
                usdc(),
                GuardOverride {
                    spread: Some(Spread::fraction(spread_numerator, 1000).expect("below one")),
                    ..GuardOverride::default()
                },
            );
            let guards = table.guards(&anyone(), &usdc());
            prop_assert_eq!(guards.spread().numerator(), spread_numerator);
            prop_assert_eq!(guards.ttl(), defaults().ttl());
            prop_assert_eq!(guards.max_move(), defaults().max_move());

            table.set_guards(
                anyone(),
                usdc(),
                GuardOverride {
                    ttl: Some(ttl(ttl_seconds)),
                    max_move: Some(max_move(move_numerator, 1000)),
                    ..GuardOverride::default()
                },
            );
            // The later override replaces the earlier one whole, so the spread
            // is the node's again.
            let guards = table.guards(&anyone(), &usdc());
            prop_assert_eq!(guards.spread(), defaults().spread());
            prop_assert_eq!(guards.ttl(), ttl(ttl_seconds));
            prop_assert_eq!(guards.max_move(), max_move(move_numerator, 1000));
        }

        #[test]
        fn a_wide_product_is_the_product(
            wide in any::<u128>(),
            narrow in any::<u64>(),
        ) {
            // The 192-bit multiplication `max_move` leans on, checked against
            // the exact arithmetic wherever the exact arithmetic fits.
            let (high, low) = wide_mul(wide, narrow);
            if let Some(exact) = wide.checked_mul(u128::from(narrow)) {
                prop_assert_eq!(high, 0);
                prop_assert_eq!(low, exact);
            } else {
                prop_assert!(high > 0);
            }
        }

        #[test]
        fn a_fitted_rate_never_exceeds_the_ratio_it_came_from(
            numerator in 1u128..,
            denominator in 1u128..,
        ) {
            if let Some(fitted) = rate_at_most(numerator, denominator) {
                // fitted.n / fitted.d <= numerator / denominator, compared
                // through the 192-bit multiply so the check itself is exact.
                prop_assert!(
                    wide_mul(denominator, fitted.numerator())
                        <= wide_mul(numerator, fitted.denominator()),
                    "{fitted} exceeds {numerator}/{denominator}"
                );
            }
        }
    }
}
