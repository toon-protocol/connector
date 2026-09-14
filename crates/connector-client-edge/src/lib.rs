//! Client-edge router, mountable rather than a server. See ADR 0001, ADR
//! 0003, and `docs/protocol/client-edge-spec.md` -- this implements §1.1
//! (transport and framing: `POST /ilp`, OER-encoded PREPARE in, OER-encoded
//! FULFILL/REJECT out, always HTTP 200 for an ILP-level outcome), all four
//! steps of §1.3 (payment claims, issues #504, #522 and #506/#544): a
//! present claim is parsed, structurally validated, checked for
//! freshness/watermark, checked to advance value by at least the
//! destination's matched app route's price, and -- last -- cryptographically
//! verified against the counterparty this connector records for the channel
//! the claim names (`ClientClaimGate` over a `ClientChannelRegistry`, issue
//! #558 -- a claim's own declared signer carries no authority, and a claim
//! naming an unrecorded channel is refused outright),
//! all before the packet is routed; and, as of issue #526, §1.4 (the x402
//! greeting) and the answering half of identity: `GET /ilp/identity`
//! reports the public key a sender seals a packet to (ADR 0018), and an
//! unpaid request to a route this connector terminates and prices is
//! answered with that route's terms (ADR 0020, ADR 0022) instead of being
//! routed at all. `GET /ilp/routes/price` answers what a given destination
//! would cost. Both live under `/ilp` (matching §3.2's `GET /ilp/versions`
//! precedent for an unauthenticated, client-edge-facing answer) rather than
//! at the bare path, since the operator surface already owns a bearer-gated
//! `GET /identity` of its own (issue #420) and the two routers are merged
//! onto one port whenever the operator surface is enabled. None of this
//! pushes anything into a network unprompted (ADR 0022) -- each is a reply
//! to a request that reached this connector's own client edge, and changes
//! no state. A request presenting no claim header and addressing an
//! unpriced (or unmatched) destination still passes through unchanged,
//! exactly as it always has, and pay-to-write is absolute for a priced
//! route -- there is no configuration, flag or build profile that disables
//! any of §1.3's checks. Identity (§1.2, beyond the key answer) remains
//! unimplemented.
//!
//! As of issue #548 this edge also implements §1.6, cost discovery: every
//! REJECT it answers with carries the running cost total in a
//! `TOON-Accumulated-Cost` header beside the unchanged OER body (ADR 0011 --
//! a reject reports what the path would have charged, rather than a quoting
//! protocol answering about a path the packet never took), and `POST
//! /ilp/probe` is the ingress a sender uses to raise one deliberately,
//! gated by [`Connector::handle_probe`]'s channel and rate-limit checks.
//!
//! Per ADR 0001, `handle_ilp` deserializes, calls exactly one method on
//! [`Connector`], and serializes; the `match` below is that serialization
//! step, not a routing or delivery decision -- those live entirely in
//! [`Connector::handle_prepare`]. `identity` and `route_price` answer
//! directly from this connector's own configuration and never touch
//! [`Connector::handle_prepare`] at all.
//!
//! As of issue #693, `POST /ilp/claim-state` (§1.10, `claim_state` module)
//! answers a bulk, owner-authenticated read of claim state -- deposit
//! total, cumulative claimed, available balance, nonce, last-claim time --
//! for every channel a caller can prove it controls with a per-channel
//! signature over a domain-separated challenge, distinct from a real
//! claim's signature. Also purely a read against existing state
//! ([`ClientClaimGate::watermark`], [`ClientClaimGate::channels`],
//! [`ClientClaimGate::last_claim_time`] -- plus, for a channel this node
//! holds as a `[[peer_channels]]` row,
//! [`connector_runtime::Connector::peer_channel_watermark`], because that
//! is the book judging claims on it, issue #1102); it never calls
//! `ingest`/`admit`,
//! so it adds nothing to the packet admission path #686/#688/#690 spent
//! this edge's history keeping cheap.

use std::num::NonZeroU32;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};

use connector_config::TransportPolicy;
use connector_domain::client_claim::ClientClaim;
use connector_domain::identity::{anonymous_identity, resolve_identity, ConfiguredIdentity};
use connector_domain::{
    agreed_required_transport, EdgeIdentity, NodeFacts, NodeSelfDescription, PacketResponse,
    Prepare, Price, Reject, RejectCode, RoutePrice,
};
use connector_runtime::{ClientRouteFacts, ClientRouteKind, Connector, ProbeDenied};
use connector_signer::nip59::{unwrap_claim, WrappedClaim};
use connector_signer::{PublicKeyBytes, Signer};

mod btp;
mod channels;
mod claim_gate;
mod claim_state;
mod lookup_budget;
mod outbound_ledger;
mod peer;
mod session_registry;
mod session_route;
pub use channels::{
    ChannelLivenessPolicy, ChannelLookupFailed, ChannelResolutionError, ClientChannelRegistry,
    ClientChannelSource, DepositFloor, EvmChannel, InvalidChannelIdentifier, SolanaChannel,
    DEFAULT_LIVENESS_TTL, DEFAULT_MIN_REATTEMPT_INTERVAL, DEFAULT_SERVE_STALE_UNTIL,
};
pub use claim_gate::{ClaimIngestRejection, ClientClaimGate};
pub use lookup_budget::{
    LookupBudgetBound, LookupBudgetExhausted, UnresolvableLookupBudget,
    UnresolvableLookupBudgetPolicy, DEFAULT_UNRESOLVABLE_LOOKUPS_PER_SIGNER,
    DEFAULT_UNRESOLVABLE_LOOKUPS_TOTAL, DEFAULT_UNRESOLVABLE_LOOKUP_MAX_WAIT,
    DEFAULT_UNRESOLVABLE_LOOKUP_WINDOW, MAX_UNRESOLVABLE_LOOKUP_WINDOW,
};
pub use outbound_ledger::ClientPayoutLedger;
pub use peer::PeerCarriages;
pub use session_registry::SESSION_LEASE_BACKSTOP_TTL;

/// The BTP carriage's default per-session in-flight window (issue #688):
/// how many of one session's frames may be past claim admission at once.
/// `16` clears the measured ~125-150 ev/s per-session wall by an order of
/// magnitude on a ~7 ms downstream (window/latency ≈ 2 000/s) while keeping
/// a session's queued work bounded and modest; the config file's
/// `btp_session_window` overrides it.
pub const DEFAULT_BTP_SESSION_WINDOW: NonZeroU32 = match NonZeroU32::new(16) {
    Some(window) => window,
    None => unreachable!(),
};

const OCTET_STREAM: &str = "application/octet-stream";
/// The one declaration, read rather than re-spelled (spec I2): the peer
/// carriage's HTTP claim header *is* this one, because ADR 0027 reuses the
/// claim carriage verbatim rather than minting a second decoder
/// (`peer-carriage-spec.md` §12.1).
const CLAIM_HEADER: &str = connector_btp::CLAIM_HEADER;
const CLAIM_WRAPPED_HEADER: &str = "ilp-payment-channel-claim-wrapped";
/// The other half of the pair (spec I2), read rather than re-spelled for the
/// same reason [`CLAIM_HEADER`] is: the peer carriage answers an uncovered
/// peer PREPARE with this edge's own greeting under this same name (issue
/// #880), so one declaration serves both.
const PAYMENT_REQUIRED_HEADER: &str = connector_btp::PAYMENT_REQUIRED_HEADER;
/// client-edge-spec.md §1.6: a REJECT's running cost total rides beside the
/// OER body in this header rather than inside it, since RFC-0027's REJECT
/// `data` is reserved for an application-level reject's own diagnostic
/// payload. Decimal `uint64`, present on every REJECT this edge answers
/// with (issue #548).
const ACCUMULATED_COST_HEADER: &str = connector_btp::ACCUMULATED_COST_HEADER;
/// client-edge-spec.md §1.2: a configured peer names itself with this
/// header. Its absence is what makes a request anonymous -- a first-class
/// path, not a fallback.
const PEER_ID_HEADER: &str = "ilp-peer-id";

struct ClientEdgeState {
    connector: Arc<Connector>,
    signer: Arc<dyn Signer>,
    claim_gate: Arc<ClientClaimGate>,
    /// This connector's own NIP-59 receiver key, used to unwrap a
    /// privacy-wrapped claim (client-edge-spec.md §1.3). `None` means this
    /// instance is not configured to receive wrapped claims -- one is
    /// refused with [`ClaimIngestRejection::WrapUnsupported`] rather than
    /// silently accepted unwrapped or left to panic.
    wrap_receiver_secret: Option<[u8; 32]>,
    /// This node's own facts (ADR 0050): its ILP addresses, its public
    /// endpoints, the peer carriages it exposes, and the channel-opening
    /// facts every settlement backend proved against a chain at startup.
    ///
    /// **One value, two projections.** `GET /ilp` serves it as this node's
    /// self-description; every x402 greeting reads its `extra` node facts off
    /// the same value (ND-11). Nothing assembles either set a second time, so
    /// the two cannot disagree -- which is the structural end of the
    /// `requiredTransport` defect that came of describing one node in two
    /// places. Default -- every field empty -- is a node with no `[node]`
    /// section and no settlement backend: it still answers, and its
    /// description simply says less.
    node: Arc<NodeFacts>,
    /// How many of one BTP session's frames may be past claim admission --
    /// waiting out the journal's group commit, being routed downstream, or
    /// answering -- at once (issue #688). Claims themselves are still
    /// judged strictly in arrival order; this bounds only the overlapped
    /// tail. See `btp::btp_session`.
    btp_session_window: NonZeroU32,
    /// The client session registry (issue #698, toon-meta#262 decision
    /// 12): which address's BTP session is live right now, fenced by a
    /// monotonic generation so a reconnect can never be raced by the
    /// session it replaced. Shared by every session `btp::btp_session`
    /// serves -- bound at auth, cleared on close -- so a reconnect on a
    /// different socket is visible to every other session immediately.
    session_registry: Arc<session_registry::SessionRegistry>,
    /// The peer carriages this node exposes (issue #678, ADR 0027,
    /// `peer-carriage-spec.md` §1). `None` -- the default, and every node
    /// whose `peer_expose` is `"neither"` -- means this edge serves clients
    /// only and every interaction on it is a client's, exactly as it was
    /// before the carriages existed.
    ///
    /// Peer traffic rides *these* listeners rather than a second socket
    /// (`docs/operators/btp-peer-transport-bringup.md`), so this field is
    /// what makes `POST /ilp` and `GET /ilp/btp` serve two audiences; §1.3
    /// forbids the listener itself deciding which, and it does not --
    /// `peer::PeerCarriages` decides by credential alone.
    peers: Option<Arc<PeerCarriages>>,
    /// The client-edge identities this node authenticates over HTTP (issue
    /// #502, client-edge-spec.md §1.2): what an `ILP-Peer-Id` header must
    /// name, and the secret its `Authorization: Bearer <secret>` must
    /// match, to be recognised as that identity rather than refused `401`.
    /// Empty -- the default -- means this node configures no peer
    /// identity: every presented `ILP-Peer-Id` fails to authenticate, and
    /// every request that presents none is anonymous, which stays a
    /// first-class path either way.
    identities: Arc<[ConfiguredIdentity]>,
}

/// Mount the client edge at `connector`, signing/answering identity with
/// `signer`: `POST /ilp` per `docs/protocol/client-edge-spec.md` §1.1, plus
/// `GET /ilp/identity` and `GET /ilp/routes/price` (§1.2/§1.4, issue #526),
/// with no configured NIP-59 receiver key -- a privacy-wrapped claim is
/// refused rather than accepted -- and a record of no payment channel at
/// all, so every claim presented to it is refused as
/// [`ClaimIngestRejection::UnknownChannel`] (issue #558). Use
/// [`router_with_wrap_key`] to accept wrapped claims, and
/// [`router_with_gate`] to give this edge the channels whose claims it
/// should accept.
pub fn router(connector: Arc<Connector>, signer: Arc<dyn Signer>) -> Router {
    router_with_wrap_key(connector, signer, None)
}

/// As [`router`], but able to unwrap a privacy-wrapped claim
/// (client-edge-spec.md §1.3) using `wrap_receiver_secret` as this
/// connector's own NIP-59 receiver key.
pub fn router_with_wrap_key(
    connector: Arc<Connector>,
    signer: Arc<dyn Signer>,
    wrap_receiver_secret: Option<[u8; 32]>,
) -> Router {
    // A gate with a record of no channel accepts nothing, so it has no
    // watermark a restart could lose and needs no durable journal (issue
    // #605). A node that means to accept claims goes through
    // `router_with_gate` and supplies a gate built over a real one.
    let gate = ClientClaimGate::restore(
        ClientChannelRegistry::new(),
        Arc::new(connector_runtime::InMemoryJournal::new()),
    )
    .expect("a fresh in-memory journal has nothing to replay");
    router_with_gate(connector, signer, wrap_receiver_secret, gate)
}

/// As [`router_with_wrap_key`], but with a fully built [`ClientClaimGate`]
/// -- the channels whose claims this edge accepts (issue #558) *and* the
/// durable journal their watermarks survive a restart in (issue #605).
///
/// The gate is passed in rather than assembled here on purpose: building
/// one can fail (a journal that will not replay), and that failure has to
/// stop the node starting, which a function returning a [`Router`] cannot
/// do. This is also the seam a node's startup arming (issue #556)
/// populates: the [`ClientChannelRegistry`] the gate was built over
/// carries both sources of a channel's record -- whatever the node
/// declared (`[[client_channels]]`) and, optionally, a
/// [`ClientChannelSource`] resolving anything else against the chain
/// ([`ClientChannelRegistry::with_source`]), which is what lets an
/// unaffiliated buyer who has opened a channel on chain pay without the
/// operator editing config first (issue #502). A registry with neither
/// refuses every claim.
pub fn router_with_gate(
    connector: Arc<Connector>,
    signer: Arc<dyn Signer>,
    wrap_receiver_secret: Option<[u8; 32]>,
    claim_gate: ClientClaimGate,
) -> Router {
    router_with_gate_and_terms(
        connector,
        signer,
        wrap_receiver_secret,
        claim_gate,
        NodeFacts::default(),
    )
}

/// As [`router_with_gate`], but also naming the client-edge identities this
/// node authenticates over HTTP (issue #502, client-edge-spec.md §1.2).
/// `identities` empty is [`router_with_gate`] exactly -- every request is
/// anonymous or, if it presents an `ILP-Peer-Id`, refused `401`.
pub fn router_with_identities(
    connector: Arc<Connector>,
    signer: Arc<dyn Signer>,
    wrap_receiver_secret: Option<[u8; 32]>,
    claim_gate: ClientClaimGate,
    identities: Arc<[ConfiguredIdentity]>,
) -> Router {
    router_with_node_facts(
        connector,
        signer,
        wrap_receiver_secret,
        Arc::new(claim_gate),
        NodeFacts::default(),
        DEFAULT_BTP_SESSION_WINDOW,
        None,
        identities,
    )
}

/// As [`router_with_gate`], but with this node's own facts (ADR 0050): the
/// channel-opening terms every settlement backend proved against a chain at
/// startup, and the addresses and endpoints it cannot introspect.
///
/// [`NodeFacts::default()`] -- the plain [`router_with_gate`] -- is a node
/// with no `[node]` section and no settlement backend, whose greeting keeps
/// its pre-#617 shape exactly.
pub fn router_with_gate_and_terms(
    connector: Arc<Connector>,
    signer: Arc<dyn Signer>,
    wrap_receiver_secret: Option<[u8; 32]>,
    claim_gate: ClientClaimGate,
    node: NodeFacts,
) -> Router {
    router_with_gate_terms_and_btp_window(
        connector,
        signer,
        wrap_receiver_secret,
        claim_gate,
        node,
        DEFAULT_BTP_SESSION_WINDOW,
    )
}

/// As [`router_with_gate_and_terms`], but naming the BTP carriage's
/// per-session in-flight window (issue #688): how many of one session's
/// frames may be past claim admission -- waiting out the journal's group
/// commit, being routed downstream, answering -- at once. Claims are still
/// judged strictly in arrival order whatever the window; `1` reproduces the
/// original lockstep session exactly. `NonZeroU32` because a window of `0`
/// is not a slower configuration, it is a session whose first paid frame
/// waits forever -- the config layer refuses it before this signature is
/// ever reached (`btp_session_window = 0`), and the type refuses it here
/// for every caller that skips the config layer.
pub fn router_with_gate_terms_and_btp_window(
    connector: Arc<Connector>,
    signer: Arc<dyn Signer>,
    wrap_receiver_secret: Option<[u8; 32]>,
    claim_gate: ClientClaimGate,
    node: NodeFacts,
    btp_session_window: NonZeroU32,
) -> Router {
    router_with_peer_carriages(
        connector,
        signer,
        wrap_receiver_secret,
        claim_gate,
        node,
        btp_session_window,
        None,
    )
}

/// As [`router_with_gate_terms_and_btp_window`], but also mounting this
/// node's **peer carriages** on the listeners it already serves (issue
/// #678, ADR 0027).
///
/// `peers` is [`PeerCarriages::from_config`]'s value -- `None` for a node
/// whose `peer_expose` is `"neither"` or that configures no peering, which
/// is every node this router served before the carriages existed and is
/// byte-identical to what it served then.
///
/// There is no second listener and no second port
/// (`docs/operators/btp-peer-transport-bringup.md`: peer carriages *"ride
/// this node's own listeners"*). A peer PREPARE is the same OER encoding
/// `POST /ilp` already carries (`peer-carriage-spec.md` §3.1), and what
/// tells a peer interaction from a client one is
/// [`connector_peer_auth::decide_role`] -- never the carriage, the
/// listener, the port or the bind address (§1.3).
#[allow(clippy::too_many_arguments)]
pub fn router_with_peer_carriages(
    connector: Arc<Connector>,
    signer: Arc<dyn Signer>,
    wrap_receiver_secret: Option<[u8; 32]>,
    claim_gate: ClientClaimGate,
    node: NodeFacts,
    btp_session_window: NonZeroU32,
    peers: Option<Arc<PeerCarriages>>,
) -> Router {
    router_with_node_facts(
        connector,
        signer,
        wrap_receiver_secret,
        Arc::new(claim_gate),
        node,
        btp_session_window,
        peers,
        Arc::from([]),
    )
}

/// As [`router_with_peer_carriages`], but also naming the client-edge
/// identities this node authenticates over HTTP -- the full form every other
/// constructor above delegates to.
///
/// `node` is the value both `GET /ilp` and every x402 greeting read this
/// node's own facts off (ADR 0050, ND-11). Its three configured fields come
/// from `[node]` -- the section that holds exactly what a node cannot
/// introspect about itself -- and its settlement terms from the backends that
/// verified them against a chain at startup. `identities` is
/// `connector_config::Config::client_identities`'s value (issue #502): every
/// `ILP-Peer-Id` this node recognises and the secret it authenticates with.
#[allow(clippy::too_many_arguments)]
pub fn router_with_node_facts(
    connector: Arc<Connector>,
    signer: Arc<dyn Signer>,
    wrap_receiver_secret: Option<[u8; 32]>,
    claim_gate: Arc<ClientClaimGate>,
    node: NodeFacts,
    btp_session_window: NonZeroU32,
    peers: Option<Arc<PeerCarriages>>,
    identities: Arc<[ConfiguredIdentity]>,
) -> Router {
    let state = Arc::new(ClientEdgeState {
        connector,
        signer,
        claim_gate,
        wrap_receiver_secret,
        node: Arc::new(node),
        btp_session_window,
        session_registry: Arc::new(session_registry::SessionRegistry::new()),
        peers,
        identities,
    });
    Router::new()
        // ADR 0050: `GET` on this connector's own URL is its
        // self-description, `POST` on the same URL is still the packet
        // endpoint. One address, two methods, and **never a `POST` for the
        // document** (ND-03) -- a self-description endpoint that grows a
        // write is purchasable peering through a side door (ADR 0043).
        .route("/ilp", post(handle_ilp).get(self_description))
        .route("/ilp/btp", get(btp::handle_btp_upgrade))
        .route("/ilp/probe", post(handle_probe))
        .route("/ilp/identity", get(identity))
        .route("/ilp/routes/price", get(route_price))
        .route("/ilp/claim-state", post(claim_state::claim_state))
        .with_state(state)
}

/// The `ILP-Payment-Channel-Claim-Wrapped` header's JSON shape
/// (client-edge-spec.md §1.3): `base64(NIP-59-wrapped claim)`. `version` and
/// `timestamp` ride the wire but are not this ticket's concern -- carried
/// only so the shape round-trips; wrap/unwrap cares about the other two
/// fields alone.
#[derive(Deserialize)]
struct WrappedClaimEnvelope {
    #[serde(rename = "ephemeralPublicKey")]
    ephemeral_public_key: String,
    #[serde(rename = "encryptedPayload")]
    encrypted_payload: String,
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Wall-clock unix seconds, for [`ClientClaimGate::note_claim_time`] -- a
/// carrier noting a claim's acceptance *after* it has already happened.
/// Not consulted by admission itself (`ingest`/`admit` take no time input
/// at all). The other production wall-clock read in this crate is
/// `crate::session_route::credit_session_earnings`'s `chrono::Utc::now()`,
/// which needs a `DateTime<Utc>` this function does not produce.
pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// This connector's own identity (ADR 0018, ADR 0022): the uncompressed
/// secp256k1 public key a sender must seal a packet's payload to before it
/// can address this connector, plus the key id identifying it. Unlike the
/// operator surface's own bearer-gated `GET /identity` (issue #420, a
/// different audience per ADR 0022) -- mounted at `/ilp/identity` so the
/// two cannot collide when both routers merge onto one port -- this one is
/// unauthenticated -- an unaffiliated sender who has never registered with this connector's
/// operator still needs this key before it can form a packet at all, and
/// answering it decides nothing and reaches nobody who did not ask.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ClientEdgeIdentity {
    #[serde(rename = "keyId")]
    key_id: String,
    #[serde(rename = "publicKey")]
    public_key: String,
}

