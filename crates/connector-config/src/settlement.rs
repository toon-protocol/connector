use std::collections::BTreeMap;
use std::path::PathBuf;

use connector_domain::{AssetChain, AssetId, AssetNamespace, Network, XrplNetwork};
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
/// chain, `[settlement.eip155.<chainId>]` and/or `[settlement.solana]` (issue #628).
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
///
/// EVM settlement is one table per network, `[settlement.eip155.<chainId>]`
/// (ADR 0086 decision 4): a map from the decimal chain id, as written, to
/// that network's table. The key is checked to be a chain id when the
/// section resolves, so a table keyed `base` is refused by name rather than
/// failing as an unknown field. `evm` is read only to refuse
/// `[settlement.evm]` by name.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawKeyedSettlementConfig {
    #[serde(default)]
    eip155: Option<BTreeMap<String, RawEvmSettlementTable>>,
    #[serde(default)]
    solana: Option<RawSolanaSettlementTable>,
    /// `[settlement.xrpl.<id>]` (ADR 0087): read as bare TOML so a table
    /// written without a network (`[settlement.xrpl]` holding `rpc_url`
    /// directly) is refused by name, which a typed map could only fail as
    /// a type error.
    #[serde(default)]
    xrpl: Option<toml::Table>,
    #[serde(default)]
    #[allow(dead_code)]
    evm: Option<toml::Value>,
}

/// `[settlement.xrpl.<id>]`: this node's x402 `batch-settlement` terms on
/// one XRPL network (ADR 0087 decisions 5 and 9). There is no contract,
/// program or token to name: the binding is the ledger's own `PayChannel`,
/// and a channel holds XRP and nothing else.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawXrplSettlementTable {
    rpc_url: String,
    key: RawSettlementKeyConfig,
    /// ADR 0073: dial this table's `rpc_url` through the root
    /// `socks_proxy`. See [`XrplSettlementConfig::rpc_via_socks_proxy`].
    #[serde(default)]
    rpc_via_socks_proxy: bool,
    #[serde(default)]
    min_settle_delay_secs: Option<u64>,
    #[serde(default)]
    redemption_safety_secs: Option<u64>,
    #[serde(default)]
    max_fee_drops: Option<u64>,
}

/// `[settlement.eip155.<chainId>]`: this node's x402 `batch-settlement`
/// terms on one EVM network (ADR 0075 decisions 1 and 9, ADR 0086 decision
/// 4). The contract is a constant of the binary
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

/// The `[settlement.eip155.<chainId>.key]`/`[settlement.solana.key]` sub-section: where
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
    /// The ledger's own `PayChannel` (ADR 0087), one table per network:
    /// `[settlement.xrpl.<id>]`.
    Xrpl,
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
            SettlementChain::Xrpl => "xrpl",
        }
    }

    /// The same chain as the domain names it (ADR 0071, issue #1290).
    ///
    /// The translation between the two enums lives here and not in
    /// `connector-domain`, because that crate must not depend on config to
    /// say what a token is -- its `asset` module says so in as many words.
    /// One function each way ([`TryFrom<AssetChain>`](SettlementChain) is the
    /// other), so that nothing downstream writes a second `match` over the
    /// two chains that could one day answer differently.
    pub const fn asset_chain(self) -> AssetChain {
        match self {
            SettlementChain::Evm => AssetChain::Evm,
            SettlementChain::Solana => AssetChain::Solana,
            SettlementChain::Xrpl => AssetChain::Xrpl,
        }
    }
}

/// Fallible, because one chain a token can live on has no settlement
/// backend: Mina, whose one asset is a zk-note vault's (ADR 0080).
impl TryFrom<AssetChain> for SettlementChain {
    type Error = AssetChain;

    fn try_from(chain: AssetChain) -> Result<SettlementChain, AssetChain> {
        match chain {
            AssetChain::Evm => Ok(SettlementChain::Evm),
            AssetChain::Solana => Ok(SettlementChain::Solana),
            AssetChain::Mina => Err(AssetChain::Mina),
            AssetChain::Xrpl => Ok(SettlementChain::Xrpl),
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
            "xrpl" => Ok(SettlementChain::Xrpl),
            other => Err(UnknownSettlementChain(other.to_string())),
        }
    }
}

/// The settlement table heading a refusal about `chain` tells an operator to
/// write: `[settlement.eip155.<chainId>]` for EVM and `[settlement.xrpl.<id>]`
/// for XRPL, which have one table per network (ADR 0086, ADR 0087), and
/// `[settlement.solana]`. Takes the chain's name in
/// any of the workspace's spellings of it (`SettlementChain`, `AssetChain`,
/// or the bare `"evm"`/`"solana"`), since the refusals carry all three.
pub(crate) fn settlement_heading(chain: impl std::fmt::Display) -> &'static str {
    match chain.to_string().parse::<SettlementChain>() {
        Ok(SettlementChain::Evm) => "[settlement.eip155.<chainId>]",
        Ok(SettlementChain::Solana) => "[settlement.solana]",
        Ok(SettlementChain::Xrpl) => "[settlement.xrpl.<id>]",
        Err(_) => "[settlement.<chain>]",
    }
}

/// The settlement table a value on `network` needs: `[settlement.eip155.<id>]`
/// for an EVM network, `[settlement.solana]` for the one Solana cluster.
pub(crate) fn network_heading(network: &Network) -> String {
    match network {
        Network::Eip155(chain_id) => format!("[settlement.{}]", eip155_table_name(*chain_id)),
        Network::Solana(_) => "[settlement.solana]".to_string(),
        // Mina has no settlement table: its one asset is the zk-note vault's.
        Network::Mina(_) => "[zk_note]".to_string(),
        Network::Xrpl(xrpl) => format!("[settlement.xrpl.{}]", xrpl.network_id()),
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
            "unknown settlement chain '{}' -- supported chains: evm, solana, xrpl",
            self.0
        )
    }
}

/// A fully validated `[settlement.eip155.<chainId>]` table: which network it
/// is for, which ERC-20 this node settles in there, its RPC endpoint, where
/// its signing key material lives, and its x402 `batch-settlement` terms.
/// Constructed only by [`resolve_settlement`], so a value that exists has
/// already had every field checked -- downstream code never re-validates any
/// of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmSettlementConfig {
    network: Network,
    rpc_url: String,
    token_address: [u8; 20],
    decimals: u8,
    key: SecretLocation,
    rpc_via_socks_proxy: bool,
    batch_settlement: EvmBatchSettlementConfig,
}

