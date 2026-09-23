//! The node self-description (ADR 0050, `docs/protocol/self-description-spec.md`):
//! the facts a stranger needs to transact with this connector, as **one**
//! document, answered by a `GET` on the connector's own client-edge URL.
//!
//! # Why this module exists rather than a second struct next to the greeting
//!
//! Before ADR 0050 a node described itself in **two** places with different
//! field sets -- the x402 greeting's `extra` block and a kind:10032
//! `IlpPeerInfo` announce -- and neither was authoritative. That is the
//! `requiredTransport` defect: the requirement was *enforced* long before it
//! was *advertised*, because only one of the two descriptions was ever
//! checked.
//!
//! [`NodeFacts`] is the single source. The document ([`NodeSelfDescription`])
//! is one projection of it and the greeting's `extra` block
//! ([`crate::x402::terms_body`]) is the other, so ND-11's *"where the two
//! disagree the document is authoritative"* holds by construction: there is
//! nothing for them to disagree about.
//!
//! # What is deliberately not here
//!
//! * **`relayUrl`.** An assertion about software *behind* the connector
//!   (ND-08). A conforming connector works with no relay in the world
//!   (ADR 0046), and a paid reverse proxy does not describe its origin.
//! * **Per-peer facts** -- peer identities, per-peering fees, caps (ND-09,
//!   ADR 0006, ADR 0049). Publishing them discloses who this node peers with
//!   and how far it trusts each. A cap is discovered by being refused
//!   (ND-10), not by being published.
//! * **A TTL, or any caching contract** (ND-04). A pushed copy needed a shelf
//!   life; a pulled one does not.
//! * **A write.** There is no `POST` here and there never will be (ND-03,
//!   ADR 0043): a self-description endpoint that grows a write is purchasable
//!   peering through a side door.

use serde::{Deserialize, Serialize};

use crate::x402::{X402ChainSettlementTerms, X402SettlementTerms};

/// The client-edge versions this connector serves, and the one an
/// unversioned `POST /ilp` resolves to (issue #1054,
/// `client-edge-spec.md` §3.1/§3.2).
///
/// One version exists. The fields are published anyway so a client can
/// *assert* its assumption rather than infer it -- which is the whole reason
/// `GET /ilp/versions` was ever proposed, and why retiring that endpoint had
/// to put the same two facts somewhere rather than nowhere.
pub const CLIENT_EDGE_SUPPORTED_VERSIONS: &[u32] = &[1];

/// The version `POST /ilp` (unversioned) serves. Always `1`, per
/// `client-edge-spec.md` §3.1's permanence guarantee.
pub const CLIENT_EDGE_DEFAULT_VERSION: u32 = 1;

/// The facts this node holds about **itself**: what it was configured with
/// and what it proved against a chain at startup.
///
/// Everything in here is true *of this connector* (ND-05). Two of its parts
/// come from different provenances and the difference matters:
///
/// * `ilp_addresses`, `http_endpoint` and `btp_endpoint` are **configured**,
///   because they cannot be introspected -- a container sees `0.0.0.0:4000`,
///   never `https://proxy.example/ilp`. They are `[node]`'s three fields.
/// * `settlements` is **proved**: each entry is what a settlement backend
///   resolved and checked against a live chain before this node agreed to
///   boot. It is never separately declared, which is what makes issue #981
///   (`solana_chain_id` defaulting to `solana:devnet` on a mainnet node)
///   impossible rather than merely detected -- there is no second
///   declaration left to disagree with (ND-07).
///
/// The default is a node that configures no `[node]` section and settles
/// nowhere: every field empty, every key absent from the wire.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeFacts {
    /// Every ILP address this node answers to, primary first.
    pub ilp_addresses: Vec<String>,
    /// Where clients pay this node over ILP-over-HTTP.
    pub http_endpoint: Option<String>,
    /// Where clients pay this node over BTP.
    pub btp_endpoint: Option<String>,
    /// The peer carriages this node exposes a listener for -- `"btp"`,
    /// `"http"`, both, or neither (ADR 0027, `peer_expose`). Which carriages
    /// exist is a fact about this node's own listeners; **who** rides them is
    /// not published (ND-09).
    pub peer_carriages: Vec<String>,
    /// Every chain this node settles on, as the settlement backend proved it.
    pub settlements: Vec<X402ChainSettlementTerms>,
}

