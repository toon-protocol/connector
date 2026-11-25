//! Issue #1691: one domain refusal, two named errors. `Config::load` and a
//! runtime route write both ask `connector_domain::peering_admission`, and
//! each maps what it gets back -- so one rule has one name wherever it fires.

use connector_config::ConfigError;
use connector_domain::peering_admission::{
    admit, ChannelFact, PeerRouteFacts, PeeringFacts, PeeringRefusal, PeeringSet,
};
use connector_runtime::PeerRouteTableError;

#[test]
fn the_pay_channel_refusal_maps_to_a_named_config_refusal_and_a_runtime_error() {
    let set = PeeringSet {
        peerings: vec![PeeringFacts {
            id: "hop".to_string(),
            inbound: vec![ChannelFact {
                network: "eip155:31337".to_string(),
            }],
            pay: vec![],
        }],
        routes: vec![PeerRouteFacts {
            prefix: "g.example".to_string(),
            peer_id: "hop".to_string(),
        }],
        settlement_networks: ["eip155:31337".to_string()].into(),
    };
    let refusal = admit(&set).expect_err("a route to a peering with no pay channel");
    assert!(matches!(
        refusal,
        PeeringRefusal::RouteWithoutPayChannel { .. }
    ));

    assert!(matches!(
        ConfigError::from(refusal.clone()),
        ConfigError::PayChannelUnbound { ref prefix, ref peer_id }
            if prefix == "g.example" && peer_id == "hop"
    ));
    assert!(matches!(
        PeerRouteTableError::from(refusal),
        PeerRouteTableError::PeerHasNoPayChannel { ref prefix, ref peer_id }
            if prefix == "g.example" && peer_id == "hop"
    ));
}
