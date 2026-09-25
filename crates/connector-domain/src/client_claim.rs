//! Client-edge payment claim shape (`docs/protocol/client-edge-spec.md` §1.3,
//! issue #504): the JSON claim a client presents in the
//! `ILP-Payment-Channel-Claim`(`-Wrapped`) header, and its structural
//! validation. Recovered from the deleted
//! `packages/connector/src/btp/btp-claim-types.ts` (git history at
//! `c4a4ad10^`), which is the shape's only prior definition -- field names,
//! required-ness and formats below are ported from its
//! `validateClaimMessage`/`validateEVMClaim`/`validateSolanaClaim`, not
//! guessed at.
//!
//! Distinct from [`crate::claim::Watermark`]'s peer-role `WireClaim`: same
//! nonce/watermark rule ([`crate::validate_claim`], [`crate::advance_watermark`]),
//! a different wire shape and a different channel namespace (a client-edge
//! claim never touches a peer channel).
//!
//! Mina is deliberately excluded from [`ClientClaim`] entirely. ADR 0002
//! drops Mina from the Rust connector, and this ticket's own acceptance
//! criteria requires refusing a Mina claim "with a reason naming the dropped
//! support, distinguishable from a malformed claim" -- [`parse_client_claim`]
//! checks the `blockchain` discriminator before attempting any Mina-specific
//! structural validation, so a well-formed Mina claim is refused for the
//! right reason rather than accidentally accepted or misreported as
//! malformed.
//!
//! Since ADR 0074 (issue #1341) a claim also carries a `scheme`
//! discriminator: absent (or `toon-channel`) is the claim above, unchanged;
//! `batch-settlement` is an x402 **voucher** ([`EvmVoucher`],
//! [`SolanaVoucher`]), whose field names follow x402's own voucher payloads.
//! Those names are provisional until the vectors pin them (#1347, ADR 0021).
//!
//! Cryptographic verification (EIP-712 recovery, Ed25519) and value binding
//! against a route's price are deliberately not this module's concern --
//! issues #506 and #507 -- so a [`ClientClaim`] carries its signature and
//! amount fields only in the string/number shape they arrived in, validated
//! for format but never interpreted cryptographically here.

use serde_json::Value;
use thiserror::Error;

/// Fields common to every claim regardless of chain (client-edge-spec.md
/// §1.3's "required fields on every claim").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientClaimCommon {
    pub message_id: String,
    pub timestamp: String,
    pub sender_id: String,
}

/// An EVM claim (Raiden-style balance proof), per
/// `btp-claim-types.ts`'s `EVMClaimMessage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmClientClaim {
    pub common: ClientClaimCommon,
    pub channel_id: String,
    pub nonce: u64,
    pub transferred_amount: u64,
    pub locked_amount: String,
    pub locks_root: String,
    pub signature: String,
    pub signer_address: String,
    pub chain_id: Option<u64>,
    pub token_network_address: Option<String>,
    pub token_address: Option<String>,
}

/// A Solana claim (Ed25519 balance proof), per
/// `btp-claim-types.ts`'s `SolanaClaimMessage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolanaClientClaim {
    pub common: ClientClaimCommon,
    pub program_id: String,
    pub channel_account: String,
    pub nonce: u64,
    pub transferred_amount: u64,
    pub signature: String,
    pub signer_public_key: String,
    pub cluster: Option<String>,
}

/// The `scheme` discriminator's value for a `toon-channel` claim -- the
/// scheme a claim that carries no `scheme` field at all is (ADR 0074
/// decision 4).
pub const SCHEME_TOON_CHANNEL: &str = "toon-channel";

/// The `scheme` discriminator's value for an x402 `batch-settlement`
/// voucher (ADR 0074 decision 4).
pub const SCHEME_BATCH_SETTLEMENT: &str = "batch-settlement";

/// Which claim scheme a claim is under (ADR 0074 decision 4, `CONTEXT.md`
/// **Claim**). A `toon-channel` claim is ordered by its nonce; a
/// `batch-settlement` claim is a **voucher**, has no nonce, and is ordered
/// by its amount alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimScheme {
    ToonChannel,
    BatchSettlement,
}

impl ClaimScheme {
    /// The discriminator's wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            ClaimScheme::ToonChannel => SCHEME_TOON_CHANNEL,
            ClaimScheme::BatchSettlement => SCHEME_BATCH_SETTLEMENT,
        }
    }
}

/// The `ChannelConfig` an EVM voucher's first presentation carries (ADR
/// 0074 decision 2): the seven immutable fields x402's `getChannelId`
/// hashes, as they arrived on the wire -- each validated for shape here and
/// decoded by the caller, which recomputes the channel id from them and
/// refuses a mismatch. The config is not readable from the chain (the
/// contract stores channels by id), so a connector learns it only from the
/// client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmVoucherChannelConfig {
    pub payer: String,
    pub payer_authorizer: String,
    pub receiver: String,
    pub receiver_authorizer: String,
    pub token: String,
    /// A Solidity `uint40`, in seconds.
    pub withdraw_delay: u64,
    pub salt: String,
}

/// An EVM **voucher**: a claim under x402's `batch-settlement` scheme on
/// `x402BatchSettlement` (ADR 0074 decision 4). Field names follow x402's
/// own voucher payload (`channelId`, `maxClaimableAmount`, `signature`,
/// `channelConfig`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmVoucher {
    pub common: ClientClaimCommon,
    /// The channel the voucher signs, `0x` + 64 hex. Named by the voucher
    /// and verified on chain, never derived (ADR 0074 decision 2).
    pub channel_id: String,
    /// The cumulative amount the voucher authorises. `uint128` on the wire;
    /// a value this connector's `u64` amounts cannot hold is refused
    /// ([`ClientClaimError::AmountOutOfRange`]), never truncated.
    pub max_claimable_amount: u64,
    /// `r ‖ s ‖ v`, `0x` + 130 hex.
    pub signature: String,
    /// Present on a channel's first voucher, and optional after.
    pub channel_config: Option<EvmVoucherChannelConfig>,
}

/// A Solana **voucher**: a claim under x402's `batch-settlement` scheme on
/// payment-channels (ADR 0074 decision 4). Field names follow x402's SVM
/// `BatchVoucher` (`channelId`, `maxClaimableAmount`, `expiresAt`,
/// `signature`), whose signature is base58 there and so here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolanaVoucher {
    pub common: ClientClaimCommon,
    /// The channel account, base58. Canonical as it arrives, like a
    /// `toon-channel` claim's `channelAccount`.
    pub channel_id: String,
    pub max_claimable_amount: u64,
    /// Ed25519, base58 of 64 bytes.
    pub signature: String,
}

/// A structurally valid, non-Mina client-edge claim (client-edge-spec.md
/// §1.3). Discriminated on chain the same way the wire's `blockchain` field
/// does, and -- since ADR 0074 -- on scheme the way its `scheme` field does:
/// [`ClientClaim::Evm`]/[`ClientClaim::Solana`] are `toon-channel` claims,
/// [`ClientClaim::EvmVoucher`]/[`ClientClaim::SolanaVoucher`] are vouchers.
/// A voucher is client edge only: a peer carriage refuses one
/// (`connector_peer_btp::claim_json::parse`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientClaim {
    Evm(EvmClientClaim),
    Solana(SolanaClientClaim),
    EvmVoucher(EvmVoucher),
    SolanaVoucher(SolanaVoucher),
}

