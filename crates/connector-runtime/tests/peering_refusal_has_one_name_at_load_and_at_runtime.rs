//! Issues #1691 and #1692: one domain refusal, two named errors. `Config::load`
//! and a runtime write both ask `connector_domain::peering_admission`, and each
//! maps what it gets back -- so one rule has one name wherever it fires.
//!
//! Every rule is built here through `admit` and mapped by both sides. The
//! mappings themselves are `match`es with no wildcard arm, so a new
//! `PeeringRefusal` fails to compile until both name it; this test then pins
//! what each one names.

use connector_config::ConfigError;
use connector_domain::peering_admission::{
    admit, ChannelFact, PeerRouteFacts, PeeringFacts, PeeringRefusal, PeeringSet, Source,
};
use connector_runtime::PeerRouteTableError;

fn base() -> PeeringSet {
    PeeringSet {
        peerings: vec![PeeringFacts::new("hop")],
        inbound: vec![ChannelFact {
            peer_id: "hop".into(),
            channel: Some("in".into()),
            chain: Some("evm"),
            network: Some("eip155:31337".into()),
        }],
        pay: vec![ChannelFact {
            peer_id: "hop".into(),
            channel: Some("out".into()),
            chain: Some("evm"),
            network: Some("eip155:31337".into()),
        }],
        routes: vec![PeerRouteFacts::new("g.example", "hop")],
        settled_chains: ["evm"].into(),
        ..PeeringSet::default()
    }
}

type Case = (
    PeeringRefusal,
    fn(ConfigError) -> bool,
    fn(PeerRouteTableError) -> bool,
);

fn refusal(mutate: impl FnOnce(&mut PeeringSet)) -> PeeringRefusal {
    let mut set = base();
    assert_eq!(admit(&set), Ok(()));
    mutate(&mut set);
    admit(&set).expect_err("the mutation breaks a rule")
}

#[test]
fn the_valid_set_is_admitted() {
    assert_eq!(admit(&base()), Ok(()));
}

#[test]
fn every_rule_has_a_config_name_and_a_runtime_name() {
    let pay_only = |set: &mut PeeringSet| set.pay[0].peer_id = "ghost".into();
    let cases: Vec<Case> = vec![
        (
            refusal(|s| {
                s.peerings[0].source = Source::Runtime;
                s.config_peer_ids.insert("hop".into());
            }),
            |e| matches!(e, ConfigError::PeeringAdmission(connector_config::PeeringAdmissionRefusal::PeeringOwnedByConfig { value }) if value == "hop"),
            |e| matches!(e, PeerRouteTableError::OwnedByConfig(v) if v == "hop"),
        ),
        (
            refusal(|s| {
                s.routes[0].source = Source::Runtime;
                s.config_prefixes.insert("g.example".into());
            }),
            |e| matches!(e, ConfigError::PeeringAdmission(connector_config::PeeringAdmissionRefusal::PeeringOwnedByConfig { value }) if value == "g.example"),
            |e| matches!(e, PeerRouteTableError::OwnedByConfig(v) if v == "g.example"),
        ),
        (
            refusal(|s| s.routes[0].peer_id = "ghost".into()),
            |e| matches!(e, ConfigError::PeeringAdmission(connector_config::PeeringAdmissionRefusal::UnknownPeerId { peer_id, .. }) if peer_id == "ghost"),
            |e| matches!(e, PeerRouteTableError::UnknownPeerId { peer_id, .. } if peer_id == "ghost"),
        ),
        (
            refusal(|s| s.inbound[0].peer_id = "ghost".into()),
            |e| matches!(e, ConfigError::PeeringAdmission(connector_config::PeeringAdmissionRefusal::PeerChannelOrphaned { peer_id }) if peer_id == "ghost"),
            |e| {
                matches!(
                    e,
                    PeerRouteTableError::Inadmissible(PeeringRefusal::PeerChannelOrphaned { .. })
                )
            },
        ),
        (
            refusal(|s| s.inbound.clear()),
            |e| matches!(e, ConfigError::PeeringAdmission(connector_config::PeeringAdmissionRefusal::PeerChannelUnbound { id }) if id == "hop"),
            |e| matches!(e, PeerRouteTableError::PeerChannelUnbound(id) if id == "hop"),
        ),
        (
            refusal(pay_only),
            |e| matches!(e, ConfigError::PeeringAdmission(connector_config::PeeringAdmissionRefusal::PayChannelOrphaned { peer_id }) if peer_id == "ghost"),
            |e| {
                matches!(
                    e,
                    PeerRouteTableError::Inadmissible(PeeringRefusal::PayChannelOrphaned { .. })
                )
            },
        ),
        (
            refusal(|s| s.settled_chains.clear()),
            |e| {
                matches!(
                    e,
                    ConfigError::PeeringAdmission(
                        connector_config::PeeringAdmissionRefusal::PayChannelWithoutX402 {
                            chain: "evm",
                            ..
                        }
                    )
                )
            },
            |e| {
                matches!(
                    e,
                    PeerRouteTableError::Inadmissible(
                        PeeringRefusal::PayChannelOnUnsettledChain { .. }
                    )
                )
            },
        ),
        (
            refusal(|s| s.pay[0].channel = s.inbound[0].channel.clone()),
            |e| matches!(e, ConfigError::PeeringAdmission(connector_config::PeeringAdmissionRefusal::ChannelInBothDirections { value }) if value == "in"),
            |e| {
                matches!(
                    e,
                    PeerRouteTableError::Inadmissible(
                        PeeringRefusal::ChannelInBothDirections { .. }
                    )
                )
            },
        ),
        (
            refusal(|s| s.pay[0].network = Some("eip155:1".into())),
            |e| {
                matches!(e, ConfigError::PeeringAdmission(connector_config::PeeringAdmissionRefusal::PeeringNetworksDiffer { inbound, outbound, .. })
                if inbound == "eip155:31337" && outbound == "eip155:1")
            },
            |e| {
                matches!(
                    e,
                    PeerRouteTableError::Inadmissible(PeeringRefusal::PeeringNetworksDiffer { .. })
                )
            },
        ),
        (
            refusal(|s| s.pay.clear()),
            |e| {
                matches!(e, ConfigError::PeeringAdmission(connector_config::PeeringAdmissionRefusal::PayChannelUnbound { prefix, peer_id })
                if prefix == "g.example" && peer_id == "hop")
            },
            |e| {
                matches!(e, PeerRouteTableError::PeerHasNoPayChannel { prefix, peer_id }
                if prefix == "g.example" && peer_id == "hop")
            },
        ),
    ];
    for (refusal, config, runtime) in cases {
        assert!(
            config(ConfigError::from(refusal.clone())),
            "{refusal:?} at load"
        );
        assert!(
            runtime(PeerRouteTableError::from(refusal.clone())),
            "{refusal:?} at runtime"
        );
    }
}
