//! Proves the devnet overlay's own committed `connector.toml` files
//! (issue #490, ADR 0013's parallel fleet) load and serve by the actual
//! compiled binary -- the first acceptance criterion, and per the issue's
//! own scope boundary, the only one this sandbox (no Docker, no
//! infrastructure credentials) can verify. Reachability of the peer or the
//! apps behind any of these routes is explicitly NOT proven here.
//!
//! The fleet is TWO files as of issue #872: the store (`infra/linode-store/`)
//! and the relay (`infra/linode-relay/`, added by #816). It was three
//! through issue #817 -- the apex (`infra/linode-node/`) sat in front of
//! both, forwarding `g.toon.ario` and `g.toon.relay` across a paid peering
//! to each. Issue #872 (toon-meta#310 / toon-meta#313's live cutover)
//! deleted `infra/linode-node/` and both peerings entirely: with no apex to
//! dial in, `infra/linode-store/` and `infra/linode-relay/` are
//! client-edge-only connectors again, each terminating its own prefix
//! directly. There are two cases per file, and they prove different things.
//!
//! **Verbatim** (`*_devnet_config_loads_and_serves_verbatim`) boots the file
//! exactly as committed, substituting only what this sandbox physically
//! cannot supply: the signer key file (real key material is never committed
//! -- see `.gitignore`, and config load refuses a `key_file` that is not
//! there), the bind addresses (fixed devnet ports would flake or collide
//! across parallel test runs -- every other test in this crate binds
//! `127.0.0.1:0` for the same reason), and `state_dir` (a container path
//! this host cannot create -- but the line itself must still be there,
//! issue #605). Nothing semantic is touched: no
//! prefix, no route, no peer, no `price`. This is the property a reader
//! assumes this file provides, and the only case that catches a committed
//! config which cannot start. Exactly that happened: a `[settlement]`
//! section naming the zero address made both files exit 1 on startup,
//! because `EvmSettlementBackend::connect` resolves a `TokenNetwork`
//! through the configured `TokenNetworkRegistry` and there is no contract
//! at that one (issue #542, issue #576).
//!
//! One more substitution joined that list when the store file's
//! `[settlement]` section went LIVE against Base Sepolia when the store box
//! grew a Rust connector of its own (the relay file was committed live from
//! day one, #816): a committed live section means the node cannot start
//! without reaching that chain -- the fail-closed behaviour ADR 0009 asks
//! for, and exactly the network dependency a test must not have. So BOTH
//! verbatim cases now boot with their live settlement sections STRIPPED
//! ([`without_live_settlement`]), and those sections are proven by the
//! cases described below.
//!
//! The store file's settlement stopped being a commented template when that
//! node became a counterparty rather than a terminus: it accepts client-edge
//! claims of its own, on whichever chain the buyer chose, so an EVM-only
//! node would refuse every Solana-paid write. Its section is asserted to
//! name the SAME registry, program and mint the relay names, because a
//! buyer's channel lives on one deployment and a node pointed elsewhere
//! cannot resolve it.
//!
//! Since #645 both files carry issue #628's KEYED per-chain shape --
//! `[settlement.evm]` + `[settlement.evm.key]` and `[settlement.solana]` +
//! `[settlement.solana.key]` -- in place of the single flat `[settlement]`
//! (`chain = "evm"`) table. They are keyed off the section HEADERS
//! ([`LIVE_SETTLEMENT_SECTIONS`]) rather than one literal marker, and
//! stripping still panics when a named section is not there live -- a
//! config that quietly loses its settlement must break this module, never
//! coast through it.
//!
//! The verbatim case also drives one claimless request at each file's own
//! terminating route and asserts it is answered with the x402 greeting
//! (#552) naming the committed `price`, rather than served for nothing.
//! Both files priced that route at `0` while the greeting had already
//! landed, so a deployed box was still an open free gateway (issue #557);
//! because the price is read off the committed text like everything else
//! here, this fails again if any file ever silently returns to zero.
//!
//! **Section** (`*_devnet_settlement_section_boots_against_a_deployed_contract`)
//! points each file's live `[settlement.evm]`, as committed, at a freshly
//! deployed contract on a disposable local `anvil`, and boots that. Both are
//! on issue #628's keyed shape, and both cases assert it rather than
//! trusting a reader's eye, because the legacy flat table they left behind
//! still parses and so a slide back would otherwise be silent. Committed
//! config shapes rot; this keeps them demonstrably working and keeps
//! `runtime::build`'s settlement construction path covered end to end by the
//! real binary. It is skipped when no `anvil` is on `PATH`.
//!
//! The `[settlement.solana]` leg is deliberately NOT booted. It is stripped
//! alongside the rest for the verbatim case and stripped again for the
//! anvil case, which boots the EVM leg only. There is no local `anvil`
//! equivalent standing by here: the committed leg names public Solana
//! devnet (`https://api.devnet.solana.com`) and a program deployed on that
//! cluster, and `SolanaSettlementBackend::connect` does not merely read --
//! it fetches the program and mint accounts AND submits a transaction
//! (`ensure_own_ata_exists`), so booting it would make this test suite
//! depend on public-internet reachability, on a third party's rate limits,
//! and on a FUNDED devnet account whose key this sandbox cannot have. A
//! chain-backed Solana case would need a `solana-test-validator` with
//! `packages/solana-program` deployed into it (the shape
//! `connector-settlement-solana`'s own tests use) and a retargeted
//! `program_id`/`token_address`; that is a different, heavier test than
//! this module's "the committed file starts" question, and it is not
//! written here. Both files' Solana identity is still checked, just more
//! weakly and in fewer places than the EVM leg's: a substring `.contains`
//! against the committed text, not a typed parse, and it lives in each
//! file's own **section** case above -- so it is skipped along with that
//! case wherever `anvil` is not on `PATH`. The EVM leg has a typed,
//! network-free check that always runs
//! ([`every_fleet_configs_settlement_evm_leg_matches_the_live_identity`]);
//! the Solana leg has no equivalent, which is a known gap this issue did
//! not widen -- the apex's own typed parse case that #872 removed covered
//! the apex file only, never these two.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use chrono::{Duration as ChronoDuration, Utc};
use connector_config::{Config, SettlementConfig, TransportPolicy};
use connector_domain::{agreed_required_transport, EnvelopeRequest, Prepare};
use connector_settlement_evm::test_support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};
use connector_settlement_evm::EvmSettlementBackend;

mod support;
use support::{parse_json_log_addr, write_config, write_raw_key_file};

const STORE_CONFIG: &str = include_str!("../../../infra/linode-store/connector-rust.toml");
const RELAY_CONFIG: &str = include_str!("../../../infra/linode-relay/connector-rust.toml");

/// The relay box's RENDERED nginx config -- the file its `nginx` service
/// actually bind-mounts.
const RELAY_NGINX_CONF: &str = include_str!("../../../infra/linode-relay/nginx/conf.d/node.conf");

/// Every nginx file any box in this repo commits: the RENDERED config a box's
/// `nginx` service bind-mounts, and the `.template` that `bootstrap.sh`
/// renders it from. Both, because the pair drift independently -- issue
/// #987's broken `/swap` form was fixed in one and left in the other once
/// already -- and the template is what a rebuilt box starts from.
///
/// # Why the faucet box is in here now (issue #1013)
///
/// This list used to be the two WATCHTOWER-managed boxes only, on the reading
/// that a literal upstream is a problem only where something recreates
/// containers unattended. `infra/linode-faucet/` and the self-hosted chain
/// box's `infra/linode/nginx/` were excluded on those grounds, and the
/// exclusion was left as a comment with nothing enforcing it.
///
/// Two things were wrong with that. The narrow one: the chain box had already
/// adopted the variable+resolver form everywhere except its own
/// `mina.conf.template`, so the exclusion was protecting drift, not a
/// decision. The load-bearing one: only the *502 until someone reloads* half
/// of the defect needs a Watchtower. The other half -- an upstream container
/// that is not running at parse time is `[emerg] host not found in upstream`,
/// which exits the nginx MASTER and takes every server block with it, ACME
/// included -- needs no recreate at all, and both excluded boxes build their
/// images on-box, where `up -d --build` recreates a container exactly the way
/// a Watchtower pull does.
///
/// So the rule is no longer "boxes with a Watchtower": it is every box, and
/// [`every_committed_box_nginx_file_is_covered_by_the_upstream_guards`] walks
/// `infra/` and fails if a committed nginx file is missing from this list --
/// which is what the old prose-only "add a box to this list when it gets a
/// Watchtower" could not do.
///
/// The chain box's four templates were named here until its provisioning was
/// deleted: that box went in the public-chain cutover (`44b15bdc`,
/// 2026-07-19) and its `infra/linode/` scripts, compose overlay, nginx
/// templates and `workflow_dispatch`-only caller
/// (`.github/workflows/devnet-deploy.yml`) followed. Nothing was exempted to
/// make that pass -- the files stopped existing, and the walk above is what
/// proves this list still names every one that does.
const BOX_NGINX_FILES: &[(&str, &str)] = &[
    (
        "infra/linode-relay/nginx/conf.d/node.conf",
        RELAY_NGINX_CONF,
    ),
    (
        "infra/linode-relay/nginx/node.conf.template",
        include_str!("../../../infra/linode-relay/nginx/node.conf.template"),
    ),
    (
        "infra/linode-store/nginx/conf.d/node.conf",
        include_str!("../../../infra/linode-store/nginx/conf.d/node.conf"),
    ),
    (
        "infra/linode-store/nginx/node.conf.template",
        include_str!("../../../infra/linode-store/nginx/node.conf.template"),
    ),
    (
        "infra/linode-faucet/nginx/conf.d/node.conf",
        include_str!("../../../infra/linode-faucet/nginx/conf.d/node.conf"),
    ),
    (
        "infra/linode-faucet/nginx/node.conf.template",
        include_str!("../../../infra/linode-faucet/nginx/node.conf.template"),
    ),
];

/// This test binary's own base port for [`Anvil::spawn`] -- distinct from
/// other test binaries' bases (`connector-settlement-evm`'s own tests use
/// 18_600; `connector-cli`'s use 18_700/18_800) so that binaries running
/// concurrently under `cargo test --workspace` don't contend for the same
/// port range.
const ANVIL_BASE_PORT: u16 = 18_500;

/// Every LIVE (uncommented) settlement section either of the fleet's two
/// files (store, relay -- issue #816/#817, apex removed by #872) commits, in
/// issue #628's keyed per-chain shape as of #645 -- the sections
/// [`without_live_settlement`] strips for the hermetic verbatim boot. Both
/// name the exact same four headers, so one list covers both of them; a file
/// that ever needs a different set is a reason to split this constant, not
/// to widen it silently.
///
/// Named exhaustively rather than matched by a `[settlement` prefix so that
/// the list itself is the claim about what the committed file contains:
/// [`without_sections`] panics on a name that is not there live, and
/// [`without_live_settlement`] panics on a live settlement section that is
/// not named here. Between them, neither losing a leg nor adding one can
/// pass silently -- which is exactly what #645's migration off the flat
/// `[settlement]` did to the string-literal marker this replaced.
const LIVE_SETTLEMENT_SECTIONS: &[&str] = &[
    "[settlement.evm]",
    "[settlement.evm.key]",
    "[settlement.solana]",
    "[settlement.solana.key]",
];

