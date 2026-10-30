//! XRPL settlement (map #1563, spec #1596, ADR 0087).
//!
//! - [`XrplBatchSettlement`]: the receiving half of the batch-settlement port
//!   over the ledger's own `PayChannel` (issue #1612). Admission, the channel's
//!   state and landing a voucher with a destination `PaymentChannelClaim`.
//! - `test_support::Xrpld`: a disposable stand-alone `xrpld` in a container,
//!   the repository's first container-backed tier-3 chain (ADR 0084).
//!
//! - The same type is the paying half (`BatchSettlementPayer`): this node
//!   opens, funds and exits its own channels, at most once per open (issue
//!   #1613).
//!
//! The watcher is a later ticket of the spec.

mod batch;
mod pay;
mod tx;

pub use batch::XrplBatchSettlement;
pub use connector_chain_rpc::RpcTransport;

#[cfg(feature = "test-util")]
pub mod test_support;
