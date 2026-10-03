//! Operator router, mountable rather than a server. See ADR 0001, ADR 0008.
//!
//! ADR 0008 splits the operator surface into a read half and a write
//! half. The read half (issue #420) is `GET` endpoints -- peers, routes,
//! channels, claims, node identity, this crate's own write audit log, and
//! the metrics surface (`GET /metrics`, ADR 0014) -- gated by a bearer
//! token and nothing else.
//!
//! This crate also carries the write half's authentication mechanism
//! (issue #421): [`rfc9421`] verifies an RFC 9421 signature from a key on
//! an operator write allowlist, with the body bound by RFC 9530
//! Content-Digest, and [`write_auth::WriteAuth`] adds replay rejection and
//! retains every accepted signature as its write's audit record (ADR
//! 0012), exposed for inspection at `GET /audit-log`.
//!
//! `POST /packets` -- originating a packet outward -- `POST /routes/leased`
//! -- creating or renewing a leased route (issue #427) -- and the channel
//! writes of ADR 0075 decision 11 (issue #1376): `POST /channels` (open an
//! outbound x402 channel, journaled before it is sent), `POST
//! /channels/:id/fund` (top one up by an increment), `POST
//! /channels/:id/withdraw` (start a withdrawal, and finish it once due) and
//! `POST /channels/:id/land` (land the latest voucher held on an inbound
//! channel now) -- are this crate's write endpoints, beside the peering and
//! route writes. The `toon-channel` writes `redeem`, `redeem-latest`,
//! `close`, `settle` and `cooperative-close` are deleted, and so are the
//! EVM `toon-channel` open and top-up (#1376, #1378): a peering, on either
//! chain (#1378, #1379), opens and funds its outbound x402 channel through
//! `POST /peers` itself. The Solana `toon-channel` open and top-up
//! followed with #1383: `POST /channels` without `terms`, and `/fund` on
//! anything but an outbound x402 channel, are refused by name. Every one
//! calls
//! [`write_auth::authenticate_write`] first and nothing else in this
//! crate accepts a body, so a write cannot reach [`Connector`] without a
//! valid, allowlisted, unexpired, non-replayed signature. Bearer tokens
//! gate reads and reads only; no shared secret is ever sufficient to move
//! value. Channel writes 503 rather than reach [`Connector`] at all on a
//! node with no x402 batch-settlement backend configured.
//!
//! Per ADR 0001, each read handler below deserializes nothing beyond the
//! bearer token (a GET request has no body) and calls exactly one
//! [`Connector`] method. Every read serializes its result as JSON except
//! `GET /metrics`, which is Prometheus text exposition format (ADR 0014)
//! -- the one format Prometheus itself can scrape.
//!
//! `GET /dashboard` is the operator dashboard (ADR 0066): one static page,
//! embedded from `dashboard.html` at build time and served with no
//! authentication, because it holds nothing. Every figure on it is
//! fetched through the bearer-gated reads above, and every change it
//! makes is an RFC 9421 write signed in the browser by an operator key
//! that never leaves it. It is a client of this surface that happens to
//! be shipped by it -- mounted with the surface, absent without it, and
//! granting no authority the reads and writes do not already grant.

mod rfc9421;
mod write_auth;

/// Signing helpers for constructing a validly-signed operator write from
/// outside this crate.
///
/// Ungated. These were behind `test-util` while the only callers were tests,
/// but `connector send` (the binary's third verb) signs a real
/// `POST /packets` with exactly these, so they are shipped code now. The
/// verification half is unaffected and stays private to this crate.
pub mod signing {
    pub use crate::rfc9421::{compute_content_digest, keyid_hex, sign_request};
}

/// The old name for [`signing`], kept so the `test-util` feature keeps
/// meaning what it meant to existing callers (`connector-cli`'s settlement
/// lifecycle test, issue #542). New code should use [`signing`] directly --
/// there is nothing test-only about it any more.
#[cfg(feature = "test-util")]
pub mod test_support {
    pub use crate::signing::{compute_content_digest, keyid_hex, sign_request};
}

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Method, Request, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use connector_client_edge::ClientClaimGate;
use connector_domain::x402::X402BatchSettlementTerms;
use connector_domain::{Prepare, Price};
use connector_runtime::{
    BatchChannelError, BatchChannelView, BatchChannels, ClaimBookKind, ClaimDirection, ClaimScheme,
    ClaimView, Connector, DeclaredRates, EstablishPeeringError, LeaseRouteError, LeasedRouteView,
    PeerRouteTableError, PeerRouteView, PeerView, RateView, RouteView, SelfDescriptionError,
    SettlementChain, WithdrawStep,
};
use connector_settlement::batch::BatchSettlementError;
use connector_signer::{derive_evm_address, to_hex, Signer, SignerError};
use url::Url;
use write_auth::{authenticate_write, AuditRecord, WriteAuth};

/// This node's own identity: the active signing key and the address
/// derived from it (ADR 0012's signer, read rather than exercised).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeIdentity {
    pub key_id: String,
    pub address: String,
}

#[derive(Clone)]
struct OperatorState {
    connector: Arc<Connector>,
    /// The client edge's own claim book (issue #1218): money accepted at
    /// `POST /ilp` -- ADR 0058's peering flow, or any ordinary paying
    /// client on a chain-resolved channel -- lands here, not in
    /// [`Connector`]'s peer-semantics `ClaimBook`, and before this field
    /// existed nothing on this surface could see it: `GET /claims` and
    /// `GET /channels` answered empty and `redeem-latest` refused while
    /// the client edge's own journal held the claim. Reads and redeems
    /// below now consult whichever book is the channel's actual authority
    /// -- peer book first, this one otherwise -- the same doctrine
    /// `POST /ilp/claim-state` established (issue #1102/#1103). Since ADR
    /// 0075 (#1380) the peer book is only the replay of a journal an older
    /// build wrote.
    claim_gate: Arc<ClientClaimGate>,
    signer: Arc<dyn Signer>,
    bearer_token: Arc<str>,
    write_auth: Arc<WriteAuth>,
    /// This node's declared pairs, or `None` for a node that deals none
    /// (issue #1297). `None` is every node that predates ADR 0071 and
    /// every node that declares no `[[tokens]]`: `GET /rates` answers an
    /// empty list for it, because "this node deals nothing" is an answer
    /// and a `404` would make an operator wonder whether the surface was
    /// too old to have the endpoint.
    rates: Option<DeclaredRates>,
    /// This node's x402 `batch-settlement` channels, both ways (ADR 0075
    /// decision 11), or `None` on a node with no x402 backend: its x402
    /// writes answer `503`, and its reads list none.
    batch: Option<Arc<BatchChannels>>,
}

/// [`router_with_batch_channels`] for a node with no x402 batch-settlement
/// backend.
pub fn router(
    connector: Arc<Connector>,
    claim_gate: Arc<ClientClaimGate>,
    signer: Arc<dyn Signer>,
    bearer_token: impl Into<String>,
    write_keys: Vec<[u8; 32]>,
    declared_rates: Option<DeclaredRates>,
) -> Router {
    router_with_batch_channels(
        connector,
        claim_gate,
        signer,
        bearer_token,
        write_keys,
        declared_rates,
        None,
    )
}

/// Mount the operator surface's read-only half at `connector`: `GET`
/// endpoints for peers, routes, channels, claims, declared rates, node
/// identity and the write audit log, each requiring the bearer token
/// `bearer_token` and nothing more (ADR 0008). `write_keys` is the
/// allowlist of ed25519 public keys permitted to sign a write once a
/// write endpoint lands (issue #421); removing a key from this list and
/// restarting revokes it, with no other change. `claim_gate` is the same
/// client-edge claim book `connector_client_edge::router_with_node_facts`
/// (or its callers) already built for `POST /ilp` -- this surface never
/// constructs its own, so the two never drift (issue #1218).
/// `declared_rates` is this node's dealing, read over the same shared
/// table the forwarding path converts against (issue #1297, ADR 0071) --
/// `None` for a node that declares no `[[tokens]]`, which is every node
/// predating the record. `batch_channels` is this node's x402 channels,
/// both ways (ADR 0075 decision 11): what the x402 channel writes drive and
/// `GET /channels` and `GET /claims` list.
pub fn router_with_batch_channels(
    connector: Arc<Connector>,
    claim_gate: Arc<ClientClaimGate>,
    signer: Arc<dyn Signer>,
    bearer_token: impl Into<String>,
    write_keys: Vec<[u8; 32]>,
    declared_rates: Option<DeclaredRates>,
    batch_channels: Option<Arc<BatchChannels>>,
) -> Router {
    let state = OperatorState {
        connector,
        claim_gate,
        signer,
        bearer_token: Arc::from(bearer_token.into()),
        write_auth: Arc::new(WriteAuth::new(write_keys)),
        rates: declared_rates,
        batch: batch_channels,
    };

    // Reads: gated by the bearer token and nothing else. Writes: gated by
    // an RFC 9421 signature and nothing else (ADR 0008) -- `route_layer`
    // only wraps the routes already added to `reads` when it is called,
    // so `writes`, merged in afterward, is never behind the bearer token.
    let reads = Router::new()
        .route("/peers", get(peers))
        .route("/routes", get(routes))
        .route("/routes/leased", get(leased_routes))
        .route("/routes/peers", get(peer_routes))
        .route("/channels", get(channels))
        .route("/claims", get(claims))
        .route("/rates", get(rates))
        .route("/identity", get(identity))
        .route("/audit-log", get(audit_log))
        .route("/metrics", get(metrics))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_token,
        ));

    let writes = Router::new()
        .route("/packets", post(originate_packet))
        .route("/routes/leased", post(create_leased_route))
        .route("/peers", post(upsert_peer))
        .route("/peers/:id", delete(remove_peer))
        .route("/routes/peers", post(upsert_peer_route))
        .route("/routes/peers/:prefix", delete(remove_peer_route))
        .route("/channels", post(open_channel))
        .route("/channels/:id/fund", post(fund_channel))
        .route("/channels/:id/withdraw", post(withdraw_channel))
        .route("/channels/:id/land", post(land_channel));

    // The dashboard page: served without a token because it is inert
    // markup -- it holds no figure and no key, and only becomes anything
    // once a browser feeds it the bearer token and an operator key, both of
    // which stay on the operator's side (ADR 0066). Mounted alongside the
    // surface it fronts, so a node with no `[operator]` has no page either.
    let pages = Router::new().route("/dashboard", get(dashboard));

    reads.merge(writes).merge(pages).with_state(state)
}

/// The operator dashboard, embedded at build time so the image ships it
/// and there is nothing else to build, host or deploy (ADR 0066).
const DASHBOARD_HTML: &str = include_str!("dashboard.html");

/// `GET /dashboard`: the page itself. The Content-Security-Policy is the
/// load-bearing header: `connect-src 'self'` means the page can talk to
/// this origin and nowhere else, so an operator key pasted into it can
/// sign for this node and cannot be carried off by anything the page might
/// be tricked into loading -- it loads nothing at all (`default-src
/// 'none'`), and a dashboard that renders API data builds DOM text rather
/// than markup for the same reason. `no-store` because the page is served
/// by the node it describes: after a release the browser should see the
/// new page, not a cached one against a changed surface.
async fn dashboard() -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
                 connect-src 'self'; form-action 'none'; base-uri 'none'; frame-ancestors 'none'",
            ),
        ],
        DASHBOARD_HTML,
    )
        .into_response()
}

