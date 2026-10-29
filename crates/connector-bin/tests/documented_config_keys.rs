//! Guards the committed config-key table against the parser (issue #900).
//!
//! `README.md`'s "Configure" section documented `peer_wire_addr`, a key the
//! parser refuses by name -- an operator who copied it got a node that
//! refused to start (connector#849). `refuses_to_start.rs` proves the
//! parser's side of that; nothing proved the table's. This file reads the
//! committed table and, for every key it names, hands the same parser a
//! probe config setting that key and asserts it is not rejected as an
//! unrecognized or removed key.
//!
//! **The table moved** (issue #1173). It used to be the README's; the README
//! is now an operator's guide and the key table lives in
//! `docs/protocol/configuration-spec.md` §2.1, which is the document that
//! already owned the configuration contract and carries the CF- rules each
//! key expresses. Nothing about the check changed -- it reads whatever rows
//! the committed table has -- but the file it reads did, and a gate that
//! silently reads the wrong file is worse than no gate, so the section
//! lookup below panics rather than defaulting when it cannot find the table.
//!
//! Deliberately does not read a second, hand-maintained list of "the keys
//! the spec should have" -- that list would drift from the table under test
//! exactly the way the table drifted from the parser. The table's own rows
//! are the input.

use std::io::Write;
use std::path::Path;

use connector_config::{Config, ConfigError};

const CONFIG_SPEC: &str = include_str!("../../../docs/protocol/configuration-spec.md");

/// The path, for error messages that have to tell somebody where to look.
const CONFIG_SPEC_PATH: &str = "docs/protocol/configuration-spec.md";

/// §2.1's text, up to (not including) the next heading -- scopes every later
/// step to the one table this check is about, so a backtick anywhere else in
/// the document (there are hundreds) can never be misread as a config key.
///
/// §2.1 specifically, not the whole of §2: §2.3 is the **tombstones**, keys
/// the parser refuses by name on purpose. Feeding those to the probe would
/// assert the exact opposite of what this file is for.
fn top_level_key_section(spec: &str) -> &str {
    let heading = "\n### 2.1 Top level\n";
    let start = spec.find(heading).unwrap_or_else(|| {
        panic!(
            "{CONFIG_SPEC_PATH} has no '### 2.1 Top level' section. That section holds the \
             config-key table this gate reads; if it was renamed or moved, repoint this file \
             rather than deleting the check."
        )
    }) + heading.len();
    let rest = &spec[start..];
    let end = rest.find("\n### ").unwrap_or(rest.len());
    &rest[..end]
}

/// The key-table's data rows, as the literal backticked text of each row's
/// first column -- `client_edge_addr`, `[signer]`, `[[routes]]`, and so on.
///
/// A markdown table row here is `| `key` | ... |`; the header row (`| key |
/// ...`) and the separator row (`| --- | ...`) don't start their first cell
/// with a backtick and are excluded by that shape, not by skipping a fixed
/// number of lines -- so inserting or reordering rows can't desync this
/// from the table it reads.
///
/// **Every** backticked token in the cell is taken, not just the first,
/// because a row may legitimately name two spellings of one thing:
/// `[settlement.eip155.<chainId>]` / `[settlement.solana]` is one row and two keys, and
/// reading only the first would quietly stop checking the second.
fn table_keys(section: &str) -> Vec<String> {
    section
        .lines()
        .filter_map(|line| {
            let first_cell = line.split('|').nth(1)?.trim();
            if !first_cell.starts_with('`') {
                return None;
            }
            let keys: Vec<String> = first_cell
                .split('`')
                .skip(1)
                .step_by(2)
                .filter(|token| !token.is_empty())
                .map(|token| token.to_string())
                .collect();
            (!keys.is_empty()).then_some(keys)
        })
        .flatten()
        .collect()
}

/// What TOML shape a table key's own spelling says it is: `[[routes]]` is
/// an array of tables, `[signer]` is a table, anything else (`apex`,
/// `peer_expose`, ...) is a plain key. Read off the key text itself so
/// nothing here has to hand-maintain a second copy of which section each
/// key lives in.
#[derive(Clone, Copy)]
enum Shape {
    Scalar,
    Table,
    Array,
}

/// A table whose header carries a placeholder segment -- `<chainId>` in
/// `[settlement.eip155.<chainId>]` (ADR 0086) -- is probed with a real chain
/// id in its place: the placeholder is not a TOML key, so leaving it in would
/// fail every probe on syntax and check nothing.
fn concrete(field: &str) -> String {
    field.replace("<chainId>", "8453")
}

