//! The chain-independent rate-source port (ADR 0071, decision 6): what
//! reading a token's price off a market means, independent of any chain,
//! any venue and any RPC.
//!
//! [`RateSource`] is the port; [`contract`] is the one contract suite that
//! defines it (ADR 0007); [`InMemoryRateSource`] is the first
//! implementation to pass that suite. The Uniswap-v3-compatible `observe()`
//! TWAP reader (issue #1293) and, after it, a Solana reader (ADR 0071 names
//! Raydium CLMM's `ObservationState` as the candidate,
//! `docs/research/token-pair-price-sources.md`) hold their real,
//! chain-backed implementations to the same suite, unmodified.
//!
//! A quote is what ADR 0071's decision 3 declares: a path of one or two
//! **operator-named** pools ending at the numeraire, each leg read as a
//! **TWAP over a window the operator set**, the legs composing. The
//! operator names the pool -- there is no discovery here, and no spot read
//! anywhere: a pool this port cannot be told about does not exist, and a
//! ratio read inside one block is not a rate.
//!
//! **No chain SDK, RPC client, HTTP client or transaction appears in this
//! crate**, and none should. That isolation is the whole point of the port
//! boundary: ADR 0071 rejected an external rate-keeper in favour of
//! self-sourcing precisely because this boundary quarantines the AMM
//! surface that self-sourcing costs. Pool layouts, tick math, `eth_call`
//! encoding and account deserialisation all live behind it, in the
//! implementation crates.
//!
//! Nothing here is on the forwarding path either. A packet never waits on
//! this port: a background poller (issue #1294) drives a source and writes
//! what it observes into the rate table, and forwarding only ever reads
//! that table.

mod in_memory;
mod port;

pub use in_memory::{InMemoryRateSource, PoolContents};
pub use port::{
    compose_rates, LegObservation, PoolId, QuoteLeg, QuoteObservation, QuotePath, QuotePathError,
    RateSource, RateSourceError,
};

#[cfg(any(test, feature = "test-util"))]
pub mod contract;
