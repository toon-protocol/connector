//! Read models for the operator surface (issue #420, ADR 0008). Each type
//! here is exactly what [`crate::Connector`] hands back to a read handler --
//! no method beyond what the handler serializes as-is.
//!
//! [`ChannelView`] gained real fields in #459, once a settlement backend
//! existed for [`Connector`] to project channel state from. [`ClaimView`]
//! gained real fields in #423, once `crate::claim::ClaimBook` existed to
//! report on. [`PeerView`] gained its first field in #884, once
//! [`Connector`] gained a runtime-mutable peer table to report on --
//! before that it was a literal empty struct, since nothing in the
//! runtime tracked peer identity at all (peer carriage credentials live
//! entirely in `connector_config::PeerConfig`, consumed once at boot and
//! never stored back on [`Connector`]). An `ExposureView` existed from
//! #424 until ADR 0031/ADR 0033 (issue #882) retired the credit-window
//! accounting it reported.
//!
//! [`RateView`] arrived in #1297 with ADR 0071's dealing: it is the one
//! read on this surface that [`Connector`] does not project, because the
//! rate table is shared with a background poller rather than owned by the
//! connector, and a page that asked the connector would be asking a
//! second holder of the same `Arc`. [`DeclaredRates`] is therefore the one
//! thing in this module with a method that does work -- see its own docs
//! for why the handler is handed a pair of facts rather than two loose
//! ones.

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use connector_domain::{AssetId, Freshness, Price, Rate, RateLookup, RefusedRefresh};
use connector_settlement::{ChannelState, ChannelStatus};
use serde::{Deserialize, Serialize};

use crate::rate_table::SharedRateTable;

/// Whether a peer or route row came from the config file, loaded once at
/// boot and immutable for the process's life, or was added at runtime
/// over the operator surface (issue #884) -- durable, but never able to
/// shadow or be shadowed by a config-file row of the same key. See
/// `docs/adr/0034-a-runtime-peer-route-table-never-shadows-the-config-file.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RouteSource {
    Config,
    Runtime,
}

/// A static route as seen by the operator surface. `price` is the schedule a
/// claim must advance by to pay for this route (issue #520, ADR 0065) --
/// always present, since a terminated route is never silently free.
///
/// It rides in the operator's own spelling: a bare integer for a flat price,
/// a `{ base, per_kib }` object for one with a slope, exactly as the config
/// file writes it. So this row is byte-identical to what it was before
/// schedules existed for every flat route, and an operator reading one back
/// sees the shape they would write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteView {
    pub prefix: String,
    pub handler_url: String,
    pub price: Price,
}

/// A leased route (issue #427) as seen by the operator surface -- only
/// ever one not yet lapsed as of this node's own clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeasedRouteView {
    pub prefix: String,
    pub peer_id: String,
    pub expires_at: DateTime<Utc>,
}

/// A peer as seen by the operator surface (issue #884): every peer id this
/// node knows, from the config file (`source: Config`) or added at
/// runtime over the operator surface (`source: Runtime`). Peer carriage
/// details -- endpoint, credential, exposure -- stay in
/// `connector_config::PeerConfig` and are not reported here; this is only
/// the identity a `peer_id` on a route resolves against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerView {
    pub id: String,
    /// This peering's flat per-packet fee (ADR 0010, ADR 0061): what this
    /// connector retains for carrying one packet to it. Reported here
    /// rather than on [`PeerRouteView`] because it is a property of the
    /// counterparty, not of any prefix routed to it.
    pub fee: u64,
    /// ADR 0049's cap: the largest amount this connector will forward to
    /// this peering in one packet. Always a number, never absent -- a
    /// peering that states none keeps
    /// [`connector_config::DEFAULT_MAX_PACKET_AMOUNT`], and reporting the
    /// bound that is actually enforced is the whole point of reporting it
    /// (0049: *"the operator surface must be able to express a cap"*, and
    /// an operator who cannot read one back cannot tell whether their
    /// write landed).
    pub max_packet_amount: u64,
    pub source: RouteSource,
}

