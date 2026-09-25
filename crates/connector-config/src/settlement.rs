use std::path::PathBuf;

use connector_domain::{AssetChain, AssetId};
use serde::Deserialize;
use url::Url;

use crate::batch_settlement::{
    resolve_evm_batch_settlement, resolve_solana_batch_settlement, EvmBatchSettlementConfig,
    RawEvmBatchSettlementTable, RawSolanaBatchSettlementTable, SolanaBatchSettlementConfig,
};
use crate::client_channel::to_hex;
use crate::error::ConfigError;
use crate::secret::SecretLocation;

/// The `[settlement]` section as written in the config file, in either shape
/// this connector accepts (issue #628).
///
/// **Legacy** ([`RawSettlementConfig`]): the single flat form every shipped
/// example and infra config already carries -- `chain`, `rpc_url`,
/// `contract_address`, `token_address`, `decimals`, `[settlement.key]` -- and
/// keeps parsing with unchanged semantics. Frozen at one chain (`"evm"`) by
/// design: a config that wants a second chain, or wants Solana at all, uses
/// the keyed form below instead of teaching this one a new `chain` value.
///
/// **Keyed** ([`RawKeyedSettlementConfig`]): `[settlement.evm]` and/or
/// `[settlement.solana]`, one table per chain -- the shape chosen at
/// decomposition (epic #627) for a node settling on more than one chain at
/// once. A keyed table by construction, so `deny_unknown_fields` guards each
/// chain's own fields without an ordering ambiguity a `[[settlement]]` array
/// would have had.
///
/// `#[serde(untagged)]` picks whichever shape matches: the legacy shape
/// requires `chain` and forbids `evm`/`solana` keys, the keyed shape forbids
/// `chain` and only recognizes `evm`/`solana` -- mutually exclusive by
/// construction, so a config can never be read two ways.
///
/// The keyed shape is boxed only because it is much the larger of the two,
/// now that each chain's table can carry a `batch_settlement` sub-table.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum RawSettlementSection {
    Legacy(RawSettlementConfig),
    Keyed(Box<RawKeyedSettlementConfig>),
}

/// The legacy flat `[settlement]` shape (issue #542): one chain, named by
/// `chain`, with its fields directly under the section. `deny_unknown_fields`
/// so a mistyped key (`rpc__url`, `contractaddress`, ...) fails config load
/// loudly instead of being parsed, silently dropped, and honoured as if it
/// had never been written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSettlementConfig {
    chain: String,
    rpc_url: String,
    contract_address: String,
    token_address: String,
    decimals: u8,
    key: RawSettlementKeyConfig,
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
    /// `connector-cli` constructs a real `SolanaSettlementBackend` for this
    /// table at startup (issue #630), with the same fail-closed identity
    /// checks (RPC reachable, program executable, mint decimals agreeing
    /// with `decimals`) `[settlement.evm]` gets (ADR 0009 stays
    /// fail-closed throughout -- see epic #627).
    #[serde(default)]
    solana: Option<RawSolanaSettlementTable>,
}

/// `[settlement.evm]`: the same fields the legacy flat shape carries, minus
/// `chain` -- the table's own key already says which chain this is.
///
/// `channel_index_from_block`/`channel_index_confirmations` (issue #661) are
/// new, additive knobs for the local `ChannelOpened`/`ChannelNewDeposit`/
/// `ChannelSettled` index built from this same `TokenNetwork`: the block to
/// backfill from on a cold start with no checkpoint, and the depth behind
/// chain head logs are applied at. Both default when omitted -- see
/// [`resolve_evm_fields`] -- so an existing `[settlement.evm]` table keeps
/// parsing with unchanged behaviour.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawEvmSettlementTable {
    rpc_url: String,
    contract_address: String,
    token_address: String,
    decimals: u8,
    key: RawSettlementKeyConfig,
    #[serde(default)]
    channel_index_from_block: Option<u64>,
    #[serde(default)]
    channel_index_confirmations: Option<u64>,
    /// ADR 0073: dial this table's `rpc_url` through the root
    /// `socks_proxy`. See [`EvmSettlementConfig::rpc_via_socks_proxy`].
    #[serde(default)]
    rpc_via_socks_proxy: bool,
    /// ADR 0074: `[settlement.evm.batch_settlement]`, the opt-in to x402
    /// batch-settlement channels on EVM. Absent is off.
    #[serde(default)]
    batch_settlement: Option<RawEvmBatchSettlementTable>,
}

/// `[settlement.solana]`: `contract_address` (an EVM `TokenNetworkRegistry`)
/// has no Solana equivalent, so this table names a `program_id` instead --
/// the deployed `payment-channel` program (`packages/solana-program`,
/// `2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip` on `solana:devnet`) the
/// `SolanaSettlementBackend` `connector-cli` constructs at startup drives
/// (issue #630).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSolanaSettlementTable {
    rpc_url: String,
    program_id: String,
    token_address: String,
    decimals: u8,
    key: RawSettlementKeyConfig,
    /// ADR 0073: dial this table's `rpc_url` through the root
    /// `socks_proxy`. See [`SolanaSettlementConfig::rpc_via_socks_proxy`].
    #[serde(default)]
    rpc_via_socks_proxy: bool,
    /// ADR 0074: `[settlement.solana.batch_settlement]`, the opt-in to x402
    /// batch-settlement channels on Solana. Absent is off.
    #[serde(default)]
    batch_settlement: Option<RawSolanaBatchSettlementTable>,
}