impl EvmSettlementConfig {
    /// The network this table is for, `eip155:<chainId>`, from its table
    /// key. **Declared and asserted**, like [`Self::decimals`] (ADR 0086
    /// decision 4): never an input to the backend, which takes the chain id
    /// from `eth_chainId`, and always checked against it when the backend
    /// connects.
    pub fn network(&self) -> &Network {
        &self.network
    }

    /// The EIP-155 chain id the table key declares.
    pub fn chain_id(&self) -> u64 {
        self.network
            .eip155_chain_id()
            .expect("an EVM table's network is always eip155")
    }

    /// The table's name as an operator wrote it, `eip155.<chainId>` -- what
    /// follows `settlement.` in its header, and what a refusal names.
    pub fn table_name(&self) -> String {
        eip155_table_name(self.chain_id())
    }

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
    /// `[settlement.eip155.<chainId>]` table carries them.
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

/// The default `max_fee_drops`: 0.1 XRP, ten thousand times the reference fee.
/// A fee the open ledger escalates past it is refused, not paid.
pub const DEFAULT_XRPL_MAX_FEE_DROPS: u64 = 100_000;

/// The default `min_settle_delay_secs`: one day, as the other chains' (ADR
/// 0074 decision 5).
pub const DEFAULT_XRPL_MIN_SETTLE_DELAY_SECS: u64 = crate::DEFAULT_BATCH_SETTLEMENT_MIN_DELAY_SECS;

/// The default `redemption_safety_secs`: how long before a payer's
/// `Expiration` this node treats a held voucher as due (x402 PR #2952's
/// example figure).
pub const DEFAULT_XRPL_REDEMPTION_SAFETY_SECS: u64 = 60;

/// The settlement port's floor under a published minimum delay: a channel
/// one second short of the minimum must still be one the ledger lets exist
/// (`connector_settlement::batch::contract::BatchContractFixture`). The
/// minimum has to exceed it.
pub const XRPL_MIN_SETTLE_DELAY_FLOOR_SECS: u64 = 901;

/// A fully validated `[settlement.xrpl.<id>]` table (ADR 0087): which XRPL
/// network it is for, its RPC endpoint, where its signing key lives and its
/// x402 `batch-settlement` terms. A channel holds XRP, so there is no token,
/// decimals or contract to name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrplSettlementConfig {
    network: XrplNetwork,
    rpc_url: String,
    key: SecretLocation,
    rpc_via_socks_proxy: bool,
    min_settle_delay_secs: u64,
    redemption_safety_secs: u64,
    max_fee_drops: u64,
}

impl XrplSettlementConfig {
    /// The network this table is for, from its table key. **Declared and
    /// asserted**: boot checks it against the endpoint's
    /// `server_info.network_id` and refuses a mismatch by name (ADR 0087
    /// decision 5).
    pub fn network(&self) -> XrplNetwork {
        self.network
    }

    /// The table's name as an operator wrote it, `xrpl.<id>`.
    pub fn table_name(&self) -> String {
        xrpl_table_name(self.network)
    }

    /// The RPC endpoint this node reaches the ledger through.
    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// Where this node's XRPL signing key material lives: a family seed
    /// file, secp256k1 or Ed25519. It pays fees, signs transactions and is
    /// the key a channel toward this node names as its `Destination`.
    pub fn key(&self) -> &SecretLocation {
        &self.key
    }

    /// Whether this table's `rpc_url` is dialed through the root
    /// `socks_proxy`, on this network's own circuit (ADR 0073, ADR 0087).
    pub fn rpc_via_socks_proxy(&self) -> bool {
        self.rpc_via_socks_proxy
    }

    /// The shortest `SettleDelay` a channel may carry and still be admitted,
    /// in seconds: above the port's 901 s floor, a day unless the table says
    /// otherwise. Published as `extra.minSettleDelay`.
    pub fn min_settle_delay_secs(&self) -> u64 {
        self.min_settle_delay_secs
    }

    /// How long before a channel's `Expiration` this node treats a held
    /// voucher as due, in seconds. Published as
    /// `extra.redemptionSafetySeconds`; below `min_settle_delay_secs`.
    pub fn redemption_safety_secs(&self) -> u64 {
        self.redemption_safety_secs
    }

    /// The most this node will pay in fee for one transaction, in drops.
    pub fn max_fee_drops(&self) -> u64 {
        self.max_fee_drops
    }

    /// XRP on this table's network, `xrpl:<id>/slip44:144`: the one asset a
    /// channel here can hold, and what a `[[tokens]]` row declares for it.
    pub fn asset(&self) -> AssetId {
        AssetId::xrp(self.network)
    }
}

/// `xrpl.<id>`: an XRPL table's name after `settlement.`.
pub(crate) fn xrpl_table_name(network: XrplNetwork) -> String {
    format!("xrpl.{}", network.network_id())
}

/// One fully validated per-chain settlement table -- typed by chain (issue
/// #628), since an EVM table and a Solana table name genuinely different
/// on-chain facts and a single shared shape would either force one to fake
/// fields it does not have or erase which chain a value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlementConfig {
    Evm(EvmSettlementConfig),
    Solana(SolanaSettlementConfig),
    Xrpl(XrplSettlementConfig),
}

impl SettlementConfig {
    /// The chain this table settles on.
    pub fn chain(&self) -> SettlementChain {
        match self {
            SettlementConfig::Evm(_) => SettlementChain::Evm,
            SettlementConfig::Solana(_) => SettlementChain::Solana,
            SettlementConfig::Xrpl(_) => SettlementChain::Xrpl,
        }
    }

