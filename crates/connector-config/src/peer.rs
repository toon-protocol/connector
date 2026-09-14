use std::collections::HashSet;
use std::fmt;

use serde::Deserialize;
use url::Url;

use crate::error::ConfigError;

/// The default for `claim_ack_timeout_ms` and `peer_answer_timeout_ms`
/// (`peer-carriage-spec.md` §6.3): thirty seconds each.
///
/// Public because a peering established at runtime (ADR 0058) has no
/// `[[peers]]` row to read a timeout off and must land on the same number a
/// config-file peering that wrote none does -- read from here rather than
/// typed again somewhere the two could drift.
pub const DEFAULT_PEER_TIMEOUT_MS: u64 = 30_000;

/// The default `max_packet_amount` (ADR 0042, "The cap"): the largest
/// amount this connector will forward to one peer in a **single packet**,
/// in the settlement asset's own base units -- 6-decimal USDC everywhere on
/// this fleet (ADR 0010, `docs/usdc-cross-chain-settlement.md`), so this is
/// **1 USDC**.
///
/// A default exists at all because ADR 0042 requires one: the cap is the
/// most a single theft by a next hop can take, and an operator who never
/// writes the field must still be bounded. Public so the runtime reads this
/// number rather than restating it, so a peer with no row of its own (one
/// added at runtime over the operator surface, issue #884) is bounded by
/// exactly the same figure a config-file peer that left the field unwritten
/// is.
///
/// # Why 1 000 000, and what was inspected to pick it
///
/// The number had to clear everything the live devnet actually carries by a
/// wide margin, so turning the cap on refuses nothing that works today:
///
/// * `infra/linode-relay/connector-rust.toml` prices `g.toon.relay` at
///   **1** µUSDC per write (buzz huddles, per audio frame at 49 fps), and
///   `infra/linode-store/connector-rust.toml` prices `g.toon.ario` at
///   **1000**. `crates/connector-bin/tests/devnet_configs_load.rs` pins both
///   as `EXPECTED_RELAY_PRICE`/`EXPECTED_STORE_PRICE`, and
///   `docs/devnet-pricing.md` is the committed table they come from.
/// * The largest *forwarded* amount this fleet ever ran was the retired
///   apex's `g.toon.ario` leg: a client paid `1002`, the apex kept a fee of
///   `2`, and **1000** went over the peering (`docs/devnet-pricing.md`, "The
///   apex forward"; `docs/protocol/money-model-pre-868.md`'s worked example).
/// * The largest single packet observed live on the fleet is **1998**
///   (the parallel-fleet comparison's `[write] … amount=1998`, a record
///   since deleted from `docs/operators/`; in git history), and the retired TypeScript `announcePrice`
///   buffer -- the biggest figure any devnet config ever named -- was
///   **2000**.
///
/// So 1 000 000 sits ~500x above the largest amount this fleet has ever put
/// in one packet and 1000x above its most expensive committed route, while
/// still being a real bound: one packet to one peer can never carry more
/// than a dollar. Neither devnet box configures a peering at all today, so
/// nothing on the fleet is even reached by this check -- the margin is for
/// the next peering, and an operator who wants more writes
/// `max_packet_amount` on that peer's own `[[peers]]` row.
pub const DEFAULT_MAX_PACKET_AMOUNT: u64 = 1_000_000;

/// Which carriage a peering rides (`peer-carriage-spec.md` §0.1). There are
/// exactly two, and neither is selected by a `transport` field: a
/// connector's *expose* set says which listeners it opens, and each peer's
/// endpoint **scheme** says which carriage this connector dials that peer
/// on (§2.1). ADR 0027 deleted the raw-TCP transport, so there is no third
/// value and nothing to add one for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerCarriage {
    /// BTP over a `wss://` websocket. Symmetric once established: either
    /// side may originate on the one session (§2.3).
    Btp,
    /// ILP-over-HTTP to an `https://` endpoint. Only the dialing side can
    /// originate (§2.3, §6.4).
    Http,
}