impl NodeFacts {
    /// The EVM entry of [`Self::settlements`], which is also the greeting's
    /// legacy `extra.settlement` object (issue #617).
    ///
    /// Derived rather than carried beside the list, so the one-chain object
    /// and the per-chain list cannot describe different deployments. A node
    /// has at most one `[settlement.evm]` table, so "the first EVM entry" is
    /// "the EVM entry".
    pub fn evm_settlement(&self) -> Option<&X402SettlementTerms> {
        self.settlements.iter().find_map(|entry| match entry {
            X402ChainSettlementTerms::Evm(evm) => Some(evm),
            X402ChainSettlementTerms::Solana(_) => None,
        })
    }
}

/// This connector's identity: the key a packet's payload is **sealed to**
/// (ADR 0018), and the key id naming it.
///
/// ND-06 makes publishing this mandatory rather than optional: a sender that
/// cannot seal to a route's terminating connector cannot have a packet
/// delivered there at all, so an unpublished identity is an unreachable
/// route.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EdgeIdentity {
    #[serde(rename = "keyId")]
    pub key_id: String,
    /// `0x`-prefixed hex of the uncompressed secp256k1 public key -- the same
    /// spelling `GET /ilp/identity` has always answered with.
    #[serde(rename = "publicKey")]
    pub public_key: String,
}

/// One priced route, as the document publishes it.
///
/// Prefix, price, and what it takes to use the route -- and nothing else. A
/// terminated route's `handler_url` is the operator's business and an app
/// fact besides (ND-08); a forwarded route's peer id and per-peering fee are
/// operator-private (ND-09). What is left is exactly what a buyer needs:
/// what to address, what it costs, and which carriage to arrive on.
///
/// `price` is a decimal **string** in the asset's base units, the same
/// spelling the greeting's `amount`/`extra.price` already use -- a `u64`
/// price is not representable in a JSON number a JavaScript reader can be
/// trusted with.
///
/// Two figures since ADR 0065, because a price is a schedule: `price` is what
/// a packet of any size costs, and `price_per_kib` is what each started
/// kibibyte of payload adds. This is the surface that keeps ADR 0011's
/// cacheability property true under a schedule -- a reader learns the whole
/// rule from one free document and computes any packet's cost itself, instead
/// of having to probe once per size.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoutePrice {
    pub prefix: String,
    pub price: String,
    /// Absent -- not `"0"` -- on a flat route, so a node serving only flat
    /// routes publishes exactly the document it published before schedules
    /// existed, and a reader written against that document is unaffected.
    #[serde(
        rename = "pricePerKib",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub price_per_kib: Option<String>,
    /// What a client should send to use this route (issue #1210): the
    /// operator's `[[routes]] request` table, converted to JSON and
    /// published verbatim -- this connector never reads a key out of it.
    /// Absent, not `null`, on a route that configured none, so a node with
    /// no `request` anywhere publishes exactly the document it published
    /// before this issue.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub request: Option<serde_json::Value>,
    /// The client transport **this** route requires (ND-05a, TOON_Network
    /// issue #111) -- `"http"` or `"btp"`, the two spellings
    /// `TransportPolicy::name` already gives the greeting's
    /// `extra.requiredTransport`, read off the very lookup both client-edge
    /// carriages enforce the pin from.
    ///
    /// Absent -- not `"both"`, not `null` -- on a route that accepts either
    /// carriage, which is every route no operator pinned. A node that pins
    /// nothing therefore publishes exactly the document it published before
    /// this field existed.
    ///
    /// [`NodeSelfDescription::required_transport`] is the same fact
    /// aggregated over this node's own addresses, and it says nothing the
    /// moment those addresses disagree. **The devnet relay is exactly such
    /// a node:** `g.toon.relay` is pinned to BTP, `g.toon.relay.ephemeral`
    /// is not, so the per-node scalar had no honest answer while the pin was
    /// fully enforced -- and a publisher reading the document dialled HTTP
    /// and had every write refused. A pin is enforced per route, so a pin is
    /// published per route.
    #[serde(
        rename = "requiredTransport",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub required_transport: Option<String>,
}

