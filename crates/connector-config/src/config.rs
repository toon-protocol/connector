use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use url::Url;

use crate::client_channel::{resolve_client_channels, ClientChannelConfig, RawClientChannel};
use crate::client_channel_asset::{resolve_client_channel_assets, ClientChannelAssets};
use crate::denomination::{
    resolve_denomination, DenominationConfig, RawRateGuards, RawRateRow, RawToken,
};
use crate::error::ConfigError;
use crate::identity::{resolve_client_identities, ClientIdentityConfig, RawClientIdentity};
use crate::node::{resolve_node, NodeConfig, RawNodeConfig};
use crate::operator::{resolve_operator, OperatorConfig, RawOperatorConfig};
use crate::pay_channel::{resolve_pay_channels, PayChannelConfig, RawPayChannel};
use crate::peer::{
    is_onion_endpoint, parse_peer_exposure, resolve_peers, PeerConfig, PeerExposure, RawPeer,
};
use crate::peer_channel::{resolve_peer_channels, PeerChannelConfig, RawPeerChannel};
use crate::peering_asset::{resolve_peering_assets, PeeringAssets};
use crate::route::{resolve_routes, PeerRouteConfig, RawChild, RawRoute, StaticRoute};
use crate::secret::{RawSignerConfig, SecretLocation};
use crate::settlement::{
    resolve_settlement, RawSettlementSection, SettlementConfig, SettlementTables,
};

/// The config file's shape exactly as written -- convenience forms
/// (`children`) intact, nothing yet validated. `deny_unknown_fields`
/// (issue #542): an unrecognized top-level key -- a typo, or a section
/// this connector doesn't understand -- fails config load loudly instead
/// of being parsed, silently dropped, and the node starting as if it had
/// never been written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    client_edge_addr: String,
    signer: RawSignerConfig,
    #[serde(default)]
    apex: Option<String>,
    #[serde(default)]
    routes: Vec<RawRoute>,
    #[serde(default)]
    children: Vec<RawChild>,
    #[serde(default)]
    operator: Option<RawOperatorConfig>,
    /// The short list of facts about this node that no node can introspect
    /// about itself (ADR 0050, issue #1080): its own PUBLIC ILP-over-HTTP and
    /// BTP endpoints, and the ILP addresses it answers to. Absent means this
    /// node was told none of them, and its self-description omits them.
    #[serde(default)]
    node: Option<RawNodeConfig>,
    /// Renamed to `[node]` (ADR 0050, issue #1080). Still parsed, and only so
    /// that a config still writing the old heading fails at boot with
    /// [`ConfigError::AnnounceSectionRenamed`] rather than tripping the
    /// generic `deny_unknown_fields` message -- the same treatment
    /// `peer_wire_addr` and `[peer_sale]` get, for the same reason: the devnet
    /// boxes run bind-mounted configs that lead the repo copies.
    #[serde(default)]
    announce: Option<toml::Value>,
    /// Removed with the raw-TCP transport (ADR 0027, issue #679). Still
    /// parsed, and only so that a stale config naming it fails at boot
    /// with [`ConfigError::PeerWireAddrRemoved`] rather than tripping the
    /// generic `deny_unknown_fields` message: the devnet boxes run
    /// bind-mounted configs that lead the repo copies, so the one that
    /// matters is the one an operator reads at 3am.
    #[serde(default)]
    peer_wire_addr: Option<toml::Value>,
    /// Which peer carriages this connector opens a listener for (issue
    /// #677, `peer-carriage-spec.md` §2.1): `"btp"`, `"http"`, `"both"` or
    /// `"neither"`. Absent means `"neither"` -- this connector dials out
    /// and accepts no peering, which is the NAT'd operator's case and the
    /// safe default, since opening a peer listener should be a line
    /// somebody wrote.
    ///
    /// Spelled as a top-level field rather than `[peers].expose` because
    /// TOML cannot hold both a `[peers]` table and a `[[peers]]` array of
    /// tables under one name; §11 leaves the spelling to this issue.
    #[serde(default)]
    peer_expose: Option<String>,
    /// Whether a `[[peers]].endpoint` may name a **plaintext** scheme --
    /// `ws://` or `http://` -- instead of the `wss://`/`https://` a peering
    /// carrying signed balance proofs otherwise requires (issue #678,
    /// gap 3).
    ///
    /// Absent and `false` are the same thing and are the production
    /// answer: a plaintext endpoint stays [`ConfigError::PeerEndpointScheme`],
    /// exactly as before this field existed. `true` is a **loopback and
    /// test** opt-in, for a harness that stands up two connectors on
    /// `127.0.0.1` with no TLS terminator between them; a node that sets it
    /// logs a `WARN` naming every plaintext peering at startup.
    ///
    /// Deliberately one top-level switch rather than a per-peer knob: a
    /// per-peer field reads as an ordinary property of that peering and
    /// would be copied into a production file one peer at a time, where
    /// this one is a single line an operator has to write about the whole
    /// node.
    #[serde(default)]
    peer_allow_plaintext_endpoints: Option<bool>,
    /// The one SOCKS5 proxy this node dials **onion** endpoints through
    /// (ADR 0070 decision 3). Absent -- the default, and the shape of every
    /// node that does not run an onion sidecar -- means every dial is
    /// direct.
    ///
    /// The URL must be `socks5h://`, and the `h` is not a preference:
    /// `socks5://` resolves the hostname locally before dialing, and no
    /// local resolver resolves a `.onion` name. A node that started with
    /// one would be a node whose onion peerings fail at dial time for a
    /// reason nothing in its log explains, which is why the scheme is
    /// [`ConfigError::SocksProxyScheme`] at load rather than a runtime
    /// surprise.
    ///
    /// One key and one proxy: there is deliberately no per-peer `proxy =`
    /// on `[[peers]]` and no all-outbound mode. Which dials take the proxy
    /// is read off the endpoint's own host, so a second place to state it
    /// would only be a second place to state it wrong. It covers the ILP
    /// wire only -- settlement RPC and a route's `handler_url` dial direct
    /// (ADR 0070 decision 4).
    #[serde(default)]
    socks_proxy: Option<String>,
    /// The peering relations this node has (issue #488; endpoint and
    /// per-relation terms, issue #677). What used to be a
    /// dialed `SocketAddr` is now an `endpoint` URL whose **scheme**
    /// selects the carriage -- `wss://` BTP, `https://` ILP-over-HTTP (ADR
    /// 0027) -- or no endpoint at all, for a peering that dials in.
    #[serde(default)]
    peers: Vec<RawPeer>,
    /// The payment channels each peering relation's claims are judged
    /// against, and the EIP-712 domain they are signed under (issue #677,
    /// ADR 0024). This is the table whose absence made ADR 0024's
    /// peer-claim mechanism inert (#620 gap 3): a peering with no row here
    /// can never take the peer role at all.
    #[serde(default)]
    peer_channels: Vec<RawPeerChannel>,
    /// The channels this node **pays** each next hop from, as an ordinary
    /// client of that hop (ADR 0042 item 2, issue #881). Absent -- the
    /// default, and every config that predates this table -- means this
    /// node covers no forward proactively and every peering keeps riding
    /// ADR 0004's postpay `pending_claim` exactly as before, which is why
    /// the table is additive rather than a migration. See
    /// [`crate::PayChannelConfig`] for where each part of the claim it
    /// configures comes from.
    #[serde(default)]
    pay_channels: Vec<RawPayChannel>,
    /// Removed with purchasable peering (ADR 0043): a peering cannot be
    /// bought, so there is no priced route that sells one and no
    /// `prefix`/`price`/`lease_seconds`/`max_purchased_rows`/
    /// `max_routes_per_payer`/`max_prefix_length`/`purchase_rate_limit`/
    /// `purchase_rate_window_seconds` for it to carry. Still parsed, and
    /// only so that a stale config naming the section fails at boot with
    /// [`ConfigError::PeerSaleRemoved`] rather than tripping the generic
    /// `deny_unknown_fields` message -- the same treatment
    /// `peer_wire_addr` above already gets, for the same reason: the
    /// devnet boxes run bind-mounted configs that lead the repo copies.
    #[serde(default)]
    peer_sale: Option<toml::Value>,
    /// One or more real settlement backends to construct at startup (issue
    /// #542; per-chain tables, issue #628). Absent means channel operations
    /// keep degrading to `ChannelOperationError::NoSettlementBackend`, same
    /// as before this section existed.
    #[serde(default)]
    settlement: Option<RawSettlementSection>,
    /// The payment channels this node accepts client-edge claims on, and
    /// the counterparty whose signature it accepts on each (issue #558).
    /// Absent -- or empty -- means this node has a record of no channel,
    /// so every claim presented at its client edge is refused as unknown.
    #[serde(default)]
    client_channels: Vec<RawClientChannel>,
    /// The client-edge identities this node authenticates over HTTP (issue
    /// #502, `docs/protocol/client-edge-spec.md` §1.2): an `id` a request
    /// presents via `ILP-Peer-Id` and the `Authorization: Bearer <secret>`
    /// it must match. Absent -- or empty -- means this node configures no
    /// peer identity, so every request is either anonymous (no
    /// `ILP-Peer-Id` presented) or refused `401` (one presented, matching
    /// nothing); anonymity stays a first-class path either way.
    #[serde(default)]
    client_identities: Vec<RawClientIdentity>,
    /// The directory this node keeps its durable money state in (issue
    /// #605): the journals whose replay is what makes a claim watermark
    /// survive a restart. Absent means this node writes none -- allowed
    /// only for a node that cannot accept a claim in the first place,
    /// since a watermark held only in memory is not a replay defence.
    #[serde(default)]
    state_dir: Option<String>,
    /// How long this node may believe a chain-resolved channel's *mutable*
    /// facts -- that it has not settled, and that its token still matches
    /// -- before re-reading them (issue #649). Absent means the client
    /// edge's own default.
    ///
    /// An operator knob rather than a constant because the trade it makes
    /// is a deployment's to make: a node on a rate-limited public RPC
    /// endpoint wants it longer, and one that wants a settled channel
    /// noticed sooner wants it shorter. `0` re-verifies on every packet,
    /// which is correct and expensive, so it is refused at load rather
    /// than silently accepted as a way to melt an endpoint.
    #[serde(default)]
    channel_liveness_ttl_secs: Option<u64>,
    /// How long past `channel_liveness_ttl_secs` a channel's last good
    /// reading may still be *served* while the chain cannot be reached
    /// (issue #649). Absent means the client edge's own default; `0` means
    /// never -- a coherent fail-closed choice, unlike a zero TTL, since
    /// nothing about it costs an extra chain read.
    #[serde(default)]
    channel_serve_stale_secs: Option<u64>,
    /// The floor on how often one channel may provoke a chain lookup, in
    /// milliseconds. Absent means the client edge's own default. This is
    /// the knob an operator on a rate-limited endpoint reaches for, and
    /// `0` is refused for the same reason a zero TTL is: it is how one
    /// packet becomes one RPC.
    #[serde(default)]
    channel_reattempt_interval_ms: Option<u64>,
    /// The rate one self-declared signer's lookups for channels that do not
    /// resolve are shaped to, per window, once the node-wide drain below is
    /// in arrears (issue #613). Absent means the client edge's own default;
    /// `0` is refused, since a rate of nothing per window would refuse
    /// every unaffiliated buyer the moment the node got busy -- i.e. switch
    /// off the registration-free path #611 exists to provide, silently and
    /// only under load.
    #[serde(default)]
    unresolvable_lookup_budget_per_signer: Option<u32>,
    /// The rate this node's lookups for channels that do not resolve are
    /// shaped to per window in total, whoever asks (issue #613). This is
    /// the one a sender cannot raise by declaring a different signer, so it
    /// is the figure an operator on a metered settlement endpoint actually
    /// sets, and it should be derived from what that endpoint can absorb.
    /// Absent means the client edge's own default; `0` is refused for the
    /// same reason as above.
    #[serde(default)]
    unresolvable_lookup_budget_total: Option<u32>,
    /// The window both rates above are expressed over, and the burst either
    /// tolerates. Absent means the client edge's own default; `0` is
    /// refused, and it is the sharpest footgun of the four: a zero-length
    /// window makes both rates infinite and the bound nothing at all, while
    /// looking configured.
    #[serde(default)]
    unresolvable_lookup_budget_window_secs: Option<u64>,
    /// How long a lookup may wait for its slot before being refused
    /// instead (issue #613). Absent means the client edge's own default;
    /// `0` is refused, because a zero wait ceiling turns the shaper back
    /// into a dropper -- and a dropping bound hands any sender able to
    /// sustain `unresolvable_lookup_budget_total` requests per window a
    /// switch that turns the registration-free path off for every new
    /// buyer, which is a worse failure than the RPC spend it prevents.
    #[serde(default)]
    unresolvable_lookup_budget_max_wait_ms: Option<u64>,
    /// How many of one BTP session's frames may be past claim admission --
    /// waiting out the journal's group commit, being routed downstream,
    /// answering -- at once (issue #688). Claims are judged strictly in
    /// arrival order regardless; this bounds only the overlapped tail.
    /// Absent means the client edge's own default; `0` is refused, since a
    /// window of nothing is not a slower session, it is a session whose
    /// first paid frame waits forever while the file reads as configured.
    /// `1` is the original lockstep session.
    #[serde(default)]
    btp_session_window: Option<u32>,
    /// The tokens this node **deals** (ADR 0071 decision 3, issue #1290),
    /// one row each: the token's chain and contract identity, which of them
    /// is the node's numeraire, and optionally where its price is read
    /// from. Absent -- the default, and every config that predates ADR 0071
    /// -- means this node deals nothing, crosses no denomination boundary,
    /// and forwards exactly as it did before the table existed.
    #[serde(default)]
    tokens: Vec<RawToken>,
    /// What this node declares about one **ordered** token pair (ADR 0071
    /// decision 3): a static rate for a pair that cannot self-source, a
    /// per-pair override of one that can, and per-pair guards over the
    /// `[rate_guards]` defaults. Ordered, never sorted -- direction is the
    /// trade, and `X -> Y` and `Y -> X` are different prices from one mid.
    #[serde(default)]
    rates: Vec<RawRateRow>,
    /// This node's dealing policy (ADR 0071 decision 5): the `spread` it
    /// earns, the `ttl` past which a rate is dead, and the `max_move` a
    /// single refresh may not jump. Required as soon as anything can
    /// produce a rate, because none of the three has a safe default; a
    /// `[[rates]]` row overrides any of them for its own pair.
    #[serde(default)]
    rate_guards: Option<RawRateGuards>,
}

/// The client edge's own defaults for the unresolvable-lookup shaper
/// (issue #613), restated here so that [`Config::load`] can validate the
/// values an operator wrote against the ones that will actually be in
/// force.
///
/// Duplicating them is deliberate and is covered by a test: without them,
/// a cross-field rule can only fire when *both* fields are present, so
/// `unresolvable_lookup_budget_total = 5` on its own -- with the per-signer
/// rate defaulting to something larger -- loads with exactly the incoherent
/// configuration the rule exists to refuse. `connector-cli`'s
/// `the_config_layers_budget_defaults_match_the_client_edges` pins them to
/// `UnresolvableLookupBudgetPolicy::default()`, which is the authority at
/// runtime, so the two cannot drift unnoticed.
const DEFAULT_UNRESOLVABLE_LOOKUPS_PER_SIGNER: u32 = 20;
const DEFAULT_UNRESOLVABLE_LOOKUPS_TOTAL: u32 = 600;
const DEFAULT_UNRESOLVABLE_LOOKUP_WINDOW_SECS: u64 = 60;
const DEFAULT_UNRESOLVABLE_LOOKUP_MAX_WAIT_MS: u64 = 2_000;

/// The longest window the client edge will honour -- a day. Restated here
/// for the same reason the rates are, and pinned to
/// `MAX_UNRESOLVABLE_LOOKUP_WINDOW` by the same `connector-cli` test.
const MAX_UNRESOLVABLE_LOOKUP_WINDOW_SECS: u64 = 86_400;

/// A fully loaded, fully validated, immutable connector configuration.
///
/// The only way to obtain one is [`Config::load`]: every field has already
/// been checked for presence, range and cross-field consistency (ADR 0009),
/// and convenience forms (`children`) have already been desugared into
/// ordinary [`StaticRoute`]s. Downstream code should never re-check a
/// [`Config`] value -- if it loaded, it is valid for the rest of the
/// process's life.
#[derive(Debug, Clone)]
pub struct Config {
    client_edge_addr: SocketAddr,
    signer_key: SecretLocation,
    routes: Vec<StaticRoute>,
    peer_routes: Vec<PeerRouteConfig>,
    peers: Vec<PeerConfig>,
    peer_expose: PeerExposure,
    peer_allow_plaintext_endpoints: bool,
    socks_proxy: Option<Url>,
    peer_channels: Vec<PeerChannelConfig>,
    pay_channels: Vec<PayChannelConfig>,
    operator: Option<OperatorConfig>,
    node: Option<NodeConfig>,
    settlements: Vec<SettlementConfig>,
    client_channels: Vec<ClientChannelConfig>,
    client_identities: Vec<ClientIdentityConfig>,
    state_dir: Option<PathBuf>,
    channel_liveness_ttl: Option<Duration>,
    channel_serve_stale: Option<Duration>,
    channel_reattempt_interval: Option<Duration>,
    unresolvable_lookups_per_signer: Option<u32>,
    unresolvable_lookups_total: Option<u32>,
    unresolvable_lookup_window: Option<Duration>,
    unresolvable_lookup_max_wait: Option<Duration>,
    btp_session_window: Option<NonZeroU32>,
    denomination: DenominationConfig,
    peering_assets: PeeringAssets,
    client_channel_assets: ClientChannelAssets,
}

