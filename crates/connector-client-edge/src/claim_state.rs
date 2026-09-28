//! `POST /ilp/claim-state` (issues #693, #1364): an owner-authenticated
//! bulk read of voucher state -- the amount watermark, the ceiling the next
//! voucher is admitted against, and the last-claim time -- over every x402
//! channel a caller can prove it controls, in one request.
//!
//! **Why this shape.** This connector is the source of truth for a
//! channel's off-chain watermark: a voucher has no nonce, so a payer that
//! lost its channel store can learn the amount its next voucher must
//! strictly exceed only here -- the chain's `totalClaimed` / `settled` is a
//! floor, trailing the watermark until this node lands its latest voucher.
//! A human managing N agents needs this for every channel at once, and needs
//! it to answer correctly precisely when the agent cannot answer for itself.
//!
//! **Auth.** Per-channel, not per-request: each entry carries its own
//! signature by the channel's **voucher signer** (EVM `payerAuthorizer`,
//! Solana `authorized_signer`, read from the chain, never from the request)
//! over the voucher claim-state challenge (`connector_signer`'s
//! `ClaimStateChallenge` under the `x402BatchSettlement` domain on EVM, the
//! `toon-voucher-claim-state-challenge-v1` message on Solana) -- distinct
//! from a voucher's own signature, so neither can be replayed as the other.
//!
//! **`scheme` is required** (ADR 0075 decision 8, issue #1384): every entry
//! names `scheme: "batch-settlement"`. An entry with no `scheme`, or with
//! `scheme: "toon-channel"`, asked about a `toon-channel` channel, whose
//! scheme is retired; it is answered `"toon-channel-refused"` by name, and
//! nothing is looked up for it.
//!
//! **What a failure reveals.** Every reason a channel entry cannot be
//! answered -- it does not exist, the signature does not verify, this
//! connector's resolution of it failed -- collapses to one generic
//! `"unverified"` result: a caller learns nothing about a channel it does
//! not control. `"expired"` and `"toon-channel-refused"` are the distinct
//! reasons, because each is a fact about the caller's own request, not about
//! the channel. Every branch logs the real cause at `debug` for the
//! operator ([`log_outcome`], [`log_lookup_error`], issue #908).
//!
//! **One book answers.** Every channel this endpoint reports is judged by
//! the client edge's [`crate::ClientClaimGate`], whichever role its vouchers
//! arrive under (ADR 0075 decision 6): a peer restoring its outbound
//! watermark from here is told exactly where the channel stands.
//!
//! **The admission path is untouched.** This handler only reads, and a
//! channel lookup that is not already known goes through the same metered
//! batch-settlement lookup a voucher's does, so a flood of fabricated
//! channel ids is bounded exactly as issue #613 bounds it for vouchers.

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};

use connector_domain::client_claim::{
    declared_scheme, parse_evm_channel_config, DeclaredScheme, SCHEME_BATCH_SETTLEMENT,
};
use connector_signer::{
    evm_voucher_signer, verify_evm_voucher_claim_state_challenge,
    verify_solana_voucher_claim_state_challenge, BatchChannelConfig,
};

use crate::channels::{decode_base58_bytes, decode_hex_bytes, ChannelResolutionError};
use crate::claim_gate::decode_evm_channel_config;
use crate::{hex_encode, now_unix, ClientEdgeState};

/// The wire response for a channel entry stays the single generic
/// `"unverified"`/`"expired"` this module's doc commits to -- but the
/// underlying cause is exactly the thing an operator debugging a refused
/// channel needs (issue #908), so every branch that leads to a refusal or a
/// success logs it here, server-side only, at `debug`. `channel_id` is
/// whatever the caller sent, even when it fails to decode -- the point of
/// this line is to be legible without also being a valid channel lookup.
fn log_outcome(blockchain: &'static str, channel_id: &str, cause: &'static str) {
    tracing::debug!(
        blockchain = %blockchain,
        channel_id = %channel_id,
        cause = %cause,
        "claim-state request resolved"
    );
}

