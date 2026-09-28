//! THE record of which Solana CLI this repository installs, and why.
//!
//! Every `release.anza.xyz/<version>/install` in this repository resolves to
//! the one constant below, and the cases in this file are what makes that
//! true. GitHub Actions cannot share an `env:` across workflow files,
//! `.sandcastle/Dockerfile` and `devbox.json` cannot read one at all, and a
//! runbook a human pastes from has to spell the command out, so the literal is
//! necessarily repeated at each install site. The single source is
//! therefore this file plus the guard: the pin lives here with its reasons,
//! the consumers carry a one-line pointer back, and
//! [`every_installed_solana_cli_is_the_recorded_pin`] fails the build the
//! moment a workflow, the image or the docs name a version this file does
//! not.
//!
//! # Why one
//!
//! Until issue #1386, this repository installed two Solana CLI versions on
//! purpose: [`RUST_GATE_CLI`] wherever the program that
//! `solana-test-validator` runs was RUN, and a second, newer line wherever
//! `packages/solana-program` -- TOON's own on-chain program -- was BUILT for
//! deploy. #1386 removed `packages/solana-program` from this repository, with
//! the CI jobs and scripts that built and deployed it
//! (`tools/solana/build-sbf.sh`, `tools/solana/deploy.sh`,
//! `ci.yml`'s `solana-program` and `solana-program-reproducibility` jobs), so
//! the second pin's whole reason to exist went with it. What is left installs
//! a Solana CLI for exactly one purpose: giving `solana-test-validator` to
//! whatever spawns it.
//!
//! ## [`RUST_GATE_CLI`] -- the CLI that runs the program
//!
//! Installed by `ci.yml`'s `rust-gate`, `local-topologies.yml`,
//! `.sandcastle/Dockerfile` and `devbox.json`. All four want the same thing: a
//! `solana-test-validator` this crate's integration tier can spawn (ADR 0007 --
//! the tier spawns its own disposable chain, so the binary has to be present),
//! loading the committed `payment-channels` and p-token fixtures
//! (`crates/connector-settlement-solana/fixtures/`) into genesis rather than
//! building anything.
//!
//! Two independent reasons, either one sufficient:
//!
//! 1. **v3's `solana-test-validator` hard-requires io_uring.** Verified against
//!    the binaries: `strings` on a v3.1.12 `solana-test-validator` contains
//!    `assertion failed: io_uring_supported()` (agave `fs/src/dirs.rs`), while
//!    the 2.1 line's contains no io_uring reference at all. The agent
//!    container's seccomp profile does not permit it, so a v3 validator panics
//!    there even though the host kernel supports io_uring.
//! 2. **The workspace pins the Solana crates to `=2.1.0`** -- `solana-sdk` and
//!    `solana-rpc-client` in `crates/connector-settlement-solana/Cargo.toml`.
//!    [`the_workspace_pins_the_solana_crates_to_the_rust_gate_line`] holds the
//!    pin and the CLI together, so bumping one surfaces the other.
//!
//! To change it: those crate pins would have to move off the 2.1 line **and**
//! v3's validator would have to stop requiring io_uring (or the sandbox start
//! permitting it). One without the other is not enough.
//!
//! ## The install sites aimed at a person
//!
//! `CONTRIBUTING.md`'s chain-binary table is the one place here that tells a
//! *human* to install this CLI with a command. It was the README's until
//! issue #1173 moved it: the README is an operator's guide now, and which CLI
//! a contributor installs to run the gate is not something an operator needs.
//!
//! (`CLAUDE.md`'s "Install the Solana CLI to run the full gate locally" is a
//! second mention and is deliberately left unversioned: it is a one-line
//! orientation sentence that gives no command and points at the table for the
//! how. Pinning it would put a second copy of the same literal in a file
//! nobody installs from. If it ever grows an actual command, it becomes a
//! site and belongs in the walk like the rest.)
//!
//! The `CONTRIBUTING.md` row exists so a contributor can run
//! `connector-settlement-solana`'s integration tier, which spawns
//! `solana-test-validator`. A local gate running a different CLI than
//! `ci.yml`'s `rust-gate` is not the gate.
//! [`the_readme_tells_a_contributor_to_install_the_cli_that_runs_the_program`]
//! holds it there.
//!
//! The faucet box's runbook (`docs/operators/faucet-box-bringup.md` step 3)
//! is the other one. The faucet box's `bootstrap.sh` installs no Solana CLI,
//! so bringing that box up means a human typing an install command before
//! running `infra/linode-faucet/generate-solana-treasury.sh`, which generates
//! and airdrops the treasury key that box's Solana USDC leg spends from. That
//! box runs no validator and compiles no Rust -- the faucet service reaches
//! Solana through `@solana/web3.js` and `@solana/spl-token` inside its
//! container and never shells out to the CLI, so the version genuinely does
//! not matter there -- but a third, unexplained pin for a box that could use
//! either is exactly the drift this file exists to prevent, so it names
//! [`RUST_GATE_CLI`] too:
//! [`the_faucet_box_runbook_names_the_cli_it_tells_an_operator_to_install`]
//! holds it there.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The one Solana CLI this repository installs anywhere. See the module
/// header before changing it.
const RUST_GATE_CLI: &str = "v2.1.21";

