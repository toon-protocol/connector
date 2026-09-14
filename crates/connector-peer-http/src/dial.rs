//! **Dial**: reaching a peer at its `https://` endpoint
//! (`peer-carriage-spec.md` §2, §3, §6, §7.2), behind the
//! [`connector_runtime::PeerTransport`] port.
//!
//! Which carriage this connector dials a given peer on is decided **solely
//! by the scheme of that peer's configured `endpoint`** (§2.1): `https://`
//! is this crate, `wss://` is `connector-peer-btp`. A peer with no endpoint
//! is accept-only and never appears here -- it dials us ([`crate::accept`]).
//! `Config::load` has already refused an endpoint whose scheme is neither
//! (`PeerEndpointScheme`) and a peering that can never establish
//! (`PeerUndialable`), so **dialability is config's answer and is not
//! re-derived here**.
//!
//! # Origination is one-way, and that is the carriage (§2.3, §6.4)
//!
//! On BTP a dialed session is symmetric once established; on HTTP only the
//! dialing side can originate. Everything else about the asymmetry follows
//! from that one sentence: debt flows with packets, packets flow only in the
//! dialing direction, so on a one-way-dialed HTTP peering **the dialing side
//! is structurally the payer**. This module is that side. The other side's
//! consequence -- the flush prompt -- is in [`crate::accept`].
//!
//! # Byte-identical retransmission (§6.3)
//!
//! A payer whose claim went unacknowledged must retransmit the latest
//! pending claim, **byte-identical if nothing has changed**, because a payee
//! is required to answer such a retransmission `accepted` rather than
//! `nonce_not_advancing`. The claim JSON of §4 carries a `timestamp`, so
//! re-rendering it with a fresh `now` would make every retransmission a
//! *different* claim at the same nonce -- which §6.3 says a payee MUST
//! refuse, and one lost ack would then wedge the peering permanently. The
//! receiver's idempotent re-ack is not enough on its own: the payer has to
//! be able to produce the same bytes twice. This transport therefore caches
//! the exact string it emitted for a `(channel, nonce, cumulative,
//! signature)` and reuses it until that claim is acknowledged or superseded,
//! exactly as the BTP carriage does.
//!
//! # One claim in flight per channel (§7.2)
//!
//! The race `client-edge-spec.md` §1.9 exists to remove is present here and
//! absent on BTP: parallel requests carrying nonces *n* and *n+1* reach the
//! payee's watermark lock in either order, and the loser is refused
//! `nonce_not_advancing` for nothing. §7.2's normative mitigation is the one
//! the client edge already ships -- **no more than one claim-bearing request
//! in flight to a peer per channel** -- and it is a per-channel lock held
//! across the request. Requests carrying no claim are unconstrained.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use connector_btp::CLAIM_HEADER;
use connector_config::{PeerCarriage, PeerChannelConfig, PeerConfig};
use connector_domain::{Fulfill, PacketResponse, Prepare, Reject, RejectCode};
use connector_peer_btp::claim_json::{self, PeerClaimDomain};
use connector_runtime::{ClaimAckOutcome, Clock, PeerForward, PeerTransport, WireClaim};
use url::Url;

use crate::headers::{self, Headers, PeerRequest, PeerResponse};

/// What an operator hits first, and diagnoses last, on this carriage.
///
/// §2.4: an operator behind NAT exposes nothing and must dial out; it can
/// hold an inbound-capable session only over a persistent socket, so it must
/// dial **BTP**. Therefore **an HTTP-only peer can neither reach nor be
/// reached by a NAT'd peer**. This is a property of the HTTP carriage, not a
/// defect scheduled for repair, and it is the least obvious thing on this
/// wire to work out from a `T01` -- so every refusal this module produces
/// says it.
pub const NAT_NOTE: &str = "an HTTP-only peer can neither reach nor be reached by a NAT'd peer: \
     the NAT'd side can only dial, and can only receive over a persistent \
     session, so that session must be BTP (peer-carriage-spec.md §2.4)";

