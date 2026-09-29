use std::path::PathBuf;

use connector_domain::{AssetChain, AssetId};
use serde::Deserialize;
use url::Url;

use crate::batch_settlement::{
    resolve_evm_batch_settlement, resolve_solana_batch_settlement, EvmBatchSettlementConfig,
    RawEvmBatchSettlementTable, RawSolanaBatchSettlementTable, SolanaBatchSettlementConfig,
};
use crate::encoding::{parse_evm_address, to_hex};
use crate::error::ConfigError;
use crate::secret::SecretLocation;

/// The `[settlement]` section as written in the config file: one table per
/// chain, `[settlement.evm]` and/or `[settlement.solana]` (issue #628).
///
/// The legacy flat shape -- `chain`, `rpc_url`, `contract_address`,
/// `token_address`, `decimals` and `[settlement.key]` directly under
/// `[settlement]` -- named a `TokenNetworkRegistry` and is refused by name
/// (ADR 0075 decision 9, issue #1385). It is still recognised, by its
/// `chain` key, purely so the refusal can say what it is: a keyed section
/// never writes `chain`, so the two can never be read two ways.
///
/// The keyed shape is boxed only because it is much the larger of the two.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum RawSettlementSection {
    #[allow(dead_code)]
    Legacy(RawLegacySettlementConfig),
    Keyed(Box<RawKeyedSettlementConfig>),
}

/// The retired flat `[settlement]` shape, recognised by its `chain` key and
/// otherwise unread: every value it could hold named a `TokenNetwork`
/// deployment, so it is refused by name
/// ([`ConfigError::SettlementLegacyShapeRemoved`]).
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct RawLegacySettlementConfig {
    chain: toml::Value,
    #[serde(flatten)]
    rest: toml::Table,
}

/// The keyed `[settlement]` shape (issue #628): zero or more per-chain
/// tables, each self-naming its chain by its own key rather than a `chain`
/// field. `deny_unknown_fields` so a chain this connector has no table for
/// (or a typo'd one, e.g. `slana`) fails loudly rather than being silently
/// ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawKeyedSettlementConfig {
    #[serde(default)]
    evm: Option<RawEvmSettlementTable>,
    #[serde(default)]
    solana: Option<RawSolanaSettlementTable>,
}

/// `[settlement.evm]`: this node's x402 `batch-settlement` terms on EVM
/// (ADR 0075 decisions 1 and 9). The contract is a constant of the binary
/// (`connector_signer::X402_BATCH_SETTLEMENT_ADDRESS`), never config.
///
/// Every key a `TokenNetwork` deployment needed is parsed only to be
/// refused by name: `contract_address` (the `TokenNetworkRegistry`),
/// `channel_index_from_block`/`channel_index_confirmations` (the deleted
/// `TokenNetwork` channel index), and the `batch_settlement` sub-table,
/// whose keys now sit in this table directly.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawEvmSettlementTable {
    rpc_url: String,
    token_address: String,
    decimals: u8,
    key: RawSettlementKeyConfig,
    /// ADR 0073: dial this table's `rpc_url` through the root
    /// `socks_proxy`. See [`EvmSettlementConfig::rpc_via_socks_proxy`].
    #[serde(default)]
    rpc_via_socks_proxy: bool,
    #[serde(default)]
    min_withdraw_delay_secs: Option<u64>,
    #[serde(default)]
    asset_eip712_name: Option<String>,
    #[serde(default)]
    asset_eip712_version: Option<String>,
    /// toon-client#695: `"eip3009"` (default) or `"permit2"`.
    #[serde(default)]
    asset_transfer_method: Option<String>,
    /// toon-client#695: the x402 facilitator payers relay deposits through.
    #[serde(default)]
    facilitator_url: Option<String>,
    #[serde(default)]
    contract_address: Option<toml::Value>,
    #[serde(default)]
    channel_index_from_block: Option<toml::Value>,
    #[serde(default)]
    channel_index_confirmations: Option<toml::Value>,
    #[serde(default)]
    batch_settlement: Option<toml::Value>,
}

/// `[settlement.solana]`: this node's x402 `batch-settlement` terms on
/// Solana (ADR 0075 decisions 1 and 9). The program is `payment-channels`
/// at the one id the binary fixes, never config, so `program_id` -- which
/// named TOON's own payment-channel program -- is refused by name, as is
/// the `batch_settlement` sub-table whose keys now sit here directly.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSolanaSettlementTable {
    rpc_url: String,
    token_address: String,
    decimals: u8,
    key: RawSettlementKeyConfig,
    /// ADR 0073: dial this table's `rpc_url` through the root
    /// `socks_proxy`. See [`SolanaSettlementConfig::rpc_via_socks_proxy`].
    #[serde(default)]
    rpc_via_socks_proxy: bool,
    #[serde(default)]
    min_grace_period_secs: Option<u64>,
    #[serde(default)]
    min_sponsored_deposit: Option<u64>,
    #[serde(default)]
    program_id: Option<toml::Value>,
    #[serde(default)]
    batch_settlement: Option<toml::Value>,
}

