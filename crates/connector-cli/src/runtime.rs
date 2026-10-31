//! Builds the live [`Connector`] and its signer from a validated [`Config`],
//! and merges the client-edge and operator routers into the one
//! [`axum::Router`] the binary serves. Per ADR 0001 this is where every
//! construction decision lives -- `connector-bin` calls exactly
//! [`build`] and [`router`] and branches on neither.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use axum::Router;

use connector_chain_rpc::{Circuit, RpcTransport};
use connector_client_edge::{
    ClientClaimGate, ClientPayoutLedger, PeerCarriages, UnresolvableLookupBudgetPolicy,
};
use connector_config::{
    Config, EvmSettlementConfig, PeerCarriage, SecretLocation, SettlementChain, SettlementConfig,
    SolanaSettlementConfig, XrplSettlementConfig,
};
use connector_domain::Network;
use connector_rate_source_evm::UniswapV3RateSource;
use connector_runtime::{
    BatchChannels, BoundedHttpSelfDescription, ConfigPeeringError, Connector, DeclaredRates,
    FileJournal, HttpAppClient, InMemoryJournal, Journal, JournalError, OutboundChannels,
    OutboundRestoreError, PeerRegistrar, PeerRoute, PeerRouteStore, PeerRouteStoreError,
    PeerTransport, QuotePathUnusable, RatePoller, RateSources, SharedRateTable, SystemClock,
};
use connector_settlement::batch::{
    BatchSettlementBackend, BatchSettlementError, BatchSettlementPayer, HeldVouchers,
};
use connector_settlement::watch_interval::watch_interval;
use connector_settlement_evm::{DepositRoute, EvmBatchSettlementBackend, EvmBatchWatcher};
use connector_settlement_solana::batch::{SolanaBatchSettlement, SolanaBatchWatcher};
use connector_settlement_xrpl::{XrplBatchSettlement, XrplBatchWatcher};
use connector_signer::{LocalSigner, Signer, SignerError, XrplSigner};

use crate::batch_settlement::{
    restore_journaled_channels, BatchSettlementChannelsAdapter, ClaimGateVouchers,
    GateChannelNetworks, NetworkVouchers,
};
use crate::peer_transport;
use solana_sdk::pubkey::Pubkey;

/// Everything that can stop a validated [`Config`] from producing a live
/// [`Connector`]. Distinct from [`connector_config::ConfigError`]: the
/// config file itself was already valid TOML with well-formed fields --
/// these errors are about the world the config points at (a key file that
/// cannot be read, or a location this binary cannot yet resolve).
#[derive(Debug)]
pub enum RuntimeError {
    /// The signer's key file exists (config load already checked that) but
    /// could not be read.
    SignerKeyFileUnreadable {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The signer's key file's contents are neither 32 raw bytes nor 64
    /// hex characters encoding 32 bytes.
    InvalidSignerKeyMaterial { path: PathBuf },
    /// `signer.kms_key_id` was configured, but no key management service
    /// backend is wired into this binary -- `connector-signer::KmsSigner`
    /// is a port over one, and `InMemoryKmsBackend` upholds its contract
    /// for tests, but no production backend exists in this workspace yet.
    /// Use `signer.key_file` instead.
    UnsupportedSignerLocation,
    /// The signer implementation itself rejected the key material (e.g.
    /// an all-zero secret key).
    Signer(SignerError),
    /// The `[settlement]` section's key file exists (config load already
    /// checked that) but could not be read.
    SettlementKeyFileUnreadable {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The `[settlement]` section's key file's contents are neither 32
    /// raw bytes nor 64 hex characters encoding 32 bytes.
    InvalidSettlementKeyMaterial { path: PathBuf },
    /// `settlement.key.kms_key_id` was configured, but no key management
    /// service backend is wired into this binary yet -- same gap as
    /// `[signer]`'s own `kms_key_id`; use `settlement.key.key_file`
    /// instead.
    UnsupportedSettlementKeyLocation,
    /// A `[settlement.<chain>]` table names a chain on which the x402
    /// contract (EVM, `x402BatchSettlement`) or program (Solana,
    /// `payment-channels`) the binary fixes is not deployed (ADR 0075
    /// decision 1): a network x402 has not deployed to is unsupported,
    /// loudly, rather than a node that admits nothing and says nothing.
    X402NotDeployed { table: String, what: String },
    /// A journal under `state_dir` holds `toon-channel` entries: claims on
    /// TOON's own retired channels this build can neither judge, land nor
    /// drain (ADR 0075 decision 8, issue #1385). Refused by name, never
    /// skipped: a skipped entry is a claim somebody could still redeem that
    /// this node has forgotten it accepted.
    ToonChannelJournal { path: PathBuf, channel: String },
    /// `[zk_note]` is set, and this build has no Mina vault adapter to read
    /// the vault or verify a request proof with (ADR 0080; the adapter is
    /// #1514). Refused by name rather than served: a zk edge that cannot
    /// verify a proof can grant no allowance.
    ZkNoteVaultNotBuilt,
    /// `[zk_note] state_key_file` could not be read, or does not hold a
    /// Mina secret key in base58 (ADR 0080, #1523).
    ZkNoteStateKey { path: PathBuf, reason: String },
    /// `state_dir` names a directory this node cannot create or write a
    /// journal file in (issue #605) -- typically a read-only mount, or a
    /// directory owned by another uid than the one the container runs as.
    /// A startup failure on purpose: the alternative is a node that serves
    /// happily and hands out free service after its next restart.
    StateDirUnusable {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Another process already holds `state_dir`'s lock (issue #1318): two
    /// processes over one directory each replay the journals once and keep
    /// the watermarks in memory, so they diverge silently. Refused by name
    /// rather than served.
    StateDirHeld { path: PathBuf },
    /// A journal under `state_dir` exists but could not be replayed --
    /// unreadable, or carrying a line this build cannot decode. Refusing
    /// to start is the whole point: the only other option is to start with
    /// watermarks this node cannot vouch for, which is exactly the defect
    /// issue #605 describes.
    JournalUnreplayable { path: PathBuf, source: JournalError },
    /// The controller day-totals journal under `state_dir` could not be
    /// opened or replayed (ADR 0078 decision 8): refused rather than let a
    /// key's day reset.
    DayTotalsUnusable(connector_operator::DayTotalsError),
    /// Issue #884's runtime peer/route table under `state_dir` exists but
    /// could not be read (unreadable, or corrupt JSON) -- refusing to
    /// start rather than serve with a table this node cannot vouch for,
    /// the same reasoning `JournalUnreplayable` applies to a claim
    /// journal.
    RuntimePeerRouteTableUnusable {
        path: PathBuf,
        source: PeerRouteStoreError,
    },
    /// A config-declared x402 peering could not be wired (ADR 0075 decision
    /// 9, issue #1380): a `[[pay_channels]]` row naming an outbound channel
    /// this node's journal does not hold, or a `[[peer_channels]]` voucher
    /// signer that cannot be bound. A refusal to start, because the
    /// alternative is a peering that refuses every forward, or never proves
    /// itself, while the file reads as configured.
    ConfigPeering(ConfigPeeringError),
    /// A `[[tokens]]` row's declared `quote` path is not one a poller can
    /// read (ADR 0071 decision 3, issue #1294): no pool, more pools than
    /// compose, legs that do not meet, or a path ending somewhere other than
    /// this node's numeraire.
    ///
    /// `Config::load` refuses every one of those by name already, so for
    /// them this is the second lock on the same door -- and a refusal to
    /// start rather than a poller quietly skipping the pair, because a
    /// skipped pair reads as priced in the file while every forward across
    /// it refuses.
    ///
    /// One variant is the *only* lock on its door:
    /// [`QuotePathUnusable::NoSourceForChain`] (issue #1302), a quote path
    /// on a chain no rate source linked into this binary can read. The
    /// config crate cannot state that one, since which readers exist is a
    /// fact about this binary's wiring rather than about the file -- see
    /// `RateSources`.
    QuotePathUnpollable { source: QuotePathUnusable },
    /// A `[settlement.<chain>]` table's endpoint cannot be dialed the way
    /// it is written: its one transport (ADR 0073), which the backend and
    /// the rate source share, could not be built.
    ///
    /// `Config::load` already refuses an `rpc_url` that is not an `http(s)`
    /// URL and a `socks_proxy` that is not `socks5h://`, so this is the
    /// second lock on those doors -- and a refusal to start, because a table
    /// with no transport has no client that could settle on it.
    SettlementEndpointUnusable { table: String, message: String },
    /// A `[settlement.<chain>]` table's x402 backend could not be bound (ADR
    /// 0074, ADR 0075): the contract or program at the fixed address is not
    /// the x402 one, the token disagrees with the table, or the chain could
    /// not be asked. A refusal to start, for ADR 0009's reason -- the
    /// alternative is a node whose greeting offers `batch-settlement` and
    /// whose every voucher fails.
    BatchSettlementUnusable {
        table: String,
        source: BatchSettlementError,
    },
    /// A `[settlement.eip155.<chainId>]` table whose RPC answers
    /// `eth_chainId` with another id (ADR 0086 decision 4). The key is
    /// declared and asserted, like `decimals`: a wrong `rpc_url` must never
    /// settle on a network other than the one the operator named.
    EvmChainIdMismatch { network: Network, reported: u64 },
    /// Two `[settlement.eip155.<chainId>]` tables whose RPCs answer
    /// `eth_chainId` with the same id (ADR 0086 decision 4) -- two anvils
    /// both left at 31337, say. Refused by name before either is bound: two
    /// tables on one chain would be one network settled twice, under one
    /// EIP-712 domain.
    EvmNetworkReportedTwice { tables: Vec<String>, reported: u64 },
    /// ADR 0086 decision 10: the outbound channel journal holds a channel
    /// on no network this node has a table for -- a version-1 record no
    /// table's EIP-712 domain reproduces (`network: None`), or a version-2
    /// record naming a network whose table was removed. The node still
    /// holds value in it, so it refuses to start rather than forget it.
    /// `channel_id` is the journal key, `evm:0x…` or `solana:…`.
    OutboundChannelOnNoNetwork {
        channel_id: String,
        network: Option<Network>,
    },
    /// ADR 0086 decision 10: a version-1 outbound record more than one
    /// table's domain reproduces. Never assigned by guess.
    OutboundChannelOnSeveralNetworks {
        channel_id: String,
        networks: Vec<Network>,
    },
    /// `[settlement.evm] asset_transfer_method` is `"eip3009"`, and the
    /// token has no ERC-3009: it fails the very probe this node's own
    /// deposits are routed by (`EvmBatchSettlementBackend::deposit_route`,
    /// ADR 0075 decision 3). A refusal to start (issue #1422), because the
    /// method is published to payers and never used by this node itself:
    /// every payer's deposit would fail on chain, and nothing here would
    /// ever see it.
    AssetTransferMethodUnsupported { table: String, token: String },
    /// `[settlement.evm] asset_eip712_name` and `asset_eip712_version` do
    /// not reproduce the token's own `DOMAIN_SEPARATOR()`, on a table that
    /// publishes `eip3009`: every payer's ERC-3009 authorization is signed
    /// under them, so every one would be refused by the token (issue
    /// #1422). Only a definite mismatch is refused; a token with no
    /// `DOMAIN_SEPARATOR()` is not judged.
    AssetEip712DomainMismatch {
        table: String,
        token: String,
        name: String,
        version: String,
    },
    /// A `[settlement.xrpl.<id>]` key file that is no XRPL family seed (ADR
    /// 0087): unreadable, or holding something else.
    XrplKeyUnusable { table: String, message: String },
    /// Two `[settlement.xrpl.<id>]` tables whose keys are one account (ADR
    /// 0087 decision 4). A claim binds no network, so an account may be on
    /// one network only: a voucher signed for a testnet channel would
    /// redeem on a mainnet channel of the same id.
    XrplAccountOnTwoNetworks {
        tables: Vec<String>,
        address: String,
    },
    /// A `[settlement.xrpl.<id>]` table whose `rpc_url` serves another
    /// network than its key names (ADR 0087 decision 5). `reported` is the
    /// endpoint's `server_info.network_id`, absent on a node with none
    /// (mainnet reports none).
    XrplNetworkMismatch {
        table: String,
        expected: u32,
        reported: Option<u32>,
    },
    /// The settlement account is not on the ledger (ADR 0087 decision 5):
    /// it has to be funded with at least the base reserve before the node
    /// can receive a channel.
    XrplAccountMissing { table: String, address: String },
    /// The settlement account holds less than the ledger's base reserve
    /// (ADR 0087 decision 5): it cannot sign the transactions that land a
    /// voucher.
    XrplAccountUnderfunded {
        table: String,
        address: String,
        balance_drops: u64,
        reserve_drops: u64,
    },
    /// `server_info` reports no validated ledger's base reserve (the endpoint
    /// is still syncing), so the settlement account's funding cannot be
    /// checked (ADR 0087 decision 5). Refused rather than assumed.
    XrplReserveUnreported { table: String },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::SignerKeyFileUnreadable { path, source } => write!(
                f,
                "failed to read signer key_file at {}: {source}",
                path.display()
            ),
            RuntimeError::InvalidSignerKeyMaterial { path } => write!(
                f,
                "signer key_file at {} must contain either 32 raw bytes or \
                 64 hex characters encoding a 32-byte secret key",
                path.display()
            ),
            RuntimeError::UnsupportedSignerLocation => write!(
                f,
                "signer.kms_key_id is configured, but no key management \
                 service backend is wired into this binary yet -- use \
                 signer.key_file"
            ),
            RuntimeError::Signer(source) => write!(f, "{source}"),
            RuntimeError::SettlementKeyFileUnreadable { path, source } => write!(
                f,
                "failed to read settlement key_file at {}: {source}",
                path.display()
            ),
            RuntimeError::InvalidSettlementKeyMaterial { path } => write!(
                f,
                "settlement key_file at {} must contain either 32 raw bytes or \
                 64 hex characters encoding a 32-byte secret key",
                path.display()
            ),
            RuntimeError::UnsupportedSettlementKeyLocation => write!(
                f,
                "settlement.key.kms_key_id is configured, but no key management \
                 service backend is wired into this binary yet -- use \
                 settlement.key.key_file"
            ),
            RuntimeError::X402NotDeployed { table, what } => write!(
                f,
                "[settlement.{table}] names a chain on which {what} is not deployed. Every \
                 channel is an x402 channel on the contract or program the binary fixes (ADR \
                 0075 decision 1), so a chain without it cannot be settled on -- point rpc_url at \
                 a chain x402 is deployed to, or remove the table"
            ),
            RuntimeError::ZkNoteVaultNotBuilt => write!(
                f,
                "[zk_note] is set, but this build has no Mina vault adapter to read the vault \
                 and verify request proofs with (ADR 0080; connector#1514), so it cannot be a \
                 zk edge yet -- remove the [zk_note] table to run this build"
            ),
            RuntimeError::ZkNoteStateKey { path, reason } => write!(
                f,
                "[zk_note] state_key_file {} is unusable: {reason} -- it must hold the zk edge's \
                 Mina secret key in base58 (EK...), as Mina's tooling exports it",
                path.display()
            ),
            RuntimeError::ToonChannelJournal { path, channel } => write!(
                f,
                "the claim journal at {} holds toon-channel entries (channel '{channel}'), and \
                 this build settles on x402 channels only (ADR 0075, issue #1385): it can \
                 neither land nor drain a claim on TOON's own channels. Drain the node on the \
                 last release that supports TOON channels -- land every inbound channel's \
                 latest claim, close and settle every TOON channel, confirm on chain none is \
                 still open (ADR 0075, \"Draining a node with live TOON channels\"; \
                 docs/operators/draining-toon-channels.md) -- then move this journal out of \
                 state_dir and start this build",
                path.display()
            ),
            RuntimeError::StateDirUnusable { path, source } => write!(
                f,
                "state_dir {} is not usable for this node's claim journals: {source} -- \
                 the connector refuses to start rather than keep claim watermarks only in \
                 memory, where a restart would make every already-spent claim replayable",
                path.display()
            ),
            RuntimeError::StateDirHeld { path } => write!(
                f,
                "state_dir {} is held by another connector process -- the connector refuses \
                 to start over a directory a live node is serving from, because two processes \
                 would each keep their own claim watermarks and settlement nonces",
                path.display()
            ),
            RuntimeError::JournalUnreplayable { path, source } => write!(
                f,
                "failed to replay the claim journal at {}: {source} -- the connector \
                 refuses to start rather than resume from watermarks it cannot vouch for",
                path.display()
            ),
            RuntimeError::DayTotalsUnusable(error) => write!(f, "{error}"),
            RuntimeError::RuntimePeerRouteTableUnusable { path, source } => write!(
                f,
                "failed to read the runtime peer/route table at {}: {source} -- the connector \
                 refuses to start rather than serve with a peer/route table it cannot vouch for",
                path.display()
            ),
            RuntimeError::ConfigPeering(source) => write!(f, "{source}"),
            RuntimeError::QuotePathUnpollable { source } => write!(
                f,
                "a declared quote path cannot be polled: {source}. A token's quote is one or \
                 two operator-named pools on that token's own settlement chain, ending at this \
                 node's numeraire (ADR 0071 decision 3); a pair that cannot be sourced that way \
                 runs a [[rates]] row instead"
            ),
            RuntimeError::SettlementEndpointUnusable { table, message } => write!(
                f,
                "[settlement.{table}] rpc_url cannot be dialed as configured: {message}. Every \
                 client of that endpoint -- the settlement backend, and on EVM the rate source \
                 -- shares one transport built from it (ADR 0073)"
            ),
            RuntimeError::BatchSettlementUnusable { table, source } => write!(
                f,
                "[settlement.{table}] could not be bound: {source}. A node must reach the x402 \
                 contract or program its vouchers are signed for (ADR 0074, ADR 0075)"
            ),
            RuntimeError::EvmChainIdMismatch { network, reported } => {
                let declared = network.eip155_chain_id().unwrap_or_default();
                write!(
                    f,
                    "[settlement.eip155.{declared}] is keyed for chain id {declared} ({network}), but \
                     its rpc_url answers eth_chainId with {reported}. The table key names the \
                     network this table settles on and is checked, never trusted (ADR 0086): \
                     point rpc_url at chain {declared}, or key the table \
                     [settlement.eip155.{reported}] if that is the network you meant"
                )
            }
            RuntimeError::OutboundChannelOnNoNetwork {
                channel_id,
                network,
            } => write!(
                f,
                "{}",
                OutboundRestoreError::OnNoNetwork {
                    channel_id: channel_id.clone(),
                    network: network.clone(),
                }
            ),
            RuntimeError::OutboundChannelOnSeveralNetworks {
                channel_id,
                networks,
            } => write!(
                f,
                "{}",
                OutboundRestoreError::OnSeveralNetworks {
                    channel_id: channel_id.clone(),
                    networks: networks.clone(),
                }
            ),
            RuntimeError::EvmNetworkReportedTwice { tables, reported } => {
                let tables: Vec<String> = tables
                    .iter()
                    .map(|table| format!("[settlement.{table}]"))
                    .collect();
                write!(
                    f,
                    "{} each answer eth_chainId with {reported}: two settlement tables on one \
                     chain are one network settled twice (ADR 0086 decision 4). Point each \
                     table's rpc_url at the chain its key names -- a local anvil takes \
                     --chain-id",
                    tables.join(" and ")
                )
            }
            RuntimeError::AssetTransferMethodUnsupported { table, token } => write!(
                f,
                "[settlement.{table}] asset_transfer_method = \"eip3009\", but token {token} does not \
                 have ERC-3009 (it reverts on authorizationState). Payers read this method to \
                 sign their deposit, so every deposit would fail on chain -- set \
                 asset_transfer_method = \"permit2\", which works for any ERC-20 (issue #1422)"
            ),
            RuntimeError::AssetEip712DomainMismatch {
                table,
                token,
                name,
                version,
            } => write!(
                f,
                "[settlement.{table}] asset_eip712_name = \"{name}\" and asset_eip712_version = \
                 \"{version}\" are not the EIP-712 domain of token {token}: they do not reproduce \
                 its DOMAIN_SEPARATOR() on this chain. Payers sign their ERC-3009 deposit under \
                 them, so the token would refuse every one -- set them to the token's own name \
                 and version (cast call {token} 'name()(string)' and 'version()(string)') \
                 (issue #1422)"
            ),
            RuntimeError::XrplKeyUnusable { table, message } => {
                write!(f, "[settlement.{table}.key] cannot be used: {message}")
            }
            RuntimeError::XrplAccountOnTwoNetworks { tables, address } => write!(
                f,
                "[settlement.{}] name one XRPL account, {address}. A claim binds no network, so \
                 an account may be on one network only: a voucher signed for a channel on one \
                 network is redeemable by anyone on another network that holds a channel with \
                 the same id. Give each table its own key (ADR 0087 decision 4)",
                tables.join("] and [settlement."),
            ),
            RuntimeError::XrplNetworkMismatch {
                table,
                expected,
                reported,
            } => write!(
                f,
                "[settlement.{table}] is for XRPL network {expected}, but its rpc_url serves {}. \
                 The table key is the ledger's server_info network_id: correct the key or point \
                 rpc_url at the right ledger (ADR 0087 decision 5)",
                reported.map_or("a node that reports no network_id (mainnet is 0)".to_string(), |id| {
                    format!("network {id}")
                }),
            ),
            RuntimeError::XrplAccountMissing { table, address } => write!(
                f,
                "[settlement.{table}]'s settlement account {address} does not exist on the \
                 ledger its rpc_url serves. Fund it with at least the base reserve; an account \
                 comes into being with its first payment (ADR 0087 decision 5)"
            ),
            RuntimeError::XrplAccountUnderfunded {
                table,
                address,
                balance_drops,
                reserve_drops,
            } => write!(
                f,
                "[settlement.{table}]'s settlement account {address} holds {balance_drops} drops, \
                 below the ledger's base reserve of {reserve_drops}. It could not sign the \
                 transaction that lands a voucher: fund it (ADR 0087 decision 5)"
            ),
            RuntimeError::XrplReserveUnreported { table } => write!(
                f,
                "[settlement.{table}]'s rpc_url reports no validated ledger, so its base reserve \
                 is unknown and the settlement account's funding cannot be checked: point it at \
                 a synced server (ADR 0087 decision 5)"
            ),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<SignerError> for RuntimeError {
    fn from(source: SignerError) -> Self {
        RuntimeError::Signer(source)
    }
}

/// Decode a signer key file's raw bytes into a 32-byte secret key: either
/// exactly 32 raw bytes, or 64 hex characters (surrounding whitespace
/// ignored) encoding 32 bytes. Both are legitimate ways to hand-author or
/// generate a key file, so both are accepted.
fn decode_secret_key(bytes: &[u8]) -> Option<[u8; 32]> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        let trimmed = text.trim();
        if trimmed.len() == 64 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
            let mut out = [0u8; 32];
            for (i, byte) in out.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&trimmed[i * 2..i * 2 + 2], 16).ok()?;
            }
            return Some(out);
        }
    }
    if bytes.len() == 32 {
        let mut out = [0u8; 32];
        out.copy_from_slice(bytes);
        return Some(out);
    }
    None
}

