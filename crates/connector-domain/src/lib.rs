//! Pure domain logic: no async, no I/O. See ADR 0001.
//!
//! ILPv4 packet types with their wire encoding -- RFC-0027's semantics in
//! TOON's own encoding, which is not byte-compatible with it (ADR 0063;
//! `packet.rs` has the table) -- over OER primitives (RFC-0030),
//! ILP address validation (RFC-0015), longest-prefix route selection,
//! flat per-packet fee arithmetic (ADR 0010) and the rational-over-base-units
//! conversion a hop crosses a denomination boundary at, over the token identity
//! a rate is declared between and the table of declared rates a forwarding
//! path reads them from -- under the spread, ttl and max_move an operator
//! guards a pair with (ADR 0071, [`rate`], [`asset`], [`rate_table`]),
//! what a terminated route charges for one packet -- a schedule over the
//! packet's payload length, flat when its slope is zero (ADR 0065, [`price`]) --
//! fulfilment / expiry rules (RFC-0022; the execution condition itself left
//! the wire under issue #1269 / ADR 0069),
//! claim nonce / watermark rules (ADR 0004, ADR 0005, issue #423), and the
//! structured envelope a packet carries to and from the app behind a
//! terminated route (ADR 0018, issue #519). Also the x402 `payment-required`
//! greeting's wire shape and its reader (issue #874, [`x402`]) -- shared here
//! because the crate that writes one and the crates that read one sit on
//! opposite sides of the graph and must not each own a definition of it, and
//! the node self-description ([`node`], ADR 0050) the greeting is now a
//! projection of.

mod address;
pub mod asset;
mod claim;
pub mod client_claim;
mod condition;
mod envelope;
mod error;
mod fee;
pub mod identity;
pub mod node;
mod oer;
mod packet;
pub mod price;
mod projection;
pub mod rate;
pub mod rate_table;
mod route;
pub mod x402;

pub use address::is_valid_ilp_address;
pub use asset::{AssetChain, AssetId, AssetIdError};
pub use claim::{advance_watermark, validate_claim, validate_price, ClaimError, Watermark};
pub use condition::{
    delivery_budget, forwarded_expiry, fulfillment_matches_condition, is_expired,
    FORWARDING_MESSAGE_WINDOW,
};
pub use envelope::{EnvelopeError, EnvelopeRequest, EnvelopeResponse};
pub use error::PacketError;
pub use fee::{amount_after_fee, amount_after_rate_and_fee, cost_before_rate_and_fee};
pub use identity::{
    anonymous_identity, resolve_identity, ConfiguredIdentity, SenderIdentity, UnauthorizedIdentity,
};
pub use node::{
    agreed_required_transport, EdgeIdentity, NodeFacts, NodeSelfDescription, RoutePrice,
    CLIENT_EDGE_DEFAULT_VERSION, CLIENT_EDGE_SUPPORTED_VERSIONS,
};
pub use packet::{Fulfill, PacketResponse, Prepare, Reject, RejectCode};
pub use price::Price;
pub use projection::{JournalEntry, Projection};
pub use rate::{Rate, RateError};
pub use rate_table::{
    Freshness, GuardError, GuardOverride, Guards, MaxMove, RateLookup, RateTable, Refresh,
    RefusedRefresh, Spread, Ttl,
};
pub use route::select_route;