const CI_WORKFLOW: &str = include_str!("../../../.github/workflows/ci.yml");
const LOCAL_TOPOLOGIES_WORKFLOW: &str =
    include_str!("../../../.github/workflows/local-topologies.yml");
const SANDCASTLE_DOCKERFILE: &str = include_str!("../../../.sandcastle/Dockerfile");
const DEVBOX_JSON: &str = include_str!("../../../devbox.json");
const CONTRIBUTING: &str = include_str!("../../../CONTRIBUTING.md");
const FAUCET_RUNBOOK: &str = include_str!("../../../docs/operators/faucet-box-bringup.md");
const FAUCET_TREASURY_SCRIPT: &str =
    include_str!("../../../infra/linode-faucet/generate-solana-treasury.sh");
const SETTLEMENT_MANIFEST: &str = include_str!("../Cargo.toml");

/// Installs this guard deliberately does not hold to a pin, each with the
/// reason -- the same shape as `tools/ci/check-tracked-secrets.sh`'s
/// allowlist, and for the same reason: a blanket rule with no escape hatch
/// gets deleted rather than amended.
///
/// It is EMPTY, and that is the current answer rather than the absence of
/// one. Its single entry was `infra/linode/bootstrap.sh`, which tracked
/// `release.anza.xyz/stable` while provisioning the **self-hosted chain box**
/// -- anvil, `solana-test-validator`, faucet, nginx -- a box that was deleted
/// in the public-chain cutover (`44b15bdc`, 2026-07-19, toon-meta#374). This
/// guard's own record of that ("Pinning it is a separate decision, and a
/// smaller one than deleting the box's provisioning outright") is what the
/// larger decision then took: the provisioning and its sole caller,
/// `.github/workflows/devnet-deploy.yml`, are gone, so there is no unpinned
/// install left to excuse. `infra/linode-relay/bootstrap.sh` and
/// `infra/linode-store/bootstrap.sh`, which provision the boxes that DO serve
/// devnet, install no Solana CLI at all.
///
/// The escape hatch stays because a blanket rule with no escape hatch gets
/// deleted rather than amended. An entry added here needs the same thing the
/// deleted one had: a named reason why the install decides nothing this
/// repository ships.
const UNPINNED_BY_DESIGN: &[&str] = &[];

/// This file's own path, repo-relative. The walk below skips it: the prose
/// above quotes the install URL in order to explain it, which would otherwise
/// make the record look like a consumer of itself.
const SELF: &str = "crates/connector-settlement-solana/tests/solana_cli_pins.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root is two levels above this crate")
}

/// Every `release.anza.xyz/<version>/install` in `raw`, as the `<version>`
/// segment exactly as written. A plain string scan rather than a YAML or JSON
/// parse: the consumers are two workflows, a Dockerfile, a JSON file and an
/// operator runbook, and what matters is the literal each of them hands to `sh`
/// -- or, in the runbook's case, the literal it tells a person to paste.
fn installed_versions(raw: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for tail in raw.split("release.anza.xyz/").skip(1) {
        if let Some(version) = tail.split("/install").next() {
            found.insert(version.to_string());
        }
    }
    found
}

