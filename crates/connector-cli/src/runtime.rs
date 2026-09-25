//! Builds the live [`Connector`] and its signer from a validated [`Config`],
//! and merges the client-edge and operator routers into the one
//! [`axum::Router`] the binary serves. Per ADR 0001 this is where every
//! construction decision lives -- `connector-bin` calls exactly
//! [`build`] and [`router`] and branches on neither.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;

use connector_chain_rpc::{Circuit, RpcTransport};
use connector_client_edge::{
    ChannelLivenessPolicy, ChannelLookupFailed, ClientChannelRegistry, ClientChannelSource,
    ClientClaimGate, ClientPayoutLedger, DepositFloor, EvmChannel, PeerCarriages, SolanaChannel,
    UnresolvableLookupBudgetPolicy,
};
use connector_config::{
    is_onion_endpoint, ClientChannelConfig, Config, EvmSettlementConfig, PayChannelConfig,
    PeerCarriage, PeerChannelConfig, SecretLocation, SettlementChain, SettlementConfig,
    SolanaSettlementConfig,
};
use connector_domain::AssetChain;
use connector_rate_source_evm::UniswapV3RateSource;
use connector_runtime::{
    BoundedHttpSelfDescription, ChannelDomain, ClaimStateChallengeSigner, Connector, DeclaredRates,
    EvmDomain, FileJournal, HttpAppClient, InMemoryJournal, Journal, JournalError,
    OutboundClientError, OutboundClientLedger, OwnedHttpClaimState, PeerRegistrar, PeerRoute,
    PeerRouteStore, PeerRouteStoreError, PeerTransport, QuotePathUnusable, RatePoller, RateSources,
    SharedRateTable, SystemClock,
};
use connector_settlement::batch::{BatchSettlementBackend, BatchSettlementError, HeldVouchers};
use connector_settlement::{SettlementBackend, SettlementError};
use connector_settlement_evm::{
    ChannelIndexLookup, EvmBatchSettlementBackend, EvmBatchWatcher, EvmChannelIndex,
    EvmChannelIndexSyncer, EvmSettlementBackend, IndexedContract, DEFAULT_POLL_INTERVAL,
};
use connector_settlement_solana::batch::{SolanaBatchSettlement, SolanaBatchWatcher};
use connector_settlement_solana::SolanaSettlementBackend;
use connector_signer::{
    derive_evm_address, Ed25519Signer, LocalEd25519Signer, LocalSigner, Signer, SignerError,
};

use crate::batch_settlement::{
    restore_journaled_channels, BatchSettlementChannelsAdapter, ClaimGateVouchers,
};
use crate::peer_transport;
use ethers::types::U256;
use solana_sdk::pubkey::Pubkey;

/// The two EIP-712 domains a startup refusal names when a declared channel
/// domain and this node's own deployment disagree (issue #1136).
///
/// `declared` is what the config file writes on the row -- ADR 0024's
/// *"configured input, per channel"*. `settled` is what
/// [`EvmSettlementBackend::connect`] resolved from `[settlement.evm]`'s
/// `TokenNetworkRegistry`: the `TokenNetwork` this node actually submits a
/// redemption to, and therefore the only `verifyingContract` whose
/// signatures it can ever collect on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmDomainMismatch {
    /// The channel the disagreeing row names -- an EVM `channel_id`, in the
    /// canonical lowercase `0x`-hex spelling `Config::load` produced.
    pub channel_id: String,
    /// The domain the row declares, and therefore the one every claim on
    /// this channel is signed and verified under.
    pub declared: EvmDomain,
    /// The domain the chain answered with when this node connected.
    pub settled: EvmDomain,
}

impl fmt::Display for EvmDomainMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "channel '{channel}' declares chain id {declared_chain} and TokenNetwork \
             {declared_network:#x}, but this node settles on chain id {settled_chain} through \
             TokenNetwork {settled_network:#x} -- the contract \
             TokenNetworkRegistry.getTokenNetwork(token_address) resolved for [settlement.evm]'s \
             own contract_address and token_address",
            channel = self.channel_id,
            declared_chain = self.declared.chain_id,
            declared_network = ethers::types::Address::from(self.declared.token_network),
            settled_chain = self.settled.chain_id,
            settled_network = ethers::types::Address::from(self.settled.token_network),
        )
    }
}

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
    /// Constructing the configured settlement backend failed -- e.g. the
    /// RPC endpoint was unreachable, or the contract address named in
    /// config has no code at it.
    Settlement(SettlementError),
    /// `state_dir` names a directory this node cannot create or write a
    /// journal file in (issue #605) -- typically a read-only mount, or a
    /// directory owned by another uid than the one the container runs as.
    /// A startup failure on purpose: the alternative is a node that serves
    /// happily and hands out free service after its next restart.
    StateDirUnusable {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A journal under `state_dir` exists but could not be replayed --
    /// unreadable, or carrying a line this build cannot decode. Refusing
    /// to start is the whole point: the only other option is to start with
    /// watermarks this node cannot vouch for, which is exactly the defect
    /// issue #605 describes.
    JournalUnreplayable { path: PathBuf, source: JournalError },
    /// Issue #884's runtime peer/route table under `state_dir` exists but
    /// could not be read (unreadable, or corrupt JSON) -- refusing to
    /// start rather than serve with a table this node cannot vouch for,
    /// the same reasoning `JournalUnreplayable` applies to a claim
    /// journal.
    RuntimePeerRouteTableUnusable {
        path: PathBuf,
        source: PeerRouteStoreError,
    },
    /// The local EVM channel index's durable snapshot under `state_dir`
    /// exists but could not be read (unreadable, or corrupt JSON) -- issue
    /// #661, same reasoning as [`RuntimeError::RuntimePeerRouteTableUnusable`]:
    /// this index is rebuildable from chain, but a corrupt file on disk is
    /// still refused rather than silently discarded, so an operator sees
    /// the problem instead of an unexplained full re-backfill.
    EvmChannelIndexUnusable {
        path: PathBuf,
        source: connector_settlement_evm::EvmChannelIndexError,
    },
    /// A `[[peer_channels]]` row's `channel_id` is not a shape
    /// [`connector_runtime::ClaimBook`] can file a watermark under (issue
    /// #678). Unreachable through `Config::load`, which canonicalizes the
    /// id to `0x` + 64 lowercase hex before this code ever sees it -- kept
    /// as a named startup failure rather than an `expect` so a future
    /// widening of the config shape refuses to start instead of panicking
    /// on the first peer claim.
    PeerChannelUnusable { channel_id: String },
    /// A `[[peer_channels]]` Solana row's `channel_account` or
    /// `counterparty_key` is not a shape
    /// [`connector_runtime::ClaimBook::set_solana_channel`] can file a
    /// watermark under (issue #759/#998). Unreachable through
    /// `Config::load`, which already checks both are base58 of exactly 32
    /// bytes -- kept as a named startup failure for the same reason
    /// [`RuntimeError::PeerChannelUnusable`] is.
    PeerChannelSolanaUnusable { channel_account: String },
    /// A `[[pay_channels]]` row's `channel_id` is not a shape
    /// [`connector_runtime::Connector::with_outbound_client_hop`] can sign
    /// against (issue #881) -- unreachable through `Config::load`, and kept
    /// as a named startup failure for exactly the reason
    /// [`RuntimeError::PeerChannelUnusable`] is.
    PayChannelUnusable { channel_id: String },
    /// A `[[pay_channels]]` row loaded, but the runtime piece it needs to
    /// sign a covering claim is missing: the `[settlement.evm]` key, or the
    /// `state_dir` its nonce floor lives under. Both are refused by
    /// `Config::load` (`PayChannelWithoutEvmSettlement`,
    /// `PayChannelsWithoutStateDir`), so this is the second lock on the
    /// same door -- and it fails the whole startup rather than silently
    /// leaving `outbound_client_hops` empty, because an empty map is
    /// indistinguishable from "nobody configured covering" and would
    /// forward uncovered while the file reads as configured.
    PayChannelUnwirable {
        peer_id: String,
        missing: &'static str,
    },
    /// The outbound client ledger under `state_dir` exists but could not be
    /// replayed or extended (issue #873/#881). Refused rather than started
    /// without: the nonce floor it holds is the only thing that stops a
    /// restart reissuing a nonce this node has already signed against a
    /// different cumulative amount.
    OutboundClientLedgerUnusable {
        path: PathBuf,
        source: OutboundClientError,
    },
    /// The HTTP client this node asks a next hop's `POST /ilp/claim-state`
    /// with could not be constructed (a TLS backend failure, in practice).
    /// A startup failure because the alternative is a configured hop whose
    /// every forward fails at packet time.
    ClaimStateClientUnusable {
        peer_id: String,
        source: reqwest::Error,
    },
    /// A `[[peer_channels]]` EVM row declares an EIP-712 domain that is not
    /// the one `[settlement.evm]` resolves to on chain (issue #1136).
    ///
    /// The paying direction, and silent until this refusal existed: a peer
    /// claim recovers correctly under the declared domain, so `ClaimBook`
    /// credits it and this node renders carriage for it -- and the very
    /// same claim recovers to a different address at
    /// `TokenNetwork.claimFromChannel`, so it can never be collected. A
    /// refusal to start rather than a warning, for the reason ADR 0009
    /// gives everywhere else: the alternative is a node that serves happily
    /// and works for nothing.
    PeerChannelDomainDisagreesWithSettlement(EvmDomainMismatch),
    /// A `[[pay_channels]]` row declares an EIP-712 domain that is not the
    /// one `[settlement.evm]` resolves to on chain (issue #1136).
    ///
    /// The outbound twin of
    /// [`RuntimeError::PeerChannelDomainDisagreesWithSettlement`]: this
    /// node signs a covering claim under the declared domain and hands it
    /// to the next hop with the PREPARE, and a claim signed under a
    /// `TokenNetwork` the channel does not live in recovers to a stranger's
    /// address at the far gate. Every forward to that hop fails -- or, if
    /// the hop's own config is stale in the same way, succeeds into a claim
    /// neither side can redeem.
    PayChannelDomainDisagreesWithSettlement(EvmDomainMismatch),
    /// A `[[client_channels]]` EVM row declares an EIP-712 domain that is
    /// not the one `[settlement.evm]` resolves to on chain (issue #1136).
    ///
    /// Same shape as the peer case, at the client edge: a buyer's claim
    /// verifies, the write is served, and the claim is worthless. **Not**
    /// covered by that table's `DepositFloor::Unknown` exemption -- see
    /// [`check_evm_channel_domains`], which says why a hand-declared
    /// channel is exempt from a chain-derived *policy* but not from a
    /// chain-stated *fact*.
    ClientChannelDomainDisagreesWithSettlement(EvmDomainMismatch),
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
    /// it is written: its one transport (ADR 0073), which the backend, the
    /// channel-index syncer and the rate source all share, could not be
    /// built.
    ///
    /// `Config::load` already refuses an `rpc_url` that is not an `http(s)`
    /// URL and a `socks_proxy` that is not `socks5h://`, so this is the
    /// second lock on those doors -- and a refusal to start, because a table
    /// with no transport has no client that could settle on it.
    SettlementEndpointUnusable {
        table: &'static str,
        message: String,
    },
    /// A `[settlement.<chain>.batch_settlement]` table's backend could not
    /// be bound (ADR 0074): the contract or program it names is not the
    /// x402 one, or the chain could not be asked. A refusal to start, for
    /// ADR 0009's reason -- the alternative is a node whose greeting offers
    /// `batch-settlement` and whose every voucher fails.
    BatchSettlementUnusable {
        table: &'static str,
        source: BatchSettlementError,
    },
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
            RuntimeError::Settlement(source) => {
                write!(
                    f,
                    "failed to construct the configured settlement backend: {source}"
                )
            }
            RuntimeError::StateDirUnusable { path, source } => write!(
                f,
                "state_dir {} is not usable for this node's claim journals: {source} -- \
                 the connector refuses to start rather than keep claim watermarks only in \
                 memory, where a restart would make every already-spent claim replayable",
                path.display()
            ),
            RuntimeError::PeerChannelUnusable { channel_id } => write!(
                f,
                "the [[peer_channels]] row for channel '{channel_id}' names an id the claim \
                 ledger cannot file a watermark under -- a peer channel id must be the \
                 channel's on-chain bytes32"
            ),
            RuntimeError::PeerChannelSolanaUnusable { channel_account } => write!(
                f,
                "the [[peer_channels]] row for Solana account '{channel_account}' names an \
                 account or counterparty key the claim ledger cannot file a watermark under -- \
                 both must be base58 of exactly 32 bytes"
            ),
            RuntimeError::PayChannelUnusable { channel_id } => write!(
                f,
                "the [[pay_channels]] row for channel '{channel_id}' names an id no covering \
                 claim can be signed against -- a pay-from channel id must be the channel's \
                 on-chain bytes32"
            ),
            RuntimeError::PayChannelUnwirable { peer_id, missing } => write!(
                f,
                "the [[pay_channels]] row for peer '{peer_id}' cannot be wired: this node has \
                 no {missing}. Every PREPARE forwarded to that peer is meant to carry a \
                 covering claim (ADR 0042), and a node that cannot sign one refuses to start \
                 rather than forward uncovered while the config file reads as configured"
            ),
            RuntimeError::OutboundClientLedgerUnusable { path, source } => write!(
                f,
                "failed to open the outbound client ledger at {}: {source} -- the connector \
                 refuses to start rather than sign covering claims from a nonce floor it \
                 cannot make durable, which is the floor that stops a restart reissuing a \
                 nonce it has already spent",
                path.display()
            ),
            RuntimeError::ClaimStateClientUnusable { peer_id, source } => write!(
                f,
                "failed to build the HTTP client this node asks peer '{peer_id}' for its \
                 claim state with: {source}"
            ),
            RuntimeError::JournalUnreplayable { path, source } => write!(
                f,
                "failed to replay the claim journal at {}: {source} -- the connector \
                 refuses to start rather than resume from watermarks it cannot vouch for",
                path.display()
            ),
            RuntimeError::RuntimePeerRouteTableUnusable { path, source } => write!(
                f,
                "failed to read the runtime peer/route table at {}: {source} -- the connector \
                 refuses to start rather than serve with a peer/route table it cannot vouch for",
                path.display()
            ),
            RuntimeError::EvmChannelIndexUnusable { path, source } => write!(
                f,
                "failed to read the local EVM channel index at {}: {source} -- the connector \
                 refuses to start rather than serve with a channel index it cannot vouch for. \
                 Since this index is rebuildable from chain, removing the file lets the node \
                 start and re-backfill from channel_index_from_block instead",
                path.display()
            ),
            RuntimeError::PeerChannelDomainDisagreesWithSettlement(mismatch) => write!(
                f,
                "a [[peer_channels]] row disagrees with this node's own settlement contract: \
                 {mismatch}. Every peer claim on that channel would verify here and recover to \
                 a different address on redemption, so this node would render carriage for \
                 money it could never collect (ADR 0024, issue #1136). Fix the row, or point \
                 [settlement.evm] at the deployment the channel actually lives in"
            ),
            RuntimeError::PayChannelDomainDisagreesWithSettlement(mismatch) => write!(
                f,
                "a [[pay_channels]] row disagrees with this node's own settlement contract: \
                 {mismatch}. Every covering claim this node signed for that hop would be one \
                 the hop cannot redeem, handed over with the PREPARE already sent (ADR 0042, \
                 issue #1136). Fix the row, or point [settlement.evm] at the deployment the \
                 channel actually lives in"
            ),
            RuntimeError::ClientChannelDomainDisagreesWithSettlement(mismatch) => write!(
                f,
                "a [[client_channels]] row disagrees with this node's own settlement contract: \
                 {mismatch}. Every client claim on that channel would verify here and recover \
                 to a different address on redemption, so this node would serve paid writes it \
                 could never collect on (ADR 0024, issue #1136). Fix the row, or point \
                 [settlement.evm] at the deployment the channel actually lives in"
            ),
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
                 client of that endpoint -- the settlement backend, and on EVM the channel-index \
                 syncer and the rate source -- shares one transport built from it (ADR 0073)"
            ),
            RuntimeError::BatchSettlementUnusable { table, source } => write!(
                f,
                "[settlement.{table}.batch_settlement] could not be bound: {source}. A node that \
                 opts in to x402 batch-settlement channels must reach the contract or program \
                 its vouchers are signed for (ADR 0074)"
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

impl From<SettlementError> for RuntimeError {
    fn from(source: SettlementError) -> Self {
        RuntimeError::Settlement(source)
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
/// [`EvmSettlementBackend::connect`]'s `private_key` argument.
fn hex_encode_32(bytes: [u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Resolve the `[settlement.key]` section to the raw 32-byte secret key
/// material it points at -- the same "32 raw bytes or 64 hex characters"
/// key-file shape [`build_signer`] already reads for `[signer]`, since both
/// are just secret-key pointers. `EvmSettlementBackend` wants that hex
/// encoded ([`read_settlement_private_key`]); `SolanaSettlementBackend`
/// wants it as an ed25519 seed, raw (issue #630).
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

/// Resolve the `[settlement.key]` section to the hex-encoded secp256k1
/// private key `EvmSettlementBackend` signs with.
fn read_settlement_private_key(location: &SecretLocation) -> Result<String, RuntimeError> {
    read_settlement_key_bytes(location).map(hex_encode_32)
}

/// Parse a `[settlement.solana]` base58 field (`program_id` or
/// `token_address`) into the `Pubkey` `SolanaSettlementBackend::connect`
/// wants. Refused -- naming the field and the value -- rather than
/// unwrapped: `connector-config` only checks these fields are non-empty
/// (issue #628), since a value that merely fails to parse as base58 is a
/// different failure from one that parses but names no executable program
/// or no SPL mint, and both must refuse startup rather than panic (issue
/// #630, ADR 0009).
fn parse_solana_pubkey(field: &'static str, value: &str) -> Result<Pubkey, RuntimeError> {
    Pubkey::from_str(value).map_err(|error| {
        RuntimeError::Settlement(SettlementError::Backend(format!(
            "[settlement.solana] {field} '{value}' is not a valid base58 Solana pubkey: {error}"
        )))
    })
}

/// Construct the settlement backend a `[settlement.evm]` (or legacy flat
/// `[settlement]`) table describes, connecting to the already-deployed
/// `TokenNetworkRegistry` it names (issue #576) -- `contract_address` -- and
/// resolving the `TokenNetwork` it actually drives through `token_address`,
/// rather than deploying a fresh one (issue #542).
///
/// Every field of the table reaches the chain here: `decimals` is handed to
/// [`EvmSettlementBackend::connect`], which refuses to connect when the
/// configured scale and the token's own `decimals()` disagree (issue #564).
/// An EVM table that names a scale the deployed token does not agree with is
/// a startup failure, not a line with no effect (ADR 0009).
async fn build_evm_settlement_backend(
    settlement: &EvmSettlementConfig,
    transport: &RpcTransport,
) -> Result<Arc<EvmSettlementBackend>, RuntimeError> {
    let private_key = read_settlement_private_key(settlement.key())?;
    let registry_address = ethers::types::Address::from(settlement.contract_address());
    let token_address = ethers::types::Address::from(settlement.token_address());
    let backend = EvmSettlementBackend::connect(
        transport,
        &private_key,
        registry_address,
        token_address,
        settlement.decimals(),
    )
    .await?;
    Ok(Arc::new(backend))
}

/// Hold every declared EVM channel domain against the `TokenNetwork` this
/// node actually redeems through (issue #1136).
///
/// # What was wrong
///
/// `[[peer_channels]]`, `[[pay_channels]]` and `[[client_channels]]` each
/// declare a `chain_id` and a `TokenNetwork`. Together those are the
/// EIP-712 domain (ADR 0024) an inbound claim's signature is recovered
/// against and an outbound claim's signature is produced under. Nothing
/// compared either to the contract this node settles through, so a row left
/// stale after a redeploy -- or simply mistyped -- produced a node that
/// **accepts** claims under domain X while **redeeming** through the
/// `TokenNetwork` `[settlement.evm]` resolves, which is Y. Silent, and in
/// the paying direction: the carriage is rendered, the claim is worthless.
///
/// [`IndexedEvmChannelSource`] already asserted this invariant in prose
/// -- *"every channel this index ever indexes belongs to the one
/// `TokenNetwork` this node's `[settlement.evm]` names"* -- while config
/// let an operator write a per-channel fact contradicting it. This function
/// is what makes that sentence true.
///
/// # Why refuse rather than derive
///
/// The Solana twin (#1128/#1134) deleted its per-row `program_id` and read
/// it from `[settlement.solana]`, and #981/#1082 did the same for the
/// client edge. That is not available here, and would be wrong here even if
/// it were:
///
/// * **ADR 0024 decided the other way, and stands.** *"The EIP-712 domain
///   (`chainId`, `verifyingContract`) is a configured input, per channel
///   ... and it is deliberately **not** read from a settlement backend."*
///   An ADR beats a habit; superseding that clause is a separate decision
///   with its own record, not a side effect of closing this hole.
/// * **The governing precedent is `decimals`, not `program_id`.** A removed
///   `program_id` was config-vs-config redundancy: the authoritative value
///   was already in the same file, so the copy carried no information.
///   `[settlement.evm]` does not name a `TokenNetwork` at all -- it names a
///   `TokenNetworkRegistry` plus a `token_address`, and the verifying
///   contract exists only as the answer
///   `TokenNetworkRegistry.getTokenNetwork(token_address)` gives. That is
///   the same shape as `[settlement.evm] decimals`, which this repo
///   declares in config and **refuses the boot over** when the token's own
///   `decimals()` disagrees (#564, `EvmSettlementBackend::connect`).
/// * **Two witnesses beat one.** Deriving the domain would leave exactly
///   one source and no cross-check, so a mistyped `token_address` -- which
///   resolves to a different *real* `TokenNetwork` -- would silently
///   re-domain every channel this node holds, moving the failure rather
///   than removing it. Declared-and-corroborated catches that too, because
///   the two sources are independent.
/// * **It keeps the file checkable without a chain.** `local_topologies_load`
///   asserts that two peered nodes write the same domain; a domain that
///   exists only after an RPC dial cannot be gate-checked at all.
///
/// # Why here, and not where every sibling rule lives
///
/// `ChannelInBothNamespaces` and `PayChannelWithoutEvmSettlement` are
/// `Config::load` refusals because both sides of those comparisons are in
/// the file. This one has a side that is only knowable after a network
/// dial, so it cannot be a load refusal and this is not an oversight:
/// `Config::load` stays total and offline.
///
/// It runs the moment [`EvmSettlementBackend::connect`] answers, before
/// anything else uses that backend -- exactly where `connect` itself checks
/// `decimals`. That the peer, pay and client tables were already wired into
/// the half-built `Connector` by then does not matter and is deliberately
/// not worked around: [`build`] returns `Err`, the half-built connector is
/// dropped, and nothing is ever served. Rewiring the domain after the fact
/// would be *deriving* it, which is the option rejected above; hoisting the
/// whole settlement loop above `Connector::new` would reorder every chain
/// dial relative to the key reads and journal opens for no gain here.
///
/// # The `DepositFloor::Unknown` question
///
/// `[[client_channels]]` records a declared channel with
/// `DepositFloor::Unknown` on purpose: hand-declaring a channel is the
/// operator's own policy decision, so it is exempt from the chain-derived
/// collateral cap (#646). The domain is **not** in that category. A deposit
/// floor is a policy -- how much risk to take on a channel nothing can be
/// asked about -- and an operator is entitled to set it. A domain is a fact
/// about which contract verifies a signature, and there is exactly one
/// right answer whenever this node has a backend at all. The exemption is
/// untouched: this check only runs when there is a chain to have asked.
///
/// # What it does not cover
///
/// A node with EVM channel rows and **no** `[settlement.evm]` table never
/// reaches here: there is no resolved `TokenNetwork` to compare against
/// because there is no backend, and since issue #1138 such a file does not
/// load at all (`PeerChannelWithoutEvmSettlement`,
/// `ClientChannelWithoutEvmSettlement`, `PayChannelWithoutEvmSettlement`).
/// The two rules are complementary and neither subsumes the other: that one
/// asks whether this node has an identity on the chain at all, which the
/// file answers offline; this one asks whether the contract it declares is
/// the one it settles through, which only the chain can answer.
fn check_evm_channel_domains(config: &Config, settled: EvmDomain) -> Result<(), RuntimeError> {
    for channel in config.peer_channels() {
        // A Solana row carries no EVM domain to disagree with: its whole
        // signed message is the channel account and the program id, and
        // #1134 already made that program the settlement table's own.
        if let PeerChannelConfig::Evm(evm) = channel {
            let declared = EvmDomain {
                chain_id: evm.chain_id(),
                token_network: evm.token_network(),
            };
            if declared != settled {
                return Err(RuntimeError::PeerChannelDomainDisagreesWithSettlement(
                    EvmDomainMismatch {
                        channel_id: evm.channel_id().to_string(),
                        declared,
                        settled,
                    },
                ));
            }
        }
    }
    for pay_channel in config.pay_channels() {
        // As above: a Solana pay channel declares no EVM domain, and its
        // program id is `[settlement.solana]`'s by construction rather than
        // a second declaration that could drift (issue #1128/#1146), so
        // there is no pair here for this check to compare.
        let PayChannelConfig::Evm(evm) = pay_channel else {
            continue;
        };
        let declared = EvmDomain {
            chain_id: evm.chain_id(),
            token_network: evm.token_network(),
        };
        if declared != settled {
            return Err(RuntimeError::PayChannelDomainDisagreesWithSettlement(
                EvmDomainMismatch {
                    channel_id: evm.channel_id().to_string(),
                    declared,
                    settled,
                },
            ));
        }
    }
    for channel in config.client_channels() {
        if let ClientChannelConfig::Evm(evm) = channel {
            let declared = EvmDomain {
                chain_id: evm.chain_id(),
                token_network: evm.token_network_address(),
            };
            if declared != settled {
                return Err(RuntimeError::ClientChannelDomainDisagreesWithSettlement(
                    EvmDomainMismatch {
                        channel_id: evm.channel_id().to_string(),
                        declared,
                        settled,
                    },
                ));
            }
        }
    }
    Ok(())
}

/// Construct the settlement backend a `[settlement.solana]` table
/// describes, binding to the already-deployed `payment-channel` program it
/// names (`program_id`) and settling in the SPL mint it names
/// (`token_address`) -- the fail-closed identity checks issue #630 wires
/// in: an unreachable RPC endpoint, a `program_id` naming no executable
/// account, a `token_address` not owned by the SPL Token program, or a
/// `decimals` the mint's own `decimals` field disagrees with are all a
/// startup failure here, not a line with no effect (the `#564` pattern,
/// Solana-flavored, ADR 0009).
async fn build_solana_settlement_backend(
    settlement: &SolanaSettlementConfig,
    transport: &RpcTransport,
) -> Result<Arc<SolanaSettlementBackend>, RuntimeError> {
    let payer_seed = read_settlement_key_bytes(settlement.key())?;
    let program_id = parse_solana_pubkey("program_id", settlement.program_id())?;
    let token_mint = parse_solana_pubkey("token_address", settlement.token_address())?;
    let backend = SolanaSettlementBackend::connect(
        transport,
        &payer_seed,
        program_id,
        token_mint,
        settlement.decimals(),
    )
    .await?;
    Ok(Arc::new(backend))
}

/// This node's receive-only backend for x402 `batch-settlement` channels on
/// EVM (ADR 0074), when `[settlement.evm.batch_settlement]` is written: built
/// from the table's own [`EvmSettlementBackend`], so it shares that backend's
/// client -- and so its one transport (ADR 0073) -- its settlement key and
/// that key's nonce count. `None` when the table is absent.
async fn build_evm_batch_settlement(
    settlement: &EvmSettlementConfig,
    backend: &EvmSettlementBackend,
) -> Result<Option<Arc<EvmBatchSettlementBackend>>, RuntimeError> {
    let Some(batch) = settlement.batch_settlement() else {
        return Ok(None);
    };
    let backend = backend
        .batch_settlement(batch.min_withdraw_delay_secs())
        .await
        .map_err(|source| RuntimeError::BatchSettlementUnusable {
            table: SettlementChain::Evm.name(),
            source,
        })?;
    Ok(Some(Arc::new(backend)))
}

/// The Solana twin of [`build_evm_batch_settlement`]: bound over the table's
/// one transport, under its settlement key as the sponsor (ADR 0074
/// decision 5), in its `token_address` mint.
async fn build_solana_batch_settlement(
    settlement: &SolanaSettlementConfig,
    transport: &RpcTransport,
) -> Result<Option<Arc<SolanaBatchSettlement>>, RuntimeError> {
    let Some(batch) = settlement.batch_settlement() else {
        return Ok(None);
    };
    let unusable = |source| RuntimeError::BatchSettlementUnusable {
        table: SettlementChain::Solana.name(),
        source,
    };
    let sponsor_seed = read_settlement_key_bytes(settlement.key())?;
    let mint = parse_solana_pubkey("token_address", settlement.token_address())?;
    let backend = SolanaBatchSettlement::connect(
        transport,
        &sponsor_seed,
        mint,
        batch.min_grace_period_secs(),
        batch.min_sponsored_deposit(),
    )
    .await
    .map_err(unusable)?;
    Ok(Some(Arc::new(backend)))
}

/// The client edge's channel records, read from the same deployed
/// `TokenNetwork` the `[settlement]` section already names (issue #556).
///
/// This is the seam issue #607 left for this work. Before it, the only
/// source of a channel's counterparty was the `[[client_channels]]` config
/// section, so a node whose operator had not written a buyer's channel
/// down by hand refused that buyer's every claim -- which contradicts
/// issue #502's *"anonymity is a first-class path, not a fallback: it is
/// how an unaffiliated buyer pays for a terminated route without
/// registering with the operator first"*. A buyer registers on chain
/// instead, and this reads that registration.
///
/// A newtype rather than an `impl` on [`EvmSettlementBackend`] itself
/// because both the trait and the type are foreign to this crate; keeping
/// the adapter here also keeps `connector-settlement-evm` free of any
/// dependency on the HTTP edge, and matches ADR 0001's rule that
/// construction decisions live in `connector-cli`.
struct SettlementChannelSource {
    backend: Arc<EvmSettlementBackend>,
}

/// Hand-written because [`EvmSettlementBackend`] holds contract handles
/// and a signing client that are not `Debug`, and
/// [`ClientChannelSource`] requires it so a registry can name its source
/// in a log line. Names the deployment rather than dumping the client.
impl fmt::Debug for SettlementChannelSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SettlementChannelSource")
            .field("token_network", &self.backend.address())
            .field("chain_id", &self.backend.chain_id())
            .finish()
    }
}

#[async_trait]
impl ClientChannelSource for SettlementChannelSource {
    async fn evm_channel(
        &self,
        channel_id: &[u8; 32],
    ) -> Result<Option<EvmChannel>, ChannelLookupFailed> {
        let resolved = self
            .backend
            .channel_counterparty_deposit(*channel_id)
            .await
            .map_err(|error| ChannelLookupFailed(error.to_string()))?;
        // The signing domain comes from the same deployment the
        // counterparty did (issue #556's open question): `TokenNetwork`
        // inherits OpenZeppelin's `EIP712("TokenNetwork", "1")`, whose
        // domain separator is built from `block.chainid` and
        // `address(this)`, so a per-entry config field for either could
        // only ever restate -- or contradict -- what the chain says.
        //
        // The deposit rides along for issue #646: a claim above what the
        // counterparty has actually deposited could never be redeemed
        // (`TokenNetwork.sol`'s `InsufficientChannelBalance`), so the
        // client edge refuses it rather than doing work it cannot be paid
        // for. `as_u64` saturates deliberately -- a deposit wider than a
        // claim's `u64` cumulative amount can never be exceeded by one, so
        // clamping to `u64::MAX` errs in the safe direction.
        Ok(resolved.map(|(counterparty, deposit)| EvmChannel {
            counterparty: counterparty.to_fixed_bytes(),
            chain_id: self.backend.chain_id(),
            token_network_address: self.backend.address().to_fixed_bytes(),
            deposit_floor: DepositFloor::AtLeast(saturating_u64(deposit)),
        }))
    }
}

/// Wraps [`SettlementChannelSource`] with the local EVM channel index
/// (issue #661): a channel the index has caught up to answers from a
/// `HashMap` probe -- no `eth_call` at all -- and a channel the index has
/// not caught up to (never opened, opened inside the confirmation window,
/// or the index's subscription is lagging/down) falls through to exactly
/// the direct chain read [`SettlementChannelSource`] always performed,
/// unchanged. This is what makes shipping the index safe incrementally: a
/// node whose sync has never once succeeded behaves byte-identically to a
/// node built before this issue landed.
///
/// Since issue #1151 one more answer falls through: an indexed channel
/// whose counterparty deposit is **zero**, which is what this index reports
/// both for a channel that holds nothing and for one whose
/// `ChannelNewDeposit` is younger than the confirmation depth. See
/// [`ClientChannelSource::evm_channel`]'s implementation below for why that
/// zero is asked of the chain rather than reported as a floor -- and why it
/// is not reported as [`DepositFloor::Unknown`] either.
///
/// `EvmChannel::chain_id`/`token_network_address` (the EIP-712 domain) come
/// from `self.fallback.backend` rather than from a field the index itself
/// stores per channel: every channel this index ever indexes belongs to the
/// one `TokenNetwork` this node's `[settlement.evm]` names, so the domain is
/// one constant for the whole index, not a per-channel fact -- storing it
/// once on the backend this source already holds is the same information,
/// without repeating an invariant on every record.
///
/// That sentence was true of this index and false of the node around it
/// until issue #1136. This source only ever answers for a channel it
/// resolved *from chain*, so its own domain was never in doubt -- but
/// `[[client_channels]]`, `[[peer_channels]]` and `[[pay_channels]]` could
/// each declare a domain naming some other `TokenNetwork`, and nothing
/// compared them. [`check_evm_channel_domains`] is what closed that, so the
/// invariant this comment asserts now holds for every channel the node
/// judges, not only for the ones this index found.
struct IndexedEvmChannelSource {
    index: Arc<EvmChannelIndex>,
    fallback: SettlementChannelSource,
}

impl fmt::Debug for IndexedEvmChannelSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IndexedEvmChannelSource")
            .field("fallback", &self.fallback)
            .field("index_last_indexed_block", &self.index.last_indexed_block())
            .finish()
    }
}

impl IndexedEvmChannelSource {
    /// What the index alone says about `channel_id` -- every one of the
    /// three reads below starts here, and all three ask it about the same
    /// address: this node's own signing address, which is what lets an
    /// `Active` answer name the *other* participant as the counterparty.
    fn lookup(&self, channel_id: &[u8; 32]) -> ChannelIndexLookup {
        self.index
            .lookup(channel_id, self.fallback.backend.own_address())
    }
}

#[async_trait]
impl ClientChannelSource for IndexedEvmChannelSource {
    async fn evm_channel(
        &self,
        channel_id: &[u8; 32],
    ) -> Result<Option<EvmChannel>, ChannelLookupFailed> {
        match self.lookup(channel_id) {
            // Issue #1151. A deposit of zero out of this index is not a
            // reading, it is the absence of one: `EvmChannelIndex::lookup`
            // answers `Active` for any channel whose `ChannelOpened` it has
            // applied and reports `unwrap_or_default()` -- zero -- for a
            // counterparty it has applied no `ChannelNewDeposit` for. That
            // is the same value whether the channel holds nothing or the
            // deposit is simply younger than the index's confirmation
            // depth, and the index can never tell those apart: it is
            // permanently `channel_index_confirmations` blocks behind head,
            // so a deposit made in that window is invisible to it no matter
            // how long the channel has existed. So the zero is passed to the
            // chain rather than reported as fact.
            //
            // **Not** answered `DepositFloor::Unknown` (the shape issue
            // #1151 floats first), because `Unknown` *exempts* a claim from
            // the collateral check entirely -- `DepositFloor::covers` is
            // unconditionally true, and `POST /ilp/claim-state` reports
            // `depositTotal: null`, which `OutboundClientLedger::next_claim`
            // reads as unbounded headroom. `openChannel` costs gas and no
            // tokens, and emits no deposit event, so "opened and never
            // funded" is exactly the case that reaches this branch: mapping
            // it to `Unknown` would let anyone open a channel naming this
            // node, deposit nothing, and spend claims this connector could
            // never redeem (`TokenNetwork.sol`'s
            // `InsufficientChannelBalance`) -- precisely the giveaway issue
            // #646 exists to prevent. The chain read below refuses that
            // channel with `AtLeast(0)` and admits a genuinely funded one,
            // which is the whole property.
            //
            // The cost is bounded by machinery that already exists:
            // `ClientChannelRegistry` memoises whatever comes back (so a
            // channel that really holds nothing costs one read per
            // `refresh_after`, not one per packet), and a claim that
            // breaches the memoised floor is rate-limited by
            // `min_reattempt_interval`. Forcing even the first read costs an
            // attacker a real `openChannel` transaction, which is dearer
            // than the two `eth_call`s it buys.
            ChannelIndexLookup::Active { deposit, .. } if deposit.is_zero() => {
                self.fallback.evm_channel(channel_id).await
            }
            ChannelIndexLookup::Active {
                counterparty,
                deposit,
            } => Ok(Some(EvmChannel {
                counterparty: counterparty.to_fixed_bytes(),
                chain_id: self.fallback.backend.chain_id(),
                token_network_address: self.fallback.backend.address().to_fixed_bytes(),
                deposit_floor: DepositFloor::AtLeast(saturating_u64(deposit)),
            })),
            // Reported `None` here -- "not a channel this connector can be
            // paid on" -- and refined to a distinguishable refusal by
            // `evm_channel_terminal` below, which the registry consults
            // only after seeing this `None`. Never falls through to the
            // chain: the index has already seen the terminal log, so a
            // chain read could only confirm what is already known.
            ChannelIndexLookup::Terminal => Ok(None),
            // The one case that costs an RPC, exactly as it always has:
            // this index has nothing to say, one way or the other.
            ChannelIndexLookup::Miss => self.fallback.evm_channel(channel_id).await,
        }
    }

    /// A breach re-read (issue #661's follow-up finding): the indexed
    /// deposit lags the chain by the confirmation depth, so `Active` is
    /// exactly the answer a breach exists to distrust -- a top-up (or a
    /// `ChannelNewDeposit` younger than the confirmation window) is real on
    /// chain before this index will admit it. The chain is asked directly,
    /// as it would have been before this index existed, so a claim main
    /// would honour is honoured here on the same submission. `Terminal`
    /// stays answered from the index: settlement is monotone and the index
    /// only applies confirmed logs, so a chain read could only repeat it.
    async fn evm_channel_fresh(
        &self,
        channel_id: &[u8; 32],
    ) -> Result<Option<EvmChannel>, ChannelLookupFailed> {
        match self.lookup(channel_id) {
            ChannelIndexLookup::Terminal => Ok(None),
            ChannelIndexLookup::Active { .. } | ChannelIndexLookup::Miss => {
                self.fallback.evm_channel(channel_id).await
            }
        }
    }

    async fn evm_channel_terminal(&self, channel_id: &[u8; 32]) -> bool {
        matches!(self.lookup(channel_id), ChannelIndexLookup::Terminal)
    }
}

/// A `U256` narrowed to `u64`, clamped rather than wrapped or panicking
/// (`ethers`' own `U256::as_u64` panics on overflow). Only ever used for a
/// deposit that bounds a `u64` claim amount from above, where clamping to
/// `u64::MAX` is indistinguishable from the true value: no `u64` cumulative
/// amount can exceed either.
fn saturating_u64(value: U256) -> u64 {
    if value > U256::from(u64::MAX) {
        u64::MAX
    } else {
        value.as_u64()
    }
}

/// The Solana twin of [`SettlementChannelSource`] (issue #631): the client
/// edge's channel records for a Solana channel nothing was declared for,
/// read from the same deployed payment-channel program the
/// `[settlement.solana]` section already names. Epic #627's remaining
/// piece from #630's own note -- "Solana channel resolution from chain ...
/// [is] epic #627's remaining children" -- this is that child.
struct SolanaChannelSource {
    backend: Arc<SolanaSettlementBackend>,
}

/// Hand-written for the same reason [`SettlementChannelSource`]'s is:
/// [`SolanaSettlementBackend`] holds an RPC client that is not `Debug`.
///
/// # Not affected by issue #1151
///
/// There is no Solana twin of [`IndexedEvmChannelSource`] and there is not
/// going to be one -- `connector_settlement_evm::channel_index`'s own doc
/// settles that as a decision, since `packages/solana-program` emits
/// free-text `msg!` lines rather than structured events, so there is nothing
/// to index. This source's `deposit_floor` is therefore decoded out of the
/// channel account on the very `getAccountInfo` the lookup already performs,
/// with no confirmation window between the read and the answer: a zero here
/// is the account's own balance and not the absence of an event, so it means
/// exactly what it says and is reported as a floor rather than asked about
/// again.
impl fmt::Debug for SolanaChannelSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SolanaChannelSource")
            .field("own_pubkey", &self.backend.own_pubkey())
            .finish()
    }
}