impl PeerCarriage {
    /// The wire-visible name (`peer-carriage-spec.md` §11 is normative for
    /// these two spellings).
    pub fn name(self) -> &'static str {
        match self {
            PeerCarriage::Btp => "btp",
            PeerCarriage::Http => "http",
        }
    }

    /// The URL scheme that selects this carriage. Both are TLS-only: a
    /// peering carries signed balance proofs (ADR 0004), so `ws://` and
    /// `http://` select nothing and are refused.
    fn from_scheme(scheme: &str) -> Option<PeerCarriage> {
        match scheme {
            "wss" => Some(PeerCarriage::Btp),
            "https" => Some(PeerCarriage::Http),
            _ => None,
        }
    }

    /// [`PeerCarriage::from_scheme`], plus the two **plaintext** schemes a
    /// node that set `peer_allow_plaintext_endpoints` may also dial
    /// (issue #678, gap 3): `ws://` selects BTP and `http://` selects
    /// ILP-over-HTTP, on exactly the same terms their TLS twins do.
    ///
    /// `allow_plaintext` is `false` on every production config and on
    /// every config that does not mention the field, and then this is
    /// [`PeerCarriage::from_scheme`] byte for byte -- a plaintext endpoint
    /// is still [`ConfigError::PeerEndpointScheme`]. The switch exists so a
    /// laptop-runnable end-to-end test can point one connector at another's
    /// loopback socket without a TLS terminator in the harness; it is not
    /// a deployment shape.
    ///
    /// **This is the scheme half of §2.1 and not the whole of it.** A
    /// caller holding a URL wants [`PeerCarriage::for_endpoint`], which is
    /// this function plus ADR 0070's host rule; this one is left public
    /// because the meaning and the scope of `peer_allow_plaintext_endpoints`
    /// are asserted directly against it, and a rule nothing can name is a
    /// rule nothing can hold still.
    pub fn from_scheme_allowing_plaintext(
        scheme: &str,
        allow_plaintext: bool,
    ) -> Option<PeerCarriage> {
        match PeerCarriage::from_scheme(scheme) {
            Some(carriage) => Some(carriage),
            None if allow_plaintext => match scheme {
                "ws" => Some(PeerCarriage::Btp),
                "http" => Some(PeerCarriage::Http),
                _ => None,
            },
            None => None,
        }
    }

    /// **The** §2.1 resolver: which carriage this connector dials `url` on,
    /// or `None` for a URL whose scheme selects nothing this node dials.
    ///
    /// Two rules, in one place. The scheme selects the carriage
    /// ([`PeerCarriage::from_scheme_allowing_plaintext`]), and -- ADR 0070
    /// decision 2 -- a host ending in `.onion` or `.anyone` permits the
    /// plaintext schemes on its own, independently of
    /// `peer_allow_plaintext_endpoints`.
    ///
    /// The onion exemption is narrow because of what a v3 onion address
    /// **is**: a base32 encoding of the ed25519 public key the circuit is
    /// encrypted and authenticated to, so a client that reached
    /// `abc...xyz.onion` reached the holder of that key or reached nothing.
    /// ADR 0004's requirement that a peering's wire be authenticated is
    /// therefore *satisfied by a different mechanism* rather than waived --
    /// which is why the exemption keys on the `ONION_SUFFIXES` below and on
    /// nothing else, and why it does not widen
    /// `peer_allow_plaintext_endpoints`, whose own doc comment above still
    /// describes a test affordance and not a deployment shape. The two
    /// spellings are one rule: the daemon renamed its TLD, and an address
    /// is the key either way (issue #1284).
    ///
    /// Public, and the only thing any call site may call, because §2.1 is
    /// one sentence: two implementations of one sentence is how a `wss://`
    /// peering ends up dialed over HTTP, and how an onion peering ends up
    /// dialed by a config-file peer and refused by a runtime one. The call
    /// sites are the config-file peer resolution below,
    /// [`crate::PeerCarriage`]'s runtime twin in `connector-runtime`'s
    /// `RuntimePeering::dial`, the self-description read that picks a
    /// dialable published endpoint (ADR 0058), and the registrar that gives
    /// a runtime peering its carriage.
    #[must_use]
    pub fn for_endpoint(url: &Url, allow_plaintext: bool) -> Option<PeerCarriage> {
        PeerCarriage::from_scheme_allowing_plaintext(
            url.scheme(),
            plaintext_permitted(url, allow_plaintext),
        )
    }
}

/// The host suffixes that make an endpoint an **onion endpoint** (ADR 0070,
/// amended by issue #1284).
///
/// A hidden-service name is a host, not a scheme and not a carriage:
/// `http://` at one is still ILP-over-HTTP and `ws://` at one is still BTP.
/// Written once so the suffix that decides a carriage and the suffix that
/// decides a SOCKS5 dial are the same characters.
///
/// **Two spellings of one thing.** Anyone Protocol's `anon` renamed the TLD
/// it publishes and routes between v0.4.9.7 (`.onion`) and v0.4.10.2
/// (`.anyone`), and the rename is total in both directions -- v0.4.10.2
/// contains no occurrence of `.onion` and refuses an address spelled that
/// way, and v0.4.9.7 does the opposite. Both are accepted because ADR
/// 0070's argument is about the ADDRESS and not about the TLD: either
/// spelling is the base32 ed25519 public key the circuit is authenticated
/// to, so the exemption those suffixes carry is earned identically. Keeping
/// `.onion` costs a second `ends_with` and lets one binary peer with either
/// daemon; dropping it would refuse a config that is correct against every
/// daemon this repository has ever run.
///
/// The narrowness that matters is untouched: these are SUFFIXES, so
/// `anyone.example` and `notreally.onion.example` are ordinary clearnet
/// hosts and a plaintext scheme at either is still refused.
const ONION_SUFFIXES: [&str; 2] = [".onion", ".anyone"];

/// Whether `url` is an **onion endpoint** -- a URL whose host ends in
/// `.onion` or `.anyone` (ADR 0070, amended by issue #1284).
///
/// The one implementation of that question, for both of the things it
/// decides: whether a plaintext scheme selects a carriage
/// ([`PeerCarriage::for_endpoint`]) and whether a dial goes through the
/// configured SOCKS5 proxy rather than direct. A second copy is how a node
/// ends up loading a peering it will not dial, or dialing one it should
/// have refused -- and, since the rename, how a node ends up accepting an
/// address on one surface and refusing the same address on the next.
///
/// Case-insensitive, because a host is: `Url` already lowercases the host
/// of a special scheme, and the explicit fold here is so a caller reading
/// this function does not have to know that. `None` host -- which no
/// `http`/`https`/`ws`/`wss` URL has, since all four are special schemes
/// and never parse without one -- is not an onion endpoint.
#[must_use]
pub fn is_onion_endpoint(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        let host = host.to_ascii_lowercase();
        ONION_SUFFIXES.iter().any(|suffix| host.ends_with(suffix))
    })
}

/// Whether a **plaintext** scheme is acceptable at `url`: the one sentence
/// every surface that reads an operator-supplied URL has to agree on.
///
/// Two ways to be acceptable, and they are independent. `allow_plaintext`
/// is the node-wide `peer_allow_plaintext_endpoints` opt-in (issue #678,
/// gap 3) -- a laptop-harness affordance, not a deployment shape. The other
/// is ADR 0070 decision 2: a `.onion` or `.anyone` host permits the
/// plaintext schemes on its own, because the address *is* the ed25519 key
/// the circuit is authenticated to, so ADR 0004's requirement is satisfied
/// by a different mechanism rather than waived.
///
/// **Named, and called from every surface, because writing it out is how it
/// gets written out wrong.** It governs four things that must not disagree
/// about one URL: which carriage a peer endpoint selects
/// ([`PeerCarriage::for_endpoint`]), whether a `[[pay_channels]]` row's
/// `client_edge_url` loads, whether the self-description a runtime peering
/// is established from may be read (ADR 0058), and -- through
/// [`is_onion_endpoint`] -- which socket the dial then leaves on. The
/// version of this sentence that omits its second clause is still a
/// plausible-looking `match` arm, and it produces a node that loads an
/// onion peering it can never cover and can never be peered with at
/// runtime.
#[must_use]
pub fn plaintext_permitted(url: &Url, allow_plaintext: bool) -> bool {
    allow_plaintext || is_onion_endpoint(url)
}