/// As [`log_outcome`], for the one refusal whose cause carries a detail of
/// its own: a lookup this connector could not complete. The variant alone is
/// not enough -- "the chain said no" and "I never got to ask" are
/// different operator problems (see [`ChannelResolutionError`]'s own doc),
/// and [`crate::channels::ChannelLookupFailed`]'s opaque string is the only
/// place the underlying reason exists -- so both go out: the variant as
/// `cause`, its `Display` as `detail`.
fn log_lookup_error(blockchain: &'static str, channel_id: &str, error: &ChannelResolutionError) {
    let cause = match error {
        ChannelResolutionError::LookupFailed(_) => "channel_lookup_failed",
        ChannelResolutionError::Budgeted(_) => "channel_lookup_budgeted",
        ChannelResolutionError::Terminal(_) => "channel_terminal",
    };
    tracing::debug!(
        blockchain = %blockchain,
        channel_id = %channel_id,
        cause = %cause,
        detail = %error,
        "claim-state request resolved"
    );
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaimStateRequest {
    channels: Vec<ChannelProofRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "blockchain", rename_all = "lowercase")]
enum ChannelProofRequest {
    #[serde(rename_all = "camelCase")]
    Evm {
        channel_id: String,
        expires: u64,
        signature: String,
        #[serde(default)]
        scheme: Option<String>,
        /// A batch-settlement channel's `ChannelConfig`, for a channel this
        /// node has no record of yet; see [`resolve_evm_voucher`].
        /// Parsed by `connector_domain`'s own `channelConfig` parser, the
        /// one a voucher's goes through.
        #[serde(default)]
        channel_config: Option<serde_json::Value>,
    },
    #[serde(rename_all = "camelCase")]
    Solana {
        channel_account: String,
        expires: u64,
        signature: String,
        #[serde(default)]
        scheme: Option<String>,
    },
}

/// The one refusal an entry is given by name rather than as
/// `"unverified"`: it asked about a retired `toon-channel` channel.
const TOON_CHANNEL_REFUSED: &str = "toon-channel-refused";

#[derive(Debug, Serialize)]
pub(crate) struct ClaimStateResponse {
    channels: Vec<ChannelStateResult>,
}

/// One requested channel's answer -- serialized flat (no enum tag) so the
/// wire shape is exactly `{"ok": true, ...state} | {"ok": false, "error":
/// "..."}`, matched on `ok` rather than a discriminant field a consumer
/// would need to know this crate's Rust type names to interpret.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ChannelStateResult {
    VerifiedVoucher(VerifiedVoucherChannelState),
    Unverified(UnverifiedChannelState),
}

/// A verified x402 `batch-settlement` channel's answer (issue #1364). A
/// voucher has no nonce, so there is none here; its watermark is an amount,
/// and the next voucher must strictly exceed `cumulativeClaimed` (ADR 0074
/// decision 3).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifiedVoucherChannelState {
    blockchain: &'static str,
    channel_id: String,
    ok: bool,
    /// Always `"batch-settlement"`: what tells a reader this entry has no
    /// `nonce`.
    scheme: &'static str,
    /// The highest cumulative amount this node has accepted a voucher for
    /// on this channel, `"0"` for none. Not the chain's `totalClaimed` /
    /// `settled`, which trails it until this node lands its latest voucher.
    cumulative_claimed: String,
    /// The highest cumulative amount a voucher may name and be accepted, as
    /// the claim gate's collateral check reads it now: the amount already
    /// landed plus what still backs a voucher above it -- EVM `balance −
    /// pendingWithdrawal`, Solana `deposit` while Open (ADR 0074 decision 5).
    /// It can fall on EVM.
    max_cumulative: String,
    /// `maxCumulative − cumulativeClaimed`, at least zero: how much the next
    /// voucher may add.
    available: String,
    /// Unix seconds this connector last accepted a voucher on this channel,
    /// or `null` if it has not since its own last restart -- best-effort and
    /// non-durable by design (see [`crate::ClientClaimGate`]'s
    /// `last_claim_seen`), unlike every other field here.
    last_claim_time: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UnverifiedChannelState {
    blockchain: &'static str,
    channel_id: String,
    ok: bool,
    /// `"expired"`, `"toon-channel-refused"` or `"unverified"` -- see this
    /// module's own doc for why nothing more specific is ever reported.
    error: &'static str,
}

fn unverified(
    blockchain: &'static str,
    channel_id: String,
    error: &'static str,
) -> ChannelStateResult {
    ChannelStateResult::Unverified(UnverifiedChannelState {
        blockchain,
        channel_id,
        ok: false,
        error,
    })
}

pub(crate) async fn claim_state(
    State(state): State<Arc<ClientEdgeState>>,
    Json(request): Json<ClaimStateRequest>,
) -> Response {
    let now = now_unix();
    let mut results = Vec::with_capacity(request.channels.len());
    for entry in request.channels {
        results.push(resolve_channel_proof(&state, entry, now).await);
    }
    Json(ClaimStateResponse { channels: results }).into_response()
}

