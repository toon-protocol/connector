//! Black-box coverage of the actual compiled binary, rather than a library
//! call: an invalid config produces a specific error on stderr and a
//! non-zero exit before anything is bound, and a valid config brings up a
//! real, servable node (ADR 0001: "loads configuration, constructs the
//! runtime, merges routers and serves").

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

fn run(config_path: Option<&std::path::Path>) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_connector"));
    if let Some(path) = config_path {
        command.arg(path);
    }
    command.output().expect("run connector binary")
}

fn spawn(config_path: &std::path::Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_connector"))
        .arg(config_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn connector binary")
}

/// Read stdout lines until the "connector listening" structured log line
/// appears, and return the address it reports. `client_edge_addr =
/// "127.0.0.1:0"` (used throughout this file) lets the OS pick a free
/// port, so this is the only way a test learns which one.
fn wait_for_listen_addr(child: &mut Child) -> String {
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut line = String::new();
    loop {
        line.clear();
        let read = stdout.read_line(&mut line).expect("read stdout");
        assert!(read > 0, "process exited before logging a listen address");
        if line.contains("connector listening") {
            let parsed: serde_json::Value =
                serde_json::from_str(&line).expect("listen line is a JSON log record");
            return parsed["fields"]["addr"]
                .as_str()
                .expect("addr field")
                .to_string();
        }
    }
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

#[tokio::test]
async fn serves_traffic_with_a_valid_config() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.example.app"
handler_url = "http://localhost:4000"
price = 0
"#,
        key_file.path().display()
    ));

    let mut child = spawn(config_file.path());
    let addr = wait_for_listen_addr(&mut child);

    // `GET /ilp` is this node's self-description (ADR 0050): free,
    // unauthenticated, and the one request a stranger who has nothing but
    // this URL can make. It answered `405` until issue #1080 -- a live
    // router, but nothing to read -- and the shipped binary answering it with
    // a real document is the end-to-end proof that the handler is mounted in
    // the artefact, not merely in a unit test's router.
    let response = reqwest::get(format!("http://{addr}/ilp"))
        .await
        .expect("request the running connector");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let document: serde_json::Value = response.json().await.expect("a JSON self-description");
    assert_eq!(document["defaultVersion"], serde_json::json!(1));
    assert!(
        document["edgeIdentity"]["publicKey"]
            .as_str()
            .is_some_and(|key| key.starts_with("0x")),
        "the edge identity is what a packet is sealed to (ADR 0018) and a route whose \
         terminating identity is unpublished is unreachable (ND-06): {document}"
    );
    assert_eq!(
        document["routes"][0]["prefix"],
        serde_json::json!("g.example.app"),
        "the configured route's price is published from the live route table: {document}"
    );

    child.kill().expect("kill connector");
    child.wait().expect("wait for connector to exit");
}

#[tokio::test]
async fn serves_the_operator_surface_when_configured() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[operator]
bearer_token = "operator-secret"
write_keys = ["0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]
"#,
        key_file.path().display()
    ));

    let mut child = spawn(config_file.path());
    let addr = wait_for_listen_addr(&mut child);

    let client = reqwest::Client::new();
    let unauthenticated = client
        .get(format!("http://{addr}/metrics"))
        .send()
        .await
        .expect("request /metrics with no token");
    assert_eq!(unauthenticated.status(), reqwest::StatusCode::UNAUTHORIZED);

    let authenticated = client
        .get(format!("http://{addr}/metrics"))
        .bearer_auth("operator-secret")
        .send()
        .await
        .expect("request /metrics with the configured token");
    assert_eq!(authenticated.status(), reqwest::StatusCode::OK);
    let body = authenticated.text().await.expect("metrics body");
    assert!(body.contains("toon_fees_earned_total"));

    child.kill().expect("kill connector");
    child.wait().expect("wait for connector to exit");
}

#[test]
fn exits_non_zero_with_an_actionable_error_on_invalid_toml() {
    let config_file = write_config("this is not valid toml {");

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.to_lowercase().contains("toml"),
        "expected a TOML-specific error, got: {stderr}"
    );
}