/// The `[settlement]`/`[settlement.evm]`/`[settlement.solana]` `key`
/// sub-section: where the key material this backend signs settlement
/// transactions with lives. Same File-or-KMS shape as the top-level
/// `[signer]` section (`crate::secret`), kept as its own type rather than
/// reused directly because these are independent config-file positions with
/// their own `deny_unknown_fields` boundary.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSettlementKeyConfig {
    #[serde(default)]
    key_file: Option<PathBuf>,
    #[serde(default)]
    kms_key_id: Option<String>,
}

/// The chains a [`SettlementConfig`] can name. `connector-cli` constructs a
/// real backend for both (issue #630 finished what #628 started), so both
/// are recognized chains here rather than [`ConfigError::SettlementUnknownChain`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettlementChain {
    Evm,
    Solana,
}

impl SettlementChain {
    /// The chain's config-file name -- the keyed `[settlement.<name>]`
    /// table key and the legacy flat table's `chain` value. The one
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

/// A chain name [`SettlementChain::from_str`] does not recognize. Unlike
/// the legacy flat table's [`ConfigError::SettlementUnknownChain`] (frozen
/// at `"evm"` by design, issue #628), this names every chain the keyed
/// config shape -- and therefore the rest of the fleet -- recognizes.
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

impl std::error::Error for UnknownSettlementChain {}

/// A fully validated `[settlement.evm]` (or legacy `[settlement]`) table:
/// which already-deployed `TokenNetworkRegistry` and ERC-20 asset this
/// backend settles through, where its RPC endpoint is, and where its signing
/// key material lives. Constructed only by [`resolve_settlement`], so a value
/// that exists has already had every field checked -- downstream code never
/// re-validates any of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmSettlementConfig {
    rpc_url: String,
    contract_address: [u8; 20],
    token_address: [u8; 20],
    decimals: u8,
    key: SecretLocation,
    channel_index_from_block: u64,
    channel_index_confirmations: u64,
    rpc_via_socks_proxy: bool,
    batch_settlement: Option<EvmBatchSettlementConfig>,
}

/// How many blocks behind chain head a `ChannelOpened`/`ChannelNewDeposit`/
/// `ChannelSettled` log must be before the local channel index applies it
/// (issue #661), when `channel_index_confirmations` is not set. Deep enough
/// that an ordinary chain reorg cannot un-confirm a log this index has
/// already applied -- there is deliberately no unwind path, so this default
/// has to actually hold rather than merely look safe on a chain that has not
/// reorged yet.
pub const DEFAULT_CHANNEL_INDEX_CONFIRMATIONS: u64 = 5;

impl EvmSettlementConfig {
    /// The RPC endpoint this backend connects through.
    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// The already-deployed `TokenNetworkRegistry` this backend resolves its
    /// actual `TokenNetwork` through, keyed by
    /// [`token_address`](Self::token_address) (issue #576) -- not a channel
    /// contract itself.
    pub fn contract_address(&self) -> [u8; 20] {
        self.contract_address
    }

    /// The ERC-20 asset every channel this backend opens settles in, and the
    /// input `TokenNetworkRegistry.getTokenNetwork` resolves
    /// [`contract_address`](Self::contract_address) against to find the
    /// actual `TokenNetwork` (issue #576).
    pub fn token_address(&self) -> [u8; 20] {
        self.token_address
    }

    /// The settlement asset's decimal precision (6 for the USDC this
    /// connector settles). See [`SettlementConfig`]'s module docs for why
    /// nothing scales by this value -- it is honoured as a startup *check*
    /// against the deployed token's own `decimals()` instead (issue #564).
    pub fn decimals(&self) -> u8 {
        self.decimals
    }

    /// Where this backend's signing key material lives.
    pub fn key(&self) -> &SecretLocation {
        &self.key
    }

    /// The block the local channel index (issue #661) backfills from on a
    /// cold start with no durable checkpoint. `0` (scan from genesis) unless
    /// `channel_index_from_block` is set -- an operator who knows their
    /// `TokenNetwork`'s deploy block should set it, since scanning a public
    /// chain from genesis is the cold-start cost this field exists to avoid.
    pub fn channel_index_from_block(&self) -> u64 {
        self.channel_index_from_block
    }

    /// How many blocks behind chain head a channel-index log must be before
    /// it is applied (issue #661) -- always at least 1, enforced at load
    /// time by [`resolve_evm_fields`], since indexing at head has nothing to
    /// fall back on when the head reorgs and this index ships no unwind
    /// path.
    pub fn channel_index_confirmations(&self) -> u64 {
        self.channel_index_confirmations
    }

    /// Whether every client of this table's `rpc_url` (the backend, the
    /// channel-index syncer and the rate source) dials through the root
    /// `socks_proxy`, on this chain's own pinned circuit (ADR 0073). `false`
    /// unless the table says so. When `true`, `Config::load` has already
    /// checked that a `socks_proxy` exists, and that the endpoint is
    /// `https` unless its host is an onion address, since an exit relay
    /// could otherwise read and rewrite every answer.
    pub fn rpc_via_socks_proxy(&self) -> bool {
        self.rpc_via_socks_proxy
    }

