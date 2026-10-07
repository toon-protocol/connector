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
//! is structurally the payer**. This module is that side; [`crate::accept`]
//! is the other.
//!
//! # What rides a forwarded PREPARE (ADR 0075 decisions 5 and 6)
//!
//! A voucher on this node's own outbound x402 channel, or -- for a packet
//! that moves no value -- the peer-role challenge. Both arrive rendered, from
//! `Connector::cover_forward`, and ride their headers verbatim: a voucher's
//! freshness is its amount, so a resend is recognised by its signature
//! rather than its bytes (ADR 0074 decision 3), and this transport caches
//! nothing. No `toon-channel` claim is rendered or sent here any more
//! (#1380), and the claim-only FLUSH went with it.
//!
//! # One voucher in flight per peering (§7.2)
//!
//! The race `client-edge-spec.md` §1.9 exists to remove is present here and
//! absent on BTP: parallel requests carrying vouchers for cumulative amounts
//! *a* and *a' > a* reach the payee's watermark lock in either order, and
//! the loser is refused as not advancing for nothing. §7.2's normative
//! mitigation is the one the client edge already ships -- **no more than
//! one voucher-bearing request in flight to a peer** -- and it is a lock per
//! relation held across the request, since a peering's vouchers are
//! cumulative on one outbound channel. Requests carrying no voucher are
//! unconstrained.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use connector_btp::{CLAIM_HEADER, PEER_CHALLENGE_HEADER};
use connector_config::{PeerCarriage, PeerConfig};
use connector_domain::{Fulfill, PacketResponse, Prepare, Reject, RejectCode};
use connector_runtime::{AnswerWait, ClaimAckOutcome, Covering, PeerForward, PeerTransport};
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
/// timeouts, the ack handling, §7.2's in-flight rule -- is provable without
/// TLS, a listener or a port number, and so the
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
/// Per **relation**, never per connection (§2.5): the timeout belongs to the
/// relation.
///
/// There is nothing here to present on the way in. ADR 0060 deleted the
/// `{peerId, secret}` credential this used to carry and set on every
/// request: what proves the peering at the far end is the voucher covering
/// each packet, or the peer-role challenge on one that moves no value (ADR
/// 0075 decision 5), which arrive rendered with the packet.
#[derive(Debug, Clone)]
pub struct PeerRelation {
    peer_id: String,
    endpoint: Url,
    peer_answer_timeout: Duration,
}

impl PeerRelation {
    /// The relation for `peer`, or `None` when this connector does not dial
    /// it over HTTP -- an accept-only peering (no endpoint) or one whose
    /// endpoint's scheme selects the BTP carriage (§2.1).
    #[must_use]
    pub fn from_config(peer: &PeerConfig) -> Option<PeerRelation> {
        if peer.dial() != Some(PeerCarriage::Http) {
            return None;
        }
        Some(PeerRelation {
            peer_id: peer.id().to_string(),
            endpoint: peer.endpoint()?.clone(),
            peer_answer_timeout: Duration::from_millis(peer.peer_answer_timeout_ms()),
        })
    }

    /// A relation assembled by hand -- for a caller that holds no `Config`,
    /// and for tests.
    #[must_use]
    pub fn new(
        peer_id: impl Into<String>,
        endpoint: Url,
        peer_answer_timeout: Duration,
    ) -> PeerRelation {
        PeerRelation {
            peer_id: peer_id.into(),
            endpoint,
            peer_answer_timeout,
        }
    }

    /// The peering this relation is for.
    #[must_use]
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }
}

struct RelationState {
    relation: PeerRelation,
    /// §7.2: the lock a voucher-bearing request to this relation holds, so
    /// at most one is in flight at a time. A peering's vouchers are
    /// cumulative on one outbound channel, so two in flight at once could
    /// arrive out of order and the lower be refused as not advancing. A
    /// `tokio::sync::Mutex` because it is held across the request's
    /// `await`, which is the whole point of holding it.
    voucher_in_flight: tokio::sync::Mutex<()>,
}