    /// Whether `asset` is the token every channel on this table settles in
    /// (ADR 0071, issue #1290; ADR 0086, issue #1598): this table's own
    /// `token_address`, on this table's own network.
    ///
    /// A question rather than a value, because a Solana table cannot name its
    /// own asset at load: CAIP-19 puts the cluster's genesis prefix in it,
    /// which only the chain knows. A `solana:` asset here is held to the one
    /// Solana table by its mint, and its genesis prefix is asserted against
    /// the cluster at boot -- declared-and-asserted, like an EVM table's key.
    ///
    /// Derived, never declared twice -- which is the point. A node that deals
    /// has to be able to ask whether the token a channel holds is one it
    /// declared, and the answer has to come from the table that already
    /// states it rather than from a second place an operator could write a
    /// different address.
    pub fn holds(&self, asset: &AssetId) -> bool {
        match self {
            SettlementConfig::Evm(evm) => {
                *asset == AssetId::erc20(evm.chain_id(), to_hex(&evm.token_address))
            }
            SettlementConfig::Solana(solana) => {
                asset.chain() == AssetChain::Solana
                    && asset.namespace() == AssetNamespace::Token
                    && asset.token() == solana.token_address
            }
            // A channel holds XRP and nothing else (ADR 0087 decision 6).
            SettlementConfig::Xrpl(xrpl) => *asset == xrpl.asset(),
        }
    }

    /// How a `[[tokens]]` row spells this table's token, for a refusal that
    /// has to tell the operator what to declare. Exact for an EVM table; a
    /// Solana table's genesis prefix is left as `<genesis prefix>`, since
    /// config cannot know it.
    pub fn asset_spelling(&self) -> String {
        match self {
            SettlementConfig::Evm(evm) => {
                AssetId::erc20(evm.chain_id(), to_hex(&evm.token_address)).to_string()
            }
            SettlementConfig::Solana(solana) => {
                format!("solana:<genesis prefix>/token:{}", solana.token_address)
            }
            SettlementConfig::Xrpl(xrpl) => xrpl.asset().to_string(),
        }
    }

    /// The network this table is for, when config can say: an EVM table's
    /// `eip155:<chainId>` or an XRPL table's `xrpl:<id>` from its key. `None`
    /// for Solana, whose CAIP-2 reference is its cluster's genesis hash,
    /// which only the chain knows.
    pub fn network(&self) -> Option<Network> {
        match self {
            SettlementConfig::Evm(evm) => Some(evm.network().clone()),
            SettlementConfig::Solana(_) => None,
            SettlementConfig::Xrpl(xrpl) => Some(Network::Xrpl(xrpl.network())),
        }
    }

    /// The table's name as written after `settlement.` in its header:
    /// `eip155.<chainId>` or `solana`. What a refusal about the table names.
    pub fn table_name(&self) -> String {
        match self {
            SettlementConfig::Evm(evm) => evm.table_name(),
            SettlementConfig::Solana(_) => SettlementChain::Solana.name().to_string(),
            SettlementConfig::Xrpl(xrpl) => xrpl.table_name(),
        }
    }

    /// This table's RPC endpoint.
    pub fn rpc_url(&self) -> &str {
        match self {
            SettlementConfig::Evm(evm) => evm.rpc_url(),
            SettlementConfig::Solana(solana) => solana.rpc_url(),
            SettlementConfig::Xrpl(xrpl) => xrpl.rpc_url(),
        }
    }

    /// Whether this table's RPC rides the root `socks_proxy` (ADR 0073).
    pub fn rpc_via_socks_proxy(&self) -> bool {
        match self {
            SettlementConfig::Evm(evm) => evm.rpc_via_socks_proxy(),
            SettlementConfig::Solana(solana) => solana.rpc_via_socks_proxy(),
            SettlementConfig::Xrpl(xrpl) => xrpl.rpc_via_socks_proxy(),
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
        let table = settlement.table_name();
        let url = Url::parse(settlement.rpc_url())
            .expect("resolve_rpc_url already parsed every settlement rpc_url");
        check_circuit_route(&url, socks_proxy).map_err(|refusal| match refusal {
            CircuitRouteRefusal::WithoutProxy => {
                ConfigError::SettlementRpcViaSocksProxyWithoutProxy { table }
            }
            CircuitRouteRefusal::Plaintext => ConfigError::SettlementRpcViaSocksProxyPlaintext {
                table,
                value: settlement.rpc_url().to_string(),
            },
        })?;
    }
    Ok(())
}

/// Which of ADR 0073's two rules a read opted onto the circuit breaks.
/// Each table that may opt in -- `[settlement.<chain>]` and `[zk_note]` --
/// names it in its own error, because what an exit relay could rewrite is
/// that table's to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CircuitRouteRefusal {
    /// The node configures no `socks_proxy` to ride.
    WithoutProxy,
    /// Plain `http://` to a host that is not an onion address.
    Plaintext,
}