/// Authenticate a write request against `state`'s [`WriteAuth`], returning
/// the `401 Unauthorized` status and body to send back immediately on
/// failure (kept as a plain `(StatusCode, String)` here rather than a
/// pre-built [`Response`] so this stays a small `Result::Err`, matching
/// [`write_auth::authenticate_write`]'s own reasoning for returning a plain
/// [`write_auth::WriteAuthError`] instead). Every write handler below calls
/// this first, before touching the body for anything else -- see the
/// module docs.
fn require_write_auth(
    state: &OperatorState,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<(), (StatusCode, String)> {
    authenticate_write(
        &state.write_auth,
        method.as_str(),
        uri.path(),
        headers,
        body,
    )
    .map(|_| ())
    .map_err(|error| (StatusCode::UNAUTHORIZED, error.to_string()))
}

async fn require_bearer_token<B>(
    State(state): State<OperatorState>,
    request: Request<B>,
    next: Next<B>,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));

    match presented {
        Some(token) if token == state.bearer_token.as_ref() => next.run(request).await,
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn peers(State(state): State<OperatorState>) -> Json<Vec<PeerView>> {
    Json(state.connector.peers())
}

async fn routes(State(state): State<OperatorState>) -> Json<Vec<RouteView>> {
    Json(state.connector.routes())
}

/// `GET /routes/leased`: every leased route (issue #427) not yet lapsed as
/// of this node's own clock -- the read side of the same table
/// `POST /routes/leased` writes to.
async fn leased_routes(State(state): State<OperatorState>) -> Json<Vec<LeasedRouteView>> {
    Json(state.connector.leased_routes())
}

/// `GET /channels`: every x402 `batch-settlement` channel (ADR 0075
/// decision 11) -- inbound ones this node holds a voucher on and outbound
/// ones it opened, each with its `direction`, `collateral`, `watermark` and
/// `status`. TOON's own channels, which this list used to lead with, are
/// deleted (issue #1385).
async fn channels(State(state): State<OperatorState>) -> Json<Vec<serde_json::Value>> {
    let mut views = Vec::new();
    if let Some(batch) = &state.batch {
        views.extend(
            batch
                .views()
                .await
                .iter()
                .map(|view| serde_json::to_value(view).expect("a channel view serializes")),
        );
    }
    Json(views)
}

/// `GET /claims`: every voucher this node holds -- those received, the
/// client edge's book (issue #1218), journaled to `client-edge-claims.log`,
/// and those signed, the outbound channels' own book, one row per channel
/// at the highest amount signed on it (ADR 0075 decision 11). `book` on
/// each row says which one it came from; a received voucher is always
/// inbound and never pending.
async fn claims(State(state): State<OperatorState>) -> Json<Vec<ClaimView>> {
    let mut views: Vec<ClaimView> = state
        .claim_gate
        .accepted_channels()
        .into_iter()
        .map(|(channel_id, watermark)| ClaimView {
            scheme: ClaimScheme::BatchSettlement,
            peer_id: None,
            channel_id,
            direction: ClaimDirection::Inbound,
            // A voucher has no nonce (ADR 0075 decision 8).
            nonce: connector_domain::VOUCHER_WATERMARK_NONCE,
            cumulative_amount: watermark.cumulative_amount,
            pending: false,
            book: ClaimBookKind::Client,
        })
        .collect();
    if let Some(batch) = &state.batch {
        views.extend(batch.outbound().claims());
    }
    Json(views)
}

/// `GET /rates`: every pair this node has declared, live, stale or
/// refused, as of one reading of the clock (issue #1297, ADR 0071).
///
/// A read, and only a read. Rates are declared in the config file and
/// observed by the background poller; ADR 0071 decision 3 keeps the
/// declaration immutable for the process lifetime (ADR 0009) and the
/// record rejected the "set from outside" shape that would put a rate on
/// this surface, so there is no `POST /rates` to pair with this and none
/// should be added without amending the record.
///
/// Empty for a node that deals nothing, which is most of them.
async fn rates(State(state): State<OperatorState>) -> Json<Vec<RateView>> {
    Json(match &state.rates {
        // The wall clock rather than a `Clock` port: this is a page being
        // refreshed by a human, not a packet being priced, and the
        // production clock is `Utc::now` anyway. Read once and handed to
        // one snapshot, so every row on the page answers for the same
        // instant.
        Some(declared) => declared.views(chrono::Utc::now()),
        None => Vec::new(),
    })
}

async fn audit_log(State(state): State<OperatorState>) -> Json<Vec<AuditRecord>> {
    Json(state.write_auth.audit_log())
}

/// `GET /metrics`: the decided metrics surface (ADR 0014) -- packets,
/// rejects, fees, exposure and settlement -- in Prometheus text exposition
/// format. A read like any other on this surface: gated by the bearer
/// token and nothing else, per ADR 0008.
async fn metrics(State(state): State<OperatorState>) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.connector.metrics().encode(),
    )
        .into_response()
}

/// `POST /packets`: an operator originates a packet outward, exactly as
/// the client edge does for an external caller -- decode a [`Prepare`],
/// call [`Connector::handle_prepare`] once, encode the outcome. The one
/// difference is what happens first: [`authenticate_write`] must accept
/// the request's RFC 9421 signature before any of that runs.
async fn originate_packet(
    State(state): State<OperatorState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_write_auth(&state, &method, &uri, &headers, &body) {
        return error.into_response();
    }

    let prepare = match Prepare::decode(&body) {
        Ok(prepare) => prepare,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };

    // An operator-originated packet is handed to the connector exactly as
    // a client's is. It declares no floor of its own: the
    // `minimum_delivery = prepare.amount` convention that used to live
    // here was a third convention no record ever carried, and it made
    // `amount - fee >= minimum_delivery` unsatisfiable for any non-zero
    // fee, so a fee-charging peering could never carry an operator's
    // packet at all (ADR 0057, issue #1143). What bounds erosion now is
    // the claim covering each crossing.
    // The cost never rides the OER encoding of a REJECT (ADR 0011), so it
    // goes beside it on every REJECT, zero included: "absent" never has to
    // carry meaning. The client edge's own answer does exactly that.
    connector_client_edge::packet_response(state.connector.handle_prepare(prepare).await)
}

/// A `POST /routes/leased` request body: create or renew a leased route
/// (ADR 0006, issue #427) forwarding `prefix` to peer `peer_id` for
/// `ttl_seconds` from this node's own clock. Posting the same `prefix`
/// again before it lapses renews it to a fresh `ttl_seconds` from whenever
/// the renewal is received -- that is the only way a leased route stays
/// alive, since nothing in the runtime extends one on its own.
///
/// Carries no `fee`: what this hop retains for carrying a packet to
/// `peer_id` is that peering's own fee, written on the `[[peers]]` row or
/// posted to `POST /peers` (ADR 0061). A controller that leased a route at
/// its own fee was setting a peering's terms through a route, which is
/// exactly what that record moved.
#[derive(Debug, Deserialize)]
struct CreateLeasedRouteRequest {
    prefix: String,
    peer_id: String,
    ttl_seconds: i64,
}

/// `POST /routes/leased`: a controller outside this connector pushes a
/// route to a peer with a time limit. Authenticated exactly like
/// `POST /packets` -- [`authenticate_write`] first, nothing else in this
/// handler accepts the request until that succeeds.
async fn create_leased_route(
    State(state): State<OperatorState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_write_auth(&state, &method, &uri, &headers, &body) {
        return error.into_response();
    }

    let request: CreateLeasedRouteRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };

    match state.connector.upsert_leased_route(
        request.prefix,
        request.peer_id,
        chrono::Duration::seconds(request.ttl_seconds),
    ) {
        Ok(view) => Json(view).into_response(),
        Err(LeaseRouteError::InvalidPrefix(prefix)) => (
            StatusCode::BAD_REQUEST,
            format!("invalid ILP address: '{prefix}'"),
        )
            .into_response(),
    }
}

/// `GET /routes/peers`: every peer-forwarding route this node knows (issue
/// #884) -- config-file and runtime alike, each tagged with its
/// [`connector_runtime::RouteSource`]. Deliberately distinct from
/// `GET /routes/leased`: a lease carries no price and does not survive a
/// restart, so it is not part of this table.
async fn peer_routes(State(state): State<OperatorState>) -> Json<Vec<PeerRouteView>> {
    Json(state.connector.peer_routes_view())
}

/// Map a [`PeerRouteTableError`] to the response `POST`/`DELETE`
/// `/peers*` and `/routes/peers*` answer with. `OwnedByConfig` and
/// `PeerInUse` are `409 Conflict` -- the request is refused because of the
/// table's *current state*, not because the request itself is malformed.
/// `UnknownPeerId`/`InvalidPrefix`/`InvalidPeerId` are `400 Bad Request` --
/// the request names something that cannot resolve to a valid row no
/// matter the table's state. `PeerNotFound`/`RouteNotFound` are `404`.
/// `Persistence` is `500` -- the durable write itself failed, so the
/// mutation was refused rather than applied in memory only.
fn peer_route_table_error_response(error: PeerRouteTableError) -> Response {
    match error {
        PeerRouteTableError::OwnedByConfig(_) | PeerRouteTableError::PeerInUse(_) => {
            (StatusCode::CONFLICT, error.to_string()).into_response()
        }
        // The last two are ADR 0058's runtime twins: a peering with no
        // channel bound to it, and a route forwarding to a peering with no
        // channel to pay from. `400`, beside `UnknownPeerId` -- the
        // request names something that cannot resolve to a valid row
        // whatever the table's current state is, which is the line this
        // function already draws.
        PeerRouteTableError::UnknownPeerId { .. }
        | PeerRouteTableError::InvalidPrefix(_)
        | PeerRouteTableError::InvalidPeerId
        | PeerRouteTableError::PeerChannelUnbound(_)
        | PeerRouteTableError::PeerHasNoPayChannel { .. } => {
            (StatusCode::BAD_REQUEST, error.to_string()).into_response()
        }
        PeerRouteTableError::PeerNotFound(_) | PeerRouteTableError::RouteNotFound(_) => {
            (StatusCode::NOT_FOUND, error.to_string()).into_response()
        }
        PeerRouteTableError::Persistence(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
        }
    }
}

/// A `POST /peers` request body: **establish a peering** (ADR 0058).
///
/// ```json
/// { "id": "apex-relay-2",
///   "url": "https://relay.example/ilp",
///   "fee": 100,
///   "max_packet_amount": 5000,
///   "deposit": 100000 }
/// ```
///
/// * `url` is the counterparty's connector URL. The node `GET`s the
///   self-description there (ADR 0050) and takes from it the endpoint, the
///   carriage that endpoint's scheme implies, the edge identity, and the
///   per-chain settlement addresses and chain facts. **Whatever that URL
///   serves is who the peering is with:** the fetched identity is not
///   checked against anything in this request, and ADR 0058 considered
///   requiring such a check and rejected it. The operator's vetting of the
///   URL is the whole of the assurance.
/// * `id` is the operator's own **local label** for the peering. Never
///   derived from the peer's ILP address -- that is self-asserted, a claim
///   and not a grant -- nor from the URL host. Refused (`409`) when the
///   config file already defines it (ADR 0034).
/// * `fee` is this peering's flat per-packet fee (ADR 0010, ADR 0061):
///   what this connector retains for carrying one packet to `id`,
///   whichever prefix the packet was addressed to. Omitted is zero -- free
///   carriage -- and a later post of the same `id` with a different `fee`
///   reprices the peering, since the packet path reads the fee off the
///   peering on every forward rather than off a copy baked into each
///   route.
/// * `max_packet_amount` is ADR 0049's **cap**: the largest amount this
///   connector will forward to `id` in one packet, refused `T04` above it.
///   Omitted -- or zero -- keeps `DEFAULT_MAX_PACKET_AMOUNT`; no value
///   here removes the bound.
/// * `chain` disambiguates the one case with no honest default: two nodes
///   settling on more than one chain in common. Left out, a single shared
///   chain is used and several are refused by name rather than resolved
///   silently, the same posture `POST /channels` takes.
/// * `deposit` is what the peering's outbound x402 channel is opened with,
///   in base units of the shared token (ADR 0075 decision 4), on either
///   chain: this node opens and funds its own channel toward the
///   counterparty and nothing else -- on Solana through the counterparty's
///   sponsor endpoint, so the counterparty holds the `payee` seat. Required
///   only when this node has no open channel toward it yet -- a repeat finds
///   that channel and spends nothing; top it up with
///   `POST /channels/:id/fund`.
///
/// `fee` and `max_packet_amount` are the operator's policy about this
/// counterparty, and are in this request precisely because no document can
/// supply them (ADR 0006).
#[derive(Debug, Deserialize)]
struct UpsertPeerRequest {
    id: String,
    url: String,
    #[serde(default)]
    fee: u64,
    #[serde(default)]
    max_packet_amount: u64,
    #[serde(default)]
    chain: Option<String>,
    #[serde(default)]
    deposit: Option<u128>,
}

/// Appended to the `502` when the URL that failed to answer a
/// self-description does not end in `/ilp` -- the single near-miss ADR 0050
/// has a name for: an origin, where this endpoint takes the connector's own
/// self-description URL. Named rather than guessed at by following a
/// redirect, because this connector reads a peer URL literally and
/// deliberately (see `connector_runtime`'s `self_description` header).
const ORIGIN_INSTEAD_OF_SELF_DESCRIPTION_URL: &str =
    " -- POST /peers takes a connector's self-description URL (ADR 0050), e.g. .../ilp, not an \
     origin; did you mean this URL with /ilp appended?";

/// Map an [`EstablishPeeringError`] to the response `POST /peers` answers
/// with.
///
/// The distinction that matters: `502 Bad Gateway` for everything the
/// **counterparty's host** did -- unreachable, redirecting, oversized,
/// malformed, or describing a node this one cannot peer with -- and `400`
/// for what this request itself got wrong. An operator reading a `502`
/// knows to go and look at the URL they named; a `400` is theirs to fix
/// here.
fn establish_peering_error_response(error: EstablishPeeringError) -> Response {
    match error {
        EstablishPeeringError::SelfDescription(SelfDescriptionError::Status {
            ref url, ..
        }) if !url.trim_end_matches('/').ends_with("/ilp") => {
            let message = format!("{error}{ORIGIN_INSTEAD_OF_SELF_DESCRIPTION_URL}");
            (StatusCode::BAD_GATEWAY, message).into_response()
        }
        EstablishPeeringError::SelfDescription(_)
        | EstablishPeeringError::NoDialableEndpoint { .. }
        | EstablishPeeringError::NoDialableClientEdge { .. }
        | EstablishPeeringError::NoSharedChain { .. }
        | EstablishPeeringError::UnreadableSettlementAddress { .. }
        | EstablishPeeringError::NetworkMismatch { .. }
        | EstablishPeeringError::NoVoucherSigner { .. }
        | EstablishPeeringError::InvalidTerms { .. } => {
            (StatusCode::BAD_GATEWAY, error.to_string()).into_response()
        }
        EstablishPeeringError::AmbiguousChain { .. }
        | EstablishPeeringError::DepositRequired { .. } => {
            (StatusCode::BAD_REQUEST, error.to_string()).into_response()
        }
        EstablishPeeringError::Binding(_) => {
            (StatusCode::CONFLICT, error.to_string()).into_response()
        }
        EstablishPeeringError::Outbound(error) => batch_channel_error_response(error),
        EstablishPeeringError::Table(error) => peer_route_table_error_response(error),
    }
}

