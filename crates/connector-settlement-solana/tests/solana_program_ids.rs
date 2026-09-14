//! THE record of which Solana payment-channel program ids this repository
//! names, and the guard that stops a fourth one appearing.
//!
//! There are exactly three, and they are not interchangeable:
//!
//! | Id | Where it lives | Recorded in |
//! | -- | -- | -- |
//! | [`MAINNET_BETA_PROGRAM_ID`] | Solana mainnet-beta | `packages/solana-program/deployments/mainnet-beta.md` |
//! | [`DEVNET_PUBLIC_PROGRAM_ID`] | public Solana devnet | `packages/solana-program/deployments/devnet-public.md` |
//! | [`LOCAL_TEST_PROGRAM_ID`] | a disposable `solana-test-validator`'s genesis | `crates/connector-settlement-solana/src/test_support.rs` |
//!
//! Every committed file that names one names one of those three, and
//! [`every_committed_program_id_is_one_of_the_two_this_repository_records`] is
//! what makes that true. The literal is necessarily repeated at each site --
//! two box TOMLs, a JSON endpoint record, a shell entrypoint, four local
//! topology files and the wire vector cannot read a Rust constant -- so the
//! single source is this file plus the walk, exactly as
//! `solana_cli_pins.rs` does for the two Solana CLI versions.
//!
//! # Why this guard exists (issue #1135)
//!
//! `infra/devnet-manage.sh` carried an `endpoints` verb that printed a JSON
//! document naming `7CLmNaK9z6QgUWQpCFdeUTqfwXeZH5ssohAKtyXKY4Hp` as the
//! devnet Solana program. Nothing else in the repository had ever named that
//! id. It survived for two months because
//! `crates/connector-bin/tests/devnet_configs_load.rs` checks the two box
//! TOMLs against each other and nothing cross-checked the **shell tooling**
//! against them, so the divergence was found by accident during an unrelated
//! sweep. The verb is gone; this walk is what would have found it on the day
//! it landed, and what will find the next one.
//!
//! # Why a wrong id is not a cosmetic problem
//!
//! ADR 0053 binds the settlement program into a claim's signed balance-proof
//! message, and since issue #1127 a claim's declared `programId` MUST name the
//! settlement program its `channelAccount` lives under. A claim genuinely
//! *signed* under one program id for a channel that lives under another does
//! not verify **today**, because the verifier rebuilds the message from the
//! channel's program. The connector currently only *warns* when a claim
//! merely declares a mismatched id, so a stale id in tooling produces a
//! mislabelled artifact -- but #1127 step 4 promotes that warning to a
//! refusal, at which point the same divergence presents as "the buyer's claim
//! is invalid" rather than "two committed files disagree".
//!
//! # The retired ids, so a grep for one lands somewhere
//!
//! [`RETIRED_PROGRAM_IDS`] holds them. They are named here and nowhere else on
//! purpose: a reader who finds one in git history, in an old client config or
//! in a support ticket needs one place that says what it was and why it is not
//! a deploy anyone should point at. The walk skips this file for that reason
//! -- it is the record, not a consumer of it.
//!
//! # Adding a third id
//!
//! Not forbidden -- a mainnet deploy would be one, and ADR 0056 says why there
//! is not one yet. It has to be a deliberate choice: add the constant here with
//! its deployment record, and the walk keeps working. What the guard refuses is
//! an id that appears in a committed file with no record behind it, which is
//! the only kind this repository has ever actually produced.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use connector_settlement_solana::test_support::LOCAL_TEST_PROGRAM_ID;
use solana_sdk::pubkey::Pubkey;

/// The `payment-channel` program deployed to **Solana mainnet-beta** on
/// 2026-08-14 by the first third-party operator, and upgraded in place to the
/// V2 layout on 2026-08-29. No file in this repository configures a node
/// against it: it is named only by its deployment record,
/// `packages/solana-program/deployments/mainnet-beta.md`, which is where an
/// operator copies it from. Recorded here so the walk knows it is a deploy
/// and not drift.
const MAINNET_BETA_PROGRAM_ID: &str = "8e7BhzydH1EqL486tw6Lp99BXviH3i5JN8qNpMSNmHj3";

/// The `payment-channel` program deployed to **public Solana devnet** on
/// 2026-07-18, which both fleet boxes settle through. Provenance is
/// `packages/solana-program/deployments/devnet-public.md` -- program id,
/// ProgramData account, upgrade authority, deploy signature and explorer link
/// -- and [`the_deployment_record_is_where_the_devnet_program_id_comes_from`]
/// holds this constant to it, so the value here is a citation rather than a
/// second opinion.
const DEVNET_PUBLIC_PROGRAM_ID: &str = "2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip";