/// The `[settlement.evm.key]`/`[settlement.solana.key]` sub-section: where
/// the key material this node signs settlement transactions and vouchers
/// with lives. Same File-or-KMS shape as the top-level `[signer]` section
/// (`crate::secret`), kept as its own type rather than reused directly
/// because these are independent config-file positions with their own
/// `deny_unknown_fields` boundary.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSettlementKeyConfig {
    #[serde(default)]
    key_file: Option<PathBuf>,
    #[serde(default)]
    kms_key_id: Option<String>,
}

/// The chains a [`SettlementConfig`] can name: EVM and Solana, the two
/// chains x402 `batch-settlement` channels live on (ADR 0075 decision 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettlementChain {
    Evm,
    Solana,
}

impl SettlementChain {
    /// The chain's config-file name -- the `[settlement.<name>]` table
    /// key. The one
    /// spelling of each chain this workspace has, reused anywhere a chain
    /// must be named to or by an operator (e.g. the operator surface's
    /// `POST /channels` `chain` field).
    pub fn name(self) -> &'static str {
        match self {
            SettlementChain::Evm => "evm",
            SettlementChain::Solana => "solana",
        }
    }

    /// The same chain as the domain names it (ADR 0071, issue #1290).
    ///
    /// The translation between the two enums lives here and not in
    /// `connector-domain`, because that crate must not depend on config to
    /// say what a token is -- its `asset` module says so in as many words.
    /// One function each way ([`From<AssetChain>`](SettlementChain) is the
    /// other), so that nothing downstream writes a second `match` over the
    /// two chains that could one day answer differently.
    pub const fn asset_chain(self) -> AssetChain {
        match self {
            SettlementChain::Evm => AssetChain::Evm,
            SettlementChain::Solana => AssetChain::Solana,
        }
    }
}

impl From<AssetChain> for SettlementChain {
    fn from(chain: AssetChain) -> SettlementChain {
        match chain {
            AssetChain::Evm => SettlementChain::Evm,
            AssetChain::Solana => SettlementChain::Solana,
        }
    }
}

impl std::fmt::Display for SettlementChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for SettlementChain {
    type Err = UnknownSettlementChain;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "evm" => Ok(SettlementChain::Evm),
            "solana" => Ok(SettlementChain::Solana),
            other => Err(UnknownSettlementChain(other.to_string())),
        }
    }
}

/// A chain name `SettlementChain::from_str` does not recognize. It names
/// every chain the `[settlement.<chain>]` tables -- and therefore the rest
/// of the fleet -- recognize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownSettlementChain(pub String);

impl std::fmt::Display for UnknownSettlementChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown settlement chain '{}' -- supported chains: evm, solana",
            self.0
        )
    }
}

/// A fully validated `[settlement.evm]` table: which ERC-20 this node
/// settles in, its RPC endpoint, where its signing key material lives, and
/// its x402 `batch-settlement` terms. Constructed only by
/// [`resolve_settlement`], so a value that exists has already had every
/// field checked -- downstream code never re-validates any of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmSettlementConfig {
    rpc_url: String,
    token_address: [u8; 20],
    decimals: u8,
    key: SecretLocation,
    rpc_via_socks_proxy: bool,
    batch_settlement: EvmBatchSettlementConfig,
}

impl EvmSettlementConfig {
    /// The RPC endpoint this node reaches the chain through.
    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// The ERC-20 asset every channel this node opens or admits settles in.
    pub fn token_address(&self) -> [u8; 20] {
        self.token_address
    }

    /// The settlement asset's decimal precision (6 for the USDC this
    /// connector settles). Nothing scales by it: it is honoured as a
    /// startup *check* against the deployed token's own `decimals()`
    /// (issue #564).
    pub fn decimals(&self) -> u8 {
        self.decimals
    }

    /// Where this node's EVM signing key material lives.
    pub fn key(&self) -> &SecretLocation {
        &self.key
    }

    /// Whether every client of this table's `rpc_url` (the backend and the
    /// rate source) dials through the root
    /// `socks_proxy`, on this chain's own pinned circuit (ADR 0073). `false`
    /// unless the table says so. When `true`, `Config::load` has already
    /// checked that a `socks_proxy` exists, and that the endpoint is
    /// `https` unless its host is an onion address, since an exit relay
    /// could otherwise read and rewrite every answer.
    pub fn rpc_via_socks_proxy(&self) -> bool {
        self.rpc_via_socks_proxy
    }

    /// This node's terms for x402 batch-settlement channels on EVM (ADR
    /// 0074, ADR 0075 decision 9): the only channels it settles on, so every
    /// `[settlement.evm]` table carries them.
    pub fn batch_settlement(&self) -> &EvmBatchSettlementConfig {
        &self.batch_settlement
    }
}

/// A fully validated `[settlement.solana]` table: which SPL mint this node
/// settles in, its RPC endpoint, where its signing key material lives, and
/// its x402 `batch-settlement` terms on `payment-channels`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolanaSettlementConfig {
    rpc_via_socks_proxy: bool,
    rpc_url: String,
    token_address: String,
    decimals: u8,
    key: SecretLocation,
    batch_settlement: SolanaBatchSettlementConfig,
}

impl SolanaSettlementConfig {
    /// The RPC endpoint this node reaches the chain through.
    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// The SPL token mint every channel this node opens or admits settles
    /// in, base58-encoded.
    pub fn token_address(&self) -> &str {
        &self.token_address
    }

    /// The settlement asset's decimal precision.
    pub fn decimals(&self) -> u8 {
        self.decimals
    }