/// Any fleet file's Solana leg alone -- stripped by the anvil-backed cases,
/// which retarget the EVM leg at a local chain and have no local stand-in
/// for this one. See the module docs for why it is not booted.
const SOLANA_SETTLEMENT_SECTIONS: &[&str] = &["[settlement.solana]", "[settlement.solana.key]"];

/// The `price` the committed files put on their **store** routes, and so
/// the amount the x402 greeting must quote for them. Deliberately a literal
/// rather than something parsed back out of the file under test: a test
/// that read the expected value from the thing it is testing would keep
/// passing if that value went back to `0`, which is exactly the regression
/// issue #557 exists to prevent. The figure's origin is parity with the
/// retired TypeScript fleet's own `price: '1000'` -- see
/// `docs/devnet-pricing.md`, which is the committed source of truth now
/// that both files that carried that literal are deleted (#901, #872).
const EXPECTED_STORE_PRICE: u64 = 1000;

/// The `price` the relay file puts on `g.toon.relay`, which is **not** the
/// store price and is deliberately not folded into one constant.
///
/// Until issue #872 removed the apex, this box's terminating route was fed
/// by a `peer_id` forward from the apex's own client edge at a
/// `price`/`fee` split of `1002`/`2` (`EXPECTED_APEX_FORWARD_PRICE`/`_FEE`,
/// removed with the apex) that had to deliver exactly this literal net of
/// its fee -- see docs/devnet-pricing.md's history for that arithmetic. Now
/// that the relay terminates `g.toon.relay` directly for its own clients,
/// this is simply the box's own price, same as `EXPECTED_STORE_PRICE` is
/// the store's.
const EXPECTED_RELAY_PRICE: u64 = 1;

/// `str::replace`, but a pattern that matches nothing is a test failure
/// rather than a silent no-op -- otherwise renaming a line in a committed
/// file would quietly turn one of the substitutions below into nothing at
/// all, and the test would go on passing while testing something else.
/// Does this TOML line assign `field`, as opposed to merely mentioning it?
/// Comment lines never count: every committed fleet file explains its own
/// settings by name in prose, so a substring match reads the header rather
/// than the config.
fn assigns(line: &str, field: &str) -> bool {
    let line = line.trim_start();
    !line.starts_with('#')
        && line
            .split_once('=')
            .is_some_and(|(key, _)| key.trim() == field)
}

/// The two files the store box's committed `[operator]` section points at
/// (issue #1003), substituted for the container paths the same way the key
/// files are -- and for the same reason: neither is committed, and config
/// load refuses a `bearer_token_file`/`write_keys_file` that is not there.
///
/// A process-wide `OnceLock` rather than a per-test temp file, so that every
/// existing caller of [`with_sandbox_paths`] picks the substitution up
/// without threading two more arguments through fourteen call sites -- and,
/// more usefully, so that a NEW test cannot forget to. The `TempDir` is held
/// inside the `OnceLock` for the lifetime of the test binary; dropping it
/// would delete the files out from under a still-running test.
///
/// The contents are what an operator would actually write: a hex token, and
/// an allowlist with a comment line above one 64-hex public key.
struct SandboxOperatorFiles {
    _dir: tempfile::TempDir,
    bearer_token: std::path::PathBuf,
    write_keys: std::path::PathBuf,
}

fn sandbox_operator_files() -> &'static SandboxOperatorFiles {
    static FILES: std::sync::OnceLock<SandboxOperatorFiles> = std::sync::OnceLock::new();
    FILES.get_or_init(|| {
        let dir = tempfile::tempdir().expect("temp operator dir");
        let bearer_token = dir.path().join("operator-bearer-token");
        std::fs::write(
            &bearer_token,
            "1f0e6a4c9b2d8e7f3a5c1b9d0e2f4a6c8b0d2e4f6a8c0b2d4e6f8a0c2b4d6e8f\n",
        )
        .expect("write sandbox operator bearer token");
        let write_keys = dir.path().join("operator-write-keys");
        std::fs::write(
            &write_keys,
            "# the sandbox's one allowlisted operator\n\
             0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n",
        )
        .expect("write sandbox operator allowlist");
        SandboxOperatorFiles {
            _dir: dir,
            bearer_token,
            write_keys,
        }
    })
}

fn replace_expecting_a_match(raw: &str, from: &str, to: &str) -> String {
    assert!(
        raw.contains(from),
        "expected to find `{from}` in the committed config text -- if that \
         line was renamed, update this test rather than letting the \
         substitution silently do nothing"
    );
    raw.replace(from, to)
}

/// Whether a committed file actually *declares* a TOML table, as opposed to
/// merely mentioning its name.
///
/// A plain `contains("[announce]")` cannot tell the two apart, and both files
/// carry a comment explaining that `[node]` was `[announce]` until ADR 0046 --
/// the history that stops the next reader from reintroducing the section. The
/// comment is the point, so this matches a header line rather than a
/// substring, the same way the `[[peers]]` check does.
fn declares_table(config: &str, header: &str) -> bool {
    config.lines().any(|line| line.trim() == header)
}

/// Substitute only what this sandbox physically cannot supply: the signer
/// key file (real key material is never committed), the relay's carried-over
/// `identity_key_file` when the file carries one (issue #870, same reason),
/// the store's `[operator]` credential files (issue #1003, same reason
/// again), the bind addresses (fixed ports collide across parallel test
/// runs) and `state_dir` (the committed value is a container path,
/// `/app/state`, which no test host can create). Every other line --
/// prefixes, handler URLs, `price`, and every `[settlement]` value -- stays
/// the literal committed content.
///
/// The `state_dir` substitution is a path swap, not a removal: the
/// committed files must keep naming one, since a devnet box without it
/// would hold its claim watermarks in memory and forget every spent claim
/// on restart (issue #605). `replace_expecting_a_match` is what makes that
/// load-bearing -- deleting the line from any of the fleet's files fails
/// this test rather than silently testing a node with no durable state.
///
/// No `secret_file` substitution any more: issue #872 removed the apex and
/// with it the only two peerings this fleet had (`apex-store`, `apex-relay`)
/// -- neither surviving file's `[[peers]]`/`[[peer_channels]]` tables exist
/// to name one. The assert below is what used to be the `None` branch of a
/// `Some`/`None` choice a caller had to make per file; now it is the only
/// shape, and it still fails loudly rather than silently coasting through a
/// peering nobody taught this helper to substitute if one is ever added back.
fn with_sandbox_paths(
    raw: &str,
    key_path: &std::path::Path,
    state_dir: &std::path::Path,
) -> String {
    let replaced = replace_expecting_a_match(
        raw,
        "key_file = \"/app/data/signer.key\"",
        &format!("key_file = \"{}\"", key_path.display()),
    );
    let replaced = replace_expecting_a_match(
        &replaced,
        "client_edge_addr = \"0.0.0.0:4000\"",
        "client_edge_addr = \"127.0.0.1:0\"",
    );
    let replaced = replace_expecting_a_match(
        &replaced,
        "state_dir = \"/app/state\"",
        &format!("state_dir = \"{}\"", state_dir.display()),
    );
    // The `[operator]` section's two files (issue #1003) -- same reasoning
    // as the key files above, and optional in the same way: only the store
    // file carries the section today, so plain `.replace` leaves the relay
    // file untouched. [`sandbox_operator_files`] owns the substitutes; a
    // path that ever moves in the committed file fails at `Config::load`
    // with `OperatorFileNotFound` rather than being silently skipped.
    let operator = sandbox_operator_files();
    let replaced = replaced.replace(
        "bearer_token_file = \"/app/data/operator-bearer-token\"",
        &format!(
            "bearer_token_file = \"{}\"",
            operator.bearer_token.display()
        ),
    );
    let replaced = replaced.replace(
        "write_keys_file = \"/app/data/operator-write-keys\"",
        &format!("write_keys_file = \"{}\"", operator.write_keys.display()),
    );
    // The regression guard for issue #1003 itself. A committed fleet config
    // must name its operator credentials BY PATH and never carry one: a
    // literal `bearer_token`/`write_keys` line here is a credential in a
    // public repository, and it is also the drift that made the store box's
    // operator surface uncommittable in the first place. Line-anchored on
    // the key left of the `=`, because the file's own header prose names
    // both settings at length while explaining them, and because
    // `bearer_token_file` starts with `bearer_token`.
    for field in ["bearer_token", "write_keys"] {
        assert!(
            !replaced.lines().any(|line| assigns(line, field)),
            "a committed fleet config carries an inline `{field} = …`. That is \
             a credential (or an authorization decision) in a PUBLIC \
             repository, and it is the exact drift issue #1003 closed -- use \
             `{field}_file` and put the file on the box (see \
             infra/linode-store/connector-rust.toml's [operator] header)"
        );
    }
    assert!(
        !replaced.contains("secret_file ="),
        "no surviving box's committed config should carry a peering \
         `secret_file` any more -- issue #872 removed the apex, and with it \
         the only two peerings this fleet had (apex-store, apex-relay). If a \
         new peering is intentional, teach this helper to substitute its \
         secret_file again"
    );
    replaced
}

/// A fleet file's two settlement key files, each pointed at a real file
/// this sandbox can supply -- the same substitution the `[signer]` key gets
/// in [`with_sandbox_paths`], and for the same reason: real key material is
/// never committed, and config load refuses a `key_file` that is not there.
///
/// Deliberately NOT the same path as the signer's on the committed box (the
/// settlement account is the funded one), but the same temp file serves
/// both legs here: `runtime::read_settlement_key_bytes` reads one shape for
/// either chain -- 32 raw bytes or 64 hex characters, exactly what
/// `write_raw_key_file` produces -- and hands the EVM leg its hex-encoded
/// secp256k1 form while the Solana leg takes the raw 32 bytes as an ed25519
/// seed (issue #630). The committed Solana file is documented as "a 32-byte
/// ed25519 seed as 64 hex chars, same shape as ./settlement-rust.key" for
/// precisely that reason.
fn with_sandbox_settlement_keys(raw: &str, key_path: &std::path::Path) -> String {
    let replaced = replace_expecting_a_match(
        raw,
        "key_file = \"/app/data/settlement.key\"",
        &format!("key_file = \"{}\"", key_path.display()),
    );
    replace_expecting_a_match(
        &replaced,
        "key_file = \"/app/data/settlement-solana.key\"",
        &format!("key_file = \"{}\"", key_path.display()),
    )
}

