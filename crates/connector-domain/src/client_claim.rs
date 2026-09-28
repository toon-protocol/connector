//! Client-edge payment claim shape (`docs/protocol/client-edge-spec.md` §1.3,
//! issue #504): the JSON claim a client presents in the
//! `ILP-Payment-Channel-Claim`(`-Wrapped`) header, and its structural
//! validation.
//!
//! **Every claim is a voucher** (ADR 0075 decision 8, issue #1384): a claim
//! under x402's `batch-settlement` scheme ([`EvmVoucher`], [`SolanaVoucher`]),
//! whose field names follow x402's own voucher payloads, pinned by the wire
//! vectors (ADR 0021). The claim's `scheme` discriminator is **required**. A
//! claim with no `scheme`, or with `scheme: "toon-channel"`, is the retired
//! `toon-channel` claim (ADR 0024 and ADR 0053, both retired by ADR 0075),
//! and is refused by name as [`ClientClaimError::ToonChannel`] -- the way
//! `blockchain: "mina"` is refused as [`ClientClaimError::Mina`] -- never
//! reported as merely malformed, so a straggler learns to upgrade rather
//! than wondering why its payment failed.
//!
//! Mina is deliberately excluded from [`ClientClaim`] entirely. ADR 0002
//! drops Mina from the Rust connector, and [`parse_client_claim`] checks the
//! `blockchain` discriminator before `scheme`, so a Mina claim is refused
//! for the right reason whatever it declares.
//!
//! The same parser serves both edges: a peer carriage reads a peer's
//! voucher through it too (`connector_peer_btp::claim_json`), so a
//! `toon-channel` claim is refused by the same name on every carriage.
//!
//! Cryptographic verification and value binding against a route's price
//! are deliberately not this module's concern, so a [`ClientClaim`] carries
//! its signature and amount only in the shape they arrived in, validated
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

/// The `scheme` value of the retired `toon-channel` claim (ADR 0075):
/// refused by name, like a claim that carries no `scheme` at all.
pub const SCHEME_TOON_CHANNEL: &str = "toon-channel";

/// The `scheme` discriminator's value for an x402 `batch-settlement`
/// voucher (ADR 0074 decision 4) -- since ADR 0075, the only scheme.
pub const SCHEME_BATCH_SETTLEMENT: &str = "batch-settlement";

/// The `ChannelConfig` an EVM voucher's first presentation carries (ADR
/// 0074 decision 2): the seven immutable fields x402's `getChannelId`
/// hashes, as they arrived on the wire -- each validated for shape here and
/// decoded by the caller, which recomputes the channel id from them and
/// refuses a mismatch. The config is not readable from the chain (the
/// contract stores channels by id), so a connector learns it only from the
/// payer.
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
    /// The channel account, base58. Canonical as it arrives.
    pub channel_id: String,
    pub max_claimable_amount: u64,
    /// Ed25519, base58 of 64 bytes.
    pub signature: String,
}

/// A structurally valid claim (client-edge-spec.md §1.3): a voucher,
/// discriminated on chain the way the wire's `blockchain` field does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientClaim {
    EvmVoucher(EvmVoucher),
    SolanaVoucher(SolanaVoucher),
}

impl ClientClaim {
    /// The channel this voucher's watermark is judged against, namespaced
    /// by chain so an EVM `channelId` and a Solana channel account can never
    /// collide, and **canonical** within that namespace (issue #643) -- see
    /// [`canonical_channel_key`] for why the second is a security property
    /// rather than tidiness.
    pub fn channel_key(&self) -> String {
        match self {
            ClientClaim::EvmVoucher(voucher) => {
                format!("{EVM_NAMESPACE}:{}", canonical_evm_id(&voucher.channel_id))
            }
            ClientClaim::SolanaVoucher(voucher) => {
                format!("{SOLANA_NAMESPACE}:{}", voucher.channel_id)
            }
        }
    }

    /// The voucher's cumulative amount: its `maxClaimableAmount`.
    pub fn transferred_amount(&self) -> u64 {
        match self {
            ClientClaim::EvmVoucher(voucher) => voucher.max_claimable_amount,
            ClientClaim::SolanaVoucher(voucher) => voucher.max_claimable_amount,
        }
    }