#[test]
fn exits_non_zero_with_a_missing_signer_key_file() {
    let config_file = write_config(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "/nonexistent/does-not-exist.key"
"#,
    );

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does-not-exist.key"),
        "expected the offending path in the error, got: {stderr}"
    );
}

#[test]
fn exits_non_zero_when_no_config_path_is_given() {
    let output = run(None);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage"));
}

#[test]
fn exits_non_zero_when_the_operator_surface_is_enabled_with_no_write_keys() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[operator]
bearer_token = "operator-secret"
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("write_keys"),
        "expected an actionable write_keys error, got: {stderr}"
    );
}

#[test]
fn exits_non_zero_when_the_operator_surface_is_enabled_with_no_bearer_token() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[operator]
write_keys = ["0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("bearer_token"),
        "expected an actionable bearer_token error, got: {stderr}"
    );
}

#[test]
fn exits_non_zero_when_the_signer_key_file_has_invalid_material() {
    let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
    key_file
        .write_all(b"not real key material, just needs to exist")
        .expect("write key file");
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("key_file"),
        "expected an actionable signer key error, got: {stderr}"
    );
}

#[test]
fn exits_non_zero_when_the_signer_location_is_kms() {
    let config_file = write_config(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
kms_key_id = "arn:aws:kms:us-east-1:123:key/abc"
"#,
    );

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("key management service"),
        "expected an actionable unsupported-signer-location error, got: {stderr}"
    );
}

#[test]
fn exits_non_zero_when_a_terminated_route_has_no_price() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.example.app"
handler_url = "http://localhost:4000"
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("price"),
        "expected an actionable missing-price error, got: {stderr}"
    );
}

/// Issue #556, the parse layer: a key misspelled *inside* a section used
/// to be dropped by `toml::from_str` and the node started as if it had
/// never been written. `[operator]` is the sharpest case -- a mistyped
/// `bearer_tokn` left the section resolving as "present but
/// unauthenticated", so ADR 0008's own refuse-to-start error fired and
/// pointed the operator at the wrong thing.
#[test]
fn exits_non_zero_on_a_misspelled_key_inside_a_section() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[operator]
bearer_tokn = "operator-secret"
write_keys = ["0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("bearer_tokn"),
        "expected the error to name the misspelled key, got: {stderr}"
    );
}

/// ADR 0061's tombstone, terminated half. A `fee` on a terminated route was
/// read by no branch of `resolve_routes` and silently earned nothing (issue
/// #556) -- the same shape #520 refused for a missing `price`. The key is
/// gone from routes entirely now, so this is the removed-field trap
/// `peer_wire_addr`, `ceiling`, `flush_interval_ms` and `[peer_sale]` get.
#[test]
fn exits_non_zero_when_a_terminated_route_sets_a_fee() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.example.app"
handler_url = "http://localhost:4000"
price = 100
fee = 5
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fee") && stderr.contains("peers"),
        "expected a named removal error pointing at the '[[peers]]' row, got: {stderr}"
    );
}

/// ADR 0061's tombstone, FORWARDED half -- the one that actually moves
/// something. `[[routes]] fee` was read and honoured on this branch, so
/// every config in this tree that charged anything wrote it here. Rejected
/// by name rather than ignored: a node whose operator wrote a fee on the
/// route believes it is charging one, and after the move it would carry for
/// free. This is the case the devnet note in issue #1159 is about -- no box
/// sets a fee today, so the fleet change is nil, but a box whose TOML grew
/// one would stop at boot by name rather than peer for free.
#[test]
fn exits_non_zero_when_a_forwarded_route_sets_a_fee() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[[peers]]
id = "store"
endpoint = "wss://store.example:443/btp"
credential = {{ secret = "shared-secret" }}
fee = 3

[[routes]]
prefix = "g.example.store"
peer_id = "store"
price = 1000
fee = 3
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fee") && stderr.contains("g.example.store"),
        "expected a named removal error naming the route, got: {stderr}"
    );
}