    /// Where this node's Solana signing key material lives.
    pub fn key(&self) -> &SecretLocation {
        &self.key
    }

    /// The Solana cluster this table's `rpc_url` names, when it is one of
    /// the well-known public endpoints or a loopback address -- `None` for
    /// any other host, e.g. a paid third-party RPC provider (Helius,
    /// Alchemy, QuickNode, ...) whose URL names no cluster at all (issue
    /// #975). Guessing wrong from a substring match would be worse than not
    /// checking, so this only recognises an exact, canonical hostname.
    ///
    /// A **hint**, and the *fallback* rather than the source: a running
    /// node takes its cluster from the chain's own genesis hash (issue
    /// #1131), which holds however the node reached the chain. What this
    /// still answers, and the genesis hash cannot, is the loopback case:
    /// `solana-test-validator` mints a fresh genesis on every run and
    /// therefore matches no published cluster hash, while its URL still
    /// says `localnet`.
    pub fn cluster_hint(&self) -> Option<&'static str> {
        cluster_hint_for_rpc_url(&self.rpc_url)
    }

    /// Whether this table's `rpc_url` is dialed through the root
    /// `socks_proxy`, on the Solana settlement circuit (ADR 0073). The same
    /// rule and the same load-time checks as
    /// [`EvmSettlementConfig::rpc_via_socks_proxy`].
    pub fn rpc_via_socks_proxy(&self) -> bool {
        self.rpc_via_socks_proxy
    }

    /// This node's terms for x402 batch-settlement channels on Solana (ADR
    /// 0074, ADR 0075 decision 9): the only channels it settles on, so every
    /// `[settlement.solana]` table carries them.
    pub fn batch_settlement(&self) -> &SolanaBatchSettlementConfig {
        &self.batch_settlement
    }
}

/// [`SolanaSettlementConfig::cluster_hint`]'s free-function half, split out
/// so it is testable against a bare URL string without building a whole
/// resolved config.
fn cluster_hint_for_rpc_url(rpc_url: &str) -> Option<&'static str> {
    let host = Url::parse(rpc_url).ok()?.host_str()?.to_ascii_lowercase();
    match host.as_str() {
        "api.mainnet-beta.solana.com" => Some("mainnet-beta"),
        "api.devnet.solana.com" => Some("devnet"),
        "api.testnet.solana.com" => Some("testnet"),
        "localhost" | "127.0.0.1" => Some("localnet"),
        _ => None,
    }
}

/// One fully validated per-chain settlement table -- typed by chain (issue
/// #628), since an EVM table and a Solana table name genuinely different
/// on-chain facts and a single shared shape would either force one to fake
/// fields it does not have or erase which chain a value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlementConfig {
    Evm(EvmSettlementConfig),
    Solana(SolanaSettlementConfig),
}

impl SettlementConfig {
    /// The chain this table settles on.
    pub fn chain(&self) -> SettlementChain {
        match self {
            SettlementConfig::Evm(_) => SettlementChain::Evm,
            SettlementConfig::Solana(_) => SettlementChain::Solana,
        }
    }

    /// The token every channel on this chain settles in, named the way
    /// a `[[tokens]]` row names one (ADR 0071, issue #1290): this table's
    /// own `token_address`, on this table's own chain.
    ///
    /// Derived, never declared twice -- which is the point. A node that
    /// deals has to be able to ask whether the token a channel holds is one
    /// it declared, and the answer has to come from the table that already
    /// states it rather than from a second place an operator could write a
    /// different address.
    pub fn asset(&self) -> AssetId {
        match self {
            SettlementConfig::Evm(evm) => AssetId::evm(to_hex(&evm.token_address)),
            SettlementConfig::Solana(solana) => AssetId::solana(&solana.token_address),
        }
    }

    /// This table's RPC endpoint.
    pub fn rpc_url(&self) -> &str {
        match self {
            SettlementConfig::Evm(evm) => evm.rpc_url(),
            SettlementConfig::Solana(solana) => solana.rpc_url(),
        }
    }

    /// Whether this table's RPC rides the root `socks_proxy` (ADR 0073).
    pub fn rpc_via_socks_proxy(&self) -> bool {
        match self {
            SettlementConfig::Evm(evm) => evm.rpc_via_socks_proxy(),
            SettlementConfig::Solana(solana) => solana.rpc_via_socks_proxy(),
        }
    }
}

/// ADR 0073 decisions 1 and 2, checked at load: a table that sends its RPC
/// through `socks_proxy` needs a `socks_proxy` to send it through, and an
/// endpoint an exit relay cannot read or rewrite.
///
/// There is no fallback to direct, so without a proxy the key could only
/// mean "fail every settlement call". Refused here rather than at the first
/// call.
///
/// Plain `http://` is refused unless the host is an onion address. Through
/// a circuit, the exit relay is a stranger on the path, and over plaintext
/// it could rewrite a channel's deposit, a transaction's receipt or a
/// blockhash, and a node that believed it would honour claims against
/// collateral that does not exist. TLS closes that. An onion host closes it
/// differently, because its address is the key the circuit authenticates
/// to, exactly as ADR 0070 decision 2 argues for peer endpoints.
pub(crate) fn check_settlement_rpc_routes(
    settlements: &[SettlementConfig],
    socks_proxy: Option<&Url>,
) -> Result<(), ConfigError> {
    for settlement in settlements.iter().filter(|s| s.rpc_via_socks_proxy()) {
        let table = settlement.chain().name();
        if socks_proxy.is_none() {
            return Err(ConfigError::SettlementRpcViaSocksProxyWithoutProxy { table });
        }
        let url = Url::parse(settlement.rpc_url())
            .expect("resolve_rpc_url already parsed every settlement rpc_url");
        if url.scheme() == "http" && !crate::is_onion_endpoint(&url) {
            return Err(ConfigError::SettlementRpcViaSocksProxyPlaintext {
                table,
                value: settlement.rpc_url().to_string(),
            });
        }
    }
    Ok(())
}