#[async_trait]
impl ClientChannelSource for SolanaChannelSource {
    async fn solana_channel(
        &self,
        channel_account: &[u8; 32],
    ) -> Result<Option<SolanaChannel>, ChannelLookupFailed> {
        // The deposit costs nothing extra here (issue #646): it is decoded
        // out of the very same channel account the counterparty comes from,
        // on the one `getAccountInfo` this lookup already performs -- it
        // was simply thrown away before.
        let resolved = self
            .backend
            .channel_counterparty_deposit(Pubkey::new_from_array(*channel_account))
            .await
            .map_err(|error| ChannelLookupFailed(error.to_string()))?;
        // The program is this backend's own: a channel resolved from chain
        // was found under the program this node settles with, and a claim on
        // it signs that program id (ADR 0053, issue #1082). It is never taken
        // from the claim, which declares a `cluster` that nothing signs.
        let program_id = self.backend.program_id().to_bytes();
        Ok(resolved.map(|(counterparty, deposit)| SolanaChannel {
            program_id,
            counterparty: counterparty.to_bytes(),
            deposit_floor: DepositFloor::AtLeast(deposit),
        }))
    }
}

/// The two journal files a node keeps under its `state_dir` (issue #605).
/// Two files rather than one because they are two different books --
/// `ClaimBook`'s channel ids are peer channels, the client edge's are
/// chain-namespaced client channels -- and because each is replayed by a
/// different owner at startup; sharing one file would mean each replaying
/// the other's entries and each holding a second writer's file handle on
/// the same path.
const PEER_CLAIM_JOURNAL: &str = "peer-claims.log";
const CLIENT_EDGE_JOURNAL: &str = "client-edge-claims.log";
/// Issue #884's runtime peer/route table -- a whole-table JSON snapshot,
/// not an append-only journal line format like the two above (see
/// `connector_runtime::PeerRouteStore`'s own docs for why).
const RUNTIME_PEER_ROUTE_TABLE: &str = "runtime-peers.json";
/// Issue #661's local EVM channel index -- a whole-table JSON snapshot for
/// the same reason [`RUNTIME_PEER_ROUTE_TABLE`] is one rather than an
/// append-only log: a settled channel is marked terminal in place, not
/// appended over (see `connector_settlement_evm::channel_index`'s own doc).
const EVM_CHANNEL_INDEX: &str = "evm-channel-index.json";

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

/// Open the local EVM channel index's durable snapshot under `state_dir`
/// (issue #661), or start an in-memory-only index when this node names no
/// `state_dir` at all -- the same degrade issue #884's runtime peer/route
/// table already established (ADR 0034): a node with no `state_dir` still
/// saves every RPC call the index avoids within a run, it just re-backfills
/// from `channel_index_from_block` on every restart rather than resuming
/// from a checkpoint.
///
/// `indexes` and `from_block` are what bind the snapshot to what it indexes
/// (issue #1282). Both come from facts this node has already established
/// against the chain itself -- the live chain id and the **resolved**
/// `TokenNetwork` off the built backend, not anything read out of the URL
/// or the registry line -- which is why this is called after the backend is
/// built rather than alongside the other `state_dir` stores.
fn open_evm_channel_index(
    state_dir: Option<&Path>,
    indexes: IndexedContract,
    from_block: u64,
) -> Result<Arc<EvmChannelIndex>, RuntimeError> {
    let path = state_dir.map(|state_dir| state_dir.join(EVM_CHANNEL_INDEX));
    let index = EvmChannelIndex::open(path.as_deref(), indexes, from_block).map_err(|source| {
        RuntimeError::EvmChannelIndexUnusable {
            path: path.unwrap_or_default(),
            source,
        }
    })?;
    Ok(Arc::new(index))
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

/// The key this node signs outbound **peer** claims with (ADR 0024), and
/// the EVM address it derives -- the `senderId`/`signerAddress` every claim
/// this node emits carries (`peer-carriage-spec.md` §4).
type PeerClaimIdentity = (Arc<dyn Signer>, [u8; 20]);

/// The key this node signs outbound **peer** claims with, and the EVM
/// address that key derives -- `None` for a node with no `[settlement.evm]`
/// table (ADR 0024's balance proof has no meaning without one).
///
/// It is the settlement key rather than `[signer]`'s identity key because a
/// peer claim is redeemed on chain by the counterparty against the
/// `TokenNetwork` this node is a channel participant in, and the participant
/// is the settlement address. The two keys are separate on purpose (ADR
/// 0022's two audiences); conflating them would produce claims that verify
/// nowhere.
fn peer_claim_identity(config: &Config) -> Result<Option<PeerClaimIdentity>, RuntimeError> {
    let Some(evm) = config
        .settlements()
        .iter()
        .find_map(|settlement| match settlement {
            SettlementConfig::Evm(evm) => Some(evm),
            SettlementConfig::Solana(_) => None,
        })
    else {
        return Ok(None);
    };
    let secret = read_settlement_key_bytes(evm.key())?;
    let signer = LocalSigner::from_secret_bytes("peer-claim-signer", secret)?;
    let address = derive_evm_address(&signer.public_key()?);
    Ok(Some((Arc::new(signer), address)))
}

/// The key this node signs outbound **peer** claims with on Solana (issue
/// #732/#998) -- the Solana counterpart of [`peer_claim_identity`], for
/// exactly the same reason: a Solana peer claim's
/// `senderId`/`signerPublicKey` is redeemed against this node's own
/// channel-participant identity, which is the `[settlement.solana]` key,
/// never `[signer]`'s. `None` for a node with no `[settlement.solana]`
/// table, which therefore signs no Solana claim at all.
///
/// Returns the signer alone, where the EVM twin also returns an address:
/// the public key the wire renders is read straight off the signer
/// ([`Ed25519Signer::public_key`]), so there is no derived second copy that
/// could drift from the key that actually signed.
fn peer_claim_identity_solana(
    config: &Config,
) -> Result<Option<Arc<dyn Ed25519Signer>>, RuntimeError> {
    let Some(solana) = config
        .settlements()
        .iter()
        .find_map(|settlement| match settlement {
            SettlementConfig::Solana(solana) => Some(solana),
            SettlementConfig::Evm(_) => None,
        })
    else {
        return Ok(None);
    };
    let secret = read_settlement_key_bytes(solana.key())?;
    let signer = LocalEd25519Signer::from_secret_bytes(secret)?;
    Ok(Some(Arc::new(signer)))
}

/// Wire every `[[peer_channels]]` row into the claim ledger (issue #678,
/// `peer-carriage-spec.md` §11): whose signature this node accepts on a
/// claim naming that channel, and -- on EVM -- the EIP-712 domain it is
/// judged under (ADR 0024). A Solana row (issue #759) has no separate
/// domain call: the channel account *is* the whole of a Solana claim's
/// signed message, so [`Connector::with_solana_channel`] does both jobs
/// [`Connector::with_channel_verification_key`] and
/// [`Connector::with_channel_domain`] do together on EVM, in one call (see
/// [`connector_runtime::ClaimBook::set_solana_channel`]'s own doc).
///
/// **These rows are now inbound-only, and that changed in issue #1145.**
/// A `[[peer_channels]]` row also used to register the channel this node
/// would *claim against* when it owed a peer for a forward it had already
/// made -- ADR 0004's postpay model, `Connector::with_peer_claim_channel`
/// feeding `ClaimBook::record_fulfillment`. Nothing owes a peer after the
/// fact any more: a forward is covered before it is sent, from the
/// `[[pay_channels]]` row that peering is now required to have (ADR 0042).
/// A peering with several rows therefore no longer needs a "first row in
/// the file wins" rule for the outbound direction -- every row here is
/// accepted *inbound*, on whichever chain it is on, and none of them is a
/// channel this node signs from.
///
/// Issue #998: before this, a Solana row loaded, validated, and reached
/// claim *rendering* (its `program_id` reaches `PeerRelation::from_config`
/// in `connector-peer-btp`/`connector-peer-http`) but never `ClaimBook`
/// itself -- so a Solana-settled peering could never verify an inbound
/// claim or sign an outbound one. `ClaimBook::set_solana_channel` (issue
/// #742/#757) closed the other half of that gap; this closes the
/// config-to-runtime wiring.
///
/// Issue #1128: `solana.program_id()` is `[settlement.solana] program_id`,
/// not a fact the row declares -- `Config::load` copies it in, and refuses
/// a row that tries to name its own. So the program `ClaimBook` verifies a
/// peer claim under is by construction the program the settlement backend
/// built below would redeem it through; there is no pair here that can
/// drift apart, and therefore nothing for this function to compare. The
/// same shape `client_channels` uses for `[[client_channels]]` since #1082.
fn wire_peer_channels(
    mut connector: Connector,
    config: &Config,
) -> Result<Connector, RuntimeError> {
    for channel in config.peer_channels() {
        // What a claim on this row names, and what `ClaimBook` files its
        // watermark under: an EVM `channel_id` or a Solana
        // `channel_account`.
        let claim_channel = match channel {
            PeerChannelConfig::Evm(evm) => evm.channel_id(),
            PeerChannelConfig::Solana(solana) => solana.channel_account(),
        };
        match channel {
            PeerChannelConfig::Evm(evm) => {
                connector =
                    connector.with_channel_verification_key(claim_channel, evm.counterparty_key());
                connector = connector
                    .with_channel_domain(
                        claim_channel,
                        ChannelDomain {
                            chain_id: evm.chain_id(),
                            token_network_address: evm.token_network(),
                        },
                    )
                    .map_err(|_| RuntimeError::PeerChannelUnusable {
                        channel_id: claim_channel.to_string(),
                    })?;
            }
            PeerChannelConfig::Solana(solana) => {
                connector = connector
                    .with_solana_channel(
                        claim_channel,
                        solana.counterparty_key(),
                        solana.program_id(),
                    )
                    .map_err(|_| RuntimeError::PeerChannelSolanaUnusable {
                        channel_account: claim_channel.to_string(),
                    })?;
            }
        }
    }
    Ok(connector)
}

/// This node's outbound client ledger (issue #873), under the same
/// `state_dir` its two claim journals live in.
///
/// A third file rather than a line in either journal, and the module header
/// of `connector_runtime::outbound_client` is explicit about why: this book
/// is not a `JournalEntry` stream, nothing replaying a journal would
/// understand it, and the inbound and outbound books must never merge.
const OUTBOUND_CLIENT_LEDGER: &str = "outbound-client.log";

/// The client one `[[pay_channels]]` hop asks `POST /ilp/claim-state` with,
/// dialed through `socks_proxy` when — and only when — that hop's client
/// edge is an onion one (ADR 0070 decision 3).
///
/// **The same host-selected rule the carriages apply, at the one ILP-wire
/// dial that is not a carriage.** `cover_forward` asks the receiver where
/// this node's claims stand on *every* covered PREPARE (issue #1102), so a
/// peering whose far side is only reachable over a circuit is not payable
/// unless this ask can reach it too — the packet would be refused for want
/// of a covering claim long before the carriage was asked to carry it. It is
/// the ILP wire by ADR 0070 decision 4's own division: the two things that
/// decision keeps direct are settlement RPC and the app's `handler_url`, and
/// both hold their own clients elsewhere. (ADR 0073 lets a settlement table
/// opt its RPC onto the proxy; that is [`settlement_transports`], not this.)
///
/// [`is_onion_endpoint`] is called rather than re-derived, for the reason
/// that function's own doc gives: the suffix that decides a carriage and the
/// suffix that decides a dial are one implementation.
///
/// A `.onion` client edge on a node that configured **no** proxy builds a
/// plain client and fails at the dial, which is the same answer the
/// carriages give: a `.onion` name resolves nowhere without a proxy, so the
/// covering claim fails as an unreachable hop rather than as anything more
/// exotic.
fn claim_state_client(
    client_edge_url: &url::Url,
    socks_proxy: Option<&url::Url>,
    answer_timeout: std::time::Duration,
) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder().timeout(answer_timeout);
    if let (true, Some(proxy)) = (is_onion_endpoint(client_edge_url), socks_proxy) {
        builder = builder.proxy(reqwest::Proxy::all(proxy.as_str())?);
    }
    builder.build()
}