async fn identity(State(state): State<Arc<ClientEdgeState>>) -> Response {
    match state.signer.public_key() {
        Ok(public_key) => Json(ClientEdgeIdentity {
            key_id: state.signer.key_id(),
            public_key: format!("0x{}", hex_encode(&public_key)),
        })
        .into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

/// `GET /ilp` -- this connector's **self-description** (ADR 0050,
/// `docs/protocol/self-description-spec.md`).
///
/// The facts a stranger needs to transact with this node, as one document,
/// with no ILP packet, no encoder and no protocol knowledge required. A
/// stranger who has this node's URL at all therefore has its description,
/// with nothing further to discover -- which is why the address is this exact
/// URL and not a sub-resource that would itself have to be found (ND-01).
///
/// **Free and unauthenticated** (ND-02): this is what ADR 0022 already means
/// by *answering* -- it decides nothing, changes nothing, and reaches nobody
/// who did not ask. **No caching contract and no TTL** (ND-04): the answer is
/// projected from live state on each request. A `ttl_secs` existed because a
/// *pushed* copy needed a shelf life; a pulled one does not.
///
/// **There is no `POST` here, and there never will be** (ND-03). This
/// endpoint publishes what an operator needs to configure a peering out of
/// band; a peering is created by an operator in the config file or through the
/// operator surface and by nothing else (ADR 0043, ADR 0006). `POST /ilp` is
/// the packet endpoint and is untouched.
///
/// # What is in it, and where each fact comes from
///
/// * addresses and public endpoints -- `[node]`, because a container sees
///   `0.0.0.0:4000` and can never learn its own public name;
/// * the peer carriages this node exposes -- `peer_expose`. **Which**
///   carriages exist, never **who** rides them (ND-09);
/// * the **edge identity**, read from the signer on every request. ND-06
///   makes this mandatory: a sender that cannot seal to a route's terminating
///   connector can never have a packet delivered there (ADR 0018);
/// * per chain, the channel-opening facts -- from the settlement backends,
///   each of which verified them against a live chain before this node agreed
///   to boot. Never separately declared (ND-07);
/// * route prices, from the same `client_route` lookup the greeting and the
///   claim gate charge from, so a buyer who reads this and one who is billed
///   see one number;
/// * `requiredTransport`, when the routes covering this node's own addresses
///   agree on one;
/// * the client-edge versions served (issue #1054).
///
/// # Rate limiting
///
/// Through the shaper that already guards unresolvable chain lookups
/// ([`UnresolvableLookupBudget`]) rather than a mechanism of its own, per
/// ADR 0050. One consequence is worth stating rather than discovering: the
/// node-wide bucket is **shared** with channel resolution, so a flood of
/// description requests shapes chain lookups too and vice versa. That is the
/// point of reusing it -- one bucket is one thing to size -- and the shaper
/// *waits* rather than dropping, so a flood costs latency, never
/// availability. A request whose slot is further out than the policy's
/// `max_wait` is answered `429` rather than held.
///
/// The shaper's per-signer axis wants an identity and a `GET` has none: this
/// declares [`SELF_DESCRIPTION_REQUESTER`], so every description request
/// shares one per-signer bucket. That axis binds only once the node-wide
/// bucket is in arrears, so an uncontended node never refuses anyone.
async fn self_description(State(state): State<Arc<ClientEdgeState>>) -> Response {
    if let Err(exhausted) = state
        .claim_gate
        .lookup_budget()
        .reserve(SELF_DESCRIPTION_REQUESTER)
        .await
    {
        return (StatusCode::TOO_MANY_REQUESTS, exhausted.to_string()).into_response();
    }

    let edge_identity = state
        .signer
        .public_key()
        .ok()
        .map(|public_key| EdgeIdentity {
            key_id: state.signer.key_id(),
            public_key: format!("0x{}", hex_encode(&public_key)),
        });

    // Read from the live route table, not from a snapshot taken at boot: a
    // runtime route written through the operator surface has to show up in the
    // next answer, which is what ND-04's "generated from live configuration"
    // means in a node whose route table is not configuration alone.
    let routes = state
        .connector
        .client_route_prices()
        .into_iter()
        .map(|route| RoutePrice {
            prefix: route.prefix,
            price: route.price.base().to_string(),
            price_per_kib: (!route.price.is_flat()).then(|| route.price.per_kib().to_string()),
            request: route.request,
        })
        .collect();

    // Only the routes covering this node's OWN addresses are consulted, which
    // is the rule the retired announce used and the one the field means: a
    // per-node scalar can only honestly describe the routes the node is
    // addressed by.
    let required_transport = agreed_required_transport(
        state
            .node
            .ilp_addresses
            .iter()
            .filter_map(|address| state.connector.client_route(address))
            .map(|route| route.transport_policy.name()),
    );

    Json(NodeSelfDescription::describe(
        &state.node,
        edge_identity,
        routes,
        required_transport,
    ))
    .into_response()
}

/// What a self-description request is budgeted against on the shaper's
/// per-signer axis.
///
/// A `GET` carries no claim and so no declared signer. A constant puts every
/// description request in one bucket, which is the honest shape: they are one
/// kind of traffic, not one caller each. It cannot collide with a real
/// signer's key -- those are hex, this is not.
const SELF_DESCRIPTION_REQUESTER: &str = "self-description";

#[derive(Deserialize)]
struct RoutePriceQuery {
    destination: String,
}

/// What a given destination would cost to deliver to, per ADR 0022's
/// "a sender can ask what a route of its costs" -- reuses
/// [`Connector::client_route_price`], the same longest-prefix lookup the
/// x402 greeting and the claim gate's value binding already use, so this
/// answers with exactly the price a real request to `destination` would be
/// charged, never a second source of truth. That lookup spans configured
/// routes of both kinds since ADR 0028, so a destination this connector
/// forwards over a peering is answered here too -- it is charged here too.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct RoutePriceView {
    destination: String,
    price: u64,
    /// What each started kibibyte of payload adds (ADR 0065). Omitted on a
    /// flat route, so this answer is byte-identical to what it was before
    /// schedules existed -- `devnet-pricing.md`'s fleet check reads `price`
    /// and is unaffected.
    #[serde(default, skip_serializing_if = "is_zero")]
    price_per_kib: u64,
}

/// `skip_serializing_if` for a slope that is not there. A free function
/// rather than a closure because that attribute takes a path.
fn is_zero(value: &u64) -> bool {
    *value == 0
}

async fn route_price(
    State(state): State<Arc<ClientEdgeState>>,
    Query(query): Query<RoutePriceQuery>,
) -> Response {
    match state.connector.client_route_price(&query.destination) {
        Some(price) => Json(RoutePriceView {
            destination: query.destination,
            price: price.base(),
            price_per_kib: price.per_kib(),
        })
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            format!(
                "no route this connector serves matches '{}'",
                query.destination
            ),
        )
            .into_response(),
    }
}

/// The x402 v2 `payment-required` greeting's wire shape, re-exported from
/// [`connector_domain::x402`] where it lives as of issue #874. This edge
/// writes it (`x402_terms_body` below); the peer carriages -- which sit
/// under this crate in the graph and cannot import it -- read it back with
/// `connector_domain::x402::parse_greeting`. One definition, so an emitter
/// change cannot leave a reader behind.
///
/// Re-exported rather than merely imported because `X402SettlementTerms`,
/// `X402ChainSettlementTerms` and `X402SolanaSettlementTerms` are this
/// crate's public configuration surface -- `connector-cli` builds them at
/// startup and hands them to [`ClientEdgeState`] -- so the paths its
/// callers already use keep working.
pub use connector_domain::x402::{
    X402ChainSettlementTerms, X402ChannelExtra, X402PaymentOption, X402PaymentRequired,
    X402Resource, X402SettlementTerms, X402SolanaSettlementTerms, X402_VERSION,
};

/// Answer an unpaid request to `destination` with terms instead of doing
/// the work (client-edge-spec.md §1.4, ADR 0022) -- this changes no state
/// and is only ever a reply to the request that asked. Two shapes of
/// request reach this: `destination` names a route this connector serves
/// and prices at `price` ("Serves" spans both kinds of configured route
/// since ADR 0028: a route that terminates here, whose price buys the
/// app's work, and one that forwards over a peering, whose price buys the
/// carriage); or the request declares `greeting` (issue #807, ADR 0069) --
/// structurally not a real payment attempt, so it is answered the same way
/// regardless of whether `destination` matches anything this node prices.
/// The terms are byte-identical either way -- nothing in this
/// shape names a route kind, and a client cannot tell, and has no reason
/// to care, which case it hit. `node` is this node's own facts -- the same
/// value `GET /ilp` serves as its self-description, projected into `extra`
/// rather than assembled a second time (ND-11).
fn payment_required(
    destination: &str,
    price: Price,
    payload_len: usize,
    node: &NodeFacts,
    request: Option<&serde_json::Value>,
) -> Response {
    x402_response(destination, price, payload_len, node, None, request)
}

/// Answer a request to `destination` that arrived over a transport its
/// route's policy does not accept (issue #701, toon-meta#262 decision 11)
/// with the same x402-shaped terms the unpaid-request greeting above uses,
/// rather than inventing a second self-description mechanism -- the client
/// learns which transport the route requires from `extra.requiredTransport`
/// (`required.name()`, e.g. `"btp"`) the same way it learns a route's price
/// from `extra.price`. This runs whether or not the request carries a valid
/// claim: paying over the wrong transport does not make the route
/// reachable that way, so `handle_ilp` checks transport before it checks
/// payment at all.
fn wrong_transport_required(
    destination: &str,
    price: Price,
    payload_len: usize,
    required: TransportPolicy,
    node: &NodeFacts,
    request: Option<&serde_json::Value>,
) -> Response {
    x402_response(
        destination,
        price,
        payload_len,
        node,
        Some(required.name()),
        request,
    )
}

fn x402_response(
    destination: &str,
    price: Price,
    payload_len: usize,
    node: &NodeFacts,
    required_transport: Option<&str>,
    request: Option<&serde_json::Value>,
) -> Response {
    let body = x402_terms_body(
        destination,
        price,
        payload_len,
        node,
        required_transport,
        request,
    );
    let header_value = BASE64.encode(&body);
    Response::builder()
        .status(StatusCode::PAYMENT_REQUIRED)
        .header(header::CONTENT_TYPE, "application/json")
        .header(PAYMENT_REQUIRED_HEADER, header_value)
        .body(Body::from(body))
        .expect("well-formed x402 response")
        .into_response()
}

/// The x402 v2 terms JSON itself (§1.4), shared by both carriages: the HTTP
/// greeting above serves it as a 402 body + `Payment-Required` header, and
/// the BTP greeting (§1.9, `btp` module) carries the same bytes as
/// `payment-required` protocolData on a REJECT -- factored so the two can
/// never drift. `required_transport` (issue #701) is `None` for an ordinary
/// unpaid-request greeting and `Some("http" | "btp")` when this same shape
/// is reused to tell a client it used the wrong transport entirely.
///
/// The construction itself is [`connector_domain::x402::terms_body`] (issue
/// #880): it moved there so the peer carriages -- which sit *below* this
/// crate in the graph and cannot import it -- can call the same emitter for
/// their own `F06` greeting rather than re-declaring the shape.
fn x402_terms_body(
    destination: &str,
    price: Price,
    payload_len: usize,
    node: &NodeFacts,
    required_transport: Option<&str>,
    request: Option<&serde_json::Value>,
) -> Vec<u8> {
    connector_domain::x402::terms_body(&connector_domain::x402::GreetingTerms {
        destination,
        price,
        payload_len,
        node: Some(node),
        required_transport,
        session_lease_ttl_ms: crate::session_registry::SESSION_LEASE_BACKSTOP_TTL.as_millis()
            as u64,
        request,
    })
}

/// Decode a claim header's raw (still base64-encoded) bytes into the
/// plaintext claim JSON, unwrapping first if `wrapped` is true.
fn decode_claim_header(
    header_value: &[u8],
    wrapped: bool,
    wrap_receiver_secret: Option<&[u8; 32]>,
) -> Result<String, ClaimIngestRejection> {
    let decoded = BASE64.decode(header_value).map_err(|error| {
        ClaimIngestRejection::Malformed(format!("claim header is not valid base64: {error}"))
    })?;

    if !wrapped {
        return String::from_utf8(decoded).map_err(|error| {
            ClaimIngestRejection::Malformed(format!("claim header is not valid UTF-8: {error}"))
        });
    }

    let Some(receiver_secret) = wrap_receiver_secret else {
        return Err(ClaimIngestRejection::WrapUnsupported);
    };

    let envelope: WrappedClaimEnvelope = serde_json::from_slice(&decoded).map_err(|error| {
        ClaimIngestRejection::Malformed(format!(
            "wrapped claim envelope is not valid JSON: {error}"
        ))
    })?;
    let ephemeral_public_key_bytes =
        hex_decode(&envelope.ephemeral_public_key).ok_or_else(|| {
            ClaimIngestRejection::Malformed(
                "wrapped claim's ephemeralPublicKey is not valid hex".to_string(),
            )
        })?;
    let ephemeral_public_key: PublicKeyBytes = ephemeral_public_key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| {
            ClaimIngestRejection::Malformed(
                "wrapped claim's ephemeralPublicKey is not 65 bytes uncompressed".to_string(),
            )
        })?;
    let encrypted_payload = BASE64
        .decode(&envelope.encrypted_payload)
        .map_err(|error| {
            ClaimIngestRejection::Malformed(format!(
                "wrapped claim's encryptedPayload is not valid base64: {error}"
            ))
        })?;

    let wrapped_claim = WrappedClaim {
        ephemeral_public_key,
        encrypted_payload,
    };
    let rumor = unwrap_claim(&wrapped_claim, receiver_secret)
        .map_err(|error| ClaimIngestRejection::WrapFailed(error.to_string()))?;
    String::from_utf8(rumor).map_err(|error| {
        ClaimIngestRejection::Malformed(format!("unwrapped claim is not valid UTF-8: {error}"))
    })
}

/// A claim that cleared [`extract_and_validate_claim`].
struct AdmittedClaim {
    /// The channel it validated on -- the evidence, and the only evidence
    /// this connector ever gets, that an unaffiliated sender holds a
    /// payment channel with it (issue #548).
    channel_key: String,
    /// This claim's self-declared signer (`ClientClaim::signer`,
    /// client-edge-spec.md §1.2), present only when the claim header that
    /// carried it arrived plaintext. `None` for a claim that arrived
    /// wrapped (`ILP-Payment-Channel-Claim-Wrapped`) -- deriving an
    /// anonymous sender's ephemeral identity from it would require
    /// unwrapping before the identity authenticating the request is known,
    /// so a wrapped-only claim is never a source for one, by construction:
    /// this field is the one place that rule is enforced.
    plaintext_signer: Option<String>,
    /// What [`crate::claim_gate::ClientClaimGate::admit`] just advanced
    /// [`Self::channel_key`]'s watermark to (issue #1012).
    watermark: AdmittedWatermark,
}

/// The watermark one admitted claim advanced its channel to (issue
/// #1012): exactly the pair [`crate::claim_gate::ClientClaimGate::roll_back`]
/// names when a forwarded route's next hop turns out never to have carried
/// the packet that claim paid for.
///
/// A named pair rather than a loose `(u64, u64)` because both carriages
/// thread it from their admission to their rollback, and two fields of the
/// same type transposed anywhere along that path would still compile --
/// [`Self::of`] is the only way to build one, so the two can only ever come
/// from the claim itself, in the order the gate reads them back in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdmittedWatermark {
    nonce: u64,
    cumulative_amount: u64,
}

impl AdmittedWatermark {
    /// Read `claim`'s own nonce and transferred (cumulative) amount.
    pub(crate) fn of(claim: &ClientClaim) -> AdmittedWatermark {
        AdmittedWatermark {
            nonce: claim.nonce(),
            cumulative_amount: claim.transferred_amount(),
        }
    }
}

/// Extract and fully validate whatever claim header `headers` carries, per
/// client-edge-spec.md §1.3, against `price` -- the matched route's price,
/// `0` for an unpriced or unmatched destination, since routing itself (not
/// this gate) is what refuses an unroutable one, with F02.
///
/// `Ok(None)` means no claim header was present at all -- reachable here
/// only when the destination is unpriced or unmatched, since `handle_ilp`
/// answers the x402 greeting instead of calling this at all for an unpaid
/// request to a priced route (issue #526) -- so the request proceeds
/// unchanged, exactly as it always has. `Ok(Some(admitted))` means a
/// present claim validated cleanly. A plaintext header takes precedence
/// when both are present, since a client presenting both is presenting the
/// same claim twice, not two different ones.
async fn extract_and_validate_claim(
    headers: &HeaderMap,
    charge: u64,
    state: &ClientEdgeState,
) -> Result<Option<AdmittedClaim>, ClaimIngestRejection> {
    let (header_value, wrapped) = if let Some(value) = headers.get(CLAIM_HEADER) {
        (value, false)
    } else if let Some(value) = headers.get(CLAIM_WRAPPED_HEADER) {
        (value, true)
    } else {
        return Ok(None);
    };

    let claim_json = decode_claim_header(
        header_value.as_bytes(),
        wrapped,
        state.wrap_receiver_secret.as_ref(),
    )?;
    let claim = state.claim_gate.ingest(&claim_json, charge).await?;
    let channel_key = claim.channel_key();
    let plaintext_signer = (!wrapped).then(|| claim.signer().to_string());
    let watermark = AdmittedWatermark::of(&claim);
    // Best-effort liveness bookkeeping for issue #693's claim-state
    // endpoint (`ClientClaimGate::note_claim_time`'s own doc): happens only
    // after `ingest` has already returned durable, never inside it.
    state.claim_gate.note_claim_time(&channel_key, now_unix());
    Ok(Some(AdmittedClaim {
        channel_key,
        plaintext_signer,
        watermark,
    }))
}

/// Answer an ILP-level outcome the way client-edge-spec.md §1.1 and §1.6
/// specify: always `200`, the OER-encoded packet as the body, and -- on a
/// REJECT and only a REJECT -- the running cost total in
/// `TOON-Accumulated-Cost` (issue #548). Every REJECT this edge answers
/// with goes through here, so none can report an outcome without also
/// reporting what reaching it cost; a FULFILL carries no such header,
/// having been paid for rather than priced.
fn packet_response(response: PacketResponse) -> Response {
    match response {
        PacketResponse::Fulfill(fulfill) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, OCTET_STREAM)],
            fulfill.encode(),
        )
            .into_response(),
        PacketResponse::Reject(reject) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, OCTET_STREAM),
                (
                    header::HeaderName::from_static(ACCUMULATED_COST_HEADER),
                    reject.accumulated_cost.to_string().as_str(),
                ),
            ],
            reject.encode(),
        )
            .into_response(),
    }
}

/// A claim-ingest refusal, as an OER REJECT. `price` is the matched route's
/// price, and rides home as `accumulated_cost` on an underpayment (issue
/// #548): that refusal's whole subject is a figure the sender did not
/// cover, and disclosing it only inside a human-readable `message` -- the
/// one channel through which a price was ever disclosed before -- forces a
/// client to parse English, or to discover the price by underpaying first,
/// which is exactly what cost discovery exists to prevent. Every other
/// refusal here is decided before any route price is in play and reports
/// `0`: nothing was traversed and nothing terminated.
fn claim_rejected_response(rejection: ClaimIngestRejection, charge: u64) -> Response {
    // Underpayment is a distinct ILP error (F03: Invalid Amount, issue
    // #522) from every other claim-ingest refusal above it (F01: Invalid
    // Packet) -- the claim is structurally and cryptographically fine, it
    // simply isn't enough value.
    //
    // `NotDurable` is a third code again (T00: Internal Error, issue
    // #605), and the only *temporary* one here: this connector could not
    // record the claim, the claim itself is fine, and a sender told F01
    // would conclude its perfectly good claim was invalid rather than
    // retry it.
    //
    // An undercollateralized claim (issue #646) is F03 too -- it is a
    // refusal about an amount, and a sender who parses codes rather than
    // English should see that much -- but it reports `accumulated_cost: 0`,
    // unlike an underpayment: the route's price *was* covered, so the
    // figure this claim failed against is the channel's on-chain deposit,
    // not a price, and #548's price disclosure is not what this refusal is
    // about. The deposit it must cover is in the message.
    //
    // A withheld channel lookup (issue #613) is T05: Rate Limited -- and a
    // *failed* one is T00, which it should have been all along. Both are
    // temporary, and that is the half that matters: neither says anything
    // is wrong with the claim. A sender told F01 concludes its claim is
    // invalid and stops, which is the right conclusion for a bad signature
    // and precisely the wrong one both here and for an RPC outage -- the
    // claim is fine, the connector is busy or broken, and the answer is
    // "try again". `ChannelLookupFailed` fell through to F01 before this
    // change, telling a paying client its claim was invalid because a third
    // party's endpoint blipped; correcting it here rather than leaving it
    // is the point of reasoning about this arm at all.
    //
    // The two are kept apart -- T05 rather than T00 for the shaper --
    // because this connector did not fail at anything when it shapes: it
    // declined to do free work right now, which is the one thing RFC-0027
    // has T05 for, and a sender parsing codes rather than English can tell
    // "retry, the node's endpoint hiccupped" from "back off, you are being
    // metered".
    packet_response(PacketResponse::Reject(claim_rejection_reject(
        rejection, charge,
    )))
}

/// The refusal itself -- `RejectCode`, `accumulated_cost` and message -- for
/// a claim-ingest rejection, independent of carriage: `claim_rejected_response`
/// wraps it for HTTP, the `btp` module frames it for a websocket session
/// (§1.9), so the taxonomy the comment above reasons through stays
/// single-sourced.
/// A client-edge PREPARE to a priced *forwarded* destination may not
/// declare a carried `amount` larger than the `price` it is charged (ADR
/// 0028). `None` when the packet is within its price, or when the route is
/// not one this rule governs.
///
/// The arithmetic this protects is `peer-semantics-pre-868.md` §4's, at the one hop
/// where "upstream" is a client rather than a peer: this connector collects
/// `price`, forwards `amount - fee`, and so earns `fee` exactly when
/// `amount == price`. Let the client pick a larger `amount` and it picks
/// this connector's loss instead -- the peer claim signed on fulfilment is
/// real value, against this node's own channel, for carriage nobody paid
/// for. A terminated route needs no such rule: nothing downstream of it is
/// paid out of the price, and `amount` reaches no wire at all.
///
/// Only the priced branch is governed. An unpriced forwarded route
/// (`price = 0`) is an operator's deliberate free carriage, and bounding
/// its amount to zero would make free carriage impossible rather than safe;
/// it keeps the behavior it had before ADR 0028 exactly.
///
/// `F03` (Invalid Amount) rather than `F01`: the packet is structurally and
/// cryptographically fine, and the refusal is about an amount -- the same
/// reading underpayment already gets. `accumulated_cost` reports the price
/// for issue #548's reason, since the price is the very figure the sender
/// must fit its amount under and it should not have to parse English to
/// learn it.
fn over_carried_reject(
    destination: &str,
    kind: ClientRouteKind,
    amount: u64,
    charge: u64,
) -> Option<Reject> {
    if kind != ClientRouteKind::Forwarded || charge == 0 || amount <= charge {
        return None;
    }
    Some(Reject {
        code: RejectCode::f03_invalid_amount(),
        triggered_by: String::new(),
        message: format!(
            "packet to '{destination}' declares amount {amount}, more than the {charge} this \
             connector charges to forward it -- a forwarded packet never carries more value \
             than it was paid for"
        ),
        data: Vec::new(),
        accumulated_cost: charge,
    })
}

fn claim_rejection_reject(rejection: ClaimIngestRejection, charge: u64) -> Reject {
    let (code, accumulated_cost) = match rejection {
        ClaimIngestRejection::Underpayment { .. } => (RejectCode::f03_invalid_amount(), charge),
        ClaimIngestRejection::Undercollateralized { .. } => (RejectCode::f03_invalid_amount(), 0),
        ClaimIngestRejection::NotDurable => (RejectCode::t00_internal_error(), 0),
        ClaimIngestRejection::ChannelLookupFailed(_) => (RejectCode::t00_internal_error(), 0),
        ClaimIngestRejection::LookupBudgetExhausted { .. } => (RejectCode::t05_rate_limited(), 0),
        _ => (RejectCode::f01_invalid_packet(), 0),
    };
    Reject {
        code,
        triggered_by: String::new(),
        message: rejection.message(),
        data: Vec::new(),
        accumulated_cost,
    }
}