fn parse_key(raw: &str) -> (&str, Shape) {
    if let Some(inner) = raw.strip_prefix("[[").and_then(|s| s.strip_suffix("]]")) {
        (inner, Shape::Array)
    } else if let Some(inner) = raw.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        (inner, Shape::Table)
    } else {
        (raw, Shape::Scalar)
    }
}

/// A probe config: the minimal document `Config::load` accepts
/// (`client_edge_addr` plus a `[signer]` pointing at a real, if content-free,
/// key file -- `SecretLocation::resolve` only checks the path exists), with
/// `field` added in the shape its README spelling calls for and, for a
/// scalar, set to a dummy string value.
///
/// `key_file` has to name a real file: `Config::load` resolves the signer
/// before it ever reaches most other fields, so a missing key file would
/// fail every probe on that alone and never exercise the key under test.
///
/// A scalar is emitted before `[signer]` and a table or array after it,
/// because TOML reads every bare key that follows a section header as
/// belonging to that section -- which is why the two aren't one match.
fn probe_toml(field: &str, shape: Shape, key_file: &Path) -> String {
    let mut doc = String::from("client_edge_addr = \"127.0.0.1:0\"\n");
    if matches!(shape, Shape::Scalar) && field != "client_edge_addr" {
        doc.push_str(&format!("{field} = \"dummy\"\n"));
    }
    doc.push_str(&format!(
        "\n[signer]\nkey_file = \"{}\"\n",
        key_file.display()
    ));
    match shape {
        Shape::Table if field != "signer" => doc.push_str(&format!("\n[{field}]\n")),
        Shape::Array => doc.push_str(&format!("\n[[{field}]]\n")),
        _ => {}
    }
    doc
}

/// One probe, end to end: build the document for `field`, write it to a real
/// file next to a real (empty) signer key file, and hand it to the same
/// `Config::load` the binary uses. Returns the document alongside the
/// parser's verdict so a failure can quote the exact text that produced it.
fn load_probe(field: &str, shape: Shape) -> (String, Result<Config, ConfigError>) {
    let key_file = tempfile::NamedTempFile::new().expect("temp signer key file");
    let text = probe_toml(field, shape, key_file.path());

    let mut config_file = tempfile::NamedTempFile::new().expect("temp probe config file");
    write!(config_file, "{text}").expect("write probe config");

    let outcome = Config::load(config_file.path());
    (text, outcome)
}

/// Whether an error's message names its key as one this parser does not
/// currently accept, rather than some unrelated problem with the dummy
/// value or the rest of the probe (a missing field inside an empty
/// `[[routes]]` entry, say -- that's a real key rejecting a fake value,
/// not a fake key).
///
/// The two phrasings below are this crate's own, consistent vocabulary for
/// "this key is not a thing you may write": serde's `deny_unknown_fields`
/// message for a key no `Raw*` struct declares at all, and this codebase's
/// own convention (`error.rs`'s `PeerWireAddrRemoved`,
/// `RawPeer.addr`'s reader) for a key that used to be read and now exists
/// only to name itself removed.
fn names_an_unrecognized_key(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("unknown field") || lower.contains("was removed")
}

#[test]
fn every_documented_config_key_is_known_to_the_parser() {
    let keys = table_keys(top_level_key_section(CONFIG_SPEC));
    assert!(
        keys.len() >= 10,
        "expected to find {CONFIG_SPEC_PATH} §2.1's key-table rows, found {}: {keys:?} -- \
         did the table move or change shape?",
        keys.len(),
    );

    for raw_key in &keys {
        let (field, shape) = parse_key(raw_key);
        let (text, outcome) = load_probe(&concrete(field), shape);

        if let Err(error) = outcome {
            let message = error.to_string();
            assert!(
                !names_an_unrecognized_key(&message),
                "{CONFIG_SPEC_PATH} §2.1 documents `{raw_key}`, but connector-config's \
                 parser rejects it as unknown or removed: {message}\n\nprobe config:\n{text}"
            );
        }
    }
}

/// A negative control on the mechanism itself: `peer_wire_addr` is a real
/// key this parser refuses by name (ADR 0027, issue #679) -- the same
/// defect this whole file exists to catch if it ever reappeared in the
/// documented table. If the probe ever stopped flagging this key, the test
/// above would be green no matter what the table said.
#[test]
fn the_probe_flags_a_key_the_parser_actually_refuses() {
    let (text, outcome) = load_probe("peer_wire_addr", Shape::Scalar);

    let error = outcome.expect_err("peer_wire_addr must be refused");
    assert!(
        names_an_unrecognized_key(&error.to_string()),
        "expected the probe to flag peer_wire_addr as removed, got: {error}\
         \n\nprobe config:\n{text}"
    );
}
