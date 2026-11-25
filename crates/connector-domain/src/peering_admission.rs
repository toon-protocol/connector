//! Peering admission: the cross-table rules a peering must satisfy, written
//! once (ADR 0058, issue #1691).
//!
//! Config load and every runtime write used to carry their own copy of each
//! rule, kept in step only by comments -- and the runtime copy of the
//! pay-channel rule was once never wired (#1217). Both now build a
//! [`PeeringSet`] and ask [`admit`]; each maps the [`PeeringRefusal`] it gets
//! back to its own named error (ADR 0009 at load, `PeerRouteTableError` at
//! runtime).
//!
//! Pure: no I/O, no clock, no keys. A later rule adds a [`PeeringRefusal`]
//! variant and a case in [`admit`]; it does not add a caller.

use std::collections::BTreeSet;

/// One channel a peering holds, named by the network it lives on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelFact {
    pub network: String,
}

/// A peering and the channels this node holds with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeeringFacts {
    pub id: String,
    /// Channels the peer pays this node on.
    pub inbound: Vec<ChannelFact>,
    /// Channels this node pays the peer on.
    pub pay: Vec<ChannelFact>,
}

/// A route that forwards to a peering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRouteFacts {
    pub prefix: String,
    pub peer_id: String,
}

/// Everything [`admit`] judges.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PeeringSet {
    pub peerings: Vec<PeeringFacts>,
    pub routes: Vec<PeerRouteFacts>,
    /// Networks this node has a settlement table for.
    pub settlement_networks: BTreeSet<String>,
}

/// The first rule a [`PeeringSet`] breaks.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeeringRefusal {
    /// A route forwards to a peering this node has no channel to pay from
    /// (ADR 0042: a connector covers every PREPARE it sends).
    #[error("route '{prefix}' forwards to peering '{peer_id}', which has no channel to pay from")]
    RouteWithoutPayChannel { prefix: String, peer_id: String },
}

/// Admit the set, or name the first rule it breaks. Routes are judged in the
/// order given, so the same input always yields the same refusal.
///
/// A route naming a peering absent from the set is not this rule's concern
/// (unknown-peer is its own rule); the pay-channel rule only fires for a
/// peering it can see.
pub fn admit(set: &PeeringSet) -> Result<(), PeeringRefusal> {
    for route in &set.routes {
        let Some(peering) = set.peerings.iter().find(|p| p.id == route.peer_id) else {
            continue;
        };
        if peering.pay.is_empty() {
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

    fn channel() -> ChannelFact {
        ChannelFact {
            network: "eip155:31337".to_string(),
        }
    }

    fn peering(id: &str, inbound: usize, pay: usize) -> PeeringFacts {
        PeeringFacts {
            id: id.to_string(),
            inbound: vec![channel(); inbound],
            pay: vec![channel(); pay],
        }
    }

    fn set(peerings: Vec<PeeringFacts>, routed: &[&str]) -> PeeringSet {
        PeeringSet {
            peerings,
            routes: routed
                .iter()
                .enumerate()
                .map(|(i, peer)| PeerRouteFacts {
                    prefix: format!("g.r{i}"),
                    peer_id: (*peer).to_string(),
                })
                .collect(),
            settlement_networks: ["eip155:31337".to_string()].into(),
        }
    }

    /// A generated peering set: per peering its inbound and pay channel
    /// counts, and routes naming those peerings by index, in any order.
    fn peering_set() -> impl Strategy<Value = PeeringSet> {
        proptest::collection::vec((0usize..3, 0usize..3), 1..6).prop_flat_map(|counts| {
            let n = counts.len();
            (Just(counts), proptest::collection::vec(0..n, 0..8)).prop_map(|(counts, routed)| {
                let peerings = counts
                    .iter()
                    .enumerate()
                    .map(|(i, (inbound, pay))| peering(&format!("p{i}"), *inbound, *pay))
                    .collect();
                let ids: Vec<String> = routed.iter().map(|i| format!("p{i}")).collect();
                let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
                set(peerings, &ids)
            })
        })
    }

    fn pay_count(set: &PeeringSet, peer_id: &str) -> usize {
        set.peerings
            .iter()
            .find(|p| p.id == peer_id)
            .map_or(0, |p| p.pay.len())
    }

    proptest! {
        #[test]
        fn a_route_to_a_peering_without_a_pay_channel_is_always_refused(
            set in peering_set(),
        ) {
            match set.routes.iter().find(|r| pay_count(&set, &r.peer_id) == 0) {
                // The first such route, in the order given, is the one named.
                Some(route) => prop_assert_eq!(
                    admit(&set),
                    Err(PeeringRefusal::RouteWithoutPayChannel {
                        prefix: route.prefix.clone(),
                        peer_id: route.peer_id.clone(),
                    })
                ),
                None => prop_assert_eq!(admit(&set), Ok(())),
            }
        }

        #[test]
        fn a_route_to_a_peering_with_a_pay_channel_is_never_refused_for_it(
            set in peering_set(),
        ) {
            if let Err(PeeringRefusal::RouteWithoutPayChannel { peer_id, .. }) = admit(&set) {
                prop_assert_eq!(pay_count(&set, &peer_id), 0);
            }
        }

        #[test]
        fn an_unrouted_peering_without_a_pay_channel_owes_nothing(
            set in peering_set(),
            inbound in 0usize..3,
        ) {
            let mut extended = set.clone();
            extended.peerings.push(peering("accept-only", inbound, 0));
            prop_assert_eq!(admit(&set), admit(&extended));
        }
    }
}