/// `POST /peers`: ADR 0058's one operator write. Authenticated exactly
/// like every other write on this surface -- [`authenticate_write`] first,
/// nothing else in this handler accepts the request until that succeeds.
/// No bearer token reaches it: establishing a peering moves value.
///
/// **This endpoint can spend gas.** It may open and fund this node's own
/// outbound channel and wait for it to confirm, so it is deliberately safe
/// to retry: repeating the same request against a peering already
/// established finds this node's open channel toward the counterparty and
/// is a success, not a second channel (ADR 0075 decision 4; an open
/// journaled but unconfirmed is resumed). The answer says which branch it
/// took --
/// `channel: { id, status: "found" | "created" }` -- so an unintended
/// second channel is visible in the operator's own output rather than
/// discovered later on a block explorer.
async fn upsert_peer(
    State(state): State<OperatorState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_write_auth(&state, &method, &uri, &headers, &body) {
        return error.into_response();
    }

    let request: UpsertPeerRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };
    let url = match Url::parse(&request.url) {
        Ok(url) => url,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("'{}' is not a URL: {error}", request.url),
            )
                .into_response()
        }
    };
    let chain = match request.chain.as_deref().map(str::parse::<SettlementChain>) {
        None => None,
        Some(Ok(chain)) => Some(chain),
        Some(Err(error)) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };

    match state
        .connector
        .establish_peering(
            request.id,
            &url,
            request.fee,
            request.max_packet_amount,
            chain,
            request.deposit,
        )
        .await
    {
        Ok(established) => Json(established).into_response(),
        Err(error) => establish_peering_error_response(error),
    }
}

/// `DELETE /peers/:id`: remove a runtime peer row (issue #884). No request
/// body; authenticated over the path and method exactly like every other
/// write, since `authenticate_write` binds the signature to the whole
/// request rather than to a body a `DELETE` need not carry.
async fn remove_peer(
    State(state): State<OperatorState>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_write_auth(&state, &method, &uri, &headers, &body) {
        return error.into_response();
    }

    match state.connector.remove_runtime_peer(&id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => peer_route_table_error_response(error),
    }
}

/// A `POST /routes/peers` request body: add or update a runtime
/// peer-forwarding route (issue #884), keyed by `prefix` exactly like
/// `POST /routes/leased` -- posting the same prefix again updates the row
/// rather than adding a duplicate.
///
/// `price` is what this node's client edge charges a client for a packet to
/// `prefix` (ADR 0028). What this hop retains of it is `peer_id`'s own fee,
/// written on the peering by `POST /peers` and never here (ADR 0061).
///
/// It takes the config file's own spelling (ADR 0065): a bare integer for a
/// flat price, `{ "base": .., "per_kib": .. }` for one with a slope. A body
/// written before schedules existed carries the former and still means what
/// it meant.
#[derive(Debug, Deserialize)]
struct UpsertPeerRouteRequest {
    prefix: String,
    peer_id: String,
    price: Price,
}

/// `POST /routes/peers`: issue #884's runtime peer-route write.
/// Authenticated exactly like every other write on this surface.
async fn upsert_peer_route(
    State(state): State<OperatorState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_write_auth(&state, &method, &uri, &headers, &body) {
        return error.into_response();
    }

    let request: UpsertPeerRouteRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };

    match state
        .connector
        .upsert_runtime_peer_route(request.prefix, request.peer_id, request.price)
    {
        Ok(view) => Json(view).into_response(),
        Err(error) => peer_route_table_error_response(error),
    }
}

/// `DELETE /routes/peers/:prefix`: remove a runtime peer-forwarding route
/// (issue #884). No request body, authenticated exactly like
/// `DELETE /peers/:id`.
async fn remove_peer_route(
    State(state): State<OperatorState>,
    Path(prefix): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_write_auth(&state, &method, &uri, &headers, &body) {
        return error.into_response();
    }

    match state.connector.remove_runtime_peer_route(&prefix) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => peer_route_table_error_response(error),
    }
}

/// A `POST /channels` body that opens an **outbound x402 channel** (ADR 0075
/// decision 11): `terms` is the counterparty's `batchSettlements` entry for
/// one chain, exactly as its self-description publishes it; `deposit` is
/// the opening deposit in the token's base units; `url` is the
/// counterparty's URL, which a Solana `sponsorEndpoint` published as a path
/// resolves against.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenOutboundChannelRequest {
    terms: X402BatchSettlementTerms,
    deposit: u128,
    #[serde(default)]
    url: Option<String>,
}

/// A `POST /channels/:id/fund` request body: `{"amount": n}` deposits `n`
/// more of this node's own collateral into one of its outbound x402
/// channels. An increment, so a retry after an ambiguous outcome deposits
/// again -- the only form an x402 channel takes (ADR 0075 decision 11).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FundChannelRequest {
    amount: u128,
}

/// The refusal a `POST /channels` body without `terms` gets: the
/// `toon-channel` open it would have been, on either chain, is retired
/// (ADR 0075, #1376, #1383).
const TOON_CHANNEL_OPEN_RETIRED: &str =
    "toon-channel channels are no longer opened here, on either chain (ADR 0075): open an \
     outbound x402 batch-settlement channel by posting the counterparty's `terms` (and a \
     `deposit`), or peer with it through POST /peers, which opens and funds one itself";

/// The refusal a top-up of a channel that is not one of this node's
/// outbound x402 channels gets -- a `toon-channel` among them, whose
/// funding is retired (ADR 0075, #1378, #1383).
const NOT_AN_OUTBOUND_X402_CHANNEL: &str =
    "not an outbound x402 channel of this node: only those are topped up here, and \
     toon-channel funding, on either chain, is retired (ADR 0075) -- a peering pays over the \
     outbound x402 channel POST /peers opens and funds, so top that one up instead";

/// The status a failed x402 channel operation answers with.
///
/// - `503`: this node has no x402 backend on that chain -- its own config.
/// - `404`: not a channel of this node's, or none held to land.
/// - `409`: the channel's state refuses the step now, and a later retry may
///   not: still opening, a withdrawal not yet due or none to finish, a
///   voucher already landed, a sealed channel, a lapsed open.
/// - `502`: the counterparty or the chain -- a sponsor that refused the
///   open, an RPC that failed.
/// - `500`: the journal could not be written, so nothing was sent.
/// - `400`: everything the request itself got wrong.
fn batch_channel_error_response(error: BatchChannelError) -> Response {
    let status = match &error {
        BatchChannelError::NoBackend(_) => StatusCode::SERVICE_UNAVAILABLE,
        BatchChannelError::UnknownChannel(_) | BatchChannelError::NoVoucherHeld(_) => {
            StatusCode::NOT_FOUND
        }
        BatchChannelError::StillOpening(_) => StatusCode::CONFLICT,
        BatchChannelError::InvalidTerms(_) => StatusCode::BAD_REQUEST,
        BatchChannelError::Journal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        BatchChannelError::Settlement(settlement) => match settlement {
            BatchSettlementError::WithdrawalNotDue { .. }
            | BatchSettlementError::NoWithdrawalPending(_)
            | BatchSettlementError::StaleVoucher { .. }
            | BatchSettlementError::ChannelSealed(_)
            | BatchSettlementError::OpenLapsed(_) => StatusCode::CONFLICT,
            BatchSettlementError::OpenRefused(_) | BatchSettlementError::Backend(_) => {
                StatusCode::BAD_GATEWAY
            }
            BatchSettlementError::NotOutbound(_) | BatchSettlementError::ChannelNotFound(_) => {
                StatusCode::NOT_FOUND
            }
            _ => StatusCode::BAD_REQUEST,
        },
    };
    (status, error.to_string()).into_response()
}

/// The `503` a node without an x402 backend answers every x402 channel
/// write with.
fn no_batch_backend() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "this node has no x402 batch-settlement backend configured",
    )
        .into_response()
}

/// `POST /channels`: open an outbound x402 channel toward a counterparty
/// (ADR 0075 decision 11). The body carries the counterparty's `terms`; a
/// body without them is refused by name, since the `toon-channel` open it
/// would once have been is retired on both chains (#1376, #1383).
/// Authenticated exactly like every other write on this surface --
/// [`authenticate_write`] first, nothing else in this handler accepts the
/// request until that succeeds.
///
/// **This endpoint spends.** It is safe to retry: the channel is journaled
/// before its opening transaction is sent, and a retry toward the same
/// receiver while that open is unconfirmed resumes it rather than opening a
/// second (ADR 0075 decision 8). The answer says which it did --
/// `resumed: true` -- so an unintended second channel is visible in the
/// operator's own output.
async fn open_channel(
    State(state): State<OperatorState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_write_auth(&state, &method, &uri, &headers, &body) {
        return error.into_response();
    }

    let fields: serde_json::Map<String, serde_json::Value> = match serde_json::from_slice(&body) {
        Ok(fields) => fields,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };
    if !fields.contains_key("terms") {
        return (StatusCode::BAD_REQUEST, TOON_CHANNEL_OPEN_RETIRED).into_response();
    }
    open_outbound_channel(&state, &body).await
}

/// The answer to an x402 `POST /channels`: the channel, and whether this
/// request resumed an open journaled earlier rather than building one.
#[derive(Debug, Serialize)]
struct OpenedOutboundChannel {
    #[serde(flatten)]
    channel: BatchChannelView,
    resumed: bool,
}

async fn open_outbound_channel(state: &OperatorState, body: &Bytes) -> Response {
    let Some(batch) = state.batch.as_ref() else {
        return no_batch_backend();
    };
    let request: OpenOutboundChannelRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };
    let url = match request.url.as_deref().map(Url::parse) {
        None => None,
        Some(Ok(url)) => Some(url),
        Some(Err(error)) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("`url` is not a URL: {error}"),
            )
                .into_response()
        }
    };
    match batch
        .open(&request.terms, url.as_ref(), request.deposit)
        .await
    {
        Ok((channel, resumed)) => Json(OpenedOutboundChannel { channel, resumed }).into_response(),
        Err(error) => batch_channel_error_response(error),
    }
}

/// `POST /channels/:id/fund`: top up one of this node's outbound x402
/// channels by `{"amount": n}`, an increment (ADR 0075 decision 11). Any
/// other channel -- a `toon-channel` included, on either chain -- is
/// refused by name: `toon-channel` funding is retired (#1378, #1383), and a
/// peering pays over the outbound x402 channel `POST /peers` opens and
/// funds.
async fn fund_channel(
    State(state): State<OperatorState>,
    Path(channel_id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_write_auth(&state, &method, &uri, &headers, &body) {
        return error.into_response();
    }
    // `total` is named before the body is parsed: it was toon-channel funding
    // to an absolute figure, retired by ADR 0075, and a body carrying it is
    // refused as that rather than as an unknown field.
    let names_total = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&body)
        .is_ok_and(|fields| fields.contains_key("total"));
    if names_total {
        return (
            StatusCode::BAD_REQUEST,
            "`total` was toon-channel funding to an absolute figure, retired by ADR 0075: an \
             x402 channel is topped up by an increment -- give exactly `amount`",
        )
            .into_response();
    }
    let request: FundChannelRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("{error}: an x402 channel is topped up by giving exactly `amount`"),
            )
                .into_response()
        }
    };
    let Some(batch) = state.batch.as_ref() else {
        return no_batch_backend();
    };
    if !batch.outbound().knows(&channel_id) {
        return (StatusCode::BAD_REQUEST, NOT_AN_OUTBOUND_X402_CHANNEL).into_response();
    }
    match batch.outbound().top_up(&channel_id, request.amount).await {
        Ok(state) => Json(batch.outbound().view_of(&state)).into_response(),
        Err(error) => batch_channel_error_response(error),
    }
}

/// The answer to `POST /channels/:id/withdraw`: which step it took, and
/// the channel after it.
#[derive(Debug, Serialize)]
struct WithdrawnChannel {
    step: WithdrawStep,
    #[serde(flatten)]
    channel: BatchChannelView,
}