/// Program ids this repository has named and no longer deploys, with why. Not
/// asserted against anything: they are recorded so that finding one is a
/// lookup rather than an investigation, and named in the walk's failure
/// message so a regression to one is reported by name.
///
/// Both belonged to the **self-hosted `solana-test-validator` box**
/// (`toon-devnet-sol`, served at `solana-rpc.<DOMAIN>`), which was deleted in
/// the public-chain cutover -- commit `44b15bdc`, 2026-07-19. Neither has ever
/// been a public-devnet deploy, and neither appears in any deployment record.
const RETIRED_PROGRAM_IDS: &[(&str, &str)] = &[
    (
        "7CLmNaK9z6QgUWQpCFdeUTqfwXeZH5ssohAKtyXKY4Hp",
        "the FIRST deploy to the self-hosted validator box, committed 2026-06-23 in `a6273570` \
         and superseded the SAME DAY by the id below (`6287c78a`: \"the validator generates a \
         fresh keypair on each deploy\"). That fix corrected the connector config and missed \
         `infra/devnet-manage.sh`'s copy, which then outlived the box, the cutover and the \
         apex -- issue #1135",
    ),
    (
        "D2Z35z8ShA4K7odczUysBYRP5hXQGDp6r5c2EBSxRsHh",
        "the SECOND and last deploy to the self-hosted validator box, 2026-06-23 (`6287c78a`). \
         Deleted with the box; the surviving local chains load the program into genesis at a \
         fixed id instead, precisely so it stops being a per-deploy artifact (issue #922)",
    ),
];

/// Files that name a program-id-shaped value this guard deliberately does not
/// hold to a recorded deploy, each with the reason -- the same shape as
/// `solana_cli_pins.rs`'s `UNPINNED_BY_DESIGN` and
/// `tools/ci/check-tracked-secrets.sh`'s allowlist, and for the same reason: a
/// blanket rule with no escape hatch gets deleted rather than amended.
///
/// An entry added here needs what these have: a named reason why the value
/// decides nothing this repository deploys or signs against.
const NOT_A_DEPLOY: &[(&str, &str)] = &[];

/// This file's own path, repo-relative. The walk skips it: the prose above
/// quotes both live ids and both retired ones in order to explain them, which
/// would otherwise make the record look like a consumer of itself.
const SELF: &str = "crates/connector-settlement-solana/tests/solana_program_ids.rs";

const STORE_CONFIG: &str = include_str!("../../../infra/linode-store/connector-rust.toml");
const RELAY_CONFIG: &str = include_str!("../../../infra/linode-relay/connector-rust.toml");
const ENDPOINTS: &str = include_str!("../../../infra/linode/endpoints.json");
const WIRE_VECTORS: &str = include_str!("../../../vectors/wire-vectors.json");
const DEPLOYMENT_RECORD: &str =
    include_str!("../../../packages/solana-program/deployments/devnet-public.md");
const DEVNET_MANAGE: &str = include_str!("../../../infra/devnet-manage.sh");
const FLEET_CONFIG_GUARD: &str =
    include_str!("../../../crates/connector-bin/tests/devnet_configs_load.rs");
const VECTOR_GENERATOR: &str = include_str!("../../../crates/connector-vectors/src/lib.rs");
const VALIDATOR_ENTRYPOINT: &str = include_str!("../../../infra/solana/entrypoint.sh");
const LOCAL_KEYS: &str = include_str!("../../../local/keys.sh");
const LOCAL_SOLO: &str = include_str!("../../../local/solo/connector.toml");
const LOCAL_MIXED_B: &str = include_str!("../../../local/mixed-chain/connector-b.toml");
const LOCAL_MIXED_C: &str = include_str!("../../../local/mixed-chain/connector-c.toml");
const LOCAL_MIXED_COMPOSE: &str = include_str!("../../../local/mixed-chain/compose.yml");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root is two levels above this crate")
}

fn recorded() -> BTreeSet<&'static str> {
    BTreeSet::from([
        MAINNET_BETA_PROGRAM_ID,
        DEVNET_PUBLIC_PROGRAM_ID,
        LOCAL_TEST_PROGRAM_ID,
    ])
}