/// ADR 0073's rules for one endpoint whose table set
/// `rpc_via_socks_proxy = true`: the node has a `socks_proxy`, and the URL
/// is https unless its host is an onion address, which authenticates the
/// service itself (ADR 0070 decision 2).
pub(crate) fn check_circuit_route(
    url: &Url,
    socks_proxy: Option<&Url>,
) -> Result<(), CircuitRouteRefusal> {
    if socks_proxy.is_none() {
        return Err(CircuitRouteRefusal::WithoutProxy);
    }
    if url.scheme() == "http" && !crate::is_onion_endpoint(url) {
        return Err(CircuitRouteRefusal::Plaintext);
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
/// terms are (ADR 0075 decision 9): `[settlement.eip155.<chainId>.key]` is this node's
/// EVM address and voucher signer, and `[settlement.solana.key]` its
/// Solana one. The connector holds a signer rather than a wallet (ADR
/// 0012), so a node with no table for a chain has no address on it at all,
/// and can neither admit a voucher nor sign one there.
#[derive(Debug, Clone)]
pub(crate) struct SettlementTables {
    /// Each EVM table's chain id, in table order (ascending).
    eip155: Vec<u64>,
    solana: bool,
    /// Each XRPL table's network, in table order (ascending).
    xrpl: Vec<XrplNetwork>,
}

impl SettlementTables {
    /// Read the tables off an already-resolved settlement list. Called
    /// once in `Config::load`, immediately after `resolve_settlement`, so
    /// every channel table is resolved against the same answer.
    pub(crate) fn of(settlements: &[SettlementConfig]) -> Self {
        SettlementTables {
            eip155: settlements
                .iter()
                .filter_map(|settlement| match settlement {
                    SettlementConfig::Evm(evm) => Some(evm.chain_id()),
                    SettlementConfig::Solana(_) | SettlementConfig::Xrpl(_) => None,
                })
                .collect(),
            solana: settlements
                .iter()
                .any(|settlement| settlement.chain() == SettlementChain::Solana),
            xrpl: settlements
                .iter()
                .filter_map(|settlement| match settlement {
                    SettlementConfig::Xrpl(xrpl) => Some(xrpl.network()),
                    _ => None,
                })
                .collect(),
        }
    }

    /// The same answer, stated directly, for a channel-table unit test
    /// that is about the channel row rather than about how a settlement
    /// table parses. `evm` is one table on chain 8453. `Config::load`
    /// always uses [`Self::of`].
    #[cfg(test)]
    pub(crate) fn for_tests(evm: bool, solana: bool) -> Self {
        SettlementTables {
            eip155: if evm { vec![8453] } else { Vec::new() },
            solana,
            xrpl: Vec::new(),
        }
    }

    /// [`Self::for_tests`] with XRPL tables on `xrpl` as well.
    #[cfg(test)]
    pub(crate) fn for_tests_with_xrpl(evm: bool, solana: bool, xrpl: &[XrplNetwork]) -> Self {
        SettlementTables {
            xrpl: xrpl.to_vec(),
            ..Self::for_tests(evm, solana)
        }
    }

    /// Whether this node pays and is paid on x402 channels on `chain`: it
    /// has a `[settlement.<chain>]` table, which since ADR 0075 always
    /// carries the chain's x402 terms. An x402 channel row on a chain
    /// without one names a channel this node can neither admit a voucher on
    /// nor sign one on (issue #1380).
    pub(crate) fn x402(&self, chain: SettlementChain) -> bool {
        match chain {
            SettlementChain::Evm => !self.eip155.is_empty(),
            SettlementChain::Solana => self.solana,
            SettlementChain::Xrpl => !self.xrpl.is_empty(),
        }
    }

    /// The network a channel row on `chain` is on (ADR 0086 decision 8),
    /// from what the row wrote: a written network must be on `chain` and
    /// name a table this node has; an unwritten one on EVM is inferred
    /// only when there is exactly one EVM table, and refused by name when
    /// there are several. A Solana row's network is as written -- config
    /// cannot know the cluster's genesis prefix -- and `None` unwritten.
    pub(crate) fn row_network(
        &self,
        table: &'static str,
        peer_id: &str,
        chain: SettlementChain,
        written: Option<&str>,
    ) -> Result<Option<Network>, ConfigError> {
        let Some(written) = written else {
            return match (chain, self.eip155.as_slice(), self.xrpl.as_slice()) {
                (SettlementChain::Solana, ..)
                | (SettlementChain::Evm, [], _)
                | (SettlementChain::Xrpl, _, []) => Ok(None),
                (SettlementChain::Evm, [only], _) => Ok(Some(Network::Eip155(*only))),
                (SettlementChain::Xrpl, _, [only]) => Ok(Some(Network::Xrpl(*only))),
                (SettlementChain::Evm, many, _) => Err(ConfigError::ChannelRowNetworkAmbiguous {
                    table,
                    peer_id: peer_id.to_string(),
                    networks: many
                        .iter()
                        .map(|id| format!("eip155:{id}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                }),
                (SettlementChain::Xrpl, _, many) => Err(ConfigError::ChannelRowNetworkAmbiguous {
                    table,
                    peer_id: peer_id.to_string(),
                    networks: many
                        .iter()
                        .map(|network| network.caip2())
                        .collect::<Vec<_>>()
                        .join(", "),
                }),
            };
        };
        let invalid = |reason: String| ConfigError::ChannelRowNetworkInvalid {
            table,
            peer_id: peer_id.to_string(),
            value: written.to_string(),
            reason,
        };
        let network = written
            .parse::<Network>()
            .map_err(|error| invalid(format!("is not a CAIP-2 network: {error}")))?;
        if network.chain() != chain.asset_chain() {
            return Err(invalid(format!(
                "is not on {}, the chain the row's channel or signer is spelled for",
                chain.name()
            )));
        }
        match &network {
            Network::Eip155(id) if !self.eip155.contains(id) => {
                Err(ConfigError::ChannelRowNetworkUnsettled {
                    table,
                    peer_id: peer_id.to_string(),
                    network: network.to_string(),
                })
            }
            Network::Xrpl(id) if !self.xrpl.contains(id) => {
                Err(ConfigError::ChannelRowNetworkUnsettled {
                    table,
                    peer_id: peer_id.to_string(),
                    network: network.to_string(),
                })
            }
            _ => Ok(Some(network)),
        }
    }
}

/// Which networks this node has a settlement table for, as an asset's
/// network asks it (ADR 0086 decision 2, issue #1598): each EVM table's
/// `eip155:<chainId>`, and the one Solana table.
///
/// The Solana table answers for any `solana:` network, because config cannot
/// know its cluster's genesis prefix; boot asserts the prefix against the
/// cluster, exactly as it asserts an EVM table's key against `eth_chainId`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SettledNetworks {
    eip155: Vec<u64>,
    solana: bool,
    xrpl: Vec<XrplNetwork>,
}

impl SettledNetworks {
    /// Read off an already-resolved settlement list.
    pub(crate) fn of(settlements: &[SettlementConfig]) -> Self {
        SettledNetworks {
            eip155: settlements
                .iter()
                .filter_map(|settlement| match settlement {
                    SettlementConfig::Evm(evm) => Some(evm.chain_id()),
                    SettlementConfig::Solana(_) | SettlementConfig::Xrpl(_) => None,
                })
                .collect(),
            solana: settlements
                .iter()
                .any(|settlement| settlement.chain() == SettlementChain::Solana),
            xrpl: settlements
                .iter()
                .filter_map(|settlement| match settlement {
                    SettlementConfig::Xrpl(xrpl) => Some(xrpl.network()),
                    _ => None,
                })
                .collect(),
        }
    }

    /// The same answer, stated directly, for a unit test about the rows that
    /// ask it rather than about how a settlement table parses.
    #[cfg(test)]
    pub(crate) fn for_tests(eip155: &[u64], solana: bool) -> Self {
        SettledNetworks {
            eip155: eip155.to_vec(),
            solana,
            xrpl: Vec::new(),
        }
    }

    /// Whether this node settles on `network`.
    pub(crate) fn settles(&self, network: &Network) -> bool {
        match network {
            Network::Eip155(chain_id) => self.eip155.contains(chain_id),
            Network::Solana(_) => self.solana,
            // No settlement backend holds MINA (ADR 0080 decision 7).
            Network::Mina(_) => false,
            Network::Xrpl(network) => self.xrpl.contains(network),
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
fn required<T>(table: &str, key: &'static str, value: Option<T>) -> Result<T, ConfigError> {
    value.ok_or_else(|| ConfigError::SettlementMissingRequiredKey {
        table: table.to_string(),
        key,
    })
}

/// `eip155.<chainId>`: an EVM table's name after `settlement.`.
pub(crate) fn eip155_table_name(chain_id: u64) -> String {
    format!("eip155.{chain_id}")
}

fn resolve_evm_fields(
    chain_id: u64,
    table: RawEvmSettlementTable,
) -> Result<EvmSettlementConfig, ConfigError> {
    let name = eip155_table_name(chain_id);
    refuse_removed(vec![
        (
            table.contract_address.is_some(),
            ConfigError::SettlementToonKeyRemoved {
                table: name.clone(),
                field: "contract_address",
            },
        ),
        (
            table.batch_settlement.is_some(),
            ConfigError::SettlementBatchSubTableRemoved {
                table: name.clone(),
            },
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
    let batch_settlement = resolve_evm_batch_settlement(
        &name,
        RawEvmBatchSettlementTable {
            min_withdraw_delay_secs: table.min_withdraw_delay_secs,
            asset_eip712_name: required(&name, "asset_eip712_name", table.asset_eip712_name)?,
            asset_eip712_version: required(
                &name,
                "asset_eip712_version",
                table.asset_eip712_version,
            )?,
            asset_transfer_method: table.asset_transfer_method,
            facilitator_url: table.facilitator_url,
        },
    )?;

    Ok(EvmSettlementConfig {
        network: Network::Eip155(chain_id),
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
                table: "solana".to_string(),
                field: "program_id",
            },
        ),
        (
            table.batch_settlement.is_some(),
            ConfigError::SettlementBatchSubTableRemoved {
                table: "solana".to_string(),
            },
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

/// Resolve `[settlement.xrpl]`: one table per network (ADR 0087 decision 5),
/// in network-id order. Each is refused by name when it is not a table of a
/// known network, writes its key inline, or breaks a bound on its terms; an
/// unknown key is refused by `deny_unknown_fields`, naming the key.
fn resolve_xrpl_tables(raw: toml::Table) -> Result<Vec<XrplSettlementConfig>, ConfigError> {
    if raw.is_empty() {
        return Err(ConfigError::SettlementXrplEmpty);
    }
    // `[settlement.xrpl] rpc_url = ...` and `[settlement.xrpl.key]`: a table
    // with no network, whose entries are a network table's own keys where
    // its networks would be.
    const TABLE_KEYS: [&str; 6] = [
        "rpc_url",
        "key",
        "rpc_via_socks_proxy",
        "min_settle_delay_secs",
        "redemption_safety_secs",
        "max_fee_drops",
    ];
    if let Some((key, _)) = raw
        .iter()
        .find(|(key, value)| !value.is_table() || TABLE_KEYS.contains(&key.as_str()))
    {
        return Err(ConfigError::SettlementXrplBareTable { key: key.clone() });
    }
    let mut tables = Vec::with_capacity(raw.len());
    for (key, value) in raw {
        let toml::Value::Table(table) = value else {
            unreachable!("a non-table entry was refused above");
        };
        let network = key
            .parse::<XrplNetwork>()
            .map_err(|_| ConfigError::SettlementXrplUnknownNetwork { key: key.clone() })?;
        let name = xrpl_table_name(network);
        if table
            .get("key")
            .is_some_and(|key| !matches!(key, toml::Value::Table(_)))
        {
            return Err(ConfigError::SettlementXrplInlineKey { table: name });
        }
        let parsed: RawXrplSettlementTable =
            table
                .try_into()
                .map_err(|source| ConfigError::SettlementXrplTableInvalid {
                    table: name.clone(),
                    source: Box::new(source),
                })?;
        tables.push(resolve_xrpl_fields(network, parsed)?);
    }
    tables.sort_by_key(|table| table.network().network_id());
    Ok(tables)
}

fn resolve_xrpl_fields(
    network: XrplNetwork,
    table: RawXrplSettlementTable,
) -> Result<XrplSettlementConfig, ConfigError> {
    let name = xrpl_table_name(network);
    let rpc_url = resolve_rpc_url(table.rpc_url)?;
    let key = resolve_settlement_key(table.key)?;
    let min_settle_delay_secs = table
        .min_settle_delay_secs
        .unwrap_or(DEFAULT_XRPL_MIN_SETTLE_DELAY_SECS);
    if min_settle_delay_secs <= XRPL_MIN_SETTLE_DELAY_FLOOR_SECS {
        return Err(ConfigError::SettlementXrplSettleDelayBelowFloor {
            table: name,
            value: min_settle_delay_secs,
            floor: XRPL_MIN_SETTLE_DELAY_FLOOR_SECS,
        });
    }
    let redemption_safety_secs = table
        .redemption_safety_secs
        .unwrap_or(DEFAULT_XRPL_REDEMPTION_SAFETY_SECS);
    if redemption_safety_secs == 0 || redemption_safety_secs >= min_settle_delay_secs {
        return Err(ConfigError::SettlementXrplRedemptionSafetyOutOfRange {
            table: name,
            value: redemption_safety_secs,
            min_settle_delay_secs,
        });
    }
    let max_fee_drops = table.max_fee_drops.unwrap_or(DEFAULT_XRPL_MAX_FEE_DROPS);
    if max_fee_drops == 0 {
        return Err(ConfigError::SettlementXrplZeroMaxFee { table: name });
    }
    Ok(XrplSettlementConfig {
        network,
        rpc_url,
        key,
        rpc_via_socks_proxy: table.rpc_via_socks_proxy,
        min_settle_delay_secs,
        redemption_safety_secs,
        max_fee_drops,
    })
}

/// Validate an optional `[settlement]` section. Presence configures one
/// x402 settlement backend per table; absence is a node that settles on no
/// chain, and so takes no claim at all (every claim is a voucher, ADR
/// 0075).
///
/// Returns every table the section names, each fully validated: the EVM
/// tables first, one per network in chain-id order, then the Solana table
/// if there is one (ADR 0086 decision 4). Several EVM tables load; whether
/// a node may *run* more than one is boot's question, not config's.
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
            if raw.evm.is_some() {
                return Err(ConfigError::SettlementEvmTableRemoved);
            }
            if raw.eip155.is_none() && raw.solana.is_none() && raw.xrpl.is_none() {
                return Err(ConfigError::SettlementSectionEmpty);
            }
            let mut out = Vec::new();
            if let Some(eip155) = raw.eip155 {
                if eip155.is_empty() {
                    return Err(ConfigError::SettlementEip155Empty);
                }
                let mut networks = Vec::with_capacity(eip155.len());
                for (key, table) in eip155 {
                    let chain_id = Network::parse_eip155_chain_id(&key)
                        .map_err(|_| ConfigError::SettlementEip155KeyNotAChainId { key })?;
                    networks.push((chain_id, table));
                }
                networks.sort_by_key(|(chain_id, _)| *chain_id);
                for (chain_id, table) in networks {
                    out.push(SettlementConfig::Evm(resolve_evm_fields(chain_id, table)?));
                }
            }
            if let Some(solana) = raw.solana {
                out.push(SettlementConfig::Solana(resolve_solana_fields(solana)?));
            }
            if let Some(xrpl) = raw.xrpl {
                for table in resolve_xrpl_tables(xrpl)? {
                    out.push(SettlementConfig::Xrpl(table));
                }
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

    /// A complete `[settlement.eip155.31337]` table, with `extra` appended
    /// to it before its key sub-table.
    fn evm_table(extra: &str, key_file: &Path) -> String {
        evm_table_on(31337, extra, key_file)
    }

    /// A complete `[settlement.eip155.<chain_id>]` table.
    fn evm_table_on(chain_id: u64, extra: &str, key_file: &Path) -> String {
        format!(
            r#"
[eip155.{chain_id}]
rpc_url = "http://127.0.0.1:8545"
token_address = "{TOKEN}"
decimals = 6
asset_eip712_name = "USDC"
asset_eip712_version = "2"
{extra}

[eip155.{chain_id}.key]
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

    /// ADR 0075 decision 9: `[settlement.eip155.<chainId>]` needs only an RPC URL, a
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

    /// toon-client#695: the deposit keys sit in `[settlement.eip155.<chainId>]` itself,
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
            message.contains("[settlement.eip155.<chainId>]") && message.contains("ADR 0075"),
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
                ref table,
                field: "contract_address",
            } if table == "eip155.31337"
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
                ref table,
                field: "program_id",
            } if table == "solana"
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
            "{}\n[eip155.31337.batch_settlement]\nmin_withdraw_delay_secs = 3600\n",
            evm_table("", key_file.path())
        ))
        .expect_err("the EVM sub-table is retired");
        assert!(matches!(
            evm,
            ConfigError::SettlementBatchSubTableRemoved { ref table } if table == "eip155.31337"
        ));
        assert!(evm
            .to_string()
            .contains("[settlement.eip155.31337.batch_settlement]"));

        let solana = resolve(&format!(
            "{}\n[solana.batch_settlement]\nmin_sponsored_deposit = 5\n",
            solana_table("", key_file.path())
        ))
        .expect_err("the Solana sub-table is retired");
        assert!(matches!(
            solana,
            ConfigError::SettlementBatchSubTableRemoved { ref table } if table == "solana"
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
            ("eip155.31337", "asset_eip712_name"),
            ("eip155.31337", "asset_eip712_version"),
            ("solana", "min_sponsored_deposit"),
        ] {
            let full = match table {
                "eip155.31337" => evm_table("", key_file.path()),
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
                    &error,
                    ConfigError::SettlementMissingRequiredKey { table: t, key: k } if t == table && *k == key
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
                ref table,
                key: "min_grace_period_secs",
                value: 60,
            } if table == "solana"
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

    /// A complete `[settlement.xrpl.<id>]` table, with `extra` appended to it
    /// before its key sub-table.
    fn xrpl_table_on(id: &str, extra: &str, key_file: &Path) -> String {
        format!(
            r#"
[xrpl.{id}]
rpc_url = "http://127.0.0.1:5005"
{extra}

[xrpl.{id}.key]
key_file = "{key}"
"#,
            key = key_file.display()
        )
    }

    fn single_xrpl(settlements: Vec<SettlementConfig>) -> XrplSettlementConfig {
        match settlements.as_slice() {
            [SettlementConfig::Xrpl(xrpl)] => xrpl.clone(),
            other => panic!("expected one XRPL table, got {other:?}"),
        }
    }

    /// ADR 0087 decisions 5 and 9: one table per network, keyed by the
    /// ledger's `network_id`, with the three terms defaulted.
    #[test]
    fn a_complete_xrpl_table_resolves_with_its_defaults() {
        let key_file = temp_key_file();
        let xrpl = single_xrpl(resolve(&xrpl_table_on("1000", "", key_file.path())).unwrap());

        assert_eq!(xrpl.network(), XrplNetwork::Standalone);
        assert_eq!(xrpl.table_name(), "xrpl.1000");
        assert_eq!(xrpl.rpc_url(), "http://127.0.0.1:5005");
        assert_eq!(
            xrpl.key(),
            &SecretLocation::File(key_file.path().to_path_buf())
        );
        assert!(!xrpl.rpc_via_socks_proxy());
        assert_eq!(xrpl.min_settle_delay_secs(), 86_400);
        assert_eq!(xrpl.redemption_safety_secs(), 60);
        assert_eq!(xrpl.max_fee_drops(), DEFAULT_XRPL_MAX_FEE_DROPS);
        assert_eq!(xrpl.asset().to_string(), "xrpl:1000/slip44:144");
    }

    #[test]
    fn an_xrpl_table_carries_the_terms_it_writes() {
        let key_file = temp_key_file();
        let xrpl = single_xrpl(
            resolve(&xrpl_table_on(
                "1",
                "rpc_via_socks_proxy = true\nmin_settle_delay_secs = 3600\n\
                 redemption_safety_secs = 120\nmax_fee_drops = 5000",
                key_file.path(),
            ))
            .unwrap(),
        );
        assert_eq!(xrpl.network(), XrplNetwork::Testnet);
        assert!(xrpl.rpc_via_socks_proxy());
        assert_eq!(xrpl.min_settle_delay_secs(), 3600);
        assert_eq!(xrpl.redemption_safety_secs(), 120);
        assert_eq!(xrpl.max_fee_drops(), 5000);
    }

    /// Tables load in network-id order beside the others, and each holds XRP
    /// on its own network and nothing else.
    #[test]
    fn xrpl_tables_follow_the_others_and_hold_xrp_on_their_own_network() {
        let key_file = temp_key_file();
        let resolved = resolve(&format!(
            "{}{}{}{}",
            xrpl_table_on("1000", "", key_file.path()),
            evm_table("", key_file.path()),
            xrpl_table_on("1", "", key_file.path()),
            solana_table("", key_file.path()),
        ))
        .unwrap();
        let chains: Vec<_> = resolved.iter().map(SettlementConfig::chain).collect();
        assert_eq!(
            chains,
            [
                SettlementChain::Evm,
                SettlementChain::Solana,
                SettlementChain::Xrpl,
                SettlementChain::Xrpl
            ]
        );
        let testnet = &resolved[2];
        assert_eq!(testnet.table_name(), "xrpl.1");
        assert_eq!(testnet.asset_spelling(), "xrpl:1/slip44:144");
        assert!(testnet.holds(&"xrpl:1/slip44:144".parse().unwrap()));
        assert!(!testnet.holds(&"xrpl:1000/slip44:144".parse().unwrap()));
        assert_eq!(testnet.network(), Some(Network::Xrpl(XrplNetwork::Testnet)));
    }

    /// ADR 0087 decision 5: a table with no network is refused by name, not
    /// read as some network's.
    #[test]
    fn a_bare_xrpl_table_is_refused_by_name() {
        let key_file = temp_key_file();
        let error = resolve(&format!(
            r#"
[xrpl]
rpc_url = "http://127.0.0.1:5005"

[xrpl.key]
key_file = "{}"
"#,
            key_file.path().display()
        ))
        .expect_err("a table with no network");
        assert!(matches!(
            error,
            ConfigError::SettlementXrplBareTable { ref key } if key == "key"
        ));
        assert!(
            error.to_string().contains("[settlement.xrpl.<id>]"),
            "{error}"
        );
    }

    #[test]
    fn an_xrpl_network_this_build_has_no_row_for_is_refused_by_name() {
        let key_file = temp_key_file();
        for key in ["7", "mainnet", "01"] {
            let error = resolve(&xrpl_table_on(key, "", key_file.path()))
                .expect_err("no row in the network table");
            assert!(
                matches!(
                    error,
                    ConfigError::SettlementXrplUnknownNetwork { key: ref named } if named == key
                ),
                "{key}: {error:?}"
            );
        }
    }

    #[test]
    fn an_xrpl_key_written_inline_is_refused_by_name() {
        let body = r#"
[xrpl.1000]
rpc_url = "http://127.0.0.1:5005"
key = "snoPBrXtMeMyMHUVTgbuqAfg1SUTb"
"#;
        let error = resolve(body).expect_err("a key is never a value");
        assert!(matches!(
            error,
            ConfigError::SettlementXrplInlineKey { ref table } if table == "xrpl.1000"
        ));
    }

    #[test]
    fn an_unknown_key_in_an_xrpl_table_is_refused_naming_it() {
        let key_file = temp_key_file();
        let error = resolve(&xrpl_table_on("1000", "network_id = 1000", key_file.path()))
            .expect_err("deny_unknown_fields");
        assert!(matches!(
            error,
            ConfigError::SettlementXrplTableInvalid { ref table, .. } if table == "xrpl.1000"
        ));
        assert!(error.to_string().contains("network_id"), "{error}");
    }

    #[test]
    fn an_xrpl_table_missing_its_rpc_url_or_key_is_refused() {
        let key_file = temp_key_file();
        let body = xrpl_table_on("1000", "", key_file.path());
        let without_rpc = body.replace("rpc_url = \"http://127.0.0.1:5005\"", "");
        assert!(matches!(
            resolve(&without_rpc),
            Err(ConfigError::SettlementXrplTableInvalid { .. })
        ));
        let without_key =
            body.replace(&format!("key_file = \"{}\"", key_file.path().display()), "");
        assert!(matches!(
            resolve(&without_key),
            Err(ConfigError::SettlementKeyLocationAmbiguous { .. })
        ));
    }

    #[test]
    fn an_empty_xrpl_heading_is_refused() {
        assert!(matches!(
            resolve("[xrpl]\n"),
            Err(ConfigError::SettlementXrplEmpty)
        ));
    }

    /// The minimum delay has to exceed the port's 901 s floor.
    #[test]
    fn an_xrpl_settle_delay_at_or_below_the_floor_is_refused() {
        let key_file = temp_key_file();
        for value in [0, 900, 901] {
            let error = resolve(&xrpl_table_on(
                "1000",
                &format!("min_settle_delay_secs = {value}"),
                key_file.path(),
            ))
            .expect_err("at or below the floor");
            assert!(
                matches!(
                    error,
                    ConfigError::SettlementXrplSettleDelayBelowFloor { value: v, floor: 901, .. } if v == value
                ),
                "{value}: {error:?}"
            );
        }
        let ok = single_xrpl(
            resolve(&xrpl_table_on(
                "1000",
                "min_settle_delay_secs = 902",
                key_file.path(),
            ))
            .unwrap(),
        );
        assert_eq!(ok.min_settle_delay_secs(), 902);
    }

    #[test]
    fn an_xrpl_redemption_safety_must_sit_inside_the_delay() {
        let key_file = temp_key_file();
        for extra in [
            "redemption_safety_secs = 0",
            "min_settle_delay_secs = 1000\nredemption_safety_secs = 1000",
            "min_settle_delay_secs = 1000\nredemption_safety_secs = 5000",
        ] {
            assert!(
                matches!(
                    resolve(&xrpl_table_on("1000", extra, key_file.path())),
                    Err(ConfigError::SettlementXrplRedemptionSafetyOutOfRange { .. })
                ),
                "{extra}"
            );
        }
    }

    #[test]
    fn a_zero_xrpl_fee_ceiling_is_refused() {
        let key_file = temp_key_file();
        assert!(matches!(
            resolve(&xrpl_table_on("1000", "max_fee_drops = 0", key_file.path())),
            Err(ConfigError::SettlementXrplZeroMaxFee { .. })
        ));
    }

    #[test]
    fn an_xrpl_rpc_url_is_held_to_the_same_checks_as_the_others() {
        let key_file = temp_key_file();
        let body = xrpl_table_on("1000", "", key_file.path())
            .replace("http://127.0.0.1:5005", "ws://127.0.0.1:6006");
        assert!(matches!(
            resolve(&body),
            Err(ConfigError::SettlementUnsupportedRpcScheme { .. })
        ));
    }

    /// The `evm` and the unknown-chain guards still stand beside it.
    #[test]
    fn xrpl_alone_is_a_section() {
        let key_file = temp_key_file();
        let resolved = resolve(&xrpl_table_on("0", "", key_file.path())).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].chain(), SettlementChain::Xrpl);
        assert_eq!(resolved[0].table_name(), "xrpl.0");
    }

    #[test]
    fn an_empty_keyed_settlement_section_is_rejected() {
        let result = resolve_settlement(Some(RawSettlementSection::Keyed(Box::new(
            RawKeyedSettlementConfig {
                eip155: None,
                solana: None,
                xrpl: None,
                evm: None,
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
    fn a_settlement_table_holds_the_token_its_channels_settle_in() {
        let key_file = temp_key_file();
        let resolved = resolve(&format!(
            "{}{}",
            evm_table("", key_file.path()),
            solana_table("", key_file.path())
        ))
        .expect("resolve");
        let lower = to_hex(&parse_evm_address(TOKEN).unwrap());

        let on_this_network: AssetId = format!("eip155:31337/erc20:{lower}").parse().unwrap();
        let on_another: AssetId = format!("eip155:8453/erc20:{lower}").parse().unwrap();
        assert!(resolved[0].holds(&on_this_network));
        assert!(
            !resolved[0].holds(&on_another),
            "the network is part of the asset"
        );
        assert_eq!(resolved[0].asset_spelling(), on_this_network.to_string());

        let mint: AssetId = format!("solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1/token:{MINT}")
            .parse()
            .unwrap();
        assert!(resolved[1].holds(&mint));
        assert!(!resolved[0].holds(&mint));
        assert_eq!(
            resolved[1].asset_spelling(),
            format!("solana:<genesis prefix>/token:{MINT}")
        );
    }

    #[test]
    fn the_two_chain_enums_translate_both_ways() {
        for chain in [SettlementChain::Evm, SettlementChain::Solana] {
            assert_eq!(SettlementChain::try_from(chain.asset_chain()), Ok(chain));
            // The two spellings are the same word, which is what keeps an
            // `evm:` asset and an `[settlement.evm]` table comparable.
            assert_eq!(chain.asset_chain().as_str(), chain.name());
        }
    }

    /// ADR 0086 decision 4: `[settlement.evm]` named no network, so it is
    /// refused by name with its rewrite, never read as some chain's table.
    #[test]
    fn the_evm_table_is_refused_by_name_with_its_rewrite() {
        let key_file = temp_key_file();
        let body = evm_table("", key_file.path())
            .replace("[eip155.31337]", "[evm]")
            .replace("[eip155.31337.key]", "[evm.key]");
        let error = resolve(&body).expect_err("[settlement.evm] is retired");

        assert!(matches!(error, ConfigError::SettlementEvmTableRemoved));
        let message = error.to_string();
        assert!(
            message.contains("[settlement.evm]")
                && message.contains("[settlement.eip155.<chainId>]")
                && message.contains("ADR 0086"),
            "the refusal names the table and its rewrite: {message}"
        );
    }

    /// The table key is a decimal chain id, or the table is refused by
    /// name: a typo must not become a silently unused table.
    #[test]
    fn a_table_key_that_is_not_a_decimal_chain_id_is_refused_by_name() {
        let key_file = temp_key_file();
        for key in ["base", "0x2105", "08453", "0"] {
            let body = evm_table("", key_file.path())
                .replace("[eip155.31337]", &format!("[eip155.\"{key}\"]"))
                .replace("[eip155.31337.key]", &format!("[eip155.\"{key}\".key]"));
            let error = resolve(&body).expect_err("not a chain id");
            assert!(
                matches!(&error, ConfigError::SettlementEip155KeyNotAChainId { key: named } if named == key),
                "{key}: {error}"
            );
            let message = error.to_string();
            assert!(
                message.contains(&format!("[settlement.eip155.{key}]")),
                "the refusal names the table: {message}"
            );
        }
    }

    /// `[settlement.eip155]` with no network under it is not EVM
    /// settlement, and is refused rather than read as an empty one.
    #[test]
    fn an_eip155_section_with_no_network_is_refused_by_name() {
        let error = resolve("[eip155]\n").expect_err("no network");
        assert!(matches!(error, ConfigError::SettlementEip155Empty));
        assert!(error.to_string().contains("[settlement.eip155]"));
    }

    /// Each table answers for its own network, in chain-id order, and two
    /// tables may name one key file (ADR 0086 decision 5). Loading two is
    /// legal here; boot refuses it until multi-network settlement lands.
    #[test]
    fn each_eip155_table_answers_for_its_own_network() {
        let key_file = temp_key_file();
        let resolved = resolve(&format!(
            "{}{}{}",
            evm_table_on(84532, "", key_file.path()),
            solana_table("", key_file.path()),
            evm_table_on(8453, "", key_file.path()),
        ))
        .expect("two EVM tables load");

        let networks: Vec<Option<Network>> =
            resolved.iter().map(SettlementConfig::network).collect();
        assert_eq!(
            networks,
            vec![
                Some(Network::Eip155(8453)),
                Some(Network::Eip155(84532)),
                None,
            ]
        );
        let SettlementConfig::Evm(base) = &resolved[0] else {
            panic!("EVM first");
        };
        assert_eq!(base.network(), &Network::Eip155(8453));
        assert_eq!(base.chain_id(), 8453);
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
