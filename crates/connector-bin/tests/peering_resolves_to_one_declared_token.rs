//! Which token a peering is denominated in, and the two ways a declaring
//! node's answer stops it at boot
//! ([ADR 0071](../../../docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)
//! decision 1, issue #1292).
//!
//! A packet's amount has no unit of its own -- it is denominated by the
//! channel it rides -- so "is this forward a conversion" is really "do
//! these two peerings hold different tokens". Nothing in this connector
//! could answer that before: an EVM `[[peer_channels]]` row names a
//! per-token `TokenNetwork` whose token only a chain read would give up,
//! and a Solana row names no token at all. The answer comes instead from
//! the `[settlement.<chain>]` table those channels settle through, which
//! already states it -- so resolution is a pure function of loaded config,
//! with no chain read and no I/O, which is what lets the forwarding path
//! ask it on every packet.
//!
//! The refusals are black-box, against the compiled binary, for the reason
//! `refuses_to_start.rs` and `declared_rates_refuse_by_name.rs` are: ADR
//! 0009's claim is that a bad declaration stops the **process**, and each
//! one asserts on the words an operator reads on stderr rather than merely
//! on a non-zero exit. The positive cases go through `Config::load`,
//! because these configs carry a `[settlement.evm]` table and a serving
//! node would dial a chain for it -- what is under test is the resolution,
//! and a loaded `Config` is the whole of it.
//!
//! # The rule that protects everyone not doing any of this
//!
//! A node that declares no `[[tokens]]` resolves no peering, gains no
//! required key and no new refusal, and loads exactly as it did before ADR
//! 0071. [`a_peering_config_that_declares_no_tokens_resolves_nothing`] is
//! that as a unit; `local_topologies_load.rs` and `devnet_configs_load.rs`
//! are it over every config this repository ships, and neither needed a
//! line changed for this work.

use std::io::Write;
use std::path::Path;
use std::process::Command;

use connector_config::Config;

/// The ERC-20 `[settlement.evm]` names below, spelled as a `[[tokens]]` row
/// names one. Checksummed on purpose: a config file is where an explorer's
/// spelling gets pasted, and an `AssetId` reads it and the lowercase one as
/// one token.
const SETTLEMENT_TOKEN: &str = "evm:0x49beE1Bca5d15Fb0963117923403F9498119a9Ce";
/// USDC on Base -- some other ERC-20, declared by a node whose peerings do
/// not hold it.
const OTHER_TOKEN: &str = "evm:0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
/// USDC on Solana: the mint `[settlement.solana]` names below, and the
/// other side of a two-chain node's boundary.
const SOLANA_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

const PEER_CHANNEL: &str = "0xaaaabbbbccccddddeeeeffff00001111aaaabbbbccccddddeeeeffff00001111";
const PEER_KEY: &str = "0x2222222222222222222222222222222222222222";
const PEER_TOKEN_NETWORK: &str = "0x3333333333333333333333333333333333333333";
const SOLANA_CHANNEL_ACCOUNT: &str = "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi";
const SOLANA_COUNTERPARTY_KEY: &str = "8pM1DN3RiT8vbom5u1sNryaNT1nyL8CTTW3b5PwWXRBH";
const SOLANA_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

fn run(config_path: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_connector"))
        .arg(config_path)
        .output()
        .expect("run connector binary")
}

fn write_config(text: &str) -> tempfile::NamedTempFile {
    let mut config_file = tempfile::NamedTempFile::new().expect("temp config file");
    write!(config_file, "{text}").expect("write config file");
    config_file
}

fn write_raw_key_file() -> tempfile::NamedTempFile {
    let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
    key_file
        .write_all(&[7u8; 32])
        .expect("write raw 32-byte key");
    key_file
}

/// One `[[tokens]]` row per asset and nothing else -- no rate and no quote,
/// so no `[rate_guards]` table is owed (issue #1290). The smallest
/// declaration that turns this rule on, and exactly what a node crossing
/// chains in one asset writes.
fn declaring(assets: &[&str]) -> String {
    assets
        .iter()
        .map(|asset| format!("\n[[tokens]]\nasset = \"{asset}\"\n"))
        .collect()
}

