//! Schema 16, issue #1709 (ND-05b): the carriage a payer must use for a
//! destination, pinned.
//!
//! Every case builds a [`NodeSelfDescription`] from the routes and node-wide
//! field it states and records what
//! [`connector_domain::required_carriage`] answers for its destination.
//! Nothing here is written by hand: a change to the rule moves this group,
//! and `vectors_up_to_date` fails until the file is regenerated.
//!
//! A replaying reader reads the contract off the cases: the longest-prefix
//! route governing `destination` (a prefix governs a destination equal to it
//! or extending it by whole dot-separated labels) answers with its own
//! `requiredTransport`; a governing route that names none, or no governing
//! route, falls to the document's own `requiredTransport`; neither means
//! `null`, no requirement. The answer is `"http"`, `"btp"` or `null` --
//! never `"both"`, which the connector never publishes.

use connector_domain::{required_carriage, NodeFacts, NodeSelfDescription, RoutePrice};
use serde::Serialize;

/// One route of the described document, in the wire's own names.
#[derive(Debug, Serialize)]
pub struct CarriageRoute {
    pub prefix: String,
    /// Absent from the document on an unpinned route.
    #[serde(rename = "requiredTransport", skip_serializing_if = "Option::is_none")]
    pub required_transport: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RequiredCarriageCase {
    pub name: String,
    pub description: String,
    /// The document's routes.
    pub routes: Vec<CarriageRoute>,
    /// The document's node-wide `requiredTransport`, or `null` when absent.
    pub node_required_transport: Option<String>,
    pub destination: String,
    /// `"http"`, `"btp"`, or `null`: no requirement.
    pub required_carriage: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RequiredCarriageVectors {
    pub cases: Vec<RequiredCarriageCase>,
}

fn route(prefix: &str, pin: Option<&str>) -> CarriageRoute {
    CarriageRoute {
        prefix: prefix.to_string(),
        required_transport: pin.map(str::to_string),
    }
}

fn case(
    name: &str,
    description: &str,
    routes: Vec<CarriageRoute>,
    node: Option<&str>,
    destination: &str,
) -> RequiredCarriageCase {
    let document = NodeSelfDescription::describe(
        &NodeFacts::default(),
        None,
        routes
            .iter()
            .map(|r| RoutePrice {
                prefix: r.prefix.clone(),
                price: "1".to_string(),
                price_per_kib: None,
                request: None,
                max_bytes: None,
                payment: None,
                required_transport: r.required_transport.clone(),
            })
            .collect(),
        node.map(str::to_string),
    );
    let answer = required_carriage(&document, destination).map(str::to_string);
    RequiredCarriageCase {
        name: name.to_string(),
        description: description.to_string(),
        routes,
        node_required_transport: node.map(str::to_string),
        destination: destination.to_string(),
        required_carriage: answer,
    }
}

pub fn generate_required_carriage_vectors() -> RequiredCarriageVectors {
    let cases = vec![
        case(
            "pinned_route",
            "A route pinned to HTTP: its own pin is the answer.",
            vec![route("g.toon.app", Some("http"))],
            None,
            "g.toon.app",
        ),
        case(
            "pinned_route_beats_the_node",
            "A route's own pin beats a node-wide value that says otherwise.",
            vec![route("g.toon.app", Some("http"))],
            Some("btp"),
            "g.toon.app",
        ),
        case(
            "unpinned_route_on_a_pinned_node",
            "A route that names no pin falls to the node-wide requirement.",
            vec![route("g.toon.app", None)],
            Some("btp"),
            "g.toon.app",
        ),
        case(
            "routes_that_disagree_pinned",
            "The devnet relay's shape: g.toon.relay is pinned to BTP and the node-wide \
             field is silent because the routes disagree.",
            vec![
                route("g.toon.relay", Some("btp")),
                route("g.toon.relay.ephemeral", None),
            ],
            None,
            "g.toon.relay",
        ),
        case(
            "routes_that_disagree_unpinned",
            "The devnet relay's other address: g.toon.relay.ephemeral is not pinned, so \
             a payer has no requirement for it.",
            vec![
                route("g.toon.relay", Some("btp")),
                route("g.toon.relay.ephemeral", None),
            ],
            None,
            "g.toon.relay.ephemeral",
        ),
        case(
            "uncovered_destination_with_a_node_value",
            "A destination no route covers falls to the node-wide value.",
            vec![route("g.toon.app", None)],
            Some("btp"),
            "g.toon.elsewhere",
        ),
        case(
            "uncovered_destination_with_no_node_value",
            "A destination no route covers, on a node that pins nothing: no requirement.",
            vec![route("g.toon.app", Some("btp"))],
            None,
            "g.toon.elsewhere",
        ),
        case(
            "sibling_label_is_not_covered",
            "A prefix covers whole labels only: g.toon.appx is not under g.toon.app.",
            vec![route("g.toon.app", Some("btp"))],
            None,
            "g.toon.appx",
        ),
        case(
            "nested_prefixes_longer_decides",
            "Nested prefixes: the longer one governs a destination under both, here \
             unpinned over a pinned parent.",
            vec![
                route("g.toon", Some("btp")),
                route("g.toon.relay", None),
                route("g.toon.relay.ephemeral", Some("http")),
            ],
            None,
            "g.toon.relay.x",
        ),
        case(
            "nested_prefixes_deepest_decides",
            "Nested prefixes: the deepest covering prefix decides.",
            vec![
                route("g.toon", Some("btp")),
                route("g.toon.relay", None),
                route("g.toon.relay.ephemeral", Some("http")),
            ],
            None,
            "g.toon.relay.ephemeral.x",
        ),
    ];
    RequiredCarriageVectors { cases }
}