async fn resolve_channel_proof(
    state: &ClientEdgeState,
    entry: ChannelProofRequest,
    now: u64,
) -> ChannelStateResult {
    match entry {
        ChannelProofRequest::Evm {
            channel_id,
            expires,
            signature,
            scheme,
            channel_config,
        } => {
            resolve_evm(
                state,
                channel_id,
                expires,
                signature,
                scheme,
                channel_config,
                now,
            )
            .await
        }
        ChannelProofRequest::Solana {
            channel_account,
            expires,
            signature,
            scheme,
        } => resolve_solana(state, channel_account, expires, signature, scheme, now).await,
    }
}

async fn resolve_evm(
    state: &ClientEdgeState,
    channel_id_text: String,
    expires: u64,
    signature_text: String,
    scheme: Option<String>,
    channel_config: Option<serde_json::Value>,
    now: u64,
) -> ChannelStateResult {
    match declared_scheme(scheme.as_deref()) {
        DeclaredScheme::BatchSettlement => {}
        DeclaredScheme::ToonChannel => {
            log_outcome("evm", &channel_id_text, "toon_channel_refused");
            return unverified("evm", channel_id_text, TOON_CHANNEL_REFUSED);
        }
        DeclaredScheme::Unknown => {
            log_outcome("evm", &channel_id_text, "unknown_scheme");
            return unverified("evm", channel_id_text, "unverified");
        }
    }
    if expires <= now {
        log_outcome("evm", &channel_id_text, "expired");
        return unverified("evm", channel_id_text, "expired");
    }
    let Some(channel_id) = decode_hex_bytes::<32>(&channel_id_text) else {
        log_outcome("evm", &channel_id_text, "malformed_channel_id");
        return unverified("evm", channel_id_text, "unverified");
    };
    let Some(signature) = decode_hex_bytes::<65>(&signature_text) else {
        log_outcome("evm", &channel_id_text, "malformed_signature");
        return unverified("evm", channel_id_text, "unverified");
    };

    let requester = format!("claim-state-challenge:{signature_text}");
    let presented = channel_config
        .filter(|config| !config.is_null())
        .map(|config| {
            let config = parse_evm_channel_config(&config).ok()?;
            decode_evm_channel_config(&config).ok()
        });
    let presented = match presented {
        Some(None) => {
            log_outcome("evm", &channel_id_text, "malformed_channel_config");
            return unverified("evm", channel_id_text, "unverified");
        }
        Some(Some(config)) => Some(config),
        None => None,
    };
    resolve_evm_voucher(
        state,
        channel_id_text,
        channel_id,
        VoucherProof {
            expires,
            signature: &signature,
            requester: &requester,
        },
        presented,
    )
    .await
}

async fn resolve_solana(
    state: &ClientEdgeState,
    channel_account_text: String,
    expires: u64,
    signature_text: String,
    scheme: Option<String>,
    now: u64,
) -> ChannelStateResult {
    match declared_scheme(scheme.as_deref()) {
        DeclaredScheme::BatchSettlement => {}
        DeclaredScheme::ToonChannel => {
            log_outcome("solana", &channel_account_text, "toon_channel_refused");
            return unverified("solana", channel_account_text, TOON_CHANNEL_REFUSED);
        }
        DeclaredScheme::Unknown => {
            log_outcome("solana", &channel_account_text, "unknown_scheme");
            return unverified("solana", channel_account_text, "unverified");
        }
    }
    if expires <= now {
        log_outcome("solana", &channel_account_text, "expired");
        return unverified("solana", channel_account_text, "expired");
    }
    let Some(channel_account) = decode_base58_bytes::<32>(&channel_account_text) else {
        log_outcome("solana", &channel_account_text, "malformed_channel_id");
        return unverified("solana", channel_account_text, "unverified");
    };
    let Ok(signature) = BASE64.decode(&signature_text) else {
        log_outcome("solana", &channel_account_text, "malformed_signature");
        return unverified("solana", channel_account_text, "unverified");
    };

    let requester = format!("claim-state-challenge:{signature_text}");
    resolve_solana_voucher(
        state,
        channel_account_text,
        channel_account,
        VoucherProof {
            expires,
            signature: &signature,
            requester: &requester,
        },
    )
    .await
}

/// A batch-settlement entry's already-decoded proof.
struct VoucherProof<'a> {
    expires: u64,
    signature: &'a [u8],
    requester: &'a str,
}