/// Uncomment the `[settlement]` block a committed file carries between its
/// `BEGIN`/`END` template markers, so the block an operator is told to fill
/// in is the block this test actually boots. A file with no such block is a
/// test failure: either the template was deleted (and this case has nothing
/// left to prove) or a real `[settlement]` section was committed (and the
/// verbatim case is the one that covers it).
/// Remove the named live (uncommented) TOML sections: each header line
/// through the line before the next header at column 0, or the end of the
/// file. Comment lines that happen to precede a removed header are left
/// where they are -- they are prose, and prose parses.
///
/// A named section that is not there live is a test failure, the same
/// fail-loud property [`replace_expecting_a_match`] gives the substitutions
/// above. Matching on the header rather than on a literal slice of file
/// text is what makes this survive the settlement block moving, being
/// reordered, or acquiring new keys, while still refusing to strip a
/// section that has been renamed out from under it.
fn without_sections(raw: &str, headers: &[&str]) -> String {
    let mut out = String::new();
    let mut removed: Vec<&str> = Vec::new();
    let mut removing = false;
    for line in raw.lines() {
        let header = line.trim_end();
        if header.starts_with('[') {
            removing = headers.contains(&header);
            if removing {
                removed.push(header);
            }
        }
        if !removing {
            out.push_str(line);
            out.push('\n');
        }
    }
    for header in headers {
        assert!(
            removed.contains(header),
            "expected a live (uncommented) `{header}` section to strip in the \
             committed config text -- if that section was renamed or removed, \
             update this test rather than letting the strip silently do nothing"
        );
    }
    out
}

/// Strip ALL of a fleet file's live settlement sections
/// ([`LIVE_SETTLEMENT_SECTIONS`]) for the verbatim boot -- see the module
/// docs: a committed live section is a startup-blocking chain dependency by
/// design (ADR 0009), and the sections themselves are proven by the
/// anvil-backed cases below. Shared by both fleet files (store, relay),
/// which both commit the same four headers.
///
/// Panics when a named section is not there live, and again when a live
/// settlement section survives that this module has not been taught about
/// -- so neither losing settlement nor adding a chain leg can weaken the
/// verbatim case unnoticed.
fn without_live_settlement(raw: &str) -> String {
    let stripped = without_sections(raw, LIVE_SETTLEMENT_SECTIONS);
    let survivor = stripped
        .lines()
        .map(str::trim_end)
        .find(|line| line.starts_with("[settlement"));
    assert!(
        survivor.is_none(),
        "the committed config has a live settlement section this test does \
         not know about ({}) -- every leg must be stripped for the hermetic \
         verbatim boot and covered by a case below; add it to \
         `LIVE_SETTLEMENT_SECTIONS`",
        survivor.unwrap_or_default()
    );
    stripped
}

/// Point an uncommented `[settlement.evm]` block at a real, disposable,
/// freshly deployed local chain. `decimals` and the key location stay the
/// literal committed content.
///
/// The committed values it looks for are the same [`FLEET_LIVE_REGISTRY`] and
/// [`EXPECTED_SETTLEMENT_TOKEN_ADDRESS`] constants the identity test asserts,
/// rather than per-call arguments: every fleet file names that one pair, and
/// reading both off one constant apiece is what keeps the substitution and
/// the identity check from drifting apart.
fn with_anvil_settlement(
    raw: &str,
    anvil_rpc_url: &str,
    contract_address: ethers::types::Address,
    token_address: ethers::types::Address,
) -> String {
    let replaced = replace_expecting_a_match(
        raw,
        "rpc_url = \"https://base-sepolia-rpc.publicnode.com\"",
        &format!("rpc_url = \"{anvil_rpc_url}\""),
    );
    let replaced = replace_expecting_a_match(
        &replaced,
        &format!("contract_address = \"{FLEET_LIVE_REGISTRY}\""),
        &format!("contract_address = \"{contract_address:?}\""),
    );
    replace_expecting_a_match(
        &replaced,
        &format!("token_address = \"{EXPECTED_SETTLEMENT_TOKEN_ADDRESS}\""),
        &format!("token_address = \"{token_address:?}\""),
    )
}

fn spawn(config_path: &std::path::Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_connector"))
        .arg(config_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn connector binary")
}

/// Reads stdout lines until `"connector listening"` is seen, returning the
/// client edge's actual bound address; or the process exits first -- which
/// fails with the exit status and whatever the process printed on stderr,
/// rather than hanging or reporting only that a pipe closed.
///
/// There is one listen line to wait for since ADR 0027 / issue #679 deleted
/// the raw-TCP transport and its separate listener.
fn wait_for_listen_line(child: &mut Child) -> String {
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut line = String::new();
    loop {
        line.clear();
        let read = stdout.read_line(&mut line).expect("read stdout");
        if read == 0 {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                use std::io::Read;
                let _ = pipe.read_to_string(&mut stderr);
            }
            let status = child.wait().expect("wait for connector to exit");
            panic!(
                "the connector exited ({status}) before logging a listen \
                 address -- this config does not start. stderr:\n{stderr}"
            );
        }
        if line.contains("connector listening") {
            return parse_json_log_addr(&line);
        }
    }
}

/// A booted connector, killed and reaped on drop -- so a test that panics
/// midway leaves no orphaned process behind -- holding its config file alive
/// for as long as the process is.
struct BootedConnector {
    child: Child,
    client_edge_addr: String,
    _config_file: tempfile::NamedTempFile,
}

impl Drop for BootedConnector {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn boot(config_text: &str) -> BootedConnector {
    let config_file = write_config(config_text);
    let mut child = spawn(config_file.path());
    let client_edge_addr = wait_for_listen_line(&mut child);
    BootedConnector {
        child,
        client_edge_addr,
        _config_file: config_file,
    }
}

/// A claimless PREPARE addressed to `destination` -- no `ILP-Payment-Channel-
/// Claim`/`-Wrapped` header exists on this HTTP request, matching what any
/// real unpaying sender would send. The envelope body is irrelevant: a
/// priced route answers with terms before ever decoding it as work for the
/// app (issue #526).
fn unpaid_prepare(destination: &str) -> Prepare {
    Prepare {
        amount: 0,
        expires_at: Utc::now() + ChronoDuration::minutes(5),
        greeting: false,
        destination: destination.to_string(),
        data: EnvelopeRequest {
            method: "POST".to_string(),
            target: "/".to_string(),
            headers: vec![],
            body: vec![],
        }
        .encode(),
    }
}

/// Issue #557's core proof: a claimless request to `destination` on a node
/// started from a committed devnet config is answered with the x402
/// greeting (HTTP 402, terms naming `expected_price`) instead of being
/// forwarded to the app -- the free-gateway failure mode this issue closes.
///
/// The price is a parameter rather than one module constant because the
/// relay and store routes are priced differently (see [`EXPECTED_RELAY_PRICE`]
/// and [`EXPECTED_STORE_PRICE`]). Every caller passes a literal, so the guard
/// keeps the property #557 needs: the expectation is never read back out of
/// the file under test.
async fn assert_answered_with_x402_greeting(
    client_edge_addr: &str,
    destination: &str,
    expected_price: u64,
) {
    let terms = x402_terms(client_edge_addr, destination).await;
    assert_eq!(
        terms["accepts"][0]["amount"],
        expected_price.to_string(),
        "greeted price for {destination} must match the route's committed `price`"
    );
    assert_ne!(
        expected_price, 0,
        "a zero price is the free-gateway regression issue #557 exists to catch"
    );
}

/// The x402 terms JSON a claimless request to `destination` is answered
/// with -- shared by [`assert_answered_with_x402_greeting`] (an ordinary
/// unpaid-request greeting) and the transport-policy test below (issue
/// #701), which reuses the same 402 shape for a different reason.
async fn x402_terms(client_edge_addr: &str, destination: &str) -> serde_json::Value {
    let response = reqwest::Client::new()
        .post(format!("http://{client_edge_addr}/ilp"))
        .body(unpaid_prepare(destination).encode())
        .send()
        .await
        .expect("POST /ilp");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::PAYMENT_REQUIRED,
        "a claimless request to a priced route must be greeted, not served"
    );
    response.json().await.expect("x402 JSON terms")
}

#[tokio::test]
async fn the_store_side_devnet_config_loads_and_serves_verbatim() {
    // The one prefix this box terminates, and the reason the config exists:
    // a store box that did not answer `g.toon.ario` could not take over from
    // the TypeScript node it replaced.
    assert!(STORE_CONFIG.contains("g.toon.ario"));
    // `g.toon.relay.ario`, the relay-hop spelling, was retired by issue #820
    // alongside `g.toon.store` -- it was never actually reachable (see
    // docs/devnet-pricing.md's "Retired names"). Asserting its ABSENCE, same
    // as the `g.toon.store` check below, so re-adding it has to be a
    // deliberate edit here too rather than drifting back in.
    assert!(!STORE_CONFIG.contains("prefix = \"g.toon.relay.ario\""));

    // No peer semantics: this node accepts no inbound peer connection and dials
    // no peer, so only the client edge comes up. ADR 0003's raw-TCP wire
    // cannot carry the public inter-node link this fleet needs (#623), so
    // the file configures neither -- see its header. (A peer-forwarded
    // route being unpriced on both sides was the file's other stated
    // reason; ADR 0028 removed it, and #678 wired the carriages, so what
    // remains is a deployment decision rather than a missing mechanism.)
    // When the inter-connector transport decision lands, this is the
    // assertion that changes.
    // Line-anchored, like the `chain = ` check further down: the header is
    // free to *name* `peer_wire_addr` while explaining at length why it is
    // gone, so only an actual uncommented assignment counts.
    assert!(
        !STORE_CONFIG
            .lines()
            .any(|line| line.starts_with("peer_wire_addr")),
        "the store config must not bind ADR 0003's plaintext peer semantics on a \
         box with no private segment -- if a peering is being added, it \
         should arrive with the transport that replaces it"
    );

    let key_file = write_raw_key_file(9);
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let connector = boot(&with_sandbox_paths(
        &without_live_settlement(STORE_CONFIG),
        key_file.path(),
        state_dir.path(),
    ));

    assert_answered_with_x402_greeting(
        &connector.client_edge_addr,
        "g.toon.ario",
        EXPECTED_STORE_PRICE,
    )
    .await;
    // `g.toon.store` was retired here too (owner decision, 2026-08-05).
    assert!(!STORE_CONFIG.contains("prefix = \"g.toon.store\""));
}