/// Which `[settlement.<chain>]` tables a config declares -- the single
/// input every "the settlement table this channel needs is absent" rule
/// reads (issue #1138).
///
/// There is **one** such rule and it governs every channel table, because
/// the reason is one reason. A `[settlement.<chain>]` table is where this
/// node's on-chain identity on that chain comes from and where its x402
/// terms are (ADR 0075 decision 9): `[settlement.evm.key]` is this node's
/// EVM address and voucher signer, and `[settlement.solana.key]` its
/// Solana one. The connector holds a signer rather than a wallet (ADR
/// 0012), so a node with no table for a chain has no address on it at all,
/// and can neither admit a voucher nor sign one there.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SettlementTables {
    evm: bool,
    solana: bool,
}

impl SettlementTables {
    /// Read the tables off an already-resolved settlement list. Called
    /// once in `Config::load`, immediately after `resolve_settlement`, so
    /// every channel table is resolved against the same answer.
    pub(crate) fn of(settlements: &[SettlementConfig]) -> Self {
        SettlementTables {
            evm: settlements
                .iter()
                .any(|settlement| settlement.chain() == SettlementChain::Evm),
            solana: settlements
                .iter()
                .any(|settlement| settlement.chain() == SettlementChain::Solana),
        }
    }

    /// The same answer, stated directly, for a channel-table unit test
    /// that is about the channel row rather than about how a settlement
    /// table parses. `Config::load` always uses [`Self::of`].
    #[cfg(test)]
    pub(crate) fn for_tests(evm: bool, solana: bool) -> Self {
        SettlementTables { evm, solana }
    }

    /// Whether this node pays and is paid on x402 channels on `chain`: it
    /// has a `[settlement.<chain>]` table, which since ADR 0075 always
    /// carries the chain's x402 terms. An x402 channel row on a chain
    /// without one names a channel this node can neither admit a voucher on
    /// nor sign one on (issue #1380).
    pub(crate) fn x402(&self, chain: SettlementChain) -> bool {
        match chain {
            SettlementChain::Evm => self.evm,
            SettlementChain::Solana => self.solana,
        }
    }
}

/// A key a `TokenNetwork`-era settlement table wrote, refused by name (ADR
/// 0075 decision 9, ADR 0009): never parsed and ignored.
fn refuse_removed(written: Vec<(bool, ConfigError)>) -> Result<(), ConfigError> {
    match written.into_iter().find(|(present, _)| *present) {
        Some((_, refusal)) => Err(refusal),
        None => Ok(()),
    }
}

/// A key ADR 0075 decision 9 makes required, written or refused by name.
fn required<T>(table: &'static str, key: &'static str, value: Option<T>) -> Result<T, ConfigError> {
    value.ok_or(ConfigError::SettlementMissingRequiredKey { table, key })
}

fn resolve_evm_fields(table: RawEvmSettlementTable) -> Result<EvmSettlementConfig, ConfigError> {
    refuse_removed(vec![
        (
            table.contract_address.is_some(),
            ConfigError::SettlementToonKeyRemoved {
                table: "evm",
                field: "contract_address",
            },
        ),
        (
            table.batch_settlement.is_some(),
            ConfigError::SettlementBatchSubTableRemoved { table: "evm" },
        ),
        (
            table.channel_index_from_block.is_some(),
            ConfigError::SettlementChannelIndexKeyRemoved {
                field: "channel_index_from_block",
            },
        ),
        (
            table.channel_index_confirmations.is_some(),
            ConfigError::SettlementChannelIndexKeyRemoved {
                field: "channel_index_confirmations",
            },
        ),
    ])?;

    let rpc_url = resolve_rpc_url(table.rpc_url)?;
    let token_address = parse_evm_address(&table.token_address).ok_or_else(|| {
        ConfigError::SettlementInvalidTokenAddress {
            value: table.token_address.clone(),
        }
    })?;
    if table.decimals == 0 {
        return Err(ConfigError::SettlementZeroDecimals);
    }
    let key = resolve_settlement_key(table.key)?;
    let batch_settlement = resolve_evm_batch_settlement(RawEvmBatchSettlementTable {
        min_withdraw_delay_secs: table.min_withdraw_delay_secs,
        asset_eip712_name: required("evm", "asset_eip712_name", table.asset_eip712_name)?,
        asset_eip712_version: required("evm", "asset_eip712_version", table.asset_eip712_version)?,
        asset_transfer_method: table.asset_transfer_method,
        facilitator_url: table.facilitator_url,
    })?;

    Ok(EvmSettlementConfig {
        rpc_url,
        token_address,
        decimals: table.decimals,
        key,
        rpc_via_socks_proxy: table.rpc_via_socks_proxy,
        batch_settlement,
    })
}