/// Wire every `[[pay_channels]]` row into the connector's client role (ADR
/// 0042 item 2, issue #881): the ledger this node signs outbound client
/// claims from, and, per next hop, the channel it pays from plus the hop
/// itself as the authority on where those claims stand.
///
/// **This is the wiring ADR 0042 names as the missing half.**
/// `Connector::with_outbound_client_hop` has existed and been correct since
/// #875/#881, and no production path called it -- so `cover_forward`
/// answered `NotConfigured` on every packet the shipped binary ever
/// forwarded, and "a connector covers every PREPARE it sends" could not
/// happen. It happens for a peering with a row here, and for no other.
///
/// **The ledger opens unconditionally (issue #1217).** A file with no
/// `[[pay_channels]]` table still gets a real, `state_dir`-backed
/// [`OutboundClientLedger`] -- or, absent a `state_dir`, the in-memory form
/// -- because `POST /peers` (ADR 0058) can register a client-role hop on a
/// running node with no `[[pay_channels]]` row at all, and a hop with no
/// ledger to sign from is accept-only exactly the way this issue is about.
/// What *is* still additive and default-off is `outbound_client_hops`
/// itself: with no `[[pay_channels]]` rows this function builds no HTTP
/// client and registers no hop, so the table holds exactly what a runtime
/// `POST /peers` write has put there and nothing else -- a node that never
/// establishes a runtime peering forwards exactly as it did before this
/// function existed. The ledger's own presence is what
/// `build_writes_an_outbound_client_ledger_even_for_a_node_with_no_pay_channels`
/// pins.
///
/// Where each part of the claim comes from is ADR 0030's table, and every
/// row of it is honoured here:
///
/// * the **signing key** is `[settlement.evm]`'s, passed in as
///   `claim_signer` -- the same key `with_signer` gives the peer role,
///   because the channel's on-chain participant is this node's settlement
///   address either way. No second key is introduced;
/// * the **nonce and cumulative amount** are the receiver's, asked over
///   `POST /ilp/claim-state` ([`OwnedHttpClaimState`]) on every covered
///   packet. Nothing local substitutes, and nothing is remembered but the
///   nonce floor;
/// * the **channel id** and the **EIP-712 domain** are the row's own.
///
/// The per-hop [`reqwest::Client`] carries that peering's own
/// `peer_answer_timeout_ms` rather than a new knob: the claim-state ask is a
/// request to that peer, on the packet's hot path, and a hop that stops
/// answering must fail the packet inside the peering's own answer budget
/// instead of holding it until the PREPARE expires. It is also the one dial
/// on this path that goes through the SOCKS5 proxy when the hop is onion --
/// see [`claim_state_client`].
fn wire_outbound_client_hops(
    mut connector: Connector,
    config: &Config,
    claim_signer: Option<Arc<dyn Signer>>,
    claim_signer_solana: Option<Arc<dyn Ed25519Signer>>,
) -> Result<Connector, RuntimeError> {
    let unwirable = |pay_channel: &PayChannelConfig, missing| RuntimeError::PayChannelUnwirable {
        peer_id: pay_channel.peer_id().to_string(),
        missing,
    };
    // A `[[pay_channels]]` row still needs a `state_dir` of its own -- its
    // nonce floor must survive a restart. A node with none at all does not:
    // the ledger below simply degrades to the in-memory form, the same
    // "no state_dir, no durability, still mutable" shape every other
    // `state_dir`-scoped store on this connector takes.
    if let (Some(first), None) = (config.pay_channels().first(), config.state_dir()) {
        return Err(unwirable(first, "state_dir to keep its nonce floor in"));
    }

    // The ledger opens regardless of `[[pay_channels]]` (issue #1217): a
    // runtime peering established over `POST /peers` (ADR 0058) can
    // register a client-role hop with no such row ever having existed, and
    // a hop with no ledger to sign from is accept-only, which is this
    // issue verbatim. FILE-BACKED where `state_dir` exists, as
    // `with_outbound_client_ledger` requires of a serving node -- a restart
    // that reissued a nonce would fork this node's own outbound nonce line.
    // `open_journal` has not run yet on this path, so the directory is
    // created here the same way it creates it.
    let ledger = match config.state_dir() {
        Some(state_dir) => {
            std::fs::create_dir_all(state_dir).map_err(|source| {
                RuntimeError::StateDirUnusable {
                    path: state_dir.to_path_buf(),
                    source,
                }
            })?;
            let ledger_path = state_dir.join(OUTBOUND_CLIENT_LEDGER);
            OutboundClientLedger::open(&ledger_path).map_err(|source| {
                RuntimeError::OutboundClientLedgerUnusable {
                    path: ledger_path,
                    source,
                }
            })?
        }
        None => OutboundClientLedger::in_memory(),
    };
    connector = connector.with_outbound_client_ledger(Arc::new(ledger));

    for pay_channel in config.pay_channels() {
        // Every row names a configured peering -- `Config::load` refuses a
        // `PayChannelOrphaned` one -- so this lookup cannot miss.
        let answer_timeout = config
            .peers()
            .iter()
            .find(|peer| peer.id() == pay_channel.peer_id())
            .map(|peer| std::time::Duration::from_millis(peer.peer_answer_timeout_ms()))
            .ok_or_else(|| unwirable(pay_channel, "peering for that peer_id"))?;
        let client = claim_state_client(
            pay_channel.client_edge_url(),
            config.socks_proxy(),
            answer_timeout,
        )
        .map_err(|source| RuntimeError::ClaimStateClientUnusable {
            peer_id: pay_channel.peer_id().to_string(),
            source,
        })?;
        // The challenge signer and the claim signer are the same key, on
        // the channel's own chain: the ask proves control of the channel's
        // on-chain participant, and the claim is signed by it.
        connector = match pay_channel {
            PayChannelConfig::Evm(evm) => {
                let Some(signer) = claim_signer.clone() else {
                    return Err(unwirable(pay_channel, "[settlement.evm] signing key"));
                };
                let claim_state = Arc::new(OwnedHttpClaimState::new(
                    client,
                    evm.client_edge_url().as_str(),
                    ClaimStateChallengeSigner::Evm(signer),
                ));
                connector
                    .with_outbound_client_hop(
                        evm.peer_id(),
                        evm.channel_id(),
                        EvmDomain {
                            chain_id: evm.chain_id(),
                            token_network: evm.token_network(),
                        },
                        claim_state,
                    )
                    .map_err(|_| RuntimeError::PayChannelUnusable {
                        channel_id: evm.channel_id().to_string(),
                    })?
            }
            // Issue #1146. `program_id` is `[settlement.solana]`'s, copied
            // in by `Config::load` rather than declared by the row, so the
            // program ADR 0053 signs into every covering claim here is by
            // construction the program this node would itself redeem the
            // claim through -- there is no pair to compare.
            PayChannelConfig::Solana(solana) => {
                let Some(signer) = claim_signer_solana.clone() else {
                    return Err(unwirable(pay_channel, "[settlement.solana] signing key"));
                };
                let claim_state = Arc::new(OwnedHttpClaimState::new(
                    client,
                    solana.client_edge_url().as_str(),
                    ClaimStateChallengeSigner::Solana(signer),
                ));
                connector
                    .with_solana_outbound_client_hop(
                        solana.peer_id(),
                        solana.channel_account(),
                        solana.program_id(),
                        claim_state,
                    )
                    .map_err(|_| RuntimeError::PayChannelUnusable {
                        channel_id: solana.channel_account().to_string(),
                    })?
            }
        };
        tracing::info!(
            peer_id = pay_channel.peer_id(),
            channel = pay_channel.channel(),
            chain = ?pay_channel.chain(),
            client_edge = %pay_channel.client_edge_url(),
            "covering every PREPARE forwarded to this peer with a client-role claim (ADR 0042)"
        );
    }
    Ok(connector)
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
    /// Where the client edge resolves an undeclared EVM payment channel
    /// (issue #556) -- `Some` exactly when `[settlement.evm]` (or the
    /// legacy `[settlement] chain = "evm"`) is configured, since the
    /// deployed `TokenNetwork` it names is what holds the answer. `None`
    /// leaves the client edge with only `[[client_channels]]` to go on for
    /// EVM claims, which is what a node with no EVM settlement backend has.
    pub client_channel_source_evm: Option<Arc<dyn ClientChannelSource>>,
    /// The Solana twin of [`Self::client_channel_source_evm`] (issue #631)
    /// -- `Some` exactly when `[settlement.solana]` is configured.
    pub client_channel_source_solana: Option<Arc<dyn ClientChannelSource>>,
    /// Which Solana cluster this node actually settles on, as the chain
    /// itself reported when the backend connected -- its genesis hash
    /// (`SolanaSettlementBackend::cluster`, issue #1131). `None` when there
    /// is no `[settlement.solana]` table, or when the chain is one no
    /// public cluster's published genesis hash matches (a
    /// `solana-test-validator`).
    ///
    /// Carried on the runtime for the same reason every other field here is
    /// (this struct's own doc): it is a fact the chain connection proved,
    /// and reconstructing it would mean connecting twice. [`router`] hands
    /// it to the client edge, where a claim's self-declared `cluster` is
    /// compared against it (issue #975).
    pub solana_cluster: Option<&'static str>,
    /// Every configured chain's channel-opening facts (issues #617, #632):
    /// one entry per `[settlement.<chain>]` table this node has, so a node
    /// settling on N chains (epic #627) publishes all N. Composed here in
    /// `build` because this is the one place the config's own values and the
    /// facts the chain connection proved (chain id, the resolved
    /// `TokenNetwork`, the backend's signing address) are both in scope.
    ///
    /// **This is the only copy.** The x402 greeting's legacy singular
    /// `extra.settlement` object used to be composed beside this list and
    /// carried separately; since ADR 0050 it is
    /// [`connector_domain::NodeFacts::evm_settlement`] -- the list's own EVM
    /// entry, derived. A node has at most one `[settlement.evm]` table, so
    /// there was never a second fact there to hold, only a second chance to
    /// disagree.
    pub settlements: Vec<connector_client_edge::X402ChainSettlementTerms>,
    /// One entry per chain this node has opted into accepting an x402
    /// `batch-settlement` channel on (ADR 0074 decision 8, issue #1345) --
    /// composed alongside `settlements` in the same loop, from the same
    /// backend, so the two can never name a different chain. Empty on a
    /// node whose settlement tables write no `batch_settlement` sub-table,
    /// which is every node before this record.
    pub batch_settlements: Vec<connector_client_edge::X402BatchSettlementTerms>,
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
    /// This node's receive-only x402 `batch-settlement` backend on EVM (ADR
    /// 0074), `Some` exactly when `[settlement.evm.batch_settlement]` is
    /// written. [`router`] hands it to the client edge's claim gate, which
    /// admits vouchers through it; every channel the client-edge journal
    /// holds vouchers on is already restored to it by the time [`build`]
    /// returns, so its `channel_state` and `land` work from the first
    /// packet. [`router`] also starts its watcher and sweep over it (issue
    /// #1344, `spawn_batch_settlement_watchers`).
    pub batch_settlement_evm: Option<Arc<EvmBatchSettlementBackend>>,
    /// The Solana twin of [`Self::batch_settlement_evm`], `Some` exactly
    /// when `[settlement.solana.batch_settlement]` is written -- and what
    /// the public sponsor endpoint (issue #1346) co-signs an `open` with.
    pub batch_settlement_solana: Option<Arc<SolanaBatchSettlement>>,
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
/// That same connection is handed to [`router`] as a
/// [`ClientChannelSource`] (issue #556): one chain connection, used both
/// to move value and to read who a channel belongs to.
///
/// A node that names a `state_dir` also has its peer claim journal armed
/// here (issue #605), so the watermarks `ClaimBook` keeps outlive the
/// process exactly as the client edge's do. That ledger sits *above* the
/// peer transport port and was untouched by #679's deletion.
pub async fn build(config: &Config) -> Result<Runtime, RuntimeError> {
    let signer = build_signer(config.signer_key())?;
    // Issue #678 gap 3, said once and loudly. A plaintext peering could not
    // have loaded unless somebody wrote `peer_allow_plaintext_endpoints`,
    // and the whole point of a loopback-and-test opt-in is that a node that
    // took it says so where an operator will see it.
    warn_about_plaintext_peerings(config);
    // The claim identity of this node's peerings (ADR 0024): the EVM
    // settlement key, because an outbound peer claim is an EIP-712 balance
    // proof the counterparty's `TokenNetwork` verifies against the channel
    // participant this node *is* on chain -- which is the settlement
    // address, never `[signer]`'s identity key (that one opens gift wraps
    // and answers `GET /ilp/identity`). A node with no `[settlement.evm]`
    // table has no such identity, emits no claim, and says so here rather
    // than signing one under a key nothing would accept.
    let peer_claim_identity = peer_claim_identity(config)?;
    let peer_signer_address = peer_claim_identity
        .as_ref()
        .map(|(_, address)| *address)
        .unwrap_or([0u8; 20]);
    // The Solana twin of the above (issue #732/#998): the
    // `[settlement.solana]` key, because a Solana peer claim is likewise
    // redeemed against the channel participant this node *is* on chain,
    // never `[signer]`'s identity key.
    let peer_claim_identity_solana = peer_claim_identity_solana(config)?;
    let peer_signer_solana_public_key = peer_claim_identity_solana
        .as_ref()
        .map(|signer| signer.public_key());
    // Issue #678 gap 2: the dial side, built from `[[peers]]` and
    // `[[peer_channels]]`. A node with no dialable peering still holds an
    // empty `InProcessPeerTransport`, so a packet routed to a peer is
    // answered `T01 peer unreachable` rather than silently dropped.
    let peer_transport = peer_transport::build_peer_transport(
        config,
        peer_signer_address,
        peer_signer_solana_public_key,
        Arc::new(SystemClock),
    );
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
    // The same node-wide opt-in a `[[peers]]` endpoint takes (issue #678
    // gap 3), so a peering established at runtime picks its carriage by
    // exactly the rule a config-file peering does.
    .with_peer_allow_plaintext_endpoints(config.peer_allow_plaintext_endpoints())
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
    // `[[peer_channels]]` reaching `ClaimBook` at last (§11: "it MUST
    // actually wire `ClaimBook`'s signer, verification key and EIP-712
    // domain, with no code-only setters left on the config path"). Before
    // this, the table loaded, validated, and reached nothing -- so every
    // peer claim was refused `unknown_channel` and none was ever signed,
    // which is #620's gap 3 surviving into the bring-up.
    // Cloned rather than moved: the very same settlement key signs this
    // node's CLIENT-role covering claims below (ADR 0030 -- "no second key
    // is introduced"), because both roles sign against the same on-chain
    // channel participant.
    let claim_signer = peer_claim_identity
        .as_ref()
        .map(|(signer, _)| Arc::clone(signer));
    // The Solana half of the same sentence (issue #1146): the ed25519 key
    // that signs this peering's outbound peer claims is the key that signs
    // its client-role covering claims too.
    let claim_signer_solana_for_cover = peer_claim_identity_solana.as_ref().map(Arc::clone);
    if let Some((claim_signer, _)) = peer_claim_identity {
        connector = connector.with_signer(claim_signer);
    }
    if let Some(claim_signer_solana) = peer_claim_identity_solana {
        connector = connector.with_solana_signer(claim_signer_solana);
    }
    connector = wire_peer_channels(connector, config)?;
    // ADR 0042 item 2, issue #881: `[[pay_channels]]` reaching
    // `Connector::with_outbound_client_hop` at last. A node with no such
    // table is untouched by this call -- see the function's own doc.
    connector = wire_outbound_client_hops(
        connector,
        config,
        claim_signer,
        claim_signer_solana_for_cover,
    )?;
    let mut client_channel_source_evm: Option<Arc<dyn ClientChannelSource>> = None;
    let mut client_channel_source_solana: Option<Arc<dyn ClientChannelSource>> = None;
    let mut solana_cluster: Option<&'static str> = None;
    let mut settlements: Vec<connector_client_edge::X402ChainSettlementTerms> = Vec::new();
    // ADR 0074 decision 8, issue #1345: this node's x402 batch-settlement
    // facts, one entry per chain whose settlement table opted in. Composed
    // in the same loop as `settlements`, off the same connected backend, so
    // the greeting's `batch-settlement` entry and its `toon-channel` entry
    // can never name two different deployments of "this chain".
    let mut batch_settlements: Vec<connector_client_edge::X402BatchSettlementTerms> = Vec::new();
    let mut batch_settlement_evm: Option<Arc<EvmBatchSettlementBackend>> = None;
    let mut batch_settlement_solana: Option<Arc<SolanaBatchSettlement>> = None;
    // Each table's one transport, built once and handed to every client of
    // its `rpc_url` below (ADR 0073 decision 2).
    let transports = settlement_transports(config)?;
    for settlement in config.settlements() {
        match settlement {
            SettlementConfig::Evm(evm) => {
                let transport = transports.for_chain(SettlementChain::Evm)?;
                let backend = build_evm_settlement_backend(evm, transport).await?;
                // The file, held against the chain, before a single fact
                // this backend resolved is used for anything else (issue
                // #1136). Same posture and same moment as `connect`'s own
                // `decimals` check (#564): the first thing to do with a
                // chain's answer is find out whether the config file agrees
                // with it.
                check_evm_channel_domains(
                    config,
                    EvmDomain {
                        chain_id: backend.chain_id(),
                        token_network: backend.address().to_fixed_bytes(),
                    },
                )?;
                // The greeting's channel-opening facts (issue #617).
                // Addresses the chain connection proved (`own_address`, the
                // resolved `TokenNetwork`, the live chain id) come from the
                // backend; the registry, token and scale come from the very
                // config lines `connect` just verified against that chain
                // (issues #564/#576).
                let evm_terms = connector_client_edge::X402SettlementTerms {
                    chain: format!("evm:{}", backend.chain_id()),
                    settlement_address: format!("{:#x}", backend.own_address()),
                    token_network_registry: format!("{:#x}", backend.registry_address()),
                    token_network: format!("{:#x}", backend.address()),
                    token_address: format!(
                        "{:#x}",
                        ethers::types::Address::from(evm.token_address())
                    ),
                    decimals: evm.decimals(),
                };
                // ADR 0074 decision 8, issue #1345: this chain's
                // batch-settlement facts, present only when this table
                // opted in. `network`, `asset` and `receiverAuthorizer`/
                // `payTo` are read off `evm_terms` just above rather than
                // recomputed, so the two entries can never disagree about
                // which chain or which address this is (CF-26).
                if let Some(batch) = evm.batch_settlement() {
                    batch_settlements.push(connector_client_edge::X402BatchSettlementTerms::Evm(
                        connector_client_edge::X402BatchSettlementEvmTerms {
                            network: format!("eip155:{}", backend.chain_id()),
                            asset: evm_terms.token_address.clone(),
                            pay_to: evm_terms.settlement_address.clone(),
                            receiver_authorizer: evm_terms.settlement_address.clone(),
                            min_withdraw_delay_secs: batch.min_withdraw_delay_secs(),
                            name: batch.asset_eip712_name().to_string(),
                            version: batch.asset_eip712_version().to_string(),
                        },
                    ));
                }
                settlements.push(connector_client_edge::X402ChainSettlementTerms::Evm(
                    evm_terms,
                ));
                // Issue #661: the local channel index answers a resolution
                // from a `HashMap` probe once it has caught up to a
                // channel, and falls through to exactly the direct chain
                // read `SettlementChannelSource` always performed for
                // everything it has not (see `IndexedEvmChannelSource`'s
                // own doc). Opened -- and, on a durable failure, refused --
                // before any traffic is served, same as every other
                // `state_dir`-scoped store (ADR 0009).
                // Bound to the chain and contract it indexes (issue #1282):
                // a state volume outlives an `[settlement.evm]` cutover, so
                // a snapshot that does not say which deployment it is about
                // is resumed against whichever one the file now names --
                // which is how a Base Sepolia checkpoint came to be resumed
                // against Base mainnet, serving five wrong-chain channels
                // out of the index the whole time. Both facts come off the
                // backend just built, so they are what the chain answered
                // rather than what the config claims.
                //
                // The same two facts `check_evm_channel_domains` took as an
                // `EvmDomain` a few lines above, and deliberately a
                // different type: that one is the EIP-712 domain a peer
                // claim is signed against (ADR 0024, issue #1136), this one
                // is the provenance of a cache. They agree here because
                // both read the same backend, not because either derives
                // from the other.
                let indexed_contract = IndexedContract {
                    chain_id: backend.chain_id(),
                    token_network: backend.address(),
                };
                let channel_index = open_evm_channel_index(
                    config.state_dir(),
                    indexed_contract,
                    evm.channel_index_from_block(),
                )?;
                // The syncer queries the contract the index is bound to,
                // from the one value, so the two cannot drift into
                // indexing one contract under another's name.
                // The same transport as the backend's: a syncer dialing on
                // its own would be the one client left direct (ADR 0073).
                let syncer = EvmChannelIndexSyncer::new(
                    transport,
                    indexed_contract.token_network,
                    evm.channel_index_confirmations(),
                    evm.channel_index_from_block(),
                );
                // Backfill-then-poll runs for the life of the process,
                // never blocking startup (issue #661's own acceptance
                // criterion) -- a lagging or never-connecting sync logs at
                // `warn` (`EvmChannelIndexSyncer::run`'s own doc) and the
                // fallback below keeps serving exactly as it does today.
                tokio::spawn(syncer.run(Arc::clone(&channel_index), DEFAULT_POLL_INTERVAL));
                client_channel_source_evm = Some(Arc::new(IndexedEvmChannelSource {
                    index: channel_index,
                    fallback: SettlementChannelSource {
                        backend: backend.clone(),
                    },
                }));
                // ADR 0074: the opt-in, bound before anything is served.
                batch_settlement_evm = build_evm_batch_settlement(evm, &backend).await?;
                connector = connector
                    .with_settlement(SettlementChain::Evm, backend as Arc<dyn SettlementBackend>);
            }
            SettlementConfig::Solana(solana) => {
                // Constructed and attached exactly as the EVM leg is (issue
                // #630) -- `SolanaSettlementBackend::connect`'s own
                // fail-closed identity checks (program reachable,
                // executable and proven to behave like the deployed
                // payment-channel program, mint owned by the SPL Token
                // program, configured decimals agreeing with the mint's
                // own) run before this node serves any traffic.
                // `ClientChannelSource` now covers Solana too (issue #631),
                // and the greeting's per-chain settlement facts are
                // composed here as well (issue #632) -- epic #627's
                // remaining children, together.
                let transport = transports.for_chain(SettlementChain::Solana)?;
                let backend = build_solana_settlement_backend(solana, transport).await?;
                // ADR 0074: the opt-in, over the same transport (ADR 0073).
                batch_settlement_solana = build_solana_batch_settlement(solana, transport).await?;
                // Which chain that connection actually reached, from the
                // chain's own genesis hash rather than from the shape of
                // the URL used to reach it (issue #1131).
                solana_cluster = backend.cluster();
                client_channel_source_solana = Some(Arc::new(SolanaChannelSource {
                    backend: backend.clone(),
                }));
                settlements.push(connector_client_edge::X402ChainSettlementTerms::Solana(
                    connector_client_edge::X402SolanaSettlementTerms {
                        chain: "solana".to_string(),
                        settlement_address: backend.own_pubkey().to_string(),
                        program_id: backend.program_id().to_string(),
                        token_address: backend.token_mint().to_string(),
                        decimals: solana.decimals(),
                    },
                ));
                // ADR 0074 decision 8, issue #1345: this node's Solana
                // batch-settlement facts, present only when this table
                // opted in. `payTo` and `feePayer` are both this backend's
                // own pubkey (decision 5's sponsor-is-the-receiving-operator
                // rule), and `network` is read off the chain's own genesis
                // hash (`caip2_network`), never guessed from the RPC URL.
                // The two minimums are the batch backend's own: what it
                // admits by and what its sponsor co-signs above are what is
                // published, with no second read of the config.
                if let Some(batch) = &batch_settlement_solana {
                    batch_settlements.push(
                        connector_client_edge::X402BatchSettlementTerms::Solana(
                            connector_client_edge::X402BatchSettlementSolanaTerms {
                                network: backend.caip2_network(),
                                asset: backend.token_mint().to_string(),
                                pay_to: backend.own_pubkey().to_string(),
                                fee_payer: backend.own_pubkey().to_string(),
                                min_grace_period_secs: batch.min_grace_period_secs(),
                                min_deposit: batch.min_sponsored_deposit().to_string(),
                            },
                        ),
                    );
                }
                connector = connector.with_settlement(
                    SettlementChain::Solana,
                    backend as Arc<dyn SettlementBackend>,
                );
            }
        }
    }
    // The peer semantics's own claim watermarks, made durable by the same
    // `state_dir` the client edge's are (issue #605, and #556's
    // reconciliation row "Journal: `ClaimBook::new(None, ..)` installs the
    // in-memory journal ... watermarks reset on restart; spent nonces
    // respend"). Both surfaces are the same sentence -- a watermark must
    // outlive the process -- so they get the same answer rather than two,
    // and arming one while leaving the other in memory would mean shipping
    // the fix and the bug side by side.
    if let Some(state_dir) = config.state_dir() {
        let journal = open_journal(state_dir, PEER_CLAIM_JOURNAL)?;
        connector = connector.with_journal(journal).map_err(|source| {
            RuntimeError::JournalUnreplayable {
                path: state_dir.join(PEER_CLAIM_JOURNAL),
                source,
            }
        })?;
        // Issue #884: replay this node's durable runtime peer/route table,
        // and arm the connector to persist future writes back to the same
        // file -- the same `state_dir` scoping as the two journals above,
        // so an operator restoring a node from `state_dir` alone restores
        // this table too. `open_journal` just created `state_dir` itself,
        // so a node with nowhere writable has already failed above with
        // the path in the message.
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
    // ADR 0074: every batch-settlement channel the client edge's journal
    // holds vouchers on, restored before anything is served, so the port
    // can land them after a restart -- restored, not re-admitted, so a rule
    // tightened since cannot strand a voucher already accepted (decision 5).
    // The journal is where an EVM channel's config survives a restart; the
    // chain never gives it back.
    if batch_settlement_evm.is_some() || batch_settlement_solana.is_some() {
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
                batch_settlement_evm
                    .as_deref()
                    .map(|backend| backend as &dyn BatchSettlementBackend),
                batch_settlement_solana
                    .as_deref()
                    .map(|backend| backend as &dyn BatchSettlementBackend),
            )
            .await;
        }
    }
    let connector = Arc::new(connector);
    Ok(Runtime {
        connector,
        signer,
        client_channel_source_evm,
        client_channel_source_solana,
        solana_cluster,
        settlements,
        batch_settlements,
        rate_table,
        batch_settlement_evm,
        batch_settlement_solana,
    })
}

/// The claim gate's view of this node's batch-settlement backends (ADR
/// 0074), or `None` when no chain has opted in -- and a gate given `None`
/// refuses every voucher by name.
fn batch_settlement_channels(runtime: &Runtime) -> Option<BatchSettlementChannelsAdapter> {
    BatchSettlementChannelsAdapter::new(
        runtime.batch_settlement_evm.as_ref().map(|backend| {
            (
                Arc::clone(backend) as Arc<dyn BatchSettlementBackend>,
                backend.domain(),
            )
        }),
        runtime
            .batch_settlement_solana
            .as_ref()
            .map(|backend| Arc::clone(backend) as Arc<dyn BatchSettlementBackend>),
    )
}

