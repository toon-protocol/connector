//! Every committed config's declared EIP-712 domain, held against the
//! deployment the node it configures will actually settle through (issue
//! #1136) -- offline, before anyone waits for a chain to say so.
//!
//! # What this is the offline half of
//!
//! `[[peer_channels]]`, `[[pay_channels]]` and `[[client_channels]]` each
//! declare a `chain_id` and a `TokenNetwork`. Together those are the EIP-712
//! domain (ADR 0024) a claim on that channel is signed and verified under.
//! Until #1136 nothing compared either to the contract the node redeems
//! through -- `[settlement.evm]` names a `TokenNetworkRegistry`, not a
//! `TokenNetwork`, and the verifying contract is whatever
//! `getTokenNetwork(token_address)` answers on connect.
//!
//! `connector_cli::runtime`'s `check_evm_channel_domains` closes that at
//! boot, against the live chain. This file closes the part of it a chain is
//! not needed for, and which a boot refusal would only tell you about after
//! a deploy:
//!
//! 1. **One node, one domain.** A node resolves exactly one `TokenNetwork`
//!    from its one `[settlement.evm]` table, so two different declared
//!    domains in one file guarantee at least one row will refuse to boot.
//!    That is checkable with no chain at all, in any config, and it is the
//!    reason the declaration is kept and corroborated rather than derived
//!    from the backend: a domain that exists only after an RPC dial cannot
//!    be gate-checked here.
//!
//! (It used to close a second part too: the `local/` topologies hardcoded the
//! `TokenNetwork` their anvil deployed, and a test here held them to it. No
//! local config declares a channel domain any more -- every local channel is
//! an x402 channel, established by `POST /peers` (ADR 0075, issue #1383) --
//! so that test asserted nothing and was deleted, as its own message asked.)
//!
//! Deliberately parsed as raw TOML rather than through `Config::load`: this
//! is a drift gate over what the *files say*, it needs none of the key
//! material and container paths a real load demands, and it keeps working if
//! the typed shape of those tables changes.

use std::collections::BTreeSet;

/// Every committed config that declares a channel table, with the path a
/// failure should name. Written out rather than globbed for the reason
/// `local_topologies_load.rs` gives for its own list: a new topology that
/// forgets its own test still has to touch this one.
const EVERY_CONFIG: &[(&str, &str)] = &[
    (
        "infra/linode-relay/connector-rust.toml",
        include_str!("../../../infra/linode-relay/connector-rust.toml"),
    ),
    (
        "infra/linode-store/connector-rust.toml",
        include_str!("../../../infra/linode-store/connector-rust.toml"),
    ),
];

/// One declared EIP-712 domain, in the spelling a comparison can use: the
/// address lowercased, because EIP-55 checksum casing is presentation and
/// two files may legitimately differ on it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DeclaredDomain {
    chain_id: i64,
    token_network: String,
    /// Where it was written, so a failure can name the row rather than the
    /// file.
    site: String,
}

/// Every EVM domain a config declares, across all three channel tables.
/// A Solana row declares no `chain_id`/`token_network` at all and is skipped
/// by construction -- its signed message binds the settlement program
/// instead (ADR 0053, issue #1134).
fn declared_domains(name: &str, raw: &str) -> Vec<DeclaredDomain> {
    let parsed: toml::Value = toml::from_str(raw).unwrap_or_else(|error| {
        panic!("{name} must be readable as TOML for this gate to mean anything: {error}")
    });
    let mut found = Vec::new();
    for (table, address_key) in [
        ("peer_channels", "token_network"),
        ("pay_channels", "token_network"),
        ("client_channels", "token_network_address"),
    ] {
        let Some(rows) = parsed.get(table).and_then(toml::Value::as_array) else {
            continue;
        };
        for (index, row) in rows.iter().enumerate() {
            let (Some(chain_id), Some(token_network)) = (
                row.get("chain_id").and_then(toml::Value::as_integer),
                row.get(address_key).and_then(toml::Value::as_str),
            ) else {
                continue;
            };
            found.push(DeclaredDomain {
                chain_id,
                token_network: token_network.to_lowercase(),
                site: format!("[[{table}]] #{index}"),
            });
        }
    }
    found
}

/// A node holds one `[settlement.evm]` table, which resolves one
/// `TokenNetwork`, so it can judge claims under exactly one EIP-712 domain.
/// Two in one file is a config where at least one row now refuses to boot
/// (issue #1136) -- and, before #1136, was a node that accepted claims under
/// one domain while redeeming through the other.
#[test]
fn no_committed_config_declares_two_evm_domains() {
    for (name, raw) in EVERY_CONFIG {
        let domains = declared_domains(name, raw);
        let distinct: BTreeSet<(i64, &str)> = domains
            .iter()
            .map(|domain| (domain.chain_id, domain.token_network.as_str()))
            .collect();
        assert!(
            distinct.len() <= 1,
            "{name} declares {} different EIP-712 domains across its channel tables, but a node \
             resolves exactly one TokenNetwork from its one [settlement.evm] table -- at least \
             one of these rows names a contract this node can never redeem through, and it \
             refuses to boot (issue #1136): {domains:#?}",
            distinct.len()
        );
    }
}
