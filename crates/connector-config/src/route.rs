use std::collections::{HashMap, HashSet};

use connector_domain::{PayloadLimit, Price};
use serde::Deserialize;
use url::Url;

use crate::error::ConfigError;

/// The largest `max_bytes` a route may declare (ADR 0081): the client
/// edge's HTTP body limit, axum's own `DefaultBodyLimit` of 2 MiB, which
/// `POST /ilp` does not override. A limit above it could never be reached on
/// that carriage, and ADR 0081 lets a route lower the carriage's limit, never
/// raise it -- raising it is issue #1529's, a separate decision.
pub const MAX_PAYLOAD_LIMIT: u64 = 2 * 1024 * 1024;

/// Maximum length of an ILP address, per RFC 0015.
const MAX_ILP_ADDRESS_LEN: usize = 1023;

/// A single label between dots is alphanumeric plus `-`/`_`, matching RFC
/// 0015 and the existing TypeScript connector's `isValidILPAddress`.
fn is_valid_label(label: &str) -> bool {
    !label.is_empty()
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// An ILP address is one or more valid labels joined by dots -- no leading,
/// trailing, or consecutive dots, and no characters outside the label set.
///
/// `pub(crate)` so the `[announce]` section (issue #784) validates the
/// addresses it broadcasts by the same rule a `[[routes]]` prefix is
/// validated by, rather than growing a second, subtly different notion of
/// what an ILP address is.
pub(crate) fn is_valid_ilp_address(address: &str) -> bool {
    !address.is_empty()
        && address.len() <= MAX_ILP_ADDRESS_LEN
        && address.split('.').all(is_valid_label)
}

/// A `[[routes]]` entry as written in the config file: forwards to the app
/// at `handler_url`, or to the peer named `peer_id` -- exactly one of the
/// two must be set.
///
/// `price` is required on **both** branches (ADR 0028), and means the same
/// thing on each: what this connector's client edge charges a client for a
/// packet to this prefix. On a terminated route it buys the app's work
/// (issue #520); on a forwarded one it buys the whole path, of which this
/// hop keeps the *peering's* fee. Its absence is
/// [`ConfigError::RouteMissingPrice`] or
/// [`ConfigError::PeerRouteMissingPrice`] respectively -- no route is ever
/// silently free (issue #557).
///
/// `fee` is **not** a route key any more (ADR 0061): what this hop retains
/// is the same number whichever prefix a packet is addressed to, so it
/// belongs to the peering and is written on the `[[peers]]` row instead. It
/// is kept here as a *parsed and rejected* tombstone
/// ([`ConfigError::RouteFeeRemoved`]) -- on **either** branch, terminated or
/// forwarded -- so a config that still writes one stops the node by name
/// rather than being read and discarded (ADR 0009). `toml::Value` rather
/// than `u64` for the same reason `ceiling` is: the key is refused for
/// existing, so any spelling of a value must reach the refusal.
///
/// `transport` is only meaningful alongside `handler_url` (toon-meta#262
/// decision 11, issue #701) -- not because a forwarded route is unreachable
/// over a client transport (ADR 0028 makes it reachable over both), but
/// because the policy is not applied to one, and it is refused rather than
/// ignored on the branch it does not belong to
/// ([`ConfigError::PeerRouteHasTransport`], issue #556).
///
/// `deny_unknown_fields` closes the same hole for every other key: a
/// mistyped `pefix` or `handler_ur` is a refuse-to-start error, not a route
/// quietly resolved from the fields that happened to spell correctly.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawRoute {
    prefix: String,
    #[serde(default)]
    handler_url: Option<String>,
    #[serde(default)]
    peer_id: Option<String>,
    /// Moved to the `[[peers]]` row (ADR 0061); parsed only so it can be
    /// refused **by name**, the way a peer's `ceiling` is.
    #[serde(default)]
    fee: Option<toml::Value>,
    #[serde(default)]
    price: Option<Price>,
    #[serde(default)]
    transport: Option<String>,
    /// What a client should send to use this route (issue #1210): an
    /// operator-declared table the connector parses only far enough to
    /// confirm it IS a table, and never reads a key out of -- the app that
    /// registered those keys is the only authority on what they mean, and
    /// the app's **own** repository, where app and connector are composed,
    /// is where a declaration here is checked against what actually runs
    /// (ADR 0067). Not `deny_unknown_fields`: that guarantee belongs to
    /// this row, not to a blob whose keys are the app's business.
    #[serde(default)]
    request: Option<toml::Table>,
    /// The largest sealed payload, in bytes, a packet to this prefix may
    /// carry (ADR 0081, issue #1530). Optional on both branches; absent means
    /// no limit beyond the carriage's own. Validated by [`parse_max_bytes`].
    #[serde(default)]
    max_bytes: Option<u64>,
}

/// A `[[children]]` entry: a convenience form that desugars into a
/// [`RawRoute`] at `<apex>.<name>` once the file is loaded (mirroring the
/// existing TypeScript `child-expander`), so the runtime never sees anything
/// but ordinary routes. Always terminates locally, so `price` is required
/// exactly like an explicit `[[routes]]` entry with a `handler_url`, and
/// `transport` (issue #701) is meaningful here too, for the same reason.
///
/// `deny_unknown_fields` (issue #556): a child always terminates locally,
/// so it has no `fee` field at all -- writing one used to vanish silently,
/// and now fails config load, matching what an explicit `[[routes]]` entry
/// does with the same key now that ADR 0061 has moved it to `[[peers]]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawChild {
    name: String,
    handler_url: String,
    #[serde(default)]
    price: Option<Price>,
    #[serde(default)]
    transport: Option<String>,
}

/// Which client transport(s) a terminated route accepts a request over
/// (toon-meta#262 decision 11, issue #701): transport is per-connector
/// policy, not a protocol constant. The right answer depends on traffic
/// shape -- a persistent, high-frequency relationship (huddles' 49 fps)
/// amortizes a BTP session's handshake and claim round trip; a one-shot
/// anonymous buyer should not need one. Defaults to [`TransportPolicy::Both`]
/// so no deployed route changes behavior until an operator opts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransportPolicy {
    /// Only `POST /ilp` is accepted; a request over the BTP session is
    /// refused with terms pointing the client at HTTP instead.
    Http,
    /// Only the BTP websocket session is accepted; a request over `POST
    /// /ilp` is refused with terms pointing the client at BTP instead.
    Btp,
    /// Both transports are accepted -- the default, and the only behavior
    /// that existed before issue #701.
    #[default]
    Both,
}

