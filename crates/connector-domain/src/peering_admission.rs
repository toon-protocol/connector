//! Peering admission: the cross-table rules a peering must satisfy, written
//! once (ADR 0058, issues #1691 and #1692).
//!
//! Config load and every runtime write used to carry their own copy of each
//! rule, kept in step only by comments -- and the runtime copy of the
//! pay-channel rule was once never wired (#1217). Both now build a
//! [`PeeringSet`] and ask [`admit`]; each maps the [`PeeringRefusal`] it gets
//! back to its own named error (ADR 0009 at load, `PeerRouteTableError` at
//! runtime). The two mappings are `match`es with no wildcard arm, so a new
//! rule does not compile until both name it.
//!
//! Pure: no I/O, no clock, no keys. A rule that has no runtime meaning (an
//! orphaned row cannot arise from `POST /peers`) is still this module's, and
//! only config load reaches it.

use std::collections::BTreeSet;

/// Where a peering or route was written: the config file, or the runtime
/// table behind the operator surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    #[default]
    Config,
    Runtime,
}

/// One channel row a peering holds: a `[[peer_channels]]` row (this node is
/// paid on it) or a `[[pay_channels]]` row (this node pays on it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelFact {
    /// The peering the row names; may name none (an orphaned row).
    pub peer_id: String,
    /// The channel's canonical id, where the row names one.
    pub channel: Option<String>,
    /// The settlement chain the row is on (`"evm"`, `"solana"`, `"xrpl"`),
    /// where known.
    pub chain: Option<&'static str>,
    /// The CAIP-2 network the row is on, where known.
    pub network: Option<String>,
}

impl ChannelFact {
    #[must_use]
    pub fn new(peer_id: impl Into<String>) -> Self {
        ChannelFact {
            peer_id: peer_id.into(),
            channel: None,
            chain: None,
            network: None,
        }
    }

    /// The CAIP-2 namespace (`eip155`, `solana`, `xrpl`) of the row's network.
    fn namespace(&self) -> Option<&str> {
        self.network
            .as_deref()
            .map(|network| network.split(':').next().unwrap_or(network))
    }
}

/// A peering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeeringFacts {
    pub id: String,
    pub source: Source,
}

impl PeeringFacts {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        PeeringFacts {
            id: id.into(),
            source: Source::Config,
        }
    }
}

/// A route that forwards to a peering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRouteFacts {
    pub prefix: String,
    pub peer_id: String,
    pub source: Source,
}

impl PeerRouteFacts {
    #[must_use]
    pub fn new(prefix: impl Into<String>, peer_id: impl Into<String>) -> Self {
        PeerRouteFacts {
            prefix: prefix.into(),
            peer_id: peer_id.into(),
            source: Source::Config,
        }
    }
}

/// Everything [`admit`] judges.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PeeringSet {
    pub peerings: Vec<PeeringFacts>,
    /// Channels peers pay this node on (`[[peer_channels]]`).
    pub inbound: Vec<ChannelFact>,
    /// Channels this node pays peers on (`[[pay_channels]]`).
    pub pay: Vec<ChannelFact>,
    pub routes: Vec<PeerRouteFacts>,
    /// Chains this node has a settlement table for.
    pub settled_chains: BTreeSet<&'static str>,
    /// Peer ids and route prefixes the config file owns, which a runtime
    /// write may never take (ADR 0034).
    pub config_peer_ids: BTreeSet<String>,
    pub config_prefixes: BTreeSet<String>,
}

