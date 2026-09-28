//! The chain-agnostic settlement port (ADR 0001, ADR 0006, ADR 0075
//! decision 2): what opening, funding, signing on, landing and withdrawing
//! an x402 `batch-settlement` channel mean, independent of any chain.
//!
//! [`batch`] is the port, with a receiving half and a paying half, its
//! contract suite (ADR 0007) and the in-memory fake every higher-level test
//! runs over. `connector-settlement-evm` and `connector-settlement-solana`
//! hold their chain-backed implementations to the same suite, unmodified.
//!
//! No chain SDK, RPC client or transaction appears in this crate; they
//! belong to the two settlement crates above.
//!
//! TOON's own two-sided channel port, `SettlementBackend`, and its EVM,
//! Solana and in-memory implementations are deleted (ADR 0075, issue
//! #1385): every channel is an x402 channel.

pub mod batch;

/// A payment channel's identifier, opaque to everything above this port:
/// the canonical `evm:0x…` or `solana:…` key of an x402 channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChannelId(pub String);

impl std::fmt::Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