/// A peer-forwarding route (as opposed to [`RouteView`]'s app-terminating
/// one) as seen by the operator surface (issue #884): every row from
/// `[[routes]]`'s peer form (`source: Config`) plus every row added at
/// runtime (`source: Runtime`). Deliberately excludes a leased route
/// (issue #427) -- [`LeasedRouteView`] already reports those, and a lease
/// carries no `price` at all, unlike either of these. Neither carries a
/// `fee`: that attaches to the peering, and is reported on [`PeerView`]
/// (ADR 0061).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerRouteView {
    pub prefix: String,
    pub peer_id: String,
    pub price: Price,
    pub source: RouteSource,
}

/// A payment channel as seen by the operator surface (issue #459).
/// `counterparty` is hex-encoded (`0x`-prefixed) since it is arbitrary
/// bytes, not necessarily UTF-8 -- an EVM backend's is a 20-byte address,
/// but the port itself (`connector_settlement::SettlementBackend::open`)
/// takes an opaque `Vec<u8>`, so this view makes no assumption about its
/// shape beyond "some bytes, safe to put in JSON".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelView {
    pub id: String,
    pub counterparty: String,
    pub status: ChannelViewStatus,
    /// What the counterparty has deposited on their own side -- the
    /// collateral backing claims this node can redeem. Keeps its name and
    /// its meaning across issue #1118; what changed is that
    /// `POST /channels/:id/fund` no longer moves it.
    pub deposited: u128,
    /// What this node has deposited on its own side -- the collateral
    /// backing claims this node signs, and what
    /// `POST /channels/:id/fund` raises (issue #1118). Added rather than
    /// replacing `deposited`, so a reader of `GET /channels` sees both
    /// halves of a two-sided channel instead of one number whose side
    /// depended on who was asking.
    pub own_deposited: u128,
    pub redeemed: u128,
}

/// A channel's lifecycle status as reported over the operator surface --
/// mirrors [`connector_settlement::ChannelStatus`] rather than reusing it
/// directly, so this crate's read models stay serializable without
/// requiring that of every port type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelViewStatus {
    Open,
    /// Closed: its challenge period is running (or has elapsed but not yet
    /// been settled) -- `redeem` still works against it (issue #574).
    Closed,
    /// Settled: terminal, no further `fund` or `redeem` is possible.
    Settled,
}

impl From<ChannelState> for ChannelView {
    fn from(state: ChannelState) -> Self {
        ChannelView {
            id: state.id.0,
            counterparty: encode_hex(&state.counterparty),
            status: match state.status {
                ChannelStatus::Open => ChannelViewStatus::Open,
                ChannelStatus::Closed => ChannelViewStatus::Closed,
                ChannelStatus::Settled => ChannelViewStatus::Settled,
            },
            deposited: state.counterparty_deposited,
            own_deposited: state.own_deposited,
            redeemed: state.redeemed,
        }
    }
}

/// `0x`-prefixed lowercase hex -- the one encoding this crate uses whenever
/// arbitrary bytes need to round-trip through JSON.
fn encode_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(2 + bytes.len() * 2);
    hex.push_str("0x");
    for byte in bytes {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// A claim as seen by the operator surface (issue #423): one entry per
/// direction per peering relation with a claim ever exchanged -- what this
/// connector has claimed to the peer ([`ClaimDirection::Outbound`]) and
/// what the peer has claimed to this connector
/// ([`ClaimDirection::Inbound`], i.e. this connector's own watermark on
/// that channel). `peer_id` is `None` on an inbound entry: the peer semantics
/// has no identity handshake yet, so an inbound claim is known only by the
/// channel it names, not by which configured peer sent it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimView {
    pub peer_id: Option<String>,
    pub channel_id: String,
    pub direction: ClaimDirection,
    pub nonce: u64,
    pub cumulative_amount: u64,
    /// `true` for an outbound claim not yet acknowledged by the peer --
    /// always `false` for an inbound claim, which is accepted or rejected
    /// the instant it is received, never left pending.
    pub pending: bool,
    /// Which of this node's two claim books this entry came from (issue
    /// #1218): [`ClaimBookKind::Peer`] for everything above, which is
    /// always `crate::ClaimBook`'s own -- `connector-operator` is the one
    /// caller that also merges in [`ClaimBookKind::Client`] entries, read
    /// from `connector_client_edge::ClientClaimGate`, a second book this
    /// crate has no dependency on and so cannot tag itself. An additive
    /// field: every row this crate itself produces is `Peer`.
    pub book: ClaimBookKind,
}

/// Which side of a peering relation a [`ClaimView`] reports on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaimDirection {
    Outbound,
    Inbound,
}