/// The relay box's own file (issue #816/#817), modelled on the store's case
/// above: a connector that terminates `g.toon.relay` against the relay app
/// now co-located with it, in place of the apex's former local `handler_url`
/// route to the same app. Issue #820 gave this box the accept-only half of
/// an apex<->relay peering; issue #872 removed it along with the apex.
#[tokio::test]
async fn the_relay_side_devnet_config_loads_and_serves_verbatim() {
    assert!(RELAY_CONFIG.contains("g.toon.relay"));

    // Issue #872: this box no longer peers with anything -- the apex that
    // used to dial in is gone. Line-anchored because the file's own header
    // prose is free to *name* either table while explaining their removal,
    // so a substring match would trip on prose rather than an actual table.
    assert!(
        !RELAY_CONFIG.lines().any(|line| line.trim() == "[[peers]]")
            && !RELAY_CONFIG
                .lines()
                .any(|line| line.trim() == "[[peer_channels]]"),
        "the relay box must not carry a [[peers]]/[[peer_channels]] table \
         any more -- issue #872 removed the apex, the only counterparty \
         that ever dialed this box's peering"
    );

    let key_file = write_raw_key_file(9);
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let connector = boot(&with_sandbox_paths(
        &without_live_settlement(RELAY_CONFIG),
        key_file.path(),
        state_dir.path(),
    ));

    assert_answered_with_x402_greeting(
        &connector.client_edge_addr,
        "g.toon.relay",
        EXPECTED_RELAY_PRICE,
    )
    .await;
}

/// Issue #833's fix, terminating side, as ADR 0050 re-homes it: the store
/// box describes `g.toon.ario` as its own, from its own `[node]` section, so
/// a client learns the terminating identity from the node that holds it --
/// the key that answers this node's `/ilp/identity`, opens every gift wrap it
/// terminates (ADR 0018), and now rides its self-description.
///
/// The section is `[node]`, not `[announce]`: the announce is gone (ADR 0046,
/// issue #1074) and the two spellings must never exist at once, which the
/// negative half below pins.
#[test]
fn the_store_devnet_config_describes_g_toon_ario_under_its_own_identity() {
    assert!(
        declares_table(STORE_CONFIG, "[node]"),
        "the store config must carry its own [node] section -- ADR 0050"
    );
    assert!(
        !declares_table(STORE_CONFIG, "[announce]"),
        "the section was RENAMED, not duplicated: `[announce]` is refused by \
         name at boot (ADR 0046, issue #1074), so a committed file carrying \
         both would not start at all"
    );

    let key_file = write_raw_key_file(9);
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let text = with_sandbox_paths(STORE_CONFIG, key_file.path(), state_dir.path());
    let text = with_sandbox_settlement_keys(&text, key_file.path());
    let config_file = write_config(&text);
    let config = Config::load(config_file.path()).expect("the committed store config must parse");

    let node = config
        .node()
        .expect("the store config's [node] section must parse");
    assert_eq!(node.primary_address(), "g.toon.ario");
    assert_eq!(
        node.http_endpoint(),
        Some("https://proxy.ario.devnet.toonprotocol.dev/ilp"),
        "the store must publish ITS OWN public edge, not the apex's -- \
         publishing the apex's endpoint here would just relocate the \
         mismatch from identity to endpoint"
    );
    assert_eq!(
        node.btp_endpoint(),
        Some("wss://proxy.ario.devnet.toonprotocol.dev/ilp/btp")
    );
}

/// Issue #1003: the store box's operator surface (ADR 0008, live since
/// toon-meta#312) is now IN the committed config, and reachable from it.
///
/// The bug this closes was not a wrong value — it was an absent section.
/// `main` carried no `[operator]` at all, because the only spelling
/// available was an inline bearer token that a public repository must never
/// hold; so the surface lived on the box alone, and a `fleet-ops` reconcile
/// of the committed tree would have deleted a live capability with nothing
/// anywhere recording that it had. Two halves, both asserted here:
///
/// * the section is present and file-backed (the inline-literal half of the
///   property is enforced for every fleet file in [`with_sandbox_paths`],
///   since a future relay `[operator]` deserves the same guard); and
/// * it actually RESOLVES — `Config::load` returns an authenticated surface,
///   not merely a section that parses. `Config` refuses to return a
///   half-configured one, so a `Some` here is the whole guarantee.
#[test]
fn the_store_devnet_config_commits_its_operator_surface_by_file_reference() {
    assert!(
        STORE_CONFIG.lines().any(|line| line.trim() == "[operator]"),
        "the store config must carry its own [operator] section -- issue \
         #1003. This box has served an authenticated operator surface since \
         toon-meta#312; a committed config without the section is a config \
         that deletes it on the next reconcile"
    );
    for field in ["bearer_token_file", "write_keys_file"] {
        assert!(
            STORE_CONFIG.lines().any(|line| assigns(line, field)),
            "the store config's [operator] section must name `{field}` -- \
             the file forms are the only ones a public repository can carry"
        );
    }

    let key_file = write_raw_key_file(9);
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let text = with_sandbox_paths(STORE_CONFIG, key_file.path(), state_dir.path());
    let text = with_sandbox_settlement_keys(&text, key_file.path());
    let config_file = write_config(&text);
    let config = Config::load(config_file.path()).expect("the committed store config must parse");

    let operator = config
        .operator()
        .expect("the committed store config must resolve an operator surface");
    assert!(
        !operator.bearer_token().is_empty(),
        "an operator surface with an empty bearer token would have no read \
         authentication -- Config::load should have refused it"
    );
    assert_eq!(
        operator.write_keys().len(),
        1,
        "the sandbox allowlist holds exactly one key, and its comment line \
         must not have become a second one"
    );
}

/// Issue #843's core property, mirroring #833's for the store: the relay
/// box's `[node]` must claim ONLY prefixes it actually terminates.
/// Asserted over the committed `[[routes]]` table rather than a literal
/// `"g.toon.relay"` on both sides -- a test that hardcodes the same string
/// twice passes when both are wrong together, which is exactly the shape
/// that let #841 slip past #839's own test.
///
/// The property survives ADR 0050 unchanged; what it is asserted against does
/// not. The section is `[node]`, `identity_key_file` and `relay_url` are gone
/// with the announce (issue #1074), and the endpoints are now what a `GET
/// /ilp` publishes rather than what a kind:10032 event carried.
#[test]
fn the_relay_devnet_config_describes_only_prefixes_it_terminates() {
    assert!(
        declares_table(RELAY_CONFIG, "[node]"),
        "the relay config must carry its own [node] section -- ADR 0050"
    );
    assert!(
        !declares_table(RELAY_CONFIG, "[announce]"),
        "the section was RENAMED, not duplicated: `[announce]` is refused by \
         name at boot (ADR 0046, issue #1074), so a committed file carrying \
         both would not start at all"
    );

    let key_file = write_raw_key_file(9);
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let text = with_sandbox_settlement_keys(
        &with_sandbox_paths(RELAY_CONFIG, key_file.path(), state_dir.path()),
        key_file.path(),
    );
    let config_file = write_config(&text);
    let config = Config::load(config_file.path()).expect("the committed relay config must parse");

    let terminated: Vec<&str> = config.routes().iter().map(|r| r.prefix()).collect();
    assert!(
        !terminated.is_empty(),
        "the relay config is expected to terminate at least `g.toon.relay` -- \
         if that changed, this test's premise needs revisiting"
    );

    let node = config
        .node()
        .expect("the relay config's [node] section must parse");
    for address in node.addresses() {
        assert!(
            terminated.iter().any(|prefix| prefix == address),
            "the relay's [node] addresses names `{address}`, which the \
             relay's own committed connector-rust.toml does not terminate \
             (terminated prefixes: {terminated:?}) -- publishing a prefix \
             this box does not terminate is issue #833's exact defect \
             reproduced on a new box: a client seals its gift wrap to this \
             node's key and the node that actually holds it (whichever one \
             that is) cannot open it"
        );
    }

    assert_eq!(
        node.http_endpoint(),
        Some("https://proxy.relay.devnet.toonprotocol.dev/ilp"),
        "the relay must publish ITS OWN public edge"
    );
    assert_eq!(
        node.btp_endpoint(),
        Some("wss://proxy.relay.devnet.toonprotocol.dev/ilp/btp")
    );
}

/// The committed store config, parsed the way the two `[announce]` tests
/// below need it: sandbox paths and sandbox settlement keys, nothing else
/// rewritten.
///
/// The temp key/state/secret files exist only for the parse -- `Config::load`
/// reads what it needs there and then, and both callers go on to read the
/// parsed config rather than back to disk -- so the guards are free to drop
/// with the call.
fn load_committed_store_config() -> Config {
    let key_file = write_raw_key_file(9);
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let text = with_sandbox_settlement_keys(
        &with_sandbox_paths(STORE_CONFIG, key_file.path(), state_dir.path()),
        key_file.path(),
    );
    let config_file = write_config(&text);
    Config::load(config_file.path()).expect("the committed store config must parse")
}

/// The relay box's committed config, on the same terms as
/// [`load_committed_store_config`]. `without_live_settlement` rather than
/// that helper's settlement-key rewrite because the tests below read this
/// box's route table and `[announce]` section, never its settlement legs.
fn load_committed_relay_config() -> Config {
    let key_file = write_raw_key_file(9);
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let text = with_sandbox_paths(
        &without_live_settlement(RELAY_CONFIG),
        key_file.path(),
        state_dir.path(),
    );
    let config_file = write_config(&text);
    Config::load(config_file.path()).expect("the committed relay config must parse")
}

/// The longest committed route prefix matching `address` at a segment
/// boundary -- the router's own selection rule, over one file's static
/// `[[routes]]`, so the transport a committed announce would declare is
/// read the same way the client edge would read it.
fn committed_transport_policy(config: &Config, address: &str) -> Option<TransportPolicy> {
    config
        .routes()
        .iter()
        .filter(|route| {
            let prefix = route.prefix();
            address == prefix
                || (address.starts_with(prefix) && address.as_bytes()[prefix.len()] == b'.')
        })
        .max_by_key(|route| route.prefix().len())
        .map(|route| route.transport_policy())
}

