//! `POST /ilp/claim-state` (issue #693, epic toon-meta#261): an
//! owner-authenticated bulk read of claim state -- deposit total,
//! cumulative claimed, available balance, nonce, last-claim time -- over
//! every channel a caller can prove it controls, in one request.
//!
//! **Why this shape.** The fleet-money epic's decision 5 makes this
//! connector the source of truth for a channel's off-chain claim
//! watermark: an agent's own client and this connector's claim gate are
//! the only two parties who know it, and an agent that is broke or dead
//! cannot afford to publish it itself (a report event would cost a paid
//! write). A human managing N agents needs this for every channel at
//! once, not one HTTP round trip per agent, and needs it to answer
//! correctly precisely when the agent it is asking about cannot answer
//! for itself.
//!
//! **Auth.** Per-channel, not per-request: each entry in the request
//! carries its own signature proving control of *that* channel, over a
//! domain-separated challenge (`connector_signer::claim_state_challenge`)
//! distinct from a real claim's balance-proof signature -- reusing the
//! claim signature scheme would make a captured challenge replayable as a
//! payment and vice versa. Verification is against the channel's already
//! *registered* counterparty (`ClientChannelRegistry`, issue #558's rule
//! applied to a read instead of a write) -- an owner whose agent keys
//! derive from its own seed can sign as any agent's channel this way, and
//! the connector never needs to know anything about that derivation; it
//! only ever checks "does this signature verify against this channel's
//! recorded key", exactly as claim verification does.
//!
//! **What a failure reveals.** Every reason a channel entry cannot be
//! answered -- it does not exist, the signature does not verify, this
//! connector's resolution of it failed -- collapses to one generic
//! `"unverified"` result. This is deliberately more conservative than
//! claim ingestion's own refusal taxonomy (client-edge-spec.md §1.3, which
//! *does* distinguish "no such channel" from "bad signature" for a payer's
//! benefit): this endpoint's whole acceptance criterion is that a caller
//! learns nothing about a channel it does not control, and "channel
//! exists but your signature is wrong" already tells an attacker the
//! channel exists. `"expired"` is the one distinct reason, because it is
//! a fact about the caller's own request, not about the channel.
//!
//! **What the operator sees instead (issue #908).** The wire response
//! stays this coarse deliberately, but that left the *operator* running
//! this node as blind as the caller it is protecting -- a refused channel
//! could not be diagnosed without a debugger, only reproduced. Every
//! branch below that refuses or verifies an entry logs the real cause at
//! `debug` ([`log_outcome`], [`log_lookup_error`]): a malformed field, no
//! record of the channel, the underlying [`ChannelResolutionError`]
//! (lookup failure, budget exhaustion, or a terminal channel, each with
//! its own string), a signature that does not verify, an expired
//! challenge, or success. This is exactly the distinction the wire
//! response must not make, moved to a place only this node's own operator
//! reads.
//!
//! **Which of this node's two books answers.** A connector keeps two
//! inbound books (`connector_runtime::outbound_client`'s header has the
//! table): the client edge's [`crate::ClientClaimGate`], and the peer
//! semantics's `ClaimBook`. Which one is the authority for a channel is a
//! property of the **channel**, never of who is asking -- a claim naming a
//! `[[peer_channels]]` row is judged by `ClaimBook`, and `Config::load`
//! refuses a channel that is also a `[[client_channels]]` row
//! (`ConfigError::ChannelInBothNamespaces`), so at most one book can ever
//! be the authority for one channel. So this endpoint asks
//! [`connector_runtime::Connector::peer_channel_watermark`] first and
//! falls back to the client edge's book, which is what lets a
//! `[[pay_channels]]` payer -- who holds one channel with its next hop in
//! both roles -- be told where its claims actually stand. See
//! [`verified_state`] for what went wrong when it did not.
//!
//! **The admission path is untouched.** This handler only reads --
//! [`crate::ClientClaimGate::watermark`], [`crate::ClientClaimGate::channels`],
//! [`crate::ClientClaimGate::last_claim_time`] -- and a channel lookup that
//! is not already known goes through the same budgeted
//! [`crate::channels::ClientChannelRegistry::evm`]/`::solana` resolution a
//! claim's own channel lookup does, so a flood of fabricated channel ids
//! against this endpoint is bounded exactly as issue #613 already bounds
//! it for claims. Nothing here calls `ingest`/`admit`, and no new work
//! lands on `handle_prepare`'s packet path (see #686/#690).

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};

use connector_domain::Watermark;
use connector_signer::{
    verify_evm_claim_state_challenge, verify_solana_claim_state_challenge, EvmClaimStateChallenge,
};

use crate::channels::{
    decode_base58_bytes, decode_hex_bytes, ChannelResolutionError, DepositFloor,
};
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
    },
    #[serde(rename_all = "camelCase")]
    Solana {
        channel_account: String,
        expires: u64,
        signature: String,
    },
}

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
    Verified(VerifiedChannelState),
    Unverified(UnverifiedChannelState),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifiedChannelState {
    blockchain: &'static str,
    channel_id: String,
    ok: bool,
    /// `null` for a declared (`[[client_channels]]`) channel, which names
    /// no amount ([`DepositFloor::Unknown`]) -- see
    /// `crate::channels`'s own doc for why that is a deliberate exemption,
    /// not a gap. A decimal string, matching the incoming claim wire's own
    /// `transferredAmount` convention, since this is a monetary value a
    /// JS `Number` cannot represent exactly past 2^53.
    deposit_total: Option<String>,
    cumulative_claimed: String,
    /// `depositTotal - cumulativeClaimed + credited` (issue #700's netting:
    /// `credited` is what this connector has separately committed to pay
    /// this channel's counterparty back, e.g. for factory work it earned --
    /// `0` for a channel nothing has been paid out on). This is the same
    /// spendable headroom figure the collateral-binding check in
    /// `client-edge-spec.md` §1.3 step 5 admits an inbound claim against,
    /// not a raw on-chain balance -- an agent that has earned enough sees
    /// its own runway rise here without any settlement having happened.
    /// `null` exactly when `depositTotal` is, for the same reason.
    available: Option<String>,
    nonce: u64,
    /// Unix seconds this connector last accepted a claim on this channel,
    /// or `null` if it has not (or not since its own last restart -- see
    /// [`crate::ClientClaimGate`]'s `last_claim_seen` doc: this figure is
    /// best-effort and non-durable by design, unlike every other field
    /// here).
    last_claim_time: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UnverifiedChannelState {
    blockchain: &'static str,
    channel_id: String,
    ok: bool,
    /// `"expired"` or `"unverified"` -- see this module's own doc for why
    /// nothing more specific is ever reported.
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
        } => resolve_evm(state, channel_id, expires, signature, now).await,
        ChannelProofRequest::Solana {
            channel_account,
            expires,
            signature,
        } => resolve_solana(state, channel_account, expires, signature, now).await,
    }
}

