//! The one way this crate builds an `eth_getLogs` query: [`scoped_event`].
//!
//! # Why every query goes through it
//!
//! `Contract::event::<D>()` builds `D::new(Filter::new(), client)`
//! (ethers-contract 2.0.14, `src/contract.rs:314`) -- a bare filter carrying
//! only the event's topic0 and whatever block range is chained onto it. Its
//! two siblings, `event_with_filter` and `event_for_name`, both
//! `.address(self.address)` on the way through; `event` is the one that does
//! not, and every abigen-generated `*_filter()` helper is `event` underneath.
//! So a query built either way names no contract at all and asks for every
//! log of that shape on the chain, although the binding it came from knows
//! its address.
//!
//! An unrestricted `eth_getLogs` is a request many public RPC providers
//! refuse outright rather than serve. The devnet boxes point
//! `[settlement.evm].rpc_url` at `https://base-sepolia-rpc.publicnode.com`,
//! which answers `-32701 Please specify an address in your request`; `anvil`
//! serves it, so nothing in the test gate notices. It has shipped twice: the
//! channel-index syncer's first range failed on every attempt (#970, verified
//! 2026-08-14 -- the retry warning was ~99.99% of 100,000 lines of connector
//! output) and the batch-settlement withdrawal watch never advanced its
//! cursor, so a payer's `withdraw` was never answered by a claim (#1367).
//!
//! Scoping a query to the contract that emits the event is also simply
//! correct -- it is the only contract whose logs any reader here wants --
//! and it makes the query cheaper on providers that would have served the
//! wide one. `tests/log_queries_are_scoped.rs` fails the build on a bare
//! `event::<D>()` or a generated `*_filter()` anywhere else under `src/`.

use std::sync::Arc;

use ethers::contract::{Contract, EthEvent, Event};
use ethers::providers::Middleware;
use ethers::types::ValueOrArray;

/// An [`Event`] query for `D`'s logs emitted by `contract` itself: topic0 is
/// `D`'s signature and `address` is the contract's own. Chain a block range
/// onto it and query.
pub(crate) fn scoped_event<M: Middleware, D: EthEvent>(
    contract: &Contract<M>,
) -> Event<Arc<M>, M, D> {
    contract
        .event::<D>()
        .address(ValueOrArray::Value(contract.address()))
}
