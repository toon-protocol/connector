//! Solana settlement (ADR 0001, ADR 0074, ADR 0075): both halves of the
//! settlement port over solana-foundation's `payment-channels` program at
//! the one id the binary fixes ([`batch::SolanaBatchSettlement`]), the
//! public sponsor endpoint's co-signing, and the sweep that lands held
//! vouchers.
//!
//! TOON's own `SolanaSettlementBackend` over its payment-channel program,
//! and that program's wire, are deleted (ADR 0075 decision 12, issue
//! #1385).

pub mod batch;
mod submit;
#[cfg(feature = "test-util")]
pub mod test_support;

/// A settlement table's endpoint, which
/// [`SolanaBatchSettlement::connect`](batch::SolanaBatchSettlement::connect)
/// takes in place of a URL (ADR 0073). Re-exported so a caller building one
/// does not need a second dependency to name it.
pub use connector_chain_rpc::RpcTransport;

use solana_sdk::genesis_config::ClusterType;
use solana_sdk::hash::Hash;

/// The public Solana cluster whose genesis block hashes to `genesis_hash`
/// -- `"mainnet-beta"`, `"devnet"` or `"testnet"` -- and `None` for a chain
/// that is none of the three (issue #1131).
///
/// A cluster's genesis hash is the one identity a Solana chain states about
/// itself. It is not a naming convention, so it holds however the node
/// reached the chain: `api.devnet.solana.com`, a Helius or Triton URL, an
/// SSH tunnel, a caching proxy, an IP literal. That is the whole reason this
/// exists next to `SolanaSettlementConfig::cluster_hint`, which can only
/// recognise a hostname it was told about in advance and answers `None` for
/// every paid RPC provider.
///
/// The three hashes are **not** written down here. They come from
/// [`ClusterType::get_genesis_hash`], `solana-sdk`'s own table, so the
/// values track the pinned SDK rather than this repository's memory of
/// them; `cluster_names_match_the_published_genesis_hashes` in this
/// module's tests pins that table to the base58 the public RPC endpoints
/// answer with, so an SDK bump that moved a value would fail the gate
/// rather than silently relabel a chain.
///
/// # `None` is a chain this connector cannot name, not an error
///
/// `ClusterType::Development` -- a `solana-test-validator`, which mints a
/// fresh genesis on every run, and therefore every `local/` topology and
/// every tier-3 test in this workspace -- has no published hash and can
/// never have one. It answers `None`, which is exactly what `cluster_hint`
/// already answers for an unrecognised host, and means the same thing: this
/// node cannot say which cluster it is on, so it compares nothing rather
/// than guessing. Refusing here would refuse to boot on every local
/// topology.
pub fn cluster_for_genesis_hash(genesis_hash: &Hash) -> Option<&'static str> {
    [
        (ClusterType::MainnetBeta, "mainnet-beta"),
        (ClusterType::Devnet, "devnet"),
        (ClusterType::Testnet, "testnet"),
    ]
    .into_iter()
    .find(|(cluster, _)| cluster.get_genesis_hash().as_ref() == Some(genesis_hash))
    .map(|(_, name)| name)
}

/// The CAIP-2 network id for `genesis_hash`: `solana:` followed by the
/// first 32 characters of the chain's own base58-encoded genesis hash (ADR
/// 0074 decision 8, issue #1345) -- CAIP-2's own truncation rule for the
/// Solana namespace, e.g. `solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1` for
/// devnet. Unlike [`cluster_for_genesis_hash`], this never answers `None`:
/// CAIP-2 is defined over the genesis hash itself, not over whether it
/// happens to match one of the three public clusters that function
/// recognises, so a `solana-test-validator`'s fresh genesis gets a network
/// id too, just not a memorable one.
///
/// `.chars().take(32)` rather than byte-slicing: base58 is ASCII, so the two
/// agree for every real genesis hash, and this reads honestly as "the first
/// 32 characters" even in the degenerate case CAIP-2's own worked examples
/// never hit.
pub fn caip2_solana_network(genesis_hash: &Hash) -> String {
    format!(
        "solana:{}",
        genesis_hash
            .to_string()
            .chars()
            .take(32)
            .collect::<String>()
    )
}

#[cfg(test)]
mod cluster_identity_tests {
    use std::str::FromStr;

    use super::*;

    /// Issue #1131. [`cluster_for_genesis_hash`] reads its hashes from
    /// `solana-sdk`'s own [`ClusterType::get_genesis_hash`] table rather
    /// than writing them down, so this test writes them down *once*, here,
    /// and pins the table to them.
    ///
    /// The three literals are what the public endpoints themselves answer
    /// to `{"jsonrpc":"2.0","id":1,"method":"getGenesisHash"}` -- verified
    /// against `api.mainnet-beta.solana.com`, `api.devnet.solana.com` and
    /// `api.testnet.solana.com` on 2026-08-24, agreeing exactly with the
    /// pinned `solana-sdk =2.1.0`. Without this, a future SDK bump that
    /// moved a hash would silently relabel a chain -- the very failure
    /// issue #975 exists to stop -- instead of failing the gate.
    #[test]
    fn cluster_names_match_the_published_genesis_hashes() {
        for (genesis_hash, expected) in [
            (
                "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d",
                "mainnet-beta",
            ),
            ("EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG", "devnet"),
            ("4uhcVJyU9pJkvQyS88uRDiswHXSCkY3zQawwpjk2NsNY", "testnet"),
        ] {
            let hash = Hash::from_str(genesis_hash).expect("a published genesis hash is base58");
            assert_eq!(
                cluster_for_genesis_hash(&hash),
                Some(expected),
                "{genesis_hash} is {expected}'s published genesis hash"
            );
        }
    }

    /// The `solana-test-validator` case, stated without needing one: a
    /// genesis hash no public cluster published names no cluster, rather
    /// than being forced into the nearest one. Every `local/` topology and
    /// every tier-3 test in this workspace lands here, so this is the
    /// branch that keeps `make local-verify` booting.
    #[test]
    fn a_genesis_hash_no_public_cluster_published_names_no_cluster() {
        assert_eq!(cluster_for_genesis_hash(&Hash::new_unique()), None);
        assert_eq!(cluster_for_genesis_hash(&Hash::default()), None);
    }

    /// ADR 0074 decision 8, issue #1345: CAIP-2's Solana namespace is the
    /// first 32 base58 characters of the genesis hash, checked against the
    /// same published mainnet and devnet hashes
    /// [`cluster_names_match_the_published_genesis_hashes`] pins.
    #[test]
    fn caip2_network_is_the_first_32_base58_characters_of_the_genesis_hash() {
        let mainnet = Hash::from_str("5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d").unwrap();
        assert_eq!(
            caip2_solana_network(&mainnet),
            "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"
        );

        let devnet = Hash::from_str("EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG").unwrap();
        assert_eq!(
            caip2_solana_network(&devnet),
            "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1"
        );
    }

    /// Unlike [`cluster_for_genesis_hash`], a chain no public cluster
    /// published still gets a real network id -- CAIP-2 is defined over the
    /// hash itself, and every `solana-test-validator` this workspace's own
    /// tests and `local/` topologies run on is exactly this case.
    #[test]
    fn caip2_network_names_an_unrecognised_chain_too() {
        let network = caip2_solana_network(&Hash::new_unique());
        assert!(
            network.starts_with("solana:") && network.len() > "solana:".len(),
            "a chain no public cluster published must still get a real network id, got {network}"
        );
    }
}