    pub fn common(&self) -> &ClientClaimCommon {
        match self {
            ClientClaim::EvmVoucher(voucher) => &voucher.common,
            ClientClaim::SolanaVoucher(voucher) => &voucher.common,
        }
    }

    /// The voucher's self-declared sender, namespaced by chain the way
    /// [`ClientClaim::channel_key`] namespaces the channel: `evm:<lower-case
    /// senderId>` or `solana:<senderId>`.
    ///
    /// **This is a self-declared value and it is not authority for
    /// anything.** A voucher declares no signer at all -- its signer is read
    /// from the chain (ADR 0074 decision 4) -- so this is its `senderId`. It
    /// exists for one narrower purpose: to be the identity an *unresolvable*
    /// channel lookup is budgeted against (issue #613), where by definition
    /// there is no recognized channel to budget against instead. A label for
    /// grouping and attribution, not a credential. Lower-cased on EVM
    /// because hex has no case, and a budget keyed by the literal text would
    /// hand one sender a fresh allowance per recasing.
    pub fn signer_key(&self) -> String {
        match self {
            ClientClaim::EvmVoucher(voucher) => format!(
                "{EVM_NAMESPACE}:{}",
                voucher.common.sender_id.to_ascii_lowercase()
            ),
            ClientClaim::SolanaVoucher(voucher) => {
                format!("{SOLANA_NAMESPACE}:{}", voucher.common.sender_id)
            }
        }
    }

    /// The voucher's self-declared `senderId`, exactly as written --
    /// unnamespaced and uncanonicalized, unlike [`ClientClaim::signer_key`].
    /// Its one consumer is [`crate::identity::resolve_identity`]'s
    /// anonymous-sender ephemeral identity (client-edge-spec.md §1.2, issue
    /// #502). **Not authority for anything**, as [`ClientClaim::signer_key`]
    /// says.
    pub fn signer(&self) -> &str {
        &self.common().sender_id
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
/// the settlement backend resolves the channel by the *decoded* bytes, and
/// the EIP-712 digest a voucher's signature recovers
/// under is computed over those same bytes, so a claim re-presented with
/// its `channelId` recased verifies identically. Only the watermark key
/// disagreed, which handed a client 2^64 fresh, empty watermarks per
/// channel: an empty watermark admits any claim naming more than zero, so
/// one signed claim bought a write once per casing it was retyped in.
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
///   re-accepting the client's entire spend history. The deploy model is baked image tags on boxes where rolling
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
    /// A claim with no `scheme`, or with `scheme: "toon-channel"`: the
    /// retired `toon-channel` claim (ADR 0075 decision 8, issue #1384).
    /// Kept distinct from [`ClientClaimError::Malformed`] for the reason
    /// [`ClientClaimError::Mina`] is: it means "this connector no longer
    /// takes this kind of claim", not "this request made no sense".
    #[error(
        "toon-channel claims are refused: ADR 0075 retired the 'toon-channel' scheme -- every \
         claim is an x402 'batch-settlement' voucher, and a claim with no 'scheme' is a \
         toon-channel claim"
    )]
    ToonChannel,
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

/// What a `scheme` field declares (ADR 0075 decision 8): the one reading
/// of it, shared by every surface that carries one -- a claim, a
/// `POST /ilp/claim-state` entry -- so they cannot disagree about what an
/// absent scheme means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredScheme {
    /// `"batch-settlement"`: a voucher, the only scheme.
    BatchSettlement,
    /// Absent, `null` or `"toon-channel"`: the retired `toon-channel`
    /// scheme, refused by name.
    ToonChannel,
    /// Anything else.
    Unknown,
}

/// Classify a `scheme` field's value, `None` for an absent or `null` one.
pub fn declared_scheme(scheme: Option<&str>) -> DeclaredScheme {
    match scheme {
        Some(SCHEME_BATCH_SETTLEMENT) => DeclaredScheme::BatchSettlement,
        None | Some(SCHEME_TOON_CHANNEL) => DeclaredScheme::ToonChannel,
        Some(_) => DeclaredScheme::Unknown,
    }
}