impl Config {
    /// Read, parse and fully validate the configuration file at `path`.
    ///
    /// This is the only startup work that may fail before the node runs:
    /// per ADR 0009, an `Err` here must stop the process before anything
    /// else starts, and an `Ok` value needs no further validation anywhere
    /// downstream.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml_str(&text, path)
    }

    fn from_toml_str(text: &str, path: &Path) -> Result<Config, ConfigError> {
        let raw: RawConfig = toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;

        let client_edge_addr = raw
            .client_edge_addr
            .parse::<SocketAddr>()
            .map_err(|source| ConfigError::InvalidBindAddr {
                value: raw.client_edge_addr.clone(),
                source,
            })?;

        let signer_key = SecretLocation::resolve(raw.signer)?;
        let (routes, peer_routes) = resolve_routes(raw.apex.as_deref(), raw.routes, raw.children)?;
        let peer_expose = parse_peer_exposure(raw.peer_expose)?;
        let peer_allow_plaintext_endpoints = raw.peer_allow_plaintext_endpoints.unwrap_or(false);
        let socks_proxy = resolve_socks_proxy(raw.socks_proxy)?;
        let peers = resolve_peers(raw.peers, peer_expose, peer_allow_plaintext_endpoints)?;
        // Resolved before every channel table rather than beside the other
        // money tables below, for two reasons that are now one rule.
        //
        // A Solana channel's settlement program is no longer a fact of its
        // own row -- it is read from here (issues #1082, #1128), and one
        // program is the only program a node can submit a redemption to.
        //
        // And a channel row whose chain has no `[settlement.<chain>]` table
        // at all is refused by name (issue #1138): that table is where this
        // node's on-chain identity on that chain comes from, so without it
        // the node cannot be a participant of any channel there and every
        // claim the row would admit is carriage rendered for money it can
        // never collect. `SettlementTables` states the rule once, and all
        // four channel tables -- peer, client, pay, on either chain --
        // answer to it.
        let settlements = resolve_settlement(raw.settlement)?;
        let settlement_tables = SettlementTables::of(&settlements);
        let peer_channels = resolve_peer_channels(raw.peer_channels, settlement_tables)?;
        for peer_route in &peer_routes {
            let Some(peer) = peers.iter().find(|peer| peer.id() == peer_route.peer_id()) else {
                return Err(ConfigError::UnknownPeerId {
                    prefix: peer_route.prefix().to_string(),
                    peer_id: peer_route.peer_id().to_string(),
                });
            };
            // The intersection rule, in the one direction this connector's
            // own file can decide (`peer-carriage-spec.md` §2.2, §6.4(1)):
            // a route whose next hop is a peering this connector can never
            // originate to is a route that could only ever answer `T01`.
            // What the *far* side exposes is not knowable from here and
            // stays a runtime dial failure.
            if !peer.can_originate() {
                return Err(ConfigError::PeerRouteUndeliverable {
                    prefix: peer_route.prefix().to_string(),
                    peer_id: peer_route.peer_id().to_string(),
                });
            }
        }
        // Orphaned rows first, then unbound peers: a mistyped `peer_id`
        // produces both at once, and "this row names a peer that does not
        // exist" is the one that names the typo.
        for channel in &peer_channels {
            if !peers.iter().any(|peer| peer.id() == channel.peer_id()) {
                return Err(ConfigError::PeerChannelOrphaned {
                    peer_id: channel.peer_id().to_string(),
                });
            }
        }
        // P2 (§1.2): a peering with no channel binding can never take the
        // peer role, so its counterparty would be admitted as an ordinary
        // client and its claims judged in the wrong namespace. Refused at
        // load, because the runtime symptom is silence.
        for peer in &peers {
            if !peer_channels
                .iter()
                .any(|channel| channel.peer_id() == peer.id())
            {
                return Err(ConfigError::PeerChannelUnbound {
                    id: peer.id().to_string(),
                });
            }
        }
        if raw.peer_sale.is_some() {
            return Err(ConfigError::PeerSaleRemoved);
        }
        if raw.peer_wire_addr.is_some() {
            return Err(ConfigError::PeerWireAddrRemoved);
        }
        let operator = resolve_operator(raw.operator)?;
        if raw.announce.is_some() {
            return Err(ConfigError::AnnounceSectionRenamed);
        }
        let node = resolve_node(raw.node, peer_expose)?;
        let client_channels = resolve_client_channels(raw.client_channels, settlement_tables)?;
        let client_identities = resolve_client_identities(raw.client_identities)?;
        // Namespace disjointness (`peer-carriage-spec.md` §1.8). Peer and
        // client watermarks are separate records by design, which is only
        // safe while no channel is in both: two namespaces over one
        // channel would let the same claim be counted as credit twice.
        // Compared within its own chain only -- an EVM `channel_id`
        // against an EVM `channel_id` (both canonicalized lowercase `0x`
        // hex), a Solana `channel_account` against a Solana
        // `channel_account` (both base58) -- each side canonicalized by
        // its own resolver, so this compares like with like.
        for peer_channel in &peer_channels {
            let (collides, value) = match peer_channel {
                PeerChannelConfig::Evm(evm_peer) => (
                    client_channels.iter().any(|client_channel| {
                        matches!(
                            client_channel,
                            ClientChannelConfig::Evm(evm_client)
                                if evm_client.channel_id() == evm_peer.channel_id()
                        )
                    }),
                    evm_peer.channel_id().to_string(),
                ),
                PeerChannelConfig::Solana(solana_peer) => (
                    client_channels.iter().any(|client_channel| {
                        matches!(
                            client_channel,
                            ClientChannelConfig::Solana(solana_client)
                                if solana_client.channel_account() == solana_peer.channel_account()
                        )
                    }),
                    solana_peer.channel_account().to_string(),
                ),
            };
            if collides {
                return Err(ConfigError::ChannelInBothNamespaces { value });
            }
        }
        // `[[pay_channels]]` (ADR 0042 item 2, issue #881): the channels
        // this node PAYS a next hop from. Three cross-table rules, each
        // refusing at load what would otherwise be a packet-time surprise
        // on the money path (ADR 0009).
        let pay_channels = resolve_pay_channels(
            raw.pay_channels,
            peer_allow_plaintext_endpoints,
            settlement_tables,
        )?;
        for pay_channel in &pay_channels {
            // A row for a peering that does not exist pays nobody -- the
            // same reasoning `PeerChannelOrphaned` applies, and the same
            // typo it catches.
            if !peers.iter().any(|peer| peer.id() == pay_channel.peer_id()) {
                return Err(ConfigError::PayChannelOrphaned {
                    peer_id: pay_channel.peer_id().to_string(),
                });
            }
            // ADR 0030, said of `[announce] pay_channel` and just as true
            // here: "that table is channels this node receives on, and this
            // is one it pays from. One channel in two roles is the same
            // collision `Config::load` already refuses between the peer and
            // client books." Compared within its own chain only -- EVM hex
            // against EVM hex, Solana base58 against Solana base58 -- each
            // side canonicalized by its own resolver, exactly as the
            // peer/client namespace check above does.
            //
            // Deliberately NOT compared against `[[peer_channels]]`: one
            // channel carrying both roles with one hop is the deployed
            // shape (the peer role for what arrives, the client role for
            // what this node sends), and `forward_via_peer_route` is built
            // for it -- a covered packet is not owed a second time on the
            // peer ledger, so exactly one book ever signs per packet.
            let collides_with_client = match pay_channel {
                PayChannelConfig::Evm(evm_pay) => client_channels.iter().any(|client_channel| {
                    matches!(
                        client_channel,
                        ClientChannelConfig::Evm(evm_client)
                            if evm_client.channel_id() == evm_pay.channel_id()
                    )
                }),
                PayChannelConfig::Solana(solana_pay) => {
                    client_channels.iter().any(|client_channel| {
                        matches!(
                            client_channel,
                            ClientChannelConfig::Solana(solana_client)
                                if solana_client.channel_account()
                                    == solana_pay.channel_account()
                        )
                    })
                }
            };
            if collides_with_client {
                // On Solana this is reached only if the rule immediately
                // below is ever relaxed: a Solana pay row must name a
                // channel the peering also binds as a `[[peer_channels]]`
                // row, and the peer/client namespace check above -- which
                // says the same thing about the same channel -- therefore
                // gets there first. Kept per chain anyway, because the
                // comparison has to be like-with-like either way and a
                // half-written rule is worse than a redundant one.
                return Err(ConfigError::PayChannelIsAlsoAClientChannel {
                    value: pay_channel.channel().to_string(),
                });
            }
            // A Solana row's claims cannot be RENDERED without the peer
            // channel row beside them (issue #1146). `programId` is a
            // required field of the Solana claim wire, unlike an EVM
            // claim's optional EIP-712 domain, and both peer carriages read
            // it from that peering's Solana `[[peer_channels]]` row
            // (`connector_peer_http::dial::PeerRelation::solana_program_ids`)
            // -- a covering claim for a channel with no such row reaches
            // `claim_json::encode` with nothing to write there, which that
            // function calls a caller bug and panics on. Refused here, by
            // name and naming the peer, rather than discovered on the
            // packet path.
            //
            // It is not an extra burden in practice: paying a hop from a
            // channel this node holds with that hop is the deployed shape,
            // and the peer row is what binds the counterparty key the same
            // channel's inbound claims are judged against.
            if let PayChannelConfig::Solana(solana_pay) = pay_channel {
                let bound_as_a_peer_channel = peer_channels.iter().any(|peer_channel| {
                    matches!(
                        peer_channel,
                        PeerChannelConfig::Solana(solana_peer)
                            if solana_peer.peer_id() == solana_pay.peer_id()
                                && solana_peer.channel_account() == solana_pay.channel_account()
                    )
                });
                if !bound_as_a_peer_channel {
                    return Err(ConfigError::PayChannelSolanaWithoutPeerChannel {
                        peer_id: solana_pay.peer_id().to_string(),
                        value: solana_pay.channel_account().to_string(),
                    });
                }
            }
        }
        // ADR 0042, and the load-time half of issue #1145: **a connector
        // covers every PREPARE it sends**, so a peering this node has a
        // route to must name the channel it pays that hop from. This is the
        // mirror of `PeerChannelUnbound` one table over -- that one refuses
        // a peering with nothing to judge an ARRIVING claim against, this
        // one refuses a peering with nothing to sign a DEPARTING claim
        // from.
        //
        // It became a refusal rather than a default the moment the postpay
        // path was deleted. Before that a peering with no row simply fell
        // back to ADR 0004 (`cover_forward` answered `NotConfigured` and
        // `pending_claim` rode the next packet); now `forward_via_peer_route`
        // has nothing to fall back to and would reject every packet on the
        // route with `T00`. ADR 0009 exists to turn exactly that kind of
        // runtime surprise into a startup refusal.
        //
        // Keyed on ROUTES, not on peerings: a peering this node only ever
        // receives from -- `local/mixed-chain`'s B holds one, and every
        // accept-only peering is one -- owes nothing and needs no row. What
        // is checked is the same `peer_id` a `[[routes]]` entry names.
        for peer_route in &peer_routes {
            if !pay_channels
                .iter()
                .any(|pay_channel| pay_channel.peer_id() == peer_route.peer_id())
            {
                return Err(ConfigError::PayChannelUnbound {
                    prefix: peer_route.prefix().to_string(),
                    peer_id: peer_route.peer_id().to_string(),
                });
            }
        }
        // ADR 0071 decisions 3 and 5 (issue #1290): the tokens this node
        // deals, its numeraire, the rates and guards it has declared.
        // Resolved against `settlement_tables` because a token's quote is
        // read over its own chain's RPC endpoint, so a quote on a chain
        // with no `[settlement.<chain>]` table is a poller that could never
        // take its first reading -- refused here rather than logged
        // forever. All three keys absent is the default value and checks
        // nothing, which is what "a node that declares none of it behaves
        // exactly as it does today" means here.
        let denomination =
            resolve_denomination(raw.tokens, raw.rates, raw.rate_guards, settlement_tables)?;
        // ADR 0071 decision 1 (issue #1292): which of those declared tokens
        // each peering's channels are denominated in, so that a forward can
        // ask whether its two legs hold different ones without asking a
        // chain. Last of the money tables, because it reads all of them --
        // the peerings, both of their channel tables and the settlement
        // tables those settle through -- and it holds every one of them to
        // the declaration resolved immediately above. A node that declared
        // no tokens resolves nothing here and is held to nothing.
        let peering_assets = resolve_peering_assets(
            &peers,
            &peer_channels,
            &pay_channels,
            &settlements,
            &denomination,
        )?;
        // ADR 0071 decision 1 (issue #1301): the same question asked of the
        // client edge -- which declared token a channel a buyer pays over
        // holds, so that a forward out of a client arrival crosses a
        // boundary on the same terms a peer arrival does. After the
        // peerings, so that a node whose peering already names an
        // undeclared token is refused by the more specific message.
        // Resolved from the settlement tables alone and keyed by chain,
        // because the channel this has to cover is the one no
        // `[[client_channels]]` row names (ADR 0052, issue #502).
        let client_channel_assets = resolve_client_channel_assets(&settlements, &denomination)?;
        let state_dir = raw.state_dir.map(PathBuf::from);
        let channel_liveness_ttl = match raw.channel_liveness_ttl_secs {
            Some(0) => return Err(ConfigError::ZeroChannelLivenessTtl),
            other => other.map(Duration::from_secs),
        };
        // Zero is allowed here and refused above, and the asymmetry is the
        // point: a zero TTL means "re-read on every packet", which is how
        // an endpoint's budget is exhausted, while a zero stale window
        // means "never serve a reading I could not confirm", which costs
        // nothing extra and is a defensible thing to want.
        let channel_serve_stale = raw.channel_serve_stale_secs.map(Duration::from_secs);
        let channel_reattempt_interval = match raw.channel_reattempt_interval_ms {
            Some(0) => return Err(ConfigError::ZeroChannelReattemptInterval),
            other => other.map(Duration::from_millis),
        };
        // A stale window shorter than the TTL is not a stricter setting,
        // it is an incoherent one: an entry would pass out of "believed"
        // and out of "servable" at the same moment, so the window it names
        // could never be used. Refused rather than silently behaving as
        // zero, since an operator who wrote it meant something.
        if let (Some(ttl), Some(stale)) = (channel_liveness_ttl, channel_serve_stale) {
            if stale > Duration::ZERO && stale < ttl {
                return Err(ConfigError::ServeStaleShorterThanLivenessTtl {
                    serve_stale_secs: stale.as_secs(),
                    ttl_secs: ttl.as_secs(),
                });
            }
        }

        // The unresolvable-lookup budget (issue #613). Every one of these
        // is refused at zero, unlike `channel_serve_stale_secs` above:
        // there, zero names a coherent fail-closed choice that costs
        // nothing extra; here, each zero either switches the
        // registration-free path off entirely or switches the budget off
        // while leaving it configured, and neither is a thing an operator
        // could mean by writing a number down.
        let unresolvable_lookups_per_signer = match raw.unresolvable_lookup_budget_per_signer {
            Some(0) => {
                return Err(ConfigError::ZeroUnresolvableLookupBudget {
                    field: "per_signer",
                })
            }
            other => other,
        };
        let unresolvable_lookups_total = match raw.unresolvable_lookup_budget_total {
            Some(0) => return Err(ConfigError::ZeroUnresolvableLookupBudget { field: "total" }),
            other => other,
        };
        let unresolvable_lookup_window_secs = match raw.unresolvable_lookup_budget_window_secs {
            Some(0) => return Err(ConfigError::ZeroUnresolvableLookupWindow),
            Some(secs) if secs > MAX_UNRESOLVABLE_LOOKUP_WINDOW_SECS => {
                return Err(ConfigError::UnresolvableLookupWindowTooLong {
                    window_secs: secs,
                    max_secs: MAX_UNRESOLVABLE_LOOKUP_WINDOW_SECS,
                })
            }
            other => other,
        };
        let unresolvable_lookup_window = unresolvable_lookup_window_secs.map(Duration::from_secs);
        let unresolvable_lookup_max_wait_ms = match raw.unresolvable_lookup_budget_max_wait_ms {
            Some(0) => return Err(ConfigError::ZeroUnresolvableLookupMaxWait),
            other => other,
        };
        // The wait ceiling is not just a timeout: it *is* the size of the
        // waiting room, since a room drained at `total / window` and holding
        // requests for `max_wait` parks `max_wait * total / window` of them.
        // A ceiling longer than the window therefore parks more than a whole
        // window's worth of drain, which is both more memory than the bound
        // is worth and a wait no packet's own deadline could survive. It is
        // the one budget knob with a coherence rule rather than only a zero
        // check, and it needs one for exactly the reason the others do:
        // nothing else in the file would tell an operator they had written
        // a room ten thousand deep.
        let effective_window_secs =
            unresolvable_lookup_window_secs.unwrap_or(DEFAULT_UNRESOLVABLE_LOOKUP_WINDOW_SECS);
        let effective_max_wait_ms =
            unresolvable_lookup_max_wait_ms.unwrap_or(DEFAULT_UNRESOLVABLE_LOOKUP_MAX_WAIT_MS);
        if effective_max_wait_ms > effective_window_secs.saturating_mul(1_000) {
            return Err(ConfigError::UnresolvableLookupMaxWaitAboveWindow {
                max_wait_ms: effective_max_wait_ms,
                window_secs: effective_window_secs,
            });
        }
        let unresolvable_lookup_max_wait =
            unresolvable_lookup_max_wait_ms.map(Duration::from_millis);
        // The BTP session window (issue #688) is refused at zero for the
        // same species of reason as the budget knobs above, with a sharper
        // edge: zero is not a stricter setting, it is a session whose
        // first paid frame waits forever for an in-flight slot that does
        // not exist -- every BTP client hangs on connect while the file
        // reads as configured. (`1` is coherent: the original lockstep
        // session.)
        let btp_session_window = match raw.btp_session_window {
            Some(0) => return Err(ConfigError::ZeroBtpSessionWindow),
            other => other.and_then(NonZeroU32::new),
        };
        // A per-signer rate above the node-wide one is not a stricter
        // setting, it is an inert one: the node-wide drain saturates first,
        // every time, so the number written for the per-signer axis could
        // never be reached. Refused rather than silently ignored, on the
        // same principle as the stale-window check above -- an operator who
        // wrote it meant something by it.
        //
        // Compared against the values that will actually be **in force**,
        // defaults filled in, rather than only when both were written: a
        // rule that fires only on the both-present case leaves the two
        // one-sided spellings of the same incoherent configuration loading
        // quietly, which is the whole hazard it exists for.
        let effective_per_signer =
            unresolvable_lookups_per_signer.unwrap_or(DEFAULT_UNRESOLVABLE_LOOKUPS_PER_SIGNER);
        let effective_total =
            unresolvable_lookups_total.unwrap_or(DEFAULT_UNRESOLVABLE_LOOKUPS_TOTAL);
        if effective_per_signer > effective_total {
            return Err(ConfigError::UnresolvableLookupPerSignerAboveTotal {
                per_signer: effective_per_signer,
                total: effective_total,
            });
        }

        // A node that can accept a claim must be able to remember having
        // accepted it (issue #605). Refused here, at load, rather than at
        // the first claim: a node whose watermarks live only in memory
        // hands out free service after every restart, and it does so
        // silently -- there is nothing in a log to see, because from the
        // gate's point of view every replayed nonce genuinely is fresh.
        //
        // The trigger is "can this node resolve a channel", not "did it
        // declare one" (issue #1186). It used to be the latter, on issue
        // #558's reasoning that a node with a record of no channel refuses
        // every claim outright and so has no watermark to lose. That was
        // true when it was written and stopped being true with chain
        // resolution: since #611 and #631 `connector-cli` registers a
        // `ClientChannelSource` over each configured settlement table, and
        // a claim naming a channel nothing declared is resolved from chain
        // and accepted. That is ADR 0052 and CF-27, and it is the whole of
        // what makes payment permissionless -- so the node MOST exposed to
        // strangers was the one this check did not reach.
        //
        // What survives of #558 is the exemption's shape, and it is worth
        // keeping: a registry with neither a record nor a source refuses
        // every claim, so a node configuring no channel book and no
        // settlement genuinely has nothing to lose. Demanding a path of it
        // would be ceremony, and ceremony is what gets configured with a
        // path nobody checked.
        //
        // Price does not enter into it. `extract_and_validate_claim` runs
        // on any request carrying a claim header whose envelope target is
        // not already refused, priced route or not, so a free route on a
        // node with settlement still advances a watermark.
        if let Some(path) = &state_dir {
            if path.exists() && !path.is_dir() {
                return Err(ConfigError::StateDirNotADirectory { path: path.clone() });
            }
        } else if !client_channels.is_empty() {
            return Err(ConfigError::ClientChannelsWithoutStateDir);
        } else if !peer_channels.is_empty() {
            // The peer half of the same rule: a peer claim's watermark is
            // no less a replay defence than a client claim's, and ADR
            // 0024's ledger is the record that has to outlive the process.
            return Err(ConfigError::PeerChannelsWithoutStateDir);
        } else if !pay_channels.is_empty() {
            // The outbound half of the same rule (issue #881). Unreachable
            // through a loadable file today -- a `[[pay_channels]]` row
            // needs a peering, a peering needs a `[[peer_channels]]` row,
            // and that already demands a `state_dir` two arms above -- and
            // written out anyway, because what it protects is different in
            // kind: the outbound client ledger's nonce floor is the one
            // number that stops a RESTART reissuing a nonce this node has
            // already signed against a different cumulative amount.
            return Err(ConfigError::PayChannelsWithoutStateDir);
        } else if !settlements.is_empty() {
            // Last, deliberately. Every channel book already requires a
            // settlement table for its own chain (CF-36, issue #1138), so
            // this arm placed first would swallow all three and answer a
            // declared-channel mistake with a message about chain
            // resolution. The specific diagnosis wins; this one catches
            // exactly the case the three books do not -- a node that
            // declares no channel and is paid by strangers anyway.
            return Err(ConfigError::SettlementWithoutStateDir);
        }

        Ok(Config {
            client_edge_addr,
            signer_key,
            routes,
            peer_routes,
            peers,
            peer_expose,
            peer_allow_plaintext_endpoints,
            socks_proxy,
            peer_channels,
            pay_channels,
            operator,
            node,
            settlements,
            client_channels,
            client_identities,
            state_dir,
            channel_liveness_ttl,
            channel_serve_stale,
            channel_reattempt_interval,
            unresolvable_lookups_per_signer,
            unresolvable_lookups_total,
            unresolvable_lookup_window,
            unresolvable_lookup_max_wait,
            btp_session_window,
            denomination,
            peering_assets,
            client_channel_assets,
        })
    }

    /// Everything this node declares about denomination (ADR 0071
    /// decisions 3 and 5, issue #1290): the tokens it deals, its numeraire,
    /// the rates and guards it has written down, and its node-wide dealing
    /// policy.
    ///
    /// Always a value, never `None`: a node that declares none of it holds
    /// the empty declaration, whose
    /// [`declares_tokens`](DenominationConfig::declares_tokens) is `false`
    /// and whose every lookup answers nothing. That is deliberately the
    /// same answer an absent section would give, with one fewer question
    /// for a caller to ask.
    pub fn denomination(&self) -> &DenominationConfig {
        &self.denomination
    }

    /// Which declared token each peering's channels are denominated in (ADR
    /// 0071 decision 1, issue #1292) -- and therefore whether a forward
    /// between two of them crosses a denomination boundary, which is the
    /// one question a converting forward turns on.
    ///
    /// Always a value, and empty for every node that declares no
    /// `[[tokens]]`: such a node resolves no peering, answers no boundary,
    /// and forwards exactly as it did before ADR 0071. A node that does
    /// declare tokens got every one of its peerings resolved here at boot,
    /// so a reader on the packet path never has a chain to ask or a
    /// refusal to make.
    pub fn peering_assets(&self) -> &PeeringAssets {
        &self.peering_assets
    }

    /// Which declared token a client channel this node accepts claims on is
    /// denominated in (ADR 0071 decision 1, issue #1301) -- and therefore
    /// whether a forward out of a buyer's own packet crosses a denomination
    /// boundary, which is the same question [`Config::peering_assets`]
    /// answers for a peer arrival.
    ///
    /// Always a value, and empty for every node that declares no
    /// `[[tokens]]`: such a node resolves no channel, answers no boundary,
    /// and forwards a client arrival exactly as it did before ADR 0071.
    /// A node that does declare tokens got every chain it can be paid on
    /// resolved here at boot -- including the chains whose channels no
    /// `[[client_channels]]` row names, which is the point.
    pub fn client_channel_assets(&self) -> &ClientChannelAssets {
        &self.client_channel_assets
    }

    /// How long a chain-resolved client channel's liveness may be believed
    /// before it is re-read (issue #649), or `None` to use the client
    /// edge's own default.
    pub fn channel_liveness_ttl(&self) -> Option<Duration> {
        self.channel_liveness_ttl
    }

    /// How long past [`Self::channel_liveness_ttl`] a channel's last good
    /// reading may still be served while the chain is unreachable, or
    /// `None` to use the client edge's own default.
    pub fn channel_serve_stale(&self) -> Option<Duration> {
        self.channel_serve_stale
    }

    /// The floor on how often one channel may provoke a chain lookup, or
    /// `None` to use the client edge's own default.
    pub fn channel_reattempt_interval(&self) -> Option<Duration> {
        self.channel_reattempt_interval
    }

    /// How many chain lookups for channels that do not resolve one declared
    /// signer may cause per window once this node's window is contended
    /// (issue #613), or `None` to use the client edge's own default.
    pub fn unresolvable_lookups_per_signer(&self) -> Option<u32> {
        self.unresolvable_lookups_per_signer
    }

    /// How many chain lookups for channels that do not resolve this node
    /// will perform per window in total, or `None` to use the client edge's
    /// own default.
    pub fn unresolvable_lookups_total(&self) -> Option<u32> {
        self.unresolvable_lookups_total
    }

    /// The window the two allowances above are counted over, or `None` to
    /// use the client edge's own default.
    pub fn unresolvable_lookup_window(&self) -> Option<Duration> {
        self.unresolvable_lookup_window
    }

    /// How long a lookup for a channel this node has never resolved may
    /// wait for its slot before being refused instead (issue #613), or
    /// `None` to use the client edge's own default.
    pub fn unresolvable_lookup_max_wait(&self) -> Option<Duration> {
        self.unresolvable_lookup_max_wait
    }

    /// How many of one BTP session's frames may be past claim admission at
    /// once (issue #688), or `None` to use the client edge's own default.
    /// `NonZeroU32` because zero was refused at load: the value in force is
    /// always a working window.
    pub fn btp_session_window(&self) -> Option<NonZeroU32> {
        self.btp_session_window
    }

    /// The socket address the client edge binds.
    pub fn client_edge_addr(&self) -> SocketAddr {
        self.client_edge_addr
    }

    /// Where this node's signing key material lives.
    pub fn signer_key(&self) -> &SecretLocation {
        &self.signer_key
    }

    /// The node's static routes -- explicit `[[routes]]` entries plus every
    /// `[[children]]` entry already expanded under `apex`.
    pub fn routes(&self) -> &[StaticRoute] {
        &self.routes
    }

    /// The node's peer routes -- every `[[routes]]` entry that names a
    /// `peer_id` instead of a `handler_url`. Each one's `peer_id` is
    /// guaranteed to name an entry in [`Config::peers`] (`Config::load`
    /// refuses to return a value where it doesn't).
    pub fn peer_routes(&self) -> &[PeerRouteConfig] {
        &self.peer_routes
    }

    /// This node's peering relations. Every one is guaranteed to carry at
    /// least one [`Config::peer_channels`] row -- [`Config::load`] refuses
    /// to return a value where one is missing, because a peering with no
    /// channel bound can never take the peer role
    /// (`peer-carriage-spec.md` §1.2, P2).
    pub fn peers(&self) -> &[PeerConfig] {
        &self.peers
    }

    /// Which peer carriages this node opens a listener for
    /// (`peer-carriage-spec.md` §2.1). Independent of how any one peer is
    /// dialed: exposing BTP says nothing about how a peer is reached, and
    /// dialing a peer over HTTP says nothing about what this node listens
    /// on.
    pub fn peer_expose(&self) -> PeerExposure {
        self.peer_expose
    }

    /// Whether this node was told it may dial a **plaintext** peer
    /// endpoint (issue #678, gap 3). `false` on every production config,
    /// including every config that does not mention the field: `ws://` and
    /// `http://` are refused at load exactly as they were before it
    /// existed.
    ///
    /// `true` is loopback and test only. A caller that has one should say
    /// so loudly at startup -- [`Config::plaintext_peerings`] is the list
    /// to name.
    pub fn peer_allow_plaintext_endpoints(&self) -> bool {
        self.peer_allow_plaintext_endpoints
    }

    /// Every peering whose endpoint is plaintext **and unauthenticated**,
    /// as `(peer id, endpoint)` -- what a node with
    /// [`Config::peer_allow_plaintext_endpoints`] set must name in its
    /// startup warning. Always empty when the switch is off, because no
    /// other endpoint with a plaintext scheme could have loaded.
    ///
    /// An **onion endpoint** is excluded (ADR 0070), and its exclusion is
    /// the difference between a true warning and a false one. A `.onion`
    /// endpoint loads its plaintext scheme without the switch, so it would
    /// otherwise be named by a warning whose text says the switch is set;
    /// and its claims do not "cross the wire in the clear" -- the circuit
    /// is encrypted and authenticated to the very key the address is.
    pub fn plaintext_peerings(&self) -> impl Iterator<Item = (&str, &Url)> {
        self.peers.iter().filter_map(|peer| {
            let endpoint = peer.endpoint()?;
            (matches!(endpoint.scheme(), "ws" | "http") && !is_onion_endpoint(endpoint))
                .then_some((peer.id(), endpoint))
        })
    }

    /// The one SOCKS5 proxy this node dials onion endpoints through (ADR
    /// 0070 decision 3), or `None` -- the default -- when every dial is
    /// direct.
    ///
    /// Always `socks5h://` when present: [`Config::load`] refuses any other
    /// scheme, so a caller never has to ask whether this proxy resolves
    /// names for it. Which dials use it is not configured anywhere -- it is
    /// read off the endpoint's host, and it covers the ILP wire only.
    pub fn socks_proxy(&self) -> Option<&Url> {
        self.socks_proxy.as_ref()
    }

    /// The payment channels this node judges peer claims against (ADR
    /// 0024). Every row names a configured peer, no channel appears twice,
    /// and none of them appears in [`Config::client_channels`] either --
    /// the peer and client watermark namespaces are disjoint by
    /// construction (`peer-carriage-spec.md` §1.8).
    pub fn peer_channels(&self) -> &[PeerChannelConfig] {
        &self.peer_channels
    }

    /// The channels bound to one peering relation. Never empty for a
    /// configured peer: an unbound peering is refused at load.
    pub fn peer_channels_for<'a>(
        &'a self,
        peer_id: &'a str,
    ) -> impl Iterator<Item = &'a PeerChannelConfig> {
        self.peer_channels
            .iter()
            .filter(move |channel| channel.peer_id() == peer_id)
    }

    /// The channels this node **pays** a next hop from, as an ordinary
    /// client of that hop (ADR 0042 item 2, issue #881) -- what
    /// `Connector::with_outbound_client_hop` is configured from. Every row
    /// names a configured peer at most once, no channel appears twice, none
    /// of them appears in [`Config::client_channels`] (that table is
    /// channels this node *receives* on, ADR 0030), and this node has a
    /// `[settlement.evm]` key to sign a claim with -- [`Config::load`]
    /// refuses to return a value where any of those does not hold.
    ///
    /// **Empty is the default and means default-off**: a peering with no
    /// row here forwards exactly as it did before this table existed,
    /// riding ADR 0004's postpay `pending_claim`.
    pub fn pay_channels(&self) -> &[PayChannelConfig] {
        &self.pay_channels
    }

    /// The operator surface's authentication, if the surface is enabled.
    /// `None` means the `[operator]` section was absent -- the surface is
    /// not started at all. A `Some` value is always fully authenticated
    /// (ADR 0008): [`Config::load`] refuses to return one that is missing
    /// a bearer token or a write-key allowlist.
    pub fn operator(&self) -> Option<&OperatorConfig> {
        self.operator.as_ref()
    }

    /// The three facts this node cannot introspect about itself (ADR 0050),
    /// or `None` when the `[node]` section is absent -- in which case the
    /// node's self-description simply omits its addresses and endpoints, and
    /// the x402 greeting omits `ilpAddresses`/`btpEndpoint` exactly as it did
    /// before issue #807.
    pub fn node(&self) -> Option<&NodeConfig> {
        self.node.as_ref()
    }

    /// Every settlement backend the `[settlement]` section configures (issue
    /// #542; per-chain tables, issue #628) -- one node can name more than
    /// one chain. Empty means no backend is constructed at startup and every
    /// channel operation answers `ChannelOperationError::NoSettlementBackend`
    /// -- the same "not started at all" degradation an absent `[operator]`
    /// section already has. At most one entry per [`SettlementChain`].
    pub fn settlements(&self) -> &[SettlementConfig] {
        &self.settlements
    }

    /// The payment channels this node accepts client-edge claims on, and
    /// the counterparty whose signature it accepts on each (issue #558).
    /// Empty means this node has a record of no channel at all, so every
    /// claim presented at its client edge is refused as unknown rather
    /// than trusted about who signed it.
    pub fn client_channels(&self) -> &[ClientChannelConfig] {
        &self.client_channels
    }

    /// The client-edge identities this node authenticates over HTTP (issue
    /// #502, `docs/protocol/client-edge-spec.md` §1.2). Empty means this
    /// node configures no peer identity -- every request is either
    /// anonymous or refused `401` for presenting an `ILP-Peer-Id` that
    /// matches nothing.
    pub fn client_identities(&self) -> &[ClientIdentityConfig] {
        &self.client_identities
    }

    /// The directory this node keeps its durable money state in -- the
    /// claim journals whose replay is what makes a watermark survive a
    /// restart (issue #605). `None` means this node writes none, which
    /// [`Config::load`] permits only when `[[client_channels]]` is empty
    /// and so no claim can ever be accepted.
    ///
    /// The directory is not created or probed here: config load says what
    /// was asked for, and whether it can actually be written is
    /// `connector-cli`'s to find out at startup, loudly, before serving.
    pub fn state_dir(&self) -> Option<&Path> {
        self.state_dir.as_deref()
    }
}