/// The mirror image (ADR 0028): a route forwarding to a peer with no
/// `price` charged the client nothing, so a node written to forward
/// packets carried them -- and paid its own peer for the carriage -- for
/// free. `price` is required on the forwarded branch exactly as it is on
/// the terminated one; issue #557's "never silently free" is not a
/// property of terminating.
#[test]
fn exits_non_zero_when_a_peer_route_sets_no_price() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.example.store"
peer_id = "store"
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("price"),
        "expected an actionable missing-price-on-a-peer-route error, got: {stderr}"
    );
}

// -- Durable claim state (issue #605) --

/// A node that can accept claims but names nowhere durable to record them
/// never starts. Without this it starts happily, serves, and gives every
/// already-spent claim back to the client the next time it restarts --
/// with nothing in any log to see, because from the gate's point of view
/// every replayed nonce genuinely looks fresh.
#[test]
fn exits_non_zero_when_client_channels_are_configured_without_a_state_dir() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_file}"

[[client_channels]]
channel_id = "0x{channel}"
counterparty = "0x00000000000000000000000000000000000000aa"
chain_id = 8453
token_network_address = "0x00000000000000000000000000000000000000bb"

# An EVM client channel needs `[settlement.evm]` too (issue #1138), and
# this test is about the other requirement -- so it carries one, or the
# refusal it asserts would never be the one reached.
[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6

[settlement.evm.key]
key_file = "{key_file}"
"#,
        key_file = key_file.path().display(),
        channel = "ab".repeat(32),
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("state_dir"),
        "expected the error to name the field to add, got: {stderr}"
    );
}

/// The `[[client_channels]]` half of issue #1138's one rule, at the level
/// an operator meets it: the binary refuses to start, names the table to
/// add, and says why. The same config with `[settlement.evm]` present is
/// the test above, which reaches a different refusal entirely.
///
/// A declared channel is deliberately usable with no *chain connection* --
/// it is answered from memory, never resolved, and exempt from the deposit
/// cap (issue #646). That latitude is over how much a counterparty may
/// spend on a channel this node is a participant of. Without
/// `[settlement.evm]` this node has no EVM address to be that participant,
/// so it would serve paid writes for claims nothing it holds could redeem.
#[test]
fn exits_non_zero_when_an_evm_client_channel_has_no_evm_settlement_table() {
    let key_file = write_raw_key_file();
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[[client_channels]]
channel_id = "0x{channel}"
counterparty = "0x00000000000000000000000000000000000000aa"
chain_id = 8453
token_network_address = "0x00000000000000000000000000000000000000bb"
"#,
        key_file = key_file.path().display(),
        state_dir = state_dir.path().display(),
        channel = "ab".repeat(32),
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[settlement.evm]") && stderr.contains("on-chain participant"),
        "expected the error to name the table to add and why, got: {stderr}"
    );
}

/// ADR 0073 decision 1, through the real binary: a settlement table that
/// asks for its RPC to ride the node's `socks_proxy`, on a node that has
/// none, stops the node by name. It never falls back to dialing direct.
#[test]
fn exits_non_zero_when_settlement_rpc_is_routed_through_a_socks_proxy_that_is_not_configured() {
    let key_file = write_raw_key_file();
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[settlement.solana]
rpc_url = "https://api.devnet.solana.com"
program_id = "2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip"
token_address = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU"
decimals = 6
rpc_via_socks_proxy = true

[settlement.solana.key]
key_file = "{key_file}"
"#,
        key_file = key_file.path().display(),
        state_dir = state_dir.path().display(),
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[settlement.solana]")
            && stderr.contains("rpc_via_socks_proxy")
            && stderr.contains("no socks_proxy"),
        "expected the error to name the table, the key and the missing proxy, got: {stderr}"
    );
}

