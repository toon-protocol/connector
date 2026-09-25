//! The receive-only settlement port for x402 `batch-settlement` channels
//! (ADR 0074 decision 9): a client opens and funds the channel, this node
//! only admits it, reads it and lands the client's vouchers on it.
//!
//! [`BatchSettlementBackend`] is the port; [`contract`] is its one contract
//! suite (ADR 0007); [`InMemoryBatchSettlement`] is the fake that passes it.
//! [`HeldVouchers`] is where the watchers and sweeps (issue #1344) read the
//! latest voucher on each channel before they land it.
//! The chain implementations are modules of `connector-settlement-evm` and
//! `connector-settlement-solana`, held to the same suite.
//!
//! A separate port from [`SettlementBackend`](crate::SettlementBackend), and
//! deliberately so: see [`BatchSettlementBackend`]'s own documentation.

mod held;
mod in_memory;
mod port;

pub use held::{HeldVoucher, HeldVouchers};

pub use in_memory::{ChannelTerms, InMemoryBatchSettlement, OpenedChannel, PayerExit};
pub use port::{
    AdmissionRefusal, BatchChannelState, BatchChannelStatus, BatchSettlementBackend,
    BatchSettlementError, ChannelPresentation, EvmChannelConfig, Voucher, VoucherSigner,
};

#[cfg(any(test, feature = "test-util"))]
pub mod contract;