/// The other half of issue #701, and the one the fleet ran WITHOUT for as
/// long as the policy has existed: a box that ENFORCES a transport must
/// PUBLISH it.
///
/// Verified live 2026-08-14 against `connector:rust-sha-415531a`: the relay
/// box refuses an HTTP-carried paid write to `g.toon.relay` (its committed
/// route below is `transport = "btp"`) and its kind:10032 announce carried
/// no `requiredTransport` key -- nor did any other announce in the fleet's
/// corpus. toon-client's `terminatorRequiresBtp` guard (toon-client#558)
/// reads exactly that key, so it could never fire and every client was
/// refused after falling through to HTTP.
///
/// ADR 0050 closes that class by construction -- there is one description of
/// a node now, and this is the rule it derives the field with. Run over the
/// COMMITTED files through the connector's own
/// [`connector_domain::agreed_required_transport`] rather than a literal, so
/// this follows the configs: unpin the relay's route and the relay assertion
/// below changes with it, pin the store's and the store assertion does.
#[test]
fn each_boxs_self_description_declares_the_transport_its_own_routes_require() {
    let relay = load_committed_relay_config();
    let relay_node = relay
        .node()
        .expect("the relay config's [node] section must parse");
    assert_eq!(
        agreed_required_transport(
            relay_node
                .addresses()
                .iter()
                .filter_map(|address| committed_transport_policy(&relay, address))
                .map(|policy| policy.name())
        )
        .as_deref(),
        Some("btp"),
        "the relay terminates `g.toon.relay` with `transport = \"btp\"` for huddles' \
         persistent sessions, so its self-description has to say so -- a client that cannot \
         read the requirement discovers it by being refused a write it has already paid to send"
    );

    let store = load_committed_store_config();
    let store_node = store
        .node()
        .expect("the store config's [node] section must parse");
    assert_eq!(
        agreed_required_transport(
            store_node
                .addresses()
                .iter()
                .filter_map(|address| committed_transport_policy(&store, address))
                .map(|policy| policy.name())
        ),
        None,
        "the store's route is left at the permissive default, so its self-description must \
         carry no `requiredTransport` key at all -- naming the default would put a key on the \
         wire to say nothing"
    );
}

/// Issue #701 (toon-meta#262 decision 11): `g.toon.relay` is restricted to
/// BTP -- a high-frequency, always-connected carriage where a persistent
/// session pays off -- while the store legs stay at the default (`both`)
/// for the one-shot anonymous uploads `channels.rs` calls "a first-class
/// path, not a fallback".
///
/// Until issue #872 removed the apex, this pin lived on the apex's
/// `g.toon.relay` peer_id forward being unable to carry one at all
/// (`transport` is illegal on a `peer_id` route,
/// `ConfigError::PeerRouteHasTransport`) -- so the apex's own greeting never
/// named a required transport, and only the relay box's own terminating
/// route enforced it. Now that both boxes terminate their own prefixes
/// directly, this test asserts the same split at the source: the relay
/// box's own greeting for `g.toon.relay` requires BTP, and the store box's
/// greeting for `g.toon.ario` does not.
#[tokio::test]
async fn the_relay_route_is_btp_only_and_the_store_routes_accept_both() {
    // Line-anchored, like the peer_wire_addr check above: the store file's
    // own header prose is free to *name* `transport = "btp"` while
    // explaining why the relay (not the store) pins it, so a substring
    // match would trip on prose rather than an actual route field.
    assert!(
        !STORE_CONFIG
            .lines()
            .any(|line| line.trim() == "transport = \"btp\""),
        "the store file must not restrict its routes to a transport -- \
         one-shot anonymous uploads stay at the default (`both`)"
    );
    assert!(
        RELAY_CONFIG.contains("transport = \"btp\""),
        "the relay box's own file must restrict its terminating route to \
         btp, per issue #701"
    );

    let key_file = write_raw_key_file(9);
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let connector = boot(&with_sandbox_paths(
        &without_live_settlement(STORE_CONFIG),
        key_file.path(),
        state_dir.path(),
    ));
    let store_terms = x402_terms(&connector.client_edge_addr, "g.toon.ario").await;
    assert!(
        store_terms["accepts"][0]["extra"]
            .get("requiredTransport")
            .is_none(),
        "the store leg left at the default must not carry requiredTransport: {store_terms}"
    );

    let relay_key_file = write_raw_key_file(10);
    let relay_state_dir = tempfile::tempdir().expect("temp state dir");
    let relay_connector = boot(&with_sandbox_paths(
        &without_live_settlement(RELAY_CONFIG),
        relay_key_file.path(),
        relay_state_dir.path(),
    ));
    let relay_own_terms = x402_terms(&relay_connector.client_edge_addr, "g.toon.relay").await;
    assert_eq!(
        relay_own_terms["accepts"][0]["extra"]["requiredTransport"], "btp",
        "the relay box's own file must require BTP on its own terminating \
         route too: {relay_own_terms}"
    );
}

/// Deploy a fresh `TokenNetworkRegistry`, a `TokenNetwork` through it, and
/// its mock USDC on a disposable local chain. The returned [`Anvil`] must
/// stay alive for as long as the addresses beside it are used.
async fn deploy_settlement_on_anvil() -> (Anvil, ethers::types::Address, ethers::types::Address) {
    let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
    let token =
        EvmSettlementBackend::deploy_mock_token(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, 1_000_000)
            .await
            .expect("deploy mock USDC");
    let settlement = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("deploy a TokenNetwork through a fresh registry");
    let registry_address = settlement.registry_address();
    drop(settlement);
    (anvil, registry_address, token)
}

/// The registry every fleet file's LIVE `[settlement.evm]` section names --
/// the deployed Base Sepolia `TokenNetworkRegistry` (#576, #577), repointed
/// here by the ERC-2771 cutover (#695/#811, broadcast 2026-08-06,
/// `docs/evm-deployment.md`). The anvil cases replace exactly this committed
/// value, so they double as the guard that the committed sections keep
/// naming it; [`every_fleet_configs_settlement_evm_leg_matches_the_live_identity`]
/// below asserts it directly, as parsed, for both surviving files.
const FLEET_LIVE_REGISTRY: &str = "0x0c41D9D424d6B075A3cEa1068a694f7847a8CCa5";

/// The retired pre-ERC-2771 `TokenNetworkRegistry` [`FLEET_LIVE_REGISTRY`]
/// replaced -- `docs/evm-deployment.md`'s "Current live deployment
/// (pre-cutover)" table and its "Rollback: one step" section, which names
/// this exact address as what a rollback reverts `contract_address` to. Not
/// itself asserted against any committed file; named only so a regression
/// back to it is called out by address in the identity test's failure
/// message, not left for a reader to recognise on sight.
const SETTLEMENT_CONTRACT_ADDRESS_ROLLBACK_TARGET: &str =
    "0xcC9079adE929b168B54145f6d25262b64FAB9D5b";

/// Both fleet files' Solana leg (`https://api.devnet.solana.com`) and the
/// deployed `payment-channel` program they settle through, wired in #633 --
/// asserted as literals here, exactly like [`FLEET_LIVE_REGISTRY`] and
/// [`EXPECTED_STORE_PRICE`], so that reading the expected values back out of the
/// file under test cannot make this pass on a file that drifted.
const FLEET_SOLANA_PROGRAM_ID: &str = "2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip";
/// The mint moved on 2026-08-27, and the reason is worth keeping next to the
/// literal. The 2026-07-18 mint (`xyc5J8Mg...`) is still on chain and still
/// holds its supply, but its MINT AUTHORITY was a key that lived outside the
/// repository and is lost -- so nobody can mint it and nobody can refill a
/// treasury holding it, which is what left the devnet faucet's Solana leg
/// dead with no repair path. This mint's authority is the faucet box's own
/// treasury (`infra/linode-faucet/create-devnet-usdc-mint.sh`), so the faucet
/// mints per drip and there is no irreplaceable key anywhere in the design.
/// See `packages/solana-program/deployments/devnet-public.md`'s mint amendment.
const FLEET_SOLANA_USDC_MINT: &str = "34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU";

/// The settlement asset's scale on every chain this fleet settles on: ADR
/// 0010's "6 decimals everywhere" (docs/usdc-cross-chain-settlement.md).
/// Both legs must agree with it or `runtime::build` refuses to start
/// against the real token (issues #564, #630) -- a mismatch committed here
/// would be a box that cannot boot.
const EXPECTED_SETTLEMENT_DECIMALS: u8 = 6;

/// The mock USDC ERC-20 every fleet config's `[settlement.evm]` leg settles
/// in. Unchanged by the #695/#811 ERC-2771 registry cutover -- only the
/// `TokenNetworkRegistry` moved (see [`FLEET_LIVE_REGISTRY`]); the token being
/// registered through it did not (`docs/evm-deployment.md`: "never a new
/// token, so no existing balance or faucet distribution is disturbed").
/// [`with_anvil_settlement`] looks for this same literal before retargeting a
/// leg at a freshly deployed mock, so the substitution and this identity
/// check read one constant instead of two copies that could drift apart.
const EXPECTED_SETTLEMENT_TOKEN_ADDRESS: &str = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce";

/// Lowercase hex, for comparing a parsed 20-byte EVM address back against
/// the committed literal.
fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Every fleet config's `[settlement.evm]` leg -- store and relay alike --
/// must name the identical registry, asset and precision: a claim or
/// channel a buyer opened against one `TokenNetworkRegistry`/token is
/// unresolvable by a box pointed at a different one. The boot tests below
/// assert the same property for store/relay with a substring `.contains`
/// check against the committed text; this asserts it as PARSED, typed
/// values against literal constants instead, which is what actually catches
/// a value that merely *looks* right in the text -- `.contains` would still
/// pass on different whitespace, a different case, or a longer address that
/// happens to contain the expected one as a substring.
///
/// Reads no chain and boots nothing, so it runs even where `anvil` is not
/// on `PATH` -- unlike the two `*_devnet_settlement_section_boots_against_a_deployed_contract`
/// cases, which are skipped there.
///
/// Failure messages name both the expected literal and the value actually
/// found, per issue #852 -- including calling out
/// [`SETTLEMENT_CONTRACT_ADDRESS_ROLLBACK_TARGET`] by address, so a silent
/// revert to the retired pre-ERC-2771 registry is named rather than just
/// failed.
#[test]
fn every_fleet_configs_settlement_evm_leg_matches_the_live_identity() {
    for (label, raw) in [("store", STORE_CONFIG), ("relay", RELAY_CONFIG)] {
        let key_file = write_raw_key_file(9);
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let text = with_sandbox_paths(raw, key_file.path(), state_dir.path());
        let text = with_sandbox_settlement_keys(&text, key_file.path());
        let config_file = write_config(&text);

        let config = Config::load(config_file.path())
            .unwrap_or_else(|e| panic!("the committed {label} config must parse: {e}"));

        let evm = config
            .settlements()
            .iter()
            .find_map(|settlement| match settlement {
                SettlementConfig::Evm(evm) => Some(evm),
                SettlementConfig::Solana(_) => None,
            })
            .unwrap_or_else(|| panic!("the {label} config must carry a live [settlement.evm] leg"));

        let contract_address = format!("0x{}", hex_lower(evm.contract_address().as_slice()));
        assert_eq!(
            contract_address.to_lowercase(),
            FLEET_LIVE_REGISTRY.to_lowercase(),
            "the {label} config's [settlement.evm] contract_address must be the live \
             TokenNetworkRegistry {FLEET_LIVE_REGISTRY} (expected), found {contract_address} -- \
             {SETTLEMENT_CONTRACT_ADDRESS_ROLLBACK_TARGET} is the retired pre-ERC-2771 registry \
             and must not be accepted silently"
        );

        let token_address = format!("0x{}", hex_lower(evm.token_address().as_slice()));
        assert_eq!(
            token_address.to_lowercase(),
            EXPECTED_SETTLEMENT_TOKEN_ADDRESS.to_lowercase(),
            "the {label} config's [settlement.evm] token_address must be \
             {EXPECTED_SETTLEMENT_TOKEN_ADDRESS} (expected), found {token_address}"
        );

        assert_eq!(
            evm.decimals(),
            EXPECTED_SETTLEMENT_DECIMALS,
            "the {label} config's [settlement.evm] decimals must be \
             {EXPECTED_SETTLEMENT_DECIMALS} (expected), found {}",
            evm.decimals()
        );
    }
}