/// One peering, bound to an EVM channel, on a node that settles on EVM --
/// the smallest file in which a peering has a token at all. `declaration`
/// is appended.
///
/// The `[settlement.evm]` table is not decoration: since issue #1138 an EVM
/// channel row does not load without one, because that table is where this
/// node's on-chain identity comes from. It is also, since ADR 0071, where
/// the peering's token comes from.
fn one_evm_peering(key_file: &Path, state_dir: &Path, declaration: &str) -> String {
    format!(
        r#"
client_edge_addr = "127.0.0.1:0"
peer_expose = "btp"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6

[settlement.evm.key]
key_file = "{key_file}"

[[peers]]
id = "store"
endpoint = "wss://store.example:443/btp"

[[peer_channels]]
peer_id = "store"
channel_id = "{PEER_CHANNEL}"
counterparty_key = "{PEER_KEY}"
chain_id = 31337
token_network = "{PEER_TOKEN_NETWORK}"
{declaration}
"#,
        state_dir = state_dir.display(),
        key_file = key_file.display(),
    )
}

/// Two peerings on two chains: `local/mixed-chain`'s middle node in
/// miniature, and the shape a converting forward is read off. `solana_peer`
/// is which peering the Solana channel is bound to -- `"to-solana"` for the
/// two-peering shape, `"from-evm"` for the one-peering-two-chains shape
/// that has no single unit.
fn peerings_on_two_chains(
    key_file: &Path,
    state_dir: &Path,
    solana_peer: &str,
    declaration: &str,
) -> String {
    let second_peering = if solana_peer == "to-solana" {
        r#"
[[peers]]
id = "to-solana"
endpoint = "wss://solana.example:443/btp"
"#
    } else {
        ""
    };
    format!(
        r#"
client_edge_addr = "127.0.0.1:0"
peer_expose = "btp"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6

[settlement.evm.key]
key_file = "{key_file}"

[settlement.solana]
rpc_url = "https://api.devnet.solana.com"
program_id = "{SOLANA_PROGRAM_ID}"
token_address = "{SOLANA_MINT}"
decimals = 6

[settlement.solana.key]
key_file = "{key_file}"

[[peers]]
id = "from-evm"
endpoint = "wss://evm.example:443/btp"
{second_peering}
[[peer_channels]]
peer_id = "from-evm"
channel_id = "{PEER_CHANNEL}"
counterparty_key = "{PEER_KEY}"
chain_id = 31337
token_network = "{PEER_TOKEN_NETWORK}"

[[peer_channels]]
peer_id = "{solana_peer}"
channel_account = "{SOLANA_CHANNEL_ACCOUNT}"
counterparty_key = "{SOLANA_COUNTERPARTY_KEY}"
{declaration}
"#,
        state_dir = state_dir.display(),
        key_file = key_file.display(),
    )
}

/// The acceptance criterion: a config declaring tokens resolves its peering
/// to exactly one of them, out of loaded config alone. The
/// `token_network` the channel row names is never consulted and no chain is
/// asked -- the token is the one `[settlement.evm]` already says every
/// channel it opens settles in.
#[test]
fn a_declaring_node_resolves_its_peering_to_one_declared_token() {
    let key_file = write_raw_key_file();
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let config_file = write_config(&one_evm_peering(
        key_file.path(),
        state_dir.path(),
        &declaring(&[SETTLEMENT_TOKEN]),
    ));

    let config = Config::load(config_file.path()).expect("a declaring config must load");
    let resolved = config.peering_assets();

    assert!(!resolved.is_empty());
    assert_eq!(
        resolved.asset("store").map(ToString::to_string),
        Some(SETTLEMENT_TOKEN.to_ascii_lowercase()),
        "the peering holds what its settlement table settles in, however the row spelled it"
    );
    // A peering against itself is not a denomination boundary, whatever it
    // holds: a forward whose two legs hold one token takes the flat fee and
    // nothing else.
    assert_eq!(resolved.boundary_between("store", "store"), None);
}

