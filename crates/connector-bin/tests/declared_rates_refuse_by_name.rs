//! What an operator may declare about denomination, and the five ways a
//! declaration stops the node at boot
//! ([ADR 0071](../../../docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)
//! decisions 3 and 5, issue #1290).
//!
//! Black-box, against the compiled binary, for the same reason
//! `refuses_to_start.rs` is: ADR 0009's whole claim is that a bad
//! declaration stops the **process** before anything is bound, and a
//! library call proves the check without proving where it runs. Each
//! refusal asserts on the words an operator actually reads on stderr, not
//! merely on a non-zero exit -- a node that refuses for the wrong stated
//! reason sends its operator to the wrong line of the file.
//!
//! The positive cases go through `Config::load` rather than the binary,
//! because a config carrying a `[settlement.evm]` table boots a real
//! settlement backend and would need a chain; what is under test here is
//! the declaration, and `Config::load` is the whole of it (a loaded
//! `Config` needs no further validation anywhere downstream).
//!
//! # The rule that protects everyone not doing any of this
//!
//! A node that declares none of it loads and behaves exactly as it does
//! today. [`a_config_declaring_no_tokens_declares_nothing`] is that as a
//! unit, and [`every_committed_fixture_loads_and_declares_no_tokens`] is
//! that over every config file this repository ships -- asserted by loading
//! them, not by reading them.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use connector_config::{Config, DenominationConfig};

/// USDC on Base, the numeraire throughout.
const USDC: &str = "evm:0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
/// ANYONE on Base -- 18 decimals, and quoted in WETH rather than in a
/// stable, which is why ADR 0071 decision 3 has two-leg quote paths at all.
const ANYONE: &str = "evm:0x9ff58f4fFB29fA2266Ab25e75e2A8b3503311656";
/// WETH on Base: an intermediate a quote path passes through, never a token
/// this node deals.
const WETH: &str = "evm:0x4200000000000000000000000000000000000006";
const POOL_ANYONE_WETH: &str = "0x1111111111111111111111111111111111111111";
const POOL_WETH_USDC: &str = "0x2222222222222222222222222222222222222222";

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

/// The smallest loadable node, plus `declaration`. No `[settlement]` table,
/// so this is the node that has RPC for no chain at all -- which is exactly
/// what the quote-without-settlement refusal is about.
fn config_without_settlement(key_file: &Path, declaration: &str) -> String {
    format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"
{declaration}
"#,
        key_file.display()
    )
}

/// The same node with a `[settlement.evm]` table -- an EVM chain it has RPC
/// for, and therefore a chain a quote path may name. `state_dir` comes with
/// it: a node that can be paid by a stranger must be able to remember
/// having been (issue #605), and `Config::load` refuses a settlement table
/// without one.
fn config_with_evm_settlement(key_file: &Path, state_dir: &Path, declaration: &str) -> String {
    format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{}"

[signer]
key_file = "{}"

[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
decimals = 6

[settlement.evm.key]
key_file = "{}"
{declaration}
"#,
        state_dir.display(),
        key_file.display(),
        key_file.display()
    )
}

/// The whole shape at once, as an operator dealing ANYONE against a USDC
/// book would write it: a numeraire, a WETH-quoted token whose price is
/// read from two pools, a static row for the pair no pool on this chain can
/// source, and a node dealing policy one row tightens.
fn a_full_declaration() -> String {
    format!(
        r#"
[[tokens]]
asset = "{USDC}"
numeraire = true

[[tokens]]
asset = "{ANYONE}"
quote = [
  {{ pool = "{POOL_ANYONE_WETH}", quote_token = "{WETH}", twap_window_secs = 1800 }},
  {{ pool = "{POOL_WETH_USDC}", quote_token = "{USDC}", twap_window_secs = 900 }},
]

[[rates]]
from = "{ANYONE}"
to = "{USDC}"
rate = {{ numerator = 1, denominator = 4200000000000 }}
spread = {{ numerator = 120, denominator = 10000 }}

[rate_guards]
spread = {{ numerator = 30, denominator = 10000 }}
ttl_secs = 300
max_move = {{ numerator = 5, denominator = 100 }}
"#
    )
}