/// Why a peer could not be reached. Carries the peer id and the endpoint
/// that was attempted, because §2.2 requires a dial failure name both rather
/// than becoming a runtime mystery.
///
/// Whether the remote actually exposes what we dial is **not** locally
/// detectable (§2.2), so it can never be a load-time error: it surfaces
/// here, and packets routed to that peer reject `T01` -- never `T00`, and
/// never a silent drop.
#[derive(Debug, PartialEq, Eq)]
pub struct HttpDialError {
    pub peer_id: String,
    pub endpoint: String,
    pub reason: String,
}

impl std::fmt::Display for HttpDialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "could not reach peer '{}' at {}: {}",
            self.peer_id, self.endpoint, self.reason
        )
    }
}

/// Puts one request on the wire and brings the response back.
///
/// A port of its own so the carriage's *behaviour* -- the headers, the
/// timeouts, the ack handling, the retransmission cache, §7.2's in-flight
/// rule -- is provable without TLS, a listener or a port number, and so the
/// HTTP library is swappable without touching any of it.
///
/// An implementation MUST NOT interpret the ILP body or any §3 header: it
/// carries bytes. In particular it MUST return a non-`200` response rather
/// than turning it into an error, because §6.2 makes the *status* meaningful
/// -- `4xx`/`5xx` say there is no ILP answer at all.
#[async_trait]
pub trait PeerHttpClient: Send + Sync {
    async fn post(
        &self,
        endpoint: &Url,
        request: PeerRequest,
    ) -> Result<PeerResponse, HttpDialError>;
}

/// One peering relation, as the dial side needs it.
///
/// Per **relation**, never per connection (§2.5): the timeouts and the
/// claim domains belong to the relation, and splitting them per connection
/// is a double-spend surface.
///
/// There is nothing here to present on the way in. ADR 0060 deleted the
/// `{peerId, secret}` credential this used to carry and set on every
/// request: what proves the peering at the far end is the claim covering
/// each packet, which this transport already renders and sends.
#[derive(Debug, Clone)]
pub struct PeerRelation {
    peer_id: String,
    endpoint: Url,
    /// Canonical EVM channel id → the EIP-712 domain its claims are signed
    /// under, from that peering's EVM-shaped `[[peer_channels]]` rows.
    domains: HashMap<String, PeerClaimDomain>,
    /// Solana channel account → the program id its claims render under
    /// (issue #759), from that peering's Solana-shaped `[[peer_channels]]`
    /// rows. See `connector_peer_btp::dial::PeerRelation`'s own field doc;
    /// this carriage holds the identical contract.
    solana_program_ids: HashMap<String, String>,
    peer_answer_timeout: Duration,
    claim_ack_timeout: Duration,
}

impl PeerRelation {
    /// The relation for `peer`, or `None` when this connector does not dial
    /// it over HTTP -- an accept-only peering (no endpoint) or one whose
    /// endpoint's scheme selects the BTP carriage (§2.1).
    #[must_use]
    pub fn from_config(peer: &PeerConfig, channels: &[PeerChannelConfig]) -> Option<PeerRelation> {
        if peer.dial() != Some(PeerCarriage::Http) {
            return None;
        }
        let endpoint = peer.endpoint()?.clone();
        let mine = channels
            .iter()
            .filter(|channel| channel.peer_id() == peer.id());
        let domains = mine
            .clone()
            .filter_map(|channel| match channel {
                PeerChannelConfig::Evm(evm) => Some((
                    claim_json::canonical_evm_channel_id(evm.channel_id()),
                    PeerClaimDomain {
                        chain_id: evm.chain_id(),
                        token_network: evm.token_network(),
                    },
                )),
                PeerChannelConfig::Solana(_) => None,
            })
            .collect();
        let solana_program_ids = mine
            .filter_map(|channel| match channel {
                PeerChannelConfig::Solana(solana) => Some((
                    solana.channel_account().to_string(),
                    solana.program_id().to_string(),
                )),
                PeerChannelConfig::Evm(_) => None,
            })
            .collect();
        Some(PeerRelation {
            peer_id: peer.id().to_string(),
            endpoint,
            domains,
            solana_program_ids,
            peer_answer_timeout: Duration::from_millis(peer.peer_answer_timeout_ms()),
            claim_ack_timeout: Duration::from_millis(peer.claim_ack_timeout_ms()),
        })
    }