impl ClientClaim {
    /// The scheme this claim is under.
    pub fn scheme(&self) -> ClaimScheme {
        match self {
            ClientClaim::Evm(_) | ClientClaim::Solana(_) => ClaimScheme::ToonChannel,
            ClientClaim::EvmVoucher(_) | ClientClaim::SolanaVoucher(_) => {
                ClaimScheme::BatchSettlement
            }
        }
    }

    /// The channel this claim's freshness/watermark is judged against,
    /// namespaced by chain so an EVM `channelId` and a Solana
    /// `channelAccount` can never collide even in the (practically
    /// impossible, since their alphabets differ) case of equal text, and
    /// **canonical** within that namespace (issue #643) -- see
    /// [`canonical_channel_key`] for why the second is a security property
    /// rather than tidiness.
    ///
    /// A voucher's key is §1.3's same (blockchain, channel) tuple in the same
    /// canonical form (ADR 0074 decision 3): only the watermark's
    /// *comparison* differs by scheme, never its key.
    pub fn channel_key(&self) -> String {
        match self {
            ClientClaim::Evm(claim) => {
                format!("{EVM_NAMESPACE}:{}", canonical_evm_id(&claim.channel_id))
            }
            ClientClaim::Solana(claim) => {
                format!("{SOLANA_NAMESPACE}:{}", claim.channel_account)
            }
            ClientClaim::EvmVoucher(voucher) => {
                format!("{EVM_NAMESPACE}:{}", canonical_evm_id(&voucher.channel_id))
            }
            ClientClaim::SolanaVoucher(voucher) => {
                format!("{SOLANA_NAMESPACE}:{}", voucher.channel_id)
            }
        }
    }

    /// The claim's nonce -- for a voucher, which has none,
    /// [`crate::VOUCHER_WATERMARK_NONCE`], the value its watermark is
    /// journaled under. A voucher is never judged by this: its freshness is
    /// [`crate::validate_voucher`], and [`crate::validate_claim`] is for
    /// `toon-channel` claims alone.
    pub fn nonce(&self) -> u64 {
        match self {
            ClientClaim::Evm(claim) => claim.nonce,
            ClientClaim::Solana(claim) => claim.nonce,
            ClientClaim::EvmVoucher(_) | ClientClaim::SolanaVoucher(_) => {
                crate::VOUCHER_WATERMARK_NONCE
            }
        }
    }

    /// The claim's cumulative amount: `transferredAmount`, or a voucher's
    /// `maxClaimableAmount`.
    pub fn transferred_amount(&self) -> u64 {
        match self {
            ClientClaim::Evm(claim) => claim.transferred_amount,
            ClientClaim::Solana(claim) => claim.transferred_amount,
            ClientClaim::EvmVoucher(voucher) => voucher.max_claimable_amount,
            ClientClaim::SolanaVoucher(voucher) => voucher.max_claimable_amount,
        }
    }

    pub fn common(&self) -> &ClientClaimCommon {
        match self {
            ClientClaim::Evm(claim) => &claim.common,
            ClientClaim::Solana(claim) => &claim.common,
            ClientClaim::EvmVoucher(voucher) => &voucher.common,
            ClientClaim::SolanaVoucher(voucher) => &voucher.common,
        }
    }

    /// The key whose signature this claim *says* it carries, namespaced by
    /// chain the same way [`ClientClaim::channel_key`] namespaces the
    /// channel: `evm:0x<40 lower-case hex>` or `solana:<base58>`.
    ///
    /// **This is a self-declared value and it is not authority for
    /// anything.** Whose signature a claim is checked against comes from
    /// the connector's own per-channel record and never from here (issue
    /// #558) -- see `connector_client_edge::ClientChannelRegistry`. It
    /// exists for one narrower purpose: to be the identity an *unresolvable*
    /// channel lookup is budgeted against (issue #613), where by definition
    /// there is no recognized channel to budget against instead, and where
    /// the alternative identities are worse. It is therefore a label for
    /// grouping and attribution, not a credential.
    ///
    /// Canonicalised for exactly the reason
    /// [`canonical_channel_key`] canonicalises an id: hex has no case, so a
    /// budget keyed by the literal text would hand one sender a fresh
    /// allowance per recasing of their own address. Solana's base58 is
    /// case-*sensitive* and a 32-byte key has one spelling, so it is left
    /// alone for the same reason the Solana channel namespace is.
    ///
    /// A voucher declares no signer at all -- its signer is read from the
    /// chain (ADR 0074 decision 4) -- so its label is its `senderId`, which
    /// is exactly as self-declared and exactly as authority-free.
    pub fn signer_key(&self) -> String {
        match self {
            ClientClaim::Evm(claim) => format!(
                "{EVM_NAMESPACE}:{}",
                claim.signer_address.to_ascii_lowercase()
            ),
            ClientClaim::Solana(claim) => {
                format!("{SOLANA_NAMESPACE}:{}", claim.signer_public_key)
            }
            ClientClaim::EvmVoucher(voucher) => format!(
                "{EVM_NAMESPACE}:{}",
                voucher.common.sender_id.to_ascii_lowercase()
            ),
            ClientClaim::SolanaVoucher(voucher) => {
                format!("{SOLANA_NAMESPACE}:{}", voucher.common.sender_id)
            }
        }
    }

    /// The claim's self-declared signer, exactly as written -- unnamespaced
    /// and uncanonicalized, unlike [`ClientClaim::signer_key`]. Its one
    /// consumer is [`crate::identity::resolve_identity`]'s anonymous-sender
    /// ephemeral identity (client-edge-spec.md §1.2, issue #502): a label
    /// derived from whatever this claim already parsed out, not a second
    /// parse of the claim JSON. **This is a self-declared value and it is
    /// not authority for anything** -- the same caveat [`ClientClaim::signer_key`]
    /// documents applies here unchanged. A voucher's is its `senderId`, for
    /// the reason [`ClientClaim::signer_key`] gives.
    pub fn signer(&self) -> &str {
        match self {
            ClientClaim::Evm(claim) => &claim.signer_address,
            ClientClaim::Solana(claim) => &claim.signer_public_key,
            ClientClaim::EvmVoucher(voucher) => &voucher.common.sender_id,
            ClientClaim::SolanaVoucher(voucher) => &voucher.common.sender_id,
        }
    }
}

/// The chain namespace [`ClientClaim::channel_key`] prefixes an EVM
/// `channelId` with.
pub const EVM_NAMESPACE: &str = "evm";

/// The chain namespace [`ClientClaim::channel_key`] prefixes a Solana
/// `channelAccount` with.
pub const SOLANA_NAMESPACE: &str = "solana";

