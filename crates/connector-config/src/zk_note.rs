//! `[zk_note]`: the one opt-in table that makes a node a **zk edge** (ADR
//! 0080, issue #1522).
//!
//! It names the vault and its Mina network, the GraphQL endpoint the vault is
//! read from and whether that rides the node's `socks_proxy`, the file holding
//! the state key the zk edge signs successors with, the verification key hash
//! request proofs are checked against, the tiers it publishes, how long an
//! allowance lives, and the **open budget** cap. Absent, the node is exactly
//! what it was before ADR 0080: `scheme: "zk-note"` is an unknown scheme.
//!
//! The vault's asset is native MINA, 9 decimals (decision 7, as amended), and
//! the table declares it by existing: inside the connector it is the
//! `AssetId` `mina:<network>/slip44:12586` ([`ZkNoteConfig::asset`]). It is the only place a
//! Mina asset is accepted -- `mina:` stays refused in `[[tokens]]` and in every
//! settlement table -- and a `[[rates]]` row may name it as its `from`.

use std::path::PathBuf;
use std::time::Duration;

use connector_domain::AssetId;
use connector_zk_note::note::policy::CHALLENGE_WINDOW_MS;
use connector_zk_note::tiers::Tiers;
use connector_zk_note::PubKey;
use serde::Deserialize;
use url::Url;

use crate::error::ConfigError;
use crate::secret::SecretLocation;
use crate::settlement::{check_circuit_route, CircuitRouteRefusal};

/// The vault asset's decimals: native MINA's nanomina.
pub const ZK_NOTE_DECIMALS: u8 = 9;

/// The longest allowance a zk edge may grant, as a fraction of zk_toon's
/// challenge window: a quarter. An allowance has to close, its charge be
/// journaled and its evidence reach the challenger inside that window, so its
/// lifetime has to be well below it (ADR 0080 decision 3, as amended); a
/// quarter leaves three quarters of the window for everything after expiry.
pub const MAX_ALLOWANCE_LIFETIME_MS: u64 = CHALLENGE_WINDOW_MS / 4;

/// The `[zk_note]` table as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawZkNoteTable {
    vault: String,
    network: String,
    graphql_url: String,
    #[serde(default)]
    rpc_via_socks_proxy: bool,
    state_key_file: PathBuf,
    verification_key_hash: String,
    tiers: Vec<u64>,
    allowance_lifetime_secs: u64,
    open_budget_cap: u64,
}

/// The Mina network a vault lives on, as its CAIP-2 id: the domain's, since
/// it is also the network half of the vault's [`AssetId`].
pub use connector_domain::MinaNetwork;

/// A validated `[zk_note]` table.
#[derive(Debug, Clone)]
pub struct ZkNoteConfig {
    vault: String,
    network: MinaNetwork,
    graphql_url: Url,
    rpc_via_socks_proxy: bool,
    state_key: SecretLocation,
    verification_key_hash: String,
    tiers: Tiers,
    allowance_lifetime: Duration,
    open_budget_cap: u64,
}

impl ZkNoteConfig {
    /// The vault's zkApp address (`B62…`): the greeting's `payTo`.
    pub fn vault(&self) -> &str {
        &self.vault
    }

    /// The vault's network.
    pub fn network(&self) -> MinaNetwork {
        self.network
    }

    /// Where the vault's state is read from.
    pub fn graphql_url(&self) -> &Url {
        &self.graphql_url
    }

    /// Whether every read of `graphql_url` rides the node's `socks_proxy`
    /// (ADR 0080 decision 9, ADR 0073).
    pub fn rpc_via_socks_proxy(&self) -> bool {
        self.rpc_via_socks_proxy
    }

    /// Where the state key lives: a file holding a Mina secret key.
    pub fn state_key(&self) -> &SecretLocation {
        &self.state_key
    }

    /// The hash of the request proof's verification key.
    pub fn verification_key_hash(&self) -> &str {
        &self.verification_key_hash
    }

    /// The tiers this zk edge publishes: a subset of zk_toon's.
    pub fn tiers(&self) -> &Tiers {
        &self.tiers
    }

    /// The longest allowance this zk edge grants.
    pub fn allowance_lifetime(&self) -> Duration {
        self.allowance_lifetime
    }

    /// The most the budgets of all open allowances may sum to, nanomina
    /// (ADR 0080 decision 5).
    pub fn open_budget_cap(&self) -> u64 {
        self.open_budget_cap
    }

    /// The vault's asset inside the connector: native MINA on the vault's
    /// network, `mina:<network>/slip44:12586`.
    pub fn asset(&self) -> AssetId {
        AssetId::mina(self.network)
    }
}