/// Every file under the repository root that installs a Solana CLI, mapped to
/// the versions it installs. Repo-relative paths, heavy directories skipped.
///
/// A walk rather than a fixed list of `include_str!`s, because the failure this
/// guards against includes *a new consumer* -- a workflow or an image added
/// later that quietly picks a different version. A guard keyed only on the
/// files that exist today would pass while the repository disagreed with
/// itself.
fn solana_cli_installs() -> BTreeMap<String, BTreeSet<String>> {
    let root = repo_root();
    let mut installs = BTreeMap::new();
    let mut queue = vec![root.clone()];

    while let Some(dir) = queue.pop() {
        let entries = std::fs::read_dir(&dir).unwrap_or_else(|error| {
            panic!("cannot read {dir:?} while scanning for Solana CLI installs: {error}")
        });
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                if !matches!(
                    name.as_str(),
                    "target" | "node_modules" | ".git" | ".devbox" | ".claude"
                ) {
                    queue.push(path);
                }
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if bytes.len() > 512 * 1024 {
                continue;
            }
            let relative = path
                .strip_prefix(&root)
                .expect("walked paths are under the root")
                .to_string_lossy()
                .to_string();
            // This file quotes the install URL while explaining it. It is the
            // record, not a consumer of it.
            if relative == SELF {
                continue;
            }
            let versions = installed_versions(&String::from_utf8_lossy(&bytes));
            if versions.is_empty() {
                continue;
            }
            installs.insert(relative, versions);
        }
    }

    installs
}

/// The lines of `ci.yml` belonging to one top-level job, so a case can assert
/// what *that* job installs rather than what the file mentions somewhere. Jobs
/// are the only two-space-indented keys in this workflow.
fn ci_job(name: &str) -> String {
    let header = format!("\n  {name}:\n");
    let after = CI_WORKFLOW
        .split_once(&header)
        .unwrap_or_else(|| panic!("ci.yml has no job named `{name}`"))
        .1;
    let mut block = String::new();
    for line in after.lines() {
        let is_next_job = line.starts_with("  ")
            && !line.starts_with("   ")
            && !line.trim_start().starts_with('#')
            && line.trim_end().ends_with(':');
        if is_next_job {
            break;
        }
        block.push_str(line);
        block.push('\n');
    }
    block
}

#[test]
fn every_installed_solana_cli_is_the_recorded_pin() {
    let mut drifted = Vec::new();

    for (file, versions) in solana_cli_installs() {
        if UNPINNED_BY_DESIGN.contains(&file.as_str()) {
            continue;
        }
        for version in versions {
            if version != RUST_GATE_CLI {
                drifted.push(format!("{file} installs {version}"));
            }
        }
    }

    assert!(
        drifted.is_empty(),
        "a Solana CLI version in this repository is not the one this file records:\n  {}\n\nThis \
         repository installs exactly one, deliberately: {RUST_GATE_CLI}, wherever \
         solana-test-validator is RUN. Read this file's header, and if the new version really is \
         right, change the constant here and say why, so the next reader is not left guessing \
         again.",
        drifted.join("\n  ")
    );
}

#[test]
fn the_repository_installs_the_solana_cli_from_exactly_the_known_places() {
    let expected: BTreeSet<&str> = BTreeSet::from([
        ".github/workflows/ci.yml",
        ".github/workflows/local-topologies.yml",
        ".sandcastle/Dockerfile",
        // The contributor-facing copy of the pin. It was the README's until
        // issue #1173 made that file an operator's guide; the table moved
        // rather than being deleted, and moved WITH its pin, which is the
        // whole reason this set is asserted by name.
        "CONTRIBUTING.md",
        "devbox.json",
        "docs/operators/faucet-box-bringup.md",
    ]);
    let actual: BTreeSet<String> = solana_cli_installs().into_keys().collect();
    let actual: BTreeSet<&str> = actual.iter().map(String::as_str).collect();

    assert_eq!(
        actual, expected,
        "the set of files installing a Solana CLI changed. A new one is not forbidden -- it just \
         has to install {RUST_GATE_CLI} and be named here so the next drift is still visible. A \
         removed one means a consumer this file claims to cover no longer exists."
    );
}

#[test]
fn the_rust_workspace_gate_installs_the_cli_that_runs_the_program() {
    assert_eq!(
        installed_versions(&ci_job("rust-gate")),
        BTreeSet::from([RUST_GATE_CLI.to_string()]),
        "ci.yml's rust-gate job must install {RUST_GATE_CLI}. It spawns \
         solana-test-validator for connector-settlement-solana's integration tier, and the v3 \
         line's validator asserts io_uring support the sandbox does not grant."
    );
}