/// The canonical form of an already-namespaced channel key -- the key a
/// watermark is filed under, and the only form a lookup of one may be
/// made in (issue #643).
///
/// # Why this exists
///
/// A watermark keyed by the literal text a claim arrived with is not a
/// replay defence, because one channel has many literal texts. `channelId`
/// is `bytes32` hex, and hex has no case: `0xAB..` and `0xab..` name the
/// same 32 bytes, and every downstream consumer already agrees they do --
/// the client edge's `ClientChannelRegistry` resolves the counterparty by
/// the *decoded* bytes, and the EIP-712 digest a claim's signature recovers
/// under is computed over those same bytes, so a claim re-presented with
/// its `channelId` recased verifies identically. Only the watermark key
/// disagreed, which handed a client 2^64 fresh, empty watermarks per
/// channel: `validate_claim(None, ..)` accepts any nonce, so one signed
/// claim bought a write once per casing it was retyped in.
///
/// So: the key is canonicalized here, once, and both the write and the
/// read of a watermark go through it. Two claims this connector would
/// verify against the same channel record are keyed the same, by
/// construction rather than by every call site remembering to lowercase.
///
/// # What canonical means, per namespace
///
/// * `evm:` -- `0x` followed by 64 lowercase hex characters, whenever the
///   id is 64 hex characters with or without that prefix (i.e. exactly
///   the shape that decodes to a 32-byte `channelId`). Exactly one
///   spelling, and it names the same bytes as every other spelling of it,
///   without this crate taking a hex dependency and without the key
///   ceasing to be greppable in a journal line.
///
///   **The `0x` is kept, and that is a compatibility requirement rather
///   than taste.** A pre-#643 build derived this key as
///   `format!("evm:{}", claim.channel_id)`, and [`parse_evm`] only ever
///   admitted a `0x`-prefixed `channelId`, so every key already on disk
///   is `evm:0x<hex>`. For the universal case -- a client sending
///   lowercase `0x` hex -- the canonical key is therefore **byte-identical
///   to the legacy one**, which is what makes rolling a node's image back
///   to a pre-#643 build a no-op instead of a catastrophe: an older binary
///   replaying a journal this build wrote derives the same key for the
///   next claim, finds the watermark, and refuses the replay. Stripping
///   the prefix would leave that older binary computing `evm:0x<hex>`
///   against a journal folded under `evm:<hex>`, getting `None`, and
///   `validate_claim(None, ..)` re-accepting the client's entire spend
///   history. The deploy model is baked image tags on boxes where rolling
///   a tag back is routine, so that is a live path, not a hypothetical.
/// * `solana:` -- identity. Base58 of an exact 32-byte decode already has
///   exactly one spelling (a differently-spelled string decodes to a
///   different, non-32-byte value), so there is nothing to normalise and
///   normalising anyway would only risk merging two ids that are not the
///   same account.
/// * anything else -- identity, byte for byte. A key in no namespace this
///   function knows is left exactly as it was found rather than guessed
///   at: a journal's entry alphabet is shared with the peer semantics, whose
///   channel ids carry no namespace prefix at all, and quietly rewriting
///   one of those would be inventing a channel.
///
/// # Injectivity
///
/// Canonicalisation only ever *merges* spellings of one channel; it never
/// merges two channels. Within `evm:`, the canonical form is `0x` plus 64
/// lowercase hex characters -- and any string of that shape is itself
/// always canonicalised rather than falling through, so the identity
/// fallback can never emit one and collide with it. Across namespaces
/// nothing can collide: the prefix is preserved.
pub fn canonical_channel_key(key: &str) -> String {
    match key.split_once(':') {
        Some((EVM_NAMESPACE, id)) => format!("{EVM_NAMESPACE}:{}", canonical_evm_id(id)),
        _ => key.to_string(),
    }
}

/// The canonical spelling of an EVM `channelId`: see
/// [`canonical_channel_key`]'s "What canonical means" for the rule and why
/// an id that is not 64 hex characters is returned untouched rather than
/// coerced.
fn canonical_evm_id(channel_id: &str) -> String {
    let hex = channel_id.strip_prefix("0x").unwrap_or(channel_id);
    if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        format!("0x{}", hex.to_ascii_lowercase())
    } else {
        channel_id.to_string()
    }
}

/// Why [`parse_client_claim`] refused a claim. [`ClientClaimError::Mina`] is
/// kept separate from [`ClientClaimError::Malformed`] on purpose -- the
/// acceptance criteria requires the two to be distinguishable, since one
/// means "this connector cannot settle this chain" and the other means "this
/// request made no sense".
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ClientClaimError {
    #[error("claim header is not valid JSON: {0}")]
    InvalidJson(String),
    #[error("claim is structurally invalid: {0}")]
    Malformed(String),
    #[error(
        "mina claims are refused: ADR 0002 drops Mina support from the Rust connector -- \
         stay on the TypeScript fleet for Mina channels"
    )]
    Mina,
    /// A voucher's amount is a valid `uint128` that this connector's `u64`
    /// amount type cannot hold (ADR 0074 decision 3). Refused rather than
    /// truncated: a truncated amount is a different voucher from the one
    /// the payer signed, and not one the chain would honour either.
    #[error(
        "claim is structurally invalid: 'maxClaimableAmount' {amount} is above the {max} this \
         connector's amounts can hold -- refused, not truncated",
        max = u64::MAX
    )]
    AmountOutOfRange { amount: u128 },
    /// A Solana voucher carries a nonzero `expiresAt` (ADR 0074 decision 3):
    /// value that could lapse before this connector lands it. x402 requires
    /// zero, so this is refused structurally.
    #[error(
        "claim is structurally invalid: a Solana voucher's 'expiresAt' must be 0, got {expires_at} \
         -- a voucher that can expire is value that can lapse before it is landed"
    )]
    VoucherExpires { expires_at: i64 },
}

fn malformed(msg: impl Into<String>) -> ClientClaimError {
    ClientClaimError::Malformed(msg.into())
}

fn required_str<'a>(
    obj: &'a serde_json::Map<String, Value>,
    field: &str,
) -> Result<&'a str, ClientClaimError> {
    obj.get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            malformed(format!(
                "missing or invalid '{field}' (expected non-empty string)"
            ))
        })
}

fn optional_str(
    obj: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<String>, ClientClaimError> {
    match obj.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(malformed(format!(
            "'{field}' must be a string when present"
        ))),
    }
}

fn required_nonce(obj: &serde_json::Map<String, Value>) -> Result<u64, ClientClaimError> {
    obj.get("nonce")
        .and_then(Value::as_u64)
        .ok_or_else(|| malformed("missing or invalid 'nonce' (expected a non-negative integer)"))
}

fn required_decimal_amount(
    obj: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<u64, ClientClaimError> {
    let raw = required_str(obj, field)?;
    if !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(malformed(format!(
            "'{field}' must be a non-negative integer string"
        )));
    }
    raw.parse::<u64>()
        .map_err(|_| malformed(format!("'{field}' does not fit in a u64")))
}