/// Which claim book a [`ClaimView`] was read out of (issue #1218): the
/// peer semantics's own `crate::ClaimBook`, journaled to
/// `peer-claims.log`, or the client edge's
/// `connector_client_edge::ClientClaimGate`, journaled separately to
/// `client-edge-claims.log`. The two never merge (`two_ledgers_never_merge.rs`);
/// this field says which one a given row answers for, since `GET /claims`
/// now reads both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaimBookKind {
    Peer,
    Client,
}

/// What one declared pair is doing as of one instant (issue #1297, ADR
/// 0071): the three answers [`connector_domain::RateLookup`] gives the
/// forwarding path, in the operator's spelling.
///
/// The names are the forward's own verdict, not a second opinion about it.
/// [`Live`](Self::Live) is the only one a forward crosses; the other two
/// both refuse, and are separate because the operator's next move differs
/// -- a [`Stale`](Self::Stale) pair is one this node deals and is
/// currently blind on (look at the poller, or at the chain it reads), a
/// [`Refused`](Self::Refused) one is a pair this node holds no rate for at
/// all (write a `[[rates]]` row, or wait for a declared quote path's first
/// observation to land).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RateViewState {
    /// In force: a forward across this pair converts at
    /// [`RateView::rate`].
    Live,
    /// Declared, but the observation behind it aged past its `ttl`. Every
    /// forward across this pair refuses until a fresh one lands, and only
    /// across this pair -- ADR 0071 decision 5's outage is deliberately
    /// narrow.
    Stale,
    /// No rate is declared for this ordered pair, so ADR 0071 decision 2
    /// refuses every cross-token forward over it rather than inventing a
    /// 1:1. What an operator sees here is either a `[[rates]]` row waiting
    /// to be written, or a declared quote path whose poller has not landed
    /// its first observation yet.
    Refused,
}

/// One ordered pair as seen by the operator surface (issue #1297, ADR
/// 0071) -- what `GET /rates` lists, and the whole of what an operator
/// needs to tell a dead poller from a quiet market without reading logs.
///
/// `from` and `to` are [`connector_domain::AssetId`]s in their one
/// spelling (`evm:0x…`, `solana:…`), which is what the config file writes
/// and what [`std::str::FromStr`] reads back -- the same courtesy
/// [`RouteView::price`] extends.
///
/// The three optional fields are read together and each combination means
/// something:
///
/// * `rate` and no `last_refreshed` -- a static `[[rates]]` row the
///   operator tends by hand. Nothing refreshes a declaration, so it has no
///   last refresh and can never go stale (ADR 0009).
/// * `rate` and a `last_refreshed` -- an observation the poller wrote,
///   still inside its `ttl`.
/// * no `rate` and a `last_refreshed` -- stale: the instant is when this
///   node last saw a price, so the gap to now is how long it has been
///   blind.
/// * neither -- nothing is declared for the pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateView {
    pub from: String,
    pub to: String,
    pub state: RateViewState,
    /// The rate a forward across this pair converts at *right now*, spread
    /// already taken -- the figure the packet path reads, not the mid the
    /// table stores. `None` unless the pair is live.
    pub rate: Option<Rate>,
    /// When the observation behind this answer was written, or `None` for
    /// a rate nothing refreshes.
    pub last_refreshed: Option<DateTime<Utc>>,
    /// A refresh this pair's `max_move` turned away since its last
    /// accepted one (ADR 0071 decision 5) -- the difference between a pair
    /// that is guarded and one that merely aged out, and the reason an
    /// operator is being woken.
    ///
    /// Orthogonal to `state`: the refusal leaves the previous value in
    /// place *and ageing*, so a pair carrying one may still be `Live` or
    /// may since have gone `Stale` on its own `ttl`. Both readings matter.
    pub refused_refresh: Option<RefusedRefreshView>,
}

/// A refresh [`connector_domain::MaxMove`] turned away, as seen by the
/// operator surface: what was offered, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedRefreshView {
    pub offered: Rate,
    pub at: DateTime<Utc>,
}

impl From<RefusedRefresh> for RefusedRefreshView {
    fn from(refused: RefusedRefresh) -> Self {
        RefusedRefreshView {
            offered: refused.offered,
            at: refused.at,
        }
    }
}