async fn resolve_evm(
    state: &ClientEdgeState,
    channel_id_text: String,
    expires: u64,
    signature_text: String,
    now: u64,
) -> ChannelStateResult {
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
    let lookup = state
        .claim_gate
        .channels()
        .evm(&channel_id, &requester)
        .await;
    let channel = match lookup {
        Ok(Some(channel)) => channel,
        Ok(None) => {
            log_outcome("evm", &channel_id_text, "channel_unknown");
            return unverified("evm", channel_id_text, "unverified");
        }
        Err(error) => {
            log_lookup_error("evm", &channel_id_text, &error);
            return unverified("evm", channel_id_text, "unverified");
        }
    };

    let challenge = EvmClaimStateChallenge {
        channel_id,
        expires,
        chain_id: channel.chain_id,
        token_network_address: channel.token_network_address,
    };
    if !verify_evm_claim_state_challenge(&challenge, &signature, &channel.counterparty) {
        log_outcome("evm", &channel_id_text, "signature_invalid");
        return unverified("evm", channel_id_text, "unverified");
    }
    log_outcome("evm", &channel_id_text, "verified");

    let channel_id_hex = format!("0x{}", hex_encode(&channel_id));
    let channel_key = format!("evm:{channel_id_hex}");
    let peer_watermark = state.connector.peer_channel_watermark(&channel_id_hex);
    ChannelStateResult::Verified(verified_state(
        "evm",
        channel_id_hex,
        state,
        &channel_key,
        peer_watermark,
        channel.deposit_floor,
        state.claim_gate.credited_evm(&channel_id),
    ))
}

async fn resolve_solana(
    state: &ClientEdgeState,
    channel_account_text: String,
    expires: u64,
    signature_text: String,
    now: u64,
) -> ChannelStateResult {
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
    let lookup = state
        .claim_gate
        .channels()
        .solana(&channel_account, &requester)
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

    if !verify_solana_claim_state_challenge(
        &channel_account,
        expires,
        &signature,
        &channel.counterparty,
    ) {
        log_outcome("solana", &channel_account_text, "signature_invalid");
        return unverified("solana", channel_account_text, "unverified");
    }
    log_outcome("solana", &channel_account_text, "verified");

    let channel_key = format!("solana:{channel_account_text}");
    let peer_watermark = state
        .connector
        .peer_channel_watermark(&channel_account_text);
    ChannelStateResult::Verified(verified_state(
        "solana",
        channel_account_text,
        state,
        &channel_key,
        peer_watermark,
        channel.deposit_floor,
        // `ClientPayoutLedger` only ever signs an EVM balance proof (issue
        // #699) -- a Solana channel has no credited amount to net yet, see
        // `ClientClaimGate::credited`'s own doc.
        0,
    ))
}