fn is_hex_of_len(s: &str, hex_chars: usize) -> bool {
    s.len() == hex_chars + 2 && s.starts_with("0x") && s[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

const BASE58_ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

fn is_base58(s: &str, min_len: usize, max_len: usize) -> bool {
    (min_len..=max_len).contains(&s.len()) && s.bytes().all(|b| BASE58_ALPHABET.contains(&b))
}

fn parse_common(
    obj: &serde_json::Map<String, Value>,
) -> Result<(&str, ClientClaimCommon), ClientClaimError> {
    let version = required_str(obj, "version")?;
    if version != "1.0" {
        return Err(malformed(format!(
            "unsupported claim version '{version}' (expected '1.0')"
        )));
    }
    let blockchain = required_str(obj, "blockchain")?;
    let message_id = required_str(obj, "messageId")?.to_string();
    let timestamp = required_str(obj, "timestamp")?;
    if !is_iso8601(timestamp) {
        return Err(malformed(format!(
            "'timestamp' must be ISO 8601 with a 'Z' timezone, got '{timestamp}'"
        )));
    }
    let timestamp = timestamp.to_string();
    let sender_id = required_str(obj, "senderId")?.to_string();
    Ok((
        blockchain,
        ClientClaimCommon {
            message_id,
            timestamp,
            sender_id,
        },
    ))
}

/// A deliberately narrow ISO-8601 check matching the deleted TS reference's
/// own regex (`YYYY-MM-DDTHH:MM:SS(.mmm)?Z`) -- this is a wire-format gate,
/// not a general-purpose date parser.
fn is_iso8601(s: &str) -> bool {
    let bytes = s.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        bytes
            .get(range.clone())
            .is_some_and(|slice| slice.iter().all(u8::is_ascii_digit) && slice.len() == range.len())
    };
    if bytes.len() == 20 {
        digits(0..4)
            && bytes[4] == b'-'
            && digits(5..7)
            && bytes[7] == b'-'
            && digits(8..10)
            && bytes[10] == b'T'
            && digits(11..13)
            && bytes[13] == b':'
            && digits(14..16)
            && bytes[16] == b':'
            && digits(17..19)
            && bytes[19] == b'Z'
    } else if bytes.len() == 24 {
        digits(0..4)
            && bytes[4] == b'-'
            && digits(5..7)
            && bytes[7] == b'-'
            && digits(8..10)
            && bytes[10] == b'T'
            && digits(11..13)
            && bytes[13] == b':'
            && digits(14..16)
            && bytes[16] == b':'
            && digits(17..19)
            && bytes[19] == b'.'
            && digits(20..23)
            && bytes[23] == b'Z'
    } else {
        false
    }
}

fn parse_evm(
    obj: &serde_json::Map<String, Value>,
    common: ClientClaimCommon,
) -> Result<EvmClientClaim, ClientClaimError> {
    let channel_id = required_str(obj, "channelId")?.to_string();
    if !is_hex_of_len(&channel_id, 64) {
        return Err(malformed(
            "'channelId' must be 0x-prefixed 64-char hex (bytes32)",
        ));
    }
    let nonce = required_nonce(obj)?;
    let transferred_amount = required_decimal_amount(obj, "transferredAmount")?;
    let locked_amount = required_str(obj, "lockedAmount")?.to_string();
    if !locked_amount.bytes().all(|b| b.is_ascii_digit()) {
        return Err(malformed(
            "'lockedAmount' must be a non-negative integer string",
        ));
    }
    let locks_root = required_str(obj, "locksRoot")?.to_string();
    if !is_hex_of_len(&locks_root, 64) {
        return Err(malformed(
            "'locksRoot' must be 0x-prefixed 64-char hex (bytes32)",
        ));
    }
    let signature = required_str(obj, "signature")?.to_string();
    let signer_address = required_str(obj, "signerAddress")?.to_string();
    if !is_hex_of_len(&signer_address, 40) {
        return Err(malformed("'signerAddress' must be 0x-prefixed 40-char hex"));
    }
    let chain_id = match obj.get("chainId") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .filter(|id| *id > 0)
                .ok_or_else(|| malformed("'chainId' must be a positive integer when present"))?,
        ),
    };
    let token_network_address = optional_str(obj, "tokenNetworkAddress")?;
    if let Some(addr) = &token_network_address {
        if !is_hex_of_len(addr, 40) {
            return Err(malformed(
                "'tokenNetworkAddress' must be 0x-prefixed 40-char hex when present",
            ));
        }
    }
    let token_address = optional_str(obj, "tokenAddress")?;
    if let Some(addr) = &token_address {
        if !is_hex_of_len(addr, 40) {
            return Err(malformed(
                "'tokenAddress' must be 0x-prefixed 40-char hex when present",
            ));
        }
    }
    Ok(EvmClientClaim {
        common,
        channel_id,
        nonce,
        transferred_amount,
        locked_amount,
        locks_root,
        signature,
        signer_address,
        chain_id,
        token_network_address,
        token_address,
    })
}

fn parse_solana(
    obj: &serde_json::Map<String, Value>,
    common: ClientClaimCommon,
) -> Result<SolanaClientClaim, ClientClaimError> {
    let program_id = required_str(obj, "programId")?.to_string();
    if !is_base58(&program_id, 32, 44) {
        return Err(malformed(
            "'programId' must be a base58-encoded Solana address (32-44 chars)",
        ));
    }
    let channel_account = required_str(obj, "channelAccount")?.to_string();
    if !is_base58(&channel_account, 32, 44) {
        return Err(malformed(
            "'channelAccount' must be a base58-encoded Solana address (32-44 chars)",
        ));
    }
    let nonce = required_nonce(obj)?;
    let transferred_amount = required_decimal_amount(obj, "transferredAmount")?;
    let signature = required_str(obj, "signature")?.to_string();
    let signer_public_key = required_str(obj, "signerPublicKey")?.to_string();
    if !is_base58(&signer_public_key, 32, 44) {
        return Err(malformed(
            "'signerPublicKey' must be a base58-encoded Solana public key (32-44 chars)",
        ));
    }
    let cluster = optional_str(obj, "cluster")?;
    if let Some(cluster) = &cluster {
        const VALID: &[&str] = &["mainnet-beta", "devnet", "testnet", "localnet"];
        if !VALID.contains(&cluster.as_str()) {
            return Err(malformed(format!(
                "'cluster' must be one of {VALID:?} when present, got '{cluster}'"
            )));
        }
    }
    Ok(SolanaClientClaim {
        common,
        program_id,
        channel_account,
        nonce,
        transferred_amount,
        signature,
        signer_public_key,
        cluster,
    })
}

/// The claim's `scheme` discriminator (ADR 0074 decision 4). Absent -- or
/// `null` -- is `toon-channel`, so every claim that predates the field
/// means exactly what it always meant.
fn parse_scheme(obj: &serde_json::Map<String, Value>) -> Result<ClaimScheme, ClientClaimError> {
    match optional_str(obj, "scheme")?.as_deref() {
        None | Some(SCHEME_TOON_CHANNEL) => Ok(ClaimScheme::ToonChannel),
        Some(SCHEME_BATCH_SETTLEMENT) => Ok(ClaimScheme::BatchSettlement),
        Some(other) => Err(malformed(format!(
            "unsupported claim scheme '{other}' (expected '{SCHEME_TOON_CHANNEL}' or \
             '{SCHEME_BATCH_SETTLEMENT}')"
        ))),
    }
}

/// A voucher's `maxClaimableAmount`: a decimal string holding a `uint128`,
/// which must also fit this connector's `u64` amounts. Anything wider than
/// a `uint128` is not an amount any voucher could sign, so malformed; a
/// valid `uint128` above `u64::MAX` is [`ClientClaimError::AmountOutOfRange`]
/// -- refused, never truncated (ADR 0074 decision 3).
fn required_voucher_amount(obj: &serde_json::Map<String, Value>) -> Result<u64, ClientClaimError> {
    let raw = required_str(obj, "maxClaimableAmount")?;
    if !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(malformed(
            "'maxClaimableAmount' must be a non-negative integer string",
        ));
    }
    let amount = raw
        .parse::<u128>()
        .map_err(|_| malformed("'maxClaimableAmount' does not fit in a uint128"))?;
    u64::try_from(amount).map_err(|_| ClientClaimError::AmountOutOfRange { amount })
}

/// The largest value a Solidity `uint40` -- `ChannelConfig.withdrawDelay`
/// -- can hold.
const UINT40_MAX: u64 = (1 << 40) - 1;

