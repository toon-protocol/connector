//! ADR 0070 decisions 1, 2 and 6: **a `.onion` address is a host**, and the
//! two carriages ADR 0027 settled on ride it unchanged.
//!
//! Asserted against `Config::load` rather than against a hand-built value,
//! for the reason `connector-peer-http`'s
//! `dial_is_the_endpoints_scheme.rs` states in its own header: which
//! carriage a peer dials on is precisely the property the carriage must
//! **not** decide for itself. So every case here hands the parser a TOML
//! string and reads the result.
//!
//! Why the exemption is narrow. A v3 onion address is a base32 encoding of
//! the ed25519 public key the circuit is encrypted and authenticated to, so
//! a client that reached `abc...xyz.onion` reached the holder of that key or
//! reached nothing -- a stronger identity binding than a CA-issued
//! certificate for a DNS name, since no third party attests it and there is
//! no issuer to mis-issue. ADR 0004's requirement that a peering's wire be
//! authenticated is therefore satisfied by a different mechanism rather than
//! waived, which is why the rule keys on the host suffix and on nothing
//! else, and why it leaves `peer_allow_plaintext_endpoints` -- a test
//! affordance, not a deployment shape -- exactly where it was.
//!
//! **Two spellings, one rule** (issue #1284). `anon` renamed the TLD it
//! publishes and routes between v0.4.9.7 (`.onion`) and v0.4.10.2
//! (`.anyone`), and the rename is total in both directions -- neither daemon
//! resolves the other's spelling. Both suffixes are accepted here because
//! the argument above is about the ADDRESS: either spelling is the base32
//! ed25519 key the circuit is authenticated to. So every case below that
//! turns on the suffix is asserted at both, and a rule that held at one and
//! not the other would be a node that loads an endpoint it cannot dial.
//!
//! This file lives in `connector-runtime` because two of the four call sites
//! that must read the one answer are here: `RuntimePeering::dial`, and the
//! self-description read that picks a dialable published endpoint (ADR
//! 0058). The other two are `connector-config`'s own peer resolution, which
//! `Config::load` exercises below, and `ConfiguredPeerTransport::register`,
//! which asks `RuntimePeering::dial`.

use std::io::Write;
use std::path::Path;

use connector_config::{Config, PeerCarriage};
use connector_runtime::RuntimePeering;

/// A real v3 onion address: 56 base32 characters. Nothing here decodes it
/// -- the rule is a suffix rule -- but a plausible one keeps the fixtures
/// honest about what an operator actually copies out of the daemon's
/// `hostname` file.
const ONION: &str = "vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd.onion";

/// The same 56 characters under the TLD `anon` v0.4.10.2 writes into
/// `HiddenServiceDir/hostname` (issue #1284). Deliberately the same key in
/// both constants: what changed upstream is how the address is SPELLED, not
/// what it is, and a fixture that changed both at once would let a rule that
/// keys on the wrong half still pass.
const ANYONE: &str = "vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd.anyone";

/// Both spellings, for the cases whose whole subject is the suffix.
const HIDDEN_SERVICE_HOSTS: [&str; 2] = [ONION, ANYONE];

const CHANNEL: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";
const OTHER_CHANNEL: &str = "0x2222222222222222222222222222222222222222222222222222222222222222";
const KEY: &str = "0x2222222222222222222222222222222222222222";
const TOKEN_NETWORK: &str = "0x3333333333333333333333333333333333333333";

/// One node peering with two onion counterparties, one per carriage, on a
/// config that **does not** set `peer_allow_plaintext_endpoints` -- which is
/// the whole point: the plaintext schemes select here because of the host
/// and for no other reason.
fn onion_peers(key_path: &Path, state_dir: &Path) -> String {
    onion_peers_at(ONION, key_path, state_dir)
}