/// Start one background poller per declared quote path, each refreshing its
/// pair into `table` at the cadence that pair's `ttl` implies (ADR 0071
/// decision 6, issue #1294). Returns how many were started.
///
/// Spawned, never awaited: a packet must never wait on a rate source, and
/// startup must never wait on a chain -- the same shape
/// `EvmChannelIndexSyncer::run` already runs in. A node whose tokens declare
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
/// (`ConfigError::TokenQuoteWithoutSettlement`). So the endpoint is that
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
        chains = %sources
            .chains()
            .map(|chain| chain.as_str())
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
/// (`ConfigError::TokenQuoteWithoutSettlement`). So the endpoint is that
/// table's `rpc_url`, read here, and a second key could only ever disagree
/// with the one the node already dials.
///
/// Reading a pool is *not* settling: this builds its own reader over the
/// same endpoint rather than reaching through a `SettlementBackend`, which
/// decision 6 keeps out of every value path. The two share a URL and
/// nothing else.
fn rate_sources(transports: &SettlementTransports) -> RateSources {
    let mut sources = RateSources::none();
    // The EVM table's own transport, shared with its backend and syncer:
    // a rate source dialing on its own would be the one client of that
    // `rpc_url` left direct (ADR 0073 decision 2).
    if let Some(transport) = &transports.evm {
        sources = sources.with(
            AssetChain::Evm,
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
    evm: Option<RpcTransport>,
    solana: Option<RpcTransport>,
}

impl SettlementTransports {
    /// The transport for a table `config.settlements()` named. The loader
    /// guarantees one per configured chain, so a miss is a wiring error
    /// reported rather than a panic.
    fn for_chain(&self, chain: SettlementChain) -> Result<&RpcTransport, RuntimeError> {
        let transport = match chain {
            SettlementChain::Evm => self.evm.as_ref(),
            SettlementChain::Solana => self.solana.as_ref(),
        };
        transport.ok_or(RuntimeError::SettlementEndpointUnusable {
            table: chain.name(),
            message: "no transport was built for a table the config names".to_string(),
        })
    }
}

/// Build [`SettlementTransports`] from `config`'s settlement tables.
pub(crate) fn settlement_transports(config: &Config) -> Result<SettlementTransports, RuntimeError> {
    let mut transports = SettlementTransports {
        evm: None,
        solana: None,
    };
    for settlement in config.settlements() {
        let chain = settlement.chain();
        let unusable = |message: String| RuntimeError::SettlementEndpointUnusable {
            table: chain.name(),
            message,
        };
        let transport = if settlement.rpc_via_socks_proxy() {
            // `Config::load` refuses this key without a `socks_proxy`, so the
            // miss is reported rather than expected; it is never a reason to
            // dial direct (ADR 0073 decision 2).
            let proxy = config.socks_proxy().ok_or_else(|| {
                unusable("rpc_via_socks_proxy is set and there is no socks_proxy".to_string())
            })?;
            let circuit = match chain {
                SettlementChain::Evm => Circuit::EvmSettlement,
                SettlementChain::Solana => Circuit::SolanaSettlement,
            };
            let transport = RpcTransport::through(settlement.rpc_url(), proxy, circuit)
                .map_err(|error| unusable(error.to_string()))?;
            tracing::info!(
                table = chain.name(),
                endpoint = %transport.endpoint(),
                circuit = circuit.socks_username(),
                "settlement rpc via socks_proxy"
            );
            transport
        } else {
            RpcTransport::direct(settlement.rpc_url())
                .map_err(|error| unusable(error.to_string()))?
        };
        match chain {
            SettlementChain::Evm => transports.evm = Some(transport),
            SettlementChain::Solana => transports.solana = Some(transport),
        }
    }
    Ok(transports)
}

/// How often [`router`]'s spawned loop sweeps the client edge's channels
/// for one the chain no longer vouches for (issue #977).
///
/// What this interval bounds is *detection*, and only that. A watermark is
/// reset only while the chain still reports its channel gone
/// ([`ClientClaimGate::reap_unresolvable_channels`]), so a sweep clears a
/// settled channel within one interval of the settle becoming visible --
/// but a channel reopened at the same deterministic address before any
/// sweep lands is one this node never observes settled at all, and it
/// keeps its predecessor's watermark. A shorter interval narrows that
/// window; nothing short of re-keying a watermark per incarnation (the
/// alternative issue #977 itself names) closes it.
///
/// Five minutes: short enough to catch the settle of a channel whose payer
/// takes a beat to reopen it, long enough that a node with few or no
/// client channels open pays nothing worth naming for the sweep -- and on
/// the same order as `DEFAULT_SERVE_STALE_UNTIL`'s own ten-minute
/// staleness ceiling for the same channels.
const CLIENT_CHANNEL_REAP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);

/// Sweep `gate`'s channels for one whose watermark should be reset every
/// [`CLIENT_CHANNEL_REAP_INTERVAL`], forever -- the periodic wrapper around
/// [`ClientClaimGate::reap_unresolvable_channels`], which does the actual
/// resolution and reset and is otherwise chain- and I/O-agnostic. Never
/// returns, so it is spawned rather than awaited: a sweep that fails costs
/// a cycle rather than the loop.
async fn reap_unresolvable_client_channels_periodically(gate: Arc<ClientClaimGate>) {
    let mut interval = tokio::time::interval(CLIENT_CHANNEL_REAP_INTERVAL);
    loop {
        interval.tick().await;
        gate.reap_unresolvable_channels().await;
    }
}

/// Start the watchers and sweeps over every batch-settlement backend this
/// node opted in to (ADR 0074 decision 5, issue #1344), reading the vouchers
/// to land from `gate`, the one place a voucher is accepted. Spawned, never
/// awaited, for the life of the process, like the reaper beside it: a step
/// that fails is logged and retried on its next tick. A node that opted in
/// on neither chain starts nothing.
///
/// - EVM: [`EvmBatchWatcher`] reads `WithdrawInitiated` every
///   [`WITHDRAWAL_WATCH_INTERVAL`] and claims the latest voucher on a
///   withdrawing channel at once, and every [`BATCH_SWEEP_INTERVAL`] claims
///   every held voucher in one `claim` and then `settle`s.
/// - Solana: [`SolanaBatchWatcher`] rediscovers every sponsored channel every
///   [`CLOSING_WATCH_INTERVAL`] -- sealing a Closing one with its latest
///   voucher, distributing a Sealed one, reclaiming a Distributed one's rent
///   -- and settles Open ones every [`OPEN_SETTLE_INTERVAL`].
///
/// Both backends read and write through the settlement table's one transport
/// (ADR 0073): the watchers are built from the backends, never from a URL.
///
/// [`WITHDRAWAL_WATCH_INTERVAL`]: connector_settlement_evm::WITHDRAWAL_WATCH_INTERVAL
/// [`BATCH_SWEEP_INTERVAL`]: connector_settlement_evm::BATCH_SWEEP_INTERVAL
/// [`CLOSING_WATCH_INTERVAL`]: connector_settlement_solana::batch::CLOSING_WATCH_INTERVAL
/// [`OPEN_SETTLE_INTERVAL`]: connector_settlement_solana::batch::OPEN_SETTLE_INTERVAL
fn spawn_batch_settlement_watchers(runtime: &Runtime, gate: &Arc<ClientClaimGate>) {
    let held: Arc<dyn HeldVouchers> = Arc::new(ClaimGateVouchers(Arc::clone(gate)));
    if let Some(backend) = &runtime.batch_settlement_evm {
        let watcher = EvmBatchWatcher::new(Arc::clone(backend), Arc::clone(&held));
        tokio::spawn(watcher.run(
            connector_settlement_evm::WITHDRAWAL_WATCH_INTERVAL,
            connector_settlement_evm::BATCH_SWEEP_INTERVAL,
        ));
    }
    if let Some(backend) = &runtime.batch_settlement_solana {
        let watcher = SolanaBatchWatcher::new(Arc::clone(backend), held);
        tokio::spawn(watcher.run(
            connector_settlement_solana::batch::CLOSING_WATCH_INTERVAL,
            connector_settlement_solana::batch::OPEN_SETTLE_INTERVAL,
        ));
    }
}

/// The channels this node accepts client-edge claims on, and whose
/// counterparty each claim's signature must recover to (issues #558,
/// #556, #631): everything `[[client_channels]]` declares, plus -- when
/// `[settlement.evm]`/`[settlement.solana]` is configured -- that chain's
/// own deployed contract/program, for any channel the config file does not
/// mention.
///
/// The two compose rather than replace each other. A declared channel is
/// still answered from config without touching a chain, so a node with no
/// settlement backend still declares its channels and a node whose RPC
/// endpoint is down still serves the channels it wrote down. What a source
/// adds is the case `[[client_channels]]` cannot express: a buyer this
/// operator has never heard of, who opened a channel on chain and wants to
/// pay for a write (issue #502).
///
/// A node with neither still has a record of no channel and refuses every
/// claim -- deliberately, since the only alternative to "no record of this
/// channel" is trusting what the claim says about its own signer, which is
/// exactly the hole #558 closes.
///
/// `solana_cluster` is the cluster the Solana chain itself named at connect
/// (`SolanaSettlementBackend::cluster`, issue #1131), or `None` when this
/// node has no Solana backend to have asked -- see the cluster comment in
/// the body for how it composes with the configured `rpc_url`'s hint.
fn client_channels(
    config: &Config,
    evm_source: Option<Arc<dyn ClientChannelSource>>,
    solana_source: Option<Arc<dyn ClientChannelSource>>,
    solana_cluster: Option<&'static str>,
) -> ClientChannelRegistry {
    let mut channels = ClientChannelRegistry::new();
    // This node's `[settlement.solana]` table, for the cluster question
    // below. The settlement *program* is no longer read here: a configured
    // Solana client channel carries it, filled in from this same table by
    // `Config::load` (ADR 0053, issues #1082 and #1138), and a row on a
    // node with no such table does not load at all.
    let solana_settlement = config
        .settlements()
        .iter()
        .find_map(|settlement| match settlement {
            connector_config::SettlementConfig::Solana(solana) => Some(solana),
            connector_config::SettlementConfig::Evm(_) => None,
        });
    // Which cluster this node settles on, when its `rpc_url` says (issue
    // #975). A claim declaring a *different* cluster is refused rather than
    // endorsed -- the one field in a Solana claim that names a chain, that
    // no signature can ever bind (a Solana program cannot know its own
    // cluster, ADR 0053), and that nothing compared before this.
    //
    // Two answers, in that order (issue #1131). `solana_cluster` is what
    // the chain said about itself when this node connected -- its genesis
    // hash, the one identity a Solana chain states rather than is named by
    // -- and it holds however the node reached the chain, including behind
    // a paid RPC provider. `cluster_hint` is a hostname allowlist over the
    // configured `rpc_url` and answers `None` for every such provider, so
    // it is the fallback rather than the source: it covers only the one
    // case the chain cannot, a `solana-test-validator`, whose fresh genesis
    // matches no published cluster hash but whose loopback URL still says
    // `localnet`.
    //
    // `None` from both still travels: a node that does not know where it
    // is compares nothing rather than guessing.
    let cluster_hint = solana_settlement.and_then(|solana| solana.cluster_hint());
    if let (Some(chain), Some(hint)) = (solana_cluster, cluster_hint) {
        if chain != hint {
            // Not a refusal. The chain is authoritative and is taken; the
            // hint being wrong is a fact about a hostname guess, and
            // refusing to boot over a guess this node has already
            // superseded would be an outage caused by the weaker source.
            tracing::warn!(
                chain_cluster = chain,
                rpc_url_hint = hint,
                "[settlement.solana] rpc_url's hostname names one cluster but the chain it \
                 reaches reports another; the chain's own genesis hash is authoritative and is \
                 what a claim's declared cluster is checked against"
            );
        }
    }
    if let Some(cluster) = solana_cluster.or(cluster_hint) {
        channels = channels.with_solana_cluster(cluster);
    }
    for channel in config.client_channels() {
        match channel {
            ClientChannelConfig::Evm(evm) => {
                channels
                    .record_evm(
                        evm.channel_id(),
                        EvmChannel {
                            counterparty: evm.counterparty(),
                            chain_id: evm.chain_id(),
                            token_network_address: evm.token_network_address(),
                            // `[[client_channels]]` declares a
                            // counterparty and a domain, never an amount,
                            // and a node with no settlement backend has no
                            // chain to ask -- so a declared channel is
                            // exempt from the collateral cap (issue #646),
                            // deliberately: hand-declaring a channel is
                            // itself the operator's policy decision,
                            // correctly located in config.
                            deposit_floor: DepositFloor::Unknown,
                        },
                    )
                    .expect(
                        "config load already validated every channel_id as a 32-byte identifier",
                    );
            }
            ClientChannelConfig::Solana(solana) => {
                // A client channel this node accepts claims on lives under
                // the one Solana program this node settles with, so the
                // program id comes from `[settlement.solana]` rather than
                // being declared a second time per channel -- ADR 0053, and
                // the same "no second declaration" rule that closed #981.
                //
                // Read off the row, which `Config::load` filled in from
                // `[settlement.solana]` (issue #1138). This arm used to
                // look the table up again here and warn-and-skip a row it
                // could not back -- a configured channel that silently
                // refused every claim as unknown. It is refused by name at
                // load now (`ClientChannelWithoutSolanaSettlement`), so
                // there is nothing left here to skip.
                channels
                    .record_solana(
                        solana.channel_account(),
                        solana.counterparty(),
                        solana.program_id(),
                    )
                    .expect(
                        "config load already validated the account, the counterparty and the \
                         settlement program id as base58 32-byte values",
                    );
            }
        }
    }
    if let Some(source) = evm_source {
        channels = channels.with_source(source);
    }
    if let Some(source) = solana_source {
        channels = channels.with_solana_source(source);
    }
    // The liveness knobs a config file turns (issue #649): how long a
    // chain-resolved channel's mutable facts may be believed, how long its
    // last good reading may still be served while the chain is
    // unreachable, and how often one channel may make this node read the
    // chain at all. Each absent field leaves the client edge's own
    // default, which is what every node that has not thought about it
    // should have -- an operator whose RPC endpoint is rate-limited is the
    // one who needs the levers, and they now have them without a rebuild.
    let defaults = ChannelLivenessPolicy::default();
    channels = channels.with_liveness_policy(ChannelLivenessPolicy {
        refresh_after: config
            .channel_liveness_ttl()
            .unwrap_or(defaults.refresh_after),
        serve_stale_until: config
            .channel_serve_stale()
            .unwrap_or(defaults.serve_stale_until),
        min_reattempt_interval: config
            .channel_reattempt_interval()
            .unwrap_or(defaults.min_reattempt_interval),
    });
    // The shaper on lookups for channels that never resolve (issue #613) --
    // the bound the liveness knobs above structurally cannot provide, since
    // each of them reads a memo entry and an unresolvable channel leaves
    // none. Same shape as the knobs above and for the same reason: what a
    // node can afford to spend discovering channels that turn out not to
    // exist depends on the settlement endpoint it is paying for, which is a
    // deployment fact rather than a protocol constant.
    let budget = UnresolvableLookupBudgetPolicy::default();
    channels = channels.with_lookup_budget(UnresolvableLookupBudgetPolicy {
        per_signer: config
            .unresolvable_lookups_per_signer()
            .unwrap_or(budget.per_signer),
        total: config.unresolvable_lookups_total().unwrap_or(budget.total),
        window: config.unresolvable_lookup_window().unwrap_or(budget.window),
        max_wait: config
            .unresolvable_lookup_max_wait()
            .unwrap_or(budget.max_wait),
    });
    channels
}

/// The client edge's claim gate: the channels this node accepts claims on,
/// resumed from the watermarks its journal already records (issue #605).
///
/// A node with no `state_dir` gets an in-memory journal, which is sound
/// only because [`Config::load`] has already refused any config that both
/// omits `state_dir` and configures a channel to accept claims on: such a
/// gate refuses every claim as unknown, so it has no watermark to lose.
///
/// `evm_source`/`solana_source` are threaded straight through to
/// [`client_channels`] (issues #556, #631): a gate resolves an undeclared
/// channel from the chain and journals its watermark like any other, so
/// the unaffiliated buyer's claims are exactly as replay-proof across a
/// restart as a declared buyer's are.
///
/// Bound to [`client_payout_ledger`] (issue #770) before it is returned:
/// without this, a node's own outbound crediting is netted against nothing
/// (`ClientClaimGate::credited_evm`'s pre-#770 default), which is exactly
/// the production gap issue #770 closes -- the gate and the ledger it nets
/// against must never be assembled separately again.
fn client_claim_gate(
    config: &Config,
    signer: Arc<dyn Signer>,
    evm_source: Option<Arc<dyn ClientChannelSource>>,
    solana_source: Option<Arc<dyn ClientChannelSource>>,
    solana_cluster: Option<&'static str>,
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
    let gate = ClientClaimGate::restore(
        client_channels(config, evm_source, solana_source, solana_cluster),
        journal,
    )
    .map_err(|source| RuntimeError::JournalUnreplayable { path, source })?;
    Ok(gate.with_payout_ledger(client_payout_ledger(config, signer)))
}

/// This connector's own outbound claim ledger for the client edge (issue
/// #770): every EVM `[[client_channels]]` entry, registered under the same
/// signer that already signs this connector's identity and its peer-role
/// outbound claims (`Connector::with_identity_signer`) -- one signing key
/// for everything this connector owes, not a second one minted for this
/// edge alone. A session earning against an undeclared (chain-resolved)
/// channel is not covered here: crediting one requires knowing which
/// domain to sign under, and only a declared `[[client_channels]]` entry
/// carries that -- the same reason `client_channels` above resolves an
/// *inbound* claim's channel from the chain but a payout ledger cannot
/// mirror it for the outbound direction.
///
/// Solana channels are skipped: [`ClientPayoutLedger`] wraps
/// `connector_runtime::ClaimBook`, which only ever signs an EVM balance
/// proof (issue #742's own scope note) -- a Solana client channel nets
/// nothing yet, matching `ClientClaimGate::credited`'s existing "Solana
/// channel nets 0" rule.
fn client_payout_ledger(config: &Config, signer: Arc<dyn Signer>) -> Arc<ClientPayoutLedger> {
    let mut ledger = ClientPayoutLedger::new();
    ledger.set_signer(signer);
    for channel in config.client_channels() {
        if let ClientChannelConfig::Evm(evm) = channel {
            ledger
                .set_channel_domain(
                    evm.channel_id(),
                    ChannelDomain {
                        chain_id: evm.chain_id(),
                        token_network_address: evm.token_network_address(),
                    },
                )
                .expect("config load already validated every channel_id as a 32-byte identifier");
        }
    }
    Arc::new(ledger)
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
///     `https://proxy.ario.devnet.toonprotocol.dev/ilp`;
///   * `peer_carriages` is `peer_expose`, i.e. which listeners this node
///     opens. **Which** carriages exist, never **who** rides them -- peer
///     identities and per-peering terms are operator-private (ND-09);
///   * `settlements` is **proved**: every entry was resolved and checked
///     against a live chain by a `SettlementBackend::connect` that refused to
///     boot on a disagreement. Nothing re-declares any of it.
fn node_facts(config: &Config, runtime: &Runtime) -> connector_domain::NodeFacts {
    let node = config.node();
    connector_domain::NodeFacts {
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
        settlements: runtime.settlements.clone(),
        batch_settlements: runtime.batch_settlements.clone(),
    }
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
    let mut claim_gate = client_claim_gate(
        config,
        signer.clone(),
        runtime.client_channel_source_evm.clone(),
        runtime.client_channel_source_solana.clone(),
        runtime.solana_cluster,
    )?;
    // ADR 0074 decision 1: vouchers are accepted on exactly the chains whose
    // `batch_settlement` table is written, and refused by name on the rest.
    if let Some(channels) = batch_settlement_channels(runtime) {
        claim_gate = claim_gate.with_batch_settlement(Arc::new(channels));
    }
    let claim_gate = Arc::new(claim_gate);
    // Issue #977: a channel's deterministic on-chain address means a
    // reopened channel reuses its settled predecessor's watermark key, and
    // nothing on the claim path itself can ever discover a reopen (see
    // `ClientClaimGate::reap_unresolvable_channels`'s own doc for why).
    // Spawned once per gate, never awaited on the startup path, mirroring
    // `EvmChannelIndexSyncer::run`'s own periodic-sweep shape.
    tokio::spawn(reap_unresolvable_client_channels_periodically(Arc::clone(
        &claim_gate,
    )));
    spawn_batch_settlement_watchers(runtime, &claim_gate);
    let app = connector_client_edge::router_with_node_facts(
        connector.clone(),
        signer.clone(),
        wrap_receiver_secret,
        claim_gate.clone(),
        node_facts(config, runtime),
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
            config.peer_channels(),
            config.peer_expose(),
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
    Ok(match config.operator() {
        Some(operator) => app.merge(connector_operator::router(
            connector,
            claim_gate,
            signer,
            operator.bearer_token().to_string(),
            operator.write_keys().to_vec(),
            declared_rates(runtime, config),
        )),
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

    /// What a `state_dir`-less, chain-less index is told it indexes. The
    /// binding (issue #1282) only ever decides whether a snapshot on disk
    /// is this index's, and these tests have no snapshot on disk at all --
    /// so any pair does, and naming anvil's chain id says which chain the
    /// tests around it are written against.
    fn test_indexed_contract() -> IndexedContract {
        IndexedContract {
            chain_id: 31_337,
            token_network: ethers::types::Address::zero(),
        }
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

    /// A throwaway signer for a [`client_claim_gate`] call whose test is
    /// about channel resolution or journal behaviour, never about payout
    /// signing (issue #770 gave the function a signer parameter it did not
    /// have before).
    fn test_signer() -> Arc<dyn Signer> {
        Arc::new(LocalSigner::generate("connector-cli-runtime-test"))
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

    /// `peer-carriage-spec.md` §11, and #620's gap 3 closed at last: the
    /// `[[peer_channels]]` table must **actually wire `ClaimBook`'s
    /// verification key and EIP-712 domain**, "with no code-only setters
    /// left on the config path".
    ///
    /// Before issue #678 the table loaded, validated and reached nothing,
    /// so every peer claim was refused `unknown_channel` and none was ever
    /// signed. `recognizes_channel` is the observable end of that wiring:
    /// it is true exactly when a counterparty address has been recorded
    /// for the channel, which is the record a peer claim's signature is
    /// recovered against.
    ///
    /// On [`wire`] rather than `build`, for the reason its Solana twin
    /// already is: since issue #1138 an EVM `[[peer_channels]]` row needs
    /// a `[settlement.evm]` table to load at all, and `build` dials the
    /// chain from one -- so keeping this on `build` would turn a question
    /// about config reaching `ClaimBook` into an anvil test.
    #[test]
    fn peer_channels_reach_the_claim_ledger() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let channel = format!("0x{}", "cd".repeat(32));
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"
peer_expose = "btp"

[signer]
key_file = "{key_file}"

[[peers]]
id = "store"
endpoint = "wss://store.example:443/ilp/btp"


[[peer_channels]]
peer_id = "store"
channel_id = "{channel}"
counterparty_key = "0x00000000000000000000000000000000000000aa"
chain_id = 31337
token_network = "0x00000000000000000000000000000000000000bb"
{settlement}"#,
                state_dir = state_dir.path().display(),
                key_file = key_path.display(),
                settlement = evm_settlement(key_path),
            )
        });

        let connector = wire(&config);

        assert!(
            connector.recognizes_channel(&channel),
            "the [[peer_channels]] row must reach ClaimBook's verification key, or every \
             peer claim on it is refused `unknown_channel` however correctly it was signed"
        );
        assert!(
            !connector.recognizes_channel(&format!("0x{}", "ee".repeat(32))),
            "and only that row -- a channel nobody configured is still unknown"
        );
    }

    /// The `[settlement.evm]` table every EVM channel row needs since issue
    /// #1138 -- that table is where this node's EVM address comes from, and
    /// a claim is redeemed by the channel's on-chain participant. Written
    /// once here because no test that uses it is *about* settlement.
    fn evm_settlement(key_path: &Path) -> String {
        format!(
            r#"
[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6

[settlement.evm.key]
key_file = "{key_file}"
"#,
            key_file = key_path.display(),
        )
    }

    /// A config with one Solana `[[peer_channels]]` row and a
    /// `[settlement.solana]` naming `program_id`. Since issue #1128 the two
    /// are inseparable -- the row has no program of its own and load
    /// refuses it without the table -- so the fixture writes both or
    /// neither.
    fn solana_peering_config(state_dir: &Path, program_id: &str) -> (Config, tempfile::TempPath) {
        config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"
peer_expose = "btp"

[signer]
key_file = "{key_file}"

[[peers]]
id = "store"
endpoint = "wss://store.example:443/ilp/btp"


[[peer_channels]]
peer_id = "store"
channel_account = "{SOLANA_PEER_CHANNEL_ACCOUNT}"
counterparty_key = "{SOLANA_PEER_COUNTERPARTY_KEY}"

[settlement.solana]
rpc_url = "https://api.devnet.solana.com"
program_id = "{program_id}"
token_address = "{SOLANA_PEER_COUNTERPARTY_KEY}"
decimals = 6

[settlement.solana.key]
key_file = "{key_file}"
"#,
                state_dir = state_dir.display(),
                key_file = key_path.display(),
            )
        })
    }

    const SOLANA_PEER_CHANNEL_ACCOUNT: &str = "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi";
    const SOLANA_PEER_COUNTERPARTY_KEY: &str = "8pM1DN3RiT8vbom5u1sNryaNT1nyL8CTTW3b5PwWXRBH";
    const SOLANA_PEER_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

    /// The production wiring under test here, called in the production
    /// order but without `build`'s settlement leg -- which dials a chain,
    /// and this is a question about config reaching `ClaimBook`, not about
    /// chain behaviour. `peer_channels_reach_the_claim_ledger` covers the
    /// `build`-calls-`wire_peer_channels` link on the EVM side.
    fn wire(config: &Config) -> Connector {
        let connector = Connector::new(
            config.routes().to_vec(),
            Vec::new(),
            Arc::new(connector_runtime::FakeAppClient::new()),
            Arc::new(connector_runtime::InProcessPeerTransport::new()),
            Arc::new(SystemClock),
        );
        wire_peer_channels(connector, config).expect("wire [[peer_channels]]")
    }

    /// The Solana twin of [`peer_channels_reach_the_claim_ledger`] (issue
    /// #998): before this, `wire_peer_channels` skipped every
    /// `PeerChannelConfig::Solana` row outright, so a Solana-settled
    /// peering loaded and validated but could never verify an inbound
    /// claim -- `recognizes_solana_channel` is the observable end of that
    /// wiring, the Solana counterpart of `recognizes_channel`.
    #[test]
    fn solana_peer_channels_reach_the_claim_ledger() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let (config, _key_path) = solana_peering_config(state_dir.path(), SOLANA_PEER_PROGRAM_ID);

        let connector = wire(&config);

        assert!(
            connector.recognizes_solana_channel(SOLANA_PEER_CHANNEL_ACCOUNT),
            "the [[peer_channels]] Solana row must reach ClaimBook's Solana verification key, \
             or every peer claim on it is refused `unknown_channel` however correctly it was \
             signed"
        );
        assert!(
            !connector
                // A real 32-byte account (the system program's), so what
                // this asserts is "nobody configured it", not "it was never
                // a well-formed account in the first place".
                .recognizes_solana_channel("11111111111111111111111111111111"),
            "and only that row -- an account nobody configured is still unknown"
        );
    }

    /// Issue #1128, at the end of the wire: the program `ClaimBook` judges
    /// a peer claim under is the program `[settlement.solana]` names, and
    /// there is no longer a per-row value that could name a different one.
    /// A peer claim signed under any other program does not verify here --
    /// which is the whole point, because any other program is one this node
    /// could never redeem the claim through.
    #[test]
    fn a_solana_peer_channel_is_judged_under_the_settlement_program() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let other_program = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
        let (config, _key_path) = solana_peering_config(state_dir.path(), other_program);

        let PeerChannelConfig::Solana(row) = &config.peer_channels()[0] else {
            panic!("expected a Solana peer channel");
        };
        assert_eq!(
            row.program_id(),
            other_program,
            "moving [settlement.solana] program_id moves the peer channel's with it -- the two \
             cannot be made to disagree, which is what stops a node accepting peer claims it \
             settles under a different program and can never redeem"
        );

        let connector = wire(&config);
        assert!(connector.recognizes_solana_channel(SOLANA_PEER_CHANNEL_ACCOUNT));
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
        let extra = &terms["accepts"][0]["extra"];
        assert_eq!(extra["ilpAddresses"], serde_json::json!(["g.toon.apex"]));
        assert_eq!(extra["btpEndpoint"], "wss://apex.example/ilp/btp");
    }

    /// A structurally-valid EVM claim naming a channel this node has no
    /// record of -- built once and reused both plaintext and wrapped, so
    /// the only difference between the two requests below is the header
    /// carrying it.
    fn undeclared_channel_claim_json() -> String {
        format!(
            r#"{{
                "version": "1.0",
                "blockchain": "evm",
                "messageId": "msg-1",
                "timestamp": "2026-02-02T12:00:00.000Z",
                "senderId": "peer-bob",
                "channelId": "0x{channel}",
                "nonce": 1,
                "transferredAmount": "0",
                "lockedAmount": "0",
                "locksRoot": "0x{zeros}",
                "signature": "0xabcdef",
                "signerAddress": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1"
            }}"#,
            channel = "ab".repeat(32),
            zeros = "0".repeat(64),
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
    /// `UnknownChannel` rejection a plaintext claim for the same channel
    /// gets -- see the next test.
    #[tokio::test]
    async fn router_unwraps_a_claim_wrapped_to_the_signer_key_and_runs_the_identical_gate() {
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
            wrapped_reject.message.contains("no record of"),
            "expected an UnknownChannel rejection, got {wrapped_reject:?}"
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
    /// malformed wrap (`Malformed`) and from the plaintext `UnknownChannel`
    /// rejection the previous test established.
    #[tokio::test]
    async fn router_refuses_a_wrap_addressed_to_a_different_receiver_distinguishably() {
        use libsecp256k1::{PublicKey, SecretKey};

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
            // The gate's own wording, not a copy of it: a reworded
            // `UnknownChannel` must not quietly make this assertion vacuous.
            connector_client_edge::ClaimIngestRejection::UnknownChannel.message(),
            "a wrap addressed to the wrong receiver must fail to unwrap, not fall through to an \
             UnknownChannel rejection"
        );
    }

    /// The one narrowing in this crate that guards a false-accept boundary
    /// (issue #646): an on-chain deposit is a `uint256` and a claim's
    /// cumulative amount is a `u64`, so the deposit has to be narrowed to
    /// be compared -- and narrowing the wrong way round would turn a huge
    /// deposit into a tiny cap (refusing good claims) or, far worse, a
    /// wrapped small number into a huge one. `ethers`' own `U256::as_u64`
    /// panics above the range; this clamps, which is sound *only* because
    /// the value is an upper bound on a `u64`: no `u64` can exceed
    /// `u64::MAX`, so a deposit wider than that and a deposit of exactly
    /// `u64::MAX` cap identically.
    #[test]
    fn a_deposit_wider_than_u64_clamps_rather_than_wrapping_or_panicking() {
        assert_eq!(saturating_u64(U256::zero()), 0);
        assert_eq!(saturating_u64(U256::from(1_000u64)), 1_000);
        assert_eq!(saturating_u64(U256::from(u64::MAX)), u64::MAX);
        assert_eq!(
            saturating_u64(U256::from(u64::MAX) + U256::one()),
            u64::MAX,
            "one past the boundary clamps rather than wrapping to 0"
        );
        assert_eq!(
            saturating_u64(U256::MAX),
            u64::MAX,
            "a deposit no u64 claim could ever exceed caps at the largest one that could"
        );
    }

    /// A node with no `[[client_channels]]` has a record of no channel, so
    /// its client edge refuses every claim rather than trusting a claim's
    /// own declared signer (issue #558).
    #[test]
    fn a_node_configuring_no_client_channels_has_a_record_of_none() {
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

        assert!(client_channels(&config, None, None, None).is_empty());
    }

    /// The liveness knobs reach the registry rather than stopping at the
    /// config struct (issue #649, and the availability review of #654):
    /// an operator who widens the re-attempt floor because their RPC
    /// endpoint is rate-limited has to actually get a widened floor.
    ///
    /// Asserted through behaviour rather than a getter, since the registry
    /// deliberately exposes none: with the interval set to ten minutes, a
    /// second lookup on the same channel inside that window must not reach
    /// the source at all.
    #[tokio::test]
    async fn the_configured_liveness_knobs_reach_the_registry() {
        use connector_client_edge::{ChannelLookupFailed, DepositFloor, EvmChannel};
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Debug, Default)]
        struct CountingSource {
            lookups: AtomicUsize,
        }

        #[async_trait]
        impl ClientChannelSource for CountingSource {
            async fn evm_channel(
                &self,
                _channel_id: &[u8; 32],
            ) -> Result<Option<EvmChannel>, ChannelLookupFailed> {
                self.lookups.fetch_add(1, Ordering::SeqCst);
                Ok(Some(EvmChannel {
                    counterparty: [0x11; 20],
                    chain_id: 8453,
                    token_network_address: [0x42; 20],
                    deposit_floor: DepositFloor::AtLeast(1_000),
                }))
            }
        }

        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
channel_liveness_ttl_secs = 1
channel_serve_stale_secs = 600
channel_reattempt_interval_ms = 600000