/// Parse the one `socks_proxy` value, if the file wrote one (ADR 0070
/// decision 3).
fn resolve_socks_proxy(raw: Option<String>) -> Result<Option<Url>, ConfigError> {
    raw.map(|value| parse_socks_proxy(&value)).transpose()
}

/// **The** `socks_proxy` rule: what a SOCKS5 proxy URL has to be for this
/// connector to dial an onion endpoint through it (ADR 0070 decision 3).
///
/// Three checks, and each is a load-time refusal on purpose, because every
/// failure it prevents is silent:
///
/// * it has to parse as a URL -- the usual mistake is a bare `host:port`,
///   which is exactly the shape every other SOCKS-taking tool accepts;
/// * the scheme has to be `socks5h`. A `socks5://` proxy asks the *client*
///   to resolve the hostname and hands the proxy an address, and nothing on
///   this machine can resolve a `.onion` name -- so a node that accepted one
///   would come up clean, serve, and then fail every onion dial with a
///   resolver error naming a host the operator can see is spelled correctly;
/// * it has to name a host. `socks5h` is not a *special* scheme in the URL
///   standard, so -- unlike every `https://` value in a config file --
///   `socks5h://` parses with an empty host, and `socks5h:9050` parses as a
///   scheme plus an opaque path rather than as the `host:port` it looks
///   like. Either would load and then fail every onion dial on a proxy
///   address that is not one.
///
/// **Public because `connector send` takes the same value as a flag.** That
/// verb loads no config file (ADR 0070 decision 5), so it cannot reach
/// [`Config::socks_proxy`] -- but it must not reach a *second rule* either.
/// It calls this and renders the [`ConfigError`] into its own usage error,
/// so there is one implementation of what a proxy URL is and one set of
/// reasons an operator is given for a bad one.
pub fn parse_socks_proxy(value: &str) -> Result<Url, ConfigError> {
    let url = Url::parse(value).map_err(|source| ConfigError::SocksProxyInvalidUrl {
        value: value.to_string(),
        source,
    })?;
    if url.scheme() != "socks5h" {
        return Err(ConfigError::SocksProxyScheme {
            value: value.to_string(),
            scheme: url.scheme().to_string(),
        });
    }
    if url.host_str().is_none() {
        return Err(ConfigError::SocksProxyNoHost {
            value: value.to_string(),
        });
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::{PeerCarriage, DEFAULT_MAX_PACKET_AMOUNT};
    use crate::route::TransportPolicy;
    use crate::settlement::SettlementChain;
    use connector_domain::{AssetId, Price};
    use std::io::Write;
    use std::path::PathBuf;

    /// The operator doc every peer-schema error message must name -- a
    /// peering that does not come up produces no other evidence an
    /// operator can read, so "which field changed, and where is that
    /// written down" has to be in the message itself.
    const BRINGUP_DOC: &str = "docs/operators/btp-peer-transport-bringup.md";

    fn with_key_file(body: impl FnOnce(&Path) -> String) -> Result<Config, ConfigError> {
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not a real key")
            .expect("write key file");
        let text = body(key_file.path());
        Config::from_toml_str(&text, Path::new("test.toml"))
    }

    #[test]
    fn loads_a_minimal_valid_config() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        })
        .expect("load");

        assert_eq!(
            config.client_edge_addr(),
            "127.0.0.1:3000".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(config.routes().len(), 0);
    }

    #[test]
    fn loads_routes_and_expanded_children() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
apex = "g.example.connector"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.example.other"
handler_url = "http://localhost:5000"
price = 25

[[children]]
name = "billing"
handler_url = "http://localhost:4000"
price = 0
"#,
                key_path.display()
            )
        })
        .expect("load");

        let prefixes: Vec<&str> = config.routes().iter().map(|r| r.prefix()).collect();
        assert_eq!(
            prefixes,
            vec!["g.example.other", "g.example.connector.billing"]
        );
        let prices: Vec<Price> = config.routes().iter().map(|r| r.price()).collect();
        assert_eq!(prices, vec![Price::flat(25), Price::FREE]);
    }

    /// Issue #1210: a route declares what a client should send it, as an
    /// arbitrary table the connector never reads a key out of -- verified
    /// here through a real TOML file, the shape an operator actually
    /// writes, nested table and array and all.
    #[test]
    fn loads_a_routes_request_table() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.toon.gas"
handler_url = "http://localhost:4000"
price = 1000

[routes.request]
protocol = "nip90"
kinds = [5096, 5098]

[routes.request.params]
chain = ["evm:84532"]
"#,
                key_path.display()
            )
        })
        .expect("load");

        assert_eq!(
            config.routes()[0].request(),
            Some(&serde_json::json!({
                "protocol": "nip90",
                "kinds": [5096, 5098],
                "params": { "chain": ["evm:84532"] },
            }))
        );
    }

    /// A route that configures no `request` publishes exactly the document
    /// it published before this issue -- `None`, not an absent-but-implied
    /// empty table.
    #[test]
    fn a_route_with_no_request_table_loads_with_none() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.example.app"
handler_url = "http://localhost:4000"
price = 1000
"#,
                key_path.display()
            )
        })
        .expect("load");

        assert_eq!(config.routes()[0].request(), None);
    }

    /// `request` must be a table -- any other shape is refused by name at
    /// load, the same treatment every other mistyped value in this file
    /// gets, rather than being read and silently misinterpreted.
    #[test]
    fn a_non_table_request_is_refused_by_name() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.example.app"
handler_url = "http://localhost:4000"
price = 1000
request = "x"
"#,
                key_path.display()
            )
        });

        assert_names_the_unknown_key(result, "request");
    }

    /// ADR 0065 (issue #984): a route's `price` may be written as a table
    /// carrying a slope, and survives a real TOML file through
    /// `Config::load` -- the shape an operator actually writes, which the
    /// `resolve_routes` tests in `route.rs` cannot reach because they start
    /// from an already-parsed `Price`.
    #[test]
    fn loads_a_route_priced_by_a_schedule() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.example.store"
handler_url = "http://localhost:5000"
price = {{ base = 1000, per_kib = 30 }}

[[routes]]
prefix = "g.example.quotes"
handler_url = "http://localhost:5001"
price = 1000
"#,
                key_path.display()
            )
        })
        .expect("load");

        assert_eq!(config.routes()[0].price(), Price::scheduled(1000, 30));
        assert_eq!(config.routes()[0].price().charge(100 * 1024), 4_000);
        // The flat spelling is untouched and still means what it meant.
        assert_eq!(config.routes()[1].price(), Price::flat(1000));
        assert!(config.routes()[1].price().is_flat());
    }

    /// A price table exists only to carry a slope, so omitting `per_kib` is
    /// refused **by name** rather than defaulted to zero: defaulting would
    /// let a route an operator meant to charge by size go out flat, losing
    /// exactly the money issue #984 is about, and silently.
    #[test]
    fn a_price_table_with_no_slope_is_refused_by_name() {
        let error = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.example.store"
handler_url = "http://localhost:5000"
price = {{ base = 1000 }}
"#,
                key_path.display()
            )
        })
        .expect_err("a slopeless price table is refused");

        let message = error.to_string();
        assert!(message.contains("per_kib"), "got: {message}");
    }

    /// `deny_unknown_fields` closes the mistyped-key hole on the route row;
    /// the price table closes its own, and names the key it did not know.
    #[test]
    fn an_unknown_key_in_a_price_table_is_refused_by_name() {
        let error = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.example.store"
handler_url = "http://localhost:5000"
price = {{ base = 1000, per_byte = 1 }}
"#,
                key_path.display()
            )
        })
        .expect_err("an unknown price key is refused");

        let message = error.to_string();
        assert!(message.contains("per_byte"), "got: {message}");
    }

    /// A `[[children]]` entry takes the same spelling, so the convenience
    /// form is not a second price grammar to keep in step. (The forwarded
    /// branch is covered at `resolve_routes` in `route.rs`, which does not
    /// need a whole peering and its channel to say the same thing.)
    #[test]
    fn a_child_takes_the_same_schedule_spelling_a_route_does() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
apex = "g.example.connector"

[signer]
key_file = "{}"