/// The ILP-over-HTTP peer carriage's dial side: one [`PeerTransport`] over
/// however many `https://` peerings this connector dials.
pub struct HttpPeerTransport {
    client: Arc<dyn PeerHttpClient>,
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
    /// it.
    relations: ArcSwap<HashMap<String, Arc<RelationState>>>,
}

impl HttpPeerTransport {
    #[must_use]
    pub fn new(client: Arc<dyn PeerHttpClient>) -> Self {
        HttpPeerTransport {
            client,
            relations: ArcSwap::from_pointee(HashMap::new()),
        }
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
                    voucher_in_flight: tokio::sync::Mutex::new(()),
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

    /// Every `https://` peering in a loaded config.
    pub fn add_peers_from_config(&self, peers: &[PeerConfig]) {
        for peer in peers {
            if let Some(relation) = PeerRelation::from_config(peer) {
                self.add_peer(relation);
            }
        }
    }

    /// The headers every peer request carries.
    ///
    /// None, now. This used to set `Toon-Peer-Auth` on every request --
    /// HTTP has no session, so the credential had to ride each one. ADR
    /// 0060 deleted it, and what identifies the peering on each request is
    /// what already had to be there: the voucher in
    /// `Toon-Payment-Channel-Claim`, or the peer-role challenge. The
    /// function survives as the one place a per-request header would be
    /// added, so a future one is added once rather than at each call site.
    fn base_headers(&self, _state: &RelationState) -> Headers {
        Headers::new()
    }

    /// POST `request`, waiting at most `wait`. `Err(true)` is a wait that
    /// ended at the packet's outgoing expiry (PF-26), `Err(false)` any other
    /// failure to be answered, the answer timeout included.
    async fn post(
        &self,
        state: &RelationState,
        request: PeerRequest,
        wait: AnswerWait,
    ) -> Result<PeerResponse, bool> {
        let timeout = wait.span;
        let answered =
            tokio::time::timeout(timeout, self.client.post(&state.relation.endpoint, request))
                .await;
        match answered {
            Ok(Ok(response))
                if response.answers_the_packet() || response.quoted_terms().is_some() =>
            {
                Ok(response)
            }
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
                Err(false)
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
                Err(false)
            }
            // §6.3 on expiry: the voucher is **not acknowledged**. The
            // peering is not torn down; the next forward asks the next hop's
            // claim-state where the channel stands before it signs again.
            Err(_) => {
                tracing::warn!(
                    peer_id = %state.relation.peer_id,
                    endpoint = %state.relation.endpoint,
                    timeout_ms = timeout.as_millis(),
                    ran_out_at_expiry = wait.ends_at_expiry,
                    "peer did not answer in time; {NAT_NOTE}"
                );
                Err(wait.ends_at_expiry)
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

impl HttpPeerTransport {
    /// [`PeerTransport::forward`], with every wait also ended at `budget`
    /// when one is given -- the packet's outgoing expiry (PF-26). Waiting for
    /// the relation's voucher-in-flight turn is bounded by it too, since that
    /// is a wait on the forward path like any other.
    async fn forward_bounded(
        &self,
        peer_id: &str,
        prepare: Prepare,
        covering: Option<Covering>,
        budget: Option<Duration>,
    ) -> PeerForward {
        let started = tokio::time::Instant::now();
        // A voucher or a challenge arrives rendered and rides its own header
        // verbatim (§1.4, §4).
        let rendered = match covering {
            Some(Covering::Voucher(json)) => Some((CLAIM_HEADER, json, true)),
            Some(Covering::Challenge(json)) => Some((PEER_CHALLENGE_HEADER, json, false)),
            None => None,
        };
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
        let voucher = matches!(rendered, Some((_, _, true)));
        if let Some((header, json, _)) = rendered.as_ref() {
            request
                .headers
                .push(*header, headers::claim_header_value(json));
        }

        // §7.2: at most one voucher-bearing request in flight per relation.
        // Requests carrying none are unconstrained, so the lock is taken
        // only when one rides.
        let _guard = if voucher {
            match budget {
                None => Some(state.voucher_in_flight.lock().await),
                Some(budget) => {
                    match tokio::time::timeout(budget, state.voucher_in_flight.lock()).await {
                        Ok(guard) => Some(guard),
                        Err(_) => return PeerForward::ran_out_at_expiry(peer_id),
                    }
                }
            }
        } else {
            None
        };
        // What the turn cost comes off the packet's budget; the answer
        // timeout starts afresh, as it always did.
        let budget = budget.map(|budget| budget.saturating_sub(started.elapsed()));
        let wait = AnswerWait::new(state.relation.peer_answer_timeout, budget);

        let response = match self.post(state, request, wait).await {
            Ok(response) => response,
            Err(true) => return PeerForward::ran_out_at_expiry(peer_id),
            Err(false) => {
                return PeerForward {
                    response: dial_failed(peer_id, &state.relation.endpoint),
                    ..PeerForward::unreachable(peer_id)
                };
            }
        };

        // §6.1/§6.2: the ack answers the voucher, independently of whatever
        // the body said about the packet. Absence and malformation both mean
        // not acknowledged, and an ack on a response to a request that
        // carried no voucher is ignored.
        let ack = if voucher {
            headers::claim_ack(&response.headers).unwrap_or(ClaimAckOutcome::NotSent)
        } else {
            ClaimAckOutcome::NotSent
        };

        // A `402` with readable terms is the far node's client edge greeting
        // a claimless packet (#1481). It carries no ILP body, so the reject
        // is this carriage's reading of it, built as the BTP greeting is: an
        // `F06` stating the charge quoted for this packet, with the terms
        // reported beside it for the connector's one retry-or-decline rule.
        // The ack is never read off it: no voucher was admitted.
        if let Some(terms) = response.quoted_terms() {
            tracing::info!(
                peer_id,
                price = terms.price().unwrap_or_default(),
                resource = %terms.resource.url,
                required_transport = terms.required_transport().unwrap_or_default(),
                "peer answered a forwarded PREPARE with a 402 and x402 terms"
            );
            let reject = Reject {
                code: RejectCode::f06_unexpected_payment(),
                triggered_by: String::new(),
                message: "No payment channel claim attached".to_string(),
                data: Vec::new(),
                accumulated_cost: terms.price().unwrap_or_default(),
            };
            return PeerForward::quoted(
                PacketResponse::Reject(reject),
                ClaimAckOutcome::NotSent,
                terms,
            );
        }

        match decode_answer(&response) {
            // An ordinary answer quotes no terms on this carriage: the peer
            // price gate's `200` is not read for a `Payment-Required` header
            // (it would turn greeted retries on for bound peers). Absence of
            // terms, never an unreadable greeting silently downgraded (see
            // `PeerForward::payment_required`).
            Some(answer) => PeerForward::answered(answer, ack),
            None => {
                tracing::warn!(peer_id, "peer answer carried no decodable ILP packet");
                PeerForward::undecodable(peer_id, ack)
            }
        }
    }
}

#[async_trait]
impl PeerTransport for HttpPeerTransport {
    async fn forward(
        &self,
        peer_id: &str,
        prepare: Prepare,
        covering: Option<Covering>,
    ) -> PeerForward {
        self.forward_bounded(peer_id, prepare, covering, None).await
    }

    async fn forward_within(
        &self,
        peer_id: &str,
        prepare: Prepare,
        covering: Option<Covering>,
        budget: Duration,
    ) -> PeerForward {
        self.forward_bounded(peer_id, prepare, covering, Some(budget))
            .await
    }
}