/// Every value `raw` assigns to a program-id key, as a Solana pubkey.
///
/// A plain string scan rather than a TOML/JSON/YAML parse, because the
/// consumers are two TOMLs, two JSON files, three shell scripts, a compose
/// file and a Markdown deployment record, and what matters is the literal each
/// of them hands to a connector, a validator or a reader -- not how its file
/// happens to be shaped.
///
/// The rule is: find a program-id key, then take the first base58 run on the
/// rest of that line that `Pubkey::from_str` accepts. The `Pubkey` step is not
/// decoration -- it is what stops `CHANGELOG.md`'s "fix Solana programId
/// ([6287c78](…/commit/6287c78ad78ca…))" from reading a 40-character git sha,
/// which is base58-clean but decodes to 30 bytes, as an address.
fn declared_program_ids(raw: &str) -> BTreeSet<String> {
    // `to_ascii_lowercase` is byte-length preserving, so offsets into the
    // lowered copy index the original correctly.
    let lowered = raw.to_ascii_lowercase();
    let mut found = BTreeSet::new();

    for key in ["program_id", "programid", "program-id", "program id"] {
        let mut cursor = 0;
        while let Some(offset) = lowered[cursor..].find(key) {
            cursor += offset + key.len();
            let line = raw[cursor..].lines().next().unwrap_or_default();
            if let Some(id) = first_pubkey(line) {
                found.insert(id);
            }
        }
    }

    found
}

/// The first maximal base58 run in `line` that is a valid 32-byte Solana
/// address.
fn first_pubkey(line: &str) -> Option<String> {
    const BASE58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let mut run = String::new();
    for character in line.chars().chain(std::iter::once(' ')) {
        if BASE58.contains(character) {
            run.push(character);
            continue;
        }
        if (32..=44).contains(&run.len()) && Pubkey::from_str(&run).is_ok() {
            return Some(run);
        }
        run.clear();
    }
    None
}

