//! `pub struct Connector` -- the packet plane. See ADR 0001.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use arc_swap::ArcSwap;
use chrono::{DateTime, Duration, Utc};
use connector_config::{
    ClientChannelAssets, PeeringAssets, SettlementChain, StaticRoute, TransportPolicy,
    DEFAULT_MAX_PACKET_AMOUNT,
};
use connector_domain::x402::X402PaymentRequired;
use connector_domain::{
    amount_after_fee, amount_after_rate_and_fee, cost_before_rate_and_fee, delivery_budget,
    forwarded_expiry, is_expired, is_valid_ilp_address, select_route, AssetId, EnvelopeRequest,
    Fulfill, PacketResponse, Prepare, Price, RateLookup, Reject, RejectCode, Watermark,
    FORWARDING_MESSAGE_WINDOW,
};
use connector_settlement::{ChannelId, Claim, SettlementBackend, SettlementError};
use connector_signer::giftwrap::{derive_fulfillment, open_request, seal_response};
use connector_signer::{Address, Ed25519Signer, Signer};
use rand::rngs::OsRng;
use rand::RngCore;
use thiserror::Error;
use tracing::Instrument;
use url::Url;

use crate::app_client::{AppClient, AppOutcome};
use crate::attribution::{apply_payment_attribution, PaymentAttribution};
use crate::claim::{
    ChannelDomain, ClaimAckOutcome, ClaimBook, ClaimRejectReason, InvalidChannelId,
    InvalidSolanaChannel, WireClaim,
};
use crate::clock::Clock;
use crate::journal::{Journal, JournalError};
use crate::metrics::Metrics;
use crate::operator_view::{
    ChannelView, ClaimView, LeasedRouteView, PeerRouteView, PeerView, RouteSource, RouteView,
};
use crate::outbound_client::{
    ClaimStateChallengeSigner, ClaimStateSource, EvmDomain, OutboundClaimBinding,
    OutboundClientLedger, OwnedHttpClaimState, SolanaDomain,
};
use crate::peer_route_store::{
    PeerRouteStore, PeerRouteStoreError, RuntimePeerChannel, RuntimePeering, RuntimePeers,
};
use crate::peer_transport::{PeerRegistrar, PeerTransport};
use crate::rate_table::SharedRateTable;
use crate::route::{LeasedRoute, PeerRoute};
use crate::self_description::{SelfDescriptionSource, UnreachableSelfDescription};

/// A reject this connector originates before a gift wrap's shared secret
/// could be recovered -- no identity key configured, or the wrap itself
/// could not be opened. Necessarily plaintext (ADR 0018: "a reject raised
/// short of the termination is necessarily plaintext... shares no secret
/// with the sender and cannot seal anything"), with empty `data` so a
/// sender can tell it apart from a sealed one
/// (`connector_signer::giftwrap::looks_like_sealed_response`).
fn unsealed_termination_reject(message: &str) -> Reject {
    Reject {
        code: RejectCode::f01_invalid_packet(),
        triggered_by: String::new(),
        message: message.to_string(),
        data: Vec::new(),
        accumulated_cost: 0,
    }
}

/// What can go wrong creating or renewing a leased route (issue #427).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LeaseRouteError {
    #[error("invalid ILP address: '{0}'")]
    InvalidPrefix(String),
}

/// What can go wrong mutating issue #884's runtime peer/route table
/// through [`Connector::upsert_runtime_peer`],
/// [`Connector::remove_runtime_peer`],
/// [`Connector::upsert_runtime_peer_route`] or
/// [`Connector::remove_runtime_peer_route`]. See
/// `docs/adr/0034-a-runtime-peer-route-table-never-shadows-the-config-file.md`
/// for the precedence rule these variants enforce.
#[derive(Debug, Error)]
pub enum PeerRouteTableError {
    #[error("invalid ILP address: '{0}'")]
    InvalidPrefix(String),
    #[error("peer id must not be empty")]
    InvalidPeerId,
    /// The config file already names this peer id or route prefix
    /// (`[[peers]]` / `[[routes]]`). A runtime write can never add,
    /// change or remove a config-file row -- config always wins, and it
    /// wins by refusing the write outright rather than by silently
    /// shadowing or being shadowed.
    #[error("'{0}' is defined in this node's config file and cannot be changed at runtime")]
    OwnedByConfig(String),
    /// A runtime route named a `peer_id` that resolves to no known peer --
    /// neither the config file nor the runtime peer table -- the runtime
    /// analogue of `connector-config`'s load-time `UnknownPeerId` check,
    /// enforced continuously here rather than once at boot.
    #[error("route '{prefix}' names unknown peer '{peer_id}'")]
    UnknownPeerId { prefix: String, peer_id: String },
    /// A runtime peer cannot be removed while a runtime route still
    /// forwards to it -- the same orphaned-row shape `UnknownPeerId`
    /// guards against at load, refused here rather than left to produce a
    /// route with a peer id nothing recognizes.
    #[error("peer '{0}' is still referenced by a runtime route")]
    PeerInUse(String),
    /// A runtime peering carried no payment-channel binding -- the runtime
    /// twin of `connector-config`'s load-time `PeerChannelUnbound`, which
    /// refuses a `[[peers]]` row with no `[[peer_channels]]` row for it.
    ///
    /// ADR 0058 requires this to be a refusal at *write* time rather than
    /// a discovery at the first arriving frame: without a channel binding
    /// a peering can never take the peer role at all (ADR 0060 -- role is
    /// a verified claim on a channel this peering configures), so a row
    /// written without one is a peering that silently only ever behaves
    /// as a stranger.
    #[error("peering '{0}' has no payment channel bound to it")]
    PeerChannelUnbound(String),
    /// A runtime route forwards to a peering this node cannot pay -- the
    /// runtime twin of the `[[pay_channels]]` rule ADR 0042 made
    /// load-refusing: *"a peering this node forwards to must name the
    /// channel it pays from"*. A forward this node cannot cover is
    /// refused rather than carried, so a route to such a peering would
    /// only ever produce refused packets.
    #[error("route '{prefix}' forwards to peering '{peer_id}', which has no channel to pay from")]
    PeerHasNoPayChannel { prefix: String, peer_id: String },
    #[error("no such runtime peer '{0}'")]
    PeerNotFound(String),
    #[error("no such runtime route '{0}'")]
    RouteNotFound(String),
    /// The durable write itself failed (disk full, permissions, etc.) --
    /// the mutation is refused rather than applied in memory only, so the
    /// in-memory table and the durable copy can never diverge.
    #[error("could not persist the runtime peer/route table: {0}")]
    Persistence(#[from] PeerRouteStoreError),
}

/// What can go wrong driving a payment channel's lifecycle through
/// [`Connector::open_channel`]/[`Connector::fund_channel`]/
/// [`Connector::close_channel`] (issue #459). [`Settlement`] carries
/// through whatever the configured [`SettlementBackend`] itself reported;
/// [`NoSettlementBackend`] is this crate's own -- a channel operation
/// reaching a node with none configured (ADR 0009: a node that never names
/// one in its config simply never gets a working channel surface, rather
/// than a panic).
///
/// [`Settlement`]: ChannelOperationError::Settlement
/// [`NoSettlementBackend`]: ChannelOperationError::NoSettlementBackend
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ChannelOperationError {
    #[error("no settlement backend is configured for this node")]
    NoSettlementBackend,
    /// This node settles on at least one chain, just not the one this
    /// operation needs: the channel id (or the caller's explicit `chain`)
    /// named a chain no `[settlement.<chain>]` table configured a backend
    /// for. Distinct from [`NoSettlementBackend`] so a both-surfaces
    /// operator error names the actual gap ("no solana backend") rather
    /// than denying the backends the node does have.
    ///
    /// [`NoSettlementBackend`]: ChannelOperationError::NoSettlementBackend
    #[error("no {0} settlement backend is configured for this node")]
    NoSettlementBackendForChain(SettlementChain),
    /// [`Connector::open_channel`] was called without naming a chain on a
    /// node that settles on more than one, where "the configured backend"
    /// denotes nothing (issue #630's review: a node with both
    /// `[settlement.evm]` and `[settlement.solana]` must not silently
    /// pick one).
    #[error("this node settles on more than one chain -- name which chain to open the channel on")]
    AmbiguousSettlementChain,
    /// [`Connector::redeem_latest_claim`] or [`Connector::cooperative_close`]
    /// was asked to redeem a channel this node has never accepted an
    /// inbound claim on (issue #425) -- distinct from
    /// [`SettlementError::StaleClaim`], which means a claim exists but the
    /// chain has already redeemed at least that much.
    #[error("no claim has been accepted on this channel to redeem")]
    NoClaimToRedeem,
    #[error(transparent)]
    Settlement(#[from] SettlementError),
}

/// Why [`Connector::handle_probe`] declined to route a packet at all,
/// before ever calling [`Connector::handle_prepare`] (issue #426, ADR
/// 0011's consequence: "a probe traverses the network and pays nothing").
/// Neither variant is a [`PacketResponse`] -- a denial here means this
/// packet was never treated as an ILP-level exchange to begin with,
/// matching how the client edge is specified to answer it with a bare
/// `403` rather than an OER REJECT body (`docs/protocol/client-edge-spec.md`
/// §1.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeDenied {
    /// `channel_id` holds no payment channel this connector recognizes.
    NoOpenChannel,
    /// `channel_id` has exceeded its configured probe rate limit.
    RateLimited,
}

/// A fixed-window rate limiter, keyed by sender identity. Counted against
/// this connector's own injected [`Clock`] rather than wall time, so tests
/// control it deterministically instead of racing real elapsed time.
///
/// One instance per budget, never one shared across budgets, so a flood
/// against one can never starve another's. This connector holds exactly
/// one: probe traffic (issue #426 / ADR 0011's "a probe traverses the
/// network and pays nothing ... so it is ... rate-limited per that
/// identity").
struct FixedWindowRateLimiter {
    max_per_window: u32,
    window: Duration,
    /// A plain [`Mutex`] rather than [`RwLock`] like `known_channels`
    /// below -- the counting path mutates on every access, so a
    /// reader/writer lock would buy nothing over mutual exclusion.
    windows: Mutex<HashMap<String, (DateTime<Utc>, u32)>>,
}

impl FixedWindowRateLimiter {
    fn new(max_per_window: u32, window: Duration) -> FixedWindowRateLimiter {
        FixedWindowRateLimiter {
            max_per_window,
            window,
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Record one attempt from `identity` at `now`, returning whether it
    /// is allowed. A window starts on an identity's first attempt (or its
    /// first attempt after its previous window elapsed) and admits up to
    /// `max_per_window` attempts before refusing the rest until the next
    /// window starts.
    fn allow(&self, identity: &str, now: DateTime<Utc>) -> bool {
        let mut windows = self.windows.lock().expect("rate limiter lock poisoned");
        match windows.get_mut(identity) {
            Some((started_at, count)) if now < *started_at + self.window => {
                if *count >= self.max_per_window {
                    false
                } else {
                    *count += 1;
                    true
                }
            }
            _ => {
                windows.insert(identity.to_string(), (now, 1));
                true
            }
        }
    }
}

/// Which kind of routing-table entry matched a packet's destination, and
/// where in its own table -- resolved by [`Connector::handle_prepare`]
/// before dispatch, so priority among same-length matches (issue #427: a
/// static route always outranks a leased route) is decided in exactly one
/// place.
enum RouteTarget {
    App(usize),
    Peer(usize),
    /// Indexes into the caller's own filtered `Vec` of currently-active
    /// leased routes, not `Connector::leased_routes` directly -- see
    /// [`Connector::leased_routes_snapshot`].
    Leased(usize),
    /// A runtime peer-forwarding route (issue #884), owned rather than
    /// indexed: unlike `leased_routes_snapshot`'s caller-held `Vec`, the
    /// snapshot this was matched out of is loaded and dropped inside
    /// `select_configured_route` itself, so the one matched
    /// [`PeerRoute`] is cloned out of it rather than borrowed -- a single,
    /// bounded-size clone per packet, not the whole-collection copy ADR
    /// 0015 warns against.
    RuntimePeer(PeerRoute),
}

/// How permanent a matched routing-table entry is, least to most -- the
/// one place route precedence is written down, read by both
/// [`RouteTarget`] and [`ConfiguredTarget`] so what the router prefers and
/// what the client edge prices can never drift apart.
///
/// A lease (issue #427) is TTL-bound and pushed by an automated
/// controller, so it is outranked by everything durable. A runtime
/// peer-forwarding route (issue #884) IS durable -- a deliberate, paid
/// relationship, not an automated push -- so it outranks a lease at the
/// same prefix, but a config-file row always wins over anything written at
/// runtime, which `upsert_runtime_peer_route` enforces by refusing a
/// runtime write that collides with a config-file prefix in the first
/// place, rather than by ranking them here. Peer routes (config or
/// runtime) fall between leases and app routes: also forwarding rather
/// than terminating, but static.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RouteRank {
    Leased = 0,
    RuntimePeer = 1,
    Peer = 2,
    App = 3,
}

impl RouteTarget {
    /// Break a tie in matched prefix length -- see [`RouteRank`].
    fn rank(&self) -> RouteRank {
        match self {
            RouteTarget::Leased(_) => RouteRank::Leased,
            RouteTarget::RuntimePeer(_) => RouteRank::RuntimePeer,
            RouteTarget::Peer(_) => RouteRank::Peer,
            // A config-file entry -- durable and priced, never shadowed
            // by a lease or a runtime peer route at the same prefix
            // length.
            RouteTarget::App(_) => RouteRank::App,
        }
    }
}

/// A fresh random id for this hop's own `"packet"` tracing span, minted at
/// packet entry and never placed on the wire (ADR 0014, amended by issue
/// #1269 / ADR 0069).
///
/// Before this, the id was the packet's own execution condition -- free
/// because it was already invariant across every hop, so two independent
/// connectors logging the same value could join their structured logs
/// across the hop boundary. That was exactly the problem: invariant *and*
/// distinctive per packet is a perfect join key, and any two hops on a
/// path -- or anyone reading two hops' logs -- could trivially link the
/// packet they each saw. Cross-hop correlation is retired, not replaced:
/// each hop now mints its own id, so logs still correlate perfectly within
/// one node's own handling of one packet, and no longer join across a
/// second one at all.
fn correlation_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Which of the two configured route kinds matched, as
/// [`Connector::select_configured_route`] resolves it -- the subset of
/// [`RouteTarget`] that exists in configuration, so a caller reading
/// configured routes alone (the client edge) needs no arm for a leased
/// route it can never be handed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfiguredTarget {
    App(usize),
    Peer(usize),
    /// A runtime peer-forwarding route (issue #884) -- "configured" in the
    /// sense this type means it (priced, static, unlike a lease), even
    /// though its row lives in the runtime table rather than the config
    /// file. See [`RouteTarget::RuntimePeer`] for why this is owned
    /// rather than indexed.
    RuntimePeer(PeerRoute),
}

impl ConfiguredTarget {
    fn into_route_target(self) -> RouteTarget {
        match self {
            ConfiguredTarget::App(index) => RouteTarget::App(index),
            ConfiguredTarget::Peer(index) => RouteTarget::Peer(index),
            ConfiguredTarget::RuntimePeer(route) => RouteTarget::RuntimePeer(route),
        }
    }

    /// The same tie-break [`RouteTarget::rank`] applies, off the same
    /// [`RouteRank`] ordering rather than one restated here, so
    /// configured-route precedence cannot drift between the router and the
    /// client edge -- and read without cloning the matched route to reach
    /// it.
    fn rank(&self) -> RouteRank {
        match self {
            ConfiguredTarget::App(_) => RouteRank::App,
            ConfiguredTarget::Peer(_) => RouteRank::Peer,
            ConfiguredTarget::RuntimePeer(_) => RouteRank::RuntimePeer,
        }
    }
}

/// Whether the route a client's destination resolves to terminates at this
/// connector's own app or forwards over a peering (ADR 0028). The client
/// edge charges both identically; it needs the distinction only for the
/// two rules that genuinely differ -- a forwarded route applies no
/// transport policy, and a priced forwarded route bounds the amount it will
/// carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientRouteKind {
    /// A `handler_url` route: `price` buys the app's work (issue #520).
    Terminated,
    /// A `peer_id` route: `price` buys the whole path, of which this hop
    /// retains `fee` (ADR 0028).
    Forwarded,
}

/// The two facts about a matched terminated route that finishing a delivery
/// needs: where to send the request, and what this connector charged for the
/// packet that carries it.
///
/// Bundled rather than passed loose for the same reason `btp.rs`'s
/// `MatchedRoute` is -- one more loose parameter puts
/// [`Connector::deliver_opened_envelope`] over clippy's argument-count lint.
/// It carries the **charge** and not the route's `Price`, deliberately: the
/// schedule was evaluated once, against the payload length of the packet that
/// arrived, before the wrap that payload lives in was opened (ADR 0065). Past
/// that point there is no length left to evaluate against and nothing here
/// can quietly re-derive a different figure.
struct PricedTermination<'a> {
    handler_url: &'a Url,
    charge: u64,
}

/// What the client edge needs to know about the configured route a
/// destination resolves to, from a single lookup (issue #701, ADR 0028):
/// the price to greet and charge, the transport policy to enforce, and
/// which kind of route answered.
///
/// Not `Copy` since issue #1210: `request` carries an owned JSON value when
/// the route configured one, so a caller that needs this more than once
/// borrows it (`.as_ref()`) rather than relying on an implicit copy.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientRouteFacts {
    pub price: Price,
    pub transport_policy: TransportPolicy,
    pub kind: ClientRouteKind,
    /// What a client should send to use this route (issue #1210) -- the
    /// matching route's `request` table, converted to JSON. `None` when the
    /// route configured none.
    pub request: Option<serde_json::Value>,
}

/// One priced prefix, as [`Connector::client_route_prices`] enumerates them
/// for the node self-description (ADR 0050).
///
/// Prefix, price, `request` and transport policy only. What else a route has
/// -- where it terminates, which peer it forwards to, what that peering costs
/// this node -- is either an app fact or an operator-private one, and neither
/// belongs in a document a stranger reads.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientRoutePrice {
    pub prefix: String,
    pub price: Price,
    /// What a client should send to use this route (issue #1210), read back
    /// through the same [`Connector::client_route`] lookup `price` is, so a
    /// prefix that appears in more than one source is quoted with the
    /// request table a real request to it would resolve.
    pub request: Option<serde_json::Value>,
    /// Which client carriage a request to this prefix must arrive on
    /// (TOON_Network issue #111), carried here for exactly the reason
    /// `price` is: it comes off the same [`Connector::client_route`] lookup,
    /// and that lookup is the one both client-edge carriages refuse a wrong
    /// carriage from. There is no second value for the advertisement to
    /// disagree with the enforcement about.
    pub transport_policy: TransportPolicy,
}

/// The connector's packet plane: a fixed set of terminated routes and peer
/// routes, an [`AppClient`] port for delivering to the apps behind
/// terminated routes, a [`PeerTransport`] port for forwarding to the next
/// hop on peer routes, and a [`Clock`] port rather than wall time.
///
/// A router (`connector-client-edge`) deserializes a request into a
/// [`Prepare`], calls exactly one method here -- [`Connector::handle_prepare`]
/// -- and serializes the result. Every routing and delivery decision is made
/// in that one method; the router makes none.
pub struct Connector {
    routes: Vec<StaticRoute>,
    peer_routes: Vec<PeerRoute>,
    /// Routes pushed at runtime over the operator surface with a time
    /// limit (ADR 0006, issue #427), keyed by prefix so pushing the same
    /// prefix again renews it rather than adding a duplicate entry. Lives
    /// only in memory: unlike `routes` and `peer_routes`, nothing here is
    /// loaded from configuration, so none of it survives a restart.
    ///
    /// Held as an atomically-swapped immutable snapshot rather than a
    /// `RwLock<HashMap<..>>` (ADR 0015, issue #452): the packet path reads
    /// the current map with a single lock-free `Arc` clone, never a lock
    /// and never a copy of every leased route, so hot-path cost does not
    /// scale with how many leases happen to be active. A write (lease
    /// creation or renewal) publishes a whole new map rather than mutating
    /// this one in place -- the rare, administrative side is where the
    /// O(n) copy belongs, not the per-packet side.
    leased_routes: ArcSwap<HashMap<String, LeasedRoute>>,
    app_client: Arc<dyn AppClient>,
    peer_transport: Arc<dyn PeerTransport>,
    clock: Arc<dyn Clock>,
    metrics: Arc<Metrics>,
    /// The real chains' settlement backends (issue #459), keyed by the
    /// chain each one settles on (issue #630: a node with both
    /// `[settlement.evm]` and `[settlement.solana]` holds *both* -- a
    /// single slot would leave whichever attached first silently
    /// unreachable while the node kept accepting its claims). At most one
    /// backend per chain, in attachment (= config) order; empty on a node
    /// that configured none, where channel operations fail with
    /// [`ChannelOperationError::NoSettlementBackend`] rather than being
    /// unreachable, matching how `leased_routes` degrades to "just empty"
    /// rather than a distinct construction path.
    settlements: Vec<(SettlementChain, Arc<dyn SettlementBackend>)>,
    /// Every channel this node has itself opened, in the order opened,
    /// each remembering the chain it was opened on.
    /// `SettlementBackend` has no "list every channel" method (a real
    /// chain has no such index either) -- this is the one thing
    /// `Connector` itself has to remember so `channels()` knows which ids
    /// to ask which backend to report on.
    known_channels: RwLock<Vec<(SettlementChain, ChannelId)>>,
    /// Claims owed to and received from every peering relation (ADR 0004,
    /// ADR 0005, issue #423): signing an outbound claim on fulfilment,
    /// verifying and watermarking an inbound one. Empty and signer-less
    /// until configured via [`Connector::with_signer`] and
    /// [`Connector::with_channel_verification_key`] -- a node with none of
    /// those simply never emits or accepts a claim, matching how
    /// `settlement` degrades to `None`.
    pub(crate) claims: ClaimBook,
    /// This connector's own identity key (ADR 0018, ADR 0022), used to open
    /// a gift wrap sealed to it (issue #524) -- distinct from `claims`'s
    /// signer, which signs outbound claims to a peer rather than performing
    /// key agreement. `None` on a node that hasn't configured one, in which
    /// case every packet routed to an app route is refused: per ADR 0018
    /// every packet's `data` is a gift wrap, so a node that cannot open one
    /// cannot terminate any app route at all, matching how `settlement`
    /// degrades to "every channel operation refuses" rather than "channel
    /// operations silently no-op".
    identity_signer: Option<Arc<dyn Signer>>,
    /// Gates [`Connector::handle_probe`] (issue #426, ADR 0011): a fixed
    /// window of probe attempts admitted per sender identity. Defaults to
    /// [`DEFAULT_PROBE_LIMIT`] per [`default_probe_window`], overridable via
    /// [`Connector::with_probe_rate_limit`] -- unlike `settlement`/`claims`
    /// above, this fails *closed* rather than open: probing pays nothing,
    /// so a node that never configures a limit still gets one rather than
    /// unbounded free traversal.
    probe_rate_limiter: FixedWindowRateLimiter,
    /// Payment channels this connector has seen a valid claim on at its own
    /// client edge (issue #548), and therefore recognizes as belonging to a
    /// sender that holds a channel with it -- the other half of
    /// [`Connector::handle_probe`]'s first gate, beside `claims`'s
    /// configured peer-role verification keys. Without this the gate is
    /// unsatisfiable on a deployed node: nothing in a node's configuration
    /// supplies a client's channel id, and a gate no node can pass is not a
    /// gate (ADR 0011's "accepted only from a sender that already holds an
    /// open payment channel with this connector"). Populated by
    /// [`Connector::recognize_channel`], which the client edge calls when a
    /// claim clears its gate.
    recognized_channels: RwLock<HashSet<String>>,
    /// This node's OUTBOUND client ledger (issue #873): the nonce line it
    /// signs claims on when it pays a next hop **as an ordinary client of
    /// that hop**, rather than as its configured peer.
    ///
    /// Deliberately not `claims` above, and the two must never merge.
    /// `claims` is the inbound journal, where this node is the authority on
    /// what it accepted; this one's authority is the RECEIVER, asked over
    /// [`crate::ClaimStateSource`] every time. See
    /// `crate::outbound_client`'s header for the full table.
    ///
    /// `None` on a node that has not been given one, in which case the
    /// forwarding path simply has no client role available to it -- the
    /// same way `settlements` degrades to "every channel operation
    /// refuses" rather than to a second construction path.
    outbound_client: Option<Arc<OutboundClientLedger>>,
    /// What this node needs in order to pay each next hop **as an ordinary
    /// client of it** (issue #875), keyed by peer id: the channel it holds
    /// with that hop, and the hop itself as the authority on where that
    /// channel's claims stand.
    ///
    /// Empty on a node nobody configured a client role for, in which case a
    /// greeted forward is relayed as the refusal it is rather than covered
    /// -- the same "degrade to just empty" shape `settlements` and
    /// `leased_routes` take.
    ///
    /// Held as an atomically-swapped immutable snapshot, like
    /// `runtime_peers` (issue #1217): `POST /peers` (ADR 0058) registers a
    /// hop live, from a handler running concurrently with the packet path,
    /// so a plain `HashMap` behind `&self` was never soundly writable here
    /// -- only construction-time `with_outbound_client_hop` ever wrote it
    /// before this, which is exactly why a runtime peering could accept but
    /// never pay.
    outbound_client_hops: ArcSwap<HashMap<String, OutboundClientHop>>,
    /// Peer ids this node's config file names (`[[peers]]`), threaded in
    /// via [`Connector::with_config_peer_ids`) purely as a reservation
    /// list (issue #884): the routing table IS the relationship set
    /// enforced at load (`connector-config`'s `UnknownPeerId` check), so a
    /// runtime write must never be able to add, update or remove a peer id
    /// the config file already owns -- config wins, and never silently.
    /// `Connector` otherwise has no reason to know these ids; it never
    /// stored peer identity before #884 (see [`PeerView`]'s history) and
    /// still stores nothing about a config peer beyond its id here.
    config_peer_ids: HashSet<String>,
    /// Peer ids added at runtime over the operator surface (issue #884).
    /// Read-mostly, like `leased_routes` (ADR 0015) -- an `ArcSwap` so
    /// `handle_prepare`'s hot path (which consults this only indirectly,
    /// through `runtime_peer_routes`' referential integrity already having
    /// been checked at write time) never locks. Unlike `leased_routes`,
    /// this is durable: every write is persisted to `runtime_store` before
    /// being published here, so it survives a restart -- the whole point
    /// of #884 versus #427's lease mechanism. A config-file peer id
    /// (`config_peer_ids`) never appears here at all: config always wins,
    /// by refusing the runtime write (ADR 0034).
    runtime_peers: ArcSwap<RuntimePeers>,
    /// Peer-forwarding routes added at runtime over the operator surface
    /// (issue #884), keyed by prefix like `leased_routes` -- but durable,
    /// and stored as a plain [`PeerRoute`] with no expiry of its own: an
    /// operator's row lapses when the operator removes it, never on a
    /// clock. Participates in
    /// `select_configured_route`/`client_route` exactly like a
    /// config-file peer route: matching is still longest-prefix-first
    /// with the same priority tie-break (issue #884's acceptance
    /// criterion "no change to how packets are matched") -- only the data
    /// source is new.
    runtime_peer_routes: ArcSwap<HashMap<String, PeerRoute>>,
    /// Serializes every runtime peer/route table WRITE (never taken by a
    /// read) so persisting to `runtime_store` and publishing the new
    /// `ArcSwap` snapshot happen exactly once per write. Deliberately not
    /// `ArcSwap::rcu` here (unlike `leased_routes`' `upsert_leased_route`):
    /// `rcu`'s closure may run more than once under contention, and this
    /// closure would perform a disk write -- rcu is only safe for a pure
    /// in-memory transform, which persisting to disk is not.
    runtime_table_lock: Mutex<()>,
    /// ADR 0042's **cap**, keyed by peer id: the largest amount this
    /// connector will forward to that peer in a single packet, from its
    /// `[[peers]]` row's `max_packet_amount`.
    ///
    /// A peer with no entry here is capped at
    /// [`connector_config::DEFAULT_MAX_PACKET_AMOUNT`] rather than
    /// uncapped -- the same "a bound exists even if nobody configured one"
    /// shape `probe_rate_limiter` already takes, and
    /// the reason this is a plain map with a defaulted lookup
    /// ([`Self::packet_cap_for`]) rather than an `Option`. That covers a
    /// peer added at runtime over the operator surface (issue #884), which
    /// has no config row to read a cap off at all and is exactly the
    /// counterparty ADR 0042 says starts at the floor.
    ///
    /// **Per packet, never an accumulation.** Nothing here counts what a
    /// peer has already been sent -- ADR 0033 deleted the exposure ceiling
    /// and this is not it (`CONTEXT.md` keeps "ceiling" and "cap" apart for
    /// this reason). The cap is checked against one packet's own forwarded
    /// amount and forgotten.
    peer_packet_caps: HashMap<String, u64>,
    /// ADR 0010's flat per-packet **fee**, keyed by peer id: what this
    /// connector retains for carrying one packet to that peer, from its
    /// `[[peers]]` row's `fee`.
    ///
    /// Keyed by peering rather than carried on the route because this hop
    /// does the same work whichever prefix the packet was addressed to (ADR
    /// 0061) -- `[[routes]] fee` is a refuse-to-start tombstone now. A peer
    /// with no entry here charges nothing, which is what an operator who
    /// wrote no fee meant; a peer added at runtime over the operator
    /// surface carries its own fee on its row instead
    /// (`runtime_peers`), and [`Self::fee_for`] reads whichever holds it.
    ///
    /// Flat and per packet, never a share of the amount: it is realized on
    /// the wire as the difference between the amount that arrived and the
    /// amount forwarded, and added to the accumulated cost of a reject this
    /// peer itself decided on (ADR 0011).
    peer_fees: HashMap<String, u64>,
    /// Reads another node's self-description so a peering can be
    /// established from a URL (ADR 0058). Defaults to
    /// [`UnreachableSelfDescription`]: a node whose builder never gave it
    /// one refuses `POST /peers` by name rather than hanging or panicking,
    /// the same "degrade to a named refusal" every other unconfigured port
    /// on this connector takes.
    ///
    /// **Never read on the packet path.** The only caller is
    /// [`Connector::establish_peering`], which is reached from one
    /// authenticated operator write.
    self_description: Arc<dyn SelfDescriptionSource>,
    /// Adds and removes a dial carriage while this process serves (ADR
    /// 0058). `None` on a node whose transport cannot be changed at
    /// runtime -- every in-process test harness, and any deployment whose
    /// peerings all come from the config file -- and then a peering
    /// established at runtime is durable and unreachable until the next
    /// boot wires it.
    peer_registrar: Option<Arc<dyn PeerRegistrar>>,
    /// The node's own `peer_allow_plaintext_endpoints` (issue #678, gap 3),
    /// carried here because a peering established at runtime decides its
    /// carriage by the same rule a config-file peering does and must reach
    /// the same answer.
    peer_allow_plaintext: bool,
    /// Where the runtime peer/route table is written durably (issue #884).
    /// `None` on a node with no `state_dir` configured -- the table is
    /// still mutable, exactly like `leased_routes` always is, it simply
    /// does not survive a restart, the same "degrade to in-memory-only"
    /// every other `state_dir`-scoped store on this connector takes.
    runtime_store: Option<PeerRouteStore>,
    /// Which declared token each configured peering's channels hold (ADR
    /// 0071 decision 1, issue #1292) -- the fact that decides whether a
    /// forward crosses a **denomination boundary** at all.
    ///
    /// Empty on every node that declares no `[[tokens]]`, which is every
    /// config this repository ships: nothing resolves, no forward can be a
    /// crossing, and [`Self::forward_via_peer_route`] runs the one
    /// subtraction it ran before ADR 0071 existed.
    ///
    /// Read on the packet path, which is why it is a pure function of
    /// loaded config held by value rather than anything with a lock or a
    /// chain behind it.
    peering_assets: PeeringAssets,
    /// Which declared token a client channel this node accepts claims on
    /// holds (ADR 0071 decision 1, issue #1301) -- the same fact
    /// [`Self::peering_assets`] holds for a peering, for the other kind of
    /// arrival a forward can come out of.
    ///
    /// Empty on every node that declares no `[[tokens]]`, exactly as its
    /// sibling is, and for the same reason. On a node that DOES deal it is
    /// the difference between a buyer's packet converting at this hop and
    /// crossing a real boundary at an implied 1:1, which is why it is keyed
    /// by the arriving channel key's chain rather than by any declared row:
    /// the buyer whose channel this node discovered on chain (ADR 0052,
    /// issue #502) has no row to be resolved from.
    client_channel_assets: ClientChannelAssets,
    /// The rates this node deals at (ADR 0071 decisions 1 and 5, issue
    /// #1294), or `None` for a node that declares no token to deal.
    ///
    /// `None` is not "convert at par": a node holding no table refuses
    /// every crossing [`Self::peering_assets`] reports, because decision
    /// 2's absence rule is the safety rule -- no declared rate, no
    /// conversion, no forward. The two fields are therefore independent,
    /// and the combination that matters is the awkward one: a node that
    /// declares tokens but no rate row resolves boundaries it will not
    /// cross, and says so in a reject rather than passing an unconverted
    /// integer across a 10^12 scale difference.
    ///
    /// The read is [`SharedRateTable::lookup`], which is a plain `fn` --
    /// that is how "the forwarding path does no I/O" is kept true by
    /// construction rather than by discipline (decision 6).
    rate_table: Option<SharedRateTable>,
}

/// Which leg a PREPARE arrived over, and therefore which denomination its
/// amount is in (ADR 0071 decision 1, issues #1295 and #1301).
///
/// A packet's amount has no unit of its own -- it is denominated by the
/// channel it rides -- so this is the incoming half of a **denomination
/// boundary**, and the two variants are the two kinds of channel a packet
/// can have been paid for over. Neither is derived from the packet: a
/// PREPARE says nothing about what unit it is in and ADR 0071 decision 7
/// keeps it that way, so each carriage hands the connector the leg it
/// authenticated.
///
/// There is no third variant, and the absence is load-bearing: an arrival
/// with no channel behind it -- an operator write, a test calling
/// [`Connector::handle_prepare`] directly -- is `None` rather than a
/// variant of this, because there is nothing to resolve rather than a
/// denomination that happens to be unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrival<'a> {
    /// A peer arrival, named by the peering both carriages authenticate
    /// before handing the packet on. Resolved through
    /// [`Connector::peering_assets`].
    Peer(&'a str),
    /// A client-edge arrival, named by the chain-namespaced channel key of
    /// the claim that admitted it
    /// ([`ClientClaim::channel_key`](connector_domain::client_claim::ClientClaim::channel_key)).
    /// Resolved through [`Connector::client_channel_assets`], which reads
    /// the key's chain and not its id -- so a channel this node discovered
    /// on chain is denominated exactly like a declared one.
    ClientChannel(&'a str),
}

/// What a client-role hop's covering claims are signed under, by chain
/// (issue #1146).
///
/// Operator config either way, the same way [`ClaimBook::set_channel_domain`]
/// and [`ClaimBook::set_solana_channel`] are for the peer role on the very
/// same channel: a node that opened this channel already knows which
/// `TokenNetwork` -- or which settlement program -- it was deployed under,
/// so this is a configured input, not a guess.
#[derive(Clone, Copy)]
enum OutboundClientDomain {
    /// The channel's EIP-712 domain (issue #881). Used to COVER a forward
    /// proactively, before any greeting exists to read a domain off of;
    /// [`Connector::cover_greeted_packet`]'s reactive retry still reads the
    /// domain from the peer's own greeting, deliberately.
    Evm(EvmDomain),
    /// The settlement program this channel lives under (ADR 0053), which is
    /// `[settlement.solana] program_id` and nothing else -- since issue
    /// #1128 there is exactly one program a node can redeem a Solana claim
    /// through, so there is exactly one a channel can be signed under. That
    /// is also why the greeting is not consulted on the retry arm: unlike an
    /// EVM `TokenNetwork`, there is no second answer a receiver could give.
    Solana(SolanaDomain),
}

/// One next hop this connector can pay as a client (issue #875).
#[derive(Clone)]
struct OutboundClientHop {
    /// The channel this node's settlement address holds with the hop, as
    /// its raw on-chain 32 bytes -- an EVM `bytes32` channel id or a Solana
    /// channel account, whichever this hop's `domain` says...
    channel: [u8; 32],
    /// ...and as the spelling a claim names it by on the wire -- `0x`
    /// lower-case hex for EVM, base58 for Solana -- kept beside it so the
    /// packet path never re-renders it.
    channel_id: String,
    /// The hop, asked where this node's claims on that channel stand. The
    /// RECEIVER is the authority on its own watermark (see
    /// `crate::outbound_client`'s header); nothing local substitutes.
    claim_state: Arc<dyn ClaimStateSource>,
    /// What this hop's covering claims are bound to, and which chain's arm
    /// of the outbound client ledger signs them.
    domain: OutboundClientDomain,
}

/// [`Connector`]'s default probe rate limit absent
/// [`Connector::with_probe_rate_limit`] -- a deliberately conservative
/// figure (issue #426): probing costs a sender nothing, so the safe default
/// is a small allowance rather than none at all.
const DEFAULT_PROBE_LIMIT: u32 = 60;

/// [`Connector`]'s default probe rate limit window, paired with
/// [`DEFAULT_PROBE_LIMIT`].
/// `bytes` as lower-case hex, no `0x`.
fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn default_probe_window() -> Duration {
    Duration::seconds(60)
}

impl Connector {
    pub fn new(
        routes: Vec<StaticRoute>,
        peer_routes: Vec<PeerRoute>,
        app_client: Arc<dyn AppClient>,
        peer_transport: Arc<dyn PeerTransport>,
        clock: Arc<dyn Clock>,
    ) -> Connector {
        Connector {
            routes,
            peer_routes,
            leased_routes: ArcSwap::from_pointee(HashMap::new()),
            app_client,
            peer_transport,
            clock,
            metrics: Arc::new(Metrics::new()),
            settlements: Vec::new(),
            known_channels: RwLock::new(Vec::new()),
            claims: ClaimBook::new(None, HashMap::new(), HashMap::new()),
            identity_signer: None,
            probe_rate_limiter: FixedWindowRateLimiter::new(
                DEFAULT_PROBE_LIMIT,
                default_probe_window(),
            ),
            recognized_channels: RwLock::new(HashSet::new()),
            outbound_client: None,
            outbound_client_hops: ArcSwap::from_pointee(HashMap::new()),
            config_peer_ids: HashSet::new(),
            runtime_peers: ArcSwap::from_pointee(RuntimePeers::new()),
            runtime_peer_routes: ArcSwap::from_pointee(HashMap::new()),
            runtime_table_lock: Mutex::new(()),
            runtime_store: None,
            self_description: Arc::new(UnreachableSelfDescription),
            peer_registrar: None,
            peer_allow_plaintext: false,
            peer_packet_caps: HashMap::new(),
            peer_fees: HashMap::new(),
            peering_assets: PeeringAssets::default(),
            client_channel_assets: ClientChannelAssets::default(),
            rate_table: None,
        }
    }

    /// Give this node the port that reads another node's self-description,
    /// so `POST /peers` can establish a peering from a URL (ADR 0058).
    /// Without one every such write is refused by name.
    pub fn with_self_description_source(mut self, source: Arc<dyn SelfDescriptionSource>) -> Self {
        self.self_description = source;
        self
    }

    /// Give this node the port that adds and removes a dial carriage while
    /// it serves (ADR 0058), so a peering established at runtime is
    /// reachable without a restart.
    pub fn with_peer_registrar(mut self, registrar: Arc<dyn PeerRegistrar>) -> Self {
        self.peer_registrar = Some(registrar);
        self
    }

    /// Tell this node whether plaintext peer endpoints are permitted
    /// (issue #678, gap 3) -- the same node-wide opt-in `Config` holds, so
    /// a peering established at runtime chooses its carriage by exactly the
    /// rule a config-file peering does.
    pub fn with_peer_allow_plaintext_endpoints(mut self, allow: bool) -> Self {
        self.peer_allow_plaintext = allow;
        self
    }

    pub(crate) fn self_description_source(&self) -> &Arc<dyn SelfDescriptionSource> {
        &self.self_description
    }

    pub(crate) fn peer_allows_plaintext(&self) -> bool {
        self.peer_allow_plaintext
    }

    /// Refuse a peering write that could never land, **before** any
    /// outbound request is made: an empty id, or one the config file owns
    /// (ADR 0034 -- config wins by refusing the write). A stranger's host
    /// is not worth dialling on behalf of a write this node has already
    /// decided against.
    pub(crate) fn refuse_unlandable_peering(&self, id: &str) -> Result<(), PeerRouteTableError> {
        if id.trim().is_empty() {
            return Err(PeerRouteTableError::InvalidPeerId);
        }
        if self.config_peer_ids.contains(id) {
            return Err(PeerRouteTableError::OwnedByConfig(id.to_string()));
        }
        Ok(())
    }

    /// The settlement backend for `chain`, for the peering path.
    pub(crate) fn settlement_on_chain(
        &self,
        chain: SettlementChain,
    ) -> Result<Arc<dyn SettlementBackend>, ChannelOperationError> {
        self.settlement_on(chain).cloned()
    }

    /// Narrow a fetched self-description's published settlements to the one
    /// chain this connector will derive a channel on.
    pub(crate) fn shared_settlement(
        &self,
        document: &connector_domain::NodeSelfDescription,
        wanted: Option<SettlementChain>,
        url: &url::Url,
    ) -> Result<crate::peering::SharedSettlement, crate::peering::EstablishPeeringError> {
        crate::peering::shared_settlement_of(
            document,
            |chain| self.settlement_on(chain).is_ok(),
            wanted,
            url,
        )
    }

    /// Register a runtime peering's channel binding with this node's claim
    /// book, so a claim on it is judged from the moment the peering is
    /// established rather than from the next boot.
    ///
    /// **PEER role only** -- a counterparty key and a signing domain for
    /// what arrives on this channel. It also names this channel as the one
    /// `ClaimBook`'s own (now inbound-only, issue #1145) `outbound_channel`
    /// bookkeeping tracks for `peer_id`, but that is not the CLIENT role
    /// ADR 0042 needs to pay a forward: this call alone leaves a runtime
    /// peering able to accept a claim and unable to sign one. See
    /// [`Connector::register_outbound_client_hop`] for the client-role half
    /// (issue #1217) -- `establish_peering` and boot rehydration both call
    /// this and that together, the same way `connector_config::pay_channel`
    /// documents the deployed shape as both roles on one channel.
    ///
    /// A binding whose identifiers do not parse is skipped rather than
    /// coerced: an unparseable channel id names no channel, and a book
    /// entry filed under a mangled key would accept claims for a channel
    /// that does not exist.
    pub(crate) fn bind_runtime_peer_channel(&self, peer_id: &str, binding: &RuntimePeerChannel) {
        match binding {
            RuntimePeerChannel::Evm {
                channel_id,
                counterparty_key,
                chain_id,
                token_network,
            } => {
                let (Some(counterparty), Some(token_network_address)) = (
                    crate::peering::parse_evm_address(counterparty_key),
                    crate::peering::parse_evm_address(token_network),
                ) else {
                    tracing::warn!(
                        peer_id,
                        "peering published an EVM address this node cannot read; \
                         its channel is not bound"
                    );
                    return;
                };
                if let Err(error) = self.claims.set_channel_domain(
                    channel_id,
                    ChannelDomain {
                        chain_id: *chain_id,
                        token_network_address,
                    },
                ) {
                    tracing::warn!(peer_id, %error, "peering's channel id is not bindable");
                    return;
                }
                self.claims.set_verification_key(channel_id, counterparty);
                self.claims.set_outbound_channel(peer_id, channel_id);
            }
            RuntimePeerChannel::Solana {
                channel_account,
                counterparty_key,
                program_id,
            } => {
                if let Err(error) =
                    self.claims
                        .set_solana_channel(channel_account, counterparty_key, program_id)
                {
                    tracing::warn!(peer_id, %error, "peering's Solana channel is not bindable");
                    return;
                }
                self.claims.set_outbound_channel(peer_id, channel_account);
            }
        }
    }

    /// Register a runtime peering's channel binding as an outbound CLIENT
    /// hop (issue #1217, ADR 0042/0058): the missing counterpart of
    /// [`Connector::bind_runtime_peer_channel`] that lets
    /// [`Connector::cover_forward`] actually pay `peer_id` for a forward,
    /// live from the moment the peering is established rather than only
    /// after the next boot rewires `[[pay_channels]]` from a config file.
    ///
    /// `client_edge_url` is [`RuntimePeering::client_edge_url`] --
    /// `POST /ilp/claim-state` is always asked over plain HTTP (issue
    /// #1146's `OwnedHttpClaimState`), whichever carriage the packet
    /// itself rides.
    ///
    /// Skipped -- with the reason logged, never silently -- for exactly the
    /// failure modes `bind_runtime_peer_channel` already tolerates (an
    /// address or channel id this node cannot read), plus one of its own:
    /// no settlement signer configured for this binding's chain. A node
    /// with no `[settlement.<chain>]` table cannot pay a next hop on that
    /// chain as a client, the same way it cannot open a channel on it
    /// either -- the peering is left accept-only on that chain rather than
    /// refused outright, since the peer role this call does not touch may
    /// still be perfectly usable.
    pub(crate) fn register_outbound_client_hop(
        &self,
        peer_id: &str,
        binding: &RuntimePeerChannel,
        client_edge_url: &str,
    ) {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(
                connector_config::DEFAULT_PEER_TIMEOUT_MS,
            ))
            .build()
            .expect("a reqwest client with only a timeout set always builds");
        let hop = match binding {
            RuntimePeerChannel::Evm {
                channel_id,
                chain_id,
                token_network,
                ..
            } => {
                let Some(signer) = self.claims.signer().cloned() else {
                    tracing::warn!(
                        peer_id,
                        "no EVM settlement signer configured; cannot pay this peer as a client"
                    );
                    return;
                };
                let Some(token_network) = crate::peering::parse_evm_address(token_network) else {
                    tracing::warn!(
                        peer_id,
                        "peering's token network is not an address this node can read; its \
                         outbound client hop is not registered"
                    );
                    return;
                };
                let Ok(channel) = crate::claim::parse_channel_id(channel_id) else {
                    tracing::warn!(
                        peer_id,
                        "peering's channel id is not bindable; its outbound client hop is not \
                         registered"
                    );
                    return;
                };
                OutboundClientHop {
                    channel,
                    channel_id: channel_id.clone(),
                    claim_state: Arc::new(OwnedHttpClaimState::new(
                        client,
                        client_edge_url,
                        ClaimStateChallengeSigner::Evm(signer),
                    )),
                    domain: OutboundClientDomain::Evm(EvmDomain {
                        chain_id: *chain_id,
                        token_network,
                    }),
                }
            }
            RuntimePeerChannel::Solana {
                channel_account,
                program_id,
                ..
            } => {
                let Some(signer) = self.claims.solana_signer().cloned() else {
                    tracing::warn!(
                        peer_id,
                        "no Solana settlement signer configured; cannot pay this peer as a client"
                    );
                    return;
                };
                let Ok(channel) = crate::claim::parse_base58_32("channel account", channel_account)
                else {
                    tracing::warn!(
                        peer_id,
                        "peering's Solana channel account is not bindable; its outbound client \
                         hop is not registered"
                    );
                    return;
                };
                let Ok(program_id) = crate::claim::parse_base58_32("program id", program_id) else {
                    tracing::warn!(
                        peer_id,
                        "peering's Solana program id is not bindable; its outbound client hop \
                         is not registered"
                    );
                    return;
                };
                OutboundClientHop {
                    channel,
                    channel_id: channel_account.clone(),
                    claim_state: Arc::new(OwnedHttpClaimState::new(
                        client,
                        client_edge_url,
                        ClaimStateChallengeSigner::Solana(signer),
                    )),
                    domain: OutboundClientDomain::Solana(SolanaDomain { program_id }),
                }
            }
        };
        self.outbound_client_hops.rcu(|current| {
            let mut next = (**current).clone();
            next.insert(peer_id.to_string(), hop.clone());
            next
        });
    }

    /// Make a runtime peering dialable, if this node holds a registrar.
    pub(crate) fn register_runtime_peering(&self, peer_id: &str, peering: &RuntimePeering) {
        if let Some(registrar) = &self.peer_registrar {
            registrar.register(peer_id, peering);
        }
    }

    /// Give each named peering the cap its `[[peers]]` row configures (ADR
    /// 0042): the largest amount this connector will forward to it in one
    /// packet. Every peer left out of `caps` -- including one added at
    /// runtime over the operator surface (issue #884) -- keeps
    /// [`connector_config::DEFAULT_MAX_PACKET_AMOUNT`], so this only ever
    /// overrides a bound that already exists; there is no call that removes
    /// one.
    pub fn with_peer_packet_caps(mut self, caps: impl IntoIterator<Item = (String, u64)>) -> Self {
        self.peer_packet_caps = caps.into_iter().collect();
        self
    }

    /// The most this connector will forward to `peer_id` in one packet --
    /// its configured cap, or [`DEFAULT_MAX_PACKET_AMOUNT`] for a peering
    /// that states none.
    ///
    /// Two sources, never overlapping, read in the same order and for the
    /// same reason [`Self::fee_for`] reads them: the config file's
    /// `[[peers]]` rows (`peer_packet_caps`) and the runtime peer table,
    /// which can never hold an id the config file owns (ADR 0034). A
    /// runtime peering states its cap on its own row, written there by
    /// `POST /peers` (ADR 0058) -- a peering established at runtime is
    /// exactly the counterparty ADR 0049 says an operator must be able to
    /// set a cap for, and the row it lands on is this one.
    ///
    /// A stated cap of zero is "this row states none", not "forward
    /// nothing": there is no call anywhere that removes a bound, and a
    /// peering that could be silently capped at zero would go dark on a
    /// default rather than on a decision.
    fn packet_cap_for(&self, peer_id: &str) -> u64 {
        if let Some(cap) = self.peer_packet_caps.get(peer_id) {
            return *cap;
        }
        match self.runtime_peers_snapshot().get(peer_id) {
            Some(peering) => crate::peering::stated_cap(peering.max_packet_amount),
            None => DEFAULT_MAX_PACKET_AMOUNT,
        }
    }

    /// Give each named peering the flat per-packet fee its `[[peers]]` row
    /// configures (ADR 0010, ADR 0061): what this connector retains for
    /// carrying one packet to it. A peering left out of `fees` charges
    /// nothing -- and one added at runtime carries its fee on its own row
    /// instead, which [`Self::fee_for`] reads.
    pub fn with_peer_fees(mut self, fees: impl IntoIterator<Item = (String, u64)>) -> Self {
        self.peer_fees = fees.into_iter().collect();
        self
    }

    /// What this connector retains for carrying one packet to `peer_id` --
    /// the peering's own flat fee (ADR 0010, ADR 0061), zero for a peering
    /// that configured none.
    ///
    /// Two sources, never overlapping: the config file's `[[peers]]` rows
    /// (`peer_fees`) and the runtime peer table (`runtime_peers`), which
    /// can never hold an id the config file owns (ADR 0034). The runtime
    /// snapshot is loaded only on a config miss, so a config peering -- the
    /// only kind either devnet box has -- costs the packet path one hash
    /// lookup and no `ArcSwap` load at all.
    pub(crate) fn fee_for(&self, peer_id: &str) -> u64 {
        match self.peer_fees.get(peer_id) {
            Some(fee) => *fee,
            None => self
                .runtime_peers_snapshot()
                .get(peer_id)
                .map_or(0, |peering| peering.fee),
        }
    }

    /// Tell this node which declared token each of its peerings holds (ADR
    /// 0071 decision 1, issues #1292, #1295) -- the table
    /// [`Config::peering_assets`](connector_config::Config::peering_assets)
    /// resolved once at boot.
    ///
    /// Without it every forward this node makes is a same-denomination
    /// forward, which is the truth for every node that declares no
    /// `[[tokens]]` and the reason the default is the empty table rather
    /// than an `Option`.
    pub fn with_peering_assets(mut self, assets: PeeringAssets) -> Self {
        self.peering_assets = assets;
        self
    }

    /// Tell this node which declared token a client channel it accepts
    /// claims on holds (ADR 0071 decision 1, issue #1301) -- the table
    /// [`Config::client_channel_assets`](connector_config::Config::client_channel_assets)
    /// resolved once at boot, and the client edge's half of
    /// [`Self::with_peering_assets`].
    ///
    /// Without it a buyer's packet crosses no boundary this hop can see and
    /// forwards at the arriving integer, which is the truth for every node
    /// that declares no `[[tokens]]` and the reason the default is the
    /// empty table rather than an `Option`. On a node that deals, leaving
    /// it out would be the unconverted crossing ADR 0071 exists to prevent
    /// -- which is why `connector-cli` sets it wherever it sets the
    /// peering table.
    pub fn with_client_channel_assets(mut self, assets: ClientChannelAssets) -> Self {
        self.client_channel_assets = assets;
        self
    }

    /// Give this node the rate table its converting forwards read (ADR 0071
    /// decision 6, issues #1294, #1295) -- the handle a background poller
    /// writes through and every crossing reads.
    ///
    /// Giving a node a table does not make it deal: what it deals is
    /// whatever the table holds a live row for, and a boundary with no row
    /// is refused exactly as it is on a node with no table at all.
    /// Withholding one is therefore not a way to turn conversion off, only
    /// a way to make every crossing refuse.
    pub fn with_rate_table(mut self, table: SharedRateTable) -> Self {
        self.rate_table = Some(table);
        self
    }

    /// The **denomination boundary** a forward that `arrived` over one leg
    /// and leaves to `outgoing_peer_id` crosses, or `None` when it crosses
    /// none (ADR 0071 decision 1, issues #1295 and #1301).
    ///
    /// Both kinds of arrival are denominated, because both are: a peer's
    /// packet is denominated by the peering's channel and a buyer's by the
    /// client channel its covering claim was written against, and the
    /// question asked of either is the identical one -- does the token it
    /// arrived in differ from the token the outgoing peering holds.
    ///
    /// `None` covers three cases that all forward the same way -- the two
    /// legs hold one token, this node declares none, or the packet named no
    /// arriving leg at all. That last one is the case worth stating: an
    /// operator write, or any caller of [`Self::handle_prepare`] itself,
    /// carries no channel and therefore no unit, so nothing can be said
    /// about what it arrived in and this node forwards it as it always did.
    /// A client-edge delivery is no longer one of those: since issue #1301
    /// it carries the channel key its claim cleared, and on a dealing node
    /// that key resolves -- including when the channel was discovered on
    /// chain rather than declared (ADR 0052), which is the case that would
    /// otherwise cross a real boundary unconverted.
    fn crossing(
        &self,
        arrived: Option<Arrival<'_>>,
        outgoing_peer_id: &str,
    ) -> Option<(&AssetId, &AssetId)> {
        let incoming = match arrived? {
            Arrival::Peer(peer_id) => self.peering_assets.asset(peer_id)?,
            Arrival::ClientChannel(channel_key) => self.client_channel_assets.asset(channel_key)?,
        };
        self.peering_assets
            .boundary_from(incoming, outgoing_peer_id)
    }

    /// Reserve every peer id this node's config file names (issue #884):
    /// the routing table IS the relationship set enforced at load
    /// (`connector-config`'s `UnknownPeerId` check), so a runtime write
    /// naming one of these ids is refused rather than allowed to shadow
    /// or be shadowed by the config-file row of the same id.
    pub fn with_config_peer_ids(mut self, ids: impl IntoIterator<Item = String>) -> Self {
        self.config_peer_ids = ids.into_iter().collect();
        self
    }

    /// Replay a durable runtime peer/route table (issue #884) into this
    /// connector and arm it to persist future writes back to the same
    /// store -- the two must always be given together, since a table
    /// replayed from `peers`/`routes` but not armed to persist further
    /// writes would silently stop being durable after the first mutation.
    /// Every replayed peering is also **re-armed**: its channel binding
    /// goes back into the claim book and its carriage back into the
    /// registrar, exactly as establishing it did. A durable row that came
    /// back as a name would be the hollow row ADR 0058 exists to remove,
    /// one restart later. Call this after
    /// [`Connector::with_peer_registrar`], or the carriages replay
    /// nowhere.
    ///
    /// Also re-registers each binding's outbound CLIENT hop (issue #1217),
    /// so a restart does not turn a payable runtime peering back into an
    /// accept-only one -- the same "everything establishing it did" claim
    /// this doc already makes, extended to the role `establish_peering`
    /// only started arming here. A row with channels but no
    /// `client_edge_url` -- the shape every row written before issue #1217
    /// has -- replays exactly as it always did: peer role bound, client
    /// role absent, accept-only until re-peered.
    pub fn with_runtime_peer_route_store(
        mut self,
        store: PeerRouteStore,
        peers: RuntimePeers,
        routes: HashMap<String, PeerRoute>,
    ) -> Self {
        for (id, peering) in &peers {
            for binding in &peering.channels {
                self.bind_runtime_peer_channel(id, binding);
                if let Some(client_edge_url) = &peering.client_edge_url {
                    self.register_outbound_client_hop(id, binding, client_edge_url);
                }
            }
            self.register_runtime_peering(id, peering);
        }
        self.runtime_peers = ArcSwap::from_pointee(peers);
        self.runtime_peer_routes = ArcSwap::from_pointee(routes);
        self.runtime_store = Some(store);
        self
    }

    /// Give this node an outbound client ledger (issue #873) so the
    /// forwarding path can pay a next hop it holds no matched credential
    /// with, as an ordinary client of that hop.
    ///
    /// Pass the FILE-BACKED form ([`OutboundClientLedger::open`]) here: a
    /// serving node restarts, and a restart that reissued a nonce would
    /// fork its own outbound nonce line. Its path must not be either
    /// journal file -- this book is not a `JournalEntry` stream, and the
    /// two ledgers must never merge.
    pub fn with_outbound_client_ledger(mut self, ledger: Arc<OutboundClientLedger>) -> Self {
        self.outbound_client = Some(ledger);
        self
    }

    /// This node's outbound client ledger, or `None` when it was never
    /// given one -- the packet path's own read of
    /// [`Connector::with_outbound_client_ledger`].
    pub fn outbound_client_ledger(&self) -> Option<&Arc<OutboundClientLedger>> {
        self.outbound_client.as_ref()
    }

    /// Configure how this node pays `peer_id` **as an ordinary client of
    /// it** (issue #875): the channel it holds with that hop, and the hop
    /// itself as the source of that channel's watermark.
    ///
    /// Deliberately separate from the PEER-role binding a `[[peer_channels]]`
    /// row makes, even where both name the same channel id. That one is now
    /// INBOUND ONLY: its outbound half signed a claim against this node's own
    /// `ClaimBook` projection once a forward had fulfilled, which was ADR
    /// 0004's postpay model and was deleted in issue #1145. This one configures the
    /// CLIENT role, whose watermark authority is the receiver, asked over
    /// `claim_state` every time. The two books must never merge (see
    /// `crate::outbound_client`'s header), and configuring them apart is
    /// where that starts.
    ///
    /// Without this -- or without [`Connector::with_outbound_client_ledger`]
    /// -- a peer that answers a forward with x402 terms simply gets its
    /// refusal relayed: there is nothing to pay it with, and a packet is
    /// never emitted claiming to have paid when it has not.
    ///
    /// Configuring a hop here also switches [`Connector::forward_via_peer_route`]
    /// (issue #881) to cover **every** packet forwarded to `peer_id` from
    /// this ledger proactively, rather than waiting for a `pending_claim`
    /// watermark the peer ledger's own postpay convention (ADR 0004) would
    /// otherwise arm. A hop this is never called for is entirely
    /// unaffected: it keeps riding `pending_claim` exactly as before this
    /// method existed.
    pub fn with_outbound_client_hop(
        self,
        peer_id: impl Into<String>,
        channel_id: impl Into<String>,
        domain: EvmDomain,
        claim_state: Arc<dyn ClaimStateSource>,
    ) -> Result<Self, InvalidChannelId> {
        let channel_id = channel_id.into();
        let channel = crate::claim::parse_channel_id(&channel_id)?;
        let mut hops = (*self.outbound_client_hops.load_full()).clone();
        hops.insert(
            peer_id.into(),
            OutboundClientHop {
                channel,
                // The canonical spelling (`peer-carriage-spec.md` §4.1): a
                // claim naming the same channel in two casings is two
                // watermarks at the far gate.
                channel_id: format!("0x{}", hex_lower(&channel)),
                claim_state,
                domain: OutboundClientDomain::Evm(domain),
            },
        );
        self.outbound_client_hops.store(Arc::new(hops));
        Ok(self)
    }

    /// [`Connector::with_outbound_client_hop`] for a **Solana** next hop
    /// (issue #1146): the channel account this node pays that hop from, and
    /// the settlement program its claims are signed under.
    ///
    /// A second method rather than a chain parameter, exactly as
    /// [`ClaimBook::set_solana_channel`] is a second method beside
    /// [`ClaimBook::set_channel_domain`]: the two chains name a channel with
    /// different kinds of identifier, and a shared entry point would have to
    /// accept both spellings for both chains and sort them out afterwards.
    ///
    /// `program_id` is `[settlement.solana] program_id` -- ADR 0053 signs it
    /// into every claim on this channel, and issue #1128 made it the single
    /// source for the peer role on the very same channel. Both it and
    /// `channel_account` must be base58 of exactly 32 bytes; anything else is
    /// refused here rather than turned into a claim verified against the
    /// wrong message.
    ///
    /// **Until this existed, a Solana peering could only be paid postpay**
    /// -- `cover_forward` had no arm to mint under, so the claim covering
    /// crossing n rode crossing n + 1, which is the model ADR 0042 exists to
    /// retire.
    pub fn with_solana_outbound_client_hop(
        self,
        peer_id: impl Into<String>,
        channel_account: impl Into<String>,
        program_id: &str,
        claim_state: Arc<dyn ClaimStateSource>,
    ) -> Result<Self, InvalidSolanaChannel> {
        let channel_account = channel_account.into();
        let channel = crate::claim::parse_base58_32("channel account", &channel_account)?;
        let program_id = crate::claim::parse_base58_32("program id", program_id)?;
        let mut hops = (*self.outbound_client_hops.load_full()).clone();
        hops.insert(
            peer_id.into(),
            OutboundClientHop {
                channel,
                // Re-encoded from the bytes rather than kept as written, so
                // the wire carries one spelling of the account however the
                // operator's config spelled it -- the base58 counterpart of
                // the EVM arm's lower-casing above.
                channel_id: bs58::encode(channel).into_string(),
                claim_state,
                domain: OutboundClientDomain::Solana(SolanaDomain { program_id }),
            },
        );
        self.outbound_client_hops.store(Arc::new(hops));
        Ok(self)
    }

    /// Configure this node's own identity key (issue #524), used to open a
    /// gift wrap sealed to it -- the same identity `connector-client-edge`
    /// reports at `GET /ilp/identity` (ADR 0022). Without one configured, a
    /// packet routed to an app route is refused rather than delivered,
    /// since there is nothing to open it with.
    pub fn with_identity_signer(mut self, signer: Arc<dyn Signer>) -> Self {
        self.identity_signer = Some(signer);
        self
    }

    /// Override the default probe rate limit (issue #426, ADR 0011): up to
    /// `max_per_window` probe attempts per sender identity within `window`,
    /// checked against this connector's own injected clock.
    pub fn with_probe_rate_limit(mut self, max_per_window: u32, window: Duration) -> Self {
        self.probe_rate_limiter = FixedWindowRateLimiter::new(max_per_window, window);
        self
    }

    /// Configure this node's own signer (issue #423), used to sign every
    /// outbound claim. Without one configured, a fulfilled packet still
    /// forwards and fulfils normally -- it simply never emits a claim,
    /// matching how a node with no settlement backend never gets a working
    /// channel surface.
    pub fn with_signer(mut self, signer: Arc<dyn Signer>) -> Self {
        self.claims.set_signer(signer);
        self
    }

    /// Configure the EVM address whose signature this node accepts on an
    /// inbound claim for `channel_id` (issue #423, peer-semantics-pre-868.md §1.1's
    /// "a configured peer id and verification key"; issue #575: this is now
    /// the channel's counterparty *address*, recovered from an EIP-712
    /// `BalanceProof` signature, not a raw public key checked against a
    /// connector-internal digest). Pair with
    /// [`Connector::with_channel_domain`] for the same `channel_id` -- a
    /// claim naming a channel with no domain configured is refused
    /// regardless of this.
    pub fn with_channel_verification_key(
        self,
        channel_id: impl Into<String>,
        counterparty: Address,
    ) -> Self {
        self.claims.set_verification_key(channel_id, counterparty);
        self
    }

    /// Configure `channel_id`'s EIP-712 signing domain -- the chain it is
    /// deployed on and the `TokenNetwork` contract that verifies a claim's
    /// signature on redemption (issue #575/#566). Required, alongside
    /// a `[[peer_channels]]` row or
    /// [`Connector::with_channel_verification_key`], before this channel
    /// can sign or accept a claim -- see [`ClaimBook::set_channel_domain`].
    /// `channel_id` must already be the channel's on-chain `bytes32`, refused
    /// otherwise rather than hashed or truncated into shape.
    pub fn with_channel_domain(
        self,
        channel_id: impl Into<String>,
        domain: ChannelDomain,
    ) -> Result<Self, InvalidChannelId> {
        self.claims.set_channel_domain(channel_id, domain)?;
        Ok(self)
    }

    /// Configure `channel_account`'s Solana peer binding (issue #732/#998):
    /// the base58 Ed25519 public key whose signature this node accepts on a
    /// claim for it. The Solana counterpart of both
    /// [`Connector::with_channel_verification_key`] and
    /// [`Connector::with_channel_domain`] in one call -- see
    /// [`ClaimBook::set_solana_channel`] for why the two cannot be
    /// separated on this chain.
    pub fn with_solana_channel(
        self,
        channel_account: impl Into<String>,
        counterparty_public_key: &str,
        program_id: &str,
    ) -> Result<Self, InvalidSolanaChannel> {
        self.claims
            .set_solana_channel(channel_account, counterparty_public_key, program_id)?;
        Ok(self)
    }

    /// Configure this node's own ed25519 identity (issue #742/#998), used to
    /// sign every outbound claim on a channel registered via
    /// [`Connector::with_solana_channel`] -- the Solana counterpart of
    /// [`Connector::with_signer`].
    pub fn with_solana_signer(mut self, signer: Arc<dyn Ed25519Signer>) -> Self {
        self.claims.set_solana_signer(signer);
        self
    }

    /// Configure the settlement backend a node's channel-lifecycle writes
    /// (issue #459) are driven against on `chain` -- callable once per
    /// chain (issue #630), so a node with both `[settlement.evm]` and
    /// `[settlement.solana]` holds both backends rather than whichever
    /// attached last. Attaching a second backend for the same chain
    /// replaces the first, matching how config load already refuses two
    /// tables for one chain. A builder rather than a [`Connector::new`]
    /// parameter deliberately -- most of this crate's own tests, and every
    /// other crate constructing a bare `Connector` today, have no
    /// settlement backend at all and shouldn't need to thread one through
    /// just to keep compiling.
    pub fn with_settlement(
        mut self,
        chain: SettlementChain,
        settlement: Arc<dyn SettlementBackend>,
    ) -> Self {
        match self
            .settlements
            .iter_mut()
            .find(|(existing, _)| *existing == chain)
        {
            Some((_, slot)) => *slot = settlement,
            None => self.settlements.push((chain, settlement)),
        }
        self
    }

    /// Configure the durable journal this node's claim state is persisted
    /// to and rebuilt from (ADR 0005, issue #424). Call this *last* in the
    /// builder chain -- rebuild uses whatever signer is already configured
    /// to re-arm any outbound claim left unacknowledged (see
    /// `ClaimBook::rebuild_from`'s own doc for why that is always safe).
    pub fn with_journal(mut self, journal: Arc<dyn Journal>) -> Result<Self, JournalError> {
        self.claims.set_journal(journal)?;
        Ok(self)
    }

    /// Create or renew a leased route (ADR 0006, issue #427): a controller
    /// outside this connector pushes a route to a peer with a time limit,
    /// keyed by `prefix`. Calling this again for a prefix already leased
    /// renews it -- `expires_at` is always computed as `ttl` from this
    /// node's own injected clock, never from the caller, so a controller
    /// cannot claim a longer lease than this node's clock allows.
    pub fn upsert_leased_route(
        &self,
        prefix: impl Into<String>,
        peer_id: impl Into<String>,
        ttl: Duration,
    ) -> Result<LeasedRouteView, LeaseRouteError> {
        let prefix = prefix.into();
        if !is_valid_ilp_address(&prefix) {
            return Err(LeaseRouteError::InvalidPrefix(prefix));
        }
        let expires_at = self.clock.now() + ttl;
        let route = LeasedRoute::new(prefix.clone(), peer_id.into(), expires_at);
        let view = leased_route_view(&route);
        self.leased_routes.rcu(|current| {
            let mut next = (**current).clone();
            next.insert(prefix.clone(), route.clone());
            next
        });
        Ok(view)
    }

    /// The current leased-route map, snapshotted as of this call (issue
    /// #452): `ArcSwap::load_full` is a single atomic `Arc`
    /// clone -- no lock and no copy of the routes themselves -- and a
    /// concurrent `upsert_leased_route` publishes an entirely new map
    /// rather than mutating the one this snapshot points at, so the
    /// snapshot stays valid for as long as its caller holds it.
    fn leased_routes_snapshot(&self) -> Arc<HashMap<String, LeasedRoute>> {
        self.leased_routes.load_full()
    }

    /// Leased routes not yet lapsed as of the injected clock, for the
    /// operator surface's read-only inspection interface. Expiry is
    /// filtered fresh on every call -- a lapsed route disappears from this
    /// list the moment it disappears from routing, with no sweep delay in
    /// between (issue #427).
    pub fn leased_routes(&self) -> Vec<LeasedRouteView> {
        let now = self.clock.now();
        self.leased_routes_snapshot()
            .values()
            .filter(|route| !is_expired(route.expires_at(), now))
            .map(leased_route_view)
            .collect()
    }

    /// The current runtime-peer table, snapshotted as of this call -- see
    /// [`Self::leased_routes_snapshot`]'s identical reasoning (issue
    /// #452/ADR 0015): a single atomic `Arc` clone, no lock, no copy of
    /// the table's contents.
    fn runtime_peers_snapshot(&self) -> Arc<RuntimePeers> {
        self.runtime_peers.load_full()
    }

    fn runtime_peer_routes_snapshot(&self) -> Arc<HashMap<String, PeerRoute>> {
        self.runtime_peer_routes.load_full()
    }

    /// Persist `peers`/`routes` to `runtime_store` if this node has one
    /// configured, otherwise a no-op -- the same "no `state_dir`, no
    /// durability, still mutable" degrade every other `state_dir`-scoped
    /// store on this connector takes. Called with the write lock already
    /// held, before the corresponding `ArcSwap` is published, so the
    /// durable copy and the in-memory table can never disagree about
    /// which write is current.
    fn persist_runtime_table(
        &self,
        peers: &RuntimePeers,
        routes: &HashMap<String, PeerRoute>,
    ) -> Result<(), PeerRouteTableError> {
        match &self.runtime_store {
            Some(store) => store
                .persist(peers, routes)
                .map_err(PeerRouteTableError::from),
            None => Ok(()),
        }
    }

    /// Add or update a runtime peering (issue #884, ADR 0058): the durable
    /// half of `POST /peers`.
    ///
    /// Refused by name -- never silently accepted as a no-op -- when `id`
    /// is empty, when it already belongs to the config file
    /// (`docs/adr/0034-a-runtime-peer-route-table-never-shadows-the-config-file.md`),
    /// or when `peering` carries **no payment-channel binding**. That last
    /// one is the runtime twin ADR 0058 requires of
    /// `connector-config`'s load-time `PeerChannelUnbound`: a peering with
    /// no channel can never take the peer role (ADR 0060), so writing one
    /// would record a relationship that behaves as a stranger and says so
    /// nowhere.
    ///
    /// Calling this again for an id already in the runtime table replaces
    /// that row wholesale, matching `upsert_leased_route`'s own
    /// renew-by-reinsertion shape. It is how a peering is repriced, and --
    /// because ADR 0059 derives the same channel from the same two
    /// participants -- how repeating `POST /peers` lands on the peering
    /// that already exists rather than a second one.
    pub fn upsert_runtime_peer(
        &self,
        id: impl Into<String>,
        peering: RuntimePeering,
    ) -> Result<PeerView, PeerRouteTableError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(PeerRouteTableError::InvalidPeerId);
        }
        if self.config_peer_ids.contains(&id) {
            return Err(PeerRouteTableError::OwnedByConfig(id));
        }
        if peering.channels.is_empty() {
            return Err(PeerRouteTableError::PeerChannelUnbound(id));
        }
        let view = PeerView {
            id: id.clone(),
            fee: peering.fee,
            max_packet_amount: crate::peering::stated_cap(peering.max_packet_amount),
            source: RouteSource::Runtime,
        };
        let _write_guard = self
            .runtime_table_lock
            .lock()
            .expect("runtime peer/route table lock poisoned");
        let mut peers = (*self.runtime_peers_snapshot()).clone();
        peers.insert(id, peering);
        self.persist_runtime_table(&peers, &self.runtime_peer_routes_snapshot())?;
        self.runtime_peers.store(Arc::new(peers));
        Ok(view)
    }

    /// The runtime peering `id` names, or `None` for an id this table does
    /// not hold. A config-file peering is never here (ADR 0034 keeps the
    /// two tables disjoint by refusing a collision).
    #[must_use]
    pub fn runtime_peering(&self, id: &str) -> Option<RuntimePeering> {
        self.runtime_peers_snapshot().get(id).cloned()
    }

    /// Remove a runtime peer row (issue #884): `DELETE /peers/:id`.
    /// Refused when `id` belongs to the config file (config rows are
    /// never removable at runtime, the same "config always wins" rule
    /// `upsert_runtime_peer` enforces on insert), when no such runtime
    /// peer exists, or when a runtime route still forwards to it -- the
    /// orphaned-row shape `connector-config`'s `UnknownPeerId` check
    /// exists to prevent at load, enforced here instead at mutation time.
    pub fn remove_runtime_peer(&self, id: &str) -> Result<(), PeerRouteTableError> {
        if self.config_peer_ids.contains(id) {
            return Err(PeerRouteTableError::OwnedByConfig(id.to_string()));
        }
        let _write_guard = self
            .runtime_table_lock
            .lock()
            .expect("runtime peer/route table lock poisoned");
        let peers = self.runtime_peers_snapshot();
        if !peers.contains_key(id) {
            return Err(PeerRouteTableError::PeerNotFound(id.to_string()));
        }
        let routes = self.runtime_peer_routes_snapshot();
        if routes.values().any(|route| route.peer_id() == id) {
            return Err(PeerRouteTableError::PeerInUse(id.to_string()));
        }
        let mut next_peers = (*peers).clone();
        next_peers.remove(id);
        self.persist_runtime_table(&next_peers, &routes)?;
        self.runtime_peers.store(Arc::new(next_peers));
        // ADR 0060 named `DELETE /peers` as the kill switch that replaced
        // revoking a shared secret: "immediate, does not require a
        // restart". It is only immediate if the carriage goes with the row.
        if let Some(registrar) = &self.peer_registrar {
            registrar.deregister(id);
        }
        Ok(())
    }

    /// Whether `prefix` is defined by the config file, as either an app
    /// route or a peer-forwarding route -- the set of prefixes a runtime
    /// write may never add, update or remove.
    fn config_owns_prefix(&self, prefix: &str) -> bool {
        self.config_prefixes().any(|owned| owned == prefix)
    }

    /// Every prefix the config file itself serves: app routes and peer
    /// forwarding routes.
    fn config_prefixes(&self) -> impl Iterator<Item = &str> {
        self.routes
            .iter()
            .map(|route| route.prefix())
            .chain(self.peer_routes.iter().map(|route| route.prefix()))
    }

    /// Add or update a runtime peer-forwarding route (issue #884):
    /// `POST /routes/peers`, keyed by `prefix` exactly like
    /// `upsert_leased_route`, so posting the same prefix again updates
    /// the row rather than adding a duplicate. Refused when `prefix` is
    /// not a valid ILP address, when it is defined by the config file
    /// (app route or peer route alike), or when `peer_id` resolves to no
    /// known peer -- the config file's or the runtime table's --
    /// mirroring `connector-config`'s load-time `UnknownPeerId` check.
    ///
    /// A **runtime** peering is additionally required to have a channel to
    /// pay from ([`PeerRouteTableError::PeerHasNoPayChannel`]) -- the
    /// runtime twin of ADR 0042's `[[pay_channels]]` load rule, which ADR
    /// 0058 requires enforced continuously rather than once at boot. That
    /// checks the CLIENT-role hop (`outbound_client_hops`, issue #1217),
    /// not `peering.channels` -- the PEER-role bindings, which are
    /// non-empty for every peering `establish_peering` ever writes and so
    /// never caught the gap this guard exists for: a peering that can
    /// accept a claim but hold nothing to sign one with. A config-file
    /// peering is not re-checked here: `Config::load` already refused to
    /// start without the row, and this table cannot see it.
    pub fn upsert_runtime_peer_route(
        &self,
        prefix: impl Into<String>,
        peer_id: impl Into<String>,
        price: Price,
    ) -> Result<PeerRouteView, PeerRouteTableError> {
        let prefix = prefix.into();
        let peer_id = peer_id.into();
        if !is_valid_ilp_address(&prefix) {
            return Err(PeerRouteTableError::InvalidPrefix(prefix));
        }
        if self.config_owns_prefix(&prefix) {
            return Err(PeerRouteTableError::OwnedByConfig(prefix));
        }
        let _write_guard = self
            .runtime_table_lock
            .lock()
            .expect("runtime peer/route table lock poisoned");
        if !self.config_peer_ids.contains(&peer_id) {
            match self.runtime_peers_snapshot().get(&peer_id) {
                None => return Err(PeerRouteTableError::UnknownPeerId { prefix, peer_id }),
                Some(_) if !self.outbound_client_hops.load_full().contains_key(&peer_id) => {
                    return Err(PeerRouteTableError::PeerHasNoPayChannel { prefix, peer_id })
                }
                Some(_) => {}
            }
        }
        let route = PeerRoute::new_scheduled(prefix.clone(), peer_id.clone(), price);
        let mut routes = (*self.runtime_peer_routes_snapshot()).clone();
        routes.insert(prefix.clone(), route);
        self.persist_runtime_table(&self.runtime_peers_snapshot(), &routes)?;
        self.runtime_peer_routes.store(Arc::new(routes));
        Ok(PeerRouteView {
            prefix,
            peer_id,
            price,
            source: RouteSource::Runtime,
        })
    }

    /// Remove a runtime peer-forwarding route (issue #884):
    /// `DELETE /routes/peers/:prefix`. Refused when `prefix` is defined by
    /// the config file, or when no such runtime route exists.
    pub fn remove_runtime_peer_route(&self, prefix: &str) -> Result<(), PeerRouteTableError> {
        if self.config_owns_prefix(prefix) {
            return Err(PeerRouteTableError::OwnedByConfig(prefix.to_string()));
        }
        let _write_guard = self
            .runtime_table_lock
            .lock()
            .expect("runtime peer/route table lock poisoned");
        let mut routes = (*self.runtime_peer_routes_snapshot()).clone();
        if routes.remove(prefix).is_none() {
            return Err(PeerRouteTableError::RouteNotFound(prefix.to_string()));
        }
        self.persist_runtime_table(&self.runtime_peers_snapshot(), &routes)?;
        self.runtime_peer_routes.store(Arc::new(routes));
        Ok(())
    }

    /// Every peer-forwarding route this node knows, config-file and
    /// runtime alike (issue #884), for the operator surface's
    /// `GET /routes/peers`. Deliberately excludes a leased route --
    /// `GET /routes/leased` already reports those, on a different
    /// lifecycle (a TTL, not a durable row).
    pub fn peer_routes_view(&self) -> Vec<PeerRouteView> {
        let mut views: Vec<PeerRouteView> = self
            .peer_routes
            .iter()
            .map(|route| PeerRouteView {
                prefix: route.prefix().to_string(),
                peer_id: route.peer_id().to_string(),
                price: route.price(),
                source: RouteSource::Config,
            })
            .collect();
        views.extend(
            self.runtime_peer_routes_snapshot()
                .values()
                .map(|route| PeerRouteView {
                    prefix: route.prefix().to_string(),
                    peer_id: route.peer_id().to_string(),
                    price: route.price(),
                    source: RouteSource::Runtime,
                }),
        );
        views
    }

    /// This connector's own metrics (ADR 0014), for the operator surface's
    /// `GET /metrics` (bearer-token gated, same as any other read).
    pub fn metrics(&self) -> Arc<Metrics> {
        self.metrics.clone()
    }

    /// Reject `prepare` outright if it isn't even eligible for routing --
    /// already past its expiry as of the injected clock -- checked before
    /// any route is selected or any app/peer is touched, so an expired
    /// packet never reaches either.
    ///
    /// Until issue #1269 this also refused a missing/all-zero execution
    /// condition (issue #417). That arm is gone along with the field: there
    /// is no longer a condition for a PREPARE to omit, and a bootstrap probe
    /// is now distinguished by the explicit `greeting` flag, checked at the
    /// client edge (`connector-client-edge`'s `handle_ilp`/`handle_frame`)
    /// before a packet ever reaches this method.
    fn reject_ineligible(&self, prepare: &Prepare) -> Option<Reject> {
        if is_expired(prepare.expires_at, self.clock.now()) {
            return Some(Reject {
                code: RejectCode::r00_transfer_timed_out(),
                triggered_by: String::new(),
                message: "prepare has expired".to_string(),
                data: Vec::new(),
                accumulated_cost: 0,
            });
        }
        None
    }

    /// Reject `prepare` outright if it fails [`Self::reject_ineligible`];
    /// otherwise route it by longest-prefix match over terminated routes and
    /// peer routes together, then either deliver it to the matching app or
    /// forward it to the matching peer -- and translate whatever comes back
    /// into the ILP-level response a client receives.
    ///
    /// Forwarding to a peer subtracts that peering relation's flat fee
    /// from `prepare.amount` (ADR 0010); a packet that does not even cover
    /// that fee is refused `R01` -- RFC 0027's Insufficient Source Amount,
    /// the standard meaning that survives ADR 0057's retirement of the
    /// minimum-delivery one -- rather than forwarded at zero. Nothing
    /// here checks a declared floor: the packet carries none since ADR
    /// 0057 (issue #1143), and what bounds erosion is the claim covering
    /// each crossing -- `cover_forward` mints for the forwarded value, so
    /// every hop holds a claim for at least what it passes on. Delivering
    /// to this connector's own app takes no fee -- a fee is earned per
    /// peering relation, not for terminating traffic at your own
    /// destination.
    ///
    /// Carries no client channel id in its `"packet"` span -- see
    /// [`Self::handle_prepare_with_client_channel`] for the client edge's
    /// entry point, which does.
    pub async fn handle_prepare(&self, prepare: Prepare) -> PacketResponse {
        self.handle_prepare_with_client_channel(prepare, None).await
    }

    /// Same as [`Self::handle_prepare`], but additionally records
    /// `client_channel_id` in the `"packet"` span alongside
    /// `correlation_id` and `destination` (issue #535, ADR 0036): the
    /// client channel whose covering claim admitted this packet -- the
    /// honest successor to the relay's retired payer-attribution header,
    /// naming the channel whose journal entries and
    /// `[[client_channels]]`/chain-resolved record say "who paid for this
    /// delivery" (ADR 0036). Carries the chain-namespaced key
    /// [`connector_domain::client_claim::ClientClaim::channel_key`] produces
    /// (`evm:<channel id>`, `solana:<channel account>`), so a claim on
    /// either chain names its channel unambiguously.
    ///
    /// `None` when no client claim admitted this packet (an unclaimed
    /// request, a peer-role arrival, or a caller using
    /// [`Self::handle_prepare`] directly) -- the field is then simply
    /// absent from the span, not recorded empty.
    pub async fn handle_prepare_with_client_channel(
        &self,
        prepare: Prepare,
        client_channel_id: Option<&str>,
    ) -> PacketResponse {
        // That same channel key is this packet's DENOMINATION (ADR 0071
        // decision 1, issue #1301), and it is the same value for the same
        // reason: the channel whose covering claim admitted the packet is
        // the channel the buyer paid over, and a claim is denominated by
        // the channel it is written against. One value, read twice, so a
        // span and a conversion can never name different channels.
        self.handle_prepare_spanned(
            prepare,
            client_channel_id,
            client_channel_id.map(Arrival::ClientChannel),
        )
        .await
    }

    /// The one body [`Self::handle_prepare_with_client_channel`] and
    /// [`Self::handle_peer_prepare`] share: open the `"packet"` span and
    /// route inside it.
    ///
    /// `arrived` is the leg this packet came in over, and is the incoming
    /// half of ADR 0071's denomination boundary (issues #1295, #1301) --
    /// the peering for a peer arrival, the client channel key for a buyer's
    /// own. See [`Self::crossing`] for what `None` means and why it is the
    /// safe answer rather than a gap.
    async fn handle_prepare_spanned(
        &self,
        prepare: Prepare,
        client_channel_id: Option<&str>,
        arrived: Option<Arrival<'_>>,
    ) -> PacketResponse {
        let span = tracing::info_span!(
            "packet",
            correlation_id = %correlation_id(),
            destination = %prepare.destination,
            client_channel_id = tracing::field::Empty,
        );
        if let Some(channel_id) = client_channel_id {
            span.record("client_channel_id", channel_id);
        }
        self.handle_prepare_traced(prepare, client_channel_id, arrived)
            .instrument(span)
            .await
    }

    /// The peer semantics's entry point (issue #423): accepts an inbound PREPARE
    /// exactly like [`Connector::handle_prepare`], but also verifies and
    /// watermarks whatever claim it carries (peer-semantics-pre-868.md §3.2).
    ///
    /// The claim outcome and the PREPARE outcome are independent -- a
    /// rejected claim does not reject the PREPARE it rode in on (§3.4), and
    /// this method decides neither from the other.
    ///
    /// Issue #752: a destination that resolves to one of this connector's
    /// own priced *terminated* routes is refused `F03_INVALID_AMOUNT`
    /// before the app is ever consulted if `prepare.amount` does not cover
    /// that route's `price` -- otherwise a peer forwarding into a priced
    /// route paid for by nothing (or by less than the route is worth) got
    /// the same free service ADR 0028 already closed off at the client
    /// edge. This is a per-packet gate, not a relation-wide throttle
    /// (`peer-semantics-pre-868.md` §5.4): it is answered from the amount already
    /// on this PREPARE via the same `client_route` lookup the client edge
    /// prices with (ADR 0028) and leaves the claim exchange itself (§3.2)
    /// untouched, and carries no x402 greeting of its own: since issue #880
    /// that greeting is emitted one layer up, by each accept pipeline's
    /// price-coverage gate (`peer-carriage-spec.md` §3.1), which refuses a
    /// peer PREPARE whose claim does not cover this same `price` before
    /// this method is ever reached. A route priced at `0` (an operator's
    /// deliberate free termination, ADR 0020) never trips this check.
    /// `arrived_from` names the peering this PREPARE came in over, which is
    /// the incoming half of ADR 0071's **denomination boundary** (issue
    /// #1295): a forward out of this packet converts when that peering and
    /// the outgoing one hold different tokens. Both carriages authenticate
    /// the peer before calling this and pass `Some`; the in-process
    /// transport models no such identity and passes `None`, which makes
    /// every forward out of it a same-denomination forward. It is a
    /// parameter rather than something derived from the claim because a
    /// peer PREPARE routinely carries none -- both accept pipelines judge
    /// the claim above this method and hand it on already decided.
    pub async fn handle_peer_prepare(
        &self,
        arrived_from: Option<&str>,
        prepare: Prepare,
        claim: Option<WireClaim>,
    ) -> (PacketResponse, ClaimAckOutcome) {
        let ack = claim.map_or(ClaimAckOutcome::NotSent, |claim| {
            self.handle_peer_claim(claim)
        });

        if let Some(route) = self.client_route(&prepare.destination) {
            // ADR 0029's per-packet coverage rule, read off the schedule at
            // this packet's own payload length (ADR 0065): the peer measures
            // the sealed wrap it was handed, which is the same figure the
            // termination below will charge and the same one the sending
            // hop's own edge collected against.
            let charge = route.price.charge(prepare.data.len());
            if route.kind == ClientRouteKind::Terminated && charge > 0 && prepare.amount < charge {
                let reject = PacketResponse::Reject(Reject {
                    code: RejectCode::f03_invalid_amount(),
                    triggered_by: String::new(),
                    message: format!(
                        "'{}' costs {} but the peer arrival carried only {}",
                        prepare.destination, charge, prepare.amount
                    ),
                    data: Vec::new(),
                    accumulated_cost: 0,
                });
                return (self.finish(reject), ack);
            }
        }

        let response = self
            .handle_prepare_spanned(prepare, None, arrived_from.map(Arrival::Peer))
            .await;
        (response, ack)
    }

    /// Record that `channel_id` names a payment channel this connector
    /// recognizes (issue #548) -- the client edge calls this the moment a
    /// claim on that channel clears its own gate (structure, freshness,
    /// value, and signature against the counterparty recorded for that
    /// channel, issue #558), which is the only evidence a connector ever
    /// gets that a sender is actually *using* a channel with it. Since
    /// #558 a connector does hold prior configuration about such a channel
    /// -- it must already record whose signature it accepts there, or no
    /// claim on it could verify -- but that says only which key may spend,
    /// never that anyone has; and no chain offers an index of who has (the
    /// same reason `known_channels` exists).
    ///
    /// Idempotent, and deliberately not undone -- a channel that has closed
    /// simply retains a probe allowance it can no longer pay with, which
    /// costs nothing beyond the rate limit that gates it anyway.
    pub fn recognize_channel(&self, channel_id: &str) {
        let mut recognized = self
            .recognized_channels
            .write()
            .expect("recognized channels lock poisoned");
        if !recognized.contains(channel_id) {
            recognized.insert(channel_id.to_string());
        }
    }

    /// Where a channel this node holds as a **peer** channel stands in the
    /// book that actually judges its claims -- [`crate::ClaimBook`]'s own
    /// inbound watermark -- or `None` when this node holds no peer channel
    /// by that name.
    ///
    /// A connector keeps two inbound books (`crate::outbound_client`'s
    /// header has the table), and which one is the authority for a channel
    /// is a property of the **channel**, never of who is asking: a claim
    /// naming a `[[peer_channels]]` row is judged here, and
    /// `Config::load` refuses a channel that is also a `[[client_channels]]`
    /// row (`ConfigError::ChannelInBothNamespaces`), so at most one book
    /// can ever be the authority. That is what lets
    /// `POST /ilp/claim-state` answer a payer correctly without knowing
    /// which role the payer holds.
    ///
    /// `Some(Watermark { nonce: 0, cumulative_amount: 0 })` for a
    /// configured peer channel nothing has claimed on yet -- deliberately
    /// not `None`, which would send the caller back to the wrong book for
    /// exactly the first claim on a channel.
    ///
    /// Covers both chains: an EVM `channel_id` (`0x` + 64 hex) and a
    /// Solana `channel_account` (base58) are drawn from disjoint spellings,
    /// so one lookup cannot answer for the other chain by accident.
    #[must_use]
    pub fn peer_channel_watermark(&self, channel_id: &str) -> Option<Watermark> {
        if !self.claims.has_verification_key(channel_id)
            && !self.claims.has_solana_channel(channel_id)
        {
            return None;
        }
        Some(
            self.claims
                .inbound_watermark(channel_id)
                .unwrap_or(Watermark {
                    nonce: 0,
                    cumulative_amount: 0,
                }),
        )
    }

    /// Whether `channel_id` is a channel this connector recognizes: either
    /// a peer channel whose verification key its operator configured
    /// ([`Connector::with_channel_verification_key`]), or a client channel
    /// [`Connector::recognize_channel`] recorded when a claim on it
    /// verified at this connector's client edge.
    pub fn recognizes_channel(&self, channel_id: &str) -> bool {
        self.claims.has_verification_key(channel_id)
            || self
                .recognized_channels
                .read()
                .expect("recognized channels lock poisoned")
                .contains(channel_id)
    }

    /// Every client channel [`Connector::recognize_channel`] has recorded
    /// (issue #1218): the only list of these this connector keeps, since a
    /// client channel is opened by its counterparty, never by this node, so
    /// it is never in [`Self::known_channels`]. `GET /channels` merges this
    /// list in so a node holding value from the client edge is reported on
    /// the same surface `POST /channels` populates for a channel this node
    /// opened itself.
    pub fn recognized_channel_ids(&self) -> Vec<String> {
        self.recognized_channels
            .read()
            .expect("recognized channels lock poisoned")
            .iter()
            .cloned()
            .collect()
    }

    /// Whether `channel_account` is a Solana peer channel this
    /// connector recognizes -- the Solana counterpart of
    /// [`Connector::recognizes_channel`] (issue #732/#998). A separate
    /// query rather than folded into `recognizes_channel`: an EVM
    /// `channel_id` and a Solana `channel_account` are drawn from disjoint
    /// spellings (`0x` + 64 hex vs. base58), so there is no name either
    /// chain's config could hand this connector that the other chain would
    /// also recognize.
    pub fn recognizes_solana_channel(&self, channel_account: &str) -> bool {
        self.claims.has_solana_channel(channel_account)
    }

    /// Entry point for a probe -- an ordinary packet a sender expects to be
    /// rejected, sent purely to learn a path's cost via the
    /// `accumulated_cost` every [`connector_domain::Reject`] now carries
    /// (issue #426, ADR 0011). Probes are not a distinct packet type and
    /// fee accumulation is not a special mode for them (ADR 0011): past the
    /// gates below this is [`Connector::handle_prepare`], routed by the
    /// ordinary routing table, and whatever comes back is reported exactly
    /// as it would be for traffic that never called this method at all.
    ///
    /// Unlike [`Connector::handle_prepare`]/[`Connector::handle_peer_prepare`],
    /// a probe is gated *before* routing is attempted: probing traverses
    /// this connector's network for free, so it is accepted only from
    /// `channel_id` identifying a payment channel this connector already
    /// recognizes (`ProbeDenied::NoOpenChannel` otherwise -- see
    /// [`Connector::recognizes_channel`] for what makes that satisfiable on
    /// a deployed node), and even then only within a rate limit per that
    /// identity (`ProbeDenied::RateLimited` otherwise) -- peer-semantics-pre-868.md
    /// §5.2's consequences, `docs/protocol/client-edge-spec.md` §1.6.
    /// Neither denial reaches [`Connector::handle_prepare`]: the packet is
    /// never forwarded.
    ///
    /// A probe is never *delivered* to a locally terminated route (issue
    /// #548). Free traversal is the whole of what ADR 0011 grants a probe;
    /// it does not also buy the work behind a priced route, which is what
    /// delivering here would hand over -- a sender able to seal a valid
    /// envelope would get the app's answer for nothing, the one thing ADR
    /// 0020's "an unpaid request to a priced route is answered with its
    /// terms" exists to prevent. A destination that terminates here is
    /// therefore answered with that route's price as `accumulated_cost`,
    /// which is exactly the figure a real request would be charged and, for
    /// a local termination, the whole path cost: no hop was traversed to
    /// reach it.
    ///
    /// A destination that *forwards* from here under a price (ADR 0028) is
    /// answered the same way, for both of the same reasons. The figure is
    /// the whole of what a real request would be charged at this edge and
    /// is known locally, so traversing to discover it would discover
    /// nothing; and a probe that traversed would make this connector
    /// forward the packet, sign a peer claim for the value it carried, and
    /// be paid nothing for it -- free traversal turned into free carriage.
    /// An unpriced forwarded route (`price = 0`, an operator's deliberate
    /// free carriage) still traverses and accumulates fees, which is ADR
    /// 0011's mechanism unchanged.
    pub async fn handle_probe(
        &self,
        channel_id: &str,
        prepare: Prepare,
    ) -> Result<PacketResponse, ProbeDenied> {
        if !self.recognizes_channel(channel_id) {
            return Err(ProbeDenied::NoOpenChannel);
        }
        if !self.probe_rate_limiter.allow(channel_id, self.clock.now()) {
            return Err(ProbeDenied::RateLimited);
        }
        if let Some(route) = self.client_route(&prepare.destination) {
            // A probe learns what the path costs *for a packet like the one
            // it sent* (ADR 0011, ADR 0065): the schedule is evaluated at the
            // probe's own payload length, so a sender that probes with the
            // body it means to send is told the figure it will be charged.
            // What makes one probe still answer every size is that the
            // schedule itself is published -- on the greeting and the node
            // self-description -- rather than only its value here.
            let price = route.price.charge(prepare.data.len());
            let answer_here = match route.kind {
                ClientRouteKind::Terminated => true,
                ClientRouteKind::Forwarded => price > 0,
            };
            if answer_here {
                let disposition = match route.kind {
                    ClientRouteKind::Terminated => "terminates at this connector",
                    ClientRouteKind::Forwarded => "forwards from this connector",
                };
                return Ok(PacketResponse::Reject(Reject {
                    code: RejectCode::f03_invalid_amount(),
                    triggered_by: String::new(),
                    message: format!(
                        "probe: '{}' {disposition} and costs {price}",
                        prepare.destination
                    ),
                    data: Vec::new(),
                    accumulated_cost: price,
                }));
            }
        }
        Ok(self.handle_prepare(prepare).await)
    }

    /// What this node's own record of a peer channel makes of `claim`'s
    /// signature -- `Ok(())`, [`ClaimRejectReason::UnknownChannel`] or
    /// [`ClaimRejectReason::SignatureInvalid`] -- with nothing accepted,
    /// no watermark advanced and nothing journaled.
    ///
    /// This is `peer-carriage-spec.md` §1.2's **P3**, and it is what
    /// decides role: a peer carriage asks this of the claim a frame
    /// carries, and admits the frame as a peer frame only on `Ok`. The
    /// check is against the counterparty key the `[[peer_channels]]` row
    /// configures, never against anything the claim declares about itself,
    /// which is the whole reason a signature proves more here than a
    /// bearer string ever did (ADR 0060).
    pub fn verify_peer_claim(&self, claim: &WireClaim) -> Result<(), ClaimRejectReason> {
        self.claims.verify_signature(claim)
    }

    /// Verify and, if valid, accept a claim received over the peer semantics --
    /// whether it rode a PREPARE or a FLUSH -- advancing its channel's
    /// watermark (issue #423, peer-semantics-pre-868.md §3.4).
    pub fn handle_peer_claim(&self, claim: WireClaim) -> ClaimAckOutcome {
        self.claims.accept_inbound(&claim)
    }

    async fn handle_prepare_traced(
        &self,
        prepare: Prepare,
        client_channel_id: Option<&str>,
        arrived: Option<Arrival<'_>>,
    ) -> PacketResponse {
        // Per-packet lines are debug, not info (issue #690): at huddle rates
        // (hundreds of packets/s) every INFO here becomes per-event disk I/O
        // through docker's json-file log driver -- the same disease as
        // relay#87's per-write console.log. The default `info` filter keeps
        // the hot path silent; RUST_LOG=connector_runtime=debug restores the
        // per-packet trace without a config change on the boxes.
        tracing::debug!("packet received");

        if let Some(reject) = self.reject_ineligible(&prepare) {
            return self.finish(PacketResponse::Reject(reject));
        }

        // Issue #452: `leased_routes_snapshot` is one lock-free `Arc`
        // clone. Every active route below is a reference borrowed
        // straight out of that snapshot -- expiry is still checked fresh
        // against the clock so a lapsed lease stops being selected
        // immediately, matching #427's guarantee, but nothing here
        // allocates a copy of the routes themselves.
        let leased_routes = self.leased_routes_snapshot();
        let now = self.clock.now();
        let active_leased: Vec<&LeasedRoute> = leased_routes
            .values()
            .filter(|route| !is_expired(route.expires_at(), now))
            .collect();

        let leased_prefixes: Vec<&str> = active_leased.iter().map(|route| route.prefix()).collect();

        // Configured routes -- terminated and forwarded -- are selected by
        // the same method the client edge's own price lookup calls (ADR
        // 0028), so what a packet is charged and where it is then sent can
        // never come from two different answers to the same question.
        let configured_match = self
            .select_configured_route(&prepare.destination)
            .map(|(len, target)| (len, target.into_route_target()));
        let leased_match = select_route(&prepare.destination, &leased_prefixes).map(|index| {
            (
                active_leased[index].prefix().len(),
                RouteTarget::Leased(index),
            )
        });

        let Some((_, target)) = [configured_match, leased_match]
            .into_iter()
            .flatten()
            .max_by_key(|(len, target)| (*len, target.rank()))
        else {
            return self.finish(PacketResponse::Reject(Reject {
                code: RejectCode::f02_unreachable(),
                triggered_by: String::new(),
                message: format!("no route to destination '{}'", prepare.destination),
                data: Vec::new(),
                accumulated_cost: 0,
            }));
        };

        // A `Cow`, not a plain clone (issue #884): the `RuntimePeer` arm
        // owns its [`PeerRoute`] already -- cloned once out of the runtime
        // snapshot inside `select_configured_route`, which drops that
        // snapshot before returning -- while the other two arms still
        // borrow straight out of the table that holds them, so a forwarded
        // packet allocates nothing here it did not before this route
        // source existed (ADR 0015).
        let peer_route: Cow<'_, PeerRoute> = match target {
            RouteTarget::App(index) => {
                tracing::debug!(handler_url = %self.routes[index].handler_url(), "routed to app");
                let response = self
                    .deliver_to_app(&self.routes[index], prepare, client_channel_id)
                    .await;
                return self.finish(response);
            }
            RouteTarget::Peer(index) => Cow::Borrowed(&self.peer_routes[index]),
            RouteTarget::Leased(index) => Cow::Borrowed(active_leased[index].as_peer_route()),
            RouteTarget::RuntimePeer(route) => Cow::Owned(route),
        };
        tracing::debug!(peer_id = %peer_route.peer_id(), "routed to peer");
        let response = self
            .forward_via_peer_route(&peer_route, prepare, arrived)
            .await;
        if matches!(response, PacketResponse::Fulfill(_)) {
            self.metrics
                .record_fee_earned(self.fee_for(peer_route.peer_id()));
        }
        self.finish(response)
    }

    /// Record the packet's final outcome -- metrics and a log line -- and
    /// pass it through unchanged. The single choke point every return path
    /// in [`Self::handle_prepare_traced`] goes through, so no outcome can be
    /// reported without also being counted.
    fn finish(&self, response: PacketResponse) -> PacketResponse {
        match &response {
            PacketResponse::Fulfill(_) => {
                self.metrics.record_fulfill();
                // debug, not info: fulfilment is the per-packet common case
                // (issue #690). Rejects below stay at info -- they are the
                // per-error path and keep the `packet` span's correlation
                // fields for diagnosis.
                tracing::debug!("packet fulfilled");
            }
            PacketResponse::Reject(reject) => {
                self.metrics.record_reject(reject.code.as_str());
                tracing::info!(code = %reject.code.as_str(), message = %reject.message, "packet rejected");
            }
        }
        response
    }

    /// Cross a **denomination boundary**: what this hop forwards to
    /// `peer_id`, in `outgoing`'s unit, for `amount` arriving in
    /// `incoming`'s -- or the reject that says why it will not
    /// ([ADR 0071](../../../docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)
    /// decisions 1 and 2, issue #1295).
    ///
    /// `floor(amount * rate) - fee`, with the fee in the outgoing leg's
    /// unit. The arithmetic is
    /// [`amount_after_rate_and_fee`]'s and lives in `connector-domain`
    /// where it can be property-tested without a connector around it; what
    /// is decided here is which rate, and which reject when there is none.
    ///
    /// # No I/O, and why that is structural
    ///
    /// The only thing consulted is [`SharedRateTable::lookup`], a plain
    /// synchronous `fn` over an `ArcSwap` (decision 6). This method is
    /// deliberately **not** `async`: a rate source reached from here would
    /// not compile, which is a stronger guarantee than watching a forward
    /// for sockets.
    ///
    /// # The two refusals, and why they are different codes
    ///
    /// ADR 0051's test is whether a sender can take a different next action
    /// on the code than on its class alone, and here it can -- the whole
    /// reason ADR 0071 insists the two refusals be distinguishable.
    ///
    /// * **Not declared** is `F02`, final: RFC 0027's own gloss is "there
    ///   was no way to forward the payment", which is exactly true of a
    ///   crossing this node has declared no price for, and ADR 0051's `F02`
    ///   row binds the move a sender makes about it -- *this path is wrong;
    ///   find another*. It is final because a declaration is config and
    ///   config is immutable for the process lifetime (ADR 0009): retrying
    ///   this packet at this hop cannot succeed, and the sender's only real
    ///   move is another path. Reusing the code is within ADR 0051's rule
    ///   rather than around it: the situation a sender can act on is the
    ///   same situation, since "no route" and "no price for the crossing
    ///   this route needs" leave a sender the identical next step.
    /// * **Stale** is `T00`, temporary: nothing about the packet is wrong
    ///   and nothing about the path is wrong -- this node's own poller has
    ///   failed to keep a price fresh, which is ADR 0051's `T00` situation
    ///   ("this connector's own configuration error ... retry later") and
    ///   ADR 0071's "staleness is an outage, on purpose". The class letter
    ///   is the instruction: `T` says retry, and a stale rate is precisely
    ///   the refusal worth retrying.
    ///
    /// `T04` and `R01` join them for the two ways the arithmetic itself
    /// refuses, and those two are **opposite** instructions, which is why
    /// they are told apart here rather than both reported as `R01`:
    ///
    /// * the converted amount does not fit the outgoing leg's `u64` -- ADR
    ///   0071's real ceiling, ~18.4 tokens on an 18-decimals leg -- and the
    ///   sender must send **less**. That is the cap's own situation and the
    ///   cap's own code (ADR 0049, ADR 0051: *send smaller, and the message
    ///   states the cap*), reached before the cap check only because there
    ///   is no representable amount left to compare against it.
    /// * the fee alone exceeds the converted amount and the sender must
    ///   send **more**: `R01`, the same question the unconverted arm asks,
    ///   now asked in the outgoing unit where decision 1 puts the fee.
    ///
    /// Telling a sender to send more when it must send less is worse than
    /// either refusal, and `amount_after_rate_and_fee` answers `None` to
    /// both; the second lookup below -- taken only on the refusal path, at
    /// a fee of zero, where the subtraction cannot be what failed -- is how
    /// the two are separated without restating the arithmetic here.
    fn convert_for_forward(
        &self,
        incoming: &AssetId,
        outgoing: &AssetId,
        peer_id: &str,
        amount: u64,
        fee: u64,
    ) -> Result<u64, Reject> {
        let undeclared = || Reject {
            code: RejectCode::f02_unreachable(),
            triggered_by: String::new(),
            message: format!(
                "forwarding to peer '{peer_id}' crosses a denomination boundary from {incoming} \
                 to {outgoing}, and this connector declares no rate for that pair: it will not \
                 convert at a rate it has not declared. Take another path"
            ),
            data: Vec::new(),
            accumulated_cost: 0,
        };

        // A node with no table at all is a node that declared no rate for
        // this pair, and answers the same way -- decision 2's absence rule
        // makes no distinction between "no row" and "no table", because
        // both are the operator not having said so.
        let Some(table) = &self.rate_table else {
            return Err(undeclared());
        };

        let rate = match table.lookup(incoming, outgoing, self.clock.now()) {
            RateLookup::Live { rate, .. } => rate,
            RateLookup::Stale { observed_at, .. } => {
                return Err(Reject {
                    code: RejectCode::t00_internal_error(),
                    triggered_by: String::new(),
                    message: format!(
                        "this connector's rate for {incoming} to {outgoing} was last observed at \
                         {} and is older than the ttl declared for that pair, so the crossing to \
                         peer '{peer_id}' is refused rather than dealt on a dead price. Retry \
                         once a fresh rate lands",
                        observed_at.to_rfc3339()
                    ),
                    data: Vec::new(),
                    accumulated_cost: 0,
                });
            }
            RateLookup::NotDeclared => return Err(undeclared()),
        };

        if let Some(forwarded_amount) = amount_after_rate_and_fee(amount, rate, fee) {
            return Ok(forwarded_amount);
        }

        // Which of the two refusals this was. At a fee of zero the
        // subtraction cannot fail, so `None` here is the outgoing leg's
        // `u64` ceiling and `Some` is the fee having eaten everything.
        match amount_after_rate_and_fee(amount, rate, 0) {
            Some(converted) => Err(Reject {
                code: RejectCode::r01_insufficient_source_amount(),
                triggered_by: String::new(),
                // Named in both units, because on a crossing the sender's
                // "send more" is a figure in its OWN unit and the fee is
                // not: `cost_before_rate_and_fee` un-converts the threshold
                // back across the boundary, which is decision 7's
                // arithmetic used one packet early.
                message: format!(
                    "peer '{peer_id}' charges a fee of {fee} in {outgoing}, and this packet's \
                     {amount} in {incoming} converts to only {converted} at the declared rate \
                     {rate}: nothing would be left to forward. Send more than {}",
                    cost_before_rate_and_fee(0, rate, fee)
                ),
                data: Vec::new(),
                accumulated_cost: 0,
            }),
            None => Err(Reject {
                code: RejectCode::t04_insufficient_liquidity(),
                triggered_by: String::new(),
                message: format!(
                    "peer '{peer_id}' has a maximum packet amount of {}, and this packet's \
                     {amount} in {incoming} converts at the declared rate {rate} to more than \
                     the {outgoing} leg can carry at all. Send less",
                    self.packet_cap_for(peer_id)
                ),
                data: Vec::new(),
                accumulated_cost: 0,
            }),
        }
    }

    /// Cross the same **denomination boundary** upstream: the running cost
    /// a reject carries back to the peering it arrived over, given `cost` in
    /// `outgoing`'s unit and this hop's own `fee`
    /// ([ADR 0071](../../../docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)
    /// decision 7, extending ADR 0011, issue #1296).
    ///
    /// `ceil((cost + fee) / rate)` -- [`cost_before_rate_and_fee`]'s
    /// arithmetic, the exact inverse of the forward's
    /// `floor(amount * rate) - fee`, with the fee added first because the
    /// fee is the outgoing peering's and the cost arrived in the outgoing
    /// unit too. The rounding goes up, against this connector, so the pair
    /// of roundings can overstate a probed cost by a base unit and can
    /// never understate one: a prober that pays what it was quoted clears.
    ///
    /// The wire is untouched by any of this. What changes is the unit the
    /// integer in `accumulated_cost` is denominated in -- which is the unit
    /// of the leg it is travelling on, as it was before this record and as
    /// every other amount on that leg already is -- so no reader ever meets
    /// a number whose unit it cannot already know, and no asset field is
    /// needed anywhere to say so.
    ///
    /// # A reject cannot be refused, so this answers a number
    ///
    /// [`Self::convert_for_forward`] can say no; this cannot. The packet is
    /// already gone, the reject is already travelling, and there is no one
    /// upstream for "I will not answer" to mean anything to. So the rate is
    /// re-read here -- the pair may have aged out of its ttl in the time the
    /// packet spent downstream -- and when the table no longer hands one
    /// back, the cost **saturates to `u64::MAX`** rather than travelling on
    /// unconverted.
    ///
    /// That is the only honest answer available, on three counts. It cannot
    /// understate, which is the one property a prober depends on and the
    /// property an unconverted integer would break by a factor of the whole
    /// decimals gap in one of the two directions. It is the saturation
    /// [`cost_before_rate_and_fee`] itself makes for a cost it cannot state,
    /// which is [`Price::charge`]'s doctrine: a cost no claim can cover
    /// refuses the packet, and refuses it without lying about the price. And
    /// it is *true* -- a pair with no live rate is a pair
    /// [`Self::convert_for_forward`] now refuses every forward across, so
    /// the path really does cost more than any packet can pay until a fresh
    /// rate lands. Quoting the last observed price instead would be dealing
    /// on a dead one, which is exactly what decision 5's ttl exists to stop.
    ///
    /// No I/O here either, for [`Self::convert_for_forward`]'s reason and
    /// asserted the same way ([`CrossesADenominationUpstream`]): a reject is
    /// as much the packet path as a forward is, and a hop that dialled a
    /// rate source to answer one would hold the reject open while it did.
    fn convert_for_reject(
        &self,
        incoming: &AssetId,
        outgoing: &AssetId,
        peer_id: &str,
        cost: u64,
        fee: u64,
    ) -> u64 {
        let Some(rate) = self
            .rate_table
            .as_ref()
            .and_then(|table| table.lookup(incoming, outgoing, self.clock.now()).rate())
        else {
            tracing::warn!(
                peer_id,
                %incoming,
                %outgoing,
                "a reject crossed back over a denomination boundary this connector no longer \
                 holds a live rate for -- reporting a cost no packet can pay rather than one \
                 in the wrong unit"
            );
            return u64::MAX;
        };

        cost_before_rate_and_fee(cost, rate, fee)
    }

    /// Forward `prepare` to `peer_route`'s peer, covered by a claim minted
    /// from the outbound CLIENT ledger before the packet is put on the wire
    /// (ADR 0042, issue #881). **There is no other way out of this method
    /// with a packet sent.** A peering this node cannot cover a forward to
    /// is refused here, naming the hop and the reason; it is never carried
    /// uncovered and nothing is ever owed for it afterwards.
    ///
    /// That "afterwards" is what issue #1145 deleted. ADR 0004's model --
    /// the claim covering crossing *n* signed once it fulfilled and riding
    /// crossing *n + 1* out of `ClaimBook::pending_claim` -- used to run
    /// here for any peering with no [`Connector::with_outbound_client_hop`],
    /// and ADR 0042 exists to retire it. It is gone from the peer role
    /// entirely: no fulfilment arms a peer claim, so no packet leaves owing
    /// one. (`ClaimBook::record_fulfillment` itself survives for a different
    /// caller on a different edge -- `ClientPayoutLedger`, this connector
    /// paying a *client* back, ADR 0026.)
    ///
    /// # The cap (ADR 0042)
    ///
    /// Before any of that, the amount this forward would put on the wire is
    /// checked against the peering's own cap
    /// ([`Self::packet_cap_for`]) and refused with `T04` if it exceeds it.
    /// The cap bounds a single packet -- how much this connector is willing
    /// to lose at once to a hop that takes the claim and does not carry --
    /// and never an accumulation. On a crossing that is the **converted**
    /// amount (ADR 0071 decision 1): the cap is configured against the
    /// outgoing peering, so it was always denominated in the outgoing
    /// leg's unit, and it is the same check either way.
    ///
    /// # The message window (PF-19)
    ///
    /// The forwarded packet's expiry is the arriving one's less
    /// [`FORWARDING_MESSAGE_WINDOW`], never the arriving one itself -- a
    /// hop keeps time back for the return leg, and one with none left
    /// refuses `R00` rather than forwarding a packet whose fulfilment could
    /// not reach it in time (issue #1174). Unilateral, and not a wire
    /// change: a shorter expiry needs no agreement from the peer receiving
    /// it.
    ///
    /// # Covering (issue #881), and the retry arm beside it
    ///
    /// [`Connector::cover_forward`] mints for exactly this packet's own
    /// forwarded value: a hop configured via
    /// [`Connector::with_outbound_client_hop`] gets a fresh claim from the
    /// outbound client ledger (issue #873) minted and attached before the
    /// packet is ever sent -- covered from the first attempt, including the
    /// first attempt after a restart, never merely recovered after a
    /// refusal. If the claim cannot be produced at all (no hop configured,
    /// no signer, no headroom, a receiver that will not report its
    /// watermark), the packet fails right there naming the hop and the
    /// reason. `Config::load` refuses a peering with a route to it and no
    /// `[[pay_channels]]` row (`ConfigError::PayChannelUnbound`), so on a
    /// configured route the no-hop case is unreachable from a file that
    /// loaded -- it is still refused here rather than assumed away, because
    /// a leased or runtime-installed route (ADR 0028) reaches this method
    /// without passing that check.
    ///
    /// The retry arm issue #875 added is kept, narrowed to what it is now
    /// actually for: a covered packet the peer STILL greets is a
    /// disagreement about the terms (the price moved, the claim did not
    /// clear the far gate) rather than the routine case, and is retried
    /// **once** with a claim minted against the peer's own quoted price
    /// from its greeting -- the authoritative figure in a disagreement,
    /// where this hop's own idea of the forwarded value is not. A second
    /// greeting after that is a failure, not a second retry.
    ///
    /// Cost: a forward spends one watermark round trip to the receiver and
    /// the one durable nonce reservation
    /// [`OutboundClientLedger::next_claim`] makes, on every packet (issue
    /// #879 measured the forwarded-packet path at 3.00 `fdatasync`/packet
    /// with exposure accounting on; this is a fourth). That is the price of
    /// ADR 0042 and it is now paid on every forward, because there is no
    /// longer a cheaper uncovered path to fall back to.
    async fn forward_via_peer_route(
        &self,
        peer_route: &PeerRoute,
        prepare: Prepare,
        arrived: Option<Arrival<'_>>,
    ) -> PacketResponse {
        // ADR 0061: this hop's fee belongs to the PEERING, so it is read off
        // `peer_route.peer_id()` rather than off the route the packet
        // matched. Every prefix forwarded to one counterparty therefore
        // costs the same, which is what "flat, per packet" always meant --
        // and ADR 0071 decision 1 adds the unit: the fee is denominated in
        // the OUTGOING leg, which is the peering it is attached to, so on a
        // crossing it is subtracted after the conversion and not before.
        let peer_id = peer_route.peer_id();
        let fee = self.fee_for(peer_id);

        // The two arms ADR 0071 decision 2 keeps apart. A forward that
        // crosses no denomination boundary -- every forward on a node that
        // declares no `[[tokens]]`, and a same-token forward on one that
        // does -- runs the one `checked_sub` it has always run, byte for
        // byte. A forward that crosses one converts first, at a rate this
        // node declared, or refuses. Sibling, never replacement.
        let forwarded_amount = match self.crossing(arrived, peer_id) {
            None => {
                // A packet that does not cover this hop's own flat fee is
                // refused rather than forwarded at whatever is left. `R01`
                // -- RFC 0027's "the amount received by a connector in the
                // path was too little to forward (zero or less)", which is
                // this case verbatim. ADR 0057's first sweep retired the
                // code outright and ADR 0051's #1143 update rehomed this
                // case to `F03`; both were wrong, and 0057's corrected
                // update says so: only R01's *minimum-delivery* meaning
                // dies with the field. `F03` is for an amount wrong against
                // a price the sender can pay; here nothing survives the fee
                // at all, the class letter is relative rather than final,
                // and the move is "send more".
                let Some(forwarded_amount) = amount_after_fee(prepare.amount, fee) else {
                    return PacketResponse::Reject(Reject {
                        code: RejectCode::r01_insufficient_source_amount(),
                        triggered_by: String::new(),
                        // The message is the sender's only way to learn its
                        // next move, and for a relative code that move is
                        // "send more" -- so both figures are named and so
                        // is the threshold to clear (ADR 0051, "what a
                        // reject carries besides its code").
                        message: format!(
                            "peer '{}' charges a fee of {} and this packet carried only {}: \
                             nothing would be left to forward. Send more than {}",
                            peer_id, fee, prepare.amount, fee
                        ),
                        data: Vec::new(),
                        accumulated_cost: 0,
                    });
                };
                forwarded_amount
            }
            Some((incoming, outgoing)) => {
                match self.convert_for_forward(incoming, outgoing, peer_id, prepare.amount, fee) {
                    Ok(forwarded_amount) => forwarded_amount,
                    Err(reject) => return PacketResponse::Reject(reject),
                }
            }
        };

        // ADR 0042, "The cap": the most this connector will hand this peer
        // in ONE packet, and therefore the most a single theft by it can
        // take -- a packet carries its own claim now, so the value on this
        // forward is at risk from the moment it leaves. Checked here,
        // before the packet is covered or sent, against the amount actually
        // going out (post-conversion and post-fee, the figure
        // `cover_forward` would mint a claim for); refused with `T04`
        // naming both numbers, never truncated and never split into two
        // packets, which would defeat the bound rather than respect it.
        //
        // Tempting and wrong: adding "and how much has this peer had
        // lately" here. That is an accumulation, ADR 0033 deleted the
        // machinery for it deliberately, and it stays deleted -- nothing is
        // ever owed between packets, so there is no running total for this
        // to bound. One packet, checked and forgotten.
        let cap = self.packet_cap_for(peer_id);
        if forwarded_amount > cap {
            return PacketResponse::Reject(Reject {
                code: RejectCode::t04_insufficient_liquidity(),
                triggered_by: String::new(),
                message: format!(
                    "peer '{peer_id}' has a maximum packet amount of {cap}, and this packet \
                     would forward {forwarded_amount}"
                ),
                data: Vec::new(),
                accumulated_cost: 0,
            });
        }

        // PF-19: the packet that goes out expires strictly before the one
        // that arrived, by a whole [`FORWARDING_MESSAGE_WINDOW`]. This hop
        // keeps that window back for the return leg -- the time a
        // fulfilment answered just inside the downstream deadline needs to
        // travel back here and be relayed on before *this* packet's own
        // deadline fires. Copying `expires_at` through verbatim, which is
        // what this did until issue #1174, handed the last hop the entire
        // remaining budget and left every hop above it holding a paid-for
        // crossing it could no longer be paid for.
        //
        // Nothing about this is a wire change and nothing downstream has to
        // agree to it: shortening is unilateral, a receiver that would
        // honour the longer expiry honours the shorter one too, and the
        // field is the same field. It is done here rather than in a
        // transport because it is a property of forwarding, not of HTTP or
        // BTP, and both carriages must do it.
        //
        // A window that leaves nothing is `R00`, the same code PF-02 gives
        // an already-expired arrival and the same reason: the packet has
        // run out of time here, this hop's fee and route are beside the
        // point, and the sender's move is a fresh packet with more budget
        // rather than anything about this path. `packet-flow-spec.md` §5
        // lists `R00` (expired) among the class-only codes -- there is no
        // distinct action to bind, so the message carries the diagnosis and
        // the code carries the class. `T00` would be a lie (this is not
        // this connector's own configuration error) and `F02` a worse one
        // (the path is fine; the clock is not).
        let Some(outgoing_expires_at) = forwarded_expiry(prepare.expires_at, self.clock.now())
        else {
            return PacketResponse::Reject(Reject {
                code: RejectCode::r00_transfer_timed_out(),
                triggered_by: String::new(),
                message: format!(
                    "packet expires at {} and forwarding to peer '{peer_id}' keeps {}s back for \
                     the return leg, which leaves no time to carry it",
                    prepare.expires_at.to_rfc3339(),
                    FORWARDING_MESSAGE_WINDOW.num_seconds(),
                ),
                data: Vec::new(),
                accumulated_cost: 0,
            });
        };

        let outgoing = Prepare {
            amount: forwarded_amount,
            expires_at: outgoing_expires_at,
            ..prepare
        };

        // ADR 0042: a connector covers every PREPARE it sends. The claim is
        // minted from this packet's own forwarded value and attached before
        // the packet leaves; a peering that cannot be covered is refused
        // here and the packet is not forwarded at all.
        //
        // There is no `NotConfigured` arm any more (issue #1145). It used to
        // fall through to `ClaimBook::pending_claim` -- ADR 0004's postpay
        // convention, armed by a *previous* fulfilment -- which is precisely
        // the model ADR 0042 retires, and while it existed "a connector
        // covers every PREPARE it sends" was a record rather than a fact.
        // Nothing this method sends is acknowledged against `self.claims`
        // either: the claim's watermark authority is the RECEIVER, asked
        // over `claim_state`, and the peer book knows nothing of it (see
        // `crate::outbound_client`'s header).
        let riding_claim = match self.cover_forward(peer_id, forwarded_amount).await {
            Ok(claim) => claim,
            Err(reason) => {
                tracing::warn!(
                    peer_id,
                    %reason,
                    "refusing to forward to this peer uncovered -- a covering claim could \
                     not be produced"
                );
                return PacketResponse::Reject(Reject {
                    code: RejectCode::t00_internal_error(),
                    triggered_by: String::new(),
                    message: format!("cannot cover a peer PREPARE to '{peer_id}': {reason}"),
                    data: Vec::new(),
                    accumulated_cost: 0,
                });
            }
        };
        let mut answer = self
            .peer_transport
            .forward(peer_id, outgoing.clone(), Some(riding_claim))
            .await;

        if let Some(terms) = answer.payment_required.take() {
            if let Some(covering) = self.cover_greeted_packet(peer_id, &terms).await {
                tracing::info!(
                    peer_id,
                    nonce = covering.nonce,
                    cumulative = covering.cumulative_amount,
                    price = terms.price().unwrap_or_default(),
                    "covering a greeted forward and retrying it once"
                );
                answer = self
                    .peer_transport
                    .forward(peer_id, outgoing, Some(covering))
                    .await;
                // Bounded: whatever the retry answered is the answer. A
                // second greeting is logged with its terms and relayed, not
                // covered again.
                if let Some(again) = &answer.payment_required {
                    tracing::warn!(
                        peer_id,
                        price = again.price().unwrap_or_default(),
                        pay_to = again.pay_to().unwrap_or_default(),
                        resource = %again.resource.url,
                        "peer demanded payment again after a covering claim -- not retrying"
                    );
                }
            }
        }
        // The retry's own `ack` is deliberately NOT fed to `self.claims`:
        // the claim it acknowledges belongs to the outbound CLIENT ledger,
        // whose authority is the receiver's watermark rather than anything
        // this book records (see `crate::outbound_client`'s header).

        match answer.response {
            // No claim is signed here, and that absence is the whole of
            // issue #1145. A fulfilment is a DELIVERY RECEIPT (ADR 0042),
            // not a payment trigger: this packet was paid for before it was
            // sent, so there is nothing left to owe once it lands. The
            // `ClaimBook::record_fulfillment` call that used to sit here was
            // the last arming site of ADR 0004's model in the peer role.
            //
            // Issue #1269 / ADR 0069: a peer's FULFILL rides home unchecked.
            // Verifying it against an execution condition used to be the one
            // thing standing between this hop and trusting the peer's word
            // outright -- but a hop is paid on arrival regardless (ADR
            // 0042), and a mismatch used to charge `price_on_reject` anyway,
            // so the check protected nothing this hop owns. The sender's own
            // end-to-end check (`connector send` against its own
            // `derive_fulfillment`) is what a forged fulfilment actually
            // meets.
            PacketResponse::Fulfill(fulfill) => PacketResponse::Fulfill(fulfill),
            // ADR 0011, peer-semantics-pre-868.md §5.2: this hop's own fee is added
            // only once it has genuinely reached `peer_id` and relays a
            // reject that peer itself decided on -- never on a reject this
            // transport synthesized locally (`reached_peer` false) because
            // the packet never actually traversed this hop in that case.
            //
            // ADR 0071 decision 7 adds the second half, and the two arms
            // here are the same two `forward_via_peer_route` opened with,
            // asked of the identical `Self::crossing` call so that a packet
            // and its reject can never disagree about whether they crossed
            // a boundary. Off a boundary the running total gains this hop's
            // fee and nothing else, byte for byte as it has since issue
            // #426. Across one it gains the fee IN THE OUTGOING UNIT --
            // which is the unit the total arrived in, since the total was
            // accumulated downstream -- and the whole is then un-converted
            // into the incoming leg's unit, so that what goes upstream is
            // denominated the way the peering it goes out over is.
            PacketResponse::Reject(mut reject) => {
                if answer.reached_peer {
                    reject.accumulated_cost = match self.crossing(arrived, peer_id) {
                        None => reject.accumulated_cost + fee,
                        Some((incoming, outgoing)) => self.convert_for_reject(
                            incoming,
                            outgoing,
                            peer_id,
                            reject.accumulated_cost,
                            fee,
                        ),
                    };
                }
                PacketResponse::Reject(reject)
            }
        }
    }

    /// Cover a forward to `peer_id` for exactly `amount` -- the value THIS
    /// packet forwards -- from the outbound CLIENT ledger (issue #873),
    /// before the packet is ever sent (issue #881). Mirrors
    /// [`Connector::cover_greeted_packet`]'s mechanism but never reads a
    /// greeting: `amount` and [`OutboundClientHop::domain`] are both known
    /// locally, which is exactly what lets this run proactively rather
    /// than only once a refusal has already taught this node a price.
    async fn cover_forward(&self, peer_id: &str, amount: u64) -> Result<WireClaim, String> {
        let hops = self.outbound_client_hops.load_full();
        let Some(hop) = hops.get(peer_id) else {
            // Neither populator of `outbound_client_hops` has armed this
            // peering -- no `[[pay_channels]]` row at boot, and no
            // `register_outbound_client_hop` from a runtime peering (issue
            // #1217) -- so there is nothing to pay it from, and since issue
            // #1145 there is no postpay path to fall through to either.
            // `Config::load` refuses a configured route to an uncovered
            // peering by name (`ConfigError::PayChannelUnbound`), so a file
            // that loaded cannot reach this; a leased or runtime-installed
            // route (ADR 0028) can, and is refused here rather than carried
            // free.
            return Err(format!(
                "no client-role channel is configured to pay peer '{peer_id}' from -- neither a \
                 '[[pay_channels]]' row nor a runtime peering (ADR 0058) has bound one -- and a \
                 connector covers every PREPARE it sends (ADR 0042)"
            ));
        };
        let Some(ledger) = self.outbound_client.as_ref() else {
            return Err(
                "a client-role channel is configured for this peer but this node has no \
                 outbound client ledger to sign from"
                    .to_string(),
            );
        };
        // The key is the settlement identity of the channel's on-chain
        // participant, on the channel's own chain (issue #1146) -- the same
        // key the PEER role on that channel signs with, because it is the
        // same participant. Chosen by matching the hop's domain, so a
        // secp256k1 key can never be paired with a Solana program id.
        let binding = match &hop.domain {
            OutboundClientDomain::Evm(domain) => {
                let Some(signer) = self.claims.signer() else {
                    return Err(
                        "this node has no EVM settlement signer to sign a claim with".to_string(),
                    );
                };
                OutboundClaimBinding::Evm {
                    domain: *domain,
                    signer: signer.as_ref(),
                }
            }
            OutboundClientDomain::Solana(domain) => {
                let Some(signer) = self.claims.solana_signer() else {
                    return Err(
                        "this node has no Solana settlement signer to sign a claim with"
                            .to_string(),
                    );
                };
                OutboundClaimBinding::Solana {
                    program_id: domain.program_id,
                    signer: signer.as_ref(),
                }
            }
        };

        let claim = match ledger
            .next_claim(
                peer_id,
                hop.claim_state.as_ref(),
                &hop.channel,
                &binding,
                amount,
            )
            .await
        {
            Ok(claim) => claim,
            Err(error) => return Err(error.to_string()),
        };
        // The wire carries `cumulative_amount` as a `uint64` (§4.2); see
        // the matching check in `cover_greeted_packet` for why this is
        // refused rather than truncated.
        let Ok(cumulative_amount) = u64::try_from(claim.cumulative) else {
            return Err(format!(
                "the covering claim's cumulative amount {} does not fit the wire's uint64",
                claim.cumulative
            ));
        };
        Ok(WireClaim {
            channel_id: hop.channel_id.clone(),
            nonce: claim.nonce,
            cumulative_amount,
            // Already discriminated by the binding that produced it -- an
            // ed25519 signature is never re-labelled as an EVM one on its
            // way to the carriage (issue #732's rule, issue #1146's arm).
            signature: claim.signature,
        })
    }

    /// Sign a claim covering the terms `peer_id` just quoted, ready to ride
    /// one retry of the packet it refused (issue #875).
    ///
    /// The claim is minted from the outbound CLIENT ledger (issue #873):
    /// its cumulative amount is the RECEIVER's own watermark advanced by the
    /// quoted price, and on EVM its EIP-712 domain is the receiver's own,
    /// read off the greeting rather than out of this node's settlement
    /// config -- a claim signed under the payer's idea of the `TokenNetwork`
    /// recovers to a different address and is refused at the far gate.
    ///
    /// **A Solana hop reads no domain off the greeting** (issue #1146), and
    /// that is not the same shortcut this method refuses for EVM. ADR 0053's
    /// binding is the settlement program id, and since issue #1128 a node
    /// has exactly one -- the program it can redeem through -- so a payer and
    /// a payee that disagreed about it would have no channel in common to be
    /// quoting each other prices about. The greeting supplies the price and
    /// nothing else. The retry arm therefore works on both chains rather
    /// than silently doing nothing on one.
    ///
    /// `None` -- with the reason logged, never silently -- for every way
    /// this node cannot pay: no ledger, no client-role channel configured
    /// for this hop, no settlement signer on that channel's chain, an EVM
    /// hop whose greeting names no EVM settlement, or a receiver that would
    /// not report the watermark (which includes refusing for want of
    /// headroom). The caller then relays the peer's refusal as it stands;
    /// nothing is ever emitted claiming to have paid when it has not.
    async fn cover_greeted_packet(
        &self,
        peer_id: &str,
        terms: &X402PaymentRequired,
    ) -> Option<WireClaim> {
        let Some(ledger) = self.outbound_client.as_ref() else {
            tracing::warn!(
                peer_id,
                "peer quoted x402 terms but this node has no outbound client ledger to pay from"
            );
            return None;
        };
        let hops = self.outbound_client_hops.load_full();
        let Some(hop) = hops.get(peer_id) else {
            tracing::warn!(
                peer_id,
                "peer quoted x402 terms but no client-role channel is configured for it"
            );
            return None;
        };
        let binding = match &hop.domain {
            OutboundClientDomain::Evm(_) => {
                let Some(signer) = self.claims.signer() else {
                    tracing::warn!(
                        peer_id,
                        "peer quoted x402 terms but this node has no EVM settlement signer to \
                         sign a claim with"
                    );
                    return None;
                };
                let Some(domain) = EvmDomain::from_greeting(terms) else {
                    tracing::warn!(
                        peer_id,
                        resource = %terms.resource.url,
                        "peer quoted x402 terms naming no EVM settlement this node can sign under"
                    );
                    return None;
                };
                OutboundClaimBinding::Evm {
                    domain,
                    signer: signer.as_ref(),
                }
            }
            OutboundClientDomain::Solana(domain) => {
                let Some(signer) = self.claims.solana_signer() else {
                    tracing::warn!(
                        peer_id,
                        "peer quoted x402 terms but this node has no Solana settlement signer to \
                         sign a claim with"
                    );
                    return None;
                };
                OutboundClaimBinding::Solana {
                    program_id: domain.program_id,
                    signer: signer.as_ref(),
                }
            }
        };
        let Some(price) = terms.price() else {
            // Unreachable through a parsed greeting (`parse_greeting`
            // refuses an unreadable amount), and still not defaulted to
            // zero: a free ride is exactly what must not be inferred.
            tracing::warn!(peer_id, "peer quoted x402 terms with no readable price");
            return None;
        };

        let claim = match ledger
            .next_claim(
                peer_id,
                hop.claim_state.as_ref(),
                &hop.channel,
                &binding,
                price,
            )
            .await
        {
            Ok(claim) => claim,
            Err(error) => {
                tracing::warn!(
                    peer_id,
                    %error,
                    "could not sign a claim covering the peer's terms"
                );
                return None;
            }
        };
        // The wire carries `cumulative_amount` as a `uint64` (§4.2); a
        // channel whose lifetime total has outgrown that is refused here
        // rather than truncated into a claim for a smaller number than the
        // one signed.
        let Ok(cumulative_amount) = u64::try_from(claim.cumulative) else {
            tracing::error!(
                peer_id,
                cumulative = %claim.cumulative,
                "the covering claim's cumulative amount does not fit the wire's uint64"
            );
            return None;
        };
        Some(WireClaim {
            channel_id: hop.channel_id.clone(),
            nonce: claim.nonce,
            cumulative_amount,
            signature: claim.signature,
        })
    }

    /// Issue #545: a reject this connector originates because the packet
    /// reached its termination -- an envelope that failed to decode below --
    /// sets `accumulated_cost` to this route's price, the same way
    /// [`Self::forward_via_peer_route`]
    /// adds a forwarding hop's fee to a relayed reject. `AppOutcome::Unreachable`
    /// does not: the app was never actually reached to do the priced work,
    /// matching how a forwarding hop that cannot reach its own peer adds
    /// nothing either. Neither does `AppOutcome::Refused` (issue #596): a
    /// target that attempts to escape the route's handler path is refused
    /// before any request is made, so like `Unreachable`, the app never did
    /// any priced work and the payer is not charged for the attempt.
    ///
    /// Per ADR 0018/issue #524, `prepare.data` is a gift wrap sealed to this
    /// connector's own identity key: opened here, above the [`AppClient`]
    /// boundary, so the port itself never sees a [`Prepare`], a key, or a
    /// secret (issue #521's boundary, extended by #524). Every return path
    /// past a successful open carries the request's own shared secret back
    /// through [`Self::seal_termination_response`] -- a FULFILL and a
    /// REJECT raised at the termination are both sealed with it (ADR 0018);
    /// only [`Self::open_termination_request`]'s own two failures, which
    /// happen before any secret is recovered, stay plaintext.
    ///
    /// `client_channel_id` is the client channel whose covering claim
    /// admitted this packet at this connector's own edge, or `None` when
    /// nothing did (a peer-role arrival, an unpriced or unclaimed
    /// request). It is the sole source of the attribution headers the
    /// delivery carries (ADR 0040, `crate::attribution`) -- which is why a
    /// packet that reached here across another hop states no payer at all
    /// rather than naming the hop it arrived from, the failure ADR 0017
    /// found in the TypeScript prototype's own header.
    ///
    /// `prepare.expires_at` travels down with it (ADR 0064, PF-25, issue
    /// #1183): the packet's deadline bounds how long this termination waits
    /// for the app, so a slow handler is abandoned rather than waited on
    /// past the instant the sender said to stop. It is read here rather
    /// than left behind with the rest of the `Prepare` because
    /// [`Self::deliver_opened_envelope`] is where the app call is made, and
    /// the deadline has to be in scope at the call it bounds.
    async fn deliver_to_app(
        &self,
        route: &StaticRoute,
        prepare: Prepare,
        client_channel_id: Option<&str>,
    ) -> PacketResponse {
        let expires_at = prepare.expires_at;
        // What this packet costs, read off the schedule at its own payload
        // length (ADR 0065) -- taken here because `open_termination_request`
        // below is the last moment `prepare.data` is whole, and because the
        // length that is charged must be the length that arrived rather than
        // anything recovered from inside the wrap. The client edge and the
        // peer gate computed the same figure from the same bytes.
        let charge = route.price().charge(prepare.data.len());

        let (envelope_bytes, shared_secret) = match self.open_termination_request(&prepare.data) {
            Ok(opened) => opened,
            Err(reject) => return PacketResponse::Reject(reject),
        };

        let inner = self
            .deliver_opened_envelope(
                PricedTermination {
                    handler_url: route.handler_url(),
                    charge,
                },
                expires_at,
                &shared_secret,
                &envelope_bytes,
                client_channel_id,
            )
            .await;
        Self::seal_termination_response(inner, &shared_secret)
    }

    /// Open the ADR 0018 gift wrap `data` carries, yielding the envelope
    /// bytes inside it and the shared secret every answer past this point
    /// is sealed with -- or the plaintext refusal to answer instead. Its
    /// two failures are the only ones a termination raises before a secret
    /// is in hand, which is exactly why they are also the only ones that
    /// stay unsealed. The one termination this connector has is
    /// [`Self::deliver_to_app`].
    fn open_termination_request(&self, data: &[u8]) -> Result<(Vec<u8>, [u8; 32]), Reject> {
        let Some(identity_signer) = self.identity_signer.as_ref() else {
            return Err(unsealed_termination_reject(
                "no identity key configured to open a sealed payload",
            ));
        };
        open_request(data, identity_signer.as_ref()).map_err(|error| {
            unsealed_termination_reject(&format!("gift wrap could not be opened: {error}"))
        })
    }

    /// The part of [`Self::deliver_to_app`] that runs once the gift wrap has
    /// been opened: decode the envelope it carried and, if that succeeds,
    /// make the request it describes -- bounded by the packet's own
    /// deadline. Split out so the caller can seal every return path
    /// uniformly with the one shared secret the wrap carried, including
    /// this method's own envelope-decode failure.
    ///
    /// # The deadline bounds the wait, and nothing else (ADR 0064, PF-25)
    ///
    /// Until issue #1183 nothing here read `expires_at` at all. Expiry was
    /// checked once on arrival ([`Self::reject_ineligible`], PF-02) and
    /// never again, so an app that took longer than the packet's whole
    /// budget was still answered for: this connector derived the fulfilment
    /// (ADR 0019) and returned a FULFILL for a packet whose deadline had
    /// already fired, to an upstream that had given up and, if it was
    /// another connector, moved on. That is the termination-side twin of
    /// the forwarding race PF-19 closed one release earlier.
    ///
    /// The rule is that the deadline bounds the *wait*, not the *answer*.
    /// [`connector_domain::delivery_budget`] says how long there is, and
    /// the app call is abandoned when that runs out; an app that answers
    /// inside the budget is answered for, however close to the line it
    /// came, and no second expiry check is applied to work already done.
    /// That asymmetry is deliberate and is the whole of ADR 0064: the claim
    /// paying for this packet was taken *before* the app was called -- at
    /// the client edge by `ClientClaimGate::ingest`, or on a peer arrival by
    /// `ClaimBook`, in both cases with the watermark advanced and journalled
    /// before routing began -- and only a forwarded route's terminal reject
    /// gives one back (`roll_back_uncarried_forward`, issue #1012). So the
    /// verdict returned here decides nothing about who is out of pocket.
    /// Rejecting a late-but-real answer would destroy work the payer has
    /// already been charged for, refund nobody, and say something false
    /// besides: under ADR 0042 a fulfilment is a delivery receipt, not a
    /// payment trigger, and the delivery genuinely happened.
    ///
    /// Both refusals are `R00` -- the code PF-02 gives an already-dead
    /// arrival and PF-19 a forward with no window left, for the same reason
    /// in all three places: the packet ran out of time, the sender's move is
    /// a fresh packet with more budget, and nothing about this route or this
    /// connector's configuration is at fault. `T01` would be the available
    /// lie -- the app was reachable, it was simply slower than the sender
    /// allowed -- and would send a sender looking for another path.
    ///
    /// Both are also raised *here*, below [`Self::open_termination_request`],
    /// so they ride home sealed to the sender's own shared secret like every
    /// other verdict a termination reaches (ADR 0018, PF-24). Checking
    /// before the wrap was open would have been marginally cheaper and would
    /// have leaked an unsealed reject for a packet this connector could read
    /// perfectly well.
    async fn deliver_opened_envelope(
        &self,
        termination: PricedTermination<'_>,
        expires_at: chrono::DateTime<Utc>,
        shared_secret: &[u8; 32],
        envelope_bytes: &[u8],
        client_channel_id: Option<&str>,
    ) -> PacketResponse {
        let mut request = match EnvelopeRequest::decode(envelope_bytes) {
            Ok(request) => request,
            Err(error) => {
                return PacketResponse::Reject(Reject {
                    code: RejectCode::f01_invalid_packet(),
                    triggered_by: String::new(),
                    message: format!("envelope did not decode: {error}"),
                    data: Vec::new(),
                    accumulated_cost: termination.charge,
                });
            }
        };

        // ADR 0040: state what this connector itself verified about the
        // payment -- and, whether or not there is anything to state, remove
        // whatever the sender wrote under those same names first, so an app
        // reading them is reading this connector or reading nothing.
        apply_payment_attribution(
            &mut request,
            client_channel_id.map(|channel_key| PaymentAttribution {
                channel_key,
                charge: termination.charge,
            }),
        );

        // PF-25's first half: a packet with nothing left is not delivered at
        // all. `accumulated_cost` is zero because the app was never asked --
        // the same figure, for the same reason, as `Unreachable` and
        // `Refused` below, and unlike the envelope-decode failure above,
        // which reached the termination and is charged this route's price
        // (issue #545).
        let Some(budget) = delivery_budget(expires_at, self.clock.now()) else {
            return PacketResponse::Reject(Reject {
                code: RejectCode::r00_transfer_timed_out(),
                triggered_by: String::new(),
                message: format!(
                    "packet expired at {} before it could be delivered -- the app was not asked",
                    expires_at.to_rfc3339(),
                ),
                data: Vec::new(),
                accumulated_cost: 0,
            });
        };

        // The wait itself is real time, not the injected clock, and that
        // split is on purpose: what to do is a decision (`delivery_budget`,
        // pure, deterministic, property-tested in `connector-domain`), while
        // when to stop waiting is a mechanism only a real timer can perform.
        // A test drives the decision through the clock port and the
        // abandonment through a genuinely slow fake app, which is what those
        // two things actually are.
        //
        // `to_std` cannot fail here -- `delivery_budget` returns a strictly
        // positive span or nothing -- and the saturating arm is a far-future
        // timer rather than a panic, so a sender naming a preposterous
        // expiry gets a wait it will never see the end of instead of taking
        // the packet path down with it.
        let budget = budget
            .to_std()
            .unwrap_or(std::time::Duration::from_secs(u64::MAX));
        let delivery = self.app_client.deliver(termination.handler_url, &request);
        let Ok(outcome) = tokio::time::timeout(budget, delivery).await else {
            // PF-25's second half. Dropping the future is what abandons the
            // request: the socket to the app closes and this connector stops
            // waiting. The app may well finish the work anyway -- that is
            // between an operator and its own handler, both inside one trust
            // domain (ADR 0019), and not a protocol question. What is a
            // protocol question is that the packet reached its termination
            // and the priced work was set in motion, so unlike the arm
            // above this reject carries the route's price (ADR 0011, issue
            // #545): a sender learns "your answer was too slow", not "this
            // path is free".
            tracing::info!(
                handler_url = %termination.handler_url,
                expires_at = %expires_at.to_rfc3339(),
                "the app did not answer within the packet's deadline -- abandoning the request"
            );
            return PacketResponse::Reject(Reject {
                code: RejectCode::r00_transfer_timed_out(),
                triggered_by: String::new(),
                message: format!(
                    "the app did not answer before this packet's expiry at {}; the request was \
                     abandoned rather than answered late",
                    expires_at.to_rfc3339(),
                ),
                data: Vec::new(),
                accumulated_cost: termination.charge,
            });
        };

        match outcome {
            // ADR 0020: an HTTP status is envelope content, never a packet
            // outcome, so any complete answer -- whatever its status --
            // rides home as a response envelope on a FULFILL. ADR
            // 0019/issue #525: the app supplies nothing toward the
            // fulfilment itself -- it is derived from this request's own
            // shared secret, the same secret every other return path here
            // seals its response with. Issue #1269 / ADR 0069: there is no
            // longer a separate execution condition to check this against --
            // checking a derivation against a condition minted from the same
            // secret was always a tautology, since only a sender who sealed
            // to this connector's identity could ever have produced a
            // wrap that opens here at all.
            AppOutcome::Answered { response } => PacketResponse::Fulfill(Fulfill {
                fulfillment: derive_fulfillment(shared_secret),
                data: response.encode(),
            }),
            AppOutcome::Unreachable { message } => PacketResponse::Reject(Reject {
                code: RejectCode::t01_peer_unreachable(),
                triggered_by: String::new(),
                message,
                data: Vec::new(),
                accumulated_cost: 0,
            }),
            // Issue #596: distinguishable from both an undecodable envelope
            // (F01, above) and an app's own answer -- including a 404,
            // which arrives as `Answered` and rides home on a FULFILL, not
            // a reject at all -- so a sender can tell "your envelope named
            // somewhere this route's handler does not expose" apart from
            // either.
            AppOutcome::Refused { message } => PacketResponse::Reject(Reject {
                code: RejectCode::f00_bad_request(),
                triggered_by: String::new(),
                message,
                data: Vec::new(),
                accumulated_cost: 0,
            }),
        }
    }

    /// Seal `response`'s `data` with `shared_secret` (ADR 0018: "a FULFILL,
    /// and a REJECT raised at the termination, are sealed back with the
    /// same shared secret"). Applied uniformly to every outcome
    /// [`Self::deliver_opened_envelope`] can produce -- a genuine fulfilment,
    /// a fulfilment that failed to verify, an unreachable app, or a
    /// malformed envelope -- since all four happen at the termination, with
    /// the secret already in hand.
    fn seal_termination_response(
        response: PacketResponse,
        shared_secret: &[u8; 32],
    ) -> PacketResponse {
        match response {
            PacketResponse::Fulfill(fulfill) => PacketResponse::Fulfill(Fulfill {
                data: seal_response(shared_secret, &fulfill.data),
                ..fulfill
            }),
            PacketResponse::Reject(reject) => PacketResponse::Reject(Reject {
                data: seal_response(shared_secret, &reject.data),
                ..reject
            }),
        }
    }

    /// The index into `self.routes` that `destination` longest-prefix
    /// matches against, if any.
    fn select_app_route(&self, destination: &str) -> Option<usize> {
        let app_prefixes: Vec<&str> = self.routes.iter().map(StaticRoute::prefix).collect();
        select_route(destination, &app_prefixes)
    }

    /// The configured route -- terminated or forwarded -- `destination`
    /// longest-prefix matches, with the length of the prefix that matched.
    ///
    /// The one place configured-route selection lives, shared by
    /// [`Self::handle_prepare_traced`] (which then weighs a leased route
    /// against the answer) and [`Self::client_route`] (which does not, ADR
    /// 0028). That sharing is the point: the client edge's claim gate
    /// (issue #522) and x402 greeting must price the route the router will
    /// actually use, and a forwarded route being priced at all (issue #620)
    /// makes "app routes only" no longer a safe simplification for either.
    ///
    /// Includes runtime peer-forwarding routes (issue #884) alongside the
    /// config file's own: both are static, priced, durable rows, so both
    /// belong in "configured" as this method means it -- only a lease
    /// (issue #427), with no price and no durability, is excluded.
    ///
    fn select_configured_route(&self, destination: &str) -> Option<(usize, ConfiguredTarget)> {
        let app_match = self.select_app_route(destination).map(|index| {
            (
                self.routes[index].prefix().len(),
                ConfiguredTarget::App(index),
            )
        });
        let peer_prefixes: Vec<&str> = self.peer_routes.iter().map(PeerRoute::prefix).collect();
        let peer_match = select_route(destination, &peer_prefixes).map(|index| {
            (
                self.peer_routes[index].prefix().len(),
                ConfiguredTarget::Peer(index),
            )
        });
        let runtime_peer_routes = self.runtime_peer_routes_snapshot();
        let runtime_list: Vec<&PeerRoute> = runtime_peer_routes.values().collect();
        let runtime_prefixes: Vec<&str> = runtime_list.iter().map(|route| route.prefix()).collect();
        let runtime_match = select_route(destination, &runtime_prefixes).map(|index| {
            (
                runtime_list[index].prefix().len(),
                ConfiguredTarget::RuntimePeer(runtime_list[index].clone()),
            )
        });
        [app_match, peer_match, runtime_match]
            .into_iter()
            .flatten()
            .max_by_key(|(len, target)| (*len, target.rank()))
    }

    /// Price, transport policy and route kind for the configured route
    /// `destination` resolves to (ADR 0028), or `None` when no configured
    /// route matches -- the single lookup both client-edge carriages make
    /// per request, so the greeting, the claim gate, the journal and
    /// `GET /ilp/routes/price` all charge one number.
    ///
    /// A forwarded route reports [`TransportPolicy::Both`]: `transport` is
    /// refused on such a route at load, so it accepts a client's request
    /// over either carriage, which is what every route did before issue
    /// #701.
    ///
    /// Leased routes (issue #427) are deliberately absent. A lease is
    /// pushed over the operator surface and carries no price, so folding
    /// one in here would let an operator-pushed longer-prefix lease zero a
    /// configured route's price -- the free-gateway failure issue #557
    /// exists to prevent, arrived at from the other direction.
    pub fn client_route(&self, destination: &str) -> Option<ClientRouteFacts> {
        self.select_configured_route(destination)
            .map(|(_, target)| match target {
                ConfiguredTarget::App(index) => ClientRouteFacts {
                    price: self.routes[index].price(),
                    transport_policy: self.routes[index].transport_policy(),
                    kind: ClientRouteKind::Terminated,
                    request: self.routes[index].request().cloned(),
                },
                ConfiguredTarget::Peer(index) => ClientRouteFacts {
                    price: self.peer_routes[index].price(),
                    transport_policy: TransportPolicy::Both,
                    kind: ClientRouteKind::Forwarded,
                    request: self.peer_routes[index].request().cloned(),
                },
                // A runtime-pushed peer route (issue #884) carries no
                // `request` table: it is written through the operator
                // surface, not `[[routes]]`, so there is no config-file row
                // for one to live on.
                ConfiguredTarget::RuntimePeer(route) => ClientRouteFacts {
                    price: route.price(),
                    transport_policy: TransportPolicy::Both,
                    kind: ClientRouteKind::Forwarded,
                    request: None,
                },
            })
    }

    /// [`Self::client_route`]'s price alone, for a caller with no use for
    /// the rest.
    pub fn client_route_price(&self, destination: &str) -> Option<Price> {
        self.client_route(destination).map(|route| route.price)
    }

    /// Every prefix this connector prices at the client edge, with what it
    /// costs -- the enumeration behind the node self-description's `routes`
    /// (ADR 0050).
    ///
    /// Each prefix's price **and its transport policy** are read back through
    /// [`Self::client_route`] rather than off the table they were found in, so
    /// a prefix that appears in more than one source is quoted at the price a
    /// real request to it would be charged and under the carriage a real
    /// request to it would have to arrive on. There is exactly one lookup rule
    /// and this uses it.
    ///
    /// Sorted by prefix, which makes the answer stable across restarts and
    /// across a runtime write that happens to land in a different map slot --
    /// a document whose field order wanders is a document nobody can diff.
    ///
    /// Deliberately carries **prefix, price, the route's `request`
    /// declaration and the carriage it pins, and nothing else** (ADR 0067 for
    /// the third, TOON_Network issue #111 for the fourth). A terminated
    /// route's `handler_url` describes software behind this connector
    /// (ND-08); a forwarded route's peer id and per-peering fee are
    /// operator-private (ND-09). A pin is none of those: it is what a client
    /// must do to reach the route at all, and withholding it does not keep a
    /// secret, it only makes the route unusable to anyone who did not guess.
    /// Leased routes are absent for the reason [`Self::client_route`] gives:
    /// a lease carries no price at all.
    pub fn client_route_prices(&self) -> Vec<ClientRoutePrice> {
        let mut prefixes: std::collections::BTreeSet<String> = self
            .routes
            .iter()
            .map(|route| route.prefix().to_string())
            .collect();
        prefixes.extend(
            self.peer_routes
                .iter()
                .map(|route| route.prefix().to_string()),
        );
        prefixes.extend(self.runtime_peer_routes.load().keys().cloned());
        prefixes
            .into_iter()
            .filter_map(|prefix| {
                self.client_route(&prefix).map(|facts| ClientRoutePrice {
                    prefix,
                    price: facts.price,
                    request: facts.request,
                    transport_policy: facts.transport_policy,
                })
            })
            .collect()
    }

    /// Whether `prepare` names a terminated app route whose envelope
    /// target will be refused (`AppOutcome::Refused`, F00, issue #596)
    /// once this packet is actually routed and delivered there -- decided
    /// without delivering anything, so the client edge can ask this
    /// *before* admitting `prepare`'s covering claim (issue #869) and skip
    /// ingesting it: a packet this connector was always going to refuse
    /// for its envelope's own shape must never spend the claim it rode in
    /// on.
    ///
    /// `false` covers every case besides a confirmed envelope-shape
    /// refusal: an unmatched destination, a forwarded route -- configured
    /// **or leased**; either way its `data` stays opaque at this hop, and
    /// only a terminated route's envelope is ever opened -- no identity
    /// key configured, a gift wrap that fails to open, or an envelope that
    /// fails to decode. The first two have no envelope to judge at this
    /// hop at all; the last three are packets this method cannot read, so
    /// it cannot tell "refused for its target's shape" from "unreadable"
    /// and declines to guess. `false` therefore leaves each of them
    /// exactly as it was before this method existed -- the covering claim
    /// is still admitted, so a packet [`Self::deliver_to_app`] then turns
    /// away for an unopenable wrap is still charged for it. That is issue
    /// #869's own complaint arriving through a different door, and closing
    /// it belongs to a separate change: this method answers only the
    /// refusal it can prove in advance. This is deliberately not a cache of
    /// [`Self::deliver_to_app`]'s decision: it repeats the same
    /// open-and-decode work, on the same immutable `prepare.data`, and it
    /// resolves the winning route by the same rule
    /// [`Self::handle_prepare_traced`] applies (longest prefix,
    /// [`RouteRank`] on a tie), so the two can never disagree about where
    /// this packet actually goes.
    pub fn envelope_target_would_be_refused(&self, prepare: &Prepare) -> bool {
        let Some((configured_len, ConfiguredTarget::App(index))) =
            self.select_configured_route(&prepare.destination)
        else {
            return false;
        };
        if self.active_lease_outranks(&prepare.destination, configured_len) {
            return false;
        }
        let Some(request) = self.opened_envelope_request(&prepare.data) else {
            return false;
        };
        crate::app_client::resolve_target_under_handler(
            self.routes[index].handler_url(),
            &request.target,
        )
        .is_err()
    }

    /// Whether a strictly longer-prefix active lease beats the configured
    /// route of prefix length `configured_len` that `destination` already
    /// resolved to -- the same winner rule the router applies (equal
    /// length ties break to the configured route, `RouteRank::App >
    /// RouteRank::Leased`). Both pre-admission probes below ask this
    /// before they judge anything: when a lease wins, the packet is
    /// *forwarded* with its data opaque at this hop, so there is no
    /// refusal to predict -- and answering `true` off the outranked
    /// configured route would let the forwarded packet skip claim
    /// admission entirely and ride for free (the review's findings on
    /// issues #869 and #944).
    fn active_lease_outranks(&self, destination: &str, configured_len: usize) -> bool {
        let leased_routes = self.leased_routes_snapshot();
        let now = self.clock.now();
        let active_leased: Vec<&LeasedRoute> = leased_routes
            .values()
            .filter(|route| !is_expired(route.expires_at(), now))
            .collect();
        let leased_prefixes: Vec<&str> = active_leased.iter().map(|route| route.prefix()).collect();
        select_route(destination, &leased_prefixes)
            .is_some_and(|index| active_leased[index].prefix().len() > configured_len)
    }

    /// Open a terminated packet's gift wrap with this node's identity key
    /// and decode the envelope inside, for a pre-admission probe that has
    /// already established the packet terminates here. `None` when there
    /// is no identity key configured, the wrap does not open, or the
    /// envelope does not decode -- three packets a probe cannot read, so
    /// it cannot tell "refused for its shape" from "unreadable" and
    /// declines to guess. Discards the shared secret: nothing is sealed
    /// on this path, and the delivery path derives its own.
    fn opened_envelope_request(&self, data: &[u8]) -> Option<EnvelopeRequest> {
        let identity_signer = self.identity_signer.as_ref()?;
        let (envelope_bytes, _shared_secret) = open_request(data, identity_signer.as_ref()).ok()?;
        EnvelopeRequest::decode(&envelope_bytes).ok()
    }

    /// This node's static routes, for the operator surface's read-only
    /// inspection interface (issue #420).
    pub fn routes(&self) -> Vec<RouteView> {
        self.routes
            .iter()
            .map(|route| RouteView {
                prefix: route.prefix().to_string(),
                handler_url: route.handler_url().to_string(),
                price: route.price(),
            })
            .collect()
    }

    /// This node's peers (issue #884): every peer id from the config file
    /// plus every runtime-added one, for the operator surface's
    /// `GET /peers`. Peer carriage details -- endpoint, credential,
    /// exposure -- are not reported here; see [`PeerView`]'s own docs.
    pub fn peers(&self) -> Vec<PeerView> {
        let mut views: Vec<PeerView> = self
            .config_peer_ids
            .iter()
            .map(|id| PeerView {
                id: id.clone(),
                fee: self.fee_for(id),
                max_packet_amount: self.packet_cap_for(id),
                source: RouteSource::Config,
            })
            .collect();
        views.extend(self.runtime_peers_snapshot().keys().map(|id| PeerView {
            id: id.clone(),
            fee: self.fee_for(id),
            max_packet_amount: self.packet_cap_for(id),
            source: RouteSource::Runtime,
        }));
        views
    }

    /// This node's payment channels (issue #459) -- every channel this
    /// node has itself opened, each reported fresh from the settlement
    /// backend that opened it (issue #630: on a node settling on more
    /// than one chain, each channel is asked about on its own chain, not
    /// whichever backend attached last). Empty on a node with no
    /// settlement backend configured, or with no channels opened yet,
    /// exactly like every other still-unpopulated operator view above.
    pub async fn channels(&self) -> Vec<ChannelView> {
        let known = self
            .known_channels
            .read()
            .expect("known channels lock poisoned")
            .clone();
        let mut views = Vec::with_capacity(known.len());
        for (chain, id) in known {
            // A known channel was opened through `chain`'s backend, so the
            // lookup cannot fail while `with_settlement` is construction-only.
            let Ok(settlement) = self.settlement_on(chain) else {
                continue;
            };
            if let Ok(state) = settlement.channel_state(&id).await {
                views.push(ChannelView::from(state));
            }
        }
        views
    }

    /// This node's own view of `channel_id`'s on-chain state, fetched fresh
    /// from whichever settlement backend the id's own chain namespace names
    /// (issue #1218): unlike [`Self::channels`], not limited to a channel
    /// this node opened itself -- a client channel its counterparty opened
    /// is exactly as reachable here, since [`Self::settlement_for_channel`]
    /// dispatches on the id's own shape, not on [`Self::known_channels`].
    pub async fn channel_view(
        &self,
        channel_id: &str,
    ) -> Result<ChannelView, ChannelOperationError> {
        let state = self
            .settlement_for_channel(channel_id)?
            .channel_state(&ChannelId(channel_id.to_string()))
            .await?;
        Ok(ChannelView::from(state))
    }

    /// The settlement backend configured for `chain`.
    /// [`ChannelOperationError::NoSettlementBackend`] on a node with no
    /// backend at all; [`ChannelOperationError::NoSettlementBackendForChain`]
    /// on a node that settles, just not there -- so the refusal names the
    /// actual gap.
    fn settlement_on(
        &self,
        chain: SettlementChain,
    ) -> Result<&Arc<dyn SettlementBackend>, ChannelOperationError> {
        if self.settlements.is_empty() {
            return Err(ChannelOperationError::NoSettlementBackend);
        }
        self.settlements
            .iter()
            .find(|(configured, _)| *configured == chain)
            .map(|(_, settlement)| settlement)
            .ok_or(ChannelOperationError::NoSettlementBackendForChain(chain))
    }

    /// Which chain's namespace `channel_id` belongs to, decided by the
    /// id's own shape: an EVM channel id is the `TokenNetwork`'s `bytes32`
    /// as (`0x`-optional) 64-character hex, a Solana one is a channel
    /// PDA's base58 32-byte account address -- the same two namespaces the
    /// client edge's `ClientChannelRegistry` already keeps separate ("a
    /// `channelId` and a `channelAccount` are different kinds of thing and
    /// can never satisfy each other"), and provably disjoint: 64 base58
    /// characters decode to ~47 bytes, never 32, and a 32-byte account is
    /// at most 44 base58 characters, never 64. `None` for an id in
    /// neither namespace.
    fn channel_id_chain(channel_id: &str) -> Option<SettlementChain> {
        let hex = channel_id.strip_prefix("0x").unwrap_or(channel_id);
        if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(SettlementChain::Evm);
        }
        if bs58::decode(channel_id)
            .into_vec()
            .is_ok_and(|bytes| bytes.len() == 32)
        {
            return Some(SettlementChain::Solana);
        }
        None
    }

    /// The settlement backend `channel_id`'s own chain names (issue #630)
    /// -- how every per-channel operation below picks a backend. A node
    /// with a single backend routes every id to it, keeping the port's
    /// "ids are opaque" promise where there is nothing to disambiguate (and
    /// keeping non-chain-shaped ids, like the in-memory backend's counters,
    /// working); only a node settling on several chains reads the id's
    /// namespace ([`Self::channel_id_chain`]). An id in no known namespace
    /// is [`SettlementError::ChannelNotFound`], exactly as each backend
    /// already answers for a malformed id ("a malformed id and one nothing
    /// was ever opened at mean the same thing").
    fn settlement_for_channel(
        &self,
        channel_id: &str,
    ) -> Result<&Arc<dyn SettlementBackend>, ChannelOperationError> {
        match self.settlements.as_slice() {
            [] => Err(ChannelOperationError::NoSettlementBackend),
            [(_, settlement)] => Ok(settlement),
            _ => {
                let chain = Self::channel_id_chain(channel_id).ok_or_else(|| {
                    ChannelOperationError::Settlement(SettlementError::ChannelNotFound(ChannelId(
                        channel_id.to_string(),
                    )))
                })?;
                self.settlement_on(chain)
            }
        }
    }

    /// Open a new channel to `counterparty` on `chain` (issue #459),
    /// remembering its id -- and the chain it lives on -- so a future
    /// [`Connector::channels`] call reports on it from the right backend.
    /// `None` means "the configured backend" and is accepted exactly when
    /// that denotes something: a node with several backends refuses with
    /// [`ChannelOperationError::AmbiguousSettlementChain`] rather than
    /// silently picking one (issue #630). The counterparty and
    /// settlement-timeout semantics are exactly the chosen
    /// [`SettlementBackend`]'s own -- this method adds nothing beyond
    /// bookkeeping.
    pub async fn open_channel(
        &self,
        chain: Option<SettlementChain>,
        counterparty: Vec<u8>,
        settlement_timeout: Duration,
    ) -> Result<ChannelView, ChannelOperationError> {
        let (chain, settlement) = match chain {
            Some(chain) => (chain, self.settlement_on(chain)?),
            None => match self.settlements.as_slice() {
                [] => return Err(ChannelOperationError::NoSettlementBackend),
                [(chain, settlement)] => (*chain, settlement),
                _ => return Err(ChannelOperationError::AmbiguousSettlementChain),
            },
        };
        let id = settlement.open(counterparty, settlement_timeout).await?;
        self.known_channels
            .write()
            .expect("known channels lock poisoned")
            .push((chain, id.clone()));
        let state = settlement.channel_state(&id).await?;
        Ok(ChannelView::from(state))
    }

    /// Deposit `amount` into `channel_id` (issue #459), on whichever chain
    /// the id itself names ([`Self::settlement_for_channel`]).
    pub async fn fund_channel(
        &self,
        channel_id: &str,
        amount: u128,
    ) -> Result<ChannelView, ChannelOperationError> {
        let state = self
            .settlement_for_channel(channel_id)?
            .fund(&ChannelId(channel_id.to_string()), amount)
            .await?;
        Ok(ChannelView::from(state))
    }

    /// Raise this node's own deposit in `channel_id` to `own_total`, on
    /// whichever chain the id names: the retry-safe form of
    /// [`Self::fund_channel`] (ADR 0073, `SettlementBackend::fund_to`).
    pub async fn fund_channel_to(
        &self,
        channel_id: &str,
        own_total: u128,
    ) -> Result<ChannelView, ChannelOperationError> {
        let state = self
            .settlement_for_channel(channel_id)?
            .fund_to(&ChannelId(channel_id.to_string()), own_total)
            .await?;
        Ok(ChannelView::from(state))
    }

    /// Redeem `claim` against `channel_id` (issue #459), on whichever chain
    /// the id itself names ([`Self::settlement_for_channel`]).
    pub async fn redeem_channel(
        &self,
        channel_id: &str,
        claim: Claim,
    ) -> Result<ChannelView, ChannelOperationError> {
        let state = self
            .settlement_for_channel(channel_id)?
            .redeem(&ChannelId(channel_id.to_string()), claim)
            .await?;
        Ok(ChannelView::from(state))
    }

    /// Close `channel_id` (issue #459), on whichever chain the id itself
    /// names ([`Self::settlement_for_channel`]): no further funding or
    /// redemption is possible against it afterward.
    pub async fn close_channel(
        &self,
        channel_id: &str,
    ) -> Result<ChannelView, ChannelOperationError> {
        let state = self
            .settlement_for_channel(channel_id)?
            .close(&ChannelId(channel_id.to_string()))
            .await?;
        Ok(ChannelView::from(state))
    }

    /// Settle `channel_id` (issue #1129) once its challenge period --
    /// the `settlement_timeout` [`Connector::open_channel`] was given,
    /// counted from [`Connector::close_channel`] -- has elapsed: the
    /// remainder of each side's deposit is paid back out on chain and the
    /// channel becomes permanently done.
    ///
    /// This is the operation that *finishes* a close. Nothing else does:
    /// [`Connector::close_channel`] only starts the challenge window
    /// (issue #574), and [`Connector::cooperative_close`] is a redeem
    /// followed by that same close, so a channel closed by either route
    /// still holds every un-claimed deposit until this runs. Before #1129
    /// no surface reached it at all and the remainder was recoverable only
    /// by an out-of-band chain call.
    ///
    /// [`SettlementError::SettlementNotYetDue`] while the window is still
    /// open (or if the channel was never closed) -- a named, retry-later
    /// answer rather than a generic backend failure. There is deliberately
    /// no timer here that settles a channel on the node's own initiative:
    /// when a node settles is an operator's call, made with an
    /// authenticated write, exactly like when it closes.
    pub async fn settle_channel(
        &self,
        channel_id: &str,
    ) -> Result<ChannelView, ChannelOperationError> {
        let state = self
            .settlement_for_channel(channel_id)?
            .settle(&ChannelId(channel_id.to_string()))
            .await?;
        Ok(ChannelView::from(state))
    }

    /// Redeem the latest claim this node has accepted on `channel_id`
    /// (issue #425, story 36): looks up the highest-nonce claim this node
    /// has ever verified and accepted from that channel's counterparty --
    /// never a superseded one, since `ClaimBook` only ever retains the
    /// latest -- and submits exactly that one claim to the configured
    /// settlement backend. [`ChannelOperationError::NoClaimToRedeem`] if
    /// this channel has never had a claim accepted on it; a claim already
    /// fully redeemed reports [`SettlementError::StaleClaim`] through the
    /// backend rather than being treated as success here, so a failed
    /// submission never silently reports a stale channel state as if the
    /// redemption happened (leaving the channel's actual on-chain state
    /// the one place a caller need look to retry).
    pub async fn redeem_latest_claim(
        &self,
        channel_id: &str,
    ) -> Result<ChannelView, ChannelOperationError> {
        let claim = self
            .claims
            .latest_inbound_claim(channel_id)
            .ok_or(ChannelOperationError::NoClaimToRedeem)?;
        let state = self
            .settlement_for_channel(channel_id)?
            .redeem(&ChannelId(channel_id.to_string()), claim)
            .await?;
        Ok(ChannelView::from(state))
    }

    /// Cooperatively close `channel_id` (issue #425, story 37): redeem
    /// whatever claim this node last accepted on it, then close -- one
    /// operator-driven action rather than two.
    ///
    /// "Cooperative" names the *claim* half only: it collects what this
    /// node is owed in the same breath as closing, rather than leaving a
    /// redemption to be raced into the challenge window afterwards. It
    /// does **not** skip that window. This method was written (2026-07-26)
    /// against a port where `close` was terminal; issue #574 reversed that
    /// two days later, and `close` now starts a challenge period on both
    /// chains -- so the deposits still sit on chain until
    /// [`Connector::settle_channel`] runs (issue #1129). A caller wanting
    /// the collateral back needs both.
    ///
    /// A channel with no claim ever accepted closes directly,
    /// exactly like [`Connector::close_channel`]. A claim already fully
    /// redeemed (`SettlementError::StaleClaim`, or `StaleNonce` -- issue
    /// #573 -- for the same already-redeemed claim) is not a reason to
    /// refuse closing -- there is nothing left to collect -- but any other
    /// redemption failure stops here without closing, so a reverted or
    /// failed settlement transaction leaves the channel open and the claim
    /// still redeemable rather than closing over an unclaimed balance.
    pub async fn cooperative_close(
        &self,
        channel_id: &str,
    ) -> Result<ChannelView, ChannelOperationError> {
        let settlement = self.settlement_for_channel(channel_id)?;
        let id = ChannelId(channel_id.to_string());
        if let Some(claim) = self.claims.latest_inbound_claim(channel_id) {
            match settlement.redeem(&id, claim).await {
                Ok(_)
                | Err(SettlementError::StaleClaim { .. })
                | Err(SettlementError::StaleNonce { .. }) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let state = settlement.close(&id).await?;
        Ok(ChannelView::from(state))
    }

    /// Claims exchanged with peers (issue #423), for the operator surface's
    /// read-only inspection interface.
    pub fn claims(&self) -> Vec<ClaimView> {
        self.claims.views()
    }

    /// The latest claim this connector's own peer-semantics book -- never
    /// the client edge's second book -- has accepted on `channel_id` (issue
    /// #1218): the read half of what [`Self::redeem_latest_claim`] and
    /// [`Self::cooperative_close`] already consult internally, exposed so a
    /// caller holding a second claim book (`connector_client_edge::ClientClaimGate`)
    /// can tell, before it redeems anything, whether the peer book is the
    /// authority for `channel_id` or whether it should fall back to its own
    /// -- the same "which book is the authority is a property of the
    /// channel, never of who is asking" doctrine `Self::peer_channel_watermark`
    /// already serves `POST /ilp/claim-state` with.
    pub fn peer_inbound_claim(&self, channel_id: &str) -> Option<connector_settlement::Claim> {
        self.claims.latest_inbound_claim(channel_id)
    }
}

fn leased_route_view(route: &LeasedRoute) -> LeasedRouteView {
    LeasedRouteView {
        prefix: route.prefix().to_string(),
        peer_id: route.peer_id().to_string(),
        expires_at: route.expires_at(),
    }
}

/// The shape a denomination crossing has: the incoming and outgoing tokens,
/// the peering being forwarded to, what arrived and what that peering
/// charges -- answering the outgoing amount, or the reject that refuses it.
type CrossesADenomination =
    fn(&Connector, &AssetId, &AssetId, &str, u64, u64) -> Result<u64, Reject>;

/// ADR 0071 decision 6's "the forwarding path does **no I/O**", asserted at
/// compile time rather than trusted -- the same argument
/// [`crate::rate_table`] makes about the table's reader, made again about
/// the one caller on the packet path that reads it.
///
/// Coercing [`Connector::convert_for_forward`] to a plain `fn` pointer only
/// type-checks while it is a synchronous function: an `async fn` returns a
/// future and would not coerce. So the day a crossing grows something to
/// await -- a rate source dialed inline, a chain read, a lock with a
/// timeout -- the build breaks here rather than a packet paying for it.
const _: CrossesADenomination = Connector::convert_for_forward;

/// The shape the same crossing has travelling upstream: the two tokens, the
/// peering the reject came back from, the running cost in the outgoing
/// leg's unit and that peering's fee -- answering the cost in the incoming
/// leg's unit. There is no refusal in the return type because there is no
/// one left to refuse: a reject is already on its way (ADR 0071 decision 7).
type CrossesADenominationUpstream = fn(&Connector, &AssetId, &AssetId, &str, u64, u64) -> u64;

/// Decision 6's no-I/O rule again, asserted about the reject path for the
/// same reason it is asserted about the forward: the return leg is the
/// packet path too, and a rate dialled from here would hold a reject open
/// while a sender waited to learn what a path costs.
const _: CrossesADenominationUpstream = Connector::convert_for_reject;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_client::FakeAppClient;
    use crate::clock::TestClock;
    use crate::peer_transport::{InProcessPeerTransport, PeerForward, PeerTransport};
    use crate::test_support::{
        answered, answered_with_status, covering, expected_fulfillment, fulfill_envelope,
        fulfill_envelope_with_status, identity_signer, open_sealed_envelope,
        sealed_envelope_request_data, sealed_envelope_request_data_with_headers,
        sealed_envelope_request_data_with_target, test_channel_domain, test_channel_id,
    };
    use async_trait::async_trait;
    use chrono::{Duration, TimeZone, Utc};
    use connector_domain::{Guards, MaxMove, Rate, RateTable, Spread, Ttl};
    use connector_signer::derive_evm_address;

    /// Seals `data` (issue #524) -- what a genuine sender does before ever
    /// transmitting a packet, so a plain `prepare()` call is, by
    /// construction, one that fulfils if it reaches an app that answers at
    /// all: the termination derives its fulfilment from this same sealed
    /// secret (ADR 0019). A test that also needs the secret back (to open
    /// the sealed response, or assert the exact fulfilment) uses
    /// [`sealed_prepare`] instead.
    fn prepare(destination: &str, data: &[u8]) -> Prepare {
        // Comfortably after `test_clock()`'s instant, so tests that don't
        // care about expiry aren't incidentally right at the boundary.
        prepare_expiring_at(
            destination,
            data,
            Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        )
    }

    fn prepare_expiring_at(
        destination: &str,
        data: &[u8],
        expires_at: chrono::DateTime<Utc>,
    ) -> Prepare {
        let (data, _shared_secret) = sealed_envelope_request_data(data);
        Prepare {
            amount: 0,
            expires_at,
            greeting: false,
            destination: destination.to_string(),
            data,
        }
    }

    fn prepare_with_amount(destination: &str, amount: u64) -> Prepare {
        Prepare {
            amount,
            ..prepare(destination, b"hello")
        }
    }

    /// `prepare("g.example.app", ..)` with `data` overwritten by
    /// caller-chosen bytes -- every termination test in this module
    /// addresses `"g.example.app"` and only cares that `data` itself is
    /// shaped correctly, since `prepare()`'s own plaintext `data` never
    /// survives past this override. Used for a `data` that is garbage, an
    /// envelope sealed under a different secret than any this test tracks,
    /// or otherwise a shape a test wants to hand `Connector::handle_prepare`
    /// without also building a whole `Prepare` by hand.
    fn prepare_with_data(data: Vec<u8>) -> Prepare {
        Prepare {
            data,
            ..prepare("g.example.app", b"unused")
        }
    }

    /// [`sealed_prepare_to`], addressed to `"g.example.app"` -- the
    /// destination almost every sealed-request test terminates at.
    fn sealed_prepare(body: &[u8]) -> (Prepare, [u8; 32]) {
        sealed_prepare_to("g.example.app", body)
    }

    /// A `Prepare` for `destination`, sealed to [`identity_signer`]'s
    /// identity and carrying `body` (issue #524) -- the common case for a
    /// test that drives `Connector::handle_prepare` directly rather than
    /// through the HTTP router and expects the packet to genuinely fulfil,
    /// the termination deriving its fulfilment from this same sealed secret
    /// (ADR 0019). Returns the shared secret alongside, to open the sealed
    /// `Fulfill`/termination-`Reject` this produces, or to compute the
    /// expected fulfilment via `expected_fulfillment`.
    fn sealed_prepare_to(destination: &str, body: &[u8]) -> (Prepare, [u8; 32]) {
        let (data, shared_secret) = sealed_envelope_request_data(body);
        let prepare = Prepare {
            data,
            ..prepare(destination, b"unused")
        };
        (prepare, shared_secret)
    }

    fn test_clock() -> Arc<TestClock> {
        Arc::new(TestClock::new(
            Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
        ))
    }

    fn connector_with(
        routes: Vec<StaticRoute>,
        app_client: Arc<FakeAppClient>,
        clock: Arc<TestClock>,
    ) -> Connector {
        Connector::new(
            routes,
            vec![],
            app_client,
            Arc::new(InProcessPeerTransport::new()),
            clock,
        )
        .with_identity_signer(identity_signer())
    }

    #[tokio::test]
    async fn delivers_a_packet_matching_a_terminated_route() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(route.handler_url(), answered(b"app said yes"));
        let clock = test_clock();
        let connector = connector_with(vec![route], app_client.clone(), clock);
        let (sealed, shared_secret) = sealed_prepare(b"hello app");

        let response = connector.handle_prepare(sealed).await;

        match response {
            PacketResponse::Fulfill(fulfill) => {
                assert_eq!(fulfill.fulfillment, expected_fulfillment(&shared_secret));
                assert_eq!(
                    open_sealed_envelope(&shared_secret, &fulfill.data),
                    fulfill_envelope(b"app said yes")
                );
            }
            other => panic!("expected a fulfill, got {other:?}"),
        }

        let deliveries = app_client.deliveries();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].request.body, b"hello app");
    }

    /// ADR 0040 (issue #994): what a terminating connector tells the app
    /// about the payment that brought a packet to it. Every case here
    /// asserts the headers a `FakeAppClient` actually recorded receiving,
    /// never the call site -- including the cases whose whole content is
    /// that a header is *absent*.
    mod payment_attribution {
        use super::*;
        use crate::attribution::{AMOUNT_HEADER, CHAIN_HEADER, PAYER_HEADER};
        use crate::Delivery;

        /// The channel key the client edge admits a covering EVM claim
        /// under -- `ClientClaim::channel_key`'s own spelling.
        const PAYING_CHANNEL: &str =
            "evm:0x1111111111111111111111111111111111111111111111111111111111111111";

        const PRICE: u64 = 1000;

        fn header<'a>(delivery: &'a Delivery, name: &str) -> Option<&'a str> {
            delivery
                .request
                .headers
                .iter()
                .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        }

        fn one_delivery(app_client: &FakeAppClient) -> Delivery {
            let deliveries = app_client.deliveries();
            assert_eq!(deliveries.len(), 1, "expected exactly one delivery");
            deliveries.into_iter().next().expect("one delivery")
        }

        /// Deliver one sealed packet to a route priced at `price`, admitted
        /// by `client_channel_id` (or by nothing, when `None`), and return
        /// what the app was handed.
        async fn deliver(
            price: u64,
            client_channel_id: Option<&str>,
            sealed_data: Vec<u8>,
        ) -> Delivery {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", price).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"stored"));
            let connector = connector_with(vec![route], app_client.clone(), test_clock());
            let prepare = Prepare {
                data: sealed_data,
                ..prepare("g.example.app", b"unused")
            };

            let response = connector
                .handle_prepare_with_client_channel(prepare, client_channel_id)
                .await;
            assert!(
                matches!(response, PacketResponse::Fulfill(_)),
                "expected the delivery to fulfil, got {response:?}"
            );
            one_delivery(&app_client)
        }

        /// The defect this ADR closes, stated as the delivery the store
        /// behind this connector actually receives: a paid write names the
        /// channel that paid for it, what it paid, and the chain that
        /// channel settles on.
        #[tokio::test]
        async fn a_paid_delivery_names_the_channel_whose_claim_admitted_it() {
            let (data, _secret) = sealed_envelope_request_data(b"an event");

            let delivery = deliver(PRICE, Some(PAYING_CHANNEL), data).await;

            assert_eq!(header(&delivery, PAYER_HEADER), Some(PAYING_CHANNEL));
            assert_eq!(header(&delivery, AMOUNT_HEADER), Some("1000"));
            assert_eq!(header(&delivery, CHAIN_HEADER), Some("evm"));
        }

        /// The chain is read off the admitted claim's own namespace, not
        /// off the destination address (ADR 0017's objection to the
        /// TypeScript header): the same destination, paid from a Solana
        /// channel, says `solana`.
        #[tokio::test]
        async fn the_chain_comes_from_the_claim_not_the_destination() {
            let (data, _secret) = sealed_envelope_request_data(b"an event");

            let delivery = deliver(
                PRICE,
                Some("solana:9xQeWvG816bUx9EPjHmaT23yvVM2ZHbGrX"),
                data,
            )
            .await;

            assert_eq!(header(&delivery, CHAIN_HEADER), Some("solana"));
        }

        /// Nothing admitted this packet at this connector's own edge -- a
        /// peer-role arrival, or an unclaimed request -- so there is no
        /// payer to name and none is invented. This is the case that makes
        /// ADR 0017's "on a longer path the header names the wrong party"
        /// unreachable rather than merely avoided.
        #[tokio::test]
        async fn a_delivery_no_claim_admitted_states_no_attribution() {
            let (data, _secret) = sealed_envelope_request_data(b"an event");

            let delivery = deliver(PRICE, None, data).await;

            assert_eq!(header(&delivery, PAYER_HEADER), None);
            assert_eq!(header(&delivery, AMOUNT_HEADER), None);
            assert_eq!(header(&delivery, CHAIN_HEADER), None);
        }

        /// A free route charged nothing, so there is no payment to
        /// attribute even when a claim rode along with the request.
        #[tokio::test]
        async fn a_free_routes_delivery_states_no_attribution() {
            let (data, _secret) = sealed_envelope_request_data(b"an event");

            let delivery = deliver(0, Some(PAYING_CHANNEL), data).await;

            assert_eq!(header(&delivery, PAYER_HEADER), None);
            assert_eq!(header(&delivery, AMOUNT_HEADER), None);
            assert_eq!(header(&delivery, CHAIN_HEADER), None);
        }

        /// The spoof defence: a sender who seals its own `X-TOON-Payer`
        /// into the envelope has it overwritten by the channel that
        /// actually paid, not appended alongside it.
        #[tokio::test]
        async fn a_spoofed_payer_is_overwritten_by_the_admitted_one() {
            let (data, _secret) = sealed_envelope_request_data_with_headers(
                "/",
                vec![
                    (
                        "X-TOON-Payer".to_string(),
                        "evm:0xdeadbeef-someone-else".to_string(),
                    ),
                    ("x-toon-amount".to_string(), "1".to_string()),
                    ("X-Toon-Chain".to_string(), "solana".to_string()),
                ],
                b"an event",
            );

            let delivery = deliver(PRICE, Some(PAYING_CHANNEL), data).await;

            assert_eq!(header(&delivery, PAYER_HEADER), Some(PAYING_CHANNEL));
            assert_eq!(header(&delivery, AMOUNT_HEADER), Some("1000"));
            assert_eq!(header(&delivery, CHAIN_HEADER), Some("evm"));
            assert_eq!(
                delivery
                    .request
                    .headers
                    .iter()
                    .filter(|(name, _)| name.eq_ignore_ascii_case(PAYER_HEADER))
                    .count(),
                1,
                "the sender's spelling must be removed, not joined by ours"
            );
        }

        /// And the harder half of the same defence: on a delivery this
        /// connector states nothing about, a sender's own headers are
        /// still removed -- otherwise addressing a free (or peer-reached)
        /// route would be all it takes to hand an app a forged payer.
        #[tokio::test]
        async fn a_spoofed_payer_does_not_survive_an_unattributed_delivery() {
            let spoofed = vec![
                ("X-TOON-Payer".to_string(), "evm:0xvictim".to_string()),
                ("X-TOON-Amount".to_string(), "999999".to_string()),
                ("X-TOON-Chain".to_string(), "evm".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ];
            let (data, _secret) =
                sealed_envelope_request_data_with_headers("/", spoofed, b"an event");

            let delivery = deliver(0, None, data).await;

            assert_eq!(header(&delivery, PAYER_HEADER), None);
            assert_eq!(header(&delivery, AMOUNT_HEADER), None);
            assert_eq!(header(&delivery, CHAIN_HEADER), None);
            // The sender's other headers are its own business and reach
            // the app untouched.
            assert_eq!(header(&delivery, "content-type"), Some("application/json"));
        }
    }

    #[tokio::test]
    async fn rejects_a_packet_with_no_matching_route() {
        let app_client = Arc::new(FakeAppClient::new());
        let clock = test_clock();
        let connector = connector_with(vec![], app_client.clone(), clock);

        let response = connector
            .handle_prepare(prepare("g.nowhere", b"hello"))
            .await;

        match response {
            PacketResponse::Reject(reject) => {
                assert_eq!(reject.code.as_str(), "F02");
                assert!(reject.message.contains("g.nowhere"));
            }
            other => panic!("expected a reject, got {other:?}"),
        }
        assert!(app_client.deliveries().is_empty());
    }

    /// A minimal [`tracing::Subscriber`] that records the field values a
    /// single named span (`"packet"`, in practice) carries -- no
    /// formatting, no filtering by level, so it captures a span's fields
    /// whether or not anything was ever logged inside it. Issue #535/ADR
    /// 0036's own acceptance criterion is that `client_channel_id` is
    /// asserted from the emitted span, not from inspecting the call site
    /// -- this is that capture.
    ///
    /// Every span gets the same id, so a deferred `record` -- which is how
    /// `client_channel_id` arrives -- cannot be attributed back to the span
    /// it belongs to and is captured unconditionally; only span *creation*
    /// is filtered by name. Harmless here: `"packet"` is the only span
    /// these tests exercise.
    struct SpanFieldCapture {
        span_name: &'static str,
        fields: Arc<Mutex<HashMap<String, String>>>,
    }

    struct StringVisitor<'a>(&'a mut HashMap<String, String>);

    impl tracing::field::Visit for StringVisitor<'_> {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0
                .insert(field.name().to_string(), format!("{value:?}"));
        }
    }

    impl tracing::Subscriber for SpanFieldCapture {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            if attrs.metadata().name() == self.span_name {
                let mut fields = self.fields.lock().unwrap();
                let mut visitor = StringVisitor(&mut fields);
                attrs.record(&mut visitor);
            }
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, values: &tracing::span::Record<'_>) {
            let mut fields = self.fields.lock().unwrap();
            let mut visitor = StringVisitor(&mut fields);
            values.record(&mut visitor);
        }

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
        fn event(&self, _event: &tracing::Event<'_>) {}
        fn enter(&self, _span: &tracing::span::Id) {}
        fn exit(&self, _span: &tracing::span::Id) {}
    }

    /// Run `work` under a [`SpanFieldCapture`] on the `"packet"` span, and
    /// return the fields that span actually recorded.
    async fn packet_span_fields<F: std::future::Future<Output = ()>>(
        work: F,
    ) -> HashMap<String, String> {
        let fields = Arc::new(Mutex::new(HashMap::new()));
        let guard = tracing::subscriber::set_default(SpanFieldCapture {
            span_name: "packet",
            fields: Arc::clone(&fields),
        });
        // Prove the `info_span!("packet")` callsite records into THIS
        // capture before running `work`, by probing the entry point until
        // the capture observably fires. Nothing weaker is race-free under
        // the default parallel `cargo test`: tracing-core caches each
        // callsite's `Interest` globally, computed once by whatever thread
        // touches it first -- possibly a concurrent test with no subscriber
        // at all, caching `Interest::never` -- and
        // `DefaultCallsite::register` publishes that interest BEFORE it
        // pushes the callsite into the registry `rebuild_interest_cache()`
        // walks, while `interest()` short-circuits on the published value.
        // So a single warm-up touch can return having fixed nothing (the
        // touch short-circuits on `never` mid-registration) and a single
        // rebuild can miss (the registry does not contain the callsite
        // yet). Probing until a field is actually captured closes every
        // ordering: once the concurrent registration completes, a rebuild
        // recomputes against this capture and the probe records.
        let probe = connector_with(vec![], Arc::new(FakeAppClient::new()), test_clock());
        let mut attempts = 0u32;
        loop {
            tracing::callsite::rebuild_interest_cache();
            let _ = probe.handle_prepare(prepare("g.nowhere", b"probe")).await;
            if fields.lock().unwrap().contains_key("correlation_id") {
                // The probe's own fields must not leak into `work`'s
                // assertions.
                fields.lock().unwrap().clear();
                break;
            }
            attempts += 1;
            assert!(
                attempts < 1_000,
                "the `packet` callsite never became enabled under this capture"
            );
            std::thread::yield_now();
        }

        work.await;

        drop(guard);
        let captured = fields.lock().unwrap();
        captured.clone()
    }

    #[tokio::test]
    async fn packet_span_carries_the_admitting_client_channel_id_when_a_claim_admitted_the_packet()
    {
        let app_client = Arc::new(FakeAppClient::new());
        let clock = test_clock();
        let connector = connector_with(vec![], app_client, clock);

        let fields = packet_span_fields(async {
            let _ = connector
                .handle_prepare_with_client_channel(
                    prepare("g.nowhere", b"hello"),
                    Some("evm:0xdeadbeef"),
                )
                .await;
        })
        .await;

        assert_eq!(
            fields.get("client_channel_id").map(String::as_str),
            Some("evm:0xdeadbeef"),
            "captured: {fields:?}"
        );
    }

    #[tokio::test]
    async fn packet_span_omits_client_channel_id_when_no_claim_admitted_the_packet() {
        let app_client = Arc::new(FakeAppClient::new());
        let clock = test_clock();
        let connector = connector_with(vec![], app_client, clock);

        let fields = packet_span_fields(async {
            // The ordinary `handle_prepare` entry point -- no admitting
            // client channel available at all, exactly the peer-role and
            // unclaimed-request shapes.
            let _ = connector
                .handle_prepare(prepare("g.nowhere", b"hello"))
                .await;
        })
        .await;

        // `correlation_id` proves the capture saw the span at all, so the
        // absence below is the field being omitted rather than nothing
        // having been captured.
        assert!(
            fields.contains_key("correlation_id"),
            "the capture saw no `packet` span: {fields:?}"
        );
        assert!(!fields.contains_key("client_channel_id"));
    }

    #[tokio::test]
    async fn rejects_a_packet_that_has_already_expired_and_never_delivers_it() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        let now = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let clock = Arc::new(TestClock::new(now));
        let connector = connector_with(vec![route], app_client.clone(), clock);
        let already_expired =
            prepare_expiring_at("g.example.app", b"hello", now - Duration::seconds(1));

        let response = connector.handle_prepare(already_expired).await;

        match response {
            PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "R00"),
            other => panic!("expected a reject, got {other:?}"),
        }
        // The in-flight record is released rather than handed to the app:
        // an expired packet never reaches delivery.
        assert!(app_client.deliveries().is_empty());
    }

    #[tokio::test]
    async fn a_packet_expires_only_once_the_injected_clock_advances_past_it() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(route.handler_url(), answered(b"still on time"));
        let start = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let clock = Arc::new(TestClock::new(start));
        let connector = connector_with(vec![route], app_client.clone(), clock.clone());
        let expires_at = start + Duration::seconds(30);

        let response = connector
            .handle_prepare(prepare_expiring_at("g.example.app", b"hello", expires_at))
            .await;
        assert!(matches!(response, PacketResponse::Fulfill(_)));

        clock.advance(Duration::seconds(30));
        let response = connector
            .handle_prepare(prepare_expiring_at("g.example.app", b"hello", expires_at))
            .await;
        match response {
            PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "R00"),
            other => panic!("expected a reject once the clock reaches expiry, got {other:?}"),
        }
    }

    /// Issue #1269 / ADR 0069: a terminated packet still derives. Before this
    /// change, `prepare_with_data` built a packet whose sealed `data` carried
    /// a different secret than the one `prepare()`'s own execution condition
    /// was minted from -- a mismatch `Self::accept_if_fulfilled` rejected
    /// with F99. There is no execution condition left to mismatch: whatever
    /// secret the wrap that actually opens carries is the one the
    /// termination derives its fulfilment from, so a genuinely-sealed,
    /// genuinely-deliverable packet fulfils regardless of which secret built
    /// `data`, exactly as it does when `prepare()`'s own default `data` is
    /// used instead.
    #[tokio::test]
    async fn a_termination_derives_from_whichever_secret_the_wrap_that_opens_actually_carries() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(route.handler_url(), answered(b"app said yes"));
        let clock = test_clock();
        let connector = connector_with(vec![route], app_client.clone(), clock);
        let (data, shared_secret) = sealed_envelope_request_data(b"hello");

        let response = connector.handle_prepare(prepare_with_data(data)).await;

        match response {
            PacketResponse::Fulfill(fulfill) => {
                assert_eq!(fulfill.fulfillment, expected_fulfillment(&shared_secret));
            }
            other => panic!("expected a fulfill, got {other:?}"),
        }
        assert_eq!(app_client.deliveries().len(), 1);
    }

    /// Issue #521's central rule (ADR 0020): "you pay for an answer, not
    /// the answer you wanted." A 404 is a real answer that consumed real
    /// work, so it fulfils exactly like a 200 does -- rejecting on a
    /// non-2xx would make app errors free.
    #[tokio::test]
    async fn a_non_2xx_response_from_the_app_still_fulfils() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(
            route.handler_url(),
            answered_with_status(402, b"insufficient funds"),
        );
        let clock = test_clock();
        let connector = connector_with(vec![route], app_client, clock);
        let (sealed, shared_secret) = sealed_prepare(b"hello");

        let response = connector.handle_prepare(sealed).await;

        match response {
            PacketResponse::Fulfill(fulfill) => {
                assert_eq!(fulfill.fulfillment, expected_fulfillment(&shared_secret));
                assert_eq!(
                    open_sealed_envelope(&shared_secret, &fulfill.data),
                    fulfill_envelope_with_status(402, b"insufficient funds")
                );
            }
            other => panic!("expected a fulfill, got {other:?}"),
        }
    }

    /// The other half of the same rule stated negatively: a non-2xx
    /// response is not itself what causes a reject, whichever secret sealed
    /// the wrap that opened (issue #1269 / ADR 0069 -- there is no longer a
    /// mismatch for it to matter against).
    #[tokio::test]
    async fn a_non_2xx_response_still_fulfils_regardless_of_which_secret_sealed_the_wrap() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(
            route.handler_url(),
            answered_with_status(402, b"insufficient funds"),
        );
        let clock = test_clock();
        let connector = connector_with(vec![route], app_client.clone(), clock);
        let (data, shared_secret) = sealed_envelope_request_data(b"hello");

        let response = connector.handle_prepare(prepare_with_data(data)).await;

        match response {
            PacketResponse::Fulfill(fulfill) => {
                assert_eq!(fulfill.fulfillment, expected_fulfillment(&shared_secret));
            }
            other => panic!("expected a fulfill, got {other:?}"),
        }
        assert_eq!(app_client.deliveries().len(), 1);
    }

    #[tokio::test]
    async fn an_unreachable_app_produces_a_peer_unreachable_reject() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        // No FakeAppClient::respond call: the fake defaults to Unreachable.
        let app_client = Arc::new(FakeAppClient::new());
        let clock = test_clock();
        let connector = connector_with(vec![route], app_client, clock);

        let response = connector
            .handle_prepare(prepare("g.example.app", b"hello"))
            .await;

        match response {
            PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "T01"),
            other => panic!("expected a reject, got {other:?}"),
        }
    }

    // `uses_the_injected_clock_rather_than_wall_time`, which lived here,
    // asserted that a `received_at` timestamp derived from the injected
    // clock reached the app client. Issue #521 removes that timestamp
    // entirely: the connector now makes exactly the request the envelope
    // describes (AC1), with no header of its own added, so there is
    // nothing left for this test to observe. The injected clock's effect
    // on expiry is unchanged and still covered by
    // `a_packet_expires_only_once_the_injected_clock_advances_past_it`.

    #[tokio::test]
    async fn selects_the_most_specific_route_when_several_match() {
        let general = StaticRoute::new("g.example", "http://localhost:4000").unwrap();
        let specific = StaticRoute::new("g.example.app", "http://localhost:5000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(specific.handler_url(), answered(b""));
        let clock = test_clock();
        let connector = connector_with(vec![general, specific.clone()], app_client.clone(), clock);

        let response = connector
            .handle_prepare(prepare("g.example.app", b"hello"))
            .await;

        assert!(matches!(response, PacketResponse::Fulfill(_)));
        assert_eq!(
            app_client.deliveries()[0].handler_url,
            *specific.handler_url()
        );
    }

    #[tokio::test]
    async fn forwards_a_packet_matching_a_peer_route_to_the_next_hop() {
        let second_hop_route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let second_hop_app_client = Arc::new(FakeAppClient::new());
        second_hop_app_client.respond(
            second_hop_route.handler_url(),
            answered(b"delivered by the second hop"),
        );
        let second_hop = Arc::new(
            Connector::new(
                vec![second_hop_route],
                vec![],
                second_hop_app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(identity_signer()),
        );
        let mut peer_transport = InProcessPeerTransport::new();
        peer_transport.add_peer("second-hop", second_hop);
        let first_hop = covering(
            Connector::new(
                vec![],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                Arc::new(FakeAppClient::new()),
                Arc::new(peer_transport),
                test_clock(),
            ),
            "second-hop",
        );
        let (sealed, shared_secret) = sealed_prepare(b"hello");

        let response = first_hop.handle_prepare(sealed).await;

        match response {
            PacketResponse::Fulfill(fulfill) => {
                assert_eq!(fulfill.fulfillment, expected_fulfillment(&shared_secret));
                assert_eq!(
                    open_sealed_envelope(&shared_secret, &fulfill.data),
                    fulfill_envelope(b"delivered by the second hop")
                );
            }
            other => panic!("expected a fulfill, got {other:?}"),
        }
        assert_eq!(second_hop_app_client.deliveries().len(), 1);
    }

    /// ADR 0057, issue #1143: no floor rides beside the packet any more,
    /// so the only arithmetic left is this hop's own fee -- and a packet
    /// that cannot even pay it is refused rather than forwarded at nothing.
    #[tokio::test]
    async fn a_hop_whose_fee_exceeds_the_packet_rejects_without_forwarding() {
        let second_hop_route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let second_hop_app_client = Arc::new(FakeAppClient::new());
        second_hop_app_client.respond(second_hop_route.handler_url(), answered(b""));
        let second_hop = Arc::new(Connector::new(
            vec![second_hop_route],
            vec![],
            second_hop_app_client.clone(),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let mut peer_transport = InProcessPeerTransport::new();
        peer_transport.add_peer("second-hop", second_hop);
        let first_hop = Connector::new(
            vec![],
            vec![PeerRoute::new("g.example.app", "second-hop")],
            Arc::new(FakeAppClient::new()),
            Arc::new(peer_transport),
            test_clock(),
        )
        .with_peer_fees([("second-hop".to_string(), 10)]);

        // amount 4, fee 10: nothing is left to forward, so this hop
        // refuses rather than carrying a packet it is not paid for.
        let response = first_hop
            .handle_prepare(prepare_with_amount("g.example.app", 4))
            .await;

        match response {
            PacketResponse::Reject(reject) => {
                // RFC 0027's `R01`, "too little to forward (zero or less)",
                // and not `F03`: the class letter is itself the sender's
                // instruction, and this one is relative -- send more --
                // rather than final (ADR 0051, ADR 0057 as corrected).
                assert_eq!(reject.code.as_str(), "R01");
                assert!(reject.message.contains("10"), "{}", reject.message);
                assert!(reject.message.contains('4'), "{}", reject.message);
            }
            other => panic!("expected a reject, got {other:?}"),
        }
        // Never forwarded a smaller amount hoping the far end would cope.
        assert!(second_hop_app_client.deliveries().is_empty());
    }

    // -- ADR 0042's cap: the largest amount this connector will forward to
    // one peer in a SINGLE packet. Every case here builds the same two-hop
    // rig the fee tests above use, so the only thing under test is which
    // amounts get past the cap.

    /// A first hop forwarding `g.example.app` to `second-hop`, whose own
    /// app answers -- the rig every cap test shares. Returns the first hop
    /// alongside the second hop's app client, which is the evidence of
    /// whether a packet was actually carried: a refused packet leaves it
    /// empty.
    fn capped_hop_pair(fee: u64, caps: Vec<(String, u64)>) -> (Connector, Arc<FakeAppClient>) {
        let second_hop_route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let second_hop_app_client = Arc::new(FakeAppClient::new());
        second_hop_app_client.respond(
            second_hop_route.handler_url(),
            answered(b"delivered by the second hop"),
        );
        let second_hop = Arc::new(
            Connector::new(
                vec![second_hop_route],
                vec![],
                second_hop_app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(identity_signer()),
        );
        let mut peer_transport = InProcessPeerTransport::new();
        peer_transport.add_peer("second-hop", second_hop);
        let first_hop = covering(
            Connector::new(
                vec![],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                Arc::new(FakeAppClient::new()),
                Arc::new(peer_transport),
                test_clock(),
            )
            .with_peer_fees([("second-hop".to_string(), fee)])
            .with_peer_packet_caps(caps),
            "second-hop",
        );
        (first_hop, second_hop_app_client)
    }

    #[tokio::test]
    async fn a_packet_at_exactly_the_cap_is_forwarded() {
        let (first_hop, second_hop_app_client) =
            capped_hop_pair(0, vec![("second-hop".to_string(), 100)]);

        let response = first_hop
            .handle_prepare(prepare_with_amount("g.example.app", 100))
            .await;

        assert!(
            matches!(response, PacketResponse::Fulfill(_)),
            "{response:?}"
        );
        assert_eq!(second_hop_app_client.deliveries().len(), 1);
    }

    #[tokio::test]
    async fn a_packet_one_unit_over_the_cap_is_refused_with_t04() {
        let (first_hop, second_hop_app_client) =
            capped_hop_pair(0, vec![("second-hop".to_string(), 100)]);

        let response = first_hop
            .handle_prepare(prepare_with_amount("g.example.app", 101))
            .await;

        match response {
            PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "T04"),
            other => panic!("expected a reject, got {other:?}"),
        }
        // Never carried, and never split into two packets that each fit:
        // the far end saw nothing at all.
        assert!(second_hop_app_client.deliveries().is_empty());
    }

    /// A refusal an operator cannot act on is a refusal they will
    /// mis-diagnose, so the message names the peering, the cap in force and
    /// the amount that exceeded it.
    #[tokio::test]
    async fn the_cap_refusal_names_the_peer_the_cap_and_the_offending_amount() {
        let (first_hop, _) = capped_hop_pair(0, vec![("second-hop".to_string(), 250)]);

        let response = first_hop
            .handle_prepare(prepare_with_amount("g.example.app", 900))
            .await;

        match response {
            PacketResponse::Reject(reject) => {
                assert!(reject.message.contains("second-hop"), "{}", reject.message);
                assert!(reject.message.contains("250"), "{}", reject.message);
                assert!(reject.message.contains("900"), "{}", reject.message);
            }
            other => panic!("expected a reject, got {other:?}"),
        }
    }

    /// ADR 0042: the cap has a default "so an operator who never configures
    /// one is still bounded". A connector nobody gave a cap to still holds
    /// this peering to `DEFAULT_MAX_PACKET_AMOUNT` -- forwarding a packet
    /// at it, refusing the one above it.
    #[tokio::test]
    async fn a_peer_with_no_configured_cap_is_still_bounded_by_the_default() {
        let (first_hop, second_hop_app_client) = capped_hop_pair(0, vec![]);

        let at_the_default = first_hop
            .handle_prepare(prepare_with_amount(
                "g.example.app",
                DEFAULT_MAX_PACKET_AMOUNT,
            ))
            .await;
        assert!(
            matches!(at_the_default, PacketResponse::Fulfill(_)),
            "{at_the_default:?}"
        );

        let over_the_default = first_hop
            .handle_prepare(prepare_with_amount(
                "g.example.app",
                DEFAULT_MAX_PACKET_AMOUNT + 1,
            ))
            .await;
        match over_the_default {
            PacketResponse::Reject(reject) => {
                assert_eq!(reject.code.as_str(), "T04");
                assert!(
                    reject
                        .message
                        .contains(&DEFAULT_MAX_PACKET_AMOUNT.to_string()),
                    "{}",
                    reject.message
                );
            }
            other => panic!("expected a reject, got {other:?}"),
        }

        // Only the first packet was ever carried.
        assert_eq!(second_hop_app_client.deliveries().len(), 1);
    }

    /// The cap bounds what this connector hands the peer, which is the
    /// amount left after its own fee -- an arriving 105 with a fee of 10
    /// puts 95 on the wire and clears a cap of 100.
    #[tokio::test]
    async fn the_cap_is_measured_against_the_amount_forwarded_not_the_amount_that_arrived() {
        let (first_hop, second_hop_app_client) =
            capped_hop_pair(10, vec![("second-hop".to_string(), 100)]);

        let response = first_hop
            .handle_prepare(prepare_with_amount("g.example.app", 105))
            .await;

        assert!(
            matches!(response, PacketResponse::Fulfill(_)),
            "{response:?}"
        );
        assert_eq!(second_hop_app_client.deliveries().len(), 1);
    }

    /// The cap bounds ONE packet, never a running total (ADR 0042; ADR 0033
    /// retired the exposure ceiling and it is not coming back). Three
    /// packets at the cap are three carried packets, not two and a refusal:
    /// each carries its own claim, so nothing accumulates between them for
    /// a cap to bound.
    #[tokio::test]
    async fn the_cap_bounds_each_packet_rather_than_a_running_total() {
        let (first_hop, second_hop_app_client) =
            capped_hop_pair(0, vec![("second-hop".to_string(), 100)]);

        for _ in 0..3 {
            let response = first_hop
                .handle_prepare(prepare_with_amount("g.example.app", 100))
                .await;
            assert!(
                matches!(response, PacketResponse::Fulfill(_)),
                "{response:?}"
            );
        }

        assert_eq!(second_hop_app_client.deliveries().len(), 3);
    }

    /// The cap is per peering: a tight one on one peer says nothing about
    /// another, which keeps its own (here, the default).
    #[tokio::test]
    async fn a_cap_on_one_peer_does_not_bind_another() {
        let (first_hop, second_hop_app_client) =
            capped_hop_pair(0, vec![("some-other-peer".to_string(), 1)]);

        let response = first_hop
            .handle_prepare(prepare_with_amount("g.example.app", 5_000))
            .await;

        assert!(
            matches!(response, PacketResponse::Fulfill(_)),
            "{response:?}"
        );
        assert_eq!(second_hop_app_client.deliveries().len(), 1);
    }

    // -- ADR 0071's converting arm (issue #1295): a forward whose two
    // peerings hold different tokens crosses a DENOMINATION BOUNDARY at a
    // rate this node declared, or is refused. Every case below builds the
    // same rig -- one hop, an upstream peering the packet arrives over, a
    // downstream peering it leaves to, and a `CarriesAndRemembers` that
    // keeps whatever actually went on the wire -- so the only thing under
    // test is the arithmetic and the refusals.

    /// USDC on Base: 6 decimals, and the numeraire throughout, as in ADR
    /// 0071's own examples.
    const USDC: &str = "evm:0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
    /// ANYONE on Base: 18 decimals against USDC's 6, which is where the
    /// `10^12` in ADR 0071's motivating error comes from.
    const ANYONE: &str = "evm:0x9ff58f4ffb29fa2266ab25e75e2a8b3503311656";
    /// USDC on Solana: the SAME asset as [`USDC`] on another chain, which
    /// `AssetId` nevertheless distinguishes -- today's `mixed-chain`
    /// crossing, and the pair ADR 0071 decision 2 talks about.
    const USDC_SOLANA: &str = "solana:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    /// One 6-decimals USDC base unit in 18-decimals ANYONE base units at
    /// four ANYONE to the USDC -- the decimals gap (`10^12`) and the market
    /// price (4) folded into one ratio, which is the only place ADR 0071
    /// decision 4 lets a scale difference live.
    const USDC_TO_ANYONE: u64 = 4_000_000_000_000;

    /// The peering a converting packet arrives over. Named rather than
    /// derived: only a peer arrival has one, and that is what
    /// `handle_peer_prepare` is handed by both carriages.
    const UPSTREAM: &str = "first-hop";

    fn asset(text: &str) -> AssetId {
        text.parse::<AssetId>().expect("a declared asset")
    }

    fn rate(numerator: u64, denominator: u64) -> Rate {
        Rate::new(numerator, denominator).expect("a rate with two non-zero halves")
    }

    /// Node defaults wide enough that no guard is what a test is about:
    /// deal at the mid, a two-minute ttl, and a `max_move` nothing here
    /// comes near.
    fn rate_guards() -> Guards {
        Guards::new(
            Spread::none(),
            Ttl::new(Duration::seconds(120)).expect("a positive ttl"),
            MaxMove::fraction(50, 100).expect("a max_move"),
        )
    }

    fn empty_table() -> SharedRateTable {
        SharedRateTable::new(RateTable::new(asset(USDC), rate_guards()))
    }

    /// A table holding one **declared** row -- a static `[[rates]]` row,
    /// which never goes stale because it is a declaration rather than an
    /// observation (ADR 0009).
    fn declaring(from: &str, to: &str, numerator: u64, denominator: u64) -> SharedRateTable {
        let table = empty_table();
        table.write(|table| table.declare(asset(from), asset(to), rate(numerator, denominator)));
        table
    }

    /// A hop that forwards `g.example.app` to `second-hop`, holding the
    /// tokens `peerings` says it holds and dealing at whatever `table`
    /// holds.
    ///
    /// `table` of `None` is a node that declares no rate at all, which is
    /// not the same thing as a node that declares no tokens: the first
    /// resolves boundaries and refuses them, the second resolves none.
    /// Both are exercised below.
    fn dealing_hop(
        peerings: &[(&str, &str)],
        fee: u64,
        cap: u64,
        table: Option<SharedRateTable>,
    ) -> (Connector, Arc<CarriesAndRemembers>, Arc<TestClock>) {
        dealing_hop_quoting(peerings, fee, cap, table, 0)
    }

    /// [`dealing_hop`] whose downstream peer answers with a running cost of
    /// `quoted` already on it -- what everything beyond this hop charges,
    /// denominated in the OUTGOING leg's unit because the outgoing leg is
    /// the one it travelled up (ADR 0011, ADR 0071 decision 7).
    fn dealing_hop_quoting(
        peerings: &[(&str, &str)],
        fee: u64,
        cap: u64,
        table: Option<SharedRateTable>,
        quoted: u64,
    ) -> (Connector, Arc<CarriesAndRemembers>, Arc<TestClock>) {
        let peer = Arc::new(CarriesAndRemembers::quoting(quoted));
        let clock = test_clock();
        let hop = dealing_hop_over(peer.clone(), peerings, fee, cap, table, clock.clone());
        (hop, peer, clock)
    }

    /// The rig both of the above build on, over whatever peer transport the
    /// case needs: a downstream that quotes a cost, one that takes time to
    /// answer, or another `Connector` entirely.
    fn dealing_hop_over(
        peer: Arc<dyn PeerTransport>,
        peerings: &[(&str, &str)],
        fee: u64,
        cap: u64,
        table: Option<SharedRateTable>,
        clock: Arc<TestClock>,
    ) -> Connector {
        let mut connector = covering(
            Connector::new(
                vec![],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                Arc::new(FakeAppClient::new()),
                peer,
                clock,
            )
            .with_peer_fees([("second-hop".to_string(), fee)])
            .with_peer_packet_caps([("second-hop".to_string(), cap)])
            .with_peering_assets(
                peerings
                    .iter()
                    .map(|(peer_id, token)| ((*peer_id).to_string(), asset(token)))
                    .collect(),
            ),
            "second-hop",
        );
        if let Some(table) = table {
            connector = connector.with_rate_table(table);
        }
        connector
    }

    /// Send `amount` into `hop` as a PEER arrival over [`UPSTREAM`] -- the
    /// only arrival that names an incoming peering, and therefore the only
    /// one that can cross a boundary.
    async fn arrives_from_upstream(hop: &Connector, amount: u64) -> PacketResponse {
        let (response, _ack) = hop
            .handle_peer_prepare(
                Some(UPSTREAM),
                prepare_with_amount("g.example.app", amount),
                None,
            )
            .await;
        response
    }

    fn refusal(response: PacketResponse) -> Reject {
        match response {
            PacketResponse::Reject(reject) => reject,
            other => panic!("expected a reject, got {other:?}"),
        }
    }

    /// Decision 1, the whole of it: `floor(amount * rate) - fee`. One USDC
    /// arrives, four ANYONE less this hop's fee leaves, and the two
    /// integers differ by `10^12` times a price -- which is exactly the
    /// error an unconverted pass-through would have made.
    #[tokio::test]
    async fn a_forward_across_a_boundary_converts_at_the_declared_rate() {
        let (hop, peer, _clock) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            // 0.001 ANYONE, in the OUTGOING leg's unit.
            1_000_000_000_000_000,
            u64::MAX,
            Some(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
        );

        // One USDC.
        arrives_from_upstream(&hop, 1_000_000).await;

        let carried = peer.carried();
        assert_eq!(carried.len(), 1, "the packet should have been forwarded");
        assert_eq!(carried[0].amount, 3_999_000_000_000_000_000);
    }

    /// Decision 1's other half, and ADR 0061's clause this record amends:
    /// the fee is the outgoing peering's, denominated in the outgoing leg's
    /// unit, so it is subtracted AFTER the conversion and is never itself
    /// converted. Two runs of the same crossing differing only in the fee
    /// differ in the forwarded figure by exactly that fee.
    #[tokio::test]
    async fn the_fee_a_crossing_takes_is_the_outgoing_peerings_in_the_outgoing_unit() {
        let fee = 1_000_000_000_000_000;
        let (free, free_peer, _) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            0,
            u64::MAX,
            Some(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
        );
        let (charging, charging_peer, _) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            fee,
            u64::MAX,
            Some(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
        );

        arrives_from_upstream(&free, 1_000_000).await;
        arrives_from_upstream(&charging, 1_000_000).await;

        assert_eq!(
            free_peer.carried()[0].amount - charging_peer.carried()[0].amount,
            fee,
            "the fee is taken in the outgoing unit, whole and unconverted"
        );
    }

    /// Decision 1: the covering claim `cover_forward` mints is for the
    /// CONVERTED figure, on the outgoing channel. Claims needed no change
    /// for this -- they are denominated by channel identity alone -- which
    /// is exactly why the figure on the claim has to be the outgoing one:
    /// nothing else on it says what unit it is in.
    #[tokio::test]
    async fn the_covering_claim_is_minted_for_the_converted_outgoing_figure() {
        let (hop, peer, _clock) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            1_000_000_000_000_000,
            u64::MAX,
            Some(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
        );

        arrives_from_upstream(&hop, 1_000_000).await;

        let claim = peer.covered_by()[0]
            .clone()
            .expect("ADR 0042: no PREPARE leaves this connector uncovered");
        assert_eq!(claim.cumulative_amount, 3_999_000_000_000_000_000);
        assert_eq!(
            claim.cumulative_amount,
            peer.carried()[0].amount,
            "the claim covers what went out, not what came in"
        );
    }

    /// The cap keeps being checked against the outgoing, post-conversion,
    /// post-fee amount -- which was always the right leg, since the cap is
    /// configured on the outgoing peering. The `T04` message still names
    /// both numbers (ADR 0049: discovery of a cap is this refusal and
    /// nothing else).
    #[tokio::test]
    async fn the_cap_is_measured_against_the_converted_amount_and_still_names_both_numbers() {
        let cap = 3_000_000_000_000_000_000;
        let (hop, peer, _clock) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            0,
            cap,
            Some(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
        );

        // One USDC converts to four ANYONE, which is over a cap of three --
        // while the amount that ARRIVED is far under it. Reading the cap
        // against the incoming figure would have carried this packet.
        let reject = refusal(arrives_from_upstream(&hop, 1_000_000).await);

        assert_eq!(reject.code.as_str(), "T04");
        assert!(
            reject.message.contains(&cap.to_string()),
            "{}",
            reject.message
        );
        assert!(
            reject.message.contains("4000000000000000000"),
            "{}",
            reject.message
        );
        assert!(peer.carried().is_empty(), "nothing may go out over the cap");
    }

    /// Decision 2, the record's whole reason for existing: an undeclared
    /// pair is REFUSED, never passed through at an implied 1:1. `F02`,
    /// final -- RFC 0027's "there was no way to forward the payment" and
    /// ADR 0051's "this path is wrong; find another" -- because a
    /// declaration is config and config is immutable for the process
    /// lifetime (ADR 0009), so retrying here cannot help.
    #[tokio::test]
    async fn a_crossing_with_no_declared_rate_is_refused_rather_than_forwarded() {
        let (hop, peer, _clock) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            0,
            u64::MAX,
            // A table, and no row for this pair.
            Some(empty_table()),
        );

        let reject = refusal(arrives_from_upstream(&hop, 1_000_000).await);

        assert_eq!(reject.code.as_str(), "F02");
        assert!(reject.message.contains(USDC), "{}", reject.message);
        assert!(reject.message.contains(ANYONE), "{}", reject.message);
        assert!(
            peer.carried().is_empty(),
            "an unconverted pass-through here is wrong by 10^12 and must not happen"
        );
    }

    /// A node that declares tokens but holds no table at all answers the
    /// same way. "No row" and "no table" are one situation -- the operator
    /// did not say so -- and decision 2's absence rule makes no
    /// distinction.
    #[tokio::test]
    async fn a_crossing_on_a_node_with_no_rate_table_is_refused_identically() {
        let (hop, peer, _clock) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            0,
            u64::MAX,
            None,
        );

        let reject = refusal(arrives_from_upstream(&hop, 1_000_000).await);

        assert_eq!(reject.code.as_str(), "F02");
        assert!(peer.carried().is_empty());
    }

    /// Decision 5's ttl: a rate not refreshed within it is dead, and the
    /// pair goes down loudly rather than dealing on the last price before
    /// the outage. The code differs from the undeclared one **on purpose**
    /// -- `T`, retry, against `F`, re-route -- because those are opposite
    /// instructions and a sender that cannot tell them apart takes the
    /// wrong one.
    #[tokio::test]
    async fn a_crossing_whose_rate_has_gone_stale_is_refused_with_a_different_code() {
        let table = empty_table();
        let (hop, peer, clock) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            0,
            u64::MAX,
            Some(table.clone()),
        );
        let observed_at = clock.now();
        table.write(|table| {
            table.refresh(
                asset(USDC),
                asset(ANYONE),
                rate(USDC_TO_ANYONE, 1),
                observed_at,
            )
        });

        // Inside the ttl this crossing deals.
        arrives_from_upstream(&hop, 1_000_000).await;
        assert_eq!(peer.carried().len(), 1);

        // Past it, the same packet is refused.
        clock.advance(Duration::seconds(121));
        let reject = refusal(arrives_from_upstream(&hop, 1_000_000).await);

        assert_eq!(
            reject.code.as_str(),
            "T00",
            "a stale rate is worth retrying, and the class letter says so"
        );
        assert!(
            reject.message.contains(&observed_at.to_rfc3339()),
            "the refusal must say when the price it will not deal on was taken: {}",
            reject.message
        );
        assert_eq!(
            peer.carried().len(),
            1,
            "the stale crossing carried nothing"
        );
    }

    /// Decision 2's same-asset sentence, in the case it actually names: a
    /// node that declares no `[[tokens]]` resolves no peering, so today's
    /// `mixed-chain` USDC-Base-to-USDC-Solana crossing is not a conversion
    /// and runs the one subtraction it ran before this record existed.
    #[tokio::test]
    async fn a_node_that_declares_no_tokens_forwards_exactly_as_before() {
        let (hop, peer, _clock) = dealing_hop(&[], 10, u64::MAX, None);

        arrives_from_upstream(&hop, 1_000).await;

        assert_eq!(peer.carried()[0].amount, 990);
    }

    /// Two peerings holding ONE token are no boundary even on a dealing
    /// node: the flat fee and nothing else, with no rate consulted and none
    /// needed.
    #[tokio::test]
    async fn two_peerings_holding_one_token_take_the_flat_fee_and_nothing_else() {
        let (hop, peer, _clock) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", USDC)],
            10,
            u64::MAX,
            // Empty on purpose: a same-token forward must not need a row.
            Some(empty_table()),
        );

        arrives_from_upstream(&hop, 1_000).await;

        assert_eq!(peer.carried()[0].amount, 990);
    }

    /// The same asset on two chains is two `AssetId`s, so on a node that
    /// DOES declare tokens it is an ordered pair like any other and the
    /// absence rule holds for it: the operator who wants it to forward at
    /// par says so with a par row, and gets par. Declared par is not
    /// implicit 1:1 -- which is why `Rate::new(1, 1)` is a value a config
    /// can express and `Rate::ONE` is not a constant this code can reach
    /// for.
    #[tokio::test]
    async fn a_dealing_node_crosses_one_asset_on_two_chains_at_the_par_rate_it_declared() {
        let (declared, peer, _clock) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", USDC_SOLANA)],
            10,
            u64::MAX,
            Some(declaring(USDC, USDC_SOLANA, 1, 1)),
        );

        arrives_from_upstream(&declared, 1_000).await;

        assert_eq!(peer.carried()[0].amount, 990);

        // And without the row, the same pair refuses -- the distinction the
        // record draws, made executable.
        let (undeclared, undeclared_peer, _clock) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", USDC_SOLANA)],
            10,
            u64::MAX,
            Some(empty_table()),
        );

        let reject = refusal(arrives_from_upstream(&undeclared, 1_000).await);
        assert_eq!(reject.code.as_str(), "F02");
        assert!(undeclared_peer.carried().is_empty());
    }

    /// `R01`'s question asked where decision 1 puts it: AFTER the
    /// conversion, in the outgoing unit. Ten incoming units convert to
    /// three outgoing ones and a fee of four eats them, so nothing would be
    /// forwarded -- and the sender's move is "send more", in its OWN unit,
    /// which the message states by un-converting the threshold back across
    /// the boundary.
    #[tokio::test]
    async fn a_crossing_whose_fee_exceeds_the_converted_amount_is_refused_r01() {
        let (hop, peer, _clock) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            4,
            u64::MAX,
            Some(declaring(USDC, ANYONE, 1, 3)),
        );

        let reject = refusal(arrives_from_upstream(&hop, 10).await);

        assert_eq!(reject.code.as_str(), "R01");
        // `ceil((0 + 4) / (1/3))` = 12 incoming units, the smallest arrival
        // that leaves anything at all.
        assert!(reject.message.contains("12"), "{}", reject.message);
        assert!(peer.carried().is_empty());
    }

    /// The other `None` [`amount_after_rate_and_fee`] answers, and the
    /// reason it is not reported as `R01`: a converted amount past the
    /// outgoing leg's `u64` is ADR 0071's real ceiling, and the sender must
    /// send **less**. Telling it to send more would be the exact opposite
    /// instruction.
    #[tokio::test]
    async fn a_crossing_the_outgoing_leg_cannot_hold_is_refused_t04_and_not_r01() {
        let cap = 1_000_000;
        let (hop, peer, _clock) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            0,
            cap,
            Some(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
        );

        let reject = refusal(arrives_from_upstream(&hop, u64::MAX).await);

        assert_eq!(
            reject.code.as_str(),
            "T04",
            "an amount the outgoing leg cannot hold is 'send less', which is the cap's \
             own situation and the cap's own code"
        );
        assert!(
            reject.message.contains(&cap.to_string()),
            "{}",
            reject.message
        );
        assert!(peer.carried().is_empty());
    }

    /// The incoming half of the boundary comes from the leg the packet
    /// arrived over, and an arrival that names none crosses nothing. That
    /// is an operator write, or any caller of `handle_prepare` itself: no
    /// channel behind it, so nothing to denominate it by. It must forward
    /// rather than refuse, or every channel-less arrival on a dealing node
    /// would go dark. A client-edge arrival is NOT one of these -- it names
    /// its channel, and issue #1301's block below is what it does.
    #[tokio::test]
    async fn an_arrival_that_names_no_peering_crosses_no_boundary() {
        let (hop, peer, _clock) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            10,
            u64::MAX,
            Some(empty_table()),
        );

        // `handle_prepare`, not `handle_peer_prepare`: no peering named.
        hop.handle_prepare(prepare_with_amount("g.example.app", 1_000))
            .await;

        assert_eq!(peer.carried()[0].amount, 990);
    }

    // -- ADR 0071's converting arm travelling the other way (decision 7,
    // extending ADR 0011, issue #1296): a reject crossing the same boundary
    // upstream adds this hop's fee in the outgoing unit and un-converts the
    // running total into the incoming leg's, so what a prober finally reads
    // is one number in its own unit. The wire is untouched -- nothing below
    // reads or writes an asset field, because the unit of `accumulated_cost`
    // is the unit of the leg it is on, exactly as `amount`'s always was.

    /// Decision 7's own arithmetic, in the case that isolates it: nothing
    /// beyond this hop charges anything, so the whole of the reported cost
    /// is this hop's own fee -- stated in the outgoing unit where ADR 0061
    /// attaches it, and un-converted into the unit the prober counts in.
    #[tokio::test]
    async fn a_reject_crossing_a_boundary_un_converts_its_cost_into_the_incoming_unit() {
        // 0.001 ANYONE, which at four ANYONE to the USDC is 250 USDC base
        // units -- and 10^12 times that as a raw integer, which is the
        // number an un-converted pass-through would have reported.
        let fee = 1_000_000_000_000_000;
        let (hop, peer, _clock) = dealing_hop(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            fee,
            u64::MAX,
            Some(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
        );

        let reject = refusal(arrives_from_upstream(&hop, 1_000_000).await);

        assert_eq!(peer.carried().len(), 1, "the packet did reach the peer");
        assert_eq!(reject.accumulated_cost, 250);
        assert_ne!(
            reject.accumulated_cost, fee,
            "reporting the outgoing figure unconverted is the 10^12 error this record exists \
             to make impossible, in the one direction where it understates"
        );
    }

    /// The order decision 7 fixes: the fee goes on **first**, in the
    /// outgoing unit, because the cost coming up from downstream is already
    /// in that unit -- and only the total is un-converted. Converting the
    /// fee separately and adding it afterwards would round twice.
    #[tokio::test]
    async fn a_reject_adds_this_hops_fee_in_the_outgoing_unit_before_un_converting() {
        // Four ANYONE -- one USDC's worth -- charged by everything beyond
        // this hop, plus this hop's own 0.001 ANYONE.
        let quoted = 4_000_000_000_000_000_000;
        let (hop, _peer, _clock) = dealing_hop_quoting(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            1_000_000_000_000_000,
            u64::MAX,
            Some(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
            quoted,
        );

        let reject = refusal(arrives_from_upstream(&hop, 1_000_000).await);

        // `ceil((4e18 + 1e15) / 4e12)`: one USDC for the far end, 250 base
        // units for this hop.
        assert_eq!(reject.accumulated_cost, 1_000_250);
    }

    /// The rounding, and which way it leans. A cost that does not divide
    /// evenly by the rate is rounded **up**, against this connector: a
    /// prober quoted the exact figure clears, and one quoted a base unit
    /// less would not.
    #[tokio::test]
    async fn un_converting_a_cost_rounds_up_against_this_connector() {
        // Three outgoing units per incoming one, and a far end charging
        // one: a third of an incoming unit, which is reported as one.
        let (hop, _peer, _clock) = dealing_hop_quoting(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            0,
            u64::MAX,
            Some(declaring(USDC, ANYONE, 3, 1)),
            1,
        );

        let reject = refusal(arrives_from_upstream(&hop, 1_000).await);

        assert_eq!(
            reject.accumulated_cost, 1,
            "rounding down would quote a cost of zero for a hop that charges something"
        );
    }

    /// The other half of AC4, and the one every node shipping today lives
    /// on: off a boundary the running total gains this hop's fee and
    /// nothing else, whether the node declares no tokens at all or declares
    /// two peerings holding one.
    #[tokio::test]
    async fn a_reject_on_a_single_denomination_path_carries_exactly_what_it_did_before() {
        let (silent, _peer, _clock) = dealing_hop_quoting(&[], 7, u64::MAX, None, 25);
        let (same_token, _peer, _clock) = dealing_hop_quoting(
            &[(UPSTREAM, USDC), ("second-hop", USDC)],
            7,
            u64::MAX,
            Some(empty_table()),
            25,
        );

        assert_eq!(
            refusal(arrives_from_upstream(&silent, 1_000).await).accumulated_cost,
            32
        );
        assert_eq!(
            refusal(arrives_from_upstream(&same_token, 1_000).await).accumulated_cost,
            32,
            "two peerings holding one token are no boundary, and consult no rate"
        );
    }

    /// ADR 0011's probe, across one boundary: one number, in the prober's
    /// own unit, and a packet carrying exactly that number clears the hop.
    /// The second half is the property decision 7's up-rounding exists for
    /// -- a probed cost that could be a base unit short would make every
    /// probed price a coin toss.
    #[tokio::test]
    async fn a_probe_across_one_boundary_answers_a_cost_that_pays_for_the_packet() {
        let quoted = 4_000_000_000_000_000_000;
        let (hop, peer, _clock) = dealing_hop_quoting(
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            1_000_000_000_000_000,
            u64::MAX,
            Some(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
            quoted,
        );

        // The probe: an ordinary packet this path was always going to
        // reject (ADR 0011 -- there is no probe packet type).
        let probed = refusal(arrives_from_upstream(&hop, 1_000_000).await).accumulated_cost;

        // Pay it, in the prober's own unit, with no conversion done by the
        // prober and no unit named anywhere on the wire.
        arrives_from_upstream(&hop, probed).await;
        assert!(
            peer.carried()[1].amount >= quoted,
            "a packet paying the probed cost must clear what lies beyond this hop: {} against \
             {quoted}",
            peer.carried()[1].amount
        );

        // And it is not merely generous: a base unit less does not clear.
        arrives_from_upstream(&hop, probed - 1).await;
        assert!(peer.carried()[2].amount < quoted);
    }

    /// The same across **two** boundaries, which is the criterion the
    /// single-hop case cannot reach: USDC in at the prober's edge, ANYONE
    /// across the middle, USDC on Solana at the far end, and one number
    /// coming back in USDC. Each hop un-converts only its own crossing;
    /// nothing accumulates a unit, and nothing on the wire says which one.
    #[tokio::test]
    async fn a_probe_across_two_boundaries_still_answers_in_the_probers_own_unit() {
        // What the far end charges, in ITS unit.
        let quoted = 1_000_000;
        let far = Arc::new(CarriesAndRemembers::quoting(quoted));
        let middle = Arc::new(dealing_hop_over(
            far.clone(),
            &[(UPSTREAM, ANYONE), ("second-hop", USDC_SOLANA)],
            // 0.001 USDC on Solana.
            1_000,
            u64::MAX,
            Some(declaring(ANYONE, USDC_SOLANA, 1, USDC_TO_ANYONE)),
            test_clock(),
        ));
        let entry = dealing_hop_over(
            Arc::new(HandsOnNaming {
                downstream: middle,
                arrives_as: UPSTREAM.to_string(),
            }),
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            // 0.001 ANYONE.
            1_000_000_000_000_000,
            u64::MAX,
            Some(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
            test_clock(),
        );

        let probed = refusal(arrives_from_upstream(&entry, 1_000_000).await).accumulated_cost;

        // One USDC for the far end, 1000 base units for the middle hop's
        // 0.001 USDC-on-Solana fee, 250 for the entry hop's 0.001 ANYONE --
        // three fees in three units, summed in one.
        assert_eq!(probed, 1_001_250);

        arrives_from_upstream(&entry, probed).await;
        assert!(
            far.carried()[1].amount >= quoted,
            "a packet paying a cost probed across two boundaries must clear both: {} against \
             {quoted}",
            far.carried()[1].amount
        );
    }

    /// The question a reject cannot dodge: the pair may have aged out of
    /// its ttl while the packet was downstream, and a reject already
    /// travelling cannot be refused. It reports a cost no packet can pay
    /// rather than one in the wrong unit -- which is also the truth, since
    /// a pair with no live rate refuses every forward across it until a
    /// fresh one lands, and is the saturation `cost_before_rate_and_fee`
    /// itself makes for a cost it cannot state.
    #[tokio::test]
    async fn a_reject_whose_rate_died_in_flight_overstates_rather_than_understating() {
        let clock = test_clock();
        let table = empty_table();
        table.write(|table| {
            table.refresh(
                asset(USDC),
                asset(ANYONE),
                rate(USDC_TO_ANYONE, 1),
                clock.now(),
            )
        });
        let hop = dealing_hop_over(
            Arc::new(AnswersLater {
                clock: clock.clone(),
                // Longer than the two-minute ttl `rate_guards` declares:
                // the forward is dealt on a live rate and the reject comes
                // home to a dead one.
                elapsed: Duration::seconds(121),
            }),
            &[(UPSTREAM, USDC), ("second-hop", ANYONE)],
            1_000_000_000_000_000,
            u64::MAX,
            Some(table),
            clock,
        );

        let reject = refusal(arrives_from_upstream(&hop, 1_000_000).await);

        assert_eq!(
            reject.accumulated_cost,
            u64::MAX,
            "quoting the last observed price would be dealing on a dead one, and reporting the \
             outgoing figure unconverted would understate by the whole decimals gap"
        );
    }

    /// A peer that hands the packet to another `Connector` in this process
    /// **and names the peering it arrives over** -- which
    /// [`InProcessPeerTransport`] deliberately does not, because a link
    /// stands for a wire rather than an identity and the id a node knows
    /// its upstream by is not the id its upstream knows itself by.
    ///
    /// A fake and not a mock (ADR 0007): the answer is whatever the
    /// downstream connector genuinely decided, and it asserts nothing about
    /// having been called. Naming the arrival is what a real carriage does
    /// -- both `POST /packets` and BTP authenticate the peer before
    /// `handle_peer_prepare` is reached -- so this is the more faithful of
    /// the two stand-ins rather than a privileged one.
    struct HandsOnNaming {
        downstream: Arc<Connector>,
        arrives_as: String,
    }

    #[async_trait]
    impl PeerTransport for HandsOnNaming {
        async fn forward(
            &self,
            _peer_id: &str,
            prepare: Prepare,
            claim: Option<WireClaim>,
        ) -> PeerForward {
            let (response, ack) = self
                .downstream
                .handle_peer_prepare(Some(&self.arrives_as), prepare, claim)
                .await;
            PeerForward::answered(response, ack)
        }

        async fn flush(&self, _peer_id: &str, _claim: WireClaim) -> ClaimAckOutcome {
            ClaimAckOutcome::NotSent
        }
    }

    /// A peer that takes `elapsed` to answer -- the time a packet really
    /// spends downstream, during which a rate this node dealt the forward
    /// on can age out of its ttl. A fake: it answers a reject of its own,
    /// `reached_peer` true, and the only unusual thing about it is that the
    /// clock has moved by the time it does.
    struct AnswersLater {
        clock: Arc<TestClock>,
        elapsed: Duration,
    }

    #[async_trait]
    impl PeerTransport for AnswersLater {
        async fn forward(
            &self,
            _peer_id: &str,
            _prepare: Prepare,
            _claim: Option<WireClaim>,
        ) -> PeerForward {
            self.clock.advance(self.elapsed);
            PeerForward::answered(
                PacketResponse::Reject(Reject {
                    code: RejectCode::f02_unreachable(),
                    triggered_by: "g.peer".to_string(),
                    message: "carried, and the far end had nowhere to put it".to_string(),
                    data: Vec::new(),
                    accumulated_cost: 0,
                }),
                ClaimAckOutcome::NotSent,
            )
        }

        async fn flush(&self, _peer_id: &str, _claim: WireClaim) -> ClaimAckOutcome {
            ClaimAckOutcome::NotSent
        }
    }

    // -- ADR 0071 decision 1 at the CLIENT EDGE (issue #1301): a buyer's
    // own packet is denominated by the channel its covering claim was
    // written against, exactly as a peer's is by the peering's, so a
    // forward out of one crosses a denomination boundary on identical
    // terms. Everything below drives `handle_prepare_with_client_channel`
    // -- the entry point both carriages use once a claim has cleared the
    // gate -- against the same downstream rig the peer cases use, so the
    // only difference under test is which door the packet came in.

    /// A channel a `[[client_channels]]` row declares: the buyer this
    /// operator has heard of.
    const DECLARED_CHANNEL: &str =
        "evm:0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    /// A channel resolved from chain and never declared anywhere (ADR
    /// 0052, issue #502): the buyer this operator has NOT heard of, and
    /// the one a rows-only resolution would leave unresolved.
    const DISCOVERED_CHANNEL: &str =
        "evm:0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// [`dealing_hop`] paid at its CLIENT EDGE instead of by a peer:
    /// `client_chains` is what each chain's `[settlement.<chain>]` table
    /// says a channel on it holds, which is the whole of how a client
    /// arrival is denominated.
    fn client_paying_hop(
        client_chains: &[(SettlementChain, &str)],
        peerings: &[(&str, &str)],
        fee: u64,
        cap: u64,
        table: Option<SharedRateTable>,
    ) -> (Connector, Arc<CarriesAndRemembers>, Arc<TestClock>) {
        client_paying_hop_quoting(client_chains, peerings, fee, cap, table, 0)
    }

    /// [`client_paying_hop`] whose downstream answers with a running cost
    /// already on it, in the OUTGOING leg's unit -- what a probe sent by a
    /// buyer meets beyond this hop.
    fn client_paying_hop_quoting(
        client_chains: &[(SettlementChain, &str)],
        peerings: &[(&str, &str)],
        fee: u64,
        cap: u64,
        table: Option<SharedRateTable>,
        quoted: u64,
    ) -> (Connector, Arc<CarriesAndRemembers>, Arc<TestClock>) {
        let peer = Arc::new(CarriesAndRemembers::quoting(quoted));
        let clock = test_clock();
        let hop = dealing_hop_over(peer.clone(), peerings, fee, cap, table, clock.clone())
            .with_client_channel_assets(
                client_chains
                    .iter()
                    .map(|(chain, token)| (*chain, asset(token)))
                    .collect(),
            );
        (hop, peer, clock)
    }

    /// Send `amount` into `hop` as a CLIENT arrival over `channel_key` --
    /// the chain-namespaced key of the claim that admitted it, which is
    /// exactly what both carriages hand `handle_prepare_with_client_channel`
    /// once the claim gate has cleared.
    async fn arrives_over_client_channel(
        hop: &Connector,
        channel_key: &str,
        amount: u64,
    ) -> PacketResponse {
        hop.handle_prepare_with_client_channel(
            prepare_with_amount("g.example.app", amount),
            Some(channel_key),
        )
        .await
    }

    /// The acceptance criterion stated directly, below the packet path:
    /// `crossing` answers `Some` for a client arrival on a node whose
    /// client channel and outgoing peering hold different tokens -- and the
    /// pair is ordered, incoming first, because direction is the trade.
    #[test]
    fn a_client_arrival_crossing_into_another_token_is_a_boundary() {
        let (hop, _peer, _clock) = client_paying_hop(
            &[(SettlementChain::Evm, USDC)],
            &[("second-hop", ANYONE)],
            0,
            u64::MAX,
            None,
        );

        assert_eq!(
            hop.crossing(Some(Arrival::ClientChannel(DECLARED_CHANNEL)), "second-hop"),
            Some((&asset(USDC), &asset(ANYONE)))
        );
        // The same client channel against a peering holding the same token
        // is not a boundary, and the flat fee is the whole of that forward.
        let (same, _peer, _clock) = client_paying_hop(
            &[(SettlementChain::Evm, USDC)],
            &[("second-hop", USDC)],
            0,
            u64::MAX,
            None,
        );
        assert_eq!(
            same.crossing(Some(Arrival::ClientChannel(DECLARED_CHANNEL)), "second-hop"),
            None
        );
    }

    /// User stories 1 and 2 of issue #1287, as arithmetic: a buyer pays for
    /// an ANYONE-denominated good with the USDC channel it already holds,
    /// and the crossing happens at THIS hop -- `floor(amount * rate) - fee`,
    /// the fee in the outgoing peering's unit. Before this, the same packet
    /// left at 1_000_000 minus the fee, across a `10^12` scale difference.
    #[tokio::test]
    async fn a_client_arrival_across_a_boundary_converts_at_the_declared_rate() {
        let fee = 1_000_000_000_000_000;
        let (hop, peer, _clock) = client_paying_hop(
            &[(SettlementChain::Evm, USDC)],
            &[("second-hop", ANYONE)],
            fee,
            u64::MAX,
            Some(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
        );

        // One USDC, over the buyer's own channel.
        arrives_over_client_channel(&hop, DECLARED_CHANNEL, 1_000_000).await;

        let carried = peer.carried();
        assert_eq!(carried.len(), 1, "the packet should have been forwarded");
        assert_eq!(carried[0].amount, 3_999_000_000_000_000_000);
        assert!(
            carried[0].amount > 1_000_000,
            "the arriving integer left unconverted is the 10^12 error this record exists to \
             prevent, and before issue #1301 that is exactly what a buyer's packet did"
        );
    }

    /// The hole this issue is really about, and the reason resolution
    /// reads the channel key's CHAIN rather than a `[[client_channels]]`
    /// row: a channel discovered on chain (ADR 0052, issue #502) has no row
    /// to be resolved from, and left unresolved it would take the
    /// unconverted arm across a real boundary. It converts identically to
    /// the declared one, because it is the same chain and therefore the
    /// same token.
    #[tokio::test]
    async fn a_channel_discovered_on_chain_converts_exactly_like_a_declared_one() {
        let (hop, peer, _clock) = client_paying_hop(
            &[(SettlementChain::Evm, USDC)],
            &[("second-hop", ANYONE)],
            0,
            u64::MAX,
            Some(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
        );

        arrives_over_client_channel(&hop, DECLARED_CHANNEL, 1_000_000).await;
        arrives_over_client_channel(&hop, DISCOVERED_CHANNEL, 1_000_000).await;

        let carried = peer.carried();
        assert_eq!(carried.len(), 2);
        assert_eq!(carried[0].amount, 4_000_000_000_000_000_000);
        assert_eq!(
            carried[1].amount, carried[0].amount,
            "a buyer this operator has never heard of is denominated by the chain its channel \
             is on, exactly like one that is declared -- there is no door into the unconverted \
             arm on a dealing node"
        );
    }

    /// Decision 2's absence rule, at the client edge: no declared rate, no
    /// conversion, no forward. `F02`, the same answer #1295 gave the
    /// peer-to-peer arm -- not a new code, because a buyer's next move is
    /// the same move.
    #[tokio::test]
    async fn a_client_arrival_with_no_declared_rate_is_refused_f02() {
        let (hop, peer, _clock) = client_paying_hop(
            &[(SettlementChain::Evm, USDC)],
            &[("second-hop", ANYONE)],
            0,
            u64::MAX,
            Some(empty_table()),
        );

        let reject = refusal(arrives_over_client_channel(&hop, DECLARED_CHANNEL, 1_000_000).await);

        assert_eq!(reject.code.as_str(), "F02");
        assert!(
            peer.carried().is_empty(),
            "a crossing with no declared rate forwards nothing at all"
        );
    }

    /// And the other of the two answers: a rate this node's own poller let
    /// go stale is `T00`, temporary, because staleness is an outage rather
    /// than a verdict about the path.
    #[tokio::test]
    async fn a_client_arrival_on_a_stale_rate_is_refused_t00() {
        let clock = test_clock();
        let table = empty_table();
        table.write(|table| {
            table.refresh(
                asset(USDC),
                asset(ANYONE),
                rate(USDC_TO_ANYONE, 1),
                clock.now(),
            )
        });
        let peer = Arc::new(CarriesAndRemembers::quoting(0));
        let hop = dealing_hop_over(
            peer.clone(),
            &[("second-hop", ANYONE)],
            0,
            u64::MAX,
            Some(table),
            clock.clone(),
        )
        .with_client_channel_assets([(SettlementChain::Evm, asset(USDC))].into_iter().collect());

        // Past the two-minute ttl `rate_guards` declares.
        clock.advance(Duration::seconds(121));
        let reject = refusal(arrives_over_client_channel(&hop, DECLARED_CHANNEL, 1_000_000).await);

        assert_eq!(reject.code.as_str(), "T00");
        assert!(peer.carried().is_empty());
    }

    /// Decision 7's inverse, applied at this hop (issue #1296): a probe a
    /// buyer sent gets its running cost back in the unit the BUYER counts
    /// in -- its own channel's -- rather than in the unit the far leg
    /// happens to settle in.
    #[tokio::test]
    async fn a_reject_crossing_back_answers_in_the_buyers_own_unit() {
        // 0.001 ANYONE of fee, which at four ANYONE to the USDC is 250 USDC
        // base units -- and 10^12 times that as a raw integer, which is
        // what an un-converted pass-through would have reported to a buyer.
        let fee = 1_000_000_000_000_000;
        let (hop, peer, _clock) = client_paying_hop(
            &[(SettlementChain::Evm, USDC)],
            &[("second-hop", ANYONE)],
            fee,
            u64::MAX,
            Some(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
        );

        let reject = refusal(arrives_over_client_channel(&hop, DECLARED_CHANNEL, 1_000_000).await);

        assert_eq!(peer.carried().len(), 1, "the packet did reach the peer");
        assert_eq!(reject.accumulated_cost, 250);
        assert_ne!(
            reject.accumulated_cost, fee,
            "a buyer that reads the outgoing figure unconverted overstates what the path costs \
             it by the whole decimals gap"
        );
    }

    /// The same probe, over a chain-discovered channel: the reject path
    /// reads the identical `crossing` call the forward did, so a packet and
    /// its reject can never disagree about whether they crossed a boundary
    /// -- whichever door the packet came in.
    #[tokio::test]
    async fn a_probe_over_a_chain_discovered_channel_answers_in_that_channels_unit() {
        // Four ANYONE charged by everything beyond this hop, plus 0.001 of
        // this hop's own.
        let (hop, _peer, _clock) = client_paying_hop_quoting(
            &[(SettlementChain::Evm, USDC)],
            &[("second-hop", ANYONE)],
            1_000_000_000_000_000,
            u64::MAX,
            Some(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
            4_000_000_000_000_000_000,
        );

        let reject =
            refusal(arrives_over_client_channel(&hop, DISCOVERED_CHANNEL, 1_000_000).await);

        // `ceil((4e18 + 1e15) / 4e12)`: one USDC for the far end, 250 base
        // units for this hop, both in the buyer's unit.
        assert_eq!(reject.accumulated_cost, 1_000_250);
    }

    /// The absence rule, which is the safety rule, at the client edge: a
    /// node that declares no `[[tokens]]` resolves no client channel,
    /// crosses no boundary, and runs `amount_after_fee` -- the one
    /// `checked_sub` it has always run. Asserted against that function
    /// directly, so "byte for byte the code it runs today" is a claim a
    /// reader can check rather than a number someone worked out.
    #[tokio::test]
    async fn a_client_arrival_on_a_node_that_resolves_no_token_runs_amount_after_fee() {
        let fee = 10;
        let (hop, peer, _clock) = client_paying_hop(&[], &[], fee, u64::MAX, None);

        arrives_over_client_channel(&hop, DECLARED_CHANNEL, 1_000).await;

        assert!(hop.client_channel_assets.is_empty());
        assert_eq!(
            hop.crossing(Some(Arrival::ClientChannel(DECLARED_CHANNEL)), "second-hop"),
            None
        );
        assert_eq!(
            peer.carried()[0].amount,
            amount_after_fee(1_000, fee).expect("the fee leaves something")
        );
    }

    /// ADR 0028 and ADR 0065, unchanged by any of the above: what a buyer
    /// pays is the charge its edge posted for the route, in the buyer's own
    /// unit, and a downstream denomination boundary is none of its
    /// business. The two hops below differ only in whether they deal -- the
    /// posted charge is the same figure on both, and only what leaves
    /// differs.
    #[tokio::test]
    async fn the_charge_a_buyer_pays_is_its_edges_own_price_whether_or_not_this_hop_deals() {
        let price = 1_100;
        let peer = Arc::new(CarriesAndRemembers::quoting(0));
        let dealing = covering(
            Connector::new(
                vec![],
                vec![PeerRoute::new_priced("g.example.app", "second-hop", price)],
                Arc::new(FakeAppClient::new()),
                peer.clone(),
                test_clock(),
            )
            // The cap is in the OUTGOING unit and this leg's is a
            // 18-decimals one, which is exactly why `local/dealing`'s own
            // config writes it out rather than taking the default.
            .with_peer_packet_caps([("second-hop".to_string(), u64::MAX)])
            .with_peering_assets(
                [("second-hop".to_string(), asset(ANYONE))]
                    .into_iter()
                    .collect(),
            )
            .with_client_channel_assets([(SettlementChain::Evm, asset(USDC))].into_iter().collect())
            .with_rate_table(declaring(USDC, ANYONE, USDC_TO_ANYONE, 1)),
            "second-hop",
        );

        let plain_peer = Arc::new(CarriesAndRemembers::quoting(0));
        let plain = covering(
            Connector::new(
                vec![],
                vec![PeerRoute::new_priced("g.example.app", "second-hop", price)],
                Arc::new(FakeAppClient::new()),
                plain_peer.clone(),
                test_clock(),
            ),
            "second-hop",
        );

        let posted = dealing
            .client_route_price("g.example.app")
            .expect("a priced forwarded route");
        assert_eq!(
            posted.charge(0),
            plain
                .client_route_price("g.example.app")
                .expect("a priced forwarded route")
                .charge(0),
            "what a buyer is asked for is the route's own price, and dealing does not move it"
        );
        assert_eq!(posted.charge(0), price);

        // A buyer that pays exactly what it was asked for is carried by
        // both -- and only the figure that LEAVES differs.
        arrives_over_client_channel(&dealing, DECLARED_CHANNEL, posted.charge(0)).await;
        arrives_over_client_channel(&plain, DECLARED_CHANNEL, posted.charge(0)).await;

        assert_eq!(peer.carried()[0].amount, 4_400_000_000_000_000);
        assert_eq!(plain_peer.carried()[0].amount, price);
    }

    #[tokio::test]
    async fn a_reject_from_the_next_hop_is_relayed_to_the_original_caller() {
        let second_hop = Arc::new(Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let mut peer_transport = InProcessPeerTransport::new();
        peer_transport.add_peer("second-hop", second_hop);
        let first_hop = covering(
            Connector::new(
                vec![],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                Arc::new(FakeAppClient::new()),
                Arc::new(peer_transport),
                test_clock(),
            ),
            "second-hop",
        );

        let response = first_hop
            .handle_prepare(prepare("g.example.app", b"hello"))
            .await;

        match response {
            PacketResponse::Reject(reject) => {
                assert_eq!(reject.code.as_str(), "F02");
                assert!(reject.message.contains("g.example.app"));
            }
            other => panic!("expected a reject, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_terminated_route_wins_over_a_shorter_peer_route() {
        let peer_route = PeerRoute::new("g.example", "second-hop");
        let terminated_route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(terminated_route.handler_url(), answered(b"handled locally"));
        let connector = Connector::new(
            vec![terminated_route],
            vec![peer_route],
            app_client,
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        )
        .with_identity_signer(identity_signer());
        let (sealed, shared_secret) = sealed_prepare(b"hello");

        let response = connector.handle_prepare(sealed).await;

        match response {
            PacketResponse::Fulfill(fulfill) => {
                assert_eq!(fulfill.fulfillment, expected_fulfillment(&shared_secret));
                assert_eq!(
                    open_sealed_envelope(&shared_secret, &fulfill.data),
                    fulfill_envelope(b"handled locally")
                );
            }
            other => panic!("expected a fulfill, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_peer_route_wins_over_a_shorter_terminated_route() {
        let terminated_route = StaticRoute::new("g.example", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        let second_hop_route = StaticRoute::new("g.example.app", "http://localhost:5000").unwrap();
        let second_hop_app_client = Arc::new(FakeAppClient::new());
        second_hop_app_client.respond(
            second_hop_route.handler_url(),
            answered(b"handled by the second hop"),
        );
        let second_hop = Arc::new(
            Connector::new(
                vec![second_hop_route],
                vec![],
                second_hop_app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(identity_signer()),
        );
        let mut peer_transport = InProcessPeerTransport::new();
        peer_transport.add_peer("second-hop", second_hop);
        let first_hop = covering(
            Connector::new(
                vec![terminated_route],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                app_client,
                Arc::new(peer_transport),
                test_clock(),
            ),
            "second-hop",
        );
        let (sealed, shared_secret) = sealed_prepare(b"hello");

        let response = first_hop.handle_prepare(sealed).await;

        match response {
            PacketResponse::Fulfill(fulfill) => {
                assert_eq!(fulfill.fulfillment, expected_fulfillment(&shared_secret));
                assert_eq!(
                    open_sealed_envelope(&shared_secret, &fulfill.data),
                    fulfill_envelope(b"handled by the second hop")
                );
            }
            other => panic!("expected a fulfill, got {other:?}"),
        }
        assert_eq!(second_hop_app_client.deliveries().len(), 1);
    }

    #[test]
    fn routes_reports_every_configured_static_route() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        let clock = test_clock();
        let connector = connector_with(vec![route], app_client, clock);

        let routes = connector.routes();

        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].prefix, "g.example.app");
        assert_eq!(routes[0].handler_url, "http://localhost:4000/");
        assert_eq!(routes[0].price, Price::flat(25));
    }

    #[test]
    fn client_route_price_reports_the_matched_routes_price() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        let clock = test_clock();
        let connector = connector_with(vec![route], app_client, clock);

        assert_eq!(
            connector.client_route_price("g.example.app"),
            Some(Price::flat(25))
        );
        assert_eq!(
            connector.client_route_price("g.example.app.sub"),
            Some(Price::flat(25))
        );
        assert_eq!(connector.client_route_price("g.nowhere"), None);
    }

    /// ADR 0028: the same lookup answers for a route that *forwards* over a
    /// peering, with that route's own `price` -- the whole of what makes a
    /// forwarded destination greetable and chargeable at the client edge.
    /// Its `fee` is deliberately a different number here, so a lookup that
    /// reached for the fee instead would fail rather than coincide.
    #[test]
    fn client_route_price_reports_a_forwarded_routes_price_not_its_fee() {
        let app_client = Arc::new(FakeAppClient::new());
        let connector = Connector::new(
            vec![],
            vec![PeerRoute::new_priced("g.example.store", "store", 100)],
            app_client,
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        )
        .with_peer_fees([("store".to_string(), 3)]);

        let facts = connector
            .client_route("g.example.store.sub")
            .expect("a forwarded route answers the client-edge lookup");
        assert_eq!(facts.price, Price::flat(100));
        assert_eq!(facts.kind, ClientRouteKind::Forwarded);
        // A forwarded route applies no transport policy: it accepts a
        // client's request over either carriage.
        assert_eq!(facts.transport_policy, TransportPolicy::Both);
        assert_eq!(connector.client_route_price("g.nowhere"), None);
    }

    /// The client edge must price the route the router will actually use.
    /// A longer forwarded prefix beneath a terminated one is the case where
    /// asking only about app routes -- what this lookup used to do -- would
    /// charge the app's price and then forward the packet over the peering.
    #[test]
    fn client_route_prices_the_route_the_router_would_choose() {
        let app_client = Arc::new(FakeAppClient::new());
        let connector = Connector::new(
            vec![StaticRoute::new_priced("g.example", "http://localhost:4000", 25).unwrap()],
            vec![PeerRoute::new_priced("g.example.store", "store", 100)],
            app_client,
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        )
        .with_peer_fees([("store".to_string(), 3)]);

        assert_eq!(
            connector.client_route_price("g.example.relay"),
            Some(Price::flat(25))
        );
        assert_eq!(
            connector.client_route_price("g.example.store"),
            Some(Price::flat(100))
        );
        assert_eq!(
            connector.client_route_price("g.example.store.sub"),
            Some(Price::flat(100))
        );
    }

    /// Issue #701: the client edge's two carriages read a transport policy
    /// off the same lookup they read a price off, so it needs the same
    /// longest-prefix matching and the same `None`-for-unmatched behavior.
    #[test]
    fn app_route_transport_policy_reports_the_matched_routes_policy() {
        let route = StaticRoute::new_priced_with_transport(
            "g.example.relay",
            "http://localhost:4000",
            25,
            TransportPolicy::Btp,
        )
        .unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        let clock = test_clock();
        let connector = connector_with(vec![route], app_client, clock);

        assert_eq!(
            connector
                .client_route("g.example.relay")
                .map(|route| route.transport_policy),
            Some(TransportPolicy::Btp)
        );
        assert_eq!(
            connector
                .client_route("g.example.relay.sub")
                .map(|route| route.transport_policy),
            Some(TransportPolicy::Btp)
        );
        assert_eq!(
            connector
                .client_route("g.nowhere")
                .map(|route| route.transport_policy),
            None
        );
    }

    /// A route that never set `transport` reports the default -- both
    /// transports accepted -- through the same accessor.
    #[test]
    fn app_route_transport_policy_defaults_to_both() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        let clock = test_clock();
        let connector = connector_with(vec![route], app_client, clock);

        assert_eq!(
            connector
                .client_route("g.example.app")
                .map(|route| route.transport_policy),
            Some(TransportPolicy::Both)
        );
    }

    #[tokio::test]
    async fn handle_prepare_records_a_fulfill_in_metrics() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(route.handler_url(), answered(b""));
        let connector = connector_with(vec![route], app_client, test_clock());

        connector
            .handle_prepare(prepare("g.example.app", b"hello"))
            .await;

        let metrics = connector.metrics().encode();
        assert!(metrics.contains(r#"toon_packets_total{outcome="fulfill"} 1"#));
    }

    #[tokio::test]
    async fn handle_prepare_records_a_reject_by_code_in_metrics() {
        let app_client = Arc::new(FakeAppClient::new());
        let connector = connector_with(vec![], app_client, test_clock());

        connector
            .handle_prepare(prepare("g.nowhere", b"hello"))
            .await;

        let metrics = connector.metrics().encode();
        assert!(metrics.contains(r#"toon_packets_total{outcome="reject"} 1"#));
        assert!(metrics.contains(r#"toon_packets_rejected_total{code="F02"} 1"#));
    }

    #[tokio::test]
    async fn forwarding_to_a_peer_records_the_earned_fee_only_on_fulfilment() {
        let second_hop_route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let second_hop_app_client = Arc::new(FakeAppClient::new());
        second_hop_app_client.respond(second_hop_route.handler_url(), answered(b""));
        let second_hop = Arc::new(
            Connector::new(
                vec![second_hop_route],
                vec![],
                second_hop_app_client,
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(identity_signer()),
        );
        let mut peer_transport = InProcessPeerTransport::new();
        peer_transport.add_peer("second-hop", second_hop);
        let first_hop = covering(
            Connector::new(
                vec![],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                Arc::new(FakeAppClient::new()),
                Arc::new(peer_transport),
                test_clock(),
            )
            .with_peer_fees([("second-hop".to_string(), 7)]),
            "second-hop",
        );

        first_hop
            .handle_prepare(prepare_with_amount("g.example.app", 100))
            .await;

        let metrics = first_hop.metrics().encode();
        assert!(metrics.contains("toon_fees_earned_total 7"));
    }

    /// Issue #427: a controller outside this connector pushes a route to a
    /// peer with a time limit, and it forwards exactly like a
    /// configuration-sourced peer route until that limit is reached.
    #[tokio::test]
    async fn a_leased_route_forwards_to_its_peer_before_it_lapses() {
        let second_hop_route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let second_hop_app_client = Arc::new(FakeAppClient::new());
        second_hop_app_client.respond(
            second_hop_route.handler_url(),
            answered(b"delivered by the second hop"),
        );
        let second_hop = Arc::new(
            Connector::new(
                vec![second_hop_route],
                vec![],
                second_hop_app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(identity_signer()),
        );
        let mut peer_transport = InProcessPeerTransport::new();
        peer_transport.add_peer("second-hop", second_hop);
        let clock = test_clock();
        // A leased route reaches `forward_via_peer_route` without passing
        // `Config::load`'s `PayChannelUnbound` check (ADR 0028), so the
        // covering configuration is what makes it deliverable at all.
        let first_hop = covering(
            Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(peer_transport),
                clock.clone(),
            ),
            "second-hop",
        );
        first_hop
            .upsert_leased_route("g.example.app", "second-hop", Duration::seconds(60))
            .unwrap();
        let (sealed, shared_secret) = sealed_prepare(b"hello");

        let response = first_hop.handle_prepare(sealed).await;

        match response {
            PacketResponse::Fulfill(fulfill) => {
                assert_eq!(fulfill.fulfillment, expected_fulfillment(&shared_secret));
                assert_eq!(
                    open_sealed_envelope(&shared_secret, &fulfill.data),
                    fulfill_envelope(b"delivered by the second hop")
                );
            }
            other => panic!("expected a fulfill, got {other:?}"),
        }
        assert_eq!(second_hop_app_client.deliveries().len(), 1);
    }

    /// AC: "A lapsed route stops being selected immediately, with no sweep
    /// delay observable to a sender" -- there is no background task here;
    /// expiry is decided fresh on every call against the injected clock.
    #[tokio::test]
    async fn a_lapsed_leased_route_stops_being_selected_immediately() {
        let clock = test_clock();
        let first_hop = Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            clock.clone(),
        );
        first_hop
            .upsert_leased_route("g.example.app", "second-hop", Duration::seconds(60))
            .unwrap();

        // Still active a moment before its limit.
        clock.advance(Duration::seconds(59));
        let response = first_hop
            .handle_prepare(prepare("g.example.app", b"hello"))
            .await;
        match response {
            PacketResponse::Reject(reject) => {
                // second-hop is unregistered on this transport, so a
                // successful *selection* still surfaces as a peer-transport
                // reject rather than F02 (no route) -- proving the route
                // was matched at all, not skipped.
                assert_ne!(reject.code.as_str(), "F02");
            }
            other => panic!("expected some reject, got {other:?}"),
        }

        // One second later, the lease has lapsed -- selected no longer.
        clock.advance(Duration::seconds(1));
        let response = first_hop
            .handle_prepare(prepare("g.example.app", b"hello"))
            .await;
        match response {
            PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F02"),
            other => panic!("expected a reject once the lease lapses, got {other:?}"),
        }
    }

    /// AC: "A leased route lapses unless renewed before its limit expires"
    /// / "A controller that stops renewing causes routes to lapse rather
    /// than persist" -- renewing before the original limit extends it past
    /// where it would otherwise have lapsed.
    #[tokio::test]
    async fn renewing_a_leased_route_before_it_lapses_keeps_it_active() {
        let clock = test_clock();
        let first_hop = Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            clock.clone(),
        );
        first_hop
            .upsert_leased_route("g.example.app", "second-hop", Duration::seconds(60))
            .unwrap();

        clock.advance(Duration::seconds(30));
        first_hop
            .upsert_leased_route("g.example.app", "second-hop", Duration::seconds(60))
            .unwrap();

        // Past the *original* lease's limit (60s from the start), but well
        // within the renewed one (60s from the 30s renewal).
        clock.advance(Duration::seconds(40));
        let response = first_hop
            .handle_prepare(prepare("g.example.app", b"hello"))
            .await;
        match response {
            PacketResponse::Reject(reject) => assert_ne!(reject.code.as_str(), "F02"),
            other => panic!("expected some reject, got {other:?}"),
        }
    }

    /// AC: "A static route always outranks a leased route for the same
    /// prefix" -- an operator's explicit configuration cannot be
    /// overridden by an automated controller.
    #[tokio::test]
    async fn a_static_route_always_outranks_a_leased_route_for_the_same_prefix() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(route.handler_url(), answered(b"handled locally"));
        let clock = test_clock();
        let connector = Connector::new(
            vec![route],
            vec![],
            app_client,
            Arc::new(InProcessPeerTransport::new()),
            clock,
        )
        .with_identity_signer(identity_signer());
        connector
            .upsert_leased_route("g.example.app", "second-hop", Duration::seconds(60))
            .unwrap();
        let (sealed, shared_secret) = sealed_prepare(b"hello");

        let response = connector.handle_prepare(sealed).await;

        match response {
            PacketResponse::Fulfill(fulfill) => {
                assert_eq!(fulfill.fulfillment, expected_fulfillment(&shared_secret));
                assert_eq!(
                    open_sealed_envelope(&shared_secret, &fulfill.data),
                    fulfill_envelope(b"handled locally")
                );
            }
            other => panic!("expected a fulfill, got {other:?}"),
        }
    }

    /// AC: "Static routes survive a restart; leased routes do not" -- a
    /// leased route lives only in the `Connector` instance it was pushed
    /// to, never in configuration, so a freshly constructed instance
    /// (standing in for "after a restart") never has it.
    #[test]
    fn leased_routes_do_not_survive_a_restart() {
        let clock = test_clock();
        let before_restart = Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            clock.clone(),
        );
        before_restart
            .upsert_leased_route("g.example.app", "second-hop", Duration::seconds(60))
            .unwrap();
        assert_eq!(before_restart.leased_routes().len(), 1);

        let after_restart = Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            clock,
        );
        assert!(after_restart.leased_routes().is_empty());
    }

    #[test]
    fn leased_routes_reports_only_currently_active_leases() {
        let clock = test_clock();
        let connector = Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            clock.clone(),
        );
        connector
            .upsert_leased_route("g.example.app", "second-hop", Duration::seconds(60))
            .unwrap();

        let leases = connector.leased_routes();
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0].prefix, "g.example.app");
        assert_eq!(leases[0].peer_id, "second-hop");

        clock.advance(Duration::seconds(60));
        assert!(connector.leased_routes().is_empty());
    }

    #[test]
    fn upsert_leased_route_rejects_an_invalid_prefix() {
        let clock = test_clock();
        let connector = Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            clock,
        );

        let result = connector.upsert_leased_route("g..app", "second-hop", Duration::seconds(60));

        assert!(matches!(result, Err(LeaseRouteError::InvalidPrefix(_))));
        assert!(connector.leased_routes().is_empty());
    }

    #[tokio::test]
    async fn peers_are_empty_until_416_lands_and_channels_and_claims_are_empty_with_nothing_configured(
    ) {
        let app_client = Arc::new(FakeAppClient::new());
        let clock = test_clock();
        let connector = connector_with(vec![], app_client, clock);

        // `peers()` reports the config file's peer ids plus any added at
        // runtime (issue #884) -- empty here because this connector was
        // built with neither.
        assert!(connector.peers().is_empty());
        assert!(connector.channels().await.is_empty());
        // No signer or peer claim channel configured, and no traffic sent:
        // nothing to report. `claims()` reporting real state once claims
        // exist is covered by the `emits_...`/`records_...`-suffixed tests
        // below.
        assert!(connector.claims().is_empty());
    }

    /// A peer transport that always answers a forward with whatever
    /// [`PacketResponse`] it was built with, regardless of what it was
    /// handed -- for a test that asserts on how this connector's own
    /// forwarding treats a downstream answer, not on what a real peer would
    /// decide.
    struct FixedResponsePeerTransport(PacketResponse);

    #[async_trait]
    impl PeerTransport for FixedResponsePeerTransport {
        async fn forward(
            &self,
            _peer_id: &str,
            _prepare: Prepare,
            _claim: Option<WireClaim>,
        ) -> PeerForward {
            PeerForward::answered(self.0.clone(), ClaimAckOutcome::NotSent)
        }

        async fn flush(&self, _peer_id: &str, _claim: WireClaim) -> ClaimAckOutcome {
            ClaimAckOutcome::NotSent
        }
    }

    /// Issue #1269 / ADR 0069: a peer's FULFILL rides home unchecked. Until
    /// this change, a forwarding hop verified a downstream fulfilment
    /// against the packet's own execution condition before relaying it
    /// (issue #417) -- but a hop is paid on arrival regardless (ADR 0042),
    /// so that check protected nothing this hop owns. Whatever a peer
    /// answers with now rides straight home, and it is the sender's own
    /// end-to-end check (`connector send` against `derive_fulfillment`) that
    /// catches a forged delivery.
    #[tokio::test]
    async fn a_peers_fulfillment_rides_home_unchecked() {
        let bogus_fulfillment = [9u8; 32]; // not derived from anything this packet sealed
        let peer_transport = FixedResponsePeerTransport(PacketResponse::Fulfill(Fulfill {
            fulfillment: bogus_fulfillment,
            data: b"claimed delivery".to_vec(),
        }));
        let connector = covering(
            Connector::new(
                vec![],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                Arc::new(FakeAppClient::new()),
                Arc::new(peer_transport),
                test_clock(),
            ),
            "second-hop",
        );

        let response = connector
            .handle_prepare(prepare("g.example.app", b"hello"))
            .await;

        match response {
            PacketResponse::Fulfill(fulfill) => {
                assert_eq!(fulfill.fulfillment, bogus_fulfillment);
                assert_eq!(fulfill.data, b"claimed delivery");
            }
            other => {
                panic!("expected the peer's fulfillment to ride home unchecked, got {other:?}")
            }
        }
    }

    /// A peer that carries whatever it is handed and keeps the packets it
    /// carried, so a test can read back the PREPARE that actually went on
    /// the wire rather than the one that arrived.
    ///
    /// It is a fake and not a mock (ADR 0007): it answers every forward the
    /// way the port says a reachable peer answers -- with a REJECT it
    /// genuinely decided on, `reached_peer` true -- and asserts nothing
    /// about being called. What the tests below assert is the state it
    /// accumulated, which is the packet itself.
    #[derive(Default)]
    struct CarriesAndRemembers {
        carried: Mutex<Vec<Prepare>>,
        /// The claim that rode with each carried packet, in the same order
        /// -- what `cover_forward` actually minted, which is the only way
        /// to see whether ADR 0071's covering claim was written for the
        /// converted figure (issue #1295).
        covered_by: Mutex<Vec<Option<WireClaim>>>,
        /// The running cost this peer's own reject arrives with: what
        /// everything beyond it charges, in ITS unit, which is the outgoing
        /// leg's (ADR 0011, ADR 0071 decision 7, issue #1296). Zero -- the
        /// default, and what every case before #1296 wanted -- is a far end
        /// that charges nothing, so the only cost a reject carries home is
        /// the fee of the hop under test.
        quotes: u64,
    }

    impl CarriesAndRemembers {
        /// A peer beyond which the path costs `quoted`, in this peer's own
        /// unit.
        fn quoting(quoted: u64) -> CarriesAndRemembers {
            CarriesAndRemembers {
                quotes: quoted,
                ..CarriesAndRemembers::default()
            }
        }

        fn carried(&self) -> Vec<Prepare> {
            self.carried.lock().unwrap().clone()
        }

        fn covered_by(&self) -> Vec<Option<WireClaim>> {
            self.covered_by.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl PeerTransport for CarriesAndRemembers {
        async fn forward(
            &self,
            _peer_id: &str,
            prepare: Prepare,
            claim: Option<WireClaim>,
        ) -> PeerForward {
            self.carried.lock().unwrap().push(prepare);
            self.covered_by.lock().unwrap().push(claim);
            PeerForward::answered(
                PacketResponse::Reject(Reject {
                    code: RejectCode::f02_unreachable(),
                    triggered_by: "g.peer".to_string(),
                    message: "carried, and the far end had nowhere to put it".to_string(),
                    data: Vec::new(),
                    accumulated_cost: self.quotes,
                }),
                ClaimAckOutcome::NotSent,
            )
        }

        async fn flush(&self, _peer_id: &str, _claim: WireClaim) -> ClaimAckOutcome {
            ClaimAckOutcome::NotSent
        }
    }

    fn forwards_to(peer: Arc<CarriesAndRemembers>, clock: Arc<TestClock>) -> Connector {
        covering(
            Connector::new(
                vec![],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                Arc::new(FakeAppClient::new()),
                peer,
                clock,
            ),
            "second-hop",
        )
    }

    /// PF-19, issue #1174: a hop decreases a packet's expiry when it
    /// forwards. Until this landed `expires_at` was copied through
    /// verbatim, so the last hop held the sender's entire remaining budget
    /// and every hop above it could have its own deadline fire while the
    /// fulfilment it had already paid for was still in flight.
    #[tokio::test]
    async fn a_forwarded_packet_expires_before_the_one_that_arrived() {
        let peer = Arc::new(CarriesAndRemembers::default());
        let clock = test_clock();
        let connector = forwards_to(peer.clone(), clock.clone());
        let expires_at = clock.now() + Duration::seconds(30);

        connector
            .handle_prepare(prepare_expiring_at("g.example.app", b"hello", expires_at))
            .await;

        let carried = peer.carried();
        assert_eq!(carried.len(), 1, "the packet should have been forwarded");
        assert!(
            carried[0].expires_at < expires_at,
            "a forwarded packet must expire strictly before the one that arrived: \
             arrived {expires_at}, forwarded {}",
            carried[0].expires_at
        );
        // The window is kept back whole, and off the arriving expiry rather
        // than off `now` -- what a hop owes the return leg is a fixed
        // amount of time, not a fraction of whatever budget it was handed.
        assert_eq!(
            carried[0].expires_at,
            expires_at - FORWARDING_MESSAGE_WINDOW
        );
    }

    /// PF-19's second half: a hop with less than a message window left does
    /// not forward a packet with a dead or negative window -- it refuses
    /// it. `R00`, the code PF-02 gives an already-expired arrival, because
    /// this is the same fact one hop later: the packet has run out of time
    /// here.
    #[tokio::test]
    async fn a_packet_with_no_time_left_for_the_return_leg_is_refused_rather_than_forwarded() {
        let peer = Arc::new(CarriesAndRemembers::default());
        let clock = test_clock();
        let connector = forwards_to(peer.clone(), clock.clone());
        // Alive on arrival -- PF-02 lets it through -- but with less than a
        // whole window left, so there is no forward that could be answered
        // in time.
        let expires_at = clock.now() + FORWARDING_MESSAGE_WINDOW - Duration::milliseconds(1);

        let response = connector
            .handle_prepare(prepare_expiring_at("g.example.app", b"hello", expires_at))
            .await;

        match response {
            PacketResponse::Reject(reject) => {
                assert_eq!(reject.code.as_str(), "R00");
                // A class-only code (`packet-flow-spec.md` §5) carries its
                // diagnosis in the message or nowhere.
                assert!(
                    reject.message.contains("return leg"),
                    "message should say what the time was needed for: {}",
                    reject.message
                );
            }
            other => panic!("expected a reject, got {other:?}"),
        }
        assert!(
            peer.carried().is_empty(),
            "a packet with no window left must never reach the wire"
        );
    }

    /// The boundary is [`is_expired`]'s, one hop out: a shortened expiry
    /// landing exactly on `now` is dead, and a packet is forwarded only
    /// when strictly more than a window remains.
    #[tokio::test]
    async fn a_packet_with_exactly_one_window_left_is_refused_and_a_hair_more_is_carried() {
        let peer = Arc::new(CarriesAndRemembers::default());
        let clock = test_clock();
        let connector = forwards_to(peer.clone(), clock.clone());

        let response = connector
            .handle_prepare(prepare_expiring_at(
                "g.example.app",
                b"hello",
                clock.now() + FORWARDING_MESSAGE_WINDOW,
            ))
            .await;
        match response {
            PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "R00"),
            other => panic!("expected a reject at exactly one window, got {other:?}"),
        }
        assert!(peer.carried().is_empty());

        connector
            .handle_prepare(prepare_expiring_at(
                "g.example.app",
                b"hello",
                clock.now() + FORWARDING_MESSAGE_WINDOW + Duration::milliseconds(1),
            ))
            .await;
        assert_eq!(peer.carried().len(), 1);
    }

    /// ADR 0064 / PF-25 (issue #1183): the packet's own deadline bounds how
    /// long a termination waits for its app.
    ///
    /// Before this landed, `deliver_opened_envelope` never read
    /// `expires_at`. Expiry was checked once on arrival (PF-02) and never
    /// again, so an app that took an hour over a thirty-second packet was
    /// still answered for: the fulfilment was derived (ADR 0019) and a
    /// FULFILL went back to an upstream that had long since given up. Every
    /// test here fails against that tree.
    ///
    /// The two doubles are the real things, not expectation-checking stubs
    /// (ADR 0007). [`SlowAppClient`] is an app that genuinely takes time and
    /// then genuinely answers -- upholding the [`AppClient`] contract
    /// exactly as [`FakeAppClient`] does, only later -- which is the whole
    /// situation under test and cannot be faked by asserting on calls.
    /// [`ClockThatMovesAfterArrival`] is the other half of the same
    /// situation: a clock that has moved on by the time delivery is reached,
    /// which is what every real clock does and what a frozen [`TestClock`]
    /// structurally cannot.
    mod termination_deadline {
        use super::*;
        use crate::app_client::AppClient;
        use crate::clock::Clock;
        use connector_domain::EnvelopeResponse;
        use connector_signer::giftwrap::looks_like_sealed_response;
        use url::Url;

        /// An app that answers -- after `delay` of real elapsed time. The
        /// delay is real because what is under test is a timer: the budget
        /// is decided from the injected clock by
        /// `connector_domain::delivery_budget` (proven exhaustively over
        /// there), and the only thing left for this side to prove is that
        /// the connector stops waiting when it runs out.
        struct SlowAppClient {
            delay: std::time::Duration,
            response: EnvelopeResponse,
            deliveries: std::sync::atomic::AtomicUsize,
        }

        impl SlowAppClient {
            fn new(delay: std::time::Duration, body: &[u8]) -> SlowAppClient {
                SlowAppClient {
                    delay,
                    response: EnvelopeResponse {
                        status: 200,
                        headers: vec![],
                        body: body.to_vec(),
                    },
                    deliveries: std::sync::atomic::AtomicUsize::new(0),
                }
            }

            fn deliveries(&self) -> usize {
                self.deliveries.load(std::sync::atomic::Ordering::SeqCst)
            }
        }

        #[async_trait]
        impl AppClient for SlowAppClient {
            async fn deliver(&self, _handler_url: &Url, _request: &EnvelopeRequest) -> AppOutcome {
                self.deliveries
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(self.delay).await;
                AppOutcome::Answered {
                    response: self.response.clone(),
                }
            }
        }

        /// A clock reporting one instant to its first reader and a much
        /// later one to every reader after it: the smallest fake that puts a
        /// packet's arrival check and its delivery on opposite sides of the
        /// same deadline. A century is not realism, it is margin -- the
        /// packet must be alive for `reject_ineligible` (the first read on
        /// this path) and unambiguously dead by the time delivery asks,
        /// however many reads the router makes in between. If a future
        /// change ever reads the clock *before* the arrival check, this test
        /// fails loudly on the wrong reject message rather than passing for
        /// the wrong reason.
        struct ClockThatMovesAfterArrival {
            arrival: DateTime<Utc>,
            reads: std::sync::atomic::AtomicUsize,
        }

        impl ClockThatMovesAfterArrival {
            fn new(arrival: DateTime<Utc>) -> ClockThatMovesAfterArrival {
                ClockThatMovesAfterArrival {
                    arrival,
                    reads: std::sync::atomic::AtomicUsize::new(0),
                }
            }
        }

        impl Clock for ClockThatMovesAfterArrival {
            fn now(&self) -> DateTime<Utc> {
                if self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    self.arrival
                } else {
                    self.arrival + Duration::days(365 * 100)
                }
            }
        }

        const PRICE: u64 = 1000;

        fn priced_route() -> StaticRoute {
            StaticRoute::new_priced("g.example.app", "http://localhost:4000", PRICE).unwrap()
        }

        fn connector_with_clock(
            route: StaticRoute,
            app_client: Arc<dyn AppClient>,
            clock: Arc<dyn Clock>,
        ) -> Connector {
            Connector::new(
                vec![route],
                vec![],
                app_client,
                Arc::new(InProcessPeerTransport::new()),
                clock,
            )
            .with_identity_signer(identity_signer())
        }

        /// A packet expiring in 100ms, handed to an app that takes two
        /// seconds. The old tree derived the fulfilment and returned a
        /// FULFILL two seconds after the sender's deadline; this one
        /// abandons the request at the deadline and says so.
        #[tokio::test]
        async fn an_app_slower_than_the_deadline_is_abandoned_rather_than_answered_late() {
            let route = priced_route();
            let app_client = Arc::new(SlowAppClient::new(
                std::time::Duration::from_secs(2),
                b"far too late",
            ));
            let clock = test_clock();
            let expires_at = clock.now() + Duration::milliseconds(100);
            let connector =
                connector_with_clock(route, app_client.clone(), clock as Arc<dyn Clock>);
            let (mut sealed, _shared_secret) = sealed_prepare(b"hello app");
            sealed.expires_at = expires_at;

            let response = connector.handle_prepare(sealed).await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "R00");
                    assert!(
                        reject.message.contains("did not answer"),
                        "{}",
                        reject.message
                    );
                    assert!(
                        reject.message.contains(&expires_at.to_rfc3339()),
                        "the reject names the deadline it fired on: {}",
                        reject.message
                    );
                    // The app was asked and the priced work was set in
                    // motion, so unlike an unreachable app this reject
                    // carries the route's price (ADR 0011, issue #545).
                    assert_eq!(reject.accumulated_cost, PRICE);
                }
                other => panic!("expected an R00 reject, got {other:?}"),
            }
            assert_eq!(
                app_client.deliveries(),
                1,
                "the app is still asked -- this packet was alive when it arrived"
            );
        }

        /// The rule is that the deadline bounds the *wait*, not the answer.
        /// An app that takes real time and comes back inside the budget is
        /// answered for exactly as a prompt one is -- there is no second
        /// expiry check applied to work already done (ADR 0064).
        #[tokio::test]
        async fn an_app_answering_inside_the_deadline_still_fulfils() {
            let route = priced_route();
            let app_client = Arc::new(SlowAppClient::new(
                std::time::Duration::from_millis(50),
                b"app said yes",
            ));
            let clock = test_clock();
            let expires_at = clock.now() + Duration::seconds(5);
            let connector =
                connector_with_clock(route, app_client.clone(), clock as Arc<dyn Clock>);
            let (mut sealed, shared_secret) = sealed_prepare(b"hello app");
            sealed.expires_at = expires_at;

            let response = connector.handle_prepare(sealed).await;

            match response {
                PacketResponse::Fulfill(fulfill) => {
                    assert_eq!(fulfill.fulfillment, expected_fulfillment(&shared_secret));
                    assert_eq!(
                        open_sealed_envelope(&shared_secret, &fulfill.data),
                        fulfill_envelope(b"app said yes")
                    );
                }
                other => panic!("expected a fulfill, got {other:?}"),
            }
            assert_eq!(app_client.deliveries(), 1);
        }

        /// PF-25's first half: a packet whose deadline has fired between
        /// arriving and reaching delivery is not handed to the app at all.
        /// Nothing was asked of it, so nothing is charged for the attempt --
        /// the same figure `Unreachable` and `Refused` carry, and a
        /// different message from the abandoned case above, so a sender can
        /// tell "too late to ask" from "your app was too slow".
        #[tokio::test]
        async fn a_packet_whose_deadline_fires_before_delivery_never_reaches_the_app() {
            let route = priced_route();
            let app_client = Arc::new(SlowAppClient::new(
                std::time::Duration::ZERO,
                b"never reached",
            ));
            let arrival = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
            let expires_at = arrival + Duration::seconds(30);
            let connector = connector_with_clock(
                route,
                app_client.clone(),
                Arc::new(ClockThatMovesAfterArrival::new(arrival)),
            );
            let (mut sealed, _shared_secret) = sealed_prepare(b"hello app");
            sealed.expires_at = expires_at;

            let response = connector.handle_prepare(sealed).await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "R00");
                    assert!(
                        reject.message.contains("the app was not asked"),
                        "{}",
                        reject.message
                    );
                    assert_eq!(reject.accumulated_cost, 0);
                }
                other => panic!("expected an R00 reject, got {other:?}"),
            }
            assert_eq!(
                app_client.deliveries(),
                0,
                "a packet with no time left is never handed to the app"
            );
        }

        /// Both refusals are raised below the gift wrap, so they ride home
        /// sealed to the sender's own shared secret like every other verdict
        /// a termination reaches (ADR 0018, PF-24) -- readable by the sender
        /// who holds that secret and by nobody else.
        #[tokio::test]
        async fn an_abandoned_deliverys_reject_is_sealed_to_the_sender() {
            let route = priced_route();
            let app_client = Arc::new(SlowAppClient::new(
                std::time::Duration::from_secs(2),
                b"far too late",
            ));
            let clock = test_clock();
            let expires_at = clock.now() + Duration::milliseconds(100);
            let connector = connector_with_clock(route, app_client, clock as Arc<dyn Clock>);
            let (mut sealed, shared_secret) = sealed_prepare(b"hello app");
            sealed.expires_at = expires_at;

            let response = connector.handle_prepare(sealed).await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "R00");
                    assert!(looks_like_sealed_response(&reject.data));
                    connector_signer::giftwrap::open_response(&shared_secret, &reject.data)
                        .expect("an abandoned delivery's reject opens with the request's secret");
                    assert!(
                        connector_signer::giftwrap::open_response(&[0xffu8; 32], &reject.data)
                            .is_err()
                    );
                }
                other => panic!("expected an R00 reject, got {other:?}"),
            }
        }
    }

    /// Issue #881: every packet forwarded to a hop configured for
    /// client-role covering carries a claim of its own, minted before the
    /// packet is sent -- plus issue #875's one bounded retry for the hop
    /// that greets a covered packet anyway.
    ///
    /// The stand-ins here are the two real roles of that exchange, not
    /// mocks with expectations: [`GreetingPeer`] is a receiver that refuses
    /// an uncovered packet with terms and carries a covered one -- exactly
    /// what a client edge does (`client-edge-spec.md` §1.4) -- and
    /// [`StandingWatermark`] is the receiver answering where this node's
    /// claims on the channel stand, the same answer `HttpClaimState` reads
    /// off a real `POST /ilp/claim-state` (proven against a live HTTP
    /// receiver in `outbound_client`'s own tests).
    mod covering_a_forward {
        use super::*;
        use crate::claim::ClaimSignature;
        use crate::outbound_client::{
            ClaimStateDomain, ClaimStateSource, ClaimWatermark, EvmDomain, OutboundClientError,
            OutboundClientLedger,
        };
        use connector_domain::x402::{
            X402ChannelExtra, X402PaymentOption, X402PaymentRequired, X402Resource,
            X402SettlementTerms, X402_VERSION,
        };
        use connector_signer::{LocalEd25519Signer, LocalSigner};
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

        /// The price the far side quotes for carrying one packet.
        const PRICE: u64 = 250;
        /// What the packet itself is worth -- deliberately different from
        /// `PRICE`, so a test can tell which figure a claim covers.
        const PACKET_AMOUNT: u64 = 1_000;

        /// The terms a receiver quotes, carrying the EVM settlement facts
        /// (issues #617/#632) a payer signs its claim under. The domain is
        /// [`test_channel_domain`]'s, because the channel the greeting is
        /// about is this crate's own test channel.
        fn quoted_terms() -> X402PaymentRequired {
            let domain = test_channel_domain();
            X402PaymentRequired {
                x402_version: X402_VERSION,
                resource: X402Resource {
                    url: "g.example.app".to_string(),
                },
                request: None,
                accepts: vec![X402PaymentOption {
                    scheme: "toon-channel".to_string(),
                    network: "g.example.app".to_string(),
                    amount: PRICE.to_string(),
                    pay_to: "g.example.app".to_string(),
                    max_timeout_seconds: 60,
                    http_endpoint: "/ilp".to_string(),
                    extra: X402ChannelExtra {
                        settlement: Some(X402SettlementTerms {
                            chain: format!("evm:{}", domain.chain_id),
                            settlement_address: format!("0x{}", "aa".repeat(20)),
                            token_network_registry: format!("0x{}", "bb".repeat(20)),
                            token_network: format!(
                                "0x{}",
                                domain
                                    .token_network_address
                                    .iter()
                                    .map(|byte| format!("{byte:02x}"))
                                    .collect::<String>()
                            ),
                            token_address: format!("0x{}", "cc".repeat(20)),
                            decimals: 6,
                        }),
                        ..X402ChannelExtra::default()
                    },
                }],
            }
        }

        /// A next hop that carries a packet only once it is covered: an
        /// uncovered PREPARE is refused with the §1.4 greeting, a covered
        /// one fulfils. `always_greets` is the far side that goes on
        /// demanding payment even after a covering claim -- a price that
        /// moved, or a claim its gate would not take.
        struct GreetingPeer {
            fulfillment: [u8; 32],
            always_greets: bool,
            /// Whether it quotes terms at all. `false` is the peer that
            /// never demands payment -- the pre-#875 world, which must be
            /// unaffected.
            greets: bool,
            /// Every forward it saw, in order, with whatever claim rode
            /// along.
            seen: Mutex<Vec<Option<WireClaim>>>,
        }

        impl GreetingPeer {
            fn new(fulfillment: [u8; 32]) -> Arc<GreetingPeer> {
                Arc::new(GreetingPeer {
                    fulfillment,
                    always_greets: false,
                    greets: true,
                    seen: Mutex::new(Vec::new()),
                })
            }

            fn seen(&self) -> Vec<Option<WireClaim>> {
                self.seen.lock().expect("seen lock poisoned").clone()
            }
        }

        #[async_trait]
        impl PeerTransport for GreetingPeer {
            async fn forward(
                &self,
                _peer_id: &str,
                _prepare: Prepare,
                claim: Option<WireClaim>,
            ) -> PeerForward {
                let covered = claim
                    .as_ref()
                    .is_some_and(|claim| claim.cumulative_amount >= PRICE);
                self.seen.lock().expect("seen lock poisoned").push(claim);
                if self.greets && (!covered || self.always_greets) {
                    return PeerForward::quoted(
                        PacketResponse::Reject(Reject {
                            code: RejectCode::f06_unexpected_payment(),
                            triggered_by: String::new(),
                            message: "payment required".to_string(),
                            data: Vec::new(),
                            accumulated_cost: 0,
                        }),
                        ClaimAckOutcome::NotSent,
                        quoted_terms(),
                    );
                }
                PeerForward::answered(
                    PacketResponse::Fulfill(Fulfill {
                        fulfillment: self.fulfillment,
                        data: Vec::new(),
                    }),
                    ClaimAckOutcome::Accepted,
                )
            }

            async fn flush(&self, _peer_id: &str, _claim: WireClaim) -> ClaimAckOutcome {
                ClaimAckOutcome::NotSent
            }
        }

        /// The receiver answering where this node's claims on the channel
        /// stand -- the authority the outbound client ledger prices every
        /// claim off (see `crate::outbound_client`'s header).
        struct StandingWatermark {
            nonce: AtomicU64,
            cumulative: AtomicU64,
            asked: AtomicU64,
            /// Which chain's ask each call made (issue #1146). A covering
            /// payer that asked one chain and signed on the other would
            /// still get an answer here, so the ask itself has to be
            /// observable for a test to say the arm was chosen correctly.
            asked_about: Mutex<Vec<ClaimStateDomain>>,
            /// The receiver that will not answer at all: there is then no
            /// watermark to advance, and nothing safe to sign.
            silent: AtomicBool,
        }

        impl StandingWatermark {
            fn at(nonce: u64, cumulative: u64) -> Arc<StandingWatermark> {
                Arc::new(StandingWatermark {
                    nonce: AtomicU64::new(nonce),
                    cumulative: AtomicU64::new(cumulative),
                    asked: AtomicU64::new(0),
                    asked_about: Mutex::new(Vec::new()),
                    silent: AtomicBool::new(false),
                })
            }
        }

        #[async_trait]
        impl ClaimStateSource for StandingWatermark {
            async fn watermark(
                &self,
                channel: &[u8; 32],
                domain: &ClaimStateDomain,
            ) -> Result<ClaimWatermark, OutboundClientError> {
                self.asked.fetch_add(1, Ordering::SeqCst);
                self.asked_about
                    .lock()
                    .expect("asked-about lock poisoned")
                    .push(*domain);
                if self.silent.load(Ordering::SeqCst) {
                    return Err(OutboundClientError::ClaimStateUnavailable {
                        channel: format!("0x{}", hex_lower(channel)),
                        reason: "the test receiver is not answering".to_string(),
                    });
                }
                Ok(ClaimWatermark {
                    nonce: self.nonce.load(Ordering::SeqCst),
                    cumulative: u128::from(self.cumulative.load(Ordering::SeqCst)),
                    available: Some(1_000_000),
                })
            }
        }

        /// [`test_channel_domain`]'s facts, as the [`EvmDomain`]
        /// `with_outbound_client_hop` (issue #881) takes as operator
        /// config rather than reads off a greeting -- the same chain id
        /// and `TokenNetwork` a channel's peer-role domain carries, since
        /// both roles sign against the very same on-chain channel.
        fn test_channel_evm_domain() -> EvmDomain {
            let domain = test_channel_domain();
            EvmDomain {
                chain_id: domain.chain_id,
                token_network: domain.token_network_address,
            }
        }

        /// A first hop routing `g.example.app` to `second-hop`, charging
        /// `fee` for the carriage, holding the client role for that hop:
        /// the ledger it signs from plus the receiver as the authority on
        /// where its claims stand. The channel is also bound in the PEER
        /// role (`with_channel_domain`) because that is the deployed
        /// shape -- one channel, the peer role for what arrives and the
        /// client role for what this node sends.
        ///
        /// It no longer registers an OUTBOUND peer-role channel, because
        /// there is no longer such a thing (issue #1145): nothing is owed
        /// to a peer after a fulfilment, so `[[peer_channels]]` is an
        /// inbound binding only.
        fn first_hop_charging(
            peer: Arc<GreetingPeer>,
            receiver: Arc<StandingWatermark>,
            ledger: Arc<OutboundClientLedger>,
            fee: u64,
        ) -> Connector {
            Connector::new(
                vec![],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                Arc::new(FakeAppClient::new()),
                peer,
                test_clock(),
            )
            .with_peer_fees([("second-hop".to_string(), fee)])
            .with_signer(Arc::new(LocalSigner::generate("settlement-key")))
            .with_channel_domain(test_channel_id(1), test_channel_domain())
            .expect("test_channel_id(1) is a valid on-chain channel id")
            .with_outbound_client_ledger(ledger)
            .with_outbound_client_hop(
                "second-hop",
                test_channel_id(1),
                test_channel_evm_domain(),
                receiver,
            )
            .expect("test_channel_id(1) is a valid on-chain channel id")
        }

        /// [`first_hop_charging`] with no fee, which is what most of these
        /// tests want: the claim then covers the packet's own amount and
        /// there is one number to follow rather than two.
        fn first_hop(
            peer: Arc<GreetingPeer>,
            receiver: Arc<StandingWatermark>,
            ledger: Arc<OutboundClientLedger>,
        ) -> Connector {
            first_hop_charging(peer, receiver, ledger, 0)
        }

        fn ledger() -> (tempfile::TempDir, Arc<OutboundClientLedger>) {
            let dir = tempfile::tempdir().expect("tempdir");
            let ledger = Arc::new(
                OutboundClientLedger::open(dir.path().join("outbound-client.log")).expect("open"),
            );
            (dir, ledger)
        }

        /// **The test that proves the hole is closed.** N consecutive
        /// forwards over a healthy link -- nothing is ever pending on the
        /// peer ledger in this test at all, so before #881 the old
        /// `pending_claim`-first design would have sent every single one
        /// of these uncovered -- each carry their OWN covering claim from
        /// the FIRST attempt: no round trip is ever spent discovering a
        /// refusal before this node pays.
        #[tokio::test]
        async fn n_consecutive_forwards_over_a_healthy_link_each_carry_their_own_covering_claim() {
            let (sealed, shared_secret) = sealed_prepare(b"hello");
            let peer = GreetingPeer::new(expected_fulfillment(&shared_secret));
            let receiver = StandingWatermark::at(7, 7_000);
            let (_dir, ledger) = ledger();
            let connector = first_hop(peer.clone(), receiver, ledger.clone());

            const N: u64 = 3;
            for _ in 0..N {
                let response = connector
                    .handle_prepare(Prepare {
                        amount: PACKET_AMOUNT,
                        ..sealed.clone()
                    })
                    .await;
                assert!(
                    matches!(response, PacketResponse::Fulfill(_)),
                    "a proactively covered forward must fulfil, got {response:?}"
                );
            }

            let seen = peer.seen();
            assert_eq!(
                seen.len(),
                N as usize,
                "no retries: every one of the N attempts was covered and fulfilled on its first try"
            );
            for (index, claim) in seen.iter().enumerate() {
                let claim = claim.as_ref().unwrap_or_else(|| {
                    panic!("packet {index} was emitted uncovered -- exactly the hole #881 closes")
                });
                assert_eq!(claim.channel_id, test_channel_id(1));
                assert_eq!(
                    claim.nonce,
                    8 + index as u64,
                    "nonces advance one per packet, starting above the receiver's watermark"
                );
                assert_eq!(
                    claim.cumulative_amount,
                    7_000 + PACKET_AMOUNT,
                    "each claim covers this node's own forwarded value over the receiver's \
                     watermark, not merely the peer's quoted price"
                );
            }
            assert_eq!(ledger.issued_nonce("second-hop"), 7 + N);
        }

        /// The first packet after a restart is covered too -- not just
        /// packets after the process has already signed one (issue #881's
        /// own acceptance). A restart reopens the SAME ledger file with a
        /// fresh, empty in-memory book, which is exactly what proves the
        /// nonce floor on disk -- not anything this process remembers --
        /// is what makes the first forward proactive rather than merely
        /// "warmed up".
        #[tokio::test]
        async fn the_first_forward_after_a_restart_is_covered() {
            let (sealed, shared_secret) = sealed_prepare(b"hello");
            let receiver = StandingWatermark::at(5, 5_000);
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("outbound-client.log");

            // A prior process's ledger, over the same file, signs one claim
            // and exits -- nothing in memory survives it.
            {
                let ledger = Arc::new(OutboundClientLedger::open(&path).expect("open"));
                let peer = GreetingPeer::new(expected_fulfillment(&shared_secret));
                let connector = first_hop(peer.clone(), receiver.clone(), ledger.clone());
                let response = connector
                    .handle_prepare(Prepare {
                        amount: PACKET_AMOUNT,
                        ..sealed.clone()
                    })
                    .await;
                assert!(matches!(response, PacketResponse::Fulfill(_)));
                assert_eq!(ledger.issued_nonce("second-hop"), 6);
            }

            // The restart: a fresh ledger, over the same file, in a fresh
            // connector.
            let ledger = Arc::new(OutboundClientLedger::open(&path).expect("reopen"));
            let peer = GreetingPeer::new(expected_fulfillment(&shared_secret));
            let connector = first_hop(peer.clone(), receiver, ledger);
            let response = connector
                .handle_prepare(Prepare {
                    amount: PACKET_AMOUNT,
                    ..sealed
                })
                .await;

            assert!(
                matches!(response, PacketResponse::Fulfill(_)),
                "the first forward after a restart must still be covered, got {response:?}"
            );
            let seen = peer.seen();
            assert_eq!(
                seen.len(),
                1,
                "covered from the first attempt -- no retry needed"
            );
            let claim = seen[0]
                .as_ref()
                .expect("the first packet after a restart must not be emitted uncovered");
            assert_eq!(
                claim.nonce, 7,
                "the restart resumes above every nonce ever issued, never reusing one"
            );
        }

        /// One retry, never a loop: a peer that keeps demanding payment
        /// despite an already-covering claim gets exactly one recovery
        /// attempt, and its second refusal is the answer.
        #[tokio::test]
        async fn a_second_payment_required_is_a_failure_rather_than_a_second_retry() {
            let (sealed, shared_secret) = sealed_prepare(b"hello");
            let mut peer = GreetingPeer::new(expected_fulfillment(&shared_secret));
            Arc::get_mut(&mut peer).expect("sole owner").always_greets = true;
            let receiver = StandingWatermark::at(0, 0);
            let (_dir, ledger) = ledger();
            let connector = first_hop(peer.clone(), receiver, ledger.clone());

            let response = connector
                .handle_prepare(Prepare {
                    amount: PACKET_AMOUNT,
                    ..sealed
                })
                .await;

            match response {
                PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F06"),
                other => panic!("expected the peer's second refusal, got {other:?}"),
            }
            assert_eq!(
                peer.seen().len(),
                2,
                "bounded: the proactively covered attempt and exactly one recovery retry"
            );
            assert_eq!(
                ledger.issued_nonce("second-hop"),
                2,
                "one claim for the proactive attempt, one more for the one bounded retry"
            );
        }

        /// No double-spend: a proactively covered forward advances the
        /// outbound client ledger exactly once, and no peer-role claim is
        /// armed for the same packet.
        #[tokio::test]
        async fn a_covered_forward_advances_one_ledger_once_and_the_other_not_at_all() {
            let (sealed, shared_secret) = sealed_prepare(b"hello");
            let peer = GreetingPeer::new(expected_fulfillment(&shared_secret));
            let receiver = StandingWatermark::at(3, 3_000);
            let (_dir, ledger) = ledger();
            let connector = first_hop(peer.clone(), receiver.clone(), ledger.clone());

            let response = connector
                .handle_prepare(Prepare {
                    amount: PACKET_AMOUNT,
                    ..sealed
                })
                .await;

            assert!(matches!(response, PacketResponse::Fulfill(_)));
            assert_eq!(
                ledger.issued_nonce("second-hop"),
                4,
                "exactly one nonce for one successful forward"
            );
            assert_eq!(
                receiver.asked.load(Ordering::SeqCst),
                1,
                "the receiver is asked once per covered packet, not once per attempt"
            );
            assert!(
                connector.claims.pending_claim("second-hop").is_none(),
                "a packet already covered by a client claim must not also arm a peer claim -- \
                 one packet, one debt"
            );
        }

        /// No double-spend, part two: even a fully failed forward --
        /// proactive cover refused, recovery retry refused too -- only
        /// ever skips nonces, and the value it would have moved is never
        /// lost: a fresh attempt against a peer that now accepts payment
        /// succeeds proactively, at a strictly higher nonce, still priced
        /// off whatever the receiver reports.
        #[tokio::test]
        async fn a_failed_forward_skips_nonces_rather_than_burning_the_ledger() {
            let (sealed, shared_secret) = sealed_prepare(b"hello");
            let mut peer = GreetingPeer::new(expected_fulfillment(&shared_secret));
            Arc::get_mut(&mut peer).expect("sole owner").always_greets = true;
            let receiver = StandingWatermark::at(11, 11_000);
            let (_dir, ledger) = ledger();
            let refusing = first_hop(peer.clone(), receiver.clone(), ledger.clone());

            let refused = refusing
                .handle_prepare(Prepare {
                    amount: PACKET_AMOUNT,
                    ..sealed.clone()
                })
                .await;
            assert!(matches!(refused, PacketResponse::Reject(_)));
            let seen = peer.seen();
            let proactive = seen[0]
                .as_ref()
                .expect("the proactive attempt did carry a claim");
            let burned = seen[1]
                .as_ref()
                .expect("the failed retry did carry a claim")
                .clone();
            assert!(
                burned.nonce > proactive.nonce,
                "the retry must burn a strictly higher nonce than the proactive attempt: \
                 {} vs {}",
                burned.nonce,
                proactive.nonce
            );

            // The receiver never recorded either claim -- it is still where
            // it was. A second attempt, against a peer that now takes
            // payment, succeeds proactively on the first try.
            let peer = GreetingPeer::new(expected_fulfillment(&shared_secret));
            let connector = first_hop(peer.clone(), receiver.clone(), ledger.clone());
            let response = connector
                .handle_prepare(Prepare {
                    amount: PACKET_AMOUNT,
                    ..sealed
                })
                .await;

            assert!(matches!(response, PacketResponse::Fulfill(_)));
            let seen = peer.seen();
            assert_eq!(
                seen.len(),
                1,
                "covered proactively -- no retry needed this time"
            );
            let covering = seen[0]
                .as_ref()
                .expect("the proactive claim covers it")
                .clone();
            assert!(
                covering.nonce > burned.nonce,
                "a nonce is never reissued: {} must exceed the burned {}",
                covering.nonce,
                burned.nonce
            );
            assert_eq!(
                covering.cumulative_amount,
                11_000 + PACKET_AMOUNT,
                "priced off the receiver's watermark advanced by this node's own forwarded \
                 value, so the failed attempts cost nothing but two nonces"
            );
        }

        /// Interop (issue #881's own acceptance): a receiver that has not
        /// taken #868/#880 and demands no payment at all still gets the
        /// default covering claim -- attaching one is harmless to a
        /// receiver that never asked for it, and this hop is configured
        /// for client-role covering regardless of what any one receiver
        /// currently enforces.
        #[tokio::test]
        async fn a_peer_that_demands_no_payment_still_gets_the_default_covering_claim() {
            let (sealed, shared_secret) = sealed_prepare(b"hello");
            let mut peer = GreetingPeer::new(expected_fulfillment(&shared_secret));
            Arc::get_mut(&mut peer).expect("sole owner").greets = false;
            let receiver = StandingWatermark::at(0, 0);
            let (_dir, ledger) = ledger();
            let connector = first_hop(peer.clone(), receiver.clone(), ledger.clone());

            let response = connector
                .handle_prepare(Prepare {
                    amount: PACKET_AMOUNT,
                    ..sealed
                })
                .await;

            assert!(matches!(response, PacketResponse::Fulfill(_)));
            let seen = peer.seen();
            assert_eq!(seen.len(), 1, "one forward, no retry needed");
            let claim = seen[0]
                .as_ref()
                .expect("covered by default even though the peer never demanded it");
            assert_eq!(claim.cumulative_amount, PACKET_AMOUNT);
            assert_eq!(ledger.issued_nonce("second-hop"), 1);
            assert_eq!(receiver.asked.load(Ordering::SeqCst), 1);
            assert!(
                connector.claims.pending_claim("second-hop").is_none(),
                "a packet covered by the client-role claim must not also arm a peer-role one"
            );
        }

        /// **A peering that cannot cover a forward does not forward it**
        /// (issue #1145). This is the deleted fallback, asserted as its
        /// own refusal: a hop with no client-role config used to fall
        /// through to `ClaimBook::pending_claim` and carry the packet on
        /// ADR 0004's postpay convention -- the model ADR 0042 exists to
        /// retire, and the reason "a connector covers every PREPARE it
        /// sends" was a record rather than a fact. There is now no arm
        /// that reaches the wire uncovered, so the packet dies here and
        /// the peer never sees it.
        ///
        /// Note which peer this uses: one that demands nothing. Even a
        /// receiver perfectly happy to carry the packet for free does not
        /// get it, because the rule is about what this connector will
        /// send, not about what the far side will accept.
        #[tokio::test]
        async fn a_hop_with_no_client_role_configured_refuses_the_forward_rather_than_sending_it() {
            let (sealed, shared_secret) = sealed_prepare(b"hello");
            let mut peer = GreetingPeer::new(expected_fulfillment(&shared_secret));
            Arc::get_mut(&mut peer).expect("sole owner").greets = false;
            let connector = Connector::new(
                vec![],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                Arc::new(FakeAppClient::new()),
                peer.clone(),
                test_clock(),
            )
            .with_signer(Arc::new(LocalSigner::generate("settlement-key")))
            .with_channel_domain(test_channel_id(1), test_channel_domain())
            .expect("test_channel_id(1) is a valid on-chain channel id");
            // Deliberately no `with_outbound_client_ledger` /
            // `with_outbound_client_hop` -- the `[[pay_channels]]` row
            // `Config::load` now refuses a routed peering for being
            // without.

            let response = connector
                .handle_prepare(Prepare {
                    amount: PACKET_AMOUNT,
                    ..sealed
                })
                .await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "T00");
                    assert!(
                        reject.message.contains("second-hop")
                            && reject.message.contains("pay_channels"),
                        "the refusal must name the hop and what is missing: {}",
                        reject.message
                    );
                }
                other => panic!("expected a local refusal, got {other:?}"),
            }
            assert!(
                peer.seen().is_empty(),
                "nothing may reach the peer when nothing could pay for it"
            );
            assert!(
                connector.claims.pending_claim("second-hop").is_none(),
                "and nothing is owed afterwards either -- the postpay ledger is not armed by \
                 anything any more"
            );
        }

        /// The fee is subtracted BEFORE the claim is minted: this hop
        /// covers what it forwards, not what arrived, so it keeps exactly
        /// its own fee (ADR 0010's earnings rule, ADR 0042's symmetric
        /// figure). The property the deleted postpay test
        /// `forwarding_to_a_peer_subtracts_that_relations_flat_fee` used to
        /// read off the peer ledger's armed claim, read off the covering
        /// claim that actually rides the packet instead.
        #[tokio::test]
        async fn a_covered_forward_covers_the_amount_after_this_hops_own_fee() {
            const FEE: u64 = 7;
            let (sealed, shared_secret) = sealed_prepare(b"hello");
            let peer = GreetingPeer::new(expected_fulfillment(&shared_secret));
            let receiver = StandingWatermark::at(4, 4_000);
            let (_dir, ledger) = ledger();
            let connector = first_hop_charging(peer.clone(), receiver, ledger, FEE);

            let response = connector
                .handle_prepare(Prepare {
                    amount: PACKET_AMOUNT,
                    ..sealed
                })
                .await;

            assert!(matches!(response, PacketResponse::Fulfill(_)));
            let seen = peer.seen();
            assert_eq!(seen.len(), 1, "covered on the first attempt");
            let claim = seen[0].as_ref().expect("covered before it was sent");
            assert_eq!(
                claim.cumulative_amount,
                4_000 + PACKET_AMOUNT - FEE,
                "the covering claim advances by what this hop FORWARDS, so the difference \
                 between what it collected and what it covered is exactly its fee"
            );
        }

        /// A receiver that will not report its watermark leaves nothing
        /// safe to sign, so the packet FAILS outright (issue #881's own
        /// acceptance) -- it is never emitted uncovered as a fallback, and
        /// never even reaches the peer: there is nothing honest to send.
        #[tokio::test]
        async fn a_receiver_that_will_not_report_its_watermark_fails_without_forwarding() {
            let (sealed, shared_secret) = sealed_prepare(b"hello");
            let peer = GreetingPeer::new(expected_fulfillment(&shared_secret));
            let receiver = StandingWatermark::at(0, 0);
            receiver.silent.store(true, Ordering::SeqCst);
            let (_dir, ledger) = ledger();
            let connector = first_hop(peer.clone(), receiver, ledger.clone());

            let response = connector
                .handle_prepare(Prepare {
                    amount: PACKET_AMOUNT,
                    ..sealed
                })
                .await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "T00");
                    assert!(
                        reject.message.contains("second-hop"),
                        "the refusal must name the next hop: {}",
                        reject.message
                    );
                }
                other => panic!("expected a local refusal, got {other:?}"),
            }
            assert_eq!(
                peer.seen().len(),
                0,
                "no watermark, no claim, and nothing sent uncovered"
            );
            assert_eq!(
                ledger.issued_nonce("second-hop"),
                0,
                "a claim that was never signed must not have consumed a nonce"
            );
        }

        /// The domain a covering claim is signed under comes from the
        /// RECEIVER's greeting (issue #873's `EvmDomain` doc), and a
        /// greeting naming no EVM settlement leaves this node nothing it can
        /// sign under -- refused rather than defaulted.
        #[test]
        fn the_signing_domain_is_read_from_the_greeting_and_refused_when_absent() {
            let terms = quoted_terms();
            let domain = test_channel_domain();
            assert_eq!(
                EvmDomain::from_greeting(&terms),
                Some(EvmDomain {
                    chain_id: domain.chain_id,
                    token_network: domain.token_network_address,
                })
            );

            let mut settlement_less = quoted_terms();
            settlement_less.accepts[0].extra.settlement = None;
            assert_eq!(EvmDomain::from_greeting(&settlement_less), None);
        }

        // -----------------------------------------------------------
        // Solana (issue #1146): the arm `cover_forward` did not have.
        // -----------------------------------------------------------

        /// The Solana channel account this node pays its next hop from.
        const SOLANA_ACCOUNT: [u8; 32] = [0x6a; 32];
        /// `[settlement.solana] program_id` -- what ADR 0053 signs into
        /// every claim on that channel.
        const SOLANA_PROGRAM: [u8; 32] = [0x2c; 32];

        fn solana_base58(bytes: &[u8; 32]) -> String {
            bs58::encode(bytes).into_string()
        }

        /// [`first_hop`]'s Solana twin: the same node, the same routes, the
        /// same ledger -- with the client-role hop bound to a Solana channel
        /// account instead of an EVM channel id, and an ed25519 settlement
        /// key to sign under.
        ///
        /// Before #1146 this connector could not be built at all:
        /// `with_outbound_client_hop` took an `EvmDomain` and nothing else,
        /// so the b-c leg of `local/mixed-chain` had no way to be covered
        /// and rode ADR 0004's postpay convention forever.
        fn first_hop_on_solana(
            peer: Arc<GreetingPeer>,
            receiver: Arc<StandingWatermark>,
            ledger: Arc<OutboundClientLedger>,
            claim_signer: Arc<dyn Ed25519Signer>,
        ) -> Connector {
            let account = solana_base58(&SOLANA_ACCOUNT);
            let program = solana_base58(&SOLANA_PROGRAM);
            // The counterparty this node's PEER book would judge inbound
            // claims on the same channel by -- the deployed shape, one
            // channel in both roles.
            let counterparty = solana_base58(
                &LocalEd25519Signer::from_secret_bytes([44u8; 32])
                    .unwrap()
                    .public_key(),
            );
            Connector::new(
                vec![],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                Arc::new(FakeAppClient::new()),
                peer,
                test_clock(),
            )
            .with_solana_signer(claim_signer)
            .with_solana_channel(account.clone(), &counterparty, &program)
            .expect("a well-formed channel account")
            .with_outbound_client_ledger(ledger)
            .with_solana_outbound_client_hop("second-hop", account, &program, receiver)
            .expect("a well-formed channel account")
        }

        /// The Solana half of
        /// [`n_consecutive_forwards_over_a_healthy_link_each_carry_their_own_covering_claim`]:
        /// a forward to a Solana peering is covered BEFORE it is sent, with
        /// an ed25519 claim that verifies under the channel's own settlement
        /// program.
        ///
        /// The three things this pins, each of which was impossible before
        /// #1146: the claim exists at all on the first attempt; it is a
        /// `ClaimSignature::Solana` rather than an EVM signature wearing a
        /// Solana channel's name; and the watermark ask that priced it was
        /// made as a Solana ask, not an EVM one that happened to be
        /// answered.
        #[tokio::test]
        async fn a_forward_to_a_solana_peering_is_covered_before_it_is_sent() {
            let (sealed, shared_secret) = sealed_prepare(b"hello");
            let peer = GreetingPeer::new(expected_fulfillment(&shared_secret));
            let receiver = StandingWatermark::at(7, 7_000);
            let (_dir, ledger) = ledger();
            let key = LocalEd25519Signer::from_secret_bytes([13u8; 32]).expect("a settlement key");
            let public_key = key.public_key();
            let connector = first_hop_on_solana(
                peer.clone(),
                Arc::clone(&receiver),
                ledger.clone(),
                Arc::new(key),
            );

            let response = connector
                .handle_prepare(Prepare {
                    amount: PACKET_AMOUNT,
                    ..sealed
                })
                .await;
            assert!(
                matches!(response, PacketResponse::Fulfill(_)),
                "a proactively covered Solana forward must fulfil, got {response:?}"
            );

            let seen = peer.seen();
            assert_eq!(seen.len(), 1, "covered on the first attempt, so no retry");
            let claim = seen[0]
                .as_ref()
                .expect("the packet must not have been emitted uncovered");
            assert_eq!(claim.channel_id, solana_base58(&SOLANA_ACCOUNT));
            assert_eq!(claim.nonce, 8);
            assert_eq!(claim.cumulative_amount, 7_000 + PACKET_AMOUNT);

            let ClaimSignature::Solana(signature) = claim.signature else {
                panic!("a Solana hop must mint a Solana claim, not an EVM one");
            };
            assert!(
                connector_signer::verify_solana_balance_proof(
                    &SOLANA_PROGRAM,
                    &SOLANA_ACCOUNT,
                    claim.nonce,
                    claim.cumulative_amount,
                    &signature,
                    &public_key,
                ),
                "the far gate must verify this claim under the channel's own settlement program"
            );

            assert_eq!(
                *receiver
                    .asked_about
                    .lock()
                    .expect("asked-about lock poisoned"),
                vec![ClaimStateDomain::Solana],
                "the watermark ask must be the Solana one -- an EVM challenge would be answered \
                 `unverified` by a real payee, and this node would have signed nothing"
            );
        }

        /// A Solana hop on a node with no ed25519 settlement key cannot sign,
        /// and the packet FAILS naming the reason rather than going out
        /// uncovered. The same rule the EVM arm holds, and the reason
        /// `cover_forward` has no "send it anyway" branch on either chain.
        #[tokio::test]
        async fn a_solana_hop_with_no_solana_signer_fails_the_packet_rather_than_sending_it() {
            let (sealed, shared_secret) = sealed_prepare(b"hello");
            let peer = GreetingPeer::new(expected_fulfillment(&shared_secret));
            let receiver = StandingWatermark::at(0, 0);
            let (_dir, ledger) = ledger();
            let account = solana_base58(&SOLANA_ACCOUNT);
            let program = solana_base58(&SOLANA_PROGRAM);
            let connector = Connector::new(
                vec![],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                Arc::new(FakeAppClient::new()),
                peer.clone(),
                test_clock(),
            )
            // Deliberately `with_signer` and NOT `with_solana_signer`: an
            // EVM key is no use to a Solana channel, and silently reaching
            // for it would sign a claim on the wrong curve.
            .with_signer(Arc::new(LocalSigner::generate("evm-only")))
            .with_outbound_client_ledger(ledger.clone())
            .with_solana_outbound_client_hop("second-hop", account, &program, receiver)
            .expect("a well-formed channel account");

            let response = connector
                .handle_prepare(Prepare {
                    amount: PACKET_AMOUNT,
                    ..sealed
                })
                .await;
            match response {
                PacketResponse::Reject(reject) => assert!(
                    reject.message.contains("Solana settlement signer"),
                    "the reject must name what is missing, got: {}",
                    reject.message
                ),
                other => panic!("expected a local refusal, got {other:?}"),
            }
            assert_eq!(
                peer.seen().len(),
                0,
                "nothing may be sent when no claim could be signed"
            );
            assert_eq!(ledger.issued_nonce("second-hop"), 0);
        }
    }

    /// Redeeming the latest claim and cooperative close (issue #425), all
    /// against `connector_settlement::InMemorySettlementBackend` -- the
    /// fake this workspace's own tests use for anything not specific to a
    /// real chain (ADR 0007). The real-chain requirement itself lives in
    /// `connector-operator`'s operator-surface test and
    /// `connector-settlement-evm`'s own integration tests.
    /// Issue #630's review finding: a node settling on more than one chain
    /// holds every configured backend, and each operator channel op reaches
    /// the backend its chain (or its channel id's namespace) names -- never
    /// whichever backend happened to attach last. Driven with a
    /// chain-tagged fake (ADR 0007) whose every answer names which slot it
    /// was registered under, so a misroute is a failed string assertion
    /// rather than an invisible wrong-chain transaction; the same routing
    /// against real chains is `connector-cli`'s
    /// `a_both_chains_config_attaches_and_routes_both_backends`.
    mod settlement_routing {
        use super::*;
        use connector_settlement::{ChannelState, InMemorySettlementBackend};

        /// A valid base58 32-byte account address -- the Solana channel-id
        /// namespace's shape.
        const SOLANA_CHANNEL: &str = "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi";
        /// A valid `0x`-prefixed 64-hex-character `bytes32` -- the EVM
        /// channel-id namespace's shape.
        const EVM_CHANNEL: &str =
            "0xabababababababababababababababababababababababababababababababab";

        /// Answers every port method with an error naming the chain slot
        /// it was registered under, so a test can assert exactly which
        /// backend an operation reached.
        struct TaggedBackend(&'static str);

        #[async_trait]
        impl SettlementBackend for TaggedBackend {
            async fn open(
                &self,
                _counterparty: Vec<u8>,
                _settlement_timeout: Duration,
            ) -> Result<ChannelId, SettlementError> {
                Err(SettlementError::Backend(format!("{}: open", self.0)))
            }

            async fn fund(
                &self,
                _channel: &ChannelId,
                _amount: u128,
            ) -> Result<ChannelState, SettlementError> {
                Err(SettlementError::Backend(format!("{}: fund", self.0)))
            }

            async fn redeem(
                &self,
                _channel: &ChannelId,
                _claim: Claim,
            ) -> Result<ChannelState, SettlementError> {
                Err(SettlementError::Backend(format!("{}: redeem", self.0)))
            }

            async fn close(&self, _channel: &ChannelId) -> Result<ChannelState, SettlementError> {
                Err(SettlementError::Backend(format!("{}: close", self.0)))
            }

            async fn settle(&self, _channel: &ChannelId) -> Result<ChannelState, SettlementError> {
                Err(SettlementError::Backend(format!("{}: settle", self.0)))
            }

            async fn channel_state(
                &self,
                _channel: &ChannelId,
            ) -> Result<ChannelState, SettlementError> {
                Err(SettlementError::Backend(format!(
                    "{}: channel_state",
                    self.0
                )))
            }

            async fn live_channel_with(
                &self,
                _counterparty: Vec<u8>,
            ) -> Result<Option<ChannelId>, SettlementError> {
                Err(SettlementError::Backend(format!(
                    "{}: live_channel_with",
                    self.0
                )))
            }
        }

        fn bare_connector() -> Connector {
            Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
        }

        fn both_chains_connector() -> Connector {
            bare_connector()
                .with_settlement(SettlementChain::Evm, Arc::new(TaggedBackend("evm")))
                .with_settlement(SettlementChain::Solana, Arc::new(TaggedBackend("solana")))
        }

        fn backend_reached(result: Result<ChannelView, ChannelOperationError>) -> String {
            match result {
                Err(ChannelOperationError::Settlement(SettlementError::Backend(tag))) => tag,
                other => panic!("expected the tagged backend's own answer, got {other:?}"),
            }
        }

        /// The regression test for the last-one-wins slot: with EVM
        /// attached first and Solana second (config resolution order), an
        /// op on an EVM-namespace channel id must still reach the EVM
        /// backend -- and the Solana twin its own.
        #[tokio::test]
        async fn a_channel_op_on_a_both_chains_node_reaches_the_ids_own_backend() {
            let connector = both_chains_connector();

            assert_eq!(
                backend_reached(connector.fund_channel(EVM_CHANNEL, 5).await),
                "evm: fund"
            );
            assert_eq!(
                backend_reached(connector.close_channel(SOLANA_CHANNEL).await),
                "solana: close"
            );
            // A bare (un-`0x`-prefixed) hex id is the same EVM namespace,
            // exactly as `EvmSettlementBackend::parse_channel_id` accepts.
            assert_eq!(
                backend_reached(
                    connector
                        .fund_channel(EVM_CHANNEL.trim_start_matches("0x"), 5)
                        .await
                ),
                "evm: fund"
            );
        }

        /// Opening names its chain explicitly; on a node with several
        /// backends, declining to name one is refused rather than
        /// silently resolved to whichever backend attached last.
        #[tokio::test]
        async fn opening_routes_by_the_named_chain_and_refuses_ambiguity() {
            let connector = both_chains_connector();

            assert_eq!(
                backend_reached(
                    connector
                        .open_channel(
                            Some(SettlementChain::Solana),
                            b"peer".to_vec(),
                            Duration::seconds(60)
                        )
                        .await
                ),
                "solana: open"
            );
            assert!(matches!(
                connector
                    .open_channel(None, b"peer".to_vec(), Duration::seconds(60))
                    .await,
                Err(ChannelOperationError::AmbiguousSettlementChain)
            ));
        }

        /// A single-backend node keeps the port's "ids are opaque"
        /// promise: every id -- including one shaped like nothing any real
        /// chain assigns, e.g. the in-memory backend's decimal counters --
        /// routes to the one backend there is, and an unnamed chain on
        /// `open_channel` denotes it unambiguously.
        #[tokio::test]
        async fn a_single_backend_node_routes_every_id_to_it() {
            let connector = bare_connector()
                .with_settlement(SettlementChain::Evm, Arc::new(TaggedBackend("evm")));

            assert_eq!(
                backend_reached(connector.fund_channel("7", 5).await),
                "evm: fund"
            );
            assert_eq!(
                backend_reached(connector.fund_channel(SOLANA_CHANNEL, 5).await),
                "evm: fund"
            );
            assert_eq!(
                backend_reached(
                    connector
                        .open_channel(None, b"peer".to_vec(), Duration::seconds(60))
                        .await
                ),
                "evm: open"
            );
        }

        /// A chain this node holds no backend for is refused naming the
        /// actual gap -- "no solana settlement backend", not "no
        /// settlement backend is configured for this node".
        #[tokio::test]
        async fn a_chain_with_no_backend_is_refused_naming_the_gap() {
            let connector = bare_connector()
                .with_settlement(SettlementChain::Evm, Arc::new(TaggedBackend("evm")));

            let result = connector
                .open_channel(
                    Some(SettlementChain::Solana),
                    b"peer".to_vec(),
                    Duration::seconds(60),
                )
                .await;
            assert!(matches!(
                result,
                Err(ChannelOperationError::NoSettlementBackendForChain(
                    SettlementChain::Solana
                ))
            ));
        }

        /// An id in no known namespace, on a node where the namespace is
        /// what routes, is "no channel to operate on" -- the same answer
        /// every backend already gives a malformed id.
        #[tokio::test]
        async fn an_id_in_no_namespace_is_not_found_on_a_both_chains_node() {
            let connector = both_chains_connector();

            let result = connector.fund_channel("not-any-chains-shape", 5).await;
            assert!(matches!(
                result,
                Err(ChannelOperationError::Settlement(
                    SettlementError::ChannelNotFound(_)
                ))
            ));
        }

        /// `channels()` reports each opened channel from the backend that
        /// opened it: two in-memory backends both assign the id "0", so a
        /// misrouted report would answer with the other chain's
        /// counterparty.
        #[tokio::test]
        async fn channels_reports_each_channel_from_its_own_chains_backend() {
            let connector = bare_connector()
                .with_settlement(
                    SettlementChain::Evm,
                    Arc::new(InMemorySettlementBackend::new()),
                )
                .with_settlement(
                    SettlementChain::Solana,
                    Arc::new(InMemorySettlementBackend::new()),
                );

            connector
                .open_channel(
                    Some(SettlementChain::Evm),
                    b"evm-peer".to_vec(),
                    Duration::seconds(60),
                )
                .await
                .expect("open on the EVM backend");
            connector
                .open_channel(
                    Some(SettlementChain::Solana),
                    b"sol-peer".to_vec(),
                    Duration::seconds(60),
                )
                .await
                .expect("open on the Solana backend");

            let views = connector.channels().await;
            let counterparties: Vec<&str> = views
                .iter()
                .map(|view| view.counterparty.as_str())
                .collect();
            assert_eq!(views.len(), 2);
            assert!(
                counterparties.contains(&to_hex(b"evm-peer").as_str())
                    && counterparties.contains(&to_hex(b"sol-peer").as_str()),
                "each channel must be reported by the backend that opened it: {counterparties:?}"
            );
        }

        /// `0x`-prefixed lowercase hex -- [`ChannelView`]'s own
        /// counterparty encoding.
        fn to_hex(bytes: &[u8]) -> String {
            let mut hex = String::from("0x");
            for byte in bytes {
                hex.push_str(&format!("{byte:02x}"));
            }
            hex
        }

        /// Issue #1218: `channel_view` reports on a channel by id alone,
        /// unlike `channels()` -- which only ever lists
        /// [`Self::known_channels`], the channels this node itself opened.
        /// A client channel, opened by its counterparty and merely
        /// recognized here, is exactly the case this exists for: the
        /// channel below is opened straight through the backend, never
        /// through `Connector::open_channel`, so it is provably absent
        /// from `known_channels`.
        #[tokio::test]
        async fn channel_view_reports_a_channel_this_node_never_itself_opened() {
            let settlement = Arc::new(InMemorySettlementBackend::new());
            let channel_id = settlement
                .open(b"counterparty".to_vec(), Duration::seconds(3600))
                .await
                .unwrap();
            let connector = bare_connector().with_settlement(SettlementChain::Evm, settlement);

            assert!(connector.channels().await.is_empty());

            let view = connector
                .channel_view(&channel_id.0)
                .await
                .expect("the backend knows this channel even though this node never opened it");
            assert_eq!(view.counterparty, to_hex(b"counterparty"));
        }

        /// A channel id no backend has ever heard of answers exactly like
        /// every other per-channel operation already does.
        #[tokio::test]
        async fn channel_view_of_an_unknown_channel_is_not_found() {
            let connector = bare_connector().with_settlement(
                SettlementChain::Evm,
                Arc::new(InMemorySettlementBackend::new()),
            );

            let result = connector.channel_view("no-such-channel").await;
            assert!(matches!(
                result,
                Err(ChannelOperationError::Settlement(
                    SettlementError::ChannelNotFound(_)
                ))
            ));
        }
    }

    mod redemption {
        use super::*;
        use connector_settlement::{ChannelStatus, InMemorySettlementBackend};
        use connector_signer::LocalSigner;

        fn connector_with_settlement(
            settlement: Arc<InMemorySettlementBackend>,
            peer_signer: &LocalSigner,
            channel_id: &str,
        ) -> Connector {
            Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_settlement(SettlementChain::Evm, settlement)
            .with_channel_verification_key(
                channel_id,
                derive_evm_address(&peer_signer.public_key().unwrap()),
            )
            .with_channel_domain(channel_id, test_channel_domain())
            .unwrap()
        }

        /// `channel_id` here is `InMemorySettlementBackend::open`'s own
        /// generated id -- a plain decimal counter (issue #575), which
        /// `crate::claim::parse_channel_id` accepts as the same on-chain
        /// value that decimal numeral names.
        fn sign_claim(
            signer: &LocalSigner,
            channel_id: &str,
            nonce: u64,
            cumulative_amount: u64,
        ) -> WireClaim {
            let on_chain_id = crate::claim::parse_channel_id(channel_id).unwrap();
            let proof = crate::claim::evm_proof(
                on_chain_id,
                test_channel_domain(),
                nonce,
                cumulative_amount,
            );
            WireClaim {
                channel_id: channel_id.to_string(),
                nonce,
                cumulative_amount,
                signature: crate::claim::ClaimSignature::Evm(
                    signer
                        .sign(&connector_signer::evm_balance_proof_digest(&proof))
                        .unwrap(),
                ),
            }
        }

        #[tokio::test]
        async fn redeeming_with_no_claim_ever_accepted_is_refused() {
            let settlement = Arc::new(InMemorySettlementBackend::new());
            let channel_id = settlement
                .open(b"peer".to_vec(), Duration::seconds(3600))
                .await
                .unwrap();
            let peer_signer = LocalSigner::generate("peer-key");
            let connector = connector_with_settlement(settlement, &peer_signer, &channel_id.0);

            let result = connector.redeem_latest_claim(&channel_id.0).await;

            assert_eq!(result, Err(ChannelOperationError::NoClaimToRedeem));
        }

        /// Issue #1218: `peer_inbound_claim` is the read half
        /// `redeem_latest_claim`/`cooperative_close` already use
        /// internally, exposed so a caller holding a second book (the
        /// client edge's own) can tell whether the peer book already
        /// answers for a channel before falling back to its own.
        #[tokio::test]
        async fn peer_inbound_claim_answers_none_until_a_claim_is_accepted_then_the_latest() {
            let settlement = Arc::new(InMemorySettlementBackend::new());
            let channel_id = settlement
                .open(b"peer".to_vec(), Duration::seconds(3600))
                .await
                .unwrap();
            settlement
                .fund_counterparty(&channel_id, 1_000)
                .await
                .unwrap();
            let peer_signer = LocalSigner::generate("peer-key");
            let connector =
                connector_with_settlement(settlement.clone(), &peer_signer, &channel_id.0);

            assert_eq!(connector.peer_inbound_claim(&channel_id.0), None);

            assert_eq!(
                connector.handle_peer_claim(sign_claim(&peer_signer, &channel_id.0, 1, 100)),
                ClaimAckOutcome::Accepted
            );
            assert_eq!(
                connector.handle_peer_claim(sign_claim(&peer_signer, &channel_id.0, 2, 400)),
                ClaimAckOutcome::Accepted
            );

            let claim = connector
                .peer_inbound_claim(&channel_id.0)
                .expect("a claim has been accepted");
            assert_eq!(claim.nonce, 2);
            assert_eq!(claim.cumulative_amount, 400);
        }

        #[tokio::test]
        async fn redeeming_submits_only_the_highest_nonce_claim_ever_accepted() {
            let settlement = Arc::new(InMemorySettlementBackend::new());
            let channel_id = settlement
                .open(b"peer".to_vec(), Duration::seconds(3600))
                .await
                .unwrap();
            // The peer signs the claims redeemed below, so it is the peer's
            // own deposit that backs them (issue #1118).
            settlement
                .fund_counterparty(&channel_id, 1_000)
                .await
                .unwrap();
            let peer_signer = LocalSigner::generate("peer-key");
            let connector =
                connector_with_settlement(settlement.clone(), &peer_signer, &channel_id.0);

            assert_eq!(
                connector.handle_peer_claim(sign_claim(&peer_signer, &channel_id.0, 1, 100)),
                ClaimAckOutcome::Accepted
            );
            assert_eq!(
                connector.handle_peer_claim(sign_claim(&peer_signer, &channel_id.0, 2, 400)),
                ClaimAckOutcome::Accepted
            );

            let view = connector.redeem_latest_claim(&channel_id.0).await.unwrap();

            // Never the superseded 100 -- only ever the latest.
            assert_eq!(view.redeemed, 400);
        }

        #[tokio::test]
        async fn a_redemption_the_backend_refuses_leaves_the_channel_untouched() {
            let settlement = Arc::new(InMemorySettlementBackend::new());
            let channel_id = settlement
                .open(b"peer".to_vec(), Duration::seconds(3600))
                .await
                .unwrap();
            // The peer signs the claims redeemed below, so it is the peer's
            // own deposit that backs them (issue #1118).
            settlement
                .fund_counterparty(&channel_id, 100)
                .await
                .unwrap();
            let peer_signer = LocalSigner::generate("peer-key");
            let connector =
                connector_with_settlement(settlement.clone(), &peer_signer, &channel_id.0);
            connector.handle_peer_claim(sign_claim(&peer_signer, &channel_id.0, 1, 500));

            let result = connector.redeem_latest_claim(&channel_id.0).await;

            assert_eq!(
                result,
                Err(ChannelOperationError::Settlement(
                    SettlementError::InsufficientChannelBalance {
                        requested: 500,
                        deposited: 100,
                    }
                ))
            );
            // Recoverable: a refused redemption never touches the channel's
            // real state, so there is nothing to roll back before retrying.
            let state = settlement.channel_state(&channel_id).await.unwrap();
            assert_eq!(state.redeemed, 0);
        }

        #[tokio::test]
        async fn cooperative_close_with_no_claim_ever_accepted_just_closes() {
            let settlement = Arc::new(InMemorySettlementBackend::new());
            let channel_id = settlement
                .open(b"peer".to_vec(), Duration::seconds(3600))
                .await
                .unwrap();
            let peer_signer = LocalSigner::generate("peer-key");
            let connector = connector_with_settlement(settlement, &peer_signer, &channel_id.0);

            let view = connector.cooperative_close(&channel_id.0).await.unwrap();

            assert_eq!(view.status, crate::operator_view::ChannelViewStatus::Closed);
        }

        #[tokio::test]
        async fn cooperative_close_redeems_the_latest_claim_before_closing() {
            let settlement = Arc::new(InMemorySettlementBackend::new());
            let channel_id = settlement
                .open(b"peer".to_vec(), Duration::seconds(3600))
                .await
                .unwrap();
            // The peer signs the claims redeemed below, so it is the peer's
            // own deposit that backs them (issue #1118).
            settlement
                .fund_counterparty(&channel_id, 1_000)
                .await
                .unwrap();
            let peer_signer = LocalSigner::generate("peer-key");
            let connector =
                connector_with_settlement(settlement.clone(), &peer_signer, &channel_id.0);
            connector.handle_peer_claim(sign_claim(&peer_signer, &channel_id.0, 1, 700));

            let view = connector.cooperative_close(&channel_id.0).await.unwrap();

            assert_eq!(view.redeemed, 700);
            assert_eq!(view.status, crate::operator_view::ChannelViewStatus::Closed);
        }

        #[tokio::test]
        async fn cooperative_close_still_closes_when_the_claim_was_already_redeemed() {
            let settlement = Arc::new(InMemorySettlementBackend::new());
            let channel_id = settlement
                .open(b"peer".to_vec(), Duration::seconds(3600))
                .await
                .unwrap();
            // The peer signs the claims redeemed below, so it is the peer's
            // own deposit that backs them (issue #1118).
            settlement
                .fund_counterparty(&channel_id, 1_000)
                .await
                .unwrap();
            let peer_signer = LocalSigner::generate("peer-key");
            let connector =
                connector_with_settlement(settlement.clone(), &peer_signer, &channel_id.0);
            connector.handle_peer_claim(sign_claim(&peer_signer, &channel_id.0, 1, 700));
            connector.redeem_latest_claim(&channel_id.0).await.unwrap();

            // The same claim is now stale on chain -- cooperative close
            // does not treat that as a reason to refuse closing.
            let view = connector.cooperative_close(&channel_id.0).await.unwrap();

            assert_eq!(view.status, crate::operator_view::ChannelViewStatus::Closed);
        }

        #[tokio::test]
        async fn a_cooperative_close_whose_redemption_fails_leaves_the_channel_open() {
            let settlement = Arc::new(InMemorySettlementBackend::new());
            let channel_id = settlement
                .open(b"peer".to_vec(), Duration::seconds(3600))
                .await
                .unwrap();
            // The peer signs the claims redeemed below, so it is the peer's
            // own deposit that backs them (issue #1118).
            settlement
                .fund_counterparty(&channel_id, 100)
                .await
                .unwrap();
            let peer_signer = LocalSigner::generate("peer-key");
            let connector =
                connector_with_settlement(settlement.clone(), &peer_signer, &channel_id.0);
            connector.handle_peer_claim(sign_claim(&peer_signer, &channel_id.0, 1, 500));

            let result = connector.cooperative_close(&channel_id.0).await;

            assert_eq!(
                result,
                Err(ChannelOperationError::Settlement(
                    SettlementError::InsufficientChannelBalance {
                        requested: 500,
                        deposited: 100,
                    }
                ))
            );
            let state = settlement.channel_state(&channel_id).await.unwrap();
            assert_eq!(state.status, ChannelStatus::Open);
        }
    }

    /// Issue #426, ADR 0011: every REJECT carries the running total of the
    /// fees of the hops it actually passed through, whatever reason it was
    /// rejected for.
    mod fee_accumulation {
        use super::*;

        /// Builds a chain hop_0 -> hop_1 -> ... -> hop_{fees.len()}, where
        /// hop_{fees.len()} has no route at all (rejects `F02`) and
        /// `fees[i]` is what hop_i charges forwarding to hop_{i+1}. Returns
        /// hop_0, the entry point, already wired to the rest of the chain
        /// via in-process peer transports.
        fn chain_of(fees: &[u64]) -> Connector {
            let terminal = Arc::new(Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ));

            let mut downstream = terminal;
            for &fee in fees.iter().skip(1).rev() {
                let mut transport = InProcessPeerTransport::new();
                transport.add_peer("next", downstream);
                downstream = Arc::new(covering(
                    Connector::new(
                        vec![],
                        vec![PeerRoute::new("g.example.app", "next")],
                        Arc::new(FakeAppClient::new()),
                        Arc::new(transport),
                        test_clock(),
                    )
                    .with_peer_fees([("next".to_string(), fee)]),
                    "next",
                ));
            }

            let mut entry_transport = InProcessPeerTransport::new();
            entry_transport.add_peer("next", downstream);
            covering(
                Connector::new(
                    vec![],
                    vec![PeerRoute::new("g.example.app", "next")],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(entry_transport),
                    test_clock(),
                )
                .with_peer_fees([("next".to_string(), fees[0])]),
                "next",
            )
        }

        #[tokio::test]
        async fn a_self_originated_reject_carries_zero_accumulated_cost() {
            let connector = connector_with(vec![], Arc::new(FakeAppClient::new()), test_clock());

            let response = connector
                .handle_prepare(prepare("g.nowhere", b"hello"))
                .await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F02");
                    assert_eq!(reject.accumulated_cost, 0);
                }
                other => panic!("expected a reject, got {other:?}"),
            }
        }

        #[tokio::test]
        async fn a_relayed_reject_gains_the_relaying_hops_fee() {
            let entry = chain_of(&[7]);

            let response = entry
                .handle_prepare(prepare_with_amount("g.example.app", 100))
                .await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F02");
                    assert_eq!(reject.accumulated_cost, 7);
                }
                other => panic!("expected a reject, got {other:?}"),
            }
        }

        #[tokio::test]
        async fn accumulated_cost_sums_across_every_successfully_forwarding_hop() {
            let entry = chain_of(&[7, 3, 11]);

            let response = entry
                .handle_prepare(prepare_with_amount("g.example.app", 1_000))
                .await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F02");
                    assert_eq!(reject.accumulated_cost, 21);
                }
                other => panic!("expected a reject, got {other:?}"),
            }
        }

        /// The hop that cannot reach its own next hop never actually
        /// forwarded the packet, so its own fee is never added -- only the
        /// hops before it, which genuinely reached their peer, add theirs
        /// (peer-semantics-pre-868.md §5.2: fee is added only when relaying a
        /// REJECT "received from its own next hop").
        #[tokio::test]
        async fn a_hop_that_cannot_reach_its_peer_does_not_add_its_own_fee() {
            // hop-0 (fee 7) -> hop-1 (fee 3), but hop-1's own peer route
            // names a peer its transport never registered -- hop-1 cannot
            // reach it at all.
            let hop1 = Arc::new(covering(
                Connector::new(
                    vec![],
                    vec![PeerRoute::new("g.example.app", "unregistered")],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_peer_fees([("unregistered".to_string(), 3)]),
                "unregistered",
            ));
            let mut hop0_transport = InProcessPeerTransport::new();
            hop0_transport.add_peer("hop-1", hop1);
            let hop0 = covering(
                Connector::new(
                    vec![],
                    vec![PeerRoute::new("g.example.app", "hop-1")],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(hop0_transport),
                    test_clock(),
                )
                .with_peer_fees([("hop-1".to_string(), 7)]),
                "hop-1",
            );

            let response = hop0
                .handle_prepare(prepare_with_amount("g.example.app", 1_000))
                .await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "T01");
                    // hop-0 reached hop-1 (adds its fee, 7); hop-1 never
                    // reached "unregistered" (adds nothing).
                    assert_eq!(reject.accumulated_cost, 7);
                }
                other => panic!("expected a reject, got {other:?}"),
            }
        }

        proptest::proptest! {
            /// Issue #426's own acceptance criterion: for any chain of hops
            /// that all successfully forward before the packet is finally
            /// rejected (no route at the far end), the accumulated cost
            /// reported to the original sender equals the sum of the fees
            /// of the hops actually traversed.
            #[test]
            fn accumulated_cost_equals_the_sum_of_the_fees_of_the_hops_traversed(
                fees in proptest::collection::vec(0u64..1_000, 1..6)
            ) {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    let entry = chain_of(&fees);

                    let response = entry
                        .handle_prepare(prepare_with_amount("g.example.app", 1_000_000))
                        .await;

                    match response {
                        PacketResponse::Reject(reject) => {
                            proptest::prop_assert_eq!(reject.code.as_str(), "F02");
                            proptest::prop_assert_eq!(reject.accumulated_cost, fees.iter().sum::<u64>());
                        }
                        other => return Err(proptest::test_runner::TestCaseError::fail(format!("expected a reject, got {other:?}"))),
                    }
                    Ok(())
                })?;
            }
        }
    }

    /// Issue #524's own acceptance criteria, exercised end to end through
    /// [`Connector::handle_prepare`] rather than only at
    /// `connector_signer::giftwrap`'s own unit level -- "demonstrated, not
    /// asserted" (the issue's own words for the forwarding-hop criterion).
    mod sealing {
        use super::*;
        use connector_signer::giftwrap::{looks_like_sealed_response, seal_request};
        use connector_signer::LocalSigner;

        /// AC1/AC3: a sender seals to the terminating connector's identity,
        /// and only that connector can open it -- a connector configured
        /// with a *different* identity cannot terminate a wrap addressed
        /// elsewhere, the same as any other hop that never held the right
        /// key. Distinguishes this from a merely-malformed envelope: the
        /// message names the wrap, not the envelope.
        #[tokio::test]
        async fn a_connector_with_a_different_identity_cannot_open_a_wrap_sealed_to_another_identity(
        ) {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"should never be reached"));
            let wrong_identity = Arc::new(LocalSigner::generate("not-the-intended-recipient"));
            let connector = Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(wrong_identity);
            // Sealed to `identity_signer()`, not `wrong_identity` above.
            let (sealed, _shared_secret) = sealed_prepare(b"hello");

            let response = connector.handle_prepare(sealed).await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F01");
                    assert!(reject.message.contains("gift wrap could not be opened"));
                    // Never reached: the wrap never opened, so there was
                    // nothing to deliver.
                }
                other => panic!("expected a reject, got {other:?}"),
            }
            assert!(app_client.deliveries().is_empty());
        }

        /// AC7: a wrap that cannot be opened at all rejects with a
        /// different message than one that opens cleanly but decodes to a
        /// malformed envelope -- two different Rust error types
        /// (`GiftWrapError` vs `EnvelopeError`) surfacing as two
        /// distinguishable reasons, not the same generic failure.
        #[tokio::test]
        async fn an_unopenable_wrap_is_distinguishable_from_one_that_opens_to_a_malformed_envelope()
        {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            let connector = connector_with(vec![route], app_client, test_clock());

            // Garbage bytes: not shaped like a gift wrap at all, so it never
            // opens.
            let unopenable = connector
                .handle_prepare(prepare_with_data(vec![0xff; 40]))
                .await;
            match unopenable {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F01");
                    assert!(reject.message.contains("gift wrap could not be opened"));
                }
                other => panic!("expected a reject, got {other:?}"),
            }

            // A wrap that opens cleanly (sealed to the right identity) but
            // whose plaintext, once decrypted, is not a valid envelope.
            let (malformed_envelope, _shared_secret) = seal_request(
                b"not a valid encoded envelope",
                &identity_signer().public_key().unwrap(),
            )
            .unwrap();
            let malformed = connector
                .handle_prepare(prepare_with_data(malformed_envelope))
                .await;
            match malformed {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F01");
                    assert!(reject.message.contains("envelope did not decode"));
                }
                other => panic!("expected a reject, got {other:?}"),
            }
        }

        /// AC2: not only a FULFILL, but a REJECT raised at the termination --
        /// here, the wrap opened cleanly (proving it was genuinely addressed
        /// to this connector) but the plaintext inside is not a valid
        /// envelope -- is sealed back with the request's own shared secret:
        /// it opens under that secret (proving only the intended sender, who
        /// holds it, could ever read it) and fails to open under any other.
        /// `reject.message` -- the human-readable reason -- rides
        /// unencrypted alongside, same as every other reject in this file;
        /// only `data` is sealed.
        #[tokio::test]
        async fn a_reject_raised_at_the_termination_is_sealed_with_the_requests_shared_secret() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"app said yes"));
            let connector = connector_with(vec![route], app_client, test_clock());
            let (malformed_envelope, shared_secret) = connector_signer::giftwrap::seal_request(
                b"not a valid encoded envelope",
                &identity_signer().public_key().unwrap(),
            )
            .unwrap();

            let response = connector
                .handle_prepare(prepare_with_data(malformed_envelope))
                .await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F01");
                    assert!(reject.message.contains("envelope did not decode"));
                    assert!(looks_like_sealed_response(&reject.data));
                    connector_signer::giftwrap::open_response(&shared_secret, &reject.data).expect(
                        "a reject raised at the termination opens with the request's own secret",
                    );
                    assert!(
                        connector_signer::giftwrap::open_response(&[0xffu8; 32], &reject.data)
                            .is_err()
                    );
                }
                other => panic!("expected a reject, got {other:?}"),
            }
        }

        /// AC4: a reject raised short of the termination -- here, no route
        /// at all -- carries no secret to seal with and is necessarily
        /// plaintext, and a sender can tell the two apart without needing
        /// to already know whether a secret exists: an unsealed reject's
        /// `data` is simply empty, never shaped like a sealed one.
        #[tokio::test]
        async fn a_reject_raised_short_of_the_termination_is_plaintext_and_distinguishable() {
            let connector = connector_with(vec![], Arc::new(FakeAppClient::new()), test_clock());

            let response = connector
                .handle_prepare(prepare("g.nowhere", b"hello"))
                .await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F02");
                    assert!(reject.data.is_empty());
                    assert!(!looks_like_sealed_response(&reject.data));
                }
                other => panic!("expected a reject, got {other:?}"),
            }
        }

        /// AC5: `accumulated_cost` -- read directly off the struct, never
        /// through `data` -- is untouched by sealing in either direction.
        /// A termination reject (sealed `data`) and a hop reject (plaintext
        /// `data`) both carry it the same way.
        #[tokio::test]
        async fn accumulated_cost_stays_outside_the_seal_on_a_termination_reject() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b""));
            let connector = connector_with(vec![route], app_client, test_clock());
            let (malformed_envelope, _shared_secret) = connector_signer::giftwrap::seal_request(
                b"not a valid encoded envelope",
                &identity_signer().public_key().unwrap(),
            )
            .unwrap();

            let response = connector
                .handle_prepare(prepare_with_data(malformed_envelope))
                .await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F01");
                    // This route is unpriced (`StaticRoute::new` defaults to
                    // 0, issue #545) -- the point here is that the field is
                    // readable and meaningful independent of whatever `data`
                    // carries, not that it is always zero; a priced route's
                    // own value is covered by
                    // `termination_pricing::an_undecodable_envelope_reject_carries_the_routes_price`.
                    assert_eq!(reject.accumulated_cost, 0);
                    assert!(looks_like_sealed_response(&reject.data));
                }
                other => panic!("expected a reject, got {other:?}"),
            }
        }
    }

    /// Issue #545: `accumulated_cost` on a reject this connector originates
    /// at a termination -- rather than relays from a peer -- carries that
    /// route's price, wiring up what #523 renamed but never connected.
    mod termination_pricing {
        use super::*;

        /// The wrap opened cleanly -- proving it was genuinely addressed and
        /// correctly encrypted to this connector's identity, i.e. the packet
        /// reached the termination -- but the plaintext inside is not a
        /// valid envelope. Still priced, unlike the wrap-couldn't-open case
        /// in `a_wrap_that_cannot_be_opened_still_carries_zero_on_a_priced_route`
        /// below.
        #[tokio::test]
        async fn an_undecodable_envelope_reject_carries_the_routes_price() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            let connector = connector_with(vec![route], app_client, test_clock());
            let (malformed_envelope, _shared_secret) = connector_signer::giftwrap::seal_request(
                b"not a valid encoded envelope",
                &identity_signer().public_key().unwrap(),
            )
            .unwrap();

            let response = connector
                .handle_prepare(prepare_with_data(malformed_envelope))
                .await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F01");
                    assert!(reject.message.contains("envelope did not decode"));
                    assert_eq!(reject.accumulated_cost, 25);
                }
                other => panic!("expected a reject, got {other:?}"),
            }
        }

        /// The wrap never opens at all -- unlike the case above, this never
        /// proves the packet was even addressed to this connector, so the
        /// packet never reaches the termination and the reject stays
        /// unpriced, exactly like `AppOutcome::Unreachable` below.
        #[tokio::test]
        async fn a_wrap_that_cannot_be_opened_still_carries_zero_on_a_priced_route() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            let connector = connector_with(vec![route], app_client, test_clock());

            // Garbage bytes: not shaped like a gift wrap at all, so it never
            // opens.
            let response = connector
                .handle_prepare(prepare_with_data(vec![0xff; 40]))
                .await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F01");
                    assert!(reject.message.contains("gift wrap could not be opened"));
                    assert_eq!(reject.accumulated_cost, 0);
                }
                other => panic!("expected a reject, got {other:?}"),
            }
        }

        /// AC3: `AppOutcome::Unreachable` never carries a price, even on a
        /// priced route -- the app was never actually reached to do the
        /// priced work, mirroring a forwarding hop that cannot reach its
        /// own peer.
        #[tokio::test]
        async fn an_unreachable_app_reject_still_carries_zero_on_a_priced_route() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
            // No `.respond()` registered: `FakeAppClient` defaults to
            // `AppOutcome::Unreachable`.
            let app_client = Arc::new(FakeAppClient::new());
            let connector = connector_with(vec![route], app_client, test_clock());
            let (sealed, _shared_secret) = sealed_prepare(b"hello");

            let response = connector.handle_prepare(sealed).await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "T01");
                    assert_eq!(reject.accumulated_cost, 0);
                }
                other => panic!("expected a reject, got {other:?}"),
            }
        }

        /// Issue #596: an envelope whose target attempts to escape the
        /// route's configured handler path is refused with a code distinct
        /// from both F01 (the envelope itself failed to decode) and F99 (a
        /// mismatched fulfilment) -- and, like `AppOutcome::Unreachable`,
        /// carries no price, since the app was never reached to do any of
        /// the priced work.
        #[tokio::test]
        async fn an_escaping_target_reject_is_distinguishable_and_carries_zero() {
            let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000/write", 25)
                .unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            let connector = connector_with(vec![route], app_client, test_clock());
            let (data, _shared_secret) =
                sealed_envelope_request_data_with_target("/admin", b"hello");

            let response = connector.handle_prepare(prepare_with_data(data)).await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F00");
                    assert_eq!(reject.accumulated_cost, 0);
                }
                other => panic!("expected a reject, got {other:?}"),
            }
        }

        /// The review's finding on issue #869's PR: the refusal probe must
        /// resolve the winning route the way the router does. With an app
        /// route on `g.example.app` and an active lease on the strictly
        /// longer `g.example.app.leased`, a packet to the leased prefix is
        /// *forwarded* -- its envelope is never opened at this hop -- so
        /// the probe must not predict an envelope-shape refusal off the
        /// outranked app route: that answer made the client edge skip
        /// claim admission and forward the packet unmetered.
        #[tokio::test]
        async fn the_refusal_probe_defers_to_a_longer_prefix_lease_that_wins_the_route() {
            let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000/write", 25)
                .unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            let connector = connector_with(vec![route], app_client, test_clock());
            let (data, _shared_secret) =
                sealed_envelope_request_data_with_target("/admin", b"hello");
            let to_leased_prefix = Prepare {
                destination: "g.example.app.leased".to_string(),
                ..prepare_with_data(data)
            };

            // Without the lease, the app route wins the leased prefix too,
            // and this escaping target is exactly what the probe reports.
            assert!(connector.envelope_target_would_be_refused(&to_leased_prefix));

            connector
                .upsert_leased_route("g.example.app.leased", "leased-peer", Duration::seconds(60))
                .unwrap();

            // With the lease active, this packet forwards -- nothing to
            // refuse, so nothing to ride free on.
            assert!(!connector.envelope_target_would_be_refused(&to_leased_prefix));

            // An equal-length lease changes nothing: the app route wins
            // that tie (issue #427, `RouteRank`), the packet terminates
            // here, and the probe still predicts the refusal.
            connector
                .upsert_leased_route("g.example.app", "leased-peer", Duration::seconds(60))
                .unwrap();
            let (data, _shared_secret) =
                sealed_envelope_request_data_with_target("/admin", b"hello");
            assert!(connector.envelope_target_would_be_refused(&prepare_with_data(data)));
        }

        /// AC4: a probe behind one forwarding hop, terminating at a priced
        /// route, reports the hop's fee plus the route's price as a single
        /// figure -- exercised end to end through two real `Connector`s
        /// joined by an in-process peer transport, not only at the
        /// termination itself.
        #[tokio::test]
        async fn a_relayed_reject_sums_the_hops_fee_and_the_terminated_routes_price() {
            let second_hop_route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
            let second_hop_app_client = Arc::new(FakeAppClient::new());
            second_hop_app_client.respond(second_hop_route.handler_url(), answered(b"irrelevant"));
            let second_hop = Arc::new(
                Connector::new(
                    vec![second_hop_route],
                    vec![],
                    second_hop_app_client,
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(identity_signer()),
            );
            let mut peer_transport = InProcessPeerTransport::new();
            peer_transport.add_peer("second-hop", second_hop);
            let first_hop = covering(
                Connector::new(
                    vec![],
                    vec![PeerRoute::new("g.example.app", "second-hop")],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(peer_transport),
                    test_clock(),
                )
                .with_peer_fees([("second-hop".to_string(), 7)]),
                "second-hop",
            );
            let (malformed_envelope, _shared_secret) = connector_signer::giftwrap::seal_request(
                b"not a valid encoded envelope",
                &identity_signer().public_key().unwrap(),
            )
            .unwrap();
            let packet = Prepare {
                amount: 100,
                ..prepare_with_data(malformed_envelope)
            };

            let response = first_hop.handle_prepare(packet).await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F01");
                    assert_eq!(reject.accumulated_cost, 7 + 25);
                }
                other => panic!("expected a reject, got {other:?}"),
            }
        }
    }

    /// Issue #752: a peer-role PREPARE reaching one of this connector's own
    /// priced terminated routes must itself carry enough value to cover
    /// that route's price, checked in `Connector::handle_peer_prepare`
    /// before the app is ever consulted -- closing the gap ADR 0028 named
    /// and left open (a connector whose priced terminated route was
    /// reached over the peer semantics served it for free).
    mod peer_role_termination_price {
        use super::*;

        /// A route priced at 25, reached over the peer semantics with a PREPARE
        /// carrying only 10, is refused before the app is ever called --
        /// unlike every other reject this connector originates for a
        /// packet that never reached its termination, this one is possible
        /// only because the connector consulted the route's price, so it
        /// gets its own dedicated proof that the app truly never saw it.
        #[tokio::test]
        async fn an_underpriced_peer_arrival_is_refused_before_the_app_is_reached() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"irrelevant"));
            let connector = connector_with(vec![route], app_client.clone(), test_clock());

            let (response, ack) = connector
                .handle_peer_prepare(
                    Some("upstream"),
                    prepare_with_amount("g.example.app", 10),
                    None,
                )
                .await;

            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F03");
                    assert_eq!(reject.accumulated_cost, 0);
                }
                other => panic!("expected an F03 reject, got {other:?}"),
            }
            assert_eq!(ack, ClaimAckOutcome::NotSent);
            assert!(app_client.deliveries().is_empty());
        }

        /// The boundary: a PREPARE carrying exactly the route's price is
        /// enough -- this is not a strict-greater-than check.
        #[tokio::test]
        async fn a_peer_arrival_that_exactly_covers_the_routes_price_is_delivered() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"irrelevant"));
            let connector = connector_with(vec![route], app_client.clone(), test_clock());

            let (response, _ack) = connector
                .handle_peer_prepare(
                    Some("upstream"),
                    prepare_with_amount("g.example.app", 25),
                    None,
                )
                .await;

            assert!(matches!(response, PacketResponse::Fulfill(_)));
            assert_eq!(app_client.deliveries().len(), 1);
        }

        /// Unlike a priced *forwarded* route at the client edge (ADR 0028's
        /// `F03` over-carry cap), a priced terminated route reached over the
        /// peer role has no upper bound -- this connector never forwards the
        /// excess anywhere, so nothing is lost by a peer that overpays it.
        #[tokio::test]
        async fn a_peer_arrival_that_overpays_the_routes_price_is_still_delivered() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"irrelevant"));
            let connector = connector_with(vec![route], app_client.clone(), test_clock());

            let (response, _ack) = connector
                .handle_peer_prepare(
                    Some("upstream"),
                    prepare_with_amount("g.example.app", 100),
                    None,
                )
                .await;

            assert!(matches!(response, PacketResponse::Fulfill(_)));
        }

        /// A route explicitly priced at zero (an operator's deliberate free
        /// termination, ADR 0020) is untouched by this check -- an operator
        /// who wrote `price = 0` still gets free carriage over the peer
        /// wire, exactly as over the client edge.
        #[tokio::test]
        async fn a_free_terminated_route_is_unaffected_by_the_price_check() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            assert_eq!(route.price(), Price::FREE);
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"irrelevant"));
            let connector = connector_with(vec![route], app_client.clone(), test_clock());

            let (response, _ack) = connector
                .handle_peer_prepare(
                    Some("upstream"),
                    prepare_with_amount("g.example.app", 0),
                    None,
                )
                .await;

            assert!(matches!(response, PacketResponse::Fulfill(_)));
        }

        /// This check is specific to the peer semantics. A destination reached
        /// through [`Connector::handle_prepare`] directly -- what the
        /// client edge itself calls, only after its own claim gate already
        /// charged `price` (issue #522) -- is never subject to it, since
        /// `handle_prepare` cannot tell whether `prepare.amount` reflects
        /// anything a client actually paid.
        #[tokio::test]
        async fn handle_prepare_itself_is_not_gated_by_the_peer_role_price_check() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"irrelevant"));
            let connector = connector_with(vec![route], app_client.clone(), test_clock());

            let response = connector
                .handle_prepare(prepare_with_amount("g.example.app", 10))
                .await;

            assert!(matches!(response, PacketResponse::Fulfill(_)));
        }
    }

    /// Issue #548, ADR 0011: `Connector::handle_probe`'s two gates, and
    /// what a probe past them is and is not allowed to reach.
    mod probing {
        use super::*;

        const CHANNEL: &str = "evm:0xchannel";

        /// The gate that made `handle_probe` unreachable in practice:
        /// nothing in a node's configuration names an unaffiliated client's
        /// channel, so before #548 the only way to satisfy it was a
        /// peer-role verification key -- and a gate no deployed node can
        /// pass is not a gate. A channel a claim has been seen on at this
        /// connector's own client edge now satisfies it.
        #[tokio::test]
        async fn a_probe_on_an_unrecognized_channel_is_denied_and_one_on_a_recognized_channel_is_not(
        ) {
            let connector = connector_with(vec![], Arc::new(FakeAppClient::new()), test_clock());

            let denied = connector
                .handle_probe(CHANNEL, prepare("g.somewhere.else", b"hello"))
                .await;
            assert_eq!(denied, Err(ProbeDenied::NoOpenChannel));

            connector.recognize_channel(CHANNEL);
            let admitted = connector
                .handle_probe(CHANNEL, prepare("g.somewhere.else", b"hello"))
                .await;
            assert!(matches!(admitted, Ok(PacketResponse::Reject(_))));
        }

        /// ADR 0011: probing traverses the network for free, so it is
        /// rate-limited per the identity that holds the channel.
        #[tokio::test]
        async fn a_recognized_channel_is_still_rate_limited() {
            let connector = connector_with(vec![], Arc::new(FakeAppClient::new()), test_clock())
                .with_probe_rate_limit(1, Duration::seconds(60));
            connector.recognize_channel(CHANNEL);

            let first = connector
                .handle_probe(CHANNEL, prepare("g.somewhere.else", b"hello"))
                .await;
            assert!(first.is_ok());

            let second = connector
                .handle_probe(CHANNEL, prepare("g.somewhere.else", b"hello"))
                .await;
            assert_eq!(second, Err(ProbeDenied::RateLimited));
        }

        /// Issue #1218: `GET /channels` needs to list a client channel
        /// this connector has only ever recognized, never opened --
        /// `recognized_channel_ids` is the one place that list is kept.
        /// Idempotent, matching `recognize_channel` itself.
        #[test]
        fn recognized_channel_ids_lists_every_recognized_channel_once() {
            let connector = connector_with(vec![], Arc::new(FakeAppClient::new()), test_clock());
            assert!(connector.recognized_channel_ids().is_empty());

            connector.recognize_channel(CHANNEL);
            connector.recognize_channel("some-other-channel");
            connector.recognize_channel(CHANNEL);

            let mut ids = connector.recognized_channel_ids();
            ids.sort();
            let mut expected = vec![CHANNEL.to_string(), "some-other-channel".to_string()];
            expected.sort();
            assert_eq!(ids, expected);
        }

        /// A probe to a route this connector terminates reports that
        /// route's price as one figure -- the whole path cost, since no hop
        /// was traversed to reach it -- and the app behind it is never
        /// asked to do the work. Free traversal is all ADR 0011 grants a
        /// probe; it does not also buy what ADR 0020 prices.
        #[tokio::test]
        async fn a_probe_to_a_priced_local_route_reports_the_price_without_delivering() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(
                route.handler_url(),
                answered(b"work the app should never do"),
            );
            let connector = connector_with(vec![route], app_client.clone(), test_clock());
            connector.recognize_channel(CHANNEL);
            let (probe, _shared_secret) = sealed_prepare(b"hello");

            let response = connector.handle_probe(CHANNEL, probe).await;

            match response {
                Ok(PacketResponse::Reject(reject)) => {
                    assert_eq!(reject.accumulated_cost, 25);
                }
                other => panic!("expected a priced reject, got {other:?}"),
            }
            assert_eq!(app_client.deliveries().len(), 0);
        }

        /// ADR 0065: a probe learns what the path costs **for a packet like
        /// the one it sent**, so two probes of different sizes to one
        /// schedule route come back with two figures.
        ///
        /// This is the half ADR 0011 was worried about -- a probe answering
        /// only for its own size. It is answered not here but on the
        /// greeting and the self-description, which publish the schedule
        /// itself, so one free read still covers every size. What this test
        /// pins is that the figure a probe does report is the true one for
        /// its own packet rather than the route's base.
        #[tokio::test]
        async fn a_probe_to_a_schedule_route_reports_the_cost_of_its_own_packet() {
            let route = StaticRoute::new_scheduled(
                "g.example.app",
                "http://localhost:4000",
                Price::scheduled(25, 4),
            )
            .unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(
                route.handler_url(),
                answered(b"work the app should never do"),
            );
            let connector = connector_with(vec![route], app_client.clone(), test_clock());
            connector.recognize_channel(CHANNEL);

            let (small, _) = sealed_prepare(b"hi");
            let (large, _) = sealed_prepare(&vec![b'x'; 4096]);
            let small_len = small.data.len();
            let large_len = large.data.len();

            let small_cost = match connector.handle_probe(CHANNEL, small).await {
                Ok(PacketResponse::Reject(reject)) => reject.accumulated_cost,
                other => panic!("expected a priced reject, got {other:?}"),
            };
            let large_cost = match connector.handle_probe(CHANNEL, large).await {
                Ok(PacketResponse::Reject(reject)) => reject.accumulated_cost,
                other => panic!("expected a priced reject, got {other:?}"),
            };

            let schedule = Price::scheduled(25, 4);
            assert_eq!(small_cost, schedule.charge(small_len));
            assert_eq!(large_cost, schedule.charge(large_len));
            assert!(
                large_cost > small_cost,
                "a bigger probe must be quoted a bigger figure: {small_cost} vs {large_cost}"
            );
            // And neither probe bought any of the app's work.
            assert_eq!(app_client.deliveries().len(), 0);
        }

        /// ADR 0029 read through ADR 0065: a peer arrival carrying enough for
        /// the route's **base** but not for the packet it actually brought is
        /// refused `F03`, and the app never sees it.
        ///
        /// The failure this rules out is the one a partial implementation
        /// would have: charging the base at the peer gate and the full
        /// schedule at the termination, so a large packet is admitted across
        /// the peering and then refused after the claim was already banked.
        #[tokio::test]
        async fn a_peer_arrival_covering_only_the_base_is_refused_for_a_large_packet() {
            let route = StaticRoute::new_scheduled(
                "g.example.app",
                "http://localhost:4000",
                Price::scheduled(100, 10),
            )
            .unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"never"));
            let connector = connector_with(vec![route], app_client.clone(), test_clock());

            let (large, _) = sealed_prepare(&vec![b'x'; 3000]);
            let charge = Price::scheduled(100, 10).charge(large.data.len());
            assert!(charge > 100, "the test packet must cost more than the base");

            // Carries the base exactly -- what a sender reading only
            // `extra.price` would send.
            let (response, _ack) = connector
                .handle_peer_prepare(
                    Some("upstream"),
                    Prepare {
                        amount: 100,
                        ..large.clone()
                    },
                    None,
                )
                .await;
            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F03");
                    assert!(
                        reject.message.contains(&charge.to_string()),
                        "the refusal must name what this packet costs, got: {}",
                        reject.message
                    );
                }
                other => panic!("expected F03, got {other:?}"),
            }
            assert_eq!(app_client.deliveries().len(), 0);

            // Carrying the packet's own charge is admitted and delivered.
            let (response, _ack) = connector
                .handle_peer_prepare(
                    Some("upstream"),
                    Prepare {
                        amount: charge,
                        ..large
                    },
                    None,
                )
                .await;
            assert!(matches!(response, PacketResponse::Fulfill(_)));
            assert_eq!(app_client.deliveries().len(), 1);
        }

        /// The same packet sent through the ordinary entry point still
        /// fulfils and still reaches the app: the rule above belongs to the
        /// probe ingress, not to routing, so ADR 0011's "probes are not a
        /// distinct packet type" holds for everything past the gate that a
        /// probe does traverse.
        #[tokio::test]
        async fn the_same_packet_through_handle_prepare_still_reaches_the_app() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"work the app does do"));
            let connector = connector_with(vec![route], app_client.clone(), test_clock());
            let (packet, _shared_secret) = sealed_prepare(b"hello");

            let response = connector.handle_prepare(packet).await;

            assert!(matches!(response, PacketResponse::Fulfill(_)));
            assert_eq!(app_client.deliveries().len(), 1);
        }

        /// A destination beyond this connector is routed by the ordinary
        /// routing table (ADR 0011), so a probe's reject sums the fees of
        /// the hops it actually reached.
        #[tokio::test]
        async fn a_probe_beyond_this_connector_accumulates_the_hops_it_traversed() {
            let connector = Connector::new(
                vec![],
                vec![PeerRoute::new("g.example", "unreachable-peer")],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_peer_fees([("unreachable-peer".to_string(), 7)]);
            connector.recognize_channel(CHANNEL);

            let response = connector
                .handle_probe(CHANNEL, prepare("g.example.remote", b"hello"))
                .await;

            match response {
                Ok(PacketResponse::Reject(reject)) => {
                    // The peer was never reached, so this hop adds nothing
                    // -- the figure is honest about what was traversed.
                    assert_eq!(reject.accumulated_cost, 0);
                }
                other => panic!("expected a reject, got {other:?}"),
            }
        }
    }

    /// Issue #452: before/after evidence for the leased-route lookup on
    /// the hot path. Not run by the normal gate -- `#[ignore]`d and meant
    /// to be run by hand, in release mode, so timing reflects real
    /// optimized-build cost rather than debug-build noise:
    ///
    /// ```text
    /// cargo test --release -p connector-runtime -- --ignored --nocapture bench_leased_route_lookup
    /// ```
    mod perf {
        use super::*;
        use std::time::Instant;

        /// A `Connector` whose leased-route table has `active_lease_count`
        /// active leases plus one that actually matches the packet this
        /// benchmark sends -- exercising exactly the per-packet cost this
        /// issue is about (`handle_prepare` walking every active leased
        /// route) regardless of how many of them happen to be irrelevant to
        /// the packet being routed.
        fn connector_with_leased_routes(active_lease_count: usize) -> Connector {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"delivered"));
            let connector = Connector::new(
                vec![route],
                vec![],
                app_client,
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(identity_signer());
            for i in 0..active_lease_count {
                connector
                    .upsert_leased_route(
                        format!("g.other-{i}.app"),
                        "unused-peer",
                        Duration::seconds(60),
                    )
                    .unwrap();
            }
            connector
        }

        /// Not a correctness assertion -- prints per-packet latency for a
        /// growing number of concurrently-active leased routes so a
        /// before/after comparison (checked out against this commit's
        /// parent, then against this commit) shows whether the fix removed
        /// the per-packet scaling this issue describes: the pre-fix path
        /// clones every active leased route (plus a second clone into
        /// `PeerRoute`) into a freshly allocated `Vec` on every single
        /// call, so its cost grows with the lease count; the post-fix path
        /// loads an `Arc`-swapped snapshot with no lock and no clone of the
        /// route data, so it should stay flat.
        #[test]
        #[ignore = "run manually for a before/after measurement, see module doc"]
        fn bench_leased_route_lookup() {
            let rt = tokio::runtime::Runtime::new().unwrap();
            const ITERATIONS: usize = 20_000;
            for &active_lease_count in &[0usize, 100, 1_000, 10_000] {
                let connector = connector_with_leased_routes(active_lease_count);
                let started = Instant::now();
                rt.block_on(async {
                    for _ in 0..ITERATIONS {
                        let response = connector
                            .handle_prepare(prepare("g.example.app", b"hello"))
                            .await;
                        assert!(matches!(response, PacketResponse::Fulfill(_)));
                    }
                });
                let elapsed = started.elapsed();
                println!(
                    "active_lease_count={active_lease_count:>6}  total={elapsed:?}  per_packet={:?}",
                    elapsed / ITERATIONS as u32
                );
            }
        }
    }

    /// Issue #884: the runtime-mutable, durable peer/route table.
    mod runtime_peer_route_table {
        use super::*;

        /// A peering as ADR 0058's `POST /peers` writes one: an endpoint
        /// to dial the counterparty on and a channel its claims are
        /// judged against, beside the operator's own fee.
        ///
        /// Every test below builds its peerings through this, because a
        /// peering with no channel bound to it is refused at write time
        /// (`PeerChannelUnbound`) -- the runtime twin of the load-time
        /// rule that a `[[peers]]` row needs a `[[peer_channels]]` row.
        /// The channel id varies with the fee only so two peerings in one
        /// test do not name the same channel.
        fn peering(fee: u64) -> RuntimePeering {
            RuntimePeering {
                fee,
                max_packet_amount: 0,
                endpoint: Some("https://peer.example/ilp".to_string()),
                edge_identity: Some("0x04ab".to_string()),
                client_edge_url: Some("https://peer.example/ilp".to_string()),
                channels: vec![RuntimePeerChannel::Evm {
                    channel_id: format!("0x{:064x}", fee + 1),
                    counterparty_key: "0x00000000000000000000000000000000000000aa".to_string(),
                    chain_id: 31337,
                    token_network: "0x00000000000000000000000000000000000000bb".to_string(),
                }],
            }
        }

        /// A round trip through the exact shape
        /// `forwards_a_packet_matching_a_peer_route_to_the_next_hop`
        /// exercises for a config-file peer route, except the peer and the
        /// route are both added at runtime over the operator surface
        /// instead of read from configuration -- proving "no change to
        /// how packets are matched" (issue #884's acceptance criterion):
        /// the same longest-prefix match and the same forwarding path
        /// carry a runtime-added row exactly as they would a config one.
        #[tokio::test]
        async fn a_runtime_peer_route_forwards_a_packet_to_the_next_hop() {
            let second_hop_route =
                StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let second_hop_app_client = Arc::new(FakeAppClient::new());
            second_hop_app_client.respond(
                second_hop_route.handler_url(),
                answered(b"delivered by the second hop"),
            );
            let second_hop = Arc::new(
                Connector::new(
                    vec![second_hop_route],
                    vec![],
                    second_hop_app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(identity_signer()),
            );
            let mut peer_transport = InProcessPeerTransport::new();
            peer_transport.add_peer("runtime-hop", second_hop);
            // A runtime peer route reaches `forward_via_peer_route`
            // without passing `Config::load` (issue #884), so it needs the
            // covering configuration for the hop it names just as a
            // configured route does.
            let first_hop = covering(
                Connector::new(
                    vec![],
                    vec![],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(peer_transport),
                    test_clock(),
                ),
                "runtime-hop",
            );
            first_hop
                .upsert_runtime_peer("runtime-hop", peering(0))
                .unwrap();
            first_hop
                .upsert_runtime_peer_route("g.example.app", "runtime-hop", Price::FREE)
                .unwrap();
            let (sealed, shared_secret) = sealed_prepare(b"hello");

            let response = first_hop.handle_prepare(sealed).await;

            match response {
                PacketResponse::Fulfill(fulfill) => {
                    assert_eq!(fulfill.fulfillment, expected_fulfillment(&shared_secret));
                    assert_eq!(
                        open_sealed_envelope(&shared_secret, &fulfill.data),
                        fulfill_envelope(b"delivered by the second hop")
                    );
                }
                other => panic!("expected a fulfill, got {other:?}"),
            }
            assert_eq!(second_hop_app_client.deliveries().len(), 1);
        }

        /// The client edge prices a runtime peer route exactly like a
        /// config one (ADR 0028) -- it is priced and durable, unlike a
        /// lease, so it belongs in `client_route`'s answer.
        #[test]
        fn client_route_prices_a_runtime_peer_route() {
            let connector = covering(
                Connector::new(
                    vec![],
                    vec![],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                ),
                "runtime-hop",
            );
            connector
                .upsert_runtime_peer("runtime-hop", peering(0))
                .unwrap();
            connector
                .upsert_runtime_peer_route("g.example.app", "runtime-hop", Price::flat(25))
                .unwrap();

            let facts = connector.client_route("g.example.app").unwrap();
            assert_eq!(facts.price, Price::flat(25));
            assert_eq!(facts.kind, ClientRouteKind::Forwarded);
        }

        /// The precedence rule (issue #884): a runtime write can never add,
        /// update or remove a peer id the config file already owns --
        /// config wins, and it wins by refusing the write outright rather
        /// than silently shadowing or being shadowed.
        #[test]
        fn a_runtime_peer_id_colliding_with_a_config_peer_id_is_refused() {
            let connector = Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_config_peer_ids(["apex-store".to_string()]);

            let error = connector
                .upsert_runtime_peer("apex-store", peering(0))
                .unwrap_err();
            assert!(matches!(error, PeerRouteTableError::OwnedByConfig(id) if id == "apex-store"));

            // Not removable at runtime either -- the same rule, checked on
            // the other write path.
            let error = connector.remove_runtime_peer("apex-store").unwrap_err();
            assert!(matches!(error, PeerRouteTableError::OwnedByConfig(id) if id == "apex-store"));
        }

        /// Same rule, the route-prefix half: a runtime route can never
        /// take a prefix the config file already routes, whether that
        /// config row terminates (an app route) or forwards (a peer
        /// route) -- this is the interaction with `connector-config`'s
        /// load-time `UnknownPeerId`/precedence story issue #884 asks to
        /// be tested, restated as a runtime-checked invariant.
        #[test]
        fn a_runtime_route_colliding_with_a_config_prefix_is_refused() {
            let app_route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let connector = Connector::new(
                vec![app_route],
                vec![PeerRoute::new("g.example.peer", "configured-peer")],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_config_peer_ids(["configured-peer".to_string()]);

            let error = connector
                .upsert_runtime_peer_route("g.example.app", "configured-peer", Price::FREE)
                .unwrap_err();
            assert!(
                matches!(error, PeerRouteTableError::OwnedByConfig(prefix) if prefix == "g.example.app")
            );

            let error = connector
                .upsert_runtime_peer_route("g.example.peer", "configured-peer", Price::FREE)
                .unwrap_err();
            assert!(
                matches!(error, PeerRouteTableError::OwnedByConfig(prefix) if prefix == "g.example.peer")
            );
        }

        /// The runtime analogue of `connector-config`'s load-time
        /// `UnknownPeerId` check (`config.rs:283-301`): a route naming a
        /// peer id nothing recognizes -- neither the config file nor the
        /// runtime peer table -- is refused rather than accepted as an
        /// orphaned row that would answer `T01` forever.
        #[test]
        fn a_runtime_route_naming_an_unknown_peer_id_is_refused() {
            let connector = Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            );

            let error = connector
                .upsert_runtime_peer_route("g.example.app", "nobody", Price::FREE)
                .unwrap_err();
            assert!(matches!(
                error,
                PeerRouteTableError::UnknownPeerId { prefix, peer_id }
                    if prefix == "g.example.app" && peer_id == "nobody"
            ));
        }

        /// A runtime route's `peer_id` may resolve to a CONFIG peer, not
        /// only a runtime one -- referential integrity is checked against
        /// the union of both tables (ADR 0034), so an operator may point a
        /// runtime prefix at a peering this node already had before any
        /// runtime mutation existed.
        #[test]
        fn a_runtime_route_may_name_a_config_peer_id() {
            let connector = Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_config_peer_ids(["configured-peer".to_string()]);

            let view = connector
                .upsert_runtime_peer_route("g.example.new", "configured-peer", Price::flat(10))
                .unwrap();
            assert_eq!(view.peer_id, "configured-peer");
            assert_eq!(view.source, RouteSource::Runtime);
        }

        /// A runtime peer cannot be removed while a runtime route still
        /// forwards to it -- the orphaned-row shape `UnknownPeerId`
        /// exists to prevent at load, enforced here at mutation time
        /// instead.
        #[test]
        fn removing_a_runtime_peer_still_referenced_by_a_runtime_route_is_refused() {
            let connector = covering(
                Connector::new(
                    vec![],
                    vec![],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                ),
                "runtime-hop",
            );
            connector
                .upsert_runtime_peer("runtime-hop", peering(0))
                .unwrap();
            connector
                .upsert_runtime_peer_route("g.example.app", "runtime-hop", Price::FREE)
                .unwrap();

            let error = connector.remove_runtime_peer("runtime-hop").unwrap_err();
            assert!(matches!(error, PeerRouteTableError::PeerInUse(id) if id == "runtime-hop"));

            connector
                .remove_runtime_peer_route("g.example.app")
                .unwrap();
            connector
                .remove_runtime_peer("runtime-hop")
                .expect("no longer referenced, now removable");
        }

        /// Priority ordering (issue #884): a runtime peer route is durable
        /// -- a deliberate, paid relationship, not an automated
        /// controller's TTL-bound push -- so it outranks a lease at the
        /// same prefix, the same way a config peer route always did.
        #[tokio::test]
        async fn a_runtime_peer_route_outranks_a_lease_at_the_same_prefix() {
            let leased_hop_route =
                StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let leased_hop_app_client = Arc::new(FakeAppClient::new());
            leased_hop_app_client
                .respond(leased_hop_route.handler_url(), answered(b"via the lease"));
            let leased_hop = Arc::new(
                Connector::new(
                    vec![leased_hop_route],
                    vec![],
                    leased_hop_app_client,
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(identity_signer()),
            );
            let runtime_hop_route =
                StaticRoute::new("g.example.app", "http://localhost:5000").unwrap();
            let runtime_hop_app_client = Arc::new(FakeAppClient::new());
            runtime_hop_app_client.respond(
                runtime_hop_route.handler_url(),
                answered(b"via the runtime route"),
            );
            let runtime_hop = Arc::new(
                Connector::new(
                    vec![runtime_hop_route],
                    vec![],
                    runtime_hop_app_client,
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(identity_signer()),
            );
            let mut peer_transport = InProcessPeerTransport::new();
            peer_transport.add_peer("leased-hop", leased_hop);
            peer_transport.add_peer("runtime-hop", runtime_hop);
            let connector = covering(
                Connector::new(
                    vec![],
                    vec![],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(peer_transport),
                    test_clock(),
                ),
                "runtime-hop",
            );
            connector
                .upsert_leased_route("g.example.app", "leased-hop", Duration::seconds(60))
                .unwrap();
            connector
                .upsert_runtime_peer("runtime-hop", peering(0))
                .unwrap();
            connector
                .upsert_runtime_peer_route("g.example.app", "runtime-hop", Price::FREE)
                .unwrap();
            let (sealed, shared_secret) = sealed_prepare(b"hello");

            let response = connector.handle_prepare(sealed).await;

            match response {
                PacketResponse::Fulfill(fulfill) => assert_eq!(
                    open_sealed_envelope(&shared_secret, &fulfill.data),
                    fulfill_envelope(b"via the runtime route")
                ),
                other => panic!("expected a fulfill via the runtime route, got {other:?}"),
            }
        }

        /// Durability (issue #884): unlike a leased route
        /// (`leased_routes_do_not_survive_a_restart`), a runtime peer and
        /// its route survive a restart when a `PeerRouteStore` backs the
        /// table -- two independent `Connector` instances opening the
        /// same store stand in for "before" and "after" a restart.
        #[test]
        fn a_runtime_peer_and_route_survive_a_restart() {
            let dir = tempfile::tempdir().expect("temp dir");
            let path = dir.path().join("runtime_peers.json");

            let (store, peers, routes) = PeerRouteStore::open(&path).expect("open");
            let before_restart = covering(
                Connector::new(
                    vec![],
                    vec![],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                ),
                "apex-relay-2",
            )
            .with_runtime_peer_route_store(store, peers, routes);
            before_restart
                .upsert_runtime_peer("apex-relay-2", peering(3))
                .unwrap();
            before_restart
                .upsert_runtime_peer_route("g.example.relay2", "apex-relay-2", Price::flat(25))
                .unwrap();
            assert_eq!(before_restart.peers().len(), 1);

            let (store, peers, routes) = PeerRouteStore::open(&path).expect("re-open");
            let after_restart = Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_runtime_peer_route_store(store, peers, routes);

            let peers = after_restart.peers();
            assert_eq!(peers.len(), 1);
            assert_eq!(peers[0].id, "apex-relay-2");
            assert_eq!(peers[0].source, RouteSource::Runtime);
            // The peering's fee came back with it (ADR 0061), and so did
            // the forward this node charges a client for.
            assert_eq!(peers[0].fee, 3);
            assert_eq!(after_restart.fee_for("apex-relay-2"), 3);
            let routes = after_restart.peer_routes_view();
            assert_eq!(routes.len(), 1);
            assert_eq!(routes[0].prefix, "g.example.relay2");
            assert_eq!(routes[0].peer_id, "apex-relay-2");
            assert_eq!(routes[0].price, Price::flat(25));
        }

        /// A node with no `state_dir` (no `PeerRouteStore` attached) still
        /// has a mutable runtime table -- it just does not survive a
        /// restart, the same "degrade to in-memory-only" every other
        /// `state_dir`-scoped store on this connector takes.
        #[test]
        fn with_no_store_the_table_is_still_mutable_but_only_in_memory() {
            let connector = Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            );

            connector
                .upsert_runtime_peer("runtime-hop", peering(0))
                .unwrap();
            assert_eq!(connector.peers().len(), 1);
        }

        /// `GET /peers`/`GET /routes/peers`' merged view (issue #884)
        /// tags every row with where it came from, so precedence is
        /// something an operator can actually verify rather than infer.
        #[test]
        fn peers_and_peer_routes_report_config_and_runtime_rows_with_their_source() {
            let connector = covering(
                Connector::new(
                    vec![],
                    vec![PeerRoute::new("g.example.configured", "configured-peer")],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                ),
                "runtime-peer",
            )
            .with_peer_fees([("configured-peer".to_string(), 1)])
            .with_config_peer_ids(["configured-peer".to_string()]);
            connector
                .upsert_runtime_peer("runtime-peer", peering(2))
                .unwrap();
            connector
                .upsert_runtime_peer_route("g.example.runtime", "runtime-peer", Price::flat(5))
                .unwrap();

            let mut peers = connector.peers();
            peers.sort_by(|a, b| a.id.cmp(&b.id));
            assert_eq!(
                peers,
                vec![
                    PeerView {
                        id: "configured-peer".to_string(),
                        fee: 1,
                        max_packet_amount: DEFAULT_MAX_PACKET_AMOUNT,
                        source: RouteSource::Config,
                    },
                    // The cap this peering states is zero -- "this row
                    // states none" -- so what is reported is the bound
                    // actually enforced, never a zero that would read as
                    // "forward nothing" (ADR 0049).
                    PeerView {
                        id: "runtime-peer".to_string(),
                        fee: 2,
                        max_packet_amount: DEFAULT_MAX_PACKET_AMOUNT,
                        source: RouteSource::Runtime,
                    },
                ]
            );

            let mut routes = connector.peer_routes_view();
            routes.sort_by(|a, b| a.prefix.cmp(&b.prefix));
            assert_eq!(
                routes,
                vec![
                    PeerRouteView {
                        prefix: "g.example.configured".to_string(),
                        peer_id: "configured-peer".to_string(),
                        price: Price::FREE,
                        source: RouteSource::Config,
                    },
                    PeerRouteView {
                        prefix: "g.example.runtime".to_string(),
                        peer_id: "runtime-peer".to_string(),
                        price: Price::flat(5),
                        source: RouteSource::Runtime,
                    },
                ]
            );
        }

        #[test]
        fn upsert_runtime_peer_route_rejects_an_invalid_prefix() {
            let connector = Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            );
            connector
                .upsert_runtime_peer("runtime-hop", peering(0))
                .unwrap();

            let error = connector
                .upsert_runtime_peer_route("not an ilp address", "runtime-hop", Price::FREE)
                .unwrap_err();
            assert!(matches!(error, PeerRouteTableError::InvalidPrefix(_)));
        }

        /// ADR 0058's first runtime twin, and the one that makes a
        /// runtime peering a peering rather than a name: a peering with no
        /// payment channel bound to it is refused **at write time**, the
        /// runtime analogue of `connector-config`'s load-time
        /// `PeerChannelUnbound`.
        ///
        /// Discovering it at the first arriving frame instead is a peering
        /// that silently only ever behaves as a stranger: role is a
        /// verified claim on a channel this peering configures (ADR 0060),
        /// so with no channel there is no route to the peer role at all.
        #[test]
        fn a_peering_with_no_channel_bound_to_it_is_refused_at_write_time() {
            let connector = Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            );

            let unbound = RuntimePeering {
                fee: 100,
                endpoint: Some("https://peer.example/ilp".to_string()),
                ..RuntimePeering::default()
            };
            let error = connector
                .upsert_runtime_peer("runtime-hop", unbound)
                .unwrap_err();

            assert!(
                matches!(error, PeerRouteTableError::PeerChannelUnbound(id) if id == "runtime-hop"),
                "an endpoint and a fee are not a peering"
            );
            assert!(
                connector.peers().is_empty(),
                "a refused write leaves no row behind"
            );
        }

        /// ADR 0058's second runtime twin: a route forwarding to a peering
        /// with no channel to pay from is refused, the runtime analogue of
        /// ADR 0042's `[[pay_channels]]` load rule.
        ///
        /// Reached here through a peering replayed out of a durable
        /// snapshot written before ADR 0058 -- the one way a channel-less
        /// runtime peering can still exist, since the write above refuses
        /// to create one. A route to it would produce nothing but refused
        /// packets: a forward this node cannot cover is refused rather
        /// than carried.
        #[test]
        fn a_route_forwarding_to_a_peering_with_no_pay_channel_is_refused() {
            let dir = tempfile::tempdir().expect("temp dir");
            let path = dir.path().join("runtime_peers.json");
            // Exactly the bytes the pre-ADR-0058 snapshot format wrote.
            std::fs::write(
                &path,
                r#"{"peers":[{"id":"legacy-hop","fee":7}],"routes":[]}"#,
            )
            .expect("write an older snapshot");
            let (store, peers, routes) = PeerRouteStore::open(&path).expect("replay it");

            let connector = Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_runtime_peer_route_store(store, peers, routes);

            let error = connector
                .upsert_runtime_peer_route("g.example.runtime", "legacy-hop", Price::flat(25))
                .unwrap_err();

            assert!(
                matches!(
                    error,
                    PeerRouteTableError::PeerHasNoPayChannel { ref peer_id, .. }
                        if peer_id == "legacy-hop"
                ),
                "expected the pay-channel twin to fire, got {error:?}"
            );
            // Distinct from `UnknownPeerId`: the peering is known, and
            // saying so is what tells an operator to fix the peering
            // rather than the route's `peer_id`.
            assert!(!matches!(error, PeerRouteTableError::UnknownPeerId { .. }));
        }

        #[test]
        fn upsert_runtime_peer_rejects_an_empty_id() {
            let connector = Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            );

            let error = connector.upsert_runtime_peer("", peering(0)).unwrap_err();
            assert!(matches!(error, PeerRouteTableError::InvalidPeerId));
        }

        #[test]
        fn removing_a_peer_or_route_that_does_not_exist_is_a_named_error() {
            let connector = Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            );

            assert!(matches!(
                connector.remove_runtime_peer("nobody"),
                Err(PeerRouteTableError::PeerNotFound(id)) if id == "nobody"
            ));
            assert!(matches!(
                connector.remove_runtime_peer_route("g.nowhere"),
                Err(PeerRouteTableError::RouteNotFound(prefix)) if prefix == "g.nowhere"
            ));
        }
    }
}