[signer]
key_file = "{}"
"#,
                key_path.display()
            )
        });

        let source = Arc::new(CountingSource::default());
        let channels = client_channels(&config, Some(source.clone()), None, None);
        let gate = ClientClaimGate::restore(channels, Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay");

        // Two claims on the same channel, either side of the one-second
        // ttl. Both are refused for their signature -- what matters is how
        // many times the source was consulted.
        let claim = |nonce: u64| {
            format!(
                r#"{{
                    "version": "1.0",
                    "blockchain": "evm",
                    "messageId": "msg-{nonce}",
                    "timestamp": "2026-02-02T12:00:00.000Z",
                    "senderId": "peer-bob",
                    "channelId": "0x{id}",
                    "nonce": {nonce},
                    "transferredAmount": "10",
                    "lockedAmount": "0",
                    "locksRoot": "0x{zeros}",
                    "signature": "0x{sig}",
                    "signerAddress": "0x1111111111111111111111111111111111111111"
                }}"#,
                id = "ab".repeat(32),
                zeros = "0".repeat(64),
                sig = "cd".repeat(65),
            )
        };
        let _ = gate.ingest(&claim(1), 0).await;
        tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
        let _ = gate.ingest(&claim(2), 0).await;

        assert_eq!(
            source.lookups.load(Ordering::SeqCst),
            1,
            "the entry aged out, but the configured ten-minute re-attempt floor still binds"
        );
    }

    /// The unresolvable-lookup budget's knobs reach the registry too
    /// (issue #613). Asserted through behaviour for the same reason as
    /// above: with a node-wide allowance of two, a sender walking channel
    /// ids reaches the source twice and no more, however many claims they
    /// present.
    #[tokio::test]
    async fn the_configured_lookup_budget_reaches_the_registry() {
        use connector_client_edge::{ChannelLookupFailed, EvmChannel};
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// A chain that knows about no channel at all -- which is what a
        /// walk of the id space looks like from the connector's side.
        #[derive(Debug, Default)]
        struct EmptyCountingSource {
            lookups: AtomicUsize,
        }

        #[async_trait]
        impl ClientChannelSource for EmptyCountingSource {
            async fn evm_channel(
                &self,
                _channel_id: &[u8; 32],
            ) -> Result<Option<EvmChannel>, ChannelLookupFailed> {
                self.lookups.fetch_add(1, Ordering::SeqCst);
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

        let source = Arc::new(EmptyCountingSource::default());
        let channels = client_channels(&config, Some(source.clone()), None, None);
        let gate = ClientClaimGate::restore(channels, Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay");

        // A fresh channel id per claim, which is the whole shape of the
        // attack: nothing this connector has ever seen, and nothing it ever
        // will resolve.
        let claim = |nonce: u64| {
            format!(
                r#"{{
                    "version": "1.0",
                    "blockchain": "evm",
                    "messageId": "msg-{nonce}",
                    "timestamp": "2026-02-02T12:00:00.000Z",
                    "senderId": "peer-bob",
                    "channelId": "0x{nonce:064x}",
                    "nonce": {nonce},
                    "transferredAmount": "10",
                    "lockedAmount": "0",
                    "locksRoot": "0x{zeros}",
                    "signature": "0x{sig}",
                    "signerAddress": "0x1111111111111111111111111111111111111111"
                }}"#,
                zeros = "0".repeat(64),
                sig = "cd".repeat(65),
            )
        };
        let mut budgeted = 0;
        for nonce in 1..=20 {
            if matches!(
                gate.ingest(&claim(nonce), 0).await,
                Err(connector_client_edge::ClaimIngestRejection::LookupBudgetExhausted { .. })
            ) {
                budgeted += 1;
            }
        }

        assert_eq!(
            source.lookups.load(Ordering::SeqCst),
            2,
            "twenty claims on twenty channels cost the configured allowance and no more"
        );
        assert_eq!(
            budgeted, 18,
            "and every claim past it says why it was refused"
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

    /// Every configured channel reaches the client edge's registry, so a
    /// claim on it is verified against the counterparty the operator
    /// declared -- and a claim on any other channel is not (issue #558).
    #[test]
    fn every_configured_client_channel_is_recorded() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"

[[client_channels]]
channel_id = "0x{channel}"
counterparty = "0x00000000000000000000000000000000000000aa"
chain_id = 8453
token_network_address = "0x00000000000000000000000000000000000000bb"
{settlement}"#,
                key_path = key_path.display(),
                state_dir = state_dir.path().display(),
                channel = "ab".repeat(32),
                settlement = evm_settlement(key_path),
            )
        });

        let channels = client_channels(&config, None, None, None);
        assert!(!channels.is_empty());
        let ClientChannelConfig::Evm(evm) = &config.client_channels()[0] else {
            panic!("expected an EVM client channel");
        };
        assert_eq!(evm.chain_id(), 8453);
        assert_eq!(evm.counterparty()[19], 0xaa);
    }

    /// The Solana twin of the above (issue #630): a declared Solana channel
    /// reaches the client edge's registry the same way a declared EVM one
    /// does, through [`ClientChannelRegistry::record_solana`] -- under the
    /// program `[settlement.solana]` names, which is the one this node
    /// could redeem the claim through (ADR 0053, issues #1082 and #1138).
    ///
    /// This test used to assert the opposite half: that a Solana row on a
    /// node with **no** `[settlement.solana]` was *not* recorded, because
    /// this arm warn-and-skipped it. It is refused at load now
    /// (`ClientChannelWithoutSolanaSettlement`), so the skip has no
    /// reachable state left to test and the refusal is
    /// `connector-config`'s.
    #[test]
    fn every_configured_solana_client_channel_is_recorded() {
        let state_dir = tempfile::tempdir().expect("temp state dir");
        let account = "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi";
        let counterparty = "8pM1DN3RiT8vbom5u1sNryaNT1nyL8CTTW3b5PwWXRBH";
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"

[[client_channels]]
channel_account = "{account}"
counterparty = "{counterparty}"

[settlement.solana]
rpc_url = "https://api.devnet.solana.com"
program_id = "{SOLANA_PEER_PROGRAM_ID}"
token_address = "{counterparty}"
decimals = 6

[settlement.solana.key]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
                state_dir = state_dir.path().display(),
            )
        });

        let channels = client_channels(&config, None, None, None);
        assert!(
            !channels.is_empty(),
            "a declared Solana client channel must reach the registry, or every claim on it is \
             refused as an unknown channel"
        );
        let ClientChannelConfig::Solana(solana) = &config.client_channels()[0] else {
            panic!("expected a Solana client channel");
        };
        assert_eq!(solana.channel_account(), account);
        assert_eq!(solana.counterparty(), counterparty);
        assert_eq!(
            solana.program_id(),
            SOLANA_PEER_PROGRAM_ID,
            "the row is judged under the program this node settles with, never a second \
             declaration of one"
        );
    }

    /// Issue #1131: the same defect as the test below, on the node shape
    /// `cluster_hint` cannot see -- one behind a paid RPC provider, whose
    /// URL names no cluster at all. Before this, `cluster_hint` answered
    /// `None` for such a URL, the registry recorded no cluster, and the
    /// #975 check silently did nothing; the claim below was accepted in
    /// full. Now the cluster comes from the chain's own genesis hash, read
    /// at connect, so the check runs however the node reached the chain.
    ///
    /// The Helius-shaped URL is the point of the test and must stay
    /// unrecognised by `cluster_hint`: if it ever became a recognised host,
    /// this test would pass through the fallback and stop covering the
    /// genesis path -- so it asserts the hint is `None` first.
    ///
    /// `Some("devnet")` stands in for what
    /// `SolanaSettlementBackend::connect` reads off the chain; that read
    /// itself is proved against a real validator in
    /// `connector-settlement-solana`'s `connect_identity` suite, which is
    /// where a chain fact belongs (ADR 0007, tier 3).
    #[tokio::test]
    async fn a_node_behind_a_paid_rpc_provider_still_checks_the_declared_cluster() {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;
        use ed25519_dalek::Signer as Ed25519Signer;
        use rand::SeedableRng;

        const PROGRAM_ID: [u8; 32] = [0xab; 32];
        const CHANNEL_ACCOUNT: [u8; 32] = [4u8; 32];
        const PAID_PROVIDER_RPC_URL: &str = "https://mainnet.helius-rpc.com/?api-key=redacted";

        let payer =
            ed25519_dalek::Keypair::generate(&mut rand::rngs::StdRng::from_seed([17u8; 32]));
        let program_id = Pubkey::new_from_array(PROGRAM_ID).to_string();
        let channel_account = Pubkey::new_from_array(CHANNEL_ACCOUNT).to_string();
        let counterparty = Pubkey::new_from_array(payer.public.to_bytes()).to_string();

        let state_dir = tempfile::tempdir().expect("temp state dir");
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"

[settlement.solana]
rpc_url = "{PAID_PROVIDER_RPC_URL}"
program_id = "{program_id}"
token_address = "So11111111111111111111111111111111111111112"
decimals = 6

[settlement.solana.key]
key_file = "{key_path}"

[[client_channels]]
channel_account = "{channel_account}"
counterparty = "{counterparty}"
"#,
                key_path = key_path.display(),
                state_dir = state_dir.path().display(),
            )
        });

        let connector_config::SettlementConfig::Solana(solana) = &config.settlements()[0] else {
            panic!("expected a Solana settlement table");
        };
        assert_eq!(
            solana.cluster_hint(),
            None,
            "the whole premise of this test is a URL whose hostname names no cluster"
        );

        // What the chain answered at connect. The URL says "mainnet" and
        // the chain says devnet -- deliberately, so a test that passed by
        // reading the URL's own substring would fail here.
        let channels = client_channels(&config, None, None, Some("devnet"));
        let gate = ClientClaimGate::restore(channels, Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay");

        let message =
            connector_signer::solana_balance_proof_message(&PROGRAM_ID, &CHANNEL_ACCOUNT, 1, 100);
        let signature = BASE64.encode(payer.sign(&message).to_bytes());
        let signed = |cluster: &str| {
            format!(
                r#"{{
                    "version": "1.0",
                    "blockchain": "solana",
                    "messageId": "msg-1",
                    "timestamp": "2026-02-02T12:00:00.000Z",
                    "senderId": "peer-carol",
                    "programId": "{program_id}",
                    "channelAccount": "{channel_account}",
                    "nonce": 1,
                    "transferredAmount": "100",
                    "signature": "{signature}",
                    "signerPublicKey": "{counterparty}",
                    "cluster": "{cluster}"
                }}"#
            )
        };

        assert_eq!(
            gate.ingest(&signed("mainnet-beta"), 0).await,
            Err(
                connector_client_edge::ClaimIngestRejection::SolanaClusterMismatch {
                    declared: "mainnet-beta".to_string(),
                    configured: "devnet",
                }
            ),
            "a node whose chain reports devnet must not endorse a claim labelled mainnet-beta, \
             even though its rpc_url's hostname names no cluster"
        );
        assert!(
            gate.ingest(&signed("devnet"), 0).await.is_ok(),
            "a correctly labelled claim on the same channel and nonce must still be accepted"
        );
    }

    /// Issue #975's wiring, end to end from a config file: the cluster a
    /// node settles on is read out of `[settlement.solana] rpc_url` and
    /// reaches the claim gate, so a genuinely signed claim wearing another
    /// chain's label is refused rather than endorsed.
    ///
    /// This is the issue's own live repro with the polarity reversed --
    /// a devnet node told it is on mainnet, rather than a mainnet node told
    /// it is on devnet. The check is symmetric, and this direction keeps a
    /// mainnet RPC URL out of a committed config file (ADR 0056: production
    /// is named and empty).
    ///
    /// Needs no validator: the channel is declared in the config, and the
    /// cluster comparison happens before any chain read.
    #[tokio::test]
    async fn a_claim_wearing_another_clusters_label_is_refused_from_a_real_config() {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;
        use ed25519_dalek::Signer as Ed25519Signer;
        use rand::SeedableRng;

        const PROGRAM_ID: [u8; 32] = [0xab; 32];
        const CHANNEL_ACCOUNT: [u8; 32] = [3u8; 32];

        let payer =
            ed25519_dalek::Keypair::generate(&mut rand::rngs::StdRng::from_seed([13u8; 32]));
        let program_id = Pubkey::new_from_array(PROGRAM_ID).to_string();
        let channel_account = Pubkey::new_from_array(CHANNEL_ACCOUNT).to_string();
        let counterparty = Pubkey::new_from_array(payer.public.to_bytes()).to_string();

        let state_dir = tempfile::tempdir().expect("temp state dir");
        let (config, _key_path) = config_with_raw_key_file(|key_path| {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"

[settlement.solana]
rpc_url = "https://api.devnet.solana.com"
program_id = "{program_id}"
token_address = "So11111111111111111111111111111111111111112"
decimals = 6

[settlement.solana.key]
key_file = "{key_path}"

[[client_channels]]
channel_account = "{channel_account}"
counterparty = "{counterparty}"
"#,
                key_path = key_path.display(),
                state_dir = state_dir.path().display(),
            )
        });

        let channels = client_channels(&config, None, None, None);
        let gate = ClientClaimGate::restore(channels, Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay");

        // Genuinely signed, under the program this node really runs, by the
        // counterparty the config really declares. Nothing about this claim
        // is forged -- only its label is wrong, which is the whole of the
        // defect.
        let signed = |cluster: &str| {
            let message = connector_signer::solana_balance_proof_message(
                &PROGRAM_ID,
                &CHANNEL_ACCOUNT,
                1,
                100,
            );
            let signature = BASE64.encode(payer.sign(&message).to_bytes());
            format!(
                r#"{{
                    "version": "1.0",
                    "blockchain": "solana",
                    "messageId": "msg-1",
                    "timestamp": "2026-02-02T12:00:00.000Z",
                    "senderId": "peer-carol",
                    "programId": "{program_id}",
                    "channelAccount": "{channel_account}",
                    "nonce": 1,
                    "transferredAmount": "100",
                    "signature": "{signature}",
                    "signerPublicKey": "{counterparty}",
                    "cluster": "{cluster}"
                }}"#
            )
        };

        assert_eq!(
            gate.ingest(&signed("mainnet-beta"), 0).await,
            Err(
                connector_client_edge::ClaimIngestRejection::SolanaClusterMismatch {
                    declared: "mainnet-beta".to_string(),
                    configured: "devnet",
                }
            ),
            "a node whose rpc_url names devnet must not endorse a claim labelled mainnet-beta"
        );

        // And the same claim, correctly labelled, still pays -- so what the
        // config wired up is a comparison, not a blanket refusal.
        assert!(
            gate.ingest(&signed("devnet"), 0).await.is_ok(),
            "a correctly labelled claim on the same channel and nonce must still be accepted"
        );
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

        client_claim_gate(&config, test_signer(), None, None, None)
            .expect("a writable state_dir produces a gate");
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

        let Err(error) = client_claim_gate(&config, test_signer(), None, None, None) else {
            panic!("an unusable state_dir must not produce a gate");
        };
        assert!(matches!(error, RuntimeError::StateDirUnusable { .. }));
        let message = error.to_string();
        assert!(
            message.contains(&state_dir.display().to_string()),
            "the failure must name the path an operator has to fix: {message}"
        );
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

        let Err(error) = client_claim_gate(&config, test_signer(), None, None, None) else {
            panic!("a corrupt journal must not produce a gate");
        };
        assert!(matches!(error, RuntimeError::JournalUnreplayable { .. }));
    }

    /// The peer semantics's own journal is armed off the same `state_dir`
    /// (issue #605, #556's "Journal" row): one answer for both surfaces,
    /// not a fix for the client edge and the same bug left standing on the
    /// wire between connectors.
    #[tokio::test]
    async fn a_configured_state_dir_also_arms_the_peer_claim_journal() {
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
        assert!(state_dir.path().join(PEER_CLAIM_JOURNAL).exists());
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

    /// Issue #1136: a declared EIP-712 domain (ADR 0024) is held against
    /// the `TokenNetwork` this node actually redeems through, and a node
    /// whose file and chain disagree refuses to start.
    ///
    /// The subject really is chain behaviour, so the refusals are driven
    /// against a real `anvil` with two real registry deployments rather
    /// than a hand-built domain: the whole point is that
    /// `[settlement.evm]` names a `TokenNetworkRegistry` and the verifying
    /// contract is whatever `getTokenNetwork(token_address)` answers, which
    /// only a chain can say. The two offline tests below cover the two
    /// questions that are *not* about a chain -- which rows are compared at
    /// all.
    mod evm_channel_domains {
        use super::*;

        use connector_settlement_evm::test_support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};

        /// Shares [`super::settlement_construction`]'s base: `Anvil::spawn`
        /// adds a process-global atomic offset, so two modules in one test
        /// binary asking for the same base still get different ports.
        const ANVIL_BASE_PORT: u16 = 18_700;

        const PEER_CHANNEL: &str =
            "0x1111111111111111111111111111111111111111111111111111111111111111";
        const PAY_CHANNEL: &str =
            "0x2222222222222222222222222222222222222222222222222222222222222222";
        const CLIENT_CHANNEL: &str =
            "0x3333333333333333333333333333333333333333333333333333333333333333";
        const COUNTERPARTY: &str = "0x4444444444444444444444444444444444444444";

        /// A key file holding `contents`, deleted when the returned handle
        /// drops. A local twin of `settlement_construction`'s own, which is
        /// private to that module.
        fn key_file_with(contents: &str) -> tempfile::TempPath {
            let mut file = tempfile::NamedTempFile::new().expect("temp key file");
            file.write_all(contents.as_bytes()).expect("write key file");
            file.into_temp_path()
        }

        /// A `[settlement.evm]` table naming `registry`/`token`, plus one
        /// row in each of the three tables that declares a domain. Each
        /// table's domain is given separately so exactly one can be made
        /// stale at a time.
        #[allow(clippy::too_many_arguments)]
        fn config_text(
            key_path: &Path,
            state_dir: &Path,
            rpc_url: &str,
            registry: ethers::types::Address,
            token: ethers::types::Address,
            peer_domain: (u64, ethers::types::Address),
            pay_domain: (u64, ethers::types::Address),
            client_domain: (u64, ethers::types::Address),
        ) -> String {
            format!(
                r#"
client_edge_addr = "127.0.0.1:0"
peer_expose = "btp"
peer_allow_plaintext_endpoints = true
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"

[settlement.evm]
rpc_url = "{rpc_url}"
contract_address = "{registry:?}"
token_address = "{token:?}"
decimals = 6

[settlement.evm.key]
key_file = "{key_path}"

[[peers]]
id = "store"
endpoint = "wss://store.example:443/btp"

[[peer_channels]]
peer_id = "store"
channel_id = "{PEER_CHANNEL}"
counterparty_key = "{COUNTERPARTY}"
chain_id = {peer_chain}
token_network = "{peer_network:?}"

[[pay_channels]]
peer_id = "store"
channel_id = "{PAY_CHANNEL}"
chain_id = {pay_chain}
token_network = "{pay_network:?}"
client_edge_url = "http://127.0.0.1:1/ilp"

[[client_channels]]
channel_id = "{CLIENT_CHANNEL}"
counterparty = "{COUNTERPARTY}"
chain_id = {client_chain}
token_network_address = "{client_network:?}"
"#,
                state_dir = state_dir.display(),
                key_path = key_path.display(),
                peer_chain = peer_domain.0,
                peer_network = peer_domain.1,
                pay_chain = pay_domain.0,
                pay_network = pay_domain.1,
                client_chain = client_domain.0,
                client_network = client_domain.1,
            )
        }

        /// A config with every table declaring the deployment's own domain
        /// boots, and each of the three tables in turn refuses to when its
        /// row names a `TokenNetwork` -- or a chain id -- the chain does not
        /// agree with.
        ///
        /// One `anvil`, two real `TokenNetworkRegistry` deployments: the
        /// second is what a redeploy leaves behind, and its `TokenNetwork`
        /// is a real contract at a real address that simply is not the one
        /// `[settlement.evm]` resolves. That is exactly the row an operator
        /// is left holding, and it is why this cannot be checked without a
        /// chain: nothing in the file names either `TokenNetwork`.
        #[tokio::test]
        async fn a_declared_domain_that_is_not_the_deployments_refuses_to_start() {
            if !require_anvil() {
                return;
            }
            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let token =
                EvmSettlementBackend::deploy_mock_token(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, 1)
                    .await
                    .expect("deploy mock USDC");
            let deployed =
                EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
                    .await
                    .expect("deploy a TokenNetwork through a fresh registry");
            let registry = deployed.registry_address();
            let live_domain = (deployed.chain_id(), deployed.address());

            // The deployment the stale row is left pointing at: a second,
            // genuinely deployed `TokenNetwork` for a second token.
            let other_token =
                EvmSettlementBackend::deploy_mock_token(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, 1)
                    .await
                    .expect("deploy a second mock token");
            let superseded =
                EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, other_token)
                    .await
                    .expect("deploy a second TokenNetwork through its own registry");
            let stale_domain = (superseded.chain_id(), superseded.address());
            assert_ne!(
                live_domain.1, stale_domain.1,
                "two registries must resolve two different TokenNetworks, or this test asserts \
                 nothing"
            );
            drop(deployed);
            drop(superseded);

            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let state_dir = tempfile::tempdir().expect("temp state dir");
            let load = |peer, pay, client| {
                load_config(&config_text(
                    &key_path,
                    state_dir.path(),
                    &anvil.rpc_url,
                    registry,
                    token,
                    peer,
                    pay,
                    client,
                ))
            };

            // The control, first: every row agreeing with the chain boots.
            // Without it a broken comparison that refused everything would
            // pass every case below.
            let agreeing = load(live_domain, live_domain, live_domain);
            build(&agreeing)
                .await
                .expect("a config whose declared domains are the deployment's own must boot");

            let peer_stale = load(stale_domain, live_domain, live_domain);
            let error = build(&peer_stale)
                .await
                .err()
                .expect("a [[peer_channels]] row naming another TokenNetwork must refuse to boot");
            let RuntimeError::PeerChannelDomainDisagreesWithSettlement(mismatch) = &error else {
                panic!("expected the peer-channel refusal, got: {error}");
            };
            assert_eq!(mismatch.channel_id, PEER_CHANNEL);
            assert_eq!(
                mismatch.declared.token_network,
                stale_domain.1.to_fixed_bytes()
            );
            assert_eq!(
                mismatch.settled.token_network,
                live_domain.1.to_fixed_bytes()
            );
            // The message names BOTH contracts: an operator holding a stale
            // row cannot fix it from a refusal that names only one.
            let said = error.to_string();
            assert!(
                said.contains(&format!("{:#x}", stale_domain.1))
                    && said.contains(&format!("{:#x}", live_domain.1)),
                "{said}"
            );

            let pay_stale = load(live_domain, stale_domain, live_domain);
            let error = build(&pay_stale)
                .await
                .err()
                .expect("a [[pay_channels]] row naming another TokenNetwork must refuse to boot");
            let RuntimeError::PayChannelDomainDisagreesWithSettlement(mismatch) = &error else {
                panic!("expected the pay-channel refusal, got: {error}");
            };
            assert_eq!(mismatch.channel_id, PAY_CHANNEL);

            let client_stale = load(live_domain, live_domain, stale_domain);
            let error = build(&client_stale).await.err().expect(
                "a [[client_channels]] row naming another TokenNetwork must refuse to boot",
            );
            let RuntimeError::ClientChannelDomainDisagreesWithSettlement(mismatch) = &error else {
                panic!("expected the client-channel refusal, got: {error}");
            };
            assert_eq!(mismatch.channel_id, CLIENT_CHANNEL);

            // The other half of the domain. `chain_id` is the easier one --
            // the backend has always known its live chain id -- and it was
            // just as uncompared.
            let wrong_chain = load((live_domain.0 + 1, live_domain.1), live_domain, live_domain);
            let error = build(&wrong_chain)
                .await
                .err()
                .expect("a row naming another chain id must refuse to boot too");
            let RuntimeError::PeerChannelDomainDisagreesWithSettlement(mismatch) = &error else {
                panic!("expected the peer-channel refusal, got: {error}");
            };
            assert_eq!(mismatch.declared.chain_id, live_domain.0 + 1);
            assert_eq!(mismatch.settled.chain_id, live_domain.0);
        }

        /// A Solana `[[peer_channels]]` row carries no EVM domain to
        /// disagree with -- its signed message is the channel account and
        /// the settlement program (ADR 0053, #1134) -- so a node holding
        /// one alongside an EVM settlement table is untouched by this
        /// check. No chain needed: the question is which rows are compared,
        /// not what the chain says.
        #[test]
        fn a_solana_peer_channel_is_not_held_against_an_evm_domain() {
            let key_file =
                key_file_with("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
            let state_dir = tempfile::tempdir().expect("temp state dir");
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"
peer_expose = "btp"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[settlement.solana]
rpc_url = "http://127.0.0.1:8899"
program_id = "2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip"
token_address = "34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU"
decimals = 6

[settlement.solana.key]
key_file = "{key_file}"

[[peers]]
id = "store"
endpoint = "wss://store.example:443/btp"

[[peer_channels]]
peer_id = "store"
channel_account = "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi"
counterparty_key = "8pM1DN3RiT8vbom5u1sNryaNT1nyL8CTTW3b5PwWXRBH"
"#,
                state_dir = state_dir.path().display(),
                key_file = key_file.display(),
            ));

            check_evm_channel_domains(
                &config,
                EvmDomain {
                    chain_id: 8453,
                    token_network: [0xab; 20],
                },
            )
            .expect("a Solana row has no EVM domain to disagree with");
        }

        /// And the empty case, so the check cannot be the reason a node
        /// with no channel tables at all stops booting.
        #[test]
        fn a_node_that_declares_no_channel_at_all_is_unaffected() {
            let key_file =
                key_file_with("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_file}"
"#,
                key_file = key_file.display(),
            ));

            check_evm_channel_domains(
                &config,
                EvmDomain {
                    chain_id: 8453,
                    token_network: [0xab; 20],
                },
            )
            .expect("nothing declared, nothing to disagree");
        }
    }

    /// **Which socket the claim-state ask left on** (ADR 0070 decision 3).
    ///
    /// A covering payer asks the receiver where its claims stand on every
    /// PREPARE it covers, so this dial is as much the ILP wire as the
    /// carriage that carries the packet afterwards -- and a hop reachable
    /// only over a circuit is not payable unless it goes the same way. No
    /// configuration value can answer "where did that connection go", so the
    /// assertion is made against a real SOCKS5 server on loopback, exactly
    /// as the two carriage crates make theirs. Nothing here reaches a
    /// third-party network and there is no daemon: a `.onion` name resolves
    /// nowhere, which is why finding it in the proxy's own record proves
    /// both that the dial traversed the proxy and that it deferred
    /// resolution to it.
    mod claim_state_dial {
        use super::*;

        use connector_runtime::Socks5TestServer;

        /// A v3 onion address's shape -- 56 base32 characters -- so the host
        /// under test is one an operator could have copied out of a
        /// `HiddenServiceDir/hostname` (ADR 0070 decision 7). No such
        /// service exists; nothing here expects one to.
        const ONION_HOST: &str = "toonexampleconnectoraddress234567abcdefghijklmnopqrstuvw.onion";

        fn timeout() -> std::time::Duration {
            std::time::Duration::from_secs(5)
        }

        #[tokio::test]
        async fn an_onion_client_edge_is_asked_through_the_proxy() {
            let proxy = Socks5TestServer::spawn_recording_only().await;
            let url = url::Url::parse(&format!("http://{ONION_HOST}/ilp")).expect("onion url");

            let client = claim_state_client(&url, Some(&proxy.proxy_url()), timeout())
                .expect("a socks5h proxy builds a client");
            let refused = client.post(url.as_str()).send().await;

            assert!(
                refused.is_err(),
                "the proxy routes nothing, so the ask cannot succeed: {refused:?}"
            );
            assert_eq!(
                proxy.targets(),
                vec![format!("{ONION_HOST}:80")],
                "the claim-state ask reached the proxy, and reached it as a NAME -- no resolver \
                 here can resolve a .onion, so a client that had tried to resolve it locally \
                 would never have got this far"
            );
        }

        /// The other half, and the one that makes the rule a SELECTION
        /// rather than a mode: the same node, the same proxy, a clearnet
        /// hop, and the proxy sees nothing.
        #[tokio::test]
        async fn a_clearnet_client_edge_is_asked_direct() {
            let proxy = Socks5TestServer::spawn_recording_only().await;
            // Bound and then dropped: the port is closed, so the direct dial
            // fails fast. What is under test is where it went, and a refusal
            // from loopback is evidence it went to loopback.
            let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            let addr = closed.local_addr().expect("addr");
            drop(closed);
            let url = url::Url::parse(&format!("http://{addr}/ilp")).expect("clearnet url");

            let client = claim_state_client(&url, Some(&proxy.proxy_url()), timeout())
                .expect("a socks5h proxy builds a client");
            let _ = client.post(url.as_str()).send().await;

            assert!(
                proxy.targets().is_empty(),
                "a clearnet client edge must be dialed direct even on a node that configured a \
                 proxy: one onion peering must not reroute the hops this node already pays, {:?}",
                proxy.targets()
            );
        }

        /// A node with no proxy still builds a client for an onion hop, and
        /// the covering claim then fails at the dial. Not a config refusal:
        /// `socks_proxy` is optional by design, and an unreachable hop is an
        /// unreachable hop.
        #[test]
        fn an_onion_client_edge_without_a_proxy_still_builds() {
            let url = url::Url::parse(&format!("http://{ONION_HOST}/ilp")).expect("onion url");

            assert!(claim_state_client(&url, None, timeout()).is_ok());
        }
    }

    /// `[[pay_channels]]` reaching `Connector::with_outbound_client_hop`
    /// (ADR 0042 item 2, issue #881) -- the wiring whose absence meant no
    /// production path ever covered a forward.
    ///
    /// Nothing here is mocked (ADR 0007). The receiver is a real HTTP
    /// server answering a real `POST /ilp/claim-state`, the claims are
    /// signed by a real `LocalSigner` from the config's own
    /// `[settlement.evm]` key file, and every signature these tests assert
    /// on is verified with `connector_signer`'s own recovery -- so a claim
    /// signed under the wrong domain, or against the wrong channel, fails
    /// them. The peer transport is a fake in the ADR 0007 sense: a real
    /// implementation of the port that records what it was handed.
    mod outbound_client_hops {
        use super::*;

        use std::sync::Mutex;

        use axum::extract::State;
        use axum::routing::post;
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;
        use connector_domain::{Fulfill, PacketResponse, Prepare};
        use connector_runtime::{
            ClaimAckOutcome, ClaimSignature, FakeAppClient, PeerForward, PeerTransport, WireClaim,
        };
        use connector_signer::{
            derive_evm_address, verify_evm_balance_proof, verify_evm_claim_state_challenge,
            verify_solana_balance_proof, verify_solana_claim_state_challenge, EvmBalanceProof,
            EvmClaimStateChallenge,
        };

        /// The peering, its peer-role channel, and the channel this node
        /// PAYS it from -- three distinct strings, so no assertion below
        /// can pass by conflating two of them.
        const PEER_ID: &str = "store";
        const PEER_CHANNEL: &str =
            "0xaaaabbbbccccddddeeeeffff00001111aaaabbbbccccddddeeeeffff00001111";
        const PAY_CHANNEL: &str =
            "0xccccddddeeeeffff00001111222233334444555566667777888899990000aaaa";
        const COUNTERPARTY: &str = "0x2222222222222222222222222222222222222222";
        const TOKEN_NETWORK: &str = "0x3333333333333333333333333333333333333333";
        const CHAIN_ID: u64 = 31_337;

        /// The route this node forwards over, and the fee it keeps.
        const ROUTE_PREFIX: &str = "g.example.store";
        const DESTINATION: &str = "g.example.store.app";
        const PEER_FEE: u64 = 3;
        const CLIENT_AMOUNT: u64 = 1_000;
        /// What actually goes over the peering, and therefore exactly what
        /// a covering claim must be worth: ADR 0042 covers the packet's own
        /// forwarded value, not the amount the client declared.
        const FORWARDED: u64 = CLIENT_AMOUNT - PEER_FEE;

        /// Where the receiver says this node's claims on [`PAY_CHANNEL`]
        /// stand. Non-zero on both axes so an assertion could not pass
        /// against a defaulted or forgotten watermark.
        const RECEIVER_NONCE: u64 = 7;
        const RECEIVER_CUMULATIVE: u64 = 7_000;

        /// `0x`-prefixed (or bare) hex as bytes. A local four-liner rather
        /// than a dependency: this is the only place in the crate that
        /// reads hex back off a wire.
        fn decode_hex(value: &str) -> Vec<u8> {
            let hex = value.trim_start_matches("0x");
            (0..hex.len() / 2)
                .map(|index| {
                    u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).expect("hex digits")
                })
                .collect()
        }

        fn channel_bytes(id: &str) -> [u8; 32] {
            decode_hex(id).try_into().expect("a 32-byte channel id")
        }

        fn token_network_bytes() -> [u8; 20] {
            decode_hex(TOKEN_NETWORK)
                .try_into()
                .expect("a 20-byte address")
        }

        /// A real next hop's claim-state endpoint: it answers
        /// `POST /ilp/claim-state` with a watermark, and keeps every ask so
        /// a test can check both that it was asked and what it was asked
        /// with.
        struct ClaimStateEdge {
            /// The `POST /ilp` URL an operator writes as
            /// `client_edge_url` -- `claim-state` hangs off it.
            url: String,
            asks: Arc<Mutex<Vec<serde_json::Value>>>,
        }

        async fn spawn_claim_state_edge() -> ClaimStateEdge {
            #[derive(Clone)]
            struct EdgeState {
                asks: Arc<Mutex<Vec<serde_json::Value>>>,
            }

            async fn claim_state(
                State(state): State<EdgeState>,
                axum::Json(body): axum::Json<serde_json::Value>,
            ) -> axum::Json<serde_json::Value> {
                state
                    .asks
                    .lock()
                    .expect("claim-state asks lock poisoned")
                    .push(body);
                axum::Json(serde_json::json!({
                    "channels": [{
                        "ok": true,
                        "nonce": RECEIVER_NONCE,
                        "cumulativeClaimed": RECEIVER_CUMULATIVE.to_string(),
                        "available": "1000000",
                    }]
                }))
            }

            let asks = Arc::new(Mutex::new(Vec::new()));
            let router = axum::Router::new()
                .route("/ilp/claim-state", post(claim_state))
                .with_state(EdgeState { asks: asks.clone() });
            let listener =
                std::net::TcpListener::bind("127.0.0.1:0").expect("bind the claim-state edge");
            let addr = listener.local_addr().expect("claim-state edge addr");
            tokio::spawn(async move {
                axum::Server::from_tcp(listener)
                    .expect("axum server from tcp listener")
                    .serve(router.into_make_service())
                    .await
                    .expect("claim-state edge server");
            });

            ClaimStateEdge {
                url: format!("http://{addr}/ilp"),
                asks,
            }
        }

        /// A real [`PeerTransport`] that fulfils every forward and keeps
        /// what rode with it -- so the forward genuinely fulfils and the
        /// postpay path's `record_fulfillment` genuinely runs. Its
        /// fulfilment rides home unchecked (issue #1269 / ADR 0069), so it
        /// need not derive from anything [`peer_bound_prepare`] sets.
        struct RecordingPeerTransport {
            fulfillment: [u8; 32],
            forwards: Mutex<Vec<(String, Prepare, Option<WireClaim>)>>,
        }

        impl RecordingPeerTransport {
            fn new() -> Arc<RecordingPeerTransport> {
                Arc::new(RecordingPeerTransport {
                    fulfillment: [42u8; 32],
                    forwards: Mutex::new(Vec::new()),
                })
            }

            fn claims(&self) -> Vec<Option<WireClaim>> {
                self.forwards
                    .lock()
                    .expect("forwards lock poisoned")
                    .iter()
                    .map(|(_, _, claim)| claim.clone())
                    .collect()
            }

            fn forwarded_amounts(&self) -> Vec<u64> {
                self.forwards
                    .lock()
                    .expect("forwards lock poisoned")
                    .iter()
                    .map(|(_, prepare, _)| prepare.amount)
                    .collect()
            }
        }

        #[async_trait]
        impl PeerTransport for RecordingPeerTransport {
            async fn forward(
                &self,
                peer_id: &str,
                prepare: Prepare,
                claim: Option<WireClaim>,
            ) -> PeerForward {
                self.forwards.lock().expect("forwards lock poisoned").push((
                    peer_id.to_string(),
                    prepare,
                    claim,
                ));
                PeerForward::answered(
                    PacketResponse::Fulfill(Fulfill {
                        fulfillment: self.fulfillment,
                        data: Vec::new(),
                    }),
                    ClaimAckOutcome::NotSent,
                )
            }

            async fn flush(&self, _peer_id: &str, _claim: WireClaim) -> ClaimAckOutcome {
                ClaimAckOutcome::NotSent
            }
        }

        fn peer_bound_prepare() -> Prepare {
            Prepare {
                amount: CLIENT_AMOUNT,
                expires_at: chrono::Utc::now() + chrono::Duration::seconds(30),
                greeting: false,
                destination: DESTINATION.to_string(),
                data: b"a forwarded packet's payload".to_vec(),
            }
        }

        /// Everything a test needs to keep alive: the temp files the config
        /// still points at, and the state directory the ledger is written
        /// into.
        struct Fixture {
            config: Config,
            state_dir: tempfile::TempDir,
            _key_path: tempfile::TempPath,
        }

        /// A node peered with [`PEER_ID`] and routing [`ROUTE_PREFIX`] to
        /// it, with the `[settlement.evm]` key a claim is signed with --
        /// plus the `[[pay_channels]]` row, unless `pay_channel` is empty.
        /// Returns a `Result` rather than a `Config`, because since issue
        /// #1145 the interesting case is the one that does not load: a
        /// peering this node routes to and has no `[[pay_channels]]` row
        /// for is refused by name.
        fn fixture(pay_channel: &str) -> Result<Fixture, connector_config::ConfigError> {
            let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
            key_file
                .write_all(&[7u8; 32])
                .expect("write raw 32-byte key");
            let key_path = key_file.into_temp_path();
            let state_dir = tempfile::tempdir().expect("temp state dir");
            let config = try_load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"
peer_expose = "btp"
peer_allow_plaintext_endpoints = true
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6

[settlement.evm.key]
key_file = "{key_file}"

[[peers]]
id = "{PEER_ID}"
endpoint = "wss://store.example:443/btp"
fee = {PEER_FEE}

[[peer_channels]]
peer_id = "{PEER_ID}"
channel_id = "{PEER_CHANNEL}"
counterparty_key = "{COUNTERPARTY}"
chain_id = {CHAIN_ID}
token_network = "{TOKEN_NETWORK}"

[[routes]]
prefix = "{ROUTE_PREFIX}"
peer_id = "{PEER_ID}"
price = {CLIENT_AMOUNT}
{pay_channel}
"#,
                state_dir = state_dir.path().display(),
                key_file = key_path.display(),
            ))?;
            Ok(Fixture {
                config,
                state_dir,
                _key_path: key_path,
            })
        }

        /// The row under test, pointed at a real claim-state edge.
        fn pay_channel_row(edge: &ClaimStateEdge) -> String {
            format!(
                r#"
[[pay_channels]]
peer_id = "{PEER_ID}"
channel_id = "{PAY_CHANNEL}"
chain_id = {CHAIN_ID}
token_network = "{TOKEN_NETWORK}"
client_edge_url = "{url}"
"#,
                url = edge.url,
            )
        }

        /// The connector `build` would produce for this config, with the
        /// recording transport in place of the dial side -- every other
        /// piece is the production wiring, called in the production order.
        fn connector(fixture: &Fixture, peer: Arc<RecordingPeerTransport>) -> Connector {
            let config = &fixture.config;
            let peer_routes = config
                .peer_routes()
                .iter()
                .map(|route| {
                    PeerRoute::new_scheduled(route.prefix(), route.peer_id(), route.price())
                })
                .collect();
            let (claim_signer, _) = peer_claim_identity(config)
                .expect("read the settlement key")
                .expect("the fixture configures [settlement.evm]");
            let mut connector = Connector::new(
                config.routes().to_vec(),
                peer_routes,
                Arc::new(FakeAppClient::new()),
                peer,
                Arc::new(SystemClock),
            )
            .with_peer_fees(
                config
                    .peers()
                    .iter()
                    .map(|peer| (peer.id().to_string(), peer.fee())),
            )
            .with_signer(Arc::clone(&claim_signer));
            connector = wire_peer_channels(connector, config).expect("wire [[peer_channels]]");
            // The fixture is EVM-only, so there is no Solana settlement key
            // to hand over -- a Solana `[[pay_channels]]` row on such a
            // node would not have loaded in the first place
            // (`PayChannelWithoutSolanaSettlement`).
            wire_outbound_client_hops(connector, config, Some(claim_signer), None)
                .expect("wire [[pay_channels]]")
        }

        /// The address every claim in these tests must recover to: the
        /// `[settlement.evm]` key's own, because the channel's on-chain
        /// participant IS this node's settlement address (ADR 0030 -- "no
        /// second key is introduced").
        fn settlement_address(fixture: &Fixture) -> [u8; 20] {
            let (signer, address) = peer_claim_identity(&fixture.config)
                .expect("read the settlement key")
                .expect("the fixture configures [settlement.evm]");
            assert_eq!(
                address,
                derive_evm_address(&signer.public_key().expect("public key")),
                "the identity used to sign and the address asserted against are one key"
            );
            address
        }

        /// **The wiring, end to end.** A peering with a `[[pay_channels]]`
        /// row covers its forward from the first attempt: the claim rides
        /// the outgoing PREPARE, it is worth exactly what this packet
        /// forwards, its nonce advances the RECEIVER's watermark (asked
        /// over a real `POST /ilp/claim-state`, never guessed), and its
        /// signature verifies against this node's settlement address under
        /// the configured channel and domain.
        #[tokio::test]
        async fn a_configured_pay_channel_covers_the_forward_and_the_claim_rides_the_prepare() {
            let edge = spawn_claim_state_edge().await;
            let fixture = fixture(&pay_channel_row(&edge)).expect("the config loads");
            let peer = RecordingPeerTransport::new();
            let connector = connector(&fixture, Arc::clone(&peer));

            let response = connector.handle_prepare(peer_bound_prepare()).await;

            assert!(
                matches!(response, PacketResponse::Fulfill(_)),
                "the covered forward fulfils: {response:?}"
            );
            assert_eq!(peer.forwarded_amounts(), vec![FORWARDED]);
            let claim = peer.claims()[0]
                .clone()
                .expect("a configured hop covers its forward -- no claim means #881 is unwired");
            assert_eq!(
                claim.channel_id, PAY_CHANNEL,
                "the covering claim names the channel this node PAYS from, not the one it \
                 judges peer claims against"
            );
            assert_eq!(
                claim.nonce,
                RECEIVER_NONCE + 1,
                "the nonce advances the receiver's own watermark"
            );
            assert_eq!(
                claim.cumulative_amount,
                RECEIVER_CUMULATIVE + FORWARDED,
                "the claim covers exactly this packet's forwarded value"
            );

            let ClaimSignature::Evm(signature) = claim.signature else {
                panic!("an EVM channel's covering claim is an EIP-712 balance proof");
            };
            assert!(
                verify_evm_balance_proof(
                    &EvmBalanceProof {
                        channel_id: channel_bytes(PAY_CHANNEL),
                        nonce: RECEIVER_NONCE + 1,
                        transferred_amount: u128::from(RECEIVER_CUMULATIVE + FORWARDED),
                        locked_amount: 0,
                        locks_root: [0u8; 32],
                        chain_id: CHAIN_ID,
                        token_network_address: token_network_bytes(),
                    },
                    &signature.to_bytes(),
                    &settlement_address(&fixture),
                ),
                "the claim must recover to this node's settlement address under the CONFIGURED \
                 chain id and TokenNetwork -- a claim signed under any other domain is refused \
                 at the far gate with the packet already paid for"
            );

            // The receiver was asked, and asked about this channel, with a
            // challenge signed by the same key: the watermark authority is
            // the receiver (issue #693), never anything remembered here.
            let asks = edge.asks.lock().expect("asks lock poisoned").clone();
            assert_eq!(asks.len(), 1, "one covered packet, one watermark ask");
            let asked = &asks[0]["channels"][0];
            assert_eq!(asked["channelId"], PAY_CHANNEL);
            let expires = asked["expires"].as_u64().expect("an expiry");
            let signature = asked["signature"].as_str().expect("a challenge signature");
            let signature = decode_hex(signature);
            assert!(
                verify_evm_claim_state_challenge(
                    &EvmClaimStateChallenge {
                        channel_id: channel_bytes(PAY_CHANNEL),
                        expires,
                        chain_id: CHAIN_ID,
                        token_network_address: token_network_bytes(),
                    },
                    &signature,
                    &settlement_address(&fixture),
                ),
                "the claim-state ask proves control of the channel's on-chain participant"
            );

            // And the nonce floor is durable, under the same `state_dir`
            // the journals are, in its own file: a restart that reissued a
            // nonce would fork this node's own outbound line.
            let ledger = fixture.state_dir.path().join(OUTBOUND_CLIENT_LEDGER);
            assert!(
                ledger.is_file(),
                "the outbound client ledger is file-backed"
            );
            let written = std::fs::read_to_string(&ledger).expect("read the ledger");
            assert!(
                written.contains(&format!("\"nonce\":{}", RECEIVER_NONCE + 1)),
                "the issued nonce reaches the disk before the claim exists: {written}"
            );
        }

        /// **The row is not optional any more** (issue #1145). ADR 0042
        /// item 2 shipped `[[pay_channels]]` as additive -- "a peering with
        /// nothing configured behaves exactly as it does now", which meant
        /// falling through to ADR 0004's postpay convention: the first
        /// forward carried no claim and the second carried the one the
        /// first fulfilment armed on the peer ledger.
        ///
        /// That convention is deleted, so this node's own config is now the
        /// thing that refuses. The same file, unchanged except for the
        /// missing row, does not load at all -- and the refusal names the
        /// peer and the route that forwards to it, because "which peering
        /// do I add a row for" is the only question an operator holding
        /// this error has.
        ///
        /// A load-time refusal rather than a packet-time one is the whole
        /// point (ADR 0009): without it this config would boot cleanly and
        /// then reject every packet on that route with `T00`.
        #[test]
        fn a_peering_this_node_forwards_to_with_no_pay_channels_row_is_refused_at_load() {
            let message = match fixture("") {
                Err(error) => error.to_string(),
                Ok(_) => panic!("a routed peering with no channel to pay it from must not load"),
            };

            assert!(
                message.contains(PEER_ID) && message.contains(ROUTE_PREFIX),
                "the refusal must name the peer and the route: {message}"
            );
            assert!(
                message.contains("[[pay_channels]]"),
                "and the table to add: {message}"
            );
            assert!(
                message.contains("breaking deploy"),
                "a newly REQUIRED key is a breaking deploy (ADR 0009), and the operator \
                 reading this is the one who has to order it: {message}"
            );
        }

        // -------------------------------------------------------------
        // Solana (issue #1146). Until this landed a Solana peering had no
        // `[[pay_channels]]` shape to configure, no arm in
        // `OutboundClientLedger::next_claim` to sign with, and no
        // challenge signer to ask its next hop's watermark with -- so it
        // could only ever be paid POSTPAY, the model ADR 0042 exists to
        // retire.
        // -------------------------------------------------------------

        /// `local/mixed-chain`'s own b-c channel account and program id, so
        /// nothing here reads as a placeholder.
        const SOLANA_CHANNEL_ACCOUNT: &str = "G5mXQzfZb4tXWX7cQvXP9ZJnDBcUo6irWTmGGtX3xpzL";
        const SOLANA_COUNTERPARTY: &str = "93mxPHokL6EhVxzVicSyouEhvTVGUcKXKtHH1uTmw2Aw";
        const SOLANA_PROGRAM_ID: &str = "HY4AYFNe5Vg5BkEwAURNsGY3uFAvGMNpAQPRtgoasJiR";

        fn base58_bytes(value: &str) -> [u8; 32] {
            bs58::decode(value)
                .into_vec()
                .expect("base58")
                .try_into()
                .expect("32 bytes")
        }

        /// [`fixture`]'s Solana twin: the same peering and route, settling
        /// on Solana instead of anvil, with a `[[pay_channels]]` row on the
        /// same channel account the `[[peer_channels]]` row binds -- the
        /// deployed shape, and on Solana a load-time requirement.
        fn solana_fixture(client_edge_url: &str) -> Fixture {
            let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
            key_file
                .write_all(&[9u8; 32])
                .expect("write raw 32-byte key");
            let key_path = key_file.into_temp_path();
            let state_dir = tempfile::tempdir().expect("temp state dir");
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"
peer_expose = "btp"
peer_allow_plaintext_endpoints = true
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[settlement.solana]
rpc_url = "http://127.0.0.1:8899"
program_id = "{SOLANA_PROGRAM_ID}"
token_address = "{SOLANA_COUNTERPARTY}"
decimals = 6

[settlement.solana.key]
key_file = "{key_file}"

[[peers]]
id = "{PEER_ID}"
endpoint = "wss://store.example:443/btp"
fee = {PEER_FEE}

[[peer_channels]]
peer_id = "{PEER_ID}"
channel_account = "{SOLANA_CHANNEL_ACCOUNT}"
counterparty_key = "{SOLANA_COUNTERPARTY}"

[[routes]]
prefix = "{ROUTE_PREFIX}"
peer_id = "{PEER_ID}"
price = {CLIENT_AMOUNT}

[[pay_channels]]
peer_id = "{PEER_ID}"
channel_account = "{SOLANA_CHANNEL_ACCOUNT}"
client_edge_url = "{client_edge_url}"
"#,
                state_dir = state_dir.path().display(),
                key_file = key_path.display(),
            ));
            Fixture {
                config,
                state_dir,
                _key_path: key_path,
            }
        }

        /// [`connector`]'s Solana twin -- the production wiring, called in
        /// the production order, with the ed25519 settlement key in place
        /// of the secp256k1 one.
        fn solana_connector(fixture: &Fixture, peer: Arc<RecordingPeerTransport>) -> Connector {
            let config = &fixture.config;
            let peer_routes = config
                .peer_routes()
                .iter()
                .map(|route| {
                    PeerRoute::new_scheduled(route.prefix(), route.peer_id(), route.price())
                })
                .collect();
            let claim_signer = peer_claim_identity_solana(config)
                .expect("read the settlement key")
                .expect("the fixture configures [settlement.solana]");
            let mut connector = Connector::new(
                config.routes().to_vec(),
                peer_routes,
                Arc::new(FakeAppClient::new()),
                peer,
                Arc::new(SystemClock),
            )
            .with_peer_fees(
                config
                    .peers()
                    .iter()
                    .map(|peer| (peer.id().to_string(), peer.fee())),
            )
            .with_solana_signer(Arc::clone(&claim_signer));
            connector = wire_peer_channels(connector, config).expect("wire [[peer_channels]]");
            wire_outbound_client_hops(connector, config, None, Some(claim_signer))
                .expect("wire [[pay_channels]]")
        }

        /// **The Solana wiring, end to end** -- the twin of
        /// [`a_configured_pay_channel_covers_the_forward_and_the_claim_rides_the_prepare`].
        ///
        /// A Solana peering with a `[[pay_channels]]` row covers its
        /// forward from the first attempt: the claim rides the outgoing
        /// PREPARE, it is an ed25519 balance proof worth exactly what this
        /// packet forwards, its nonce advances the receiver's own watermark
        /// (asked over a real `POST /ilp/claim-state`), and the ask itself
        /// carries the Solana challenge -- `channelAccount` in base58 and a
        /// base64 ed25519 signature -- that
        /// `verify_solana_claim_state_challenge` accepts. That last check is
        /// the one that says the ask matches the verifier that already
        /// existed, rather than being a second, plausible-looking design.
        #[tokio::test]
        async fn a_configured_solana_pay_channel_covers_the_forward_with_an_ed25519_claim() {
            let edge = spawn_claim_state_edge().await;
            let fixture = solana_fixture(&edge.url);
            let peer = RecordingPeerTransport::new();
            let connector = solana_connector(&fixture, Arc::clone(&peer));
            let public_key = peer_claim_identity_solana(&fixture.config)
                .expect("read the settlement key")
                .expect("the fixture configures [settlement.solana]")
                .public_key();

            let response = connector.handle_prepare(peer_bound_prepare()).await;

            assert!(
                matches!(response, PacketResponse::Fulfill(_)),
                "the covered forward fulfils: {response:?}"
            );
            assert_eq!(peer.forwarded_amounts(), vec![FORWARDED]);
            let claim = peer.claims()[0]
                .clone()
                .expect("a configured Solana hop covers its forward");
            assert_eq!(
                claim.channel_id, SOLANA_CHANNEL_ACCOUNT,
                "the covering claim names the channel account in base58, the spelling the \
                 Solana claim namespace uses"
            );
            assert_eq!(claim.nonce, RECEIVER_NONCE + 1);
            assert_eq!(claim.cumulative_amount, RECEIVER_CUMULATIVE + FORWARDED);

            let ClaimSignature::Solana(signature) = claim.signature else {
                panic!("a Solana channel's covering claim is an ed25519 balance proof");
            };
            assert!(
                verify_solana_balance_proof(
                    &base58_bytes(SOLANA_PROGRAM_ID),
                    &base58_bytes(SOLANA_CHANNEL_ACCOUNT),
                    RECEIVER_NONCE + 1,
                    RECEIVER_CUMULATIVE + FORWARDED,
                    &signature,
                    &public_key,
                ),
                "the claim must verify against this node's own Solana settlement key under the \
                 program `[settlement.solana]` names -- ADR 0053 signs that program id into the \
                 message, so a claim minted under any other is refused at the far gate with the \
                 packet already paid for"
            );

            // The ask: one, about this account, with a challenge the far
            // side's own verifier accepts.
            let asks = edge.asks.lock().expect("asks lock poisoned").clone();
            assert_eq!(asks.len(), 1, "one covered packet, one watermark ask");
            let asked = &asks[0]["channels"][0];
            assert_eq!(asked["blockchain"], "solana");
            assert_eq!(asked["channelAccount"], SOLANA_CHANNEL_ACCOUNT);
            assert!(
                asked.get("channelId").is_none(),
                "an EVM-shaped ask about a Solana channel is answered `unverified`: {asked}"
            );
            let expires = asked["expires"].as_u64().expect("an expiry");
            let challenge_signature = BASE64
                .decode(asked["signature"].as_str().expect("a challenge signature"))
                .expect("the challenge signature is base64, as the far side decodes it");
            assert!(
                verify_solana_claim_state_challenge(
                    &base58_bytes(SOLANA_CHANNEL_ACCOUNT),
                    expires,
                    &challenge_signature,
                    &public_key,
                ),
                "the claim-state ask proves control of the channel's on-chain participant"
            );

            // And the nonce floor is durable, exactly as on the EVM leg:
            // one ledger, one file, whichever chain the hop settles on.
            let ledger = fixture.state_dir.path().join(OUTBOUND_CLIENT_LEDGER);
            let written = std::fs::read_to_string(&ledger).expect("read the ledger");
            assert!(
                written.contains(&format!("\"nonce\":{}", RECEIVER_NONCE + 1)),
                "the issued nonce reaches the disk before the claim exists: {written}"
            );
        }

        /// Issue #1217: the whole production build chain, not just the
        /// wiring function -- a config with no `[[pay_channels]]` table
        /// still opens a real, `state_dir`-backed outbound client ledger,
        /// because a runtime peering established later over `POST /peers`
        /// (ADR 0058) needs one to ever be payable, and nothing rebuilds
        /// this node's wiring when that happens.
        #[tokio::test]
        async fn build_writes_an_outbound_client_ledger_even_for_a_node_with_no_pay_channels() {
            let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
            key_file
                .write_all(&[7u8; 32])
                .expect("write raw 32-byte key");
            let key_path = key_file.into_temp_path();
            let state_dir = tempfile::tempdir().expect("temp state dir");
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"
"#,
                state_dir = state_dir.path().display(),
                key_file = key_path.display(),
            ));

            let runtime = build(&config).await.expect("build");

            // Not file existence: `OutboundClientLedger::open` writes nothing
            // until a claim is actually issued ("a missing file is an empty
            // ledger, not an error"), so an empty file would prove nothing
            // either way. `outbound_client_ledger()` is the direct answer to
            // "is one wired at all" -- `None` is exactly what let issue
            // #1217's peering accept a claim and sign none.
            assert!(
                runtime.connector.outbound_client_ledger().is_some(),
                "a node with a state_dir must have an outbound client ledger wired, whether or \
                 not it has a `[[pay_channels]]` row -- a runtime peering (ADR 0058) may still \
                 need it"
            );
        }
    }

    mod settlement_construction {
        use super::*;
        use chrono::Duration;
        use connector_settlement_evm::test_support::{
            anvil_available, require_anvil, Anvil, DEPLOYER_PRIVATE_KEY,
        };
        use connector_settlement_evm::{ChannelIndexEvent, OrderedChannelIndexEvent};
        use connector_settlement_solana::test_support::{
            fund, require_solana_test_validator, SolanaValidator, LOCAL_TEST_PROGRAM_ID,
        };
        use connector_settlement_solana::SolanaSettlementBackend;
        use ethers::signers::Signer as EvmSigner;
        use solana_rpc_client::nonblocking::rpc_client::RpcClient;
        use solana_sdk::commitment_config::CommitmentConfig;
        use solana_sdk::signature::{Keypair, Signer as SolanaSigner};

        /// This test binary's own base port for [`Anvil::spawn`] -- distinct
        /// from other test binaries' bases (`connector-settlement-evm`'s own
        /// tests use 18_600; `connector-bin`'s use 18_500;
        /// `connector-cli`'s own `settlement_lifecycle` integration test
        /// uses 18_800) so that binaries running concurrently under `cargo
        /// test --workspace` don't contend for the same port range.
        const ANVIL_BASE_PORT: u16 = 18_700;

        fn key_file_with(contents: &str) -> tempfile::TempPath {
            let mut file = tempfile::NamedTempFile::new().expect("temp key file");
            file.write_all(contents.as_bytes()).expect("write key file");
            file.into_temp_path()
        }

        /// A `[settlement.solana]` (or `[settlement.solana.key]`) key file
        /// carrying `seed` as 32 raw bytes -- the ed25519 seed
        /// [`SolanaSettlementBackend::connect`] signs with, the Solana
        /// twin of [`key_file_with`]'s hex-encoded secp256k1 key.
        fn raw_key_file(seed: [u8; 32]) -> tempfile::TempPath {
            let mut file = tempfile::NamedTempFile::new().expect("temp key file");
            file.write_all(&seed).expect("write raw key file");
            file.into_temp_path()
        }

        /// AC: "`connector-cli::runtime::build` constructs the configured
        /// backend and passes it to `Connector::with_settlement`, so a node
        /// with settlement configured never answers `NoSettlementBackend`" --
        /// driven against a real, disposable `anvil` chain end to end:
        /// `build` reads the `[settlement]` section from a config file (no
        /// backend injected directly), and the resulting `Connector` opens a
        /// real channel against a real, freshly deployed `TokenNetwork`,
        /// resolved through a freshly deployed registry.
        #[tokio::test]
        async fn a_configured_settlement_section_is_constructed_and_attached() {
            if !anvil_available() {
                eprintln!(
                    "skipping: `anvil` not found on PATH (install via https://getfoundry.sh)"
                );
                return;
            }

            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let token = EvmSettlementBackend::deploy_mock_token(
                &anvil.rpc_url,
                DEPLOYER_PRIVATE_KEY,
                1_000_000,
            )
            .await
            .expect("deploy mock USDC");
            let settlement_backend =
                EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
                    .await
                    .expect("deploy a TokenNetwork through a fresh registry");
            let registry_address = settlement_backend.registry_address();
            drop(settlement_backend);

            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_path}"

[settlement]
chain = "evm"
rpc_url = "{rpc_url}"
contract_address = "{registry_address:?}"
token_address = "{token:?}"
decimals = 6

[settlement.key]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
                rpc_url = anvil.rpc_url,
                registry_address = registry_address,
                token = token,
            ));

            let runtime = build(&config).await.expect("build");
            let connector = runtime.connector.clone();
            // A real 20-byte EVM address (issue #576): `TokenNetwork`
            // requires a counterparty able to sign balance proofs, not an
            // arbitrary peer name.
            let counterparty =
                ethers::signers::LocalWallet::new(&mut ethers::core::rand::thread_rng())
                    .address()
                    .as_bytes()
                    .to_vec();
            let opened = connector
                .open_channel(None, counterparty, Duration::seconds(3600))
                .await
                .expect("a settlement backend was constructed and attached");
            assert_eq!(opened.deposited, 0);
        }

        /// The raw 32 bytes behind a `ChannelId`'s `0x`-prefixed 64-hex
        /// string -- what every on-chain-facing type in these tests keys a
        /// channel by.
        fn channel_id_bytes(id: &str) -> [u8; 32] {
            let hex_digits = id.trim_start_matches("0x");
            let mut out = [0u8; 32];
            for (i, byte) in out.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&hex_digits[i * 2..i * 2 + 2], 16)
                    .expect("channel id is 0x-prefixed 64-hex");
            }
            out
        }

        /// Issue #661's own acceptance criterion, proven at the seam
        /// `connector-cli` actually wires: "an EVM channel the index holds
        /// resolves with zero RPC calls on the packet path -- asserted by a
        /// test that counts provider calls, not by inspection". Rather than
        /// build a call-counting RPC proxy, this kills the fallback's own
        /// path to the chain outright (the anvil process backing it is
        /// dropped) after the index has been populated -- if
        /// `IndexedEvmChannelSource::evm_channel` ever consulted the
        /// fallback for this channel, the lookup would fail with a
        /// connection error instead of answering correctly.
        #[tokio::test]
        async fn an_index_resolved_channel_answers_correctly_with_the_chain_unreachable() {
            if !anvil_available() {
                eprintln!(
                    "skipping: `anvil` not found on PATH (install via https://getfoundry.sh)"
                );
                return;
            }

            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let token = EvmSettlementBackend::deploy_mock_token(
                &anvil.rpc_url,
                DEPLOYER_PRIVATE_KEY,
                1_000_000,
            )
            .await
            .expect("deploy mock USDC");
            let backend = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
                .await
                .expect("deploy a TokenNetwork through a fresh registry");

            let counterparty_address =
                ethers::signers::LocalWallet::new(&mut ethers::core::rand::thread_rng()).address();
            let channel = backend
                .open(
                    counterparty_address.as_bytes().to_vec(),
                    Duration::seconds(3601),
                )
                .await
                .expect("open a channel");
            backend
                .fund_counterparty(&channel, 750)
                .await
                .expect("fund the counterparty's side of the channel");

            let channel_id = channel_id_bytes(&channel.0);
            let index = Arc::new(
                EvmChannelIndex::open(None, test_indexed_contract(), 0)
                    .expect("open in-memory index"),
            );
            index
                .apply(
                    vec![
                        OrderedChannelIndexEvent {
                            block_number: 1,
                            log_index: 0,
                            event: ChannelIndexEvent::Opened {
                                channel_id,
                                participant1: backend.own_address(),
                                participant2: counterparty_address,
                            },
                        },
                        OrderedChannelIndexEvent {
                            block_number: 2,
                            log_index: 0,
                            event: ChannelIndexEvent::NewDeposit {
                                channel_id,
                                participant: counterparty_address,
                                total_deposit: ethers::types::U256::from(750u64),
                            },
                        },
                    ],
                    2,
                )
                .expect("apply");

            let source = IndexedEvmChannelSource {
                index,
                fallback: SettlementChannelSource {
                    backend: Arc::new(backend),
                },
            };

            // Kill the chain the fallback would otherwise read from -- a
            // subsequent `eth_call` through it fails with a connection
            // error, so a wrong answer here (or an error) proves the
            // index was bypassed.
            drop(anvil);

            let resolved = source
                .evm_channel(&channel_id)
                .await
                .expect("the index answered without touching the (now-dead) chain")
                .expect("the channel is active in the index");
            assert_eq!(resolved.counterparty, counterparty_address.to_fixed_bytes());
            assert_eq!(resolved.deposit_floor, DepositFloor::AtLeast(750));
        }

        /// The follow-up finding on issue #661's PR: an indexed deposit
        /// lags the chain by the confirmation depth, so a channel whose
        /// `ChannelOpened` is confirmed but whose `ChannelNewDeposit` is
        /// not answers `Active` with a floor of zero -- and a breach
        /// re-read served from the same index would refuse a claim the
        /// chain honours. `evm_channel_fresh` must bypass the index and
        /// read the chain, exactly as a node on `main` would have.
        #[tokio::test]
        async fn a_breach_re_read_bypasses_the_index_and_reads_the_chain() {
            if !anvil_available() {
                eprintln!(
                    "skipping: `anvil` not found on PATH (install via https://getfoundry.sh)"
                );
                return;
            }

            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let token = EvmSettlementBackend::deploy_mock_token(
                &anvil.rpc_url,
                DEPLOYER_PRIVATE_KEY,
                1_000_000,
            )
            .await
            .expect("deploy mock USDC");
            let backend = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
                .await
                .expect("deploy a TokenNetwork through a fresh registry");

            let counterparty_address =
                ethers::signers::LocalWallet::new(&mut ethers::core::rand::thread_rng()).address();
            let channel = backend
                .open(
                    counterparty_address.as_bytes().to_vec(),
                    Duration::seconds(3601),
                )
                .await
                .expect("open a channel");
            backend
                .fund_counterparty(&channel, 750)
                .await
                .expect("fund the counterparty's side of the channel");

            // The index has seen the open but not the deposit -- the
            // "confirmed `ChannelOpened`, unconfirmed `ChannelNewDeposit`"
            // window the finding describes.
            let channel_id = channel_id_bytes(&channel.0);
            let index = Arc::new(
                EvmChannelIndex::open(None, test_indexed_contract(), 0)
                    .expect("open in-memory index"),
            );
            index
                .apply(
                    vec![OrderedChannelIndexEvent {
                        block_number: 1,
                        log_index: 0,
                        event: ChannelIndexEvent::Opened {
                            channel_id,
                            participant1: backend.own_address(),
                            participant2: counterparty_address,
                        },
                    }],
                    1,
                )
                .expect("apply");

            let source = IndexedEvmChannelSource {
                index: index.clone(),
                fallback: SettlementChannelSource {
                    backend: Arc::new(backend),
                },
            };

            // The ordinary read used to answer from the index with a floor
            // of zero; since issue #1151 the index's zero is a cue to read
            // the chain, so this one finds the real 750 too. Kept here as
            // the assertion that the two reads now agree in this window --
            // the disagreement was the bug.
            let cached = source
                .evm_channel(&channel_id)
                .await
                .expect("index lookup")
                .expect("active in the index");
            assert_eq!(cached.deposit_floor, DepositFloor::AtLeast(750));

            // The breach read reaches the chain and finds the real 750.
            let fresh = source
                .evm_channel_fresh(&channel_id)
                .await
                .expect("chain lookup")
                .expect("active on chain");
            assert_eq!(fresh.deposit_floor, DepositFloor::AtLeast(750));

            // A terminal record, though, stays answered from the index
            // even on a breach: settlement is monotone and the index only
            // applies confirmed logs, so the chain could only repeat it.
            // Proven the same way as the lookup test above: with the chain
            // dead, an answer at all is proof the index answered.
            index
                .apply(
                    vec![OrderedChannelIndexEvent {
                        block_number: 2,
                        log_index: 0,
                        event: ChannelIndexEvent::Settled { channel_id },
                    }],
                    2,
                )
                .expect("apply the settlement");
            drop(anvil);
            assert_eq!(
                source
                    .evm_channel_fresh(&channel_id)
                    .await
                    .expect("the terminal answer needs no chain"),
                None
            );
        }

        /// Issue #1151, both halves of it in one test because either half
        /// alone is satisfiable by a wrong fix.
        ///
        /// Two channels, identical as far as this node's index is
        /// concerned: it has applied the `ChannelOpened` of each and the
        /// `ChannelNewDeposit` of neither. One is funded on chain, the other
        /// has never held anything. The index reports a deposit of zero for
        /// both, because that value is what it reports for every
        /// counterparty it has applied no deposit event for -- so the fix
        /// cannot be "believe the index" (the funded channel is then
        /// refused for headroom it has, which is the bug) and it cannot be
        /// "call the index's zero [`DepositFloor::Unknown`]" either, since
        /// `Unknown` covers every claim and the unfunded channel would
        /// become an unlimited line of credit this connector could never
        /// redeem. Only asking the chain separates them.
        #[tokio::test]
        async fn a_channel_funded_inside_the_index_window_is_spendable_and_an_unfunded_one_is_not()
        {
            if !require_anvil() {
                return;
            }

            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let token = EvmSettlementBackend::deploy_mock_token(
                &anvil.rpc_url,
                DEPLOYER_PRIVATE_KEY,
                1_000_000,
            )
            .await
            .expect("deploy mock USDC");
            let backend = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
                .await
                .expect("deploy a TokenNetwork through a fresh registry");

            let funded_counterparty =
                ethers::signers::LocalWallet::new(&mut ethers::core::rand::thread_rng()).address();
            let funded = backend
                .open(
                    funded_counterparty.as_bytes().to_vec(),
                    Duration::seconds(3601),
                )
                .await
                .expect("open the funded channel");
            backend
                .fund_counterparty(&funded, 750)
                .await
                .expect("real ERC-20 value moves on chain for the funded channel");

            let empty_counterparty =
                ethers::signers::LocalWallet::new(&mut ethers::core::rand::thread_rng()).address();
            // Opened and deliberately never funded: `openChannel` costs gas
            // and no tokens, and emits no `ChannelNewDeposit` at all, so
            // this is the shape an attacker gets for free.
            let empty = backend
                .open(
                    empty_counterparty.as_bytes().to_vec(),
                    Duration::seconds(3601),
                )
                .await
                .expect("open the never-funded channel");

            let funded_id = channel_id_bytes(&funded.0);
            let empty_id = channel_id_bytes(&empty.0);
            let index = Arc::new(
                EvmChannelIndex::open(None, test_indexed_contract(), 0)
                    .expect("open in-memory index"),
            );
            index
                .apply(
                    vec![
                        OrderedChannelIndexEvent {
                            block_number: 1,
                            log_index: 0,
                            event: ChannelIndexEvent::Opened {
                                channel_id: funded_id,
                                participant1: backend.own_address(),
                                participant2: funded_counterparty,
                            },
                        },
                        OrderedChannelIndexEvent {
                            block_number: 1,
                            log_index: 1,
                            event: ChannelIndexEvent::Opened {
                                channel_id: empty_id,
                                participant1: backend.own_address(),
                                participant2: empty_counterparty,
                            },
                        },
                    ],
                    1,
                )
                .expect("apply both opens and neither deposit");

            let source = IndexedEvmChannelSource {
                index,
                fallback: SettlementChannelSource {
                    backend: Arc::new(backend),
                },
            };

            let funded = source
                .evm_channel(&funded_id)
                .await
                .expect("the funded channel resolves")
                .expect("the funded channel is active");
            assert_eq!(
                funded.deposit_floor,
                DepositFloor::AtLeast(750),
                "the index's zero for a deposit it has not applied yet must not become the \
                 collateral ceiling -- the chain holds 750"
            );
            assert!(
                funded.deposit_floor.covers(500),
                "a claim well inside the real deposit is spendable"
            );

            let empty = source
                .evm_channel(&empty_id)
                .await
                .expect("the never-funded channel resolves")
                .expect("the never-funded channel is active");
            assert_eq!(
                empty.deposit_floor,
                DepositFloor::AtLeast(0),
                "a channel that genuinely holds nothing reports a floor of zero, never \
                 DepositFloor::Unknown -- Unknown exempts, and nothing here earns the exemption"
            );
            assert!(
                !empty.deposit_floor.covers(1),
                "a claim of one unit against a channel holding nothing is refused"
            );
        }

        /// Issue #632's EVM-only acceptance criterion: "EVM-only node:
        /// greeting unchanged apart from the additive one-entry list".
        ///
        /// Since ADR 0050 there is nothing left for the two to disagree
        /// about: the greeting's legacy singular `extra.settlement` object is
        /// `NodeFacts::evm_settlement()`, i.e. this list's own EVM entry, so
        /// what this asserts is that the derivation finds it.
        #[tokio::test]
        async fn an_evm_only_node_composes_the_legacy_terms_and_a_one_entry_settlements_list() {
            if !anvil_available() {
                eprintln!(
                    "skipping: `anvil` not found on PATH (install via https://getfoundry.sh)"
                );
                return;
            }

            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let token = EvmSettlementBackend::deploy_mock_token(
                &anvil.rpc_url,
                DEPLOYER_PRIVATE_KEY,
                1_000_000,
            )
            .await
            .expect("deploy mock USDC");
            let settlement_backend =
                EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
                    .await
                    .expect("deploy a TokenNetwork through a fresh registry");
            let registry_address = settlement_backend.registry_address();
            drop(settlement_backend);

            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_path}"