fn resolve_solana_fields(
    table: RawSolanaSettlementTable,
) -> Result<SolanaSettlementConfig, ConfigError> {
    refuse_removed(vec![
        (
            table.program_id.is_some(),
            ConfigError::SettlementToonKeyRemoved {
                table: "solana",
                field: "program_id",
            },
        ),
        (
            table.batch_settlement.is_some(),
            ConfigError::SettlementBatchSubTableRemoved { table: "solana" },
        ),
    ])?;

    let rpc_url = resolve_rpc_url(table.rpc_url)?;
    if table.token_address.trim().is_empty() {
        return Err(ConfigError::SettlementMissingSolanaTokenAddress);
    }
    if table.decimals == 0 {
        return Err(ConfigError::SettlementZeroDecimals);
    }
    let key = resolve_settlement_key(table.key)?;
    let batch_settlement = resolve_solana_batch_settlement(RawSolanaBatchSettlementTable {
        min_grace_period_secs: table.min_grace_period_secs,
        min_sponsored_deposit: required(
            "solana",
            "min_sponsored_deposit",
            table.min_sponsored_deposit,
        )?,
    })?;

    Ok(SolanaSettlementConfig {
        rpc_via_socks_proxy: table.rpc_via_socks_proxy,
        rpc_url,
        token_address: table.token_address,
        decimals: table.decimals,
        key,
        batch_settlement,
    })
}

/// Validate an optional `[settlement]` section. Presence configures one
/// x402 settlement backend per table; absence is a node that settles on no
/// chain, and so takes no claim at all (every claim is a voucher, ADR
/// 0075).
///
/// Returns every chain the section names, each fully validated, at most one
/// per [`SettlementChain`] by construction.
pub(crate) fn resolve_settlement(
    raw: Option<RawSettlementSection>,
) -> Result<Vec<SettlementConfig>, ConfigError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };

    match raw {
        RawSettlementSection::Legacy(_) => Err(ConfigError::SettlementLegacyShapeRemoved),
        RawSettlementSection::Keyed(raw) => {
            let raw = *raw;
            if raw.evm.is_none() && raw.solana.is_none() {
                return Err(ConfigError::SettlementSectionEmpty);
            }
            let mut out = Vec::new();
            if let Some(evm) = raw.evm {
                out.push(SettlementConfig::Evm(resolve_evm_fields(evm)?));
            }
            if let Some(solana) = raw.solana {
                out.push(SettlementConfig::Solana(resolve_solana_fields(solana)?));
            }
            Ok(out)
        }
    }
}

fn resolve_settlement_key(raw: RawSettlementKeyConfig) -> Result<SecretLocation, ConfigError> {
    match (raw.key_file, raw.kms_key_id) {
        (Some(path), None) => {
            if !path.is_file() {
                return Err(ConfigError::SettlementKeyFileNotFound(path));
            }
            Ok(SecretLocation::File(path))
        }
        (None, Some(key_id)) => {
            if key_id.trim().is_empty() {
                return Err(ConfigError::SettlementKmsIdEmpty);
            }
            Ok(SecretLocation::Kms { key_id })
        }
        (None, None) => Err(ConfigError::SettlementKeyLocationAmbiguous {
            reason: "neither 'key_file' nor 'kms_key_id' is set",
        }),
        (Some(_), Some(_)) => Err(ConfigError::SettlementKeyLocationAmbiguous {
            reason: "both 'key_file' and 'kms_key_id' are set",
        }),
    }
}