/// Every file under the repository root that names a Solana payment-channel
/// program id, mapped to what it names: the ids it *declares* against a
/// program-id key, and the recorded ids it merely *mentions*.
///
/// A walk rather than a fixed list of `include_str!`s, because the failure
/// this guards against includes *a new site* -- a script, a topology or a
/// deployment doc added later that quietly picks a different id. A guard keyed
/// only on the files that exist today would pass while the repository
/// disagreed with itself, which is precisely what happened for two months.
///
/// `.rs` files are excluded, and that is a decision rather than an oversight.
/// This workspace's unit tests use deliberately fake program-id-shaped
/// constants as fixtures -- `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`
/// (SPL Token's own program), `TokenNetworkProgram11111111111111111111111`,
/// the system program -- because a config-validation or claim-carriage test
/// needs *an* address, not a deployed one. Walking them would either fail on
/// every such fixture or need an allowlist longer than the guard, and an
/// allowlist that long is how a guard gets deleted. The Rust constants that do
/// mirror a real deploy are held by name instead, in
/// [`the_fleet_guard_and_the_wire_vector_generator_mirror_the_deployed_devnet_program`].
fn program_id_sites() -> BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)> {
    let root = repo_root();
    let recorded = recorded();
    let mut sites = BTreeMap::new();
    let mut queue = vec![root.clone()];

    while let Some(dir) = queue.pop() {
        let entries = std::fs::read_dir(&dir).unwrap_or_else(|error| {
            panic!("cannot read {dir:?} while scanning for Solana program ids: {error}")
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
            if name.ends_with(".rs") {
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
            if relative == SELF {
                continue;
            }
            let raw = String::from_utf8_lossy(&bytes);
            let declared = declared_program_ids(&raw);
            let mentioned: BTreeSet<String> = recorded
                .iter()
                .filter(|id| raw.contains(**id))
                .map(|id| (*id).to_string())
                .collect();
            if declared.is_empty() && mentioned.is_empty() {
                continue;
            }
            sites.insert(relative, (declared, mentioned));
        }
    }

    sites
}

#[test]
fn every_committed_program_id_is_one_of_the_two_this_repository_records() {
    let recorded = recorded();
    let mut drifted = Vec::new();

    for (file, (declared, _)) in program_id_sites() {
        if NOT_A_DEPLOY.iter().any(|(path, _)| *path == file) {
            continue;
        }
        for id in declared {
            if !recorded.contains(id.as_str()) {
                let retired = RETIRED_PROGRAM_IDS
                    .iter()
                    .find(|(candidate, _)| *candidate == id)
                    .map(|(_, why)| format!(" -- a RETIRED id: {why}"))
                    .unwrap_or_default();
                drifted.push(format!("{file} names {id}{retired}"));
            }
        }
    }

    assert!(
        drifted.is_empty(),
        "a committed file names a Solana payment-channel program id this file does not \
         record:\n  {}\n\nThis repository has exactly three, deliberately: \
         {MAINNET_BETA_PROGRAM_ID} on MAINNET-BETA (packages/solana-program/deployments/\
         mainnet-beta.md), {DEVNET_PUBLIC_PROGRAM_ID} on PUBLIC DEVNET (packages/solana-program/\
         deployments/devnet-public.md), and {LOCAL_TEST_PROGRAM_ID} inside a disposable \
         solana-test-validator's genesis. They are not interchangeable, and since ADR 0053 the \
         program id is bound into a claim's signed balance proof -- a claim signed under one for \
         a channel that lives under the other does not verify. If the new id really is a deploy, \
         it needs a deployment record under packages/solana-program/deployments/ and a constant \
         here saying which chain it is on; if it is a placeholder that decides nothing, add it to \
         NOT_A_DEPLOY with that reason.",
        drifted.join("\n  ")
    );
}

#[test]
fn the_repository_names_a_solana_program_id_in_exactly_the_known_places() {
    let expected: BTreeSet<&str> = BTreeSet::from([
        // The operator's guide's "Get paid" step: the devnet
        // `[settlement.solana]` table an operator pastes, addresses included,
        // so pointing a node at a chain is not a scavenger hunt across the
        // deployment records.
        "README.md",
        "deploy/connector-rust/README.md",
        "deploy/connector-rust/connector.production.toml",
        "docs/adr/0056-production-is-a-named-empty-tier.md",
        "infra/linode-relay/connector-rust.toml",
        "infra/linode-store/connector-rust.toml",
        "infra/linode/endpoints.json",
        "infra/solana/entrypoint.sh",
        // The two topologies that put a node on the validator: `mixed-chain`
        // crosses a chain without crossing a denomination, and `dealing`
        // (ADR 0071) crosses both. Each names the disposable validator's
        // genesis id, never a deployed one.
        "local/dealing/compose.yml",
        "local/dealing/connector-b.toml",
        "local/dealing/connector-c.toml",
        "local/keys.sh",
        "local/mixed-chain/compose.yml",
        "local/mixed-chain/connector-b.toml",
        "local/mixed-chain/connector-c.toml",
        "local/solo/connector.toml",
        "packages/solana-program/deployments/devnet-public.md",
        "packages/solana-program/deployments/mainnet-beta.md",
        "vectors/wire-vectors.json",
    ]);
    let actual: BTreeSet<String> = program_id_sites().into_keys().collect();
    let actual: BTreeSet<&str> = actual.iter().map(String::as_str).collect();

    assert_eq!(
        actual, expected,
        "the set of non-Rust files naming a Solana program id changed. A new one is not \
         forbidden -- it just has to be a deliberate choice between the three ids this file \
         records, and named here so the next drift is still visible. A file that DISAPPEARED \
         from this set is the more interesting direction: it usually means a site's id was \
         mangled into something that is no longer a valid address, which the value check above \
         cannot see because there is no longer an address there to check."
    );
}

#[test]
fn the_deployment_record_is_where_the_devnet_program_id_comes_from() {
    assert!(
        DEPLOYMENT_RECORD.contains(DEVNET_PUBLIC_PROGRAM_ID),
        "packages/solana-program/deployments/devnet-public.md no longer names \
         {DEVNET_PUBLIC_PROGRAM_ID}. That record -- program id, ProgramData account, upgrade \
         authority, deploy signature, explorer link -- is the only provenance this constant has. \
         Without it the id in this file is an assertion nobody can check, and every site the walk \
         holds to it is being held to a number rather than to a deploy."
    );
    assert!(
        DEPLOYMENT_RECORD.contains("api.devnet.solana.com"),
        "packages/solana-program/deployments/devnet-public.md no longer names the cluster it \
         records a deploy to. Which chain an id is on is half of what makes it usable: ADR 0056 \
         blocks production partly because this program is devnet-only, and a record that stops \
         saying so stops supporting that decision."
    );
}

#[test]
fn the_fleet_and_the_wire_contract_name_the_deployed_devnet_program() {
    for (name, raw) in [
        ("infra/linode-store/connector-rust.toml", STORE_CONFIG),
        ("infra/linode-relay/connector-rust.toml", RELAY_CONFIG),
        ("infra/linode/endpoints.json", ENDPOINTS),
        ("vectors/wire-vectors.json", WIRE_VECTORS),
    ] {
        assert_eq!(
            declared_program_ids(raw),
            BTreeSet::from([DEVNET_PUBLIC_PROGRAM_ID.to_string()]),
            "{name} must name {DEVNET_PUBLIC_PROGRAM_ID} and nothing else. The two box configs \
             are what the fleet settles under, endpoints.json is the document a third-party \
             payer configures itself from, and the wire vector is the normative cross-repo \
             contract (ADR 0021) that toon-client, rig and swap build claims against. A payer \
             configured from one of these and a node configured from another must agree, or the \
             claim's signed balance proof is built over a different message than the one the \
             verifier rebuilds."
        );
    }
}

#[test]
fn the_local_topologies_name_only_the_disposable_validators_program() {
    for (name, raw) in [
        ("infra/solana/entrypoint.sh", VALIDATOR_ENTRYPOINT),
        ("local/keys.sh", LOCAL_KEYS),
        ("local/solo/connector.toml", LOCAL_SOLO),
        ("local/mixed-chain/connector-b.toml", LOCAL_MIXED_B),
        ("local/mixed-chain/connector-c.toml", LOCAL_MIXED_C),
    ] {
        assert_eq!(
            declared_program_ids(raw),
            BTreeSet::from([LOCAL_TEST_PROGRAM_ID.to_string()]),
            "{name} must name {LOCAL_TEST_PROGRAM_ID}, the id \
             `infra/solana/entrypoint.sh` loads payment_channel.so under in a disposable \
             validator's genesis. Naming the devnet deploy here would point a local topology at \
             a program that does not exist on its chain."
        );
    }
    assert!(
        LOCAL_MIXED_COMPOSE.contains(LOCAL_TEST_PROGRAM_ID)
            && !LOCAL_MIXED_COMPOSE.contains(DEVNET_PUBLIC_PROGRAM_ID),
        "local/mixed-chain/compose.yml's channel_open call must pass {LOCAL_TEST_PROGRAM_ID}. It \
         passes the program positionally rather than as `program_id = …`, so the walk's value \
         check cannot read it -- this is the assertion that covers it, and \
         the_repository_names_a_solana_program_id_in_exactly_the_known_places is what notices if \
         the literal stops being there at all."
    );
}

#[test]
fn the_fleet_guard_and_the_wire_vector_generator_mirror_the_deployed_devnet_program() {
    assert!(
        FLEET_CONFIG_GUARD.contains(&format!(
            "FLEET_SOLANA_PROGRAM_ID: &str = \"{DEVNET_PUBLIC_PROGRAM_ID}\""
        )),
        "crates/connector-bin/tests/devnet_configs_load.rs's FLEET_SOLANA_PROGRAM_ID is no longer \
         {DEVNET_PUBLIC_PROGRAM_ID}. That constant is what holds both box TOMLs to a single \
         Solana deploy; this case is what holds the constant to the deployment record, so the \
         chain runs record -> constant -> committed config rather than stopping at the constant."
    );
    assert!(
        VECTOR_GENERATOR.contains(&format!(
            "SOLANA_SETTLEMENT_PROGRAM_ID: &str = \"{DEVNET_PUBLIC_PROGRAM_ID}\""
        )),
        "crates/connector-vectors/src/lib.rs's SOLANA_SETTLEMENT_PROGRAM_ID is no longer \
         {DEVNET_PUBLIC_PROGRAM_ID}. It is what `generate-vectors` writes into \
         vectors/wire-vectors.json's Solana claim, which is the normative contract toon-client, \
         rig and swap build against (ADR 0021). Regenerating the vector from a drifted constant \
         would publish the drift as the contract."
    );
}

/// Issue #1135's own regression case, stated by name rather than left as a
/// consequence of the walk, so the failure says what came back.
#[test]
fn the_fleet_lifecycle_script_names_no_program_id_of_its_own() {
    assert!(
        declared_program_ids(DEVNET_MANAGE).is_empty(),
        "infra/devnet-manage.sh names a Solana program id again. It carried one for two months \
         in an `endpoints` verb that printed a document nobody generated any more -- see issue \
         #1135 and RETIRED_PROGRAM_IDS above. The devnet's endpoints are hand-maintained in \
         infra/linode/endpoints.json (infra/linode/README.md says why); a lifecycle script that \
         provisions boxes has no business holding a second copy of a settlement address."
    );
}