/// An EVM entry under `scheme: "batch-settlement"` (issue #1364): an x402
/// channel this node receives vouchers on, proved by its **voucher signer**
/// -- the verified `ChannelConfig`'s `payerAuthorizer`, else `payer` (ADR
/// 0074 decision 4) -- over the claim-state challenge under
/// `x402BatchSettlement`'s domain.
///
/// The chain stores the channel by id alone, so the config comes from this
/// node's record of the channel -- every channel it has accepted a voucher
/// on, journaled -- or, for one it has not, from the entry's own
/// `channelConfig`. Either way it must hash to `channelId`, and the signer
/// is read from what the backend admitted, never from the request. A client
/// that lost its store therefore needs only the channel id and its signing
/// key for any channel it has paid on.
async fn resolve_evm_voucher(
    state: &ClientEdgeState,
    channel_id_text: String,
    channel_id: [u8; 32],
    proof: VoucherProof<'_>,
    presented: Option<BatchChannelConfig>,
) -> ChannelStateResult {
    let lookup = state
        .claim_gate
        .evm_voucher_channel(&channel_id, presented, proof.requester)
        .await;
    let (domain, channel) = match lookup {
        Ok(Some(found)) => found,
        Ok(None) => {
            log_outcome("evm", &channel_id_text, "channel_unknown");
            return unverified("evm", channel_id_text, "unverified");
        }
        Err(error) => {
            log_lookup_error("evm", &channel_id_text, &error);
            return unverified("evm", channel_id_text, "unverified");
        }
    };
    if !verify_evm_voucher_claim_state_challenge(
        &domain,
        &channel_id,
        proof.expires,
        proof.signature,
        &evm_voucher_signer(&channel.config),
    ) {
        log_outcome("evm", &channel_id_text, "signature_invalid");
        return unverified("evm", channel_id_text, "unverified");
    }
    log_outcome("evm", &channel_id_text, "verified");
    let channel_id_hex = format!("0x{}", hex_encode(&channel_id));
    let channel_key = format!("evm:{channel_id_hex}");
    verified_voucher_state(
        "evm",
        channel_id_hex,
        state,
        &channel_key,
        channel.max_cumulative,
    )
}

/// A Solana entry under `scheme: "batch-settlement"` (issue #1364), proved
/// by the channel account's `authorized_signer`, read from the chain.
async fn resolve_solana_voucher(
    state: &ClientEdgeState,
    channel_account_text: String,
    channel_account: [u8; 32],
    proof: VoucherProof<'_>,
) -> ChannelStateResult {
    let lookup = state
        .claim_gate
        .solana_voucher_channel(&channel_account, proof.requester)
        .await;
    let channel = match lookup {
        Ok(Some(channel)) => channel,
        Ok(None) => {
            log_outcome("solana", &channel_account_text, "channel_unknown");
            return unverified("solana", channel_account_text, "unverified");
        }
        Err(error) => {
            log_lookup_error("solana", &channel_account_text, &error);
            return unverified("solana", channel_account_text, "unverified");
        }
    };
    if !verify_solana_voucher_claim_state_challenge(
        &channel_account,
        proof.expires,
        proof.signature,
        &channel.authorized_signer,
    ) {
        log_outcome("solana", &channel_account_text, "signature_invalid");
        return unverified("solana", channel_account_text, "unverified");
    }
    log_outcome("solana", &channel_account_text, "verified");
    let channel_key = format!("solana:{channel_account_text}");
    verified_voucher_state(
        "solana",
        channel_account_text,
        state,
        &channel_key,
        channel.max_cumulative,
    )
}

/// A verified voucher channel's figures (issue #1364).
///
/// The watermark is the client edge's book's: a peer's vouchers are judged
/// there too, against the channel's one watermark whichever role they
/// arrive under (ADR 0075 decision 6), so a peer restoring its outbound
/// watermark from here is told exactly where the channel stands and never
/// signs a voucher that fails to advance.
fn verified_voucher_state(
    blockchain: &'static str,
    channel_id: String,
    state: &ClientEdgeState,
    channel_key: &str,
    max_cumulative: u64,
) -> ChannelStateResult {
    let cumulative_claimed = state
        .claim_gate
        .watermark(channel_key)
        .map_or(0, |watermark| watermark.cumulative_amount);
    ChannelStateResult::VerifiedVoucher(VerifiedVoucherChannelState {
        blockchain,
        channel_id,
        ok: true,
        scheme: SCHEME_BATCH_SETTLEMENT,
        cumulative_claimed: cumulative_claimed.to_string(),
        max_cumulative: max_cumulative.to_string(),
        available: max_cumulative
            .saturating_sub(cumulative_claimed)
            .to_string(),
        last_claim_time: state.claim_gate.last_claim_time(channel_key),
    })
}