/// Validate the table. `socks_proxy` is the node's own, which the GraphQL
/// reads may opt into.
pub(crate) fn resolve_zk_note(
    raw: RawZkNoteTable,
    socks_proxy: Option<&Url>,
) -> Result<ZkNoteConfig, ConfigError> {
    // A zkApp address is a Mina public key; one that does not decode is a
    // vault no payer could deposit into.
    if PubKey::from_address(&raw.vault).is_err() {
        return Err(ConfigError::ZkNoteVaultInvalid { value: raw.vault });
    }
    let network = match raw.network.as_str() {
        "mina:mainnet" => MinaNetwork::Mainnet,
        "mina:devnet" => MinaNetwork::Devnet,
        _ => return Err(ConfigError::ZkNoteNetworkInvalid { value: raw.network }),
    };
    let graphql_url = Url::parse(&raw.graphql_url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .ok_or_else(|| ConfigError::ZkNoteGraphqlUrlInvalid {
            value: raw.graphql_url.clone(),
        })?;
    // ADR 0073's two rules for a read that rides the circuit: the ones a
    // settlement table is held to, named for this table.
    if raw.rpc_via_socks_proxy {
        check_circuit_route(&graphql_url, socks_proxy).map_err(|refusal| match refusal {
            CircuitRouteRefusal::WithoutProxy => ConfigError::ZkNoteViaSocksProxyWithoutProxy,
            CircuitRouteRefusal::Plaintext => ConfigError::ZkNoteViaSocksProxyPlaintext {
                value: raw.graphql_url.clone(),
            },
        })?;
    }
    if !raw.state_key_file.is_file() {
        return Err(ConfigError::ZkNoteStateKeyFileNotFound(raw.state_key_file));
    }
    // A Mina verification key hash is a field element, printed in decimal.
    let canonical = !raw.verification_key_hash.is_empty()
        && raw
            .verification_key_hash
            .bytes()
            .all(|b| b.is_ascii_digit())
        && raw
            .verification_key_hash
            .parse::<connector_zk_note::Field>()
            .is_ok_and(|field| {
                connector_zk_note::note::field_to_string(field) == raw.verification_key_hash
            });
    if !canonical {
        return Err(ConfigError::ZkNoteVerificationKeyHashInvalid {
            value: raw.verification_key_hash,
        });
    }
    let tiers =
        Tiers::new(raw.tiers).map_err(|source| ConfigError::ZkNoteTiersInvalid { source })?;
    let lifetime_ms = raw.allowance_lifetime_secs.saturating_mul(1_000);
    if lifetime_ms == 0 || lifetime_ms > MAX_ALLOWANCE_LIFETIME_MS {
        return Err(ConfigError::ZkNoteAllowanceLifetimeInvalid {
            secs: raw.allowance_lifetime_secs,
            max_secs: MAX_ALLOWANCE_LIFETIME_MS / 1_000,
        });
    }
    let smallest = tiers.as_slice()[0].budget();
    if raw.open_budget_cap < smallest {
        return Err(ConfigError::ZkNoteOpenBudgetCapBelowTier {
            cap: raw.open_budget_cap,
            tier: smallest,
        });
    }
    Ok(ZkNoteConfig {
        vault: raw.vault,
        network,
        graphql_url,
        rpc_via_socks_proxy: raw.rpc_via_socks_proxy,
        state_key: SecretLocation::File(raw.state_key_file),
        verification_key_hash: raw.verification_key_hash,
        tiers,
        allowance_lifetime: Duration::from_millis(lifetime_ms),
        open_budget_cap: raw.open_budget_cap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use connector_zk_note::note::policy::TIERS;
    use connector_zk_note::{Keypair, Scalar, SecKey};
    use std::io::Write;
    use std::path::Path;

    /// A real Mina address, made from a throwaway secret.
    fn address() -> String {
        Keypair::try_from(SecKey::new(Scalar::from(7u64)))
            .expect("a keypair")
            .public
            .into_address()
    }

    const VK_HASH: &str = "12345678901234567890";

    fn table(state_key: &Path, extra: &str) -> String {
        format!(
            r#"
[zk_note]
vault = "{vault}"
network = "mina:devnet"
graphql_url = "https://api.minascan.io/node/devnet/v1/graphql"
state_key_file = "{key}"
verification_key_hash = "{VK_HASH}"
tiers = [{t0}, {t1}]
allowance_lifetime_secs = 3600
open_budget_cap = {cap}
{extra}
"#,
            vault = address(),
            key = state_key.display(),
            t0 = TIERS[0],
            t1 = TIERS[1],
            cap = 10 * TIERS[1],
        )
    }

    /// Load a node with `[zk_note]` and whatever `edit` does to its text.
    /// `state_dir` is set unless `edit` removes it.
    fn load(edit: impl Fn(String) -> String) -> Result<Config, ConfigError> {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut signer = tempfile::NamedTempFile::new().expect("signer key");
        signer.write_all(b"not a real key").expect("write");
        let state_key = dir.path().join("zk-state.key");
        std::fs::write(&state_key, b"not a real key").expect("write");
        let text = format!(
            r#"
client_edge_addr = "127.0.0.1:3000"
state_dir = "{state}"

[signer]
key_file = "{signer}"
{table}
"#,
            state = dir.path().join("state").display(),
            signer = signer.path().display(),
            table = table(&state_key, ""),
        );
        Config::from_toml_str(&edit(text), Path::new("test.toml"))
    }

    #[test]
    fn a_zk_note_table_loads_and_declares_native_mina() {
        let config = load(|text| text).expect("loads");
        let zk = config.zk_note().expect("the table");
        assert_eq!(zk.vault(), address());
        assert_eq!(zk.network().caip2(), "mina:devnet");
        assert_eq!(zk.asset().to_string(), "mina:devnet/slip44:12586");
        assert_eq!(zk.verification_key_hash(), VK_HASH);
        assert_eq!(zk.allowance_lifetime(), Duration::from_secs(3600));
        assert_eq!(zk.open_budget_cap(), 10 * TIERS[1]);
        let tiers: Vec<u64> = zk.tiers().as_slice().iter().map(|t| t.budget()).collect();
        assert_eq!(tiers, [TIERS[0], TIERS[1]]);
        assert!(!zk.rpc_via_socks_proxy());
    }

    #[test]
    fn without_the_table_a_node_is_no_zk_edge() {
        let config =
            load(|text| text[..text.find("[zk_note]").unwrap()].to_string()).expect("loads");
        assert!(config.zk_note().is_none());
    }

    #[test]
    fn a_tier_zk_toon_does_not_publish_is_refused_naming_it() {
        let error = load(|text| {
            text.replace(
                &format!("tiers = [{}, {}]", TIERS[0], TIERS[1]),
                "tiers = [12345]",
            )
        })
        .unwrap_err();
        assert!(
            matches!(error, ConfigError::ZkNoteTiersInvalid { .. }),
            "{error}"
        );
        assert!(error.to_string().contains("12345"), "{error}");
    }

    #[test]
    fn a_zk_edge_needs_a_state_dir() {
        let error = load(|text| {
            text.lines()
                .filter(|line| !line.starts_with("state_dir"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_err();
        assert!(
            matches!(error, ConfigError::ZkNoteWithoutStateDir),
            "{error}"
        );
    }

    #[test]
    fn an_allowance_lifetime_not_well_below_the_challenge_window_is_refused() {
        for secs in ["0", "43201"] {
            let error = load(|text| {
                text.replace(
                    "allowance_lifetime_secs = 3600",
                    &format!("allowance_lifetime_secs = {secs}"),
                )
            })
            .unwrap_err();
            assert!(
                matches!(error, ConfigError::ZkNoteAllowanceLifetimeInvalid { .. }),
                "{secs}: {error}"
            );
        }
        load(|text| {
            text.replace(
                "allowance_lifetime_secs = 3600",
                "allowance_lifetime_secs = 43200",
            )
        })
        .expect("a quarter of the window is the longest");
    }

    #[test]
    fn an_open_budget_cap_below_the_smallest_tier_is_refused() {
        let error = load(|text| {
            text.replace(
                &format!("open_budget_cap = {}", 10 * TIERS[1]),
                &format!("open_budget_cap = {}", TIERS[0] - 1),
            )
        })
        .unwrap_err();
        assert!(
            matches!(error, ConfigError::ZkNoteOpenBudgetCapBelowTier { .. }),
            "{error}"
        );
    }

    #[test]
    fn each_malformed_pin_is_refused_by_name() {
        let vault = load(|text| text.replace(&address(), "B62qnotanaddress")).unwrap_err();
        assert!(
            matches!(vault, ConfigError::ZkNoteVaultInvalid { .. }),
            "{vault}"
        );
        let network = load(|text| text.replace("mina:devnet", "mina:berkeley")).unwrap_err();
        assert!(
            matches!(network, ConfigError::ZkNoteNetworkInvalid { .. }),
            "{network}"
        );
        let url = load(|text| text.replace("https://api", "ftp://api")).unwrap_err();
        assert!(
            matches!(url, ConfigError::ZkNoteGraphqlUrlInvalid { .. }),
            "{url}"
        );
        let hash = load(|text| text.replace(VK_HASH, "0x12")).unwrap_err();
        assert!(
            matches!(hash, ConfigError::ZkNoteVerificationKeyHashInvalid { .. }),
            "{hash}"
        );
        let key = load(|text| text.replace("zk-state.key", "missing.key")).unwrap_err();
        assert!(
            matches!(key, ConfigError::ZkNoteStateKeyFileNotFound(_)),
            "{key}"
        );
        let unknown = load(|text| text.replace("open_budget_cap", "budget = 1\nopen_budget_cap"))
            .unwrap_err();
        assert!(matches!(unknown, ConfigError::Parse { .. }), "{unknown}");
    }

    #[test]
    fn graphql_over_the_circuit_follows_adr_0073s_rules() {
        let opt_in = |text: String| {
            text.replace(
                "open_budget_cap",
                "rpc_via_socks_proxy = true\nopen_budget_cap",
            )
        };
        let error = load(opt_in).unwrap_err();
        assert!(
            matches!(error, ConfigError::ZkNoteViaSocksProxyWithoutProxy),
            "{error}"
        );
        let with_proxy = |text: String| {
            opt_in(text).replace(
                "state_dir",
                "socks_proxy = \"socks5h://127.0.0.1:9050\"\nstate_dir",
            )
        };
        load(with_proxy).expect("https over the circuit");
        let error = load(|text| with_proxy(text).replace("https://api", "http://api")).unwrap_err();
        assert!(
            matches!(error, ConfigError::ZkNoteViaSocksProxyPlaintext { .. }),
            "{error}"
        );
    }

    const USDC: &str = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce";
    /// The vault's asset on the fixture's `mina:devnet`.
    const MINA: &str = "mina:devnet/slip44:12586";

    fn usdc() -> AssetId {
        AssetId::erc20(84532, USDC)
    }

    /// A zk edge that also settles on EVM: its forwards are paid in the
    /// settlement token, so they cross from native MINA at a declared rate.
    fn with_evm(text: String, rates: &str) -> String {
        let key = text
            .lines()
            .find_map(|line| line.strip_prefix("key_file = "))
            .expect("signer key line")
            .to_string();
        format!(
            r#"{text}
[settlement.eip155.84532]
rpc_url = "https://base-sepolia.example"
token_address = "{USDC}"
decimals = 6
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[settlement.eip155.84532.key]
key_file = {key}

[[tokens]]
asset = "eip155:84532/erc20:{USDC}"
numeraire = true

[rate_guards]
spread = {{ numerator = 1, denominator = 100 }}
ttl_secs = 60
max_move = {{ numerator = 1, denominator = 10 }}
{rates}
"#
        )
    }

    #[test]
    fn a_forward_out_of_mina_needs_a_declared_rate_naming_the_pair() {
        let error = load(|text| with_evm(text, "")).unwrap_err();
        let ConfigError::ZkNoteRateMissing { from, to } = &error else {
            panic!("{error}");
        };
        assert_eq!(from.to_string(), MINA);
        assert_eq!(to, &usdc().to_string());

        let config = load(|text| {
            with_evm(
                text,
                &format!(
                    "[[rates]]\nfrom = \"{MINA}\"\nto = \"eip155:84532/erc20:{USDC}\"\nrate = {{ numerator = 1, denominator = 1000 }}\n"
                ),
            )
        })
        .expect("a declared rate out of MINA");
        assert!(config
            .denomination()
            .rate(&AssetId::mina(MinaNetwork::Devnet), &usdc())
            .is_some());
        // ADR 0071 at a zk edge: an allowance-paid packet is denominated in
        // the vault's asset, so a forward out of it converts at that rate.
        assert_eq!(
            config
                .client_channel_assets()
                .asset("zk-note:00112233445566778899aabbccddeeff"),
            Some(&AssetId::mina(MinaNetwork::Devnet))
        );
    }

    #[test]
    fn mina_is_still_refused_outside_the_zk_note_table() {
        // As a dealt token.
        let error = load(|text| {
            with_evm(text, "").replace(
                "[rate_guards]",
                &format!("[[tokens]]\nasset = \"{MINA}\"\n\n[rate_guards]"),
            )
        })
        .unwrap_err();
        assert!(
            matches!(error, ConfigError::TokenMinaNative { .. }),
            "{error}"
        );
        assert!(error.to_string().contains(MINA), "{error}");
        // As a rate's `from`, on a node with no zk-note table.
        let error = load(|text| {
            let text = with_evm(
                text,
                &format!(
                    "[[rates]]\nfrom = \"{MINA}\"\nto = \"eip155:84532/erc20:{USDC}\"\nrate = {{ numerator = 1, denominator = 1000 }}\n"
                ),
            );
            let start = text.find("[zk_note]").unwrap();
            let end = text.find("[settlement.eip155.84532]").unwrap();
            format!("{}{}", &text[..start], &text[end..])
        })
        .unwrap_err();
        assert!(
            matches!(error, ConfigError::RateRowUnknownToken { .. }),
            "{error}"
        );
        // As a settlement table.
        let error = load(|text| format!("{text}\n[settlement.mina]\nrpc_url = \"https://x\"\n"))
            .unwrap_err();
        assert!(matches!(error, ConfigError::Parse { .. }), "{error}");
    }
}