fn parse_evm_voucher_channel_config(
    obj: &serde_json::Map<String, Value>,
) -> Result<Option<EvmVoucherChannelConfig>, ClientClaimError> {
    let config = match obj.get("channelConfig") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Object(config)) => config,
        Some(_) => return Err(malformed("'channelConfig' must be an object when present")),
    };
    let address = |field: &str| -> Result<String, ClientClaimError> {
        let value = required_str(config, field)
            .map_err(|_| malformed(format!("'channelConfig.{field}' is missing")))?;
        if !is_hex_of_len(value, 40) {
            return Err(malformed(format!(
                "'channelConfig.{field}' must be 0x-prefixed 40-char hex"
            )));
        }
        Ok(value.to_string())
    };
    let withdraw_delay = config
        .get("withdrawDelay")
        .and_then(Value::as_u64)
        .filter(|delay| *delay <= UINT40_MAX)
        .ok_or_else(|| {
            malformed("'channelConfig.withdrawDelay' must be an integer that fits a uint40")
        })?;
    let salt =
        required_str(config, "salt").map_err(|_| malformed("'channelConfig.salt' is missing"))?;
    if !is_hex_of_len(salt, 64) {
        return Err(malformed(
            "'channelConfig.salt' must be 0x-prefixed 64-char hex (bytes32)",
        ));
    }
    Ok(Some(EvmVoucherChannelConfig {
        payer: address("payer")?,
        payer_authorizer: address("payerAuthorizer")?,
        receiver: address("receiver")?,
        receiver_authorizer: address("receiverAuthorizer")?,
        token: address("token")?,
        withdraw_delay,
        salt: salt.to_string(),
    }))
}

fn parse_evm_voucher(
    obj: &serde_json::Map<String, Value>,
    common: ClientClaimCommon,
) -> Result<EvmVoucher, ClientClaimError> {
    let channel_id = required_str(obj, "channelId")?.to_string();
    if !is_hex_of_len(&channel_id, 64) {
        return Err(malformed(
            "'channelId' must be 0x-prefixed 64-char hex (bytes32)",
        ));
    }
    let max_claimable_amount = required_voucher_amount(obj)?;
    let signature = required_str(obj, "signature")?.to_string();
    if !is_hex_of_len(&signature, 130) {
        return Err(malformed(
            "a voucher's 'signature' must be 0x-prefixed 130-char hex (r ‖ s ‖ v)",
        ));
    }
    let channel_config = parse_evm_voucher_channel_config(obj)?;
    Ok(EvmVoucher {
        common,
        channel_id,
        max_claimable_amount,
        signature,
        channel_config,
    })
}

fn parse_solana_voucher(
    obj: &serde_json::Map<String, Value>,
    common: ClientClaimCommon,
) -> Result<SolanaVoucher, ClientClaimError> {
    let channel_id = required_str(obj, "channelId")?.to_string();
    if !is_base58(&channel_id, 32, 44) {
        return Err(malformed(
            "'channelId' must be a base58-encoded Solana address (32-44 chars)",
        ));
    }
    let max_claimable_amount = required_voucher_amount(obj)?;
    let expires_at = obj
        .get("expiresAt")
        .and_then(Value::as_i64)
        .ok_or_else(|| malformed("missing or invalid 'expiresAt' (expected an integer)"))?;
    if expires_at != 0 {
        return Err(ClientClaimError::VoucherExpires { expires_at });
    }
    let signature = required_str(obj, "signature")?.to_string();
    // Base58 of 64 bytes is at most 88 characters; its exact length is
    // checked where it is decoded.
    if !is_base58(&signature, 1, 88) {
        return Err(malformed(
            "a Solana voucher's 'signature' must be base58 (an Ed25519 signature)",
        ));
    }
    Ok(SolanaVoucher {
        common,
        channel_id,
        max_claimable_amount,
        signature,
    })
}