/// The store box's settlement is no longer a commented template waiting on a
/// deployment -- it is live, and it names the SAME contracts the relay box's
/// own file names, because a claim this node accepts was written against a
/// channel the buyer opened on the shared devnet deployment. A store node
/// pointed at a different registry cannot resolve that channel, so this
/// asserts the two files agree rather than merely that each parses.
#[tokio::test]
async fn the_store_devnet_settlement_section_boots_against_a_deployed_contract() {
    if !require_anvil() {
        return;
    }
    let (anvil, contract_address, token) = deploy_settlement_on_anvil().await;

    let key_file = write_raw_key_file(9);
    let state_dir = tempfile::tempdir().expect("temp state dir");
    // Issue #648: the store config followed the apex onto issue #628's KEYED
    // shape. The legacy flat `[settlement]` + `chain = "evm"` still parses,
    // so nothing below would fail if it slid back -- it would just quietly
    // go on writing a shape the fleet no longer uses, and one that cannot
    // grow a second chain. Asserted here rather than left to a reader's eye.
    assert!(
        STORE_CONFIG.contains("[settlement.evm]")
            && STORE_CONFIG.contains("[settlement.evm.key]")
            && STORE_CONFIG.contains("[settlement.solana]")
            && STORE_CONFIG.contains("[settlement.solana.key]"),
        "the store config must carry both legs on the keyed \
         `[settlement.<chain>]` shape (issue #628, #648), not the legacy \
         flat `[settlement]` table -- an EVM-only store node refuses every \
         Solana-paid write, and this fleet's peering settles on solana:devnet"
    );
    // Line-anchored: the prose above is free to *name* the legacy
    // `chain = "evm"` key while explaining why it is gone, so only an actual
    // uncommented assignment counts.
    assert!(
        !STORE_CONFIG
            .lines()
            .any(|line| line.starts_with("chain = ")),
        "`chain` is the legacy flat shape's discriminator -- a keyed table \
         names its chain by its own key"
    );
    assert!(
        STORE_CONFIG.contains(FLEET_LIVE_REGISTRY),
        "the store leg must name the fleet's deployed TokenNetworkRegistry \
         ({FLEET_LIVE_REGISTRY}) -- a buyer's channel lives on one \
         deployment, and a node pointed elsewhere cannot resolve it"
    );
    assert!(
        STORE_CONFIG.contains(FLEET_SOLANA_PROGRAM_ID)
            && STORE_CONFIG.contains(FLEET_SOLANA_USDC_MINT),
        "the store leg must name the fleet's Solana payment-channel program \
         and mint, for the same reason"
    );

    // Anvil stands in for Base Sepolia; the Solana leg is stripped because
    // there is no local validator in this test (see the module docs).
    let text = without_sections(STORE_CONFIG, SOLANA_SETTLEMENT_SECTIONS);
    let text = with_anvil_settlement(&text, &anvil.rpc_url, contract_address, token);
    let text = replace_expecting_a_match(
        &text,
        "key_file = \"/app/data/settlement.key\"",
        &format!("key_file = \"{}\"", key_file.path().display()),
    );
    let text = with_sandbox_paths(&text, key_file.path(), state_dir.path());

    drop(boot(&text));
}

/// The relay box's own live `[settlement.evm]` leg (issue #816/#817), boots
/// against a freshly deployed local chain exactly like the store's case
/// above. It names the SAME registry the store file names: its client edge
/// already accepts an unaffiliated buyer's own on-chain channel (the relay
/// file's own header, issue #556/#611), and that buyer's channel lives on
/// the one shared deployment. That was true independently of the
/// apex<->relay peering issue #820 gave this box and issue #872 removed
/// again -- it held before the peering existed and holds after it is gone.
#[tokio::test]
async fn the_relay_devnet_settlement_section_boots_against_a_deployed_contract() {
    if !require_anvil() {
        return;
    }
    let (anvil, contract_address, token) = deploy_settlement_on_anvil().await;

    let key_file = write_raw_key_file(9);
    let state_dir = tempfile::tempdir().expect("temp state dir");

    assert!(
        RELAY_CONFIG.contains("[settlement.evm]")
            && RELAY_CONFIG.contains("[settlement.evm.key]")
            && RELAY_CONFIG.contains("[settlement.solana]")
            && RELAY_CONFIG.contains("[settlement.solana.key]"),
        "the relay config must carry both legs on the keyed \
         `[settlement.<chain>]` shape (issue #628), like the store"
    );
    assert!(
        RELAY_CONFIG.contains(FLEET_LIVE_REGISTRY),
        "the relay leg must name the same deployed TokenNetworkRegistry as \
         the store ({FLEET_LIVE_REGISTRY}) -- a buyer's channel lives on one \
         deployment, and a node pointed elsewhere cannot resolve it"
    );
    assert!(
        RELAY_CONFIG.contains(FLEET_SOLANA_PROGRAM_ID)
            && RELAY_CONFIG.contains(FLEET_SOLANA_USDC_MINT),
        "the relay leg must name the same Solana payment-channel program and \
         mint as the store, for the same reason"
    );

    // Anvil stands in for Base Sepolia; the Solana leg is stripped for the
    // same reason the store's is -- there is no local validator in this test.
    let text = without_sections(RELAY_CONFIG, SOLANA_SETTLEMENT_SECTIONS);
    let text = with_anvil_settlement(&text, &anvil.rpc_url, contract_address, token);
    let text = replace_expecting_a_match(
        &text,
        "key_file = \"/app/data/settlement.key\"",
        &format!("key_file = \"{}\"", key_file.path().display()),
    );
    let text = with_sandbox_paths(&text, key_file.path(), state_dir.path());

    drop(boot(&text));
}

// ═══════════════════════════════════════════════════════════════════════════
// Box nginx upstreams re-resolve (issue #993, and issue #987's URI gotcha)
// ═══════════════════════════════════════════════════════════════════════════

/// The `proxy_pass` argument of every non-comment `proxy_pass` line in an
/// nginx file, with its trailing `;` stripped. A line scan: no nginx parser
/// exists in this tree, and the shape being read is one this repo writes by
/// hand.
fn proxy_pass_targets(raw: &str) -> Vec<&str> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix("proxy_pass "))
        .map(|rest| rest.trim_end_matches(';').trim())
        .collect()
}

/// nginx resolves a LITERAL upstream hostname once, at config-parse time,
/// and caches the address for the worker's life. On a box where something
/// recreates containers unattended -- the label-scoped Watchtower of
/// toon-meta#403 pulling a new `:release` digest -- the recreated container
/// comes back on a NEW address and this edge answers `502` until a human
/// runs `nginx -s reload`. That is the outage this test exists to prevent;
/// it happened on the store box on 2026-08-16.
///
/// Naming the upstream through a variable moves resolution to request time,
/// where each file's `resolver 127.0.0.11 valid=10s` applies, and the edge
/// self-heals inside the TTL. Proven live on both boxes by forcing an
/// upstream onto a new address (relay `swap-node` .7 -> .8, store
/// `connector-rust` .4 -> .8): both back inside 3s with no reload.
///
/// A literal upstream has a second failure mode this also refuses: an
/// upstream container that is not running at parse time is `[emerg] host not
/// found in upstream`, which exits the whole nginx master -- every server
/// block in the file, not just the one location. `nginx -t` on the relay
/// file as committed before this test failed exactly that way.
///
/// That second mode is why this covers EVERY box (issue #1013) and not just
/// the two a Watchtower recreates: it needs no unattended recreate, only a
/// container that happens to be down when nginx parses. See
/// [`BOX_NGINX_FILES`].
#[test]
fn no_box_nginx_names_a_literal_upstream() {
    for (name, raw) in BOX_NGINX_FILES {
        for target in proxy_pass_targets(raw) {
            assert!(
                target.starts_with("http://$") || target.starts_with('$'),
                "{name} proxies to `{target}` -- a literal upstream hostname \
                 is resolved ONCE at config-parse time, so a recreate of that \
                 container 502s this edge until someone reloads nginx, and a \
                 container that is simply DOWN at parse time is `[emerg] host \
                 not found in upstream`, which exits the nginx master (issue \
                 #993). Name the upstream through a variable (`set $upstream \
                 <container>;`) so the file's own `resolver` re-resolves it \
                 per request."
            );
        }
    }
}

/// [`BOX_NGINX_FILES`] is written out by hand: a guard that globbed its own
/// inputs would silently change what it covers whenever the filesystem changed. The
/// cost of that choice is that a NEW box's nginx config is unguarded until
/// someone remembers the list -- which is exactly what happened to the faucet
/// and chain boxes, whose exclusion lived in a doc comment that nothing could
/// enforce.
///
/// This test pays that cost off without giving up the explicit list: it walks
/// `infra/` and fails if it finds a committed nginx file the list does not
/// name. Coverage still only ever changes in a reviewed diff -- the failure
/// mode is a red test naming the missing file, not silent drift in either
/// direction.
#[test]
fn every_committed_box_nginx_file_is_covered_by_the_upstream_guards() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("crates/connector-bin/../.. is the repo root");

    let mut found = Vec::new();
    collect_nginx_files(&repo_root.join("infra"), &repo_root, &mut found);
    found.sort();

    assert!(
        !found.is_empty(),
        "walked {}/infra and found no nginx config at all -- this guard is \
         reading the wrong tree and would pass no matter what was committed",
        repo_root.display()
    );

    let missing: Vec<&String> = found
        .iter()
        .filter(|path| !BOX_NGINX_FILES.iter().any(|(name, _)| name == path))
        .collect();

    assert!(
        missing.is_empty(),
        "{missing:?} are committed nginx configs that `BOX_NGINX_FILES` does \
         not name, so `no_box_nginx_names_a_literal_upstream` and \
         `no_variable_upstream_carries_a_static_uri_part` cannot \
         see them. Add each file to that list (`include_str!` + its \
         repo-relative path) -- a guard that cannot see a committed file \
         cannot refuse anything about it."
    );
}