/// The first rule a [`PeeringSet`] breaks.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeeringRefusal {
    /// A runtime peering takes an id the config file owns (ADR 0034).
    #[error("'{0}' is defined in this node's config file and cannot be changed at runtime")]
    PeerIdOwnedByConfig(String),
    /// A runtime route takes a prefix the config file owns (ADR 0034).
    #[error("'{0}' is defined in this node's config file and cannot be changed at runtime")]
    PrefixOwnedByConfig(String),
    /// A route forwards to a peering that exists in neither table.
    #[error("route '{prefix}' names unknown peer '{peer_id}'")]
    UnknownPeer { prefix: String, peer_id: String },
    /// A channel binding names no peering.
    #[error("peer channel row names peer '{peer_id}', which is no peering")]
    PeerChannelOrphaned { peer_id: String },
    /// A peering with no channel bound to it: it could never take the peer
    /// role (ADR 0058, ADR 0060).
    #[error("peering '{0}' has no payment channel bound to it")]
    PeeringWithoutChannel(String),
    /// A pay-channel row names no peering.
    #[error("pay channel row names peer '{peer_id}', which is no peering")]
    PayChannelOrphaned { peer_id: String },
    /// A pay channel on a chain this node has no settlement table for.
    #[error("pay channel for peer '{peer_id}' is on {chain}, which this node has no settlement table for")]
    PayChannelOnUnsettledChain {
        peer_id: String,
        chain: &'static str,
    },
    /// One channel this node pays on and is also paid on (ADR 0075).
    #[error("channel '{channel}' is both an inbound and an outbound channel")]
    ChannelInBothDirections { channel: String },
    /// A peering's two channels on one chain name different networks
    /// (ADR 0086 decision 11).
    #[error("peering '{peer_id}' has its channels on {inbound} and {outbound}")]
    PeeringNetworksDiffer {
        peer_id: String,
        inbound: String,
        outbound: String,
    },
    /// A route forwards to a peering this node has no channel to pay from
    /// (ADR 0042: a connector covers every PREPARE it sends).
    #[error("route '{prefix}' forwards to peering '{peer_id}', which has no channel to pay from")]
    RouteWithoutPayChannel { prefix: String, peer_id: String },
}

/// The pay-channel-on-an-unsettled-chain rule alone, for a caller (a channel
/// row's own resolver) that meets the row before it has a set.
pub fn check_pay_chain_settled(
    peer_id: &str,
    chain: &'static str,
    settled_chains: &BTreeSet<&'static str>,
) -> Result<(), PeeringRefusal> {
    if settled_chains.contains(chain) {
        Ok(())
    } else {
        Err(PeeringRefusal::PayChannelOnUnsettledChain {
            peer_id: peer_id.to_string(),
            chain,
        })
    }
}

/// The ADR 0034 rule alone: a runtime write never takes a peer id or prefix
/// the config file owns. It needs only the peerings and routes, so a caller
/// can refuse a write before it has the rest of the set (before dialling the
/// peer, say); [`admit`] runs it first.
pub fn check_config_ownership(set: &PeeringSet) -> Result<(), PeeringRefusal> {
    for peering in &set.peerings {
        if peering.source == Source::Runtime && set.config_peer_ids.contains(&peering.id) {
            return Err(PeeringRefusal::PeerIdOwnedByConfig(peering.id.clone()));
        }
    }
    for route in &set.routes {
        if route.source == Source::Runtime && set.config_prefixes.contains(&route.prefix) {
            return Err(PeeringRefusal::PrefixOwnedByConfig(route.prefix.clone()));
        }
    }
    Ok(())
}

/// The rules that need only the peerings, their routes and the rows peers
/// pay this node on: unknown peer, orphaned inbound rows and unbound
/// peerings. A caller that has those tables before its pay rows (config load,
/// which refuses them where it always has) can run them alone; [`admit`] runs
/// them after [`check_config_ownership`].
pub fn check_bindings(set: &PeeringSet) -> Result<(), PeeringRefusal> {
    let has_peering = |id: &str| set.peerings.iter().any(|p| p.id == id);

    for route in &set.routes {
        if !has_peering(&route.peer_id) {
            return Err(PeeringRefusal::UnknownPeer {
                prefix: route.prefix.clone(),
                peer_id: route.peer_id.clone(),
            });
        }
    }
    // Orphaned rows first, then unbound peerings: a mistyped `peer_id`
    // produces both at once, and "this row names a peer that does not exist"
    // is the one that names the typo.
    for row in &set.inbound {
        if !has_peering(&row.peer_id) {
            return Err(PeeringRefusal::PeerChannelOrphaned {
                peer_id: row.peer_id.clone(),
            });
        }
    }
    for peering in &set.peerings {
        if !set.inbound.iter().any(|row| row.peer_id == peering.id) {
            return Err(PeeringRefusal::PeeringWithoutChannel(peering.id.clone()));
        }
    }
    Ok(())
}