#[test]
fn the_container_topologies_and_the_agent_image_install_the_cli_that_runs_the_program() {
    assert_eq!(
        installed_versions(LOCAL_TOPOLOGIES_WORKFLOW),
        BTreeSet::from([RUST_GATE_CLI.to_string()]),
        "local-topologies.yml must install {RUST_GATE_CLI}, the same CLI ci.yml's rust-gate \
         installs -- it runs the shipped image against a real validator."
    );
    assert_eq!(
        installed_versions(SANDCASTLE_DOCKERFILE),
        BTreeSet::from([RUST_GATE_CLI.to_string()]),
        ".sandcastle/Dockerfile must install {RUST_GATE_CLI}, the same CLI the gate installs. An \
         agent that passes locally on a different toolchain than the gate has learned nothing."
    );
}

#[test]
fn contributing_tells_a_contributor_to_install_the_cli_that_runs_the_program() {
    assert_eq!(
        installed_versions(CONTRIBUTING),
        BTreeSet::from([RUST_GATE_CLI.to_string()]),
        "CONTRIBUTING.md's table of chain binaries the test gate needs must give a \
         {RUST_GATE_CLI} install command for solana-test-validator. That row named `Solana CLI` \
         with no version once, which sends a contributor to whatever `stable` is that day -- a v3 \
         validator whose io_uring assertion this repository's own sandbox cannot satisfy, on a \
         workspace pinned to the 2.1 crate line. A local gate on a different CLI than ci.yml's \
         rust-gate is not the gate.\n\nThe table lived in README.md until issue #1173; if it has \
         moved again, move this check with it rather than deleting it."
    );
}

#[test]
fn the_faucet_box_runbook_names_the_cli_it_tells_an_operator_to_install() {
    assert_eq!(
        installed_versions(FAUCET_RUNBOOK),
        BTreeSet::from([RUST_GATE_CLI.to_string()]),
        "docs/operators/faucet-box-bringup.md step 3 must hand the operator a {RUST_GATE_CLI} \
         install command. The faucet box's bootstrap.sh installs no Solana CLI, and that step is \
         where a human puts one on a box that then holds a devnet USDC treasury. It said `install \
         the Solana CLI` with no version once, and this guard could not see it -- prose has no \
         literal to walk. Write the command, not the instruction."
    );
    assert!(
        FAUCET_TREASURY_SCRIPT.contains("docs/operators/faucet-box-bringup.md step 3 names"),
        "infra/linode-faucet/generate-solana-treasury.sh no longer points at \
         docs/operators/faucet-box-bringup.md step 3 for which Solana CLI to install. It \
         deliberately carries no version literal of its own so the two cannot drift; a pointer \
         that stops resolving turns back into the unversioned `install it on the box` this case \
         exists to prevent."
    );
}

#[test]
fn devbox_installs_the_cli_that_runs_the_program() {
    let bare = RUST_GATE_CLI.trim_start_matches('v');
    assert_eq!(
        installed_versions(DEVBOX_JSON),
        BTreeSet::from([RUST_GATE_CLI.to_string()]),
        "devbox.json's init_hook must install {RUST_GATE_CLI} -- a devbox shell is where a \
         contributor runs connector-settlement-solana's integration tier by hand."
    );
    let devbox_job = ci_job("devbox-validate");
    assert!(
        devbox_job.contains(&format!("solana-cli {}", bare.replace('.', "\\."))),
        "ci.yml's devbox-validate job no longer asserts `solana-cli {bare}`, so devbox.json could \
         drift from {RUST_GATE_CLI} without failing anything."
    );
    assert!(
        devbox_job.contains(&format!("solana-cli-${{{{ runner.os }}}}-{RUST_GATE_CLI}")),
        "ci.yml's devbox-validate cache key no longer names {RUST_GATE_CLI}, so a version bump \
         would silently restore a stale CLI from cache."
    );
}

#[test]
fn the_workspace_pins_the_solana_crates_to_the_rust_gate_line() {
    let line = RUST_GATE_CLI
        .trim_start_matches('v')
        .rsplit_once('.')
        .expect("the pin is v<major>.<minor>.<patch>")
        .0;
    let pin = format!("\"={line}.0\"");

    for krate in ["solana-rpc-client", "solana-sdk"] {
        assert!(
            SETTLEMENT_MANIFEST.contains(&format!("{krate} = {pin}")),
            "crates/connector-settlement-solana/Cargo.toml no longer pins {krate} to {pin}. Half \
             the reason ci.yml, local-topologies.yml, .sandcastle/Dockerfile and devbox.json \
             install Solana CLI {RUST_GATE_CLI} is that the CLI driving the validator matches the \
             release line this crate compiles against. Moving the crate off the {line} line means \
             revisiting that pin -- see this file's header for the other half (io_uring)."
        );
    }
}
