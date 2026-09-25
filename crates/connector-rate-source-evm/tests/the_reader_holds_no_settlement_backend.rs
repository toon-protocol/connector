//! ADR 0071 decision 6's other half, asserted rather than assumed (issue
//! #1293): the first `RateSource` reads **over** a settlement backend's
//! existing RPC endpoint, and **the backend itself remains no part of any
//! value path**.
//!
//! That is a claim about a dependency edge, and the pressure on it is obvious
//! the moment someone wants a pool address resolved against a `TokenNetwork`,
//! or a rate written straight into a claim. A reader that held a settlement
//! backend could sign; this one cannot, because it does not have one. The
//! crate's whole input is an RPC URL.
//!
//! The mirror of `connector-rate-source`'s own
//! `no_chain_sdk_reaches_the_port`, and read the same way: off this crate's
//! own manifest, which is where a dependency is actually added.

use std::collections::BTreeSet;

/// Everything this reader may depend on at build time, and nothing else.
/// Adding to this list is deliberate by construction -- which is the point.
const DECLARED: [&str; 7] = [
    "connector-chain-rpc",
    "connector-domain",
    "connector-rate-source",
    "async-trait",
    "chrono",
    "ethers",
    "thiserror",
];

/// Prefixes that would mean this reader had grown a way to move money: a
/// settlement backend, the port they implement, or a signer. `ethers` is here
/// deliberately and is *not* one of these -- an RPC client that can only
/// `eth_call` is what reading a pool costs, and the key that would turn a call
/// into a transaction lives in a crate this one cannot see.
const NOT_HERE: [&str; 4] = [
    "connector-settlement",
    "connector-signer",
    "connector-operator",
    "connector-runtime",
];

#[test]
fn the_reader_declares_no_settlement_backend_and_no_signer() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("this crate's own manifest");
    let dependencies = declared_dependencies(&manifest);

    for forbidden in NOT_HERE {
        for dependency in &dependencies {
            assert!(
                !dependency.starts_with(forbidden),
                "'{dependency}' can settle or sign: a rate source reads a market over an RPC \
                 endpoint and holds no part of any value path (ADR 0071 decision 6)"
            );
        }
    }

    assert_eq!(
        dependencies,
        DECLARED.iter().map(|name| name.to_string()).collect(),
        "this reader's dependencies changed; if the addition really belongs in a rate source, \
         say so in DECLARED and in the crate's own doc"
    );
}

/// The `[dependencies]` table's keys, and only that table's. The
/// `[dev-dependencies]` are a test build's -- the disposable-`anvil` harness
/// this crate's tier-3 tests borrow from `connector-settlement-evm` lives
/// there, and a harness that spawns a chain is not a backend that settles on
/// one.
fn declared_dependencies(manifest: &str) -> BTreeSet<String> {
    manifest
        .lines()
        .skip_while(|line| line.trim() != "[dependencies]")
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with('['))
        .filter_map(|line| line.split_once(['=', ' ']))
        .map(|(name, _)| name.trim().to_string())
        .filter(|name| !name.is_empty() && !name.starts_with('#'))
        .collect()
}