    /// A relation assembled by hand -- for a caller that holds no `Config`,
    /// and for tests.
    #[must_use]
    pub fn new(
        peer_id: impl Into<String>,
        endpoint: Url,
        domains: HashMap<String, PeerClaimDomain>,
        solana_program_ids: HashMap<String, String>,
        peer_answer_timeout: Duration,
        claim_ack_timeout: Duration,
    ) -> PeerRelation {
        PeerRelation {
            peer_id: peer_id.into(),
            endpoint,
            domains,
            solana_program_ids,
            peer_answer_timeout,
            claim_ack_timeout,
        }
    }

    /// The peering this relation is for.
    #[must_use]
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }
}

/// What a relation's claim exchange remembers between requests.
#[derive(Default)]
struct Pending {
    /// Canonical channel id → the claim last emitted on it and the exact
    /// JSON string it went out as (§6.3's byte-identical retransmission).
    emitted: HashMap<String, (WireClaim, String)>,
    /// Channels a payee has prompted a flush for and we have not answered
    /// yet (§6.4). A hint and only a hint: nothing here obliges this
    /// connector to do anything, and dropping the set entirely would still
    /// be a conforming payer.
    hinted: HashSet<String>,
}

struct RelationState {
    relation: PeerRelation,
    pending: Mutex<Pending>,
    /// §7.2: canonical channel id → the lock a claim-bearing request to that
    /// channel holds. A `tokio::sync::Mutex` because it is held across the
    /// request's `await`, which is the whole point of holding it.
    in_flight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// The ILP-over-HTTP peer carriage's dial side: one [`PeerTransport`] over
/// however many `https://` peerings this connector dials.
pub struct HttpPeerTransport {
    client: Arc<dyn PeerHttpClient>,
    /// This connector's own EVM address -- the `senderId`/`signerAddress` of
    /// every claim it emits (§4). One per node, because a claim's signer is
    /// `ClaimBook`'s signer.
    signer_address: [u8; 20],
    /// This connector's own ed25519 public key -- the Solana counterpart of
    /// `signer_address` (issue #742). See
    /// `connector_peer_btp::dial::BtpPeerTransport`'s own field doc; this
    /// carriage holds the identical contract.
    solana_signer_public_key: Option<[u8; 32]>,
    clock: Arc<dyn Clock>,
    /// Peer id → that peering's relation and its claim-exchange state.
    ///
    /// Copy-on-write behind an [`ArcSwap`] since ADR 0058: a peering
    /// established over the operator surface must be dialable **while this
    /// process serves**, and a map built once at boot is precisely what
    /// made a runtime peer row a name with nothing behind it. Reads stay on
    /// the packet path and stay lock-free; a write clones a map with one
    /// entry per peering, which is an operator-frequency cost.
    ///
    /// Each entry is an [`Arc`] so a forward already in flight keeps the
    /// state it started on even if the peering is deregistered underneath
    /// it -- the alternative is a claim's pending set vanishing mid-request.
    relations: ArcSwap<HashMap<String, Arc<RelationState>>>,
}

impl HttpPeerTransport {
    #[must_use]
    pub fn new(
        client: Arc<dyn PeerHttpClient>,
        signer_address: [u8; 20],
        clock: Arc<dyn Clock>,
    ) -> Self {
        HttpPeerTransport {
            client,
            signer_address,
            solana_signer_public_key: None,
            clock,
            relations: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Configure this connector's own ed25519 identity for rendering an
    /// outbound Solana peer claim (issue #742) -- the Solana counterpart of
    /// the `signer_address` [`Self::new`] takes for EVM.
    pub fn set_solana_signer_public_key(&mut self, public_key: [u8; 32]) {
        self.solana_signer_public_key = Some(public_key);
    }

    /// Register a peering this connector dials over `https://`, replacing
    /// whatever was registered under the same id.
    ///
    /// Callable while the process serves (ADR 0058), not only during
    /// construction: `POST /peers` establishes a peering and this is how it
    /// becomes reachable without a restart.
    pub fn add_peer(&self, relation: PeerRelation) {
        self.rebind(|relations| {
            relations.insert(
                relation.peer_id.clone(),
                Arc::new(RelationState {
                    relation,
                    pending: Mutex::new(Pending::default()),
                    in_flight: Mutex::new(HashMap::new()),
                }),
            );
        });
    }

    /// Stop dialing `peer_id`. A no-op for an id this transport never
    /// held.
    ///
    /// The other half of runtime registration: `DELETE /peers` is the kill
    /// switch ADR 0060 named when it removed the shared secret, and a kill
    /// switch that leaves the carriage dialing is not one.
    pub fn remove_peer(&self, peer_id: &str) {
        self.rebind(|relations| {
            relations.remove(peer_id);
        });
    }

    /// Replace the relation map with a copy that has `change` applied. See
    /// the field's own doc for what this costs and why the packet path
    /// pays nothing for it.
    fn rebind(&self, change: impl FnOnce(&mut HashMap<String, Arc<RelationState>>)) {
        let mut next = (**self.relations.load()).clone();
        change(&mut next);
        self.relations.store(Arc::new(next));
    }

    /// The relation registered for `peer_id`, held past the read so a
    /// forward in flight is unaffected by a concurrent deregistration.
    fn relation(&self, peer_id: &str) -> Option<Arc<RelationState>> {
        self.relations.load().get(peer_id).cloned()
    }

    /// Every `https://` peering in a loaded config, with its channels.
    pub fn add_peers_from_config(&self, peers: &[PeerConfig], channels: &[PeerChannelConfig]) {
        for peer in peers {
            if let Some(relation) = PeerRelation::from_config(peer, channels) {
                self.add_peer(relation);
            }
        }
    }

    /// The channels a payee has asked this connector to flush and that it
    /// has not answered yet (§6.4). Observability, and how a test asserts a
    /// hint was read at all -- the hint creates no obligation, so nothing in
    /// the packet path depends on this being drained.
    #[must_use]
    pub fn flush_hints(&self, peer_id: &str) -> Vec<String> {
        let Some(state) = self.relation(peer_id) else {
            return Vec::new();
        };
        let state = state.as_ref();
        let pending = state.pending.lock().expect("pending claims lock poisoned");
        let mut hints: Vec<String> = pending.hinted.iter().cloned().collect();
        hints.sort();
        hints
    }

    /// The headers every peer request carries.
    ///
    /// None, now. This used to set `Toon-Peer-Auth` on every request --
    /// HTTP has no session, so the credential had to ride each one. ADR
    /// 0060 deleted it, and what identifies the peering on each request is
    /// what already had to be there: `Toon-Payment-Channel-Claim`. The
    /// function survives as the one place a per-request header would be
    /// added, so a future one is added once rather than at each call site.
    fn base_headers(&self, _state: &RelationState) -> Headers {
        Headers::new()
    }

    /// The claim header for `claim`, reusing the exact bytes already emitted
    /// for it if this is a retransmission (§6.3).
    fn claim_header(&self, state: &RelationState, claim: &WireClaim) -> String {
        let channel_id = claim_json::canonical_evm_channel_id(&claim.channel_id);
        let mut pending = state.pending.lock().expect("pending claims lock poisoned");
        if let Some((emitted, json)) = pending.emitted.get(&channel_id) {
            if emitted == claim {
                return headers::claim_header_value(json);
            }
        }
        // A channel with no `[[peer_channels]]` row here rides without a
        // domain: the fields are optional, and a *zero* domain would be a
        // structurally invalid claim the peer could not even read a verdict
        // out of. Omitted, the peer judges it against its own record and
        // answers `unknown_channel`, which is the right answer to a channel
        // neither end has bound.
        let domain = state.relation.domains.get(&channel_id).copied();
        // Unlike `domain`, a Solana channel with no matching row has no
        // "ride without it" fallback -- `programId` is a required wire
        // field (`claim_json::encode`'s own doc), so a Solana claim
        // reaching here for an unconfigured channel is a caller bug
        // `encode` panics on, exactly as it does for a missing signing
        // identity.
        let solana_program_id = state.relation.solana_program_ids.get(&channel_id);
        let json = claim_json::encode(
            claim,
            &self.signer_address,
            self.solana_signer_public_key.as_ref(),
            solana_program_id.map(String::as_str),
            domain,
            &format!("{channel_id}:{}", claim.nonce),
            &self
                .clock
                .now()
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string(),
        );
        let value = headers::claim_header_value(&json);
        pending.emitted.insert(channel_id, (claim.clone(), json));
        value
    }

    /// An acknowledged claim is no longer pending, so the next claim on that
    /// channel is rendered fresh rather than retransmitted -- and no hint for
    /// it is outstanding.
    fn claim_acknowledged(&self, state: &RelationState, channel_id: &str) {
        let channel_id = claim_json::canonical_evm_channel_id(channel_id);
        let mut pending = state.pending.lock().expect("pending claims lock poisoned");
        pending.emitted.remove(&channel_id);
        pending.hinted.remove(&channel_id);
    }

    /// §7.2: the lock a claim-bearing request to `channel_id` holds, so at
    /// most one is in flight to that channel at a time.
    fn in_flight_lock(
        &self,
        state: &RelationState,
        channel_id: &str,
    ) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            state
                .in_flight
                .lock()
                .expect("in-flight lock map poisoned")
                .entry(claim_json::canonical_evm_channel_id(channel_id))
                .or_default(),
        )
    }