/// Everything `GET /rates` reads: this node's rate table, and the pairs it
/// has committed to sourcing (issue #1297).
///
/// The one type here that is not a read model, and it earns its place by
/// being the *whole* of what the handler needs -- a handler holding two
/// loose halves could ask them about two different instants, which is the
/// one failure this read exists to avoid.
///
/// Two sources of pairs, one source of truth about each. The table's own
/// [`connector_domain::RateTable::declared_pairs`] is every row an operator declared or a
/// poller wrote. `quoted` adds the `(token, numeraire)` pair of every
/// declared quote path, which holds **no row at all** until the poller's
/// first observation lands -- without it, the pair a dead-from-boot poller
/// was supposed to price would be missing from the one page that exists to
/// say a poller is dead. Neither source ever decides a pair's *state*:
/// that is [`connector_domain::RateTable::lookup`]'s answer and nothing else's, so this page
/// and the packet path cannot disagree.
///
/// Cheap to clone: the table handle is two `Arc`s and the pair list is a
/// third.
#[derive(Clone)]
pub struct DeclaredRates {
    table: SharedRateTable,
    quoted: Arc<[(AssetId, AssetId)]>,
}

impl DeclaredRates {
    /// `quoted` is the `(token, numeraire)` pair of every declared quote
    /// path -- `connector_config::DenominationConfig::quoted_tokens`
    /// against the table's numeraire. Empty is right for a node whose
    /// every pair is a static `[[rates]]` row.
    pub fn new(
        table: SharedRateTable,
        quoted: impl IntoIterator<Item = (AssetId, AssetId)>,
    ) -> DeclaredRates {
        DeclaredRates {
            table,
            quoted: quoted.into_iter().collect(),
        }
    }

    /// Every declared pair, as of `now`, from **one** snapshot of the
    /// table.
    ///
    /// One [`SharedRateTable::read`] and one instant for the whole page:
    /// a refresh landing while this runs is seen by the next call or by
    /// none of it, never by half of it, so no reader ever sees a pair live
    /// on one row and stale on another. Sorted, and deduplicated across
    /// the two sources, so an operator watching the page sees a row stay
    /// where it was.
    pub fn views(&self, now: DateTime<Utc>) -> Vec<RateView> {
        let table = self.table.read();
        let pairs: BTreeSet<(&AssetId, &AssetId)> = table
            .declared_pairs()
            .chain(self.quoted.iter().map(|(from, to)| (from, to)))
            .collect();
        pairs
            .into_iter()
            .map(|(from, to)| rate_view(from, to, table.lookup(from, to, now)))
            .collect()
    }
}

impl std::fmt::Debug for DeclaredRates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeclaredRates")
            .field("table", &self.table)
            .field("quoted", &self.quoted)
            .finish()
    }
}