/// A node with nowhere writable fails at startup, naming the path -- not
/// at the first claim, hours later, on a packet path where the only
/// honest answer left is to refuse a claim that was perfectly good.
#[test]
fn exits_non_zero_when_the_state_dir_cannot_be_written() {
    let key_file = write_raw_key_file();
    // A regular file standing where a directory is asked for: the same
    // failure a read-only mount produces, reproducible without root.
    let blocker = tempfile::NamedTempFile::new().expect("temp file");
    let state_dir = blocker.path().join("state");
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"
"#,
        key_file = key_file.path().display(),
        state_dir = state_dir.display(),
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("state_dir") && stderr.contains(&state_dir.display().to_string()),
        "expected the error to name the unusable path, got: {stderr}"
    );
}

/// A journal this build cannot decode stops the node. Refusing to start is
/// the whole point: the only other option is starting from no watermarks,
/// which is exactly the defect.
#[test]
fn exits_non_zero_on_a_corrupt_claim_journal() {
    let key_file = write_raw_key_file();
    let state_dir = tempfile::tempdir().expect("temp state dir");
    std::fs::write(
        state_dir.path().join("client-edge-claims.log"),
        "not a journal entry\n",
    )
    .expect("write a corrupt journal");
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"
"#,
        key_file = key_file.path().display(),
        state_dir = state_dir.path().display(),
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("client-edge-claims.log"),
        "expected the error to name the journal it could not replay, got: {stderr}"
    );
}

// -- The deleted raw-TCP transport (ADR 0027, issue #679) and the peer
// config surface that replaced it (issue #677, peer-carriage-spec.md
// §11) --

/// ADR 0009's "refuse to start" contract against the peer config surface:
/// a route naming a `peer_id` no `[[peers]]` entry configures is exactly
/// the kind of misconfigured node that must never come up quietly
/// connected to nothing. (Moved here from
/// `two_connectors_and_a_stub_app.rs`, which was deleted with the raw-TCP
/// wire it proved.)
#[test]
fn exits_non_zero_when_a_peer_route_names_an_unconfigured_peer_id() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[[routes]]
prefix = "g.b"
peer_id = "peer-b"
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("peer-b"),
        "expected an actionable unknown-peer-id error, got: {stderr}"
    );
}

/// The devnet boxes run bind-mounted configs that lead the repo copies, so
/// a stale one naming the removed `peer_wire_addr` has to stop the binary
/// and say where to read about it -- not be ignored into a node that looks
/// healthy and never peers.
#[test]
fn exits_non_zero_when_a_stale_config_sets_peer_wire_addr() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
peer_wire_addr = "0.0.0.0:4001"

[signer]
key_file = "{}"
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("peer_wire_addr")
            && stderr.contains("docs/operators/btp-peer-transport-bringup.md"),
        "expected a named removal error pointing at the bring-up doc, got: {stderr}"
    );
}

/// ADR 0060's tombstone. `credential` was REQUIRED of every peering until
/// this release and appears in every committed topology config, so a stale
/// copy setting it is the common case rather than the exotic one. It has to
/// stop the binary by name: a peering whose secret was silently ignored would
/// look configured, start clean, and differ from a working one only in that
/// the operator still believes a shared string is doing something.
#[test]
fn exits_non_zero_when_a_peer_sets_a_credential() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[[peers]]
id = "peer-b"
endpoint = "https://peer-b.example/ilp"

[peers.credential]
secret = "s3cret-peering-key"
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("credential") && stderr.contains("peer-b"),
        "expected the removed 'credential' key refused by name, got: {stderr}"
    );
}

/// The `[[peers]]` half: a `SocketAddr`-shaped `addr` is the other thing a
/// stale config carries, and it fails the same way.
#[test]
fn exits_non_zero_when_a_stale_peer_entry_sets_addr() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[[peers]]
id = "store"
addr = "127.0.0.1:4001"
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("store") && stderr.contains("docs/operators/btp-peer-transport-bringup.md"),
        "expected a named removal error pointing at the bring-up doc, got: {stderr}"
    );
}