/// The raw 32-byte identity secret `[signer]` points at.
///
/// Exposed to the crate (issue #784) because a kind:10032 announce is signed
/// BIP-340 Schnorr over the event's own id, which needs the scalar itself
/// rather than a [`Signer`]'s recoverable-ECDSA `sign` -- see
/// `connector_signer::nostr`. Nothing outside this crate can call it: key
/// material stays behind `connector-signer` and the one function here that
/// already had to read it.
pub(crate) fn read_signer_secret(location: &SecretLocation) -> Result<[u8; 32], RuntimeError> {
    match location {
        SecretLocation::File(path) => {
            let bytes =
                std::fs::read(path).map_err(|source| RuntimeError::SignerKeyFileUnreadable {
                    path: path.clone(),
                    source,
                })?;
            decode_secret_key(&bytes)
                .ok_or_else(|| RuntimeError::InvalidSignerKeyMaterial { path: path.clone() })
        }
        SecretLocation::Kms { .. } => Err(RuntimeError::UnsupportedSignerLocation),
    }
}

fn build_signer(location: &SecretLocation) -> Result<Arc<dyn Signer>, RuntimeError> {
    let secret = read_signer_secret(location)?;
    let signer = LocalSigner::from_secret_bytes("connector-signer", secret)?;
    Ok(Arc::new(signer))
}

/// Encode 32 raw bytes as 64 lowercase hex characters -- what
/// `ethers::signers::LocalWallet`'s `FromStr` impl expects,
/// [`EvmBatchSettlementBackend::connect`]'s `private_key` argument.
fn hex_encode_32(bytes: [u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Resolve the `[settlement.key]` section to the raw 32-byte secret key
/// material it points at -- the same "32 raw bytes or 64 hex characters"
/// key-file shape [`build_signer`] already reads for `[signer]`, since both
/// are just secret-key pointers. The EVM backend wants that hex encoded
/// ([`read_settlement_private_key`]); the Solana one wants it as an ed25519
/// seed, raw (issue #630).
pub(crate) fn read_settlement_key_bytes(
    location: &SecretLocation,
) -> Result<[u8; 32], RuntimeError> {
    match location {
        SecretLocation::File(path) => {
            let bytes = std::fs::read(path).map_err(|source| {
                RuntimeError::SettlementKeyFileUnreadable {
                    path: path.clone(),
                    source,
                }
            })?;
            decode_secret_key(&bytes)
                .ok_or_else(|| RuntimeError::InvalidSettlementKeyMaterial { path: path.clone() })
        }
        SecretLocation::Kms { .. } => Err(RuntimeError::UnsupportedSettlementKeyLocation),
    }
}

/// Resolve the `[settlement.evm.key]` section to the hex-encoded secp256k1
/// private key the EVM backend signs with.
fn read_settlement_private_key(location: &SecretLocation) -> Result<String, RuntimeError> {
    read_settlement_key_bytes(location).map(hex_encode_32)
}

/// ADR 0086 decision 4: what each `[settlement.eip155.<chainId>]` table's
/// RPC reports as its chain id, read over the table's own transport before
/// anything is bound, and two tables reporting one id refused by name.
/// Each table's report is then asserted against its key as it is built.
async fn evm_reported_chain_ids(
    config: &Config,
    transports: &SettlementTransports,
) -> Result<BTreeMap<Network, u64>, RuntimeError> {
    let mut reported: BTreeMap<Network, u64> = BTreeMap::new();
    for settlement in config.settlements() {
        let SettlementConfig::Evm(evm) = settlement else {
            continue;
        };
        let transport = transports.for_settlement(settlement)?;
        let id = EvmBatchSettlementBackend::chain_id_at(transport)
            .await
            .map_err(|source| unbound(&evm.table_name(), source))?;
        let twins: Vec<String> = reported
            .iter()
            .filter(|(_, held)| **held == id)
            .map(|(network, _)| format!("eip155.{}", network.eip155_chain_id().unwrap_or_default()))
            .collect();
        if !twins.is_empty() {
            let mut tables = twins;
            tables.push(evm.table_name());
            return Err(RuntimeError::EvmNetworkReportedTwice {
                tables,
                reported: id,
            });
        }
        reported.insert(evm.network().clone(), id);
    }
    Ok(reported)
}

/// Map a backend that would not bind to its refusal: by name when the x402
/// contract or program is absent from the chain (ADR 0075 decision 1).
fn unbound(table: &str, source: BatchSettlementError) -> RuntimeError {
    let table = table.to_string();
    match source {
        BatchSettlementError::NotDeployed(what) => RuntimeError::X402NotDeployed { table, what },
        source => RuntimeError::BatchSettlementUnusable { table, source },
    }
}

/// This node's x402 settlement backend on EVM, both halves (ADR 0074, ADR
/// 0075), built from its `[settlement.evm]` table over the table's one
/// transport (ADR 0073): refuses, by name, a chain without
/// `x402BatchSettlement` (ADR 0075 decision 1), a `decimals` the token
/// disagrees with (issue #564), and a deposit method or asset domain the
/// token cannot honour ([`check_published_deposit_terms`], issue #1422).
async fn build_evm_batch_settlement(
    settlement: &EvmSettlementConfig,
    transport: &RpcTransport,
    reported: u64,
) -> Result<Arc<EvmBatchSettlementBackend>, RuntimeError> {
    let table = settlement.table_name();
    // ADR 0086 decision 4: the table key is asserted against the chain
    // before anything is bound to it, and is never an input to the backend.
    if reported != settlement.chain_id() {
        return Err(RuntimeError::EvmChainIdMismatch {
            network: settlement.network().clone(),
            reported,
        });
    }
    let private_key = read_settlement_private_key(settlement.key())?;
    let backend = EvmBatchSettlementBackend::connect(
        transport,
        &private_key,
        ethers::types::Address::from(settlement.token_address()),
        settlement.decimals(),
        settlement.batch_settlement().min_withdraw_delay_secs(),
    )
    .await
    .map_err(|source| unbound(&table, source))?;
    check_published_deposit_terms(settlement, &backend).await?;
    Ok(Arc::new(backend))
}

/// Refuse a `[settlement.evm]` table whose published deposit terms the
/// token cannot honour (issue #1422). This node never deposits under them
/// -- its own deposits are routed by what the token answers -- so a wrong
/// value would fail only other payers, on chain, where nothing here sees
/// it.
///
/// * `asset_transfer_method = "eip3009"` on a token that fails
///   [`EvmBatchSettlementBackend::deposit_route`]'s ERC-3009 probe: the
///   same probe, so the published method and this node's own route are
///   one answer;
/// * under `eip3009`, an `asset_eip712_name` and `asset_eip712_version`
///   that do not reproduce the token's `DOMAIN_SEPARATOR()`. A token with
///   none is not judged. Under `permit2` the domain is not checked: a
///   Permit2 deposit is signed under Permit2's domain, and the asset's is
///   used only by a facilitator's optional EIP-2612 sponsoring, on a token
///   whose domain need not take this shape at all.
///
/// `permit2` itself needs no check: it moves any ERC-20. Permit2 sits
/// beneath x402's own `Permit2DepositCollector`, deployed with the
/// `x402BatchSettlement` that `connect` has already found on this chain,
/// and this node's own Permit2 deposits ask nothing more of it.
async fn check_published_deposit_terms(
    settlement: &EvmSettlementConfig,
    backend: &EvmBatchSettlementBackend,
) -> Result<(), RuntimeError> {
    let terms = settlement.batch_settlement();
    if terms.asset_transfer_method() != connector_domain::x402::X402AssetTransferMethod::Eip3009 {
        return Ok(());
    }
    let table = settlement.table_name();
    let token = format!(
        "{:#x}",
        ethers::types::Address::from(settlement.token_address())
    );
    let route = backend
        .deposit_route()
        .await
        .map_err(|source| unbound(&table, source))?;
    if route != DepositRoute::Erc3009 {
        return Err(RuntimeError::AssetTransferMethodUnsupported { table, token });
    }
    let matches = backend
        .asset_eip712_domain_matches(terms.asset_eip712_name(), terms.asset_eip712_version())
        .await
        .map_err(|source| unbound(&table, source))?;
    if matches == Some(false) {
        return Err(RuntimeError::AssetEip712DomainMismatch {
            table,
            token,
            name: terms.asset_eip712_name().to_string(),
            version: terms.asset_eip712_version().to_string(),
        });
    }
    Ok(())
}

/// The Solana twin of [`build_evm_batch_settlement`]: bound over the table's
/// one transport, under its settlement key as the sponsor (ADR 0074
/// decision 5), in its `token_address` mint; refuses, by name, a chain
/// without `payment-channels`.
///
/// Given the node's `socks_proxy`, if any, for its paying half's posts to a
/// counterparty's sponsor endpoint (ADR 0070, issue #1379): an onion
/// counterparty's sponsor is reached through it, every other one direct,
/// by `connector_config::is_onion_endpoint`. The settlement `rpc_url` is
/// not affected -- that is `transport`'s, under ADR 0073.
async fn build_solana_batch_settlement(
    settlement: &SolanaSettlementConfig,
    transport: &RpcTransport,
    socks_proxy: Option<&url::Url>,
) -> Result<Arc<SolanaBatchSettlement>, RuntimeError> {
    let table = SettlementChain::Solana.name();
    let batch = settlement.batch_settlement();
    let sponsor_seed = read_settlement_key_bytes(settlement.key())?;
    let mint = Pubkey::from_str(settlement.token_address()).map_err(|error| {
        RuntimeError::BatchSettlementUnusable {
            table: table.to_string(),
            source: BatchSettlementError::Backend(format!(
                "token_address '{}' is not a valid base58 Solana pubkey: {error}",
                settlement.token_address()
            )),
        }
    })?;
    let backend = SolanaBatchSettlement::connect(
        transport,
        &sponsor_seed,
        mint,
        settlement.decimals(),
        batch.min_grace_period_secs(),
        batch.min_sponsored_deposit(),
    )
    .await
    .map_err(|source| unbound(table, source))?;
    let backend = match socks_proxy {
        Some(proxy) => backend
            .with_socks_proxy(proxy)
            .map_err(|source| unbound(table, source))?,
        None => backend,
    };
    Ok(Arc::new(backend))
}

/// The key an `[settlement.xrpl.<id>]` table signs with: an XRPL family seed
/// in a file, secp256k1 or Ed25519 (ADR 0087 decision 2).
fn read_xrpl_signer(settlement: &XrplSettlementConfig) -> Result<XrplSigner, RuntimeError> {
    match settlement.key() {
        SecretLocation::File(path) => {
            XrplSigner::from_key_file(path).map_err(|error| RuntimeError::XrplKeyUnusable {
                table: settlement.table_name(),
                message: error.to_string(),
            })
        }
        SecretLocation::Kms { .. } => Err(RuntimeError::UnsupportedSettlementKeyLocation),
    }
}

/// ADR 0087 decision 4: one XRPL account is one network. Every table's
/// account is derived from its key -- no network is dialed -- and two tables
/// resolving to one account are refused by name, before either is bound.
fn check_xrpl_accounts_are_distinct(config: &Config) -> Result<(), RuntimeError> {
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for settlement in config.settlements() {
        let SettlementConfig::Xrpl(xrpl) = settlement else {
            continue;
        };
        let address = read_xrpl_signer(xrpl)?.classic_address();
        seen.entry(address).or_default().push(xrpl.table_name());
    }
    match seen.into_iter().find(|(_, tables)| tables.len() > 1) {
        Some((address, tables)) => Err(RuntimeError::XrplAccountOnTwoNetworks { tables, address }),
        None => Ok(()),
    }
}

/// This node's x402 settlement backend on one XRPL network (ADR 0087): the
/// receiving half, bound over the table's one transport (ADR 0073).
///
/// Refused by name, before the backend is built: an endpoint that cannot be
/// read, a `server_info.network_id` that is not the table's key, a settlement
/// account the ledger does not hold, one below the base reserve, and an
/// endpoint with no validated ledger to read that reserve from (decision 5). It never checks the PayChan amendment, which a testnet reports
/// disabled while its channels work.
async fn build_xrpl_batch_settlement(
    settlement: &XrplSettlementConfig,
    transport: &RpcTransport,
) -> Result<Arc<XrplBatchSettlement>, RuntimeError> {
    let table = settlement.table_name();
    let signer = read_xrpl_signer(settlement)?;
    let address = signer.classic_address();
    let unreadable =
        |error: connector_chain_rpc::xrpl::XrplRpcError| RuntimeError::BatchSettlementUnusable {
            table: table.clone(),
            source: BatchSettlementError::Backend(error.to_string()),
        };
    let rpc = connector_chain_rpc::xrpl::XrplRpc::new(transport.clone());
    let expected = settlement.network().network_id();
    let info = rpc.server_info().await.map_err(unreadable)?;
    // A node with no `network_id` is mainnet's (0).
    if info.network_id.unwrap_or(0) != expected {
        return Err(RuntimeError::XrplNetworkMismatch {
            table,
            expected,
            reported: info.network_id,
        });
    }
    let Some(account) = rpc.account_info(&address).await.map_err(unreadable)? else {
        return Err(RuntimeError::XrplAccountMissing { table, address });
    };
    let Some(reserve_drops) = info.reserve_base_drops else {
        return Err(RuntimeError::XrplReserveUnreported { table });
    };
    if account.balance_drops < reserve_drops {
        return Err(RuntimeError::XrplAccountUnderfunded {
            table,
            address,
            balance_drops: account.balance_drops,
            reserve_drops,
        });
    }
    let backend = XrplBatchSettlement::connect(
        transport.clone(),
        expected,
        signer,
        settlement.min_settle_delay_secs(),
    )
    .await
    .map_err(|source| unbound(&table, source))?
    .with_max_fee_drops(settlement.max_fee_drops());
    Ok(Arc::new(backend))
}

/// Refuse a `state_dir` whose claim journals hold `toon-channel` entries
/// (ADR 0075 decision 8, issue #1385), before anything is served.
///
/// Both files are read: `peer-claims.log`, which only an older build wrote
/// and nothing opens any more, and `client-edge-claims.log`, which holds
/// vouchers now and may hold a pre-ADR 0075 client's `toon-channel` claims
/// beside them. A file that does not exist holds nothing. One that cannot
/// be read is [`RuntimeError::JournalUnreplayable`], as it always was.
fn refuse_toon_channel_journals(state_dir: &Path) -> Result<(), RuntimeError> {
    for name in [PEER_CLAIM_JOURNAL, CLIENT_EDGE_JOURNAL] {
        let path = state_dir.join(name);
        if !path.exists() {
            continue;
        }
        let unreplayable = |source| RuntimeError::JournalUnreplayable {
            path: path.clone(),
            source,
        };
        let entries = FileJournal::open(&path)
            .and_then(|journal| journal.read_all())
            .map_err(unreplayable)?;
        if let Some(channel) = connector_client_edge::first_toon_channel_entry(&entries) {
            return Err(RuntimeError::ToonChannelJournal { path, channel });
        }
    }
    Ok(())
}

/// The peer claim journal an older build kept under `state_dir` (issue
/// #605): the `toon-channel` claims its peers paid it with. Nothing writes
/// or replays it since ADR 0075; it is read only so that one holding such
/// claims is refused at boot ([`refuse_toon_channel_journals`]).
const PEER_CLAIM_JOURNAL: &str = "peer-claims.log";
/// The client edge's claim journal: every voucher it accepted, and each
/// x402 channel it admitted.
const CLIENT_EDGE_JOURNAL: &str = "client-edge-claims.log";
/// The third book (ADR 0075 decision 8): the x402 channels this node pays
/// on -- each one's record, journaled before its opening transaction is
/// sent, and every voucher signed on it. A file of its own, because its own
/// owner, [`OutboundChannels`], replays it.
const OUTBOUND_CHANNEL_JOURNAL: &str = "outbound-channels.log";
/// Issue #884's runtime peer/route table -- a whole-table JSON snapshot,
/// not an append-only journal line format like the two above (see
/// `connector_runtime::PeerRouteStore`'s own docs for why).
const RUNTIME_PEER_ROUTE_TABLE: &str = "runtime-peers.json";
/// The file whose advisory lock says one process owns `state_dir` (issue
/// #1318). Empty and never written: the journals themselves are not locked,
/// so `FileJournal` and the table's write-then-rename work unchanged.
const STATE_DIR_LOCK: &str = ".state-dir.lock";

/// Take an exclusive OS advisory lock on `state_dir`, creating the directory
/// if it is not there yet. The operating system releases it when the process
/// exits for any reason, so a crash leaves nothing to clean up.
fn lock_state_dir(state_dir: &Path) -> Result<std::fs::File, RuntimeError> {
    let unusable = |source| RuntimeError::StateDirUnusable {
        path: state_dir.to_path_buf(),
        source,
    };
    std::fs::create_dir_all(state_dir).map_err(unusable)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state_dir.join(STATE_DIR_LOCK))
        .map_err(unusable)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(RuntimeError::StateDirHeld {
            path: state_dir.to_path_buf(),
        }),
        Err(std::fs::TryLockError::Error(source)) => Err(unusable(source)),
    }
}

/// Open `name` under this node's configured `state_dir`, creating the
/// directory if it is not there yet.
///
/// Opening happens at startup, before anything is served, precisely so a
/// node with nowhere writable fails here -- with the path in the message --
/// rather than at the first claim, hours later, on a packet path where the
/// only honest answer left is to refuse the claim (issue #605).
fn open_journal(state_dir: &Path, name: &str) -> Result<Arc<dyn Journal>, RuntimeError> {
    std::fs::create_dir_all(state_dir).map_err(|source| RuntimeError::StateDirUnusable {
        path: state_dir.to_path_buf(),
        source,
    })?;
    let path = state_dir.join(name);
    let journal = FileJournal::open(&path).map_err(|source| match source {
        JournalError::Io(source) => RuntimeError::StateDirUnusable {
            path: path.clone(),
            source,
        },
        source => RuntimeError::JournalUnreplayable {
            path: path.clone(),
            source,
        },
    })?;
    Ok(Arc::new(journal))
}

/// Name every plaintext peering at startup, loudly (issue #678, gap 3).
///
/// `peer_allow_plaintext_endpoints` is a loopback-and-test opt-in and
/// nothing else: a peering carries signed balance proofs (ADR 0004), so
/// `ws://` and `http://` remain a hard `PeerEndpointScheme` load error on
/// every config that does not set it. A node that *did* set it is one whose
/// peer claims cross the wire in the clear, and the only thing worse than
/// that in a test harness is that in production with nobody noticing.
fn warn_about_plaintext_peerings(config: &Config) {
    for (peer_id, endpoint) in config.plaintext_peerings() {
        tracing::warn!(
            peer_id,
            %endpoint,
            "peer_allow_plaintext_endpoints is set and this peering is dialed in the clear -- \
             every claim on it is readable on the wire. This is a loopback and test setting; \
             see docs/operators/btp-peer-transport-bringup.md"
        );
    }
}

/// Name every published `[node]` endpoint that only a payer on this host or
/// docker network can dial (issue #1533). A warning, never a refusal: the same
/// values are right in tests, `local/` topologies and a same-host trial.
fn warn_about_host_local_endpoints(config: &Config) {
    let Some(node) = config.node() else { return };
    for (field, value) in node.host_local_endpoints() {
        tracing::warn!(
            field,
            value,
            "[node] {field} names a host no payer outside this machine can reach, and it is \
             published as written in this node's self-description. Unless every payer is on \
             this host or docker network, publish the URL a payer outside this host dials: \
             the public https:// (or wss://) URL in front of the node"
        );
    }
}

/// Everything [`build`] produced from a validated [`Config`], and
/// everything [`router`] needs from it. A struct rather than a tuple
/// because the third member is the kind of thing that only ever grows: as
/// issue #556 arms more of the node, more of what `build` connects has to
/// reach the routers without being reconstructed (and, for a chain
/// connection, without being connected twice).
pub struct Runtime {
    pub connector: Arc<Connector>,
    pub signer: Arc<dyn Signer>,
    /// One entry per chain this node has opted into accepting an x402
    /// `batch-settlement` channel on (ADR 0074 decision 8, issue #1345),
    /// composed from the connected backend. Empty on a node whose
    /// settlement tables write no `batch_settlement` sub-table -- a node that
    /// then takes no claim at all, since every claim is a voucher (ADR 0075,
    /// issue #1384). The self-description's `settlements` -- the
    /// `toon-channel` terms composed beside this list -- is gone with that
    /// scheme.
    pub batch_settlements: Vec<connector_client_edge::X402BatchSettlementTerms>,
    /// The key this node signs its vouchers with on each chain it pays x402
    /// on (ADR 0075 decisions 3 and 10), published so a peer can bind this
    /// node's channel toward it: the EVM settlement address, which every
    /// outbound EVM channel names as `payer` and `payerAuthorizer`, and the
    /// Solana settlement key. Read off the backends this node connected,
    /// alongside `batch_settlements`, never declared.
    pub voucher_signers: Vec<connector_domain::VoucherSignerFact>,
    /// The rates this node deals at, or `None` for a node that declares no
    /// token to deal (ADR 0071 decision 6, issue #1294) -- which is every
    /// node that predates the record, and is why this is an `Option` rather
    /// than an empty table.
    ///
    /// Built here, from the immutable declaration in `[[tokens]]`,
    /// `[[rates]]` and `[rate_guards]`, for this struct's own reason: it is
    /// composed once at boot and every reader afterwards shares it rather
    /// than rebuilding one of its own. Two readers are expected --
    /// the forwarding path, which reads it for every forward that crosses a
    /// denomination boundary, and the operator surface, which renders what
    /// it holds -- and both go through
    /// [`connector_runtime::SharedRateTable`]'s synchronous read API, which
    /// is what keeps ADR 0071's "the forwarding path does no I/O" true by
    /// construction rather than by discipline.
    ///
    /// Writing it is [`spawn_rate_pollers`]'s job, and nothing else's.
    pub rate_table: Option<SharedRateTable>,
    /// This node's x402 `batch-settlement` backend on each EVM network (ADR
    /// 0074, ADR 0086), one per `[settlement.eip155.<chainId>]` table, keyed
    /// by its CAIP-2 network. [`router`] hands them to the client edge's
    /// claim gate, which admits each voucher through its own network's;
    /// every channel the client-edge journal holds vouchers on is already
    /// restored to its network's by the time [`build`] returns, so
    /// `channel_state` and `land` work from the first packet. [`router`]
    /// also starts a watcher and sweep over each (issue #1344,
    /// `spawn_batch_settlement_watchers`).
    pub batch_settlement_evm: BTreeMap<Network, Arc<EvmBatchSettlementBackend>>,
    /// The Solana twin of [`Self::batch_settlement_evm`], `Some` exactly
    /// when `[settlement.solana]` is written -- and what
    /// the public sponsor endpoint (issue #1346) co-signs an `open` with.
    pub batch_settlement_solana: Option<Arc<SolanaBatchSettlement>>,
    /// The XRPL twin of [`Self::batch_settlement_evm`] (ADR 0087): the
    /// receiving half over each `[settlement.xrpl.<id>]` table, keyed by its
    /// CAIP-2 network. No paying half is built on XRPL yet, so none is among
    /// the outbound channels' payers.
    pub batch_settlement_xrpl: BTreeMap<Network, Arc<XrplBatchSettlement>>,
    /// The x402 channels this node pays on (ADR 0075 decisions 8 and 11),
    /// over the paying half of the backends above and journaled to
    /// [`OUTBOUND_CHANNEL_JOURNAL`]: `Some` exactly when either backend is.
    /// Every channel that journal names is restored by the time [`build`]
    /// returns, with the highest voucher journaled on it, so the operator
    /// surface can fund, withdraw from and sign on it from the first
    /// request.
    pub outbound_channels: Option<Arc<OutboundChannels>>,
    /// The lock on `state_dir` (issue #1318), `Some` exactly when the node
    /// names one. `run` moves it into the `RunningNode` it hands back, because
    /// the runtime is dropped before the process stops serving.
    pub state_dir_lock: Option<std::fs::File>,
}