[settlement]
chain = "evm"
rpc_url = "{rpc_url}"
contract_address = "{registry_address:?}"
token_address = "{token:?}"
decimals = 6

[settlement.key]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
                rpc_url = anvil.rpc_url,
                registry_address = registry_address,
                token = token,
            ));

            let runtime = build(&config).await.expect("build");
            let facts = connector_domain::NodeFacts {
                settlements: runtime.settlements.clone(),
                ..Default::default()
            };
            let terms = facts
                .evm_settlement()
                .cloned()
                .expect("an EVM settlement section yields the legacy greeting object");
            assert_eq!(
                runtime.settlements,
                vec![connector_client_edge::X402ChainSettlementTerms::Evm(terms)],
                "an EVM-only node's settlements list is a one-entry list, and the legacy object is that entry"
            );
        }

        /// AC (issue #564): "`decimals` is honoured: ... startup compares
        /// it with the token contract's own `decimals()` and refuses to
        /// start when the two disagree, naming both". The mock USDC
        /// deployed below is 6-decimal, as every token in this fleet is
        /// (`docs/usdc-cross-chain-settlement.md`); a config file claiming
        /// `decimals = 18` against it must fail to build rather than load
        /// clean and settle at a scale nobody consults.
        #[tokio::test]
        async fn settlement_decimals_the_token_disagrees_with_refuses_to_build() {
            if !anvil_available() {
                eprintln!(
                    "skipping: `anvil` not found on PATH (install via https://getfoundry.sh)"
                );
                return;
            }

            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let token = EvmSettlementBackend::deploy_mock_token(
                &anvil.rpc_url,
                DEPLOYER_PRIVATE_KEY,
                1_000_000,
            )
            .await
            .expect("deploy mock USDC");
            let settlement_backend =
                EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
                    .await
                    .expect("deploy a TokenNetwork through a fresh registry");
            let registry_address = settlement_backend.registry_address();
            drop(settlement_backend);

            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_path}"