/// Every `*.conf` / `*.template` under an `nginx/` directory in `dir`,
/// recursively, as repo-relative paths. Used only by
/// [`every_committed_box_nginx_file_is_covered_by_the_upstream_guards`].
fn collect_nginx_files(dir: &std::path::Path, repo_root: &std::path::Path, out: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => panic!("read_dir({}) failed: {err}", dir.display()),
    };

    for entry in entries {
        let path = entry.expect("a readable directory entry").path();
        if path.is_dir() {
            collect_nginx_files(&path, repo_root, out);
            continue;
        }

        let relative = path
            .strip_prefix(repo_root)
            .expect("walked path is under the repo root")
            .to_string_lossy()
            .replace('\\', "/");

        if relative.contains("/nginx/")
            && (relative.ends_with(".conf") || relative.ends_with(".template"))
        {
            out.push(relative);
        }
    }
}

/// The other half of the same fix. Issue #987 filed this as "with a variable
/// upstream nginx IGNORES the URI part and forwards the original
/// `$request_uri`", and #999 wrote that sentence into this guard and into six
/// box nginx headers. **That rule is wrong**, and issue #1023 measured it:
///
/// * nginx's own `proxy_pass` docs, under "the part of a request URI to be
///   replaced cannot be determined": *"When variables are used in
///   `proxy_pass` [...] if URI is specified in the directive, it is passed to
///   the server as is, replacing the original request URI."* Passed, not
///   ignored. (The neighbouring bullet -- a `rewrite ... break` in the
///   location makes a LITERAL `proxy_pass`'s URI part ignored -- is the one
///   that actually says "ignored", and is the likely source of the mix-up.)
/// * Reproduced on nginx 1.18.0 / 1.24.0 / 1.26.3 / 1.28.3 / 1.30.4 /
///   1.31.3: `location /graphql` + `proxy_pass $up/node/devnet/v1/graphql;`
///   makes the upstream see `/node/devnet/v1/graphql`, every version.
/// * End-to-end against the real upstream: the chain box's committed mina
///   block (deleted with that box's provisioning; see [`BOX_NGINX_FILES`]),
///   run verbatim in a throwaway nginx, returned **200** from
///   `api.minascan.io`. The counterfactual #987 predicts
///   (`proxy_pass $up$request_uri;`) returns **404** -- `api.minascan.io`
///   serves `/node/devnet/v1/graphql` and 404s `/graphql`, so the two
///   readings are distinguishable and the honoured one is what happens.
///
/// What is true is narrower, and is what this guard now refuses. A URI part
/// on a variable upstream is **static**: it replaces the WHOLE request URI,
/// so nothing of the request survives it -- not the part of the path past
/// the location, and not the query string. Measured, same harness:
///
/// | request | literal `proxy_pass http://h/t` | variable `proxy_pass $u/t` |
/// |---|---|---|
/// | `/loc` | `/t` | `/t` |
/// | `/loc/extra` | `/t/extra` | `/t` |
/// | `/loc?a=b` | `/t?a=b` | `/t` |
///
/// So a variable `proxy_pass` that carries a URI part is only ever correct
/// where the location maps to exactly ONE upstream path and no query string
/// has to survive -- and it silently is not correct otherwise, which reads
/// identically in the file. The fleet's default form stays
/// `rewrite ^.*$ /<target> break;` plus a `proxy_pass` naming host and port
/// only: a bare variable `proxy_pass` forwards the (possibly rewritten)
/// `$uri` AND the query string, which is what a proxy generally wants.
/// Appending `$uri`/`$request_uri` to the `proxy_pass` itself is refused by
/// the same rule -- it double-applies against a `rewrite`, and `$uri` drops
/// the args on its own.
///
/// #987's live 404 therefore had some other cause than the one it names.
/// #999's own body records these files drifting apart ("that is how #987
/// survived"), and a rendered `conf.d/node.conf` on the box carrying a bare
/// `proxy_pass http://$swap_upstream:3400;` would produce exactly the 404
/// that was seen. Its fix is right either way and is untouched.
///
/// # Coverage
///
/// Two shapes were unchecked before #1023 and are checked now:
///
/// * `proxy_pass $var/uri;` -- a variable carrying its own scheme. The old
///   guard only matched a `http://$` prefix, so it `continue`d past the one
///   line in the tree that actually has a URI part on a variable upstream.
/// * the URL literals a `map`/`set` feeds those variables from. The value is
///   where a URI part would hide from a scan of `proxy_pass` lines alone;
///   [`upstream_url_literals`] reads them.
#[test]
fn no_variable_upstream_carries_a_static_uri_part() {
    for (name, raw) in BOX_NGINX_FILES {
        for target in proxy_pass_targets(raw) {
            // A variable upstream, whether the scheme is written in the
            // directive (`http://$upstream:4000`) or carried inside the
            // variable (`$backend`, `$mina_upstream`). A literal upstream is
            // `no_box_nginx_names_a_literal_upstream`'s business, and nginx
            // does do clean prefix substitution for those.
            let after_scheme = strip_url_scheme(target);
            if !after_scheme.starts_with('$') {
                continue;
            }
            if STATIC_URI_PART_EXEMPTIONS
                .iter()
                .any(|(file, exempt, _)| file == name && exempt == &target)
            {
                continue;
            }
            assert!(
                !after_scheme.contains('/'),
                "{name} proxies to `{target}` -- a URI part on a VARIABLE \
                 upstream is honoured, but statically: it replaces the whole \
                 request URI, dropping both the path past the location and \
                 the query string (issue #1023 measured this; #987's \
                 \"nginx ignores it\" is not what nginx does). Write \
                 `rewrite ^.*$ /<target> break;` and a `proxy_pass` with \
                 host and port only, which forwards the rewritten `$uri` \
                 and the args -- or, if the static URI is genuinely what \
                 this location wants, add it to \
                 `STATIC_URI_PART_EXEMPTIONS` with the reason."
            );
        }
        for literal in upstream_url_literals(raw) {
            assert!(
                !strip_url_scheme(literal).contains('/'),
                "{name} feeds an upstream variable the literal `{literal}`, \
                 which carries a URI part. That path reaches `proxy_pass` \
                 through the variable, where the same static-URI rule \
                 applies and no scan of `proxy_pass` lines can see it. Keep \
                 these values host-and-port only and put any path in a \
                 `rewrite`."
            );
        }
    }
}

/// Every `proxy_pass` in the tree that deliberately carries a URI part on a
/// variable upstream, with the reason. An exemption here is a reviewed diff
/// and a named justification rather than a shape the scan quietly walks
/// past, which is what the old `http://$`-only prefix match amounted to.
///
/// [`every_static_uri_part_exemption_still_exists`] fails if an entry stops
/// matching a real line, so a stale exemption is a red test rather than a
/// widening nobody notices.
///
/// It is EMPTY. Its one entry was the chain box's Mina passthrough
/// (`infra/linode/nginx/devnet.conf.template`, `proxy_pass
/// $mina_upstream/node/devnet/v1/graphql`), where the static URI was the
/// whole point: `api.minascan.io` serves the public devnet's GraphQL at one
/// fixed path and 404s `/graphql`. That box's provisioning was deleted with
/// the box (`44b15bdc`, 2026-07-19), so the exemption went the way this test
/// says a stale one should -- dropped, because the line it excused is gone.
/// Nothing in the surviving boxes' nginx needs one.
const STATIC_URI_PART_EXEMPTIONS: &[(&str, &str, &str)] = &[];

/// An exemption that no longer matches anything is worse than none: it reads
/// as a live decision about a line that is gone, and it is one search-and-
/// replace away from silently exempting something else.
#[test]
fn every_static_uri_part_exemption_still_exists() {
    for (file, target, reason) in STATIC_URI_PART_EXEMPTIONS {
        let (_, raw) = BOX_NGINX_FILES
            .iter()
            .find(|(name, _)| name == file)
            .unwrap_or_else(|| panic!("{file} is exempted but is not in BOX_NGINX_FILES"));

        assert!(
            proxy_pass_targets(raw).contains(target),
            "{file} no longer proxies to `{target}`, but \
             STATIC_URI_PART_EXEMPTIONS still exempts it. Drop the entry -- \
             the reason it carried (\"{reason}\") is about a line that is \
             not there."
        );
    }
}

/// `target` with a leading `http://` or `https://` removed, if it had one.
/// Used to ask "is this upstream a variable, and does it carry a path?"
/// without caring whether the scheme was written in the directive or is
/// carried inside the variable.
fn strip_url_scheme(target: &str) -> &str {
    target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))
        .unwrap_or(target)
}

/// Every `"http://..."` / `"https://..."` value an upstream variable is fed
/// from: the values of a `map` block and of a `set` directive. Those are the
/// two ways a URL reaches a `proxy_pass` in these files without appearing on
/// the `proxy_pass` line itself.
///
/// Deliberately NOT every quoted URL in the file -- `add_header
/// Access-Control-Allow-Origin "https://proxy.${DOMAIN}"` is a quoted URL
/// that is not an upstream, and a path in one would be a CORS bug, not this
/// one.
fn upstream_url_literals(raw: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut in_map = false;

    for line in raw.lines().map(str::trim) {
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with("map ") && line.ends_with('{') {
            in_map = true;
            continue;
        }
        if in_map && line.starts_with('}') {
            in_map = false;
            continue;
        }
        if !in_map && !line.starts_with("set ") {
            continue;
        }

        for piece in line.split('"').skip(1).step_by(2) {
            if piece.starts_with("http://") || piece.starts_with("https://") {
                out.push(piece);
            }
        }
    }

    out
}

// ═══════════════════════════════════════════════════════════════════════════
// The rolling-swap maker's own config (issue #983, toon-meta#402)
// ═══════════════════════════════════════════════════════════════════════════

/// The maker's committed config skeleton. Not a connector config -- it is
/// read by `toon-swap --config` from the swap repo -- but it names the same
/// on-chain deployment the rest of this fleet settles on, which is a
/// property this suite already asserts for everything else and is the one
/// that took the maker down live.
const RELAY_SWAP_CONFIG: &str = include_str!("../../../infra/linode-relay/swap.config.json");

/// The deployed `TokenNetwork` for USDC on Base Sepolia
/// (`packages/contracts/deployments.json`, docs/evm-deployment.md), resolved
/// from [`FLEET_LIVE_REGISTRY`]. A literal here for the same reason every
/// other `FLEET_*` address is one.
const FLEET_LIVE_TOKEN_NETWORK: &str = "0xe9E05dfecfe165266C88d73e61D483612651952a";