/// Construct the live [`Connector`] and [`Signer`] a validated [`Config`]
/// describes. Every `peer_id`-targeted `[[routes]]` entry becomes a
/// [`PeerRoute`] alongside the terminated
/// [`connector_config::StaticRoute`]s -- though nothing can currently
/// traverse one: ADR 0027 / issue #679 deleted the raw-TCP transport that
/// was the only [`connector_runtime::PeerTransport`] a built node held,
/// and the carriages replacing it (BTP over `wss://`, ILP-over-HTTP over
/// `https://`) are issue #676. Until one lands this node holds an empty
/// [`InProcessPeerTransport`], so a packet routed to a peer is answered
/// `T01 peer unreachable` rather than dropped. Every configured
/// `[settlement.<chain>]` table's real chain-backed [`SettlementBackend`]
/// is connected and attached under its own chain via
/// [`Connector::with_settlement`] (issues #542, #630 -- a node with both
/// tables holds both backends, and operator channel ops route per chain) --
/// those connections are why this function is `async` at all; an
/// unconfigured node still builds with no settlement backend, same as
/// before this section existed.
///
/// A node that names a `state_dir` also has its peer claim journal armed
/// here (issue #605), so the watermarks `ClaimBook` keeps outlive the
/// process exactly as the client edge's do. That ledger sits *above* the
/// peer transport port and was untouched by #679's deletion.
pub async fn build(config: &Config) -> Result<Runtime, RuntimeError> {
    // Issue #1318: the first thing done with `state_dir`, before any journal
    // is read or any chain dialed.
    let state_dir_lock = config.state_dir().map(lock_state_dir).transpose()?;
    let signer = build_signer(config.signer_key())?;
    // ADR 0075 decision 8, issue #1385: a node whose journals still hold
    // claims on TOON's own channels is refused by name before any chain is
    // dialed -- this build could neither land nor drain them.
    if let Some(state_dir) = config.state_dir() {
        refuse_toon_channel_journals(state_dir)?;
    }
    // Issue #678 gap 3, said once and loudly. A plaintext peering could not
    // have loaded unless somebody wrote `peer_allow_plaintext_endpoints`,
    // and the whole point of a loopback-and-test opt-in is that a node that
    // took it says so where an operator will see it.
    warn_about_plaintext_peerings(config);
    warn_about_host_local_endpoints(config);
    // Issue #678 gap 2: the dial side, built from `[[peers]]`. A node with
    // no dialable peering still holds an empty `InProcessPeerTransport`, so
    // a packet routed to a peer is answered `T01 peer unreachable` rather
    // than silently dropped.
    let peer_transport = peer_transport::build_peer_transport(config);
    let peer_routes = config
        .peer_routes()
        .iter()
        // ADR 0028: a forwarded route carries the client-edge `price` its
        // config entry names. What this hop retains of it is the peering's
        // own fee, wired below from `[[peers]]` (ADR 0061). Built with
        // `new_scheduled` rather than `new` so a route that loses its price
        // on the way into the runtime is a compile error, not a silently free
        // gateway -- and it carries the whole schedule (ADR 0065), so a
        // forwarded route's slope survives the trip into the runtime the way
        // its base always has.
        .map(|route| {
            PeerRoute::new_scheduled(route.prefix(), route.peer_id(), route.price())
                .with_request(route.request().cloned())
                .with_max_bytes(route.max_bytes())
        })
        .collect();
    let mut connector = Connector::new(
        config.routes().to_vec(),
        peer_routes,
        Arc::new(HttpAppClient::new()),
        Arc::clone(&peer_transport) as Arc<dyn PeerTransport>,
        Arc::new(SystemClock),
    )
    .with_identity_signer(signer.clone())
    // ADR 0058: the same transport, in its other role. `POST /peers`
    // establishes a peering and registers its carriage here, and
    // `DELETE /peers/:id` takes it away -- neither waits for a restart,
    // which is the whole of what made a runtime peer row a name with
    // nothing behind it.
    .with_peer_registrar(Arc::clone(&peer_transport) as Arc<dyn PeerRegistrar>)
    // The one outbound request this connector makes to an
    // operator-supplied host, and it is bounded (ADR 0058): a whole-
    // exchange timeout, a body cap enforced as the body streams, and no
    // redirect followed. Never reached from the packet path.
    // ADR 0070 decision 3: an onion counterparty's self-description is read
    // through the same one proxy the carriages dial through, selected by the
    // same host rule. Without it `POST /peers` could not establish a peering
    // with a node whose only published URL is a `.onion` one -- the write
    // reads that URL before any carriage is chosen.
    .with_self_description_source(Arc::new(BoundedHttpSelfDescription::new(
        config.peer_allow_plaintext_endpoints(),
        config.socks_proxy(),
    )))
    // ...and a runtime peering's claim-state ask, where the peer's client
    // edge is an onion host, by the same rule (issue #1379): an onion peer's
    // watermark is restored over the circuit its packets ride.
    .with_socks_proxy(config.socks_proxy().cloned())
    // The same node-wide opt-in a `[[peers]]` endpoint takes (issue #678
    // gap 3), so a peering established at runtime picks its carriage by
    // exactly the rule a config-file peering does.
    .with_peer_allow_plaintext_endpoints(config.peer_allow_plaintext_endpoints())
    // ADR 0077: the operator's bounded, in-memory packet history, off unless
    // `[operator] packet_history` names a size.
    .with_packet_history(
        config
            .operator()
            .map_or(0, |operator| operator.packet_history()),
    )
    // Issue #884: the routing table IS the relationship set enforced at
    // load (`connector-config`'s `UnknownPeerId` check), so a runtime
    // write must never be able to add, update or remove a peer id the
    // config file already owns. `Connector` needs every config peer id
    // to enforce that, even though it stores nothing else about a
    // config peer (see `PeerView`'s own docs).
    .with_config_peer_ids(config.peers().iter().map(|peer| peer.id().to_string()))
    // ADR 0042's cap: the largest amount this node will forward to each
    // peer in ONE packet, straight off its `[[peers]]` row. Defaulted at
    // the config layer (`connector_config::DEFAULT_MAX_PACKET_AMOUNT`), so
    // a row that writes nothing still arrives here bounded -- and a peer
    // this call never names (one added at runtime over the operator
    // surface, issue #884) is bounded by the same figure inside
    // `Connector` itself.
    .with_peer_packet_caps(
        config
            .peers()
            .iter()
            .map(|peer| (peer.id().to_string(), peer.max_packet_amount())),
    )
    // ADR 0010's flat per-packet fee, straight off each `[[peers]]` row
    // (ADR 0061): what this node retains for carrying one packet to that
    // counterparty, whichever prefix it was addressed to. Defaulted to zero
    // at the config layer, so a row that writes nothing carries for free --
    // and so does a peer this call never names, one added at runtime over
    // the operator surface with a fee on its own row instead.
    .with_peer_fees(
        config
            .peers()
            .iter()
            .map(|peer| (peer.id().to_string(), peer.fee())),
    );
    // ADR 0074 decision 8, issue #1345: this node's x402 batch-settlement
    // facts, one entry per chain whose settlement table opted in, off the
    // connected backend -- the greeting's whole `accepts[]` (ADR 0075
    // decision 10).
    let mut batch_settlements: Vec<connector_client_edge::X402BatchSettlementTerms> = Vec::new();
    let mut voucher_signers: Vec<connector_domain::VoucherSignerFact> = Vec::new();
    let mut batch_settlement_evm: BTreeMap<Network, Arc<EvmBatchSettlementBackend>> =
        BTreeMap::new();
    let mut batch_settlement_solana: Option<Arc<SolanaBatchSettlement>> = None;
    let mut batch_settlement_xrpl: BTreeMap<Network, Arc<XrplBatchSettlement>> = BTreeMap::new();
    // ADR 0087 decision 4, before any ledger is dialed: one account is one
    // network, and two tables on one are refused by name.
    check_xrpl_accounts_are_distinct(config)?;
    // Each table's one transport, built once and handed to every client of
    // its `rpc_url` below (ADR 0073 decision 2).
    let transports = settlement_transports(config)?;
    // ADR 0086 decision 4: every EVM table's chain id, read before any is
    // bound, so two tables on one chain are refused as that.
    let reported = evm_reported_chain_ids(config, &transports).await?;
    for settlement in config.settlements() {
        match settlement {
            SettlementConfig::Evm(evm) => {
                let transport = transports.for_settlement(settlement)?;
                // ADR 0075 decision 1: bound before anything is served, and
                // refused by name on a chain without `x402BatchSettlement`.
                let backend =
                    build_evm_batch_settlement(evm, transport, reported[evm.network()]).await?;
                let batch = evm.batch_settlement();
                let network = format!("eip155:{}", backend.domain().chain_id);
                // The connection proved the settlement address and the live
                // chain id; the token comes from the config line `connect`
                // just verified against that chain (issue #564).
                let settlement_address = format!("{:#x}", backend.own_address());
                let token_address =
                    format!("{:#x}", ethers::types::Address::from(evm.token_address()));
                // ADR 0075 decisions 3 and 10: the settlement key signs this
                // node's vouchers (`payerAuthorizer == payer`), so the
                // settlement address is the voucher signer a peer binds this
                // node's channel toward it by.
                voucher_signers.push(connector_domain::VoucherSignerFact {
                    network: network.clone(),
                    signer: settlement_address.clone(),
                });
                // ADR 0074 decision 8, issue #1345: this chain's
                // batch-settlement facts, the greeting's `accepts[]` entry.
                batch_settlements.push(connector_client_edge::X402BatchSettlementTerms::Evm(
                    connector_client_edge::X402BatchSettlementEvmTerms {
                        network,
                        asset: token_address,
                        pay_to: settlement_address.clone(),
                        receiver_authorizer: settlement_address,
                        min_withdraw_delay_secs: batch.min_withdraw_delay_secs(),
                        name: batch.asset_eip712_name().to_string(),
                        version: batch.asset_eip712_version().to_string(),
                        // toon-client#695: configured, like `name` and
                        // `version`; this node never calls the facilitator.
                        asset_transfer_method: batch.asset_transfer_method(),
                        facilitator: batch.facilitator_url().map(str::to_string),
                    },
                ));
                batch_settlement_evm.insert(evm.network().clone(), backend);
            }
            SettlementConfig::Solana(solana) => {
                let transport = transports.for_settlement(settlement)?;
                // ADR 0075 decision 1: bound before anything is served, and
                // refused by name on a chain without `payment-channels`;
                // the mint must be the SPL Token program's and agree with
                // `decimals`.
                let backend =
                    build_solana_batch_settlement(solana, transport, config.socks_proxy()).await?;
                // ADR 0074 decision 8, issue #1345: `payTo` and `feePayer`
                // are both the sponsor key (decision 5's
                // sponsor-is-the-receiving-operator rule), and `network` is
                // read off the chain's own genesis hash, never guessed from
                // the RPC URL. The two minimums are the backend's own: what
                // it admits by and what its sponsor co-signs above are what
                // is published. Issue #1357 adds x402's required
                // `tokenProgram` and where the sponsor is served.
                let sponsor = backend.sponsor().to_string();
                // ADR 0075 decisions 3 and 10: the Solana settlement key is
                // `authorized_signer` on every channel this node opens, and
                // what a peer binds that channel by (#1379).
                voucher_signers.push(connector_domain::VoucherSignerFact {
                    network: backend.caip2_network(),
                    signer: sponsor.clone(),
                });
                batch_settlements.push(connector_client_edge::X402BatchSettlementTerms::Solana(
                    connector_client_edge::X402BatchSettlementSolanaTerms {
                        network: backend.caip2_network(),
                        asset: backend.mint().to_string(),
                        pay_to: sponsor.clone(),
                        fee_payer: sponsor,
                        min_grace_period_secs: backend.min_grace_period_secs(),
                        token_program: backend.token_program().to_string(),
                        min_deposit: backend.min_sponsored_deposit().to_string(),
                        // The path `sponsor::router` mounts, from the one
                        // constant, so the greeting can never name a path
                        // nothing serves.
                        sponsor_endpoint: crate::sponsor::SPONSOR_PATH.to_string(),
                    },
                ));
                batch_settlement_solana = Some(backend);
            }
            SettlementConfig::Xrpl(xrpl) => {
                let transport = transports.for_settlement(settlement)?;
                // ADR 0087 decision 5: the network, the account and its
                // reserve are checked by name before the backend is bound.
                let backend = build_xrpl_batch_settlement(xrpl, transport).await?;
                let network = xrpl.network().caip2().to_string();
                // ADR 0087 decision 9: the channel key is the settlement
                // account's own public key -- the one a payer's channel
                // names as its destination is the address derived from it.
                voucher_signers.push(connector_domain::VoucherSignerFact {
                    network: network.clone(),
                    signer: backend.public_key_hex(),
                });
                batch_settlements.push(connector_client_edge::X402BatchSettlementTerms::Xrpl(
                    connector_client_edge::X402BatchSettlementXrplTerms {
                        network,
                        asset: xrpl.asset().to_string(),
                        pay_to: backend.address().to_string(),
                        binding: connector_domain::x402::XRPL_PAYCHAN_BINDING.to_string(),
                        min_settle_delay_secs: xrpl.min_settle_delay_secs(),
                        redemption_safety_secs: xrpl.redemption_safety_secs(),
                    },
                ));
                batch_settlement_xrpl.insert(Network::Xrpl(xrpl.network()), backend);
            }
        }
    }
    // ADR 0075 decisions 4 and 8: the x402 channels this node pays on,
    // restored from their journal before the runtime peer table below is
    // replayed, because a durable x402 peering is rehydrated onto them --
    // one replayed first would have nothing to sign its forwards on.
    let outbound_channels = restore_outbound_channels(
        config,
        &batch_settlement_evm,
        &batch_settlement_solana,
        &batch_settlement_xrpl,
    )
    .await?;
    if let Some(outbound) = &outbound_channels {
        let networks = batch_settlements
            .iter()
            .map(|terms| match terms {
                connector_client_edge::X402BatchSettlementTerms::Evm(evm) => {
                    (SettlementChain::Evm, evm.network.clone())
                }
                connector_client_edge::X402BatchSettlementTerms::Solana(solana) => {
                    (SettlementChain::Solana, solana.network.clone())
                }
                connector_client_edge::X402BatchSettlementTerms::Xrpl(xrpl) => {
                    (SettlementChain::Xrpl, xrpl.network.clone())
                }
            })
            .collect();
        connector = connector.with_outbound_channels(Arc::clone(outbound), networks);
    }
    // ADR 0075 decisions 4, 6 and 9, issue #1380: the config file's own
    // peerings, on x402 channels. After the outbound channels above, which a
    // `[[pay_channels]]` row names one of; before the runtime table below,
    // so a durable runtime row cannot take a signer a config row binds.
    connector = wire_config_peerings(connector, config)?;
    if let Some(state_dir) = config.state_dir() {
        // Issue #884: replay this node's durable runtime peer/route table,
        // and arm the connector to persist future writes back to the same
        // file -- the same `state_dir` scoping as the journals, so an
        // operator restoring a node from `state_dir` alone restores this
        // table too. `create_dir_all` first, so a node with nowhere
        // writable fails here with the path in the message.
        std::fs::create_dir_all(state_dir).map_err(|source| RuntimeError::StateDirUnusable {
            path: state_dir.to_path_buf(),
            source,
        })?;
        let table_path = state_dir.join(RUNTIME_PEER_ROUTE_TABLE);
        let (store, runtime_peers, runtime_peer_routes) = PeerRouteStore::open(&table_path)
            .map_err(|source| RuntimeError::RuntimePeerRouteTableUnusable {
                path: table_path,
                source,
            })?;
        connector =
            connector.with_runtime_peer_route_store(store, runtime_peers, runtime_peer_routes);
    }
    // ADR 0071 decision 6, issue #1294: the table a converting forward
    // reads, built from what the file declares and from nothing else. A node
    // that declares no token gets `None` here, starts no poller and forwards
    // exactly as it did before the record -- no branch below this one is
    // reached at all.
    let rate_table = SharedRateTable::from_config(config.denomination());
    if let Some(table) = &rate_table {
        let held = table.read();
        tracing::info!(
            numeraire = %held.numeraire(),
            declared_pairs = held.declared_pairs().count(),
            quoted_tokens = config.denomination().quoted_tokens().count(),
            "dealing across denominations at declared rates (ADR 0071)"
        );
        // Every declared quote path, connected to the reader that can
        // actually read it and left polling in the background. A node whose
        // pairs are all static `[[rates]]` rows starts nothing here.
        spawn_quote_path_pollers(table, config, &transports)?;
    }
    // ADR 0071 decisions 1 and 2, issues #1295 and #1301: the facts the
    // forwarding path's converting arm reads, and the only ones. Which
    // token each leg holds decides whether a forward crosses a
    // denomination boundary at all, and the table decides what it crosses
    // at -- or that it refuses.
    //
    // BOTH asset tables go on, always together. A packet can arrive over a
    // peering or over a client channel, both are denominated, and a node
    // given only one of the two would convert one kind of arrival and carry
    // the other across the same boundary at an implied 1:1 -- which is the
    // 10^12 error rather than a partial rollout of the fix. They are one
    // wiring step for that reason, even though they are two tables.
    //
    // The rate table is wired separately because it is independently
    // absent, and the combination that matters is the awkward one: a node
    // that declares tokens but no rate row has asset tables and no rate
    // table, resolves boundaries it has priced nothing for, and must refuse
    // every crossing rather than pass an integer across a scale difference.
    // Handing the table only when both exist would turn that refusal back
    // into the silent pass-through ADR 0071 exists to make impossible.
    connector = connector
        .with_peering_assets(config.peering_assets().clone())
        .with_client_channel_assets(config.client_channel_assets().clone());
    if let Some(table) = &rate_table {
        connector = connector.with_rate_table(table.clone());
    }
    // ADR 0080 decision 6: an allowance-paid delivery states the vault's
    // Mina network as its chain, and no payer.
    if let Some(zk_note) = config.zk_note() {
        connector = connector.with_zk_note_network(zk_note.network().caip2());
    }
    // ADR 0074: every batch-settlement channel the client edge's journal
    // holds vouchers on, restored before anything is served, so the port
    // can land them after a restart -- restored, not re-admitted, so a rule
    // tightened since cannot strand a voucher already accepted (decision 5).
    // The journal is where an EVM channel's config survives a restart; the
    // chain never gives it back.
    if !batch_settlement_evm.is_empty()
        || batch_settlement_solana.is_some()
        || !batch_settlement_xrpl.is_empty()
    {
        if let Some(state_dir) = config.state_dir() {
            let path = state_dir.join(CLIENT_EDGE_JOURNAL);
            let unreplayable = |source| RuntimeError::JournalUnreplayable {
                path: path.clone(),
                source,
            };
            let entries = open_journal(state_dir, CLIENT_EDGE_JOURNAL)?
                .read_all()
                .map_err(unreplayable)?;
            let channels =
                connector_client_edge::journaled_batch_channels(&entries).map_err(unreplayable)?;
            restore_journaled_channels(
                &channels,
                &receivers(
                    &batch_settlement_evm,
                    &batch_settlement_solana,
                    &batch_settlement_xrpl,
                ),
            )
            .await;
        }
    }
    let connector = Arc::new(connector);
    Ok(Runtime {
        connector,
        signer,
        batch_settlements,
        voucher_signers,
        rate_table,
        batch_settlement_evm,
        batch_settlement_solana,
        batch_settlement_xrpl,
        outbound_channels,
        state_dir_lock,
    })
}