    /// This node's terms for x402 batch-settlement channels on EVM, or
    /// `None` when it has not opted in (ADR 0074 decision 1). A node that
    /// has not offers no `batch-settlement` entry on EVM and refuses an EVM
    /// voucher by name.
    pub fn batch_settlement(&self) -> Option<&EvmBatchSettlementConfig> {
        self.batch_settlement.as_ref()
    }
}

/// A fully validated `[settlement.solana]` table: which deployed
/// `payment-channel` program instance (`packages/solana-program`) this
/// backend drives, where its RPC endpoint is, and where its signing key
/// material lives. `connector-cli` constructs the real backend from this at
/// startup (issue #630).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolanaSettlementConfig {
    rpc_via_socks_proxy: bool,
    rpc_url: String,
    program_id: String,
    token_address: String,
    decimals: u8,
    key: SecretLocation,
    batch_settlement: Option<SolanaBatchSettlementConfig>,
}

impl SolanaSettlementConfig {
    /// The RPC endpoint this backend connects through.
    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// The deployed `payment-channel` program (`packages/solana-program`)
    /// instance this backend would drive, base58-encoded.
    pub fn program_id(&self) -> &str {
        &self.program_id
    }

    /// The SPL token mint every channel this backend opens would settle in,
    /// base58-encoded.
    pub fn token_address(&self) -> &str {
        &self.token_address
    }

    /// The settlement asset's decimal precision.
    pub fn decimals(&self) -> u8 {
        self.decimals
    }

    /// Where this backend's signing key material lives.
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
    /// A **hint**, and since issue #1131 the *fallback* rather than the
    /// source: a running node takes its cluster from the chain's own
    /// genesis hash, read once when the Solana backend connects
    /// (`SolanaSettlementBackend::cluster`), which holds however the node
    /// reached the chain and so covers every host this list does not. What
    /// this still answers, and the genesis hash cannot, is the loopback
    /// case: `solana-test-validator` mints a fresh genesis on every run and
    /// therefore matches no published cluster hash, while its URL still
    /// says `localnet`. Nothing consults this before a backend exists, so
    /// there is no ordering problem -- the two are read together, in
    /// `connector-cli`'s `client_channels`.
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

    /// This node's terms for x402 batch-settlement channels on Solana, or
    /// `None` when it has not opted in (ADR 0074 decision 1). A node that
    /// has not offers no `batch-settlement` entry on Solana, sponsors no
    /// channel and refuses a Solana voucher by name.
    pub fn batch_settlement(&self) -> Option<&SolanaBatchSettlementConfig> {
        self.batch_settlement.as_ref()
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
/// on-chain facts (a `TokenNetworkRegistry` address vs. a program id) and a
/// single shared shape would either force one to fake fields it does not
/// have or erase which chain a value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlementConfig {
    Evm(EvmSettlementConfig),
    Solana(SolanaSettlementConfig),
}

impl SettlementConfig {
    /// The chain this settlement backend talks to.
    pub fn chain(&self) -> SettlementChain {
        match self {
            SettlementConfig::Evm(_) => SettlementChain::Evm,
            SettlementConfig::Solana(_) => SettlementChain::Solana,
        }
    }

    /// The token every channel this backend opens settles in, named the way
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

/// Which `[settlement.<chain>]` tables a config declares, and the one
/// value out of them a channel row needs -- the single input every "the
/// settlement table this channel needs is absent" rule reads (issue
/// #1138).
///
/// There is **one** such rule and it governs all four channel tables,
/// because the reason is one reason. A `[settlement.<chain>]` table is not
/// merely how a node *submits* a redemption: it is where the node's
/// on-chain identity on that chain comes from. `[settlement.evm.key]` is
/// this node's EVM address and `[settlement.solana.key]` its Solana one,
/// the connector holds a signer rather than a wallet (ADR 0012), and
/// "there is no second key to configure and none is invented" (ADR 0030,
/// as [`crate::ConfigError::PayChannelWithoutEvmSettlement`] already says).
/// A node with no table for a chain therefore has no address on it at all,
/// so it cannot be a participant of any channel there:
/// `TokenNetwork.claimFromChannel` refuses a caller that is not a
/// participant (`InvalidParticipant`,
/// `packages/contracts/src/TokenNetwork.sol:308`) and the Solana program
/// refuses a `claimer` account that is not one (`UnauthorizedSigner`,
/// `packages/solana-program/src/processor.rs:747`).
///
/// So a channel row whose chain has no settlement table names a channel
/// this node is not in, and every claim admitted on that row is carriage
/// rendered for money it can never collect. That is a **fact** about the
/// chain with exactly one answer, not a policy an operator may set -- the
/// same category issue #1136 put the EIP-712 domain in, and the reason
/// `connector_client_edge::DepositFloor::Unknown`'s latitude does not
/// reach it: a deposit floor is how much risk to take on a channel this
/// node *is* a participant of, so it presupposes redeemability rather than
/// conferring it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SettlementTables<'a> {
    evm: bool,
    solana_program_id: Option<&'a str>,
}

impl<'a> SettlementTables<'a> {
    /// Read the tables off an already-resolved settlement list. Called
    /// once in `Config::load`, immediately after `resolve_settlement`, so
    /// every channel table is resolved against the same answer.
    pub(crate) fn of(settlements: &'a [SettlementConfig]) -> Self {
        SettlementTables {
            evm: settlements
                .iter()
                .any(|settlement| matches!(settlement, SettlementConfig::Evm(_))),
            solana_program_id: settlements.iter().find_map(|settlement| match settlement {
                SettlementConfig::Solana(solana) => Some(solana.program_id()),
                SettlementConfig::Evm(_) => None,
            }),
        }
    }