impl fmt::Display for PeerCarriage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Which peer carriages this connector opens a listener for
/// (`peer-carriage-spec.md` §2.1) -- a set over `{btp, http}`, spelled as
/// the four values that set can take because TOML cannot hold both a
/// `[peers]` table and a `[[peers]]` array of tables under one name.
///
/// [`PeerExposure::Neither`] is the empty set, and it is **legal and
/// meaningful**: it is the NAT'd operator, who exposes nothing and only
/// dials out (§2.4). It is also the default, because opening a peer
/// listener is a decision an operator makes rather than one a missing line
/// makes for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeerExposure {
    /// The empty set: this connector opens no peer listener and can only
    /// dial out. The NAT'd case (§2.4).
    #[default]
    Neither,
    /// BTP only -- the carriage a NAT'd counterparty can dial and then be
    /// reached back over.
    Btp,
    /// ILP-over-HTTP only. Peers exclusively with dialable counterparties
    /// (§2.4).
    Http,
    /// Both listeners.
    Both,
}

impl PeerExposure {
    /// The spelling an operator writes.
    pub fn name(self) -> &'static str {
        match self {
            PeerExposure::Neither => "neither",
            PeerExposure::Btp => "btp",
            PeerExposure::Http => "http",
            PeerExposure::Both => "both",
        }
    }

    /// Whether this connector opens a listener for `carriage`.
    pub fn exposes(self, carriage: PeerCarriage) -> bool {
        match carriage {
            PeerCarriage::Btp => matches!(self, PeerExposure::Btp | PeerExposure::Both),
            PeerCarriage::Http => matches!(self, PeerExposure::Http | PeerExposure::Both),
        }
    }

    /// Whether this connector opens no peer listener at all.
    pub fn is_empty(self) -> bool {
        matches!(self, PeerExposure::Neither)
    }
}

impl fmt::Display for PeerExposure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for PeerExposure {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "neither" => Ok(PeerExposure::Neither),
            "btp" => Ok(PeerExposure::Btp),
            "http" => Ok(PeerExposure::Http),
            "both" => Ok(PeerExposure::Both),
            _ => Err(()),
        }
    }
}

/// Whether a peer PREPARE this connector would **forward** onward (ADR
/// 0042's item 3) is refused when it arrives uncovered, or admitted and
/// logged.
///
/// **The only claim-enforcement knob a peering still has.** Its sibling
/// `claim_enforcement` -- ADR 0029's rule for an arrival to a priced
/// **termination** -- was deleted with its `"observe"` escape hatch (ADR
/// 0042 item 4, issue #1077); a terminated arrival is now always enforced
/// and the key is a parsed-and-rejected tombstone
/// ([`ConfigError::PeerClaimEnforcementRemoved`]). This field survived that
/// deletion because the two migrations default in opposite directions and
/// end on different days:
///
/// - Terminated arrivals had been enforced since issue #880, so `Enforce`
///   was the default there and `"observe"` was the escape hatch, dated for
///   removal from the day it shipped.
/// - Forwarded arrivals have **never** been charged: neither box on the
///   fleet covers a forward yet (`[[pay_channels]]` shipped but is opt-in
///   per peering and no committed config writes one), so a default of
///   `Enforce` would stop forwarding across the fleet the moment the binary
///   rolled. [`ForwardedClaimEnforcement::Observe`] is therefore the
///   **default**, and an operator flips a peering to
///   [`ForwardedClaimEnforcement::Enforce`] once that peering's counterparty
///   is covering its forwards.
///
/// Folding the two into one field would have made one of those defaults
/// wrong, and folding them into one *variant set* would have tied this
/// field's default to the terminated escape hatch's dated deletion --
/// which has since happened, and would have taken this default with it.
///
/// **Temporary, like its sibling.** Once every peering across the fleet
/// covers its forwards and reads `Enforce`, this field and its `Observe`
/// variant should be deleted so the ADR 0042 rule is simply the behaviour,
/// using the same removed-field-trap convention `ceiling`/`flush_interval_ms`
/// use (`ConfigError::PeerCeilingRemoved`, [`resolve_peers`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ForwardedClaimEnforcement {
    /// Admit an uncovered forwarded arrival, logging it exactly the way a
    /// refusal would be logged. **The default**, because enforcing by
    /// default breaks a fleet whose send halves are not live yet.
    #[default]
    Observe,
    /// Refuse an uncovered forwarded arrival (`F06_UNEXPECTED_PAYMENT` plus
    /// the x402 greeting), the same refusal a priced termination's arrival
    /// gets. ADR 0042's permanent rule, opted into per peering.
    Enforce,
}

impl ForwardedClaimEnforcement {
    /// The spelling an operator writes.
    pub fn name(self) -> &'static str {
        match self {
            ForwardedClaimEnforcement::Observe => "observe",
            ForwardedClaimEnforcement::Enforce => "enforce",
        }
    }
}

impl fmt::Display for ForwardedClaimEnforcement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for ForwardedClaimEnforcement {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "observe" => Ok(ForwardedClaimEnforcement::Observe),
            "enforce" => Ok(ForwardedClaimEnforcement::Enforce),
            _ => Err(()),
        }
    }
}