/// The maker holds TWO EVM contract addresses that are easy to read as one
/// thing and are not:
///
/// * `tokenNetworkAddress` -- the LEG-A `TokenNetwork`. Money coming IN: the
///   ordinary payment-channel contract a taker already holds a funded channel
///   on, and the one the maker VERIFIES an incoming claim against before it
///   quotes. It must be the fleet's one deployment, for the same reason every
///   `[settlement.evm]` section on this fleet must be: a claim resolves
///   against one deployment or it does not resolve.
/// * `channelAddress` -- the LEG-B `RollingSwapChannel` (issues #973/#974).
///   Money going OUT: a different contract with a different ABI, the one the
///   maker SIGNS its own v2 EIP-712 balance proofs against.
///
/// swap#134 made `tokenNetworkAddress` required, and a config carrying only
/// `channelAddress` does NOT fall back to it -- the live maker crash-looped
/// until the field was added on the box. So this asserts three things: the
/// field is present, it names the fleet's `TokenNetwork`, and it is not the
/// same address as `channelAddress`. The last is the whole point: the two
/// being interchangeable is the belief that caused the outage, and a config
/// where they are equal is that belief written down.
#[test]
fn the_makers_leg_a_token_network_is_the_fleets_and_is_not_its_leg_b_channel() {
    let config: serde_json::Value =
        serde_json::from_str(RELAY_SWAP_CONFIG).expect("swap.config.json must be valid JSON");
    let providers = config["chainProviders"]
        .as_array()
        .expect("swap.config.json must carry a `chainProviders` array");
    assert!(
        !providers.is_empty(),
        "swap.config.json's `chainProviders` is empty -- the maker has no \
         chain to verify a claim on"
    );

    for provider in providers {
        let chain_id = provider["chainId"].as_str().unwrap_or("<unset>");
        let token_network = provider["tokenNetworkAddress"].as_str().unwrap_or_else(|| {
            panic!(
                "swap.config.json's `{chain_id}` provider has no \
                 `tokenNetworkAddress`. swap#134 made it REQUIRED and there \
                 is no fallback to `channelAddress`: the live maker \
                 crash-looped on exactly this omission"
            )
        });
        assert_eq!(
            token_network.to_lowercase(),
            FLEET_LIVE_TOKEN_NETWORK.to_lowercase(),
            "swap.config.json's `{chain_id}` names TokenNetwork \
             {token_network}, not the fleet's {FLEET_LIVE_TOKEN_NETWORK} -- \
             the maker would verify a taker's leg-A claim against a \
             deployment no channel on this fleet lives on"
        );

        let channel = provider["channelAddress"]
            .as_str()
            .unwrap_or_else(|| panic!("swap.config.json's `{chain_id}` has no `channelAddress`"));
        assert_ne!(
            channel.to_lowercase(),
            token_network.to_lowercase(),
            "swap.config.json's `{chain_id}` gives `channelAddress` and \
             `tokenNetworkAddress` the SAME address. They are different \
             contracts with different ABIs -- leg A is the TokenNetwork an \
             incoming claim is verified against, leg B is the \
             RollingSwapChannel the maker signs its payout against"
        );
    }

    assert_eq!(
        config["chainProviders"][0]["registryAddress"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase(),
        FLEET_LIVE_REGISTRY.to_lowercase(),
        "swap.config.json must name the same TokenNetworkRegistry as the \
         rest of the fleet"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// The public endpoints document (`infra/linode/endpoints.json`)
// ═══════════════════════════════════════════════════════════════════════════

/// The hand-maintained document a third party configures itself from
/// (`infra/linode/README.md`: "live and hand-maintained"). Not a connector
/// config -- no binary loads it -- but it publishes the same Base Sepolia
/// deployment the two box TOMLs settle on, and it publishes one thing they
/// do not: the resolved `TokenNetwork` itself.
///
/// That extra field is why it needs a guard of its own. A connector is
/// configured with the REGISTRY (`[settlement.evm] contract_address`) and
/// resolves the `TokenNetwork` through it at boot, so
/// [`FLEET_LIVE_REGISTRY`] is the whole of what
/// [`every_fleet_configs_settlement_evm_leg_matches_the_live_identity`] has
/// to hold the fleet to. This file states the DERIVED answer as a literal,
/// and a literal cannot re-derive itself when the registry moves.
///
/// It did not. The 2026-08-06 ERC-2771 cutover (#695/#811,
/// `docs/evm-deployment.md`) repointed `registryAddress` in both of this
/// file's blocks and left `tokenNetworkUsdc` -- the next line down, in both
/// -- naming `0x1E95493f…`, the 2026-07-18 contract the cutover replaced. It
/// stood wrong for three weeks. Nothing broke on the fleet, because nothing
/// on the fleet reads it; a third party that read it opened channels on a
/// contract the live registry does not resolve.
const ENDPOINTS_JSON: &str = include_str!("../../../infra/linode/endpoints.json");

/// The two blocks of `endpoints.json` that describe this fleet's EVM chain.
/// `baseSepolia` is a declared mirror of `evm` ("Mirror of the evm block",
/// its own `_note`) kept for consumers that read that key -- so both are
/// held to the same values, and neither is ever checked against the other.
///
/// Checking them against each other is the guard that would have passed:
/// the cutover left BOTH copies stale, identically. Only a literal that the
/// broadcast record moves can catch a value that stopped tracking the chain.
const ENDPOINTS_EVM_BLOCKS: [&str; 2] = ["evm", "baseSepolia"];

/// `endpoints.json`'s EVM blocks must name the fleet's live deployment, and
/// `tokenNetworkUsdc` in particular must be the `TokenNetwork` that
/// [`FLEET_LIVE_REGISTRY`] resolves [`EXPECTED_SETTLEMENT_TOKEN_ADDRESS`] to
/// -- which is what [`FLEET_LIVE_TOKEN_NETWORK`] records.
///
/// **Deliberately offline.** The honest statement of this property is
/// `registry.getTokenNetwork(token) == tokenNetworkUsdc`, and that is an
/// `eth_call`. The workspace gate runs on every push and must not need a
/// chain (ADR 0009's fail-closed boot is exactly the network dependency the
/// verbatim cases above substitute away), so the chain half lives in
/// `.github/workflows/base-sepolia-redeem-gate.yml`, whose dry run already
/// resolves the registry against Base Sepolia and now compares that answer
/// to this very file. The two halves compose: this test pins the document to
/// the constant, and that job pins the constant to the chain. Either alone
/// would have missed this -- a chain check nothing dispatches, or a document
/// check with nothing behind the number.
#[test]
fn the_public_endpoints_document_names_the_fleets_live_evm_deployment() {
    let endpoints: serde_json::Value =
        serde_json::from_str(ENDPOINTS_JSON).expect("endpoints.json must be valid JSON");

    for block in ENDPOINTS_EVM_BLOCKS {
        let chain = endpoints
            .get(block)
            .unwrap_or_else(|| panic!("endpoints.json has no `{block}` block"));

        let field = |name: &str| -> String {
            chain
                .get(name)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("endpoints.json's `{block}` block has no `{name}`"))
                .to_lowercase()
        };

        assert_eq!(
            field("registryAddress"),
            FLEET_LIVE_REGISTRY.to_lowercase(),
            "endpoints.json's `{block}.registryAddress` is not the fleet's \
             TokenNetworkRegistry {FLEET_LIVE_REGISTRY}. A payer configured \
             from this document would open its channel through a different \
             registry than the one both box TOMLs name in `[settlement.evm] \
             contract_address`"
        );

        assert_eq!(
            field("tokenNetworkUsdc"),
            FLEET_LIVE_TOKEN_NETWORK.to_lowercase(),
            "endpoints.json's `{block}.tokenNetworkUsdc` is not \
             {FLEET_LIVE_TOKEN_NETWORK}, the TokenNetwork that \
             {FLEET_LIVE_REGISTRY} resolves \
             {EXPECTED_SETTLEMENT_TOKEN_ADDRESS} to on Base Sepolia. This \
             field is DERIVED from the two above it and cannot re-derive \
             itself: if the registry moved, this moves with it in the same \
             commit. It did not on 2026-08-06, and this document advertised \
             a retired contract for three weeks"
        );

        assert_eq!(
            field("tokenAddress"),
            EXPECTED_SETTLEMENT_TOKEN_ADDRESS.to_lowercase(),
            "endpoints.json's `{block}.tokenAddress` is not the mock USDC \
             this fleet settles in. The ERC-2771 cutover registered the SAME \
             token through a new registry ({FLEET_LIVE_REGISTRY}), so a \
             different token here is not a cutover -- it is a different \
             currency"
        );

        assert_eq!(
            chain
                .get("tokenDecimals")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(EXPECTED_SETTLEMENT_DECIMALS)),
            "endpoints.json's `{block}.tokenDecimals` is not \
             {EXPECTED_SETTLEMENT_DECIMALS}. ADR 0010's uniform scale is what \
             lets a claim's base units mean the same thing on every chain"
        );

        assert_eq!(
            chain.get("chainId").and_then(serde_json::Value::as_u64),
            Some(84_532),
            "endpoints.json's `{block}.chainId` is not Base Sepolia's 84532. \
             The chain id is half of the EIP-712 domain a claim is signed \
             under, so a wrong one here produces signatures the fleet's \
             TokenNetwork rejects"
        );
    }

    assert!(
        !ENDPOINTS_JSON.contains(SETTLEMENT_CONTRACT_ADDRESS_ROLLBACK_TARGET),
        "endpoints.json names the retired pre-ERC-2771 registry \
         {SETTLEMENT_CONTRACT_ADDRESS_ROLLBACK_TARGET}. That address is the \
         rollback target in docs/evm-deployment.md and nothing this document \
         should advertise -- a rollback repoints the boxes and this file \
         together, in one commit, not this file on its own"
    );
}

/// The retired `TokenNetwork` by name, so the failure says what came back
/// rather than leaving a reader to recognise an address on sight -- the same
/// service [`SETTLEMENT_CONTRACT_ADDRESS_ROLLBACK_TARGET`] does for the
/// registry it replaced.
///
/// It is a separate case from the identity check above because it asks a
/// different question. That one asks whether the live values are right;
/// this asks whether the dead one is gone from the whole document, `_note`
/// prose included -- except the one deliberate mention, the `_tokenNetworkNote`
/// that records this exact defect. A stale address surviving in a comment is
/// how the next reader gets it back.
const RETIRED_PRE_CUTOVER_TOKEN_NETWORK: &str = "0x1E95493fEF46707E034b4a1945f25a8C76A1823D";

#[test]
fn the_public_endpoints_document_advertises_the_retired_token_network_nowhere() {
    let mentions = ENDPOINTS_JSON
        .to_lowercase()
        .matches(&RETIRED_PRE_CUTOVER_TOKEN_NETWORK[..10].to_lowercase())
        .count();

    assert_eq!(
        mentions, 1,
        "endpoints.json mentions the retired pre-ERC-2771 TokenNetwork \
         {RETIRED_PRE_CUTOVER_TOKEN_NETWORK} {mentions} times; exactly one is \
         expected, the `_tokenNetworkNote` that records why this field went \
         stale on 2026-08-06 and how to re-derive it. Zero means that note \
         was deleted and the lesson with it; more than one means the address \
         is being advertised again somewhere in the document"
    );
}