impl TransportPolicy {
    /// The config-file spelling of this policy -- the one spelling this
    /// workspace uses anywhere a transport policy is named to or by an
    /// operator.
    pub fn name(self) -> &'static str {
        match self {
            TransportPolicy::Http => "http",
            TransportPolicy::Btp => "btp",
            TransportPolicy::Both => "both",
        }
    }

    /// Whether a request over `POST /ilp` is accepted under this policy.
    pub fn accepts_http(self) -> bool {
        matches!(self, TransportPolicy::Http | TransportPolicy::Both)
    }

    /// Whether a request over the BTP websocket session is accepted under
    /// this policy.
    pub fn accepts_btp(self) -> bool {
        matches!(self, TransportPolicy::Btp | TransportPolicy::Both)
    }
}

impl std::fmt::Display for TransportPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for TransportPolicy {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "http" => Ok(TransportPolicy::Http),
            "btp" => Ok(TransportPolicy::Btp),
            "both" => Ok(TransportPolicy::Both),
            _ => Err(()),
        }
    }
}

/// A static route that terminates at this connector: packets matching
/// `prefix` are delivered to the app at `handler_url`, which charges
/// `price` for the work it does -- distinct from a peering's `fee`, which
/// buys carriage rather than the terminating app's work (issue #520, ADR
/// 0061). A peer route has a `price` of its own (ADR 0028), meaning the
/// same client-facing thing this one does. A route is never silently free: [`resolve_routes`] refuses to
/// return one with no configured price, so `price == 0` always means the
/// operator wrote it deliberately.
///
/// Constructed only by [`resolve_routes`], so a value that exists has
/// already had its prefix, URL, price and transport policy validated --
/// downstream code never re-checks any of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticRoute {
    prefix: String,
    handler_url: Url,
    price: Price,
    transport_policy: TransportPolicy,
    request: Option<serde_json::Value>,
    max_bytes: Option<PayloadLimit>,
}

impl StaticRoute {
    /// Construct and fully validate a single static route directly, priced
    /// at zero and accepting both transports -- the same validation
    /// [`resolve_routes`] applies, exposed for a caller that already has a
    /// prefix and handler URL in hand (tests, and any future
    /// operator-surface route creation) rather than a whole config file.
    /// Unlike the config-file path there is no load-time gate here to make
    /// a zero price silent, so a caller that cares about pricing should use
    /// [`StaticRoute::new_priced`] instead.
    pub fn new(
        prefix: impl Into<String>,
        handler_url: impl Into<String>,
    ) -> Result<StaticRoute, ConfigError> {
        build_route(
            prefix.into(),
            handler_url.into(),
            Some(Price::FREE),
            TransportPolicy::Both,
            None,
        )
    }

    /// Construct and fully validate a single static route with an explicit
    /// price (issue #520), accepting both transports.
    pub fn new_priced(
        prefix: impl Into<String>,
        handler_url: impl Into<String>,
        price: u64,
    ) -> Result<StaticRoute, ConfigError> {
        StaticRoute::new_scheduled(prefix, handler_url, Price::flat(price))
    }

    /// Construct and fully validate a single static route charging a whole
    /// schedule (ADR 0065, issue #984) -- a `price` that may carry a slope,
    /// of which [`StaticRoute::new_priced`] above is the flat case.
    pub fn new_scheduled(
        prefix: impl Into<String>,
        handler_url: impl Into<String>,
        price: Price,
    ) -> Result<StaticRoute, ConfigError> {
        build_route(
            prefix.into(),
            handler_url.into(),
            Some(price),
            TransportPolicy::Both,
            None,
        )
    }

    /// Construct and fully validate a single static route with an explicit
    /// price and transport policy (issue #701).
    pub fn new_priced_with_transport(
        prefix: impl Into<String>,
        handler_url: impl Into<String>,
        price: u64,
        transport_policy: TransportPolicy,
    ) -> Result<StaticRoute, ConfigError> {
        StaticRoute::new_scheduled_with_transport(
            prefix,
            handler_url,
            Price::flat(price),
            transport_policy,
        )
    }

    /// Construct and fully validate a single static route charging a whole
    /// schedule, with an explicit transport policy (ADR 0065, issue #701).
    pub fn new_scheduled_with_transport(
        prefix: impl Into<String>,
        handler_url: impl Into<String>,
        price: Price,
        transport_policy: TransportPolicy,
    ) -> Result<StaticRoute, ConfigError> {
        build_route(
            prefix.into(),
            handler_url.into(),
            Some(price),
            transport_policy,
            None,
        )
    }

    /// The destination prefix this route terminates.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The app handler this route's traffic is delivered to.
    pub fn handler_url(&self) -> &Url {
        &self.handler_url
    }

    /// The price schedule a claim must advance by to pay for this route
    /// (issue #520) -- flat exactly when its slope is zero, which is every
    /// route ADR 0020 could express and every route the fleet runs.
    ///
    /// A *schedule*, not a figure: what one packet costs is
    /// `price().charge(prepare.data.len())` (ADR 0065, issue #984), and every
    /// gate that charges evaluates it that way. Charged against a client-edge
    /// claim (issue #522) and, since issue #752, checked against a peer-role
    /// arrival's own `amount` before it is delivered
    /// (`Connector::handle_peer_prepare`).
    pub fn price(&self) -> Price {
        self.price
    }

    /// Which client transport(s) this route accepts a request over (issue
    /// #701). Defaults to [`TransportPolicy::Both`] when the operator wrote
    /// nothing.
    pub fn transport_policy(&self) -> TransportPolicy {
        self.transport_policy
    }

    /// What a client should send to use this route (issue #1210) -- the
    /// operator's `request` table, converted to JSON and published verbatim
    /// in the node self-description and the x402 greeting. `None` when the
    /// operator wrote nothing, which is every route before this issue.
    pub fn request(&self) -> Option<&serde_json::Value> {
        self.request.as_ref()
    }

    /// Attach a request declaration to a route built directly rather than
    /// from a config file (issue #1210) -- a fixture's shorthand for what
    /// `[[routes]] request = { ... }` gives a config-loaded route.
    pub fn with_request(mut self, request: serde_json::Value) -> StaticRoute {
        self.request = Some(request);
        self
    }

    /// The largest sealed payload a packet to this prefix may carry (ADR
    /// 0081, issue #1530) -- refused `F00` above it, before any claim is
    /// taken. `None` when the operator wrote no `max_bytes`, which is every
    /// route before ADR 0081.
    pub fn max_bytes(&self) -> Option<PayloadLimit> {
        self.max_bytes
    }

    /// Attach a payload limit to a route built directly rather than from a
    /// config file (ADR 0081) -- a fixture's shorthand for what
    /// `[[routes]] max_bytes = ...` gives a config-loaded route.
    pub fn with_max_bytes(mut self, max_bytes: Option<PayloadLimit>) -> StaticRoute {
        self.max_bytes = max_bytes;
        self
    }
}