#[test]
fn a_full_declaration_loads_and_every_value_is_reachable() {
    let key_file = write_raw_key_file();
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let config_file = write_config(&config_with_evm_settlement(
        key_file.path(),
        state_dir.path(),
        &a_full_declaration(),
    ));

    let config = Config::load(config_file.path()).expect("a declaring config must load");
    let declared = config.denomination();

    assert!(declared.declares_tokens());
    assert_eq!(
        declared.numeraire().map(ToString::to_string),
        Some(USDC.to_ascii_lowercase())
    );

    let anyone = ANYONE.parse().expect("an asset");
    let usdc = USDC.parse().expect("an asset");
    let token = declared
        .token(&anyone)
        .expect("the dealt token resolves from the loaded config alone");
    let quote = token.quote().expect("its quote path is reachable");
    assert_eq!(quote.legs().len(), 2);
    assert_eq!(quote.legs()[0].pool(), POOL_ANYONE_WETH);
    assert_eq!(
        quote.legs()[1].quote_token().to_string(),
        USDC.to_ascii_lowercase(),
        "the last leg lands on the numeraire, which is what makes composition sound"
    );

    let rate = declared
        .rate(&anyone, &usdc)
        .expect("the static row's rate is reachable");
    assert_eq!(rate.numerator(), 1);
    assert_eq!(rate.denominator(), 4_200_000_000_000);

    let guards = declared
        .guards_for(&anyone, &usdc)
        .expect("the pair's guards are the node's, with this row's override applied");
    assert_eq!(
        guards.spread().to_string(),
        "120/10000",
        "the row's own spread overrides the node's 30/10000"
    );
    assert_eq!(
        guards.ttl().as_time_delta().num_seconds(),
        300,
        "and leaves the two it says nothing about alone"
    );
    assert_eq!(guards.max_move().to_string(), "5/100");

    // Direction is the trade: the reverse pair is a different row, and this
    // config declares none.
    assert_eq!(declared.rate(&usdc, &anyone), None);
}

/// ADR 0071 decision 3's mixed numeraire. Two pegs means every cross rate
/// composed through them prices the peg between them at exactly 1 -- a peg
/// nobody declared, which holds right up until it does not.
#[test]
fn exits_non_zero_on_a_mixed_numeraire() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&config_without_settlement(
        key_file.path(),
        &format!(
            r#"
[[tokens]]
asset = "{USDC}"
numeraire = true

[[tokens]]
asset = "{ANYONE}"
numeraire = true
"#
        ),
    ));

    let output = run(config_file.path());

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("two numeraires"),
        "expected a named mixed-numeraire error, got: {stderr}"
    );
}

/// The zero denominator. The check itself is `connector_domain::Rate`'s --
/// a rate with one is unconstructable -- and what this asserts is that its
/// refusal reaches boot naming the **row**, since "there is no rational
/// over zero" on its own does not tell an operator which pair to go and
/// look at.
#[test]
fn exits_non_zero_on_a_zero_denominator() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&config_without_settlement(
        key_file.path(),
        &format!(
            r#"
[[tokens]]
asset = "{USDC}"
numeraire = true

[[tokens]]
asset = "{ANYONE}"

[[rates]]
from = "{USDC}"
to = "{ANYONE}"
rate = {{ numerator = 3, denominator = 0 }}

[rate_guards]
spread = {{ numerator = 30, denominator = 10000 }}
ttl_secs = 300
max_move = {{ numerator = 5, denominator = 100 }}
"#
        ),
    ));

    let output = run(config_file.path());

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("denominator") && stderr.contains(&ANYONE.to_ascii_lowercase()),
        "expected the refusal to carry the domain's words AND name the row, got: {stderr}"
    );
}

/// A rate filed under a token no `[[tokens]]` row declares is a rate no
/// forward ever looks up, which is a pair that refuses while the file reads
/// as configured.
#[test]
fn exits_non_zero_on_a_rate_row_naming_an_undeclared_token() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&config_without_settlement(
        key_file.path(),
        &format!(
            r#"
[[tokens]]
asset = "{USDC}"
numeraire = true

[[rates]]
from = "{USDC}"
to = "{WETH}"
rate = {{ numerator = 1, denominator = 2 }}

[rate_guards]
spread = {{ numerator = 30, denominator = 10000 }}
ttl_secs = 300
max_move = {{ numerator = 5, denominator = 100 }}
"#
        ),
    ));

    let output = run(config_file.path());

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a token this node deals") && stderr.contains(WETH),
        "expected the refusal to name the undeclared token, got: {stderr}"
    );
}