/// Admit the set, or name the first rule it breaks. Rules run in a fixed
/// order, and each walks its rows in the order given, so the same input
/// always yields the same refusal. A rule judges only the rows it names, so
/// adding an unrelated valid peering never changes another's verdict.
pub fn admit(set: &PeeringSet) -> Result<(), PeeringRefusal> {
    check_config_ownership(set)?;
    check_bindings(set)?;
    for row in &set.pay {
        if let Some(chain) = row.chain {
            check_pay_chain_settled(&row.peer_id, chain, &set.settled_chains)?;
        }
    }
    // A pay row is judged whole -- its peering, its direction, its network
    // -- before the next row is looked at.
    for row in &set.pay {
        if !set.peerings.iter().any(|p| p.id == row.peer_id) {
            return Err(PeeringRefusal::PayChannelOrphaned {
                peer_id: row.peer_id.clone(),
            });
        }
        if let Some(channel) = &row.channel {
            if set
                .inbound
                .iter()
                .any(|inbound| inbound.channel.as_ref() == Some(channel))
            {
                return Err(PeeringRefusal::ChannelInBothDirections {
                    channel: channel.clone(),
                });
            }
        }
        if let (Some(outbound), Some(namespace)) = (&row.network, row.namespace()) {
            let differing = set
                .inbound
                .iter()
                .filter(|inbound| inbound.peer_id == row.peer_id)
                .filter(|inbound| inbound.namespace() == Some(namespace))
                .find_map(|inbound| inbound.network.as_ref().filter(|n| *n != outbound));
            if let Some(inbound) = differing {
                return Err(PeeringRefusal::PeeringNetworksDiffer {
                    peer_id: row.peer_id.clone(),
                    inbound: inbound.clone(),
                    outbound: outbound.clone(),
                });
            }
        }
    }
    // Keyed on ROUTES, not on peerings: a peering this node only ever
    // receives from owes nothing and needs no pay row. A route naming a
    // peering absent from the set was refused above.
    for route in &set.routes {
        if !set.pay.iter().any(|row| row.peer_id == route.peer_id) {
            return Err(PeeringRefusal::RouteWithoutPayChannel {
                prefix: route.prefix.clone(),
                peer_id: route.peer_id.clone(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const NETWORKS: [&str; 3] = ["eip155:1", "eip155:2", "solana:main"];

    fn row(peer: &str, channel: Option<&str>, network: Option<&str>) -> ChannelFact {
        ChannelFact {
            peer_id: peer.to_string(),
            channel: channel.map(str::to_string),
            chain: network.map(|n| {
                if n.starts_with("solana") {
                    "solana"
                } else {
                    "evm"
                }
            }),
            network: network.map(str::to_string),
        }
    }

    /// A set that satisfies every rule: each peering has an inbound row and
    /// a pay row on distinct channels and one network, and a route.
    fn valid(n: usize) -> PeeringSet {
        let mut set = PeeringSet {
            settled_chains: ["evm", "solana"].into(),
            ..PeeringSet::default()
        };
        for i in 0..n {
            let id = format!("p{i}");
            set.peerings.push(PeeringFacts::new(&id));
            set.inbound
                .push(row(&id, Some(&format!("in{i}")), Some("eip155:1")));
            set.pay
                .push(row(&id, Some(&format!("out{i}")), Some("eip155:1")));
            set.routes.push(PeerRouteFacts::new(format!("g.r{i}"), &id));
        }
        set
    }

    /// An arbitrary, usually invalid, set drawn from small pools so every
    /// rule has a chance to fire and to collide with another.
    fn any_set() -> impl Strategy<Value = PeeringSet> {
        let peer = (0usize..5).prop_map(|i| format!("p{i}"));
        let channel = proptest::option::of((0usize..4).prop_map(|i| format!("c{i}")));
        let network = proptest::option::of(0usize..NETWORKS.len());
        let fact = (peer.clone(), channel, network).prop_map(|(peer, channel, network)| {
            row(&peer, channel.as_deref(), network.map(|i| NETWORKS[i]))
        });
        (
            proptest::collection::vec(0usize..4, 0..4),
            proptest::collection::vec(fact.clone(), 0..5),
            proptest::collection::vec(fact, 0..5),
            proptest::collection::vec((0usize..5, 0usize..5), 0..5),
            proptest::collection::vec(any::<bool>(), 3),
        )
            .prop_map(|(peerings, inbound, pay, routes, settled)| {
                let mut set = PeeringSet {
                    inbound,
                    pay,
                    ..PeeringSet::default()
                };
                for i in peerings {
                    set.peerings.push(PeeringFacts::new(format!("p{i}")));
                }
                for (r, p) in routes {
                    set.routes
                        .push(PeerRouteFacts::new(format!("g.r{r}"), format!("p{p}")));
                }
                for (chain, on) in ["evm", "solana", "xrpl"].into_iter().zip(settled) {
                    if on {
                        set.settled_chains.insert(chain);
                    }
                }
                set
            })
    }

    #[test]
    fn a_valid_set_is_admitted() {
        assert_eq!(admit(&valid(3)), Ok(()));
        assert_eq!(admit(&PeeringSet::default()), Ok(()));
    }

    #[test]
    fn a_runtime_peering_on_a_config_id_is_refused() {
        let mut set = valid(1);
        set.peerings[0].source = Source::Runtime;
        set.config_peer_ids.insert("p0".to_string());
        assert_eq!(
            admit(&set),
            Err(PeeringRefusal::PeerIdOwnedByConfig("p0".into()))
        );
    }

    #[test]
    fn a_runtime_route_on_a_config_prefix_is_refused() {
        let mut set = valid(1);
        set.routes[0].source = Source::Runtime;
        set.config_prefixes.insert("g.r0".to_string());
        assert_eq!(
            admit(&set),
            Err(PeeringRefusal::PrefixOwnedByConfig("g.r0".into()))
        );
    }

    #[test]
    fn a_config_row_never_collides_with_the_config_it_is() {
        let mut set = valid(1);
        set.config_peer_ids.insert("p0".to_string());
        set.config_prefixes.insert("g.r0".to_string());
        assert_eq!(admit(&set), Ok(()));
    }

    #[test]
    fn a_pay_row_is_judged_whole_before_the_next() {
        // Row 0 is in both directions, row 1 is orphaned: row 0 is named,
        // as `Config::load` named it before the rules moved here.
        let mut set = valid(1);
        set.pay[0].channel = set.inbound[0].channel.clone();
        set.pay.push(row("ghost", None, None));
        assert_eq!(
            admit(&set),
            Err(PeeringRefusal::ChannelInBothDirections {
                channel: "in0".into()
            })
        );
    }

    proptest! {
        #[test]
        fn a_route_to_no_peering_is_refused(n in 0usize..4, at in 0usize..5) {
            let mut set = valid(n);
            set.routes.insert(at.min(set.routes.len()), PeerRouteFacts::new("g.ghost", "ghost"));
            prop_assert_eq!(
                admit(&set),
                Err(PeeringRefusal::UnknownPeer { prefix: "g.ghost".into(), peer_id: "ghost".into() })
            );
        }

        #[test]
        fn an_inbound_row_for_no_peering_is_refused(n in 0usize..4) {
            let mut set = valid(n);
            set.inbound.push(row("ghost", None, None));
            prop_assert_eq!(
                admit(&set),
                Err(PeeringRefusal::PeerChannelOrphaned { peer_id: "ghost".into() })
            );
        }

        #[test]
        fn a_peering_with_no_inbound_row_is_refused(n in 1usize..4, drop in 0usize..4) {
            let mut set = valid(n);
            let drop = drop % n;
            let id = format!("p{drop}");
            set.inbound.retain(|r| r.peer_id != id);
            prop_assert_eq!(admit(&set), Err(PeeringRefusal::PeeringWithoutChannel(id)));
        }

        #[test]
        fn a_pay_row_for_no_peering_is_refused(n in 0usize..4) {
            let mut set = valid(n);
            set.pay.push(row("ghost", None, None));
            prop_assert_eq!(
                admit(&set),
                Err(PeeringRefusal::PayChannelOrphaned { peer_id: "ghost".into() })
            );
        }

        #[test]
        fn a_pay_row_on_an_unsettled_chain_is_refused(n in 1usize..4, which in 0usize..4) {
            let mut set = valid(n);
            set.settled_chains.remove("solana");
            let i = which % n;
            set.pay[i] = row(&format!("p{i}"), Some("sol-out"), Some("solana:main"));
            prop_assert_eq!(
                admit(&set),
                Err(PeeringRefusal::PayChannelOnUnsettledChain {
                    peer_id: format!("p{i}"),
                    chain: "solana",
                })
            );
        }

        #[test]
        fn a_channel_paid_on_and_paid_by_is_refused(n in 1usize..4, which in 0usize..4) {
            let mut set = valid(n);
            let i = which % n;
            set.pay[i].channel = set.inbound[i].channel.clone();
            prop_assert_eq!(
                admit(&set),
                Err(PeeringRefusal::ChannelInBothDirections { channel: format!("in{i}") })
            );
        }

        #[test]
        fn two_networks_on_one_chain_for_one_peering_are_refused(n in 1usize..4, which in 0usize..4) {
            let mut set = valid(n);
            let i = which % n;
            set.pay[i].network = Some("eip155:2".to_string());
            prop_assert_eq!(
                admit(&set),
                Err(PeeringRefusal::PeeringNetworksDiffer {
                    peer_id: format!("p{i}"),
                    inbound: "eip155:1".into(),
                    outbound: "eip155:2".into(),
                })
            );
        }

        #[test]
        fn networks_on_two_chains_for_one_peering_are_admitted(n in 1usize..4, which in 0usize..4) {
            let mut set = valid(n);
            let i = which % n;
            set.pay[i].network = Some("solana:main".to_string());
            set.pay[i].chain = Some("solana");
            prop_assert_eq!(admit(&set), Ok(()));
        }

        #[test]
        fn a_route_to_a_peering_with_no_pay_row_is_refused(n in 1usize..4, which in 0usize..4) {
            let mut set = valid(n);
            let i = which % n;
            let id = format!("p{i}");
            set.pay.retain(|r| r.peer_id != id);
            let first = set.routes.iter().find(|r| r.peer_id == id).unwrap().clone();
            prop_assert_eq!(
                admit(&set),
                Err(PeeringRefusal::RouteWithoutPayChannel {
                    prefix: first.prefix,
                    peer_id: id,
                })
            );
        }

        #[test]
        fn an_unrouted_peering_with_no_pay_row_owes_nothing(n in 0usize..4) {
            let mut set = valid(n);
            set.peerings.push(PeeringFacts::new("accept-only"));
            set.inbound.push(row("accept-only", Some("in-only"), None));
            prop_assert_eq!(admit(&set), Ok(()));
        }

        #[test]
        fn the_same_input_always_yields_the_same_first_refusal(set in any_set()) {
            prop_assert_eq!(admit(&set), admit(&set.clone()));
            // Config load runs the binding rules early and `admit` later; on
            // one input the two entry points name the same first refusal.
            if let Err(refusal) = check_config_ownership(&set).and_then(|()| check_bindings(&set)) {
                prop_assert_eq!(admit(&set), Err(refusal));
            }
        }

        #[test]
        fn adding_an_unrelated_valid_peering_never_changes_a_verdict(set in any_set()) {
            // Valid on its own: its pay row is on a chain the set already
            // settles, or on no chain at all when it settles none.
            let (network, chain) = match set.settled_chains.iter().next() {
                Some(&"solana") => (Some("solana:zz"), Some("solana")),
                Some(&"xrpl") => (Some("xrpl:zz"), Some("xrpl")),
                Some(_) => (Some("eip155:9"), Some("evm")),
                None => (None, None),
            };
            let mut extended = set.clone();
            extended.peerings.push(PeeringFacts::new("zz"));
            extended.inbound.push(row("zz", Some("zz-in"), network));
            let mut pay = row("zz", Some("zz-out"), network);
            pay.chain = chain;
            extended.pay.push(pay);
            extended.routes.push(PeerRouteFacts::new("g.zz", "zz"));
            // The new peering sits last in every walk, so a refusal in the
            // set is the one the extension reports as well.
            prop_assert_eq!(admit(&set), admit(&extended));
        }
    }
}