/// Parse and structurally validate a client-edge claim's JSON body
/// (client-edge-spec.md §1.3): required/optional fields per chain and
/// scheme, and their formats. Does not check freshness, value or
/// cryptography -- those are [`crate::validate_claim`] (or, for a voucher,
/// [`crate::validate_voucher`]) and issues #506/#507 respectively.
///
/// `blockchain: "mina"` is refused as [`ClientClaimError::Mina`] before any
/// Mina-specific field is even inspected -- a well-formed Mina claim is
/// refused for the deliberate reason ADR 0002 gives, never reported as
/// malformed. That holds whatever `scheme` it declares.
///
/// `scheme` (ADR 0074 decision 4) selects the claim's shape: absent or
/// `toon-channel` is today's claim, `batch-settlement` a voucher. This
/// function parses both; which carriages *accept* a voucher is theirs to
/// say, and every peer carriage refuses one.
pub fn parse_client_claim(json: &str) -> Result<ClientClaim, ClientClaimError> {
    let value: Value =
        serde_json::from_str(json).map_err(|e| ClientClaimError::InvalidJson(e.to_string()))?;
    let obj = value
        .as_object()
        .ok_or_else(|| malformed("claim must be a JSON object"))?;

    let (blockchain, common) = parse_common(obj)?;
    if blockchain == "mina" {
        return Err(ClientClaimError::Mina);
    }
    match (blockchain, parse_scheme(obj)?) {
        ("evm", ClaimScheme::ToonChannel) => parse_evm(obj, common).map(ClientClaim::Evm),
        ("solana", ClaimScheme::ToonChannel) => parse_solana(obj, common).map(ClientClaim::Solana),
        ("evm", ClaimScheme::BatchSettlement) => {
            parse_evm_voucher(obj, common).map(ClientClaim::EvmVoucher)
        }
        ("solana", ClaimScheme::BatchSettlement) => {
            parse_solana_voucher(obj, common).map(ClientClaim::SolanaVoucher)
        }
        (other, _) => Err(malformed(format!("unsupported blockchain '{other}'"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evm_claim_json() -> String {
        let channel_id = format!("0x{}", "ab".repeat(32));
        let locks_root = format!("0x{}", "0".repeat(64));
        format!(
            r#"{{
            "version": "1.0",
            "blockchain": "evm",
            "messageId": "claim-1",
            "timestamp": "2026-02-02T12:00:00.000Z",
            "senderId": "peer-bob",
            "channelId": "{channel_id}",
            "nonce": 5,
            "transferredAmount": "1000000000000000000",
            "lockedAmount": "0",
            "locksRoot": "{locks_root}",
            "signature": "0xabcdef",
            "signerAddress": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1"
        }}"#
        )
    }

    /// A conforming fixture: `programId` is a settlement program a channel
    /// could live under, which is what a payer MUST write there
    /// (`client-edge-spec.md` §1.3, issue #1127). It was the **system
    /// program** until now -- no channel lives under that, so this shared
    /// structural validator's own example was the one value the pinned rule
    /// excludes. Nothing here consults the field (that is the caller's job,
    /// and only the client edge does it), so no assertion moves.
    fn solana_claim_json() -> &'static str {
        r#"{
            "version": "1.0",
            "blockchain": "solana",
            "messageId": "claim-2",
            "timestamp": "2026-02-02T12:00:00Z",
            "senderId": "peer-carol",
            "programId": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
            "channelAccount": "So11111111111111111111111111111111111111112",
            "nonce": 3,
            "transferredAmount": "42",
            "signature": "deadbeef",
            "signerPublicKey": "So11111111111111111111111111111111111111112"
        }"#
    }

    #[test]
    fn a_well_formed_evm_claim_parses() {
        let claim = parse_client_claim(&evm_claim_json()).expect("parses");
        match claim {
            ClientClaim::Evm(evm) => {
                assert_eq!(evm.nonce, 5);
                assert_eq!(evm.transferred_amount, 1_000_000_000_000_000_000);
                assert_eq!(evm.common.message_id, "claim-1");
            }
            other => panic!("expected an EVM claim, got {other:?}"),
        }
    }

    #[test]
    fn a_well_formed_solana_claim_parses() {
        let claim = parse_client_claim(solana_claim_json()).expect("parses");
        match claim {
            ClientClaim::Solana(solana) => {
                assert_eq!(solana.nonce, 3);
                assert_eq!(solana.transferred_amount, 42);
            }
            other => panic!("expected a Solana claim, got {other:?}"),
        }
    }

    #[test]
    fn channel_key_is_namespaced_by_chain() {
        let claim = parse_client_claim(solana_claim_json()).expect("parses");
        assert!(claim.channel_key().starts_with("solana:"));
    }

    /// Issue #613: the identity an unresolvable lookup is budgeted against
    /// is namespaced by chain for the same reason the channel key is -- an
    /// EVM address and a Solana public key are different kinds of thing,
    /// and one budget must never be spendable from the other's namespace.
    #[test]
    fn a_signer_key_is_namespaced_by_chain() {
        assert!(parse_client_claim(&evm_claim_json())
            .expect("parses")
            .signer_key()
            .starts_with("evm:"));
        assert!(parse_client_claim(solana_claim_json())
            .expect("parses")
            .signer_key()
            .starts_with("solana:"));
    }

    /// Unlike `signer_key`, `signer` carries the claim's self-declared
    /// value exactly as written -- no chain namespace, no case
    /// canonicalization -- since its one consumer (`identity::resolve_identity`)
    /// formats it into `http:<signer>` itself (client-edge-spec.md §1.2).
    #[test]
    fn signer_is_the_self_declared_value_unnamespaced_and_uncanonicalized() {
        assert_eq!(
            parse_client_claim(&evm_claim_json())
                .expect("parses")
                .signer(),
            "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1"
        );
        assert_eq!(
            parse_client_claim(solana_claim_json())
                .expect("parses")
                .signer(),
            "So11111111111111111111111111111111111111112"
        );
    }

    /// An address's casing is notation, not identity (the same rule #643
    /// established for a `channelId`). A budget keyed by the literal text
    /// would hand one sender 2^40 fresh allowances for the price of
    /// re-typing their own checksummed address, which is not a budget.
    #[test]
    fn an_evm_signer_key_is_the_same_whatever_case_its_hex_arrived_in() {
        let checksummed = parse_client_claim(&evm_claim_json())
            .expect("parses")
            .signer_key();
        let lower = parse_client_claim(&evm_claim_json().replace(
            "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1",
            "0x742d35cc6634c0532925a3b844bc9e7595f0beb1",
        ))
        .expect("parses")
        .signer_key();

        assert_eq!(checksummed, lower);
        assert_eq!(
            checksummed,
            "evm:0x742d35cc6634c0532925a3b844bc9e7595f0beb1"
        );
    }

    // -- Canonical channel keys (issue #643) --

    /// The defect itself: hex has no case, so the same signed claim
    /// retyped in a different casing must not be a different channel. On a
    /// tree without this fix the two keys differ, each gets its own empty
    /// watermark, and `validate_claim(None, ..)` accepts the replay.
    #[test]
    fn an_evm_channel_key_is_the_same_whatever_case_its_hex_arrived_in() {
        let lower = evm_claim_json();
        let upper = lower.replace(&"ab".repeat(32), &"AB".repeat(32));
        assert_ne!(lower, upper, "the two spellings must actually differ");

        let lower_key = parse_client_claim(&lower).expect("parses").channel_key();
        let upper_key = parse_client_claim(&upper).expect("parses").channel_key();

        assert_eq!(lower_key, upper_key);
        assert_eq!(lower_key, format!("evm:0x{}", "ab".repeat(32)));
    }

    /// Mixed casing -- the shape a real attacker would reach for, since it
    /// looks least like a deliberate probe -- collapses the same way.
    #[test]
    fn an_evm_channel_key_is_the_same_for_mixed_case_hex() {
        let mixed = evm_claim_json().replace(&"ab".repeat(32), &"aB".repeat(32));
        assert_eq!(
            parse_client_claim(&mixed).expect("parses").channel_key(),
            format!("evm:0x{}", "ab".repeat(32))
        );
    }

    /// The prefix variant. Today's parser requires the `0x` prefix, so a
    /// bare-hex `channelId` never reaches a watermark at all -- but the
    /// client edge's `decode_hex_bytes` strips the prefix when it resolves
    /// the channel record, so the two *would* be one channel the moment
    /// step 1 were loosened. The key is canonical over both spellings now,
    /// so loosening the parser can never reopen this hole.
    #[test]
    fn a_bare_hex_channel_id_is_refused_today_and_canonicalises_to_the_prefixed_key_anyway() {
        let bare = evm_claim_json().replace(&format!("0x{}", "ab".repeat(32)), &"ab".repeat(32));
        assert!(
            matches!(
                parse_client_claim(&bare),
                Err(ClientClaimError::Malformed(_))
            ),
            "a bare-hex channelId is a structural failure today"
        );

        assert_eq!(
            canonical_channel_key(&format!("evm:{}", "AB".repeat(32))),
            canonical_channel_key(&format!("evm:0x{}", "ab".repeat(32)))
        );
    }

    /// **The rollback contract.** For the universal case -- a client
    /// sending the lowercase `0x` hex `parse_evm` has always required --
    /// the canonical key must be *byte-identical* to the key a pre-#643
    /// build derived, or rolling a node's image back to one of those
    /// builds orphans every watermark this build wrote and re-accepts the
    /// client's whole spend history. The legacy formula is reproduced
    /// here rather than called: it no longer exists in this tree, and
    /// this test is the contract with a binary that is not this one.
    #[test]
    fn the_canonical_key_is_byte_identical_to_the_key_a_pre_643_build_derived() {
        let claim = parse_client_claim(&evm_claim_json()).expect("parses");
        let ClientClaim::Evm(evm) = &claim else {
            panic!("expected an EVM claim");
        };

        // Exactly `ClientClaim::channel_key` as it stood before #643.
        let legacy_key = format!("evm:{}", evm.channel_id);

        assert_eq!(
            claim.channel_key(),
            legacy_key,
            "a rolled-back binary must derive the same key this build files under"
        );
        assert_eq!(claim.channel_key(), format!("evm:0x{}", "ab".repeat(32)));
    }

    /// Canonicalising a key that is already canonical changes nothing --
    /// what makes it safe to apply on both the write and the read of a
    /// watermark, and on a journal entry written by either build.
    #[test]
    fn canonicalising_a_canonical_key_is_a_no_op() {
        let key = format!("evm:0x{}", "ab".repeat(32));
        assert_eq!(canonical_channel_key(&key), key);
        assert_eq!(canonical_channel_key(&canonical_channel_key(&key)), key);
    }

    /// Solana ids are canonical as they arrive: base58 of an exact 32-byte
    /// decode has one spelling, and case is *significant* in base58 -- so
    /// this must leave them strictly alone, or it would merge two accounts
    /// that are genuinely different.
    #[test]
    fn a_solana_channel_key_is_left_exactly_as_it_arrived() {
        let account = "So11111111111111111111111111111111111111112";
        assert_eq!(
            canonical_channel_key(&format!("solana:{account}")),
            format!("solana:{account}")
        );
        assert_ne!(
            canonical_channel_key(&format!("solana:{}", account.to_lowercase())),
            format!("solana:{account}"),
            "base58 is case-sensitive: these are different accounts"
        );
    }

    /// A key in no namespace this function knows -- a peer channel id
    /// sharing the journal's entry alphabet, say -- is returned byte for
    /// byte. Rewriting one would be inventing a channel.
    #[test]
    fn a_key_in_no_known_namespace_is_returned_untouched() {
        for key in ["channel-a", "0xABCDEF", "", "mina:B62qFoo", "evm"] {
            assert_eq!(canonical_channel_key(key), key);
        }
    }

    /// Canonicalisation merges spellings of one channel, never two
    /// channels: an EVM id that is not 64 hex characters is left as it was
    /// and still cannot collide with one that was canonicalised.
    #[test]
    fn canonicalisation_never_merges_two_different_evm_channels() {
        let a = canonical_channel_key(&format!("evm:0x{}", "ab".repeat(32)));
        let b = canonical_channel_key(&format!("evm:0x{}", "cd".repeat(32)));
        let short = canonical_channel_key("evm:0xabcd");
        assert_ne!(a, b);
        assert_ne!(a, short);
        assert_eq!(short, "evm:0xabcd");
    }

    #[test]
    fn not_json_at_all_is_invalid_json_not_malformed() {
        let err = parse_client_claim("not json").unwrap_err();
        assert!(matches!(err, ClientClaimError::InvalidJson(_)));
    }

    #[test]
    fn a_claim_missing_a_required_field_is_malformed() {
        let json = r#"{
            "version": "1.0",
            "blockchain": "evm",
            "messageId": "claim-1",
            "timestamp": "2026-02-02T12:00:00.000Z",
            "senderId": "peer-bob"
        }"#;
        let err = parse_client_claim(json).unwrap_err();
        assert!(matches!(err, ClientClaimError::Malformed(_)));
    }

    /// The Solana half of the case above, on the one field issue #1127
    /// pinned. `programId` names the settlement program the
    /// `channelAccount` lives under (`client-edge-spec.md` §1.3), and it is
    /// **required**: a claim that omits it declares no program at all, which
    /// is a different and worse thing than declaring the wrong one. Only the
    /// EVM shape had a missing-field test before, so the required-ness of
    /// this field rested on `required_str`'s implementation alone.
    #[test]
    fn a_solana_claim_with_no_program_id_is_malformed() {
        let without = solana_claim_json().replace(
            r#""programId": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA","#,
            "",
        );
        assert!(
            !without.contains("programId"),
            "the field really was removed"
        );
        let err = parse_client_claim(&without).unwrap_err();
        assert!(matches!(err, ClientClaimError::Malformed(_)), "{err:?}");
    }

    #[test]
    fn a_claim_with_a_field_in_the_wrong_format_for_its_chain_is_malformed() {
        let channel_id_field = format!(r#""channelId": "0x{}""#, "ab".repeat(32));
        let bad = evm_claim_json().replace(&channel_id_field, r#""channelId": "not-hex""#);
        let err = parse_client_claim(&bad).unwrap_err();
        assert!(matches!(err, ClientClaimError::Malformed(_)));
    }

    #[test]
    fn a_mina_claim_is_refused_distinguishably_from_malformed() {
        let json = r#"{
            "version": "1.0",
            "blockchain": "mina",
            "messageId": "claim-3",
            "timestamp": "2026-02-02T12:00:00.000Z",
            "senderId": "peer-dave",
            "zkAppAddress": "B620000000000000000000000000000000000000000000000000",
            "tokenId": "1",
            "balanceCommitment": "abc",
            "nonce": 1,
            "proof": "AAAA",
            "salt": "salt"
        }"#;
        let err = parse_client_claim(json).unwrap_err();
        assert_eq!(err, ClientClaimError::Mina);
        assert_ne!(err, ClientClaimError::Malformed("anything".to_string()));
    }

    #[test]
    fn an_unsupported_blockchain_is_malformed_not_mina() {
        let json = r#"{
            "version": "1.0",
            "blockchain": "bitcoin",
            "messageId": "claim-4",
            "timestamp": "2026-02-02T12:00:00.000Z",
            "senderId": "peer-erin"
        }"#;
        let err = parse_client_claim(json).unwrap_err();
        assert!(matches!(err, ClientClaimError::Malformed(_)));
    }

    #[test]
    fn a_wrong_version_is_malformed() {
        let bad = evm_claim_json().replace(r#""version": "1.0""#, r#""version": "2.0""#);
        let err = parse_client_claim(&bad).unwrap_err();
        assert!(matches!(err, ClientClaimError::Malformed(_)));
    }

    #[test]
    fn optional_fields_are_accepted_when_present_and_valid() {
        let with_optional = evm_claim_json().replace(
            r#""signerAddress": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1""#,
            r#""signerAddress": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1", "chainId": 8453, "tokenNetworkAddress": "0x1234567890123456789012345678901234567890""#,
        );
        let claim = parse_client_claim(&with_optional).expect("parses");
        match claim {
            ClientClaim::Evm(evm) => {
                assert_eq!(evm.chain_id, Some(8453));
                assert_eq!(
                    evm.token_network_address,
                    Some("0x1234567890123456789012345678901234567890".to_string())
                );
            }
            ClientClaim::Solana(_) => panic!("expected an EVM claim"),
            other => panic!("expected an EVM claim, got {other:?}"),
        }
    }

    // -- The scheme discriminator and the voucher shape (ADR 0074, #1341) --

    fn evm_channel_config_json() -> String {
        format!(
            r#"{{
                "payer": "0x{payer}",
                "payerAuthorizer": "0x70997970c51812dc3a010c7d01b50e0d17dc79c8",
                "receiver": "0x{receiver}",
                "receiverAuthorizer": "0x{receiver}",
                "token": "0x{token}",
                "withdrawDelay": 86400,
                "salt": "0x{salt}"
            }}"#,
            payer = "11".repeat(20),
            receiver = "33".repeat(20),
            token = "55".repeat(20),
            salt = "66".repeat(32),
        )
    }

    fn evm_voucher_json() -> String {
        evm_voucher_json_with(Some(&evm_channel_config_json()))
    }

    fn evm_voucher_json_with(channel_config: Option<&str>) -> String {
        let config = channel_config
            .map(|config| format!(r#","channelConfig": {config}"#))
            .unwrap_or_default();
        format!(
            r#"{{
            "version": "1.0",
            "blockchain": "evm",
            "scheme": "batch-settlement",
            "messageId": "voucher-1",
            "timestamp": "2026-09-25T12:00:00.000Z",
            "senderId": "client-1",
            "channelId": "0x{channel}",
            "maxClaimableAmount": "5000",
            "signature": "0x{signature}1b"{config}
        }}"#,
            channel = "88".repeat(32),
            signature = "ab".repeat(64),
        )
    }

    fn solana_voucher_json() -> String {
        r#"{
            "version": "1.0",
            "blockchain": "solana",
            "scheme": "batch-settlement",
            "messageId": "voucher-2",
            "timestamp": "2026-09-25T12:00:00Z",
            "senderId": "client-2",
            "channelId": "So11111111111111111111111111111111111111112",
            "maxClaimableAmount": "42",
            "expiresAt": 0,
            "signature": "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYjCJjBRnbJLgp8uirBgmQpjKhoR4tjF3ZpRzrFmBV6UjKdiSZkQUW"
        }"#
        .to_string()
    }

    #[test]
    fn a_claim_with_no_scheme_is_a_toon_channel_claim() {
        let claim = parse_client_claim(&evm_claim_json()).expect("parses");
        assert_eq!(claim.scheme(), ClaimScheme::ToonChannel);
        assert!(matches!(claim, ClientClaim::Evm(_)));
    }

    /// Naming the default scheme explicitly changes nothing: the same bytes
    /// with `"scheme": "toon-channel"` parse to the same claim.
    #[test]
    fn an_explicit_toon_channel_scheme_parses_exactly_as_an_absent_one() {
        let explicit = evm_claim_json().replace(
            r#""blockchain": "evm","#,
            r#""blockchain": "evm", "scheme": "toon-channel","#,
        );
        assert_eq!(
            parse_client_claim(&explicit).expect("parses"),
            parse_client_claim(&evm_claim_json()).expect("parses")
        );
    }

    #[test]
    fn an_unknown_scheme_is_malformed() {
        let unknown = evm_claim_json().replace(
            r#""blockchain": "evm","#,
            r#""blockchain": "evm", "scheme": "exact","#,
        );
        assert!(matches!(
            parse_client_claim(&unknown),
            Err(ClientClaimError::Malformed(_))
        ));
    }

    #[test]
    fn a_well_formed_evm_voucher_parses() {
        let claim = parse_client_claim(&evm_voucher_json()).expect("parses");
        assert_eq!(claim.scheme(), ClaimScheme::BatchSettlement);
        let ClientClaim::EvmVoucher(voucher) = &claim else {
            panic!("expected an EVM voucher, got {claim:?}");
        };
        assert_eq!(voucher.max_claimable_amount, 5_000);
        let config = voucher.channel_config.as_ref().expect("config present");
        assert_eq!(config.withdraw_delay, 86_400);
        assert_eq!(claim.transferred_amount(), 5_000);
        assert_eq!(claim.nonce(), crate::VOUCHER_WATERMARK_NONCE);
    }

    #[test]
    fn an_evm_voucher_needs_no_channel_config_after_its_first() {
        let without = parse_client_claim(&evm_voucher_json_with(None));
        let ClientClaim::EvmVoucher(voucher) = without.expect("parses") else {
            panic!("expected an EVM voucher");
        };
        assert_eq!(voucher.channel_config, None);
    }

    #[test]
    fn a_well_formed_solana_voucher_parses() {
        let claim = parse_client_claim(&solana_voucher_json()).expect("parses");
        assert_eq!(claim.scheme(), ClaimScheme::BatchSettlement);
        let ClientClaim::SolanaVoucher(voucher) = &claim else {
            panic!("expected a Solana voucher, got {claim:?}");
        };
        assert_eq!(voucher.max_claimable_amount, 42);
    }

    /// §1.3's (blockchain, channel) tuple, in its canonical form: a voucher
    /// is keyed exactly as a claim on the same channel id would be.
    #[test]
    fn a_voucher_is_keyed_by_the_same_canonical_tuple_as_a_claim() {
        let upper = evm_voucher_json().replace(&"88".repeat(32), &"8A".repeat(32));
        assert_eq!(
            parse_client_claim(&upper).expect("parses").channel_key(),
            format!("evm:0x{}", "8a".repeat(32))
        );
        assert_eq!(
            parse_client_claim(&solana_voucher_json())
                .expect("parses")
                .channel_key(),
            "solana:So11111111111111111111111111111111111111112"
        );
    }

    #[test]
    fn a_solana_voucher_with_a_nonzero_expiry_is_refused_structurally() {
        for expires_at in [1i64, -1, i64::MAX] {
            let json = solana_voucher_json().replace(
                r#""expiresAt": 0"#,
                &format!(r#""expiresAt": {expires_at}"#),
            );
            assert_eq!(
                parse_client_claim(&json),
                Err(ClientClaimError::VoucherExpires { expires_at })
            );
        }
    }

    #[test]
    fn a_solana_voucher_with_no_expiry_is_malformed() {
        let json = solana_voucher_json().replace(r#""expiresAt": 0,"#, "");
        assert!(matches!(
            parse_client_claim(&json),
            Err(ClientClaimError::Malformed(_))
        ));
    }

    #[test]
    fn a_voucher_amount_above_u64_is_refused_not_truncated() {
        let above = u128::from(u64::MAX) + 1;
        let json = evm_voucher_json().replace(r#""5000""#, &format!(r#""{above}""#));
        assert_eq!(
            parse_client_claim(&json),
            Err(ClientClaimError::AmountOutOfRange { amount: above })
        );
    }

    #[test]
    fn a_voucher_amount_above_u128_is_malformed() {
        let json =
            evm_voucher_json().replace(r#""5000""#, r#""340282366920938463463374607431768211456""#);
        assert!(matches!(
            parse_client_claim(&json),
            Err(ClientClaimError::Malformed(_))
        ));
    }

    #[test]
    fn a_withdraw_delay_wider_than_a_uint40_is_malformed() {
        let json = evm_voucher_json().replace("86400", &(UINT40_MAX + 1).to_string());
        assert!(matches!(
            parse_client_claim(&json),
            Err(ClientClaimError::Malformed(_))
        ));
    }

    #[test]
    fn an_evm_voucher_signature_must_be_65_bytes_of_hex() {
        let json = evm_voucher_json().replace(&format!("{}1b", "ab".repeat(64)), "abcdef");
        assert!(matches!(
            parse_client_claim(&json),
            Err(ClientClaimError::Malformed(_))
        ));
    }

    /// A Mina claim is refused by name whatever scheme it declares -- a
    /// voucher on a chain this connector cannot settle is still that chain.
    #[test]
    fn a_mina_voucher_is_still_refused_as_mina() {
        let json =
            solana_voucher_json().replace(r#""blockchain": "solana""#, r#""blockchain": "mina""#);
        assert_eq!(parse_client_claim(&json), Err(ClientClaimError::Mina));
    }

    proptest::proptest! {
        /// ADR 0074 decision 3: a Solana voucher that could expire is
        /// refused, whatever the expiry -- in the past, the future, or
        /// negative -- and only `0` parses.
        #[test]
        fn a_solana_voucher_parses_only_at_a_zero_expiry(expires_at in proptest::prelude::any::<i64>()) {
            let json = solana_voucher_json()
                .replace(r#""expiresAt": 0"#, &format!(r#""expiresAt": {expires_at}"#));
            let parsed = parse_client_claim(&json);
            if expires_at == 0 {
                proptest::prop_assert!(parsed.is_ok());
            } else {
                proptest::prop_assert_eq!(parsed, Err(ClientClaimError::VoucherExpires { expires_at }));
            }
        }

        /// ADR 0074 decision 3: every `uint128` amount either parses to
        /// exactly itself or is refused as out of range -- never truncated.
        #[test]
        fn a_voucher_amount_is_exact_or_refused(amount in proptest::prelude::any::<u128>()) {
            let json = evm_voucher_json().replace(r#""5000""#, &format!(r#""{amount}""#));
            match parse_client_claim(&json) {
                Ok(claim) => {
                    proptest::prop_assert!(amount <= u128::from(u64::MAX));
                    proptest::prop_assert_eq!(u128::from(claim.transferred_amount()), amount);
                }
                Err(error) => {
                    proptest::prop_assert!(amount > u128::from(u64::MAX));
                    proptest::prop_assert_eq!(error, ClientClaimError::AmountOutOfRange { amount });
                }
            }
        }
    }
}