/// The document itself, as it goes on the wire.
///
/// Built by [`NodeSelfDescription::describe`] from live state on each
/// request: [`NodeFacts`] is fixed for the process lifetime (configuration
/// is immutable after boot, ADR 0009), but the route table is not -- a
/// runtime route written through the operator surface must show up in the
/// next answer, which is what "generated from live configuration" means and
/// why nothing here is cached (ND-04).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeSelfDescription {
    #[serde(
        rename = "ilpAddresses",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub ilp_addresses: Vec<String>,
    #[serde(
        rename = "httpEndpoint",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub http_endpoint: Option<String>,
    #[serde(
        rename = "btpEndpoint",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub btp_endpoint: Option<String>,
    /// Always written, even empty: "this node exposes no peer carriage" is an
    /// answer, and an absent key would read as "this node did not say".
    #[serde(rename = "peerCarriages", default)]
    pub peer_carriages: Vec<String>,
    /// Absent only on a node whose signer cannot produce a public key at all
    /// -- a broken node, not a configuration choice.
    #[serde(
        rename = "edgeIdentity",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub edge_identity: Option<EdgeIdentity>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub settlements: Vec<X402ChainSettlementTerms>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub routes: Vec<RoutePrice>,
    /// The one client transport the routes covering this node's own
    /// addresses require, when they agree on one that is not the permissive
    /// default -- see [`agreed_required_transport`].
    #[serde(
        rename = "requiredTransport",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub required_transport: Option<String>,
    #[serde(rename = "supportedVersions")]
    pub supported_versions: Vec<u32>,
    #[serde(rename = "defaultVersion")]
    pub default_version: u32,
}

impl NodeSelfDescription {
    /// Project one document out of this node's facts, its signing identity
    /// and its live route table.
    ///
    /// Takes the parts rather than a `&Connector` so the projection is
    /// exercisable without standing up a runtime: the rule is the part that
    /// has to be right.
    pub fn describe(
        facts: &NodeFacts,
        edge_identity: Option<EdgeIdentity>,
        routes: Vec<RoutePrice>,
        required_transport: Option<String>,
    ) -> NodeSelfDescription {
        NodeSelfDescription {
            ilp_addresses: facts.ilp_addresses.clone(),
            http_endpoint: facts.http_endpoint.clone(),
            btp_endpoint: facts.btp_endpoint.clone(),
            peer_carriages: facts.peer_carriages.clone(),
            edge_identity,
            settlements: facts.settlements.clone(),
            routes,
            required_transport,
            supported_versions: CLIENT_EDGE_SUPPORTED_VERSIONS.to_vec(),
            default_version: CLIENT_EDGE_DEFAULT_VERSION,
        }
    }
}

/// What one route's transport policy says on the wire: `Some("http")` or
/// `Some("btp")` for a pinned route, `None` for one that accepts either.
///
/// `policy` is the policy's config-file spelling -- the same spelling the
/// greeting's `extra.requiredTransport` uses, so no two surfaces describe one
/// policy by two names.
///
/// `"both"` becomes `None` rather than `Some("both")` because the field
/// answers *which carriage must I use*, and "either" is not an answer to
/// that: it is the absence of a requirement. Omitting it also keeps an
/// unpinned node's document byte-identical to the one it published before
/// this field existed (ND-05a).
///
/// The one place that knows `"both"` means silence.
/// [`agreed_required_transport`] defers to it, so a per-route entry and the
/// per-node scalar cannot come to different conclusions about the same
/// policy.
pub fn published_required_transport(policy: &str) -> Option<String> {
    (policy != "both").then(|| policy.to_string())
}