/// A quote whose last leg answers in WETH rather than in the numeraire.
/// The refusal that makes composition sound: `(X/numeraire) / (Y/numeraire)`
/// treats every quote as though it answered in the numeraire, so one that
/// does not is not a wrong number -- it is a number in a unit nothing
/// records.
#[test]
fn exits_non_zero_on_a_quote_that_does_not_end_at_the_numeraire() {
    let key_file = write_raw_key_file();
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let config_file = write_config(&config_with_evm_settlement(
        key_file.path(),
        state_dir.path(),
        &format!(
            r#"
[[tokens]]
asset = "{USDC}"
numeraire = true

[[tokens]]
asset = "{ANYONE}"
quote = [
  {{ pool = "{POOL_ANYONE_WETH}", quote_token = "{WETH}", twap_window_secs = 1800 }},
]

[rate_guards]
spread = {{ numerator = 30, denominator = 10000 }}
ttl_secs = 300
max_move = {{ numerator = 5, denominator = 100 }}
"#
        ),
    ));

    let output = run(config_file.path());

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not at the numeraire") && stderr.contains(WETH),
        "expected the refusal to name where the path actually ends, got: {stderr}"
    );
}

/// A quote on a chain the node has no `[settlement.<chain>]` table for. A
/// quote is read over that table's own RPC endpoint (ADR 0071 decision 3
/// puts a token's quote on its own settlement chain for exactly that
/// reason), so this is a poller that could never take its first reading.
#[test]
fn exits_non_zero_on_a_quote_whose_chain_has_no_settlement_table() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&config_without_settlement(
        key_file.path(),
        &format!(
            r#"
[[tokens]]
asset = "{USDC}"
numeraire = true

[[tokens]]
asset = "{ANYONE}"
quote = [
  {{ pool = "{POOL_WETH_USDC}", quote_token = "{USDC}", twap_window_secs = 1800 }},
]

[rate_guards]
spread = {{ numerator = 30, denominator = 10000 }}
ttl_secs = 300
max_move = {{ numerator = 5, denominator = 100 }}
"#
        ),
    ));

    let output = run(config_file.path());

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[settlement.evm]") && stderr.contains(&ANYONE.to_ascii_lowercase()),
        "expected the refusal to name the missing settlement table, got: {stderr}"
    );
}

/// `deny_unknown_fields` reaches inside every new table too (ADR 0009): a
/// key misspelled in one is a line silently dropped and a node running on
/// a policy nobody wrote, which is the failure the posture exists for.
#[test]
fn exits_non_zero_on_a_misspelled_key_inside_a_new_table() {
    for (declaration, misspelling) in [
        (
            format!("[[tokens]]\nasset = \"{USDC}\"\nnumeraire_token = true\n"),
            "numeraire_token",
        ),
        (
            format!(
                "[[tokens]]\nasset = \"{USDC}\"\n\n[[rates]]\nfrom = \"{USDC}\"\nto = \
                 \"{ANYONE}\"\nspread_bps = 30\n"
            ),
            "spread_bps",
        ),
        (
            "[rate_guards]\nspread = { numerator = 30, denominator = 10000 }\nttl_seconds = \
             300\nmax_move = { numerator = 5, denominator = 100 }\n"
                .to_string(),
            "ttl_seconds",
        ),
    ] {
        let key_file = write_raw_key_file();
        let config_file = write_config(&config_without_settlement(key_file.path(), &declaration));

        let output = run(config_file.path());

        assert!(!output.status.success(), "'{misspelling}' must not load");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(misspelling),
            "expected the error to name the misspelled key '{misspelling}', got: {stderr}"
        );
    }
}

/// The rule everyone not dealing is protected by, as a unit: a config that
/// declares none of this loads, and every lookup answers nothing rather
/// than something.
#[test]
fn a_config_declaring_no_tokens_declares_nothing() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&config_without_settlement(key_file.path(), ""));

    let config = Config::load(config_file.path()).expect("a config declaring nothing must load");
    let declared = config.denomination();

    assert_eq!(declared, &DenominationConfig::default());
    assert!(!declared.declares_tokens());
    assert_eq!(declared.numeraire(), None);
    assert_eq!(declared.guards(), None);
    assert_eq!(declared.quoted_tokens().count(), 0);
    assert_eq!(
        declared.rate(
            &USDC.parse().expect("an asset"),
            &ANYONE.parse().expect("an asset")
        ),
        None,
        "an absent rate is not 1:1 and never becomes it (ADR 0071 decision 2)"
    );
}