[[children]]
name = "billing"
handler_url = "http://localhost:4000"
price = {{ base = 7, per_kib = 3 }}
"#,
                key_path.display()
            )
        })
        .expect("load");

        assert_eq!(config.routes()[0].prefix(), "g.example.connector.billing");
        assert_eq!(config.routes()[0].price(), Price::scheduled(7, 3));
    }

    /// Issue #701: a route's `transport` field survives a real TOML file
    /// through `Config::load`, and a route that omits it defaults to
    /// accepting both -- matching the devnet shape (relay restricted to
    /// BTP, store left at the default).
    #[test]
    fn loads_a_route_restricted_to_btp_alongside_one_accepting_both() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.example.relay"
handler_url = "http://localhost:5000"
price = 1000
transport = "btp"

[[routes]]
prefix = "g.example.store"
handler_url = "http://localhost:6000"
price = 1000
"#,
                key_path.display()
            )
        })
        .expect("load");

        let policies: Vec<TransportPolicy> = config
            .routes()
            .iter()
            .map(|r| r.transport_policy())
            .collect();
        assert_eq!(policies, vec![TransportPolicy::Btp, TransportPolicy::Both]);
    }

    #[test]
    fn rejects_a_terminated_route_with_no_price() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.example.other"
handler_url = "http://localhost:5000"
"#,
                key_path.display()
            )
        });

        assert!(matches!(result, Err(ConfigError::RouteMissingPrice { .. })));
    }

    // -- The peer carriage config surface (issue #677,
    // `peer-carriage-spec.md` §11) --

    const PEER_CHANNEL: &str = "0xaaaabbbbccccddddeeeeffff00001111aaaabbbbccccddddeeeeffff00001111";
    const PEER_KEY: &str = "0x2222222222222222222222222222222222222222";
    const PEER_TOKEN_NETWORK: &str = "0x3333333333333333333333333333333333333333";

    /// An `[settlement.evm]` table and its key, in the shape a channel row
    /// on this chain now requires (issue #1138): that table is where this
    /// node's EVM address comes from, and a channel row names it as this
    /// node's on-chain participant. Written once here rather than inline
    /// in each fixture, since no test below is *about* how a settlement
    /// table parses.
    fn evm_settlement(key_path: &Path) -> String {
        format!(
            r#"
[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6

[settlement.evm.key]
key_file = "{key_file}"
"#,
            key_file = key_path.display(),
        )
    }

    /// A `[[peers]]`/`[[peer_channels]]` pair in its correct shape, the
    /// one an operator should be able to copy. Every negative test below
    /// spoils exactly one thing about it, so what each error is *about* is
    /// the diff between it and this.
    ///
    /// It carries `[settlement.evm]` because since issue #1138 an EVM
    /// channel row without one does not load: a peer claim is redeemed by
    /// the channel's on-chain participant and that address is this table's
    /// key, so a peering bound to an EVM channel on a node with no EVM
    /// settlement is bound on paper only.
    ///
    /// And it carries `[[pay_channels]]` because since issue #1145 a
    /// peering a `[[routes]]` entry FORWARDS to does not load without one:
    /// a connector covers every PREPARE it sends (ADR 0042), and there is
    /// no postpay path left for an uncovered forward to fall back to. One
    /// channel in both roles with one hop is the deployed shape, so it is
    /// the peer row's own channel.
    fn peering_config(key_path: &Path, state_dir: &Path, spoil: &str) -> String {
        let base = format!(
            r#"
client_edge_addr = "127.0.0.1:3000"
peer_expose = "btp"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"
{settlement}
[[peers]]
id = "store"
endpoint = "wss://store.example:443/btp"
fee = 3

[[peer_channels]]
peer_id = "store"
channel_id = "{PEER_CHANNEL}"
counterparty_key = "{PEER_KEY}"
chain_id = 31337
token_network = "{PEER_TOKEN_NETWORK}"

[[routes]]
prefix = "g.example.store"
peer_id = "store"
price = 1000

[[pay_channels]]
peer_id = "store"
channel_id = "{PEER_CHANNEL}"
chain_id = 31337
token_network = "{PEER_TOKEN_NETWORK}"
client_edge_url = "https://store.example/ilp"
"#,
            state_dir = state_dir.display(),
            key_file = key_path.display(),
            settlement = evm_settlement(key_path),
        );
        format!("{base}{spoil}")
    }

    /// [`peering_config`]'s text with its `[[pay_channels]]` row cut off:
    /// what the file looked like before issue #1145 made the row required,
    /// and the base a test writes its own row onto.
    fn without_pay_channel(text: String) -> String {
        text.split_once("\n[[pay_channels]]")
            .expect("peering_config writes a pay-channel row")
            .0
            .to_string()
    }

    /// Load `peering_config` with `edit` applied to its text -- the
    /// spoil-one-thing helper the named-error tests share.
    fn load_peering(edit: impl Fn(String) -> String) -> Result<Config, ConfigError> {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not a real key")
            .expect("write key file");
        let text = edit(peering_config(key_file.path(), state_dir.path(), ""));
        Config::from_toml_str(&text, Path::new("test.toml"))
    }

    /// The whole surface round-trips from a real TOML file: the exposure
    /// set, the endpoint and the carriage its scheme selects, the
    /// per-relation terms and their defaults, and the channel binding that
    /// makes the peering a peering at all.
    #[test]
    fn loads_the_full_peer_and_peer_channels_shape() {
        let config = load_peering(|text| text).expect("load");

        assert_eq!(config.peer_expose(), PeerExposure::Btp);
        assert!(config.peer_expose().exposes(PeerCarriage::Btp));
        assert!(!config.peer_expose().exposes(PeerCarriage::Http));

        assert_eq!(config.peers().len(), 1);
        let peer = &config.peers()[0];
        assert_eq!(peer.id(), "store");
        assert_eq!(
            peer.endpoint().map(url::Url::as_str),
            Some("wss://store.example/btp")
        );
        assert_eq!(peer.dial(), Some(PeerCarriage::Btp));
        assert_eq!(peer.claim_ack_timeout_ms(), 30_000);
        assert_eq!(peer.peer_answer_timeout_ms(), 30_000);
        assert!(peer.can_originate());

        assert_eq!(config.peer_channels().len(), 1);
        let channel = &config.peer_channels()[0];
        assert_eq!(channel.peer_id(), "store");
        let PeerChannelConfig::Evm(evm) = channel else {
            panic!("expected an EVM peer channel");
        };
        assert_eq!(evm.channel_id(), PEER_CHANNEL);
        assert_eq!(evm.counterparty_key(), [0x22u8; 20]);
        assert_eq!(evm.chain_id(), 31_337);
        assert_eq!(evm.token_network(), [0x33u8; 20]);
        assert_eq!(config.peer_channels_for("store").count(), 1);
        assert_eq!(config.peer_channels_for("nobody").count(), 0);

        assert_eq!(config.peer_routes().len(), 1);
        assert_eq!(config.peer_routes()[0].peer_id(), "store");
        assert_eq!(config.peer_routes()[0].price(), Price::flat(1000));
        // ADR 0061: the fee rode in on the `[[peers]]` row, not the route.
        assert_eq!(config.peers()[0].fee(), 3);
    }

    /// ADR 0042's cap round-trips from a real TOML file, and a file that
    /// says nothing about it still comes back bounded.
    #[test]
    fn a_written_max_packet_amount_round_trips_and_an_omitted_one_defaults() {
        let defaulted = load_peering(|text| text).expect("load");
        assert_eq!(
            defaulted.peers()[0].max_packet_amount(),
            DEFAULT_MAX_PACKET_AMOUNT
        );

        let written = load_peering(|text| {
            text.replace(
                "endpoint = \"wss://store.example:443/btp\"",
                "endpoint = \"wss://store.example:443/btp\"\nmax_packet_amount = 250000",
            )
        })
        .expect("load");
        assert_eq!(written.peers()[0].max_packet_amount(), 250_000);
    }

    /// A cap of zero is a peering that refuses every packet, so it is
    /// refused at load with a message naming the peer and the rule.
    #[test]
    fn rejects_a_max_packet_amount_of_zero() {
        let result = load_peering(|text| {
            text.replace(
                "endpoint = \"wss://store.example:443/btp\"",
                "endpoint = \"wss://store.example:443/btp\"\nmax_packet_amount = 0",
            )
        });

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::PeerMaxPacketAmountZero { id } if id == "store"),
        );
        assert!(
            message.contains("max_packet_amount = 0") && message.contains("ONE packet"),
            "got: {message}"
        );
    }

    /// A negative cap is not a smaller cap: `max_packet_amount` is an
    /// unsigned amount, and a file that writes one is refused rather than
    /// wrapped around into an enormous one.
    #[test]
    fn rejects_a_negative_max_packet_amount() {
        let result = load_peering(|text| {
            text.replace(
                "endpoint = \"wss://store.example:443/btp\"",
                "endpoint = \"wss://store.example:443/btp\"\nmax_packet_amount = -1",
            )
        });

        assert!(
            matches!(result, Err(ConfigError::Parse { .. })),
            "{result:?}"
        );
    }

    /// An `https://` peer rides the HTTP carriage instead -- the scheme is
    /// the *only* thing that decides it (§2.1).
    #[test]
    fn an_https_endpoint_selects_the_http_carriage() {
        let config = load_peering(|text| {
            text.replace("wss://store.example:443/btp", "https://store.example/ilp")
        })
        .expect("load");

        assert_eq!(config.peers()[0].dial(), Some(PeerCarriage::Http));
    }

    /// `peer_expose = "neither"` is the NAT'd operator: it exposes nothing
    /// and only dials, which is legal and must stay expressible.
    #[test]
    fn a_natd_operator_exposes_neither_carriage_and_still_loads() {
        let config =
            load_peering(|text| text.replace("peer_expose = \"btp\"", "peer_expose = \"neither\""))
                .expect("load");

        assert_eq!(config.peer_expose(), PeerExposure::Neither);
        assert!(config.peer_expose().is_empty());
        assert!(config.peers()[0].can_originate());
    }

    /// Omitting `peer_expose` entirely is the same as `"neither"`: a peer
    /// listener is opened only by a line somebody wrote.
    #[test]
    fn an_omitted_peer_expose_defaults_to_neither() {
        let config =
            load_peering(|text| text.replace("peer_expose = \"btp\"\n", "")).expect("load");

        assert_eq!(config.peer_expose(), PeerExposure::Neither);
    }

    /// §11 `PeerUndialable`: nothing to dial, and nothing to be dialed on.
    #[test]
    fn rejects_a_peering_that_can_never_establish() {
        let result = load_peering(|text| {
            text.replace("peer_expose = \"btp\"", "peer_expose = \"neither\"")
                .replace("endpoint = \"wss://store.example:443/btp\"\n", "")
        });

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::PeerUndialable { id } if id == "store"),
        );
        assert!(
            message.contains("can never establish") && message.contains(BRINGUP_DOC),
            "got: {message}"
        );
    }

    /// §11 `PeerEndpointScheme`: a scheme that names no carriage. `ws://`
    /// is the interesting spelling -- it is a real websocket scheme, just
    /// not a TLS one, and a peering carries signed balance proofs.
    #[test]
    fn rejects_an_endpoint_scheme_that_selects_no_carriage() {
        for (written, scheme) in [
            ("ws://store.example/btp", "ws"),
            ("http://store.example/ilp", "http"),
            ("tcp://store.example:4001", "tcp"),
        ] {
            let result = load_peering(|text| text.replace("wss://store.example:443/btp", written));

            let message = expect_error(result, |error| {
                matches!(error, ConfigError::PeerEndpointScheme { id, scheme: s, .. }
                    if id == "store" && s == scheme)
            });
            assert!(
                message.contains("selects no peer carriage")
                    && message.contains("wss://")
                    && message.contains(BRINGUP_DOC),
                "got: {message}"
            );
        }
    }

    /// Issue #678, gap 3: `peer_allow_plaintext_endpoints` widens which
    /// **schemes** resolve, never what they resolve to. `ws://` selects the
    /// same BTP carriage `wss://` does and `http://` the same
    /// ILP-over-HTTP one, so a harness can point one connector at another's
    /// loopback socket without a TLS terminator -- and a scheme that names
    /// no carriage at all is still refused, switch or no switch.
    #[test]
    fn the_plaintext_opt_in_resolves_ws_and_http_onto_the_same_two_carriages() {
        for (written, carriage) in [
            ("ws://store.example/btp", PeerCarriage::Btp),
            ("http://store.example/ilp", PeerCarriage::Http),
        ] {
            let config = load_peering(|text| {
                text.replace("wss://store.example:443/btp", written)
                    .replace(
                        "peer_expose = \"btp\"",
                        "peer_expose = \"btp\"\npeer_allow_plaintext_endpoints = true",
                    )
            })
            .expect("a plaintext endpoint loads once the node has opted in");

            assert!(config.peer_allow_plaintext_endpoints());
            assert_eq!(config.peers()[0].dial(), Some(carriage));
            assert_eq!(
                config
                    .plaintext_peerings()
                    .map(|(id, endpoint)| (id.to_string(), endpoint.as_str().to_string()))
                    .collect::<Vec<_>>()
                    .len(),
                1,
                "a node that opted in must be able to name every peering it dials in the clear"
            );
        }

        let result = load_peering(|text| {
            text.replace("wss://store.example:443/btp", "tcp://store.example:4001")
                .replace(
                    "peer_expose = \"btp\"",
                    "peer_expose = \"btp\"\npeer_allow_plaintext_endpoints = true",
                )
        });
        assert!(matches!(
            result,
            Err(ConfigError::PeerEndpointScheme { .. })
        ));
    }

    /// The default is off, and off is the production answer: a config that
    /// does not mention the field refuses `ws://` exactly as it did before
    /// the field existed -- which is what
    /// `rejects_an_endpoint_scheme_that_selects_no_carriage` above asserts,
    /// asserted here from the switch's own side.
    #[test]
    fn the_plaintext_opt_in_is_off_unless_a_config_says_otherwise() {
        let config = load_peering(|text| text).expect("load");

        assert!(!config.peer_allow_plaintext_endpoints());
        assert_eq!(config.plaintext_peerings().count(), 0);
    }

    /// A minimal config plus whatever lines `extra` adds -- enough to
    /// exercise a top-level scalar without dragging a peering in, since
    /// `socks_proxy` is a node-wide value and validating it needs no peer
    /// at all.
    fn load_with_extra(extra: &str) -> Result<Config, ConfigError> {
        with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
{extra}

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        })
    }

    /// ADR 0070 decision 3: the one proxy an operator writes down loads and
    /// reads back off the loaded `Config`, unchanged.
    #[test]
    fn loads_a_socks5h_proxy() {
        let config = load_with_extra(r#"socks_proxy = "socks5h://127.0.0.1:9050""#).expect("load");

        assert_eq!(
            config.socks_proxy().map(Url::as_str),
            Some("socks5h://127.0.0.1:9050")
        );
    }

    /// Absent is the default and the shape of every node that does not run
    /// an onion sidecar -- which is every committed config in this
    /// repository.
    #[test]
    fn a_config_with_no_socks_proxy_dials_direct() {
        let config = load_with_extra("").expect("load");

        assert!(config.socks_proxy().is_none());
    }

    /// The refusal ADR 0070 decision 3 exists for. `socks5://` is what
    /// every other SOCKS-taking tool spells, and it is the one value that
    /// would load and then break exactly the peerings the proxy was
    /// configured for -- so the message has to carry the reason, not just
    /// the rule.
    #[test]
    fn rejects_a_socks_proxy_whose_scheme_drops_the_h() {
        for written in ["socks5://127.0.0.1:9050", "http://127.0.0.1:9050"] {
            let result = load_with_extra(&format!("socks_proxy = \"{written}\""));

            let message = expect_error(
                result,
                |error| matches!(error, ConfigError::SocksProxyScheme { value, .. } if value == written),
            );
            assert!(
                message.contains("socks5h")
                    && message.contains(".onion")
                    && message.contains("LOCALLY"),
                "the message must say why the 'h' is not a preference, got: {message}"
            );
        }
    }

    /// A separate variant from the scheme error, for the same reason a peer
    /// endpoint has two: "you wrote a bare host:port" and "you wrote the
    /// wrong scheme" are different mistakes with different fixes. This is
    /// also the verdict `documented_config_keys.rs`'s probe gets when it
    /// hands the key its dummy value, so it must not read as a key the
    /// parser does not know.
    #[test]
    fn rejects_a_socks_proxy_that_is_not_a_url_at_all() {
        let result = load_with_extra(r#"socks_proxy = "127.0.0.1:9050""#);

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::SocksProxyInvalidUrl { value, .. } if value == "127.0.0.1:9050"),
        );
        let lowered = message.to_lowercase();
        assert!(
            !lowered.contains("unknown field") && !lowered.contains("was removed"),
            "socks_proxy is a key this parser knows; its message must not read as one it \
             does not, got: {message}"
        );
    }

    /// A value that parses but names no proxy is refused too. `socks5h` is
    /// not a *special* URL scheme, so both of these load as far as `Url` is
    /// concerned -- and a loaded `Config` is supposed to need no further
    /// validation anywhere (ADR 0009), so "it parsed" is not the bar.
    #[test]
    fn rejects_a_socks_proxy_that_names_no_host() {
        for written in [
            // The scheme and nothing else.
            "socks5h://",
            // Looks like a `host:port`, and is a scheme plus an opaque
            // path -- the likelier of the two to be written by hand.
            "socks5h:9050",
        ] {
            let result = load_with_extra(&format!("socks_proxy = \"{written}\""));
            let message = expect_error(
                result,
                |error| matches!(error, ConfigError::SocksProxyNoHost { value } if value == written),
            );
            assert!(
                message.contains("socks5h://<host>:<port>"),
                "the message has to show the shape that works, got: {message}"
            );
        }
    }

    /// The old shape was a `SocketAddr`, so URL parsing is new and its
    /// failures need a name of their own -- separate from the scheme
    /// error, because "you wrote a host:port" and "you wrote the wrong
    /// scheme" are different mistakes with different fixes.
    #[test]
    fn rejects_an_endpoint_that_is_not_a_url_at_all() {
        let result =
            load_peering(|text| text.replace("wss://store.example:443/btp", "127.0.0.1:4001"));

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::InvalidPeerEndpoint { id, .. } if id == "store"),
        );
        assert!(
            message.contains("is a URL") && message.contains(BRINGUP_DOC),
            "got: {message}"
        );
    }

    /// A `wss://` URL with no host is refused as an unparseable endpoint
    /// (the URL standard makes a host mandatory for both of our schemes),
    /// which is the same named error and the same message as any other
    /// malformed one.
    #[test]
    fn rejects_an_endpoint_with_no_host_to_dial() {
        let result =
            load_peering(|text| text.replace("wss://store.example:443/btp", "wss://:443/btp"));

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::InvalidPeerEndpoint { id, .. } if id == "store"),
        );
        assert!(message.contains("is a URL"), "got: {message}");
    }

    /// ADR 0060: `credential` is a tombstone, refused by name from a real
    /// TOML file -- every spelling of it, including the `secret_file` form
    /// a deployed node used to write.
    #[test]
    fn rejects_a_peer_that_still_writes_a_credential() {
        for written in [
            "credential = { secret = \"shared-secret\" }",
            "credential = { secret_file = \"/app/data/store-peer.secret\" }",
            "credential = {}",
        ] {
            let result = load_peering(|text| {
                text.replace(
                    "endpoint = \"wss://store.example:443/btp\"",
                    &format!("endpoint = \"wss://store.example:443/btp\"\n{written}"),
                )
            });

            let message = expect_error(
                result,
                |error| matches!(error, ConfigError::PeerCredentialRemoved { id } if id == "store"),
            );
            assert!(
                message.contains("ADR 0060") && message.contains("verified claim"),
                "{written}: got {message}"
            );
        }
    }

    /// §11 `PeerChannelUnbound`: P2 of the role rule. This is the exact
    /// defect that made ADR 0024 inert.
    #[test]
    fn rejects_a_peer_with_no_channel_binding() {
        let result = load_peering(|text| {
            let (head, rest) = text.split_once("[[peer_channels]]").expect("fixture");
            let (_, routes) = rest.split_once("[[routes]]").expect("fixture");
            format!("{head}[[routes]]{routes}")
        });

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::PeerChannelUnbound { id } if id == "store"),
        );
        assert!(
            message.contains("a channel binding") && message.contains(BRINGUP_DOC),
            "got: {message}"
        );
    }

    /// §11 `PeerChannelOrphaned`: a binding to a peering that does not
    /// exist.
    #[test]
    fn rejects_a_peer_channel_naming_an_unconfigured_peer() {
        let result = load_peering(|text| {
            text.replace(
                "peer_id = \"store\"\nchannel_id",
                "peer_id = \"ghost\"\nchannel_id",
            )
        });

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::PeerChannelOrphaned { peer_id } if peer_id == "ghost"),
        );
        assert!(
            message.contains("no '[[peers]]' entry configures") && message.contains(BRINGUP_DOC),
            "got: {message}"
        );
    }

    const SOLANA_CHANNEL_ACCOUNT: &str = "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi";
    const SOLANA_COUNTERPARTY_KEY: &str = "8pM1DN3RiT8vbom5u1sNryaNT1nyL8CTTW3b5PwWXRBH";
    const SOLANA_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

    /// A `[[peers]]`/Solana-shaped `[[peer_channels]]` pair, the Solana
    /// counterpart of `peering_config` (issue #759).
    ///
    /// `program_id_line` is what a file that still writes the key removed by
    /// issue #1128 looks like; `settlement_program_id` is the
    /// `[settlement.solana]` table the row now takes its program from, and
    /// `None` omits the table entirely.
    fn solana_peering_config(
        key_path: &Path,
        state_dir: &Path,
        program_id_line: &str,
        settlement_program_id: Option<&str>,
    ) -> String {
        let settlement = settlement_program_id.map_or_else(String::new, |program_id| {
            format!(
                r#"
[settlement.solana]
rpc_url = "https://api.devnet.solana.com"
program_id = "{program_id}"
token_address = "{SOLANA_COUNTERPARTY_KEY}"
decimals = 6

[settlement.solana.key]
key_file = "{key_file}"
"#,
                key_file = key_path.display(),
            )
        });
        format!(
            r#"
client_edge_addr = "127.0.0.1:3000"
peer_expose = "btp"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[[peers]]
id = "store"
endpoint = "wss://store.example:443/btp"

[[peer_channels]]
peer_id = "store"
channel_account = "{SOLANA_CHANNEL_ACCOUNT}"
counterparty_key = "{SOLANA_COUNTERPARTY_KEY}"
{program_id_line}
{settlement}
"#,
            state_dir = state_dir.display(),
            key_file = key_path.display(),
        )
    }

    /// The ordinary case: no per-row `program_id`, and a
    /// `[settlement.solana]` naming [`SOLANA_PROGRAM_ID`].
    fn load_solana_peering(program_id_line: &str) -> Result<Config, ConfigError> {
        load_solana_peering_settling_under(program_id_line, Some(SOLANA_PROGRAM_ID))
    }

    fn load_solana_peering_settling_under(
        program_id_line: &str,
        settlement_program_id: Option<&str>,
    ) -> Result<Config, ConfigError> {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not a real key")
            .expect("write key file");
        let text = solana_peering_config(
            key_file.path(),
            state_dir.path(),
            program_id_line,
            settlement_program_id,
        );
        Config::from_toml_str(&text, Path::new("test.toml"))
    }

    /// Issue #759's AC as issue #1128 leaves it: a Solana
    /// `[[peer_channels]]` row loads, is typed distinctly from an EVM one,
    /// and carries the program id `[settlement.solana]` names -- at the
    /// full `Config::load` level, which is where the two tables meet.
    #[test]
    fn loads_a_solana_peer_channel_with_the_settlement_tables_program_id() {
        let config = load_solana_peering("").expect("load");

        assert_eq!(config.peer_channels().len(), 1);
        let PeerChannelConfig::Solana(solana) = &config.peer_channels()[0] else {
            panic!("expected a Solana peer channel");
        };
        assert_eq!(solana.peer_id(), "store");
        assert_eq!(solana.channel_account(), SOLANA_CHANNEL_ACCOUNT);
        assert_eq!(solana.counterparty_key(), SOLANA_COUNTERPARTY_KEY);
        assert_eq!(solana.program_id(), SOLANA_PROGRAM_ID);
        assert_eq!(config.peer_channels()[0].chain(), SettlementChain::Solana);
    }

    /// Issue #1128, at the level an operator meets it: the config file that
    /// used to produce a node verifying peer claims under one program while
    /// settling under another does not load at all, and the message names
    /// the key to delete.
    #[test]
    fn rejects_a_solana_peer_channel_that_still_declares_its_own_program_id() {
        let result = load_solana_peering_settling_under(
            r#"program_id = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM""#,
            Some(SOLANA_PROGRAM_ID),
        );

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::PeerChannelProgramIdRemoved { peer_id } if peer_id == "store"),
        );
        assert!(
            message.contains("program_id")
                && message.contains("[settlement.solana] program_id")
                && message.contains("never settle"),
            "got: {message}"
        );
    }

    /// The other half of #1128's refusal: no `[settlement.solana]` at all
    /// means there is no program id anywhere, so the row cannot be bound --
    /// and is refused rather than skipped, because `PeerChannelUnbound`
    /// already guarantees every peering has a row and a skipped one would
    /// leave the peering bound on paper only.
    #[test]
    fn rejects_a_solana_peer_channel_on_a_node_that_does_not_settle_on_solana() {
        let result = load_solana_peering_settling_under("", None);

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::PeerChannelWithoutSolanaSettlement { peer_id } if peer_id == "store"),
        );
        assert!(message.contains("[settlement.solana]"), "got: {message}");
    }

    // -- a peering resolves to the token its channel holds (ADR 0071
    // decision 1, issue #1292) --
    //
    // File-level proofs, because the rule is cross-table and no part of it
    // is inside `[[peer_channels]]`: the token comes from the `[settlement]`
    // table that peering's channels settle through, and whether it is a
    // token this node deals comes from `[[tokens]]`.

    /// The ERC-20 `evm_settlement` names, spelled as a `[[tokens]]` row
    /// names one. Checksummed on purpose: a config file is where an
    /// explorer's spelling gets pasted, and an `AssetId` reads it and the
    /// lowercase one as a single token.
    const SETTLEMENT_TOKEN: &str = "evm:0x49beE1Bca5d15Fb0963117923403F9498119a9Ce";
    /// USDC on Base -- some other ERC-20. Declared alone, it makes a node
    /// that deals a token none of its peerings hold.
    const OTHER_TOKEN: &str = "evm:0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
    /// USDC on Solana, the mint `[settlement.solana]` names below.
    const SOLANA_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    /// One `[[tokens]]` row per asset, and nothing else: no rate, no quote,
    /// and therefore no `[rate_guards]` needed (issue #1290). Declaring
    /// tokens alone is exactly what a node crossing chains in one asset
    /// writes, and it is the smallest declaration that turns the rule on.
    fn declaring(assets: &[&str]) -> String {
        assets
            .iter()
            .map(|asset| format!("\n[[tokens]]\nasset = \"{asset}\"\n"))
            .collect()
    }

    /// Both settlement tables at once -- `local/solo`'s shape, and what a
    /// node whose peerings sit on two chains needs before either can be
    /// read as a token.
    fn both_settlements(key_path: &Path) -> String {
        format!(
            r#"{evm}
[settlement.solana]
rpc_url = "https://api.devnet.solana.com"
program_id = "{SOLANA_PROGRAM_ID}"
token_address = "{SOLANA_MINT}"
decimals = 6

[settlement.solana.key]
key_file = "{key_file}"
"#,
            evm = evm_settlement(key_path),
            key_file = key_path.display(),
        )
    }

    /// Two peerings on two chains -- `local/mixed-chain`'s middle node, in
    /// miniature: one bound to an EVM channel, one to a Solana channel, on
    /// a node settling on both. The shape a converting forward reads, since
    /// the two peerings are denominated in two different tokens.
    fn peerings_on_two_chains(key_path: &Path, state_dir: &Path, declaration: &str) -> String {
        format!(
            r#"
client_edge_addr = "127.0.0.1:3000"
peer_expose = "btp"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"
{settlements}
[[peers]]
id = "from-evm"
endpoint = "wss://evm.example:443/btp"

[[peers]]
id = "to-solana"
endpoint = "wss://solana.example:443/btp"

[[peer_channels]]
peer_id = "from-evm"
channel_id = "{PEER_CHANNEL}"
counterparty_key = "{PEER_KEY}"
chain_id = 31337
token_network = "{PEER_TOKEN_NETWORK}"

[[peer_channels]]
peer_id = "to-solana"
channel_account = "{SOLANA_CHANNEL_ACCOUNT}"
counterparty_key = "{SOLANA_COUNTERPARTY_KEY}"
{declaration}
"#,
            state_dir = state_dir.display(),
            key_file = key_path.display(),
            settlements = both_settlements(key_path),
        )
    }

    /// The same two channels bound to **one** peering: an EVM row and a
    /// Solana row both naming `store`. It loads today, and it is the shape
    /// that has no single unit.
    fn one_peering_on_two_chains(key_path: &Path, state_dir: &Path, declaration: &str) -> String {
        peerings_on_two_chains(key_path, state_dir, declaration)
            .replace("peer_id = \"to-solana\"", "peer_id = \"from-evm\"")
            .replace(
                "\n[[peers]]\nid = \"to-solana\"\nendpoint = \"wss://solana.example:443/btp\"\n",
                "",
            )
    }

    fn load_text(text: &str) -> Result<Config, ConfigError> {
        Config::from_toml_str(text, Path::new("test.toml"))
    }

    /// The acceptance criterion itself: a config declaring tokens resolves
    /// every peering to exactly one of them, from loaded config alone --
    /// the `TokenNetwork` its `[[peer_channels]]` row names is never read,
    /// and no chain is asked.
    #[test]
    fn a_peering_resolves_to_the_declared_token_its_channels_hold() {
        let config =
            load_peering(|text| format!("{text}{}", declaring(&[SETTLEMENT_TOKEN]))).expect("load");

        let resolved = config.peering_assets();
        assert!(!resolved.is_empty());
        assert_eq!(
            resolved.asset("store").map(ToString::to_string),
            Some(SETTLEMENT_TOKEN.to_ascii_lowercase()),
            "the peering holds what [settlement.evm] settles in, however the row spelled it"
        );
        assert_eq!(
            resolved
                .iter()
                .map(|(peer_id, _)| peer_id)
                .collect::<Vec<_>>(),
            vec!["store"]
        );
        // A peering with itself is not a boundary, whatever it holds.
        assert_eq!(resolved.boundary_between("store", "store"), None);
    }

    /// The rule that protects every node not doing any of this: no
    /// `[[tokens]]`, nothing resolved, no new required key and no new
    /// refusal -- the same peering config loads, unchanged, and this is the
    /// shape every fixture in this repository is committed in.
    #[test]
    fn a_node_that_declares_no_tokens_resolves_no_peering() {
        let config = load_peering(|text| text).expect("load");

        assert!(config.peering_assets().is_empty());
        assert_eq!(config.peering_assets().asset("store"), None);
        assert_eq!(
            config.peering_assets().boundary_between("store", "store"),
            None
        );
    }

    /// A node that deals is held to it: a peering whose token it never
    /// declared is refused at boot, naming the peering and the token it
    /// holds, rather than reaching a forward that cannot say what unit it
    /// is carrying.
    #[test]
    fn a_peering_holding_an_undeclared_token_is_refused_by_name() {
        let result = load_peering(|text| format!("{text}{}", declaring(&[OTHER_TOKEN])));

        let message = expect_error(result, |error| {
            matches!(
                error,
                ConfigError::PeeringTokenNotDeclared { peer_id, asset }
                    if peer_id == "store"
                        && asset.to_string() == SETTLEMENT_TOKEN.to_ascii_lowercase()
            )
        });
        assert!(
            message.contains("'store'")
                && message.contains(&SETTLEMENT_TOKEN.to_ascii_lowercase())
                && message.contains("[[tokens]]"),
            "got: {message}"
        );
    }

    /// The ordered pair issue #1295 asks for, on the shape it asks it of:
    /// two peerings on two chains hold two tokens, and the answer comes
    /// back in the order asked -- direction is the trade, and the reverse
    /// pair is a different price.
    #[test]
    fn two_peerings_on_two_chains_are_an_ordered_pair_of_tokens() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not a real key")
            .expect("write key file");
        let solana_token = format!("solana:{SOLANA_MINT}");
        let config = load_text(&peerings_on_two_chains(
            key_file.path(),
            state_dir.path(),
            &declaring(&[SETTLEMENT_TOKEN, &solana_token]),
        ))
        .expect("load");

        let resolved = config.peering_assets();
        let evm: AssetId = SETTLEMENT_TOKEN.parse().expect("an asset");
        let solana: AssetId = solana_token.parse().expect("an asset");
        assert_eq!(resolved.asset("from-evm"), Some(&evm));
        assert_eq!(resolved.asset("to-solana"), Some(&solana));
        assert_eq!(
            resolved.boundary_between("from-evm", "to-solana"),
            Some((&evm, &solana))
        );
        assert_eq!(
            resolved.boundary_between("to-solana", "from-evm"),
            Some((&solana, &evm))
        );
    }

    /// One peering, two chains, two tokens: refused, because a packet's
    /// amount is denominated by the channel it rides and this peering rides
    /// two. Reachable from a file that loads today, which is why it is a
    /// named refusal rather than an assumption.
    #[test]
    fn one_peering_whose_channels_sit_on_two_chains_is_refused_by_name() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not a real key")
            .expect("write key file");
        let solana_token = format!("solana:{SOLANA_MINT}");
        let result = load_text(&one_peering_on_two_chains(
            key_file.path(),
            state_dir.path(),
            &declaring(&[SETTLEMENT_TOKEN, &solana_token]),
        ));

        let message = expect_error(result, |error| {
            matches!(
                error,
                ConfigError::PeeringTokenAmbiguous { peer_id, .. } if peer_id == "from-evm"
            )
        });
        assert!(
            message.contains("two tokens")
                && message.contains(&SETTLEMENT_TOKEN.to_ascii_lowercase())
                && message.contains(SOLANA_MINT),
            "got: {message}"
        );
    }

    /// And the same two-chain file with no `[[tokens]]` at all loads, as it
    /// always has: the ambiguity is only a problem for a node that has to
    /// name a unit, and a node that deals nothing never does.
    #[test]
    fn one_peering_on_two_chains_still_loads_when_nothing_is_declared() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not a real key")
            .expect("write key file");
        let config = load_text(&one_peering_on_two_chains(
            key_file.path(),
            state_dir.path(),
            "",
        ))
        .expect("a node that declares nothing is held to nothing");

        assert!(config.peering_assets().is_empty());
    }

    // -- a client channel resolves to the token its chain settles in (ADR
    // 0071 decision 1, issue #1301) --
    //
    // The client edge's half of the rule above, and keyed by CHAIN rather
    // than by declared row: a `[settlement.<chain>]` table is what lets
    // this node accept a claim on a channel no `[[client_channels]]` row
    // names (ADR 0052, issue #502), so every chain with such a table can
    // carry an arrival a forward has to denominate.

    /// A channel key exactly as
    /// `connector_domain::client_claim::ClientClaim::channel_key` renders
    /// one, on a channel this file never mentions. That is the point: the
    /// id is not read, only the namespace before the colon.
    const UNDECLARED_EVM_CHANNEL_KEY: &str =
        "evm:0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// [`peering_config`] plus a second settlement table and a
    /// `[[client_channels]]` row on it -- an EVM peering and a Solana
    /// client edge, which is the shape that separates the two rules: the
    /// peering resolves against the EVM token and the client channel
    /// against the Solana one.
    fn client_channel_on_a_second_chain(key_path: &Path, declaration: &str) -> String {
        format!(
            r#"
[settlement.solana]
rpc_url = "https://api.devnet.solana.com"
program_id = "{SOLANA_PROGRAM_ID}"
token_address = "{SOLANA_MINT}"
decimals = 6

[settlement.solana.key]
key_file = "{key_file}"

[[client_channels]]
channel_account = "{SOLANA_CHANNEL_ACCOUNT}"
counterparty = "{SOLANA_COUNTERPARTY_KEY}"
{declaration}"#,
            key_file = key_path.display(),
        )
    }

    /// The acceptance criterion: a client arrival resolves to the declared
    /// token its channel holds, read off `[settlement.<chain>] token_address`
    /// and nothing declared a second time -- the same rule the peering
    /// table already uses, asked of the client edge.
    #[test]
    fn a_client_channel_resolves_to_the_declared_token_its_chain_settles_in() {
        let config =
            load_peering(|text| format!("{text}{}", declaring(&[SETTLEMENT_TOKEN]))).expect("load");

        let resolved = config.client_channel_assets();
        assert!(!resolved.is_empty());
        assert_eq!(
            resolved
                .asset(UNDECLARED_EVM_CHANNEL_KEY)
                .map(ToString::to_string),
            Some(SETTLEMENT_TOKEN.to_ascii_lowercase()),
            "a channel on the EVM chain holds what [settlement.evm] settles in -- including a \
             channel this file never named, which is the one this rule exists for"
        );
        assert_eq!(
            resolved.asset(&format!("solana:{SOLANA_CHANNEL_ACCOUNT}")),
            None,
            "no [settlement.solana] table, so no claim on a Solana channel could be admitted \
             here and there is nothing to resolve"
        );
    }

    /// The rule that protects every node not doing any of this, restated
    /// for the client edge: no `[[tokens]]`, nothing resolved, and the same
    /// file loads unchanged. This is the shape every config in this
    /// repository is committed in.
    #[test]
    fn a_node_that_declares_no_tokens_resolves_no_client_channel() {
        let config = load_peering(|text| text).expect("load");

        assert!(config.client_channel_assets().is_empty());
        assert_eq!(
            config
                .client_channel_assets()
                .asset(UNDECLARED_EVM_CHANNEL_KEY),
            None
        );
    }

    /// A node that deals is held to it. The peering here resolves fine --
    /// its EVM token is declared -- and the refusal is about the chain the
    /// CLIENT EDGE can be paid on, which nothing else in the file would
    /// have caught. Unresolved, a buyer paying over that chain would have
    /// forwarded across a real boundary at an implied 1:1.
    #[test]
    fn a_client_channel_chain_holding_an_undeclared_token_is_refused_by_name() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not a real key")
            .expect("write key file");
        let text = peering_config(
            key_file.path(),
            state_dir.path(),
            &client_channel_on_a_second_chain(key_file.path(), &declaring(&[SETTLEMENT_TOKEN])),
        );

        let message = expect_error(load_text(&text), |error| {
            matches!(
                error,
                ConfigError::ClientChannelTokenNotDeclared { chain, asset }
                    if *chain == SettlementChain::Solana
                        && asset.to_string() == format!("solana:{SOLANA_MINT}")
            )
        });
        assert!(
            message.contains("solana")
                && message.contains(SOLANA_MINT)
                && message.contains("[[tokens]]"),
            "got: {message}"
        );
    }

    /// And declaring that chain's token is what makes the same file load --
    /// both chains resolved, each to its own settlement table's token, so
    /// every arrival this node can be paid on has a unit.
    #[test]
    fn declaring_both_chains_tokens_resolves_both_client_edges() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not a real key")
            .expect("write key file");
        let solana_token = format!("solana:{SOLANA_MINT}");
        let text = peering_config(
            key_file.path(),
            state_dir.path(),
            &client_channel_on_a_second_chain(
                key_file.path(),
                &declaring(&[SETTLEMENT_TOKEN, &solana_token]),
            ),
        );

        let config = load_text(&text).expect("load");

        let resolved = config.client_channel_assets();
        assert_eq!(
            resolved
                .asset(UNDECLARED_EVM_CHANNEL_KEY)
                .map(ToString::to_string),
            Some(SETTLEMENT_TOKEN.to_ascii_lowercase())
        );
        assert_eq!(
            resolved
                .asset(&format!("solana:{SOLANA_CHANNEL_ACCOUNT}"))
                .map(ToString::to_string),
            Some(solana_token)
        );
    }

    // -- "the settlement table this channel needs is absent" (issue #1138)
    //
    // One rule for all four channel tables, stated in
    // `crate::settlement::SettlementTables` and in
    // `docs/protocol/peer-carriage-spec.md` §11. These are the file-level
    // proofs: what an operator actually meets, and the ordering between the
    // refusals when one file trips more than one.

    /// The EVM half of #1134's rule, at the level an operator meets it: a
    /// peering bound to an EVM channel on a node with no `[settlement.evm]`
    /// does not load. It used to load and verify the peer's inbound claims
    /// under a domain no address this node holds could ever redeem at.
    #[test]
    fn rejects_an_evm_peer_channel_on_a_node_that_does_not_settle_on_evm() {
        let result = load_peering(|text| {
            let start = text.find("[settlement.evm]").expect("the fixture has one");
            let end = text.find("[[peers]]").expect("the fixture has one");
            let mut without = text.clone();
            without.replace_range(start..end, "");
            without
        });

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::PeerChannelWithoutEvmSettlement { peer_id } if peer_id == "store"),
        );
        assert!(
            message.contains("[settlement.evm]") && message.contains("InvalidParticipant"),
            "got: {message}"
        );
    }

    /// The client edge's EVM half, and the answer to the question issue
    /// #1138 called the hard one: **the declared-channel path's latitude
    /// does not extend to redeemability.** `DepositFloor::Unknown` lets an
    /// operator vouch for how much a counterparty may spend on a channel
    /// this node is a participant of; it is not a way to declare a channel
    /// this node has no address to be a participant of.
    #[test]
    fn rejects_an_evm_client_channel_on_a_node_that_does_not_settle_on_evm() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let channel = format!("0x{}", "ab".repeat(32));
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"