/// The same fixture at whichever spelling of a hidden-service host is under
/// test. Every caller that does not name one gets `.onion`, so the cases
/// this file already had read exactly as they did.
fn onion_peers_at(host: &str, key_path: &Path, state_dir: &Path) -> String {
    format!(
        r#"
client_edge_addr = "127.0.0.1:3000"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[[peers]]
id = "onion-btp"
endpoint = "ws://{host}/btp"

[[peers]]
id = "onion-http"
endpoint = "http://{host}/ilp"

[[peer_channels]]
peer_id = "onion-btp"
channel_id = "{CHANNEL}"
counterparty_key = "{KEY}"
chain_id = 31337
token_network = "{TOKEN_NETWORK}"

[[peer_channels]]
peer_id = "onion-http"
channel_id = "{OTHER_CHANNEL}"
counterparty_key = "{KEY}"
chain_id = 31337
token_network = "{TOKEN_NETWORK}"

[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6

[settlement.evm.key]
key_file = "{key_file}"
"#,
        state_dir = state_dir.display(),
        key_file = key_path.display(),
    )
}

/// Load `text` through the same `Config::load` the binary runs, with a real
/// signer key file and a real state directory beside it.
fn load(
    text: impl FnOnce(&Path, &Path) -> String,
) -> Result<Config, connector_config::ConfigError> {
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
    key_file.write_all(b"not a real key").expect("write key");
    let mut config_file = tempfile::Builder::new()
        .suffix(".toml")
        .tempfile()
        .expect("temp config file");
    config_file
        .write_all(text(key_file.path(), state_dir.path()).as_bytes())
        .expect("write config");
    Config::load(config_file.path())
}

fn dial_of(config: &Config, peer_id: &str) -> Option<PeerCarriage> {
    config
        .peers()
        .iter()
        .find(|peer| peer.id() == peer_id)
        .unwrap_or_else(|| panic!("peer '{peer_id}' is in the fixture"))
        .dial()
}

/// Decision 2, both halves at once: `ws://` at a hidden-service host selects
/// BTP and `http://` at one selects ILP-over-HTTP, on a node that opted into
/// nothing.
///
/// Run at both spellings (issue #1284). A node peering with a v0.4.10.2
/// daemon writes `.anyone` in exactly the place a node peering with a
/// v0.4.9.7 one writes `.onion`, and the exemption is the address's to earn
/// either way.
#[test]
fn an_onion_host_selects_both_carriages_without_the_plaintext_opt_in() {
    for host in HIDDEN_SERVICE_HOSTS {
        let config = load(|key_file, state_dir| onion_peers_at(host, key_file, state_dir))
            .unwrap_or_else(|error| panic!("an endpoint at {host} loads: {error}"));

        assert!(
            !config.peer_allow_plaintext_endpoints(),
            "the fixture must not set the opt-in, or it proves nothing about the host rule"
        );
        assert_eq!(
            dial_of(&config, "onion-btp"),
            Some(PeerCarriage::Btp),
            "ws://{host} selects BTP"
        );
        assert_eq!(
            dial_of(&config, "onion-http"),
            Some(PeerCarriage::Http),
            "http://{host} selects ILP-over-HTTP"
        );
    }
}

/// Decision 1: the set stays two-valued. Whatever an onion peering resolves
/// to is one of the two values that already existed -- there is no third to
/// be, which is what ADR 0070's own falsifier (`PeerCarriage::Onion`
/// matching nothing under `crates/`) says from the other direction.
#[test]
fn an_onion_peerings_carriage_is_one_of_the_two_that_already_existed() {
    let config = load(onion_peers).expect("load");

    for peer in config.peers() {
        let carriage = peer.dial().expect("both fixture peerings are dialable");
        assert!(
            matches!(carriage, PeerCarriage::Btp | PeerCarriage::Http),
            "{} resolved to something outside the two-valued set",
            peer.id()
        );
        assert!(
            matches!(carriage.name(), "btp" | "http"),
            "§11's normative spellings are unchanged"
        );
    }
}

/// The exemption is narrow, confirmed rather than assumed: the same two
/// schemes at a host that is not `.onion` are still `PeerEndpointScheme`,
/// on the same config that accepts them at an onion host.
#[test]
fn a_plaintext_scheme_at_any_other_host_is_still_refused() {
    for endpoint in [
        "ws://peer.example/btp",
        "http://peer.example/ilp",
        // The suffix is a suffix, not a substring: a host that merely
        // contains the word, or names it as a label that is not the last
        // one, is an ordinary clearnet host. Both spellings, because
        // widening the rule to a second TLD (issue #1284) is exactly the
        // change that could widen it to a second SHAPE by accident.
        "http://onion.example/ilp",
        "http://notreally.onion.example/ilp",
        "http://anyone.example/ilp",
        "http://notreally.anyone.example/ilp",
    ] {
        let error = load(|key_file, state_dir| {
            onion_peers(key_file, state_dir).replace(&format!("http://{ONION}/ilp"), endpoint)
        })
        .expect_err(endpoint);

        assert!(
            matches!(
                error,
                connector_config::ConfigError::PeerEndpointScheme { ref value, .. }
                    if value == endpoint
            ),
            "{endpoint} must still be refused with the existing error, got: {error}"
        );
    }
}

/// `peer_allow_plaintext_endpoints` is unchanged in **meaning** and in
/// **scope**. ADR 0070 permits the plaintext schemes at one host suffix; it
/// is not a back door into plaintext peering generally, and this is the
/// assertion that says so from the flag's side.
///
/// Asked of `from_scheme_allowing_plaintext` directly, because that is the
/// function the flag names: what it permits with the flag off, what it
/// permits with the flag on, and -- the scope half -- that the flag reaches
/// schemes and never hosts.
#[test]
fn the_plaintext_opt_in_still_means_exactly_what_it_meant() {
    use PeerCarriage::{Btp, Http};

    // Off: the TLS pair and nothing else. This is the production answer and
    // the answer on every config that never mentions the field.
    assert_eq!(
        PeerCarriage::from_scheme_allowing_plaintext("wss", false),
        Some(Btp)
    );
    assert_eq!(
        PeerCarriage::from_scheme_allowing_plaintext("https", false),
        Some(Http)
    );
    assert_eq!(
        PeerCarriage::from_scheme_allowing_plaintext("ws", false),
        None
    );
    assert_eq!(
        PeerCarriage::from_scheme_allowing_plaintext("http", false),
        None
    );

    // On: those two, plus their plaintext twins. Still nothing else -- the
    // flag has never widened the set of *carriages*, only the set of
    // schemes that reach them.
    assert_eq!(
        PeerCarriage::from_scheme_allowing_plaintext("ws", true),
        Some(Btp)
    );
    assert_eq!(
        PeerCarriage::from_scheme_allowing_plaintext("http", true),
        Some(Http)
    );
    for outside in ["btp", "ilp", "tcp", "socks5h", "file", "ftp", "onion"] {
        assert_eq!(
            PeerCarriage::from_scheme_allowing_plaintext(outside, true),
            None,
            "'{outside}://' selects no carriage however the opt-in is set"
        );
    }

    // Scope: the flag is about schemes. An onion endpoint resolves the same
    // whether it is set or not, which is what "independent of
    // `peer_allow_plaintext_endpoints`" means in the decision.
    let onion = url::Url::parse(&format!("http://{ONION}/ilp")).expect("parse");
    assert_eq!(PeerCarriage::for_endpoint(&onion, false), Some(Http));
    assert_eq!(PeerCarriage::for_endpoint(&onion, true), Some(Http));
}

/// A node that set the opt-in still gets its startup warning about the
/// peerings that are genuinely in the clear -- and an onion peering is not
/// one of them. `plaintext_peerings` is what `warn_about_plaintext_peerings`
/// names, and naming an onion endpoint there would print a warning whose
/// own text ("peer_allow_plaintext_endpoints is set...") is false about it.
#[test]
fn an_onion_peering_is_not_named_as_a_peering_in_the_clear() {
    for host in HIDDEN_SERVICE_HOSTS {
        let config = load(|key_file, state_dir| {
            onion_peers_at(host, key_file, state_dir).replace(
                "client_edge_addr = \"127.0.0.1:3000\"",
                "client_edge_addr = \"127.0.0.1:3000\"\npeer_allow_plaintext_endpoints = true",
            )
        })
        .unwrap_or_else(|error| panic!("an endpoint at {host} loads: {error}"));

        assert!(config.peer_allow_plaintext_endpoints());
        assert_eq!(
            config.plaintext_peerings().count(),
            0,
            "{host}: the circuit is encrypted and authenticated to the key the address is"
        );
    }
}

/// Decision 6, first half: a `.onion` URL is a legal value for the existing
/// `[node]` endpoint keys. Migrating to an onion endpoint changes a value
/// and not a schema -- there is no second key per carriage.
#[test]
fn an_onion_url_is_accepted_in_both_node_endpoint_keys() {
    for host in HIDDEN_SERVICE_HOSTS {
        let config = load(|key_file, state_dir| {
            format!(
                "{}\n[node]\naddresses = [\"g.toon.onion-node\"]\n\
                 http_endpoint = \"http://{host}/ilp\"\n\
                 btp_endpoint = \"ws://{host}/ilp/btp\"\n",
                onion_peers_at(host, key_file, state_dir).replace(
                    "client_edge_addr = \"127.0.0.1:3000\"",
                    "client_edge_addr = \"127.0.0.1:3000\"\npeer_expose = \"both\""
                ),
            )
        })
        .unwrap_or_else(|error| {
            panic!("a node at {host} describes itself with its own endpoints: {error}")
        });

        let node = config.node().expect("[node]");
        assert_eq!(
            node.http_endpoint(),
            Some(format!("http://{host}/ilp").as_str())
        );
        assert_eq!(
            node.btp_endpoint(),
            Some(format!("ws://{host}/ilp/btp").as_str())
        );
    }
}

/// Decision 6, second half: issue #1220's rule is unmodified. An endpoint is
/// required exactly when `peer_expose` opens that listener, and an onion
/// node is not exempt from it -- a node that publishes nothing is a node
/// nobody can peer with, whatever it is reachable over.
#[test]
fn an_exposed_listener_still_requires_an_endpoint_on_an_onion_node() {
    let error = load(|key_file, state_dir| {
        format!(
            "{}\n[node]\naddresses = [\"g.toon.onion-node\"]\n\
             btp_endpoint = \"ws://{ONION}/ilp/btp\"\n",
            onion_peers(key_file, state_dir).replace(
                "client_edge_addr = \"127.0.0.1:3000\"",
                "client_edge_addr = \"127.0.0.1:3000\"\npeer_expose = \"both\""
            ),
        )
    })
    .expect_err("an exposed carriage with no http_endpoint is refused");

    assert!(
        matches!(
            error,
            connector_config::ConfigError::NodeMissingEndpoint { field, .. }
                if field == "http_endpoint"
        ),
        "got: {error}"
    );
}

/// The rule reaches a peering established at runtime from a URL (ADR 0058)
/// by being **called** there, not by a second copy: a runtime peering with a
/// `.onion` endpoint decides its carriage identically to a config-file one.
///
/// The two answers are compared to each other rather than each to a
/// constant, so this fails if the two implementations ever diverge, whatever
/// they diverge to.
#[test]
fn a_runtime_peering_decides_an_onion_endpoint_identically() {
    for host in HIDDEN_SERVICE_HOSTS {
        let config = load(|key_file, state_dir| onion_peers_at(host, key_file, state_dir))
            .unwrap_or_else(|error| panic!("an endpoint at {host} loads: {error}"));

        for (peer_id, endpoint) in [
            ("onion-btp", format!("ws://{host}/btp")),
            ("onion-http", format!("http://{host}/ilp")),
        ] {
            let runtime = RuntimePeering {
                endpoint: Some(endpoint.clone()),
                ..RuntimePeering::default()
            };

            assert_eq!(
                runtime.dial(false),
                dial_of(&config, peer_id),
                "{endpoint} must decide the same carriage from a runtime row as from the \
                 config file"
            );
            assert_eq!(
                runtime.dial(false),
                runtime.dial(true),
                "and independently of the plaintext opt-in, on both paths"
            );
        }
    }
}

/// **The rename is a spelling, and this is the test that says so** (issue
/// #1284).
///
/// `anon` v0.4.9.7 publishes and routes `.onion`; v0.4.10.2 publishes and
/// routes `.anyone` and contains no occurrence of the older TLD at all.
/// Neither daemon resolves the other's spelling, so a connector that knew
/// only one of them could load an address its own operator's daemon had
/// just written and refuse it -- which is what this repository did until
/// this test existed.
///
/// The two answers are compared to **each other** rather than each to a
/// constant, for the reason the runtime-peering case above is written that
/// way: this fails if the spellings ever diverge, whatever they diverge to.
/// What it deliberately does not assert is that a `.onion` address is
/// reachable from a `.anyone` daemon -- it is not, and that is the sidecar's
/// business rather than the connector's. This node accepts both because it
/// dials whichever one its own operator's daemon speaks.
#[test]
fn both_spellings_of_a_hidden_service_host_decide_identically() {
    let by_spelling: Vec<Vec<(String, Option<PeerCarriage>)>> = HIDDEN_SERVICE_HOSTS
        .iter()
        .map(|host| {
            let config = load(|key_file, state_dir| onion_peers_at(host, key_file, state_dir))
                .unwrap_or_else(|error| panic!("an endpoint at {host} loads: {error}"));
            config
                .peers()
                .iter()
                .map(|peer| (peer.id().to_string(), peer.dial()))
                .collect()
        })
        .collect();

    assert_eq!(
        by_spelling[0], by_spelling[1],
        "the same 56-character key under the two TLDs `anon` has published must select the same \
         carriage for the same peering. One spelling accepted and the other refused is an \
         operator whose node will not dial the address their own daemon generated."
    );
    assert!(
        by_spelling[0].iter().all(|(_, dial)| dial.is_some()),
        "and it must be a carriage rather than None at both -- two refusals are also 'identical'"
    );
}