/// Wire the config file's peerings onto x402 channels (ADR 0075 decisions 4,
/// 6 and 9, issue #1380).
///
/// * Each `[[peer_channels]]` row binds its **voucher signer** to its
///   peering -- on its one `inbound_channel`, when the row names one -- so a
///   voucher on a channel the chain records that signer for, or the
///   claim-state challenge it signs for a packet that moves no value,
///   proves the peer role on either carriage (decision 5). The peer's
///   channel itself is admitted by the claim gate exactly as a client's is.
/// * Each `[[pay_channels]]` row registers this node's own **outbound x402
///   channel** as the one every forward to that peering is covered on, with
///   the next hop's `POST /ilp/claim-state` as the watermark authority on
///   restore (decision 6). The channel must be one this node's
///   outbound-channel journal holds: a row naming anything else is refused
///   here, by name, rather than refusing every forward at packet time.
///
/// The claim-state ask takes the peering's own `peer_answer_timeout_ms`,
/// and leaves on `socks_proxy` when the next hop's client edge is an onion
/// host (`Connector::with_socks_proxy`, set before this runs).
fn wire_config_peerings(
    mut connector: Connector,
    config: &Config,
) -> Result<Connector, RuntimeError> {
    for row in config.peer_channels() {
        connector = connector
            .with_config_voucher_signer(
                row.peer_id(),
                row.chain(),
                row.voucher_signer(),
                row.inbound_channel(),
                row.network(),
            )
            .map_err(RuntimeError::ConfigPeering)?;
        tracing::info!(
            peer_id = row.peer_id(),
            chain = ?row.chain(),
            voucher_signer = row.voucher_signer(),
            inbound_channel = row.inbound_channel().unwrap_or("any"),
            "a voucher by this signer proves this config peering (ADR 0075)"
        );
    }
    for row in config.pay_channels() {
        // Every row names a configured peering -- `Config::load` refuses a
        // `PayChannelOrphaned` one -- so this lookup cannot miss.
        let answer_timeout = config
            .peers()
            .iter()
            .find(|peer| peer.id() == row.peer_id())
            .expect("Config::load refuses a [[pay_channels]] row naming no [[peers]] entry")
            .peer_answer_timeout_ms();
        connector = connector
            .with_config_pay_channel(
                row.peer_id(),
                row.outbound_channel(),
                row.client_edge_url(),
                std::time::Duration::from_millis(answer_timeout),
                row.network(),
            )
            .map_err(RuntimeError::ConfigPeering)?;
        tracing::info!(
            peer_id = row.peer_id(),
            channel = row.outbound_channel(),
            client_edge = %row.client_edge_url(),
            "covering every PREPARE forwarded to this peer with a voucher on this node's own \
             outbound x402 channel (ADR 0042, ADR 0075)"
        );
    }
    Ok(connector)
}

/// The x402 channels this node pays on, restored from
/// [`OUTBOUND_CHANNEL_JOURNAL`] to the paying half of each batch-settlement
/// backend (ADR 0075 decision 8), or `None` when this node has none. A
/// journal that will not replay stops the node, as the claim books' do: a
/// channel it cannot read back is one holding this node's deposit that it
/// would otherwise forget. A node with no `state_dir` keeps the record in
/// memory, which survives nothing -- the same posture its claim books take.
async fn restore_outbound_channels(
    config: &Config,
    evm: &BTreeMap<Network, Arc<EvmBatchSettlementBackend>>,
    solana: &Option<Arc<SolanaBatchSettlement>>,
    xrpl: &BTreeMap<Network, Arc<XrplBatchSettlement>>,
) -> Result<Option<Arc<OutboundChannels>>, RuntimeError> {
    // ADR 0086: one payer per network, each EVM table's own.
    let mut payers: Vec<(Network, Arc<dyn BatchSettlementPayer>)> = evm
        .iter()
        .map(|(network, backend)| {
            (
                network.clone(),
                Arc::clone(backend) as Arc<dyn BatchSettlementPayer>,
            )
        })
        .collect();
    if let Some(solana) = solana {
        payers.push((
            solana_network_of(solana),
            Arc::clone(solana) as Arc<dyn BatchSettlementPayer>,
        ));
    }
    // An XRPL table has no paying half yet (ADR 0087), so it adds no payer
    // -- but a node that settles on XRPL alone still has the channels the
    // operator surface lists, which hang off this record, and a journal that
    // holds an outbound channel it can no longer pay from is refused by name
    // rather than forgotten.
    if payers.is_empty() && xrpl.is_empty() {
        return Ok(None);
    }
    let (journal, path): (Arc<dyn Journal>, PathBuf) = match config.state_dir() {
        Some(state_dir) => (
            open_journal(state_dir, OUTBOUND_CHANNEL_JOURNAL)?,
            state_dir.join(OUTBOUND_CHANNEL_JOURNAL),
        ),
        None => (
            Arc::new(connector_runtime::InMemoryJournal::new()),
            PathBuf::from(OUTBOUND_CHANNEL_JOURNAL),
        ),
    };
    let outbound =
        OutboundChannels::restore(journal, payers)
            .await
            .map_err(|error| match error {
                OutboundRestoreError::Journal(source) => {
                    RuntimeError::JournalUnreplayable { path, source }
                }
                OutboundRestoreError::OnNoNetwork {
                    channel_id,
                    network,
                } => RuntimeError::OutboundChannelOnNoNetwork {
                    channel_id,
                    network,
                },
                OutboundRestoreError::OnSeveralNetworks {
                    channel_id,
                    networks,
                } => RuntimeError::OutboundChannelOnSeveralNetworks {
                    channel_id,
                    networks,
                },
            })?;
    Ok(Some(Arc::new(outbound)))
}

/// Every x402 channel of this node's, both ways, as the operator surface
/// drives them (ADR 0075 decision 11): the outbound ones [`build`]
/// restored, the receiving half of each backend, the claim gate's held
/// vouchers, and this node's own network on each chain, which a
/// counterparty's terms must match.
fn batch_channels_for_operator(
    runtime: &Runtime,
    claim_gate: &Arc<ClientClaimGate>,
) -> Option<Arc<BatchChannels>> {
    let outbound = Arc::clone(runtime.outbound_channels.as_ref()?);
    Some(Arc::new(BatchChannels::new(
        outbound,
        runtime_receivers(runtime),
        Arc::new(ClaimGateVouchers(Arc::clone(claim_gate))),
    )))
}

/// The receiving half of every backend this node connected, keyed by its
/// CAIP-2 network (ADR 0086, ADR 0087): each EVM table's, the Solana one,
/// then each XRPL table's.
fn receivers(
    evm: &BTreeMap<Network, Arc<EvmBatchSettlementBackend>>,
    solana: &Option<Arc<SolanaBatchSettlement>>,
    xrpl: &BTreeMap<Network, Arc<XrplBatchSettlement>>,
) -> Vec<(Network, Arc<dyn BatchSettlementBackend>)> {
    let mut receivers: Vec<(Network, Arc<dyn BatchSettlementBackend>)> = evm
        .iter()
        .map(|(network, backend)| {
            (
                network.clone(),
                Arc::clone(backend) as Arc<dyn BatchSettlementBackend>,
            )
        })
        .collect();
    if let Some(solana) = solana {
        receivers.push((
            solana_network_of(solana),
            Arc::clone(solana) as Arc<dyn BatchSettlementBackend>,
        ));
    }
    receivers.extend(xrpl.iter().map(|(network, backend)| {
        (
            network.clone(),
            Arc::clone(backend) as Arc<dyn BatchSettlementBackend>,
        )
    }));
    receivers
}

fn runtime_receivers(runtime: &Runtime) -> Vec<(Network, Arc<dyn BatchSettlementBackend>)> {
    receivers(
        &runtime.batch_settlement_evm,
        &runtime.batch_settlement_solana,
        &runtime.batch_settlement_xrpl,
    )
}

/// The CAIP-2 network a connected Solana backend read off its cluster.
fn solana_network_of(backend: &SolanaBatchSettlement) -> Network {
    backend
        .caip2_network()
        .parse()
        .expect("a connected Solana backend names a CAIP-2 network")
}

/// The claim gate's view of this node's batch-settlement backends (ADR
/// 0074), or `None` when no chain has opted in -- and a gate given `None`
/// refuses every voucher by name.
fn batch_settlement_channels(runtime: &Runtime) -> Option<BatchSettlementChannelsAdapter> {
    BatchSettlementChannelsAdapter::new(
        runtime
            .batch_settlement_evm
            .values()
            .map(|backend| {
                (
                    Arc::clone(backend) as Arc<dyn BatchSettlementBackend>,
                    backend.domain(),
                )
            })
            .collect(),
        runtime.batch_settlement_solana.as_ref().map(|backend| {
            (
                Arc::clone(backend) as Arc<dyn BatchSettlementBackend>,
                solana_network_of(backend),
            )
        }),
        runtime
            .batch_settlement_xrpl
            .iter()
            .filter_map(|(network, backend)| match network {
                Network::Xrpl(xrpl) => Some((
                    Arc::clone(backend) as Arc<dyn BatchSettlementBackend>,
                    *xrpl,
                )),
                _ => None,
            })
            .collect(),
    )
}

/// Start one background poller per declared quote path, each refreshing its
/// pair into `table` at the cadence that pair's `ttl` implies (ADR 0071
/// decision 6, issue #1294). Returns how many were started.
///
/// Spawned, never awaited: a packet must never wait on a rate source, and
/// startup must never wait on a chain -- the same shape the batch-settlement
/// watchers run in. A node whose tokens declare
/// no quote path starts nothing and says nothing; its pairs are the static
/// `[[rates]]` rows the operator tends by hand.
///
/// Separate from [`build`] because the two halves arrive separately: the
/// table is built from config alone, while a source is a chain reader
/// (issue #1293) constructed from a settlement table's own RPC endpoint.
/// This is the one place the two meet, and it takes the port rather than any
/// implementation of it -- which is what lets the same wiring serve the EVM
/// reader, a Solana one after it, and the in-memory source the tests drive.
///
/// `sources` is a map from chain to reader rather than one reader, because
/// ADR 0071 decision 3 puts a token's quote pools on that token's own
/// settlement chain (issue #1302): a token whose chain nothing in `sources`
/// reads refuses startup here, rather than starting a poller whose every
/// tick fails and whose pair goes dark one `ttl` later.
pub fn spawn_rate_pollers(
    table: &SharedRateTable,
    sources: &RateSources,
    config: &Config,
) -> Result<usize, RuntimeError> {
    let pollers = RatePoller::for_config(table, sources, config.denomination())
        .map_err(|source| RuntimeError::QuotePathUnpollable { source })?;
    let started = pollers.len();
    for poller in pollers {
        tracing::info!(
            token = %poller.token(),
            cadence_seconds = poller.cadence().as_secs(),
            "polling a declared quote path to keep its rate fresh"
        );
        tokio::spawn(poller.run());
    }
    Ok(started)
}

/// Point a real rate source at this node's declared quote paths and start
/// them polling (ADR 0071 decision 6) -- the production call
/// [`spawn_rate_pollers`] was built for. Returns how many started.
///
/// **There is no endpoint key for this, and there should not be one.**
/// Decision 3 puts a quote's pools on the token's *own settlement chain*
/// precisely because that is the one chain a peering already guarantees
/// this node RPC for, and `Config::load` refuses a quote on a chain with no
/// `[settlement.<chain>]` table by name
/// (`ConfigError::TokenWithoutSettlement`). So the endpoint is that
/// table's `rpc_url`, read here, and a second key could only ever disagree
/// with the one the node already dials.
///
/// Reading a pool is *not* settling: this builds its own reader over the
/// same endpoint rather than reaching through a `SettlementBackend`, which
/// decision 6 keeps out of every value path. The two share a URL and
/// nothing else.
///
/// A node whose tokens declare no quote path starts nothing and says
/// nothing -- its pairs are the static `[[rates]]` rows an operator tends
/// by hand, which is also what decision 6 leaves every Solana-side pair on
/// until someone writes a source for that chain against the port.
///
/// **A quote path on a chain nothing here reads refuses startup** (issue
/// #1302), named by [`QuotePathUnusable::NoSourceForChain`]. This function
/// used to `warn!` about such a path and then hand it the EVM reader
/// anyway; [`RateSources`] carries the reasoning for the change, including
/// why the refusal is here rather than a sixth `ConfigError`. The warning
/// is gone rather than kept beside the refusal: one fact with two homes is
/// one fact that drifts.
fn spawn_quote_path_pollers(
    table: &SharedRateTable,
    config: &Config,
    transports: &SettlementTransports,
) -> Result<usize, RuntimeError> {
    let denomination = config.denomination();
    if denomination.quoted_tokens().next().is_none() {
        return Ok(0);
    }
    let sources = rate_sources(transports);
    let started = spawn_rate_pollers(table, &sources, config)?;
    tracing::info!(
        started,
        networks = %sources
            .networks()
            .map(|network| network.to_string())
            .collect::<Vec<_>>()
            .join(","),
        "reading declared quote paths by TWAP over each token's own settlement endpoint"
    );
    Ok(started)
}

/// Every chain this binary can read pools on, pointed at the endpoint the
/// node already settles that chain over (ADR 0071 decision 6, issue #1302).
///
/// One entry today: the Uniswap-v3-compatible TWAP reader over
/// `[settlement.evm] rpc_url`. A Solana reader written against the port
/// (decision 6 leaves it unwritten) is one more `with` here and nothing
/// else -- which is the point of building the map rather than teaching
/// `connector-config` a list of chains it would have to be kept in step
/// with.
///
/// **There is no endpoint key for this, and there should not be one.**
/// Decision 3 puts a quote's pools on the token's *own settlement chain*
/// precisely because that is the one chain a peering already guarantees
/// this node RPC for, and `Config::load` refuses a quote on a chain with no
/// `[settlement.<chain>]` table by name
/// (`ConfigError::TokenWithoutSettlement`). So the endpoint is that
/// table's `rpc_url`, read here, and a second key could only ever disagree
/// with the one the node already dials.
///
/// Reading a pool is *not* settling: this builds its own reader over the
/// same endpoint rather than reaching through a `SettlementBackend`, which
/// decision 6 keeps out of every value path. The two share a URL and
/// nothing else.
fn rate_sources(transports: &SettlementTransports) -> RateSources {
    let mut sources = RateSources::none();
    // Each EVM table's own transport, shared with its backend and syncer:
    // a rate source dialing on its own would be the one client of that
    // `rpc_url` left direct (ADR 0073 decision 2). One per network, reading
    // only that network's pools (ADR 0086 decision 13).
    for (network, transport) in &transports.evm {
        sources = sources.with(
            network.clone(),
            Arc::new(UniswapV3RateSource::connect(transport)),
        );
    }
    sources
}

/// Each settlement table's one [`RpcTransport`] (ADR 0073 decision 2):
/// built here, once, and handed to every client of that table's `rpc_url`
/// -- the backend, and on EVM the channel-index syncer and the rate source.
/// One of them left on a client of its own would be one client outside the
/// bounds, the refusal retries and the circuit.
///
/// A table with `rpc_via_socks_proxy = true` gets a transport through the
/// node's one `socks_proxy`, on its chain's own pinned [`Circuit`]; every
/// other table dials direct. Which is which is the table's own key, never
/// inferred from the `rpc_url` (ADR 0073 decision 1).
pub(crate) struct SettlementTransports {
    /// One per EVM table, keyed by its network (ADR 0086).
    evm: BTreeMap<Network, RpcTransport>,
    solana: Option<RpcTransport>,
    /// One per XRPL table, keyed by its network (ADR 0087).
    xrpl: BTreeMap<Network, RpcTransport>,
}

impl SettlementTransports {
    /// The transport for a table `config.settlements()` named. One is built
    /// per table, so a miss is a wiring error reported rather than a panic.
    fn for_settlement(&self, settlement: &SettlementConfig) -> Result<&RpcTransport, RuntimeError> {
        let transport = match settlement {
            SettlementConfig::Evm(evm) => self.evm.get(evm.network()),
            SettlementConfig::Solana(_) => self.solana.as_ref(),
            SettlementConfig::Xrpl(xrpl) => self.xrpl.get(&Network::Xrpl(xrpl.network())),
        };
        transport.ok_or_else(|| RuntimeError::SettlementEndpointUnusable {
            table: settlement.table_name(),
            message: "no transport was built for a table the config names".to_string(),
        })
    }
}

/// Build [`SettlementTransports`] from `config`'s settlement tables.
pub(crate) fn settlement_transports(config: &Config) -> Result<SettlementTransports, RuntimeError> {
    let mut transports = SettlementTransports {
        evm: BTreeMap::new(),
        solana: None,
        xrpl: BTreeMap::new(),
    };
    for settlement in config.settlements() {
        let unusable = |message: String| RuntimeError::SettlementEndpointUnusable {
            table: settlement.table_name(),
            message,
        };
        let transport = if settlement.rpc_via_socks_proxy() {
            // `Config::load` refuses this key without a `socks_proxy`, so the
            // miss is reported rather than expected; it is never a reason to
            // dial direct (ADR 0073 decision 2).
            let proxy = config.socks_proxy().ok_or_else(|| {
                unusable("rpc_via_socks_proxy is set and there is no socks_proxy".to_string())
            })?;
            // ADR 0086 decision 7: a circuit of its own per network.
            let circuit = match settlement {
                SettlementConfig::Evm(evm) => Circuit::EvmSettlement {
                    chain_id: evm.chain_id(),
                },
                SettlementConfig::Solana(_) => Circuit::SolanaSettlement,
                // ADR 0087: a circuit of its own per XRPL network.
                SettlementConfig::Xrpl(xrpl) => Circuit::XrplSettlement {
                    network_id: xrpl.network().network_id(),
                },
            };
            let transport = RpcTransport::through(settlement.rpc_url(), proxy, circuit)
                .map_err(|error| unusable(error.to_string()))?;
            tracing::info!(
                table = settlement.table_name(),
                endpoint = %transport.endpoint(),
                circuit = circuit.socks_username(),
                "settlement rpc via socks_proxy"
            );
            transport
        } else {
            RpcTransport::direct(settlement.rpc_url())
                .map_err(|error| unusable(error.to_string()))?
        };
        match settlement {
            SettlementConfig::Evm(evm) => {
                transports.evm.insert(evm.network().clone(), transport);
            }
            SettlementConfig::Solana(_) => transports.solana = Some(transport),
            SettlementConfig::Xrpl(xrpl) => {
                transports
                    .xrpl
                    .insert(Network::Xrpl(xrpl.network()), transport);
            }
        }
    }
    Ok(transports)
}

/// Start the watchers and sweeps over every batch-settlement backend this
/// node opted in to (ADR 0074 decision 5, issue #1344), reading the vouchers
/// to land from `gate`, the one place a voucher is accepted. Spawned, never
/// awaited, for the life of the process: a step
/// that fails is logged and retried on its next tick. A node that opted in
/// on neither chain starts nothing.
///
/// - EVM: [`EvmBatchWatcher`] reads `WithdrawInitiated` every
///   [`watch_interval`] of the node's `min_withdraw_delay_secs` and claims
///   the latest voucher on a withdrawing channel at once, and every
///   [`BATCH_SWEEP_INTERVAL`] claims every held voucher in one `claim` and
///   then `settle`s.
/// - Solana: [`SolanaBatchWatcher`] rediscovers every sponsored channel every
///   [`watch_interval`] of the node's `min_grace_period_secs` -- sealing a
///   Closing one with its latest voucher, distributing a Sealed one,
///   reclaiming a Distributed one's rent -- and settles Open ones every [`OPEN_SETTLE_INTERVAL`]. The Open settle
///   check runs only on a watch pass, so a longer watch rounds it up to the
///   next pass.
///
/// - XRPL: [`XrplBatchWatcher`], one per table, reads every [`watch_interval`]
///   of the table's `min_settle_delay_secs` and answers a payer's exit by landing
///   the latest voucher with `tfClose` (ADR 0087 decision 8).
///
/// No watch interval is configurable: [`watch_interval`] is the node's
/// published minimum delay over ninety, clamped to ten seconds..fifteen
/// minutes, so ninety passes fit inside the shortest delay any admitted
/// channel can carry and an idle node spends no more RPC than that needs.
///
/// Both backends read and write through the settlement table's one transport
/// (ADR 0073): the watchers are built from the backends, never from a URL.
///
/// [`watch_interval`]: connector_settlement::watch_interval::watch_interval
/// [`BATCH_SWEEP_INTERVAL`]: connector_settlement_evm::BATCH_SWEEP_INTERVAL
/// [`OPEN_SETTLE_INTERVAL`]: connector_settlement_solana::batch::OPEN_SETTLE_INTERVAL
fn spawn_batch_settlement_watchers(runtime: &Runtime, gate: &Arc<ClientClaimGate>) {
    // ADR 0086: one watcher per network, each handed only the vouchers held
    // on its own network's channels.
    let networks: Vec<Network> = runtime_receivers(runtime)
        .into_iter()
        .map(|(network, _)| network)
        .collect();
    let held_on = |network: &Network| -> Arc<dyn HeldVouchers> {
        Arc::new(NetworkVouchers {
            held: ClaimGateVouchers(Arc::clone(gate)),
            network: network.clone(),
            networks: networks.clone(),
        })
    };
    for (network, backend) in &runtime.batch_settlement_evm {
        let watcher = EvmBatchWatcher::new(Arc::clone(backend), held_on(network));
        tokio::spawn(watcher.run(
            watch_interval(backend.min_withdraw_delay_secs()),
            connector_settlement_evm::BATCH_SWEEP_INTERVAL,
        ));
    }
    if let Some(backend) = &runtime.batch_settlement_solana {
        let watcher =
            SolanaBatchWatcher::new(Arc::clone(backend), held_on(&solana_network_of(backend)));
        tokio::spawn(watcher.run(
            watch_interval(backend.min_grace_period_secs()),
            connector_settlement_solana::batch::OPEN_SETTLE_INTERVAL,
        ));
    }
    for (network, backend) in &runtime.batch_settlement_xrpl {
        let watcher = XrplBatchWatcher::new(Arc::clone(backend), held_on(network));
        tokio::spawn(watcher.run(watch_interval(backend.min_settle_delay_secs())));
    }
}