[[client_channels]]
channel_id = "{channel}"
counterparty = "0x00000000000000000000000000000000000000aa"
chain_id = 8453
token_network_address = "0x00000000000000000000000000000000000000bb"
"#,
                key_path = key_path.display(),
                state_dir = state_dir.path().display(),
            )
        });

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::ClientChannelWithoutEvmSettlement { channel_id } if *channel_id == channel),
        );
        assert!(
            message.contains("[settlement.evm]") && message.contains("not a policy"),
            "got: {message}"
        );
    }

    /// The client edge's Solana half, which was a `connector-cli`
    /// warn-and-skip: the row loaded, was not recorded, and every claim on
    /// it was then refused as an unknown channel. Refused by name at load
    /// instead, so the two client-edge chains answer the question the same
    /// way and both answer it the way the peer table does.
    #[test]
    fn rejects_a_solana_client_channel_on_a_node_that_does_not_settle_on_solana() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"

[[client_channels]]
channel_account = "{SOLANA_CHANNEL_ACCOUNT}"
counterparty = "{SOLANA_COUNTERPARTY_KEY}"
"#,
                key_path = key_path.display(),
                state_dir = state_dir.path().display(),
            )
        });

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::ClientChannelWithoutSolanaSettlement { channel_account } if channel_account == SOLANA_CHANNEL_ACCOUNT),
        );
        assert!(
            message.contains("[settlement.solana]") && message.contains("ADR 0053"),
            "got: {message}"
        );
    }

    /// A Solana `[[client_channels]]` row carries the program its claims
    /// are judged under, filled in from `[settlement.solana]` (issues
    /// #1082, #1138) rather than looked up again in `connector-cli`. The
    /// value reaching a loaded `Config` is the settlement table's, by
    /// construction.
    #[test]
    fn a_solana_client_channel_takes_its_program_id_from_the_settlement_table() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not a real key")
            .expect("write key file");
        let text = format!(
            "{}\n[[client_channels]]\nchannel_account = \"{SOLANA_COUNTERPARTY_KEY}\"\n\
             counterparty = \"{SOLANA_CHANNEL_ACCOUNT}\"\n",
            solana_peering_config(
                key_file.path(),
                state_dir.path(),
                "",
                Some(SOLANA_PROGRAM_ID),
            ),
        );
        let config = Config::from_toml_str(&text, Path::new("test.toml")).expect("load");

        let ClientChannelConfig::Solana(solana) = &config.client_channels()[0] else {
            panic!("expected a Solana client channel");
        };
        assert_eq!(solana.program_id(), SOLANA_PROGRAM_ID);
    }

    /// The rule is **per chain**: a row needs the table for its own chain
    /// and no other. This is the shape `local/mixed-chain/connector-c.toml`
    /// is committed in -- a Solana peering on a node with no
    /// `[settlement.evm]` at all -- and it must keep loading.
    #[test]
    fn a_solana_only_node_needs_no_evm_settlement_table() {
        let config = load_solana_peering("").expect("load");

        assert_eq!(config.peer_channels().len(), 1);
        assert!(
            config
                .settlements()
                .iter()
                .all(|settlement| settlement.chain() == SettlementChain::Solana),
            "the fixture must have no EVM settlement, or this proves nothing"
        );
    }

    /// The Solana counterpart of `rejects_one_channel_configured_in_both_namespaces`:
    /// the namespace-disjointness rule (§1.8) applies within the Solana
    /// chain too, not just EVM.
    #[test]
    fn rejects_a_solana_channel_configured_in_both_namespaces() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not a real key")
            .expect("write key file");
        let text = format!(
            "{}\n[[client_channels]]\nchannel_account = \"{SOLANA_CHANNEL_ACCOUNT}\"\ncounterparty = \"{SOLANA_COUNTERPARTY_KEY}\"\n",
            solana_peering_config(
                key_file.path(),
                state_dir.path(),
                "",
                Some(SOLANA_PROGRAM_ID),
            ),
        );
        let result = Config::from_toml_str(&text, Path::new("test.toml"));

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::ChannelInBothNamespaces { value } if value == SOLANA_CHANNEL_ACCOUNT),
        );
        assert!(
            message.contains("counted as credit twice"),
            "got: {message}"
        );
    }

    /// §11 `ChannelInBothNamespaces` (§1.8): the check that stops a peer
    /// claim and a client claim describing the same money. Written in
    /// mixed case on one side to prove the comparison is over the
    /// canonical form, not the operator's spelling.
    #[test]
    fn rejects_one_channel_configured_in_both_namespaces() {
        let result = load_peering(|text| {
            format!(
                r#"{text}
[[client_channels]]
channel_id = "{}"
counterparty = "{PEER_KEY}"
chain_id = 31337
token_network_address = "{PEER_TOKEN_NETWORK}"
"#,
                PEER_CHANNEL.to_uppercase().replace("0X", "0x"),
            )
        });

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::ChannelInBothNamespaces { value } if value == PEER_CHANNEL),
        );
        assert!(
            message.contains("counted as credit twice") && message.contains(BRINGUP_DOC),
            "got: {message}"
        );
    }

    // -- `[[pay_channels]]` (ADR 0042 item 2, issue #881) ---------------
    //
    // The cross-table rules. The single-row shape is `pay_channel`'s own
    // unit tests; these are the three things only `Config::load` can see.

    /// The peering of [`peering_config`], plus the `[settlement.evm]` key a
    /// covering claim is signed with and the `[[pay_channels]]` row that
    /// says which channel to sign on -- with `edit` applied to the whole
    /// text, the same spoil-one-thing shape [`load_peering`] uses.
    ///
    /// No `[[client_channels]]` row: that is the collision one test below
    /// adds on purpose.
    fn load_pay_channel_config(edit: impl Fn(String) -> String) -> Result<Config, ConfigError> {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not a real key")
            .expect("write key file");
        // `peering_config` already carries `[settlement.evm]` -- the table
        // an EVM `[[peer_channels]]` row needs since issue #1138, and the
        // same one a covering claim is signed with.
        // The fixture's own row is removed and this one written in its
        // place: `peering_config` has carried a `[[pay_channels]]` row
        // since issue #1145 made it required of a routed peering, and two
        // rows for one peering is `PayChannelDuplicatePeer` -- a different
        // error from any of the ones below.
        let base = without_pay_channel(peering_config(key_file.path(), state_dir.path(), ""));
        let text = format!(
            r#"{base}
[[pay_channels]]
peer_id = "store"
channel_id = "{PAY_CHANNEL}"
chain_id = 31337
token_network = "{PEER_TOKEN_NETWORK}"
client_edge_url = "https://store.example/ilp"
"#
        );
        Config::from_toml_str(&edit(text), Path::new("test.toml"))
    }

    /// A channel that is NOT [`PEER_CHANNEL`]: the pay-from channel and the
    /// peer-role channel may be the same one (that is the deployed shape),
    /// but a test that used one string could not tell the two roles apart.
    const PAY_CHANNEL: &str = "0xccccddddeeeeffff00001111222233334444555566667777888899990000aaaa";

    /// The round trip, from a real TOML file: a `[[pay_channels]]` row
    /// reaches [`Config::pay_channels`] with its channel id canonicalized
    /// and its domain and client edge intact.
    #[test]
    fn loads_the_full_pay_channels_shape() {
        let config = load_pay_channel_config(|text| text).expect("load");

        assert_eq!(config.pay_channels().len(), 1);
        let PayChannelConfig::Evm(pay) = &config.pay_channels()[0] else {
            panic!("an EVM-shaped row resolves to the EVM variant");
        };
        assert_eq!(pay.peer_id(), "store");
        assert_eq!(pay.channel_id(), PAY_CHANNEL);
        assert_eq!(pay.chain_id(), 31_337);
        assert_eq!(pay.token_network(), [0x33u8; 20]);
        assert_eq!(pay.client_edge_url().as_str(), "https://store.example/ilp");
    }

    /// **The row became required, and this is where that is decided**
    /// (issue #1145). ADR 0042 item 2 shipped `[[pay_channels]]` as
    /// additive -- "a peering with nothing configured behaves exactly as it
    /// does now" -- which meant ADR 0004's postpay convention. That
    /// convention is deleted, so the same peering with the table removed no
    /// longer loads: without a channel to pay the hop from,
    /// `forward_via_peer_route` would refuse every packet on that route at
    /// packet time, and turning a runtime surprise into a startup refusal
    /// is what ADR 0009 exists for.
    ///
    /// Keyed on the ROUTE, so the message names both. A peering with no
    /// route to it -- every accept-only peering is one -- owes nothing and
    /// is untouched; `an_accept_only_peering_loads_with_no_ceiling` is that
    /// case and carries no row.
    #[test]
    fn a_peering_this_node_forwards_to_with_no_pay_channels_row_is_refused() {
        let error = load_peering(without_pay_channel)
            .expect_err("a routed peering with nothing to pay it from must not load");

        assert!(
            matches!(
                &error,
                ConfigError::PayChannelUnbound { prefix, peer_id }
                    if prefix == "g.example.store" && peer_id == "store"
            ),
            "{error:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains("[[pay_channels]]") && message.contains("breaking deploy"),
            "the refusal must say what to add and that adding it is a breaking deploy: {message}"
        );
    }

    /// **The collision, by name** (ADR 0030): `[[client_channels]]` is
    /// channels this node RECEIVES on and `[[pay_channels]]` is one it PAYS
    /// from, so one channel in both is the same double-count
    /// `ChannelInBothNamespaces` refuses between the peer and client books.
    /// Written in mixed case on one side to prove the comparison is over
    /// the canonical form rather than the operator's spelling.
    #[test]
    fn rejects_a_pay_channel_that_is_also_a_client_channel() {
        let result = load_pay_channel_config(|text| {
            format!(
                r#"{text}
[[client_channels]]
channel_id = "{}"
counterparty = "{PEER_KEY}"
chain_id = 31337
token_network_address = "{PEER_TOKEN_NETWORK}"
"#,
                PAY_CHANNEL.to_uppercase().replace("0X", "0x"),
            )
        });

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::PayChannelIsAlsoAClientChannel { value } if value == PAY_CHANNEL),
        );
        assert!(
            message.contains("RECEIVES") && message.contains("PAYS"),
            "got: {message}"
        );
    }

    /// The pay-from channel and the peer-role channel with the SAME hop may
    /// be one channel, and this is the test that says so on purpose: the
    /// peer role judges what arrives, the client role covers what this node
    /// sends, and `forward_via_peer_route` never lets both books sign for
    /// one packet.
    #[test]
    fn a_pay_channel_may_be_the_same_channel_as_that_peering_s_peer_channel() {
        let config =
            load_pay_channel_config(|text| text.replace(PAY_CHANNEL, PEER_CHANNEL)).expect("load");

        assert_eq!(config.pay_channels()[0].channel(), PEER_CHANNEL);
    }

    /// A row for a peering that does not exist pays nobody -- the same
    /// typo `PeerChannelOrphaned` catches, on the other table.
    #[test]
    fn rejects_a_pay_channel_naming_an_unconfigured_peer() {
        let result = load_pay_channel_config(|text| {
            text.replace(
                "peer_id = \"store\"\nchannel_id = \"0xcccc",
                "peer_id = \"stroe\"\nchannel_id = \"0xcccc",
            )
        });

        expect_error(
            result,
            |error| matches!(error, ConfigError::PayChannelOrphaned { peer_id } if peer_id == "stroe"),
        );
    }

    /// The signing key is `[settlement.evm]`'s and there is no second one
    /// (ADR 0030's table), so a row with no table to sign under is refused
    /// at load rather than failing every forward it was configured for.
    ///
    /// Built on the **Solana** peering rather than the EVM one, and that is
    /// the shape of the rule rather than a convenience: since issue #1138
    /// an EVM `[[peer_channels]]` row also needs `[settlement.evm]`, and
    /// `PeerChannelUnbound` requires every peering to carry a channel row
    /// -- so the only file that reaches this refusal is one peering over a
    /// chain it does settle on while paying over one it does not.
    #[test]
    fn rejects_a_pay_channel_with_no_evm_settlement_table() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not a real key")
            .expect("write key file");
        let text = format!(
            "{}\n[[pay_channels]]\npeer_id = \"store\"\nchannel_id = \"{PAY_CHANNEL}\"\n\
             chain_id = 31337\ntoken_network = \"{PEER_TOKEN_NETWORK}\"\n\
             client_edge_url = \"https://store.example/ilp\"\n",
            solana_peering_config(
                key_file.path(),
                state_dir.path(),
                "",
                Some(SOLANA_PROGRAM_ID),
            ),
        );
        let result = Config::from_toml_str(&text, Path::new("test.toml"));

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::PayChannelWithoutEvmSettlement { peer_id } if peer_id == "store"),
        );
        assert!(message.contains("no second key"), "got: {message}");
    }

    // -- `[[pay_channels]]`, Solana (issue #1146) -----------------------
    //
    // The table's second chain shape, and the cross-table rules only
    // `Config::load` can see. Until this landed, a Solana peering could not
    // be covered at all and was therefore payable only postpay -- the model
    // ADR 0042 exists to retire.

    /// A Solana peering plus the `[[pay_channels]]` row that pays it, with
    /// `edit` applied to the whole text -- the Solana counterpart of
    /// [`load_pay_channel_config`].
    ///
    /// The pay row names the SAME `channel_account` as the peering's
    /// `[[peer_channels]]` row, which is both the deployed shape and, on
    /// Solana, a load-time requirement: `programId` is a required field of
    /// the claim wire and the peer carriage renders it from that row.
    fn load_solana_pay_channel_config(
        edit: impl Fn(String) -> String,
    ) -> Result<Config, ConfigError> {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not a real key")
            .expect("write key file");
        let base = solana_peering_config(
            key_file.path(),
            state_dir.path(),
            "",
            Some(SOLANA_PROGRAM_ID),
        );
        let text = format!(
            r#"{base}
[[pay_channels]]
peer_id = "store"
channel_account = "{SOLANA_CHANNEL_ACCOUNT}"
client_edge_url = "https://store.example/ilp"
"#
        );
        Config::from_toml_str(&edit(text), Path::new("test.toml"))
    }

    /// The round trip, from a real TOML file: a Solana `[[pay_channels]]`
    /// row reaches [`Config::pay_channels`] typed as such, carrying the
    /// program id `[settlement.solana]` names rather than one it declared.
    #[test]
    fn loads_the_full_solana_pay_channels_shape() {
        let config = load_solana_pay_channel_config(|text| text).expect("load");

        assert_eq!(config.pay_channels().len(), 1);
        let PayChannelConfig::Solana(pay) = &config.pay_channels()[0] else {
            panic!("a Solana-shaped row resolves to the Solana variant");
        };
        assert_eq!(pay.peer_id(), "store");
        assert_eq!(pay.channel_account(), SOLANA_CHANNEL_ACCOUNT);
        assert_eq!(
            pay.program_id(),
            SOLANA_PROGRAM_ID,
            "the program a covering claim is signed under is the one this node settles through \
             (ADR 0053, issue #1128) -- never a second declaration that could drift"
        );
        assert_eq!(pay.client_edge_url().as_str(), "https://store.example/ilp");
        assert_eq!(config.pay_channels()[0].chain(), SettlementChain::Solana);
    }

    /// A Solana pay row whose channel is not also bound as a
    /// `[[peer_channels]]` row is refused **at load**, naming the peer.
    ///
    /// Not a preference: `programId` is a required field of the Solana
    /// claim wire (unlike an EVM claim's optional EIP-712 domain, which
    /// simply rides absent), and both peer carriages render it from that
    /// peering's Solana peer-channel row. Without one, every covering claim
    /// this row minted would reach `claim_json::encode` with nothing to
    /// write there -- a caller bug it panics on, on the packet path, with
    /// the money already committed.
    #[test]
    fn rejects_a_solana_pay_channel_that_is_not_also_a_peer_channel() {
        let result = load_solana_pay_channel_config(|text| {
            text.replace(
                &format!("channel_account = \"{SOLANA_CHANNEL_ACCOUNT}\"\ncounterparty_key"),
                &format!("channel_account = \"{SOLANA_COUNTERPARTY_KEY}\"\ncounterparty_key"),
            )
        });

        let message = expect_error(result, |error| {
            matches!(
                error,
                ConfigError::PayChannelSolanaWithoutPeerChannel { peer_id, value }
                    if peer_id == "store" && value == SOLANA_CHANNEL_ACCOUNT
            )
        });
        assert!(
            message.contains("programId") && message.contains("[[peer_channels]]"),
            "got: {message}"
        );
    }

    /// The Solana half of `rejects_a_pay_channel_with_no_evm_settlement_table`:
    /// no `[settlement.solana]` is both no ed25519 key to sign a covering
    /// claim with and no program id to sign it under.
    #[test]
    fn rejects_a_solana_pay_channel_with_no_solana_settlement_table() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not a real key")
            .expect("write key file");
        let text = format!(
            "{}\n[[pay_channels]]\npeer_id = \"store\"\n\
             channel_account = \"{SOLANA_CHANNEL_ACCOUNT}\"\n\
             client_edge_url = \"https://store.example/ilp\"\n",
            peering_config(key_file.path(), state_dir.path(), ""),
        );
        let result = Config::from_toml_str(&text, Path::new("test.toml"));

        let message = expect_error(result, |error| {
            matches!(
                error,
                ConfigError::PayChannelWithoutSolanaSettlement { peer_id } if peer_id == "store"
            )
        });
        assert!(message.contains("ADR 0030"), "got: {message}");
    }

    /// The namespace rule, on the other chain: `[[client_channels]]` is
    /// channels this node RECEIVES on and `[[pay_channels]]` is one it PAYS
    /// from, so one channel account in both is refused -- and on Solana it
    /// is `ChannelInBothNamespaces` rather than
    /// `PayChannelIsAlsoAClientChannel` that says so.
    ///
    /// That is a consequence of the rule above, not a gap. A Solana pay row
    /// must name a channel the peering also binds as a `[[peer_channels]]`
    /// row, so the peer/client namespace check -- which runs first, and
    /// says the same thing about the same channel -- always gets there
    /// first. Asserted rather than left to be discovered, because "which
    /// error does an operator actually see" is the whole value of refusing
    /// by name.
    #[test]
    fn a_solana_pay_channel_that_is_also_a_client_channel_is_refused_by_the_namespace_rule() {
        let result = load_solana_pay_channel_config(|text| {
            format!(
                r#"{text}
[[client_channels]]
channel_account = "{SOLANA_CHANNEL_ACCOUNT}"
counterparty = "{SOLANA_COUNTERPARTY_KEY}"
"#
            )
        });

        let message = expect_error(result, |error| {
            matches!(
                error,
                ConfigError::ChannelInBothNamespaces { value }
                    if value == SOLANA_CHANNEL_ACCOUNT
            )
        });
        assert!(
            message.contains("counted as credit twice"),
            "got: {message}"
        );
    }

    /// ADR 0031/ADR 0033, issue #882: an accept-only peering used to be
    /// refused with no explicit `ceiling` (§6.4(3)) -- the credit window's
    /// only real bound for a side that cannot originate a flush. That bound
    /// is retired along with the ceiling itself; an accept-only peering now
    /// loads with no ceiling-shaped config at all.
    #[test]
    fn an_accept_only_peering_loads_with_no_ceiling() {
        let config =
            load_peering(|text| text.replace("endpoint = \"wss://store.example:443/btp\"\n", ""))
                .expect("load");

        assert_eq!(config.peers()[0].dial(), None);
    }

    /// §11's removed-field row, `ceiling` half (ADR 0033, issue #882): a
    /// devnet box's bind-mounted TOML that still sets it gets a named error,
    /// not a silent unknown-field drop.
    ///
    /// Asserts on **ADR 0033**, the record that removed the machinery -- not
    /// on the reasoning behind it (issue #1068). This assertion previously
    /// pinned "ADR 0031", which is superseded in full by ADR 0042 and whose
    /// covering-claim rule is still unbuilt for forwarded arrivals; pinning
    /// it is how the wrong citation survived in a message an operator reads
    /// when their node refuses to boot.
    #[test]
    fn rejects_a_peering_that_still_sets_ceiling() {
        let result = load_peering(|text| {
            text.replace(
                "endpoint = \"wss://store.example:443/btp\"\n",
                "endpoint = \"wss://store.example:443/btp\"\nceiling = 1000000\n",
            )
        });

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::PeerCeilingRemoved { id } if id == "store"),
        );
        assert!(
            message.contains("ADR 0033") && message.contains(BRINGUP_DOC),
            "got: {message}"
        );
    }

    /// §11's removed-field row, `flush_interval_ms` half.
    #[test]
    fn rejects_a_peering_that_still_sets_flush_interval_ms() {
        let result = load_peering(|text| {
            text.replace(
                "endpoint = \"wss://store.example:443/btp\"\n",
                "endpoint = \"wss://store.example:443/btp\"\nflush_interval_ms = 5000\n",
            )
        });

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::PeerFlushIntervalRemoved { id } if id == "store"),
        );
        assert!(
            message.contains("ADR 0033") && message.contains(BRINGUP_DOC),
            "got: {message}"
        );
    }

    /// §11 `PeerRouteUndeliverable` (§2.2, §6.4(1)): accept-only, and this
    /// connector exposes only HTTP, so packets can only ever flow the
    /// other way -- the route could answer nothing but `T01`.
    #[test]
    fn rejects_a_route_to_a_peer_this_connector_can_never_originate_to() {
        let result = load_peering(|text| {
            text.replace("peer_expose = \"btp\"", "peer_expose = \"http\"")
                .replace("endpoint = \"wss://store.example:443/btp\"\n", "")
        });

        let message = expect_error(result, |error| {
            matches!(error, ConfigError::PeerRouteUndeliverable { prefix, peer_id }
                if prefix == "g.example.store" && peer_id == "store")
        });
        assert!(
            message.contains("can never originate to") && message.contains(BRINGUP_DOC),
            "got: {message}"
        );
    }

    /// §11 `DuplicatePeerId`.
    #[test]
    fn rejects_a_duplicate_peer_id() {
        let result = load_peering(|text| {
            text.replace(
                "[[peer_channels]]",
                "[[peers]]\nid = \"store\"\nendpoint = \"wss://other.example/btp\"\n\n[[peer_channels]]",
            )
        });

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::DuplicatePeerId { id } if id == "store"),
        );
        assert!(
            message.contains("unanswerable") && message.contains(BRINGUP_DOC),
            "got: {message}"
        );
    }

    /// §11's removed-field row, `[[peers]]` half: the boxes run
    /// bind-mounted configs that lead the repo, so a stale `addr` stops
    /// the node and says where to read about it.
    #[test]
    fn rejects_a_stale_peer_entry_that_still_sets_addr() {
        let result = load_peering(|text| {
            text.replace(
                "[[peers]]\nid = \"store\"\n",
                "[[peers]]\nid = \"store\"\naddr = \"127.0.0.1:5000\"\n",
            )
        });

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::PeerAddrRemoved { id } if id == "store"),
        );
        assert!(
            message.contains("removed with the raw-TCP transport")
                && message.contains("endpoint")
                && message.contains(BRINGUP_DOC),
            "got: {message}"
        );
    }

    /// §11's removed-field row, `peer_wire_addr` half. The *field* is
    /// still live on this branch -- deleting the listener it binds is PR
    /// #718 / issue #679's work, not this one's -- so what is asserted
    /// here is the error identity and its message, which #718 constructs.
    #[test]
    fn the_removed_peer_wire_addr_error_names_the_bringup_doc() {
        let message = ConfigError::PeerWireAddrRemoved.to_string();

        assert!(
            message.contains("peer_wire_addr")
                && message.contains("removed with the raw-TCP transport")
                && message.contains(BRINGUP_DOC),
            "got: {message}"
        );
    }

    /// A peer claim's watermark is no less a replay defence than a client
    /// claim's (issue #605).
    #[test]
    fn rejects_peer_channels_with_no_state_dir() {
        let result = load_peering(|text| {
            let start = text.find("state_dir = ").expect("fixture");
            let end = text[start..].find('\n').expect("fixture") + start + 1;
            format!("{}{}", &text[..start], &text[end..])
        });

        let message = expect_error(result, |error| {
            matches!(error, ConfigError::PeerChannelsWithoutStateDir)
        });
        assert!(message.contains("spendable again"), "got: {message}");
    }

    #[test]
    fn rejects_an_unrecognized_peer_expose_value() {
        let result =
            load_peering(|text| text.replace("peer_expose = \"btp\"", "peer_expose = \"tcp\""));

        let message = expect_error(
            result,
            |error| matches!(error, ConfigError::InvalidPeerExposure { value } if value == "tcp"),
        );
        assert!(message.contains("neither"), "got: {message}");
    }

    /// Assert `result` failed with the error `predicate` accepts, and hand
    /// back its rendered message -- so every named-error test can go on to
    /// assert what the operator actually reads, not merely that load
    /// failed.
    fn expect_error(
        result: Result<Config, ConfigError>,
        predicate: impl Fn(&ConfigError) -> bool,
    ) -> String {
        let error = result.expect_err("expected this config to be refused at load");
        assert!(predicate(&error), "wrong error variant: {error:?}");
        error.to_string()
    }

    #[test]
    fn a_config_with_no_peers_has_an_empty_list() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        })
        .expect("load");

        assert!(config.peers().is_empty());
        assert!(config.peer_routes().is_empty());
    }

    #[test]
    fn rejects_a_peer_route_naming_an_unconfigured_peer_id() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.peer-b"