    /// §6.4: record a payee's flush prompt, and drop the ones this
    /// connector cannot act on.
    ///
    /// "A payer with **no** pending claim for the named channel, or that
    /// does not recognise the channel, MUST ignore the header." Ignoring is
    /// what happens to anything not already in the retransmission cache: it
    /// is not answered, not acknowledged, and never an error.
    fn note_flush_hints(&self, state: &RelationState, response: &PeerResponse) {
        let requested = headers::flush_requested(&response.headers);
        if requested.is_empty() {
            return;
        }
        let mut pending = state.pending.lock().expect("pending claims lock poisoned");
        for channel_id in requested {
            let channel_id = claim_json::canonical_evm_channel_id(&channel_id);
            if pending.emitted.contains_key(&channel_id) {
                pending.hinted.insert(channel_id);
            } else {
                tracing::debug!(
                    peer_id = %state.relation.peer_id,
                    %channel_id,
                    "peer prompted a flush for a channel we hold no pending claim on; ignored"
                );
            }
        }
    }

    /// The pending claim a hint asks for, if this request can carry it
    /// (§6.4).
    ///
    /// A payer holding a pending claim for a hinted channel "SHOULD send that
    /// claim on its next request to that peer, or immediately as a standalone
    /// claim POST". This is the first of those, and it is the cheap one: no
    /// extra round trip, and the bytes are the ones already emitted, so it is
    /// §6.3's byte-identical retransmission rather than a new claim. It can
    /// only ride a request that carries no claim of its own -- one request
    /// carries at most one claim (§3, §7.2).
    fn hinted_retransmission(&self, state: &RelationState) -> Option<(WireClaim, String)> {
        let pending = state.pending.lock().expect("pending claims lock poisoned");
        let channel_id = pending.hinted.iter().next()?;
        let (claim, json) = pending.emitted.get(channel_id)?;
        Some((claim.clone(), headers::claim_header_value(json)))
    }