[settlement]
chain = "evm"
rpc_url = "{rpc_url}"
contract_address = "{registry_address:?}"
token_address = "{token:?}"
decimals = 18

[settlement.key]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
                rpc_url = anvil.rpc_url,
                registry_address = registry_address,
                token = token,
            ));

            let error = build(&config)
                .await
                .err()
                .expect("a decimals the token disagrees with refuses to build");
            let message = error.to_string();
            // Both values are named, so an operator reading the failure can
            // tell which side is wrong without opening a block explorer.
            assert!(
                message.contains("decimals is 18") && message.contains("decimals() = 6"),
                "the failure must name both the configured and the on-chain decimals: {message}"
            );
        }

        /// AC: "a node with no settlement section still starts and still
        /// serves, degrading exactly as an absent `[operator]` section
        /// does" -- no anvil needed here, since nothing should even try to
        /// connect to a chain.
        #[tokio::test]
        async fn no_settlement_section_still_builds_and_degrades_to_no_backend() {
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
            let result = runtime
                .connector
                .open_channel(
                    None,
                    b"no-settlement-peer".to_vec(),
                    Duration::seconds(3600),
                )
                .await;
            assert!(matches!(
                result,
                Err(connector_runtime::ChannelOperationError::NoSettlementBackend)
            ));
        }

        /// AC (issue #630): "Node with `[settlement.solana]` starts against
        /// the devnet validator" -- driven end to end through `build`
        /// reading a config file (no backend injected directly), the
        /// Solana twin of
        /// `a_configured_settlement_section_is_constructed_and_attached`
        /// above: a real, disposable `solana-test-validator` running the
        /// real `packages/solana-program` artifact, and the resulting
        /// `Connector` opens a real channel through it.
        #[tokio::test]
        async fn a_solana_only_settlement_section_is_constructed_and_attached() {
            if !require_solana_test_validator() {
                return;
            }

            let validator = SolanaValidator::spawn().await;
            let program_id =
                Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
            let deployed = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
                .await
                .expect("bind to the genesis-loaded payment-channel program");
            let token_mint = deployed.token_mint();
            drop(deployed);

            let seed = [11u8; 32];
            let payer =
                solana_sdk::signer::keypair::keypair_from_seed(&seed).expect("derive keypair");
            let rpc = RpcClient::new_with_commitment(
                validator.rpc_url.clone(),
                CommitmentConfig::confirmed(),
            );
            fund(&rpc, &payer.pubkey()).await;

            let key_path = raw_key_file(seed);
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_path}"

[settlement.solana]
rpc_url = "{rpc_url}"
program_id = "{program_id}"
token_address = "{token_mint}"
decimals = 6

[settlement.solana.key]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
                rpc_url = validator.rpc_url,
            ));

            let runtime = build(&config).await.expect("build");
            let connector = runtime.connector.clone();
            // A real 32-byte Solana pubkey (issue #567's `open` accepts
            // nothing else): a fresh identity this test holds no key for,
            // exactly as `a_configured_settlement_section_is_constructed_and_attached`
            // generates an arbitrary EVM counterparty above.
            let counterparty = Keypair::new().pubkey().to_bytes().to_vec();
            let opened = connector
                .open_channel(None, counterparty, Duration::seconds(3600))
                .await
                .expect("a solana settlement backend was constructed and attached");
            assert_eq!(opened.deposited, 0);
        }

        /// AC (issue #630): "... decimals/asset-scale mismatch refuses
        /// startup with a clear error" -- the Solana twin of
        /// `settlement_decimals_the_token_disagrees_with_refuses_to_build`
        /// above. `deploy` mints a fresh 6-decimal SPL mint; a config file
        /// claiming `decimals = 9` against it must fail to build rather
        /// than load clean and settle at a scale nobody consults.
        #[tokio::test]
        async fn solana_decimals_the_mint_disagrees_with_refuses_to_build() {
            if !require_solana_test_validator() {
                return;
            }

            let validator = SolanaValidator::spawn().await;
            let program_id =
                Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
            let deployed = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
                .await
                .expect("bind to the genesis-loaded payment-channel program");
            let token_mint = deployed.token_mint();
            drop(deployed);

            let seed = [12u8; 32];
            let payer =
                solana_sdk::signer::keypair::keypair_from_seed(&seed).expect("derive keypair");
            let rpc = RpcClient::new_with_commitment(
                validator.rpc_url.clone(),
                CommitmentConfig::confirmed(),
            );
            fund(&rpc, &payer.pubkey()).await;

            let key_path = raw_key_file(seed);
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_path}"

[settlement.solana]
rpc_url = "{rpc_url}"
program_id = "{program_id}"
token_address = "{token_mint}"
decimals = 9

[settlement.solana.key]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
                rpc_url = validator.rpc_url,
            ));

            let error = build(&config)
                .await
                .err()
                .expect("a decimals the mint disagrees with refuses to build");
            let message = error.to_string();
            assert!(
                message.contains("decimals is 9") && message.contains("decimals = 6"),
                "the failure must name both the configured and the on-chain decimals: {message}"
            );
        }

        /// A config naming both `[settlement.evm]` and `[settlement.solana]`
        /// constructs both real backends and the built `Connector` holds
        /// *both*, each reachable on its own chain (issue #630, and its
        /// review's merge blocker: `Connector`'s settlement slot was
        /// last-one-wins, so on exactly this config every operator channel
        /// op silently targeted Solana -- an EVM `open_channel` here
        /// answered "a packages/solana-program counterparty must be a
        /// 32-byte Solana pubkey, got 20 bytes"). Driven end to end
        /// through `build` reading a config file: an EVM operator op lands
        /// on the EVM backend (a real channel opens on the anvil chain), a
        /// Solana one on the Solana backend, and per-channel-id ops route
        /// each id to its own chain.
        #[tokio::test]
        async fn a_both_chains_config_attaches_and_routes_both_backends() {
            if !anvil_available() {
                eprintln!(
                    "skipping: `anvil` not found on PATH (install via https://getfoundry.sh)"
                );
                return;
            }
            if !require_solana_test_validator() {
                return;
            }

            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let token = EvmSettlementBackend::deploy_mock_token(
                &anvil.rpc_url,
                DEPLOYER_PRIVATE_KEY,
                1_000_000,
            )
            .await
            .expect("deploy mock USDC");
            let settlement_backend =
                EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
                    .await
                    .expect("deploy a TokenNetwork through a fresh registry");
            let registry_address = settlement_backend.registry_address();
            drop(settlement_backend);

            let validator = SolanaValidator::spawn().await;
            let program_id =
                Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
            let deployed = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
                .await
                .expect("bind to the genesis-loaded payment-channel program");
            let token_mint = deployed.token_mint();
            drop(deployed);

            let seed = [13u8; 32];
            let payer =
                solana_sdk::signer::keypair::keypair_from_seed(&seed).expect("derive keypair");
            let rpc = RpcClient::new_with_commitment(
                validator.rpc_url.clone(),
                CommitmentConfig::confirmed(),
            );
            fund(&rpc, &payer.pubkey()).await;

            let evm_key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let solana_key_path = raw_key_file(seed);
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{evm_key_path}"

[settlement.evm]
rpc_url = "{evm_rpc_url}"
contract_address = "{registry_address:?}"
token_address = "{token:?}"
decimals = 6

[settlement.evm.key]
key_file = "{evm_key_path}"

[settlement.solana]
rpc_url = "{solana_rpc_url}"
program_id = "{program_id}"
token_address = "{token_mint}"
decimals = 6

[settlement.solana.key]
key_file = "{solana_key_path}"
"#,
                evm_key_path = evm_key_path.display(),
                solana_key_path = solana_key_path.display(),
                evm_rpc_url = anvil.rpc_url,
                solana_rpc_url = validator.rpc_url,
                registry_address = registry_address,
                token = token,
            ));

            let runtime = build(&config)
                .await
                .expect("both legs construct and attach without either refusing startup");
            let connector = runtime.connector.clone();

            // The regression op: an EVM channel open on a both-chains node
            // must reach the EVM backend. On the last-one-wins slot this
            // exact call hit the Solana backend and refused the 20-byte
            // counterparty.
            let evm_counterparty =
                ethers::signers::LocalWallet::new(&mut ethers::core::rand::thread_rng())
                    .address()
                    .as_bytes()
                    .to_vec();
            let evm_channel = connector
                .open_channel(
                    Some(SettlementChain::Evm),
                    evm_counterparty,
                    Duration::seconds(3600),
                )
                .await
                .expect("an EVM open on a both-chains node reaches the EVM backend");
            assert!(
                evm_channel.id.starts_with("0x"),
                "a TokenNetwork bytes32 channel id, not a Solana account: {}",
                evm_channel.id
            );

            // The Solana twin.
            let solana_counterparty = Keypair::new().pubkey().to_bytes().to_vec();
            let solana_channel = connector
                .open_channel(
                    Some(SettlementChain::Solana),
                    solana_counterparty,
                    Duration::seconds(3600),
                )
                .await
                .expect("a Solana open on a both-chains node reaches the Solana backend");
            assert!(
                Pubkey::from_str(&solana_channel.id).is_ok(),
                "a channel PDA account address, not a TokenNetwork bytes32: {}",
                solana_channel.id
            );

            // Both backends are attached and reachable: the operator's
            // channel list reports each channel fresh from its own chain.
            let channels = connector.channels().await;
            assert_eq!(channels.len(), 2);
            assert!(channels.iter().any(|view| view.id == evm_channel.id));
            assert!(channels.iter().any(|view| view.id == solana_channel.id));

            // Per-channel-id ops route by the id's own namespace: closing
            // each channel lands on the chain that opened it (on the
            // last-one-wins slot, closing the EVM id asked Solana, which
            // knows no such channel).
            connector
                .close_channel(&evm_channel.id)
                .await
                .expect("closing the EVM channel routes to the EVM backend");
            connector
                .close_channel(&solana_channel.id)
                .await
                .expect("closing the Solana channel routes to the Solana backend");

            // And an open that names no chain is ambiguous here, not
            // silently resolved to either backend.
            let ambiguous = connector
                .open_channel(
                    None,
                    Keypair::new().pubkey().to_bytes().to_vec(),
                    Duration::seconds(3600),
                )
                .await;
            assert!(matches!(
                ambiguous,
                Err(connector_runtime::ChannelOperationError::AmbiguousSettlementChain)
            ));
        }

        /// Issue #632's two-chain acceptance criterion: "Two-chain node:
        /// greeting carries both chains' entries in `settlements`; legacy
        /// `settlement` object unchanged". Driven through the same real
        /// anvil + solana-test-validator harness
        /// `a_both_chains_config_attaches_and_routes_both_backends` uses,
        /// so both chains' facts genuinely came from live `connect()` calls
        /// rather than a fixture.
        #[tokio::test]
        async fn a_both_chains_config_composes_both_chains_greeting_facts() {
            if !anvil_available() {
                eprintln!(
                    "skipping: `anvil` not found on PATH (install via https://getfoundry.sh)"
                );
                return;
            }
            if !require_solana_test_validator() {
                return;
            }

            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let token = EvmSettlementBackend::deploy_mock_token(
                &anvil.rpc_url,
                DEPLOYER_PRIVATE_KEY,
                1_000_000,
            )
            .await
            .expect("deploy mock USDC");
            let settlement_backend =
                EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
                    .await
                    .expect("deploy a TokenNetwork through a fresh registry");
            let registry_address = settlement_backend.registry_address();
            drop(settlement_backend);

            let validator = SolanaValidator::spawn().await;
            let program_id =
                Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
            let deployed = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
                .await
                .expect("bind to the genesis-loaded payment-channel program");
            let token_mint = deployed.token_mint();
            drop(deployed);

            let seed = [17u8; 32];
            let payer =
                solana_sdk::signer::keypair::keypair_from_seed(&seed).expect("derive keypair");
            let rpc = RpcClient::new_with_commitment(
                validator.rpc_url.clone(),
                CommitmentConfig::confirmed(),
            );
            fund(&rpc, &payer.pubkey()).await;

            let evm_key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let solana_key_path = raw_key_file(seed);
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{evm_key_path}"

[settlement.evm]
rpc_url = "{evm_rpc_url}"
contract_address = "{registry_address:?}"
token_address = "{token:?}"
decimals = 6

[settlement.evm.key]
key_file = "{evm_key_path}"

[settlement.solana]
rpc_url = "{solana_rpc_url}"
program_id = "{program_id}"
token_address = "{token_mint}"
decimals = 6

[settlement.solana.key]
key_file = "{solana_key_path}"
"#,
                evm_key_path = evm_key_path.display(),
                solana_key_path = solana_key_path.display(),
                evm_rpc_url = anvil.rpc_url,
                solana_rpc_url = validator.rpc_url,
                registry_address = registry_address,
                token = token,
            ));

            let runtime = build(&config)
                .await
                .expect("both legs construct and attach without either refusing startup");

            let facts = connector_domain::NodeFacts {
                settlements: runtime.settlements.clone(),
                ..Default::default()
            };
            let evm_terms = facts
                .evm_settlement()
                .cloned()
                .expect("the EVM leg is the one the legacy greeting object is derived from");
            assert_eq!(
                evm_terms.token_address,
                format!("{token:#x}"),
                "the legacy settlement object names the EVM leg alone, unaffected by the Solana leg"
            );

            assert_eq!(
                runtime.settlements.len(),
                2,
                "both configured chains carry an entry: {:?}",
                runtime.settlements
            );
            assert!(
                runtime.settlements.contains(
                    &connector_client_edge::X402ChainSettlementTerms::Evm(evm_terms)
                ),
                "the settlements list carries the same EVM entry as the legacy object"
            );
            let solana_entry = runtime
                .settlements
                .iter()
                .find_map(|entry| match entry {
                    connector_client_edge::X402ChainSettlementTerms::Solana(terms) => {
                        Some(terms.clone())
                    }
                    _ => None,
                })
                .expect("the settlements list carries a Solana entry");
            assert_eq!(solana_entry.chain, "solana");
            assert_eq!(solana_entry.program_id, program_id.to_string());
            assert_eq!(solana_entry.token_address, token_mint.to_string());
            assert_eq!(solana_entry.decimals, 6);
        }

        /// ADR 0074 decision 8, issue #1345, end to end: a node that opts
        /// into `batch_settlement` on both chains composes both chains'
        /// x402 batch-settlement facts, read off the very backends
        /// `settlements` is composed from -- so the two lists can never
        /// name two different deployments of "this chain" (CF-26).
        #[tokio::test]
        async fn a_both_chains_config_composes_both_chains_batch_settlement_facts() {
            if !require_anvil() {
                return;
            }
            if !require_solana_test_validator() {
                return;
            }

            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            // x402's contracts at their canonical addresses: the EVM batch backend
            // refuses to bind unless x402BatchSettlement answers there.
            connector_settlement_evm::test_support::x402::X402Chain::place(&anvil.rpc_url).await;
            let token = EvmSettlementBackend::deploy_mock_token(
                &anvil.rpc_url,
                DEPLOYER_PRIVATE_KEY,
                1_000_000,
            )
            .await
            .expect("deploy mock USDC");
            let settlement_backend =
                EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
                    .await
                    .expect("deploy a TokenNetwork through a fresh registry");
            let registry_address = settlement_backend.registry_address();
            let evm_settlement_address = settlement_backend.own_address();
            drop(settlement_backend);

            let validator = SolanaValidator::spawn().await;
            let program_id =
                Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
            let deployed = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
                .await
                .expect("bind to the genesis-loaded payment-channel program");
            let token_mint = deployed.token_mint();
            drop(deployed);

            let seed = [23u8; 32];
            let payer =
                solana_sdk::signer::keypair::keypair_from_seed(&seed).expect("derive keypair");
            let rpc = RpcClient::new_with_commitment(
                validator.rpc_url.clone(),
                CommitmentConfig::confirmed(),
            );
            fund(&rpc, &payer.pubkey()).await;

            let evm_key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let solana_key_path = raw_key_file(seed);
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{evm_key_path}"

[settlement.evm]
rpc_url = "{evm_rpc_url}"
contract_address = "{registry_address:?}"
token_address = "{token:?}"
decimals = 6

[settlement.evm.key]
key_file = "{evm_key_path}"

[settlement.evm.batch_settlement]
min_withdraw_delay_secs = 3600
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[settlement.solana]
rpc_url = "{solana_rpc_url}"
program_id = "{program_id}"
token_address = "{token_mint}"
decimals = 6

[settlement.solana.key]
key_file = "{solana_key_path}"

[settlement.solana.batch_settlement]
min_sponsored_deposit = 1000000
min_grace_period_secs = 3600
"#,
                evm_key_path = evm_key_path.display(),
                solana_key_path = solana_key_path.display(),
                evm_rpc_url = anvil.rpc_url,
                solana_rpc_url = validator.rpc_url,
                registry_address = registry_address,
                token = token,
            ));

            let runtime = build(&config)
                .await
                .expect("both legs opt into batch settlement without either refusing startup");

            // No `{:?}` of the facts: they carry keys derived from the
            // settlement key files (rust/cleartext-logging).
            assert_eq!(
                runtime.batch_settlements.len(),
                2,
                "both configured chains opted in"
            );

            let evm_chain_id = runtime
                .settlements
                .iter()
                .find_map(|entry| match entry {
                    connector_client_edge::X402ChainSettlementTerms::Evm(terms) => {
                        Some(terms.chain.trim_start_matches("evm:").to_string())
                    }
                    _ => None,
                })
                .expect("the settlements list carries an EVM entry");
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
                evm_batch.network,
                format!("eip155:{evm_chain_id}"),
                "the same chain id `settlements` proved, spelled CAIP-2"
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
        }

        /// Issue #630's review, finding 2: a `[settlement.solana]`
        /// `program_id` that names a real, executable program which is
        /// *not* the deployed payment-channel program (here: SPL Token
        /// itself, executable on every cluster) must refuse startup naming
        /// the program id -- not pass a mere "exists and is executable"
        /// check and fail lazily at the first settle.
        #[tokio::test]
        async fn a_solana_program_id_naming_some_other_program_refuses_to_build() {
            if !require_solana_test_validator() {
                return;
            }

            let validator = SolanaValidator::spawn().await;
            let program_id =
                Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
            let deployed = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
                .await
                .expect("bind to the genesis-loaded payment-channel program");
            let token_mint = deployed.token_mint();
            drop(deployed);

            let seed = [14u8; 32];
            let payer =
                solana_sdk::signer::keypair::keypair_from_seed(&seed).expect("derive keypair");
            let rpc = RpcClient::new_with_commitment(
                validator.rpc_url.clone(),
                CommitmentConfig::confirmed(),
            );
            fund(&rpc, &payer.pubkey()).await;

            let key_path = raw_key_file(seed);
            let wrong_program_id = spl_token_program_id();
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_path}"