/// ADR 0042 item 4 (issue #1077): `claim_enforcement` is gone, and a config
/// that still writes it must stop the binary rather than boot a node that
/// enforces something its operator believes it does not. `"observe"` is the
/// value that matters -- the one an operator ran deliberately to admit
/// uncovered arrivals -- and the error has to say the key by name and that
/// the still-live `forwarded_claim_enforcement` is a different field, since
/// the two spellings differ by one word.
#[test]
fn exits_non_zero_when_a_stale_peer_entry_sets_claim_enforcement() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
peer_expose = "btp"

[signer]
key_file = "{}"

[[peers]]
id = "store"
endpoint = "wss://store.example/btp"
claim_enforcement = "observe"
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("claim_enforcement")
            && stderr.contains("store")
            && stderr.contains("forwarded_claim_enforcement")
            && stderr.contains("docs/operators/claim-policy-rollout.md"),
        "expected a named removal error that also disambiguates the surviving \
         forwarded_claim_enforcement, got: {stderr}"
    );
}

/// The surviving half of the pair is still an accepted key: deleting one
/// field must not have taken the other with it. This config is deliberately
/// invalid for an unrelated reason (a peering with no `[[peer_channels]]`
/// row), because a valid one would boot and serve forever -- and
/// `deny_unknown_fields` rejects at *parse* time, before `resolve_peers`
/// ever reaches the channel-binding check, so reaching that error at all is
/// the proof that `forwarded_claim_enforcement` parsed.
#[test]
fn forwarded_claim_enforcement_is_still_an_accepted_key() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
peer_expose = "btp"

[signer]
key_file = "{}"

[[peers]]
id = "store"
endpoint = "wss://store.example/btp"
forwarded_claim_enforcement = "enforce"
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[[peer_channels]]") && !stderr.contains("forwarded_claim_enforcement"),
        "expected forwarded_claim_enforcement to parse and the load to fail on the missing \
         channel binding instead, got: {stderr}"
    );
}

/// A peering with no `[[peer_channels]]` row can never take the peer role
/// -- there is no channel for a claim to name and no counterparty key to
/// verify one against (§1.2's P2) -- so it would come up looking configured
/// and admit its counterparty as an ordinary client. The whole point of
/// issue #677's load-time checks is that this is loud.
///
/// This test used to name the missing **credential**, which was the other
/// way a peering could look configured and peer with nobody. ADR 0060
/// deleted that surface; the channel binding is the whole of what a peering
/// now needs, so it is the whole of what this asserts.
#[test]
fn exits_non_zero_when_a_peer_has_no_channel_binding() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
peer_expose = "btp"

[signer]
key_file = "{}"

[[peers]]
id = "store"
endpoint = "wss://store.example/btp"
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no '[[peer_channels]]' entry")
            && stderr.contains("docs/operators/btp-peer-transport-bringup.md"),
        "expected a named unbound-peering error pointing at the bring-up doc, got: {stderr}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// `[announce]` became `[node]`, and its announce-only keys are tombstones
// (ADR 0050 / issue #1080, ADR 0046 / issue #1074)
// ═══════════════════════════════════════════════════════════════════════════
//
// Grouped together because they are one migration, and because the whole
// point of ADR 0009's removed-key rule is that a stale committed file stops
// the binary BY NAME rather than loading with a key silently dropped. The
// devnet boxes bind-mount configs that lead this repo, so the message an
// operator reads at 3am is the artifact these tests exist to pin.

/// The section was RENAMED, not deleted -- two of its fields feed the packet
/// path (the x402 greeting carries them so a client with a stale genesis seed
/// can bootstrap, issue #807) -- so the error has to say `[node]`, not merely
/// that `[announce]` is gone. An operator told only that something is removed
/// has to go and find out what replaced it.
#[test]
fn exits_non_zero_when_a_stale_config_still_writes_the_announce_section() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[announce]
addresses = ["g.example.node"]
http_endpoint = "https://node.example/ilp"
btp_endpoint = "wss://node.example/ilp/btp"
"#,
        key_file.path().display()
    ));

    let output = run(Some(config_file.path()));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[announce]") && stderr.contains("[node]"),
        "the error must name BOTH the old heading and the new one -- this is a rename, and an \
         operator reading it needs the replacement, got: {stderr}"
    );
    assert!(
        stderr.contains("ADR 0050"),
        "and cite the record that made the decision, got: {stderr}"
    );
}