    /// The same answer, stated directly, for a channel-table unit test
    /// that is about the channel row rather than about how a settlement
    /// table parses. `Config::load` always uses [`Self::of`].
    #[cfg(test)]
    pub(crate) fn for_tests(evm: bool, solana_program_id: Option<&'a str>) -> Self {
        SettlementTables {
            evm,
            solana_program_id,
        }
    }

    /// Whether this node has an EVM settlement table, and therefore an EVM
    /// on-chain identity a channel can name as its other participant.
    pub(crate) fn evm(&self) -> bool {
        self.evm
    }

    /// `[settlement.solana] program_id` -- the one program this node can
    /// redeem a Solana claim under, and since ADR 0053 part of what every
    /// Solana claim signs. `None` is a node with no Solana table at all,
    /// which is both "no program to judge a claim under" and "no Solana
    /// identity for a channel to be paid at".
    pub(crate) fn solana_program_id(&self) -> Option<&'a str> {
        self.solana_program_id
    }
}

/// Parse a 20-byte EVM address written as 40 hex characters, an optional
/// `0x`/`0X` prefix accepted since that is how every address in this
/// workspace's own docs, infra and decision comments is already written
/// (e.g. `'0x49beE1Bca5d15Fb0963117923403F9498119a9Ce'`).
pub(crate) fn parse_evm_address(value: &str) -> Option<[u8; 20]> {
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if hex.len() != 40 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 20];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
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

/// Shared rpc_url validation between the EVM and Solana tables (and the
/// legacy shape): non-empty, a well-formed URL, and http(s) -- none of this
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

/// Shared between the `[settlement.evm]` table and the legacy flat
/// `[settlement]` shape (which is converted into one at the call site): both
/// name the same fields, just under different config-file positions.
fn resolve_evm_fields(table: RawEvmSettlementTable) -> Result<EvmSettlementConfig, ConfigError> {
    let rpc_url = resolve_rpc_url(table.rpc_url)?;

    let contract_address = parse_evm_address(&table.contract_address).ok_or_else(|| {
        ConfigError::SettlementInvalidContractAddress {
            value: table.contract_address.clone(),
        }
    })?;
    let token_address = parse_evm_address(&table.token_address).ok_or_else(|| {
        ConfigError::SettlementInvalidTokenAddress {
            value: table.token_address.clone(),
        }
    })?;

    if table.decimals == 0 {
        return Err(ConfigError::SettlementZeroDecimals);
    }

    let key = resolve_settlement_key(table.key)?;

    let channel_index_confirmations = table
        .channel_index_confirmations
        .unwrap_or(DEFAULT_CHANNEL_INDEX_CONFIRMATIONS);
    if channel_index_confirmations == 0 {
        return Err(ConfigError::SettlementChannelIndexConfirmationsZero);
    }

    Ok(EvmSettlementConfig {
        rpc_url,
        contract_address,
        token_address,
        decimals: table.decimals,
        key,
        channel_index_from_block: table.channel_index_from_block.unwrap_or(0),
        channel_index_confirmations,
        rpc_via_socks_proxy: table.rpc_via_socks_proxy,
        batch_settlement: table
            .batch_settlement
            .map(resolve_evm_batch_settlement)
            .transpose()?,
    })
}

fn resolve_solana_fields(
    table: RawSolanaSettlementTable,
) -> Result<SolanaSettlementConfig, ConfigError> {
    let rpc_url = resolve_rpc_url(table.rpc_url)?;

    if table.program_id.trim().is_empty() {
        return Err(ConfigError::SettlementMissingProgramId);
    }
    if table.token_address.trim().is_empty() {
        return Err(ConfigError::SettlementMissingSolanaTokenAddress);
    }

    if table.decimals == 0 {
        return Err(ConfigError::SettlementZeroDecimals);
    }

    let key = resolve_settlement_key(table.key)?;

    Ok(SolanaSettlementConfig {
        rpc_via_socks_proxy: table.rpc_via_socks_proxy,
        rpc_url,
        program_id: table.program_id,
        token_address: table.token_address,
        decimals: table.decimals,
        key,
        batch_settlement: table
            .batch_settlement
            .map(resolve_solana_batch_settlement)
            .transpose()?,
    })
}