/// Parse the top-level `peer_expose` field, defaulting to
/// [`PeerExposure::Neither`] when the operator wrote nothing -- and
/// refusing a written-but-unrecognized spelling by name, the same way
/// `[[routes]]`'s `transport` is (issue #701): `deny_unknown_fields`
/// closes the mistyped-*key* hole, and this closes the mistyped-*value*
/// one.
pub(crate) fn parse_peer_exposure(value: Option<String>) -> Result<PeerExposure, ConfigError> {
    match value {
        None => Ok(PeerExposure::default()),
        Some(value) => value
            .parse()
            .map_err(|()| ConfigError::InvalidPeerExposure { value }),
    }
}

/// A `[[peers]]` entry as written in the config file: one peering
/// relation, named so a `[[routes]]` entry can target it by `peer_id`.
///
/// `deny_unknown_fields` (issue #556): a peer entry carrying a key this
/// build does not read -- a typo, or a field from a shape this connector
/// does not implement -- fails config load loudly rather than being
/// dropped and the node peering on terms nobody wrote. `addr` is kept as a
/// *parsed and rejected* field rather than left to that generic message,
/// so a stale bind-mounted box config gets told what happened and where to
/// read about it (ADR 0027, issue #679). `ceiling`/`flush_interval_ms` are
/// kept the same way (ADR 0031, ADR 0033, issue #882): the credit window
/// they bounded is retired now that every peer PREPARE carries its own
/// covering claim, and a devnet box's bind-mounted TOML still names them.
/// `credential` joins them under ADR 0060: the `{peerId, secret}` bearer
/// secret is deleted outright, and a peering is proven by a verified claim
/// on one of its `[[peer_channels]]` rows instead.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPeer {
    id: String,
    /// Removed with the raw-TCP transport (ADR 0027, issue #679); a peer
    /// is reached by `endpoint` now.
    #[serde(default)]
    addr: Option<toml::Value>,
    #[serde(default)]
    endpoint: Option<String>,
    /// Deleted with the peer shared secret (ADR 0060, issue #1157): role
    /// is P2 + P3 -- a channel binding and a verified claim signature --
    /// and a bearer string decides nothing. Parsed as an opaque value only
    /// so a config that still writes one is refused **by name**
    /// ([`ConfigError::PeerCredentialRemoved`]) rather than dropped, the
    /// posture ADR 0009 requires of every removed key.
    #[serde(default)]
    credential: Option<toml::Value>,
    /// Removed with the credit window (ADR 0031, ADR 0033, issue #882);
    /// there is no trailing exposure left for a ceiling to bound.
    #[serde(default)]
    ceiling: Option<toml::Value>,
    /// Removed with the credit window (ADR 0031, ADR 0033, issue #882);
    /// a claim no longer trails the fulfilment it covers, so there is
    /// nothing left to flush on a timer.
    #[serde(default)]
    flush_interval_ms: Option<toml::Value>,
    #[serde(default)]
    claim_ack_timeout_ms: Option<u64>,
    #[serde(default)]
    peer_answer_timeout_ms: Option<u64>,
    /// Removed with the B6 migration ramp it selected (ADR 0042 item 4,
    /// issue #1077): a terminated arrival is enforced unconditionally, so
    /// there is no mode left for this key to pick. Parsed as an opaque
    /// value only so it can be refused **by name**, the way `ceiling` and
    /// `flush_interval_ms` are.
    #[serde(default)]
    claim_enforcement: Option<toml::Value>,
    /// ADR 0042's item 3: `"enforce"` refuses a peer PREPARE this connector
    /// would forward onward when it arrives without a claim covering the
    /// packet's own `amount`. Omitted, or written `"observe"`, is
    /// [`ForwardedClaimEnforcement::Observe`] -- admitted and logged, the
    /// **default**, because the fleet's send halves are not live yet. See
    /// [`ForwardedClaimEnforcement`] for why it outlived the
    /// `claim_enforcement` knob it was deliberately kept separate from.
    #[serde(default)]
    forwarded_claim_enforcement: Option<String>,
    /// ADR 0042's cap: the largest amount this connector will forward to
    /// this peer in one packet. Omitted is [`DEFAULT_MAX_PACKET_AMOUNT`] --
    /// there is no "unbounded" spelling, deliberately.
    #[serde(default)]
    max_packet_amount: Option<u64>,
    /// ADR 0061's fee: the flat amount this connector retains for carrying
    /// one packet to this peer. Omitted is zero -- free carriage, and the
    /// value every config in this tree that never wrote one already had.
    #[serde(default)]
    fee: Option<u64>,
}

/// A fully validated peering relation. Constructed only by
/// [`resolve_peers`], so a value that exists has a non-empty id unique
/// among every other configured peer and -- if it carries an endpoint at
/// all -- one whose scheme names a real carriage.
///
/// One value per **peering relation**, never per carriage and never per
/// connection (`peer-carriage-spec.md` §2.5): the claim watermarks belong
/// to the relation, and splitting them per carriage is a double-spend
/// surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerConfig {
    id: String,
    endpoint: Option<Url>,
    dial: Option<PeerCarriage>,
    can_originate: bool,
    claim_ack_timeout_ms: u64,
    peer_answer_timeout_ms: u64,
    forwarded_claim_enforcement: ForwardedClaimEnforcement,
    max_packet_amount: u64,
    fee: u64,
}

impl PeerConfig {
    /// This peering relation's id -- what a `[[routes]]` entry's `peer_id`
    /// refers to, and the name every `[[peer_channels]]` row of this
    /// relation binds its channels under.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The URL this connector dials to reach the peer, or `None` for an
    /// **accept-only** peering: one this connector never dials and that
    /// dials in instead (§2.1).
    pub fn endpoint(&self) -> Option<&Url> {
        self.endpoint.as_ref()
    }

