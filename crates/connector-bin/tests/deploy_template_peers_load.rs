//! Proves `deploy/connector-rust/connector.toml`'s own commented `[[peers]]`
//! peering example still boots on the current binary (issue #1221).
//!
//! ADR 0060 deleted `[[peers]].credential` outright -- it is parsed solely
//! to be refused by name (`ConfigError::PeerCredentialRemoved`). The
//! template's example predated that deletion and, until this test, nothing
//! caught it teaching a shape `Config::load` refuses on sight: uncommenting
//! it as written was a load failure, the same defect #1178 fixed one file
//! over in `peer-carriage-spec.md`.
//!
//! This test takes the template's commented peering block verbatim -- only
//! the leading `# ` comment markers come off -- and supplies the two things
//! the template itself deliberately leaves unconfigured: real (if
//! content-free) key files, and a `[settlement.evm]` table with its
//! `batch_settlement` sub-table, which the example's EVM `[[peer_channels]]`
//! row requires (issues #1138 and #1380) and
//! which is out of this example's scope to teach. If a future edit
//! reintroduces a removed key -- a credential, a `ceiling`, a
//! `claim_enforcement` -- `Config::load` refuses it by name and this test
//! fails with that exact message.

use std::path::{Path, PathBuf};

use connector_config::Config;

const TEMPLATE: &str = include_str!("../../../deploy/connector-rust/connector.toml");

/// The line the template's own prose tells an operator to uncomment first
/// (see the "NOTE: uncommenting this block" comment above it). Everything
/// from here to end of file is the `#`-prefixed peering example.
const PEERING_EXAMPLE_MARKER: &str = "\n# [[peers]]\n";

/// `peer_expose` is a root-level key (issue #1221): TOML has no way to
/// write a root-table key once a table header has appeared earlier in the
/// file, so it lives near the top of the template, beside `state_dir`, and
/// not in the `[[peers]]` block [`PEERING_EXAMPLE_MARKER`] bounds.
/// Uncommented here the same way the template's own prose tells an
/// operator to.
const PEER_EXPOSE_LINE: &str = "# peer_expose = \"btp\"";

/// The peering example's dial endpoint, named once because two tests key
/// off it: one loads the example that contains it, the other reintroduces a
/// `credential` next to it.
const PEER_ENDPOINT_LINE: &str = "endpoint = \"wss://store.example.net:443/ilp/btp\"";