/// The one client transport every covered route requires, or `None` when
/// there is no single honest answer.
///
/// `policies` is the transport policy of each route covering one of this
/// node's own addresses, by that policy's config-file spelling -- the same
/// spelling the greeting's `extra.requiredTransport` uses, so the two
/// surfaces cannot drift into describing one policy by two names.
///
/// `None` in three distinct cases, all of which mean "say nothing":
///
///   * no address of this node resolves to a route it serves;
///   * the routes that do resolve disagree, which no per-node scalar can
///     describe;
///   * they agree on `"both"`, the permissive default every route had before
///     issue #701. Emitting it would be true and useless.
///
/// **The second case is not rare, and it is what TOON_Network issue #111 was
/// filed about.** A node answering to more than one address usually pins some
/// of them and not others -- the devnet relay answers to `g.toon.relay`,
/// pinned to BTP, and `g.toon.relay.ephemeral`, which is not -- so this
/// returns `None` and the node says nothing about a pin it enforces on every
/// packet. That is why [`RoutePrice::required_transport`] exists: the pin is
/// enforced per route, so it is published per route, and this scalar is the
/// convenience for a node where one answer covers everything rather than the
/// only place a pin is named.
pub fn agreed_required_transport<'a>(
    policies: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let mut policies = policies.into_iter();
    let first = policies.next()?;
    if !policies.all(|policy| policy == first) {
        return None;
    }
    published_required_transport(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evm() -> X402SettlementTerms {
        X402SettlementTerms {
            chain: "evm:84532".to_string(),
            settlement_address: "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf".to_string(),
            token_network_registry: "0xcc9079ade929b168b54145f6d25262b64fab9d5b".to_string(),
            token_network: "0x1e95493fef46707e034b4a1945f25a8c76a1823d".to_string(),
            token_address: "0x49bee1bca5d15fb0963117923403f9498119a9ce".to_string(),
            decimals: 6,
        }
    }

    fn facts() -> NodeFacts {
        NodeFacts {
            ilp_addresses: vec!["g.toon.ario".to_string()],
            http_endpoint: Some("https://proxy.example/ilp".to_string()),
            btp_endpoint: Some("wss://proxy.example/ilp/btp".to_string()),
            peer_carriages: vec!["btp".to_string()],
            settlements: vec![X402ChainSettlementTerms::Evm(evm())],
        }
    }

    /// The legacy one-chain greeting object is the list's EVM entry, never a
    /// second field somebody has to remember to keep in step.
    #[test]
    fn the_evm_settlement_is_the_lists_own_evm_entry() {
        assert_eq!(facts().evm_settlement(), Some(&evm()));
        assert_eq!(NodeFacts::default().evm_settlement(), None);
    }

    /// A node with no `[node]` section and no settlement backend answers a
    /// document with no facts in it rather than failing to answer -- the
    /// version fields alone, which are true of every node.
    #[test]
    fn a_node_that_configured_nothing_still_describes_its_versions() {
        let document = NodeSelfDescription::describe(&NodeFacts::default(), None, Vec::new(), None);
        let json = serde_json::to_value(&document).expect("serializes");

        assert_eq!(json["supportedVersions"], serde_json::json!([1]));
        assert_eq!(json["defaultVersion"], serde_json::json!(1));
        assert_eq!(json["peerCarriages"], serde_json::json!([]));
        for absent in [
            "ilpAddresses",
            "httpEndpoint",
            "btpEndpoint",
            "edgeIdentity",
            "settlements",
            "routes",
            "requiredTransport",
        ] {
            assert!(
                json.get(absent).is_none(),
                "'{absent}' must be absent rather than null on a node that has none"
            );
        }
    }

    /// Every field name the emitted document carries, at any depth.
    ///
    /// The forbidden-name check below walks these rather than searching the
    /// serialized text: `settlements` and `settlementAddress` both contain
    /// "ttl", and a substring search reads the chain this node settles on as
    /// an announce TTL. A field name is the thing ND-08/ND-09 forbid, so a
    /// field name is what gets compared.
    fn field_names(value: &serde_json::Value, into: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(fields) => {
                for (name, child) in fields {
                    into.push(name.clone());
                    field_names(child, into);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    field_names(item, into);
                }
            }
            _ => {}
        }
    }

    /// ND-08/ND-09, asserted on the emitted document rather than trusted to
    /// the struct definition: no relay, no peer, no cap, no TTL, ever.
    #[test]
    fn the_document_carries_no_relay_no_peer_and_no_ttl() {
        let document = NodeSelfDescription::describe(
            &facts(),
            Some(EdgeIdentity {
                key_id: "key-1".to_string(),
                public_key: "0x04ab".to_string(),
            }),
            vec![RoutePrice {
                prefix: "g.toon.ario".to_string(),
                price: "1000".to_string(),
                price_per_kib: None,
                request: None,
                required_transport: None,
            }],
            Some("btp".to_string()),
        );
        let json = serde_json::to_value(&document).expect("serializes");
        let mut names = Vec::new();
        field_names(&json, &mut names);

        for forbidden in ["relay", "peerId", "peer_id", "cap", "ttl", "fee", "notice"] {
            assert!(
                !names
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(forbidden)),
                "the self-description must not carry a '{forbidden}' field: {names:?}"
            );
        }
    }

    /// A price is a decimal string, not a JSON number: a `u64` price is not
    /// safely representable in one.
    #[test]
    fn a_route_price_rides_as_a_decimal_string() {
        let document = NodeSelfDescription::describe(
            &facts(),
            None,
            vec![RoutePrice {
                prefix: "g.toon.ario".to_string(),
                price: u64::MAX.to_string(),
                price_per_kib: None,
                request: None,
                required_transport: None,
            }],
            None,
        );
        let json = serde_json::to_value(&document).expect("serializes");

        assert_eq!(
            json["routes"][0]["price"],
            serde_json::json!("18446744073709551615")
        );
    }

    /// Issue #1210: a route's `request` table rides as JSON equal to what
    /// the operator wrote, and a route that configured none has no
    /// `request` key at all -- not `null`.
    #[test]
    fn a_routes_request_table_rides_as_the_equivalent_json() {
        let document = NodeSelfDescription::describe(
            &facts(),
            None,
            vec![
                RoutePrice {
                    prefix: "g.toon.gas".to_string(),
                    price: "1000".to_string(),
                    price_per_kib: None,
                    request: Some(serde_json::json!({"protocol": "nip90", "kinds": [5096, 5098]})),
                    required_transport: None,
                },
                RoutePrice {
                    prefix: "g.toon.ario".to_string(),
                    price: "1000".to_string(),
                    price_per_kib: None,
                    request: None,
                    required_transport: None,
                },
            ],
            None,
        );
        let json = serde_json::to_value(&document).expect("serializes");

        assert_eq!(
            json["routes"][0]["request"],
            serde_json::json!({"protocol": "nip90", "kinds": [5096, 5098]})
        );
        assert!(
            json["routes"][1].get("request").is_none(),
            "a route with no request table must publish no 'request' key at all"
        );
    }

    /// TOON_Network#111: a pinned route names its carriage on its own entry,
    /// and an unpinned one carries no such key at all -- not `"both"`, not
    /// `null`. A node that pins nothing publishes exactly the document it
    /// published before this field existed (ND-05a).
    #[test]
    fn a_pinned_route_names_its_carriage_and_an_unpinned_one_says_nothing() {
        let document = NodeSelfDescription::describe(
            &facts(),
            None,
            vec![
                RoutePrice {
                    prefix: "g.toon.relay".to_string(),
                    price: "1".to_string(),
                    price_per_kib: None,
                    request: None,
                    required_transport: published_required_transport("btp"),
                },
                RoutePrice {
                    prefix: "g.toon.relay.ephemeral".to_string(),
                    price: "0".to_string(),
                    price_per_kib: None,
                    request: None,
                    required_transport: published_required_transport("both"),
                },
            ],
            // Those two policies disagree, so the per-node scalar has nothing
            // honest to say -- which is the devnet relay, and the reason the
            // per-route field exists.
            agreed_required_transport(["btp", "both"]),
        );
        let json = serde_json::to_value(&document).expect("serializes");

        assert_eq!(json["routes"][0]["requiredTransport"], "btp");
        assert!(
            json["routes"][1].get("requiredTransport").is_none(),
            "a route accepting either carriage must publish no 'requiredTransport' key at all"
        );
        assert!(
            json.get("requiredTransport").is_none(),
            "and the node-wide scalar stays silent rather than picking one of them"
        );
    }

    /// `"both"` is the absence of a requirement, so it is spelled as an
    /// absence -- one rule, in one place, for the per-route field and the
    /// per-node scalar alike.
    #[test]
    fn the_permissive_default_is_published_as_no_field_at_all() {
        assert_eq!(published_required_transport("btp"), Some("btp".to_string()));
        assert_eq!(
            published_required_transport("http"),
            Some("http".to_string())
        );
        assert_eq!(published_required_transport("both"), None);
    }

    #[test]
    fn one_agreed_policy_that_is_not_the_default_is_the_answer() {
        assert_eq!(
            agreed_required_transport(["btp", "btp"]),
            Some("btp".to_string())
        );
    }

    #[test]
    fn disagreement_no_route_and_the_permissive_default_all_say_nothing() {
        assert_eq!(agreed_required_transport(["btp", "http"]), None);
        assert_eq!(agreed_required_transport(std::iter::empty()), None);
        assert_eq!(agreed_required_transport(["both", "both"]), None);
    }

    /// The document round-trips, so a client SDK written against these names
    /// reads back what this connector wrote.
    #[test]
    fn the_document_round_trips_through_its_own_wire_names() {
        let document = NodeSelfDescription::describe(
            &facts(),
            Some(EdgeIdentity {
                key_id: "key-1".to_string(),
                public_key: "0x04ab".to_string(),
            }),
            vec![RoutePrice {
                prefix: "g.toon.ario".to_string(),
                price: "1000".to_string(),
                price_per_kib: None,
                request: None,
                required_transport: None,
            }],
            Some("btp".to_string()),
        );
        let bytes = serde_json::to_vec(&document).expect("serializes");
        let back: NodeSelfDescription = serde_json::from_slice(&bytes).expect("round-trips");

        assert_eq!(back, document);
    }
}