peer_id = "peer-b"
price = 1000
"#,
                key_path.display()
            )
        });

        assert!(matches!(result, Err(ConfigError::UnknownPeerId { .. })));
    }

    /// ADR 0043: purchasable peering is removed, so a config still naming
    /// `[peer_sale]` must stop the node by name rather than be silently
    /// dropped -- the same treatment `peer_wire_addr` below already gets,
    /// for the same reason (the devnet boxes run bind-mounted configs that
    /// lead the repo copies).
    #[test]
    fn rejects_a_config_that_still_sets_peer_sale() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[peer_sale]
prefix = "g.example.node.peer-sale"
price = 5000
lease_seconds = 3600
"#,
                key_path.display()
            )
        });

        let Err(error) = result else {
            panic!("expected a config error");
        };
        assert!(matches!(error, ConfigError::PeerSaleRemoved));
        let message = error.to_string();
        assert!(
            message.contains("peer_sale"),
            "the error must name the section an operator has to delete: {message}"
        );
    }

    /// The abuse-bound half of the same section (ADR 0039's own fields) is
    /// refused by the very same trap: the whole table is gone, not just
    /// its price.
    #[test]
    fn rejects_a_config_that_still_sets_peer_sale_abuse_bounds() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[peer_sale]
prefix = "g.example.node.peer-sale"
price = 5000
lease_seconds = 3600
max_purchased_rows = 8
max_routes_per_payer = 2
max_prefix_length = 64
purchase_rate_limit = 3
purchase_rate_window_seconds = 30
"#,
                key_path.display()
            )
        });

        assert!(matches!(result, Err(ConfigError::PeerSaleRemoved)));
    }

    /// ADR 0027 / issue #679: the raw-TCP transport is deleted, so a
    /// config still naming its bind address must stop the node by name.
    /// Silently ignoring it is the failure mode that matters -- the devnet
    /// boxes run bind-mounted configs that lead the repo copies, so a
    /// stale one would otherwise come up looking healthy and never peer.
    #[test]
    fn rejects_a_config_that_still_sets_peer_wire_addr() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