/// Validate an optional `[settlement]` section, in either shape it can take
/// (issue #628). Presence configures one or more real settlement backends
/// (issue #542, epic #627); absence means channel operations keep degrading
/// to `ChannelOperationError::NoSettlementBackend`, exactly as before this
/// section existed.
///
/// Returns every chain the section names, each fully validated. At most one
/// entry per [`SettlementChain`] -- the keyed shape has exactly one table per
/// recognized chain by construction, and the legacy shape only ever names
/// one chain at all.
pub(crate) fn resolve_settlement(
    raw: Option<RawSettlementSection>,
) -> Result<Vec<SettlementConfig>, ConfigError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };

    match raw {
        RawSettlementSection::Legacy(raw) => {
            match raw.chain.as_str() {
                "evm" => {}
                other => {
                    return Err(ConfigError::SettlementUnknownChain {
                        value: other.to_string(),
                    })
                }
            };
            let evm = resolve_evm_fields(RawEvmSettlementTable {
                rpc_url: raw.rpc_url,
                contract_address: raw.contract_address,
                token_address: raw.token_address,
                decimals: raw.decimals,
                key: raw.key,
                // The legacy flat shape is frozen (issue #628): it has no
                // channel_index_* fields of its own, so both default exactly
                // as an omitted keyed [settlement.evm] table would.
                channel_index_from_block: None,
                channel_index_confirmations: None,
                // Frozen too: a node that wants its settlement RPC on a
                // circuit writes the keyed `[settlement.evm]` table.
                rpc_via_socks_proxy: false,
                // And for batch settlement (ADR 0074): the opt-in is a
                // sub-table of the keyed `[settlement.evm]` only.
                batch_settlement: None,
            })?;
            Ok(vec![SettlementConfig::Evm(evm)])
        }
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn raw(
        chain: &str,
        rpc_url: &str,
        contract_address: &str,
        token_address: &str,
        decimals: u8,
        key_file: Option<PathBuf>,
    ) -> RawSettlementSection {
        RawSettlementSection::Legacy(RawSettlementConfig {
            chain: chain.to_string(),
            rpc_url: rpc_url.to_string(),
            contract_address: contract_address.to_string(),
            token_address: token_address.to_string(),
            decimals,
            key: RawSettlementKeyConfig {
                key_file,
                kms_key_id: None,
            },
        })
    }

    fn temp_key_file() -> tempfile::NamedTempFile {
        tempfile::NamedTempFile::new().expect("temp key file")
    }

    const CONTRACT: &str = "0x1234567890123456789012345678901234567890";
    const TOKEN: &str = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce";

    fn expect_single_evm(settlements: Vec<SettlementConfig>) -> EvmSettlementConfig {
        assert_eq!(settlements.len(), 1);
        match settlements.into_iter().next().unwrap() {
            SettlementConfig::Evm(evm) => evm,
            SettlementConfig::Solana(_) => panic!("expected an evm settlement config"),
        }
    }

    #[test]
    fn absent_settlement_section_resolves_to_none() {
        let resolved = resolve_settlement(None).expect("resolve");
        assert!(resolved.is_empty());
    }

    #[test]
    fn a_fully_configured_evm_section_resolves() {
        let key_file = temp_key_file();
        let resolved = resolve_settlement(Some(raw(
            "evm",
            "http://127.0.0.1:8545",
            CONTRACT,
            TOKEN,
            6,
            Some(key_file.path().to_path_buf()),
        )))
        .expect("resolve");
        let resolved = expect_single_evm(resolved);

        assert_eq!(resolved.rpc_url(), "http://127.0.0.1:8545");
        assert_eq!(
            resolved.contract_address(),
            parse_evm_address(CONTRACT).unwrap()
        );
        assert_eq!(resolved.token_address(), parse_evm_address(TOKEN).unwrap());
        assert_eq!(resolved.decimals(), 6);
        assert_eq!(
            resolved.key(),
            &SecretLocation::File(key_file.path().to_path_buf())
        );
    }

    #[test]
    fn a_contract_address_without_a_0x_prefix_still_parses() {
        let key_file = temp_key_file();
        let resolved = resolve_settlement(Some(raw(
            "evm",
            "http://127.0.0.1:8545",
            "1234567890123456789012345678901234567890",
            TOKEN,
            6,
            Some(key_file.path().to_path_buf()),
        )))
        .expect("resolve");
        let resolved = expect_single_evm(resolved);
        assert_eq!(
            resolved.contract_address(),
            parse_evm_address(CONTRACT).unwrap()
        );
    }

    #[test]
    fn rejects_an_unknown_chain() {
        let key_file = temp_key_file();
        let result = resolve_settlement(Some(raw(
            "made-up-chain",
            "http://127.0.0.1:8545",
            CONTRACT,
            TOKEN,
            6,
            Some(key_file.path().to_path_buf()),
        )));
        assert!(matches!(
            result,
            Err(ConfigError::SettlementUnknownChain { .. })
        ));
    }

    /// The legacy flat shape stays frozen at `chain = "evm"` (issue #628):
    /// `"solana"` is only reachable through the keyed `[settlement.solana]`
    /// table below, not by teaching the old `chain` field a new value.
    #[test]
    fn the_legacy_flat_shape_rejects_solana() {
        let key_file = temp_key_file();
        let result = resolve_settlement(Some(raw(
            "solana",
            "http://127.0.0.1:8545",
            CONTRACT,
            TOKEN,
            6,
            Some(key_file.path().to_path_buf()),
        )));
        assert!(matches!(
            result,
            Err(ConfigError::SettlementUnknownChain { .. })
        ));
    }

    #[test]
    fn rejects_an_empty_rpc_url() {
        let key_file = temp_key_file();
        let result = resolve_settlement(Some(raw(
            "evm",
            "",
            CONTRACT,
            TOKEN,
            6,
            Some(key_file.path().to_path_buf()),
        )));
        assert!(matches!(result, Err(ConfigError::SettlementMissingRpcUrl)));
    }

    #[test]
    fn rejects_a_non_http_rpc_scheme() {
        let key_file = temp_key_file();
        let result = resolve_settlement(Some(raw(
            "evm",
            "ws://127.0.0.1:8545",
            CONTRACT,
            TOKEN,
            6,
            Some(key_file.path().to_path_buf()),
        )));
        assert!(matches!(
            result,
            Err(ConfigError::SettlementUnsupportedRpcScheme { .. })
        ));
    }

    #[test]
    fn rejects_a_malformed_rpc_url() {
        let key_file = temp_key_file();
        let result = resolve_settlement(Some(raw(
            "evm",
            "not a url",
            CONTRACT,
            TOKEN,
            6,
            Some(key_file.path().to_path_buf()),
        )));
        assert!(matches!(
            result,
            Err(ConfigError::SettlementInvalidRpcUrl { .. })
        ));
    }

    #[test]
    fn rejects_an_invalid_contract_address() {
        let key_file = temp_key_file();
        let result = resolve_settlement(Some(raw(
            "evm",
            "http://127.0.0.1:8545",
            "not-an-address",
            TOKEN,
            6,
            Some(key_file.path().to_path_buf()),
        )));
        assert!(matches!(
            result,
            Err(ConfigError::SettlementInvalidContractAddress { .. })
        ));
    }

    #[test]
    fn rejects_an_invalid_token_address() {
        let key_file = temp_key_file();
        let result = resolve_settlement(Some(raw(
            "evm",
            "http://127.0.0.1:8545",
            CONTRACT,
            "not-an-address",
            6,
            Some(key_file.path().to_path_buf()),
        )));
        assert!(matches!(
            result,
            Err(ConfigError::SettlementInvalidTokenAddress { .. })
        ));
    }

    #[test]
    fn rejects_zero_decimals() {
        let key_file = temp_key_file();
        let result = resolve_settlement(Some(raw(
            "evm",
            "http://127.0.0.1:8545",
            CONTRACT,
            TOKEN,
            0,
            Some(key_file.path().to_path_buf()),
        )));
        assert!(matches!(result, Err(ConfigError::SettlementZeroDecimals)));
    }

    #[test]
    fn rejects_a_settlement_key_naming_neither_location() {
        let result = resolve_settlement(Some(raw(
            "evm",
            "http://127.0.0.1:8545",
            CONTRACT,
            TOKEN,
            6,
            None,
        )));
        assert!(matches!(
            result,
            Err(ConfigError::SettlementKeyLocationAmbiguous { .. })
        ));
    }

    #[test]
    fn rejects_a_settlement_key_file_that_does_not_exist() {
        let result = resolve_settlement(Some(raw(
            "evm",
            "http://127.0.0.1:8545",
            CONTRACT,
            TOKEN,
            6,
            Some(PathBuf::from("/nonexistent/does-not-exist.key")),
        )));
        assert!(matches!(
            result,
            Err(ConfigError::SettlementKeyFileNotFound(_))
        ));
    }

    #[test]
    fn an_unknown_key_in_the_settlement_section_is_rejected_at_parse_time() {
        let key_file = temp_key_file();
        let text = format!(
            r#"
chain = "evm"
rpc_url = "http://127.0.0.1:8545"
contract_address = "{CONTRACT}"
token_address = "{TOKEN}"
decimals = 6
made_up_field = "oops"

[key]
key_file = "{}"
"#,
            key_file.path().display()
        );
        let result: Result<RawSettlementConfig, _> = toml::from_str(&text);
        assert!(result.is_err());
    }

    // -- keyed per-chain tables (issue #628) --

    fn keyed_toml(body: &str) -> RawSettlementSection {
        toml::from_str(body).expect("valid keyed settlement toml")
    }

    #[test]
    fn a_keyed_evm_table_resolves_the_same_as_the_legacy_shape() {
        let key_file = temp_key_file();
        let text = format!(
            r#"
[evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "{CONTRACT}"
token_address = "{TOKEN}"
decimals = 6

[evm.key]
key_file = "{}"
"#,
            key_file.path().display()
        );
        let resolved = resolve_settlement(Some(keyed_toml(&text))).expect("resolve");
        let resolved = expect_single_evm(resolved);
        assert_eq!(resolved.rpc_url(), "http://127.0.0.1:8545");
        assert_eq!(
            resolved.contract_address(),
            parse_evm_address(CONTRACT).unwrap()
        );
    }

    #[test]
    fn a_keyed_solana_table_parses_into_typed_config() {
        let key_file = temp_key_file();
        let text = format!(
            r#"
[solana]
rpc_url = "http://127.0.0.1:8899"
program_id = "TokenNetworkProgram11111111111111111111111"
token_address = "SoLMint11111111111111111111111111111111111"
decimals = 6

[solana.key]
key_file = "{}"
"#,
            key_file.path().display()
        );
        let resolved = resolve_settlement(Some(keyed_toml(&text))).expect("resolve");
        assert_eq!(resolved.len(), 1);
        match &resolved[0] {
            SettlementConfig::Solana(solana) => {
                assert_eq!(solana.rpc_url(), "http://127.0.0.1:8899");
                assert_eq!(
                    solana.program_id(),
                    "TokenNetworkProgram11111111111111111111111"
                );
                assert_eq!(
                    solana.token_address(),
                    "SoLMint11111111111111111111111111111111111"
                );
                assert_eq!(solana.decimals(), 6);
            }
            SettlementConfig::Evm(_) => panic!("expected a solana settlement config"),
        }
        assert_eq!(resolved[0].chain(), SettlementChain::Solana);
    }

    // -- x402 batch-settlement opt-in (ADR 0074, issue #1340) --

    fn keyed_both(evm_extra: &str, solana_extra: &str, key_file: &Path) -> String {
        format!(
            r#"
[evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "{CONTRACT}"
token_address = "{TOKEN}"
decimals = 6

[evm.key]
key_file = "{key}"
{evm_extra}

[solana]
rpc_url = "http://127.0.0.1:8899"
program_id = "TokenNetworkProgram11111111111111111111111"
token_address = "SoLMint11111111111111111111111111111111111"
decimals = 6

[solana.key]
key_file = "{key}"
{solana_extra}
"#,
            key = key_file.display()
        )
    }

    fn both(settlements: &[SettlementConfig]) -> (&EvmSettlementConfig, &SolanaSettlementConfig) {
        match settlements {
            [SettlementConfig::Evm(evm), SettlementConfig::Solana(solana)] => (evm, solana),
            other => panic!("expected an evm and a solana table, got {other:?}"),
        }
    }

    /// Decision 1: off unless configured, per chain. A settlement table that
    /// says nothing about batch settlement has not opted in.
    #[test]
    fn batch_settlement_is_off_unless_its_table_is_written() {
        let key_file = temp_key_file();
        let resolved = resolve_settlement(Some(keyed_toml(&keyed_both("", "", key_file.path()))))
            .expect("resolve");
        let (evm, solana) = both(&resolved);

        assert_eq!(evm.batch_settlement(), None);
        assert_eq!(solana.batch_settlement(), None);
    }

    /// Each chain opts in on its own: EVM on, Solana still off.
    #[test]
    fn batch_settlement_is_opted_into_per_chain() {
        let key_file = temp_key_file();
        let resolved = resolve_settlement(Some(keyed_toml(&keyed_both(
            "[evm.batch_settlement]\nmin_withdraw_delay_secs = 3600",
            "",
            key_file.path(),
        ))))
        .expect("resolve");
        let (evm, solana) = both(&resolved);

        let batch = evm.batch_settlement().expect("EVM opted in");
        assert_eq!(batch.min_withdraw_delay_secs(), 3600);
        assert_eq!(solana.batch_settlement(), None);

        let resolved = resolve_settlement(Some(keyed_toml(&keyed_both(
            "",
            "[solana.batch_settlement]\nmin_sponsored_deposit = 5",
            key_file.path(),
        ))))
        .expect("resolve");
        let (evm, solana) = both(&resolved);
        assert_eq!(evm.batch_settlement(), None);
        let batch = solana.batch_settlement().expect("Solana opted in");
        assert_eq!(batch.min_sponsored_deposit(), 5);
    }

    /// A refusal inside the sub-table surfaces from `resolve_settlement`
    /// with its own name, not as a generic parse failure.
    #[test]
    fn a_below_floor_batch_settlement_delay_fails_the_settlement_section_by_name() {
        let key_file = temp_key_file();
        let result = resolve_settlement(Some(keyed_toml(&keyed_both(
            "",
            "[solana.batch_settlement]\nmin_sponsored_deposit = 5\nmin_grace_period_secs = 60",
            key_file.path(),
        ))));
        assert!(matches!(
            result,
            Err(ConfigError::BatchSettlementDelayBelowFloor {
                table: "solana",
                key: "min_grace_period_secs",
                value: 60,
            })
        ));
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

    #[test]
    fn declaring_both_evm_and_solana_parses_both_as_typed_per_chain_config() {
        let key_file = temp_key_file();
        let text = format!(
            r#"
[evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "{CONTRACT}"
token_address = "{TOKEN}"
decimals = 6

[evm.key]
key_file = "{key_path}"

[solana]
rpc_url = "http://127.0.0.1:8899"
program_id = "TokenNetworkProgram11111111111111111111111"
token_address = "SoLMint11111111111111111111111111111111111"
decimals = 6

[solana.key]
key_file = "{key_path}"
"#,
            key_path = key_file.path().display()
        );
        let resolved = resolve_settlement(Some(keyed_toml(&text))).expect("resolve");
        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().any(|s| s.chain() == SettlementChain::Evm));
        assert!(resolved
            .iter()
            .any(|s| s.chain() == SettlementChain::Solana));
    }

    #[test]
    fn an_empty_keyed_settlement_section_is_rejected() {
        let result = resolve_settlement(Some(keyed_toml("")));
        assert!(matches!(result, Err(ConfigError::SettlementSectionEmpty)));
    }

    #[test]
    fn a_solana_table_missing_program_id_is_rejected() {
        let key_file = temp_key_file();
        let text = format!(
            r#"
[solana]
rpc_url = "http://127.0.0.1:8899"
program_id = ""
token_address = "SoLMint11111111111111111111111111111111111"
decimals = 6

[solana.key]
key_file = "{}"
"#,
            key_file.path().display()
        );
        let result = resolve_settlement(Some(keyed_toml(&text)));
        assert!(matches!(
            result,
            Err(ConfigError::SettlementMissingProgramId)
        ));
    }

    #[test]
    fn an_evm_table_with_no_channel_index_fields_gets_the_default_confirmation_depth() {
        let key_file = temp_key_file();
        let text = format!(
            r#"
[evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "{CONTRACT}"
token_address = "{TOKEN}"
decimals = 6

[evm.key]
key_file = "{}"
"#,
            key_file.path().display()
        );
        let resolved = resolve_settlement(Some(keyed_toml(&text))).expect("resolve");
        let resolved = expect_single_evm(resolved);
        assert_eq!(resolved.channel_index_from_block(), 0);
        assert_eq!(
            resolved.channel_index_confirmations(),
            DEFAULT_CHANNEL_INDEX_CONFIRMATIONS
        );
    }

    #[test]
    fn an_evm_table_can_set_the_channel_index_from_block_and_confirmations() {
        let key_file = temp_key_file();
        let text = format!(
            r#"
[evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "{CONTRACT}"
token_address = "{TOKEN}"
decimals = 6
channel_index_from_block = 123456
channel_index_confirmations = 12

[evm.key]
key_file = "{}"
"#,
            key_file.path().display()
        );
        let resolved = resolve_settlement(Some(keyed_toml(&text))).expect("resolve");
        let resolved = expect_single_evm(resolved);
        assert_eq!(resolved.channel_index_from_block(), 123456);
        assert_eq!(resolved.channel_index_confirmations(), 12);
    }

    #[test]
    fn a_channel_index_confirmations_of_zero_is_rejected_at_load_time() {
        let key_file = temp_key_file();
        let text = format!(
            r#"
[evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "{CONTRACT}"
token_address = "{TOKEN}"
decimals = 6
channel_index_confirmations = 0

[evm.key]
key_file = "{}"
"#,
            key_file.path().display()
        );
        let result = resolve_settlement(Some(keyed_toml(&text)));
        assert!(matches!(
            result,
            Err(ConfigError::SettlementChannelIndexConfirmationsZero)
        ));
    }

    #[test]
    fn the_legacy_flat_shape_gets_the_default_channel_index_confirmations() {
        let key_file = temp_key_file();
        let resolved = resolve_settlement(Some(raw(
            "evm",
            "http://127.0.0.1:8545",
            CONTRACT,
            TOKEN,
            6,
            Some(key_file.path().to_path_buf()),
        )))
        .expect("resolve");
        let resolved = expect_single_evm(resolved);
        assert_eq!(resolved.channel_index_from_block(), 0);
        assert_eq!(
            resolved.channel_index_confirmations(),
            DEFAULT_CHANNEL_INDEX_CONFIRMATIONS
        );
    }

    #[test]
    fn an_unknown_key_in_a_keyed_evm_table_is_rejected_at_parse_time() {
        let key_file = temp_key_file();
        let text = format!(
            r#"
[evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "{CONTRACT}"
token_address = "{TOKEN}"
decimals = 6
made_up_field = "oops"

[evm.key]
key_file = "{}"
"#,
            key_file.path().display()
        );
        let result: Result<RawSettlementSection, _> = toml::from_str(&text);
        assert!(result.is_err());
    }

    /// ADR 0071, issue #1290: a settlement table already states which token
    /// its channels settle in, so the asset a `[[tokens]]` row would have to
    /// match is derived from it rather than declared a second time.
    #[test]
    fn a_settlement_table_names_the_token_its_channels_settle_in() {
        let key_file = temp_key_file();
        let evm = resolve_settlement(Some(raw(
            "evm",
            "http://127.0.0.1:8545",
            CONTRACT,
            TOKEN,
            6,
            Some(key_file.path().to_path_buf()),
        )))
        .expect("resolve");
        // The checksummed spelling the config carries and the lowercase one
        // an `AssetId` keys by are one token.
        assert_eq!(
            SettlementConfig::Evm(expect_single_evm(evm)).asset(),
            AssetId::evm(TOKEN)
        );

        let solana = SettlementConfig::Solana(SolanaSettlementConfig {
            rpc_via_socks_proxy: false,
            rpc_url: "http://127.0.0.1:8899".to_string(),
            program_id: "2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip".to_string(),
            token_address: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            decimals: 6,
            key: SecretLocation::File(key_file.path().to_path_buf()),
            batch_settlement: None,
        });
        assert_eq!(
            solana.asset(),
            AssetId::solana("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")
        );
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
}