/// A route whose traffic this connector forwards to a peer rather than
/// terminating at an app of its own. This is the config-file counterpart of
/// `connector_runtime::PeerRoute` -- kept as its own type here (rather than
/// depending on `connector-runtime`, which itself depends on this crate) --
/// `connector-cli` converts one into the other when it builds the runtime
/// `Connector`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRouteConfig {
    prefix: String,
    peer_id: String,
    price: Price,
    request: Option<serde_json::Value>,
    max_bytes: Option<PayloadLimit>,
}

impl PeerRouteConfig {
    /// The destination prefix this route forwards.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The peer this route's traffic is forwarded to, by id -- matched
    /// against a `[[peers]]` entry's own `id`.
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// The price schedule this connector's client edge charges a client for
    /// a packet to this prefix (ADR 0028) -- greeted, gated and journaled on
    /// exactly the path a terminated route's own price is, and evaluated at
    /// the packet's own payload length the same way (ADR 0065). Always
    /// written down: [`resolve_routes`] refuses a forwarded route with no
    /// price, so a free schedule always means deliberate free carriage.
    pub fn price(&self) -> Price {
        self.price
    }

    /// What a client should send to use this route (issue #1210) -- see
    /// [`StaticRoute::request`]; same meaning, same opacity, on the
    /// forwarded branch.
    pub fn request(&self) -> Option<&serde_json::Value> {
        self.request.as_ref()
    }

    /// The largest sealed payload a packet to this prefix may carry (ADR
    /// 0081) -- see [`StaticRoute::max_bytes`]; a forwarding hop refuses on
    /// it exactly as a termination does, since the length it measures is
    /// carriage rather than content.
    pub fn max_bytes(&self) -> Option<PayloadLimit> {
        self.max_bytes
    }
}

/// Validate a route's `prefix` field, shared by [`build_route`] and
/// [`build_peer_route`] since both kinds of route are keyed by the same
/// ILP address rules.
fn validate_prefix(prefix: String) -> Result<String, ConfigError> {
    if !is_valid_ilp_address(&prefix) {
        return Err(ConfigError::InvalidAddress {
            field: "prefix",
            value: prefix,
        });
    }
    Ok(prefix)
}

fn build_route(
    prefix: String,
    handler_url: String,
    price: Option<Price>,
    transport_policy: TransportPolicy,
    request: Option<toml::Table>,
) -> Result<StaticRoute, ConfigError> {
    let prefix = validate_prefix(prefix)?;
    let url = Url::parse(&handler_url).map_err(|source| ConfigError::InvalidHandlerUrl {
        prefix: prefix.clone(),
        value: handler_url.clone(),
        source,
    })?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(ConfigError::UnsupportedUrlScheme {
            prefix,
            value: handler_url,
        });
    }
    let price = price.ok_or_else(|| ConfigError::RouteMissingPrice {
        prefix: prefix.clone(),
        handler_url: url.to_string(),
    })?;
    Ok(StaticRoute {
        prefix,
        handler_url: url,
        price,
        transport_policy,
        request: request.map(request_to_json),
        max_bytes: None,
    })
}

/// Convert a parsed `request` table into the JSON value the self-description
/// and the x402 greeting actually publish (issue #1210) -- once, at load,
/// rather than on every request answered. A `toml::Table` that parsed at all
/// always converts: its keys are already `String`s, and nothing JSON cannot
/// represent survives a `[[routes]]` entry an operator would plausibly write.
fn request_to_json(table: toml::Table) -> serde_json::Value {
    serde_json::to_value(table).expect("a parsed TOML table always converts to JSON")
}

/// Parse a `[[routes]]`/`[[children]]` entry's `transport` field (issue
/// #701), defaulting to [`TransportPolicy::Both`] when the operator wrote
/// nothing -- a refuse-to-start error rather than a silently ignored value
/// when the string is written but not one of the three recognized spellings
/// (`deny_unknown_fields` already closes the mistyped-key hole; this closes
/// the mistyped-value one).
fn parse_transport_policy(
    prefix: &str,
    value: Option<String>,
) -> Result<TransportPolicy, ConfigError> {
    match value {
        None => Ok(TransportPolicy::default()),
        Some(value) => value
            .parse()
            .map_err(|()| ConfigError::InvalidTransportPolicy {
                prefix: prefix.to_string(),
                value,
            }),
    }
}

/// Record `route`'s `(handler_url, price)` pair, refusing a second route
/// that reuses the same `handler_url` at a different price (issue #520):
/// the app behind that handler cannot tell which request arrived under
/// which price, so the cheaper one would always win.
///
/// Compares whole **schedules** since ADR 0065, which is why [`Price`] is a
/// struct rather than an enum: `1000` and `{ base = 1000, per_kib = 0 }` are
/// one value, so writing a handler's price both ways is agreement rather than
/// a conflict, while any difference in either field is a conflict for exactly
/// the reason above.
fn insert_consistent_handler_price(
    seen: &mut HashMap<String, (String, Price)>,
    route: &StaticRoute,
) -> Result<(), ConfigError> {
    let handler_url = route.handler_url().to_string();
    match seen.get(&handler_url) {
        Some((first_prefix, first_price)) if *first_price != route.price() => {
            Err(ConfigError::ConflictingHandlerPrice {
                handler_url,
                first_prefix: first_prefix.clone(),
                first_price: *first_price,
                second_prefix: route.prefix().to_string(),
                second_price: route.price(),
            })
        }
        Some(_) => Ok(()),
        None => {
            seen.insert(handler_url, (route.prefix().to_string(), route.price()));
            Ok(())
        }
    }
}

fn build_peer_route(
    prefix: String,
    peer_id: String,
    price: Option<Price>,
    request: Option<toml::Table>,
) -> Result<PeerRouteConfig, ConfigError> {
    let prefix = validate_prefix(prefix)?;
    if peer_id.trim().is_empty() {
        return Err(ConfigError::RoutePeerIdEmpty { prefix });
    }
    let price = price.ok_or_else(|| ConfigError::PeerRouteMissingPrice {
        prefix: prefix.clone(),
        peer_id: peer_id.clone(),
    })?;
    Ok(PeerRouteConfig {
        prefix,
        peer_id,
        price,
        request: request.map(request_to_json),
        max_bytes: None,
    })
}