/// The client edge's claim gate, resumed from the watermarks its journal
/// already records (issue #605), with this node's discovery budget (issue
/// #613): how many lookups for channels that never resolve it will make.
///
/// A node with no `state_dir` gets an in-memory journal: its vouchers'
/// watermarks do not outlive the process.
///
/// Bound to a [`ClientPayoutLedger`] over `outbound` -- this node's journaled
/// outbound x402 channels -- before it is returned (issue #770, ADR 0075
/// decision 7): a client session's earnings are paid as vouchers on the
/// channel an operator opened toward that client, signed by the chain's
/// settlement key, and the watermark survives a restart with the channel's
/// journal. A node with no batch-settlement backend has no outbound
/// channels and pays no client.
fn client_claim_gate(
    config: &Config,
    outbound: Option<Arc<OutboundChannels>>,
) -> Result<ClientClaimGate, RuntimeError> {
    let (journal, path) = match config.state_dir() {
        Some(state_dir) => (
            open_journal(state_dir, CLIENT_EDGE_JOURNAL)?,
            state_dir.join(CLIENT_EDGE_JOURNAL),
        ),
        None => (
            Arc::new(InMemoryJournal::new()) as Arc<dyn Journal>,
            PathBuf::from(CLIENT_EDGE_JOURNAL),
        ),
    };
    // The shaper on lookups for channels that never resolve (issue #613): a
    // voucher naming a channel nobody opened is a free chain read for its
    // sender, and what a node can afford to spend discovering channels that
    // turn out not to exist depends on the settlement endpoint it pays for.
    let budget = UnresolvableLookupBudgetPolicy::default();
    // The gate's zk edge (`ClientClaimGate::with_zk_edge`) needs a
    // `ZkNoteVault`, and the only one is the in-memory fake. #1514's Mina
    // adapter is what this refusal becomes the construction of -- with
    // `ClientClaimGate::run_allowance_sweep` spawned beside it (#1523): the
    // boot sweep already runs inside `with_zk_edge`. The adapter also reads
    // `Subsidy::read(vault, zk_note.subsidy_budget())` there, once, through the
    // table's `RpcTransport`, and a failed read fails the boot (decision 14).
    if config.zk_note().is_some() {
        return Err(RuntimeError::ZkNoteVaultNotBuilt);
    }
    let gate = ClientClaimGate::restore(journal)
        .map_err(|source| RuntimeError::JournalUnreplayable { path, source })?
        .with_lookup_budget(UnresolvableLookupBudgetPolicy {
            per_signer: config
                .unresolvable_lookups_per_signer()
                .unwrap_or(budget.per_signer),
            total: config.unresolvable_lookups_total().unwrap_or(budget.total),
            window: config.unresolvable_lookup_window().unwrap_or(budget.window),
            max_wait: config
                .unresolvable_lookup_max_wait()
                .unwrap_or(budget.max_wait),
        });
    Ok(match outbound {
        Some(outbound) => gate.with_payout_ledger(Arc::new(ClientPayoutLedger::new(outbound))),
        None => gate,
    })
}

/// This node's own facts (ADR 0050), assembled once at router construction:
/// the single value `GET /ilp` serves as this node's self-description and
/// every x402 greeting projects its `extra` block out of (ND-11).
///
/// There is no second assembly anywhere. Before this, the greeting's node
/// facts were three separate arguments to the router and a kind:10032
/// announce composed a fourth set of its own -- which is exactly how
/// `requiredTransport` came to be enforced long before it was advertised, and
/// how `[announce].solana_chain_id` came to label a mainnet node's settlement
/// facts `solana:devnet` (issue #981). One value cannot disagree with itself.
///
/// Each field's provenance, because they differ and the difference is the
/// point (ND-05, ND-07):
///
///   * the three `[node]` fields are **configured**, because no process can
///     introspect them: a container sees `0.0.0.0:4000`, never
///     `https://toonprotocol.dev/ilp`;
///   * `peer_carriages` is `peer_expose`, i.e. which listeners this node
///     opens. **Which** carriages exist, never **who** rides them -- peer
///     identities and per-peering terms are operator-private (ND-09);
///   * `batch_settlements` and `voucher_signers` are **proved**: every entry
///     was read off a backend that connected to a live chain and refused to
///     boot on a disagreement. Nothing re-declares any of it.
fn node_facts(
    config: &Config,
    runtime: &Runtime,
) -> Result<connector_domain::NodeFacts, RuntimeError> {
    let node = config.node();
    Ok(connector_domain::NodeFacts {
        ilp_addresses: node
            .map(|node| node.addresses().to_vec())
            .unwrap_or_default(),
        http_endpoint: node
            .and_then(|node| node.http_endpoint())
            .map(str::to_string),
        btp_endpoint: node
            .and_then(|node| node.btp_endpoint())
            .map(str::to_string),
        // Every carriage this node exposes a listener for, in the operator's
        // own spelling. `"neither"` -- the default -- is an empty list rather
        // than the word: a reader asks "can I peer over BTP?", and an empty
        // list answers it without knowing this config file's vocabulary.
        peer_carriages: [PeerCarriage::Btp, PeerCarriage::Http]
            .into_iter()
            .filter(|carriage| config.peer_expose().exposes(*carriage))
            .map(|carriage| carriage.name().to_string())
            .collect(),
        batch_settlements: runtime.batch_settlements.clone(),
        voucher_signers: runtime.voucher_signers.clone(),
        zk_note: config.zk_note().map(zk_note_terms).transpose()?,
    })
}

/// A zk edge's standing terms (ADR 0080 decisions 7 and 8, #1523), read off
/// the `[zk_note]` table and the state key it names -- the one fact the
/// greeting's `zk-note` entry and the self-description's `zkNote` project.
/// `allowanceEndpoint` is the path the endpoints mount, from the one
/// constant, so the greeting can never name a path nothing serves.
fn zk_note_terms(
    zk_note: &connector_config::ZkNoteConfig,
) -> Result<connector_domain::x402::X402ZkNoteTerms, RuntimeError> {
    let state_key = read_zk_note_state_key(zk_note.state_key())?;
    Ok(connector_domain::x402::X402ZkNoteTerms {
        network: zk_note.network().caip2().to_string(),
        asset: ZK_NOTE_ASSET.to_string(),
        pay_to: zk_note.vault().to_string(),
        verification_key_hash: zk_note.verification_key_hash().to_string(),
        state_key: state_key.public_key().into_address(),
        tiers: zk_note
            .tiers()
            .as_slice()
            .iter()
            .map(|tier| tier.budget().to_string())
            .collect(),
        allowance_lifetime_secs: zk_note.allowance_lifetime().as_secs(),
        allowance_endpoint: connector_client_edge::ZK_NOTE_ALLOWANCES_PATH.to_string(),
        subsidy: None,
        subsidised_last_period: None,
    })
}

/// Native MINA as the greeting names it (ADR 0080 decision 7, as amended):
/// SLIP-44 coin type 12586, which with the CAIP-2 `network` reads as the
/// CAIP-19 `mina:<network>/slip44:12586`.
const ZK_NOTE_ASSET: &str = "slip44:12586";

/// The zk edge's state key, from `[zk_note] state_key_file`: read and held
/// by `connector-signer`, which alone may (coding-standards.md, "One crate
/// owns key material").
fn read_zk_note_state_key(
    location: &connector_config::SecretLocation,
) -> Result<connector_signer::ZkStateSigner, RuntimeError> {
    let connector_config::SecretLocation::File(path) = location else {
        return Err(RuntimeError::ZkNoteStateKey {
            path: PathBuf::new(),
            reason: "a KMS-held state key is not supported".to_string(),
        });
    };
    connector_signer::ZkStateSigner::from_key_file(path).map_err(|error| {
        RuntimeError::ZkNoteStateKey {
            path: path.clone(),
            reason: match error {
                connector_signer::ZkStateKeyError::Unreadable { source, .. } => source.to_string(),
                connector_signer::ZkStateKeyError::NotAMinaKey { .. } => {
                    "not a base58 Mina secret key".to_string()
                }
            },
        }
    })
}

/// Merge the client edge and (if `[operator]` is configured) the operator
/// surface into the one router the binary serves. The operator router is
/// mounted only when [`Config::operator`] is `Some` -- absence means the
/// surface is not started at all, exactly as it means for
/// [`connector_operator::router`] itself.
///
/// Fallible since issue #605: the client edge's claim gate is restored from
/// a durable journal here, and a journal that will not replay must stop the
/// node starting rather than let it start at no watermarks.
pub fn router(runtime: &Runtime, config: &Config) -> Result<Router, RuntimeError> {
    let connector = runtime.connector.clone();
    let signer = runtime.signer.clone();
    // Issue #556: a privacy-wrapped claim
    // (`ILP-Payment-Channel-Claim-Wrapped`, client-edge-spec.md §1.3) is
    // opened with this node's `[signer]` key, not with a second receiver
    // key of its own. `connector-config/src/announce.rs` states the rule:
    // `GET /ilp/identity` and every gift wrap this node opens both use
    // `[signer]` -- and since that endpoint is the only surface publishing
    // a receiver public key, a sender can wrap to no other one. No new
    // config section exists or is needed for this.
    let wrap_receiver_secret = Some(read_signer_secret(config.signer_key())?);
    let mut claim_gate = client_claim_gate(config, runtime.outbound_channels.clone())?;
    // Vouchers are accepted on exactly the chains whose `batch_settlement`
    // table is written, and refused by name on the rest.
    if let Some(channels) = batch_settlement_channels(runtime) {
        claim_gate = claim_gate.with_batch_settlement(Arc::new(channels));
    }
    let claim_gate = Arc::new(claim_gate);
    // ADR 0086 decision 14: which network a payment's channel is on, for an
    // app's `X-TOON-Network`, is the gate's to say -- it verified the
    // channel under that network's domain.
    connector.set_channel_networks(Arc::new(GateChannelNetworks {
        gate: Arc::clone(&claim_gate),
        networks: runtime_receivers(runtime)
            .into_iter()
            .map(|(network, _)| network)
            .collect(),
    }));
    spawn_batch_settlement_watchers(runtime, &claim_gate);
    let app = connector_client_edge::router_with_node_facts(
        connector.clone(),
        signer.clone(),
        wrap_receiver_secret,
        claim_gate.clone(),
        node_facts(config, runtime)?,
        config
            .btp_session_window()
            .unwrap_or(connector_client_edge::DEFAULT_BTP_SESSION_WINDOW),
        // Issue #678 gap 1: the accept side. There is no second listener --
        // the peer carriages ride the `POST /ilp` and `GET /ilp/btp` this
        // router already serves, and role is decided by authentication
        // (`peer-carriage-spec.md` §1.3), never by the port. `None` for a
        // node whose `peer_expose` is `"neither"`, which is the default.
        PeerCarriages::from_config(
            connector.clone(),
            config.peers(),
            config.peer_expose(),
            // ADR 0075 decision 5 (issue #1377): a voucher, or a peer-role
            // challenge, proves the peer role once its channel's voucher
            // signer is bound to a peering, and the claim gate is what
            // resolves the channel and reads that signer off the chain.
            Some(claim_gate.clone() as Arc<dyn connector_peer_btp::VoucherEvidence>),
        ),
        // Issue #502: every `[[client_identities]]` entry, as the
        // `id`/`secret` pair `resolve_identity` authenticates an
        // `ILP-Peer-Id` against. Empty is every node before this config
        // section existed -- every request is anonymous or, if it presents
        // an `ILP-Peer-Id`, refused `401`.
        config
            .client_identities()
            .iter()
            .map(|identity| connector_domain::identity::ConfiguredIdentity {
                id: identity.id().to_string(),
                secret: identity.secret().to_string(),
            })
            .collect(),
    );
    // ADR 0074 decision 9, issue #1346: the public Solana sponsor endpoint,
    // on the client edge's listener. Mounted whether or not this node opted
    // in, so a node that has not refuses by name.
    let app = app.merge(crate::sponsor::router(
        runtime.batch_settlement_solana.clone(),
    ));
    // ADR 0080 decision 8, #1523: the zk edge's allowance endpoints, beside
    // the sponsor's and on the same terms -- mounted on every node, so one
    // that is no zk edge refuses them by name.
    let app = app.merge(connector_client_edge::zk_note_router(claim_gate.clone()));
    Ok(match config.operator() {
        Some(operator) => {
            let batch_channels = batch_channels_for_operator(runtime, &claim_gate);
            // A spending limit counts in a journal under `state_dir` (ADR
            // 0078 decision 8), opened here so a node with nowhere to write
            // it fails at boot rather than at a controller's first packet.
            let day_totals = match config.state_dir() {
                Some(state_dir) => Some(
                    connector_operator::DayTotals::open(
                        &state_dir.join(connector_operator::DAY_TOTALS_JOURNAL),
                        Arc::new(connector_runtime::SystemClock),
                    )
                    .map_err(RuntimeError::DayTotalsUnusable)?,
                ),
                None => None,
            };
            app.merge(connector_operator::router_with_controller_keys(
                connector,
                claim_gate,
                signer,
                operator.bearer_token().to_string(),
                operator.write_keys().to_vec(),
                // The file itself, not the list it held at boot: the operator
                // surface reads it again on every signed request (ADR 0079).
                operator.controller_keys_file().cloned().map_or_else(
                    || connector_operator::ControllerKeySource::Fixed(Vec::new()),
                    connector_operator::ControllerKeySource::File,
                ),
                day_totals,
                declared_rates(runtime, config),
                batch_channels,
            ))
        }
        None => app,
    })
}