    /// Which carriage this connector dials this peer on, decided **solely**
    /// by the endpoint's scheme (§2.1). `None` for an accept-only peering.
    pub fn dial(&self) -> Option<PeerCarriage> {
        self.dial
    }

    /// Whether this connector can ever send a packet to this peer.
    ///
    /// True if it dials the peer (on either carriage), or if it exposes
    /// BTP -- a dialed BTP session is symmetric once established, so a peer
    /// that dials in over `wss://` can be originated to on that same
    /// session (§2.3). False for the one remaining shape: an accept-only
    /// peering on a connector that exposes only HTTP, where packets can
    /// only ever flow the other way (§6.4(1)).
    pub fn can_originate(&self) -> bool {
        self.can_originate
    }

    /// How long a sent claim may go unacknowledged before it is
    /// retransmitted (§6.3). Defaults to 30 000 ms.
    pub fn claim_ack_timeout_ms(&self) -> u64 {
        self.claim_ack_timeout_ms
    }

    /// How long a request to this peer may go unanswered (§6.3). Defaults
    /// to 30 000 ms.
    pub fn peer_answer_timeout_ms(&self) -> u64 {
        self.peer_answer_timeout_ms
    }

    /// Whether an uncovered **forwarded** arrival from this peering is
    /// refused or admitted-and-logged (ADR 0042's item 3). Defaults to
    /// [`ForwardedClaimEnforcement::Observe`], the permissive way, for the
    /// reason that type documents. An uncovered arrival to a priced
    /// **termination** has no such knob: it is always refused (ADR 0029,
    /// issue #880; the `claim_enforcement` escape hatch was deleted by
    /// issue #1077).
    pub fn forwarded_claim_enforcement(&self) -> ForwardedClaimEnforcement {
        self.forwarded_claim_enforcement
    }

    /// This peering's **cap** (ADR 0042): the largest amount this connector
    /// will forward to it in a single packet. A packet needing more is
    /// refused with `T04`, never carried and never split.
    ///
    /// Bounds one packet, not an accumulation -- ADR 0033 retired the
    /// exposure ceiling and it is not coming back (see `CONTEXT.md`'s
    /// glossary, which keeps "ceiling" and "cap" apart for exactly this
    /// reason). Defaults to [`DEFAULT_MAX_PACKET_AMOUNT`], so a peering that
    /// wrote nothing is still bounded.
    pub fn max_packet_amount(&self) -> u64 {
        self.max_packet_amount
    }

    /// This peering's flat per-packet **fee** (ADR 0010, ADR 0061): what
    /// this connector retains for carrying one packet to this counterparty,
    /// realized on the wire as the difference between the amount that
    /// arrived and the amount forwarded, and added to the accumulated cost
    /// of a reject this peer itself decided on (ADR 0011).
    ///
    /// Flat, per packet, and independent of the amount carried. It attaches
    /// here rather than to a `[[routes]]` entry because this hop does the
    /// same work whichever prefix the packet was addressed to (ADR 0061);
    /// `[[routes]] fee` is a refuse-to-start tombstone
    /// ([`crate::ConfigError::RouteFeeRemoved`]).
    ///
    /// Defaults to zero -- free carriage. Unlike
    /// [`Self::max_packet_amount`], which exists to bound a loss, a fee
    /// bounds nothing, so "the operator wrote no number" can safely mean
    /// "charge nothing" here.
    pub fn fee(&self) -> u64 {
        self.fee
    }
}