/// One pair's lookup, rendered. The whole mapping from what the forwarding
/// path is told to what an operator is shown, in one place.
fn rate_view(from: &AssetId, to: &AssetId, looked_up: RateLookup) -> RateView {
    let (state, rate, last_refreshed) = match looked_up {
        RateLookup::Live {
            rate, freshness, ..
        } => (
            RateViewState::Live,
            Some(rate),
            match freshness {
                // A declaration has no last refresh, because nothing
                // refreshes it -- reporting `now`, or the boot instant,
                // would be this page inventing an observation.
                Freshness::Declared => None,
                Freshness::Observed(at) => Some(at),
            },
        ),
        RateLookup::Stale { observed_at, .. } => (RateViewState::Stale, None, Some(observed_at)),
        RateLookup::NotDeclared => (RateViewState::Refused, None, None),
    };
    RateView {
        from: from.to_string(),
        to: to.to_string(),
        state,
        rate,
        last_refreshed,
        refused_refresh: looked_up.refused().map(RefusedRefreshView::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeDelta, TimeZone};
    use connector_domain::{Guards, MaxMove, RateTable, Spread, Ttl};

    /// USDC on Base, the numeraire -- ADR 0071's own example, and the one
    /// `connector-config`'s denomination tests and `rate_table`'s use.
    const USDC: &str = "evm:0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
    /// ANYONE on Base: 18 decimals against USDC's 6, and the token whose
    /// price comes off a pool rather than out of the file.
    const ANYONE: &str = "evm:0x9ff58f4ffb29fa2266ab25e75e2a8b3503311656";
    /// A second dealt token, for the pairs that must not move when one
    /// does.
    const WETH: &str = "evm:0x4200000000000000000000000000000000000006";

    fn asset(text: &str) -> AssetId {
        text.parse::<AssetId>().expect("a declared asset")
    }

    fn rate(numerator: u64) -> Rate {
        Rate::new(numerator, 1).expect("a rate")
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap() + TimeDelta::seconds(seconds)
    }

    /// A two-minute `ttl` and a tight `max_move`: both guards are subjects
    /// here, so neither is set wide enough to be unreachable.
    fn guards() -> Guards {
        Guards::new(
            Spread::none(),
            Ttl::new(TimeDelta::seconds(120)).expect("a positive ttl"),
            MaxMove::fraction(10, 100).expect("a max_move"),
        )
    }

    fn table() -> SharedRateTable {
        SharedRateTable::new(RateTable::new(asset(USDC), guards()))
    }

    fn row<'a>(views: &'a [RateView], from: &str, to: &str) -> &'a RateView {
        views
            .iter()
            .find(|view| view.from == from && view.to == to)
            .unwrap_or_else(|| panic!("no row for {from} -> {to} in {views:#?}"))
    }

    /// The ordinary healthy pair: a poller's observation, inside its ttl.
    /// The rate reported is the one a forward would convert at, and the
    /// instant is when it was last written -- the two facts the ticket
    /// asks a live pair for.
    #[test]
    fn a_live_pair_reports_the_rate_in_force_and_its_last_refresh() {
        let shared = table();
        shared.write(|table| table.refresh(asset(ANYONE), asset(USDC), rate(4), at(0)));

        let views = DeclaredRates::new(shared, []).views(at(60));

        let live = row(&views, ANYONE, USDC);
        assert_eq!(live.state, RateViewState::Live);
        assert_eq!(live.rate, Some(rate(4)));
        assert_eq!(live.last_refreshed, Some(at(0)));
        assert_eq!(live.refused_refresh, None);
    }

    /// A static `[[rates]]` row never goes stale and nothing refreshes it,
    /// so it reports a rate and no last refresh. An operator reading a
    /// blank there is reading "this one is yours to tend", not "this one
    /// has never been seen".
    #[test]
    fn a_declared_row_is_live_with_no_last_refresh() {
        let shared = table();
        shared.write(|table| table.declare(asset(ANYONE), asset(USDC), rate(4)));

        let views = DeclaredRates::new(shared, []).views(at(100_000));

        let declared = row(&views, ANYONE, USDC);
        assert_eq!(declared.state, RateViewState::Live);
        assert_eq!(declared.rate, Some(rate(4)));
        assert_eq!(declared.last_refreshed, None);
    }

    /// The outage the ticket exists for: the pair that took the routes
    /// across it down reports when it was last seen, so an operator can
    /// read how long this node has been blind without opening a log.
    #[test]
    fn a_stale_pair_reports_when_it_was_last_seen_and_no_rate() {
        let shared = table();
        shared.write(|table| table.refresh(asset(ANYONE), asset(USDC), rate(4), at(0)));

        let views = DeclaredRates::new(shared, []).views(at(600));

        let stale = row(&views, ANYONE, USDC);
        assert_eq!(stale.state, RateViewState::Stale);
        assert_eq!(stale.rate, None);
        assert_eq!(stale.last_refreshed, Some(at(0)));
    }

    /// The ticket's third distinction, and the reason `refused_refresh` is
    /// a field rather than a state: a pair `max_move` is holding a line on
    /// is still trading, and an operator must be able to tell it from one
    /// that merely aged out. Both facts are on the same row because both
    /// came out of the same lookup.
    #[test]
    fn a_max_move_refusal_is_visible_on_a_pair_that_is_still_live() {
        let shared = table();
        shared.write(|table| table.refresh(asset(ANYONE), asset(USDC), rate(4), at(0)));
        // Four to forty is well outside a ten-percent bound: probable
        // manipulation, refused, previous value serving out its own ttl.
        shared.write(|table| table.refresh(asset(ANYONE), asset(USDC), rate(40), at(30)));

        let views = DeclaredRates::new(shared.clone(), []).views(at(60));

        let guarded = row(&views, ANYONE, USDC);
        assert_eq!(guarded.state, RateViewState::Live);
        assert_eq!(guarded.rate, Some(rate(4)), "the previous value stands");
        assert_eq!(
            guarded.refused_refresh,
            Some(RefusedRefreshView {
                offered: rate(40),
                at: at(30),
            })
        );

        // And once the value it was holding ages out, the same row says
        // both things at once: blind, and blind while guarded.
        let aged = DeclaredRates::new(shared, []).views(at(600));
        let aged = row(&aged, ANYONE, USDC);
        assert_eq!(aged.state, RateViewState::Stale);
        assert!(aged.refused_refresh.is_some(), "{aged:#?}");
    }

    /// A declared quote path holds no row until its poller's first
    /// observation lands, so the pair a poller that never started was
    /// meant to price is exactly the one an operator needs to see. It
    /// reports what the forwarding path would do with it: refuse.
    #[test]
    fn a_quoted_pair_with_no_observation_yet_reports_refused() {
        let shared = table();

        let views = DeclaredRates::new(shared, [(asset(ANYONE), asset(USDC))]).views(at(0));

        let blind = row(&views, ANYONE, USDC);
        assert_eq!(blind.state, RateViewState::Refused);
        assert_eq!(blind.rate, None);
        assert_eq!(blind.last_refreshed, None);
    }

    /// The same pair from both sources is one row, not two: a token that
    /// is both quoted and declared is one pair, and the table is the
    /// authority on what it is doing.
    #[test]
    fn a_pair_that_is_both_quoted_and_held_is_listed_once() {
        let shared = table();
        shared.write(|table| table.refresh(asset(ANYONE), asset(USDC), rate(4), at(0)));

        let views = DeclaredRates::new(shared, [(asset(ANYONE), asset(USDC))]).views(at(60));

        assert_eq!(views.len(), 1, "{views:#?}");
        assert_eq!(views[0].state, RateViewState::Live);
    }

    /// One snapshot, one instant: every row on the page is read from the
    /// same table and against the same clock reading, so a pair cannot be
    /// live on one row and stale on another. Two pairs with different
    /// observation instants, read at one `now`, each answer for their own
    /// age -- and the second pair is untouched by the first's outage,
    /// which is ADR 0071 decision 5's narrow-outage claim on this surface.
    #[test]
    fn every_row_answers_for_the_same_instant() {
        let shared = table();
        shared.write(|table| table.refresh(asset(ANYONE), asset(USDC), rate(4), at(0)));
        shared.write(|table| table.refresh(asset(WETH), asset(USDC), rate(3000), at(540)));

        let views = DeclaredRates::new(shared, []).views(at(600));

        assert_eq!(row(&views, ANYONE, USDC).state, RateViewState::Stale);
        assert_eq!(row(&views, WETH, USDC).state, RateViewState::Live);
    }

    /// A stable order, so an operator watching the page sees a row stay
    /// where it was rather than shuffle under the cursor on each poll.
    #[test]
    fn pairs_are_listed_in_a_stable_order() {
        let shared = table();
        shared.write(|table| table.refresh(asset(WETH), asset(USDC), rate(3000), at(0)));
        shared.write(|table| table.refresh(asset(ANYONE), asset(USDC), rate(4), at(0)));

        let declared = DeclaredRates::new(shared, [(asset(WETH), asset(USDC))]);

        let first: Vec<String> = declared.views(at(1)).into_iter().map(|v| v.from).collect();
        let second: Vec<String> = declared.views(at(2)).into_iter().map(|v| v.from).collect();
        assert_eq!(first, second);
        assert_eq!(first, vec![WETH.to_string(), ANYONE.to_string()]);
    }

    /// The wire shape, pinned: an operator reads back the spelling they
    /// write in the config file -- an asset as `chain:token`, a rate as
    /// the `{ numerator, denominator }` fraction `[[rates]]` takes.
    #[test]
    fn a_view_serializes_in_the_operators_own_spelling() {
        let shared = table();
        shared.write(|table| table.refresh(asset(ANYONE), asset(USDC), rate(4), at(0)));

        let views = DeclaredRates::new(shared, []).views(at(60));
        let json = serde_json::to_value(&views).expect("a serializable view");

        assert_eq!(json[0]["from"], serde_json::json!(ANYONE));
        assert_eq!(json[0]["to"], serde_json::json!(USDC));
        assert_eq!(json[0]["state"], serde_json::json!("live"));
        assert_eq!(
            json[0]["rate"],
            serde_json::json!({ "numerator": 4, "denominator": 1 })
        );
    }
}