/// `deposit_total`, `cumulativeClaimed`, `nonce` and the netted
/// `available` this endpoint reports for one verified channel (issue
/// #700, `client-edge-spec.md` §1.10): `available` is the same spendable
/// headroom [`crate::claim_gate::ClientClaimGate`]'s collateral check
/// admits against -- `deposit - owed + credited`, `owed` being
/// `cumulative_claimed` below -- so a fleet dashboard reads one number
/// that already reflects an agent's own earnings, not a raw on-chain
/// balance a human would have to net by hand.
///
/// # Which book the watermark comes from
///
/// `peer_watermark` is
/// [`connector_runtime::Connector::peer_channel_watermark`]'s answer:
/// `Some` exactly when this node holds the channel as a
/// `[[peer_channels]]` row, in which case that book -- and not the client
/// edge's -- is the one every claim on the channel is judged against, so it
/// is the one this endpoint has to report.
///
/// Answering every channel out of the client edge's book was a measured
/// failure, not a theoretical one. `[[pay_channels]]` (ADR 0042 item 2)
/// makes a forwarding node ask this endpoint, on every covered packet, for
/// the state of a channel it holds with its next hop in **both** roles --
/// `connector_config`'s `pay_channel` module calls that "the deployed
/// shape" -- and `OutboundClientLedger::next_claim` signs
/// `cumulative + amount` at `max(nonce, issued_floor) + 1` from the answer.
/// A client-edge book no peer claim ever reaches answers nonce 0 /
/// cumulative 0 forever, so the payer re-signs the same cumulative amount
/// at a fresh nonce on every packet: the payee accepts each one (a nonce
/// did advance) and each one advances nothing, and a priced peer
/// termination refuses every packet after the first with `F06`,
/// `advanced = 0`.
///
/// `last_claim_time` stays the client edge's. The peer book journals no
/// timestamp on `InboundClaimAccepted`, so a peer channel reports `null`
/// there -- which that field's own doc already admits as a best-effort,
/// non-durable answer, and which no payer reads.
fn verified_state(
    blockchain: &'static str,
    channel_id: String,
    state: &ClientEdgeState,
    channel_key: &str,
    peer_watermark: Option<Watermark>,
    deposit_floor: DepositFloor,
    credited: u64,
) -> VerifiedChannelState {
    let watermark = peer_watermark.or_else(|| state.claim_gate.watermark(channel_key));
    let cumulative_claimed = watermark.map(|w| w.cumulative_amount).unwrap_or(0);
    let nonce = watermark.map(|w| w.nonce).unwrap_or(0);
    let deposit_total = deposit_floor.deposit();
    let available = deposit_total.map(|deposit| {
        deposit
            .saturating_add(credited)
            .saturating_sub(cumulative_claimed)
    });

    VerifiedChannelState {
        blockchain,
        channel_id,
        ok: true,
        deposit_total: deposit_total.map(|amount| amount.to_string()),
        cumulative_claimed: cumulative_claimed.to_string(),
        available: available.map(|amount| amount.to_string()),
        nonce,
        last_claim_time: state.claim_gate.last_claim_time(channel_key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use chrono::{TimeZone, Utc};
    use connector_domain::Prepare;
    use connector_runtime::{
        ChannelDomain, ClaimAckOutcome, ClaimSignature, Connector, FakeAppClient, InMemoryJournal,
        InProcessPeerTransport, TestClock, WireClaim,
    };
    use connector_signer::{
        evm_claim_state_challenge_digest, solana_claim_state_challenge_message, LocalSigner, Signer,
    };
    use ed25519_dalek::Signer as Ed25519Signer;
    use libsecp256k1::{Message, PublicKey, SecretKey};
    use rand::SeedableRng;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tower::ServiceExt;

    use crate::channels::test_source::FakeChannelSource;
    use crate::{
        router_with_gate, ChannelLookupFailed, ClientChannelRegistry, ClientClaimGate,
        ClientPayoutLedger, EvmChannel,
    };

    const EVM_CHAIN_ID: u64 = 8453;
    const EVM_TOKEN_NETWORK_ADDRESS: [u8; 20] = [0x42; 20];
    const EVM_CHANNEL_ID: [u8; 32] = [0xab; 32];
    const KNOWN_DEPOSIT: u64 = 1_000_000;
    const SOLANA_CHANNEL_ACCOUNT: [u8; 32] = [3u8; 32];

    fn evm_channel_id_hex() -> String {
        format!("0x{}", hex_encode(&EVM_CHANNEL_ID))
    }

    fn evm_signer() -> (SecretKey, connector_signer::Address) {
        let secret = SecretKey::parse(&[9u8; 32]).unwrap();
        let public = PublicKey::from_secret_key(&secret);
        (
            secret,
            connector_signer::derive_evm_address(&public.serialize()),
        )
    }

    fn sign_evm(secret: &SecretKey, digest: &[u8; 32]) -> Vec<u8> {
        let message = Message::parse(digest);
        let (signature, recovery_id) = libsecp256k1::sign(&message, secret);
        let mut bytes = signature.serialize().to_vec();
        let recovery_byte: u8 = recovery_id.into();
        bytes.push(recovery_byte + 27);
        bytes
    }

    fn evm_challenge_signature(secret: &SecretKey, channel_id: [u8; 32], expires: u64) -> String {
        let challenge = EvmClaimStateChallenge {
            channel_id,
            expires,
            chain_id: EVM_CHAIN_ID,
            token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
        };
        let digest = evm_claim_state_challenge_digest(&challenge);
        format!("0x{}", hex_encode(&sign_evm(secret, &digest)))
    }

    fn solana_signer() -> ed25519_dalek::Keypair {
        let mut rng = rand::rngs::StdRng::from_seed([13u8; 32]);
        ed25519_dalek::Keypair::generate(&mut rng)
    }

    fn base58_encode(bytes: &[u8]) -> String {
        bs58::encode(bytes).into_string()
    }

    fn solana_challenge_signature(keypair: &ed25519_dalek::Keypair, expires: u64) -> String {
        let message = solana_claim_state_challenge_message(&SOLANA_CHANNEL_ACCOUNT, expires);
        let signature = keypair.sign(&message);
        BASE64.encode(signature.to_bytes())
    }

    /// A registry with one EVM channel resolved (not declared) with a known
    /// deposit, via a [`FakeChannelSource`] -- `record_evm` alone always
    /// leaves [`DepositFloor::Unknown`] (a declared channel names no
    /// amount), so exercising `depositTotal`/`available` as real numbers
    /// needs the resolution path a `[settlement]`-backed node actually
    /// uses. Also declares one Solana channel with the usual "no deposit
    /// knowable" shape, for the tests that only care about the signature
    /// and watermark halves.
    fn test_channels() -> ClientChannelRegistry {
        let (_secret, address) = evm_signer();
        let source = FakeChannelSource::knowing(vec![(
            EVM_CHANNEL_ID,
            EvmChannel {
                counterparty: address,
                chain_id: EVM_CHAIN_ID,
                token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                deposit_floor: DepositFloor::AtLeast(KNOWN_DEPOSIT),
            },
        )]);
        let mut channels = ClientChannelRegistry::new().with_source(Arc::new(source));
        channels
            .record_solana(
                &base58_encode(&SOLANA_CHANNEL_ACCOUNT),
                &base58_encode(&solana_signer().public.to_bytes()),
                "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx",
            )
            .expect("a 32-byte base58 channel account");
        channels
    }

    fn test_gate() -> ClientClaimGate {
        ClientClaimGate::restore(test_channels(), Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay")
    }

    fn test_signer() -> Arc<dyn Signer> {
        Arc::new(LocalSigner::generate("test-signer"))
    }

    fn test_connector() -> Arc<Connector> {
        Arc::new(Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            Arc::new(TestClock::new(
                Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
            )),
        ))
    }

    fn far_future_expiry() -> u64 {
        Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp() as u64
    }

    fn long_past_expiry() -> u64 {
        Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp() as u64
    }

    async fn post_claim_state(gate: ClientClaimGate, body: serde_json::Value) -> serde_json::Value {
        let app = router_with_gate(test_connector(), test_signer(), None, gate);
        let request = Request::builder()
            .method("POST")
            .uri("/ilp/claim-state")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        serde_json::from_slice(&bytes).expect("valid JSON response")
    }

    /// Issue #908: this module's own two logging call sites
    /// ([`log_outcome`], [`log_lookup_error`]) never appear on the wire, so
    /// asserting they fire -- and with which cause -- needs a `Subscriber`
    /// installed for the duration of a request, exactly like
    /// `connector_runtime::connector`'s `SpanFieldCapture` does for the
    /// `"packet"` span, adapted to events instead.
    ///
    /// Only this module's own events are kept: `enabled` has to answer
    /// `true` for everything (see [`claim_state_events`] on why a narrower
    /// answer is not race-free), which otherwise leaves the assertions
    /// below counting whatever axum, tower or hyper happens to log on the
    /// same thread during the request.
    struct EventFieldCapture {
        events: Arc<Mutex<Vec<HashMap<String, String>>>>,
    }

    /// `tracing`'s default target -- the module path of the
    /// [`log_outcome`]/[`log_lookup_error`] call sites, which are in the
    /// parent module, not in `tests`.
    const CLAIM_STATE_TARGET: &str = "connector_client_edge::claim_state";

    struct StringVisitor<'a>(&'a mut HashMap<String, String>);

    impl tracing::field::Visit for StringVisitor<'_> {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0
                .insert(field.name().to_string(), format!("{value:?}"));
        }
    }

    impl tracing::Subscriber for EventFieldCapture {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            if event.metadata().target() != CLAIM_STATE_TARGET {
                return;
            }
            let mut fields = HashMap::new();
            let mut visitor = StringVisitor(&mut fields);
            event.record(&mut visitor);
            self.events
                .lock()
                .expect("capture lock poisoned")
                .push(fields);
        }

        fn enter(&self, _span: &tracing::span::Id) {}
        fn exit(&self, _span: &tracing::span::Id) {}
    }

    /// Run `work` under an [`EventFieldCapture`] and return every event
    /// this module's logging recorded, in `work`'s own order, alongside
    /// `work`'s result.
    ///
    /// Probes both [`log_outcome`] and [`log_lookup_error`] -- this
    /// module's only two logging call sites, however many distinct
    /// `cause`s flow through them at runtime -- and rebuilds tracing-core's
    /// callsite interest cache until the probes are observably captured.
    /// A single rebuild is not race-free under the default parallel
    /// `cargo test`: each call site's `Interest` is cached globally,
    /// computed once by whichever thread touches it first, and a
    /// concurrent test elsewhere in this file that never installs a
    /// subscriber can be that first toucher, caching `Interest::never`
    /// before this capture ever runs. See
    /// `connector_runtime::connector`'s `packet_span_fields` for the fuller
    /// account of why probing until the capture actually fires is the only
    /// race-free fix.
    async fn claim_state_events<T, F: std::future::Future<Output = T>>(
        work: F,
    ) -> (Vec<HashMap<String, String>>, T) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let guard = tracing::subscriber::set_default(EventFieldCapture {
            events: Arc::clone(&events),
        });
        let mut attempts = 0u32;
        loop {
            tracing::callsite::rebuild_interest_cache();
            log_outcome("evm", "probe", "probe");
            log_lookup_error(
                "evm",
                "probe",
                &ChannelResolutionError::LookupFailed(ChannelLookupFailed("probe".to_string())),
            );
            if events.lock().expect("capture lock poisoned").len() >= 2 {
                events.lock().expect("capture lock poisoned").clear();
                break;
            }
            attempts += 1;
            assert!(
                attempts < 1_000,
                "the claim-state log call sites never became enabled under this capture"
            );
            std::thread::yield_now();
        }

        let result = work.await;

        drop(guard);
        let captured = events.lock().expect("capture lock poisoned").clone();
        (captured, result)
    }

    #[tokio::test]
    async fn a_verified_evm_channel_reports_deposit_cumulative_available_and_nonce() {
        let (secret, _address) = evm_signer();
        let expires = far_future_expiry();
        let body = serde_json::json!({
            "channels": [{
                "blockchain": "evm",
                "channelId": evm_channel_id_hex(),
                "expires": expires,
                "signature": evm_challenge_signature(&secret, EVM_CHANNEL_ID, expires),
            }]
        });

        let response = post_claim_state(test_gate(), body).await;
        let entry = &response["channels"][0];
        assert_eq!(entry["ok"], true);
        assert_eq!(entry["blockchain"], "evm");
        assert_eq!(entry["depositTotal"], KNOWN_DEPOSIT.to_string());
        assert_eq!(entry["cumulativeClaimed"], "0");
        assert_eq!(entry["available"], KNOWN_DEPOSIT.to_string());
        assert_eq!(entry["nonce"], 0);
        assert!(entry["lastClaimTime"].is_null());
    }

    /// A ledger crediting `channel_id` for `amount` -- the netting
    /// counterpart to the payments `test_channels`/`test_gate` already
    /// simulate on the inbound side (issue #700).
    fn payout_ledger_crediting(channel_id: &str, amount: u64) -> Arc<ClientPayoutLedger> {
        let mut ledger = ClientPayoutLedger::new();
        ledger.set_signer(Arc::new(LocalSigner::generate("payout-key")));
        ledger
            .set_channel_domain(
                channel_id,
                connector_runtime::ChannelDomain {
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                },
            )
            .expect("test channel id is valid");
        let ledger = Arc::new(ledger);
        ledger
            .record_payout(
                channel_id,
                amount,
                Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
            )
            .expect("signer and domain configured");
        ledger
    }

    #[tokio::test]
    async fn a_channel_credited_by_the_payout_ledger_reports_available_raised_by_it() {
        let (secret, _address) = evm_signer();
        let ledger = payout_ledger_crediting(&evm_channel_id_hex(), 300_000);
        let gate = test_gate().with_payout_ledger(ledger);
        let expires = far_future_expiry();
        let body = serde_json::json!({
            "channels": [{
                "blockchain": "evm",
                "channelId": evm_channel_id_hex(),
                "expires": expires,
                "signature": evm_challenge_signature(&secret, EVM_CHANNEL_ID, expires),
            }]
        });

        let response = post_claim_state(gate, body).await;
        let entry = &response["channels"][0];
        assert_eq!(entry["ok"], true);
        assert_eq!(entry["depositTotal"], KNOWN_DEPOSIT.to_string());
        assert_eq!(entry["cumulativeClaimed"], "0");
        // deposit - owed(0) + credited: strictly more than the raw
        // on-chain deposit alone, which is the entire point of issue #700.
        assert_eq!(entry["available"], (KNOWN_DEPOSIT + 300_000).to_string());
    }

    #[tokio::test]
    async fn a_declared_solana_channel_reports_null_deposit_and_available() {
        let keypair = solana_signer();
        let expires = far_future_expiry();
        let body = serde_json::json!({
            "channels": [{
                "blockchain": "solana",
                "channelAccount": base58_encode(&SOLANA_CHANNEL_ACCOUNT),
                "expires": expires,
                "signature": solana_challenge_signature(&keypair, expires),
            }]
        });

        let response = post_claim_state(test_gate(), body).await;
        let entry = &response["channels"][0];
        assert_eq!(entry["ok"], true);
        assert_eq!(entry["blockchain"], "solana");
        assert!(entry["depositTotal"].is_null());
        assert_eq!(entry["cumulativeClaimed"], "0");
        assert!(entry["available"].is_null());
        assert_eq!(entry["nonce"], 0);
    }

    #[tokio::test]
    async fn a_wrong_signature_and_an_unknown_channel_report_the_identical_generic_error() {
        let expires = far_future_expiry();
        // A channel this registry knows about, but signed by the wrong key.
        let forger_secret = SecretKey::parse(&[42u8; 32]).unwrap();
        let wrong_signature = evm_challenge_signature(&forger_secret, EVM_CHANNEL_ID, expires);

        // A channel this registry has never heard of, with a well-formed
        // but meaningless signature.
        let unknown_channel_id = [0x99u8; 32];
        let unknown_channel_signature =
            evm_challenge_signature(&forger_secret, unknown_channel_id, expires);

        let body = serde_json::json!({
            "channels": [
                {
                    "blockchain": "evm",
                    "channelId": evm_channel_id_hex(),
                    "expires": expires,
                    "signature": wrong_signature,
                },
                {
                    "blockchain": "evm",
                    "channelId": format!("0x{}", hex_encode(&unknown_channel_id)),
                    "expires": expires,
                    "signature": unknown_channel_signature,
                },
            ]
        });

        let response = post_claim_state(test_gate(), body).await;
        for index in 0..2 {
            let entry = &response["channels"][index];
            assert_eq!(entry["ok"], false);
            assert_eq!(entry["error"], "unverified");
            // Confirms the two failures are byte-identical shapes -- a
            // caller cannot tell "wrong key" from "no such channel" apart.
            assert_eq!(entry.as_object().unwrap().len(), 4);
        }
    }

    /// Issue #908: the wire response for a bad signature and an unknown
    /// channel is the identical `"unverified"` shape (proven above), but an
    /// operator reading this node's own logs must be able to tell them
    /// apart -- that is the whole point of the issue. Same two requests as
    /// the wire-shape test above; this one asserts on what got logged
    /// instead of what got answered.
    #[tokio::test]
    async fn an_unknown_channel_and_a_bad_signature_are_logged_with_different_causes() {
        let expires = far_future_expiry();
        let forger_secret = SecretKey::parse(&[42u8; 32]).unwrap();
        let wrong_signature = evm_challenge_signature(&forger_secret, EVM_CHANNEL_ID, expires);

        let unknown_channel_id = [0x99u8; 32];
        let unknown_channel_signature =
            evm_challenge_signature(&forger_secret, unknown_channel_id, expires);

        let body = serde_json::json!({
            "channels": [
                {
                    "blockchain": "evm",
                    "channelId": evm_channel_id_hex(),
                    "expires": expires,
                    "signature": wrong_signature,
                },
                {
                    "blockchain": "evm",
                    "channelId": format!("0x{}", hex_encode(&unknown_channel_id)),
                    "expires": expires,
                    "signature": unknown_channel_signature,
                },
            ]
        });

        let (events, response) = claim_state_events(post_claim_state(test_gate(), body)).await;

        for index in 0..2 {
            assert_eq!(response["channels"][index]["error"], "unverified");
        }
        let causes: Vec<Option<&str>> = events
            .iter()
            .map(|fields| fields.get("cause").map(String::as_str))
            .collect();
        assert_eq!(
            causes,
            vec![Some("signature_invalid"), Some("channel_unknown")],
            "the two identical wire refusals must be distinguishable in the log"
        );
    }

    /// Issue #908: a lookup this connector could not complete (an
    /// unreachable settlement RPC, say) is not the same event as a channel
    /// this connector has simply never heard of -- both answer the wire the
    /// identical `"unverified"`, but the log must carry the underlying
    /// [`crate::channels::ChannelLookupFailed`] reason, not just the fact
    /// that *something* failed.
    #[tokio::test]
    async fn a_channel_lookup_failure_is_logged_with_its_underlying_reason() {
        let source = FakeChannelSource::unreachable("settlement rpc unreachable");
        let channels = ClientChannelRegistry::new().with_source(Arc::new(source));
        let gate = ClientClaimGate::restore(channels, Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay");

        let expires = far_future_expiry();
        let body = serde_json::json!({
            "channels": [{
                "blockchain": "evm",
                "channelId": evm_channel_id_hex(),
                "expires": expires,
                "signature": format!("0x{}", "11".repeat(65)),
            }]
        });

        let (events, response) = claim_state_events(post_claim_state(gate, body)).await;

        assert_eq!(response["channels"][0]["ok"], false);
        assert_eq!(response["channels"][0]["error"], "unverified");

        let event = events
            .iter()
            .find(|fields| fields.get("cause").map(String::as_str) == Some("channel_lookup_failed"))
            .expect("a channel_lookup_failed event");
        assert_eq!(
            event.get("detail").map(String::as_str),
            Some("settlement rpc unreachable")
        );
    }

    /// Issue #908: a settled channel is a known, definitive fact (issue
    /// #661) -- distinct in the log from both an unknown channel and a
    /// bare lookup failure, the same way [`crate::ClaimIngestRejection`]
    /// keeps it distinct on the claim-ingestion path.
    #[tokio::test]
    async fn a_terminal_channel_is_logged_distinctly_from_an_unknown_one() {
        let source = FakeChannelSource::knowing(vec![]);
        source.now_terminal(EVM_CHANNEL_ID);
        let channels = ClientChannelRegistry::new().with_source(Arc::new(source));
        let gate = ClientClaimGate::restore(channels, Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay");

        let expires = far_future_expiry();
        let body = serde_json::json!({
            "channels": [{
                "blockchain": "evm",
                "channelId": evm_channel_id_hex(),
                "expires": expires,
                "signature": format!("0x{}", "11".repeat(65)),
            }]
        });

        let (events, response) = claim_state_events(post_claim_state(gate, body)).await;

        assert_eq!(response["channels"][0]["error"], "unverified");
        let event = events
            .iter()
            .find(|fields| fields.get("cause").map(String::as_str) == Some("channel_terminal"))
            .expect("a channel_terminal event");
        assert!(event
            .get("detail")
            .is_some_and(|detail| detail.contains("settled")));
    }

    /// Issue #908: malformed input (an unparseable channel id or
    /// signature) never reaches a channel lookup at all, and the log line
    /// says so distinctly from both "no such channel" and "bad signature".
    #[tokio::test]
    async fn malformed_fields_are_logged_before_any_lookup_is_attempted() {
        let body = serde_json::json!({
            "channels": [
                {
                    "blockchain": "evm",
                    "channelId": "not-hex",
                    "expires": far_future_expiry(),
                    "signature": format!("0x{}", "11".repeat(65)),
                },
                {
                    "blockchain": "evm",
                    "channelId": evm_channel_id_hex(),
                    "expires": far_future_expiry(),
                    "signature": "not-hex-either",
                },
            ]
        });

        let (events, response) = claim_state_events(post_claim_state(test_gate(), body)).await;

        for index in 0..2 {
            assert_eq!(response["channels"][index]["error"], "unverified");
        }
        let causes: Vec<Option<&str>> = events
            .iter()
            .map(|fields| fields.get("cause").map(String::as_str))
            .collect();
        assert_eq!(
            causes,
            vec![Some("malformed_channel_id"), Some("malformed_signature")]
        );
    }

    /// Issue #908: a successful verification logs too -- "nothing happened"
    /// and "a request I cannot explain happened" must not look the same in
    /// the log.
    #[tokio::test]
    async fn a_verified_channel_is_logged_as_verified() {
        let (secret, _address) = evm_signer();
        let expires = far_future_expiry();
        let body = serde_json::json!({
            "channels": [{
                "blockchain": "evm",
                "channelId": evm_channel_id_hex(),
                "expires": expires,
                "signature": evm_challenge_signature(&secret, EVM_CHANNEL_ID, expires),
            }]
        });

        let (events, response) = claim_state_events(post_claim_state(test_gate(), body)).await;

        assert_eq!(response["channels"][0]["ok"], true);
        let causes: Vec<Option<&str>> = events
            .iter()
            .map(|fields| fields.get("cause").map(String::as_str))
            .collect();
        assert_eq!(causes, vec![Some("verified")]);
    }

    /// Issue #908: the Solana branch refuses on the same generic
    /// `"unverified"` as the EVM one, so it owes the operator the same
    /// distinguishing log line -- and the line is only actionable if it also
    /// says *which* chain and *which* channel, which is the half of the
    /// issue's ask ("the channel id and the branch taken") the
    /// cause-only assertions above do not pin down.
    #[tokio::test]
    async fn a_bad_solana_signature_is_logged_with_its_chain_and_channel() {
        let channel_account = base58_encode(&SOLANA_CHANNEL_ACCOUNT);
        let body = serde_json::json!({
            "channels": [{
                "blockchain": "solana",
                "channelAccount": channel_account,
                "expires": far_future_expiry(),
                "signature": BASE64.encode([7u8; 64]),
            }]
        });

        let (events, response) = claim_state_events(post_claim_state(test_gate(), body)).await;

        assert_eq!(response["channels"][0]["error"], "unverified");
        let logged: Vec<(Option<&str>, Option<&str>, Option<&str>)> = events
            .iter()
            .map(|fields| {
                (
                    fields.get("blockchain").map(String::as_str),
                    fields.get("channel_id").map(String::as_str),
                    fields.get("cause").map(String::as_str),
                )
            })
            .collect();
        assert_eq!(
            logged,
            vec![(
                Some("solana"),
                Some(channel_account.as_str()),
                Some("signature_invalid")
            )]
        );
    }

    #[tokio::test]
    async fn an_expired_challenge_is_refused_distinctly_from_an_unverified_one() {
        let (secret, _address) = evm_signer();
        let expires = long_past_expiry();
        let body = serde_json::json!({
            "channels": [{
                "blockchain": "evm",
                "channelId": evm_channel_id_hex(),
                "expires": expires,
                "signature": evm_challenge_signature(&secret, EVM_CHANNEL_ID, expires),
            }]
        });

        let response = post_claim_state(test_gate(), body).await;
        let entry = &response["channels"][0];
        assert_eq!(entry["ok"], false);
        assert_eq!(entry["error"], "expired");
    }

    #[tokio::test]
    async fn a_batch_of_several_channels_is_resolved_independently_and_in_order() {
        let (secret, _address) = evm_signer();
        let keypair = solana_signer();
        let expires = far_future_expiry();
        let forger_secret = SecretKey::parse(&[42u8; 32]).unwrap();

        let body = serde_json::json!({
            "channels": [
                {
                    "blockchain": "evm",
                    "channelId": evm_channel_id_hex(),
                    "expires": expires,
                    "signature": evm_challenge_signature(&secret, EVM_CHANNEL_ID, expires),
                },
                {
                    "blockchain": "solana",
                    "channelAccount": base58_encode(&SOLANA_CHANNEL_ACCOUNT),
                    "expires": expires,
                    "signature": solana_challenge_signature(&keypair, expires),
                },
                {
                    "blockchain": "evm",
                    "channelId": evm_channel_id_hex(),
                    "expires": expires,
                    "signature": evm_challenge_signature(&forger_secret, EVM_CHANNEL_ID, expires),
                },
            ]
        });

        let response = post_claim_state(test_gate(), body).await;
        let channels = response["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 3);
        assert_eq!(channels[0]["ok"], true);
        assert_eq!(channels[0]["blockchain"], "evm");
        assert_eq!(channels[1]["ok"], true);
        assert_eq!(channels[1]["blockchain"], "solana");
        assert_eq!(channels[2]["ok"], false);
        assert_eq!(channels[2]["error"], "unverified");
    }

    #[tokio::test]
    async fn a_real_claim_updates_cumulative_claimed_nonce_and_last_claim_time() {
        let (secret, address) = evm_signer();
        let gate = test_gate();
        let balance_proof = connector_signer::EvmBalanceProof {
            channel_id: EVM_CHANNEL_ID,
            nonce: 1,
            transferred_amount: 500,
            locked_amount: 0,
            locks_root: [0u8; 32],
            chain_id: EVM_CHAIN_ID,
            token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
        };
        let balance_proof_digest = connector_signer::evm_balance_proof_digest(&balance_proof);
        let balance_proof_signature = format!(
            "0x{}",
            hex_encode(&sign_evm(&secret, &balance_proof_digest))
        );
        let claim_json = serde_json::json!({
            "version": "1.0",
            "blockchain": "evm",
            "messageId": "m1",
            "timestamp": "2030-01-01T00:00:00Z",
            "senderId": "sender",
            "channelId": evm_channel_id_hex(),
            "nonce": 1,
            "transferredAmount": "500",
            "lockedAmount": "0",
            "locksRoot": format!("0x{}", "0".repeat(64)),
            "signature": balance_proof_signature,
            "signerAddress": format!("0x{}", hex_encode(&address)),
            "chainId": EVM_CHAIN_ID,
            "tokenNetworkAddress": format!("0x{}", hex_encode(&EVM_TOKEN_NETWORK_ADDRESS)),
        })
        .to_string();

        let connector = test_connector();
        let signer = test_signer();
        let app = router_with_gate(Arc::clone(&connector), Arc::clone(&signer), None, gate);

        let prepare = Prepare {
            amount: 0,
            expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
            greeting: false,
            destination: "g.nowhere".to_string(),
            data: Vec::new(),
        };
        let request = Request::builder()
            .method("POST")
            .uri("/ilp")
            .header(crate::CLAIM_HEADER, BASE64.encode(claim_json))
            .body(Body::from(prepare.encode()))
            .unwrap();
        let before = now_unix();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let expires = far_future_expiry();
        let body = serde_json::json!({
            "channels": [{
                "blockchain": "evm",
                "channelId": evm_channel_id_hex(),
                "expires": expires,
                "signature": evm_challenge_signature(&secret, EVM_CHANNEL_ID, expires),
            }]
        });
        let request = Request::builder()
            .method("POST")
            .uri("/ilp/claim-state")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        let entry = &response["channels"][0];
        assert_eq!(entry["ok"], true);
        assert_eq!(entry["nonce"], 1);
        assert_eq!(entry["cumulativeClaimed"], "500");
        assert_eq!(entry["available"], (KNOWN_DEPOSIT - 500).to_string());
        let last_claim_time = entry["lastClaimTime"]
            .as_u64()
            .expect("a recorded claim time");
        assert!(last_claim_time >= before);
    }

    /// A peer claim on `EVM_CHANNEL_ID`, signed by the same settlement key
    /// that signs this channel's claim-state challenge -- one key, both
    /// roles, exactly as `[[pay_channels]]` describes the deployed shape.
    fn peer_claim(secret: &SecretKey, nonce: u64, cumulative_amount: u64) -> WireClaim {
        let digest =
            connector_signer::evm_balance_proof_digest(&connector_signer::EvmBalanceProof {
                channel_id: EVM_CHANNEL_ID,
                nonce,
                transferred_amount: u128::from(cumulative_amount),
                locked_amount: 0,
                locks_root: [0u8; 32],
                chain_id: EVM_CHAIN_ID,
                token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
            });
        let bytes = sign_evm(secret, &digest);
        // `sign_evm` emits the Ethereum-wallet `{27, 28}` recovery
        // convention the claim wire carries; `ClaimSignature` holds
        // libsecp256k1's raw `{0, 1}` (issue #590).
        let mut raw = bytes.clone();
        raw[64] -= 27;
        WireClaim {
            channel_id: evm_channel_id_hex(),
            nonce,
            cumulative_amount,
            signature: ClaimSignature::Evm(
                connector_signer::Signature::from_bytes(&raw).expect("65 bytes"),
            ),
        }
    }

    /// A connector holding `EVM_CHANNEL_ID` as a **peer** channel -- the
    /// `[[peer_channels]]` shape: a counterparty key whose signature it
    /// accepts there, and that channel's EIP-712 domain.
    fn connector_with_peer_channel(counterparty: connector_signer::Address) -> Arc<Connector> {
        Arc::new(
            Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                Arc::new(TestClock::new(
                    Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
                )),
            )
            .with_channel_verification_key(evm_channel_id_hex(), counterparty)
            .with_channel_domain(
                evm_channel_id_hex(),
                ChannelDomain {
                    chain_id: EVM_CHAIN_ID,
                    token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                },
            )
            .expect("a well-formed channel id"),
        )
    }

    /// **The `[[pay_channels]]` defect.** A channel this node holds in its
    /// PEER book -- claims on it arrive peer-role and advance
    /// `ClaimBook`'s own inbound watermark -- must be answered out of that
    /// book, because that is the book the next claim on it will be judged
    /// against.
    ///
    /// `[[pay_channels]]` (ADR 0042 item 2) makes the payer ask this
    /// endpoint on every covered packet and take the answer as gospel:
    /// `OutboundClientLedger::next_claim` signs `cumulative + amount` at
    /// `max(nonce, issued_floor) + 1`. An answer of `nonce 0 /
    /// cumulativeClaimed 0` for a channel already at nonce 1 / 1000 makes
    /// the payer re-sign the SAME cumulative amount at a fresh nonce
    /// forever: the payee accepts each one (a nonce did advance) and each
    /// one advances nothing, so a priced peer termination refuses every
    /// packet after the first with `F06`, `advanced = 0`.
    ///
    /// `Config::load` refuses a channel that is both a `[[peer_channels]]`
    /// and a `[[client_channels]]` row (`ChannelInBothNamespaces`), so the
    /// channel alone tells this node which of its two books is the
    /// authority -- it never has to know who is asking.
    #[tokio::test]
    async fn a_peer_channel_is_answered_out_of_the_peer_book() {
        let (secret, address) = evm_signer();
        let connector = connector_with_peer_channel(address);
        let app = router_with_gate(connector.clone(), test_signer(), None, test_gate());
        let ask = |app: axum::Router| async move {
            let expires = far_future_expiry();
            let body = serde_json::json!({
                "channels": [{
                    "blockchain": "evm",
                    "channelId": evm_channel_id_hex(),
                    "expires": expires,
                    "signature": evm_challenge_signature(&secret, EVM_CHANNEL_ID, expires),
                }]
            });
            let request = Request::builder()
                .method("POST")
                .uri("/ilp/claim-state")
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["channels"][0].clone()
        };

        // Before the first crossing: a configured peer channel nothing has
        // claimed on. Zero, and the payer's first claim is nonce 1 for the
        // packet's own forwarded value.
        let entry = ask(app.clone()).await;
        assert_eq!(entry["ok"], true);
        assert_eq!(entry["nonce"], 0);
        assert_eq!(entry["cumulativeClaimed"], "0");

        assert_eq!(
            connector.handle_peer_claim(peer_claim(&secret, 1, 1_000)),
            ClaimAckOutcome::Accepted,
            "the peer book must accept the claim this test is about"
        );

        // After it. This is the answer the defect got wrong: the client
        // edge's book is still at zero and always will be, because no peer
        // claim ever reaches it.
        let entry = ask(app.clone()).await;
        assert_eq!(entry["ok"], true);
        assert_eq!(
            entry["nonce"], 1,
            "the peer book's nonce, not the client edge's zero"
        );
        assert_eq!(
            entry["cumulativeClaimed"], "1000",
            "the peer book's cumulative amount, not the client edge's zero"
        );
        assert_eq!(entry["available"], (KNOWN_DEPOSIT - 1_000).to_string());
        // Never the peer book's -- it journals no timestamp for an accepted
        // inbound claim, and no payer reads this field.
        assert!(entry["lastClaimTime"].is_null());

        // And it keeps advancing, which is the whole point: the payer's
        // next claim is nonce 2 for cumulative 2000, which advances the
        // payee's watermark by the packet's value instead of by nothing.
        assert_eq!(
            connector.handle_peer_claim(peer_claim(&secret, 2, 2_000)),
            ClaimAckOutcome::Accepted
        );
        let entry = ask(app).await;
        assert_eq!(entry["nonce"], 2);
        assert_eq!(entry["cumulativeClaimed"], "2000");
    }

    /// The other half of the rule: a channel this node holds no
    /// `[[peer_channels]]` row for is still answered out of the client
    /// edge's book, exactly as before. Same channel id, same challenge --
    /// only the peer registration is gone, and with it the peer book's say.
    #[tokio::test]
    async fn a_channel_that_is_not_a_peer_channel_still_reads_the_client_edge_book() {
        let (secret, _address) = evm_signer();
        let expires = far_future_expiry();
        let body = serde_json::json!({
            "channels": [{
                "blockchain": "evm",
                "channelId": evm_channel_id_hex(),
                "expires": expires,
                "signature": evm_challenge_signature(&secret, EVM_CHANNEL_ID, expires),
            }]
        });

        let response = post_claim_state(test_gate(), body).await;

        let entry = &response["channels"][0];
        assert_eq!(entry["ok"], true);
        assert_eq!(entry["nonce"], 0);
        assert_eq!(entry["cumulativeClaimed"], "0");
        assert_eq!(entry["available"], KNOWN_DEPOSIT.to_string());
    }
}