/// What `GET /rates` reads on a dealing node, or `None` on one that deals
/// nothing (ADR 0071, issue #1297).
///
/// The table itself is the one [`build`] put on the [`Runtime`] -- the same
/// handle the forwarding path converts against and the poller refreshes, so
/// the page cannot disagree with what packets see. What the table does not
/// know is which pairs this node has *committed* to sourcing: a declared
/// quote path holds no row until its poller's first observation lands, and
/// a poller that never started would otherwise leave its pair missing from
/// the one page that exists to say so. Those pairs are read back out of the
/// config here, where the declaration lives.
fn declared_rates(runtime: &Runtime, config: &Config) -> Option<DeclaredRates> {
    let table = runtime.rate_table.as_ref()?;
    let numeraire = config.denomination().numeraire()?;
    Some(DeclaredRates::new(
        table.clone(),
        config
            .denomination()
            .quoted_tokens()
            .map(|(token, _)| (token.clone(), numeraire.clone())),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::io::Write;
    use tower::ServiceExt;

    fn load_config(text: &str) -> Config {
        try_load_config(text).expect("load config")
    }

    /// [`load_config`] without the `expect`, for a test whose subject IS
    /// the refusal (issue #1145's required `[[pay_channels]]` row).
    fn try_load_config(text: &str) -> Result<Config, connector_config::ConfigError> {
        let mut config_file = tempfile::NamedTempFile::new().expect("temp config file");
        write!(config_file, "{}", with_state_dir(text)).expect("write config file");
        Config::load(config_file.path())
    }

    /// Give a fixture its own `state_dir` unless it names one itself.
    ///
    /// CF-39 as amended by issue #1186 refuses a settlement table with no
    /// durable watermark location, and most fixtures here configure
    /// settlement -- so without this every one of them would carry the same
    /// boilerplate line.
    ///
    /// **A fresh directory per call, never a shared one.** These journals are
    /// real: a constant path lets one test's accepted claim become the next
    /// test's replay, which is exactly what a first attempt at this hit
    /// (`Undercollateralized` expected, `NonceNotAdvancing` returned, because
    /// the watermark had survived from an earlier run). `keep` keeps the
    /// directory rather than deleting it at end of scope, since the runtime
    /// opens its journal well after this function has returned.
    fn with_state_dir(text: &str) -> String {
        if text.contains("state_dir") {
            return text.to_string();
        }
        let dir = tempfile::tempdir().expect("temp state dir").keep();
        format!("state_dir = \"{}\"\n{text}", dir.display())
    }

    /// Load a minimal config with `extra` spliced in at the top level, and
    /// hand back the *result* rather than unwrapping it -- for a test whose
    /// subject is whether a configuration loads at all.
    fn raw_config_result(extra: &str) -> Result<Config, connector_config::ConfigError> {
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(&[7u8; 32])
            .expect("write raw 32-byte key");
        let key_path = key_file.into_temp_path();
        let mut config_file = tempfile::NamedTempFile::new().expect("temp config file");
        write!(
            config_file,
            "{}",
            with_state_dir(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"
{extra}

[signer]
key_file = "{}"
"#,
                key_path.display()
            ))
        )
        .expect("write config file");
        Config::load(config_file.path())
    }

    /// Returns the loaded [`Config`] together with the [`tempfile::TempPath`]
    /// its `signer.key_file` points at -- the caller must keep the returned
    /// path alive (a binding, not `_`) for as long as anything still needs
    /// to read the key file, since [`build`] re-reads it rather than
    /// caching its bytes at config-load time.
    fn config_with_raw_key_file(
        body: impl FnOnce(&std::path::Path) -> String,
    ) -> (Config, tempfile::TempPath) {
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(&[7u8; 32])
            .expect("write raw 32-byte key");
        let key_path = key_file.into_temp_path();
        let config = load_config(&body(&key_path));
        (config, key_path)
    }

    #[tokio::test]
    async fn builds_a_connector_from_a_raw_32_byte_key_file() {
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        });

        let runtime = build(&config).await.expect("build");
        assert!(runtime.connector.routes().is_empty());
    }

    #[tokio::test]
    async fn builds_a_signer_from_a_hex_encoded_key_file() {
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"0707070707070707070707070707070707070707070707070707070707070707")
            .expect("write hex key");
        let config = load_config(&format!(
            r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"
"#,
            key_file.path().display()
        ));

        let result = build(&config).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn rejects_key_material_that_is_neither_32_bytes_nor_64_hex_chars() {
        let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
        key_file
            .write_all(b"not real key material")
            .expect("write bad key");
        let config = load_config(&format!(
            r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"
"#,
            key_file.path().display()
        ));

        let result = build(&config).await;
        assert!(matches!(
            result,
            Err(RuntimeError::InvalidSignerKeyMaterial { .. })
        ));
    }

    #[tokio::test]
    async fn a_kms_location_is_an_explicit_unsupported_error_not_a_panic() {
        let config = load_config(
            r#"
client_edge_addr = "127.0.0.1:0"

[signer]
kms_key_id = "arn:aws:kms:us-east-1:123:key/abc"
"#,
        );

        let result = build(&config).await;
        assert!(matches!(
            result,
            Err(RuntimeError::UnsupportedSignerLocation)
        ));
    }

    #[tokio::test]
    async fn router_mounts_only_the_client_edge_when_no_operator_section_is_configured() {
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        });
        let runtime = build(&config).await.expect("build");
        let app = router(&runtime, &config).expect("router");

        // No `[operator]` section: `/routes` (an operator-surface path)
        // 404s -- it was never merged in, rather than merged in and
        // rejecting for lack of a bearer token.
        //
        // `/metrics` and `/admin/metrics.json` are asserted alongside it for
        // a reason `/routes` does not carry (issue #753). When the devnet cut
        // over to this connector the public demo dashboard lost the
        // TypeScript `/admin/metrics.json` it polled, and the obvious repair
        // -- serve a counter snapshot from the always-mounted client edge, so
        // no `[operator]` section is needed -- is exactly what ADR 0014
        // refused: the metrics surface is five decided names, in Prometheus
        // text, behind the operator surface's bearer token, "avoid[ing]
        // introducing a second, differently-authenticated (or
        // unauthenticated) HTTP surface". A node that configures no operator
        // therefore exposes no metrics AT ALL, which is the property this
        // asserts. `/admin/metrics.json` is named literally because that is
        // the path a future shim would be tempted to reintroduce.
        for path in ["/routes", "/metrics", "/admin/metrics.json"] {
            let request = Request::builder().uri(path).body(Body::empty()).unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{path} must not be served without an [operator] section"
            );
        }
    }

    /// Issue #502, wired end to end: `router` reads `[[client_identities]]`
    /// off the same [`Config`] `build` validated and threads it into the
    /// client edge, so a request presenting an `ILP-Peer-Id` this node
    /// configures but the wrong secret is refused `401` by the router this
    /// crate actually serves -- not just the library-level unit tests in
    /// `connector-client-edge` that construct a `ConfiguredIdentity` by
    /// hand.
    #[tokio::test]
    async fn router_refuses_an_unauthenticated_client_identity() {
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[[client_identities]]
id = "peer-a"
secret = "s3cr3t"
"#,
                key_path.display()
            )
        });
        let runtime = build(&config).await.expect("build");
        let app = router(&runtime, &config).expect("router");

        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .header("ilp-peer-id", "peer-a")
            .header("authorization", "Bearer wrong")
            .body(Body::from(vec![0u8; 4]))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// Issue #807, wired end to end and re-homed by ADR 0050: `router` reads
    /// `[node]` -- the section `[announce]` became when the announce was
    /// removed (ADR 0046, issue #1074) -- off the same [`Config`] `build`
    /// validated, and threads it into the client edge's node facts, so a
    /// zero-condition PREPARE answered by the router this crate actually
    /// serves carries this node's own `ilpAddresses`/`btpEndpoint`, not just
    /// the library-level unit tests in `connector-client-edge` that build the
    /// facts by hand.
    ///
    /// The greeting is a projection of the same facts `GET /ilp` publishes,
    /// so this asserts the wiring on the one surface a client that cannot yet
    /// address the node still reaches.
    #[tokio::test]
    async fn router_answers_a_zero_condition_greeting_with_the_node_configured_bootstrap_identity()
    {
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
peer_expose = "both"

[signer]
key_file = "{}"

[node]
addresses = ["g.toon.apex"]
http_endpoint = "https://apex.example/ilp"
btp_endpoint = "wss://apex.example/ilp/btp"
"#,
                key_path.display()
            )
        });
        let runtime = build(&config).await.expect("build");
        let app = router(&runtime, &config).expect("router");

        // A well-formed, zero-amount, `greeting`-flagged PREPARE addressed
        // to this node's own configured address -- exactly the shape a
        // client with no other way to learn `ilpAddresses`/`btpEndpoint`
        // sends when probing the edge it can reach but has never
        // bootstrapped against.
        let prepare = connector_domain::Prepare {
            amount: 0,
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(30),
            greeting: true,
            destination: "g.toon.apex".to_string(),
            data: Vec::new(),
        };
        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .body(Body::from(prepare.encode()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let terms: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let extra = &terms["extensions"]["toon"]["info"];
        assert_eq!(extra["ilpAddresses"], serde_json::json!(["g.toon.apex"]));
        assert_eq!(extra["btpEndpoint"], "wss://apex.example/ilp/btp");
    }

    /// A structurally-valid EVM voucher on a node that settles on no chain
    /// -- built once and reused both plaintext and wrapped, so the only
    /// difference between the two requests below is the header carrying it.
    fn undeclared_channel_claim_json() -> String {
        format!(
            r#"{{
                "version": "1.0",
                "blockchain": "evm",
                "network": "eip155:84532",
                "scheme": "batch-settlement",
                "messageId": "msg-1",
                "timestamp": "2026-02-02T12:00:00.000Z",
                "senderId": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1",
                "channelId": "0x{channel}",
                "maxClaimableAmount": "1",
                "signature": "0x{signature}"
            }}"#,
            channel = "ab".repeat(32),
            signature = "cd".repeat(65),
        )
    }

    fn hex_encode_bytes(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn base64_encode_bytes(bytes: &[u8]) -> String {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;
        BASE64.encode(bytes)
    }

    /// A well-formed `ILP-Payment-Channel-Claim-Wrapped` envelope, NIP-59
    /// sealing `claim_json` to `receiver_public`.
    fn wrapped_claim_header_value(claim_json: &str, receiver_public: &[u8; 65]) -> String {
        use libsecp256k1::SecretKey;

        let sender_secret = SecretKey::parse(&[3u8; 32]).expect("valid secret key");
        let wrapped =
            connector_signer::wrap_claim(claim_json.as_bytes(), &sender_secret, receiver_public)
                .expect("wrap claim");
        let envelope_json = format!(
            r#"{{"ephemeralPublicKey":"{}","encryptedPayload":"{}","timestamp":0,"version":"1.0"}}"#,
            hex_encode_bytes(&wrapped.ephemeral_public_key),
            base64_encode_bytes(&wrapped.encrypted_payload),
        );
        base64_encode_bytes(envelope_json.as_bytes())
    }

    fn unmatched_destination_prepare_body() -> Vec<u8> {
        connector_domain::Prepare {
            amount: 0,
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(30),
            greeting: false,
            destination: "g.example.unmatched".to_string(),
            data: Vec::new(),
        }
        .encode()
    }

    /// Issue #556, item 1: `router` now passes the `[signer]` key as
    /// `wrap_receiver_secret` instead of `None`, so a claim wrapped to this
    /// node's own signer key is unwrapped rather than refused
    /// `WrapUnsupported` -- and then runs the exact same client-edge gate a
    /// plaintext claim runs, unwrapping granting no exemption at the step
    /// that follows it. Proven by wrapping a claim that names a channel
    /// this node has no record of, and getting back the identical
    /// `BatchSettlementNotAccepted` rejection a plaintext claim for the same channel
    /// gets -- see the next test.
    #[tokio::test]
    async fn router_unwraps_a_claim_wrapped_to_the_signer_key_and_runs_the_identical_gate() {
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"

# A destination something serves: a claim riding a packet to one nothing
# serves is never looked at (issue #1446).
[[routes]]
prefix = "g.example.unmatched"
handler_url = "http://localhost:4000/"
price = 1

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        });
        let runtime = build(&config).await.expect("build");
        let receiver_public = runtime.signer.public_key().expect("signer public key");
        let app = router(&runtime, &config).expect("router");

        let claim_json = undeclared_channel_claim_json();
        let wrapped_header = wrapped_claim_header_value(&claim_json, &receiver_public);

        let plaintext_request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .header(
                "ilp-payment-channel-claim",
                base64_encode_bytes(claim_json.as_bytes()),
            )
            .body(Body::from(unmatched_destination_prepare_body()))
            .unwrap();
        let plaintext_response = app.clone().oneshot(plaintext_request).await.unwrap();
        assert_eq!(plaintext_response.status(), StatusCode::OK);
        let plaintext_bytes = hyper::body::to_bytes(plaintext_response.into_body())
            .await
            .unwrap();
        let plaintext_reject =
            connector_domain::Reject::decode(&plaintext_bytes).expect("decode reject");

        let wrapped_request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .header("ilp-payment-channel-claim-wrapped", wrapped_header)
            .body(Body::from(unmatched_destination_prepare_body()))
            .unwrap();
        let wrapped_response = app.oneshot(wrapped_request).await.unwrap();
        assert_eq!(wrapped_response.status(), StatusCode::OK);
        let wrapped_bytes = hyper::body::to_bytes(wrapped_response.into_body())
            .await
            .unwrap();
        let wrapped_reject =
            connector_domain::Reject::decode(&wrapped_bytes).expect("decode reject");

        assert!(
            wrapped_reject.message.contains("does not settle on"),
            "expected a BatchSettlementNotAccepted rejection, got {wrapped_reject:?}"
        );
        assert_eq!(
            wrapped_reject.code, plaintext_reject.code,
            "a wrapped claim must run the identical gate a plaintext claim runs"
        );
        assert_eq!(
            wrapped_reject.message, plaintext_reject.message,
            "unwrapping must grant no exemption -- the same claim rejected the same way \
             whichever header carried it"
        );
    }

    /// Issue #556's other acceptance criterion for the receiver key: a wrap
    /// addressed to a key that is not this node's `[signer]` key fails to
    /// unwrap -- refused `WrapFailed` -- distinguishably both from a
    /// malformed wrap (`Malformed`) and from the plaintext `BatchSettlementNotAccepted`
    /// rejection the previous test established.
    #[tokio::test]
    async fn router_refuses_a_wrap_addressed_to_a_different_receiver_distinguishably() {
        use libsecp256k1::{PublicKey, SecretKey};

        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"

# A destination something serves: a claim riding a packet to one nothing
# serves is never looked at (issue #1446).
[[routes]]
prefix = "g.example.unmatched"
handler_url = "http://localhost:4000/"
price = 1

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        });
        let runtime = build(&config).await.expect("build");
        let app = router(&runtime, &config).expect("router");

        // A key that is deliberately NOT this node's own [signer] key.
        let other_receiver_secret = SecretKey::parse(&[9u8; 32]).expect("valid secret key");
        let other_receiver_public = PublicKey::from_secret_key(&other_receiver_secret).serialize();
        let claim_json = undeclared_channel_claim_json();
        let wrong_receiver_header = wrapped_claim_header_value(&claim_json, &other_receiver_public);

        let wrong_receiver_request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .header("ilp-payment-channel-claim-wrapped", wrong_receiver_header)
            .body(Body::from(unmatched_destination_prepare_body()))
            .unwrap();
        let wrong_receiver_response = app.clone().oneshot(wrong_receiver_request).await.unwrap();
        assert_eq!(wrong_receiver_response.status(), StatusCode::OK);
        let wrong_receiver_bytes = hyper::body::to_bytes(wrong_receiver_response.into_body())
            .await
            .unwrap();
        let wrong_receiver_reject =
            connector_domain::Reject::decode(&wrong_receiver_bytes).expect("decode reject");
        assert!(
            wrong_receiver_reject
                .message
                .contains("failed to unwrap claim"),
            "expected a WrapFailed rejection, got {wrong_receiver_reject:?}"
        );

        let malformed_request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .header("ilp-payment-channel-claim-wrapped", "not-valid-base64!!")
            .body(Body::from(unmatched_destination_prepare_body()))
            .unwrap();
        let malformed_response = app.oneshot(malformed_request).await.unwrap();
        assert_eq!(malformed_response.status(), StatusCode::OK);
        let malformed_bytes = hyper::body::to_bytes(malformed_response.into_body())
            .await
            .unwrap();
        let malformed_reject =
            connector_domain::Reject::decode(&malformed_bytes).expect("decode reject");

        assert_ne!(
            wrong_receiver_reject.message, malformed_reject.message,
            "a wrap addressed to the wrong receiver is a different failure from a malformed wrap"
        );
        assert_ne!(
            wrong_receiver_reject.message,
            // The gate's own wording, not a copy of it: a reworded refusal
            // must not quietly make this assertion vacuous.
            connector_client_edge::ClaimIngestRejection::BatchSettlementNotAccepted.message(),
            "a wrap addressed to the wrong receiver must fail to unwrap, not fall through to the \
             plaintext voucher's own rejection"
        );
    }

    /// The unresolvable-lookup budget's knobs reach the claim gate (issue
    /// #613). Asserted through behaviour: with a node-wide allowance of two, a
    /// sender walking channel ids reaches the settlement backend twice and no
    /// more, however many vouchers they present.
    #[tokio::test]
    async fn the_configured_lookup_budget_reaches_the_claim_gate() {
        use connector_client_edge::{
            AdmittedEvmVoucherChannel, AdmittedSolanaVoucherChannel, BatchSettlementChannels,
            ChannelResolutionError,
        };
        use connector_signer::{BatchChannelConfig, BatchSettlementDomain};
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// A chain that knows about no channel at all -- which is what a
        /// walk of the id space looks like from the connector's side.
        #[derive(Debug, Default)]
        struct EmptyCountingBackend {
            lookups: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl BatchSettlementChannels for EmptyCountingBackend {
            fn evm_domains(&self) -> Vec<BatchSettlementDomain> {
                vec![BatchSettlementDomain::x402(84_532)]
            }
            fn solana_network(&self) -> Option<connector_domain::Network> {
                None
            }
            async fn evm(
                &self,
                _network: &connector_domain::Network,
                _channel_id: &[u8; 32],
                _presented_config: Option<&BatchChannelConfig>,
            ) -> Result<Option<AdmittedEvmVoucherChannel>, ChannelResolutionError> {
                self.lookups.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            }
            async fn solana(
                &self,
                _channel_account: &[u8; 32],
            ) -> Result<Option<AdmittedSolanaVoucherChannel>, ChannelResolutionError> {
                Ok(None)
            }
        }

        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
unresolvable_lookup_budget_per_signer = 2
unresolvable_lookup_budget_total = 2
unresolvable_lookup_budget_window_secs = 600
unresolvable_lookup_budget_max_wait_ms = 1

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        });

        let backend = Arc::new(EmptyCountingBackend::default());
        let gate = client_claim_gate(&config, None)
            .expect("a fresh in-memory journal has nothing to replay")
            .with_batch_settlement(backend.clone());

        // A fresh channel id per voucher, which is the whole shape of the
        // attack: nothing this connector has ever seen, and nothing it ever
        // will resolve.
        let voucher = |n: u64| {
            serde_json::json!({
                "version": "1.0",
                "blockchain": "evm",
                "network": "eip155:84532",
                "scheme": "batch-settlement",
                "messageId": format!("msg-{n}"),
                "timestamp": "2026-02-02T12:00:00.000Z",
                "senderId": "0x1111111111111111111111111111111111111111",
                "channelId": format!("0x{n:064x}"),
                "maxClaimableAmount": "10",
                "signature": format!("0x{}", "cd".repeat(65)),
            })
            .to_string()
        };
        let mut budgeted = 0;
        for n in 1..=20 {
            if matches!(
                gate.ingest(&voucher(n), 0).await,
                Err(connector_client_edge::ClaimIngestRejection::LookupBudgetExhausted { .. })
            ) {
                budgeted += 1;
            }
        }

        assert_eq!(
            backend.lookups.load(Ordering::SeqCst),
            2,
            "twenty vouchers on twenty channels cost the configured allowance and no more"
        );
        assert_eq!(
            budgeted, 18,
            "and every voucher past it says why it was refused"
        );
    }

    /// `connector-config` restates the client edge's own budget defaults so
    /// that it can validate a one-sided configuration against the values
    /// that will actually be in force (issue #613's review). Restating them
    /// is only safe if they cannot drift, and this is the only crate that
    /// can see both.
    #[test]
    fn the_config_layers_budget_defaults_match_the_client_edges() {
        use connector_client_edge::MAX_UNRESOLVABLE_LOOKUP_WINDOW;

        let edge = UnresolvableLookupBudgetPolicy::default();

        // A configuration naming *only* the node-wide rate, set to exactly
        // the client edge's own default per-signer rate. If the config
        // layer's copy of that default agrees, this is coherent and loads;
        // if the two had drifted in either direction, one of the four
        // assertions here would fail.
        assert!(
            raw_config_result(&format!(
                "unresolvable_lookup_budget_total = {}",
                edge.per_signer
            ))
            .is_ok(),
            "per_signer == total is coherent, so the config layer's default per-signer rate is \
             not above {}",
            edge.per_signer
        );
        assert!(
            raw_config_result(&format!(
                "unresolvable_lookup_budget_total = {}",
                edge.per_signer - 1
            ))
            .is_err(),
            "...and not below it either"
        );

        // The node-wide default, checked from the other side.
        assert!(raw_config_result(&format!(
            "unresolvable_lookup_budget_per_signer = {}",
            edge.total
        ))
        .is_ok());
        assert!(raw_config_result(&format!(
            "unresolvable_lookup_budget_per_signer = {}",
            edge.total + 1
        ))
        .is_err());

        // ...and the window and wait ceiling, by the same trick: a ceiling
        // exactly equal to the default window is coherent, one millisecond
        // past it is not.
        assert!(raw_config_result(&format!(
            "unresolvable_lookup_budget_max_wait_ms = {}",
            edge.window.as_millis()
        ))
        .is_ok());
        assert!(raw_config_result(&format!(
            "unresolvable_lookup_budget_max_wait_ms = {}",
            edge.window.as_millis() + 1
        ))
        .is_err());

        // And the cap the client edge clamps a window to is the one the
        // config layer refuses above.
        assert!(raw_config_result(&format!(
            "unresolvable_lookup_budget_window_secs = {}",
            MAX_UNRESOLVABLE_LOOKUP_WINDOW.as_secs()
        ))
        .is_ok());
        assert!(raw_config_result(&format!(
            "unresolvable_lookup_budget_window_secs = {}",
            MAX_UNRESOLVABLE_LOOKUP_WINDOW.as_secs() + 1
        ))
        .is_err());
    }

    /// Issue #605's startup half: a node that names a `state_dir` gets a
    /// real, on-disk claim gate, and the file it journals to is under that
    /// directory -- which is what an operator has to mount for the
    /// watermarks to outlive the container.
    #[test]
    fn a_configured_state_dir_is_where_the_client_edge_journals() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
                state_dir = state_dir.path().display(),
            )
        });

        client_claim_gate(&config, None).expect("a writable state_dir produces a gate");
        assert!(
            state_dir.path().join(CLIENT_EDGE_JOURNAL).exists(),
            "the journal file is created at startup, not lazily at the first claim"
        );
    }

    /// A `state_dir` this node cannot write is a startup failure naming the
    /// path -- not a node that serves happily and forgets every spent claim
    /// at its next restart (issue #605).
    #[test]
    fn a_state_dir_that_cannot_be_created_refuses_to_build_a_gate() {
        let blocker = tempfile::NamedTempFile::new().expect("temp file");
        // A regular file where a directory is asked for: `create_dir_all`
        // cannot make this into a directory, exactly as it cannot make a
        // directory under a read-only mount.
        let state_dir = blocker.path().join("state");
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
                state_dir = state_dir.display(),
            )
        });

        let Err(error) = client_claim_gate(&config, None) else {
            panic!("an unusable state_dir must not produce a gate");
        };
        assert!(matches!(error, RuntimeError::StateDirUnusable { .. }));
        let message = error.to_string();
        assert!(
            message.contains(&state_dir.display().to_string()),
            "the failure must name the path an operator has to fix: {message}"
        );
    }

    /// ADR 0080: a `[zk_note]` table on a build with no Mina vault adapter
    /// is refused by name at boot, never served as a zk edge that grants
    /// nothing.
    #[test]
    fn a_zk_note_table_without_a_vault_adapter_refuses_to_build_a_gate() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let state_key = state_dir.path().join("zk-state.key");
        std::fs::write(&state_key, b"not a real key").expect("write");
        let vault = connector_zk_note::Keypair::try_from(connector_zk_note::SecKey::new(
            connector_zk_note::Scalar::from(7u64),
        ))
        .expect("a keypair")
        .public
        .into_address();
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"

[zk_note]
vault = "{vault}"
network = "mina:devnet"
graphql_url = "https://api.minascan.io/node/devnet/v1/graphql"
state_key_file = "{state_key}"
verification_key_hash = "1"
tiers = [100000000]
allowance_lifetime_secs = 3600
open_budget_cap = 1000000000
"#,
                key_path = key_path.display(),
                state_dir = state_dir.path().display(),
                state_key = state_key.display(),
            )
        });
        let Err(error) = client_claim_gate(&config, None) else {
            panic!("a zk-note table must not produce a gate this build cannot back");
        };
        assert!(matches!(error, RuntimeError::ZkNoteVaultNotBuilt));
        assert!(error.to_string().contains("[zk_note]"), "{error}");
    }

    /// ADR 0080 (#1523): the greeting's and self-description's zk-note
    /// terms are read off the table and the state key file, the state key
    /// published as the address of the key in that file, the endpoint as
    /// the path the endpoints mount -- and a file holding no Mina key is
    /// refused by name.
    #[test]
    fn a_zk_edges_terms_are_read_off_its_table_and_state_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let secret = connector_zk_note::SecKey::new(connector_zk_note::Scalar::from(9u64));
        let address = connector_zk_note::Keypair::try_from(secret.clone())
            .expect("a keypair")
            .public
            .into_address();
        let state_key = dir.path().join("zk-state.key");
        std::fs::write(&state_key, format!("{}\n", secret.to_base58())).expect("write");
        let table = |state_key: &std::path::Path| {
            config_with_raw_key_file(|key_path| {
                format!(
                    r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"

[zk_note]
vault = "{address}"
network = "mina:devnet"
graphql_url = "https://api.minascan.io/node/devnet/v1/graphql"
state_key_file = "{state_key}"
verification_key_hash = "1"
tiers = [100000000, 10000000]
allowance_lifetime_secs = 3600
open_budget_cap = 1000000000
"#,
                    key_path = key_path.display(),
                    state_dir = dir.path().display(),
                    state_key = state_key.display(),
                )
            })
            .0
        };
        let config = table(&state_key);
        let terms = zk_note_terms(config.zk_note().expect("a zk edge")).expect("terms");
        assert_eq!(
            terms,
            connector_domain::x402::X402ZkNoteTerms {
                network: "mina:devnet".to_string(),
                asset: "slip44:12586".to_string(),
                pay_to: address.clone(),
                verification_key_hash: "1".to_string(),
                state_key: address.clone(),
                tiers: vec!["10000000".to_string(), "100000000".to_string()],
                subsidy: None,
                subsidised_last_period: None,
                allowance_lifetime_secs: 3600,
                allowance_endpoint: "/ilp/zk-note/allowances".to_string(),
            }
        );

        let garbage = dir.path().join("garbage.key");
        std::fs::write(&garbage, "not a key").expect("write");
        let config = table(&garbage);
        assert!(matches!(
            zk_note_terms(config.zk_note().unwrap()),
            Err(RuntimeError::ZkNoteStateKey { .. })
        ));
    }

    /// A journal carrying a line this build cannot decode stops the node
    /// starting. The alternative -- skipping the line, or starting empty --
    /// is precisely the "silently start from zero" this ticket forbids.
    #[test]
    fn a_corrupt_client_edge_journal_refuses_to_build_a_gate() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        std::fs::write(
            state_dir.path().join(CLIENT_EDGE_JOURNAL),
            "not a journal entry\n",
        )
        .expect("write a corrupt journal");
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
                state_dir = state_dir.path().display(),
            )
        });

        let Err(error) = client_claim_gate(&config, None) else {
            panic!("a corrupt journal must not produce a gate");
        };
        assert!(matches!(error, RuntimeError::JournalUnreplayable { .. }));
    }

    /// Nothing writes the peer claim journal since ADR 0075 (#1380, #1385):
    /// a node built on a fresh `state_dir` does not create one. An older
    /// build's is only read, to be refused if it holds `toon-channel` claims.
    #[tokio::test]
    async fn a_fresh_state_dir_gets_no_peer_claim_journal() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
                state_dir = state_dir.path().display(),
            )
        });

        let _runtime = build(&config).await.expect("build");
        assert!(!state_dir.path().join(PEER_CLAIM_JOURNAL).exists());
    }

    /// A node with no `state_dir` still builds -- config load has already
    /// guaranteed it has no channel to accept a claim on, so it has no
    /// watermark a restart could lose.
    #[tokio::test]
    async fn a_node_with_no_state_dir_and_no_client_channels_still_builds() {
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        });

        let runtime = build(&config).await.expect("build");
        assert!(router(&runtime, &config).is_ok());
    }

    #[tokio::test]
    async fn router_mounts_the_operator_surface_when_configured() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[operator]