[settlement.solana]
rpc_url = "{rpc_url}"
program_id = "{wrong_program_id}"
token_address = "{token_mint}"
decimals = 6

[settlement.solana.key]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
                rpc_url = validator.rpc_url,
            ));

            let error = build(&config)
                .await
                .err()
                .expect("a program_id naming some other executable program refuses to build");
            let message = error.to_string();
            assert!(
                message.contains(&wrong_program_id.to_string()),
                "the failure must name the configured program id: {message}"
            );
        }

        /// Issue #631's security review, finding 1 (mint binding), full
        /// stack: the deployed program lets any payer open a channel with
        /// ANY mint, and the balance-proof signature does not cover the
        /// mint, so without `channel_counterparty`'s mint check a claim on
        /// a channel funded with a worthless SPL token would buy
        /// USDC-priced writes. Here the wrong-mint channel is genuinely
        /// opened and funded on a real validator, the claim's signature is
        /// genuinely valid -- and the claim gate, resolving through the
        /// same [`SolanaChannelSource`] `build` wires up, still refuses it
        /// as an unknown channel. The control at the end accepts the
        /// byte-identical claim through a backend configured with the
        /// channel's own mint, proving the refusal was the mint binding
        /// and nothing else.
        #[tokio::test]
        async fn a_validly_signed_claim_on_a_wrong_mint_channel_is_refused_as_unknown() {
            use base64::engine::general_purpose::STANDARD as BASE64;
            use base64::Engine;

            if !require_solana_test_validator() {
                return;
            }

            let validator = SolanaValidator::spawn().await;
            let program_id =
                Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");

            // A real, open, funded channel on the junk mint, with this
            // node's own identity as a participant.
            let opener = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
                .await
                .expect("bind to the genesis-loaded payment-channel program");
            let junk_mint = opener.token_mint();
            let counterparty = opener
                .test_counterparty_pubkey()
                .expect("deploy() holds a counterparty key");
            let channel = opener
                .open(counterparty.clone(), Duration::seconds(3600))
                .await
                .expect("open a channel on the junk mint");
            opener
                .test_fund_counterparty(&channel, 1_000)
                .await
                .expect("fund the junk-mint channel with a real on-chain deposit");

            // A genuinely valid claim on it, signed by the channel's real
            // counterparty key.
            let signature = opener
                .test_sign_claim(&channel, 1, 100)
                .expect("deploy() holds the counterparty key to sign with");
            let counterparty_base58 = Pubkey::try_from(counterparty.as_slice())
                .expect("32-byte pubkey")
                .to_string();
            let claim_json = format!(
                r#"{{
                    "version": "1.0",
                    "blockchain": "solana",
                    "messageId": "msg-1",
                    "timestamp": "2026-02-02T12:00:00.000Z",
                    "senderId": "peer-mallory",
                    "programId": "{program_id}",
                    "channelAccount": "{channel_account}",
                    "nonce": 1,
                    "transferredAmount": "100",
                    "signature": "{signature}",
                    "signerPublicKey": "{counterparty_base58}"
                }}"#,
                channel_account = channel.0,
                signature = BASE64.encode(&signature),
            );

            // The node under test: the SAME on-chain identity, configured
            // to settle in a DIFFERENT (real) mint.
            let other = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
                .await
                .expect("deploy a second backend for its fresh mint");
            let configured_mint = other.token_mint();
            assert_ne!(junk_mint, configured_mint);
            drop(other);
            let node_backend = SolanaSettlementBackend::connect(
                &connector_settlement_solana::RpcTransport::direct(&validator.rpc_url)
                    .expect("rpc transport"),
                &opener.test_payer_seed(),
                program_id,
                configured_mint,
                6,
            )
            .await
            .expect("connect under the opener's identity, bound to the configured mint");

            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            ));

            let gate = client_claim_gate(
                &config,
                test_signer(),
                None,
                Some(Arc::new(SolanaChannelSource {
                    backend: Arc::new(node_backend),
                })),
                None,
            )
            .expect("a config with no state_dir produces an in-memory gate");
            let rejection = gate
                .ingest(&claim_json, 100)
                .await
                .expect_err("a claim on a wrong-mint channel must be refused");
            assert!(
                matches!(
                    rejection,
                    connector_client_edge::ClaimIngestRejection::UnknownChannel
                ),
                "refused as an unknown channel, not any other reason: {}",
                rejection.message()
            );

            // Control: the byte-identical claim is accepted through a
            // backend configured with the channel's own mint.
            let matching_backend = SolanaSettlementBackend::connect(
                &connector_settlement_solana::RpcTransport::direct(&validator.rpc_url)
                    .expect("rpc transport"),
                &opener.test_payer_seed(),
                program_id,
                junk_mint,
                6,
            )
            .await
            .expect("connect under the opener's identity, bound to the channel's own mint");
            let gate = client_claim_gate(
                &config,
                test_signer(),
                None,
                Some(Arc::new(SolanaChannelSource {
                    backend: Arc::new(matching_backend),
                })),
                None,
            )
            .expect("a config with no state_dir produces an in-memory gate");
            gate.ingest(&claim_json, 100)
                .await
                .expect("the identical claim is valid and accepted when the mint matches");
        }

        /// Issue #646's EVM half, through the same [`SettlementChannelSource`]
        /// `build` wires up: the deposit is not in `channels(id)` at all
        /// (`TokenNetwork.sol:73-77` keeps it in
        /// `participants[channelId][counterparty]`), so this is the one
        /// extra `eth_call` the cap costs -- paid once, when the channel is
        /// first seen, and memoised after.
        ///
        /// A claim one base unit above a genuinely deposited 1_000 is
        /// refused, and the byte-identical claim is honoured after a real
        /// `setTotalDeposit` covers it.
        #[tokio::test]
        async fn a_claim_above_an_evm_channels_on_chain_deposit_is_refused_until_it_is_funded() {
            use connector_settlement_evm::test_support::DEPLOYER_PRIVATE_KEY as EVM_DEPLOYER;
            use connector_signer::{
                derive_evm_address, evm_balance_proof_digest, to_hex, EvmBalanceProof,
            };
            use libsecp256k1::{Message, PublicKey, SecretKey};

            if !anvil_available() {
                eprintln!(
                    "skipping: `anvil` not found on PATH (install via https://getfoundry.sh)"
                );
                return;
            }

            let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
            let token =
                EvmSettlementBackend::deploy_mock_token(&anvil.rpc_url, EVM_DEPLOYER, 1_000_000)
                    .await
                    .expect("deploy mock USDC");
            let backend = Arc::new(
                EvmSettlementBackend::deploy(&anvil.rpc_url, EVM_DEPLOYER, token)
                    .await
                    .expect("deploy a TokenNetwork through a fresh registry"),
            );

            // A real counterparty whose key this test holds, so the claim
            // below is one `TokenNetwork.claimFromChannel` would recover.
            let secret = SecretKey::parse(&[11u8; 32]).expect("valid secret key");
            let counterparty = derive_evm_address(&PublicKey::from_secret_key(&secret).serialize());
            let channel = backend
                .open(counterparty.to_vec(), Duration::seconds(3600))
                .await
                .expect("open a real channel");
            let state = backend
                .fund_counterparty(&channel, 1_000)
                .await
                .expect("fund the channel with real ERC-20 value");
            assert_eq!(state.counterparty_deposited, 1_000);

            let claim_json = |nonce: u64, transferred_amount: u64| {
                let channel_id = channel_id_bytes(&channel.0);
                let proof = EvmBalanceProof {
                    channel_id,
                    nonce,
                    transferred_amount: u128::from(transferred_amount),
                    locked_amount: 0,
                    locks_root: [0u8; 32],
                    chain_id: backend.chain_id(),
                    token_network_address: backend.address().to_fixed_bytes(),
                };
                let message = Message::parse(&evm_balance_proof_digest(&proof));
                let (signature, recovery_id) = libsecp256k1::sign(&message, &secret);
                let mut bytes = signature.serialize().to_vec();
                let recovery_byte: u8 = recovery_id.into();
                bytes.push(recovery_byte + 27);
                format!(
                    r#"{{
                        "version": "1.0",
                        "blockchain": "evm",
                        "messageId": "msg-{nonce}",
                        "timestamp": "2026-02-02T12:00:00.000Z",
                        "senderId": "peer-mallory",
                        "channelId": "{channel_id_hex}",
                        "nonce": {nonce},
                        "transferredAmount": "{transferred_amount}",
                        "lockedAmount": "0",
                        "locksRoot": "0x{zeros}",
                        "signature": "0x{signature}",
                        "signerAddress": "{signer}"
                    }}"#,
                    channel_id_hex = channel.0,
                    zeros = "0".repeat(64),
                    signature = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                    signer = to_hex(&counterparty),
                )
            };

            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            ));
            let gate = client_claim_gate(
                &config,
                test_signer(),
                Some(Arc::new(SettlementChannelSource {
                    backend: backend.clone(),
                })),
                None,
                None,
            )
            .expect("a config with no state_dir produces an in-memory gate");

            let rejection = gate
                .ingest(&claim_json(1, 1_001), 100)
                .await
                .expect_err("a claim above the on-chain deposit must be refused");
            assert_eq!(
                rejection,
                connector_client_edge::ClaimIngestRejection::Undercollateralized {
                    claimed: 1_001,
                    deposited: 1_000,
                },
                "{}",
                rejection.message()
            );

            let topped_up = backend
                .fund_counterparty(&channel, 1)
                .await
                .expect("a real second setTotalDeposit");
            assert_eq!(topped_up.counterparty_deposited, 1_001);

            // The identical claim, at the identical nonce, once the chain
            // says it can be redeemed.
            accepted_within_the_reattempt_interval(&gate, &claim_json(1, 1_001), 100).await;
        }

        /// Present `claim_json` until it is accepted, or give up.
        ///
        /// A refused undercollateralized claim is expected to become good
        /// once the deposit lands, but not necessarily on the very next
        /// submission: the re-read that notices the deposit is rate-limited
        /// per channel (`ChannelLivenessPolicy::min_reattempt_interval`),
        /// because a refusal that consumes no nonce could otherwise be
        /// re-presented as an unlimited free chain read. Retrying here is
        /// the point rather than a workaround -- it is what proves the
        /// interval is a delay of seconds and not a wall, under the very
        /// policy a production node runs.
        async fn accepted_within_the_reattempt_interval(
            gate: &ClientClaimGate,
            claim_json: &str,
            price: u64,
        ) {
            for _ in 0..40 {
                match gate.ingest(claim_json, price).await {
                    Ok(_) => return,
                    Err(connector_client_edge::ClaimIngestRejection::Undercollateralized {
                        ..
                    }) => {
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    }
                    Err(other) => panic!("unexpected refusal: {}", other.message()),
                }
            }
            panic!(
                "the deposit landed on chain, so the identical claim must become good once the \
                 per-channel re-attempt interval has passed -- it never did"
            );
        }

        /// A Solana claim JSON on `channel`, signed by the channel's real
        /// on-chain counterparty through `opener`'s held key -- the same
        /// shape the wrong-mint test above builds by hand, factored out
        /// because the collateral tests below need several.
        fn solana_claim_json(
            opener: &SolanaSettlementBackend,
            channel: &connector_settlement::ChannelId,
            program_id: Pubkey,
            counterparty: &[u8],
            nonce: u64,
            transferred_amount: u64,
        ) -> String {
            use base64::engine::general_purpose::STANDARD as BASE64;
            use base64::Engine;

            let signature = opener
                .test_sign_claim(channel, nonce, u128::from(transferred_amount))
                .expect("deploy() holds the counterparty key to sign with");
            let counterparty_base58 = Pubkey::try_from(counterparty)
                .expect("32-byte pubkey")
                .to_string();
            format!(
                r#"{{
                    "version": "1.0",
                    "blockchain": "solana",
                    "messageId": "msg-{nonce}",
                    "timestamp": "2026-02-02T12:00:00.000Z",
                    "senderId": "peer-mallory",
                    "programId": "{program_id}",
                    "channelAccount": "{channel_account}",
                    "nonce": {nonce},
                    "transferredAmount": "{transferred_amount}",
                    "signature": "{signature}",
                    "signerPublicKey": "{counterparty_base58}"
                }}"#,
                channel_account = channel.0,
                signature = BASE64.encode(&signature),
            )
        }

        /// Issue #646 on the chain it was actually observed on, end to end
        /// through the same [`SolanaChannelSource`] `build` wires up: the
        /// literal #633 scenario is a real channel PDA opened with a **zero**
        /// USDC deposit, whose validly-signed claims the connector accepted
        /// (nonce 6, 6000 base units) against a vault holding nothing. Every
        /// one of those claims would have reverted
        /// `TransferredAmountExceedsDeposit` at redemption
        /// (`packages/solana-program/src/processor.rs:781-788`).
        ///
        /// The second half proves the refusal is a bound and not a wall: a
        /// real on-chain `Deposit` makes the byte-identical claim, at the
        /// identical nonce, good -- which is exactly what that program's own
        /// comment promises ("a participant who intends to spend more can
        /// deposit first and resubmit the claim").
        #[tokio::test]
        async fn a_claim_above_a_solana_channels_zero_deposit_is_refused_until_it_is_funded() {
            if !require_solana_test_validator() {
                return;
            }

            let validator = SolanaValidator::spawn().await;
            let program_id =
                Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");

            let opener = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
                .await
                .expect("bind to the genesis-loaded payment-channel program");
            let token_mint = opener.token_mint();
            let counterparty = opener
                .test_counterparty_pubkey()
                .expect("deploy() holds a counterparty key");
            // Opened and never funded -- the #633 channel exactly.
            let channel = opener
                .open(counterparty.clone(), Duration::seconds(3600))
                .await
                .expect("open a channel with no deposit at all");

            let node_backend = SolanaSettlementBackend::connect(
                &connector_settlement_solana::RpcTransport::direct(&validator.rpc_url)
                    .expect("rpc transport"),
                &opener.test_payer_seed(),
                program_id,
                token_mint,
                6,
            )
            .await
            .expect("connect under the opener's identity, bound to the channel's own mint");

            let key_path = key_file_with(DEPLOYER_PRIVATE_KEY);
            let config = load_config(&format!(
                r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_path}"
"#,
                key_path = key_path.display(),
            ));
            let gate = client_claim_gate(
                &config,
                test_signer(),
                None,
                Some(Arc::new(SolanaChannelSource {
                    backend: Arc::new(node_backend),
                })),
                None,
            )
            .expect("a config with no state_dir produces an in-memory gate");

            let claim_json =
                solana_claim_json(&opener, &channel, program_id, &counterparty, 6, 6_000);
            let rejection = gate
                .ingest(&claim_json, 100)
                .await
                .expect_err("an uncollateralized claim must be refused");
            assert_eq!(
                rejection,
                connector_client_edge::ClaimIngestRejection::Undercollateralized {
                    claimed: 6_000,
                    deposited: 0,
                },
                "refused for what it is, not as a bad signature or an underpayment: {}",
                rejection.message()
            );

            // A real deposit, from the counterparty's own key, into the
            // channel's own vault.
            let funded = opener
                .test_fund_counterparty(&channel, 6_000)
                .await
                .expect("deposit real SPL value into the channel vault");
            assert_eq!(funded.counterparty_deposited, 6_000);

            // The byte-identical claim redeems now, so the gate accepts it
            // now: the memoised floor was a lower bound and the breach
            // re-read it.
            accepted_within_the_reattempt_interval(&gate, &claim_json, 100).await;
        }

        /// Issue #649 against a real validator: a channel resolved while it
        /// was payable, then genuinely closed and settled on chain, must
        /// stop buying writes. The deployed program zeroes the channel PDA
        /// on settlement (`processor.rs:635-647`), so the chain's answer
        /// afterwards is "no such channel" -- but a resolution cache that is
        /// never invalidated goes on answering from the reading it took
        /// while the channel was open, for the life of the process.
        ///
        /// The registry re-verifies liveness on expiry through the same
        /// refresh path the deposit floor uses; `Duration::ZERO` here makes
        /// that observable without the test sleeping.
        #[tokio::test]
        async fn a_solana_channel_settled_after_resolution_stops_being_accepted() {
            if !require_solana_test_validator() {
                return;
            }

            let validator = SolanaValidator::spawn().await;
            let program_id =
                Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");

            let opener = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
                .await
                .expect("bind to the genesis-loaded payment-channel program");
            let token_mint = opener.token_mint();
            let counterparty = opener
                .test_counterparty_pubkey()
                .expect("deploy() holds a counterparty key");
            // A zero-length challenge period, so this test can settle the
            // channel for real without waiting one out.
            let channel = opener
                .open(counterparty.clone(), Duration::zero())
                .await
                .expect("open an instantly-settleable channel");
            opener
                .test_fund_counterparty(&channel, 1_000)
                .await
                .expect("a real on-chain deposit, so the claim below is genuinely collateralized");

            let node_backend = SolanaSettlementBackend::connect(
                &connector_settlement_solana::RpcTransport::direct(&validator.rpc_url)
                    .expect("rpc transport"),
                &opener.test_payer_seed(),
                program_id,
                token_mint,
                6,
            )
            .await
            .expect("connect under the opener's identity, bound to the channel's own mint");

            let gate = ClientClaimGate::restore(
                ClientChannelRegistry::new()
                    .with_solana_source(Arc::new(SolanaChannelSource {
                        backend: Arc::new(node_backend),
                    }))
                    // Re-verify on every lookup, so the settlement below is
                    // noticed without this test waiting out a refresh
                    // interval. What is under test is that the settled
                    // channel stops resolving at all, not how long that
                    // takes.
                    .with_liveness_policy(
                        connector_client_edge::ChannelLivenessPolicy::reverify_every_lookup(),
                    ),
                Arc::new(InMemoryJournal::new()),
            )
            .expect("a fresh in-memory journal has nothing to replay");

            gate.ingest(
                &solana_claim_json(&opener, &channel, program_id, &counterparty, 1, 100),
                100,
            )
            .await
            .expect("payable while the channel is open and funded");

            // Genuinely settled on chain: closed, then settled, which the
            // program completes by zeroing the channel account.
            opener.close(&channel).await.expect("close the channel");
            let settled = opener.settle(&channel).await.expect("settle the channel");
            assert_eq!(settled.status, connector_settlement::ChannelStatus::Settled);

            let rejection = gate
                .ingest(
                    &solana_claim_json(&opener, &channel, program_id, &counterparty, 2, 200),
                    100,
                )
                .await
                .expect_err("a claim on a settled channel can never be redeemed");
            assert_eq!(
                rejection,
                connector_client_edge::ClaimIngestRejection::UnknownChannel,
                "the settled-channel refusal must not be bypassed by a stale cache: {}",
                rejection.message()
            );
        }

        /// The SPL Token program id -- a program that exists, is
        /// executable, and is definitely not the payment-channel program,
        /// on every Solana cluster including a fresh test validator.
        fn spl_token_program_id() -> Pubkey {
            Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
                .expect("the canonical SPL Token program id")
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
        /// The payment-channel program id the local topology names. Nothing
        /// in these tests dials it.
        const SOL_PROGRAM_ID: &str = "HY4AYFNe5Vg5BkEwAURNsGY3uFAvGMNpAQPRtgoasJiR";

        fn asset(text: &str) -> AssetId {
            text.parse::<AssetId>().expect("a declared asset")
        }

        /// A node dealing the same asset across two chains at a hand-tended
        /// rate. No `quote` anywhere, so it needs no settlement table and
        /// [`build`] dials no chain -- the table is a pure product of the
        /// file.
        #[tokio::test]
        async fn a_dealing_node_carries_its_rate_table_on_the_runtime() {
            let (config, _key_path) = config_with_raw_key_file(|key_path| {
                format!(
                    r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_file}"

[[tokens]]
asset = "evm:{USDC}"
numeraire = true

[[tokens]]
asset = "solana:{USDC_SOLANA}"

[[rates]]
from = "solana:{USDC_SOLANA}"
to = "evm:{USDC}"
rate = {{ numerator = 1, denominator = 1 }}

[rate_guards]
spread = {{ numerator = 30, denominator = 10000 }}
ttl_secs = 300
max_move = {{ numerator = 5, denominator = 100 }}
"#,
                    key_file = key_path.display()
                )
            });

            let runtime = build(&config).await.expect("build");

            let table = runtime
                .rate_table
                .expect("a node that declares tokens deals");
            let held = table.read();
            assert_eq!(held.numeraire(), &asset(&format!("evm:{USDC}")));
            assert!(
                held.lookup(
                    &asset(&format!("solana:{USDC_SOLANA}")),
                    &asset(&format!("evm:{USDC}")),
                    chrono::Utc::now(),
                )
                .is_live(),
                "a declared row is in force from boot and never ages"
            );
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

[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0xe7f1725e7734ce288f8367e1bb143e90bb3f0512"
token_address = "{USDC}"
decimals = 6

[settlement.evm.key]
key_file = "{key_file}"

[[tokens]]
asset = "evm:{USDC}"
numeraire = true

[[tokens]]
asset = "evm:{ANYONE}"
quote = [
  {{ pool = "{POOL_ANYONE_USDC}", quote_token = "evm:{USDC}", twap_window_secs = 1800 }},
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
                    base: asset(&format!("evm:{ANYONE}")),
                    quote: asset(&format!("evm:{USDC}")),
                    rate: connector_domain::Rate::new(4, 1_000_000_000_000).expect("a rate"),
                    observed_at,
                    shortest_window: chrono::Duration::seconds(60),
                    longest_window: chrono::Duration::seconds(3600),
                },
            );

            let started = spawn_rate_pollers(
                &table,
                &RateSources::reading(AssetChain::Evm, source),
                &config,
            )
            .expect("a usable quote path");

            assert_eq!(started, 1, "one token declared a quote path");
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            assert!(
                table
                    .lookup(
                        &asset(&format!("evm:{ANYONE}")),
                        &asset(&format!("evm:{USDC}")),
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

[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0xe7f1725e7734ce288f8367e1bb143e90bb3f0512"
token_address = "{USDC}"
decimals = 6

[settlement.evm.key]
key_file = "{key_file}"

[[tokens]]
asset = "evm:{USDC}"
numeraire = true

[[tokens]]
asset = "evm:{ANYONE}"
quote = [
  {{ pool = "{POOL_ANYONE_USDC}", quote_token = "evm:{USDC}", twap_window_secs = 1800 }},
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
program_id = "{SOL_PROGRAM_ID}"
token_address = "{USDC_SOLANA}"
decimals = 6

[settlement.solana.key]
key_file = "{key_file}"

[[tokens]]
asset = "solana:{USDC_SOLANA}"
numeraire = true

[[tokens]]
asset = "solana:{SOL_DEALT}"
quote = [
  {{ pool = "{POOL_SOL_DEALT_USDC}", quote_token = "solana:{USDC_SOLANA}", twap_window_secs = 900 }},
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
                            chain: AssetChain::Solana,
                        },
                    } if token == &asset(&format!("solana:{SOL_DEALT}"))
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
        /// to poll it over. It starts nothing -- and, since this is the
        /// path `build` takes for such a node, it also needs no settlement
        /// table to boot.
        #[tokio::test]
        async fn a_node_with_no_quote_path_starts_no_poller() {
            let (config, _key_path) = config_with_raw_key_file(|key_path| {
                format!(
                    r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{key_file}"

[[tokens]]
asset = "evm:{USDC}"
numeraire = true

[[tokens]]
asset = "solana:{USDC_SOLANA}"

[[rates]]
from = "solana:{USDC_SOLANA}"
to = "evm:{USDC}"
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

[settlement.evm]
rpc_url = "http://{EVM_HOST}/"
contract_address = "0x00000000000000000000000000000000000000aa"
token_address = "0x00000000000000000000000000000000000000bb"
decimals = 6
rpc_via_socks_proxy = {evm_proxied}

[settlement.evm.key]
key_file = "{key}"

[settlement.solana]
rpc_url = "http://{SOLANA_HOST}/"
program_id = "2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip"
token_address = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU"
decimals = 6
rpc_via_socks_proxy = true

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
            let evm = transports.for_chain(SettlementChain::Evm).expect("evm");
            let solana = transports
                .for_chain(SettlementChain::Solana)
                .expect("solana");
            assert!(evm.is_proxied() && solana.is_proxied());

            // The three EVM clients `build` makes from the one transport.
            assert_eq!(
                EvmRpc::provider(evm.clone())
                    .get_block_number()
                    .await
                    .expect("the backend's reads, through the proxy")
                    .as_u64(),
                42
            );
            let index = EvmChannelIndex::open(
                None,
                IndexedContract {
                    chain_id: 84_532,
                    token_network: ethers::types::Address::zero(),
                },
                1_000,
            )
            .expect("an in-memory index");
            EvmChannelIndexSyncer::new(evm, ethers::types::Address::zero(), 1, 1_000)
                .sync_once(&index)
                .await
                .expect("the syncer's head read, through the proxy");
            let _ = UniswapV3RateSource::connect(evm)
                .observe(&QuoteLeg {
                    pool: PoolId("0x00000000000000000000000000000000000000cc".to_string()),
                    base: connector_domain::AssetId::evm(
                        "0x00000000000000000000000000000000000000bb",
                    ),
                    quote: connector_domain::AssetId::evm(
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
                    && c.username.as_deref() == Some(Circuit::EvmSettlement.socks_username())),
                "{connects:?}"
            );
            assert!(
                connects
                    .iter()
                    .any(|c| c.target == format!("{SOLANA_HOST}:80")
                        && c.username.as_deref()
                            == Some(Circuit::SolanaSettlement.socks_username())),
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
            assert!(!transports
                .for_chain(SettlementChain::Evm)
                .expect("evm")
                .is_proxied());
            assert!(transports
                .for_chain(SettlementChain::Solana)
                .expect("solana")
                .is_proxied());
        }
    }
}
