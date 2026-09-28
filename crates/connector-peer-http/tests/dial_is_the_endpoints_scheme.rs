//! §2.1, §11: what a **loaded configuration** says about this carriage,
//! asserted against `Config::load` rather than against a hand-built value
//! -- because which carriage a peer dials on is precisely the property the
//! carriage must **not** decide for itself.
//!
//! **Which carriage this connector dials a peer on is decided solely by
//! the scheme of that peer's `endpoint`** (§2.1). The HTTP transport
//! registers `https://` peerings and no others; a `wss://` peering is the
//! BTP carriage's and an endpoint-less one is accept-only. `Config::load`
//! has already refused any other scheme (`PeerEndpointScheme`) and a
//! peering that can never establish (`PeerUndialable`), so dialability is
//! config's answer and is not re-derived at runtime.
//!
//! Before ADR 0031/ADR 0033 (issue #882) an accept-only peering also had to
//! carry an explicit `ceiling` or refuse to load (§6.4(3),
//! `AcceptOnlyPeerWithoutCeiling`) -- retired along with the credit window
//! it bounded.

use std::io::Write;
use std::path::Path;

use connector_config::Config;
use connector_peer_http::dial::PeerRelation;

/// Each peering's voucher signer (ADR 0075): one signer proves one peering.
const KEY: &str = "0x2222222222222222222222222222222222222222";
const OTHER_KEY: &str = "0x3333333333333333333333333333333333333333";
const THIRD_KEY: &str = "0x4444444444444444444444444444444444444444";

/// One connector that dials one peer over HTTP, one over BTP, and accepts a
/// third -- the three shapes §2.1 distinguishes, in one file.
fn peering_config(key_path: &Path, state_dir: &Path) -> String {
    format!(
        r#"
client_edge_addr = "127.0.0.1:3000"
peer_expose = "btp"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[[peers]]
id = "over-http"
endpoint = "https://peer.example:443/ilp"

[[peers]]
id = "over-btp"
endpoint = "wss://peer.example:443/btp"

[[peers]]
id = "accept-only"

[[peer_channels]]
peer_id = "over-http"
voucher_signer = "{KEY}"

[[peer_channels]]
peer_id = "over-btp"
voucher_signer = "{OTHER_KEY}"

[[peer_channels]]
peer_id = "accept-only"
voucher_signer = "{THIRD_KEY}"

# An EVM `[[peer_channels]]` row needs `[settlement.evm]` with its x402
# `batch_settlement` sub-table (ADR 0075, issue #1380): the peer's vouchers
# are admitted by that table's receiving half.
[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[settlement.evm.key]
key_file = "{key_file}"

"#,
        state_dir = state_dir.display(),
        key_file = key_path.display(),
    )
}

fn load() -> Config {
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
    key_file.write_all(b"not a real key").expect("write key");
    let mut config_file = tempfile::Builder::new()
        .suffix(".toml")
        .tempfile()
        .expect("temp config file");
    config_file
        .write_all(peering_config(key_file.path(), state_dir.path()).as_bytes())
        .expect("write config");
    Config::load(config_file.path()).expect("load")
}

#[test]
fn only_an_https_endpoint_selects_this_carriage() {
    let config = load();

    let dialed: Vec<&str> = config
        .peers()
        .iter()
        .filter(|peer| PeerRelation::from_config(peer).is_some())
        .map(connector_config::PeerConfig::id)
        .collect();

    assert_eq!(
        dialed,
        vec!["over-http"],
        "the carriage is the endpoint's scheme and nothing else (§2.1)"
    );
}

/// ADR 0031/ADR 0033, issue #882: an accept-only peering now loads with no
/// ceiling-shaped config at all -- the credit window that requirement
/// bounded is retired.
#[test]
fn an_accept_only_peering_loads_with_no_ceiling() {
    let config = load();
    let accept_only = config
        .peers()
        .iter()
        .find(|peer| peer.id() == "accept-only")
        .expect("the accept-only peering");

    assert_eq!(accept_only.endpoint(), None);
    assert_eq!(accept_only.dial(), None);
}