/// Strip exactly one leading `#`, and the space after it if there is one,
/// from every line of a comment block -- the shape the template's own
/// comments use throughout. Panics on a line that isn't commented, since
/// that means [`PEERING_EXAMPLE_MARKER`] no longer bounds what this test
/// thinks it bounds.
fn uncomment(block: &str) -> String {
    block
        .lines()
        .map(|line| {
            if line.is_empty() {
                return String::new();
            }
            line.strip_prefix("# ")
                .or_else(|| line.strip_prefix('#'))
                .unwrap_or_else(|| {
                    panic!(
                        "expected every line of the template's peering example to start with \
                         '#', found: {line:?} -- did prose sneak into the commented block, or \
                         did the block stop being fully commented?"
                    )
                })
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The template cut at [`PEERING_EXAMPLE_MARKER`]: everything above the
/// peering example, and the example itself with one level of comment marker
/// removed.
fn preamble_and_peering_example() -> (&'static str, String) {
    let start = TEMPLATE.find(PEERING_EXAMPLE_MARKER).unwrap_or_else(|| {
        panic!(
            "deploy/connector-rust/connector.toml no longer has a '# [[peers]]' line -- if the \
             peering example moved or was reworded, repoint this test's marker rather than \
             deleting it"
        )
    });
    (&TEMPLATE[..start], uncomment(&TEMPLATE[start..]))
}

fn file_with(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write sandbox file");
    path
}

#[test]
fn the_templates_peering_example_loads() {
    let (preamble, peering_example) = preamble_and_peering_example();

    let dir = tempfile::tempdir().expect("tempdir");
    let signer_key = file_with(dir.path(), "signer.key", "");
    let settlement_key = file_with(dir.path(), "settlement.key", "");
    let state_dir = dir.path().join("state");
    std::fs::create_dir_all(&state_dir).expect("create sandbox state dir");

    let mut doc = preamble.to_string();
    let peer_expose_at = doc.find(PEER_EXPOSE_LINE).unwrap_or_else(|| {
        panic!(
            "deploy/connector-rust/connector.toml no longer has a commented '{PEER_EXPOSE_LINE}' \
             line above its peering example -- see issue #1221 for why it must stay a root-level \
             key"
        )
    });
    // Root scope ends at the first table header (`\n[`): a `peer_expose`
    // written after one parses as that table's key, not the connector's, and
    // `deny_unknown_fields` refuses it. Uncommenting the line has to work
    // where the template puts it, so pin that it is still above the headers.
    let first_table_header_at = doc.find("\n[").unwrap_or(doc.len());
    assert!(
        peer_expose_at < first_table_header_at,
        "deploy/connector-rust/connector.toml's commented 'peer_expose' line moved below a \
         [table] header -- uncommenting it there is a load failure (issue #1221)"
    );
    doc = doc.replace(PEER_EXPOSE_LINE, "peer_expose = \"btp\"");
    doc = doc.replace(
        "key_file = \"/app/data/signer.key\"",
        &format!("key_file = \"{}\"", signer_key.display()),
    );
    doc = doc.replace(
        "state_dir = \"/app/state\"",
        &format!("state_dir = \"{}\"", state_dir.display()),
    );
    doc = doc.replace(
        "write_keys = [\"REPLACE-WITH-A-64-HEX-CHARACTER-ED25519-PUBLIC-KEY-SEE-README-STEP-3\"]",
        "write_keys = \
         [\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"]",
    );

    // Not part of the template: the example's EVM `[[peer_channels]]` row
    // requires a `[settlement.evm]` table with x402 batch settlement switched
    // on to bind against (issue #1138, issue #1380),
    // and the template names no settlement chain at all -- rightly, since
    // which chain an operator settles on is theirs to pick. Supplied here
    // so this test proves the PEERING example loads, not that the template
    // should also teach settlement configuration it deliberately omits.
    doc.push_str(&format!(
        "\n[settlement.evm]\n\
         rpc_url = \"http://127.0.0.1:8545\"\n\
         contract_address = \"0x1234567890123456789012345678901234567890\"\n\
         token_address = \"0x49beE1Bca5d15Fb0963117923403F9498119a9Ce\"\n\
         decimals = 6\n\
         \n\
         [settlement.evm.batch_settlement]\n\
         asset_eip712_name = \"USD Coin\"\n\
         asset_eip712_version = \"2\"\n\
         \n\
         [settlement.evm.key]\n\
         key_file = \"{}\"\n",
        settlement_key.display()
    ));
    doc.push('\n');
    doc.push_str(&peering_example);

    let config_path = file_with(dir.path(), "connector.toml", &doc);
    Config::load(&config_path).unwrap_or_else(|error| {
        panic!(
            "deploy/connector-rust/connector.toml's peering example failed to load: {error}\n\n\
             assembled config:\n{doc}"
        )
    });
}

/// A negative control on [`uncomment`] and the marker itself: if the
/// template's example still wrote the credential ADR 0060 deleted, this
/// test would have to fail with `PeerCredentialRemoved`, not with some
/// unrelated parse error. Proves the harness would actually catch the
/// regression #1221 fixed, not just that today's file happens to load.
#[test]
fn the_harness_would_catch_a_reintroduced_credential() {
    let (_, peering_example) = preamble_and_peering_example();
    for expected in ["[[peers]]", "id = \"store\"", PEER_ENDPOINT_LINE] {
        assert!(
            peering_example.contains(expected),
            "uncommenting produced text with no {expected:?} in it, so it is not the peering \
             example this control means to spoil: {peering_example}"
        );
    }

    let spoiled = peering_example.replace(
        PEER_ENDPOINT_LINE,
        &format!("{PEER_ENDPOINT_LINE}\ncredential = {{ secret = \"x\" }}"),
    );

    // The reintroduced-credential doc is missing state_dir, settlement and
    // valid operator settings -- fine, because PeerCredentialRemoved must
    // win the race against every other refusal for this control to prove
    // anything. `[[peers]].credential` is parsed before those other tables
    // are even reached (`connector-config/src/peer.rs`), so it does.
    let dir = tempfile::tempdir().expect("tempdir");
    let signer_key = file_with(dir.path(), "signer.key", "");
    let doc = format!(
        "client_edge_addr = \"127.0.0.1:0\"\n\n[signer]\nkey_file = \"{}\"\n\n{}",
        signer_key.display(),
        spoiled
    );
    let config_path = file_with(dir.path(), "connector.toml", &doc);
    let error = Config::load(&config_path).expect_err("a reintroduced credential must be refused");
    assert!(
        error.to_string().contains("ADR 0060"),
        "expected a PeerCredentialRemoved-shaped error naming ADR 0060, got: {error}"
    );
}