/// The bearer credential a request presented via `Authorization`
/// (client-edge-spec.md §1.2): `Bearer <secret>`, scheme matched
/// case-insensitively and a bare credential with no scheme tolerated --
/// mirrors BTP's own tolerant `secret: ''` auth frame. An absent
/// `Authorization` header is an empty credential, deliberately not
/// distinguished from a header present with an empty value: this is what
/// lets a configured identity with an empty secret authenticate a request
/// that omits the header entirely.
fn extract_bearer(headers: &HeaderMap) -> String {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return String::new();
    };
    let Ok(value) = value.to_str() else {
        return String::new();
    };
    match value.split_once(' ') {
        Some((scheme, credential)) if scheme.eq_ignore_ascii_case("bearer") => {
            credential.to_string()
        }
        _ => value.to_string(),
    }
}

async fn handle_ilp(
    State(state): State<Arc<ClientEdgeState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // **Role is decided before anything else happens** (peer-carriage-spec.md
    // §1.5): before a claim is decoded, before a watermark is consulted,
    // before a packet is even decoded -- a peer FLUSH is a POST with an
    // *empty* body (§3), which the client edge's own decode below would
    // refuse as malformed. A client-role request gets `None` back and every
    // line after this one runs exactly as it did before peering existed.
    if let Some(peers) = state.peers.as_ref() {
        if let Some(response) = peers.handle_http(&headers, &body).await {
            return response;
        }
    }

    // client-edge-spec.md §1.2: authentication outranks the greeting -- a
    // presented `ILP-Peer-Id` that fails to authenticate is refused `401`
    // before the route is even looked up, so an unauthorised caller is
    // never told a priced route's terms (`402`) and a credential failure
    // is never reported as a payment outcome. A request presenting no
    // `ILP-Peer-Id` is unaffected here -- it is anonymous, resolved once
    // the claim (if any) has been admitted, below.
    let presented_peer_id = headers
        .get(PEER_ID_HEADER)
        .and_then(|value| value.to_str().ok());
    let authenticated_peer = match presented_peer_id.map(|peer_id| {
        resolve_identity(
            Some(peer_id),
            &extract_bearer(&headers),
            None,
            &state.identities,
        )
    }) {
        None => None,
        Some(Ok(identity)) => Some(identity),
        Some(Err(rejection)) => {
            tracing::warn!(
                peer_id = %rejection.peer_id,
                "client-edge identity presented but failed to authenticate"
            );
            return (StatusCode::UNAUTHORIZED, rejection.to_string()).into_response();
        }
    };

    let prepare = match Prepare::decode(&body) {
        Ok(prepare) => prepare,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };

    // An unpaid request -- no claim header of either kind -- addressing a
    // route this connector serves and prices is answered with that route's
    // terms instead of being routed at all (client-edge-spec.md §1.4, ADR
    // 0022): no free work for an anonymous, unpaying caller, whether the
    // work is an app's or a peering's carriage. A present claim header
    // suppresses the greeting unconditionally (its validation, including
    // underpayment, is §1.3's job below); an unpriced or unmatched
    // destination is unaffected and falls through unchanged, exactly as it
    // always has -- unless the PREPARE itself declares `greeting` (issue
    // #807), checked below.
    let has_claim_header =
        headers.contains_key(CLAIM_HEADER) || headers.contains_key(CLAIM_WRAPPED_HEADER);
    // Issue #807 / #1269: a greeting probe can never be routed regardless of
    // destination, so it is answered here rather than falling through to
    // `Connector::handle_prepare`. `packages/announcer/src/edge-client.ts`'s
    // `fetchGreeting` builds exactly this shape. A client whose genesis peer
    // seed is stale or missing has no `[[routes]]`-matching destination to
    // probe with either, so gating the greeting on a route match at all (the
    // pre-#807 behaviour, still required for a real, non-greeting PREPARE
    // just below) left it with nothing to probe with at all. Until issue
    // #1269 this was inferred from an all-zero execution condition; it is
    // now the packet's own explicit `greeting` flag.
    // No matching configured route means nothing here is priced -- routing
    // itself (not this gate) is what refuses an unroutable destination,
    // with F02. One lookup serves every fact (issue #701, ADR 0028): the
    // price, the transport policy and the route kind come from the same
    // matched route, and it is the same selection `handle_prepare` will
    // route by, so what is charged and where the packet goes cannot
    // disagree.
    let client_route = state.connector.client_route(&prepare.destination);
    // What this packet costs here: the schedule at its own payload length
    // (ADR 0065). One figure, computed once, and everything below -- the
    // greeting, the over-carry bound, the claim gate and the reject's
    // accumulated cost -- is charged against it, so what is greeted and what
    // is collected cannot disagree.
    let schedule = client_route
        .as_ref()
        .map_or(Price::FREE, |route| route.price);
    let charge = schedule.charge(prepare.data.len());
    // Issue #1210: what a client should send to use this route, greeted
    // alongside its price -- read off the same lookup, so the two cannot
    // name different routes.
    let request = client_route
        .as_ref()
        .and_then(|route| route.request.as_ref());

    // Transport policy (issue #701, toon-meta#262 decision 11) is checked
    // before payment is considered at all: a route restricted to BTP is
    // unreachable over HTTP whether or not the request carries a valid
    // claim, so a paid request over the wrong transport is refused exactly
    // like an unpaid one. A destination matching no configured route is
    // unaffected -- `None` here, same as an unmatched destination's price
    // -- and a forwarded route reports `Both`, so this never fires for one.
    if let Some(policy) = client_route.as_ref().map(|route| route.transport_policy) {
        if !policy.accepts_http() {
            return wrong_transport_required(
                &prepare.destination,
                schedule,
                prepare.data.len(),
                policy,
                &state.node,
                request,
            );
        }
    }

    if !has_claim_header && (charge > 0 || prepare.greeting) {
        return payment_required(
            &prepare.destination,
            schedule,
            prepare.data.len(),
            &state.node,
            request,
        );
    }

    // ADR 0028: refused *before* the claim is ingested, so a packet this
    // connector will not carry never spends the client's watermark. The
    // greeting above runs first, so an unpaying client learns the price it
    // must size its amount under rather than this refusal.
    if let Some(route) = client_route.as_ref() {
        if let Some(reject) =
            over_carried_reject(&prepare.destination, route.kind, prepare.amount, charge)
        {
            return packet_response(PacketResponse::Reject(reject));
        }
    }

    // A claim header's validation failure rejects the packet before it is
    // routed at all (client-edge-spec.md §1.3) -- the app is never asked to
    // do work that was never validly paid for.
    let mut plaintext_claim_signer = None;
    // Issue #535/ADR 0036: the channel this packet's covering claim admitted
    // on, carried into the `"packet"` span so a paid delivery is joinable to
    // this gate's own claim journal (`state_dir/client-edge-claims.log`)
    // `InboundClaimAccepted` entries under the same channel key -- the
    // honest successor to the relay's retired payer-attribution header.
    // `None` for an unclaimed request (unpriced/unmatched destination), the
    // only shape that reaches routing without one.
    let mut client_channel_id = None;
    // Issue #1012: the watermark this packet's covering claim advanced its
    // channel to, kept only so a forwarded route whose next hop terminally
    // rejects can name exactly this claim to `ClientClaimGate::roll_back`
    // below. `None` on every path `client_channel_id` is also `None` on.
    let mut admitted_claim_watermark = None;
    // Issue #869: the converse of the invariant stated above -- a packet
    // whose envelope will be refused for its own target shape
    // (`AppOutcome::Refused`, F00) is never going to reach the app either,
    // however good the claim covering it is. Rather than ingest that claim
    // and then answer a refusal that says nothing was charged, the claim
    // is left entirely unadmitted: routing below still runs unchanged for
    // both branches and raises the identical F00 itself
    // (`Connector::deliver_to_app`'s own check, untouched), so a sender
    // whose claim was good is told exactly what it was told before --
    // only that no watermark moves getting there. A sender whose claim
    // would *also* have been refused now hears about the target instead
    // of the claim (F00 rather than §1.3's F01/F03/T00 taxonomy): that
    // claim is never looked at on this path, since nothing about it could
    // make this packet deliverable.
    if !state.connector.envelope_target_would_be_refused(&prepare) {
        match extract_and_validate_claim(&headers, charge, &state).await {
            Err(rejection) => return claim_rejected_response(rejection, charge),
            // A claim that cleared the gate is this connector's evidence
            // that the sender holds the channel it names (issue #548),
            // which is what makes that sender eligible to probe at
            // `POST /ilp/probe` later.
            Ok(Some(admitted)) => {
                state.connector.recognize_channel(&admitted.channel_key);
                plaintext_claim_signer = admitted.plaintext_signer;
                admitted_claim_watermark = Some(admitted.watermark);
                client_channel_id = Some(admitted.channel_key);
            }
            Ok(None) => {}
        }
    }

    // client-edge-spec.md §1.2: the sender's identity, resolved and made
    // available to everything downstream that needs a payer (issue #502).
    // A presented `ILP-Peer-Id` was already resolved -- and authenticated,
    // or this request would have been refused `401` -- above, before the
    // claim was looked at; the claim signer is consulted only for a
    // request that presented none, and never a wrapped-only claim's, since
    // `plaintext_claim_signer` is `None` for one by construction.
    let identity =
        authenticated_peer.unwrap_or_else(|| anonymous_identity(plaintext_claim_signer.as_deref()));
    tracing::debug!(identity = %identity.id(), "client-edge request identity resolved");

    // Issue #736: routing is `Connector::handle_prepare`'s three configured
    // sources first, then whatever client session `state.session_registry`
    // has bound to this destination -- see `session_route::route_prepare`.
    let response =
        session_route::route_prepare(&state, prepare, client_channel_id.as_deref()).await;
    roll_back_uncarried_forward(
        &state,
        is_forwarded_route(client_route),
        client_channel_id.as_deref(),
        admitted_claim_watermark,
        &response,
    )
    .await;
    packet_response(response)
}

/// Whether the single route lookup both carriages make (issue #701)
/// matched a *forwarded* route -- the one shape
/// [`roll_back_uncarried_forward`] acts on. One definition, called from
/// both carriages, for the same reason every other rule the two share has
/// one: what the HTTP path charges and what the BTP path charges must not
/// drift.
pub(crate) fn is_forwarded_route(client_route: Option<ClientRouteFacts>) -> bool {
    matches!(
        client_route.map(|route| route.kind),
        Some(ClientRouteKind::Forwarded)
    )
}

/// Issue #1012: a client-priced forwarded route (ADR 0028) admits the
/// client's covering claim before learning whether the next hop will
/// fulfil it -- `Connector::forward_via_peer_route` can only greet, retry
/// and finally accept or reject once it has actually tried the peer. A
/// terminal reject reaching here means the packet this claim paid for was
/// never carried, so the claim is not counted against the client: its
/// watermark is rolled back to what it was immediately before this
/// request, via [`crate::claim_gate::ClientClaimGate::roll_back`], which
/// itself no-ops (rather than corrupts) if a later claim on the same
/// channel has since moved on. A no-op on every other shape -- an
/// unforwarded route, a fulfilment, or a request that admitted no claim at
/// all -- so this changes nothing about what those already do.
pub(crate) async fn roll_back_uncarried_forward(
    state: &ClientEdgeState,
    is_forwarded_route: bool,
    client_channel_id: Option<&str>,
    admitted_claim_watermark: Option<AdmittedWatermark>,
    response: &PacketResponse,
) {
    if !is_forwarded_route || !matches!(response, PacketResponse::Reject(_)) {
        return;
    }
    let (Some(channel_key), Some(watermark)) = (client_channel_id, admitted_claim_watermark) else {
        return;
    };
    if let Err(error) = state
        .claim_gate
        .roll_back(channel_key, watermark.nonce, watermark.cumulative_amount)
        .await
    {
        tracing::error!(
            channel = channel_key,
            error = %error.message(),
            "a forwarded route's next hop rejected this packet, but this claim's watermark \
             could not be durably rolled back -- the client remains charged for a packet \
             this connector never carried"
        );
    }
}

