//! A routing-table entry whose next hop is another connector rather than
//! this one's own app. Paired with `connector_config::StaticRoute` at the
//! [`crate::Connector`] level -- `connector_domain::select_route` picks the
//! most specific prefix across both kinds without caring which one it is.

use chrono::{DateTime, Utc};
use connector_domain::Price;

/// A route whose traffic this connector forwards to a peer's connector for
/// the next hop, rather than terminating it at an app of its own.
///
/// Carries **one** figure: `price`, what this connector's own client edge
/// charges a *client* for a packet to this prefix (ADR 0028), out of which
/// the rest of the path is paid. A schedule since ADR 0065, evaluated at the
/// packet's own payload length exactly as a termination's is -- the edge
/// measures the sealed wrap it was handed, which needs no more of the packet
/// than forwarding it does. What this hop retains of it is the
/// peering's flat per-packet **fee** (ADR 0010), which is not here and
/// never was a property of the prefix -- this hop does the same work
/// whichever prefix the packet was addressed to, so the fee attaches to the
/// peering `peer_id` names (ADR 0061) and the packet path reads it through
/// `Connector::fee_for`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRoute {
    prefix: String,
    peer_id: String,
    price: Price,
    request: Option<serde_json::Value>,
}

impl PeerRoute {
    /// A forwarded route that this connector's client edge serves for free
    /// -- `price` zero. The shape a leased route (which has no price field
    /// at all) always has, and the shorthand a test that is about carriage
    /// rather than charging wants.
    pub fn new(prefix: impl Into<String>, peer_id: impl Into<String>) -> PeerRoute {
        PeerRoute::new_scheduled(prefix, peer_id, Price::FREE)
    }

    /// A forwarded route with an explicit client-edge price (ADR 0028) --
    /// what `connector-cli` builds from a `[[routes]]` entry's own `price`,
    /// which config load requires to be written down.
    pub fn new_priced(
        prefix: impl Into<String>,
        peer_id: impl Into<String>,
        price: u64,
    ) -> PeerRoute {
        PeerRoute::new_scheduled(prefix, peer_id, Price::flat(price))
    }

    /// A forwarded route charging a whole schedule (ADR 0065) -- what
    /// `connector-cli` builds from a `[[routes]]` entry's own `price`, of
    /// which [`PeerRoute::new_priced`] above is the flat case.
    pub fn new_scheduled(
        prefix: impl Into<String>,
        peer_id: impl Into<String>,
        price: Price,
    ) -> PeerRoute {
        PeerRoute {
            prefix: prefix.into(),
            peer_id: peer_id.into(),
            price,
            request: None,
        }
    }

    /// Attach what a client should send to use this route (issue #1210) --
    /// `connector-cli` calls this with the config-file row's own
    /// `connector_config::PeerRouteConfig::request` when it builds this
    /// route from `[[routes]]`, and the runtime peer/route table with the
    /// `request` a `POST /routes/peers` carried (issue #1479). A route built
    /// any other way -- a lease, or a test that does not care -- keeps
    /// `None`, the value every constructor above already gives it.
    pub fn with_request(mut self, request: Option<serde_json::Value>) -> PeerRoute {
        self.request = request;
        self
    }

    /// The destination prefix this route forwards.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The peer this route's traffic is forwarded to, by id -- resolved to
    /// an actual peer connection through the [`crate::PeerTransport`] port.
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// The price schedule this connector's client edge charges a client for
    /// a packet to this prefix (ADR 0028), greeted and gated on exactly the
    /// path a terminated route's price is, and evaluated at that packet's own
    /// payload length the same way (ADR 0065). Free on a leased route, which
    /// has no price field to carry.
    pub fn price(&self) -> Price {
        self.price
    }

    /// What a client should send to use this route (issue #1210). `None`
    /// on a route built with no [`PeerRoute::with_request`] call, or with
    /// `None` -- every leased route, and any peer route declared without one.
    pub fn request(&self) -> Option<&serde_json::Value> {
        self.request.as_ref()
    }
}

/// A [`PeerRoute`] pushed onto this connector's routing table by a
/// controller outside it, with a time limit (ADR 0006, issue #427): it
/// lapses unless renewed before `expires_at`, preserving the
/// withdrawal-safety property route learning used to provide -- a route to
/// a peer that can no longer deliver stops being used on its own, rather
/// than rotting until a human notices. Unlike a [`PeerRoute`] from
/// configuration, this exists only in memory and does not survive a
/// restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeasedRoute {
    route: PeerRoute,
    expires_at: DateTime<Utc>,
}

impl LeasedRoute {
    pub fn new(
        prefix: impl Into<String>,
        peer_id: impl Into<String>,
        expires_at: DateTime<Utc>,
    ) -> LeasedRoute {
        LeasedRoute {
            route: PeerRoute::new(prefix, peer_id),
            expires_at,
        }
    }

    pub fn prefix(&self) -> &str {
        self.route.prefix()
    }

    pub fn peer_id(&self) -> &str {
        self.route.peer_id()
    }

    /// The instant, as this connector's injected clock reports it, past
    /// which this lease has lapsed.
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// This lease's route, borrowed as a plain [`PeerRoute`] (issue #452):
    /// once a lease has been selected for a packet it forwards exactly like
    /// a peer route from configuration, and this hands that shared
    /// forwarding behaviour a reference into the snapshot already being
    /// held rather than a fresh clone.
    pub fn as_peer_route(&self) -> &PeerRoute {
        &self.route
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn exposes_prefix_peer_id_and_price() {
        let route = PeerRoute::new_priced("g.example.remote", "peer-b", 25);
        assert_eq!(route.prefix(), "g.example.remote");
        assert_eq!(route.peer_id(), "peer-b");
        assert_eq!(route.price(), Price::flat(25));
    }

    #[test]
    fn leased_route_exposes_prefix_peer_id_and_expiry() {
        let expires_at = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let route = LeasedRoute::new("g.example.remote", "peer-b", expires_at);
        assert_eq!(route.prefix(), "g.example.remote");
        assert_eq!(route.peer_id(), "peer-b");
        assert_eq!(route.expires_at(), expires_at);
    }
}