bearer_token = "operator-secret"
write_keys = ["{key}"]
"#,
                key_path.display()
            )
        });
        let runtime = build(&config).await.expect("build");
        let app = router(&runtime, &config).expect("router");

        // The operator surface is mounted: `/routes` is a real path now,
        // rejecting for lack of a bearer token rather than 404ing.
        let request = Request::builder()
            .uri("/routes")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// The node reads `controller_keys_file` itself, not the list it held
    /// at boot (ADR 0079): a key removed from the file is refused its next
    /// request by the same running router.
    #[tokio::test]
    async fn a_controller_key_removed_from_its_file_is_refused_with_no_restart() {
        use connector_operator::signing::{keyid_hex, sign_request};

        let secret = ed25519_dalek::SecretKey::from_bytes(&[7u8; 32]).expect("secret");
        let agent = ed25519_dalek::Keypair {
            public: (&secret).into(),
            secret,
        };
        let controllers = tempfile::NamedTempFile::new().expect("controller keys file");
        std::fs::write(
            controllers.path(),
            format!(
                "[agent]\nkey = \"{}\"\nscope = [\"channels-land\"]\n",
                keyid_hex(&agent)
            ),
        )
        .expect("write controller keys");
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[operator]
bearer_token = "operator-secret"
write_keys = ["0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]
controller_keys_file = "{}"
"#,
                key_path.display(),
                controllers.path().display()
            )
        });
        let runtime = build(&config).await.expect("build");
        let app = router(&runtime, &config).expect("router");
        let signed_read = |created: u64| {
            let (sig_input, sig, digest) =
                sign_request(&agent, "GET", "/peers", &[], created, Some(9_999_999_999));
            let request = Request::builder()
                .uri("/peers")
                .header("signature-input", sig_input)
                .header("signature", sig)
                .header("content-digest", digest)
                .body(Body::empty())
                .unwrap();
            app.clone().oneshot(request)
        };

        assert_eq!(signed_read(1_000).await.unwrap().status(), StatusCode::OK);
        std::fs::write(controllers.path(), "").expect("revoke");
        assert_eq!(
            signed_read(1_001).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    /// The dashboard is part of the operator surface (ADR 0066): mounted
    /// with it, and -- like `/metrics` -- a 404 rather than a page on a
    /// node that configured no `[operator]`, so an unconfigured node
    /// advertises nothing about having one.
    #[tokio::test]
    async fn the_dashboard_is_mounted_with_the_operator_surface_and_not_without_it() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let get_dashboard = |app: axum::Router| async move {
            let request = Request::builder()
                .uri("/dashboard")
                .body(Body::empty())
                .unwrap();
            app.oneshot(request).await.unwrap().status()
        };

        let (with_operator, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"

[operator]
bearer_token = "operator-secret"
write_keys = ["{key}"]
"#,
                key_path.display()
            )
        });
        let runtime = build(&with_operator).await.expect("build");
        let app = router(&runtime, &with_operator).expect("router");
        assert_eq!(get_dashboard(app).await, StatusCode::OK);

        let (without_operator, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        });
        let runtime = build(&without_operator).await.expect("build");
        let app = router(&runtime, &without_operator).expect("router");
        assert_eq!(get_dashboard(app).await, StatusCode::NOT_FOUND);
    }

    mod settlement_construction {
        use super::*;
        use connector_settlement_evm::test_support::x402::X402Chain;
        use connector_settlement_evm::test_support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};
        use connector_settlement_solana::test_support::{
            create_mint, fund, require_solana_test_validator, SolanaValidator,
        };
        use ethers::signers::Signer as EvmSigner;
        use solana_rpc_client::nonblocking::rpc_client::RpcClient;
        use solana_sdk::commitment_config::CommitmentConfig;
        use solana_sdk::signature::{Keypair, Signer as SolanaSigner};

        /// This test binary's own base port for [`Anvil::spawn`] -- distinct
        /// from other test binaries' bases so that binaries running
        /// concurrently under `cargo test --workspace` don't contend for the
        /// same port range.
        const ANVIL_BASE_PORT: u16 = 18_700;

        fn key_file_with(contents: &str) -> tempfile::TempPath {
            let mut file = tempfile::NamedTempFile::new().expect("temp key file");
            file.write_all(contents.as_bytes()).expect("write key file");
            file.into_temp_path()
        }

        /// A `[settlement.solana.key]` key file carrying `seed` as 32 raw
        /// bytes -- the ed25519 seed the Solana backend signs with, the
        /// Solana twin of [`key_file_with`]'s hex-encoded secp256k1 key.
        fn raw_key_file(seed: [u8; 32]) -> tempfile::TempPath {
            let mut file = tempfile::NamedTempFile::new().expect("temp key file");
            file.write_all(&seed).expect("write raw key file");
            file.into_temp_path()
        }

        fn evm_table(
            rpc_url: &str,
            token: ethers::types::Address,
            decimals: u8,
            key: &str,
        ) -> String {
            evm_table_with(
                rpc_url,
                token,
                decimals,
                key,
                "asset_eip712_name = \"USDC\"\nasset_eip712_version = \"2\"\n\
                 asset_transfer_method = \"permit2\"",
            )
        }

        /// [`evm_table`] with `deposit_terms` -- the asset's EIP-712 keys
        /// and, where written, `asset_transfer_method` -- in place of its
        /// own.
        fn evm_table_with(
            rpc_url: &str,
            token: ethers::types::Address,
            decimals: u8,
            key: &str,
            deposit_terms: &str,
        ) -> String {
            format!(
                r#"
[settlement.eip155.31337]
rpc_url = "{rpc_url}"
token_address = "{token:?}"
decimals = {decimals}
min_withdraw_delay_secs = 3600
{deposit_terms}
facilitator_url = "https://facilitator.example/x402"

[settlement.eip155.31337.key]
key_file = "{key}"
"#
            )
        }

        fn solana_table(rpc_url: &str, mint: Pubkey, decimals: u8, key: &str) -> String {
            format!(
                r#"
[settlement.solana]
rpc_url = "{rpc_url}"
token_address = "{mint}"
decimals = {decimals}
min_sponsored_deposit = 1000000
min_grace_period_secs = 3600

[settlement.solana.key]
key_file = "{key}"
"#
            )
        }

        fn node_config(signer_key: &str, tables: &str) -> Config {
            load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{signer_key}"
{tables}
"#
            ))
        }

        /// A validator with a funded settlement key and a 6-decimal mint.
        async fn solana_chain(validator: &SolanaValidator, seed: [u8; 32]) -> Pubkey {
            let rpc = RpcClient::new_with_commitment(
                validator.rpc_url.clone(),
                CommitmentConfig::confirmed(),
            );
            let authority = Keypair::new();
            fund(&rpc, &authority.pubkey()).await;
            let payer =
                solana_sdk::signer::keypair::keypair_from_seed(&seed).expect("derive keypair");
            fund(&rpc, &payer.pubkey()).await;
            create_mint(&rpc, &authority, 6).await
        }

        /// A node with no `[settlement]` section settles on no chain: it
        /// builds, holds no x402 backend and offers no `accepts[]` entry.
        #[tokio::test]
        async fn no_settlement_section_still_builds_with_no_backend() {
            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let config = node_config(&key_path.display().to_string(), "");
            let runtime = build(&config).await.expect("builds with no settlement");
            assert!(runtime.batch_settlement_evm.is_empty());
            assert!(runtime.batch_settlement_solana.is_none());
            assert!(runtime.batch_settlements.is_empty());
        }

        /// ADR 0075 decision 1: a `[settlement.evm]` table pointed at a chain
        /// without `x402BatchSettlement` refuses to build, by name.
        #[tokio::test]
        async fn an_evm_chain_without_x402_refuses_to_build_by_name() {
            if !require_anvil() {
                return;
            }
            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let token = connector_settlement_evm::test_support::deploy_plain_token(
                &anvil.rpc_url,
                DEPLOYER_PRIVATE_KEY,
                0,
            )
            .await;
            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let key = key_path.display().to_string();
            let config = node_config(&key, &evm_table(&anvil.rpc_url, token, 6, &key));

            let Err(error) = build(&config).await else {
                panic!("built on a chain without x402BatchSettlement");
            };
            assert!(
                matches!(&error, RuntimeError::X402NotDeployed { table, what } if table == "eip155.31337" && what.contains("x402BatchSettlement")),
                "{error}"
            );
            assert!(
                error.to_string().contains("[settlement.eip155.31337]"),
                "{error}"
            );
        }

        /// ADR 0086 decision 4: the table key is declared and asserted. A
        /// table keyed `8453` whose RPC answers `eth_chainId` with another
        /// id is refused by name, both ids in the message, before anything
        /// is bound -- a wrong RPC URL cannot settle on the wrong network.
        #[tokio::test]
        async fn an_evm_table_whose_rpc_reports_another_chain_id_refuses_to_build_by_name() {
            if !require_anvil() {
                return;
            }
            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let mut x402 = X402Chain::place(&anvil.rpc_url).await;
            let token = x402.deploy_fiat_token().await;
            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let key = key_path.display().to_string();
            let table =
                evm_table(&anvil.rpc_url, token, 6, &key).replace("eip155.31337", "eip155.8453");
            let config = node_config(&key, &table);

            let Err(error) = build(&config).await else {
                panic!("built a table keyed 8453 on a chain that reports 31337");
            };
            assert!(
                matches!(
                    &error,
                    RuntimeError::EvmChainIdMismatch { network, reported: 31337 }
                        if *network == Network::Eip155(8453)
                ),
                "{error}"
            );
            let message = error.to_string();
            assert!(
                message.contains("[settlement.eip155.8453]")
                    && message.contains("8453")
                    && message.contains("31337"),
                "the refusal names the table and both ids: {message}"
            );
        }

        /// ADR 0075 decision 1, Solana: a chain without `payment-channels`
        /// refuses to build, by name.
        #[tokio::test]
        async fn a_solana_chain_without_payment_channels_refuses_to_build_by_name() {
            if !require_solana_test_validator() {
                return;
            }
            let validator = SolanaValidator::spawn_without_payment_channels().await;
            let seed = [21u8; 32];
            let mint = solana_chain(&validator, seed).await;
            let signer_key = key_file_with(DEPLOYER_PRIVATE_KEY);
            let solana_key = raw_key_file(seed);
            let config = node_config(
                &signer_key.display().to_string(),
                &solana_table(
                    &validator.rpc_url,
                    mint,
                    6,
                    &solana_key.display().to_string(),
                ),
            );

            let Err(error) = build(&config).await else {
                panic!("built on a chain without payment-channels");
            };
            assert!(
                matches!(&error, RuntimeError::X402NotDeployed { table, what } if table == "solana" && what.contains("payment-channels")),
                "{error}"
            );
        }

        /// A node dealing two tokens on one network at a hand-tended rate
        /// carries its rate table on the runtime. No `quote` anywhere, so no
        /// poller is started and the table is a pure product of the file --
        /// but every token names its network (ADR 0086 decision 2), so the
        /// node holds that network's settlement table and boots against it.
        #[tokio::test]
        async fn a_dealing_node_carries_its_rate_table_on_the_runtime() {
            if !require_anvil() {
                return;
            }
            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let mut x402 = X402Chain::place(&anvil.rpc_url).await;
            let token = x402.deploy_fiat_token().await;
            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let key = key_path.display().to_string();
            let usdc = format!("eip155:31337/erc20:{token:#x}");
            let anyone = "eip155:31337/erc20:0x9ff58f4ffb29fa2266ab25e75e2a8b3503311656";
            let config = node_config(
                &key,
                &format!(
                    r#"{table}
[[tokens]]
asset = "{usdc}"
numeraire = true

[[tokens]]
asset = "{anyone}"

[[rates]]
from = "{anyone}"
to = "{usdc}"
rate = {{ numerator = 1, denominator = 4000000000000 }}

[rate_guards]
spread = {{ numerator = 30, denominator = 10000 }}
ttl_secs = 300
max_move = {{ numerator = 5, denominator = 100 }}
"#,
                    table = evm_table(&anvil.rpc_url, token, 6, &key),
                ),
            );

            let runtime = build(&config).await.expect("build");

            let table = runtime
                .rate_table
                .expect("a node that declares tokens deals");
            let held = table.read();
            let usdc: connector_domain::AssetId = usdc.parse().expect("an asset");
            let anyone: connector_domain::AssetId = anyone.parse().expect("an asset");
            assert_eq!(held.numeraire(), &usdc);
            assert!(
                held.lookup(&anyone, &usdc, chrono::Utc::now()).is_live(),
                "a declared row is in force from boot and never ages"
            );
        }

        /// Issue #564: a `decimals` the token disagrees with refuses to
        /// build rather than being a line with no effect.
        #[tokio::test]
        async fn settlement_decimals_the_token_disagrees_with_refuses_to_build() {
            if !require_anvil() {
                return;
            }
            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let mut x402 = X402Chain::place(&anvil.rpc_url).await;
            let token = x402.deploy_fiat_token().await;
            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let key = key_path.display().to_string();
            let config = node_config(&key, &evm_table(&anvil.rpc_url, token, 18, &key));

            let Err(error) = build(&config).await else {
                panic!("built over a decimals the token disagrees with");
            };
            let message = error.to_string();
            assert!(
                message.contains("decimals") && message.contains("18"),
                "{message}"
            );
        }

        /// The published EVM `assetTransferMethod` a built node offers.
        fn published_method(runtime: &Runtime) -> connector_domain::x402::X402AssetTransferMethod {
            runtime
                .batch_settlements
                .iter()
                .find_map(|entry| match entry {
                    connector_client_edge::X402BatchSettlementTerms::Evm(terms) => {
                        Some(terms.asset_transfer_method)
                    }
                    _ => None,
                })
                .expect("the batch-settlement list carries an EVM entry")
        }

        /// Issue #1422: a token with ERC-3009 -- Circle's FiatToken v2.2 --
        /// boots under `eip3009`, written or left at its default, with the
        /// asset's own EIP-712 domain.
        #[tokio::test]
        async fn an_erc3009_token_builds_under_eip3009() {
            if !require_anvil() {
                return;
            }
            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let mut x402 = X402Chain::place(&anvil.rpc_url).await;
            let token = x402.deploy_fiat_token().await;
            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let key = key_path.display().to_string();
            for deposit_terms in [
                "asset_eip712_name = \"USDC\"\nasset_eip712_version = \"2\"\n\
                 asset_transfer_method = \"eip3009\"",
                "asset_eip712_name = \"USDC\"\nasset_eip712_version = \"2\"",
            ] {
                let config = node_config(
                    &key,
                    &evm_table_with(&anvil.rpc_url, token, 6, &key, deposit_terms),
                );
                let runtime = build(&config)
                    .await
                    .expect("an ERC-3009 token builds under eip3009");
                assert_eq!(
                    published_method(&runtime),
                    connector_domain::x402::X402AssetTransferMethod::Eip3009
                );
            }
        }

        /// Issue #1422: a token without ERC-3009 is refused `eip3009` by
        /// name -- written, or left at its default -- and the refusal names
        /// `permit2` as the fix.
        #[tokio::test]
        async fn a_token_without_erc3009_refuses_to_build_under_eip3009() {
            if !require_anvil() {
                return;
            }
            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            X402Chain::place(&anvil.rpc_url).await;
            let token = connector_settlement_evm::test_support::deploy_plain_token(
                &anvil.rpc_url,
                DEPLOYER_PRIVATE_KEY,
                0,
            )
            .await;
            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let key = key_path.display().to_string();
            for deposit_terms in [
                "asset_eip712_name = \"USDC\"\nasset_eip712_version = \"2\"\n\
                 asset_transfer_method = \"eip3009\"",
                "asset_eip712_name = \"USDC\"\nasset_eip712_version = \"2\"",
            ] {
                let config = node_config(
                    &key,
                    &evm_table_with(&anvil.rpc_url, token, 6, &key, deposit_terms),
                );
                let Err(error) = build(&config).await else {
                    panic!("built eip3009 over a token without ERC-3009");
                };
                assert!(
                    matches!(&error, RuntimeError::AssetTransferMethodUnsupported { token: named, .. } if *named == format!("{token:#x}")),
                    "{error}"
                );
                let message = error.to_string();
                for named in [
                    "[settlement.eip155.31337]",
                    "asset_transfer_method = \"eip3009\"",
                    "ERC-3009",
                    "asset_transfer_method = \"permit2\"",
                ] {
                    assert!(message.contains(named), "expected {named} in: {message}");
                }
            }
        }

        /// Issue #1422: the same token builds under `permit2`, which moves
        /// any ERC-20, and publishes it.
        #[tokio::test]
        async fn a_token_without_erc3009_builds_under_permit2() {
            if !require_anvil() {
                return;
            }
            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            X402Chain::place(&anvil.rpc_url).await;
            let token = connector_settlement_evm::test_support::deploy_plain_token(
                &anvil.rpc_url,
                DEPLOYER_PRIVATE_KEY,
                0,
            )
            .await;
            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let key = key_path.display().to_string();
            let config = node_config(&key, &evm_table(&anvil.rpc_url, token, 6, &key));

            let runtime = build(&config)
                .await
                .expect("permit2 builds over a token without ERC-3009");
            assert_eq!(
                published_method(&runtime),
                connector_domain::x402::X402AssetTransferMethod::Permit2
            );
        }

        /// Issue #1422: under `eip3009`, an asset EIP-712 name or version
        /// that does not reproduce the token's own `DOMAIN_SEPARATOR()` is
        /// refused by name -- here Base mainnet USDC's `"USD Coin"` on a
        /// token named `"USDC"`, and the right name at the wrong version.
        /// Under `permit2` the same values are not judged.
        #[tokio::test]
        async fn an_asset_domain_the_token_disagrees_with_refuses_to_build_under_eip3009() {
            if !require_anvil() {
                return;
            }
            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let mut x402 = X402Chain::place(&anvil.rpc_url).await;
            let token = x402.deploy_fiat_token().await;
            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let key = key_path.display().to_string();
            for (name, version) in [("USD Coin", "2"), ("USDC", "1")] {
                let config = node_config(
                    &key,
                    &evm_table_with(
                        &anvil.rpc_url,
                        token,
                        6,
                        &key,
                        &format!(
                            "asset_eip712_name = \"{name}\"\nasset_eip712_version = \"{version}\""
                        ),
                    ),
                );
                let Err(error) = build(&config).await else {
                    panic!("built under an EIP-712 domain the token disagrees with");
                };
                assert!(
                    matches!(&error, RuntimeError::AssetEip712DomainMismatch { name: n, version: v, .. } if n == name && v == version),
                    "{error}"
                );
                let message = error.to_string();
                for named in [
                    "asset_eip712_name",
                    "asset_eip712_version",
                    "DOMAIN_SEPARATOR",
                ] {
                    assert!(message.contains(named), "expected {named} in: {message}");
                }

                let config = node_config(
                    &key,
                    &evm_table_with(
                        &anvil.rpc_url,
                        token,
                        6,
                        &key,
                        &format!(
                            "asset_eip712_name = \"{name}\"\nasset_eip712_version = \"{version}\"\n\
                             asset_transfer_method = \"permit2\""
                        ),
                    ),
                );
                build(&config)
                    .await
                    .expect("under permit2 the asset domain is not judged");
            }
        }

        #[tokio::test]
        async fn solana_decimals_the_mint_disagrees_with_refuses_to_build() {
            if !require_solana_test_validator() {
                return;
            }
            let validator = SolanaValidator::spawn().await;
            let seed = [22u8; 32];
            let mint = solana_chain(&validator, seed).await;
            let signer_key = key_file_with(DEPLOYER_PRIVATE_KEY);
            let solana_key = raw_key_file(seed);
            let config = node_config(
                &signer_key.display().to_string(),
                &solana_table(
                    &validator.rpc_url,
                    mint,
                    9,
                    &solana_key.display().to_string(),
                ),
            );

            let Err(error) = build(&config).await else {
                panic!("built over a decimals the mint disagrees with");
            };
            let message = error.to_string();
            assert!(
                message.contains("decimals") && message.contains('9'),
                "{message}"
            );
        }

        /// ADR 0074 decision 8, issue #1345, end to end: a node settling on
        /// both chains composes both chains' x402 batch-settlement facts and
        /// voucher signers, read off the very backends it connected -- so
        /// the greeting can never name a deployment the node is not on
        /// (CF-26). And, since ADR 0087 (#1614), beside an XRPL table's, here
        /// over a scripted ledger.
        #[tokio::test]
        async fn a_both_chains_config_composes_both_chains_batch_settlement_facts() {
            if !require_anvil() {
                return;
            }
            if !require_solana_test_validator() {
                return;
            }

            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let mut x402 = X402Chain::place(&anvil.rpc_url).await;
            let token = x402.deploy_fiat_token().await;
            let evm_settlement_address = DEPLOYER_PRIVATE_KEY
                .parse::<ethers::signers::LocalWallet>()
                .expect("key")
                .address();

            let validator = SolanaValidator::spawn().await;
            let seed = [23u8; 32];
            let token_mint = solana_chain(&validator, seed).await;
            let sponsor = solana_sdk::signer::keypair::keypair_from_seed(&seed)
                .expect("derive keypair")
                .pubkey();

            // An XRPL ledger that answers just what boot reads: its network,
            // its reserve and the node's funded account.
            let xrpl_seed = connector_signer::xrpl_family_seed(
                connector_signer::XrplAlgorithm::Secp256k1,
                &[41; 16],
            );
            let xrpl_node =
                connector_signer::XrplSigner::from_family_seed(&xrpl_seed).expect("a seed");
            let xrpl_address = xrpl_node.classic_address();
            let ledger_account = xrpl_address.clone();
            let ledger = connector_chain_rpc::FakeRpc::spawn(move |call| {
                use connector_chain_rpc::RpcReply;
                match call.method.as_str() {
                    "server_info" => RpcReply::Result(serde_json::json!({
                        "status": "success",
                        "info": {
                            "network_id": 1000,
                            "server_state": "full",
                            "validated_ledger": { "seq": 4, "reserve_base_xrp": 1 },
                        },
                    })),
                    "account_info" if call.params[0]["account"] == ledger_account => {
                        RpcReply::Result(serde_json::json!({
                            "status": "success",
                            "account_data": {
                                "Balance": "100000000", "Sequence": 1, "OwnerCount": 0,
                            },
                        }))
                    }
                    _ => RpcReply::Result(serde_json::json!({
                        "status": "error", "error": "actNotFound", "error_message": "no",
                    })),
                }
            })
            .await;
            let xrpl_key_path = key_file_with(&xrpl_seed);

            let evm_key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let evm_key = evm_key_path.display().to_string();
            let solana_key_path = raw_key_file(seed);
            let config = node_config(
                &evm_key,
                &format!(
                    "{}{}\n[settlement.xrpl.1000]\nrpc_url = \"{}\"\n\n\
                     [settlement.xrpl.1000.key]\nkey_file = \"{}\"\n",
                    evm_table(&anvil.rpc_url, token, 6, &evm_key),
                    solana_table(
                        &validator.rpc_url,
                        token_mint,
                        6,
                        &solana_key_path.display().to_string()
                    ),
                    ledger.url(),
                    xrpl_key_path.display(),
                ),
            );

            let runtime = build(&config)
                .await
                .expect("all three chains bind without any refusing startup");
            assert!(!runtime.batch_settlement_evm.is_empty());
            assert!(runtime.batch_settlement_solana.is_some());
            assert_eq!(runtime.batch_settlement_xrpl.len(), 1);

            // No `{:?}` of the facts: they carry keys derived from the
            // settlement key files (rust/cleartext-logging).
            assert_eq!(runtime.batch_settlements.len(), 3, "all three chains");
            assert_eq!(runtime.voucher_signers.len(), 3, "all three chains");

            let xrpl_batch = runtime
                .batch_settlements
                .iter()
                .find_map(|entry| match entry {
                    connector_client_edge::X402BatchSettlementTerms::Xrpl(terms) => {
                        Some(terms.clone())
                    }
                    _ => None,
                })
                .expect("the batch-settlement list carries an XRPL entry");
            assert_eq!(xrpl_batch.network, "xrpl:1000");
            assert_eq!(xrpl_batch.asset, "xrpl:1000/slip44:144");
            assert_eq!(xrpl_batch.pay_to, xrpl_address);
            assert_eq!(xrpl_batch.binding, "xrpl-paychan-v1");
            assert_eq!(xrpl_batch.min_settle_delay_secs, 86_400);
            assert_eq!(xrpl_batch.redemption_safety_secs, 60);
            assert!(
                runtime
                    .voucher_signers
                    .iter()
                    .any(|fact| fact.network == "xrpl:1000"
                        && fact.signer
                            == xrpl_node
                                .public_key()
                                .iter()
                                .map(|byte| format!("{byte:02x}"))
                                .collect::<String>()),
                "the XRPL channel key is a voucher signer"
            );

            let evm_batch = runtime
                .batch_settlements
                .iter()
                .find_map(|entry| match entry {
                    connector_client_edge::X402BatchSettlementTerms::Evm(terms) => {
                        Some(terms.clone())
                    }
                    _ => None,
                })
                .expect("the batch-settlement list carries an EVM entry");
            assert_eq!(
                evm_batch.network, "eip155:31337",
                "the chain id the connected backend read, spelled CAIP-2"
            );
            assert_eq!(evm_batch.asset, format!("{token:#x}"));
            assert_eq!(evm_batch.pay_to, format!("{evm_settlement_address:#x}"));
            assert_eq!(
                evm_batch.receiver_authorizer,
                format!("{evm_settlement_address:#x}"),
                "receiverAuthorizer is never delegated (ADR 0074 decision 5): it is always this \
                 node's own settlement address"
            );
            assert_eq!(evm_batch.min_withdraw_delay_secs, 3600);
            assert_eq!(evm_batch.name, "USDC");
            assert_eq!(evm_batch.version, "2");
            assert_eq!(
                evm_batch.asset_transfer_method,
                connector_domain::x402::X402AssetTransferMethod::Permit2,
                "the configured deposit method, not x402's default (toon-client#695)"
            );
            assert_eq!(
                evm_batch.facilitator.as_deref(),
                Some("https://facilitator.example/x402"),
                "the configured facilitator, verbatim (toon-client#695)"
            );

            let solana_batch = runtime
                .batch_settlements
                .iter()
                .find_map(|entry| match entry {
                    connector_client_edge::X402BatchSettlementTerms::Solana(terms) => {
                        Some(terms.clone())
                    }
                    _ => None,
                })
                .expect("the batch-settlement list carries a Solana entry");
            assert!(
                solana_batch.network.starts_with("solana:"),
                "got {}",
                solana_batch.network
            );
            assert_eq!(solana_batch.asset, token_mint.to_string());
            assert_eq!(solana_batch.pay_to, sponsor.to_string());
            assert_eq!(
                solana_batch.pay_to, solana_batch.fee_payer,
                "the sponsor is the receiving operator (ADR 0074 decision 5): one settlement \
                 key, both roles"
            );
            assert_eq!(solana_batch.min_grace_period_secs, 3600);
            assert_eq!(
                solana_batch.min_deposit, "1000000",
                "the sponsor's minimum deposit is published (ADR 0074 decision 5)"
            );
            assert_eq!(
                solana_batch.token_program,
                spl_token::id().to_string(),
                "x402's required tokenProgram is the one program the backend proved owns the \
                 mint, and the only one the sponsor co-signs an open under (issue #1357)"
            );
            assert_eq!(
                solana_batch.sponsor_endpoint,
                crate::sponsor::SPONSOR_PATH,
                "the greeting names the path the router actually mounts (issue #1357)"
            );
        }
    }

    /// ADR 0071 decision 6, issue #1294: what a dealing node's config
    /// produces at boot, and what a node that declares nothing produces
    /// instead.
    mod denomination {
        use super::*;
        use connector_domain::AssetId;
        use connector_rate_source::{InMemoryRateSource, PoolContents, PoolId};

        /// USDC on Base -- the numeraire, and this node's settlement token.
        const USDC: &str = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
        /// USDC on Solana: the same asset on the other chain, which is why
        /// the pair below is 1:1 and is declared rather than sourced.
        const USDC_SOLANA: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
        /// ANYONE on Base: 18 decimals against USDC's 6, quoted through one
        /// operator-named pool.
        const ANYONE: &str = "0x9ff58f4ffb29fa2266ab25e75e2a8b3503311656";
        const POOL_ANYONE_USDC: &str = "0x1111111111111111111111111111111111111111";
        /// The mock SPL mint `local/dealing` uses, here a Solana-side
        /// node's dealt token. A quote path on Solana is only expressible
        /// when the numeraire is on Solana too -- every leg stays on the
        /// token's own chain and the last one lands on the numeraire -- so
        /// `USDC_SOLANA` is that node's numeraire and this is what it
        /// deals.
        const SOL_DEALT: &str = "5i3gfxLCbMdWppYxEsa55MNLkxwAZHsm3SqoJWyKFckX";
        /// A base58 32-byte pool name, which is all a Solana pool is here.
        const POOL_SOL_DEALT_USDC: &str = "58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2";

        fn asset(text: &str) -> AssetId {
            text.parse::<AssetId>().expect("a declared asset")
        }

        /// The easy path, and every node that predates ADR 0071: no token,
        /// no numeraire, no table, nothing started.
        #[tokio::test]
        async fn a_node_that_declares_no_tokens_carries_no_rate_table() {
            let (config, _key_path) = config_with_raw_key_file(|key_path| {
                format!(
                    r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{}"
"#,
                    key_path.display()
                )
            });

            let runtime = build(&config).await.expect("build");

            assert!(runtime.rate_table.is_none());
        }

        /// The seam issue #1293's reader drops into: a declared quote path,
        /// a source behind the port, and a poller spawned per path that
        /// prices the pair without anything on the packet path asking it to.
        ///
        /// Against the in-memory source, which upholds the port's own
        /// contract suite -- a fake, not a stub (ADR 0007). `start_paused`
        /// asserts the schedule rather than spending it in real seconds.
        #[tokio::test(start_paused = true)]
        async fn a_spawned_poller_prices_a_declared_quote_path() {
            let observed_at = chrono::Utc::now();
            let (config, _key_path) = config_with_raw_key_file(|key_path| {
                format!(
                    r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_file}"

[settlement.eip155.31337]
rpc_url = "http://127.0.0.1:8545"
token_address = "{USDC}"
decimals = 6
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[settlement.eip155.31337.key]
key_file = "{key_file}"

[[tokens]]
asset = "eip155:31337/erc20:{USDC}"
numeraire = true

[[tokens]]
asset = "eip155:31337/erc20:{ANYONE}"
quote = [
  {{ pool = "{POOL_ANYONE_USDC}", quote_token = "eip155:31337/erc20:{USDC}", twap_window_secs = 1800 }},
]

[rate_guards]
spread = {{ numerator = 30, denominator = 10000 }}
ttl_secs = 300
max_move = {{ numerator = 5, denominator = 100 }}
"#,
                    key_file = key_path.display()
                )
            });
            // `build` is not called here: it would dial the RPC endpoint
            // that settlement table names, and this test is about the
            // poller, not about a chain.
            let table = SharedRateTable::from_config(config.denomination())
                .expect("a node that declares tokens deals");
            let source = Arc::new(InMemoryRateSource::new());
            source.insert_pool(
                PoolId(POOL_ANYONE_USDC.to_string()),
                PoolContents {
                    base: asset(&format!("eip155:31337/erc20:{ANYONE}")),
                    quote: asset(&format!("eip155:31337/erc20:{USDC}")),
                    rate: connector_domain::Rate::new(4, 1_000_000_000_000).expect("a rate"),
                    observed_at,
                    shortest_window: chrono::Duration::seconds(60),
                    longest_window: chrono::Duration::seconds(3600),
                },
            );

            let started = spawn_rate_pollers(
                &table,
                &RateSources::reading(Network::Eip155(31337), source),
                &config,
            )
            .expect("a usable quote path");

            assert_eq!(started, 1, "one token declared a quote path");
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            assert!(
                table
                    .lookup(
                        &asset(&format!("eip155:31337/erc20:{ANYONE}")),
                        &asset(&format!("eip155:31337/erc20:{USDC}")),
                        observed_at,
                    )
                    .is_live(),
                "the spawned poller read the path without anything asking it to"
            );
        }

        /// The wiring `build` now does for real: a declared quote path is
        /// read over the `rpc_url` of the settlement table for the token's
        /// own chain, and the poller is running before the node serves
        /// anything.
        ///
        /// No chain is dialed to assert it, and that is the reader's own
        /// design -- `UniswapV3RateSource::connect` touches nothing, so a
        /// source pointed at an endpoint nothing answers on is still a
        /// source, and what this test proves is that production picks one
        /// up and starts the poller rather than logging that it cannot.
        #[tokio::test]
        async fn a_declared_quote_path_is_polled_over_its_own_chains_endpoint() {
            let (config, _key_path) = config_with_raw_key_file(|key_path| {
                format!(
                    r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_file}"

[settlement.eip155.31337]
rpc_url = "http://127.0.0.1:8545"
token_address = "{USDC}"
decimals = 6
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[settlement.eip155.31337.key]
key_file = "{key_file}"

[[tokens]]
asset = "eip155:31337/erc20:{USDC}"
numeraire = true

[[tokens]]
asset = "eip155:31337/erc20:{ANYONE}"
quote = [
  {{ pool = "{POOL_ANYONE_USDC}", quote_token = "eip155:31337/erc20:{USDC}", twap_window_secs = 1800 }},
]

[rate_guards]
spread = {{ numerator = 30, denominator = 10000 }}
ttl_secs = 300
max_move = {{ numerator = 5, denominator = 100 }}
"#,
                    key_file = key_path.display()
                )
            });
            let table = SharedRateTable::from_config(config.denomination())
                .expect("a node that declares tokens deals");

            let started = spawn_quote_path_pollers(
                &table,
                &config,
                &settlement_transports(&config).expect("transports"),
            )
            .expect("a quote path on the chain this node settles on");

            assert_eq!(started, 1, "the one declared quote path is polled");
        }

        /// Issue #1302: a declared quote path on a chain no rate source
        /// linked into this binary can read refuses startup, rather than
        /// being handed the EVM reader and warned about.
        ///
        /// This is the whole of the loose end. `Config::load` accepts this
        /// file -- the quote is on the token's own settlement chain, that
        /// chain has a `[settlement]` table, and the path ends at the
        /// numeraire -- and before the fix its one quoted token got the EVM
        /// reader, whose every tick failed; one `ttl` later every forward
        /// across the pair refused, with a boot `warn!` as the only clue.
        /// `spawn_quote_path_pollers` is called from [`build`], so this
        /// error is a node that does not start.
        #[tokio::test]
        async fn a_quote_path_on_a_chain_no_source_reads_refuses_to_start() {
            let state_dir = tempfile::tempdir().expect("temp state dir");
            let (config, _key_path) = config_with_raw_key_file(|key_path| {
                format!(
                    r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[settlement.solana]
rpc_url = "http://127.0.0.1:8899"
token_address = "{USDC_SOLANA}"
decimals = 6
min_sponsored_deposit = 1

[settlement.solana.key]
key_file = "{key_file}"

[[tokens]]
asset = "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1/token:{USDC_SOLANA}"
numeraire = true

[[tokens]]
asset = "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1/token:{SOL_DEALT}"
quote = [
  {{ pool = "{POOL_SOL_DEALT_USDC}", quote_token = "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1/token:{USDC_SOLANA}", twap_window_secs = 900 }},
]

[rate_guards]
spread = {{ numerator = 30, denominator = 10000 }}
ttl_secs = 300
max_move = {{ numerator = 5, denominator = 100 }}
"#,
                    state_dir = state_dir.path().display(),
                    key_file = key_path.display()
                )
            });
            let table = SharedRateTable::from_config(config.denomination())
                .expect("a node that declares tokens deals");

            let refused = spawn_quote_path_pollers(
                &table,
                &config,
                &settlement_transports(&config).expect("transports"),
            )
            .expect_err("no source reads Solana pools");

            assert!(
                matches!(
                    &refused,
                    RuntimeError::QuotePathUnpollable {
                        source: QuotePathUnusable::NoSourceForChain {
                            token,
                            network: Network::Solana(_),
                        },
                    } if token == &asset(&format!("solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1/token:{SOL_DEALT}"))
                ),
                "the refusal names the token and the chain: {refused:?}"
            );
            let said = refused.to_string();
            assert!(
                said.contains("no rate source") && said.contains("solana"),
                "and says which refusal it is -- a chain nothing reads, not a chain with no \
                 settlement table: {said}"
            );
        }

        /// The other half, unchanged by the wiring: a node whose every pair
        /// is a static `[[rates]]` row has nothing to poll and no endpoint
        /// to poll it over. It starts nothing. (Its tokens still need the
        /// settlement tables their networks name, ADR 0086 decision 2.)
        #[tokio::test]
        async fn a_node_with_no_quote_path_starts_no_poller() {
            let (config, _key_path) = config_with_raw_key_file(|key_path| {
                format!(
                    r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_file}"

[settlement.eip155.31337]
rpc_url = "http://127.0.0.1:8545"
token_address = "{USDC}"
decimals = 6
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[settlement.eip155.31337.key]
key_file = "{key_file}"

[settlement.solana]
rpc_url = "http://127.0.0.1:8899"
token_address = "{USDC_SOLANA}"
decimals = 6
min_sponsored_deposit = 1

[settlement.solana.key]
key_file = "{key_file}"

[[tokens]]
asset = "eip155:31337/erc20:{USDC}"
numeraire = true

[[tokens]]
asset = "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1/token:{USDC_SOLANA}"

[[rates]]
from = "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1/token:{USDC_SOLANA}"
to = "eip155:31337/erc20:{USDC}"
rate = {{ numerator = 1, denominator = 1 }}

[rate_guards]
spread = {{ numerator = 30, denominator = 10000 }}
ttl_secs = 300
max_move = {{ numerator = 5, denominator = 100 }}
"#,
                    key_file = key_path.display()
                )
            });
            let table = SharedRateTable::from_config(config.denomination())
                .expect("a node that declares tokens deals");

            assert_eq!(
                spawn_quote_path_pollers(
                    &table,
                    &config,
                    &settlement_transports(&config).expect("transports")
                )
                .expect("nothing to poll is not an error"),
                0
            );
        }
    }

    /// ADR 0073 decisions 2 and 3 at the seam where the node builds its
    /// settlement clients: a table with `rpc_via_socks_proxy = true` reaches
    /// its endpoint through the node's `socks_proxy`, as a name, on its own
    /// chain's circuit, and every client built from its one transport does.
    ///
    /// Both endpoints are onion names, which nothing on this machine
    /// resolves, and the fake RPCs behind them listen on loopback. So a
    /// request that reached a fake at all went through the SOCKS5 server's
    /// route table: the dial was proxied and deferred resolution to the
    /// proxy. The proxy's own record then says which username each chain
    /// authenticated with.
    mod settlement_rpc_route {
        use super::*;
        use std::collections::HashMap;

        use connector_chain_rpc::evm::EvmRpc;
        use connector_chain_rpc::solana::rpc_client;
        use connector_chain_rpc::{FakeRpc, RpcReply};
        use connector_rate_source::{PoolId, QuoteLeg, RateSource};
        use connector_runtime::Socks5TestServer;
        use ethers::providers::Middleware;
        use solana_rpc_client::rpc_client::RpcClientConfig;
        use solana_sdk::commitment_config::CommitmentConfig;

        const EVM_HOST: &str = "evmsettlementrpcevmsettlementrpcevmsettlementrpcevmsett.onion";
        const SOLANA_HOST: &str = "solanasettlementrpcsolanasettlementrpcsolanasettlementr.anyone";

        fn config(proxy: &url::Url, evm_proxied: bool, key_path: &std::path::Path) -> Config {
            load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "/tmp/connector-settlement-rpc-route"
socks_proxy = "{proxy}"

[signer]
key_file = "{key}"

[settlement.eip155.31337]
rpc_url = "http://{EVM_HOST}/"
token_address = "0x00000000000000000000000000000000000000bb"
decimals = 6
rpc_via_socks_proxy = {evm_proxied}
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[settlement.eip155.31337.key]
key_file = "{key}"

[settlement.solana]
rpc_url = "http://{SOLANA_HOST}/"
token_address = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU"
decimals = 6
rpc_via_socks_proxy = true
min_sponsored_deposit = 1

[settlement.solana.key]
key_file = "{key}"
"#,
                key = key_path.display(),
            ))
        }

        #[tokio::test]
        async fn every_client_of_a_proxied_table_rides_the_proxy_on_that_chains_circuit() {
            let evm_rpc = FakeRpc::spawn(|call| match call.method.as_str() {
                "eth_blockNumber" => RpcReply::Result(serde_json::json!("0x2a")),
                _ => RpcReply::Error {
                    code: -32601,
                    message: "not served by this fake".to_string(),
                },
            })
            .await;
            let solana_rpc = FakeRpc::spawn(|_| RpcReply::Result(serde_json::json!(7))).await;
            let proxy = Socks5TestServer::spawn(HashMap::from([
                (format!("{EVM_HOST}:80"), evm_rpc.addr()),
                (format!("{SOLANA_HOST}:80"), solana_rpc.addr()),
            ]))
            .await;
            let key = tempfile::NamedTempFile::new().expect("key file");
            let config = config(&proxy.proxy_url(), true, key.path());

            let transports = settlement_transports(&config).expect("transports");
            let evm = evm_transport(&transports, 31337);
            let solana = solana_transport(&transports);
            assert!(evm.is_proxied() && solana.is_proxied());

            // The two EVM clients `build` makes from the one transport.
            assert_eq!(
                EvmRpc::provider(evm.clone())
                    .get_block_number()
                    .await
                    .expect("the backend's reads, through the proxy")
                    .as_u64(),
                42
            );
            let _ = UniswapV3RateSource::connect(evm)
                .observe(&QuoteLeg {
                    pool: PoolId("0x00000000000000000000000000000000000000cc".to_string()),
                    base: connector_domain::AssetId::erc20(
                        31337,
                        "0x00000000000000000000000000000000000000bb",
                    ),
                    quote: connector_domain::AssetId::erc20(
                        31337,
                        "0x00000000000000000000000000000000000000dd",
                    ),
                    window: chrono::Duration::seconds(60),
                })
                .await;
            assert!(
                evm_rpc.count("eth_getBlockByNumber") >= 1,
                "the rate source's read reached the onion-named endpoint, so it was proxied"
            );

            // The Solana backend's client, on its own circuit.
            let client = rpc_client(
                solana,
                RpcClientConfig::with_commitment(CommitmentConfig::confirmed()),
            );
            assert_eq!(client.get_slot().await.expect("through the proxy"), 7);

            let connects = proxy.connects();
            assert!(
                connects.iter().any(|c| c.target == format!("{EVM_HOST}:80")
                    && c.username.as_deref()
                        == Some(
                            Circuit::EvmSettlement { chain_id: 31337 }
                                .socks_username()
                                .as_str()
                        )),
                "{connects:?}"
            );
            assert!(
                connects
                    .iter()
                    .any(|c| c.target == format!("{SOLANA_HOST}:80")
                        && c.username.as_deref()
                            == Some(Circuit::SolanaSettlement.socks_username().as_str())),
                "{connects:?}"
            );
            assert!(
                connects.iter().all(|c| c.username.is_some()),
                "no settlement dial went through the proxy unpinned: {connects:?}"
            );
        }

        #[tokio::test]
        async fn a_table_that_does_not_opt_in_dials_direct_beside_one_that_does() {
            let proxy = Socks5TestServer::spawn_recording_only().await;
            let key = tempfile::NamedTempFile::new().expect("key file");
            let config = config(&proxy.proxy_url(), false, key.path());

            let transports = settlement_transports(&config).expect("transports");
            assert!(!evm_transport(&transports, 31337).is_proxied());
            assert!(solana_transport(&transports).is_proxied());
        }

        fn evm_transport(transports: &SettlementTransports, chain_id: u64) -> &RpcTransport {
            transports
                .evm
                .get(&Network::Eip155(chain_id))
                .expect("a transport per EVM table")
        }

        fn solana_transport(transports: &SettlementTransports) -> &RpcTransport {
            transports.solana.as_ref().expect("the Solana table's")
        }

        const SECOND_EVM_HOST: &str =
            "secondevmrpcsecondevmrpcsecondevmrpcsecondevmrpcsecondevm.onion";

        /// Two EVM tables, each opting in or not as `proxied` says, on two
        /// onion hosts.
        fn two_evm_config(
            proxy: &url::Url,
            proxied: [bool; 2],
            key_path: &std::path::Path,
        ) -> Config {
            let table = |chain_id: u64, host: &str, proxied: bool| {
                format!(
                    r#"
[settlement.eip155.{chain_id}]
rpc_url = "http://{host}/"
token_address = "0x00000000000000000000000000000000000000bb"
decimals = 6
rpc_via_socks_proxy = {proxied}
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[settlement.eip155.{chain_id}.key]
key_file = "{key}"
"#,
                    key = key_path.display(),
                )
            };
            load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "/tmp/connector-settlement-rpc-route"
socks_proxy = "{proxy}"

[signer]
key_file = "{key}"
{first}{second}"#,
                key = key_path.display(),
                first = table(31337, EVM_HOST, proxied[0]),
                second = table(31338, SECOND_EVM_HOST, proxied[1]),
            ))
        }

        /// ADR 0086 decision 7 (amending ADR 0073 decision 3): under
        /// `rpc_via_socks_proxy`, each EVM table's RPC rides a circuit of its
        /// own, pinned by a SOCKS username naming its network -- so no one
        /// exit sees two of this node's networks.
        #[tokio::test]
        async fn each_evm_table_rides_a_circuit_of_its_own() {
            let answer = |call: &connector_chain_rpc::RpcCall| match call.method.as_str() {
                "eth_blockNumber" => RpcReply::Result(serde_json::json!("0x2a")),
                _ => RpcReply::Error {
                    code: -32601,
                    message: "not served by this fake".to_string(),
                },
            };
            let first_rpc = FakeRpc::spawn(answer).await;
            let second_rpc = FakeRpc::spawn(answer).await;
            let proxy = Socks5TestServer::spawn(HashMap::from([
                (format!("{EVM_HOST}:80"), first_rpc.addr()),
                (format!("{SECOND_EVM_HOST}:80"), second_rpc.addr()),
            ]))
            .await;
            let key = tempfile::NamedTempFile::new().expect("key file");
            let config = two_evm_config(&proxy.proxy_url(), [true, true], key.path());

            let transports = settlement_transports(&config).expect("transports");
            for chain_id in [31337, 31338] {
                let transport = evm_transport(&transports, chain_id);
                assert!(transport.is_proxied());
                EvmRpc::provider(transport.clone())
                    .get_block_number()
                    .await
                    .expect("through the proxy");
            }

            let connects = proxy.connects();
            for (host, circuit) in [
                (EVM_HOST, "toon-settlement-eip155-31337"),
                (SECOND_EVM_HOST, "toon-settlement-eip155-31338"),
            ] {
                let dials: Vec<_> = connects
                    .iter()
                    .filter(|c| c.target == format!("{host}:80"))
                    .collect();
                assert!(!dials.is_empty(), "{host} was dialed: {connects:?}");
                assert!(
                    dials.iter().all(|c| c.username.as_deref() == Some(circuit)),
                    "every dial to {host} is on circuit {circuit}: {connects:?}"
                );
            }
        }

        /// ADR 0086 decision 7: `rpc_via_socks_proxy` is each table's own --
        /// one EVM network on the circuit, another dialed direct.
        #[tokio::test]
        async fn rpc_via_socks_proxy_is_per_evm_table() {
            let proxy = Socks5TestServer::spawn_recording_only().await;
            let key = tempfile::NamedTempFile::new().expect("key file");
            let config = two_evm_config(&proxy.proxy_url(), [true, false], key.path());

            let transports = settlement_transports(&config).expect("transports");
            assert!(evm_transport(&transports, 31337).is_proxied());
            assert!(!evm_transport(&transports, 31338).is_proxied());
        }
    }
}
