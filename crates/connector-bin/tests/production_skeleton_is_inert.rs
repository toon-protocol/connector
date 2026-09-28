//! The production configuration skeleton must stay a skeleton.
//! [ADR 0056](../../../docs/adr/0056-production-is-a-named-empty-tier.md).
//!
//! `deploy/connector-rust/connector.production.toml` names a tier that does
//! not exist: no machine, no mainnet contract, no key, no deploy. Its whole
//! value is that the questions are written down somewhere they will be found.
//! Its whole risk is the opposite — that somebody treats it as a template with
//! blanks and fills one in, and a file that loads is a file that can be
//! `docker run`.
//!
//! Two things could turn it into something real without anyone deciding to:
//!
//! * a placeholder replaced with a plausible value, one at a time, until the
//!   file loads;
//! * a devnet address copied in "to have something valid there" — which is the
//!   worse half, because a devnet registry or the devnet payment-channel
//!   program under a mainnet RPC produces a node that takes money for claims
//!   it can never redeem (ADR 0053 binds the settlement program into a Solana
//!   claim's signed message, so the program id is part of what was signed).
//!
//! A separate harness from `fleet_release_gate.rs` on purpose, for the reason
//! that file gives about itself: this asserts a property of a committed
//! *skeleton*, that one asserts a property of the committed *pipeline*, and
//! the two are edited by different work.

use std::path::{Path, PathBuf};

fn production_skeleton_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../deploy/connector-rust/connector.production.toml")
}

/// The load must FAIL. ADR 0009 makes the connector refuse to start on an
/// invalid config and say why, and this file is invalid on purpose so that
/// refusal is what a stray `docker run` gets.
///
/// If this test ever fails, do not "fix" the file back to invalid without
/// reading why it became valid. A skeleton that loads means somebody has
/// begun standing up a production node, and ADR 0056's open questions —
/// custody of the signer, journal durability, who may reach the client edge —
/// are the checklist that was skipped, not the config.
#[test]
fn the_production_skeleton_refuses_to_load() {
    let path = production_skeleton_path();
    let result = connector_config::Config::load(&path);

    if let Err(e) = &result {
        println!("refused as committed with: {e}");
    }
    assert!(
        result.is_err(),
        "deploy/connector-rust/connector.production.toml LOADED. It is a \
         skeleton for a tier that does not exist (no machines, no mainnet \
         contracts, no keys — ADR 0056) and every value in it is invalid on \
         purpose, so that it cannot be run by accident. Something filled a \
         placeholder in. See the file's own header for what is blocked and \
         why: there is no mainnet TokenNetworkRegistry deployed, and the \
         Solana payment-channel program exists on devnet only."
    );
}

/// And it must still refuse once the key files exist.
///
/// The case above passes on the shallowest possible ground: `Config::load`
/// checks `[signer] key_file` early and the skeleton names a path nothing has.
/// That alone would be a weak guarantee — the first person to mount a key
/// would discover the rest of the file is only one address away from booting,
/// which is exactly the incremental slide ADR 0056 is written against.
///
/// So substitute real key material at all three `key_file` paths — the same
/// substitution `devnet_configs_load.rs` makes when it boots the fleet's own
/// committed configs, and for the same reason — and require the file to be
/// refused anyway, on something semantic.
#[test]
fn the_production_skeleton_refuses_to_load_even_with_key_files_present() {
    let raw = std::fs::read_to_string(production_skeleton_path())
        .expect("deploy/connector-rust/connector.production.toml must exist — ADR 0056");

    let dir = tempfile::tempdir().expect("tempdir");
    let key_path = dir.path().join("substituted.key");
    // A valid secp256k1 scalar, so the refusal below cannot be "that is not a
    // key" wearing a different hat.
    std::fs::write(&key_path, "11".repeat(32)).expect("write key");

    let substituted = raw.replace(
        "/nonexistent/production-signer-key-does-not-exist",
        key_path.to_str().expect("utf-8 temp path"),
    );
    let substituted = substituted.replace(
        "/nonexistent/production-evm-settlement-key-does-not-exist",
        key_path.to_str().expect("utf-8 temp path"),
    );
    let substituted = substituted.replace(
        "/nonexistent/production-solana-settlement-key-does-not-exist",
        key_path.to_str().expect("utf-8 temp path"),
    );
    assert!(
        !substituted.contains("/nonexistent/"),
        "the skeleton's key_file placeholders were renamed. Update the three \
         literals in this test, or it silently stops substituting anything and \
         re-asserts the shallow case above."
    );

    let config_path = dir.path().join("connector.production.toml");
    std::fs::write(&config_path, substituted).expect("write substituted config");

    let result = connector_config::Config::load(&config_path);
    if let Err(e) = &result {
        println!("refused with keys present: {e}");
    }
    assert!(
        result.is_err(),
        "deploy/connector-rust/connector.production.toml loads once key files \
         exist. The only thing standing between the skeleton and a runnable \
         production node was then a mounted key — which is the incremental \
         slide ADR 0056 exists to refuse. Its operator token, its write key \
         and both settlement sections must all stay invalid: there is no \
         mainnet TokenNetworkRegistry, and the Solana payment-channel program \
         is devnet-only."
    );
}