/// The ordered pair the converting forward asks for (issue #1295), on the
/// shape it asks it of: two peerings on two chains hold two tokens, and the
/// answer comes back in the order asked. Direction is the trade -- the
/// reverse pair is a different price from the same mid.
#[test]
fn two_peerings_on_two_chains_answer_with_the_ordered_pair_they_hold() {
    let key_file = write_raw_key_file();
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let solana_token = format!("solana:{SOLANA_MINT}");
    let config_file = write_config(&peerings_on_two_chains(
        key_file.path(),
        state_dir.path(),
        "to-solana",
        &declaring(&[SETTLEMENT_TOKEN, &solana_token]),
    ));

    let config = Config::load(config_file.path()).expect("a declaring config must load");
    let resolved = config.peering_assets();

    let forward = resolved
        .boundary_between("from-evm", "to-solana")
        .expect("two peerings on two chains hold two tokens");
    assert_eq!(
        (forward.0.to_string(), forward.1.to_string()),
        (SETTLEMENT_TOKEN.to_ascii_lowercase(), solana_token.clone())
    );

    let back = resolved
        .boundary_between("to-solana", "from-evm")
        .expect("and the reverse pair is the reverse pair");
    assert_eq!(
        (back.0.to_string(), back.1.to_string()),
        (solana_token, SETTLEMENT_TOKEN.to_ascii_lowercase())
    );
}

/// The rule that protects every node not doing any of this: the same
/// peering file, with no `[[tokens]]`, loads and resolves nothing. No new
/// required key, no new refusal, and nothing for a forward to read -- which
/// is what "behaves exactly as it did before" means at this layer.
#[test]
fn a_peering_config_that_declares_no_tokens_resolves_nothing() {
    let key_file = write_raw_key_file();
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let config_file = write_config(&one_evm_peering(key_file.path(), state_dir.path(), ""));

    let config = Config::load(config_file.path()).expect("a non-declaring config must load");

    assert!(config.peering_assets().is_empty());
    assert_eq!(config.peering_assets().asset("store"), None);
    assert_eq!(
        config.peering_assets().boundary_between("store", "store"),
        None
    );
}

/// Not even when the file is one a declaring node would be refused for: a
/// peering whose channels sit on two chains loads exactly as it always has
/// while nothing is declared. The ambiguity is only a problem for a node
/// that has to name a unit, and a node that deals nothing never does.
#[test]
fn a_two_chain_peering_still_loads_when_nothing_is_declared() {
    let key_file = write_raw_key_file();
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let config_file = write_config(&peerings_on_two_chains(
        key_file.path(),
        state_dir.path(),
        "from-evm",
        "",
    ));

    let config = Config::load(config_file.path()).expect("a non-declaring config must load");

    assert!(config.peering_assets().is_empty());
}

/// A node that does deal is held to the rule: a peering holding a token it
/// never declared stops the process, naming the peering and the token, so
/// the operator reads it at boot rather than discovering mid-packet that
/// the hop cannot say what unit it is carrying -- by which time the packet
/// is paid for (ADR 0042).
#[test]
fn exits_non_zero_on_a_peering_holding_an_undeclared_token() {
    let key_file = write_raw_key_file();
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let config_file = write_config(&one_evm_peering(
        key_file.path(),
        state_dir.path(),
        &declaring(&[OTHER_TOKEN]),
    ));

    let output = run(config_file.path());

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("peering 'store'")
            && stderr.contains(&SETTLEMENT_TOKEN.to_ascii_lowercase())
            && stderr.contains("[[tokens]]"),
        "expected the refusal to name the peering and the token it holds, got: {stderr}"
    );
}

/// The other way a declaring node fails to resolve exactly one token: one
/// peering whose `[[peer_channels]]` rows sit on two chains, and therefore
/// in two tokens. A packet's amount is denominated by the channel it rides,
/// so a peering riding two has no unit for a forward to convert into.
#[test]
fn exits_non_zero_on_a_peering_whose_channels_sit_on_two_chains() {
    let key_file = write_raw_key_file();
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let solana_token = format!("solana:{SOLANA_MINT}");
    let config_file = write_config(&peerings_on_two_chains(
        key_file.path(),
        state_dir.path(),
        "from-evm",
        &declaring(&[SETTLEMENT_TOKEN, &solana_token]),
    ));

    let output = run(config_file.path());

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("peering 'from-evm'")
            && stderr.contains("two tokens")
            && stderr.contains(SOLANA_MINT),
        "expected the refusal to name the peering and both tokens, got: {stderr}"
    );
}