/// The claim's `scheme` discriminator, **required** (ADR 0075 decision 8).
/// Absent, `null` or `"toon-channel"` is the retired `toon-channel` claim,
/// refused by name as [`ClientClaimError::ToonChannel`]; anything but
/// `"batch-settlement"` is malformed.
fn require_batch_settlement(obj: &serde_json::Map<String, Value>) -> Result<(), ClientClaimError> {
    let scheme = optional_str(obj, "scheme")?;
    match declared_scheme(scheme.as_deref()) {
        DeclaredScheme::BatchSettlement => Ok(()),
        DeclaredScheme::ToonChannel => Err(ClientClaimError::ToonChannel),
        DeclaredScheme::Unknown => Err(malformed(format!(
            "unsupported claim scheme '{}' (expected '{SCHEME_BATCH_SETTLEMENT}')",
            scheme.unwrap_or_default()
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
    match obj.get("channelConfig") {
        None | Some(Value::Null) => Ok(None),
        Some(config) => parse_evm_channel_config(config).map(Some),
    }
}

/// An EVM voucher's `channelConfig` object, on its own: the one parser of
/// that wire shape, shared by a voucher and by a claim-state entry that
/// presents a channel's config (issue #1364).
pub fn parse_evm_channel_config(
    config: &Value,
) -> Result<EvmVoucherChannelConfig, ClientClaimError> {
    let Value::Object(config) = config else {
        return Err(malformed("'channelConfig' must be an object when present"));
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
    Ok(EvmVoucherChannelConfig {
        payer: address("payer")?,
        payer_authorizer: address("payerAuthorizer")?,
        receiver: address("receiver")?,
        receiver_authorizer: address("receiverAuthorizer")?,
        token: address("token")?,
        withdraw_delay,
        salt: salt.to_string(),
    })
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

/// Parse and structurally validate a claim's JSON body (client-edge-spec.md
/// §1.3): required fields per chain, and their formats. Does not check
/// freshness, value or cryptography -- those are [`crate::validate_voucher`]
/// and the caller's signature check.
///
/// `blockchain: "mina"` is refused as [`ClientClaimError::Mina`] before
/// `scheme` is even inspected, whatever it declares; then a claim whose
/// `scheme` is absent or `"toon-channel"` is refused as
/// [`ClientClaimError::ToonChannel`] -- both before any other field is
/// read, so a straggler's claim is refused by name whatever else about it
/// has drifted, never reported as malformed.
pub fn parse_client_claim(json: &str) -> Result<ClientClaim, ClientClaimError> {
    let value: Value =
        serde_json::from_str(json).map_err(|e| ClientClaimError::InvalidJson(e.to_string()))?;
    let obj = value
        .as_object()
        .ok_or_else(|| malformed("claim must be a JSON object"))?;

    if obj.get("blockchain").and_then(Value::as_str) == Some("mina") {
        return Err(ClientClaimError::Mina);
    }
    require_batch_settlement(obj)?;
    let (blockchain, common) = parse_common(obj)?;
    match blockchain {
        "evm" => parse_evm_voucher(obj, common).map(ClientClaim::EvmVoucher),
        "solana" => parse_solana_voucher(obj, common).map(ClientClaim::SolanaVoucher),
        other => Err(malformed(format!("unsupported blockchain '{other}'"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "senderId": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1",
            "channelId": "0x{channel}",
            "maxClaimableAmount": "5000",
            "signature": "0x{signature}1b"{config}
        }}"#,
            channel = "ab".repeat(32),
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
            "senderId": "So11111111111111111111111111111111111111112",
            "channelId": "So11111111111111111111111111111111111111112",
            "maxClaimableAmount": "42",
            "expiresAt": 0,
            "signature": "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYjCJjBRnbJLgp8uirBgmQpjKhoR4tjF3ZpRzrFmBV6UjKdiSZkQUW"
        }"#
        .to_string()
    }

    /// The retired `toon-channel` claim, exactly as a client that predates
    /// ADR 0075 still sends it: no `scheme`, a nonce, and a balance proof.
    fn toon_channel_claim_json() -> String {
        format!(
            r#"{{
            "version": "1.0",
            "blockchain": "evm",
            "messageId": "claim-1",
            "timestamp": "2026-02-02T12:00:00.000Z",
            "senderId": "peer-bob",
            "channelId": "0x{channel}",
            "nonce": 5,
            "transferredAmount": "1000",
            "lockedAmount": "0",
            "locksRoot": "0x{zero}",
            "signature": "0xabcdef",
            "signerAddress": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1"
        }}"#,
            channel = "ab".repeat(32),
            zero = "0".repeat(64),
        )
    }

    /// The Solana `toon-channel` claim a pre-ADR 0075 client sends.
    fn solana_toon_channel_claim_json() -> &'static str {
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

    // -- The retired toon-channel claim (ADR 0075 decision 8, #1384) --

    /// A claim with no `scheme` is a `toon-channel` claim, and is refused by
    /// name on both chains -- never parsed, and never reported as malformed.
    #[test]
    fn a_claim_with_no_scheme_is_refused_as_toon_channel() {
        assert_eq!(
            parse_client_claim(&toon_channel_claim_json()),
            Err(ClientClaimError::ToonChannel)
        );
        assert_eq!(
            parse_client_claim(solana_toon_channel_claim_json()),
            Err(ClientClaimError::ToonChannel)
        );
    }

    /// Naming the retired scheme explicitly is refused the same way.
    #[test]
    fn an_explicit_toon_channel_scheme_is_refused_by_name() {
        let explicit = toon_channel_claim_json().replace(
            r#""blockchain": "evm","#,
            r#""blockchain": "evm", "scheme": "toon-channel","#,
        );
        assert_eq!(
            parse_client_claim(&explicit),
            Err(ClientClaimError::ToonChannel)
        );
    }

    /// A voucher that merely lost its `scheme` is refused as `toon-channel`
    /// too: the field is required, and its absence has exactly one meaning.
    #[test]
    fn a_voucher_without_its_scheme_is_refused_as_toon_channel() {
        let unschemed = evm_voucher_json().replace(r#""scheme": "batch-settlement","#, "");
        assert!(!unschemed.contains("scheme"));
        assert_eq!(
            parse_client_claim(&unschemed),
            Err(ClientClaimError::ToonChannel)
        );
    }

    /// The refusal names the retirement: a straggler reading it learns to
    /// upgrade, and it is distinguishable from a malformed claim.
    #[test]
    fn the_toon_channel_refusal_names_the_retirement() {
        let error = parse_client_claim(&toon_channel_claim_json()).unwrap_err();
        assert!(!matches!(error, ClientClaimError::Malformed(_)));
        let message = error.to_string();
        assert!(message.contains("toon-channel"), "{message}");
        assert!(message.contains("ADR 0075"), "{message}");
        assert!(message.contains("batch-settlement"), "{message}");
    }

    /// The scheme is read before any other field (bar Mina), so a
    /// straggler's claim whose other fields have drifted is still refused by
    /// name rather than reported as malformed.
    #[test]
    fn a_toon_channel_claim_is_refused_by_name_whatever_else_it_carries() {
        let drifted = toon_channel_claim_json()
            .replace(r#""version": "1.0","#, r#""version": "0.9","#)
            .replace(r#""senderId": "peer-bob","#, "");
        assert_eq!(
            parse_client_claim(&drifted),
            Err(ClientClaimError::ToonChannel)
        );
    }

    #[test]
    fn an_unknown_scheme_is_malformed() {
        let unknown =
            evm_voucher_json().replace(r#""scheme": "batch-settlement""#, r#""scheme": "exact""#);
        assert!(matches!(
            parse_client_claim(&unknown),
            Err(ClientClaimError::Malformed(_))
        ));
    }

    // -- Vouchers (ADR 0074, #1341) --

    #[test]
    fn a_well_formed_evm_voucher_parses() {
        let claim = parse_client_claim(&evm_voucher_json()).expect("parses");
        let ClientClaim::EvmVoucher(voucher) = &claim else {
            panic!("expected an EVM voucher, got {claim:?}");
        };
        assert_eq!(voucher.max_claimable_amount, 5_000);
        let config = voucher.channel_config.as_ref().expect("config present");
        assert_eq!(config.withdraw_delay, 86_400);
        assert_eq!(claim.transferred_amount(), 5_000);
        assert_eq!(claim.common().message_id, "voucher-1");
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
        let ClientClaim::SolanaVoucher(voucher) = &claim else {
            panic!("expected a Solana voucher, got {claim:?}");
        };
        assert_eq!(voucher.max_claimable_amount, 42);
    }

    #[test]
    fn channel_key_is_namespaced_by_chain() {
        assert_eq!(
            parse_client_claim(&solana_voucher_json())
                .expect("parses")
                .channel_key(),
            "solana:So11111111111111111111111111111111111111112"
        );
        assert!(parse_client_claim(&evm_voucher_json())
            .expect("parses")
            .channel_key()
            .starts_with("evm:"));
    }

    /// Issue #613: the identity an unresolvable lookup is budgeted against
    /// is namespaced by chain for the same reason the channel key is.
    #[test]
    fn a_signer_key_is_namespaced_by_chain() {
        assert!(parse_client_claim(&evm_voucher_json())
            .expect("parses")
            .signer_key()
            .starts_with("evm:"));
        assert!(parse_client_claim(&solana_voucher_json())
            .expect("parses")
            .signer_key()
            .starts_with("solana:"));
    }

    /// `signer` is the voucher's `senderId` exactly as written -- no chain
    /// namespace, no case canonicalization -- since its one consumer
    /// (`identity::resolve_identity`) formats it itself.
    #[test]
    fn signer_is_the_self_declared_sender_unnamespaced_and_uncanonicalized() {
        assert_eq!(
            parse_client_claim(&evm_voucher_json())
                .expect("parses")
                .signer(),
            "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1"
        );
    }

    /// An address's casing is notation, not identity (the rule #643
    /// established for a `channelId`): a budget keyed by the literal text
    /// would hand one sender fresh allowances for re-typing its address.
    #[test]
    fn an_evm_signer_key_is_the_same_whatever_case_its_hex_arrived_in() {
        let checksummed = parse_client_claim(&evm_voucher_json())
            .expect("parses")
            .signer_key();
        let lower = parse_client_claim(&evm_voucher_json().replace(
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

    /// The defect itself: hex has no case, so the same signed voucher
    /// retyped in a different casing must not be a different channel, or
    /// each spelling gets its own empty watermark.
    #[test]
    fn an_evm_channel_key_is_the_same_whatever_case_its_hex_arrived_in() {
        let lower = evm_voucher_json();
        let upper = lower.replace(
            &format!("0x{}", "ab".repeat(32)),
            &format!("0x{}", "AB".repeat(32)),
        );
        assert_ne!(lower, upper, "the two spellings must actually differ");
        let lower_key = parse_client_claim(&lower).expect("parses").channel_key();
        let upper_key = parse_client_claim(&upper).expect("parses").channel_key();
        assert_eq!(lower_key, upper_key);
        assert_eq!(lower_key, format!("evm:0x{}", "ab".repeat(32)));
    }

    /// Mixed casing collapses the same way.
    #[test]
    fn an_evm_channel_key_is_the_same_for_mixed_case_hex() {
        let mixed = evm_voucher_json().replace(
            &format!("0x{}", "ab".repeat(32)),
            &format!("0x{}", "aB".repeat(32)),
        );
        assert_eq!(
            parse_client_claim(&mixed).expect("parses").channel_key(),
            format!("evm:0x{}", "ab".repeat(32))
        );
    }

    /// The prefix variant: the parser requires `0x`, so a bare-hex
    /// `channelId` never reaches a watermark -- and the key is canonical
    /// over both spellings anyway, so loosening the parser can never reopen
    /// the hole.
    #[test]
    fn a_bare_hex_channel_id_is_refused_today_and_canonicalises_to_the_prefixed_key_anyway() {
        let bare = evm_voucher_json().replace(
            &format!(r#""channelId": "0x{}""#, "ab".repeat(32)),
            &format!(r#""channelId": "{}""#, "ab".repeat(32)),
        );
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

    /// **The rollback contract.** For a client sending lowercase `0x` hex,
    /// the canonical key must be byte-identical to the key a pre-#643 build
    /// derived (`format!("evm:{}", channel_id)`), or rolling a node back to
    /// one orphans every watermark this build wrote.
    #[test]
    fn the_canonical_key_is_byte_identical_to_the_key_a_pre_643_build_derived() {
        let claim = parse_client_claim(&evm_voucher_json()).expect("parses");
        let ClientClaim::EvmVoucher(voucher) = &claim else {
            panic!("expected an EVM voucher");
        };
        assert_eq!(claim.channel_key(), format!("evm:{}", voucher.channel_id));
        assert_eq!(claim.channel_key(), format!("evm:0x{}", "ab".repeat(32)));
    }

    /// Canonicalising a canonical key changes nothing -- what makes it safe
    /// on both the write and the read of a watermark.
    #[test]
    fn canonicalising_a_canonical_key_is_a_no_op() {
        let key = format!("evm:0x{}", "ab".repeat(32));
        assert_eq!(canonical_channel_key(&key), key);
        assert_eq!(canonical_channel_key(&canonical_channel_key(&key)), key);
    }

    /// Solana ids are canonical as they arrive, and base58 is
    /// case-sensitive, so they are left strictly alone.
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

    /// A key in no namespace this function knows is returned byte for byte.
    #[test]
    fn a_key_in_no_known_namespace_is_returned_untouched() {
        for key in ["channel-a", "0xABCDEF", "", "mina:B62qFoo", "evm"] {
            assert_eq!(canonical_channel_key(key), key);
        }
    }

    /// Canonicalisation merges spellings of one channel, never two channels.
    #[test]
    fn canonicalisation_never_merges_two_different_evm_channels() {
        let a = canonical_channel_key(&format!("evm:0x{}", "ab".repeat(32)));
        let b = canonical_channel_key(&format!("evm:0x{}", "cd".repeat(32)));
        let short = canonical_channel_key("evm:0xabcd");
        assert_ne!(a, b);
        assert_ne!(a, short);
        assert_eq!(short, "evm:0xabcd");
    }

    // -- Structure --

    #[test]
    fn not_json_at_all_is_invalid_json_not_malformed() {
        let err = parse_client_claim("not json").unwrap_err();
        assert!(matches!(err, ClientClaimError::InvalidJson(_)));
    }

    #[test]
    fn a_voucher_missing_a_required_field_is_malformed() {
        let without = evm_voucher_json().replace(r#""maxClaimableAmount": "5000","#, "");
        assert!(matches!(
            parse_client_claim(&without),
            Err(ClientClaimError::Malformed(_))
        ));
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
            "nonce": 1
        }"#;
        let err = parse_client_claim(json).unwrap_err();
        assert_eq!(err, ClientClaimError::Mina);
        assert_ne!(err, ClientClaimError::Malformed("anything".to_string()));
    }

    /// A Mina claim is refused by name whatever scheme it declares -- or
    /// fails to declare: Mina is checked before `scheme`.
    #[test]
    fn a_mina_claim_is_refused_as_mina_whatever_its_scheme() {
        let voucher =
            solana_voucher_json().replace(r#""blockchain": "solana""#, r#""blockchain": "mina""#);
        assert_eq!(parse_client_claim(&voucher), Err(ClientClaimError::Mina));
        let unschemed =
            toon_channel_claim_json().replace(r#""blockchain": "evm""#, r#""blockchain": "mina""#);
        assert_eq!(parse_client_claim(&unschemed), Err(ClientClaimError::Mina));
    }

    #[test]
    fn an_unsupported_blockchain_is_malformed_not_mina() {
        let json =
            evm_voucher_json().replace(r#""blockchain": "evm""#, r#""blockchain": "bitcoin""#);
        assert!(matches!(
            parse_client_claim(&json),
            Err(ClientClaimError::Malformed(_))
        ));
    }

    #[test]
    fn a_wrong_version_is_malformed() {
        let bad = evm_voucher_json().replace(r#""version": "1.0""#, r#""version": "2.0""#);
        assert!(matches!(
            parse_client_claim(&bad),
            Err(ClientClaimError::Malformed(_))
        ));
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

    proptest::proptest! {
        /// ADR 0074 decision 3: a Solana voucher that could expire is
        /// refused, whatever the expiry, and only `0` parses.
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

        /// ADR 0075 decision 8: whatever else a claim carries, no `scheme`
        /// or `"toon-channel"` is refused by name, never parsed.
        #[test]
        fn no_scheme_or_toon_channel_is_always_refused_by_name(
            explicit in proptest::prelude::any::<bool>(),
            solana in proptest::prelude::any::<bool>(),
        ) {
            let base = if solana { solana_voucher_json() } else { evm_voucher_json() };
            let replacement = if explicit { r#""scheme": "toon-channel","# } else { "" };
            let json = base.replace(r#""scheme": "batch-settlement","#, replacement);
            proptest::prop_assert_eq!(parse_client_claim(&json), Err(ClientClaimError::ToonChannel));
        }
    }
}
