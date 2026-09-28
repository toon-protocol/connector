//! The settlement port for x402 `batch-settlement` channels (ADR 0074
//! decision 9, ADR 0075 decision 2), in two halves. As the **receiver**
//! ([`BatchSettlementBackend`]) this node admits a channel a payer opened
//! toward it, reads it and lands the payer's vouchers on it. As the
//! **payer** ([`BatchSettlementPayer`]) it opens and funds a channel toward
//! a counterparty, signs vouchers on it and withdraws what was not landed.
//!
//! [`contract`] holds each half to its one suite (ADR 0007);
//! [`InMemoryBatchSettlement`] is the fake that implements both and passes
//! both. [`HeldVouchers`] is where the watchers and sweeps (issue #1344)
//! read the latest voucher on each channel before they land it.
//! The chain implementations are modules of `connector-settlement-evm` and
//! `connector-settlement-solana`, held to the same suites.
//!
//! A separate port from [`SettlementBackend`](crate::SettlementBackend), and
//! deliberately so: see [`BatchSettlementBackend`]'s own documentation.

mod held;
mod in_memory;
mod port;

pub use held::{HeldVoucher, HeldVouchers};

pub use in_memory::{ChannelTerms, InMemoryBatchChain, InMemoryBatchSettlement, PayerExit};
pub use port::{
    AdmissionRefusal, BatchChannelState, BatchChannelStatus, BatchSettlementBackend,
    BatchSettlementError, BatchSettlementPayer, ChannelPresentation, EvmChannelConfig,
    EvmReceiverTerms, OpenedChannel, OutboundChannelRecord, OutboundChannelState, ReceiverTerms,
    SolanaReceiverTerms, Voucher, VoucherSigner,
};

#[cfg(any(test, feature = "test-util"))]
pub mod contract;