peer_wire_addr = "127.0.0.1:4001"

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        });

        let Err(error) = result else {
            panic!("expected a config error");
        };
        assert!(matches!(error, ConfigError::PeerWireAddrRemoved));
        assert!(error
            .to_string()
            .contains("docs/operators/btp-peer-transport-bringup.md"));
    }

    /// The `[[peers]]` half of the same removal.
    #[test]
    fn rejects_a_peer_that_still_sets_a_socket_addr() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[[peers]]
id = "peer-b"
addr = "127.0.0.1:5000"
"#,
                key_path.display()
            )
        });

        assert!(matches!(result, Err(ConfigError::PeerAddrRemoved { .. })));
    }

    #[test]
    fn loads_a_kms_signer_location() {
        let config = Config::from_toml_str(
            r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
kms_key_id = "arn:aws:kms:us-east-1:123:key/abc"
"#,
            Path::new("test.toml"),
        )
        .expect("load");

        assert_eq!(
            config.signer_key(),
            &SecretLocation::Kms {
                key_id: "arn:aws:kms:us-east-1:123:key/abc".to_string()
            }
        );
    }

    #[test]
    fn rejects_malformed_toml() {
        let result = Config::from_toml_str("this is not { valid toml", Path::new("test.toml"));
        assert!(matches!(result, Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn rejects_an_invalid_bind_address() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "not-an-address"

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        });
        assert!(matches!(result, Err(ConfigError::InvalidBindAddr { .. })));
    }

    #[test]
    fn rejects_a_missing_signer_key_file() {
        let result = Config::from_toml_str(
            r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "/nonexistent/does-not-exist.key"
"#,
            Path::new("test.toml"),
        );
        assert!(matches!(result, Err(ConfigError::SignerKeyFileNotFound(_))));
    }

    #[test]
    fn load_reports_the_path_on_a_missing_file() {
        let result = Config::load(&PathBuf::from("/nonexistent/connector.toml"));
        assert!(matches!(result, Err(ConfigError::Io { .. })));
    }

    #[test]
    fn a_config_with_no_operator_section_has_no_operator_config() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        })
        .expect("load");

        assert_eq!(config.operator(), None);
    }

    #[test]
    fn a_fully_configured_operator_section_loads() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[operator]
bearer_token = "secret-token"
write_keys = ["{key}"]
"#,
                key_path.display()
            )
        })
        .expect("load");

        let operator = config.operator().expect("operator config");
        assert_eq!(operator.bearer_token(), "secret-token");
        assert_eq!(operator.write_keys().len(), 1);
    }

    /// Issue #1003, end to end through `Config::load`: the shape the store
    /// box's committed `connector-rust.toml` uses, so what CI proves is the
    /// spelling a fleet config is allowed to carry -- both settings as
    /// paths, no credential anywhere in the file.
    #[test]
    fn an_operator_section_written_as_file_references_loads() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        let mut token_file = tempfile::NamedTempFile::new().expect("temp token file");
        std::io::Write::write_all(&mut token_file, b"token-from-a-file\n").expect("write token");
        let mut keys_file = tempfile::NamedTempFile::new().expect("temp keys file");
        std::io::Write::write_all(&mut keys_file, format!("# alice\n{key}\n").as_bytes())
            .expect("write keys");

        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{signer}"

[operator]
bearer_token_file = "{token}"
write_keys_file = "{keys}"
"#,
                signer = key_path.display(),
                token = token_file.path().display(),
                keys = keys_file.path().display(),
            )
        })
        .expect("load");

        let operator = config.operator().expect("operator config");
        assert_eq!(operator.bearer_token(), "token-from-a-file");
        assert_eq!(operator.write_keys().len(), 1);

        // The whole point: a `Config` gets logged whole at startup, and the
        // token that gates every operator read must not ride along.
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("token-from-a-file"), "{rendered}");
    }

    /// A file the config names and the box does not have is a
    /// refuse-to-start, not a surface that comes up and rejects every
    /// request -- the same contract `[signer] key_file` has (ADR 0009).
    #[test]
    fn refuses_to_start_when_an_operator_file_is_missing() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[operator]
bearer_token_file = "/nonexistent/operator-bearer-token"
write_keys = ["{key}"]
"#,
                key_path.display()
            )
        });

        let message = result.expect_err("missing operator file").to_string();
        assert!(message.contains("bearer_token_file"), "{message}");
        assert!(
            message.contains("/nonexistent/operator-bearer-token"),
            "{message}"
        );
    }

    #[test]
    fn refuses_to_start_when_the_operator_section_names_a_token_twice() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[operator]
bearer_token = "secret-token"
bearer_token_file = "/app/data/operator-bearer-token"
write_keys = ["{key}"]
"#,
                key_path.display()
            )
        });

        assert!(matches!(
            result,
            Err(ConfigError::OperatorSettingAmbiguous {
                literal: "bearer_token",
                file: "bearer_token_file",
            })
        ));
    }

    #[test]
    fn refuses_to_start_when_the_operator_surface_is_enabled_without_write_keys() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[operator]
bearer_token = "secret-token"
"#,
                key_path.display()
            )
        });

        assert!(matches!(result, Err(ConfigError::OperatorNoWriteKeys)));
    }

    #[test]
    fn refuses_to_start_when_the_operator_surface_is_enabled_without_a_bearer_token() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[operator]
write_keys = ["{key}"]
"#,
                key_path.display()
            )
        });

        assert!(matches!(
            result,
            Err(ConfigError::OperatorMissingBearerToken)
        ));
    }

    #[test]
    fn a_config_with_no_settlement_section_has_no_settlement_config() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        })
        .expect("load");

        assert!(config.settlements().is_empty());
    }

    #[test]
    fn a_fully_configured_settlement_section_loads() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
state_dir = "/tmp"

[signer]
key_file = "{}"

[settlement]
chain = "evm"
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6

[settlement.key]
key_file = "{}"
"#,
                key_path.display(),
                key_path.display()
            )
        })
        .expect("load");

        assert_eq!(config.settlements().len(), 1);
        let settlement = &config.settlements()[0];
        assert_eq!(settlement.chain(), crate::SettlementChain::Evm);
        let crate::SettlementConfig::Evm(evm) = settlement else {
            panic!("expected an evm settlement config");
        };
        assert_eq!(evm.rpc_url(), "http://127.0.0.1:8545");
        assert_eq!(evm.decimals(), 6);
    }

    /// The new keyed shape (issue #628): `[settlement.evm]` alone resolves
    /// the same facts the legacy flat shape does.
    #[test]
    fn a_keyed_evm_settlement_table_loads() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
state_dir = "/tmp"

[signer]
key_file = "{}"

[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6

[settlement.evm.key]
key_file = "{}"
"#,
                key_path.display(),
                key_path.display()
            )
        })
        .expect("load");

        assert_eq!(config.settlements().len(), 1);
        assert_eq!(config.settlements()[0].chain(), crate::SettlementChain::Evm);
    }

    /// AC: "A config declaring both [settlement.evm] and [settlement.solana]
    /// parses into typed per-chain settlement config".
    #[test]
    fn declaring_both_evm_and_solana_settlement_tables_loads_both() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
state_dir = "/tmp"

[signer]
key_file = "{key_path}"

[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x49beE1Bca5d15Fb0963117893403F9498119a9Ce"
decimals = 6

[settlement.evm.key]
key_file = "{key_path}"

[settlement.solana]
rpc_url = "http://127.0.0.1:8899"
program_id = "TokenNetworkProgram11111111111111111111111"
token_address = "SoLMint11111111111111111111111111111111111"
decimals = 6

[settlement.solana.key]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        })
        .expect("load");

        assert_eq!(config.settlements().len(), 2);
        assert!(config
            .settlements()
            .iter()
            .any(|s| s.chain() == crate::SettlementChain::Evm));
        assert!(config
            .settlements()
            .iter()
            .any(|s| s.chain() == crate::SettlementChain::Solana));
    }

    /// AC: "[settlement.solana] alone: config loads" -- construction refusal
    /// is `connector-cli`'s to enforce (epic #627's fail-closed-per-chain),
    /// not config load's.
    #[test]
    fn a_solana_only_settlement_section_still_loads() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
state_dir = "/tmp"

[signer]
key_file = "{key_path}"

[settlement.solana]
rpc_url = "http://127.0.0.1:8899"
program_id = "TokenNetworkProgram11111111111111111111111"
token_address = "SoLMint11111111111111111111111111111111111"
decimals = 6

[settlement.solana.key]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        })
        .expect("load");

        assert_eq!(config.settlements().len(), 1);
        assert_eq!(
            config.settlements()[0].chain(),
            crate::SettlementChain::Solana
        );
    }

    #[test]
    fn a_settlement_section_that_cannot_be_satisfied_refuses_to_load() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[settlement]
chain = "made-up-chain"
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6

[settlement.key]
key_file = "{}"
"#,
                key_path.display(),
                key_path.display()
            )
        });

        assert!(matches!(
            result,
            Err(ConfigError::SettlementUnknownChain { .. })
        ));
    }

    #[test]
    fn an_unknown_top_level_key_is_rejected_rather_than_silently_ignored() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
made_up_top_level_field = "oops"

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        });

        assert!(matches!(result, Err(ConfigError::Parse { .. })));
    }

    /// Issue #556's parse-layer spine: `deny_unknown_fields` on
    /// `RawConfig` alone only guards the top level. A typo *inside* a
    /// section was still parsed, dropped, and the node started as if the
    /// key had never been written -- so a misspelled `bearer_tokn` read as
    /// an unauthenticated operator surface and a misspelled `key_fle` read
    /// as a signer with no location at all. Each of these now fails at the
    /// parse stage, and the message names the offending key.
    fn assert_names_the_unknown_key(result: Result<Config, ConfigError>, key: &str) {
        let Err(ConfigError::Parse { source, .. }) = result else {
            panic!("expected a parse error naming {key}, got {result:?}");
        };
        let message = source.to_string();
        assert!(
            message.contains(key),
            "parse error should name the offending key {key}, got: {message}"
        );
    }

    #[test]
    fn an_unknown_key_in_the_signer_section_is_rejected() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"
kms_key_di = "transposed"
"#,
                key_path.display()
            )
        });

        assert_names_the_unknown_key(result, "kms_key_di");
    }

    #[test]
    fn an_unknown_key_in_the_operator_section_is_rejected() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[operator]
bearer_token = "operator-secret"
write_keys = ["{key}"]
bearer_tokn = "typo"
"#,
                key_path.display()
            )
        });

        assert_names_the_unknown_key(result, "bearer_tokn");
    }

    #[test]
    fn an_unknown_key_in_a_peer_entry_is_rejected() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[[peers]]
id = "store"
adrr = "127.0.0.1:4002"
"#,
                key_path.display()
            )
        });

        assert_names_the_unknown_key(result, "adrr");
    }

    #[test]
    fn an_unknown_key_in_a_route_entry_is_rejected() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.example.app"
handler_url = "http://localhost:4000"
price = 100
pirce = 5
"#,
                key_path.display()
            )
        });

        assert_names_the_unknown_key(result, "pirce");
    }

    /// A `[[children]]` entry has no `fee` field at all -- and since ADR
    /// 0061 neither does a `[[routes]]` entry, which refuses the key by
    /// name of its own. Here the generic unknown-key refusal is what
    /// catches it.
    #[test]
    fn an_unknown_key_in_a_child_entry_is_rejected() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
