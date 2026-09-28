//! The chain-agnostic settlement backend port (ADR 0001, ADR 0002, ADR
//! 0006): what opening, funding, closing and redeeming a payment channel
//! mean, independent of any chain.
//!
//! [`SettlementBackend`] is the port; [`contract`] is the one contract
//! suite that defines it (ADR 0007); [`InMemorySettlementBackend`] is the
//! first implementation to pass that suite. `connector-settlement-evm` and
//! `connector-settlement-solana` (issue #459 and its Solana counterpart)
//! hold their real, chain-backed implementations to the same suite,
//! unmodified.
//!
//! No chain SDK, RPC client or transaction appears in this crate -- that is
//! deliberately out of scope here (issue #458) and belongs to the two
//! settlement crates above instead.
//!
//! [`batch`] is a second port beside it, for x402 `batch-settlement`
//! channels (ADR 0074 decision 9), with a receiving half and a paying half
//! (ADR 0075 decision 2). It grows into the only port: `SettlementBackend`
//! and its implementations are deleted once nothing calls them (issue
//! #1385).

pub mod batch;
mod in_memory;
mod port;

pub use in_memory::InMemorySettlementBackend;
pub use port::{ChannelId, ChannelState, ChannelStatus, Claim, SettlementBackend, SettlementError};

#[cfg(any(test, feature = "test-util"))]
pub mod contract;