/// `POST /ilp/probe` -- a probe's ingress (client-edge-spec.md §1.6, ADR
/// 0011): an ordinary PREPARE a sender expects to be rejected, sent to
/// learn what a path costs from the `TOON-Accumulated-Cost` the REJECT
/// comes back with. Body and response framing are `POST /ilp`'s exactly
/// (§1.1); what differs is the gate in front, and that nothing is charged.
///
/// Because a probe traverses this connector's network for free, it is
/// accepted only from a sender identified by a payment channel claim, and
/// only within a rate limit per that channel -- ADR 0011's two conditions,
/// enforced in [`Connector::handle_probe`]. Both denials, and a probe that
/// presents no usable claim at all, are `403`: the sender may be perfectly
/// well authenticated (§1.2's `401` is a different failure) and is simply
/// not authorized to probe. A `403` carries no OER body, per §1.1's rule
/// that a non-2xx status never does.
///
/// The claim here identifies rather than pays: it is validated in full,
/// against a price of `0`, so possession of the channel is proven and a
/// replayed claim is still refused, but no value need advance. A sender
/// probes by reissuing at the same cumulative amount with a fresh nonce.
async fn handle_probe(
    State(state): State<Arc<ClientEdgeState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let prepare = match Prepare::decode(&body) {
        Ok(prepare) => prepare,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };

    let channel_key = match extract_and_validate_claim(&headers, 0, &state).await {
        Ok(Some(admitted)) => admitted.channel_key,
        Ok(None) => {
            return (
                StatusCode::FORBIDDEN,
                "a probe must identify itself with a payment channel claim".to_string(),
            )
                .into_response();
        }
        Err(rejection) => return (StatusCode::FORBIDDEN, rejection.message()).into_response(),
    };

    match state.connector.handle_probe(&channel_key, prepare).await {
        Ok(response) => packet_response(response),
        Err(ProbeDenied::NoOpenChannel) => (
            StatusCode::FORBIDDEN,
            format!("no payment channel this connector recognizes: '{channel_key}'"),
        )
            .into_response(),
        Err(ProbeDenied::RateLimited) => (
            StatusCode::FORBIDDEN,
            format!("probe rate limit exceeded for '{channel_key}'"),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use chrono::{TimeZone, Utc};
    use connector_config::StaticRoute;
    use connector_domain::{EnvelopeRequest, EnvelopeResponse, Fulfill, Reject};
    use connector_runtime::{
        AppOutcome, FakeAppClient, InMemoryJournal, InProcessPeerTransport, PeerRoute,
        RuntimePeerChannel, RuntimePeering, TestClock,
    };
    use connector_signer::LocalSigner;
    use tower::ServiceExt;

    /// A claim gate over `channels`, journaling to a store that lives no
    /// longer than the test does. These tests are about the HTTP surface
    /// in front of the gate; that a watermark survives a restart is
    /// `claim_gate`'s own `durability` module, over a real file.
    fn test_gate(channels: ClientChannelRegistry) -> ClientClaimGate {
        ClientClaimGate::restore(channels, Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay")
    }

    /// A structured envelope's plain encoding (ADR 0018/issue #519) -- used
    /// directly only by tests whose `Prepare` never reaches
    /// `Connector::deliver_to_app` at all (a reject raised short of the
    /// termination neither inspects nor requires a sealed `data`). Any test
    /// that expects real app delivery must use [`sealed_sample_prepare`]
    /// instead, sealed to whichever identity the terminating `Connector` is
    /// configured with -- per ADR 0018, `deliver_to_app` cannot open a
    /// plaintext envelope at all now that sealing is mandatory (issue #524).
    fn envelope_request_data(body: &[u8]) -> Vec<u8> {
        EnvelopeRequest {
            method: "POST".to_string(),
            target: "/".to_string(),
            headers: vec![],
            body: body.to_vec(),
        }
        .encode()
    }

    /// Open `data` (a `Fulfill.data`, or a termination `Reject.data`) with
    /// `shared_secret` and decode it as a response envelope.
    fn open_sealed_envelope(shared_secret: &[u8; 32], data: &[u8]) -> EnvelopeResponse {
        let opened = connector_signer::giftwrap::open_response(shared_secret, data)
            .expect("open sealed response");
        EnvelopeResponse::decode(&opened).expect("decode response envelope")
    }

    /// The `AppOutcome` a `FakeAppClient` produces for an app that answers
    /// `200` with `body`. The app supplies nothing toward fulfilment (issue
    /// #525): a termination derives its own answer's fulfilment from the
    /// packet's sealed secret, never from anything in this response.
    fn answered(body: &[u8]) -> AppOutcome {
        answered_with_status(200, body)
    }

    fn answered_with_status(status: u16, body: &[u8]) -> AppOutcome {
        AppOutcome::Answered {
            response: EnvelopeResponse {
                status,
                headers: vec![],
                body: body.to_vec(),
            },
        }
    }

    /// The response envelope `Connector::deliver_to_app` seals into
    /// `Fulfill.data` for the same inputs `answered` above configures a
    /// `FakeAppClient` with. Compare against [`open_sealed_envelope`]'s
    /// result, since sealing makes the raw wire bytes non-deterministic per
    /// call.
    fn fulfill_envelope(body: &[u8]) -> EnvelopeResponse {
        fulfill_envelope_with_status(200, body)
    }

    fn fulfill_envelope_with_status(status: u16, body: &[u8]) -> EnvelopeResponse {
        EnvelopeResponse {
            status,
            headers: vec![],
            body: body.to_vec(),
        }
    }

    /// A fixed EIP-712 domain for this module's one peer claim test
    /// (issue #575/#566) -- an arbitrary but consistent chain id and
    /// `TokenNetwork` address.
    pub(crate) fn test_channel_domain() -> connector_runtime::ChannelDomain {
        connector_runtime::ChannelDomain {
            chain_id: 84_532,
            token_network_address: [0x1E; 20],
        }
    }

    /// A valid on-chain `bytes32` peer channel id for tests (issue
    /// #575's AC4).
    pub(crate) fn channel_a() -> String {
        format!("0x{:064x}", 1)
    }

    /// A next hop reporting where this node's claims on a channel stand --
    /// the authority every covering claim is priced off (see
    /// `connector_runtime::outbound_client`'s header). A fake upholding the
    /// port's contract rather than a stub with expectations (ADR 0007).
    struct ReportsAWatermark;

    #[async_trait::async_trait]
    impl connector_runtime::ClaimStateSource for ReportsAWatermark {
        async fn watermark(
            &self,
            _channel: &[u8; 32],
            _domain: &connector_runtime::ClaimStateDomain,
        ) -> Result<connector_runtime::ClaimWatermark, connector_runtime::OutboundClientError>
        {
            Ok(connector_runtime::ClaimWatermark {
                nonce: 0,
                cumulative: 0,
                available: Some(u128::MAX),
            })
        }
    }

    /// Give `connector` the `[[pay_channels]]` half of a peering: ADR 0042
    /// requires a connector to cover every PREPARE it sends, so since issue
    /// #1145 a forward to a hop with no channel to pay it from is refused
    /// outright rather than carried on ADR 0004's postpay convention. Every
    /// test here that forwards to a peer needs this, and a fixture without
    /// it is one no configuration could produce
    /// (`ConfigError::PayChannelUnbound`).
    pub(crate) fn covering(connector: Connector, peer_id: &str) -> Connector {
        connector
            // The settlement key the covering claim is signed with. A test
            // that cares which key that is calls `with_signer` again after
            // this, which is the same setter.
            .with_signer(Arc::new(LocalSigner::generate("covering-settlement")))
            .with_outbound_client_ledger(Arc::new(
                connector_runtime::OutboundClientLedger::in_memory(),
            ))
            .with_outbound_client_hop(
                peer_id,
                channel_a(),
                connector_runtime::EvmDomain {
                    chain_id: test_channel_domain().chain_id,
                    token_network: test_channel_domain().token_network_address,
                },
                Arc::new(ReportsAWatermark),
            )
            .expect("channel_a() is a valid on-chain channel id")
    }

    fn test_signer() -> Arc<dyn Signer> {
        Arc::new(LocalSigner::generate("test-signer"))
    }

    /// A `Prepare` with no sealed wrap -- `data` is a plain, unsealed
    /// envelope, so a test using this bare (rather than
    /// [`sealed_sample_prepare`]) must not expect a `Fulfill`.
    fn sample_prepare(destination: &str) -> Prepare {
        Prepare {
            amount: 0,
            // Comfortably after `test_clock()`'s instant (2030-01-01).
            expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
            greeting: false,
            destination: destination.to_string(),
            data: envelope_request_data(b"hello app"),
        }
    }

    /// As [`sample_prepare`], but with `data` sealed to `receiver_public`
    /// (issue #524) -- for a test whose `Prepare` genuinely reaches
    /// `Connector::deliver_to_app` and expects it to fulfil, the termination
    /// deriving its fulfilment from this same sealed secret (ADR 0019, issue
    /// #525). Returns the shared secret alongside, to open the sealed
    /// `Fulfill`/termination-`Reject` this produces.
    fn sealed_sample_prepare(
        destination: &str,
        receiver_public: &PublicKeyBytes,
    ) -> (Prepare, [u8; 32]) {
        sealed_sample_prepare_with_target(destination, "/", receiver_public)
    }

    /// As [`sealed_sample_prepare`], but with the envelope's own `target`
    /// (issue #596/#869) set to `target` rather than hard-coded to `"/"` --
    /// for a test asserting on what happens when that target does, or does
    /// not, resolve under the matched route's handler path.
    fn sealed_sample_prepare_with_target(
        destination: &str,
        target: &str,
        receiver_public: &PublicKeyBytes,
    ) -> (Prepare, [u8; 32]) {
        let envelope = EnvelopeRequest {
            method: "POST".to_string(),
            target: target.to_string(),
            headers: vec![],
            body: b"hello app".to_vec(),
        }
        .encode();
        let (data, shared_secret) =
            connector_signer::giftwrap::seal_request(&envelope, receiver_public).expect("seal");
        (
            Prepare {
                data,
                ..sample_prepare(destination)
            },
            shared_secret,
        )
    }

    /// As [`sealed_sample_prepare`], but with an envelope body of `body` --
    /// for a test about ADR 0065, where what a packet costs depends on how
    /// long its sealed `data` is and two packets must therefore differ in
    /// length.
    fn sealed_sample_prepare_with_body(
        destination: &str,
        body: &[u8],
        receiver_public: &PublicKeyBytes,
    ) -> (Prepare, [u8; 32]) {
        let envelope = EnvelopeRequest {
            method: "POST".to_string(),
            target: "/".to_string(),
            headers: vec![],
            body: body.to_vec(),
        }
        .encode();
        let (data, shared_secret) =
            connector_signer::giftwrap::seal_request(&envelope, receiver_public).expect("seal");
        (
            Prepare {
                data,
                ..sample_prepare(destination)
            },
            shared_secret,
        )
    }

    fn sealed_sample_prepare_with_amount(
        destination: &str,
        amount: u64,
        receiver_public: &PublicKeyBytes,
    ) -> (Prepare, [u8; 32]) {
        let (prepare, shared_secret) = sealed_sample_prepare(destination, receiver_public);
        (Prepare { amount, ..prepare }, shared_secret)
    }

    fn test_clock() -> Arc<TestClock> {
        Arc::new(TestClock::new(
            Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
        ))
    }

    #[tokio::test]
    async fn a_client_sending_a_matching_packet_receives_the_apps_outcome() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(route.handler_url(), answered(b"app said yes"));
        let signer = test_signer();
        let connector = Arc::new(
            Connector::new(
                vec![route],
                vec![],
                app_client,
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(signer.clone()),
        );
        let (prepare, shared_secret) =
            sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
        let app = router(connector, signer);

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(prepare.encode()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            OCTET_STREAM
        );

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let fulfill = Fulfill::decode(&bytes).expect("decode fulfill");
        assert_eq!(
            open_sealed_envelope(&shared_secret, &fulfill.data),
            fulfill_envelope(b"app said yes")
        );
    }

    #[tokio::test]
    async fn a_packet_with_no_matching_route_is_rejected_with_a_specific_reason() {
        let app_client = Arc::new(FakeAppClient::new());
        let connector = Arc::new(Connector::new(
            vec![],
            vec![],
            app_client,
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router(connector, test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(sample_prepare("g.nowhere").encode()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        // An ILP-level outcome, even a reject, is always HTTP 200 (client-edge-spec.md §1.1).
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let reject = Reject::decode(&bytes).expect("decode reject");
        assert_eq!(reject.code.as_str(), "F02");
        assert!(reject.message.contains("g.nowhere"));
    }

    #[tokio::test]
    async fn a_malformed_request_body_is_a_400() {
        let app_client = Arc::new(FakeAppClient::new());
        let connector = Arc::new(Connector::new(
            vec![],
            vec![],
            app_client,
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router(connector, test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(vec![0xff, 0xff, 0xff, 0xff]))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// Issue #521's central rule (ADR 0020): "you pay for an answer, not
    /// the answer you wanted." A non-2xx response from the app is a real
    /// answer, still HTTP 200 at the client edge, riding home as a
    /// response envelope on a FULFILL -- not converted into a rejection.
    #[tokio::test]
    async fn a_non_2xx_response_from_the_app_still_returns_200_with_a_fulfill_body() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(
            route.handler_url(),
            answered_with_status(402, b"payment required"),
        );
        let signer = test_signer();
        let connector = Arc::new(
            Connector::new(
                vec![route],
                vec![],
                app_client,
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(signer.clone()),
        );
        let (prepare, shared_secret) =
            sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
        let app = router(connector, signer);

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(prepare.encode()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let fulfill = Fulfill::decode(&bytes).expect("decode fulfill");
        assert_eq!(
            open_sealed_envelope(&shared_secret, &fulfill.data),
            fulfill_envelope_with_status(402, b"payment required")
        );
    }

    /// Two connectors, driven only through the first one's router: a client
    /// posts a packet to the first connector, which has no app of its own
    /// for this destination and instead forwards it over an in-process peer
    /// transport to the second connector, which delivers it to its app.
    #[tokio::test]
    async fn a_client_packet_is_forwarded_to_a_second_connector_and_delivered_to_its_app() {
        let second_hop_route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let second_hop_app_client = Arc::new(FakeAppClient::new());
        second_hop_app_client.respond(
            second_hop_route.handler_url(),
            answered(b"delivered by the second connector"),
        );
        let second_hop_identity = test_signer();
        let second_hop = Arc::new(
            Connector::new(
                vec![second_hop_route],
                vec![],
                second_hop_app_client,
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(second_hop_identity.clone()),
        );
        let mut peer_transport = InProcessPeerTransport::new();
        peer_transport.add_peer("second-hop", second_hop);
        let first_hop = Arc::new(covering(
            Connector::new(
                vec![],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                Arc::new(FakeAppClient::new()),
                Arc::new(peer_transport),
                test_clock(),
            ),
            "second-hop",
        ));
        // Sealed to the *second* hop's identity -- the connector that
        // actually terminates this route, not the one the client's router
        // request happens to land on first.
        let (prepare, shared_secret) =
            sealed_sample_prepare("g.example.app", &second_hop_identity.public_key().unwrap());
        let app = router(first_hop, test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(prepare.encode()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let fulfill = Fulfill::decode(&bytes).expect("decode fulfill");
        assert_eq!(
            open_sealed_envelope(&shared_secret, &fulfill.data),
            fulfill_envelope(b"delivered by the second connector")
        );
    }

    /// The first connector's flat fee (ADR 0010) for its peering relation
    /// with the second connector is subtracted before forwarding, and the
    /// second connector -- reachable only through the first one's router,
    /// exactly like a real client -- observes the discounted amount.
    #[tokio::test]
    async fn a_client_packet_forwarded_to_a_peer_is_charged_that_relations_flat_fee() {
        use connector_signer::{LocalSigner, Signer};

        let second_hop_route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let second_hop_app_client = Arc::new(FakeAppClient::new());
        second_hop_app_client.respond(
            second_hop_route.handler_url(),
            answered(b"delivered by the second connector"),
        );
        let payer_signer = LocalSigner::generate("payer-claim-key");
        let payer_address =
            connector_signer::derive_evm_address(&payer_signer.public_key().unwrap());
        let second_hop_identity = test_signer();
        let second_hop = Arc::new(
            Connector::new(
                vec![second_hop_route],
                vec![],
                second_hop_app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_channel_verification_key(channel_a(), payer_address)
            .with_channel_domain(channel_a(), test_channel_domain())
            .unwrap()
            .with_identity_signer(second_hop_identity.clone()),
        );
        let second_hop_claims = Arc::clone(&second_hop);
        let mut peer_transport = InProcessPeerTransport::new();
        peer_transport.add_peer("second-hop", second_hop);
        let first_hop = Arc::new(
            covering(
                Connector::new(
                    vec![],
                    vec![PeerRoute::new("g.example.app", "second-hop")],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(peer_transport),
                    test_clock(),
                )
                .with_peer_fees([("second-hop".to_string(), 3)])
                .with_channel_domain(channel_a(), test_channel_domain())
                .unwrap(),
                "second-hop",
            )
            // After `covering`, so this is the key the covering claim is
            // actually signed with -- and therefore the one the second hop
            // has registered as its counterparty.
            .with_signer(Arc::new(payer_signer)),
        );
        let (prepare, _shared_secret) = sealed_sample_prepare_with_amount(
            "g.example.app",
            50,
            &second_hop_identity.public_key().unwrap(),
        );
        let app = router(first_hop.clone(), test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(prepare.encode()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        Fulfill::decode(&bytes).expect("decode fulfill");
        // The port never sees a `Prepare` (issue #521), so the forwarded
        // amount is asserted through the covering claim that rode it --
        // 50 minus this peer relationship's flat fee of 3. Read off the
        // SECOND hop, which verified and watermarked it: since issue #1145
        // the claim is minted before the packet is sent and its watermark
        // authority is the receiver, so the payer's own peer book records
        // nothing at all.
        assert!(first_hop.claims().is_empty());
        let accepted = second_hop_claims.claims();
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].cumulative_amount, 47);
    }

    /// A packet forwarded to a second connector that has no route for it is
    /// rejected there, and that rejection reaches the original client
    /// unchanged through the first connector's router.
    #[tokio::test]
    async fn a_reject_with_no_route_at_the_second_hop_reaches_the_original_client() {
        let second_hop = Arc::new(Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let mut peer_transport = InProcessPeerTransport::new();
        peer_transport.add_peer("second-hop", second_hop);
        let first_hop = Arc::new(covering(
            Connector::new(
                vec![],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                Arc::new(FakeAppClient::new()),
                Arc::new(peer_transport),
                test_clock(),
            ),
            "second-hop",
        ));
        let app = router(first_hop, test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(sample_prepare("g.example.app").encode()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let reject = Reject::decode(&bytes).expect("decode reject");
        assert_eq!(reject.code.as_str(), "F02");
        assert!(reject.message.contains("g.example.app"));
    }

    /// Two separate connectors: a client posts to the first connector's
    /// router, which has no app of its own for this destination and
    /// forwards the packet across the [`PeerTransport`] port to the second
    /// connector, which delivers it to its app -- and the fulfillment
    /// travels back the same way.
    ///
    /// This used to run over the raw-TCP transport's `PeerWireServer`.
    /// ADR 0027 / issue #679 deleted that wire, so the hop is made over
    /// the in-process transport instead; what is under test here is the
    /// client edge handing a peer-routed packet to whatever transport is
    /// installed, which is carriage-independent. The statement that a
    /// *network* transport upholds the port's contract belongs to
    /// `peer_transport.rs`'s contract suite, which #676's carriages join.
    #[tokio::test]
    async fn a_client_packet_is_forwarded_over_the_peer_transport_to_a_second_connector() {
        let second_hop_route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let second_hop_app_client = Arc::new(FakeAppClient::new());
        second_hop_app_client.respond(
            second_hop_route.handler_url(),
            answered(b"delivered over the peer transport"),
        );
        let second_hop_identity = test_signer();
        let second_hop = Arc::new(
            Connector::new(
                vec![second_hop_route],
                vec![],
                second_hop_app_client,
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(second_hop_identity.clone()),
        );

        let mut peer_transport = InProcessPeerTransport::new();
        peer_transport.add_peer("second-hop", second_hop);
        let first_hop = Arc::new(covering(
            Connector::new(
                vec![],
                vec![PeerRoute::new("g.example.app", "second-hop")],
                Arc::new(FakeAppClient::new()),
                Arc::new(peer_transport),
                test_clock(),
            ),
            "second-hop",
        ));
        let (prepare, shared_secret) =
            sealed_sample_prepare("g.example.app", &second_hop_identity.public_key().unwrap());
        let app = router(first_hop, test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(prepare.encode()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let fulfill = Fulfill::decode(&bytes).expect("decode fulfill");
        assert_eq!(
            open_sealed_envelope(&shared_secret, &fulfill.data),
            fulfill_envelope(b"delivered over the peer transport")
        );
    }

    /// A sender can ask this connector who it is and get back the key it
    /// would seal a packet to (ADR 0018, ADR 0022, issue #526) --
    /// unauthenticated, no request body, and answered from this connector's
    /// own signer with no state change.
    #[tokio::test]
    async fn a_sender_can_ask_this_connectors_identity_and_gets_its_public_key() {
        let connector = Arc::new(Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let signer = test_signer();
        let app = router(connector, signer.clone());

        let request = Request::builder()
            .method("GET")
            .uri("/ilp/identity")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let identity: ClientEdgeIdentity = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(identity.key_id, signer.key_id());
        assert_eq!(
            identity.public_key,
            format!("0x{}", hex_encode(&signer.public_key().unwrap()))
        );
    }

    /// A sender can ask what a given terminated route costs and gets that
    /// route's configured price back (ADR 0022, issue #526), reading the
    /// same value `app_route_price` -- and so the claim gate and the x402
    /// greeting -- would charge against a real request.
    #[tokio::test]
    async fn a_sender_can_ask_what_a_route_costs() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 42).unwrap();
        let connector = Arc::new(Connector::new(
            vec![route],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router(connector, test_signer());

        let request = Request::builder()
            .method("GET")
            .uri("/ilp/routes/price?destination=g.example.app.sub")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let view: RoutePriceView = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(view.price, 42);
    }

    /// Asking about a destination that matches no locally-terminated route
    /// is a 404, not a fabricated price.
    #[tokio::test]
    async fn asking_the_price_of_an_unmatched_destination_is_a_404() {
        let connector = Arc::new(Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router(connector, test_signer());

        let request = Request::builder()
            .method("GET")
            .uri("/ilp/routes/price?destination=g.nowhere")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// The heart of issue #526: an unpaid request to a route this connector
    /// terminates and prices is answered with its terms (x402 v2, §1.4)
    /// instead of ever reaching the app -- the free-gateway failure mode
    /// ADR 0022 exists to close.
    #[tokio::test]
    async fn an_unpaid_request_to_a_priced_route_is_answered_with_terms_not_performed() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        // Deliberately no `app_client.respond(...)` registered: `deliveries()`
        // records a call the moment `AppClient::deliver` runs, regardless of
        // outcome, so an empty list below proves the app was never reached
        // at all -- not merely that it answered unfavorably.
        let connector = Arc::new(Connector::new(
            vec![route],
            vec![],
            app_client.clone(),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router(connector, test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(sample_prepare("g.example.app").encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        let payment_required_header = response
            .headers()
            .get(PAYMENT_REQUIRED_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let terms: X402PaymentRequired = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(terms.x402_version, 2);
        assert_eq!(terms.resource.url, "g.example.app");
        assert_eq!(terms.accepts.len(), 1, "terms are carried as a list");
        assert_eq!(terms.accepts[0].amount, "100");

        // The header carries the same body the greeting sends over the wire.
        let header_bytes = BASE64.decode(&payment_required_header).unwrap();
        assert_eq!(header_bytes, bytes.to_vec());

        assert!(
            app_client.deliveries().is_empty(),
            "the app must never be asked to do the work an unpaid request didn't pay for"
        );

        // A node with no settlement backend keeps the pre-#617 greeting
        // shape exactly: no `settlement` key at all, not a null one. Issue
        // #632 adds `settlements` beside it on the same terms: absent, not
        // an empty array, on a settlement-less node.
        let extra = serde_json::to_value(&terms.accepts[0].extra).unwrap();
        assert!(
            extra.get("settlement").is_none(),
            "a settlement-less node's greeting must not carry a settlement key: {extra}"
        );
        assert!(
            extra.get("settlements").is_none(),
            "a settlement-less node's greeting must not carry a settlements key: {extra}"
        );
    }

    /// Issue #1210: a route's `request` table rides on the HTTP 402
    /// greeting, at the top level -- beside `resource`, not inside
    /// `accepts[]` -- and a route with none is unaffected.
    #[tokio::test]
    async fn an_unpaid_request_to_a_route_with_a_request_table_is_greeted_with_it() {
        let declared = serde_json::json!({"protocol": "nip90", "kinds": [5096, 5098]});
        let route = StaticRoute::new_priced("g.toon.gas", "http://localhost:4000", 100)
            .unwrap()
            .with_request(declared.clone());
        let plain_route =
            StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
        let connector = Arc::new(Connector::new(
            vec![route, plain_route],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router(connector, test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(sample_prepare("g.toon.gas").encode()))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["request"], declared);
        assert!(
            value["accepts"][0].get("request").is_none(),
            "request describes the resource, not a payment option"
        );

        let plain_request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(sample_prepare("g.example.app").encode()))
            .unwrap();
        let plain_response = app.oneshot(plain_request).await.unwrap();
        let bytes = hyper::body::to_bytes(plain_response.into_body())
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            value.get("request").is_none(),
            "a route with no request table must greet with no request key: {value}"
        );
    }

    /// Issue #803: a `greeting`-flagged PREPARE addressed at a priced route
    /// is greeted (`402`), never routed. `handle_ilp`'s greeting branch
    /// (client-edge-spec.md §1.4) runs entirely before
    /// `Connector::handle_prepare` is ever reached: an unpaid request to a
    /// route this connector serves and prices is answered with terms
    /// "instead of being routed at all" (§1.4), so the flag on a PREPARE
    /// that never gets routed changes nothing else about it. This is what
    /// `packages/announcer`'s x402 probe (`edge-client.ts`'s
    /// `fetchGreeting`) relies on.
    #[tokio::test]
    async fn an_unpaid_greeting_prepare_to_a_priced_route_is_still_greeted() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        let connector = Arc::new(Connector::new(
            vec![route],
            vec![],
            app_client.clone(),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router(connector, test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(greeting_prepare("g.example.app").encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
        assert!(response.headers().get(PAYMENT_REQUIRED_HEADER).is_some());
        assert!(
            app_client.deliveries().is_empty(),
            "the app must never be asked to do the work an unpaid request didn't pay for"
        );
    }

    /// Issue #722: the x402 greeting advertises the session lease backstop
    /// TTL the client session registry actually enforces, so a TS (or any
    /// other language) client can honour freshness <= lease without
    /// duplicating a Rust `pub const`. This pins the advertised value
    /// directly to `SESSION_LEASE_BACKSTOP_TTL` -- changing the constant
    /// changes what is advertised, provably, because both sides of this
    /// assertion read the same const.
    #[tokio::test]
    async fn the_x402_greeting_advertises_the_session_lease_ttl() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
        let connector = Arc::new(Connector::new(
            vec![route],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router(connector, test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(sample_prepare("g.example.app").encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let terms: X402PaymentRequired = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            terms.accepts[0].extra.session_lease_ttl_ms,
            crate::session_registry::SESSION_LEASE_BACKSTOP_TTL.as_millis() as u64,
            "the advertised lease must be exactly what the session registry enforces"
        );
    }

    /// Issue #701 (toon-meta#262 decision 11): a route restricted to BTP
    /// refuses an HTTP request the same way an unpaid request is refused --
    /// terms instead of the app's work -- except here `extra` names which
    /// transport the route actually requires; the route's price is
    /// irrelevant to why this request was refused.
    #[tokio::test]
    async fn an_http_request_to_a_btp_only_route_is_answered_with_the_required_transport() {
        let route = StaticRoute::new_priced_with_transport(
            "g.example.relay",
            "http://localhost:4000",
            1000,
            TransportPolicy::Btp,
        )
        .unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        let connector = Arc::new(Connector::new(
            vec![route],
            vec![],
            app_client.clone(),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router(connector, test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(sample_prepare("g.example.relay").encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let terms: X402PaymentRequired = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            terms.accepts[0].extra.required_transport.as_deref(),
            Some("btp"),
            "the client should learn this route requires BTP"
        );

        assert!(
            app_client.deliveries().is_empty(),
            "a request over the wrong transport must never reach the app"
        );
    }

    /// The mirror case: a route with no transport restriction (the default)
    /// is unaffected -- its unpaid-request greeting carries no
    /// `requiredTransport` at all, exactly the pre-#701 shape.
    #[tokio::test]
    async fn a_route_accepting_both_transports_never_carries_a_required_transport() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
        let connector = Arc::new(Connector::new(
            vec![route],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router(connector, test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(sample_prepare("g.example.app").encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let terms: X402PaymentRequired = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(terms.accepts[0].extra.required_transport, None);
        let extra = serde_json::to_value(&terms.accepts[0].extra).unwrap();
        assert!(
            extra.get("requiredTransport").is_none(),
            "an unrestricted route's greeting must not carry a requiredTransport key: {extra}"
        );
    }

    /// Issue #617: a node WITH a settlement backend answers the greeting
    /// with its channel-opening facts -- the counterparty address, chain,
    /// registry, resolved `TokenNetwork`, token and scale -- so an
    /// unaffiliated buyer can open a channel by ASKING (ADR 0022) instead
    /// of needing an announce this connector never makes.
    #[tokio::test]
    async fn an_unpaid_request_to_a_settling_node_is_answered_with_channel_opening_facts() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
        let connector = Arc::new(Connector::new(
            vec![route],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let terms = X402SettlementTerms {
            chain: "evm:84532".to_string(),
            settlement_address: "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf".to_string(),
            token_network_registry: "0xcc9079ade929b168b54145f6d25262b64fab9d5b".to_string(),
            token_network: "0x1e95493fef46707e034b4a1945f25a8c76a1823d".to_string(),
            token_address: "0x49bee1bca5d15fb0963117923403f9498119a9ce".to_string(),
            decimals: 6,
        };
        let app = router_with_gate_and_terms(
            connector,
            test_signer(),
            None,
            test_gate(ClientChannelRegistry::new()),
            NodeFacts {
                settlements: vec![X402ChainSettlementTerms::Evm(terms.clone())],
                ..Default::default()
            },
        );

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(sample_prepare("g.example.app").encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let answered: X402PaymentRequired = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            answered.accepts[0].extra.settlement.as_ref(),
            Some(&terms),
            "the greeting must carry the node's channel-opening facts verbatim"
        );

        // Issue #632: an EVM-only node's additive `settlements` list is a
        // one-entry list, its entry byte-identical to the legacy object.
        assert_eq!(
            answered.accepts[0].extra.settlements,
            vec![X402ChainSettlementTerms::Evm(terms)],
            "an EVM-only node's settlements list must carry exactly one entry, matching `settlement` verbatim"
        );
    }

    /// Issue #632: a node settling on two chains carries BOTH chains'
    /// channel-opening facts in `extra.settlements`, while the legacy
    /// `extra.settlement` object stays exactly what it was before this
    /// issue -- the EVM entry alone, unaffected by the Solana leg's
    /// presence. This is the demoable slice's acceptance criterion: "a node
    /// with both [settlement.evm] and [settlement.solana] greets with both
    /// chains' facts; an EVM-only node greets with the legacy object
    /// unchanged plus a one-entry list."
    #[tokio::test]
    async fn a_two_chain_node_greets_with_both_chains_facts_and_an_unchanged_legacy_object() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
        let connector = Arc::new(Connector::new(
            vec![route],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let evm_terms = X402SettlementTerms {
            chain: "evm:84532".to_string(),
            settlement_address: "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf".to_string(),
            token_network_registry: "0xcc9079ade929b168b54145f6d25262b64fab9d5b".to_string(),
            token_network: "0x1e95493fef46707e034b4a1945f25a8c76a1823d".to_string(),
            token_address: "0x49bee1bca5d15fb0963117923403f9498119a9ce".to_string(),
            decimals: 6,
        };
        let solana_terms = X402SolanaSettlementTerms {
            chain: "solana".to_string(),
            settlement_address: "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin".to_string(),
            program_id: "2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip".to_string(),
            token_address: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            decimals: 6,
        };
        let app = router_with_gate_and_terms(
            connector,
            test_signer(),
            None,
            test_gate(ClientChannelRegistry::new()),
            NodeFacts {
                settlements: vec![
                    X402ChainSettlementTerms::Evm(evm_terms.clone()),
                    X402ChainSettlementTerms::Solana(solana_terms.clone()),
                ],
                ..Default::default()
            },
        );

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(sample_prepare("g.example.app").encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let answered: X402PaymentRequired = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(
            answered.accepts[0].extra.settlement.as_ref(),
            Some(&evm_terms),
            "the legacy settlement object stays the EVM leg alone, unchanged by the Solana leg"
        );
        assert_eq!(
            answered.accepts[0].extra.settlements,
            vec![
                X402ChainSettlementTerms::Evm(evm_terms),
                X402ChainSettlementTerms::Solana(solana_terms),
            ],
            "a two-chain node's settlements list carries both chains' facts"
        );
    }

    /// A claim header suppresses the greeting even to a priced route --
    /// §1.3's own validation (freshness, value, eventually signature) is
    /// what judges a paid request, not the unpaid-answer path.
    #[tokio::test]
    async fn a_present_claim_header_suppresses_the_greeting() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(route.handler_url(), answered(b"ok"));
        let signer = test_signer();
        let connector = Arc::new(
            Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(signer.clone()),
        );
        let (prepare, _shared_secret) =
            sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
        let app = router_with_gate(
            connector,
            signer,
            None,
            test_gate(claim_headers::test_channels()),
        );

        // A genuinely signed claim: since issue #506/#544 the gate's last
        // stage verifies the signature, so a placeholder one would be
        // refused here for a signature-shaped reason and this test would
        // no longer be observing what it names -- that a *present* claim
        // sends the request down §1.3's validation path rather than the
        // unpaid-greeting one.
        let claim_json = claim_headers::evm_claim_json(1, 100);
        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .header(CLAIM_HEADER, BASE64.encode(claim_json.as_bytes()))
            .body(Body::from(prepare.encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        Fulfill::decode(&bytes).expect("a validly paid claim still reaches the app");
        assert_eq!(app_client.deliveries().len(), 1);
    }

    /// An unpaid request to an explicitly free (`price == 0`) route is
    /// unaffected -- it still reaches the app exactly as it always has,
    /// since there is nothing to charge and so nothing to answer with
    /// terms instead of. This holds because `sealed_sample_prepare` does not
    /// declare `greeting`; see the `a_greeting_prepare_*` tests below (issue
    /// #807) for the different case where the PREPARE itself does.
    #[tokio::test]
    async fn an_unpaid_request_to_a_free_route_still_reaches_the_app() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(route.handler_url(), answered(b"free work"));
        let signer = test_signer();
        let connector = Arc::new(
            Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(signer.clone()),
        );
        let (prepare, _shared_secret) =
            sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
        let app = router(connector, signer);

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(prepare.encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        Fulfill::decode(&bytes).expect("decode fulfill");
        assert_eq!(app_client.deliveries().len(), 1);
    }

    /// A `greeting`-flagged PREPARE with no claim header. Never expected to
    /// reach a route or be fulfilled -- so every greeting test below uses
    /// this rather than [`sample_prepare`], whose `greeting` is `false`.
    fn greeting_prepare(destination: &str) -> Prepare {
        Prepare {
            greeting: true,
            ..sample_prepare(destination)
        }
    }

    /// The core fix for issue #807: `packages/announcer/src/edge-client.ts`'s
    /// `fetchGreeting` probe builds exactly this shape -- a well-formed,
    /// zero-amount, `greeting`-flagged PREPARE -- and expects `402` back,
    /// even when its destination matches no configured route. Before this
    /// fix, such a destination fell through the old `price > 0`-gated
    /// greeting straight into `Connector::handle_prepare`, which -- back
    /// when a bootstrap probe was inferred from a missing execution
    /// condition rather than stated as its own flag -- refused it with an
    /// opaque packet-level reject a bootstrapping client (whose genesis peer
    /// seed is exactly what it is missing, so it has no destination to
    /// probe with that this connector prices) could not act on. A greeting
    /// probe can never be routed at all regardless of destination, so it is
    /// answered the same way a priced route's unpaid request is -- with
    /// terms, never performed.
    #[tokio::test]
    async fn a_greeting_prepare_to_an_unmatched_destination_is_answered_with_the_greeting() {
        let app_client = Arc::new(FakeAppClient::new());
        let connector = Arc::new(Connector::new(
            vec![],
            vec![],
            app_client.clone(),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router(connector, test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(greeting_prepare("g.nowhere").encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::PAYMENT_REQUIRED,
            "a greeting PREPARE must be greeted even when its destination matches no \
             configured route, not F02'd"
        );

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let terms: X402PaymentRequired = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(terms.accepts[0].amount, "0");
        assert!(
            app_client.deliveries().is_empty(),
            "a bootstrap probe must never reach an app"
        );
    }

    /// The same probe shape, but addressing a route this connector matches
    /// and explicitly prices at 0 (free) -- distinguishing "the destination
    /// happens to be free" from "there is no destination at all" above.
    /// Both fall under the same rule: a `greeting`-flagged PREPARE is a
    /// probe regardless of what -- if anything -- it addresses.
    #[tokio::test]
    async fn a_greeting_prepare_to_a_free_route_is_also_answered_with_the_greeting() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(route.handler_url(), answered(b"free work"));
        let connector = Arc::new(Connector::new(
            vec![route],
            vec![],
            app_client.clone(),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router(connector, test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(greeting_prepare("g.example.app").encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
        assert!(
            app_client.deliveries().is_empty(),
            "a greeting probe must never reach the app, priced or not"
        );
    }

    /// The one case issue #807 deliberately leaves alone: a present claim
    /// header suppresses the greeting unconditionally (client-edge-spec.md
    /// §1.4, unchanged by this issue), so a `greeting`-flagged PREPARE that
    /// also carries a claim header still falls through to
    /// `Connector::handle_prepare` -- the flag is never consulted once a
    /// claim is attached, which is what keeps it from ever being usable to
    /// route a packet for free (issue #1269). This particular fixture's
    /// connector has no identity key configured, so the fall-through still
    /// lands on `F01` -- but now for the honest reason that its gift wrap
    /// cannot be opened at all, not because the packet failed some
    /// condition invariant that no longer exists.
    #[tokio::test]
    async fn a_greeting_prepare_with_a_claim_header_falls_through_and_is_routed_normally() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        let connector = Arc::new(Connector::new(
            vec![route],
            vec![],
            app_client.clone(),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router_with_gate(
            connector,
            test_signer(),
            None,
            test_gate(claim_headers::test_channels()),
        );

        let claim_json = claim_headers::evm_claim_json(1, 100);
        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .header(CLAIM_HEADER, BASE64.encode(claim_json.as_bytes()))
            .body(Body::from(greeting_prepare("g.example.app").encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        // An ILP-level outcome, even a reject, is always HTTP 200 (client-edge-spec.md §1.1).
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let reject = Reject::decode(&bytes).expect("decode reject");
        assert_eq!(reject.code.as_str(), "F01");
        assert!(
            reject.message.contains("identity"),
            "F01 here comes from this fixture's connector having no identity key to open the \
             gift wrap with, not from any longer-retired condition check: {}",
            reject.message
        );
        assert!(
            app_client.deliveries().is_empty(),
            "a claim-bearing greeting PREPARE must not reach the app either"
        );
    }

    // ── `GET /ilp`: the node self-description (ADR 0050, issue #1080) ─────

    /// A router whose node facts are fully populated, over one priced route --
    /// the shape every assertion below reads a document out of.
    fn described_node() -> Router {
        let route = StaticRoute::new_priced_with_transport(
            "g.example.app",
            "http://localhost:4000",
            100,
            TransportPolicy::Btp,
        )
        .unwrap();
        let connector = Arc::new(Connector::new(
            vec![route],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        router_with_node_facts(
            connector,
            test_signer(),
            None,
            Arc::new(test_gate(ClientChannelRegistry::new())),
            NodeFacts {
                ilp_addresses: vec!["g.example.app".to_string()],
                http_endpoint: Some("https://node.example/ilp".to_string()),
                btp_endpoint: Some("wss://node.example/ilp/btp".to_string()),
                peer_carriages: vec!["btp".to_string()],
                settlements: vec![X402ChainSettlementTerms::Evm(X402SettlementTerms {
                    chain: "evm:84532".to_string(),
                    settlement_address: "0xf29fd62c4848b9573c9b90adbf61b664f386d9cf".to_string(),
                    token_network_registry: "0xcc9079ade929b168b54145f6d25262b64fab9d5b"
                        .to_string(),
                    token_network: "0x1e95493fef46707e034b4a1945f25a8c76a1823d".to_string(),
                    token_address: "0x49bee1bca5d15fb0963117923403f9498119a9ce".to_string(),
                    decimals: 6,
                })],
            },
            DEFAULT_BTP_SESSION_WINDOW,
            None,
            Arc::from([]),
        )
    }

    async fn self_description_of(app: Router) -> serde_json::Value {
        let request = Request::builder()
            .method("GET")
            .uri("/ilp")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "GET /ilp is free and unauthenticated (ND-02)"
        );
        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        serde_json::from_slice(&bytes).expect("a JSON self-description")
    }

    /// ND-01/ND-05: a `GET` on this connector's own URL answers the facts a
    /// stranger needs to transact with it -- no ILP packet, no encoder, no
    /// protocol knowledge -- and every one of them is true of THIS node.
    #[tokio::test]
    async fn a_get_on_the_connectors_own_url_returns_its_self_description() {
        let document = self_description_of(described_node()).await;

        assert_eq!(
            document["ilpAddresses"],
            serde_json::json!(["g.example.app"])
        );
        assert_eq!(
            document["httpEndpoint"],
            serde_json::json!("https://node.example/ilp")
        );
        assert_eq!(
            document["btpEndpoint"],
            serde_json::json!("wss://node.example/ilp/btp")
        );
        assert_eq!(document["peerCarriages"], serde_json::json!(["btp"]));
        assert_eq!(
            document["settlements"][0]["chain"],
            serde_json::json!("evm:84532")
        );
        assert_eq!(
            document["settlements"][0]["tokenNetworkRegistry"],
            serde_json::json!("0xcc9079ade929b168b54145f6d25262b64fab9d5b"),
            "the channel-opening facts come from the settlement backend that verified them \
             against a chain at startup, never a second declaration (ND-07)"
        );
        assert_eq!(
            document["routes"],
            serde_json::json!([{ "prefix": "g.example.app", "price": "100" }])
        );
        assert_eq!(
            document["requiredTransport"],
            serde_json::json!("btp"),
            "the route covering this node's own address is pinned to BTP, so the document says \
             so -- the defect ADR 0050 closes by construction"
        );
        assert_eq!(document["supportedVersions"], serde_json::json!([1]));
        assert_eq!(document["defaultVersion"], serde_json::json!(1));
    }

    /// Issue #1210: a route's `request` table shows up on its own entry in
    /// the self-description as JSON equal to the table, and a route with
    /// none has no `request` key at all.
    #[tokio::test]
    async fn the_self_description_carries_each_routes_request_table() {
        let declared = serde_json::json!({"protocol": "nip90", "kinds": [5096, 5098]});
        let with_request = StaticRoute::new_priced("g.toon.gas", "http://localhost:4000", 100)
            .unwrap()
            .with_request(declared.clone());
        let without_request =
            StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
        let connector = Arc::new(Connector::new(
            vec![with_request, without_request],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router(connector, test_signer());

        let document = self_description_of(app).await;

        let routes = document["routes"].as_array().unwrap();
        let gas = routes
            .iter()
            .find(|route| route["prefix"] == "g.toon.gas")
            .unwrap();
        assert_eq!(gas["request"], declared);
        let plain = routes
            .iter()
            .find(|route| route["prefix"] == "g.example.app")
            .unwrap();
        assert!(
            plain.get("request").is_none(),
            "a route with no request table must have no request key: {plain}"
        );
    }

    /// ND-06: the edge identity -- the key a packet's payload is sealed to
    /// (ADR 0018) -- must be published, because a route whose terminating
    /// identity is unpublished cannot be delivered to at all. It is the same
    /// key `GET /ilp/identity` has always answered with; what changes is that
    /// it now rides the ONE document a stranger reads.
    #[tokio::test]
    async fn the_self_description_publishes_the_key_a_packet_is_sealed_to() {
        let signer = test_signer();
        let expected = format!(
            "0x{}",
            hex_encode(&signer.public_key().expect("public key"))
        );
        let connector = Arc::new(Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router_with_node_facts(
            connector,
            signer.clone(),
            None,
            Arc::new(test_gate(ClientChannelRegistry::new())),
            NodeFacts::default(),
            DEFAULT_BTP_SESSION_WINDOW,
            None,
            Arc::from([]),
        );

        let document = self_description_of(app).await;

        assert_eq!(
            document["edgeIdentity"]["publicKey"],
            serde_json::json!(expected)
        );
        assert_eq!(
            document["edgeIdentity"]["keyId"],
            serde_json::json!(signer.key_id())
        );
    }

    /// The per-peer packet cap the two tests below install. Four digits, so a
    /// random public key's hex can spell it by accident -- which is exactly
    /// what flaked this suite (#1206).
    const UNPUBLISHED_PEER_PACKET_CAP: u64 = 9_999;

    /// A node with one forwarded route whose counterparty carries a packet
    /// cap, rendered by whichever signer the caller hands in -- the two tests
    /// below differ only in that signer.
    fn node_with_a_capped_peer(signer: Arc<dyn Signer>) -> Router {
        let peer_route = PeerRoute::new_priced("g.peer.somewhere", "a-counterparty", 7);
        let connector = Connector::new(
            vec![],
            vec![peer_route],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        )
        .with_peer_packet_caps([("a-counterparty".to_string(), UNPUBLISHED_PEER_PACKET_CAP)]);
        router_with_node_facts(
            Arc::new(connector),
            signer,
            None,
            Arc::new(test_gate(ClientChannelRegistry::new())),
            NodeFacts::default(),
            DEFAULT_BTP_SESSION_WINDOW,
            None,
            Arc::from([]),
        )
    }

    /// ND-08/ND-09, asserted on the bytes a client actually receives: nothing
    /// about software BEHIND this connector, and nothing about who it peers
    /// with or how far it trusts them. A `relayUrl` was the last place ADR
    /// 0046's removed relay assumption survived; a cap is discovered by being
    /// refused (ND-10), never by being published.
    #[tokio::test]
    async fn the_self_description_carries_no_relay_no_peer_and_no_cap() {
        let document = self_description_of(node_with_a_capped_peer(test_signer())).await;
        let rendered = serde_json::to_string(&document).unwrap();

        assert!(
            !rendered.contains("a-counterparty"),
            "a forwarded route's price may be published; WHO carries it may not (ND-09): {rendered}"
        );
        assert!(
            !json_carries_amount(&document, UNPUBLISHED_PEER_PACKET_CAP),
            "a cap is discovered by its T04 refusal, never by being published (ND-10): {document}"
        );
        for forbidden in ["relay", "ttl", "notice"] {
            assert!(
                !rendered.to_lowercase().contains(forbidden),
                "the document must not carry '{forbidden}': {rendered}"
            );
        }
        assert_eq!(
            document["routes"],
            serde_json::json!([{ "prefix": "g.peer.somewhere", "price": "7" }]),
            "the forwarded route's own price is a fact about what THIS node charges, and is \
             answered at GET /ilp/routes/price already"
        );
    }

    /// Regression for #1206: the cap check above must fail on a leaked
    /// **field**, never on the edge signer's public key incidentally spelling
    /// the same digits. `test_signer()` generates a fresh secp256k1 key per
    /// run, and a 130-hex-char string spells the cap about 1 run in 540 --
    /// rare enough to survive a normal gate run, common enough to have flaked
    /// this suite. Rather than wait on those odds, generate keys until one
    /// actually collides and prove the document still passes with it
    /// installed.
    #[tokio::test]
    async fn the_no_cap_assertion_survives_a_signer_key_that_spells_the_cap() {
        let spelled_cap = UNPUBLISHED_PEER_PACKET_CAP.to_string();
        let colliding_signer = (0..10_000)
            .map(|generation| LocalSigner::generate(format!("test-signer-{generation}")))
            .find(|signer| {
                hex_encode(&signer.public_key().expect("public key")).contains(&spelled_cap)
            })
            .expect("10,000 random keys should turn up at least one cap-spelling collision");

        let document =
            self_description_of(node_with_a_capped_peer(Arc::new(colliding_signer))).await;

        assert!(
            !json_carries_amount(&document, UNPUBLISHED_PEER_PACKET_CAP),
            "the signer's public key spelling the cap must not trip the assertion: {document}"
        );
    }

    /// Walks a parsed JSON document for `amount` at any depth, as a number or
    /// as the decimal string this document renders amounts with -- a
    /// `RoutePrice` publishes a price as `"7"`, not `7`, so a leaked cap would
    /// most likely be a string too. Reading the document's shape rather than
    /// its rendered bytes is what keeps a hex value -- a public key, an
    /// address -- from spelling the same digits and failing the check (#1206).
    fn json_carries_amount(value: &serde_json::Value, amount: u64) -> bool {
        match value {
            serde_json::Value::Number(number) => number.as_u64() == Some(amount),
            serde_json::Value::String(text) => *text == amount.to_string(),
            serde_json::Value::Array(items) => {
                items.iter().any(|item| json_carries_amount(item, amount))
            }
            serde_json::Value::Object(fields) => fields
                .values()
                .any(|field| json_carries_amount(field, amount)),
            _ => false,
        }
    }

    /// ND-03, and the one prohibition ADR 0050 states rather than implies: the
    /// document endpoint never accepts a write. `POST /ilp` is unchanged --
    /// it is the packet endpoint -- so this asserts the shape of that
    /// unchanged-ness rather than a 405: a POST is still an ILP exchange,
    /// answered with terms, and never a request for peering.
    #[tokio::test]
    async fn a_post_to_the_same_url_is_still_the_packet_endpoint_and_never_a_write() {
        let app = described_node();
        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(greeting_prepare("g.example.app").encode()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            StatusCode::PAYMENT_REQUIRED,
            "POST /ilp answers an unpaid request with terms, exactly as it did before GET was \
             mounted beside it"
        );
    }

    /// ND-04: no caching contract and no TTL -- the answer is projected from
    /// live state on each request. Asserted by changing the live state and
    /// asking again: a route leased in between shows up without a restart,
    /// which no cached or boot-time snapshot could do.
    #[tokio::test]
    async fn the_self_description_is_generated_from_live_state_on_each_request() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
        // Issue #1217: `upsert_runtime_peer_route` below now checks for a
        // CLIENT-role hop, not merely a peer-role channel binding -- `covering`
        // gives this peering one, the same way a `[[pay_channels]]` row would.
        let connector = Arc::new(covering(
            Connector::new(
                vec![route],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ),
            "later",
        ));
        let facts = NodeFacts::default();
        let first = self_description_of(router_with_node_facts(
            connector.clone(),
            test_signer(),
            None,
            Arc::new(test_gate(ClientChannelRegistry::new())),
            facts.clone(),
            DEFAULT_BTP_SESSION_WINDOW,
            None,
            Arc::from([]),
        ))
        .await;
        assert_eq!(
            first["routes"],
            serde_json::json!([{ "prefix": "g.example.app", "price": "100" }])
        );

        connector
            .upsert_runtime_peer(
                "later",
                // A peering with no channel bound to it is refused at
                // write time (ADR 0058), so the route this test is
                // actually about needs a peering that could carry it.
                RuntimePeering {
                    endpoint: Some("https://later.example/ilp".to_string()),
                    channels: vec![RuntimePeerChannel::Evm {
                        channel_id: format!("0x{}", "ab".repeat(32)),
                        counterparty_key: "0x00000000000000000000000000000000000000aa".to_string(),
                        chain_id: 31337,
                        token_network: "0x00000000000000000000000000000000000000bb".to_string(),
                    }],
                    ..RuntimePeering::default()
                },
            )
            .expect("add a runtime peer");
        connector
            .upsert_runtime_peer_route("g.later", "later", Price::flat(42))
            .expect("add a runtime route");

        let second = self_description_of(router_with_node_facts(
            connector,
            test_signer(),
            None,
            Arc::new(test_gate(ClientChannelRegistry::new())),
            facts,
            DEFAULT_BTP_SESSION_WINDOW,
            None,
            Arc::from([]),
        ))
        .await;

        assert_eq!(
            second["routes"],
            serde_json::json!([
                { "prefix": "g.example.app", "price": "100" },
                { "prefix": "g.later", "price": "42" },
            ]),
            "a route written after boot is in the next answer: nothing here is snapshotted"
        );
    }

    /// ND-11: the greeting is a PROJECTION of the same source, so the two
    /// cannot disagree. Read both surfaces off one router and compare the node
    /// facts field by field -- which is the whole property, since the failure
    /// this replaces was two descriptions of one node drifting apart.
    #[tokio::test]
    async fn the_greeting_projects_the_same_node_facts_the_document_publishes() {
        let document = self_description_of(described_node()).await;

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(greeting_prepare("g.example.app").encode()))
            .unwrap();
        let response = described_node().oneshot(request).await.unwrap();
        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let greeting: X402PaymentRequired = serde_json::from_slice(&bytes).unwrap();
        let extra = &greeting.accepts[0].extra;

        assert_eq!(
            serde_json::to_value(&extra.ilp_addresses).unwrap(),
            document["ilpAddresses"]
        );
        assert_eq!(
            serde_json::to_value(&extra.btp_endpoint).unwrap(),
            document["btpEndpoint"]
        );
        assert_eq!(
            serde_json::to_value(&extra.settlements).unwrap(),
            document["settlements"]
        );
        assert_eq!(
            serde_json::to_value(extra.settlement.as_ref()).unwrap(),
            document["settlements"][0],
            "the greeting's legacy one-chain object is the list's own EVM entry, derived rather \
             than carried beside it"
        );
    }

    /// ND-12: the greeting keeps its own job. Fields that exist only to serve
    /// an in-flight transaction stay there and are NOT promoted into the node
    /// description -- `payTo`, the route's price and the session lease TTL are
    /// terms for one specific priced route, in band, to a client that just
    /// tried to use it.
    #[tokio::test]
    async fn the_documents_projection_does_not_swallow_the_greetings_own_terms() {
        let document = self_description_of(described_node()).await;

        for in_flight in ["payTo", "maxTimeoutSeconds", "sessionLeaseTtlMs"] {
            assert!(
                !serde_json::to_string(&document)
                    .unwrap()
                    .contains(in_flight),
                "'{in_flight}' serves one transaction and belongs to the greeting alone"
            );
        }
    }

    /// Issue #807's second half, now ADR 0050's ND-11: the greeting carries
    /// this node's own ILP address(es) and BTP endpoint when `[node]`
    /// configures them, so a client whose genesis peer seed is stale or
    /// missing can bootstrap from the answer alone. Unlike the legacy
    /// `extra.ilpAddress`, which echoes back whatever `destination` the
    /// probing PREPARE named, these are the node's own authoritative facts
    /// regardless of what was probed -- and they are read off the very value
    /// `GET /ilp` serves, never assembled beside it.
    #[tokio::test]
    async fn the_greeting_carries_this_nodes_own_facts_when_configured() {
        let connector = Arc::new(Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router_with_node_facts(
            connector,
            test_signer(),
            None,
            Arc::new(test_gate(ClientChannelRegistry::new())),
            NodeFacts {
                ilp_addresses: vec!["g.toon.apex".to_string(), "g.toon.apex.alt".to_string()],
                btp_endpoint: Some("wss://apex.example/ilp/btp".to_string()),
                ..Default::default()
            },
            DEFAULT_BTP_SESSION_WINDOW,
            None,
            Arc::from([]),
        );

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(greeting_prepare("g.whatever").encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let terms: X402PaymentRequired = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            terms.accepts[0].extra.ilp_addresses,
            vec!["g.toon.apex".to_string(), "g.toon.apex.alt".to_string()],
            "ilpAddresses must be this node's own configured addresses, not an echo"
        );
        assert_eq!(
            terms.accepts[0].extra.btp_endpoint.as_deref(),
            Some("wss://apex.example/ilp/btp")
        );
        // The legacy field is untouched: still an echo of the probed destination.
        assert_eq!(terms.accepts[0].extra.ilp_address, "g.whatever");
    }

    /// The absence half of the test above: a node with no `[node]` section
    /// configured ([`NodeFacts::default()`], [`router`]'s default) keeps the
    /// pre-#807 shape exactly -- no `ilpAddresses`/`btpEndpoint` key at all,
    /// not empty/null ones, so a parser written before those fields existed
    /// is unaffected.
    #[tokio::test]
    async fn the_greeting_omits_this_nodes_own_facts_when_not_configured() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
        let connector = Arc::new(Connector::new(
            vec![route],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let app = router(connector, test_signer());

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(sample_prepare("g.example.app").encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let terms: X402PaymentRequired = serde_json::from_slice(&bytes).unwrap();
        assert!(terms.accepts[0].extra.ilp_addresses.is_empty());
        assert!(terms.accepts[0].extra.btp_endpoint.is_none());

        let extra = serde_json::to_value(&terms.accepts[0].extra).unwrap();
        assert!(
            extra.get("ilpAddresses").is_none(),
            "a node with no [announce] must not carry an ilpAddresses key: {extra}"
        );
        assert!(
            extra.get("btpEndpoint").is_none(),
            "a node with no [announce] must not carry a btpEndpoint key: {extra}"
        );
    }

    /// End-to-end claim ingest (issue #504, #506/#544): a claim presented in
    /// `ILP-Payment-Channel-Claim`(`-Wrapped`) is parsed, structurally
    /// validated, checked for freshness/watermark and cryptographically
    /// verified before the packet is routed, exercised at this crate's real
    /// HTTP seam rather than against `ClientClaimGate` directly.
    mod claim_headers {
        use super::*;
        use libsecp256k1::{Message, PublicKey, SecretKey};

        const EVM_CHAIN_ID: u64 = 8453;
        const EVM_TOKEN_NETWORK_ADDRESS: [u8; 20] = [0x42; 20];

        /// base58 of `[7u8; 32]`, the settlement program every Solana channel
        /// in this module lives under -- so it is also the `programId` a
        /// conforming claim on one of them declares
        /// (`client-edge-spec.md` §1.3: the settlement program the
        /// `channelAccount` lives under).
        ///
        /// Every Solana claim below used to declare its own *payer's* public
        /// key here, which names no program at all. Nothing failed, because
        /// the field grants nothing -- the signature is checked against the
        /// channel's program (ADR 0053) -- but these fixtures are the closest
        /// thing this crate has to a written-down example of a real claim,
        /// and one that cannot be conformed to is worse than none (issue
        /// #1127).
        const SOLANA_CHANNEL_PROGRAM_BASE58: &str = "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx";

        /// The literal above is base58 of `[7u8; 32]` -- the bytes the
        /// balance proofs below are signed under and the
        /// `FakeSolanaChannelSource` records. Nothing else in this module
        /// says so, and a claim declaring a program its channel does not live
        /// under is precisely what these fixtures are no longer supposed to
        /// be.
        #[test]
        fn the_declared_solana_program_is_the_one_these_channels_live_under() {
            assert_eq!(
                bs58::decode(SOLANA_CHANNEL_PROGRAM_BASE58)
                    .into_vec()
                    .expect("a base58 literal"),
                [7u8; 32],
            );
        }

        /// The one channel every claim below is presented on, recorded with
        /// [`evm_signer`]'s address as its counterparty (issue #558) -- a
        /// claim signed by anyone else, or naming any other channel, is
        /// refused however well-formed it is.
        pub(super) fn test_channels() -> ClientChannelRegistry {
            let (_secret, counterparty) = evm_signer();
            let mut channels = ClientChannelRegistry::new();
            channels
                .record_evm(
                    &"ab".repeat(32),
                    EvmChannel {
                        counterparty,
                        chain_id: EVM_CHAIN_ID,
                        token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                        // Declared, so exempt from the collateral cap
                        // (issue #646) exactly as config records are.
                        deposit_floor: crate::DepositFloor::Unknown,
                    },
                )
                .expect("a 32-byte hex channel id");
            channels
        }

        /// A fixed, deterministic EVM keypair every genuine claim below is
        /// signed with, so each test's own signature verifies.
        fn evm_signer() -> (SecretKey, connector_signer::Address) {
            let secret = SecretKey::parse(&[9u8; 32]).unwrap();
            let public = PublicKey::from_secret_key(&secret);
            (
                secret,
                connector_signer::derive_evm_address(&public.serialize()),
            )
        }

        /// Sign `digest` exactly the way a real EVM wallet would (a 65-byte
        /// `r || s || v` signature, `v` in the conventional `{27, 28}` range).
        fn sign_evm(secret: &SecretKey, digest: &[u8; 32]) -> Vec<u8> {
            let message = Message::parse(digest);
            let (signature, recovery_id) = libsecp256k1::sign(&message, secret);
            let mut bytes = signature.serialize().to_vec();
            let recovery_byte: u8 = recovery_id.into();
            bytes.push(recovery_byte + 27);
            bytes
        }

        /// An EVM claim JSON carrying whatever `signature` hex string is
        /// given verbatim, genuine or not.
        fn evm_claim_json_with_signature(
            nonce: u64,
            transferred_amount: u64,
            signature_hex: &str,
        ) -> String {
            let (_secret, address) = evm_signer();
            evm_claim_json_with_signature_and_signer(
                nonce,
                transferred_amount,
                signature_hex,
                &address,
            )
        }

        /// As [`evm_claim_json_with_signature`], but declaring whatever
        /// `signer_address` it is given -- what a forger does (issue #558):
        /// sign with a key of one's own and name oneself the payer.
        fn evm_claim_json_with_signature_and_signer(
            nonce: u64,
            transferred_amount: u64,
            signature_hex: &str,
            signer_address: &connector_signer::Address,
        ) -> String {
            let address = signer_address;
            format!(
                r#"{{
                    "version": "1.0",
                    "blockchain": "evm",
                    "messageId": "msg-{nonce}",
                    "timestamp": "2026-02-02T12:00:00.000Z",
                    "senderId": "peer-bob",
                    "channelId": "0x{channel}",
                    "nonce": {nonce},
                    "transferredAmount": "{transferred_amount}",
                    "lockedAmount": "0",
                    "locksRoot": "0x{zeros}",
                    "signature": "{signature_hex}",
                    "signerAddress": "{address}",
                    "chainId": {EVM_CHAIN_ID},
                    "tokenNetworkAddress": "{token_network_address}"
                }}"#,
                channel = "ab".repeat(32),
                zeros = "0".repeat(64),
                address = connector_signer::to_hex(address),
                token_network_address = connector_signer::to_hex(&EVM_TOKEN_NETWORK_ADDRESS),
            )
        }

        /// A claim over the recorded channel, genuinely and correctly
        /// signed -- by a key that is not that channel's counterparty, and
        /// declaring itself the payer. The forger of issue #558.
        fn forged_evm_claim_json(nonce: u64, transferred_amount: u64) -> String {
            let secret = SecretKey::parse(&[0x5a; 32]).unwrap();
            let address = connector_signer::derive_evm_address(
                &PublicKey::from_secret_key(&secret).serialize(),
            );
            let channel = "ab".repeat(32);
            let mut channel_id = [0u8; 32];
            channel_id.copy_from_slice(&hex::decode(&channel).unwrap());
            let proof = connector_signer::EvmBalanceProof {
                channel_id,
                nonce,
                transferred_amount: u128::from(transferred_amount),
                locked_amount: 0,
                locks_root: [0u8; 32],
                chain_id: EVM_CHAIN_ID,
                token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
            };
            let signature = sign_evm(&secret, &connector_signer::evm_balance_proof_digest(&proof));
            evm_claim_json_with_signature_and_signer(
                nonce,
                transferred_amount,
                &format!("0x{}", hex_encode(&signature)),
                &address,
            )
        }

        /// An EVM claim JSON with a genuine EIP-712 signature over its own
        /// fields (issue #506/#544) -- every test using this helper
        /// exercises the real verification path, not a bypass.
        pub(super) fn evm_claim_json(nonce: u64, transferred_amount: u64) -> String {
            let channel = "ab".repeat(32);
            let mut channel_id = [0u8; 32];
            channel_id.copy_from_slice(&hex::decode(&channel).unwrap());
            let (secret, _address) = evm_signer();
            let proof = connector_signer::EvmBalanceProof {
                channel_id,
                nonce,
                transferred_amount: u128::from(transferred_amount),
                locked_amount: 0,
                locks_root: [0u8; 32],
                chain_id: EVM_CHAIN_ID,
                token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
            };
            let signature = sign_evm(&secret, &connector_signer::evm_balance_proof_digest(&proof));
            evm_claim_json_with_signature(
                nonce,
                transferred_amount,
                &format!("0x{}", hex_encode(&signature)),
            )
        }

        fn mina_claim_json() -> &'static str {
            r#"{
                "version": "1.0",
                "blockchain": "mina",
                "messageId": "claim-1",
                "timestamp": "2026-02-02T12:00:00.000Z",
                "senderId": "peer-dave",
                "zkAppAddress": "irrelevant",
                "tokenId": "1",
                "balanceCommitment": "abc",
                "nonce": 1,
                "proof": "AAAA",
                "salt": "salt"
            }"#
        }

        pub(super) fn request_with_claim_header(
            prepare: &Prepare,
            header_name: &str,
            claim_json: &str,
        ) -> Request<Body> {
            let encoded = BASE64.encode(claim_json.as_bytes());
            Request::builder()
                .method("POST")
                .uri("/ilp")
                .header(header_name, encoded)
                .body(Body::from(prepare.encode()))
                .unwrap()
        }

        #[tokio::test]
        async fn a_fresh_plaintext_claim_lets_the_packet_reach_the_app() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );
            let (prepare, _shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
            let app = router_with_gate(connector, signer, None, test_gate(test_channels()));

            let request =
                request_with_claim_header(&prepare, CLAIM_HEADER, &evm_claim_json(1, 100));
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);

            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            Fulfill::decode(&bytes).expect("decode fulfill");
            assert_eq!(app_client.deliveries().len(), 1);
        }

        #[tokio::test]
        async fn a_replayed_claim_nonce_rejects_before_reaching_the_app() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );
            let app = router_with_gate(connector, signer.clone(), None, test_gate(test_channels()));

            let (first_prepare, _shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
            let first =
                request_with_claim_header(&first_prepare, CLAIM_HEADER, &evm_claim_json(5, 500));
            let response = app.clone().oneshot(first).await.unwrap();
            Fulfill::decode(&hyper::body::to_bytes(response.into_body()).await.unwrap())
                .expect("first claim accepted");

            // The replay is rejected on the claim nonce alone, before the
            // envelope would ever need to open -- plaintext is fine here.
            let replay = request_with_claim_header(
                &sample_prepare("g.example.app"),
                CLAIM_HEADER,
                &evm_claim_json(5, 999),
            );
            let response = app.oneshot(replay).await.unwrap();
            // An ILP-level outcome, even a reject, is always HTTP 200.
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code.as_str(), "F01");

            // The replay never reached the app: still exactly one delivery.
            assert_eq!(app_client.deliveries().len(), 1);
        }

        #[tokio::test]
        async fn a_malformed_claim_header_rejects_with_f01_before_reaching_the_app() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let connector = Arc::new(Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ));
            let app = router_with_gate(connector, test_signer(), None, test_gate(test_channels()));

            let request = request_with_claim_header(
                &sample_prepare("g.example.app"),
                CLAIM_HEADER,
                r#"{"version":"1.0","blockchain":"evm"}"#,
            );
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code.as_str(), "F01");
            assert!(reject.message.contains("structurally invalid"));
            assert!(app_client.deliveries().is_empty());
        }

        #[tokio::test]
        async fn a_mina_claim_is_rejected_with_a_reason_distinguishable_from_malformed() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            let connector = Arc::new(Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ));
            let app = router_with_gate(connector, test_signer(), None, test_gate(test_channels()));

            let request = request_with_claim_header(
                &sample_prepare("g.example.app"),
                CLAIM_HEADER,
                mina_claim_json(),
            );
            let response = app.oneshot(request).await.unwrap();
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code.as_str(), "F01");
            assert!(reject.message.contains("ADR 0002"));
            assert!(!reject.message.contains("structurally invalid"));
            assert!(app_client.deliveries().is_empty());
        }

        #[tokio::test]
        async fn a_wrapped_claim_is_unwrapped_and_lets_the_packet_reach_the_app() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let identity_signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(identity_signer.clone()),
            );

            // The claim's own NIP-59 wrap/unwrap key -- unrelated to the
            // connector's gift-wrap identity above (ADR 0018): this pair
            // protects the *claim header's* privacy (issue #504's §1.3),
            // not the packet payload.
            let sender_secret = SecretKey::parse(&[1u8; 32]).unwrap();
            let receiver_secret_bytes = [2u8; 32];
            let receiver_secret = SecretKey::parse(&receiver_secret_bytes).unwrap();
            let receiver_public = PublicKey::from_secret_key(&receiver_secret);

            let claim_json = evm_claim_json(1, 100);
            let wrapped = connector_signer::wrap_claim(
                claim_json.as_bytes(),
                &sender_secret,
                &receiver_public.serialize(),
            )
            .expect("wrap");
            let envelope_json = format!(
                r#"{{"ephemeralPublicKey":"{}","encryptedPayload":"{}","timestamp":0,"version":"1.0"}}"#,
                hex_encode(&wrapped.ephemeral_public_key),
                BASE64.encode(&wrapped.encrypted_payload),
            );

            let (prepare, _shared_secret) =
                sealed_sample_prepare("g.example.app", &identity_signer.public_key().unwrap());
            let app = router_with_gate(
                connector,
                identity_signer,
                Some(receiver_secret_bytes),
                test_gate(test_channels()),
            );
            let request = request_with_claim_header(&prepare, CLAIM_WRAPPED_HEADER, &envelope_json);
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            Fulfill::decode(&bytes).expect("decode fulfill");
            assert_eq!(app_client.deliveries().len(), 1);
        }

        #[tokio::test]
        async fn a_wrapped_claim_with_no_configured_receiver_key_is_refused() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            let connector = Arc::new(Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ));
            // `router`, not `router_with_wrap_key`: no receiver key configured.
            let app = router(connector, test_signer());

            let envelope_json = r#"{"ephemeralPublicKey":"04","encryptedPayload":"AAAA","timestamp":0,"version":"1.0"}"#;
            let request = request_with_claim_header(
                &sample_prepare("g.example.app"),
                CLAIM_WRAPPED_HEADER,
                envelope_json,
            );
            let response = app.oneshot(request).await.unwrap();
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code.as_str(), "F01");
            assert!(reject.message.contains("not configured to unwrap"));
            assert!(app_client.deliveries().is_empty());
        }

        /// A claim advancing by at least a priced route's price is
        /// accepted and the packet is delivered (issue #522).
        #[tokio::test]
        async fn a_claim_covering_the_routes_price_is_accepted_and_delivered() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );
            let (prepare, _shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
            let app = router_with_gate(connector, signer, None, test_gate(test_channels()));

            let request =
                request_with_claim_header(&prepare, CLAIM_HEADER, &evm_claim_json(1, 100));
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);

            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            Fulfill::decode(&bytes).expect("decode fulfill");
            assert_eq!(app_client.deliveries().len(), 1);
        }

        /// Issue #869: a packet refused for its envelope's own target
        /// shape (`AppOutcome::Refused`, F00) must never advance the
        /// covering claim's watermark -- the payer is told `accumulated_
        /// cost` 0 and the app is never asked to do anything, so the claim
        /// it rode in on must still be spendable afterward. Proven the same
        /// way `a_replayed_claim_nonce_rejects_before_reaching_the_app`
        /// proves the opposite direction: the identical claim, resent with
        /// a target that resolves cleanly, is still accepted -- which is
        /// only possible if the first, refused attempt left the watermark
        /// untouched.
        #[tokio::test]
        async fn a_claim_covering_a_packet_refused_for_envelope_shape_is_never_spent() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );
            let app = router_with_gate(connector, signer.clone(), None, test_gate(test_channels()));
            let claim = evm_claim_json(1, 100);

            let (escaping_prepare, _shared_secret) = sealed_sample_prepare_with_target(
                "g.example.app",
                "/write",
                &signer.public_key().unwrap(),
            );
            let escaping_request =
                request_with_claim_header(&escaping_prepare, CLAIM_HEADER, &claim);
            let response = app.clone().oneshot(escaping_request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code.as_str(), "F00");
            assert_eq!(reject.accumulated_cost, 0);
            assert!(
                app_client.deliveries().is_empty(),
                "an escaping target must never reach the app"
            );

            let (valid_prepare, _shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
            let valid_request = request_with_claim_header(&valid_prepare, CLAIM_HEADER, &claim);
            let response = app.oneshot(valid_request).await.unwrap();
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            Fulfill::decode(&bytes).expect("the unspent claim is still accepted");
            assert_eq!(app_client.deliveries().len(), 1);
        }

        /// Issue #701: transport policy is checked before payment is
        /// considered at all -- a fully valid, correctly-priced claim over
        /// HTTP does not make a BTP-only route reachable that way. The
        /// request is refused with the same self-diagnosing terms an
        /// unpaid request gets, and the app is never asked to do the work.
        #[tokio::test]
        async fn a_valid_claim_to_a_btp_only_route_is_still_refused_with_wrong_transport_terms() {
            let route = StaticRoute::new_priced_with_transport(
                "g.example.relay",
                "http://localhost:4000",
                100,
                TransportPolicy::Btp,
            )
            .unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );
            let (prepare, _shared_secret) =
                sealed_sample_prepare("g.example.relay", &signer.public_key().unwrap());
            let app = router_with_gate(connector, signer, None, test_gate(test_channels()));

            let request =
                request_with_claim_header(&prepare, CLAIM_HEADER, &evm_claim_json(1, 100));
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let terms: X402PaymentRequired = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                terms.accepts[0].extra.required_transport.as_deref(),
                Some("btp")
            );

            assert!(
                app_client.deliveries().is_empty(),
                "a valid claim over the wrong transport must never reach the app"
            );
        }

        /// A claim advancing by less than a priced route's price is
        /// refused as underpayment (F03), distinguishably from a stale,
        /// malformed or unverifiable claim (all F01), and never reaches
        /// the app (issue #522).
        #[tokio::test]
        async fn a_claim_underpaying_the_routes_price_is_refused_as_underpayment() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let connector = Arc::new(Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ));
            let app = router_with_gate(connector, test_signer(), None, test_gate(test_channels()));

            let request = request_with_claim_header(
                &sample_prepare("g.example.app"),
                CLAIM_HEADER,
                &evm_claim_json(1, 99),
            );
            let response = app.oneshot(request).await.unwrap();
            // An ILP-level outcome, even a reject, is always HTTP 200.
            assert_eq!(response.status(), StatusCode::OK);

            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code.as_str(), "F03");
            assert_ne!(reject.code.as_str(), "F01");
            assert!(app_client.deliveries().is_empty());
        }

        /// ADR 0065 (issue #984) at the seam that actually takes the money:
        /// two packets to ONE route, differing only in how much payload they
        /// carry, are charged different amounts -- and the bigger one is not
        /// paid for by a claim that covers the route's base.
        ///
        /// This is the whole point of the record. Under ADR 0020 both packets
        /// cost `100` and the second was carried at a loss; the deployed
        /// workaround (a second route at a second handler) could not stop a
        /// large packet being sent to the cheap one, which is what #984
        /// measured on a live node.
        #[tokio::test]
        async fn two_packets_of_different_sizes_to_one_route_are_charged_differently() {
            let route = StaticRoute::new_scheduled(
                "g.example.app",
                "http://localhost:4000",
                Price::scheduled(100, 10),
            )
            .unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );
            let receiver = signer.public_key().unwrap();

            let (small, _) = sealed_sample_prepare_with_body("g.example.app", b"hi", &receiver);
            let (large, _) =
                sealed_sample_prepare_with_body("g.example.app", &vec![b'x'; 3000], &receiver);

            let schedule = Price::scheduled(100, 10);
            let small_charge = schedule.charge(small.data.len());
            let large_charge = schedule.charge(large.data.len());
            assert!(
                large_charge > small_charge,
                "the two packets must actually differ in price, or this test proves nothing: \
                 {small_charge} vs {large_charge}"
            );

            let app = router_with_gate(connector, signer, None, test_gate(test_channels()));

            // 1. The small packet, paid for exactly.
            let response = app
                .clone()
                .oneshot(request_with_claim_header(
                    &small,
                    CLAIM_HEADER,
                    &evm_claim_json(1, small_charge),
                ))
                .await
                .unwrap();
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            assert!(
                Fulfill::decode(&bytes).is_ok(),
                "a claim advancing exactly this packet's charge must buy it"
            );
            assert_eq!(app_client.deliveries().len(), 1);

            // 2. The large packet, paid for at the route's BASE -- which is
            //    what a sender that read only `extra.price` and ignored the
            //    slope would send, and what every pre-schedule sender sends.
            //    Refused, and the reject states the figure that would have
            //    been enough (issue #548), so the sender does not have to
            //    guess or parse English to find it.
            let response = app
                .clone()
                .oneshot(request_with_claim_header(
                    &large,
                    CLAIM_HEADER,
                    &evm_claim_json(2, small_charge + 100),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            // `accumulated_cost` rides BESIDE the packet, never in its OER
            // encoding (ADR 0018, ADR 0063) -- on this carriage that is the
            // `Toon-Accumulated-Cost` response header.
            let stated_cost = response
                .headers()
                .get(ACCUMULATED_COST_HEADER)
                .expect("a priced refusal states what the packet would have cost")
                .to_str()
                .unwrap()
                .to_string();
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code.as_str(), "F03");
            assert_eq!(
                stated_cost,
                large_charge.to_string(),
                "the refusal must report what THIS packet costs, not the route's base"
            );
            assert_ne!(stated_cost, "100", "the base is not what this packet costs");
            assert_eq!(
                app_client.deliveries().len(),
                1,
                "the underpaid packet must never reach the app"
            );

            // 3. The same large packet, paid for at its own charge.
            let response = app
                .oneshot(request_with_claim_header(
                    &large,
                    CLAIM_HEADER,
                    &evm_claim_json(3, small_charge + large_charge),
                ))
                .await
                .unwrap();
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            assert!(
                Fulfill::decode(&bytes).is_ok(),
                "a claim advancing this packet's own charge must buy it"
            );

            let deliveries = app_client.deliveries();
            assert_eq!(deliveries.len(), 2);
            // ADR 0040 narrowed by ADR 0065: the app is told what was
            // actually charged for the delivery in front of it, which for a
            // schedule route is not the route's base.
            let stated = deliveries[1]
                .request
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("X-TOON-Amount"))
                .map(|(_, value)| value.clone())
                .expect("a paid delivery states its amount");
            assert_eq!(stated, large_charge.to_string());
            assert_ne!(stated, "100", "the base is not what this delivery cost");
        }

        /// A claim's value is checked before this ingress would ever spend
        /// cryptographic work verifying its signature -- proven here by a
        /// claim whose signature is garbage, yet still refused for
        /// underpayment (F03) rather than as an unverifiable signature
        /// (which would also be F01, indistinguishable from this by code
        /// alone), since the value check runs unconditionally before
        /// verification is ever attempted (issue #522, #506/#544).
        #[tokio::test]
        async fn the_value_check_runs_before_any_cryptographic_work() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            let connector = Arc::new(Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ));
            let app = router_with_gate(connector, test_signer(), None, test_gate(test_channels()));

            let garbage_signature_claim =
                evm_claim_json_with_signature(1, 50, "0xnotarealsignatureatall");
            let request = request_with_claim_header(
                &sample_prepare("g.example.app"),
                CLAIM_HEADER,
                &garbage_signature_claim,
            );
            let response = app.oneshot(request).await.unwrap();
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code.as_str(), "F03");
            assert!(app_client.deliveries().is_empty());
        }

        /// A claim whose value binding passes but whose signature does not
        /// verify is refused before the packet reaches the app -- the
        /// gate's actual last stage (issue #506/#544).
        #[tokio::test]
        async fn a_claim_failing_signature_verification_never_reaches_the_app() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            let connector = Arc::new(Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ));
            let app = router_with_gate(connector, test_signer(), None, test_gate(test_channels()));

            let unverifiable_claim = evm_claim_json_with_signature(1, 100, "0xabcd");
            let request = request_with_claim_header(
                &sample_prepare("g.example.app"),
                CLAIM_HEADER,
                &unverifiable_claim,
            );
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code.as_str(), "F01");
            assert!(reject.message.contains("signature"));
            assert!(app_client.deliveries().is_empty());
        }

        /// The forger of issue #558, at this crate's real HTTP seam: a
        /// claim signed perfectly well with a key of the sender's own, over
        /// a channel this connector *does* have a record of, declaring that
        /// key as its signer. It never reaches the app, because the key is
        /// not the channel's counterparty.
        #[tokio::test]
        async fn a_claim_signed_by_a_key_that_is_not_the_channels_counterparty_never_reaches_the_app(
        ) {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let connector = Arc::new(Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ));
            let app = router_with_gate(connector, test_signer(), None, test_gate(test_channels()));

            let request = request_with_claim_header(
                &sample_prepare("g.example.app"),
                CLAIM_HEADER,
                &forged_evm_claim_json(1, 100),
            );
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code.as_str(), "F01");
            assert!(
                reject.message.contains("counterparty"),
                "the refusal names why it was refused: {}",
                reject.message
            );
            assert!(
                app_client.deliveries().is_empty(),
                "a forger must never buy the app's work"
            );
        }

        /// A claim naming a channel this connector has no record of is
        /// refused with its own reason, distinguishable from a bad
        /// signature -- and, since the registry is what says which channels
        /// exist, an edge mounted with no channels at all accepts nothing
        /// (issue #558's AC2 and AC8).
        #[tokio::test]
        async fn a_claim_on_an_unrecorded_channel_never_reaches_the_app() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let connector = Arc::new(Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ));
            // `router`, not `router_with_gate`: no channel recorded.
            let app = router(connector, test_signer());

            let request = request_with_claim_header(
                &sample_prepare("g.example.app"),
                CLAIM_HEADER,
                &evm_claim_json(1, 100),
            );
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code.as_str(), "F01");
            assert!(
                reject.message.contains("no record of"),
                "an unknown channel is refused for being unknown, not for a bad signature: {}",
                reject.message
            );
            assert!(app_client.deliveries().is_empty());
        }

        /// Issues #556/#502, at the real HTTP seam: a buyer this operator
        /// has never heard of -- no `[[client_channels]]` entry, nothing
        /// declared for their channel at all -- pays and the write lands,
        /// because the connector resolves the channel's counterparty from
        /// the chain the channel was opened on.
        ///
        /// This is the test that fails on `origin/main`: there, the only
        /// possible record is a declared one, so this exact request is
        /// refused F01 "no record of" and the app is never asked to work.
        #[tokio::test]
        async fn a_claim_on_a_channel_only_the_chain_knows_about_reaches_the_app() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );

            // Nothing declared. The source stands in for
            // `TokenNetwork.channels(id)`, which is what names the buyer
            // as this channel's counterparty.
            let (_secret, counterparty) = evm_signer();
            let mut channel_id = [0u8; 32];
            channel_id.copy_from_slice(&hex::decode("ab".repeat(32)).unwrap());
            let channels = ClientChannelRegistry::new().with_source(Arc::new(
                crate::channels::test_source::FakeChannelSource::knowing(vec![(
                    channel_id,
                    EvmChannel {
                        counterparty,
                        chain_id: EVM_CHAIN_ID,
                        token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                        deposit_floor: crate::DepositFloor::AtLeast(1_000_000),
                    },
                )]),
            ));
            assert!(
                !channels.is_empty(),
                "a registry with a source can vouch for channels nobody wrote down"
            );

            let (prepare, _shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
            let app = router_with_gate(connector, signer, None, test_gate(channels));
            let request =
                request_with_claim_header(&prepare, CLAIM_HEADER, &evm_claim_json(1, 100));
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);

            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            Fulfill::decode(&bytes).expect(
                "an unaffiliated buyer's on-chain channel is payable without a config edit",
            );
            assert_eq!(app_client.deliveries().len(), 1);
        }

        /// A lookup this connector could not complete refuses the claim --
        /// it never degrades to believing what the claim says about its
        /// own signer -- and says which failure it was, so an operator can
        /// tell a broken RPC endpoint from a sender guessing channel ids.
        #[tokio::test]
        async fn a_claim_whose_channel_lookup_fails_never_reaches_the_app() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let connector = Arc::new(Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ));
            let channels = ClientChannelRegistry::new().with_source(Arc::new(
                crate::channels::test_source::FakeChannelSource::unreachable("connection refused"),
            ));
            let app = router_with_gate(connector, test_signer(), None, test_gate(channels));

            let request = request_with_claim_header(
                &sample_prepare("g.example.app"),
                CLAIM_HEADER,
                &evm_claim_json(1, 100),
            );
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("decode reject");
            // T00, not F01 (issue #613's review): a failed lookup is this
            // connector's problem and not the claim's, so it must come back
            // as a *temporary* error. Told F01 a sender concludes its
            // perfectly good claim is invalid and stops, because a third
            // party's RPC endpoint blipped.
            assert_eq!(reject.code.as_str(), "T00");
            assert!(
                reject.message.contains("could not look up"),
                "an unreachable chain is reported as such, not as an unknown channel: {}",
                reject.message
            );
            assert!(app_client.deliveries().is_empty());
        }

        /// Issue #630's demoable slice, proven at this crate's own real
        /// HTTP seam -- the same one
        /// `a_fresh_plaintext_claim_lets_the_packet_reach_the_app` proves
        /// for EVM, above: a Solana channel declared only in
        /// [`ClientChannelRegistry`] (the `[[client_channels]]`-equivalent
        /// -- no chain resolution, no settlement backend, matching
        /// `ClientChannelRegistry::solana`'s own "declared records only"
        /// doc), a genuinely Ed25519-signed claim over it, verified,
        /// journaled and forwarded to the app.
        #[tokio::test]
        async fn a_declared_solana_client_channel_claim_reaches_the_app() {
            use base64::engine::general_purpose::STANDARD as BASE64;
            use base64::Engine;
            use ed25519_dalek::{Keypair as SolanaKeypair, Signer as Ed25519Signer};
            use rand::rngs::OsRng;

            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );

            // The channel's counterparty must be a real Ed25519 identity
            // able to sign a genuine balance proof (issue #558) -- generated
            // here rather than a placeholder, since the claim below signs
            // with it for real.
            let counterparty_keypair = SolanaKeypair::generate(&mut OsRng);
            let channel_account = [0x42u8; 32];
            let channel_account_base58 = bs58::encode(channel_account).into_string();
            let counterparty_base58 =
                bs58::encode(counterparty_keypair.public.to_bytes()).into_string();

            // Declared, not resolved from chain: `[[client_channels]]`'s
            // own registration path (issue #630). The chain-resolved twin
            // of this test, for a channel nothing declared, is issue
            // #631's `a_solana_claim_on_a_channel_only_the_chain_knows_about_reaches_the_app`
            // below.
            let mut channels = ClientChannelRegistry::new();
            channels
                .record_solana(
                    &channel_account_base58,
                    &counterparty_base58,
                    SOLANA_CHANNEL_PROGRAM_BASE58,
                )
                .expect("valid base58 32-byte accounts");

            let nonce = 1u64;
            let transferred_amount = 100u64;
            let message = connector_signer::solana_balance_proof_message(
                &[7u8; 32],
                &channel_account,
                nonce,
                transferred_amount,
            );
            let signature = counterparty_keypair.sign(&message);
            let signature_base64 = BASE64.encode(signature.to_bytes());

            let claim_json = format!(
                r#"{{
                    "version": "1.0",
                    "blockchain": "solana",
                    "messageId": "msg-1",
                    "timestamp": "2026-02-02T12:00:00.000Z",
                    "senderId": "peer-bob",
                    "programId": "{SOLANA_CHANNEL_PROGRAM_BASE58}",
                    "channelAccount": "{channel_account_base58}",
                    "nonce": {nonce},
                    "transferredAmount": "{transferred_amount}",
                    "signature": "{signature_base64}",
                    "signerPublicKey": "{counterparty_base58}"
                }}"#,
            );

            let (prepare, _shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
            let app = router_with_gate(connector, signer, None, test_gate(channels));

            let request = request_with_claim_header(&prepare, CLAIM_HEADER, &claim_json);
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);

            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            Fulfill::decode(&bytes).expect("decode fulfill");
            assert_eq!(app_client.deliveries().len(), 1);
        }

        /// The forger of issue #558, Solana-flavored: a claim genuinely
        /// signed, but by a key that is not the declared channel's
        /// counterparty, and declaring itself the payer anyway. Must be
        /// refused exactly as the equivalent EVM forgery is -- the claim's
        /// own `signerPublicKey` is never trusted, only the registry's
        /// declared counterparty is checked against.
        #[tokio::test]
        async fn a_solana_claim_forged_by_a_non_counterparty_key_is_refused() {
            use base64::engine::general_purpose::STANDARD as BASE64;
            use base64::Engine;
            use ed25519_dalek::{Keypair as SolanaKeypair, Signer as Ed25519Signer};
            use rand::rngs::OsRng;

            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );

            let real_counterparty = SolanaKeypair::generate(&mut OsRng);
            let forger = SolanaKeypair::generate(&mut OsRng);
            let channel_account = [0x43u8; 32];
            let channel_account_base58 = bs58::encode(channel_account).into_string();
            let real_counterparty_base58 =
                bs58::encode(real_counterparty.public.to_bytes()).into_string();
            let forger_base58 = bs58::encode(forger.public.to_bytes()).into_string();

            let mut channels = ClientChannelRegistry::new();
            channels
                .record_solana(
                    &channel_account_base58,
                    &real_counterparty_base58,
                    SOLANA_CHANNEL_PROGRAM_BASE58,
                )
                .expect("valid base58 32-byte accounts");

            let nonce = 1u64;
            let transferred_amount = 100u64;
            let message = connector_signer::solana_balance_proof_message(
                &[7u8; 32],
                &channel_account,
                nonce,
                transferred_amount,
            );
            // Signed genuinely -- just by the wrong key.
            let signature = forger.sign(&message);
            let signature_base64 = BASE64.encode(signature.to_bytes());

            let claim_json = format!(
                r#"{{
                    "version": "1.0",
                    "blockchain": "solana",
                    "messageId": "msg-1",
                    "timestamp": "2026-02-02T12:00:00.000Z",
                    "senderId": "peer-bob",
                    "programId": "{SOLANA_CHANNEL_PROGRAM_BASE58}",
                    "channelAccount": "{channel_account_base58}",
                    "nonce": {nonce},
                    "transferredAmount": "{transferred_amount}",
                    "signature": "{signature_base64}",
                    "signerPublicKey": "{forger_base58}"
                }}"#,
            );

            let (prepare, _shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
            let app = router_with_gate(connector, signer, None, test_gate(channels));

            let request = request_with_claim_header(&prepare, CLAIM_HEADER, &claim_json);
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);

            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code.as_str(), "F01");
            assert!(app_client.deliveries().is_empty());
        }

        /// Issue #631, the Solana twin of
        /// `a_claim_on_a_channel_only_the_chain_knows_about_reaches_the_app`
        /// above: a buyer this operator has never heard of -- no
        /// `[[client_channels]]` entry, nothing declared for their channel
        /// at all -- pays and the write lands, because the connector
        /// resolves the channel's counterparty from the deployed Solana
        /// payment-channel program the channel was opened on.
        #[tokio::test]
        async fn a_solana_claim_on_a_channel_only_the_chain_knows_about_reaches_the_app() {
            use base64::engine::general_purpose::STANDARD as BASE64;
            use base64::Engine;
            use ed25519_dalek::{Keypair as SolanaKeypair, Signer as Ed25519Signer};
            use rand::rngs::OsRng;

            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );

            // Nothing declared. The source stands in for
            // `SolanaSettlementBackend::channel_counterparty`, which is
            // what names the buyer as this channel's counterparty.
            let counterparty_keypair = SolanaKeypair::generate(&mut OsRng);
            let channel_account = [0x44u8; 32];
            let channel_account_base58 = bs58::encode(channel_account).into_string();
            let counterparty_base58 =
                bs58::encode(counterparty_keypair.public.to_bytes()).into_string();

            let channels = ClientChannelRegistry::new().with_solana_source(Arc::new(
                crate::channels::test_source::FakeSolanaChannelSource::knowing(vec![(
                    channel_account,
                    crate::SolanaChannel {
                        program_id: [7u8; 32],
                        counterparty: counterparty_keypair.public.to_bytes(),
                        deposit_floor: crate::DepositFloor::AtLeast(1_000_000),
                    },
                )]),
            ));
            assert!(
                !channels.is_empty(),
                "a registry with a source can vouch for channels nobody wrote down"
            );

            let nonce = 1u64;
            let transferred_amount = 100u64;
            let message = connector_signer::solana_balance_proof_message(
                &[7u8; 32],
                &channel_account,
                nonce,
                transferred_amount,
            );
            let signature = counterparty_keypair.sign(&message);
            let signature_base64 = BASE64.encode(signature.to_bytes());

            let claim_json = format!(
                r#"{{
                    "version": "1.0",
                    "blockchain": "solana",
                    "messageId": "msg-1",
                    "timestamp": "2026-02-02T12:00:00.000Z",
                    "senderId": "peer-bob",
                    "programId": "{SOLANA_CHANNEL_PROGRAM_BASE58}",
                    "channelAccount": "{channel_account_base58}",
                    "nonce": {nonce},
                    "transferredAmount": "{transferred_amount}",
                    "signature": "{signature_base64}",
                    "signerPublicKey": "{counterparty_base58}"
                }}"#,
            );

            let (prepare, _shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
            let app = router_with_gate(connector, signer, None, test_gate(channels));

            let request = request_with_claim_header(&prepare, CLAIM_HEADER, &claim_json);
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);

            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            Fulfill::decode(&bytes).expect(
                "an unaffiliated Solana buyer's on-chain channel is payable without a config edit",
            );
            assert_eq!(app_client.deliveries().len(), 1);
        }

        /// The Solana twin of `a_claim_whose_channel_lookup_fails_never_reaches_the_app`:
        /// a lookup this connector could not complete refuses the claim
        /// rather than believing what it says about its own signer.
        #[tokio::test]
        async fn a_solana_claim_whose_channel_lookup_fails_never_reaches_the_app() {
            use base64::engine::general_purpose::STANDARD as BASE64;
            use base64::Engine;
            use ed25519_dalek::{Keypair as SolanaKeypair, Signer as Ed25519Signer};
            use rand::rngs::OsRng;

            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );
            let channels = ClientChannelRegistry::new().with_solana_source(Arc::new(
                crate::channels::test_source::FakeSolanaChannelSource::unreachable(
                    "connection refused",
                ),
            ));
            let app = router_with_gate(connector, signer.clone(), None, test_gate(channels));

            let keypair = SolanaKeypair::generate(&mut OsRng);
            let channel_account = [0x45u8; 32];
            let channel_account_base58 = bs58::encode(channel_account).into_string();
            let signer_base58 = bs58::encode(keypair.public.to_bytes()).into_string();
            let message = connector_signer::solana_balance_proof_message(
                &[7u8; 32],
                &channel_account,
                1,
                100,
            );
            let signature_base64 = BASE64.encode(keypair.sign(&message).to_bytes());

            let claim_json = format!(
                r#"{{
                    "version": "1.0",
                    "blockchain": "solana",
                    "messageId": "msg-1",
                    "timestamp": "2026-02-02T12:00:00.000Z",
                    "senderId": "peer-bob",
                    "programId": "{SOLANA_CHANNEL_PROGRAM_BASE58}",
                    "channelAccount": "{channel_account_base58}",
                    "nonce": 1,
                    "transferredAmount": "100",
                    "signature": "{signature_base64}",
                    "signerPublicKey": "{signer_base58}"
                }}"#,
            );

            let (prepare, _shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
            let request = request_with_claim_header(&prepare, CLAIM_HEADER, &claim_json);
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("decode reject");
            // T00, not F01 (issue #613's review): a failed lookup is this
            // connector's problem and not the claim's, so it must come back
            // as a *temporary* error. Told F01 a sender concludes its
            // perfectly good claim is invalid and stops, because a third
            // party's RPC endpoint blipped.
            assert_eq!(reject.code.as_str(), "T00");
            assert!(
                reject.message.contains("could not look up"),
                "an unreachable chain is reported as such, not as an unknown channel: {}",
                reject.message
            );
            assert!(app_client.deliveries().is_empty());
        }
    }

    /// Cost discovery at the client edge (issue #548,
    /// `client-edge-spec.md` §1.6, ADR 0011): the running cost total a
    /// REJECT reports, and `POST /ilp/probe`, the ingress a sender uses to
    /// raise one deliberately. `tests/accumulated_cost_header.rs` covers
    /// the header on a path with no claim in it at all; these cover the
    /// cases that need a real claim, and so need `claim_headers`'s signer.
    mod cost_discovery {
        use super::claim_headers::{evm_claim_json, request_with_claim_header, test_channels};
        use super::*;

        const PRICE: u64 = 100;

        fn cost_header(response: &Response) -> Option<u64> {
            response
                .headers()
                .get(ACCUMULATED_COST_HEADER)
                .map(|value| value.to_str().unwrap().parse::<u64>().unwrap())
        }

        fn probe_request(prepare: &Prepare, claim_json: Option<&str>) -> Request<Body> {
            let builder = Request::builder().method("POST").uri("/ilp/probe");
            let builder = match claim_json {
                Some(claim_json) => {
                    builder.header(CLAIM_HEADER, BASE64.encode(claim_json.as_bytes()))
                }
                None => builder,
            };
            builder
                .body(Body::from(prepare.encode()))
                .expect("well-formed probe request")
        }

        /// A router over one priced, terminating route whose app answers,
        /// plus the app client so a test can assert whether the app was
        /// ever asked to do anything.
        fn priced_route_router() -> (Router, Arc<FakeAppClient>, Arc<dyn Signer>) {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", PRICE).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );
            // Since #558 a claim verifies against the counterparty this
            // connector records for the channel it names, so a router with
            // no channels recorded refuses every claim -- including the
            // paid write these tests set themselves up with. The recorded
            // channel is `claim_headers`' own, the one `evm_claim_json`
            // signs against.
            (
                router_with_gate(connector, signer.clone(), None, test_gate(test_channels())),
                app_client,
                signer,
            )
        }

        /// Before #548 a route's price was disclosed only inside an
        /// underpayment reject's human-readable `message` -- so a client
        /// learned a price by underpaying first, which is exactly what cost
        /// discovery exists to prevent. The figure now rides the header a
        /// client already reads for every other reject.
        #[tokio::test]
        async fn an_underpaying_claim_reports_the_routes_price_in_the_header() {
            let (app, app_client, _signer) = priced_route_router();

            let request = request_with_claim_header(
                &sample_prepare("g.example.app"),
                CLAIM_HEADER,
                &evm_claim_json(1, PRICE - 1),
            );
            let response = app.oneshot(request).await.unwrap();

            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(cost_header(&response), Some(PRICE));
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            assert_eq!(Reject::decode(&bytes).unwrap().code.as_str(), "F03");
            assert!(app_client.deliveries().is_empty());
        }

        /// Every other claim refusal is decided before any route price is
        /// in play: nothing was traversed and nothing terminated, so the
        /// figure is `0` -- present, and honestly zero.
        #[tokio::test]
        async fn a_malformed_claim_reports_zero_rather_than_the_routes_price() {
            let (app, _app_client, _signer) = priced_route_router();

            let request =
                request_with_claim_header(&sample_prepare("g.example.app"), CLAIM_HEADER, "{}");
            let response = app.oneshot(request).await.unwrap();

            assert_eq!(cost_header(&response), Some(0));
        }

        /// ADR 0011: probing traverses the network for free, so it is
        /// accepted only from a sender holding a channel this connector
        /// recognizes. A probe presenting nothing is not authorized, and
        /// says so with a status distinct from an ILP-level outcome.
        #[tokio::test]
        async fn a_probe_presenting_no_claim_is_forbidden() {
            let (app, _app_client, _signer) = priced_route_router();

            let response = app
                .oneshot(probe_request(&sample_prepare("g.example.app"), None))
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }

        /// A claim that verifies proves the sender holds the channel, but
        /// this connector has never seen that channel before -- ADR 0011's
        /// first condition is not met and the packet is never forwarded.
        #[tokio::test]
        async fn a_probe_on_a_channel_this_connector_has_never_seen_is_forbidden() {
            let (app, app_client, _signer) = priced_route_router();

            let response = app
                .oneshot(probe_request(
                    &sample_prepare("g.example.app"),
                    Some(&evm_claim_json(1, PRICE)),
                ))
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert!(app_client.deliveries().is_empty());
        }

        /// The gate is satisfiable by a deployed node (issue #548's last
        /// acceptance criterion): a claim clearing §1.3's gate at this edge
        /// is how a connector learns a sender is actually *using* a channel
        /// with it. Since issue #558 that node does hold prior
        /// configuration about the channel -- it must already record the
        /// counterparty whose signature it accepts there, or the claim
        /// could not verify at all -- but recording whose signature is
        /// accepted is not the same as having been paid, and it is the
        /// payment that this gate records. No chain indexes that either.
        /// Having paid once, the same sender may probe -- and what comes
        /// back is the route's price as one figure, with the app still only
        /// ever having been asked to do the work that was paid for.
        #[tokio::test]
        async fn a_probe_from_a_sender_that_has_paid_reports_the_price_without_delivering() {
            let (app, app_client, signer) = priced_route_router();
            let (paid, _shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());

            let paid_response = app
                .clone()
                .oneshot(request_with_claim_header(
                    &paid,
                    CLAIM_HEADER,
                    &evm_claim_json(1, PRICE),
                ))
                .await
                .unwrap();
            assert_eq!(paid_response.status(), StatusCode::OK);
            assert_eq!(app_client.deliveries().len(), 1);

            // A fresh nonce at the same cumulative amount: the claim
            // identifies, and advances no value at all.
            let probe_response = app
                .oneshot(probe_request(
                    &sample_prepare("g.example.app"),
                    Some(&evm_claim_json(2, PRICE)),
                ))
                .await
                .unwrap();

            assert_eq!(probe_response.status(), StatusCode::OK);
            assert_eq!(cost_header(&probe_response), Some(PRICE));
            let bytes = hyper::body::to_bytes(probe_response.into_body())
                .await
                .unwrap();
            Reject::decode(&bytes).expect("a probe is answered with a reject");
            // Still one: free traversal is all a probe buys.
            assert_eq!(app_client.deliveries().len(), 1);
        }

        /// Issue #548's fifth acceptance criterion, end to end: a sender
        /// that funds a packet from the figure a probe returned is not then
        /// rejected for underpaying it. The figure a probe reports and the
        /// figure the claim gate charges are the same route price read from
        /// the same lookup, so this holds by construction -- and this test
        /// is what keeps it holding.
        #[tokio::test]
        async fn funding_a_packet_from_the_figure_a_probe_returned_is_not_underpayment() {
            let (app, app_client, signer) = priced_route_router();
            let (paid, _shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());

            // Pay once, so this connector recognizes the channel.
            app.clone()
                .oneshot(request_with_claim_header(
                    &paid,
                    CLAIM_HEADER,
                    &evm_claim_json(1, PRICE),
                ))
                .await
                .unwrap();

            let probe_response = app
                .clone()
                .oneshot(probe_request(
                    &sample_prepare("g.example.app"),
                    Some(&evm_claim_json(2, PRICE)),
                ))
                .await
                .unwrap();
            let quoted = cost_header(&probe_response).expect("a probe reports a figure");

            // Fund exactly what the probe quoted, and nothing more.
            let (second, _shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
            let response = app
                .oneshot(request_with_claim_header(
                    &second,
                    CLAIM_HEADER,
                    &evm_claim_json(3, PRICE + quoted),
                ))
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            Fulfill::decode(&bytes).expect("a packet funded from the probe's figure fulfils");
            assert_eq!(app_client.deliveries().len(), 2);
        }
    }
    /// Issue #620 / ADR 0028: a route that *forwards* over a peering is
    /// greeted, gated and journaled at this client edge on exactly the path
    /// a terminated one uses.
    ///
    /// Hermetic on purpose. `connector-bin`'s `two_connectors_peer.rs`
    /// proves the same properties against two real spawned binaries, real
    /// sockets and a real chain -- and skips itself entirely when `anvil`
    /// is unavailable. These cases need no chain, so the free-gateway guard
    /// they carry cannot be silently skipped in an environment that lacks
    /// one, which is the whole point of writing them here as well.
    mod forwarded_routes {
        use super::claim_headers::{evm_claim_json, request_with_claim_header, test_channels};
        use super::*;

        /// What the forwarded route charges this connector's own clients,
        /// and what it retains of that -- deliberately different numbers,
        /// so a lookup that reached for the fee could not coincide with the
        /// right answer.
        const FORWARD_PRICE: u64 = 100;
        const FORWARD_FEE: u64 = 3;

        const PEER_ID: &str = "beta";
        const FORWARD_PREFIX: &str = "g.example.beta";
        const REMOTE_APP: &str = "g.example.beta.app";

        /// A payer whose only route to [`REMOTE_APP`] is a priced peering,
        /// and the real downstream `Connector` on the other end of it --
        /// returned so a test can assert on what did or did not reach the
        /// app *behind* the peering, which is what "carried for free" means
        /// concretely.
        fn payer_over_a_priced_peering(
            signer: Arc<dyn connector_signer::Signer>,
        ) -> (Arc<Connector>, Arc<FakeAppClient>) {
            let remote_route = StaticRoute::new(REMOTE_APP, "http://localhost:4000").unwrap();
            let remote_app = Arc::new(FakeAppClient::new());
            remote_app.respond(remote_route.handler_url(), answered(b"across the peering"));
            let payee = Arc::new(
                Connector::new(
                    vec![remote_route],
                    vec![],
                    remote_app.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );

            let mut transport = InProcessPeerTransport::new();
            transport.add_peer(PEER_ID, payee);
            let payer = Arc::new(covering(
                Connector::new(
                    vec![],
                    vec![PeerRoute::new_priced(
                        FORWARD_PREFIX,
                        PEER_ID,
                        FORWARD_PRICE,
                    )],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(transport),
                    test_clock(),
                )
                .with_peer_fees([(PEER_ID.to_string(), FORWARD_FEE)])
                .with_identity_signer(signer),
                PEER_ID,
            ));
            (payer, remote_app)
        }

        /// The free-gateway guard, on the forwarded branch. Before ADR 0028
        /// this exact request was carried across the peering and answered
        /// by the app behind it, for nothing -- while the payer signed a
        /// peer claim for the value it carried.
        #[tokio::test]
        async fn an_unpaid_request_to_a_priced_forwarded_route_is_greeted_not_carried() {
            let signer = test_signer();
            let (payer, remote_app) = payer_over_a_priced_peering(signer.clone());
            let app = router(payer, signer);

            let request = Request::builder()
                .method("POST")
                .uri("/ilp")
                .body(Body::from(sample_prepare(REMOTE_APP).encode()))
                .unwrap();
            let response = app.oneshot(request).await.unwrap();

            assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let terms: X402PaymentRequired = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(terms.resource.url, REMOTE_APP);
            assert_eq!(
                terms.accepts[0].amount,
                FORWARD_PRICE.to_string(),
                "the greeting quotes the forwarded route's `price`, never its `fee`"
            );
            assert!(
                remote_app.deliveries().is_empty(),
                "an unpaid request must not cross the peering at all"
            );
        }

        /// The paying half: a real claim covering the price gets the packet
        /// across the peering and fulfilled, so the greeting above is a
        /// gate rather than a wall.
        #[tokio::test]
        async fn a_claim_covering_the_price_carries_the_packet_across_the_peering() {
            let signer = test_signer();
            let (payer, remote_app) = payer_over_a_priced_peering(signer.clone());
            let app = router_with_gate(payer, signer.clone(), None, test_gate(test_channels()));

            // `amount == price` is the arithmetic ADR 0028 intends: the hop
            // collects `FORWARD_PRICE` and forwards `FORWARD_PRICE -
            // FORWARD_FEE`, earning exactly its fee.
            let (prepare, shared_secret) = sealed_sample_prepare_with_amount(
                REMOTE_APP,
                FORWARD_PRICE,
                &signer.public_key().unwrap(),
            );
            let response = app
                .oneshot(request_with_claim_header(
                    &prepare,
                    CLAIM_HEADER,
                    &evm_claim_json(1, FORWARD_PRICE),
                ))
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let fulfill = Fulfill::decode(&bytes).expect("a paid forwarded packet fulfils");
            assert_eq!(
                open_sealed_envelope(&shared_secret, &fulfill.data),
                fulfill_envelope(b"across the peering")
            );
            assert_eq!(remote_app.deliveries().len(), 1);
        }

        /// A claim that does not cover the forwarded route's price is
        /// refused for exactly the reason a terminated route's would be
        /// (§1.3) -- underpayment is not a property of terminating.
        #[tokio::test]
        async fn a_claim_below_the_price_never_crosses_the_peering() {
            let signer = test_signer();
            let (payer, remote_app) = payer_over_a_priced_peering(signer.clone());
            let app = router_with_gate(payer, signer.clone(), None, test_gate(test_channels()));

            let (prepare, _shared_secret) = sealed_sample_prepare_with_amount(
                REMOTE_APP,
                FORWARD_PRICE,
                &signer.public_key().unwrap(),
            );
            let response = app
                .oneshot(request_with_claim_header(
                    &prepare,
                    CLAIM_HEADER,
                    &evm_claim_json(1, FORWARD_PRICE - 1),
                ))
                .await
                .unwrap();

            let cost = response
                .headers()
                .get(ACCUMULATED_COST_HEADER)
                .expect("an underpayment reports the price it fell short of")
                .to_str()
                .unwrap()
                .to_string();
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("an underpaid packet is rejected");
            assert_eq!(reject.code.as_str(), "F03");
            assert_eq!(cost, FORWARD_PRICE.to_string());
            assert!(
                remote_app.deliveries().is_empty(),
                "an underpaid request must not cross the peering either"
            );
        }

        /// ADR 0028's amount bound: paying `FORWARD_PRICE` does not buy the
        /// carriage of an arbitrarily larger amount. The packet is refused
        /// before the claim is ingested, so nothing downstream sees it and
        /// nothing is spent.
        #[tokio::test]
        async fn a_forwarded_route_never_carries_more_value_than_its_price() {
            let signer = test_signer();
            let (payer, remote_app) = payer_over_a_priced_peering(signer.clone());
            let app = router_with_gate(payer, signer.clone(), None, test_gate(test_channels()));

            let (prepare, _shared_secret) = sealed_sample_prepare_with_amount(
                REMOTE_APP,
                FORWARD_PRICE + 1,
                &signer.public_key().unwrap(),
            );
            let response = app
                .oneshot(request_with_claim_header(
                    &prepare,
                    CLAIM_HEADER,
                    &evm_claim_json(1, FORWARD_PRICE),
                ))
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = Reject::decode(&bytes).expect("an over-carried packet is rejected");
            assert_eq!(reject.code.as_str(), "F03");
            assert!(
                remote_app.deliveries().is_empty(),
                "the packet this connector refused to carry must not have been carried"
            );
        }

        /// The counterweight: an unpriced forwarded route (`price = 0`, an
        /// operator's deliberate free carriage) keeps the behavior it had
        /// before ADR 0028 -- no greeting, no claim required, and no bound
        /// on the amount it carries beyond the fee arithmetic itself.
        #[tokio::test]
        async fn an_unpriced_forwarded_route_still_carries_for_free() {
            let signer = test_signer();
            let remote_route = StaticRoute::new(REMOTE_APP, "http://localhost:4000").unwrap();
            let remote_app = Arc::new(FakeAppClient::new());
            remote_app.respond(remote_route.handler_url(), answered(b"free carriage"));
            let payee = Arc::new(
                Connector::new(
                    vec![remote_route],
                    vec![],
                    remote_app.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );
            let mut transport = InProcessPeerTransport::new();
            transport.add_peer(PEER_ID, payee);
            let payer = Arc::new(covering(
                Connector::new(
                    vec![],
                    vec![PeerRoute::new_priced(FORWARD_PREFIX, PEER_ID, 0)],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(transport),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
                PEER_ID,
            ));
            let app = router(payer, signer.clone());

            let (prepare, _shared_secret) =
                sealed_sample_prepare(REMOTE_APP, &signer.public_key().unwrap());
            let request = Request::builder()
                .method("POST")
                .uri("/ilp")
                .body(Body::from(prepare.encode()))
                .unwrap();
            let response = app.oneshot(request).await.unwrap();

            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            Fulfill::decode(&bytes).expect("a deliberately free forwarded route still carries");
            assert_eq!(remote_app.deliveries().len(), 1);
        }

        /// Issue #1012: a client-priced forwarded route (ADR 0028) admits
        /// the client's claim before learning whether the next hop will
        /// carry the packet at all. When it refuses -- here, the next hop
        /// has no route for the destination, the same shape
        /// `a_reject_with_no_route_at_the_second_hop_reaches_the_original_client`
        /// exercises unpriced, but this time over a route that actually
        /// admitted a claim -- that claim must not count against the
        /// client. Proven behaviourally rather than by reaching into the
        /// gate's internals: the exact same claim (identical nonce and
        /// cumulative amount) presented a second time is judged exactly as
        /// fresh as the first, which is only possible if this connector
        /// rolled its watermark back rather than leaving it charged. Left
        /// charged, `ClientClaimGate::admit` would refuse the resubmission
        /// as a stale nonce (F01) before the packet is ever routed a
        /// second time -- rolled back, it is admitted again, forwarded
        /// again, and meets the identical F02 the first attempt did.
        #[tokio::test]
        async fn a_forwarded_routes_next_hop_reject_does_not_charge_the_client() {
            let signer = test_signer();
            let payee = Arc::new(Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ));
            let mut transport = InProcessPeerTransport::new();
            transport.add_peer(PEER_ID, payee);
            let payer = Arc::new(covering(
                Connector::new(
                    vec![],
                    vec![PeerRoute::new_priced(
                        FORWARD_PREFIX,
                        PEER_ID,
                        FORWARD_PRICE,
                    )],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(transport),
                    test_clock(),
                )
                .with_peer_fees([(PEER_ID.to_string(), FORWARD_FEE)])
                .with_identity_signer(signer.clone()),
                PEER_ID,
            ));
            let app = router_with_gate(payer, signer.clone(), None, test_gate(test_channels()));

            let claim_json = evm_claim_json(1, FORWARD_PRICE);
            let attempt = |app: axum::Router| {
                let claim_json = claim_json.clone();
                let signer = signer.clone();
                async move {
                    let (prepare, _shared_secret) = sealed_sample_prepare_with_amount(
                        REMOTE_APP,
                        FORWARD_PRICE,
                        &signer.public_key().unwrap(),
                    );
                    let response = app
                        .oneshot(request_with_claim_header(
                            &prepare,
                            CLAIM_HEADER,
                            &claim_json,
                        ))
                        .await
                        .unwrap();
                    let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
                    Reject::decode(&bytes).expect("no route at the next hop is a reject")
                }
            };

            let first = attempt(app.clone()).await;
            assert_eq!(
                first.code.as_str(),
                "F02",
                "the next hop's own refusal, reached because the claim WAS admitted and forwarded"
            );

            let second = attempt(app).await;
            assert_eq!(
                second.code.as_str(),
                "F02",
                "the identical claim reached the peer again rather than being refused as a \
                 stale nonce -- the first reject did not charge the client"
            );
        }

        /// Issue #1012's converse, and the guard that the rollback above
        /// stays confined to the reject: a forward that FULFILLs still
        /// advances the watermark by exactly `price`, as it always has.
        /// Proven the same behavioural way, since the gate's watermarks
        /// are not reachable from here: after the fulfilment the identical
        /// claim is refused as a stale nonce (`F01`), and the next claim
        /// must clear a cumulative amount a full `FORWARD_PRICE` higher --
        /// one base unit short of that is an underpayment (`F03`), exactly
        /// on it fulfils again. Nothing but a watermark sitting at
        /// precisely `FORWARD_PRICE` produces all three answers.
        #[tokio::test]
        async fn a_forwarded_routes_fulfilment_still_advances_the_watermark_by_the_price() {
            let signer = test_signer();
            let (payer, remote_app) = payer_over_a_priced_peering(signer.clone());
            let app = router_with_gate(payer, signer.clone(), None, test_gate(test_channels()));

            let attempt = |app: axum::Router, claim_json: String| {
                let signer = signer.clone();
                async move {
                    let (prepare, _shared_secret) = sealed_sample_prepare_with_amount(
                        REMOTE_APP,
                        FORWARD_PRICE,
                        &signer.public_key().unwrap(),
                    );
                    let response = app
                        .oneshot(request_with_claim_header(
                            &prepare,
                            CLAIM_HEADER,
                            &claim_json,
                        ))
                        .await
                        .unwrap();
                    hyper::body::to_bytes(response.into_body()).await.unwrap()
                }
            };

            let first = attempt(app.clone(), evm_claim_json(1, FORWARD_PRICE)).await;
            Fulfill::decode(&first).expect("a paid forwarded packet fulfils");

            let replayed = attempt(app.clone(), evm_claim_json(1, FORWARD_PRICE)).await;
            assert_eq!(
                Reject::decode(&replayed)
                    .expect("a replayed claim is rejected")
                    .code
                    .as_str(),
                "F01",
                "the fulfilled packet's claim stayed charged -- the watermark did not roll back"
            );

            let short = attempt(app.clone(), evm_claim_json(2, 2 * FORWARD_PRICE - 1)).await;
            assert_eq!(
                Reject::decode(&short)
                    .expect("an underpaying claim is rejected")
                    .code
                    .as_str(),
                "F03",
                "the watermark sits at exactly FORWARD_PRICE, so advancing by one less than \
                 the price underpays"
            );

            let second = attempt(app, evm_claim_json(2, 2 * FORWARD_PRICE)).await;
            Fulfill::decode(&second).expect("a claim advancing by a full price fulfils again");
            assert_eq!(
                remote_app.deliveries().len(),
                2,
                "exactly the two paid packets crossed the peering"
            );
        }

        /// §1.7: `GET /ilp/routes/price` answers for a destination this
        /// connector forwards, because it charges for one. Answering `404`
        /// here -- what it did when the lookup read terminated routes only
        /// -- would tell a client a route it is about to be charged for is
        /// free.
        #[tokio::test]
        async fn the_price_endpoint_answers_for_a_forwarded_destination() {
            let signer = test_signer();
            let (payer, _remote_app) = payer_over_a_priced_peering(signer.clone());
            let app = router(payer, signer);

            let request = Request::builder()
                .uri(format!("/ilp/routes/price?destination={REMOTE_APP}"))
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let view: RoutePriceView = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                view,
                RoutePriceView {
                    destination: REMOTE_APP.to_string(),
                    price: FORWARD_PRICE,
                    price_per_kib: 0,
                }
            );

            let unmatched = Request::builder()
                .uri("/ilp/routes/price?destination=g.nowhere")
                .body(Body::empty())
                .unwrap();
            assert_eq!(
                app.oneshot(unmatched).await.unwrap().status(),
                StatusCode::NOT_FOUND,
                "a destination this connector serves no route for is still 404"
            );
        }
    }

    /// Client-edge sender identity (issue #502, client-edge-spec.md §1.2):
    /// a configured peer authenticating with a bearer secret, or an
    /// anonymous sender. Exercised at this crate's real HTTP seam, the same
    /// way `claim_headers` exercises claim ingest.
    mod identity_headers {
        use super::*;
        use libsecp256k1::{PublicKey, SecretKey};

        fn identity(id: &str, secret: &str) -> ConfiguredIdentity {
            ConfiguredIdentity {
                id: id.to_string(),
                secret: secret.to_string(),
            }
        }

        fn router_over(
            connector: Arc<Connector>,
            signer: Arc<dyn Signer>,
            identities: Vec<ConfiguredIdentity>,
        ) -> Router {
            router_with_identities(
                connector,
                signer,
                None,
                test_gate(ClientChannelRegistry::new()),
                Arc::from(identities),
            )
        }

        fn request_with_identity(
            prepare: &Prepare,
            peer_id: Option<&str>,
            authorization: Option<&str>,
        ) -> Request<Body> {
            let mut builder = Request::builder().method("POST").uri("/ilp");
            if let Some(peer_id) = peer_id {
                builder = builder.header(PEER_ID_HEADER, peer_id);
            }
            if let Some(authorization) = authorization {
                builder = builder.header(header::AUTHORIZATION, authorization);
            }
            builder.body(Body::from(prepare.encode())).unwrap()
        }

        #[tokio::test]
        async fn a_configured_peer_presenting_correct_credentials_still_reaches_the_app() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );
            let (prepare, _shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
            let app = router_over(connector, signer, vec![identity("peer-a", "s3cr3t")]);

            let request = request_with_identity(&prepare, Some("peer-a"), Some("Bearer s3cr3t"));
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(app_client.deliveries().len(), 1);
        }

        #[tokio::test]
        async fn an_empty_configured_secret_authenticates_with_no_authorization_header() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );
            let (prepare, _shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
            let app = router_over(connector, signer, vec![identity("peer-a", "")]);

            let request = request_with_identity(&prepare, Some("peer-a"), None);
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(app_client.deliveries().len(), 1);
        }

        #[tokio::test]
        async fn a_wrong_secret_is_401_before_a_priced_routes_terms_are_answered() {
            let route =
                StaticRoute::new_priced("g.example.app", "http://localhost:4000", 100).unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            let connector = Arc::new(Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ));
            let app = router_over(connector, test_signer(), vec![identity("peer-a", "s3cr3t")]);

            let request = request_with_identity(
                &sample_prepare("g.example.app"),
                Some("peer-a"),
                Some("Bearer wrong"),
            );
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert!(app_client.deliveries().is_empty());
        }

        #[tokio::test]
        async fn an_identity_naming_no_configured_peer_is_401_not_anonymous() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let connector = Arc::new(Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ));
            let app = router_over(connector, test_signer(), vec![]);

            let request =
                request_with_identity(&sample_prepare("g.example.app"), Some("peer-a"), None);
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert!(
                app_client.deliveries().is_empty(),
                "a request naming an unrecognised identity must not be treated as anonymous"
            );
        }

        /// AC: neither the x402 answer, claim ingest, nor envelope delivery
        /// changes behaviour for a request that presents no identity --
        /// configuring `[[client_identities]]` at all must not perturb an
        /// anonymous request's outcome.
        #[tokio::test]
        async fn an_anonymous_request_is_unaffected_by_configured_identities_existing() {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"ok"));
            let signer = test_signer();
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client.clone(),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(signer.clone()),
            );
            let (prepare, shared_secret) =
                sealed_sample_prepare("g.example.app", &signer.public_key().unwrap());
            let app = router_over(connector, signer, vec![identity("peer-a", "s3cr3t")]);

            let request = request_with_identity(&prepare, None, None);
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let fulfill = Fulfill::decode(&bytes).expect("decode fulfill");
            assert_eq!(
                open_sealed_envelope(&shared_secret, &fulfill.data),
                fulfill_envelope(b"ok")
            );
        }

        #[test]
        fn extract_bearer_tolerates_a_bare_credential_with_no_scheme() {
            let mut headers = HeaderMap::new();
            headers.insert(header::AUTHORIZATION, "s3cr3t".parse().unwrap());
            assert_eq!(extract_bearer(&headers), "s3cr3t");
        }

        #[test]
        fn extract_bearer_matches_the_bearer_scheme_case_insensitively() {
            let mut headers = HeaderMap::new();
            headers.insert(header::AUTHORIZATION, "bearer s3cr3t".parse().unwrap());
            assert_eq!(extract_bearer(&headers), "s3cr3t");
        }

        #[test]
        fn extract_bearer_is_empty_with_no_authorization_header() {
            assert_eq!(extract_bearer(&HeaderMap::new()), "");
        }

        fn state_with_claim_gate(
            claim_gate: crate::claim_gate::ClientClaimGate,
        ) -> ClientEdgeState {
            ClientEdgeState {
                connector: Arc::new(Connector::new(
                    vec![],
                    vec![],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )),
                signer: test_signer(),
                claim_gate: claim_gate.into(),
                wrap_receiver_secret: Some([2u8; 32]),
                node: Arc::new(NodeFacts::default()),
                btp_session_window: DEFAULT_BTP_SESSION_WINDOW,
                session_registry: Arc::new(session_registry::SessionRegistry::new()),
                peers: None,
                identities: Arc::from([]),
            }
        }

        /// AC: "its ephemeral identity derives from the signer of the
        /// claim `ClientClaimGate` already parsed, not from a second parse
        /// of the claim JSON" -- a plaintext claim's already-verified
        /// self-declared signer is threaded out as `plaintext_signer`.
        #[tokio::test]
        async fn a_plaintext_claim_admits_with_its_self_declared_signer() {
            let state = state_with_claim_gate(test_gate(super::claim_headers::test_channels()));
            let claim_json = super::claim_headers::evm_claim_json(1, 100);
            let mut headers = HeaderMap::new();
            headers.insert(
                CLAIM_HEADER,
                BASE64.encode(claim_json.as_bytes()).parse().unwrap(),
            );

            let admitted = extract_and_validate_claim(&headers, 0, &state)
                .await
                .expect("claim admits")
                .expect("claim header present");
            assert_eq!(
                admitted.plaintext_signer.as_deref(),
                Some("0x58da990a8f4a3a6ca7cb6315d68a140105917352")
            );
        }

        /// AC: "a request carrying only a wrapped claim gets the fixed
        /// anonymous identity, even though the connector unwraps it" -- a
        /// wrapped claim's signer must never surface as `plaintext_signer`,
        /// however successfully it unwraps and admits.
        #[tokio::test]
        async fn a_wrapped_claim_admits_with_no_plaintext_signer() {
            let state = state_with_claim_gate(test_gate(super::claim_headers::test_channels()));
            let sender_secret = SecretKey::parse(&[1u8; 32]).unwrap();
            let receiver_secret = SecretKey::parse(&[2u8; 32]).unwrap();
            let receiver_public = PublicKey::from_secret_key(&receiver_secret);
            let claim_json = super::claim_headers::evm_claim_json(1, 100);
            let wrapped = connector_signer::wrap_claim(
                claim_json.as_bytes(),
                &sender_secret,
                &receiver_public.serialize(),
            )
            .expect("wrap");
            let envelope_json = format!(
                r#"{{"ephemeralPublicKey":"{}","encryptedPayload":"{}","timestamp":0,"version":"1.0"}}"#,
                hex_encode(&wrapped.ephemeral_public_key),
                BASE64.encode(&wrapped.encrypted_payload),
            );
            let mut headers = HeaderMap::new();
            headers.insert(
                CLAIM_WRAPPED_HEADER,
                BASE64.encode(envelope_json.as_bytes()).parse().unwrap(),
            );

            let admitted = extract_and_validate_claim(&headers, 0, &state)
                .await
                .expect("claim admits")
                .expect("claim header present");
            assert_eq!(admitted.plaintext_signer, None);
        }
    }
}