/// Shared rpc_url validation between the EVM and Solana tables : non-empty, a well-formed URL, and http(s) -- none of this
/// is chain-specific.
fn resolve_rpc_url(rpc_url: String) -> Result<String, ConfigError> {
    if rpc_url.trim().is_empty() {
        return Err(ConfigError::SettlementMissingRpcUrl);
    }
    let url = Url::parse(&rpc_url).map_err(|source| ConfigError::SettlementInvalidRpcUrl {
        value: rpc_url.clone(),
        source,
    })?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(ConfigError::SettlementUnsupportedRpcScheme { value: rpc_url });
    }
    Ok(rpc_url)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const TOKEN: &str = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce";
    const MINT: &str = "SoLMint11111111111111111111111111111111111";

    fn temp_key_file() -> tempfile::NamedTempFile {
        tempfile::NamedTempFile::new().expect("temp key file")
    }

    fn section(body: &str) -> RawSettlementSection {
        toml::from_str(body).expect("valid settlement TOML")
    }

    fn resolve(body: &str) -> Result<Vec<SettlementConfig>, ConfigError> {
        resolve_settlement(Some(section(body)))
    }

    /// A complete `[settlement.evm]` table, with `extra` appended to it
    /// before its key sub-table.
    fn evm_table(extra: &str, key_file: &Path) -> String {
        format!(
            r#"
[evm]
rpc_url = "http://127.0.0.1:8545"
token_address = "{TOKEN}"
decimals = 6
asset_eip712_name = "USDC"
asset_eip712_version = "2"
{extra}

[evm.key]
key_file = "{key}"
"#,
            key = key_file.display()
        )
    }

    /// A complete `[settlement.solana]` table, with `extra` appended.
    fn solana_table(extra: &str, key_file: &Path) -> String {
        format!(
            r#"
[solana]
rpc_url = "http://127.0.0.1:8899"
token_address = "{MINT}"
decimals = 6
min_sponsored_deposit = 5
{extra}

[solana.key]
key_file = "{key}"
"#,
            key = key_file.display()
        )
    }

    fn single_evm(settlements: Vec<SettlementConfig>) -> EvmSettlementConfig {
        match settlements.as_slice() {
            [SettlementConfig::Evm(evm)] => evm.clone(),
            other => panic!("expected one EVM table, got {other:?}"),
        }
    }

    fn single_solana(settlements: Vec<SettlementConfig>) -> SolanaSettlementConfig {
        match settlements.as_slice() {
            [SettlementConfig::Solana(solana)] => solana.clone(),
            other => panic!("expected one Solana table, got {other:?}"),
        }
    }

    #[test]
    fn absent_settlement_section_resolves_to_none() {
        let resolved = resolve_settlement(None).expect("resolve");
        assert!(resolved.is_empty());
    }

    /// ADR 0075 decision 9: `[settlement.evm]` needs only an RPC URL, a
    /// token, its decimals, its asset's EIP-712 domain and a key, and its
    /// x402 terms sit in the table itself.
    #[test]
    fn a_complete_evm_table_resolves_with_its_x402_terms() {
        let key_file = temp_key_file();
        let evm = single_evm(
            resolve(&evm_table(
                "min_withdraw_delay_secs = 3600",
                key_file.path(),
            ))
            .expect("resolve"),
        );

        assert_eq!(evm.rpc_url(), "http://127.0.0.1:8545");
        assert_eq!(evm.token_address(), parse_evm_address(TOKEN).unwrap());
        assert_eq!(evm.decimals(), 6);
        assert_eq!(
            evm.key(),
            &SecretLocation::File(key_file.path().to_path_buf())
        );
        assert_eq!(evm.batch_settlement().min_withdraw_delay_secs(), 3600);
        assert_eq!(evm.batch_settlement().asset_eip712_name(), "USDC");
        assert_eq!(evm.batch_settlement().asset_eip712_version(), "2");
        assert_eq!(
            evm.batch_settlement().asset_transfer_method(),
            connector_domain::x402::X402AssetTransferMethod::Eip3009
        );
        assert_eq!(evm.batch_settlement().facilitator_url(), None);
    }

    /// toon-client#695: the deposit keys sit in `[settlement.evm]` itself,
    /// like every other x402 term, and reach the resolved terms.
    #[test]
    fn an_evm_table_carries_its_deposit_method_and_facilitator() {
        let key_file = temp_key_file();
        let evm = single_evm(
            resolve(&evm_table(
                "asset_transfer_method = \"permit2\"\nfacilitator_url = \"https://facilitator.example/x402\"",
                key_file.path(),
            ))
            .expect("resolve"),
        );
        assert_eq!(
            evm.batch_settlement().asset_transfer_method(),
            connector_domain::x402::X402AssetTransferMethod::Permit2
        );
        assert_eq!(
            evm.batch_settlement().facilitator_url(),
            Some("https://facilitator.example/x402")
        );
    }

    /// EVM-only: a Solana table that writes either key is refused as an
    /// unknown field, not silently ignored.
    #[test]
    fn a_solana_table_refuses_the_evm_deposit_keys() {
        let key_file = temp_key_file();
        for extra in [
            "asset_transfer_method = \"permit2\"",
            "facilitator_url = \"https://facilitator.example\"",
        ] {
            let body = solana_table(extra, key_file.path());
            let result: Result<RawSettlementSection, _> = toml::from_str(&body);
            assert!(result.is_err(), "{extra} must be refused on Solana");
        }
    }

    /// `[settlement.solana]` needs an RPC URL, a mint, its decimals and a
    /// key, plus the minimums (ADR 0075 decision 9).
    #[test]
    fn a_complete_solana_table_resolves_with_its_x402_terms() {
        let key_file = temp_key_file();
        let solana = single_solana(
            resolve(&solana_table(
                "min_grace_period_secs = 1800",
                key_file.path(),
            ))
            .expect("resolve"),
        );

        assert_eq!(solana.rpc_url(), "http://127.0.0.1:8899");
        assert_eq!(solana.token_address(), MINT);
        assert_eq!(solana.decimals(), 6);
        assert_eq!(solana.batch_settlement().min_grace_period_secs(), 1800);
        assert_eq!(solana.batch_settlement().min_sponsored_deposit(), 5);
    }

    #[test]
    fn declaring_both_evm_and_solana_parses_both_as_typed_per_chain_config() {
        let key_file = temp_key_file();
        let resolved = resolve(&format!(
            "{}{}",
            evm_table("", key_file.path()),
            solana_table("", key_file.path())
        ))
        .expect("resolve");

        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].chain(), SettlementChain::Evm);
        assert_eq!(resolved[1].chain(), SettlementChain::Solana);
    }

    /// The flat `[settlement]` shape named a `TokenNetworkRegistry` and is
    /// refused by name (ADR 0075 decision 9), never read as an empty keyed
    /// section.
    #[test]
    fn the_legacy_flat_shape_is_refused_by_name() {
        let key_file = temp_key_file();
        let error = resolve(&format!(
            r#"
chain = "evm"
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "{TOKEN}"
decimals = 6

[key]
key_file = "{key}"
"#,
            key = key_file.path().display()
        ))
        .expect_err("the flat shape is retired");

        assert!(matches!(error, ConfigError::SettlementLegacyShapeRemoved));
        let message = error.to_string();
        assert!(
            message.contains("[settlement.evm]") && message.contains("ADR 0075"),
            "the refusal names the shape to write and the record: {message}"
        );
    }

    /// `contract_address` named the `TokenNetworkRegistry`; the x402
    /// contract is a constant of the binary (ADR 0075 decisions 1 and 9).
    #[test]
    fn an_evm_contract_address_is_refused_by_name() {
        let key_file = temp_key_file();
        let error = resolve(&evm_table(
            "contract_address = \"0x1234567890123456789012345678901234567890\"",
            key_file.path(),
        ))
        .expect_err("contract_address is retired");

        assert!(matches!(
            error,
            ConfigError::SettlementToonKeyRemoved {
                table: "evm",
                field: "contract_address",
            }
        ));
        assert!(error.to_string().contains("contract_address"));
    }

    /// `program_id` named TOON's own payment-channel program;
    /// `payment-channels` is a constant of the binary.
    #[test]
    fn a_solana_program_id_is_refused_by_name() {
        let key_file = temp_key_file();
        let error = resolve(&solana_table(
            "program_id = \"2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip\"",
            key_file.path(),
        ))
        .expect_err("program_id is retired");

        assert!(matches!(
            error,
            ConfigError::SettlementToonKeyRemoved {
                table: "solana",
                field: "program_id",
            }
        ));
        assert!(error.to_string().contains("program_id"));
    }

    /// The `batch_settlement` sub-table's keys moved up a level, so the
    /// sub-table itself is refused by name on either chain rather than
    /// silently ignored.
    #[test]
    fn the_batch_settlement_sub_table_is_refused_by_name_on_both_chains() {
        let key_file = temp_key_file();
        let evm = resolve(&format!(
            "{}\n[evm.batch_settlement]\nmin_withdraw_delay_secs = 3600\n",
            evm_table("", key_file.path())
        ))
        .expect_err("the EVM sub-table is retired");
        assert!(matches!(
            evm,
            ConfigError::SettlementBatchSubTableRemoved { table: "evm" }
        ));
        assert!(evm
            .to_string()
            .contains("[settlement.evm.batch_settlement]"));

        let solana = resolve(&format!(
            "{}\n[solana.batch_settlement]\nmin_sponsored_deposit = 5\n",
            solana_table("", key_file.path())
        ))
        .expect_err("the Solana sub-table is retired");
        assert!(matches!(
            solana,
            ConfigError::SettlementBatchSubTableRemoved { table: "solana" }
        ));
    }

    /// Refused since #1384, and still refused once the table moved.
    #[test]
    fn the_retired_channel_index_keys_are_refused_by_name() {
        let key_file = temp_key_file();
        for field in ["channel_index_from_block", "channel_index_confirmations"] {
            let error = resolve(&evm_table(&format!("{field} = 1"), key_file.path()))
                .expect_err("the channel index keys are retired");
            assert!(
                matches!(
                    error,
                    ConfigError::SettlementChannelIndexKeyRemoved { field: named } if named == field
                ),
                "{field}: {error}"
            );
        }
    }

    /// ADR 0075 decision 9: accepting x402 channels is no longer an opt-in,
    /// so each chain's terms that have no safe default are required, and a
    /// missing one is refused by name.
    #[test]
    fn each_newly_required_key_is_refused_by_name_when_missing() {
        let key_file = temp_key_file();
        for (table, key) in [
            ("evm", "asset_eip712_name"),
            ("evm", "asset_eip712_version"),
            ("solana", "min_sponsored_deposit"),
        ] {
            let full = match table {
                "evm" => evm_table("", key_file.path()),
                _ => solana_table("", key_file.path()),
            };
            let written: String = full
                .lines()
                .filter(|line| !line.starts_with(key))
                .map(|line| format!("{line}\n"))
                .collect();
            let error = resolve(&written).expect_err("a required key is missing");
            assert!(
                matches!(
                    error,
                    ConfigError::SettlementMissingRequiredKey { table: t, key: k } if t == table && k == key
                ),
                "{table}.{key}: {error}"
            );
            let message = error.to_string();
            assert!(
                message.contains(&format!("[settlement.{table}]")) && message.contains(key),
                "the refusal names the table and the key: {message}"
            );
        }
    }

    #[test]
    fn a_below_floor_delay_fails_the_settlement_section_by_name() {
        let key_file = temp_key_file();
        let error = resolve(&solana_table("min_grace_period_secs = 60", key_file.path()))
            .expect_err("below x402's floor");
        assert!(matches!(
            error,
            ConfigError::BatchSettlementDelayBelowFloor {
                table: "solana",
                key: "min_grace_period_secs",
                value: 60,
            }
        ));
    }

    #[test]
    fn rejects_an_empty_rpc_url() {
        let key_file = temp_key_file();
        let body = evm_table("", key_file.path()).replace("http://127.0.0.1:8545", "");
        assert!(matches!(
            resolve(&body),
            Err(ConfigError::SettlementMissingRpcUrl)
        ));
    }

    #[test]
    fn rejects_a_non_http_rpc_scheme() {
        let key_file = temp_key_file();
        let body = evm_table("", key_file.path()).replace("http://", "ws://");
        assert!(matches!(
            resolve(&body),
            Err(ConfigError::SettlementUnsupportedRpcScheme { .. })
        ));
    }

    #[test]
    fn rejects_a_malformed_rpc_url() {
        let key_file = temp_key_file();
        let body = evm_table("", key_file.path()).replace("http://127.0.0.1:8545", "not a url");
        assert!(matches!(
            resolve(&body),
            Err(ConfigError::SettlementInvalidRpcUrl { .. })
        ));
    }

    #[test]
    fn rejects_an_invalid_token_address() {
        let key_file = temp_key_file();
        let body = evm_table("", key_file.path()).replace(TOKEN, "not-an-address");
        assert!(matches!(
            resolve(&body),
            Err(ConfigError::SettlementInvalidTokenAddress { .. })
        ));
    }

    #[test]
    fn rejects_an_empty_solana_mint() {
        let key_file = temp_key_file();
        let body = solana_table("", key_file.path()).replace(MINT, " ");
        assert!(matches!(
            resolve(&body),
            Err(ConfigError::SettlementMissingSolanaTokenAddress)
        ));
    }

    #[test]
    fn rejects_zero_decimals() {
        let key_file = temp_key_file();
        let body = evm_table("", key_file.path()).replace("decimals = 6", "decimals = 0");
        assert!(matches!(
            resolve(&body),
            Err(ConfigError::SettlementZeroDecimals)
        ));
    }

    #[test]
    fn rejects_a_settlement_key_naming_neither_location() {
        let key_file = temp_key_file();
        let body = evm_table("", key_file.path())
            .replace(&format!("key_file = \"{}\"", key_file.path().display()), "");
        assert!(matches!(
            resolve(&body),
            Err(ConfigError::SettlementKeyLocationAmbiguous { .. })
        ));
    }

    #[test]
    fn rejects_a_settlement_key_file_that_does_not_exist() {
        let body = evm_table("", Path::new("/nonexistent/settlement.key"));
        assert!(matches!(
            resolve(&body),
            Err(ConfigError::SettlementKeyFileNotFound(_))
        ));
    }

    #[test]
    fn an_empty_keyed_settlement_section_is_rejected() {
        let result = resolve_settlement(Some(RawSettlementSection::Keyed(Box::new(
            RawKeyedSettlementConfig {
                evm: None,
                solana: None,
            },
        ))));
        assert!(matches!(result, Err(ConfigError::SettlementSectionEmpty)));
    }

    #[test]
    fn an_unknown_key_in_a_keyed_evm_table_is_rejected_at_parse_time() {
        let key_file = temp_key_file();
        let body = evm_table("rpc__url = \"typo\"", key_file.path());
        assert!(toml::from_str::<RawSettlementSection>(&body).is_err());
    }

    #[test]
    fn a_settlement_table_names_the_token_its_channels_settle_in() {
        let key_file = temp_key_file();
        let resolved = resolve(&format!(
            "{}{}",
            evm_table("", key_file.path()),
            solana_table("", key_file.path())
        ))
        .expect("resolve");

        assert_eq!(
            resolved[0].asset(),
            AssetId::evm(to_hex(&parse_evm_address(TOKEN).unwrap()))
        );
        assert_eq!(resolved[1].asset(), AssetId::solana(MINT));
    }

    #[test]
    fn the_two_chain_enums_translate_both_ways() {
        for chain in [SettlementChain::Evm, SettlementChain::Solana] {
            assert_eq!(SettlementChain::from(chain.asset_chain()), chain);
            // The two spellings are the same word, which is what keeps an
            // `evm:` asset and an `[settlement.evm]` table comparable.
            assert_eq!(chain.asset_chain().as_str(), chain.name());
        }
    }

    #[test]
    fn cluster_hint_recognises_the_canonical_public_solana_rpc_hosts() {
        assert_eq!(
            cluster_hint_for_rpc_url("https://api.mainnet-beta.solana.com"),
            Some("mainnet-beta")
        );
        assert_eq!(
            cluster_hint_for_rpc_url("https://api.devnet.solana.com"),
            Some("devnet")
        );
        assert_eq!(
            cluster_hint_for_rpc_url("https://api.testnet.solana.com"),
            Some("testnet")
        );
        assert_eq!(
            cluster_hint_for_rpc_url("http://127.0.0.1:8899"),
            Some("localnet")
        );
        assert_eq!(
            cluster_hint_for_rpc_url("http://localhost:8899"),
            Some("localnet")
        );
    }

    /// A third-party RPC provider's URL names no cluster at all -- this must
    /// answer `None`, not guess, since a wrong guess would refuse every
    /// genuine claim a node configured against it ever receives (issue
    /// #975).
    #[test]
    fn cluster_hint_is_none_for_an_rpc_host_it_does_not_recognise() {
        assert_eq!(
            cluster_hint_for_rpc_url("https://solana-mainnet.g.alchemy.com/v2/abc123"),
            None
        );
        assert_eq!(cluster_hint_for_rpc_url("https://example.com"), None);
    }
}