/// Parse a `[[routes]]` entry's `max_bytes` (ADR 0081), refusing by name a
/// value that is not a limit at all -- zero refuses every payload -- or one
/// above [`MAX_PAYLOAD_LIMIT`], which no packet on the client edge's HTTP
/// carriage could ever reach.
fn parse_max_bytes(prefix: &str, value: Option<u64>) -> Result<Option<PayloadLimit>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value > MAX_PAYLOAD_LIMIT {
        return Err(ConfigError::RouteMaxBytesAboveCarriage {
            prefix: prefix.to_string(),
            value,
            limit: MAX_PAYLOAD_LIMIT,
        });
    }
    PayloadLimit::new(value)
        .map(Some)
        .ok_or_else(|| ConfigError::RouteMaxBytesZero {
            prefix: prefix.to_string(),
        })
}

fn insert_unique_prefix(seen: &mut HashSet<String>, prefix: &str) -> Result<(), ConfigError> {
    if !seen.insert(prefix.to_string()) {
        return Err(ConfigError::DuplicatePrefix {
            prefix: prefix.to_string(),
        });
    }
    Ok(())
}

/// Resolve `routes` and desugar `children` (under `apex`) into fully
/// validated, deduplicated route tables -- app routes and peer routes,
/// sharing one prefix namespace (a peer route and an app route can never
/// claim the same prefix). `children` always desugars into app routes,
/// matching its role as a convenience form for this node's own apps.
pub(crate) fn resolve_routes(
    apex: Option<&str>,
    raw_routes: Vec<RawRoute>,
    raw_children: Vec<RawChild>,
) -> Result<(Vec<StaticRoute>, Vec<PeerRouteConfig>), ConfigError> {
    let mut seen = HashSet::with_capacity(raw_routes.len() + raw_children.len());
    let mut handler_prices = HashMap::new();
    let mut routes = Vec::with_capacity(raw_routes.len());
    let mut peer_routes = Vec::new();

    for raw in raw_routes {
        // ADR 0061: a fee attaches to a peering, not to a route. Refused on
        // BOTH branches and before either is examined -- a terminated route
        // never had one to charge (issue #556's rule, unchanged), and a
        // forwarded one now reads it off the `[[peers]]` row its `peer_id`
        // names. Refused by name rather than read and discarded, because an
        // operator who wrote a fee here believes this hop is charging it
        // (ADR 0009).
        if raw.fee.is_some() {
            return Err(ConfigError::RouteFeeRemoved { prefix: raw.prefix });
        }
        let max_bytes = parse_max_bytes(&raw.prefix, raw.max_bytes)?;
        match (raw.handler_url, raw.peer_id) {
            (Some(handler_url), None) => {
                let transport_policy = parse_transport_policy(&raw.prefix, raw.transport)?;
                let mut route = build_route(
                    raw.prefix,
                    handler_url,
                    raw.price,
                    transport_policy,
                    raw.request,
                )?;
                route.max_bytes = max_bytes;
                insert_unique_prefix(&mut seen, route.prefix())?;
                insert_consistent_handler_price(&mut handler_prices, &route)?;
                routes.push(route);
            }
            (None, Some(peer_id)) => {
                // A forwarded route carries one number of its own (ADR
                // 0028): `price`, what this connector's client edge charges
                // a client for the packet. What this hop retains of it is
                // the peering's `fee` (ADR 0061), read off `[[peers]]`.
                // What a forwarded route still cannot carry is a
                // `transport` policy -- not because it is unreachable over
                // a client transport, but because that policy is not
                // applied to a forwarded route (issue #701); refused rather
                // than read and discarded (issue #556).
                if let Some(value) = raw.transport {
                    return Err(ConfigError::PeerRouteHasTransport {
                        prefix: raw.prefix,
                        value,
                    });
                }
                let mut route = build_peer_route(raw.prefix, peer_id, raw.price, raw.request)?;
                route.max_bytes = max_bytes;
                insert_unique_prefix(&mut seen, route.prefix())?;
                peer_routes.push(route);
            }
            (None, None) => {
                return Err(ConfigError::RouteMissingTarget { prefix: raw.prefix });
            }
            (Some(_), Some(_)) => {
                return Err(ConfigError::RouteTargetAmbiguous { prefix: raw.prefix });
            }
        }
    }

    if raw_children.is_empty() {
        return Ok((routes, peer_routes));
    }

    let apex = apex.ok_or(ConfigError::MissingApex)?;
    if !is_valid_ilp_address(apex) {
        return Err(ConfigError::InvalidAddress {
            field: "apex",
            value: apex.to_string(),
        });
    }

    for child in raw_children {
        if !is_valid_label(&child.name) {
            return Err(ConfigError::InvalidChildName { name: child.name });
        }
        let prefix = format!("{apex}.{}", child.name);
        let transport_policy = parse_transport_policy(&prefix, child.transport)?;
        let route = build_route(
            prefix,
            child.handler_url,
            child.price,
            transport_policy,
            None,
        )?;
        insert_unique_prefix(&mut seen, route.prefix())?;
        insert_consistent_handler_price(&mut handler_prices, &route)?;
        routes.push(route);
    }

    Ok((routes, peer_routes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(prefix: &str, handler_url: &str) -> RawRoute {
        priced_route(prefix, handler_url, 0)
    }

    fn priced_route(prefix: &str, handler_url: &str, price: u64) -> RawRoute {
        RawRoute {
            prefix: prefix.to_string(),
            handler_url: Some(handler_url.to_string()),
            peer_id: None,
            fee: None,
            price: Some(Price::flat(price)),
            transport: None,
            request: None,
            max_bytes: None,
        }
    }

    fn peer_route(prefix: &str, peer_id: &str) -> RawRoute {
        priced_peer_route(prefix, peer_id, 0)
    }

    fn priced_peer_route(prefix: &str, peer_id: &str, price: u64) -> RawRoute {
        RawRoute {
            prefix: prefix.to_string(),
            handler_url: None,
            peer_id: Some(peer_id.to_string()),
            fee: None,
            price: Some(Price::flat(price)),
            transport: None,
            request: None,
            max_bytes: None,
        }
    }

    fn child(name: &str, handler_url: &str) -> RawChild {
        RawChild {
            name: name.to_string(),
            handler_url: handler_url.to_string(),
            price: Some(Price::FREE),
            transport: None,
        }
    }

    #[test]
    fn static_route_new_validates_like_resolve_routes() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").expect("new");
        assert_eq!(route.prefix(), "g.example.app");
        assert_eq!(route.price(), Price::FREE);

        let result = StaticRoute::new("g..app", "http://localhost:4000");
        assert!(matches!(result, Err(ConfigError::InvalidAddress { .. })));
    }

    #[test]
    fn static_route_new_priced_carries_the_given_price() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 42)
            .expect("new_priced");
        assert_eq!(route.price(), Price::flat(42));
    }

    /// ADR 0081: a forwarding hop refuses on its own route's limit exactly as
    /// a termination does, so `max_bytes` loads on the forwarded branch too.
    #[test]
    fn a_forwarded_route_carries_its_payload_limit() {
        let mut raw = priced_peer_route("g.peer-b", "peer-b", 100);
        raw.max_bytes = Some(4096);
        let (_, peer_routes) = resolve_routes(None, vec![raw], vec![]).expect("resolve");
        assert_eq!(peer_routes[0].max_bytes(), PayloadLimit::new(4096));
    }

    #[test]
    fn a_payload_limit_at_the_carriage_limit_loads_and_one_over_is_refused() {
        let mut at = priced_route("g.example.at", "http://localhost:4000", 1);
        at.max_bytes = Some(MAX_PAYLOAD_LIMIT);
        let (routes, _) = resolve_routes(None, vec![at], vec![]).expect("at the limit loads");
        assert_eq!(routes[0].max_bytes(), PayloadLimit::new(MAX_PAYLOAD_LIMIT));

        let mut over = priced_peer_route("g.peer-b", "peer-b", 1);
        over.max_bytes = Some(MAX_PAYLOAD_LIMIT + 1);
        assert!(matches!(
            resolve_routes(None, vec![over], vec![]),
            Err(ConfigError::RouteMaxBytesAboveCarriage { prefix, .. }) if prefix == "g.peer-b"
        ));
    }

    #[test]
    fn resolves_explicit_routes() {
        let (routes, peer_routes) = resolve_routes(
            None,
            vec![priced_route("g.example.app", "http://localhost:4000", 25)],
            vec![],
        )
        .expect("resolve");

        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].prefix(), "g.example.app");
        assert_eq!(routes[0].handler_url().as_str(), "http://localhost:4000/");
        assert_eq!(routes[0].price(), Price::flat(25));
        assert!(peer_routes.is_empty());
    }

    #[test]
    fn a_terminated_route_with_no_price_is_rejected_at_load() {
        let result = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.example.app".to_string(),
                handler_url: Some("http://localhost:4000".to_string()),
                peer_id: None,
                fee: None,
                price: None,
                transport: None,
                request: None,
                max_bytes: None,
            }],
            vec![],
        );
        assert!(matches!(result, Err(ConfigError::RouteMissingPrice { .. })));
    }

    /// ADR 0061: a fee attaches to a peering, not to a route. A terminated
    /// route never had one to charge (issue #556's rule, which this
    /// subsumes), and the error now names where the key went rather than
    /// suggesting `price` in its place.
    #[test]
    fn a_terminated_route_that_sets_a_fee_is_rejected_rather_than_ignored() {
        let result = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.example.app".to_string(),
                handler_url: Some("http://localhost:4000".to_string()),
                peer_id: None,
                fee: Some(toml::Value::Integer(5)),
                price: Some(Price::flat(100)),
                transport: None,
                request: None,
                max_bytes: None,
            }],
            vec![],
        );
        assert!(matches!(
            result,
            Err(ConfigError::RouteFeeRemoved { prefix }) if prefix == "g.example.app"
        ));
    }

    /// `fee = 0` is refused too: the point is that the key was written at
    /// all, not that its value was non-zero -- `fee` is parsed as an opaque
    /// `toml::Value` precisely so every spelling reaches the refusal.
    #[test]
    fn a_terminated_route_that_sets_a_zero_fee_is_still_rejected() {
        let result = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.example.app".to_string(),
                handler_url: Some("http://localhost:4000".to_string()),
                peer_id: None,
                fee: Some(toml::Value::Integer(0)),
                price: Some(Price::flat(100)),
                transport: None,
                request: None,
                max_bytes: None,
            }],
            vec![],
        );
        assert!(matches!(result, Err(ConfigError::RouteFeeRemoved { .. })));
    }

    /// ADR 0061's actual move: the key is refused on a **forwarded** route
    /// too, which is the only branch that ever read it. `TerminatedRouteHasFee`
    /// refused it on one branch and honoured it on the other; a tombstone
    /// that a forwarded route could still write would leave every config in
    /// the tree spelling a fee the connector no longer reads from there.
    #[test]
    fn a_forwarded_route_that_sets_a_fee_is_rejected_too() {
        let result = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.example.store".to_string(),
                handler_url: None,
                peer_id: Some("store".to_string()),
                fee: Some(toml::Value::Integer(3)),
                price: Some(Price::flat(100)),
                transport: None,
                request: None,
                max_bytes: None,
            }],
            vec![],
        );
        assert!(matches!(
            result,
            Err(ConfigError::RouteFeeRemoved { prefix }) if prefix == "g.example.store"
        ));
    }

    /// ADR 0028: a forwarded route carries a `price` of its own -- what
    /// this connector's client edge charges the client for the whole path.
    /// Before ADR 0028 that was a hard `PeerRouteHasPrice` error, which is
    /// exactly why no configuration could charge for a packet that crossed
    /// a peering (issue #620). What this hop retains of it is the peering's
    /// fee, and lives on `[[peers]]` (ADR 0061).
    #[test]
    fn a_peer_route_carries_a_client_edge_price() {
        let (routes, peer_routes) = resolve_routes(
            None,
            vec![priced_peer_route("g.example.store", "store", 100)],
            vec![],
        )
        .expect("resolve");

        assert!(routes.is_empty());
        assert_eq!(peer_routes[0].price(), Price::flat(100));
    }

    /// The counterweight: a price on the branch it belongs to is still read
    /// and still honoured, on both kinds of route.
    #[test]
    fn each_field_is_honoured_on_the_branch_it_belongs_to() {
        let (routes, peer_routes) = resolve_routes(
            None,
            vec![
                priced_route("g.example.app", "http://localhost:4000", 100),
                priced_peer_route("g.example.store", "store", 40),
            ],
            vec![],
        )
        .expect("resolve");

        assert_eq!(routes[0].price(), Price::flat(100));
        assert_eq!(peer_routes[0].price(), Price::flat(40));
    }

    #[test]
    fn a_terminated_route_priced_zero_is_deliberately_free_not_rejected() {
        let (routes, _) = resolve_routes(
            None,
            vec![priced_route("g.example.app", "http://localhost:4000", 0)],
            vec![],
        )
        .expect("resolve");
        assert_eq!(routes[0].price(), Price::FREE);
    }

    /// Issue #557's "never silently free", applied to the forwarded branch
    /// too (ADR 0028): a `peer_id` route with no `price` is a refuse-to-
    /// start error rather than a route that quietly charges nothing at the
    /// client edge -- which is precisely the free-gateway shape a peer
    /// route had by construction before this change.
    #[test]
    fn a_peer_route_with_no_price_is_rejected_at_load() {
        let result = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.peer-b".to_string(),
                handler_url: None,
                peer_id: Some("peer-b".to_string()),
                fee: None,
                price: None,
                transport: None,
                request: None,
                max_bytes: None,
            }],
            vec![],
        );
        assert!(matches!(
            result,
            Err(ConfigError::PeerRouteMissingPrice { peer_id, .. }) if peer_id == "peer-b"
        ));
    }

    /// The counterweight, exactly as for a terminated route: `price = 0` is
    /// deliberate free carriage, written down, and is not an error.
    #[test]
    fn a_peer_route_priced_zero_is_deliberately_free_not_rejected() {
        let (_, peer_routes) =
            resolve_routes(None, vec![peer_route("g.peer-b", "peer-b")], vec![]).expect("resolve");
        assert_eq!(peer_routes[0].price(), Price::FREE);
    }

    #[test]
    fn a_child_with_no_price_is_rejected_at_load() {
        let result = resolve_routes(
            Some("g.example.connector"),
            vec![],
            vec![RawChild {
                name: "billing".to_string(),
                handler_url: "http://localhost:4001".to_string(),
                price: None,
                transport: None,
            }],
        );
        assert!(matches!(result, Err(ConfigError::RouteMissingPrice { .. })));
    }

    #[test]
    fn two_routes_sharing_a_handler_at_the_same_price_both_load() {
        let (routes, _) = resolve_routes(
            None,
            vec![
                priced_route("g.example.a", "http://localhost:4000", 10),
                priced_route("g.example.b", "http://localhost:4000", 10),
            ],
            vec![],
        )
        .expect("resolve");
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn two_routes_sharing_a_handler_at_different_prices_are_rejected() {
        let result = resolve_routes(
            None,
            vec![
                priced_route("g.example.a", "http://localhost:4000", 10),
                priced_route("g.example.b", "http://localhost:4000", 20),
            ],
            vec![],
        );
        assert!(matches!(
            result,
            Err(ConfigError::ConflictingHandlerPrice { .. })
        ));
    }

    #[test]
    fn a_route_and_a_child_sharing_a_handler_at_different_prices_are_rejected() {
        let result = resolve_routes(
            Some("g.example.connector"),
            vec![priced_route("g.example.other", "http://localhost:4000", 10)],
            vec![RawChild {
                name: "billing".to_string(),
                handler_url: "http://localhost:4000".to_string(),
                price: Some(Price::flat(20)),
                transport: None,
            }],
        );
        assert!(matches!(
            result,
            Err(ConfigError::ConflictingHandlerPrice { .. })
        ));
    }

    // --- ADR 0065: a price may carry a slope (issue #984) ---------------

    fn scheduled_route(prefix: &str, handler_url: &str, price: Price) -> RawRoute {
        RawRoute {
            prefix: prefix.to_string(),
            handler_url: Some(handler_url.to_string()),
            peer_id: None,
            fee: None,
            price: Some(price),
            transport: None,
            request: None,
            max_bytes: None,
        }
    }

    #[test]
    fn a_terminated_route_resolves_a_whole_schedule() {
        let (routes, _) = resolve_routes(
            None,
            vec![scheduled_route(
                "g.example.store",
                "http://localhost:4000",
                Price::scheduled(1000, 30),
            )],
            vec![],
        )
        .expect("resolve");

        assert_eq!(routes[0].price(), Price::scheduled(1000, 30));
        // What one packet costs is the schedule evaluated at its own length.
        assert_eq!(routes[0].price().charge(100 * 1024), 4_000);
    }

    #[test]
    fn a_forwarded_route_resolves_a_whole_schedule() {
        // ADR 0028 prices a forwarded route at the client edge, and ADR 0065
        // does not carve it out: the edge measures the same payload length a
        // termination would.
        let (_, peer_routes) = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.peer-b".to_string(),
                handler_url: None,
                peer_id: Some("peer-b".to_string()),
                fee: None,
                price: Some(Price::scheduled(100, 5)),
                transport: None,
                request: None,
                max_bytes: None,
            }],
            vec![],
        )
        .expect("resolve");

        assert_eq!(peer_routes[0].price(), Price::scheduled(100, 5));
    }

    #[test]
    fn a_child_resolves_a_whole_schedule() {
        let (routes, _) = resolve_routes(
            Some("g.example.connector"),
            vec![],
            vec![RawChild {
                name: "billing".to_string(),
                handler_url: "http://localhost:4000".to_string(),
                price: Some(Price::scheduled(7, 3)),
                transport: None,
            }],
        )
        .expect("resolve");

        assert_eq!(routes[0].prefix(), "g.example.connector.billing");
        assert_eq!(routes[0].price(), Price::scheduled(7, 3));
    }

    #[test]
    fn one_handler_priced_flat_and_by_schedule_is_rejected() {
        let result = resolve_routes(
            None,
            vec![
                priced_route("g.example.cheap", "http://localhost:4000", 1000),
                scheduled_route(
                    "g.example.dear",
                    "http://localhost:4000",
                    Price::scheduled(1000, 30),
                ),
            ],
            vec![],
        );
        // Same reason as the flat case: the app behind that handler cannot
        // tell which request arrived under which schedule, so every packet
        // above a kibibyte would be bought at the flat route's price.
        assert!(matches!(
            result,
            Err(ConfigError::ConflictingHandlerPrice { .. })
        ));
    }

    #[test]
    fn one_handler_priced_flat_and_at_a_zero_slope_is_accepted() {
        // `1000` and `{ base = 1000, per_kib = 0 }` are the same value, so
        // spelling one handler's price both ways is agreement, not conflict.
        let (routes, _) = resolve_routes(
            None,
            vec![
                priced_route("g.example.one", "http://localhost:4000", 1000),
                scheduled_route(
                    "g.example.two",
                    "http://localhost:4000",
                    Price::scheduled(1000, 0),
                ),
            ],
            vec![],
        )
        .expect("resolve");

        assert_eq!(routes[0].price(), routes[1].price());
    }

    #[test]
    fn one_handler_at_two_identical_schedules_is_accepted() {
        let (routes, _) = resolve_routes(
            None,
            vec![
                scheduled_route(
                    "g.example.one",
                    "http://localhost:4000",
                    Price::scheduled(1000, 30),
                ),
                scheduled_route(
                    "g.example.two",
                    "http://localhost:4000",
                    Price::scheduled(1000, 30),
                ),
            ],
            vec![],
        )
        .expect("resolve");

        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn one_handler_at_two_slopes_over_one_base_is_rejected() {
        let result = resolve_routes(
            None,
            vec![
                scheduled_route(
                    "g.example.one",
                    "http://localhost:4000",
                    Price::scheduled(1000, 30),
                ),
                scheduled_route(
                    "g.example.two",
                    "http://localhost:4000",
                    Price::scheduled(1000, 31),
                ),
            ],
            vec![],
        );
        assert!(matches!(
            result,
            Err(ConfigError::ConflictingHandlerPrice { .. })
        ));
    }

    #[test]
    fn a_conflicting_schedule_is_reported_with_both_spellings() {
        let result = resolve_routes(
            None,
            vec![
                priced_route("g.example.cheap", "http://localhost:4000", 1000),
                scheduled_route(
                    "g.example.dear",
                    "http://localhost:4000",
                    Price::scheduled(1000, 30),
                ),
            ],
            vec![],
        );
        let message = result.expect_err("conflicting").to_string();
        // An error naming two prices has to name what distinguishes them, or
        // it reads as "1000 is not 1000".
        assert!(message.contains("1000 + 30/KiB"), "got: {message}");
    }

    #[test]
    fn resolves_explicit_peer_routes() {
        let (routes, peer_routes) =
            resolve_routes(None, vec![peer_route("g.peer-b", "peer-b")], vec![]).expect("resolve");

        assert!(routes.is_empty());
        assert_eq!(peer_routes.len(), 1);
        assert_eq!(peer_routes[0].prefix(), "g.peer-b");
        assert_eq!(peer_routes[0].peer_id(), "peer-b");
    }

    #[test]
    fn rejects_a_route_with_neither_handler_url_nor_peer_id() {
        let result = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.example.app".to_string(),
                handler_url: None,
                peer_id: None,
                fee: None,
                price: None,
                transport: None,
                request: None,
                max_bytes: None,
            }],
            vec![],
        );
        assert!(matches!(
            result,
            Err(ConfigError::RouteMissingTarget { .. })
        ));
    }

    #[test]
    fn rejects_a_route_with_both_handler_url_and_peer_id() {
        let result = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.example.app".to_string(),
                handler_url: Some("http://localhost:4000".to_string()),
                peer_id: Some("peer-b".to_string()),
                fee: None,
                price: None,
                transport: None,
                request: None,
                max_bytes: None,
            }],
            vec![],
        );
        assert!(matches!(
            result,
            Err(ConfigError::RouteTargetAmbiguous { .. })
        ));
    }

    #[test]
    fn rejects_a_peer_route_with_an_empty_peer_id() {
        let result = resolve_routes(None, vec![peer_route("g.peer-b", "   ")], vec![]);
        assert!(matches!(result, Err(ConfigError::RoutePeerIdEmpty { .. })));
    }

    #[test]
    fn a_peer_route_colliding_with_an_app_route_is_a_duplicate() {
        let result = resolve_routes(
            None,
            vec![
                route("g.example.app", "http://localhost:4000"),
                peer_route("g.example.app", "peer-b"),
            ],
            vec![],
        );
        assert!(matches!(result, Err(ConfigError::DuplicatePrefix { .. })));
    }

    #[test]
    fn rejects_an_invalid_prefix() {
        let result = resolve_routes(None, vec![route("g..app", "http://localhost:4000")], vec![]);
        assert!(matches!(result, Err(ConfigError::InvalidAddress { .. })));
    }

    #[test]
    fn rejects_a_non_http_handler_url() {
        let result = resolve_routes(None, vec![route("g.example.app", "not a url")], vec![]);
        assert!(matches!(result, Err(ConfigError::InvalidHandlerUrl { .. })));
    }

    #[test]
    fn rejects_a_non_http_scheme() {
        let result = resolve_routes(
            None,
            vec![route("g.example.app", "ftp://localhost:4000")],
            vec![],
        );
        assert!(matches!(
            result,
            Err(ConfigError::UnsupportedUrlScheme { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_prefixes() {
        let result = resolve_routes(
            None,
            vec![
                route("g.example.app", "http://localhost:4000"),
                route("g.example.app", "http://localhost:5000"),
            ],
            vec![],
        );
        assert!(matches!(result, Err(ConfigError::DuplicatePrefix { .. })));
    }

    #[test]
    fn expands_children_under_the_apex() {
        let (routes, peer_routes) = resolve_routes(
            Some("g.example.connector"),
            vec![],
            vec![child("billing", "http://localhost:4001")],
        )
        .expect("resolve");

        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].prefix(), "g.example.connector.billing");
        assert!(peer_routes.is_empty());
    }

    #[test]
    fn children_without_an_apex_is_a_specific_error() {
        let result = resolve_routes(
            None,
            vec![],
            vec![child("billing", "http://localhost:4001")],
        );
        assert!(matches!(result, Err(ConfigError::MissingApex)));
    }

    #[test]
    fn rejects_a_child_name_with_a_dot() {
        let result = resolve_routes(
            Some("g.example.connector"),
            vec![],
            vec![child("billing.sub", "http://localhost:4001")],
        );
        assert!(matches!(result, Err(ConfigError::InvalidChildName { .. })));
    }

    #[test]
    fn a_child_colliding_with_an_explicit_route_is_a_duplicate() {
        let result = resolve_routes(
            Some("g.example.connector"),
            vec![route(
                "g.example.connector.billing",
                "http://localhost:9000",
            )],
            vec![child("billing", "http://localhost:4001")],
        );
        assert!(matches!(result, Err(ConfigError::DuplicatePrefix { .. })));
    }

    /// Issue #701: an operator who writes nothing gets exactly the
    /// pre-#701 behavior -- both transports accepted -- so no deployed
    /// route changes until an operator opts in.
    #[test]
    fn a_route_with_no_transport_field_defaults_to_both() {
        let (routes, _) = resolve_routes(
            None,
            vec![priced_route("g.example.app", "http://localhost:4000", 10)],
            vec![],
        )
        .expect("resolve");
        assert_eq!(routes[0].transport_policy(), TransportPolicy::Both);
    }

    #[test]
    fn a_route_can_be_restricted_to_btp_only() {
        let (routes, _) = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.example.relay".to_string(),
                handler_url: Some("http://localhost:4000".to_string()),
                peer_id: None,
                fee: None,
                price: Some(Price::flat(1000)),
                transport: Some("btp".to_string()),
                request: None,
                max_bytes: None,
            }],
            vec![],
        )
        .expect("resolve");
        assert_eq!(routes[0].transport_policy(), TransportPolicy::Btp);
    }

    #[test]
    fn a_route_can_be_restricted_to_http_only() {
        let (routes, _) = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.example.app".to_string(),
                handler_url: Some("http://localhost:4000".to_string()),
                peer_id: None,
                fee: None,
                price: Some(Price::flat(10)),
                transport: Some("http".to_string()),
                request: None,
                max_bytes: None,
            }],
            vec![],
        )
        .expect("resolve");
        assert_eq!(routes[0].transport_policy(), TransportPolicy::Http);
    }

    #[test]
    fn a_route_can_write_both_explicitly() {
        let (routes, _) = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.example.store".to_string(),
                handler_url: Some("http://localhost:4000".to_string()),
                peer_id: None,
                fee: None,
                price: Some(Price::flat(10)),
                transport: Some("both".to_string()),
                request: None,
                max_bytes: None,
            }],
            vec![],
        )
        .expect("resolve");
        assert_eq!(routes[0].transport_policy(), TransportPolicy::Both);
    }

    /// A mistyped value (e.g. `transport = "btponly"`) is a refuse-to-start
    /// error, not a route silently resolved to the default -- the same
    /// principle `deny_unknown_fields` applies to keys, applied to this
    /// field's value.
    #[test]
    fn an_unrecognized_transport_value_is_rejected_at_load() {
        let result = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.example.app".to_string(),
                handler_url: Some("http://localhost:4000".to_string()),
                peer_id: None,
                fee: None,
                price: Some(Price::flat(10)),
                transport: Some("carrier-pigeon".to_string()),
                request: None,
                max_bytes: None,
            }],
            vec![],
        );
        assert!(matches!(
            result,
            Err(ConfigError::InvalidTransportPolicy { value, .. }) if value == "carrier-pigeon"
        ));
    }

    /// A peer route's `transport` is refused rather than silently ignored
    /// (issue #556's principle, applied by issue #701). ADR 0028 changed
    /// why, not whether: such a route *is* reached over a client transport
    /// now, so the field is no longer meaningless -- it is simply not
    /// applied to a forwarded route, which accepts both.
    #[test]
    fn a_peer_route_that_sets_a_transport_is_rejected_rather_than_ignored() {
        let result = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.example.store".to_string(),
                handler_url: None,
                peer_id: Some("store".to_string()),
                fee: None,
                price: None,
                transport: Some("btp".to_string()),
                request: None,
                max_bytes: None,
            }],
            vec![],
        );
        assert!(matches!(
            result,
            Err(ConfigError::PeerRouteHasTransport { value, .. }) if value == "btp"
        ));
    }

    /// A child (issue #701) can restrict its transport exactly like an
    /// explicit `[[routes]]` entry -- it always terminates locally, so the
    /// field is meaningful there too.
    #[test]
    fn a_child_can_be_restricted_to_btp_only() {
        let (routes, _) = resolve_routes(
            Some("g.example.connector"),
            vec![],
            vec![RawChild {
                name: "relay".to_string(),
                handler_url: "http://localhost:4001".to_string(),
                price: Some(Price::flat(1000)),
                transport: Some("btp".to_string()),
            }],
        )
        .expect("resolve");
        assert_eq!(routes[0].transport_policy(), TransportPolicy::Btp);
    }

    #[test]
    fn transport_policy_accepts_predicates_match_their_name() {
        assert!(TransportPolicy::Http.accepts_http());
        assert!(!TransportPolicy::Http.accepts_btp());
        assert!(TransportPolicy::Btp.accepts_btp());
        assert!(!TransportPolicy::Btp.accepts_http());
        assert!(TransportPolicy::Both.accepts_http());
        assert!(TransportPolicy::Both.accepts_btp());
    }

    #[test]
    fn static_route_new_and_new_priced_default_to_both_transports() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        assert_eq!(route.transport_policy(), TransportPolicy::Both);
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 10).unwrap();
        assert_eq!(route.transport_policy(), TransportPolicy::Both);
    }

    #[test]
    fn static_route_new_priced_with_transport_carries_the_given_policy() {
        let route = StaticRoute::new_priced_with_transport(
            "g.example.relay",
            "http://localhost:4000",
            1000,
            TransportPolicy::Btp,
        )
        .unwrap();
        assert_eq!(route.transport_policy(), TransportPolicy::Btp);
    }

    // --- issue #1210: a route declares its request shape --------------

    fn table(pairs: &[(&str, toml::Value)]) -> toml::Table {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect()
    }

    /// A route's `request` table survives resolution verbatim, converted to
    /// the JSON value the self-description and the x402 greeting publish --
    /// arrays, nested tables, integers and strings all round-trip.
    #[test]
    fn a_routes_request_table_resolves_to_the_equivalent_json() {
        let request = table(&[
            ("protocol", toml::Value::String("nip90".to_string())),
            (
                "kinds",
                toml::Value::Array(vec![toml::Value::Integer(5096), toml::Value::Integer(5098)]),
            ),
            (
                "params",
                toml::Value::Table(table(&[(
                    "chain",
                    toml::Value::Array(vec![toml::Value::String("evm:84532".to_string())]),
                )])),
            ),
        ]);
        let (routes, _) = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.toon.gas".to_string(),
                handler_url: Some("http://localhost:4000".to_string()),
                peer_id: None,
                fee: None,
                price: Some(Price::flat(1000)),
                transport: None,
                request: Some(request),
                max_bytes: None,
            }],
            vec![],
        )
        .expect("resolve");

        assert_eq!(
            routes[0].request(),
            Some(&serde_json::json!({
                "protocol": "nip90",
                "kinds": [5096, 5098],
                "params": { "chain": ["evm:84532"] },
            }))
        );
    }

    /// A route that configures none is unaffected -- `None`, not an empty
    /// table, which is every route before this issue.
    #[test]
    fn a_route_with_no_request_table_has_none() {
        let (routes, _) = resolve_routes(
            None,
            vec![priced_route("g.example.app", "http://localhost:4000", 10)],
            vec![],
        )
        .expect("resolve");
        assert_eq!(routes[0].request(), None);
    }

    /// A forwarded route can carry the same declaration, for the same
    /// opaque reason a terminated route can: the connector never reads
    /// either one's keys.
    #[test]
    fn a_forwarded_routes_request_table_resolves_too() {
        let (_, peer_routes) = resolve_routes(
            None,
            vec![RawRoute {
                prefix: "g.peer-b".to_string(),
                handler_url: None,
                peer_id: Some("peer-b".to_string()),
                fee: None,
                price: Some(Price::flat(100)),
                transport: None,
                request: Some(table(&[(
                    "protocol",
                    toml::Value::String("nip90".to_string()),
                )])),
                max_bytes: None,
            }],
            vec![],
        )
        .expect("resolve");

        assert_eq!(
            peer_routes[0].request(),
            Some(&serde_json::json!({ "protocol": "nip90" }))
        );
    }
}