/// The three surviving fields load under the new heading. The positive half:
/// without it, "refuses `[announce]`" would also pass on a binary that
/// refused everything.
#[test]
fn a_node_section_with_its_three_surviving_fields_serves() {
    let key_file = write_raw_key_file();
    let config_file = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
peer_expose = "both"

[signer]
key_file = "{}"

[node]
addresses = ["g.example.node"]
http_endpoint = "https://node.example/ilp"
btp_endpoint = "wss://node.example/ilp/btp"
"#,
        key_file.path().display()
    ));

    let mut child = spawn(config_file.path());
    let addr = wait_for_listen_addr(&mut child);
    assert!(!addr.is_empty());
    child.kill().expect("kill connector");
    let _ = child.wait();
}

/// Every announce-only key, refused **by name** (ADR 0009). Driven as a table
/// rather than fifteen tests because the property is identical for all of
/// them, and a table is the only form in which "did we miss one?" is
/// answerable by reading.
///
/// `relay_url` and `solana_chain_id` are the two worth naming individually.
/// `relay_url` asserted that a Nostr relay for free reads sat behind this
/// node -- an APPLICATION fact, and the last place ADR 0046's removed relay
/// assumption survived (ND-08). `solana_chain_id` was a second declaration of
/// a fact `[settlement.solana]` already held and had verified against a
/// chain; it defaulted to `solana:devnet`, nothing compared the two, and a
/// mainnet node therefore described itself as devnet (issue #981). Neither
/// comes back.
#[test]
fn exits_non_zero_naming_each_announce_only_key_a_stale_config_still_sets() {
    let removed: [(&str, &str); 15] = [
        ("relay_url", r#"relay_url = "wss://relay.example""#),
        ("publish_to", r#"publish_to = "g.toon.relay""#),
        (
            "publish_btp_url",
            r#"publish_btp_url = "wss://relay.example/ilp/btp""#,
        ),
        (
            "pay_channel",
            r#"pay_channel = "0xdeaddeaddeaddeaddeaddeaddeaddeaddeaddeaddeaddeaddeaddeaddeadc0de""#,
        ),
        ("route_publish", r#"route_publish = "g.toon.relay""#),
        ("route_store", r#"route_store = "g.toon.ario""#),
        ("asset_code", r#"asset_code = "USDC""#),
        ("asset_scale", "asset_scale = 6"),
        ("solana_chain_id", r#"solana_chain_id = "solana:devnet""#),
        ("ttl_secs", "ttl_secs = 600"),
        (
            "identity_key_file",
            r#"identity_key_file = "/app/data/announce.key""#,
        ),
        ("notice_id", r#"notice_id = "2026-08-13-two-box-cutover""#),
        ("notice_severity", r#"notice_severity = "info""#),
        ("notice_summary", r#"notice_summary = "read the notes""#),
        ("notice_url", r#"notice_url = "https://example.com/notice""#),
    ];

    for (name, line) in removed {
        let key_file = write_raw_key_file();
        let config_file = write_config(&format!(
            r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key}"

[node]
addresses = ["g.example.node"]
http_endpoint = "https://node.example/ilp"
btp_endpoint = "wss://node.example/ilp/btp"
{line}
"#,
            key = key_file.path().display()
        ));

        let output = run(Some(config_file.path()));

        assert!(
            !output.status.success(),
            "`{name}` was removed with the announce and must stop the binary, not load"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(name),
            "the error must name `{name}` -- a removed key reported as merely an unknown field \
             sends an operator reading a schema instead of a changelog, got: {stderr}"
        );
        assert!(
            stderr.contains("ADR 0046") || stderr.contains("#1074"),
            "and cite what removed it, got: {stderr}"
        );
    }
}