apex = "g.example"

[signer]
key_file = "{}"

[[children]]
name = "app"
handler_url = "http://localhost:4000"
price = 100
fee = 5
"#,
                key_path.display()
            )
        });

        assert_names_the_unknown_key(result, "fee");
    }

    /// The counterweight: a config file using every section this build
    /// supports, with no unknown key anywhere, still loads. Without this
    /// the tests above are satisfied by a config crate that refuses
    /// everything.
    #[test]
    fn a_config_using_every_supported_section_still_loads() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
peer_expose = "both"
apex = "g.example"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[[peers]]
id = "store"
endpoint = "wss://store.example:443/btp"
fee = 3

[[peer_channels]]
peer_id = "store"
channel_id = "{PEER_CHANNEL}"
counterparty_key = "{PEER_KEY}"
chain_id = 31337
token_network = "{PEER_TOKEN_NETWORK}"

[[routes]]
prefix = "g.example.app"
handler_url = "http://localhost:4000"
price = 100

[[routes]]
prefix = "g.example.store"
peer_id = "store"
price = 1000

# Required of a peering this node forwards to since issue #1145: a
# connector covers every PREPARE it sends (ADR 0042).
[[pay_channels]]
peer_id = "store"
channel_id = "{PEER_CHANNEL}"
chain_id = 31337
token_network = "{PEER_TOKEN_NETWORK}"
client_edge_url = "https://store.example/ilp"

[[children]]
name = "child"
handler_url = "http://localhost:4100"
price = 7

[operator]
bearer_token = "operator-secret"
write_keys = ["{key}"]
{settlement}"#,
                key_file = key_path.display(),
                state_dir = std::env::temp_dir()
                    .join("connector-config-every-section-state")
                    .display(),
                settlement = evm_settlement(key_path),
            )
        })
        .expect("load");

        assert_eq!(config.routes().len(), 2);
        assert_eq!(config.peer_routes().len(), 1);
        assert_eq!(config.peers().len(), 1);
        assert_eq!(config.peer_channels().len(), 1);
        assert_eq!(config.peer_expose(), PeerExposure::Both);
        assert!(config.operator().is_some());
    }

    // -- state_dir (issue #605) --

    /// A node that can accept claims but has nowhere durable to record
    /// them is refused at load. Without this it starts, serves, and hands
    /// out free service after every restart -- silently, because a
    /// forgotten watermark makes every replayed nonce look fresh.
    #[test]
    fn client_channels_without_a_state_dir_is_refused_at_load() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{key_path}"
{settlement}
[[client_channels]]
channel_id = "0x{channel}"
counterparty = "0x00000000000000000000000000000000000000aa"
chain_id = 8453
token_network_address = "0x00000000000000000000000000000000000000bb"
"#,
                key_path = key_path.display(),
                settlement = evm_settlement(key_path),
                channel = "ab".repeat(32),
            )
        });

        assert!(matches!(
            result,
            Err(ConfigError::ClientChannelsWithoutStateDir)
        ));
        // The message has to tell the operator what to do about it, since
        // this refusal is the first they will hear of the requirement.
        let message = result.unwrap_err().to_string();
        assert!(message.contains("state_dir"), "{message}");
    }

    /// Issue #1186: the shape this check used to miss, and the one an
    /// operator should actually be running -- a priced terminated route and
    /// a settlement backend, declaring no channel at all.
    ///
    /// A settlement table registers the `ClientChannelSource` that resolves
    /// an undeclared channel from chain (ADR 0052, CF-27), so this node takes
    /// payment from senders it was never configured for. Before #1186 it was
    /// the one shape that could boot with its watermarks in memory, which
    /// made the node most exposed to strangers the one the parser did not
    /// protect.
    #[test]
    fn refuses_a_settlement_backend_without_a_state_dir() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{key_path}"

[[routes]]
prefix = "g.example.app"
handler_url = "http://app:3100/"
price = 1000

{settlement}
"#,
                key_path = key_path.display(),
                settlement = evm_settlement(key_path),
            )
        });

        assert!(
            matches!(result, Err(ConfigError::SettlementWithoutStateDir)),
            "a node that resolves channels from chain must be made to keep its \
             watermarks somewhere durable"
        );
        let message = result.unwrap_err().to_string();
        assert!(message.contains("state_dir"), "{message}");
        // The operator has to be told WHY, or the fix reads as ceremony and
        // gets a path nobody checked.
        assert!(message.contains("chain"), "{message}");
    }

    /// The exemption #558 bought, and the half of its reasoning that survives
    /// #1186: with neither a channel book nor a settlement table, the claim
    /// registry has neither a record nor a source, so it refuses every claim
    /// and genuinely has no watermark to lose. Demanding a path of it would
    /// be ceremony.
    #[test]
    fn a_node_that_can_resolve_no_channel_needs_no_state_dir() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{key_path}"

[[routes]]
prefix = "g.example.app"
handler_url = "http://app:3100/"
price = 0
"#,
                key_path = key_path.display(),
            )
        })
        .expect("a node with no settlement and no channel book loads without a state_dir");

        assert!(config.state_dir().is_none());
    }

    /// The same config with a `state_dir` loads, and reports it.
    #[test]
    fn client_channels_with_a_state_dir_loads() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"
{settlement}
[[client_channels]]
channel_id = "0x{channel}"
counterparty = "0x00000000000000000000000000000000000000aa"
chain_id = 8453
token_network_address = "0x00000000000000000000000000000000000000bb"
"#,
                key_path = key_path.display(),
                state_dir = state_dir.path().display(),
                settlement = evm_settlement(key_path),
                channel = "ab".repeat(32),
            )
        })
        .expect("load");

        assert_eq!(config.state_dir(), Some(state_dir.path()));
    }

    /// A node with no channels needs no `state_dir`: it refuses every
    /// claim as unknown (issue #558), so it has no watermark to lose. The
    /// requirement follows the capability, not the ceremony.
    #[test]
    fn no_client_channels_needs_no_state_dir() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        })
        .expect("load");

        assert_eq!(config.state_dir(), None);
    }

    // -- client_identities (issue #502) --

    /// `[[client_identities]]` needs no `state_dir` -- it is an HTTP-layer
    /// credential, not a payment channel, and carries no watermark to lose.
    #[test]
    fn client_identities_load_and_need_no_state_dir() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[[client_identities]]
id = "peer-a"
secret = "s3cr3t"

[[client_identities]]
id = "peer-b"
"#,
                key_path.display()
            )
        })
        .expect("load");

        assert_eq!(config.state_dir(), None);
        let identities = config.client_identities();
        assert_eq!(identities.len(), 2);
        assert_eq!(identities[0].id(), "peer-a");
        assert_eq!(identities[0].secret(), "s3cr3t");
        assert_eq!(identities[1].id(), "peer-b");
        assert_eq!(identities[1].secret(), "");
    }

    /// AC: "a duplicate identity is refused at load."
    #[test]
    fn a_duplicate_client_identity_id_is_refused_at_load() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{}"

[[client_identities]]
id = "peer-a"
secret = "one"

[[client_identities]]
id = "peer-a"
secret = "two"
"#,
                key_path.display()
            )
        });

        assert!(matches!(
            result,
            Err(ConfigError::DuplicateClientIdentityId { id }) if id == "peer-a"
        ));
    }

    /// Issue #649: how long a chain-resolved channel's liveness may be
    /// believed is an operator knob, because the trade it makes -- RPC
    /// load against how quickly a settled channel stops being paid on --
    /// belongs to a deployment and not to a constant in this repository.
    #[test]
    fn a_channel_liveness_ttl_is_read_from_config() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
channel_liveness_ttl_secs = 15

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        })
        .expect("a config naming a liveness ttl loads");

        assert_eq!(
            config.channel_liveness_ttl(),
            Some(std::time::Duration::from_secs(15))
        );
    }

    /// Absent means "whatever the client edge's own default is" -- not
    /// zero, which is the one value that would turn every packet into a
    /// chain read.
    #[test]
    fn an_absent_channel_liveness_ttl_is_the_edges_own_default() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        })
        .expect("a config naming no liveness ttl loads");

        assert_eq!(config.channel_liveness_ttl(), None);
    }

    /// Zero is refused rather than obeyed: it reads as "always fresh" and
    /// behaves as "one chain read per packet", which is how an operator
    /// exhausts an RPC endpoint's budget and takes their own paid writes
    /// down with it.
    #[test]
    fn a_zero_channel_liveness_ttl_is_refused_at_load() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
channel_liveness_ttl_secs = 0

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        });

        assert!(matches!(result, Err(ConfigError::ZeroChannelLivenessTtl)));
    }

    /// The other two liveness knobs (the availability review of #654): an
    /// operator on a rate-limited public RPC endpoint is exactly who needs
    /// to widen the stale window and the re-attempt floor, and before this
    /// they had no lever short of a rebuild.
    #[test]
    fn the_stale_window_and_reattempt_interval_are_read_from_config() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
channel_liveness_ttl_secs = 30
channel_serve_stale_secs = 1800
channel_reattempt_interval_ms = 5000

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        })
        .expect("a config naming all three liveness knobs loads");

        assert_eq!(
            config.channel_liveness_ttl(),
            Some(std::time::Duration::from_secs(30))
        );
        assert_eq!(
            config.channel_serve_stale(),
            Some(std::time::Duration::from_secs(1800))
        );
        assert_eq!(
            config.channel_reattempt_interval(),
            Some(std::time::Duration::from_millis(5000))
        );
    }

    /// The BTP session window (issue #688): how many of one session's
    /// frames may be past claim admission at once. Read as written when
    /// non-zero, `None` when absent (the client edge's default applies).
    #[test]
    fn the_btp_session_window_is_read_from_config() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
btp_session_window = 4

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        })
        .expect("a config naming the window loads");

        assert_eq!(
            config.btp_session_window(),
            std::num::NonZeroU32::new(4),
            "the configured window is in force"
        );
    }

    /// An absent window is `None` -- the client edge's own default applies
    /// -- never a guessed number of this crate's own.
    #[test]
    fn an_absent_btp_session_window_defers_to_the_client_edge() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        })
        .expect("a config not naming the window loads");
        assert_eq!(config.btp_session_window(), None);
    }

    /// A zero window is refused at load (issue #688): it is not a slower
    /// session, it is a session whose first paid frame waits forever for
    /// an in-flight slot that does not exist -- every BTP client hangs on
    /// connect while the file reads as configured.
    #[test]
    fn a_zero_btp_session_window_is_refused_at_load() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
btp_session_window = 0

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        });

        assert!(matches!(result, Err(ConfigError::ZeroBtpSessionWindow)));
    }

    /// The unresolvable-lookup budget (issue #613): what a node will spend
    /// discovering channels that turn out not to exist, per declared signer
    /// and in total. An operator on a metered settlement endpoint is
    /// exactly who needs to set the second one.
    #[test]
    fn the_unresolvable_lookup_budget_is_read_from_config() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
unresolvable_lookup_budget_per_signer = 3
unresolvable_lookup_budget_total = 20
unresolvable_lookup_budget_window_secs = 30
unresolvable_lookup_budget_max_wait_ms = 750

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        })
        .expect("a config naming all four budget knobs loads");

        assert_eq!(config.unresolvable_lookups_per_signer(), Some(3));
        assert_eq!(config.unresolvable_lookups_total(), Some(20));
        assert_eq!(
            config.unresolvable_lookup_window(),
            Some(std::time::Duration::from_secs(30))
        );
        assert_eq!(
            config.unresolvable_lookup_max_wait(),
            Some(std::time::Duration::from_millis(750))
        );
    }

    /// Absent means the client edge's own default, the same as every other
    /// knob here -- a node that has not thought about this should get a
    /// bound rather than none.
    #[test]
    fn an_absent_unresolvable_lookup_budget_is_the_edges_own_default() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        })
        .expect("a config naming no budget loads");

        assert_eq!(config.unresolvable_lookups_per_signer(), None);
        assert_eq!(config.unresolvable_lookups_total(), None);
        assert_eq!(config.unresolvable_lookup_window(), None);
        assert_eq!(config.unresolvable_lookup_max_wait(), None);
    }

    /// Zero allowances are refused, and the reason is the mirror image of
    /// the zero ttl above: that one reads as strictness and melts an
    /// endpoint, this one reads as strictness and silently switches off the
    /// registration-free path #611 exists to provide.
    #[test]
    fn a_zero_unresolvable_lookup_allowance_is_refused_at_load() {
        for field in ["per_signer", "total"] {
            let result = with_key_file(|key_path| {
                format!(
                    r#"
client_edge_addr = "127.0.0.1:3000"
unresolvable_lookup_budget_{field} = 0

[signer]
key_file = "{key_path}"
"#,
                    key_path = key_path.display(),
                )
            });

            assert!(
                matches!(
                    result,
                    Err(ConfigError::ZeroUnresolvableLookupBudget { .. })
                ),
                "unresolvable_lookup_budget_{field} = 0 must not load"
            );
        }
    }

    /// A zero-length window is the sharpest of the three footguns: it
    /// restarts on every request, so both allowances are spendable in full
    /// by every request and the budget bounds nothing while looking like it
    /// is configured.
    #[test]
    fn a_zero_unresolvable_lookup_window_is_refused_at_load() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
unresolvable_lookup_budget_window_secs = 0

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        });

        assert!(matches!(
            result,
            Err(ConfigError::ZeroUnresolvableLookupWindow)
        ));
    }

    /// A zero wait ceiling is refused, and it is the one whose reason is
    /// least obvious from the field name: it does not tighten the bound, it
    /// converts it from a shaper into a dropper, and a dropping bound hands
    /// a flooder a switch that turns the registration-free path off for
    /// every new buyer.
    #[test]
    fn a_zero_unresolvable_lookup_wait_ceiling_is_refused_at_load() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
unresolvable_lookup_budget_max_wait_ms = 0

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        });

        assert!(matches!(
            result,
            Err(ConfigError::ZeroUnresolvableLookupMaxWait)
        ));
    }

    /// The wait ceiling is the size of the waiting room, not a timeout, so
    /// it needs a coherence rule and not only a zero check: a ceiling
    /// longer than the window parks more than a whole window's worth of
    /// drain. Issue #613's review, finding C -- it was the one budget knob
    /// with nothing but a zero check, and nothing else in the file would
    /// have told an operator they had written a room thousands deep.
    #[test]
    fn a_wait_ceiling_longer_than_the_window_is_refused_at_load() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
unresolvable_lookup_budget_window_secs = 60
unresolvable_lookup_budget_max_wait_ms = 600000

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        });

        assert!(matches!(
            result,
            Err(ConfigError::UnresolvableLookupMaxWaitAboveWindow {
                max_wait_ms: 600_000,
                window_secs: 60
            })
        ));
    }

    /// ...and against the *defaults* when only one side is written, for the
    /// same reason the rates are: a ceiling above the default window is the
    /// same incoherence spelled one-sidedly.
    #[test]
    fn a_one_sided_wait_ceiling_is_validated_against_the_default_window() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
unresolvable_lookup_budget_max_wait_ms = 90000

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        });

        assert!(matches!(
            result,
            Err(ConfigError::UnresolvableLookupMaxWaitAboveWindow { .. })
        ));
    }

    /// A window longer than the client edge will honour is refused rather
    /// than silently clamped: a rate limit whose window outlives the
    /// process is not a rate limit, and past a point the arithmetic over it
    /// stops fitting an instant. (The client edge clamps as well, since its
    /// policy struct is public and reachable without this check -- but a
    /// value that reached here was *written down*, and silently obeying
    /// something other than what an operator wrote is what this whole file
    /// exists not to do.)
    #[test]
    fn an_absurdly_long_unresolvable_lookup_window_is_refused_at_load() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
unresolvable_lookup_budget_window_secs = 9000000000000

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        });

        assert!(
            matches!(
                result,
                Err(ConfigError::UnresolvableLookupWindowTooLong {
                    max_secs: 86_400,
                    ..
                })
            ),
            "{result:?}"
        );
    }

    /// A per-signer rate above the node-wide one is inert rather than
    /// dangerous -- the drain saturates first every time -- and is refused
    /// for the same reason a stale window shorter than the ttl is: an
    /// operator who wrote a number meant something by it.
    #[test]
    fn a_per_signer_allowance_above_the_node_wide_one_is_refused_at_load() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
unresolvable_lookup_budget_per_signer = 1000
unresolvable_lookup_budget_total = 10

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        });

        assert!(matches!(
            result,
            Err(ConfigError::UnresolvableLookupPerSignerAboveTotal {
                per_signer: 1000,
                total: 10
            })
        ));
    }

    /// ...and the same rule fires when only *one* of the two is written,
    /// because it is compared against the values that will actually be in
    /// force. A rule that needed both present would let the two one-sided
    /// spellings of the same incoherent configuration load quietly, which
    /// is the whole hazard.
    #[test]
    fn a_one_sided_unresolvable_lookup_budget_is_validated_against_the_defaults() {
        // A node-wide rate below the *default* per-signer rate.
        let total_only = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
unresolvable_lookup_budget_total = 5

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        });
        assert!(
            matches!(
                total_only,
                Err(ConfigError::UnresolvableLookupPerSignerAboveTotal { total: 5, .. })
            ),
            "a total below the default per-signer rate is the same incoherence"
        );

        // ...and a per-signer rate above the *default* node-wide one.
        let per_signer_only = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
unresolvable_lookup_budget_per_signer = 10000

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        });
        assert!(matches!(
            per_signer_only,
            Err(ConfigError::UnresolvableLookupPerSignerAboveTotal {
                per_signer: 10000,
                ..
            })
        ));
    }

    /// Zero is refused for the re-attempt floor for the same reason it is
    /// refused for the ttl: it is the value that turns one packet into one
    /// RPC request.
    #[test]
    fn a_zero_reattempt_interval_is_refused_at_load() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
channel_reattempt_interval_ms = 0

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        });

        assert!(matches!(
            result,
            Err(ConfigError::ZeroChannelReattemptInterval)
        ));
    }

    /// ...but zero *is* allowed for the stale window, and the asymmetry is
    /// deliberate: "never serve a reading I could not confirm" is a
    /// defensible fail-closed choice that costs no extra chain read, which
    /// is precisely what a zero ttl or a zero interval would not be.
    #[test]
    fn a_zero_stale_window_is_allowed_because_it_costs_nothing() {
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
channel_serve_stale_secs = 0

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        })
        .expect("never serving a stale reading is a choice an operator may make");

        assert_eq!(
            config.channel_serve_stale(),
            Some(std::time::Duration::ZERO)
        );
    }

    /// A stale window shorter than the ttl names a window that could never
    /// be used -- an entry would stop being believed and stop being
    /// servable at the same moment. Refused rather than silently treated
    /// as zero, since whoever wrote it meant something else.
    #[test]
    fn a_stale_window_shorter_than_the_ttl_is_refused_at_load() {
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
channel_liveness_ttl_secs = 60
channel_serve_stale_secs = 30

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            )
        });

        assert!(matches!(
            result,
            Err(ConfigError::ServeStaleShorterThanLivenessTtl {
                serve_stale_secs: 30,
                ttl_secs: 60,
            })
        ));
    }

    /// `state_dir` pointing at something that is not a directory is a
    /// load failure, not a surprise at the first journal write.
    #[test]
    fn a_state_dir_that_is_a_file_is_refused_at_load() {
        let not_a_dir = tempfile::NamedTempFile::new().expect("temp file");
        let result = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
                state_dir = not_a_dir.path().display(),
            )
        });

        assert!(matches!(
            result,
            Err(ConfigError::StateDirNotADirectory { .. })
        ));
    }

    /// A `state_dir` that does not exist yet is not a load failure:
    /// creating it is startup's job (`connector-cli`), and an operator
    /// mounting a fresh empty volume must not have to pre-create it.
    #[test]
    fn a_state_dir_that_does_not_exist_yet_still_loads() {
        let parent = tempfile::tempdir().expect("temp dir");
        let state_dir = parent.path().join("not-created-yet");
        let config = with_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:3000"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
                state_dir = state_dir.display(),
            )
        })
        .expect("load");

        assert_eq!(config.state_dir(), Some(state_dir.as_path()));
    }
}
