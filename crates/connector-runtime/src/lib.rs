//! The packet plane and its ports. See ADR 0001.

mod app_client;
mod attribution;
mod claim;
mod clock;
mod connector;
mod journal;
mod metrics;
mod operator_view;
mod outbound_client;
mod peer_route_store;
mod peer_transport;
mod peering;
mod rate_poller;
mod rate_table;
mod route;
mod self_description;
// Behind a feature rather than `#[cfg(test)]`, unlike `test_support` below:
// both peer-carriage crates need it and a `#[cfg(test)]` item is invisible
// outside its own crate -- but it binds a listener, so the shipped binary
// must not carry it either. See the module's header.
#[cfg(any(test, feature = "test-support"))]
mod socks5_test_server;
#[cfg(test)]
mod test_support;

pub use app_client::{AppClient, AppOutcome, Delivery, FakeAppClient, HttpAppClient};
// The three request headers a terminating connector states to the app
// about the payment that brought a packet to it (ADR 0040) -- exported so
// a test, an operator tool or a second implementation names them from one
// place rather than retyping a string literal.
pub use attribution::{AMOUNT_HEADER, CHAIN_HEADER, PAYER_HEADER};
pub use claim::{
    ChannelDomain, ClaimAckOutcome, ClaimBook, ClaimRejectReason, ClaimSignature, InvalidChannelId,
    InvalidSolanaChannel, SolanaChannel, WireClaim,
};
pub use clock::{Clock, SystemClock, TestClock};
pub use connector::{
    ChannelOperationError, ClientRouteFacts, ClientRouteKind, ClientRoutePrice, Connector,
    LeaseRouteError, PeerRouteTableError, ProbeDenied,
};
// Re-exported for callers that hold a `Connector` but not a config-crate
// dependency of their own (`connector-operator`): the chain key
// `Connector::with_settlement` files a settlement backend under, and
// `Connector::open_channel` names a backend by.
pub use connector_config::{SettlementChain, UnknownSettlementChain};
pub use journal::{FileJournal, InMemoryJournal, Journal, JournalError};
pub use metrics::Metrics;
pub use operator_view::{
    ChannelView, ChannelViewStatus, ClaimBookKind, ClaimDirection, ClaimView, LeasedRouteView,
    PeerRouteView, PeerView, RouteSource, RouteView,
};
// The OUTBOUND client ledger (issue #873) -- what this node signs to pay a
// next hop, deliberately a different book from `ClaimBook`'s inbound
// journal above. See `outbound_client`'s header for the table of
// differences and for why the two must never merge.
pub use outbound_client::{
    ClaimStateChallengeSigner, ClaimStateDomain, ClaimStateSource, ClaimWatermark, EvmDomain,
    HttpClaimState, OutboundClaim, OutboundClaimBinding, OutboundClientError, OutboundClientLedger,
    OwnedHttpClaimState, SolanaDomain,
};
pub use peer_route_store::{
    PeerRouteStore, PeerRouteStoreError, RuntimePeerChannel, RuntimePeering, RuntimePeers,
};
pub use peer_transport::{
    InProcessPeerTransport, PeerForward, PeerRegistrar, PeerTransport, NO_SOCKS_PROXY,
};
// ADR 0058's one operator write: establish a peering from a URL.
pub use peering::{
    ChannelBranch, EstablishPeeringError, EstablishedChannel, PeeringEstablished,
    PEERING_SETTLEMENT_TIMEOUT_SECONDS,
};
// The rate table a forward reads and the background poller that keeps it
// fresh (ADR 0071 decision 6, issue #1294). Two halves of one rule: the
// poller is the only thing here that awaits anything, and the read side is
// synchronous by construction so that no packet can wait on a rate.
pub use rate_poller::{poll_interval, QuotePathUnusable, RatePoller};
pub use rate_table::SharedRateTable;
pub use route::{LeasedRoute, PeerRoute};
// Reading ANOTHER node's self-description, so a peering can be established
// from a URL (ADR 0058, ADR 0050). The one outbound request this connector
// makes to an operator-supplied host, and it is bounded.
pub use self_description::{
    BoundedHttpSelfDescription, SelfDescriptionError, SelfDescriptionSource,
    UnreachableSelfDescription, FETCH_TIMEOUT, MAX_DOCUMENT_BYTES,
};
#[cfg(any(test, feature = "test-support"))]
pub use socks5_test_server::Socks5TestServer;
