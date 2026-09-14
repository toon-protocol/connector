//! ADR 0071 decision 6's own isolation, asserted rather than assumed
//! (issue #1291): **no chain SDK, RPC client or HTTP client appears in this
//! crate's dependencies.**
//!
//! The port boundary is what quarantines the AMM surface that self-sourcing
//! costs -- ADR 0071 rejected an external rate-keeper in its favour on
//! exactly that basis, and `connector-settlement` keeps the same isolation
//! for the same reason. The pressure is real and arrives with the first
//! reader: the moment a pool layout, a tick-math port or an `eth_call`
//! encoder is easier to put *here* than behind the port, the boundary is
//! gone and the second implementation is being written against the first.
//!
//! Read off this crate's own manifest, which is where a dependency is
//! actually added. `cargo tree` is the wider check and it is an acceptance
//! criterion of #1291; this is the one that stays true afterwards.

use std::collections::BTreeSet;

/// Every crate this port may depend on at build time, and nothing else.
/// Adding to this list is deliberate by construction -- which is the point.
const DECLARED: [&str; 4] = ["async-trait", "chrono", "connector-domain", "thiserror"];

/// Families whose presence would mean the port had grown a chain, a socket
/// or a wire of its own. Not exhaustive, and not meant to be: the
/// allow-list above is what actually holds the line. This is the message a
/// reader gets when they cross it.
const NOT_HERE: [&str; 14] = [
    "alloy",
    "ethers",
    "web3",
    "solana-",
    "anchor-",
    "spl-",
    "reqwest",
    "hyper",
    "ureq",
    "surf",
    "isahc",
    "curl",
    "jsonrpsee",
    "tonic",
];

#[test]
fn the_port_declares_no_chain_sdk_no_rpc_client_and_no_http_client() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("this crate's own manifest");
    let dependencies = declared_dependencies(&manifest);

    for forbidden in NOT_HERE {
        for dependency in &dependencies {
            assert!(
                !dependency.starts_with(forbidden),
                "'{dependency}' is a chain SDK, RPC or HTTP client: it belongs behind the \
                 RateSource port (a reader crate of its own), not in the port itself"
            );
        }
    }

    assert_eq!(
        dependencies,
        DECLARED.iter().map(|name| name.to_string()).collect(),
        "the port's dependencies changed; if the addition really belongs here rather than \
         behind the port, say so in DECLARED and in the crate's own doc"
    );
}

/// The `[dependencies]` table's keys, and only that table's -- the
/// `[dev-dependencies]` are a test build's, never a node's.
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