    /// §6.2/§6.3: the ack answers the claim, independently of whatever the
    /// body said about the packet. **Absence and malformation both mean not
    /// acknowledged**, and an ack on a response answering a request that
    /// carried no claim is ignored.
    fn read_ack(
        &self,
        state: &RelationState,
        claim: Option<&WireClaim>,
        response: &PeerResponse,
    ) -> ClaimAckOutcome {
        let Some(claim) = claim else {
            return ClaimAckOutcome::NotSent;
        };
        let ack = headers::claim_ack(&response.headers).unwrap_or(ClaimAckOutcome::NotSent);
        if ack == ClaimAckOutcome::Accepted {
            self.claim_acknowledged(state, &claim.channel_id);
        }
        ack
    }

    async fn post(
        &self,
        state: &RelationState,
        request: PeerRequest,
        timeout: Duration,
    ) -> Option<PeerResponse> {
        let answered =
            tokio::time::timeout(timeout, self.client.post(&state.relation.endpoint, request))
                .await;
        match answered {
            Ok(Ok(response)) if response.answers_the_packet() => Some(response),
            // §6.2: `4xx`/`5xx` are reserved for a malformed request or a
            // connector fault -- there is no ILP answer, so there is nothing
            // to read, and in particular no ack to read off it.
            Ok(Ok(response)) => {
                tracing::warn!(
                    peer_id = %state.relation.peer_id,
                    endpoint = %state.relation.endpoint,
                    status = response.status,
                    "peer answered with no ILP body; {NAT_NOTE}"
                );
                None
            }
            // `HttpDialError` carries the endpoint it attempted but not the
            // peer id -- the client that mints one holds a URL and nothing
            // else -- so the relation's own id is logged beside it. Without
            // that, the one line naming *why* a dial failed (a refused
            // connection, or an onion endpoint on a node with no
            // `socks_proxy`) does not say which peering it was for.
            Ok(Err(error)) => {
                tracing::warn!(
                    peer_id = %state.relation.peer_id,
                    %error,
                    "peer request failed; {NAT_NOTE}"
                );
                None
            }
            // §6.3 on expiry: the claim is **not acknowledged**. The peering
            // is not torn down, no new claim is minted at a higher nonce for
            // the same cumulative, and the packet's value stays in this
            // connector's owed projection.
            Err(_) => {
                tracing::warn!(
                    peer_id = %state.relation.peer_id,
                    endpoint = %state.relation.endpoint,
                    timeout_ms = timeout.as_millis(),
                    "peer did not answer in time; {NAT_NOTE}"
                );
                None
            }
        }
    }
}

/// §6.4(1), the consequence that actually bites at configuration time: on
/// HTTP only the dialing side can originate, so **the accept-only side can
/// never forward a packet to that peer**. Where a route naming it as next
/// hop was detectable at load, `Config::load` already refused it
/// (`PeerRouteUndeliverable`); where it was not, this is the `T01`, and it
/// says why rather than leaving an operator to infer it from a route table.
fn peer_not_dialable(peer_id: &str) -> PacketResponse {
    PacketResponse::Reject(Reject {
        code: RejectCode::t01_peer_unreachable(),
        triggered_by: String::new(),
        message: format!(
            "peer '{peer_id}' is not dialable over HTTP from this connector: on HTTP only the \
             dialing side can originate, so packets flow only in the dialing direction \
             (peer-carriage-spec.md §6.4(1)). {NAT_NOTE}"
        ),
        data: Vec::new(),
        accumulated_cost: 0,
    })
}

/// §2.2: a dial that did not reach the peer rejects **`T01` naming both the
/// peer and the endpoint that was attempted** -- never `T00`, and never a
/// silent drop.
///
/// The endpoint is here because §2.2 requires a dial failure to name it
/// rather than become a runtime mystery, and because since ADR 0070 an
/// endpoint is the whole of why a dial went where it went: an operator
/// reading this reject on a node with no `socks_proxy` sees the `.onion`
/// host that could not be reached, and the log line beside it says why.
/// The reason itself stays in the log rather than riding upstream --
/// what the previous hop needs is that this hop could not deliver.
fn dial_failed(peer_id: &str, endpoint: &Url) -> PacketResponse {
    PacketResponse::Reject(Reject {
        code: RejectCode::t01_peer_unreachable(),
        triggered_by: String::new(),
        message: format!("peer '{peer_id}' unreachable at {endpoint}"),
        data: Vec::new(),
        accumulated_cost: 0,
    })
}

/// Read a peer's answer back off the response: the packet's own verdict from
/// the body, and a REJECT's running cost from the `Toon-Accumulated-Cost`
/// header the client edge already uses (§5.2).
fn decode_answer(response: &PeerResponse) -> Option<PacketResponse> {
    if let Ok(fulfill) = Fulfill::decode(&response.body) {
        return Some(PacketResponse::Fulfill(fulfill));
    }
    let mut reject = Reject::decode(&response.body).ok()?;
    reject.accumulated_cost = headers::accumulated_cost(&response.headers);
    Some(PacketResponse::Reject(reject))
}

#[async_trait]
impl PeerTransport for HttpPeerTransport {
    async fn forward(
        &self,
        peer_id: &str,
        prepare: Prepare,
        claim: Option<WireClaim>,
    ) -> PeerForward {
        let Some(state) = self.relation(peer_id) else {
            tracing::warn!(peer_id, "no HTTP peering to originate to; {NAT_NOTE}");
            return PeerForward {
                response: peer_not_dialable(peer_id),
                ..PeerForward::unreachable(peer_id)
            };
        };
        let state = state.as_ref();

        let mut request = PeerRequest {
            headers: self.base_headers(state),
            // §8.1: `data` rides byte-for-byte unchanged. `Prepare::encode`
            // is the same OER encoding every other carriage puts on a wire,
            // and nothing here re-wraps, pads or truncates a payload it holds
            // no key for.
            body: prepare.encode(),
        };
        // A hinted retransmission rides only a request that carries no claim
        // of its own, and only ever as the *same bytes* already emitted
        // (§6.3, §6.4). It is this connector's own housekeeping, not the
        // caller's claim: the caller sent none, so it is told none was
        // acknowledged, and the ack is applied to the retransmission cache
        // here instead.
        let hinted = claim
            .is_none()
            .then(|| self.hinted_retransmission(state))
            .flatten();
        let carried = claim.as_ref().map(|claim| self.claim_header(state, claim));
        let claim_channel = claim
            .as_ref()
            .map(|claim| claim.channel_id.clone())
            .or_else(|| hinted.as_ref().map(|(claim, _)| claim.channel_id.clone()));
        if let Some(value) = carried.or_else(|| hinted.as_ref().map(|(_, value)| value.clone())) {
            request.headers.push(CLAIM_HEADER, value);
        }

        // §7.2: at most one claim-bearing request in flight per channel.
        // Requests carrying no claim are unconstrained, so the lock is taken
        // only when one rides.
        let lock = claim_channel
            .as_deref()
            .map(|channel_id| self.in_flight_lock(state, channel_id));
        let _guard = match lock.as_ref() {
            Some(lock) => Some(lock.lock().await),
            None => None,
        };

        let Some(response) = self
            .post(state, request, state.relation.peer_answer_timeout)
            .await
        else {
            return PeerForward {
                response: dial_failed(peer_id, &state.relation.endpoint),
                ..PeerForward::unreachable(peer_id)
            };
        };
        self.note_flush_hints(state, &response);

        let ack = self.read_ack(state, claim.as_ref(), &response);
        if let Some((hinted, _)) = hinted.as_ref() {
            // The caller's accounting is untouched: it sent no claim, so it
            // is told `NotSent` below. Ours is not -- an accepted
            // retransmission is settled and must stop being retransmitted.
            if headers::claim_ack(&response.headers) == Some(ClaimAckOutcome::Accepted) {
                self.claim_acknowledged(state, &hinted.channel_id);
            }
        }

        match decode_answer(&response) {
            // No terms are ever reported on this carriage: reading the HTTP
            // 402's own `payment-required` body is issue #874's BTP-side
            // change and has no twin here yet, so a peer that greets this
            // connector over HTTP is reported as an ordinary refusal --
            // absence of terms, never an unreadable greeting silently
            // downgraded (see `PeerForward::payment_required`).
            Some(answer) => PeerForward::answered(answer, ack),
            None => {
                tracing::warn!(peer_id, "peer answer carried no decodable ILP packet");
                PeerForward::undecodable(peer_id, ack)
            }
        }
    }

    async fn flush(&self, peer_id: &str, claim: WireClaim) -> ClaimAckOutcome {
        let Some(state) = self.relation(peer_id) else {
            tracing::warn!(peer_id, "no HTTP peering to flush to; {NAT_NOTE}");
            return ClaimAckOutcome::NotSent;
        };
        let state = state.as_ref();

        // FLUSH (§3): a **POST with an empty ILP body** plus the claim
        // header -- the standalone-claim shape the client edge already
        // defines (`client-edge-spec.md` §1.9 step 5).
        let mut request = PeerRequest {
            headers: self.base_headers(state),
            body: Vec::new(),
        };
        request
            .headers
            .push(CLAIM_HEADER, self.claim_header(state, &claim));

        let lock = self.in_flight_lock(state, &claim.channel_id);
        let _guard = lock.lock().await;

        let Some(response) = self
            .post(state, request, state.relation.claim_ack_timeout)
            .await
        else {
            return ClaimAckOutcome::NotSent;
        };
        self.note_flush_hints(state, &response);
        self.read_ack(state, Some(&claim), &response)
    }
}
