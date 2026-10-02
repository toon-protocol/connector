//! The packet plane and its ports. See ADR 0001.

mod app_client;
mod attribution;
mod batch_channels;
mod claim;
mod clock;
mod connector;
mod journal;
mod metrics;
mod operator_view;
mod outbound_voucher;
mod peer_route_store;
mod peer_transport;
mod peering;
mod rate_poller;
mod rate_table;
mod route;
mod self_description;
mod voucher_binding;
// Behind a feature rather than `#[cfg(test)]`, unlike `test_support` below:
// both peer-carriage crates need it and a `#[cfg(test)]` item is invisible
// outside its own crate -- but it binds a listener, so the shipped binary
// must not carry it either. See the module's header.
#[cfg(any(test, feature = "test-support"))]
mod socks5_test_server;
// A payable hop for a test that forwards (ADR 0042): a fake, so behind the
// same feature as the SOCKS5 server above and for the same reason.
#[cfg(any(test, feature = "test-support"))]
pub mod covering_fake;
#[cfg(test)]
mod test_support;

pub use app_client::{AppClient, AppOutcome, Delivery, FakeAppClient, HttpAppClient};
// The three request headers a terminating connector states to the app
// about the payment that brought a packet to it (ADR 0040) -- exported so
// a test, an operator tool or a second implementation names them from one
// place rather than retyping a string literal.
pub use attribution::{AMOUNT_HEADER, CHAIN_HEADER, PAYER_HEADER};
pub use batch_channels::{
    receiver_terms, BatchChannelError, BatchChannelView, BatchChannelViewStatus, BatchChannels,
    ChannelDirection, OutboundChannels, WithdrawStep,
};
pub use claim::{ClaimAckOutcome, ClaimRejectReason, Covering};
pub use clock::{Clock, SystemClock, TestClock};
pub use connector::{
    ClientRouteFacts, ClientRouteKind, ClientRoutePrice, ConfigPeeringError, Connector,
    LeaseRouteError, PeerRouteTableError, ProbeDenied,
};
// Re-exported for callers that hold a `Connector` but not a config-crate
// dependency of their own (`connector-operator`): the chain key an x402
// channel and its settlement backend are filed under.
pub use connector_config::{SettlementChain, UnknownSettlementChain};
pub use journal::{FileJournal, InMemoryJournal, Journal, JournalError};
pub use metrics::Metrics;
pub use operator_view::{
    ClaimBookKind, ClaimDirection, ClaimScheme, ClaimView, DeclaredRates, LeasedRouteView,
    PeerRouteView, PeerView, RateView, RateViewState, RefusedRefreshView, RouteSource, RouteView,
};
// What this node puts on a peer carriage when it pays over one of its own
// x402 channels (ADR 0075 decisions 5 and 6), and how it asks the receiver
// where that channel's watermark stands.
pub use outbound_voucher::{
    challenge_entry, voucher_json, HttpVoucherState, VoucherStateSource, PEER_CHALLENGE_TTL_SECS,
};
pub use peer_route_store::{
    PeerRouteStore, PeerRouteStoreError, RuntimePeerChannel, RuntimePeering, RuntimePeers,
};
pub use peer_transport::{
    AnswerWait, InProcessPeerTransport, PeerForward, PeerRegistrar, PeerTransport, NO_SOCKS_PROXY,
};
// ADR 0058's one operator write: establish a peering from a URL.
pub use peering::{ChannelBranch, EstablishPeeringError, EstablishedChannel, PeeringEstablished};
// The rate table a forward reads and the background poller that keeps it
// fresh (ADR 0071 decision 6, issue #1294). Two halves of one rule: the
// poller is the only thing here that awaits anything, and the read side is
// synchronous by construction so that no packet can wait on a rate.
pub use rate_poller::{poll_interval, QuotePathUnusable, RatePoller, RateSources};
pub use rate_table::SharedRateTable;
pub use route::{LeasedRoute, PeerRoute};
// Reading ANOTHER node's self-description, so a peering can be established
// from a URL (ADR 0058, ADR 0050). The one outbound request this connector
// makes to an operator-supplied host, and it is bounded.
pub use self_description::{
    BoundedHttpSelfDescription, SelfDescriptionError, SelfDescriptionSource,
    UnreachableSelfDescription, FETCH_TIMEOUT, MAX_DOCUMENT_BYTES,
};
// ADR 0075 decision 4 (issue #1377): an inbound x402 channel is a peer's
// when its voucher signer is bound to that peering.
#[cfg(any(test, feature = "test-support"))]
pub use socks5_test_server::{Socks5TestServer, SocksConnect};
pub use voucher_binding::{VoucherBindingError, VoucherSigner, VoucherSignerBindings};