/// The same rule over every config file this repository ships, asserted by
/// **loading** each one rather than by reading it.
///
/// The walk is over the tree rather than over a hand-kept list, for the
/// reason `devnet_configs_load.rs`'s own nginx walk gives: a list of files
/// to check drifts from the files that exist, and the drift is invisible.
/// Its one exemption is the production skeleton, which
/// `production_skeleton_is_inert.rs` owns and whose every value is invalid
/// on purpose.
///
/// Only what this sandbox physically cannot supply is substituted -- key
/// files (real key material is never committed), the operator credential
/// files, `state_dir` (a container path) and the bind address (a fixed port
/// collides across parallel runs) -- keyed on the setting's NAME rather
/// than on the committed value, so a fixture that moves a path still loads
/// here. Every other line, `[settlement]` values included, is the literal
/// committed content.
#[test]
fn every_committed_fixture_loads_and_declares_no_tokens() {
    let fixtures = committed_config_fixtures();
    assert!(
        fixtures.len() >= 10,
        "expected to find the committed connector configs under infra/, deploy/ and local/, \
         found {}: {fixtures:?}",
        fixtures.len()
    );

    let sandbox = Sandbox::new();
    for path in fixtures {
        let text = sandbox.rewrite(&std::fs::read_to_string(&path).expect("read a fixture"));
        let config_file = write_config(&text);
        let config = Config::load(config_file.path())
            .unwrap_or_else(|error| panic!("{} must still load: {error}", path.display()));
        assert_eq!(
            config.denomination(),
            &DenominationConfig::default(),
            "{} declares a token, which no committed fixture does yet -- and the point of \
             this check is that ADR 0071 costs a node that declares none of it nothing",
            path.display()
        );
    }
}

/// Every `*.toml` under `infra/`, `deploy/` and `local/` that is a
/// connector config, minus the production skeleton.
fn committed_config_fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let mut found = Vec::new();
    let mut pending = vec![root.join("infra"), root.join("deploy"), root.join("local")];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("a readable directory entry").path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            // `local/.keys/` is gitignored working material a topology
            // renders at run time, not a committed fixture.
            if path.is_dir() && !name.starts_with('.') {
                pending.push(path);
                continue;
            }
            // ADR 0056: named and empty. Every value in it is invalid on
            // purpose and `production_skeleton_is_inert.rs` is what asserts
            // it stays that way.
            if name == "connector.production.toml" {
                continue;
            }
            if path
                .extension()
                .is_some_and(|extension| extension == "toml")
                && std::fs::read_to_string(&path)
                    .is_ok_and(|text| text.contains("client_edge_addr"))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// One ed25519 public key, in the 64-hex shape `[operator] write_keys`
/// takes. Content-free: nothing here signs anything, and the private half
/// never existed.
const OPERATOR_WRITE_KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn quoted(key: &str, value: &str) -> String {
    format!("{key} = \"{value}\"")
}

/// The files and values a committed fixture names but a test host cannot
/// have, each created once and substituted into every fixture.
struct Sandbox {
    key_file: tempfile::NamedTempFile,
    bearer_token: tempfile::NamedTempFile,
    write_keys: tempfile::NamedTempFile,
    state_dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        let mut bearer_token = tempfile::NamedTempFile::new().expect("temp bearer token file");
        write!(bearer_token, "sandbox-operator-token").expect("write bearer token");
        let mut write_keys = tempfile::NamedTempFile::new().expect("temp write keys file");
        writeln!(write_keys, "{OPERATOR_WRITE_KEY}").expect("write operator write keys");
        Sandbox {
            key_file: write_raw_key_file(),
            bearer_token,
            write_keys,
            state_dir: tempfile::tempdir().expect("temp state dir"),
        }
    }

    /// Rewrite every assignment this sandbox owns, line by line and keyed
    /// on the setting's name, leaving comments and every other line exactly
    /// as committed.
    fn rewrite(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for line in text.lines() {
            let trimmed = line.trim_start();
            let key = trimmed
                .split('=')
                .next()
                .unwrap_or_default()
                .trim_end()
                .to_string();
            let substitute = match key.as_str() {
                _ if trimmed.starts_with('#') => None,
                "key_file" => Some(quoted(&key, &self.key_file.path().display().to_string())),
                "bearer_token_file" => Some(quoted(
                    &key,
                    &self.bearer_token.path().display().to_string(),
                )),
                "write_keys_file" => {
                    Some(quoted(&key, &self.write_keys.path().display().to_string()))
                }
                "state_dir" => Some(quoted(&key, &self.state_dir.path().display().to_string())),
                "client_edge_addr" => Some(quoted(&key, "127.0.0.1:0")),
                // `deploy/connector-rust/connector.toml`'s own header says
                // its placeholders are "deliberately invalid ... or fail to
                // parse", so the operator surface it teaches is supplied
                // here the way step 3 of its README tells an operator to.
                "write_keys" => Some(format!("write_keys = [\"{OPERATOR_WRITE_KEY}\"]")),
                _ => None,
            };
            match substitute {
                Some(replacement) => out.push_str(&replacement),
                None => out.push_str(line),
            }
            out.push('\n');
        }
        out
    }
}