/// `POST /channels/:id/withdraw`: wind down an outbound x402 channel (ADR
/// 0075 decision 11). One lever with two steps, which the chain decides:
/// on an open channel it **starts** -- EVM `initiateWithdraw`, Solana
/// `request_close` -- and once that is due it **finishes** -- EVM
/// `finalizeWithdraw`, Solana `distribute`. Between them the receiver has
/// the channel's delay in which to land its latest voucher, which is what
/// protects it. Called too early, it answers `409` with the seconds left.
/// No request body.
///
/// Replaces the `toon-channel` `close` and `settle` writes, deleted here.
async fn withdraw_channel(
    State(state): State<OperatorState>,
    Path(channel_id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_write_auth(&state, &method, &uri, &headers, &body) {
        return error.into_response();
    }
    let Some(batch) = state.batch.as_ref() else {
        return no_batch_backend();
    };
    match batch.outbound().withdraw(&channel_id).await {
        Ok((step, state)) => Json(WithdrawnChannel {
            step,
            channel: batch.outbound().view_of(&state),
        })
        .into_response(),
        Err(error) => batch_channel_error_response(error),
    }
}

/// `POST /channels/:id/land`: land the latest voucher this node holds on an
/// inbound x402 channel now (ADR 0075 decision 11) -- EVM `claim`, Solana
/// `settle`, or `settle_and_seal` on a channel its payer is closing. The
/// watchers and sweeps land vouchers on their own; this is the manual lever
/// for planned maintenance. No request body: the voucher landed is the one
/// the client edge accepted and journaled, never one the caller supplies.
///
/// Replaces the `toon-channel` `redeem`, `redeem-latest` and
/// `cooperative-close` writes, deleted here. Landing the same voucher twice
/// answers `409` by name.
async fn land_channel(
    State(state): State<OperatorState>,
    Path(channel_id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(error) = require_write_auth(&state, &method, &uri, &headers, &body) {
        return error.into_response();
    }
    let Some(batch) = state.batch.as_ref() else {
        return no_batch_backend();
    };
    match batch.land(&channel_id).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => batch_channel_error_response(error),
    }
}

async fn identity(State(state): State<OperatorState>) -> Response {
    match node_identity(state.signer.as_ref()) {
        Ok(identity) => Json(identity).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

fn node_identity(signer: &dyn Signer) -> Result<NodeIdentity, SignerError> {
    let public_key = signer.public_key()?;
    Ok(NodeIdentity {
        key_id: signer.key_id(),
        address: to_hex(&derive_evm_address(&public_key)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use connector_config::StaticRoute;
    use connector_runtime::{
        FakeAppClient, InMemoryJournal, InProcessPeerTransport, RouteSource, TestClock,
    };
    use connector_signer::LocalSigner;
    use tower::ServiceExt;

    /// The `[[pay_channels]]` half of a peering, which ADR 0042 requires of
    /// every peering a node forwards to and issue #1145 made unavoidable: a
    /// forward this node cannot cover is refused `T00` before the transport
    /// is reached at all. Since ADR 0075 the covering is a voucher on this
    /// node's own outbound x402 channel.
    fn covering(connector: Connector, peer_id: &str) -> Connector {
        connector_runtime::covering_fake::covering(connector, peer_id)
    }

    /// A client-edge claim book with no settlement backend, journaling
    /// nowhere durable -- the operator-surface tests below that are not
    /// specifically about the client-edge book (most of them) need a
    /// `ClientClaimGate` to satisfy `router`'s signature and nothing more.
    fn empty_claim_gate() -> Arc<ClientClaimGate> {
        Arc::new(
            ClientClaimGate::restore(Arc::new(InMemoryJournal::new()))
                .expect("a fresh in-memory journal has nothing to replay"),
        )
    }

    fn test_router(routes: Vec<StaticRoute>, bearer_token: &str) -> Router {
        let app_client = Arc::new(FakeAppClient::new());
        let clock = Arc::new(TestClock::new(
            chrono::Utc::now(), // only used to satisfy the Connector constructor; unread here
        ));
        let connector = Arc::new(Connector::new(
            routes,
            vec![],
            app_client,
            Arc::new(InProcessPeerTransport::new()),
            clock,
        ));
        let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("operator-test-key"));
        router(
            connector,
            empty_claim_gate(),
            signer,
            bearer_token.to_string(),
            vec![],
            None,
        )
    }

    async fn get(app: Router, path: &str, bearer_token: Option<&str>) -> Response {
        let mut builder = Request::builder().method("GET").uri(path);
        if let Some(token) = bearer_token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = builder.body(Body::empty()).unwrap();
        app.oneshot(request).await.unwrap()
    }

    #[tokio::test]
    async fn a_request_with_no_bearer_token_is_rejected() {
        let app = test_router(vec![], "correct-token");
        let response = get(app, "/routes", None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_request_with_the_wrong_bearer_token_is_rejected() {
        let app = test_router(vec![], "correct-token");
        let response = get(app, "/routes", Some("wrong-token")).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// ADR 0066: the dashboard page is served without a bearer token,
    /// because it is inert -- markup that holds nothing, fetches every
    /// figure through the gated reads and signs every write in the
    /// browser. Serving it cannot leak what it does not contain, and the
    /// CSP pins whatever it is fed to this origin.
    #[tokio::test]
    async fn the_dashboard_page_needs_no_token_and_talks_only_to_its_own_origin() {
        let app = test_router(vec![], "correct-token");
        let response = get(app, "/dashboard", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        let csp = response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .expect("the dashboard carries a CSP")
            .to_str()
            .unwrap();
        assert!(csp.contains("default-src 'none'"), "{csp}");
        assert!(csp.contains("connect-src 'self'"), "{csp}");
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        assert!(
            !DASHBOARD_HTML.contains("correct-token"),
            "the page is static and carries no credential"
        );
    }

    /// The page is a client of this router and is held to it here: every
    /// read this router mounts is one the page fetches, every write the
    /// page makes is one this router mounts, and the signature base it
    /// builds in the browser spells the covered components and algorithm
    /// exactly as [`rfc9421`] verifies them. A path or parameter renamed
    /// on one side without the other fails this test rather than 401ing
    /// or 404ing in an operator's browser.
    #[test]
    fn the_dashboard_reads_the_whole_read_surface_and_signs_as_the_verifier_expects() {
        for path in [
            "/metrics",
            "/identity",
            "/peers",
            "/channels",
            "/claims",
            "/rates",
            "/routes",
            "/routes/peers",
            "/routes/leased",
            "/audit-log",
        ] {
            assert!(
                DASHBOARD_HTML.contains(&format!("read('{path}'")),
                "the dashboard does not read {path}"
            );
        }
        for write in [
            "write('POST', '/peers'",
            "write('POST', '/routes/peers'",
            "write('POST', '/routes/leased'",
            "write('DELETE', `/peers/${",
            "write('DELETE', `/routes/peers/${",
            // ADR 0075 decision 11: the x402 channel writes.
            "write('POST', '/channels'",
            "channelPath(c, 'fund')",
            "channelPath(c, 'withdraw')",
            "channelPath(c, 'land')",
            "`/channels/${encodeURIComponent(c.id)}/${step}`",
        ] {
            assert!(
                DASHBOARD_HTML.contains(write),
                "the dashboard lacks {write}"
            );
        }

        let components = rfc9421::COVERED_COMPONENTS
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            DASHBOARD_HTML.contains(&format!("({components});created=")),
            "the browser signs over a different component set than the node verifies"
        );
        assert!(DASHBOARD_HTML.contains(&format!("alg=\"{}\"", rfc9421::SIGNATURE_ALG)));
        assert!(DASHBOARD_HTML.contains("'Content-Digest': digest"));
        assert!(
            !DASHBOARD_HTML.contains("innerHTML"),
            "API data is rendered as text, never as markup"
        );
    }

    #[tokio::test]
    async fn routes_reports_the_connectors_configured_static_routes() {
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
        let app = test_router(vec![route], "correct-token");

        let response = get(app, "/routes", Some("correct-token")).await;
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let routes: Vec<RouteView> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].prefix, "g.example.app");
        assert_eq!(routes[0].handler_url, "http://localhost:4000/");
        assert_eq!(routes[0].price, Price::flat(25));
    }

    #[tokio::test]
    async fn peers_channels_claims_and_audit_log_read_as_empty_lists() {
        let app = test_router(vec![], "correct-token");

        for path in ["/peers", "/channels", "/claims", "/audit-log"] {
            let response = get(app.clone(), path, Some("correct-token")).await;
            assert_eq!(response.status(), StatusCode::OK, "path {path}");
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body, serde_json::json!([]), "path {path}");
        }
    }

    #[tokio::test]
    async fn metrics_reports_prometheus_text_and_requires_the_bearer_token() {
        let app = test_router(vec![], "correct-token");

        let unauthenticated = get(app.clone(), "/metrics", None).await;
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let response = get(app, "/metrics", Some("correct-token")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("toon_fees_earned_total"));
    }

    #[tokio::test]
    async fn identity_reports_the_signers_key_id_and_derived_address() {
        let app_client = Arc::new(FakeAppClient::new());
        let clock = Arc::new(TestClock::new(chrono::Utc::now()));
        let connector = Arc::new(Connector::new(
            vec![],
            vec![],
            app_client,
            Arc::new(InProcessPeerTransport::new()),
            clock,
        ));
        let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("operator-test-key"));
        let expected = node_identity(signer.as_ref()).unwrap();
        let app = router(
            connector,
            empty_claim_gate(),
            signer,
            "correct-token".to_string(),
            vec![],
            None,
        );

        let response = get(app, "/identity", Some("correct-token")).await;
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let identity: NodeIdentity = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(identity, expected);
    }

    #[tokio::test]
    async fn there_is_no_write_endpoint_to_change_state_through() {
        let app = test_router(vec![], "correct-token");

        let request = Request::builder()
            .method("POST")
            .uri("/routes")
            .header(header::AUTHORIZATION, "Bearer correct-token")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    /// ADR 0071's operator read (issue #1297): which declared pairs are
    /// live, stale or refused, so an operator can tell a dead poller from
    /// a quiet market without reading logs.
    ///
    /// Driven as an external caller through the production `router()`, and
    /// against the same `SharedRateTable` a forwarding path would hold --
    /// not a projection of one, which is the whole point: the page reports
    /// what packets actually see.
    mod declared_rates {
        use super::*;
        use chrono::TimeDelta;
        use connector_domain::{AssetId, Guards, MaxMove, Rate, RateTable, Spread, Ttl};
        use connector_runtime::{RateViewState, SharedRateTable};

        /// USDC on Base, the numeraire -- ADR 0071's own example.
        const USDC: &str = "evm:0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
        /// ANYONE on Base: the dealt token, 18 decimals against USDC's 6.
        const ANYONE: &str = "evm:0x9ff58f4ffb29fa2266ab25e75e2a8b3503311656";

        fn asset(text: &str) -> AssetId {
            text.parse::<AssetId>().expect("a declared asset")
        }

        fn rate(numerator: u64) -> Rate {
            Rate::new(numerator, 1).expect("a rate")
        }

        /// A two-minute `ttl`, so "just observed" and "observed ten
        /// minutes ago" land either side of it against the wall clock the
        /// handler reads.
        fn dealing_table() -> SharedRateTable {
            SharedRateTable::new(RateTable::new(
                asset(USDC),
                Guards::new(
                    Spread::none(),
                    Ttl::new(TimeDelta::seconds(120)).expect("a positive ttl"),
                    MaxMove::fraction(10, 100).expect("a max_move"),
                ),
            ))
        }

        fn dealing_router(rates: DeclaredRates) -> Router {
            let clock = Arc::new(TestClock::new(chrono::Utc::now()));
            let connector = Arc::new(Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                clock,
            ));
            router(
                connector,
                empty_claim_gate(),
                Arc::new(LocalSigner::generate("operator-test-key")),
                "correct-token".to_string(),
                vec![],
                Some(rates),
            )
        }

        async fn read_rates(app: Router) -> Vec<RateView> {
            let response = get(app, "/rates", Some("correct-token")).await;
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            serde_json::from_slice(&bytes).expect("a list of declared pairs")
        }

        /// AC 5. A node that declares no tokens holds no table at all, and
        /// the answer to "what do you deal" is "nothing" -- an empty list.
        /// A `404` would leave an operator wondering whether the node was
        /// too old to have the endpoint.
        #[tokio::test]
        async fn a_node_that_deals_nothing_reports_an_empty_set() {
            let app = test_router(vec![], "correct-token");

            let response = get(app, "/rates", Some("correct-token")).await;

            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                serde_json::json!([])
            );
        }

        /// AC 4, first half: this is a read, so it is behind the read gate
        /// (ADR 0008's bearer token) and nothing else.
        #[tokio::test]
        async fn the_read_is_behind_the_bearer_token() {
            let app = dealing_router(DeclaredRates::new(dealing_table(), []));

            assert_eq!(
                get(app.clone(), "/rates", None).await.status(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                get(app, "/rates", Some("wrong-token")).await.status(),
                StatusCode::UNAUTHORIZED
            );
        }

        /// AC 4, second half: it adds no write path. ADR 0071 rejected the
        /// "set from outside" shape (ADR 0049's) in favour of self-sourcing,
        /// and a rate an operator could POST would be a declaration
        /// changing mid-process, which ADR 0009 forbids outright.
        #[tokio::test]
        async fn there_is_no_way_to_set_a_rate_through_this_surface() {
            let app = dealing_router(DeclaredRates::new(dealing_table(), []));

            let request = Request::builder()
                .method("POST")
                .uri("/rates")
                .header(header::AUTHORIZATION, "Bearer correct-token")
                .body(Body::empty())
                .unwrap();

            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        }

        /// ACs 1 and 2. Three pairs in one table, in the three states, read
        /// in one request: the live one carries the rate a forward would
        /// convert at and when it was last refreshed, and the other two
        /// carry neither, because neither has a rate to carry.
        #[tokio::test]
        async fn every_declared_pair_reports_its_state() {
            let table = dealing_table();
            let now = chrono::Utc::now();
            // Observed a moment ago: inside its ttl, so trading.
            table.write(|rates| rates.refresh(asset(ANYONE), asset(USDC), rate(4), now));
            // A second token, observed ten minutes ago: aged out, and the
            // outage is its own -- ANYONE above is untouched by it.
            let weth = asset("evm:0x4200000000000000000000000000000000000006");
            table.write(|rates| {
                rates.refresh(
                    weth.clone(),
                    asset(USDC),
                    rate(3000),
                    now - TimeDelta::seconds(600),
                )
            });
            // A third, declared as a quote path and never yet observed:
            // the poller that was meant to price it has landed nothing.
            let unpriced = asset("evm:0x0b3e328455c4059eeb9e3f84b5543f74e24e7e1b");
            let app = dealing_router(DeclaredRates::new(table, [(unpriced.clone(), asset(USDC))]));

            let views = read_rates(app).await;

            let find = |from: &str| {
                views
                    .iter()
                    .find(|view| view.from == from)
                    .unwrap_or_else(|| panic!("no row for {from} in {views:#?}"))
                    .clone()
            };
            let live = find(ANYONE);
            assert_eq!(live.state, RateViewState::Live);
            assert_eq!(live.rate, Some(rate(4)));
            assert_eq!(live.last_refreshed, Some(now));
            assert_eq!(live.to, USDC);

            let stale = find(&weth.to_string());
            assert_eq!(stale.state, RateViewState::Stale);
            assert_eq!(stale.rate, None);
            assert_eq!(
                stale.last_refreshed,
                Some(now - TimeDelta::seconds(600)),
                "a stale pair still says when it was last seen"
            );

            let refused = find(&unpriced.to_string());
            assert_eq!(refused.state, RateViewState::Refused);
            assert_eq!(refused.rate, None);
            assert_eq!(refused.last_refreshed, None);
        }

        /// AC 3. The pair `max_move` is holding a line on is still
        /// trading, and must not read as one that aged out -- which is why
        /// the refusal is a field beside the state rather than a state of
        /// its own. Both facts come out of the one lookup the forwarding
        /// path makes, so there is no second surface to disagree with it.
        #[tokio::test]
        async fn a_guarded_pair_is_not_a_pair_that_aged_out() {
            let table = dealing_table();
            let now = chrono::Utc::now();
            table.write(|rates| rates.refresh(asset(ANYONE), asset(USDC), rate(4), now));
            // Ten times the standing rate, against a ten-percent bound:
            // probable manipulation, refused, previous value left ageing.
            table.write(|rates| rates.refresh(asset(ANYONE), asset(USDC), rate(40), now));
            // A second pair that merely aged out, with no refusal on it.
            let weth = asset("evm:0x4200000000000000000000000000000000000006");
            table.write(|rates| {
                rates.refresh(
                    weth.clone(),
                    asset(USDC),
                    rate(3000),
                    now - TimeDelta::seconds(600),
                )
            });
            let app = dealing_router(DeclaredRates::new(table, []));

            let views = read_rates(app).await;

            let guarded = views
                .iter()
                .find(|view| view.from == ANYONE)
                .expect("the guarded pair");
            assert_eq!(guarded.state, RateViewState::Live);
            assert_eq!(guarded.rate, Some(rate(4)), "the previous value stands");
            let refused = guarded
                .refused_refresh
                .expect("a max_move refusal an operator is being woken for");
            assert_eq!(refused.offered, rate(40));

            let aged = views
                .iter()
                .find(|view| view.from == weth.to_string())
                .expect("the pair that aged out");
            assert_eq!(aged.state, RateViewState::Stale);
            assert_eq!(
                aged.refused_refresh, None,
                "nothing refused this one; it simply went quiet"
            );
        }
    }

    /// The write-authentication mechanism (issue #421), exercised end to
    /// end over real HTTP against the actual production `router()` and
    /// its one write endpoint, `POST /packets`. Every AC is driven as an
    /// external caller, matching #420's precedent: no port bound, no
    /// privileged in-process access, just requests through
    /// `tower::ServiceExt::oneshot`.
    mod write_authentication {
        use super::*;
        use crate::rfc9421::{keyid_hex, sign_request};
        use connector_domain::RejectCode;
        use ed25519_dalek::Keypair;
        use rand::rngs::OsRng;

        fn keypair() -> Keypair {
            Keypair::generate(&mut OsRng)
        }

        fn sample_prepare() -> Prepare {
            Prepare {
                amount: 0,
                expires_at: chrono::Utc::now() + chrono::Duration::minutes(1),
                greeting: false,
                destination: "g.example.nowhere".to_string(),
                data: b"originated by the operator".to_vec(),
            }
        }

        /// Sign an OER-encoded write body bound for `/packets`, returning
        /// the three headers a caller presents.
        fn sign(keypair: &Keypair, body: &[u8], expires: u64) -> (String, String, String) {
            sign_request(keypair, "POST", "/packets", body, 1_000, Some(expires))
        }

        fn packets_request(
            body: Vec<u8>,
            signature_input: Option<&str>,
            signature: Option<&str>,
            content_digest: Option<&str>,
            bearer_token: Option<&str>,
        ) -> Request<Body> {
            let mut builder = Request::builder().method("POST").uri("/packets");
            if let Some(v) = signature_input {
                builder = builder.header("signature-input", v);
            }
            if let Some(v) = signature {
                builder = builder.header("signature", v);
            }
            if let Some(v) = content_digest {
                builder = builder.header("content-digest", v);
            }
            if let Some(token) = bearer_token {
                builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
            }
            builder.body(Body::from(body)).unwrap()
        }

        fn router_with_write_keys(write_keys: Vec<[u8; 32]>) -> Router {
            let app_client = Arc::new(FakeAppClient::new());
            let clock = Arc::new(TestClock::new(chrono::Utc::now()));
            let connector = Arc::new(Connector::new(
                vec![],
                vec![],
                app_client,
                Arc::new(InProcessPeerTransport::new()),
                clock,
            ));
            let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("operator-test-key"));
            router(
                connector,
                empty_claim_gate(),
                signer,
                "correct-token".to_string(),
                write_keys,
                None,
            )
        }

        #[tokio::test]
        async fn a_write_with_no_signature_at_all_is_rejected() {
            let app = router_with_write_keys(vec![]);
            let body = sample_prepare().encode();

            let response = app
                .oneshot(packets_request(body, None, None, None, None))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn a_bearer_token_alone_does_not_authorize_a_write() {
            // Bearer tokens gate reads; they must never substitute for a
            // write's signature (ADR 0008).
            let app = router_with_write_keys(vec![]);
            let body = sample_prepare().encode();

            let response = app
                .oneshot(packets_request(
                    body,
                    None,
                    None,
                    None,
                    Some("correct-token"),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn a_validly_signed_write_from_an_allowlisted_key_originates_the_packet() {
            let keypair = keypair();
            let app = router_with_write_keys(vec![keypair.public.to_bytes()]);
            let body = sample_prepare().encode();
            let (sig_input, sig, digest) = sign(&keypair, &body, 9_999_999_999);

            let response = app
                .oneshot(packets_request(
                    body,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);

            // No route matches -- the packet was genuinely originated
            // into the connector's packet plane, not short-circuited.
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = connector_domain::Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code, RejectCode::f02_unreachable());
        }

        #[tokio::test]
        async fn a_signature_from_a_key_not_on_the_allowlist_is_rejected() {
            let signer = keypair();
            let app = router_with_write_keys(vec![]); // signer's key is not allowlisted
            let body = sample_prepare().encode();
            let (sig_input, sig, digest) = sign(&signer, &body, 9_999_999_999);

            let response = app
                .oneshot(packets_request(
                    body,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn removing_a_key_from_the_allowlist_revokes_it_with_no_other_change() {
            let keypair = keypair();
            let body = sample_prepare().encode();
            let (sig_input, sig, digest) = sign(&keypair, &body, 9_999_999_999);

            let allowed = router_with_write_keys(vec![keypair.public.to_bytes()]);
            let response = allowed
                .oneshot(packets_request(
                    body.clone(),
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);

            // Identical request, identical signature -- only the
            // configured allowlist changed.
            let revoked = router_with_write_keys(vec![]);
            let response = revoked
                .oneshot(packets_request(
                    body,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn an_expired_signature_is_rejected() {
            let keypair = keypair();
            let app = router_with_write_keys(vec![keypair.public.to_bytes()]);
            let body = sample_prepare().encode();
            // Already expired relative to any wall-clock "now".
            let (sig_input, sig, digest) = sign(&keypair, &body, 1);

            let response = app
                .oneshot(packets_request(
                    body,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn a_replayed_signature_is_rejected_the_second_time() {
            let keypair = keypair();
            let app = router_with_write_keys(vec![keypair.public.to_bytes()]);
            let body = sample_prepare().encode();
            let (sig_input, sig, digest) = sign(&keypair, &body, 9_999_999_999);

            let first = app
                .clone()
                .oneshot(packets_request(
                    body.clone(),
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(first.status(), StatusCode::OK);

            let replay = app
                .oneshot(packets_request(
                    body,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn a_captured_request_cannot_be_replayed_with_altered_contents() {
            let keypair = keypair();
            let app = router_with_write_keys(vec![keypair.public.to_bytes()]);
            let original = sample_prepare().encode();
            let (sig_input, sig, digest) = sign(&keypair, &original, 9_999_999_999);

            let mut tampered_prepare = sample_prepare();
            tampered_prepare.destination = "g.attacker.somewhere.else".to_string();
            let tampered = tampered_prepare.encode();

            let response = app
                .oneshot(packets_request(
                    tampered,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn every_accepted_write_is_retained_in_the_audit_log_and_read_back_over_the_operator_surface(
        ) {
            let keypair = keypair();
            let app = router_with_write_keys(vec![keypair.public.to_bytes()]);
            let body = sample_prepare().encode();
            let (sig_input, sig, digest) = sign(&keypair, &body, 9_999_999_999);

            let write_response = app
                .clone()
                .oneshot(packets_request(
                    body,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(write_response.status(), StatusCode::OK);

            let audit_response = get(app, "/audit-log", Some("correct-token")).await;
            assert_eq!(audit_response.status(), StatusCode::OK);

            let bytes = hyper::body::to_bytes(audit_response.into_body())
                .await
                .unwrap();
            let log: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(log.len(), 1);
            assert_eq!(log[0]["keyid"], keyid_hex(&keypair));
            assert_eq!(log[0]["path"], "/packets");
        }

        /// Issue #1460: a REJECT answered by `POST /packets` carries its
        /// accumulated cost in `TOON-Accumulated-Cost`, because the cost never
        /// rides the OER bytes (ADR 0011). One forwarding hop charging 7, relaying
        /// a REJECT its peer genuinely decided on, is the smallest path whose
        /// cost is not zero.
        #[tokio::test]
        async fn a_reject_relayed_through_a_paying_hop_carries_that_hops_fee() {
            use connector_runtime::PeerRoute;

            let keypair = keypair();
            let clock = Arc::new(TestClock::new(chrono::Utc::now()));
            let second_hop = Arc::new(Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                clock.clone(),
            ));
            let mut peer_transport = InProcessPeerTransport::new();
            peer_transport.add_peer("second-hop", second_hop);
            let connector = Arc::new(covering(
                Connector::new(
                    vec![],
                    vec![PeerRoute::new("g.example", "second-hop")],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(peer_transport),
                    clock,
                )
                .with_peer_fees([("second-hop".to_string(), 7)]),
                "second-hop",
            ));
            let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("operator-test-key"));
            let app = router(
                connector,
                empty_claim_gate(),
                signer,
                "correct-token".to_string(),
                vec![keypair.public.to_bytes()],
                None,
            );

            let mut prepare = sample_prepare();
            prepare.destination = "g.example.remote".to_string();
            prepare.amount = 100;
            let body = prepare.encode();
            let (sig_input, sig, digest) = sign(&keypair, &body, 9_999_999_999);

            let response = app
                .oneshot(packets_request(
                    body,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response
                    .headers()
                    .get(connector_btp::ACCUMULATED_COST_HEADER)
                    .map(|value| value.to_str().unwrap()),
                Some("7")
            );
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = connector_domain::Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code, RejectCode::f02_unreachable());
        }

        /// A FULFILL has no cost to report, so it carries no such header.
        #[tokio::test]
        async fn a_fulfill_answered_by_packets_carries_no_cost_header() {
            use connector_domain::{EnvelopeRequest, EnvelopeResponse};
            use connector_runtime::AppOutcome;
            use connector_signer::giftwrap::seal_request;

            let keypair = keypair();
            let route = connector_config::StaticRoute::new("g.example.app", "http://app.example/")
                .expect("a valid route");
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(
                route.handler_url(),
                AppOutcome::Answered {
                    response: EnvelopeResponse {
                        status: 200,
                        headers: vec![],
                        body: b"delivered".to_vec(),
                    },
                },
            );
            let identity = LocalSigner::generate("operator-test-identity");
            let identity_public_key = identity.public_key().expect("a public key");
            let connector = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client,
                    Arc::new(InProcessPeerTransport::new()),
                    Arc::new(TestClock::new(chrono::Utc::now())),
                )
                .with_identity_signer(Arc::new(identity)),
            );
            let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("operator-test-key"));
            let app = router(
                connector,
                empty_claim_gate(),
                signer,
                "correct-token".to_string(),
                vec![keypair.public.to_bytes()],
                None,
            );

            let plaintext = EnvelopeRequest {
                method: "POST".to_string(),
                target: "/".to_string(),
                headers: vec![],
                body: b"hello".to_vec(),
            }
            .encode();
            let (data, _shared_secret) =
                seal_request(&plaintext, &identity_public_key).expect("seal");
            let mut prepare = sample_prepare();
            prepare.destination = "g.example.app".to_string();
            prepare.data = data;
            let body = prepare.encode();
            let (sig_input, sig, digest) = sign(&keypair, &body, 9_999_999_999);

            let response = app
                .oneshot(packets_request(
                    body,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                response
                    .headers()
                    .get(connector_btp::ACCUMULATED_COST_HEADER)
                    .is_none(),
                "a FULFILL carries no cost header"
            );
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            connector_domain::Fulfill::decode(&bytes).expect("a FULFILL");
        }

        #[tokio::test]
        async fn a_read_route_still_requires_the_bearer_token_and_not_a_write_signature() {
            let app = router_with_write_keys(vec![]);
            let response = get(app, "/routes", None).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        /// ADR 0057, issue #1143: an originated packet declares no floor,
        /// so a fee-charging peering is one an operator's packet can
        /// actually cross. The old `minimum_delivery = prepare.amount`
        /// convention made `amount - fee >= minimum_delivery` unsatisfiable
        /// for any non-zero fee and refused this packet here, without ever
        /// reaching the peer. It now reaches the transport -- and the peer
        /// is simply not registered, which is `T01` and not a verdict on
        /// the amount.
        #[tokio::test]
        async fn an_originated_packet_crosses_a_fee_charging_peering() {
            use connector_runtime::PeerRoute;

            let keypair = keypair();
            let app_client = Arc::new(FakeAppClient::new());
            let clock = Arc::new(TestClock::new(chrono::Utc::now()));
            let connector = Arc::new(covering(
                Connector::new(
                    vec![],
                    vec![PeerRoute::new("g.example", "peer-1")],
                    app_client,
                    Arc::new(InProcessPeerTransport::new()),
                    clock,
                )
                .with_peer_fees([("peer-1".to_string(), 5)]),
                "peer-1",
            ));
            let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("operator-test-key"));
            let app = router(
                connector,
                empty_claim_gate(),
                signer,
                "correct-token".to_string(),
                vec![keypair.public.to_bytes()],
                None,
            );

            let mut prepare = sample_prepare();
            prepare.amount = 100;
            let body = prepare.encode();
            let (sig_input, sig, digest) = sign(&keypair, &body, 9_999_999_999);

            let response = app
                .oneshot(packets_request(
                    body,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let cost_header = response
                .headers()
                .get(connector_btp::ACCUMULATED_COST_HEADER)
                .map(|value| value.to_str().unwrap().to_owned());

            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = connector_domain::Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code, RejectCode::t01_peer_unreachable());
            assert!(reject.message.contains("peer-1"), "{}", reject.message);
            // Present on every REJECT, whatever its value; `decode` leaves
            // the field zero because the cost never rides the OER bytes.
            assert_eq!(cost_header.as_deref(), Some("0"));
        }
    }

    /// `POST /routes/leased` (issue #427): a controller outside this
    /// connector pushes a route to a peer with a time limit, driven end to
    /// end over real HTTP exactly like `POST /packets`'s write-auth suite
    /// above -- no signature, no write.
    mod leased_route_writes {
        use super::*;
        use crate::rfc9421::sign_request;
        use chrono::TimeZone;
        use ed25519_dalek::Keypair;
        use rand::rngs::OsRng;

        fn keypair() -> Keypair {
            Keypair::generate(&mut OsRng)
        }

        fn sign(keypair: &Keypair, body: &[u8], expires: u64) -> (String, String, String) {
            sign_request(
                keypair,
                "POST",
                "/routes/leased",
                body,
                1_000,
                Some(expires),
            )
        }

        fn leased_route_request(
            body: Vec<u8>,
            signature_input: Option<&str>,
            signature: Option<&str>,
            content_digest: Option<&str>,
        ) -> Request<Body> {
            let mut builder = Request::builder().method("POST").uri("/routes/leased");
            if let Some(v) = signature_input {
                builder = builder.header("signature-input", v);
            }
            if let Some(v) = signature {
                builder = builder.header("signature", v);
            }
            if let Some(v) = content_digest {
                builder = builder.header("content-digest", v);
            }
            builder.body(Body::from(body)).unwrap()
        }

        fn router_with(clock: Arc<TestClock>, write_keys: Vec<[u8; 32]>) -> Router {
            let app_client = Arc::new(FakeAppClient::new());
            // A leased route reaches `forward_via_peer_route` without ever
            // passing `Config::load`, so ADR 0042's covering configuration
            // has to be supplied here for a packet on one to be deliverable
            // at all (issue #1145).
            let connector = Arc::new(covering(
                Connector::new(
                    vec![],
                    vec![],
                    app_client,
                    Arc::new(InProcessPeerTransport::new()),
                    clock,
                ),
                "peer-1",
            ));
            let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("operator-test-key"));
            router(
                connector,
                empty_claim_gate(),
                signer,
                "correct-token".to_string(),
                write_keys,
                None,
            )
        }

        #[tokio::test]
        async fn creating_a_leased_route_requires_a_valid_write_signature() {
            let clock = Arc::new(TestClock::new(chrono::Utc::now()));
            let app = router_with(clock, vec![]);
            let body = serde_json::to_vec(&serde_json::json!({
                "prefix": "g.example.leased",
                "peer_id": "peer-1",
                "fee": 0,
                "ttl_seconds": 60,
            }))
            .unwrap();

            let response = app
                .oneshot(leased_route_request(body, None, None, None))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn a_validly_signed_write_creates_a_leased_route_visible_over_the_read_surface() {
            let start = chrono::Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
            let clock = Arc::new(TestClock::new(start));
            let keypair = keypair();
            let app = router_with(clock, vec![keypair.public.to_bytes()]);
            let body = serde_json::to_vec(&serde_json::json!({
                "prefix": "g.example.leased",
                "peer_id": "peer-1",
                "ttl_seconds": 60,
            }))
            .unwrap();
            let (sig_input, sig, digest) = sign(&keypair, &body, 9_999_999_999);

            let write_response = app
                .clone()
                .oneshot(leased_route_request(
                    body,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                ))
                .await
                .unwrap();
            assert_eq!(write_response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(write_response.into_body())
                .await
                .unwrap();
            let created: LeasedRouteView = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(created.prefix, "g.example.leased");
            assert_eq!(created.peer_id, "peer-1");
            assert_eq!(created.expires_at, start + chrono::Duration::seconds(60));

            let read_response = get(app, "/routes/leased", Some("correct-token")).await;
            assert_eq!(read_response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(read_response.into_body())
                .await
                .unwrap();
            let leases: Vec<LeasedRouteView> = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(leases, vec![created]);
        }

        #[tokio::test]
        async fn renewing_a_leased_route_extends_its_expiry_from_the_renewal_time() {
            let start = chrono::Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
            let clock = Arc::new(TestClock::new(start));
            let keypair = keypair();
            let app = router_with(clock.clone(), vec![keypair.public.to_bytes()]);
            let body = serde_json::to_vec(&serde_json::json!({
                "prefix": "g.example.leased",
                "peer_id": "peer-1",
                "fee": 0,
                "ttl_seconds": 60,
            }))
            .unwrap();
            let (sig_input, sig, digest) = sign(&keypair, &body, 9_999_999_999);
            let response = app
                .clone()
                .oneshot(leased_route_request(
                    body,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);

            clock.advance(chrono::Duration::seconds(30));
            // A different `ttl_seconds` than the original request, both to
            // avoid signing an identical body (which the replay cache
            // would reject) and to prove the renewed expiry is computed
            // from *this* request's ttl, not the original's.
            let renewal_body = serde_json::to_vec(&serde_json::json!({
                "prefix": "g.example.leased",
                "peer_id": "peer-1",
                "fee": 0,
                "ttl_seconds": 90,
            }))
            .unwrap();
            let (sig_input, sig, digest) = sign(&keypair, &renewal_body, 9_999_999_999);
            let response = app
                .oneshot(leased_route_request(
                    renewal_body,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let renewed: LeasedRouteView = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                renewed.expires_at,
                start + chrono::Duration::seconds(30) + chrono::Duration::seconds(90)
            );
        }

        #[tokio::test]
        async fn an_invalid_prefix_is_rejected_with_bad_request() {
            let clock = Arc::new(TestClock::new(chrono::Utc::now()));
            let keypair = keypair();
            let app = router_with(clock, vec![keypair.public.to_bytes()]);
            let body = serde_json::to_vec(&serde_json::json!({
                "prefix": "g..leased",
                "peer_id": "peer-1",
                "fee": 0,
                "ttl_seconds": 60,
            }))
            .unwrap();
            let (sig_input, sig, digest) = sign(&keypair, &body, 9_999_999_999);

            let response = app
                .oneshot(leased_route_request(
                    body,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }

        /// AC: "A route can be created over the operator surface with a
        /// time limit" -- proven end to end by creating one, then routing
        /// a packet that only matches it. `peer-1` is unregistered on this
        /// test's `InProcessPeerTransport`, so a successful *match*
        /// surfaces as T01 (peer unreachable) rather than F02 (no route)
        /// -- exactly the distinction issue #427's connector-level tests
        /// use to prove selection without standing up a second connector.
        #[tokio::test]
        async fn a_leased_route_created_over_the_operator_surface_is_used_for_routing() {
            use connector_domain::RejectCode;

            let clock = Arc::new(TestClock::new(chrono::Utc::now()));
            let keypair = keypair();
            let app = router_with(clock, vec![keypair.public.to_bytes()]);
            let route_body = serde_json::to_vec(&serde_json::json!({
                "prefix": "g.example.leased",
                "peer_id": "peer-1",
                "fee": 0,
                "ttl_seconds": 60,
            }))
            .unwrap();
            let (sig_input, sig, digest) = sign(&keypair, &route_body, 9_999_999_999);
            let response = app
                .clone()
                .oneshot(leased_route_request(
                    route_body,
                    Some(&sig_input),
                    Some(&sig),
                    Some(&digest),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);

            let prepare = Prepare {
                amount: 0,
                expires_at: chrono::Utc::now() + chrono::Duration::minutes(1),
                greeting: false,
                destination: "g.example.leased".to_string(),
                data: b"routed over a freshly created lease".to_vec(),
            };
            let packet_body = prepare.encode();
            let (sig_input, sig, digest) = sign_request(
                &keypair,
                "POST",
                "/packets",
                &packet_body,
                1_000,
                Some(9_999_999_999),
            );
            let mut packet_request = Request::builder().method("POST").uri("/packets");
            packet_request = packet_request
                .header("signature-input", &sig_input)
                .header("signature", &sig)
                .header("content-digest", &digest);
            let response = app
                .oneshot(packet_request.body(Body::from(packet_body)).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let reject = connector_domain::Reject::decode(&bytes).expect("decode reject");
            assert_eq!(reject.code, RejectCode::t01_peer_unreachable());
        }
    }

    /// Issue #884's runtime peer/route table writes: `POST`/`DELETE`
    /// `/peers*` and `/routes/peers*`. Same authentication contract as
    /// every other write on this surface (ADR 0008), exercised end to end
    /// over real HTTP against the actual production `router()`.
    mod runtime_peer_route_writes {
        use super::*;
        use crate::rfc9421::sign_request;
        use connector_domain::x402::{X402BatchSettlementEvmTerms, X402BatchSettlementTerms};
        use connector_domain::{EdgeIdentity, NodeFacts, NodeSelfDescription, VoucherSignerFact};
        use connector_runtime::{BoundedHttpSelfDescription, OutboundChannels, PeerRouteView};
        use connector_settlement::batch::{
            BatchSettlementPayer, InMemoryBatchChain, InMemoryBatchSettlement, PayerExit,
            ReceiverTerms,
        };
        use ed25519_dalek::Keypair;
        use rand::rngs::OsRng;
        use std::net::SocketAddr;

        fn keypair() -> Keypair {
            Keypair::generate(&mut OsRng)
        }

        /// A **real** node self-description on a **real** socket, served
        /// by axum on loopback.
        ///
        /// `POST /peers` establishes a peering by fetching this document
        /// (ADR 0058), and what it does with the answer -- which endpoint
        /// it dials, which settlement address it derives a channel from --
        /// is the behaviour under test. A fake handing back a value would
        /// skip the fetch, which is the half that is new.
        fn serve_self_description(settlement_address: &str) -> SocketAddr {
            // The fake chain's settled token: what the counterparty's x402
            // terms must name for this node's paying half to open on them.
            let ReceiverTerms::Evm(fake) =
                InMemoryBatchSettlement::new(PayerExit::Withdrawal, 0).published_terms()
            else {
                unreachable!("an EVM-shaped fake publishes EVM terms");
            };
            let token = format!(
                "0x{}",
                fake.token
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            );
            let document = NodeSelfDescription::describe(
                &NodeFacts {
                    ilp_addresses: vec!["g.example.counterparty".to_string()],
                    http_endpoint: Some("http://counterparty.example/ilp".to_string()),
                    btp_endpoint: None,
                    peer_carriages: vec!["http".to_string()],
                    batch_settlements: vec![X402BatchSettlementTerms::Evm(
                        X402BatchSettlementEvmTerms {
                            network: NETWORK.to_string(),
                            asset: token,
                            pay_to: settlement_address.to_string(),
                            receiver_authorizer: settlement_address.to_string(),
                            min_withdraw_delay_secs: 86_400,
                            name: "USDC".to_string(),
                            version: "2".to_string(),
                            asset_transfer_method: Default::default(),
                            facilitator: None,
                        },
                    )],
                    voucher_signers: vec![VoucherSignerFact {
                        network: NETWORK.to_string(),
                        signer: settlement_address.to_string(),
                    }],
                },
                Some(EdgeIdentity {
                    key_id: "counterparty-key".to_string(),
                    public_key: "0x04ab".to_string(),
                }),
                Vec::new(),
                None,
            );
            let app = Router::new().route(
                "/ilp",
                axum::routing::get(move || {
                    let document = document.clone();
                    async move { Json(document) }
                }),
            );
            let server = axum::Server::bind(&"127.0.0.1:0".parse().expect("loopback"))
                .serve(app.into_make_service());
            let addr = server.local_addr();
            tokio::spawn(async move {
                let _ = server.await;
            });
            addr
        }

        /// The counterparty's EVM settlement address, as its document
        /// publishes it. Deliberately not this node's own and deliberately
        /// not an edge identity: the channel derives from the settlement
        /// address of the chain in question.
        const COUNTERPARTY_SETTLEMENT: &str = "0x00000000000000000000000000000000000000aa";

        /// The CAIP-2 network both nodes' x402 terms name.
        const NETWORK: &str = "eip155:31337";

        /// A `POST /peers` body: the operator's label, the counterparty's
        /// URL, and the operator's own policy about them -- with the
        /// deposit this node's own outbound channel is opened with (ADR
        /// 0075 decision 4).
        fn peer_body(id: &str, addr: SocketAddr, fee: u64) -> Vec<u8> {
            serde_json::to_vec(&serde_json::json!({
                "id": id,
                "url": format!("http://{addr}/ilp"),
                "fee": fee,
                "deposit": 1_000,
            }))
            .unwrap()
        }

        async fn router_with(write_keys: Vec<[u8; 32]>) -> Router {
            let app_client = Arc::new(FakeAppClient::new());
            let clock = Arc::new(TestClock::new(chrono::Utc::now()));
            // This node's paying half, over the settlement port's in-memory
            // fake -- the fake that passes the paying contract suite, so
            // the open these tests drive is the one a chain-backed payer
            // takes (ADR 0007).
            let payer = Arc::new(InMemoryBatchSettlement::on(
                InMemoryBatchChain::new(PayerExit::Withdrawal),
                0x01,
                86_400,
            ));
            payer.fund(10_000);
            let outbound = OutboundChannels::restore(
                Arc::new(connector_runtime::InMemoryJournal::new()),
                vec![(SettlementChain::Evm, payer as Arc<dyn BatchSettlementPayer>)],
            )
            .await
            .expect("an empty journal replays");
            let connector = Arc::new(
                Connector::new(
                    vec![],
                    vec![],
                    app_client,
                    Arc::new(InProcessPeerTransport::new()),
                    clock,
                )
                // ADR 0075 decision 4: an EVM peering opens this node's own
                // outbound x402 channel, on these channels.
                .with_outbound_channels(
                    Arc::new(outbound),
                    vec![(SettlementChain::Evm, NETWORK.to_string())],
                )
                // Loopback is `http://`, so these tests are a node that
                // opted into plaintext peer endpoints -- the same opt-in
                // every `local/` topology takes for the same reason.
                .with_self_description_source(Arc::new(BoundedHttpSelfDescription::new(true, None)))
                .with_peer_allow_plaintext_endpoints(true),
            );
            let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("operator-test-key"));
            router(
                connector,
                empty_claim_gate(),
                signer,
                "correct-token".to_string(),
                write_keys,
                None,
            )
        }

        fn signed(keypair: &Keypair, method: &str, path: &str, body: Vec<u8>) -> Request<Body> {
            let (sig_input, sig, digest) =
                sign_request(keypair, method, path, &body, 1_000, Some(9_999_999_999));
            Request::builder()
                .method(method)
                .uri(path)
                .header("signature-input", sig_input)
                .header("signature", sig)
                .header("content-digest", digest)
                .body(Body::from(body))
                .unwrap()
        }

        fn unsigned(method: &str, path: &str, body: Vec<u8>) -> Request<Body> {
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::from(body))
                .unwrap()
        }

        #[tokio::test]
        async fn upserting_a_peer_requires_a_valid_write_signature() {
            let app = router_with(vec![]).await;
            let addr = serve_self_description(COUNTERPARTY_SETTLEMENT);

            let response = app
                .oneshot(unsigned(
                    "POST",
                    "/peers",
                    peer_body("runtime-hop", addr, 0),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        #[tokio::test]
        async fn a_validly_signed_write_creates_a_peer_visible_over_the_read_surface() {
            let keypair = keypair();
            let app = router_with(vec![keypair.public.to_bytes()]).await;
            let addr = serve_self_description(COUNTERPARTY_SETTLEMENT);

            let write_response = app
                .clone()
                .oneshot(signed(
                    &keypair,
                    "POST",
                    "/peers",
                    peer_body("runtime-hop", addr, 0),
                ))
                .await
                .unwrap();
            assert_eq!(write_response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(write_response.into_body())
                .await
                .unwrap();
            let established: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(established["id"], "runtime-hop");
            assert_eq!(established["source"], "runtime");
            // The answer says which branch the find-or-open took, so an
            // unintended second channel is visible here (ADR 0058, ADR 0075).
            assert_eq!(established["channel"]["status"], "created");
            assert_eq!(established["channel"]["chain"], "evm");
            let created: PeerView = serde_json::from_value(established).unwrap();

            let read_response = get(app, "/peers", Some("correct-token")).await;
            assert_eq!(read_response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(read_response.into_body())
                .await
                .unwrap();
            let peers: Vec<PeerView> = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(peers, vec![created]);
        }

        #[tokio::test]
        async fn a_validly_signed_write_creates_a_peer_route_visible_over_the_read_surface() {
            let keypair = keypair();
            let app = router_with(vec![keypair.public.to_bytes()]).await;
            let addr = serve_self_description(COUNTERPARTY_SETTLEMENT);
            app.clone()
                .oneshot(signed(
                    &keypair,
                    "POST",
                    "/peers",
                    peer_body("runtime-hop", addr, 3),
                ))
                .await
                .unwrap();
            let route_body = serde_json::to_vec(&serde_json::json!({
                "prefix": "g.example.runtime",
                "peer_id": "runtime-hop",
                "price": 25,
            }))
            .unwrap();

            let write_response = app
                .clone()
                .oneshot(signed(&keypair, "POST", "/routes/peers", route_body))
                .await
                .unwrap();
            assert_eq!(write_response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(write_response.into_body())
                .await
                .unwrap();
            let created: PeerRouteView = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(created.prefix, "g.example.runtime");
            assert_eq!(created.peer_id, "runtime-hop");
            assert_eq!(created.price, Price::flat(25));
            assert_eq!(created.source, RouteSource::Runtime);

            let read_response = get(app, "/routes/peers", Some("correct-token")).await;
            assert_eq!(read_response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(read_response.into_body())
                .await
                .unwrap();
            let routes: Vec<PeerRouteView> = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(routes, vec![created]);
        }

        /// A route naming a peer id nothing recognizes -- the runtime
        /// analogue of `connector-config`'s load-time `UnknownPeerId` --
        /// is `400`, not silently accepted as an orphaned row.
        #[tokio::test]
        async fn a_peer_route_naming_an_unknown_peer_id_is_a_bad_request() {
            let keypair = keypair();
            let app = router_with(vec![keypair.public.to_bytes()]).await;
            let body = serde_json::to_vec(&serde_json::json!({
                "prefix": "g.example.runtime",
                "peer_id": "nobody",
                "price": 0,
            }))
            .unwrap();

            let response = app
                .oneshot(signed(&keypair, "POST", "/routes/peers", body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }

        /// A validly signed `DELETE /peers/:id` removes a runtime peer
        /// (issue #884); it no longer appears over `GET /peers`.
        #[tokio::test]
        async fn a_validly_signed_delete_removes_a_peer() {
            let keypair = keypair();
            let app = router_with(vec![keypair.public.to_bytes()]).await;
            let addr = serve_self_description(COUNTERPARTY_SETTLEMENT);
            app.clone()
                .oneshot(signed(
                    &keypair,
                    "POST",
                    "/peers",
                    peer_body("runtime-hop", addr, 0),
                ))
                .await
                .unwrap();

            let delete_response = app
                .clone()
                .oneshot(signed(&keypair, "DELETE", "/peers/runtime-hop", Vec::new()))
                .await
                .unwrap();
            assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

            let read_response = get(app, "/peers", Some("correct-token")).await;
            let bytes = hyper::body::to_bytes(read_response.into_body())
                .await
                .unwrap();
            let peers: Vec<PeerView> = serde_json::from_slice(&bytes).unwrap();
            assert!(peers.is_empty());
        }

        #[tokio::test]
        async fn deleting_a_peer_requires_a_valid_write_signature() {
            let app = router_with(vec![]).await;

            let response = app
                .oneshot(unsigned("DELETE", "/peers/runtime-hop", Vec::new()))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        /// A validly signed `DELETE /routes/peers/:prefix` removes a
        /// runtime peer route; it no longer appears over
        /// `GET /routes/peers`.
        #[tokio::test]
        async fn a_validly_signed_delete_removes_a_peer_route() {
            let keypair = keypair();
            let app = router_with(vec![keypair.public.to_bytes()]).await;
            let addr = serve_self_description(COUNTERPARTY_SETTLEMENT);
            app.clone()
                .oneshot(signed(
                    &keypair,
                    "POST",
                    "/peers",
                    peer_body("runtime-hop", addr, 0),
                ))
                .await
                .unwrap();
            let route_body = serde_json::to_vec(&serde_json::json!({
                "prefix": "g.example.runtime",
                "peer_id": "runtime-hop",
                "fee": 0,
                "price": 0,
            }))
            .unwrap();
            app.clone()
                .oneshot(signed(&keypair, "POST", "/routes/peers", route_body))
                .await
                .unwrap();

            let delete_response = app
                .clone()
                .oneshot(signed(
                    &keypair,
                    "DELETE",
                    "/routes/peers/g.example.runtime",
                    Vec::new(),
                ))
                .await
                .unwrap();
            assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

            let read_response = get(app, "/routes/peers", Some("correct-token")).await;
            let bytes = hyper::body::to_bytes(read_response.into_body())
                .await
                .unwrap();
            let routes: Vec<PeerRouteView> = serde_json::from_slice(&bytes).unwrap();
            assert!(routes.is_empty());
        }

        /// A `DELETE /peers/:id` naming a peer still referenced by a
        /// runtime route is `409`, not a silently orphaned route.
        #[tokio::test]
        async fn deleting_a_peer_still_referenced_by_a_route_is_a_conflict() {
            let keypair = keypair();
            let app = router_with(vec![keypair.public.to_bytes()]).await;
            let addr = serve_self_description(COUNTERPARTY_SETTLEMENT);
            app.clone()
                .oneshot(signed(
                    &keypair,
                    "POST",
                    "/peers",
                    peer_body("runtime-hop", addr, 0),
                ))
                .await
                .unwrap();
            let route_body = serde_json::to_vec(&serde_json::json!({
                "prefix": "g.example.runtime",
                "peer_id": "runtime-hop",
                "fee": 0,
                "price": 0,
            }))
            .unwrap();
            app.clone()
                .oneshot(signed(&keypair, "POST", "/routes/peers", route_body))
                .await
                .unwrap();

            let response = app
                .oneshot(signed(&keypair, "DELETE", "/peers/runtime-hop", Vec::new()))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT);
        }
    }

    /// The x402 channel writes (ADR 0075 decision 11), driven as an external
    /// caller through the production router over the settlement port's
    /// in-memory fake -- the fake that passes both halves' contract suites,
    /// so these tests exercise the surface, and the chains' own suites the
    /// chains (ADR 0007). The end-to-end runs against anvil and a validator
    /// are the settlement crates' and `connector-cli`'s.
    mod batch_channel_writes {
        use super::*;
        use crate::rfc9421::sign_request;
        use connector_runtime::OutboundChannels;
        use connector_settlement::batch::{
            BatchSettlementBackend, BatchSettlementPayer, HeldVoucher, InMemoryBatchChain,
            InMemoryBatchSettlement, PayerExit,
        };
        use ed25519_dalek::Keypair;
        use rand::rngs::OsRng;
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::RwLock;

        const ONE_DAY: u64 = 86_400;

        /// This node (`0x01` on the fake chain) and a counterparty (`0x02`),
        /// each holding 10,000; this node's surface; and the vouchers its
        /// client edge holds, which a test fills.
        struct Node {
            app: Router,
            operator: Keypair,
            created: AtomicU64,
            batch: Arc<BatchChannels>,
            chain: Arc<InMemoryBatchChain>,
            this: Arc<InMemoryBatchSettlement>,
            counterparty: Arc<InMemoryBatchSettlement>,
            held: Arc<RwLock<Vec<HeldVoucher>>>,
        }

        impl Node {
            async fn new() -> Node {
                let chain = InMemoryBatchChain::new(PayerExit::Withdrawal);
                let this = Arc::new(InMemoryBatchSettlement::on(
                    Arc::clone(&chain),
                    0x01,
                    ONE_DAY,
                ));
                let counterparty = Arc::new(InMemoryBatchSettlement::on(
                    Arc::clone(&chain),
                    0x02,
                    ONE_DAY,
                ));
                this.fund(10_000);
                counterparty.fund(10_000);
                let outbound = OutboundChannels::restore(
                    Arc::new(InMemoryJournal::new()),
                    vec![(
                        SettlementChain::Evm,
                        Arc::clone(&this) as Arc<dyn BatchSettlementPayer>,
                    )],
                )
                .await
                .expect("an empty journal replays");
                let held = Arc::new(RwLock::new(Vec::new()));
                let batch = Arc::new(BatchChannels::new(
                    Arc::new(outbound),
                    vec![(
                        SettlementChain::Evm,
                        Arc::clone(&this) as Arc<dyn BatchSettlementBackend>,
                    )],
                    Arc::clone(&held) as Arc<dyn connector_settlement::batch::HeldVouchers>,
                    Vec::new(),
                ));
                let operator = Keypair::generate(&mut OsRng);
                let connector = Arc::new(Connector::new(
                    vec![],
                    vec![],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(InProcessPeerTransport::new()),
                    Arc::new(TestClock::new(chrono::Utc::now())),
                ));
                let app = router_with_batch_channels(
                    connector,
                    empty_claim_gate(),
                    Arc::new(LocalSigner::generate("operator-test-key")),
                    "correct-token".to_string(),
                    vec![operator.public.to_bytes()],
                    None,
                    Some(Arc::clone(&batch)),
                );
                Node {
                    app,
                    operator,
                    created: AtomicU64::new(1_000),
                    batch,
                    chain,
                    this,
                    counterparty,
                    held,
                }
            }

            /// A write signed by this node's operator key. Each is signed
            /// at a fresh `created`, so a repeated write is a new request and
            /// not a replay.
            async fn write(
                &self,
                path: &str,
                body: serde_json::Value,
            ) -> (StatusCode, serde_json::Value) {
                let body = if body.is_null() {
                    Vec::new()
                } else {
                    serde_json::to_vec(&body).unwrap()
                };
                let created = self.created.fetch_add(1, Ordering::SeqCst);
                let (sig_input, sig, digest) = sign_request(
                    &self.operator,
                    "POST",
                    path,
                    &body,
                    created,
                    Some(9_999_999_999),
                );
                let request = Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("signature-input", sig_input)
                    .header("signature", sig)
                    .header("content-digest", digest)
                    .body(Body::from(body))
                    .unwrap();
                read_json(self.app.clone().oneshot(request).await.unwrap()).await
            }

            async fn read(&self, path: &str) -> serde_json::Value {
                let (status, body) =
                    read_json(get(self.app.clone(), path, Some("correct-token")).await).await;
                assert_eq!(status, StatusCode::OK);
                body
            }
        }

        async fn read_json(response: Response) -> (StatusCode, serde_json::Value) {
            let status = response.status();
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            let body = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                serde_json::Value::String(String::from_utf8_lossy(&bytes).into())
            });
            (status, body)
        }

        /// The counterparty's `batchSettlements` entry, as its
        /// self-description publishes it.
        fn counterparty_terms() -> serde_json::Value {
            let address = format!("0x{}", "02".repeat(20));
            serde_json::json!({
                "network": "eip155:31337",
                "asset": format!("0x{}", "70".repeat(20)),
                "payTo": address,
                "receiverAuthorizer": address,
                "withdrawDelay": ONE_DAY,
                "name": "USDC",
                "version": "2",
            })
        }

        fn channel_writes(id: &str) -> [(String, serde_json::Value); 4] {
            [
                (
                    "/channels".to_string(),
                    serde_json::json!({ "terms": counterparty_terms(), "deposit": 1_000 }),
                ),
                (
                    format!("/channels/{id}/fund"),
                    serde_json::json!({ "amount": 1 }),
                ),
                (format!("/channels/{id}/withdraw"), serde_json::Value::Null),
                (format!("/channels/{id}/land"), serde_json::Value::Null),
            ]
        }

        /// ADR 0008 for every channel write: no signature, the read token,
        /// or a signature from a key not on `write_keys` each buys a `401`,
        /// and nothing moves.
        #[tokio::test]
        async fn every_channel_write_needs_an_allowlisted_signature_and_a_bearer_token_is_not_one()
        {
            let node = Node::new().await;
            let stranger = Keypair::generate(&mut OsRng);
            for (path, body) in channel_writes(&format!("0x{}", "ab".repeat(32))) {
                let body = if body.is_null() {
                    Vec::new()
                } else {
                    serde_json::to_vec(&body).unwrap()
                };
                let unsigned = Request::builder()
                    .method("POST")
                    .uri(&path)
                    .body(Body::from(body.clone()))
                    .unwrap();
                let bearer = Request::builder()
                    .method("POST")
                    .uri(&path)
                    .header(header::AUTHORIZATION, "Bearer correct-token")
                    .body(Body::from(body.clone()))
                    .unwrap();
                let (sig_input, sig, digest) =
                    sign_request(&stranger, "POST", &path, &body, 1_000, Some(9_999_999_999));
                let foreign = Request::builder()
                    .method("POST")
                    .uri(&path)
                    .header("signature-input", sig_input)
                    .header("signature", sig)
                    .header("content-digest", digest)
                    .body(Body::from(body))
                    .unwrap();
                for request in [unsigned, bearer, foreign] {
                    let response = node.app.clone().oneshot(request).await.unwrap();
                    assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
                }
            }
            assert_eq!(node.this.balance(), 10_000, "nothing moved");
        }

        /// ADR 0075 decision 11's removals: the five `toon-channel` writes
        /// are gone, not refused -- there is no route to reach.
        #[tokio::test]
        async fn the_retired_toon_channel_writes_are_gone() {
            let node = Node::new().await;
            for step in [
                "redeem",
                "redeem-latest",
                "settle",
                "close",
                "cooperative-close",
            ] {
                let (status, _) = node
                    .write(
                        &format!("/channels/0x{}/{step}", "ab".repeat(32)),
                        serde_json::Value::Null,
                    )
                    .await;
                assert_eq!(status, StatusCode::NOT_FOUND, "{step}");
            }
        }

        /// Open, fund, list and withdraw an outbound channel, every step a
        /// signed write, every figure read back over the surface.
        #[tokio::test]
        async fn an_outbound_channel_is_opened_funded_listed_and_withdrawn() {
            let node = Node::new().await;
            let (status, opened) = node
                .write(
                    "/channels",
                    serde_json::json!({ "terms": counterparty_terms(), "deposit": 1_000 }),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{opened}");
            assert_eq!(opened["scheme"], "batch-settlement");
            assert_eq!(opened["direction"], "outbound");
            assert_eq!(opened["status"], "open");
            assert_eq!(opened["collateral"], 1_000);
            assert_eq!(opened["counterparty"], format!("0x{}", "02".repeat(20)));
            assert_eq!(opened["resumed"], false);
            assert_eq!(node.this.balance(), 9_000);
            let id = opened["id"].as_str().expect("an id").to_string();

            let (status, funded) = node
                .write(
                    &format!("/channels/{id}/fund"),
                    serde_json::json!({ "amount": 500 }),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{funded}");
            assert_eq!(funded["collateral"], 1_500);
            let (status, _) = node
                .write(
                    &format!("/channels/{id}/fund"),
                    serde_json::json!({ "total": 2_000 }),
                )
                .await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "an x402 channel takes an increment, never a total"
            );

            let listed = node.read("/channels").await;
            let row = listed
                .as_array()
                .expect("a list")
                .iter()
                .find(|row| row["id"] == id.as_str())
                .expect("the channel is listed");
            assert_eq!(row["direction"], "outbound");
            assert_eq!(row["collateral"], 1_500);
            assert_eq!(row["watermark"], 0);

            let (status, started) = node
                .write(&format!("/channels/{id}/withdraw"), serde_json::Value::Null)
                .await;
            assert_eq!(status, StatusCode::OK, "{started}");
            assert_eq!(started["step"], "started");
            assert_eq!(started["status"], "withdrawing");
            let (status, early) = node
                .write(&format!("/channels/{id}/withdraw"), serde_json::Value::Null)
                .await;
            assert_eq!(status, StatusCode::CONFLICT, "{early}");
            node.chain.advance_time(ONE_DAY);
            let (status, finished) = node
                .write(&format!("/channels/{id}/withdraw"), serde_json::Value::Null)
                .await;
            assert_eq!(status, StatusCode::OK, "{finished}");
            assert_eq!(finished["step"], "finished");
            assert_eq!(node.this.balance(), 10_000);
        }

        /// ADR 0075, #1376, #1383: the `toon-channel` open is retired on
        /// both chains. A `POST /channels` body without `terms` -- the old
        /// `counterparty_hex` shape, on either chain or none -- is refused
        /// by name, not answered with a deserialisation error, and opens
        /// nothing.
        #[tokio::test]
        async fn a_channel_open_without_terms_is_refused_by_name() {
            let node = Node::new().await;
            for body in [
                serde_json::json!({
                    "counterparty_hex": "ab".repeat(20),
                    "settlement_timeout_seconds": 3600,
                    "chain": "solana",
                }),
                serde_json::json!({
                    "counterparty_hex": "ab".repeat(20),
                    "settlement_timeout_seconds": 3600,
                    "chain": "evm",
                }),
                serde_json::json!({ "deposit": 1_000 }),
            ] {
                let (status, refusal) = node.write("/channels", body.clone()).await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
                let refusal = refusal.as_str().expect("a text refusal");
                assert!(
                    refusal.contains("toon-channel") && refusal.contains("`terms`"),
                    "{refusal}"
                );
            }
            assert_eq!(node.this.balance(), 10_000, "nothing moved");
            let listed = node.read("/channels").await;
            assert!(
                !listed
                    .as_array()
                    .expect("a list")
                    .iter()
                    .any(|row| row["direction"] == "outbound"),
                "{listed}"
            );
        }

        /// ADR 0075, #1378, #1383: `POST /channels/:id/fund` tops up this
        /// node's own outbound x402 channels and nothing else. A channel
        /// id that is not one -- a `toon-channel` id on either chain, or a
        /// channel the counterparty opened toward this node -- is refused
        /// by name, and a `total` (the retired `toon-channel` form) is a
        /// `400` that says to give `amount`.
        #[tokio::test]
        async fn funding_anything_but_an_outbound_x402_channel_is_refused_by_name() {
            let node = Node::new().await;
            let inbound = node
                .counterparty
                .open(node.this.published_terms(), 1_000)
                .await
                .expect("the counterparty opens toward this node");
            let inbound = inbound.presentation.channel().0.clone();
            for id in [
                format!("0x{}", "ab".repeat(32)),
                "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
                inbound,
            ] {
                let (status, refusal) = node
                    .write(
                        &format!("/channels/{id}/fund"),
                        serde_json::json!({ "amount": 500 }),
                    )
                    .await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "{id}");
                let refusal = refusal.as_str().expect("a text refusal");
                assert!(
                    refusal.contains("not an outbound x402 channel of this node")
                        && refusal.contains("toon-channel funding"),
                    "{refusal}"
                );
            }
            assert_eq!(node.this.balance(), 10_000, "nothing moved");

            let (status, refusal) = node
                .write(
                    &format!("/channels/0x{}/fund", "ab".repeat(32)),
                    serde_json::json!({ "total": 500 }),
                )
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            let refusal = refusal.as_str().expect("a text refusal");
            assert!(
                refusal.contains("give exactly `amount`") && refusal.contains("`total`"),
                "{refusal}"
            );
        }

        /// Land the held latest voucher on an inbound channel; landing it
        /// again is a `409` by name, and a channel with none held a `404`.
        #[tokio::test]
        async fn the_held_voucher_on_an_inbound_channel_lands_over_the_surface() {
            let node = Node::new().await;
            let opened = node
                .counterparty
                .open(node.this.published_terms(), 1_000)
                .await
                .expect("the counterparty opens toward this node");
            let channel = opened.presentation.channel().clone();
            let voucher = node
                .counterparty
                .sign_voucher(&channel, 300)
                .await
                .expect("sign");
            node.held.write().unwrap().push(HeldVoucher {
                presentation: opened.presentation,
                voucher,
            });

            let listed = node.read("/channels").await;
            let row = listed
                .as_array()
                .expect("a list")
                .iter()
                .find(|row| row["id"] == channel.0.as_str())
                .expect("the inbound channel is listed");
            assert_eq!(row["direction"], "inbound");
            assert_eq!(row["watermark"], 300);
            assert_eq!(row["landed"], 0);

            let path = format!("/channels/{}/land", channel.0);
            let (status, landed) = node.write(&path, serde_json::Value::Null).await;
            assert_eq!(status, StatusCode::OK, "{landed}");
            assert_eq!(landed["landed"], 300);
            let (status, _) = node.write(&path, serde_json::Value::Null).await;
            assert_eq!(status, StatusCode::CONFLICT);
            let (status, _) = node
                .write(
                    &format!("/channels/0x{}/land", "cd".repeat(32)),
                    serde_json::Value::Null,
                )
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
        }

        /// `GET /claims` shows vouchers signed, with their direction and
        /// scheme.
        #[tokio::test]
        async fn claims_show_the_vouchers_this_node_signed() {
            let node = Node::new().await;
            let (_, opened) = node
                .write(
                    "/channels",
                    serde_json::json!({ "terms": counterparty_terms(), "deposit": 1_000 }),
                )
                .await;
            let id = opened["id"].as_str().expect("an id").to_string();
            node.batch
                .outbound()
                .sign_voucher(&id, 250)
                .await
                .expect("sign");
            let claims: Vec<ClaimView> =
                serde_json::from_value(node.read("/claims").await).unwrap();
            let [claim] = claims.as_slice() else {
                panic!("one claim, got {claims:?}");
            };
            assert_eq!(claim.direction, ClaimDirection::Outbound);
            assert_eq!(claim.scheme, ClaimScheme::BatchSettlement);
            assert_eq!(claim.book, ClaimBookKind::Outbound);
            assert_eq!(claim.channel_id, format!("evm:{id}"));
            assert_eq!(claim.cumulative_amount, 250);
        }

        /// A node with no x402 backend answers every x402 channel write
        /// with `503`, the same as a node with no settlement backend.
        #[tokio::test]
        async fn a_node_with_no_x402_backend_answers_503() {
            let keypair = Keypair::generate(&mut OsRng);
            let app = router(
                Arc::new(Connector::new(
                    vec![],
                    vec![],
                    Arc::new(FakeAppClient::new()),
                    Arc::new(InProcessPeerTransport::new()),
                    Arc::new(TestClock::new(chrono::Utc::now())),
                )),
                empty_claim_gate(),
                Arc::new(LocalSigner::generate("operator-test-key")),
                "correct-token".to_string(),
                vec![keypair.public.to_bytes()],
                None,
            );
            for (created, (path, body)) in
                (1_000..).zip(channel_writes(&format!("0x{}", "ab".repeat(32))))
            {
                let body = if body.is_null() {
                    Vec::new()
                } else {
                    serde_json::to_vec(&body).unwrap()
                };
                let (sig_input, sig, digest) =
                    sign_request(&keypair, "POST", &path, &body, created, Some(9_999_999_999));
                let request = Request::builder()
                    .method("POST")
                    .uri(&path)
                    .header("signature-input", sig_input)
                    .header("signature", sig)
                    .header("content-digest", digest)
                    .body(Body::from(body))
                    .unwrap();
                let response = app.clone().oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
            }
        }
    }
}