/// Validate every `[[peers]]` entry against `expose`, the carriages this
/// connector opens listeners for.
///
/// `expose` is an argument rather than a field on each peer because it is a
/// property of *this connector*, not of any one peering: the whole point of
/// §2.1 is that the two axes are independent, and the load-time checks that
/// matter -- [`ConfigError::PeerUndialable`] and, via
/// [`PeerConfig::can_originate`], [`ConfigError::PeerRouteUndeliverable`]
/// -- are exactly the ones that need both axes at once.
///
/// `allow_plaintext` is issue #678's loopback opt-in
/// (`peer_allow_plaintext_endpoints`): `false` -- the default and every
/// production config -- keeps `ws://` and `http://` a hard
/// [`ConfigError::PeerEndpointScheme`], exactly as before the switch
/// existed.
pub(crate) fn resolve_peers(
    raw: Vec<RawPeer>,
    expose: PeerExposure,
    allow_plaintext: bool,
) -> Result<Vec<PeerConfig>, ConfigError> {
    let mut seen = HashSet::with_capacity(raw.len());
    let mut peers = Vec::with_capacity(raw.len());

    for peer in raw {
        if peer.id.trim().is_empty() {
            return Err(ConfigError::PeerIdEmpty);
        }
        if peer.addr.is_some() {
            return Err(ConfigError::PeerAddrRemoved { id: peer.id });
        }
        if peer.ceiling.is_some() {
            return Err(ConfigError::PeerCeilingRemoved { id: peer.id });
        }
        if peer.flush_interval_ms.is_some() {
            return Err(ConfigError::PeerFlushIntervalRemoved { id: peer.id });
        }
        // ADR 0042 item 4 (issue #1077): the B6 ramp is gone, so `"observe"`
        // no longer names a mode -- and `"enforce"` names the only behaviour
        // there is. Refused by name rather than ignored, because a config
        // still writing `"observe"` was written by an operator who believes
        // this peering admits uncovered arrivals, and it does not.
        if peer.claim_enforcement.is_some() {
            return Err(ConfigError::PeerClaimEnforcementRemoved { id: peer.id });
        }
        // ADR 0060 (issue #1157): the `{peerId, secret}` bearer credential
        // is deleted, not renamed. Refused by name rather than ignored,
        // because an operator who still writes one believes this peering is
        // authenticated by it, and a claim signature is what authenticates
        // it now.
        if peer.credential.is_some() {
            return Err(ConfigError::PeerCredentialRemoved { id: peer.id });
        }
        if !seen.insert(peer.id.clone()) {
            return Err(ConfigError::DuplicatePeerId { id: peer.id });
        }

        let (endpoint, dial) = match peer.endpoint {
            None => (None, None),
            Some(value) => {
                let url =
                    Url::parse(&value).map_err(|source| ConfigError::InvalidPeerEndpoint {
                        id: peer.id.clone(),
                        value: value.clone(),
                        source,
                    })?;
                let carriage =
                    PeerCarriage::for_endpoint(&url, allow_plaintext).ok_or_else(|| {
                        ConfigError::PeerEndpointScheme {
                            id: peer.id.clone(),
                            value: value.clone(),
                            scheme: url.scheme().to_string(),
                        }
                    })?;
                // No host check is needed: `wss` and `https` are both
                // *special* schemes in the URL standard, so a URL without
                // a host never parses in the first place and comes back
                // as `InvalidPeerEndpoint` above.
                (Some(url), Some(carriage))
            }
        };

        // ADR 0042 (item 3): a mistyped `forwarded_claim_enforcement` is
        // refused by name for the same reason its sibling above is, and the
        // stakes run the other way -- a typo meant as "enforce" that fell
        // through to the default would leave this peering carrying forwards
        // for free, silently, which is precisely the gap ADR 0042 closes.
        let forwarded_claim_enforcement = match peer.forwarded_claim_enforcement {
            None => ForwardedClaimEnforcement::default(),
            Some(value) => {
                value
                    .parse()
                    .map_err(|()| ConfigError::InvalidForwardedClaimEnforcement {
                        id: peer.id.clone(),
                        value,
                    })?
            }
        };

        // ADR 0042's cap. `0` is refused by name rather than taken
        // literally: a cap of zero refuses every packet this peering could
        // ever carry, which is a peering that silently does nothing, and
        // "I meant to disable the cap" is the likeliest thing a `0` was
        // written for. There is no disabling spelling -- the cap's whole
        // point is that a bound always exists.
        let max_packet_amount = match peer.max_packet_amount {
            None => DEFAULT_MAX_PACKET_AMOUNT,
            Some(0) => return Err(ConfigError::PeerMaxPacketAmountZero { id: peer.id }),
            Some(written) => written,
        };

        // A peering with nothing to dial, on a connector with nothing to
        // dial into, can never establish -- and no amount of retrying
        // changes that (§2.2).
        if endpoint.is_none() && expose.is_empty() {
            return Err(ConfigError::PeerUndialable { id: peer.id });
        }

        let can_originate = endpoint.is_some() || expose.exposes(PeerCarriage::Btp);

        peers.push(PeerConfig {
            id: peer.id,
            endpoint,
            dial,
            can_originate,
            claim_ack_timeout_ms: peer.claim_ack_timeout_ms.unwrap_or(DEFAULT_PEER_TIMEOUT_MS),
            peer_answer_timeout_ms: peer
                .peer_answer_timeout_ms
                .unwrap_or(DEFAULT_PEER_TIMEOUT_MS),
            forwarded_claim_enforcement,
            max_packet_amount,
            fee: peer.fee.unwrap_or(0),
        });
    }

    Ok(peers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(id: &str) -> RawPeer {
        RawPeer {
            id: id.to_string(),
            addr: None,
            endpoint: Some("wss://peer.example:443/btp".to_string()),
            credential: None,
            ceiling: None,
            flush_interval_ms: None,
            claim_ack_timeout_ms: None,
            peer_answer_timeout_ms: None,
            claim_enforcement: None,
            forwarded_claim_enforcement: None,
            max_packet_amount: None,
            fee: None,
        }
    }

    #[test]
    fn resolves_a_wss_peer_onto_the_btp_carriage() {
        let peers =
            resolve_peers(vec![raw("peer-b")], PeerExposure::Neither, false).expect("resolve");

        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].id(), "peer-b");
        assert_eq!(peers[0].dial(), Some(PeerCarriage::Btp));
        assert_eq!(
            peers[0].endpoint().map(Url::as_str),
            Some("wss://peer.example/btp")
        );
        assert!(peers[0].can_originate());
        assert_eq!(peers[0].claim_ack_timeout_ms(), 30_000);
        assert_eq!(peers[0].peer_answer_timeout_ms(), 30_000);
    }

    /// ADR 0042 item 4 (issue #1077): the B6 ramp is gone, so a config that
    /// still writes the key is refused **by name** rather than ignored. The
    /// `"observe"` spelling is the one that matters -- an operator running
    /// it believes this peering admits uncovered arrivals to a priced
    /// termination, and no build does that any more.
    #[test]
    fn claim_enforcement_observe_is_refused_as_a_removed_key() {
        let mut entry = raw("peer-b");
        entry.claim_enforcement = Some(toml::Value::String("observe".to_string()));

        assert!(matches!(
            resolve_peers(vec![entry], PeerExposure::Neither, false),
            Err(ConfigError::PeerClaimEnforcementRemoved { ref id }) if id == "peer-b"
        ));
    }

    /// The `"enforce"` spelling names what every build now does
    /// unconditionally -- and is refused just the same, because a key that
    /// is accepted for one value and rejected for another teaches an
    /// operator that the key still selects something.
    #[test]
    fn claim_enforcement_enforce_is_refused_as_a_removed_key_too() {
        let mut entry = raw("peer-b");
        entry.claim_enforcement = Some(toml::Value::String("enforce".to_string()));

        assert!(matches!(
            resolve_peers(vec![entry], PeerExposure::Neither, false),
            Err(ConfigError::PeerClaimEnforcementRemoved { ref id }) if id == "peer-b"
        ));
    }

    /// ADR 0042 (item 3), the fleet-safety property: a peer that writes
    /// nothing forwards exactly as it did before this knob existed --
    /// uncovered forwarded arrivals admitted and logged, never refused.
    /// Defaulting this the way the deleted `claim_enforcement` defaulted
    /// would have stopped forwarding across a fleet whose send halves are
    /// not live.
    #[test]
    fn forwarded_claim_enforcement_defaults_to_observe() {
        let peers =
            resolve_peers(vec![raw("peer-b")], PeerExposure::Neither, false).expect("resolve");

        assert_eq!(
            peers[0].forwarded_claim_enforcement(),
            ForwardedClaimEnforcement::Observe
        );
    }

    /// Keeping the two knobs separate is what let one be deleted without
    /// the other: a peering that writes neither field enforces the
    /// terminated rule unconditionally (ADR 0029, no knob left) while still
    /// only observing the forwarded one (ADR 0042). Had they been folded
    /// into one field, issue #1077 would have taken this default with it.
    #[test]
    fn deleting_the_terminated_knob_left_the_forwarded_default_permissive() {
        let peers =
            resolve_peers(vec![raw("peer-b")], PeerExposure::Neither, false).expect("resolve");

        assert_eq!(
            peers[0].forwarded_claim_enforcement(),
            ForwardedClaimEnforcement::Observe
        );
    }

    /// The flip an operator makes once this peering's counterparty covers
    /// its forwards.
    #[test]
    fn forwarded_claim_enforcement_enforce_is_parsed_by_name() {
        let mut entry = raw("peer-b");
        entry.forwarded_claim_enforcement = Some("enforce".to_string());

        let peers = resolve_peers(vec![entry], PeerExposure::Neither, false).expect("resolve");

        assert_eq!(
            peers[0].forwarded_claim_enforcement(),
            ForwardedClaimEnforcement::Enforce
        );
    }

    /// Writing the default out explicitly is the same as omitting it.
    #[test]
    fn forwarded_claim_enforcement_observe_is_parsed_by_name() {
        let mut entry = raw("peer-b");
        entry.forwarded_claim_enforcement = Some("observe".to_string());

        let peers = resolve_peers(vec![entry], PeerExposure::Neither, false).expect("resolve");

        assert_eq!(
            peers[0].forwarded_claim_enforcement(),
            ForwardedClaimEnforcement::Observe
        );
    }

    /// A mistyped value is refused by name. The stakes are the mirror image
    /// of the deleted `claim_enforcement`'s: here a typo meant as "enforce"
    /// would fall through to the permissive default and carry forwards for
    /// free.
    #[test]
    fn forwarded_claim_enforcement_refuses_an_unrecognized_spelling_by_name() {
        let mut entry = raw("peer-b");
        entry.forwarded_claim_enforcement = Some("enfroce".to_string());

        assert!(matches!(
            resolve_peers(vec![entry], PeerExposure::Neither, false),
            Err(ConfigError::InvalidForwardedClaimEnforcement { ref id, ref value })
                if id == "peer-b" && value == "enfroce"
        ));
    }

    /// ADR 0042: an operator who never writes a cap still gets one. This is
    /// the property the whole default exists for -- there is no spelling
    /// that leaves a peering unbounded.
    #[test]
    fn a_peer_that_configures_no_cap_still_gets_the_default_one() {
        let peers =
            resolve_peers(vec![raw("peer-b")], PeerExposure::Neither, false).expect("resolve");

        assert_eq!(peers[0].max_packet_amount(), DEFAULT_MAX_PACKET_AMOUNT);
        assert_eq!(peers[0].max_packet_amount(), 1_000_000);
    }

    /// The cap is per peering, so two rows in one file hold two different
    /// caps -- how far this connector trusts each peer, separately.
    #[test]
    fn each_peering_carries_its_own_cap() {
        let mut tight = raw("peer-tight");
        tight.max_packet_amount = Some(50);
        let mut generous = raw("peer-generous");
        generous.max_packet_amount = Some(5_000_000);

        let peers =
            resolve_peers(vec![tight, generous], PeerExposure::Neither, false).expect("resolve");

        assert_eq!(peers[0].max_packet_amount(), 50);
        assert_eq!(peers[1].max_packet_amount(), 5_000_000);
    }

    /// ADR 0061: a peering that writes no fee carries for free. Unlike the
    /// cap, a fee bounds nothing, so an unwritten one can safely mean
    /// "charge nothing" -- and it is what every config in this tree that
    /// never wrote a `[[routes]] fee` already meant.
    #[test]
    fn a_peering_that_configures_no_fee_carries_for_free() {
        let peers =
            resolve_peers(vec![raw("peer-b")], PeerExposure::Neither, false).expect("resolve");

        assert_eq!(peers[0].fee(), 0);
    }

    /// The fee is per peering, so two rows in one file hold two different
    /// fees -- what this connector charges to carry to each counterparty,
    /// separately. It is emphatically not per prefix: that is the whole of
    /// ADR 0061, and why `[[routes]] fee` is now a tombstone.
    #[test]
    fn each_peering_carries_its_own_fee() {
        let mut cheap = raw("peer-cheap");
        cheap.fee = Some(50);
        let mut dear = raw("peer-dear");
        dear.fee = Some(100);

        let peers =
            resolve_peers(vec![cheap, dear], PeerExposure::Neither, false).expect("resolve");

        assert_eq!(peers[0].fee(), 50);
        assert_eq!(peers[1].fee(), 100);
    }

    /// `fee = 0` written down is free carriage the operator chose, and
    /// resolves exactly like the unwritten case: there is nothing to refuse
    /// here, unlike a cap of zero below.
    #[test]
    fn a_fee_of_zero_is_deliberate_free_carriage() {
        let mut entry = raw("peer-b");
        entry.fee = Some(0);

        let peers = resolve_peers(vec![entry], PeerExposure::Neither, false).expect("resolve");
        assert_eq!(peers[0].fee(), 0);
    }

    /// `0` is refused by name: it would refuse every packet the peering
    /// could carry, and it is what someone reaching for a non-existent
    /// "disable the cap" spelling would write.
    #[test]
    fn a_cap_of_zero_is_refused_by_name() {
        let mut entry = raw("peer-b");
        entry.max_packet_amount = Some(0);

        assert!(matches!(
            resolve_peers(vec![entry], PeerExposure::Neither, false),
            Err(ConfigError::PeerMaxPacketAmountZero { ref id }) if id == "peer-b"
        ));
    }

    #[test]
    fn resolves_an_https_peer_onto_the_http_carriage() {
        let mut entry = raw("peer-b");
        entry.endpoint = Some("https://peer.example/ilp".to_string());

        let peers = resolve_peers(vec![entry], PeerExposure::Neither, false).expect("resolve");

        assert_eq!(peers[0].dial(), Some(PeerCarriage::Http));
    }

    #[test]
    fn rejects_an_empty_id() {
        let mut entry = raw("peer-b");
        entry.id = "  ".to_string();

        assert!(matches!(
            resolve_peers(vec![entry], PeerExposure::Both, false),
            Err(ConfigError::PeerIdEmpty)
        ));
    }

    /// An accept-only peering on a connector that exposes BTP can still be
    /// originated to: the peer dials in and the session is symmetric.
    #[test]
    fn an_accept_only_peer_can_be_originated_to_when_btp_is_exposed() {
        let mut entry = raw("peer-b");
        entry.endpoint = None;

        let peers = resolve_peers(vec![entry], PeerExposure::Btp, false).expect("resolve");

        assert_eq!(peers[0].dial(), None);
        assert!(peers[0].can_originate());
    }

    /// The one shape that cannot: accept-only, and this connector exposes
    /// only HTTP, so the peer's own requests are the only direction there
    /// is.
    #[test]
    fn an_accept_only_peer_cannot_be_originated_to_over_http_only() {
        let mut entry = raw("peer-b");
        entry.endpoint = None;

        let peers = resolve_peers(vec![entry], PeerExposure::Http, false).expect("resolve");

        assert!(!peers[0].can_originate());
    }

    /// §11's removed-field row, `ceiling`/`flush_interval_ms` half (ADR
    /// 0031, ADR 0033, issue #882): a stale bind-mounted box config gets a
    /// named error, not a silent drop.
    #[test]
    fn setting_ceiling_is_refused_by_name() {
        let mut entry = raw("peer-b");
        entry.ceiling = Some(toml::Value::Integer(1_000));

        assert!(matches!(
            resolve_peers(vec![entry], PeerExposure::Neither, false),
            Err(ConfigError::PeerCeilingRemoved { ref id }) if id == "peer-b"
        ));
    }

    #[test]
    fn setting_flush_interval_ms_is_refused_by_name() {
        let mut entry = raw("peer-b");
        entry.flush_interval_ms = Some(toml::Value::Integer(5_000));

        assert!(matches!(
            resolve_peers(vec![entry], PeerExposure::Neither, false),
            Err(ConfigError::PeerFlushIntervalRemoved { ref id }) if id == "peer-b"
        ));
    }

    /// ADR 0060: the peering secret is deleted, not renamed. A config
    /// that still writes one is refused **by name** -- never dropped and
    /// never ignored -- because an operator who wrote it believes this
    /// peering is authenticated by it, and a verified claim is what
    /// authenticates it now.
    #[test]
    fn a_credential_is_refused_as_a_removed_key() {
        for written in [
            toml::Value::Table(toml::map::Map::new()),
            toml::Value::String("shared-secret".to_string()),
            toml::Value::Table({
                let mut table = toml::map::Map::new();
                table.insert(
                    "secret".to_string(),
                    toml::Value::String("shared-secret".to_string()),
                );
                table
            }),
            toml::Value::Table({
                let mut table = toml::map::Map::new();
                table.insert(
                    "secret_file".to_string(),
                    toml::Value::String("/app/data/peer.secret".to_string()),
                );
                table
            }),
        ] {
            let mut entry = raw("peer-b");
            entry.credential = Some(written.clone());

            assert!(
                matches!(
                    resolve_peers(vec![entry], PeerExposure::Neither, false),
                    Err(ConfigError::PeerCredentialRemoved { ref id }) if id == "peer-b"
                ),
                "credential = {written:?} should be refused by name"
            );
        }
    }

    #[test]
    fn exposure_parses_all_four_spellings_and_defaults_to_neither() {
        assert_eq!(
            parse_peer_exposure(None).expect("default"),
            PeerExposure::Neither
        );
        for (written, expected) in [
            ("neither", PeerExposure::Neither),
            ("btp", PeerExposure::Btp),
            ("http", PeerExposure::Http),
            ("both", PeerExposure::Both),
        ] {
            assert_eq!(
                parse_peer_exposure(Some(written.to_string())).expect("parse"),
                expected
            );
        }
    }

    #[test]
    fn exposure_refuses_an_unrecognized_spelling_by_name() {
        let result = parse_peer_exposure(Some("carrier-pigeon".to_string()));

        assert!(matches!(
            result,
            Err(ConfigError::InvalidPeerExposure { ref value }) if value == "carrier-pigeon"
        ));
    }

    #[test]
    fn exposure_answers_the_intersection_questions() {
        assert!(PeerExposure::Both.exposes(PeerCarriage::Btp));
        assert!(PeerExposure::Both.exposes(PeerCarriage::Http));
        assert!(PeerExposure::Btp.exposes(PeerCarriage::Btp));
        assert!(!PeerExposure::Btp.exposes(PeerCarriage::Http));
        assert!(!PeerExposure::Neither.exposes(PeerCarriage::Btp));
        assert!(PeerExposure::Neither.is_empty());
        assert!(!PeerExposure::Http.is_empty());
    }
}