/// No real EVM address may appear in it.
///
/// The header explains the block: no mainnet token has been chosen for this
/// tier, so `token_address` has no correct value in existence. The failure
/// mode this guards is not "the file loads" (the case above covers that) but
/// "somebody pasted a devnet address in to make it look finished". A connector
/// pointed at a devnet token under a mainnet RPC settles in a token nobody
/// chose. The x402 contract is a constant of the binary (ADR 0075), so no
/// address of it belongs here either.
///
/// Comments are scanned too, deliberately. A commented-out real address is a
/// value one keystroke from being live, and this file is read by whoever
/// eventually does stand the tier up.
#[test]
fn the_production_skeleton_names_no_real_evm_address() {
    let raw = std::fs::read_to_string(production_skeleton_path())
        .expect("deploy/connector-rust/connector.production.toml must exist — ADR 0056");

    for (i, line) in raw.lines().enumerate() {
        let mut rest = line;
        while let Some(at) = rest.find("0x") {
            let candidate: String = rest[at + 2..].chars().take(40).collect();
            assert!(
                !(candidate.len() == 40 && candidate.chars().all(|c| c.is_ascii_hexdigit())),
                "deploy/connector-rust/connector.production.toml line {} contains what looks \
                 like a real EVM address (`0x{candidate}`). No mainnet \
                 token has been chosen for this tier, so there is no correct value for one \
                 here, and a devnet address copied across is worse than an invalid \
                 placeholder. See ADR 0056.",
                i + 1
            );
            rest = &rest[at + 2..];
        }
    }
}

/// The devnet Solana payment-channel program id, which is the one address a
/// reader of this file is most likely to reach for: it is the only deployed
/// program there is, it is written in the file's own header as the reason the
/// Solana section is blocked, and it is three lines from where it would have
/// to be pasted.
///
/// It may appear in prose. It may not appear as a `program_id` value. ADR 0053
/// binds the settlement program into a Solana claim's signed message, so a
/// mainnet-pointed node naming the devnet program is not merely misconfigured
/// — it advertises a domain no counterparty can settle against, after taking
/// the money.
const DEVNET_SOLANA_PROGRAM_ID: &str = "2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip";

#[test]
fn the_production_skeleton_never_sets_the_devnet_solana_program_as_a_value() {
    let raw = std::fs::read_to_string(production_skeleton_path())
        .expect("deploy/connector-rust/connector.production.toml must exist — ADR 0056");

    for (i, line) in raw.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            // Prose. The header cites this id precisely to say it is
            // devnet-only; that citation is the point of the file.
            continue;
        }
        assert!(
            !line.contains(DEVNET_SOLANA_PROGRAM_ID),
            "deploy/connector-rust/connector.production.toml line {} sets the DEVNET \
             payment-channel program `{DEVNET_SOLANA_PROGRAM_ID}` as a config value. That \
             program is deployed on Solana devnet and nowhere else. ADR 0053 binds the \
             settlement program into a claim's signed message, so a production node naming \
             it would accept claims against a domain that does not exist on the chain it \
             settles on. Cite it in a comment if you must; do not set it.",
            i + 1
        );
    }
}
