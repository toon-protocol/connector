//! Generates the committed cross-repo wire-vector set (issue #527, ADR 0021)
//! from fixed literal fixtures run through the real implementations these
//! vectors are evidence for -- never from bytes captured once and pinned
//! after the fact. See `docs/protocol/wire-vectors.md` for the invariants
//! each section below is evidence of.
//!
//! Every vector [`generate`] emits is checked, before being returned,
//! against the same function that would validate it for real (`decode`,
//! `open_request`/`open_response`, or `derive_fulfillment`) -- this module
//! cannot silently commit a vector its own implementation would reject or
//! fail to reproduce.
//!
//! Fixtures (`identity_secret`, `ephemeral_secret`, ...) are literal,
//! non-secret bytes chosen only so this crate compiles to the same output
//! every time it runs -- never a real operator's key.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use connector_btp::{
    decode_frame, encode_error, encode_message, encode_response, encode_transfer, ProtocolData,
    ACCUMULATED_COST_HEADER, CLAIM_ACK_HEADER, CLAIM_ACK_PROTOCOL, CLAIM_HEADER, CONTENT_TYPE_TEXT,
    PAYOUT_CLAIM_PROTOCOL, PEER_CHALLENGE_HEADER, PEER_CHALLENGE_PROTOCOL,
};
use connector_domain::client_claim::{
    self, ClientClaim, ClientClaimError, SCHEME_BATCH_SETTLEMENT,
};
use connector_domain::{
    validate_voucher, ClaimError, EnvelopeError, EnvelopeRequest, EnvelopeResponse, Fulfill,
    Prepare, Price, Reject, RejectCode, VoucherAdmission, VoucherWatermark,
};
use connector_peer_btp::role_gate::{self, RefusedEvidence};
use connector_peer_btp::{ack, challenge_json, claim_json, fields, ClaimDecodeError};
use connector_peer_http::headers::{
    accumulated_cost as http_accumulated_cost, claim_ack as http_claim_ack, claim_ack_header_value,
    claim_header_value, claim_json as http_claim_json, peer_challenge_header_value, Headers,
    PeerRequest, PeerResponse,
};
use connector_runtime::{challenge_entry, voucher_json, ClaimAckOutcome, ClaimRejectReason};
use connector_settlement::batch::{ChannelPresentation, EvmChannelConfig, Voucher};
use connector_settlement::ChannelId as SettlementChannelId;
use connector_signer::giftwrap::{
    derive_fulfillment, open_request, open_response, seal_request_with_randomness,
    seal_response_with_randomness,
};
use connector_signer::{
    derive_evm_address, evm_batch_channel_id, evm_voucher_claim_state_challenge_digest,
    evm_voucher_digest, evm_voucher_signer, solana_voucher_claim_state_challenge_message,
    solana_voucher_message, verify_evm_voucher, verify_evm_voucher_claim_state_challenge,
    verify_solana_voucher, verify_solana_voucher_claim_state_challenge, Address,
    BatchChannelConfig, BatchSettlementDomain, Ed25519Signer, LocalEd25519Signer, LocalSigner,
    Signer, X402_BATCH_SETTLEMENT_ADDRESS,
};
use serde::Serialize;

/// The vector-set schema version. Bump when a field's meaning changes in a
/// way an existing SDK's replay code would misread -- a purely additive
/// field does not require a bump.
///
/// **2** (issue #1127): a Solana claim's `programId` acquired a meaning. It
/// had none before -- `client-edge-spec.md` §1.3 called it decorative, this
/// file's own `claim_solana` fixture declared the system program, and any
/// base58 32-byte value conformed. It now MUST name the settlement program
/// the claim's `channelAccount` lives under, the same 32 bytes ADR 0053 binds
/// into the signed balance proof. An SDK carrying the version-1 reading of
/// that field into a real claim builder is emitting a non-conforming claim,
/// which is exactly what this number exists to announce -- the connector
/// still accepts such a claim on the strength of its signature, and refusing
/// on it waits on this adoption (§1.3, issue #1127 step 4).
///
/// **3** (issue #1143): minimum delivery is retired (ADR 0057). The
/// `minimum_delivery` field is gone from both `peer_prepare` cases, the
/// `peer_minimum_delivery_absent` and `peer_minimum_delivery_malformed`
/// sections are deleted, and the `toon-minimum-delivery` entry and
/// `Toon-Minimum-Delivery` header no longer ride the pinned frames -- so
/// `peer_prepare.btp_message_hex` changed bytes. A replaying SDK that still
/// emits the field is emitting a field no connector reads, and one that
/// expects `R01` for an unmet floor is expecting a verdict no connector
/// reaches -- but `R01` itself stays in the vocabulary with RFC 0027's own
/// meaning, "the amount received by a connector in the path was too little to
/// forward", which is what a hop answers when its fee alone exceeds the amount
/// (ADR 0051 as corrected). Removals rather than additions, which is what this
/// number exists to announce.
///
/// That correction landed without a further bump, on purpose: it moves no
/// frame in this file. The reject vocabulary is prose in ADR 0051 rather than
/// a pinned vector, so `schema_version` -- which guards the bytes an SDK
/// replays -- has nothing to announce, and bumping it would spend a cross-repo
/// signal on a file that did not change.
///
/// **4** (issue #1157): the `{peerId, secret}` peer credential is deleted
/// (ADR 0060). The `peer_carriage.credential` section is gone, and with it
/// the `btp_raw_hex` and `http_base64` bytes an SDK replayed to build a
/// dialer's `auth` protocolData entry and its `Toon-Peer-Auth` header. Both
/// names leave the wire together, because peer behaviour on one carriage and
/// not the other is a defect rather than a property of the carriage
/// (ADR 0027). A replaying SDK that still sends either is sending a field no
/// connector reads -- harmless, since a receiver ignores an arriving header
/// rather than refusing it, which is what lets the two ends of a peering be
/// upgraded in either order. What the SDK must NOT infer is that its peering
/// is thereby unauthenticated: role is now decided by the covering claim's
/// signature against the counterparty key `[[peer_channels]]` configures,
/// which every peer frame already carried (ADR 0042) and which is strictly
/// stronger than the string it replaces. A removal, which is what this number
/// exists to announce.
///
/// **5** (issue #1269 / ADR 0069): `executionCondition` is deleted from
/// `Prepare` and a `greeting` boolean takes its place -- `-31` bytes per
/// packet per hop, since a `bool` where a 32-byte `UInt256` was is not
/// merely a smaller field but the field's own semantics changing kind, from
/// a cryptographic commitment to a stated flag. `PreparePacketFields.
/// execution_condition_hex` is gone; `PreparePacketFields.greeting` takes
/// its place, so `peer_prepare.prepare`/`prepare_no_claim`'s
/// `btp_message_hex`/`http_body_hex` all changed bytes. The `fulfilment`
/// section narrows to what still holds: `derive_fulfillment`'s own
/// determinism, with no condition left to derive from a fulfilment or match
/// one against -- `FulfilmentCase.condition_hex` and `.matches` are gone. A
/// replaying SDK that still sends a 32-byte condition is sending a field no
/// connector reads or checks; one still checking a returned fulfilment
/// against a condition it minted should instead compare the fulfilment
/// directly against `derive_fulfillment(shared_secret)`, which is what
/// `connector send`'s own end-to-end check now does.
///
/// **6** (issue #1347 / ADR 0074 decision 7): a client-edge claim gains a
/// `scheme` discriminator (issue #1341), and a claim under
/// `scheme: "batch-settlement"` is a **voucher** -- x402's own claim, on a
/// channel this connector never opens, verified against a different
/// signature scheme per chain (`connector_signer::voucher_signature`) and
/// with no nonce: its freshness is an amount-only watermark
/// (`connector_domain::validate_voucher`), not the nonce rule every prior
/// claim used. A new top-level `claim_voucher` section carries the two
/// voucher shapes (`evm`, `solana`), the amount-only watermark's three
/// outcomes and a Solana voucher's structural refusal on a nonzero
/// `expiresAt`. Every existing section's bytes are unchanged -- this is
/// additive, a new section rather than a changed one -- but the bump still
/// matters: an SDK that has not read the `scheme` discriminator has no
/// signal that a second claim shape now rides the same claim header/
/// protocolData entry it already parses, and would misread one as a
/// malformed `toon-channel` claim rather than a voucher it may not yet
/// support.
///
/// **7** (issue #1384 / ADR 0075 decision 14): **every claim is a voucher.**
/// The `toon-channel` claim scheme is retired (ADR 0024 and ADR 0053, retired
/// by ADR 0075), and with it every section that pinned it: the top-level
/// `claim` section (the EIP-712 `BalanceProof`), `peer_carriage`'s
/// `claim_evm`, `claim_solana` and `claim_digest_hex`, its FLUSH cases
/// (`flush`, `flush_ack`, `flush_requested`) and its nonce cases
/// (`claim_retransmit`, `claim_same_nonce_different_bytes`), and the whole
/// `channel_control_declaration` section -- the `TokenNetwork`-domain
/// `auth_channel_proof`, replaced by the voucher claim-state challenge. In
/// their place: `peer_carriage.voucher_evm`/`voucher_solana` (a peer's
/// voucher, as the paying node renders it), `peer_carriage.prepare` re-based
/// on that voucher, `peer_carriage.zero_value_challenge` (a zero-value peer
/// PREPARE carrying no voucher and the peer-role challenge in its own slot),
/// a top-level `toon_channel_refused` section (a `toon-channel` claim, and a
/// claim with no `scheme`, refused by name at the client edge and on both
/// peer carriages), and a top-level `payout_voucher` section (the
/// `payout-claim` TRANSFER a connector pays a client with). The
/// `nonce_not_advancing` ack reason is no longer pinned: no voucher verdict
/// carries it. A replaying SDK that still sends a claim with no `scheme` is
/// sending a claim every connector refuses -- which is what this number
/// exists to announce.
pub const SCHEMA_VERSION: u32 = 7;

fn seq_bytes<const N: usize>(start: u8) -> [u8; N] {
    let mut out = [0u8; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = start.wrapping_add(i as u8);
    }
    out
}

fn hex_of(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// Decode a fixed-length hex literal (no `0x` prefix) into an array,
/// panicking on a bad literal -- these are hardcoded fixtures in this file,
/// so a failure here is a typo, not input.
fn hex_bytes<const N: usize>(text: &str) -> [u8; N] {
    let decoded = hex::decode(text).unwrap_or_else(|e| panic!("fixture hex {text:?}: {e}"));
    let len = decoded.len();
    decoded
        .try_into()
        .unwrap_or_else(|_| panic!("fixture hex is {N} bytes, got {len}: {text:?}"))
}

#[derive(Debug, Serialize)]
pub struct WireVectors {
    pub schema_version: u32,
    pub envelope: EnvelopeVectors,
    pub giftwrap: GiftwrapVectors,
    pub fulfilment: FulfilmentVectors,
    pub peer_carriage: PeerCarriageVectors,
    pub charge: ChargeVectors,
    pub claim_voucher: ClaimVoucherVectors,
    pub voucher_claim_state_challenge: VoucherClaimStateChallengeVectors,
    pub toon_channel_refused: ToonChannelRefusedVectors,
    pub payout_voucher: PayoutVoucherVectors,
}

#[derive(Debug, Serialize)]
pub struct EnvelopeVectors {
    pub valid: Vec<EnvelopeValidVector>,
    pub invalid: Vec<EnvelopeInvalidVector>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "direction", rename_all = "snake_case")]
pub enum EnvelopeFields {
    Request {
        method: String,
        target: String,
        headers: Vec<(String, String)>,
        body_hex: String,
    },
    Response {
        status: u16,
        headers: Vec<(String, String)>,
        body_hex: String,
    },
}

#[derive(Debug, Serialize)]
pub struct EnvelopeValidVector {
    pub name: &'static str,
    pub encoded_hex: String,
    pub decoded: EnvelopeFields,
}

#[derive(Debug, Serialize)]
pub struct EnvelopeInvalidVector {
    pub name: &'static str,
    pub direction: &'static str,
    pub bytes_hex: String,
    pub expected_error: &'static str,
}

#[derive(Debug, Serialize)]
pub struct GiftwrapVectors {
    pub receiver_identity_secret_hex: String,
    pub receiver_identity_public_hex: String,
    pub cases: Vec<GiftwrapCase>,
}

#[derive(Debug, Serialize)]
pub struct GiftwrapCase {
    pub name: &'static str,
    pub ephemeral_secret_hex: String,
    pub shared_secret_hex: String,
    pub request_nonce_hex: String,
    pub response_nonce_hex: String,
    pub request_envelope: EnvelopeFields,
    pub request_envelope_hex: String,
    pub request_wrap_hex: String,
    pub response_envelope: EnvelopeFields,
    pub response_envelope_hex: String,
    pub response_wrap_hex: String,
}

/// Issue #1269 / ADR 0069 retired the execution condition from the wire, so
/// this section is narrower than it once was: it pins only
/// `derive_fulfillment`'s own determinism -- the one relation a downstream
/// implementer still needs, since a termination derives its fulfilment from
/// a request's shared secret (ADR 0019) and a sender checks a delivery end
/// to end by comparing what comes back against that same derivation
/// (`connector send`). There is no condition left to derive one from or to
/// match against.
#[derive(Debug, Serialize)]
pub struct FulfilmentVectors {
    pub cases: Vec<FulfilmentCase>,
}

#[derive(Debug, Serialize)]
pub struct FulfilmentCase {
    pub name: &'static str,
    pub shared_secret_hex: String,
    pub fulfilment_hex: String,
}

/// This module's own name for each [`EnvelopeError`] variant -- stable
/// across a `Debug` reformat, and independent of Rust's `Debug` output
/// shape, since a replaying SDK matches on this string, not on
/// `format!("{err:?}")`.
fn error_tag(err: &EnvelopeError) -> &'static str {
    match err {
        EnvelopeError::BufferUnderflow => "buffer_underflow",
        EnvelopeError::NonCanonicalLength => "non_canonical_length",
        EnvelopeError::LengthDeterminantOverflow => "length_determinant_overflow",
        EnvelopeError::InvalidType => "invalid_type",
        EnvelopeError::InvalidUtf8(_) => "invalid_utf8",
        EnvelopeError::TrailingBytes => "trailing_bytes",
    }
}

fn generate_envelope_vectors() -> EnvelopeVectors {
    let minimal_request = EnvelopeRequest {
        method: "GET".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: vec![],
    };
    let posted_order = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/orders".to_string(),
        headers: vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("x-request-id".to_string(), "vector-0001".to_string()),
        ],
        body: b"{\"item\":\"widget\"}".to_vec(),
    };
    let duplicate_headers = EnvelopeRequest {
        method: "GET".to_string(),
        target: "/search?q=%E2%9C%93".to_string(),
        headers: vec![
            ("x-a".to_string(), "1".to_string()),
            ("x-a".to_string(), "2".to_string()),
        ],
        body: vec![],
    };
    let ok_response = EnvelopeResponse {
        status: 200,
        headers: vec![("content-type".to_string(), "application/json".to_string())],
        body: b"{\"ok\":true}".to_vec(),
    };
    let binary_body_response = EnvelopeResponse {
        status: 206,
        headers: vec![
            (
                "content-type".to_string(),
                "application/octet-stream".to_string(),
            ),
            ("x-a".to_string(), "1".to_string()),
            ("x-a".to_string(), "2".to_string()),
        ],
        body: vec![0x00, 0x01, 0xff, 0xfe, 0x80, 0x7f],
    };

    let mut valid = Vec::new();
    for (name, request) in [
        ("minimal_get_request", minimal_request),
        ("post_with_headers_and_json_body", posted_order),
        ("get_with_duplicate_header_names", duplicate_headers),
    ] {
        let encoded = request.encode();
        let decoded = EnvelopeRequest::decode(&encoded)
            .unwrap_or_else(|e| panic!("vector {name} does not decode: {e:?}"));
        assert_eq!(decoded, request, "vector {name} did not round-trip");
        valid.push(EnvelopeValidVector {
            name,
            encoded_hex: hex_of(&encoded),
            decoded: EnvelopeFields::Request {
                method: request.method,
                target: request.target,
                headers: request.headers.clone(),
                body_hex: hex_of(&request.body),
            },
        });
    }
    for (name, response) in [
        ("ok_json_response", ok_response),
        (
            "partial_content_binary_body_and_duplicate_headers",
            binary_body_response,
        ),
    ] {
        let encoded = response.encode();
        let decoded = EnvelopeResponse::decode(&encoded)
            .unwrap_or_else(|e| panic!("vector {name} does not decode: {e:?}"));
        assert_eq!(decoded, response, "vector {name} did not round-trip");
        valid.push(EnvelopeValidVector {
            name,
            encoded_hex: hex_of(&encoded),
            decoded: EnvelopeFields::Response {
                status: response.status,
                headers: response.headers.clone(),
                body_hex: hex_of(&response.body),
            },
        });
    }

    let mut invalid = Vec::new();

    let canonical_get_root: Vec<u8> = vec![1, 0x03, b'G', b'E', b'T', 0x01, b'/', 0x00, 0x00];

    let mut wrong_type_as_request = canonical_get_root.clone();
    wrong_type_as_request[0] = 2;
    invalid.push(check_invalid(
        "request_decode_rejects_wrong_type_byte",
        "request",
        wrong_type_as_request,
        EnvelopeError::InvalidType,
        |b| EnvelopeRequest::decode(b).err(),
    ));

    let ok_response_encoded = EnvelopeResponse {
        status: 200,
        headers: vec![],
        body: vec![],
    }
    .encode();
    let mut wrong_type_as_response = ok_response_encoded.clone();
    wrong_type_as_response[0] = 1;
    invalid.push(check_invalid(
        "response_decode_rejects_wrong_type_byte",
        "response",
        wrong_type_as_response,
        EnvelopeError::InvalidType,
        |b| EnvelopeResponse::decode(b).err(),
    ));

    let truncated = canonical_get_root[..canonical_get_root.len() - 1].to_vec();
    invalid.push(check_invalid(
        "request_decode_rejects_truncated_input",
        "request",
        truncated,
        EnvelopeError::BufferUnderflow,
        |b| EnvelopeRequest::decode(b).err(),
    ));

    let mut trailing = canonical_get_root.clone();
    trailing.push(0xff);
    invalid.push(check_invalid(
        "request_decode_rejects_trailing_bytes",
        "request",
        trailing,
        EnvelopeError::TrailingBytes,
        |b| EnvelopeRequest::decode(b).err(),
    ));

    let invalid_utf8 = vec![1u8, 0x01, 0x80];
    invalid.push(check_invalid(
        "request_decode_rejects_invalid_utf8_in_method",
        "request",
        invalid_utf8,
        EnvelopeError::InvalidUtf8("method"),
        |b| EnvelopeRequest::decode(b).err(),
    ));

    let mut non_minimal_length = canonical_get_root.clone();
    non_minimal_length.splice(1..2, [0x81, 0x03]);
    invalid.push(check_invalid(
        "request_decode_rejects_a_non_minimal_length_determinant",
        "request",
        non_minimal_length,
        EnvelopeError::NonCanonicalLength,
        |b| EnvelopeRequest::decode(b).err(),
    ));

    let zero_length_alias = vec![1u8, 0x80, 0x01, b'/', 0x00, 0x00];
    invalid.push(check_invalid(
        "request_decode_rejects_a_zero_length_long_form_alias",
        "request",
        zero_length_alias,
        EnvelopeError::NonCanonicalLength,
        |b| EnvelopeRequest::decode(b).err(),
    ));

    let mut over_long_determinant = vec![1u8, 0x89];
    over_long_determinant.extend([0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03]);
    over_long_determinant.extend([b'G', b'E', b'T', 0x01, b'/', 0x00, 0x00]);
    invalid.push(check_invalid(
        "request_decode_rejects_an_over_long_determinant_instead_of_truncating",
        "request",
        over_long_determinant,
        EnvelopeError::LengthDeterminantOverflow,
        |b| EnvelopeRequest::decode(b).err(),
    ));

    EnvelopeVectors { valid, invalid }
}

fn check_invalid(
    name: &'static str,
    direction: &'static str,
    bytes: Vec<u8>,
    expected: EnvelopeError,
    decode: impl Fn(&[u8]) -> Option<EnvelopeError>,
) -> EnvelopeInvalidVector {
    let actual = decode(&bytes).unwrap_or_else(|| panic!("vector {name} unexpectedly decoded"));
    assert_eq!(actual, expected, "vector {name} produced the wrong error");
    EnvelopeInvalidVector {
        name,
        direction,
        bytes_hex: hex_of(&bytes),
        expected_error: error_tag(&actual),
    }
}

fn generate_giftwrap_vectors() -> (GiftwrapVectors, [u8; 32]) {
    let identity_secret = seq_bytes::<32>(0x01);
    let ephemeral_secret = seq_bytes::<32>(0x21);
    let shared_secret = seq_bytes::<32>(0x41);
    let request_nonce = seq_bytes::<12>(0x61);
    let response_nonce = seq_bytes::<12>(0x6d);

    let receiver = LocalSigner::from_secret_bytes("vector-fixture-identity", identity_secret)
        .expect("fixture identity secret is a valid secp256k1 scalar");
    let receiver_public = receiver
        .public_key()
        .expect("fixture identity has a public key");

    let request_envelope = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/orders".to_string(),
        headers: vec![("content-type".to_string(), "application/json".to_string())],
        body: b"{\"item\":\"widget\"}".to_vec(),
    };
    let request_plaintext = request_envelope.encode();

    let request_wrap = seal_request_with_randomness(
        &request_plaintext,
        &receiver_public,
        &ephemeral_secret,
        &shared_secret,
        &request_nonce,
    )
    .expect("fixture request seals cleanly");
    let (opened_plaintext, opened_secret) = open_request(&request_wrap, &receiver)
        .expect("the receiver's own signer opens a wrap sealed to its identity");
    assert_eq!(opened_plaintext, request_plaintext);
    assert_eq!(opened_secret, shared_secret);

    let response_envelope = EnvelopeResponse {
        status: 200,
        headers: vec![("content-type".to_string(), "application/json".to_string())],
        body: b"{\"ok\":true}".to_vec(),
    };
    let response_plaintext = response_envelope.encode();

    let response_wrap =
        seal_response_with_randomness(&shared_secret, &response_plaintext, &response_nonce);
    let opened_response = open_response(&shared_secret, &response_wrap)
        .expect("the request's own shared secret opens the response sealed with it");
    assert_eq!(opened_response, response_plaintext);

    let case = GiftwrapCase {
        name: "sealed_request_and_response_round_trip",
        ephemeral_secret_hex: hex_of(&ephemeral_secret),
        shared_secret_hex: hex_of(&shared_secret),
        request_nonce_hex: hex_of(&request_nonce),
        response_nonce_hex: hex_of(&response_nonce),
        request_envelope: EnvelopeFields::Request {
            method: request_envelope.method,
            target: request_envelope.target,
            headers: request_envelope.headers.clone(),
            body_hex: hex_of(&request_envelope.body),
        },
        request_envelope_hex: hex_of(&request_plaintext),
        request_wrap_hex: hex_of(&request_wrap),
        response_envelope: EnvelopeFields::Response {
            status: response_envelope.status,
            headers: response_envelope.headers.clone(),
            body_hex: hex_of(&response_envelope.body),
        },
        response_envelope_hex: hex_of(&response_plaintext),
        response_wrap_hex: hex_of(&response_wrap),
    };

    (
        GiftwrapVectors {
            receiver_identity_secret_hex: hex_of(&identity_secret),
            receiver_identity_public_hex: hex_of(&receiver_public),
            cases: vec![case],
        },
        shared_secret,
    )
}

fn generate_fulfilment_vectors(giftwrap_shared_secret: [u8; 32]) -> FulfilmentVectors {
    let fulfilment = derive_fulfillment(&giftwrap_shared_secret);

    let other_secret = seq_bytes::<32>(0x81);
    let other_fulfilment = derive_fulfillment(&other_secret);
    assert_ne!(
        fulfilment, other_fulfilment,
        "two different secrets must never derive the same fulfilment in this fixture"
    );

    FulfilmentVectors {
        cases: vec![
            FulfilmentCase {
                name: "derived_fulfilment_from_the_giftwrap_sections_secret",
                shared_secret_hex: hex_of(&giftwrap_shared_secret),
                fulfilment_hex: hex_of(&fulfilment),
            },
            FulfilmentCase {
                name: "derived_fulfilment_from_a_different_secret",
                shared_secret_hex: hex_of(&other_secret),
                fulfilment_hex: hex_of(&other_fulfilment),
            },
        ],
    }
}

// ---------------------------------------------------------------------
// Peer carriage (issue #729, `docs/protocol/peer-carriage-spec.md` §10;
// ADR 0075 decisions 5 and 6, issue #1384)
// ---------------------------------------------------------------------
//
// Most items are *(pair)*s: one BTP encoding and one HTTP encoding of the
// same fixture value, generated in one pass and asserted decoded-equal
// (§10.1's pairing rule, spec I1). Every function here calls the real code
// -- the paying side's own renderers (`connector_runtime::voucher_json`,
// `connector_runtime::challenge_entry`), `connector_peer_btp`'s codec and
// role gate, and `connector_peer_http`'s headers and evidence reader --
// never a hand-rolled parallel encoder, so a vector this module emits is a
// vector its own implementation would also emit and accept.
//
// Every claim on a peer carriage is an x402 **voucher** since ADR 0075
// (schema 7): the `toon-channel` peer claim, its FLUSH, and its nonce
// retransmission cases are gone with the scheme.

/// The facts an EVM voucher's signature hangs off -- its x402
/// `ChannelConfig`, the channel id that config hashes to, and the EIP-712
/// digest `getVoucherDigest(channelId, amount)` answers -- shared, under the
/// same field names, by `claim_voucher.evm`, `peer_carriage.voucher_evm` and
/// `payout_voucher.evm`, so one cross-check against the deployed contract
/// (`connector-settlement-evm/tests/x402_voucher_vector.rs`) reads all three
/// the same way.
#[derive(Debug, Serialize)]
pub struct EvmVoucherFacts {
    /// The EIP-712 domain's `chainId` (Base Sepolia's real id).
    pub chain_id: u64,
    /// `x402BatchSettlement`'s one deployed address.
    pub verifying_contract_hex: String,
    pub channel_config: VoucherEvmChannelConfigFields,
    /// `getChannelId(channel_config)`.
    pub channel_id_hex: String,
    pub max_claimable_amount: u64,
    /// `getVoucherDigest(channel_id_hex, max_claimable_amount)`.
    pub digest_hex: String,
    /// `evm_voucher_signer(channel_config)`.
    pub signer_address_hex: String,
    /// `r ‖ s ‖ v`, 65 bytes, `v` 27 or 28.
    pub signature_hex: String,
}

fn channel_config_fields(config: &BatchChannelConfig) -> VoucherEvmChannelConfigFields {
    VoucherEvmChannelConfigFields {
        payer_hex: hex_of(&config.payer),
        payer_authorizer_hex: hex_of(&config.payer_authorizer),
        receiver_hex: hex_of(&config.receiver),
        receiver_authorizer_hex: hex_of(&config.receiver_authorizer),
        token_hex: hex_of(&config.token),
        withdraw_delay: config.withdraw_delay,
        salt_hex: hex_of(&config.salt),
    }
}

/// A node's EVM **settlement key** -- the key that signs every voucher on
/// its outbound channels (ADR 0075 decision 3: `payerAuthorizer == payer`,
/// both its settlement address). A fixture scalar, never a real key.
fn node_settlement_signer(label: &'static str, start: u8) -> (LocalSigner, Address) {
    let signer = LocalSigner::from_secret_bytes(label, seq_bytes::<32>(start))
        .expect("fixture secret is a valid secp256k1 scalar");
    let address = derive_evm_address(&signer.public_key().expect("fixture has a key"));
    (signer, address)
}

/// The `ChannelConfig` a paying node builds toward `receiver` (ADR 0075
/// decision 3): itself as `payer` and `payerAuthorizer`, the receiver in
/// both receiving seats.
fn outbound_channel_config(payer: Address, receiver: Address, salt: u8) -> BatchChannelConfig {
    BatchChannelConfig {
        payer,
        payer_authorizer: payer,
        receiver,
        receiver_authorizer: receiver,
        token: [0x55; 20],
        withdraw_delay: 86_400,
        salt: [salt; 32],
    }
}

fn to_settlement_config(config: &BatchChannelConfig) -> EvmChannelConfig {
    EvmChannelConfig {
        payer: config.payer,
        payer_authorizer: config.payer_authorizer,
        receiver: config.receiver,
        receiver_authorizer: config.receiver_authorizer,
        token: config.token,
        withdraw_delay: config.withdraw_delay,
        salt: config.salt,
    }
}

/// Sign an EVM voucher for `amount` on `config`'s channel with `signer`, in
/// the wallet convention (`v` 27/28) the paying half's key produces, and
/// self-verify it against the channel's voucher signer.
fn sign_evm_voucher(
    signer: &LocalSigner,
    config: &BatchChannelConfig,
    amount: u64,
) -> (EvmVoucherFacts, EvmPresentation) {
    let domain = BatchSettlementDomain::x402(VOUCHER_EVM_CHAIN_ID);
    let channel_id = evm_batch_channel_id(&domain, config);
    let digest = evm_voucher_digest(&domain, &channel_id, u128::from(amount));
    let mut signature = signer
        .sign(&digest)
        .expect("fixture signer signs its own digest")
        .to_bytes();
    signature[64] += 27;
    let voucher_signer = evm_voucher_signer(config);
    assert!(
        verify_evm_voucher(
            &domain,
            &channel_id,
            u128::from(amount),
            &signature,
            &voucher_signer
        ),
        "a fixture voucher must verify against its channel's voucher signer"
    );
    let facts = EvmVoucherFacts {
        chain_id: VOUCHER_EVM_CHAIN_ID,
        verifying_contract_hex: hex_of(&X402_BATCH_SETTLEMENT_ADDRESS),
        channel_config: channel_config_fields(config),
        channel_id_hex: hex_of(&channel_id),
        max_claimable_amount: amount,
        digest_hex: hex_of(&digest),
        signer_address_hex: hex_of(&voucher_signer),
        signature_hex: hex_of(&signature),
    };
    let presentation = EvmPresentation {
        presentation: ChannelPresentation::Evm {
            channel: SettlementChannelId(format!("0x{}", hex_of(&channel_id))),
            config: to_settlement_config(config),
        },
        channel_id,
        signature,
    };
    (facts, presentation)
}

/// A signed EVM voucher's channel, as the paying half presents it.
struct EvmPresentation {
    presentation: ChannelPresentation,
    channel_id: [u8; 32],
    signature: [u8; 65],
}

/// The timestamp every rendered fixture claim carries.
const FIXTURE_TIMESTAMP: &str = "2030-01-01T00:00:00.000Z";

/// A peer voucher: what a paying node puts in the claim slot of a PREPARE it
/// forwards (ADR 0075 decision 6), rendered by the paying side's own
/// `connector_runtime::voucher_json`, and its two transfer encodings -- raw
/// UTF-8 in the BTP `payment-channel-claim` entry, base64 in the
/// `Payment-Channel-Claim` header (§4).
#[derive(Debug, Serialize)]
pub struct PeerVoucherEvmCase {
    pub name: &'static str,
    #[serde(flatten)]
    pub facts: EvmVoucherFacts,
    pub json: String,
    pub btp_raw_hex: String,
    pub http_base64: String,
}

/// The Solana twin of [`PeerVoucherEvmCase`]: a voucher on a
/// `payment-channels` channel whose `authorized_signer` is the paying
/// node's Solana settlement key.
#[derive(Debug, Serialize)]
pub struct PeerVoucherSolanaCase {
    pub name: &'static str,
    pub channel_account_base58: String,
    /// The paying node's Solana settlement key: the channel's
    /// `authorized_signer`.
    pub authorized_signer_base58: String,
    pub signer_secret_hex: String,
    pub max_claimable_amount: u64,
    /// `solana_voucher_message(channel, amount, 0)`'s 50 bytes.
    pub signed_message_hex: String,
    pub signature_base58: String,
    pub json: String,
    pub btp_raw_hex: String,
    pub http_base64: String,
}

/// The OER `Prepare` a peer PREPARE carries (§10.2 items 5, 6).
#[derive(Debug, Serialize)]
pub struct PreparePacketFields {
    pub amount: u64,
    pub expires_at: String,
    pub greeting: bool,
    pub destination: String,
    pub data_hex: String,
}

/// A PREPARE on both carriages, with whatever evidence rides beside it: a
/// voucher in the claim slot, a peer-role challenge in its own slot, or
/// nothing.
#[derive(Debug, Serialize)]
pub struct PreparePairCase {
    pub name: &'static str,
    pub prepare: PreparePacketFields,
    /// The claim slot's JSON -- always a voucher -- or `None`.
    pub claim_json: Option<String>,
    /// The peer-role challenge slot's JSON (§1.4), or `None`.
    pub challenge_json: Option<String>,
    pub btp_message_hex: String,
    pub http_headers: Vec<(String, String)>,
    pub http_body_hex: String,
}

/// A zero-value peer PREPARE (ADR 0075 decision 5): it carries **no
/// voucher**, and carries the voucher claim-state challenge instead, signed
/// by the channel's voucher signer, so the receiver can attribute it to the
/// peering. The challenge's message and signature are pinned alongside the
/// frames.
#[derive(Debug, Serialize)]
pub struct PeerZeroValueCase {
    pub name: &'static str,
    /// The channel the challenge names: `peer_carriage.voucher_evm`'s.
    pub channel_id_hex: String,
    /// Unix seconds. A receiver honours a challenge only while `expires` is
    /// ahead of its clock and no more than 300 seconds ahead
    /// (`peer-carriage-spec.md` §1.2) -- a fact about the clock at
    /// verification, which this static vector cannot encode; it pins a far
    /// future `expires` so the signature and bytes are reproducible.
    pub expires: u64,
    /// `keccak256(0x1901 ‖ x402DomainSeparator ‖ structHash)` for
    /// `ClaimStateChallenge(bytes32 channelId,uint256 expires)`.
    pub digest_hex: String,
    pub signer_address_hex: String,
    /// `r ‖ s ‖ v`, 65 bytes, `v` 27 or 28.
    pub signature_hex: String,
    pub packet: PreparePairCase,
}

/// A judged voucher's verdict, as it rides a RESPONSE (§10.2 items 7-9).
#[derive(Debug, Serialize)]
pub struct AckFields {
    pub result: &'static str,
    pub reason: Option<&'static str>,
}

/// A RESPONSE answering a voucher-bearing frame: the packet it answers, and
/// the claim-ack riding beside it -- independently (§6.2).
#[derive(Debug, Serialize)]
pub struct PeerAnswerCase {
    pub name: String,
    pub packet: &'static str,
    pub packet_hex: String,
    pub ack: Option<AckFields>,
    pub accumulated_cost: Option<u64>,
    pub btp_response_hex: String,
    pub http_status: u16,
    pub http_headers: Vec<(String, String)>,
    pub http_body_hex: String,
}

/// An ack whose JSON does not decode to either verdict -- §6.3's "not
/// acknowledged", pinned as a raw payload rather than through
/// [`ack::encode`], which can never produce one (§10.2 item 12).
#[derive(Debug, Serialize)]
pub struct PeerMalformedAckCase {
    pub name: &'static str,
    pub malformed_json: String,
    pub btp_raw_hex: String,
    pub http_base64: String,
}

/// A sealed giftwrap payload carried unchanged as a PREPARE's `data`, on
/// both carriages (§8.1, §10.2 item 20).
#[derive(Debug, Serialize)]
pub struct PeerForwardedDataCase {
    pub name: &'static str,
    pub sealed_data_hex: String,
    pub btp_ilp_packet_prepare_hex: String,
    pub http_body_hex: String,
}

/// Schema 7 (ADR 0075): no `toon-channel` claim (`claim_evm`,
/// `claim_solana`, `claim_digest_hex`), no FLUSH (`flush`, `flush_ack`,
/// `flush_requested`) and no nonce cases (`claim_retransmit`,
/// `claim_same_nonce_different_bytes`); a peer voucher on each chain and a
/// zero-value packet carrying the peer-role challenge in their place. A
/// voucher's own retransmission rule is `claim_voucher.amount_only_watermark`.
#[derive(Debug, Serialize)]
pub struct PeerCarriageVectors {
    pub voucher_evm: PeerVoucherEvmCase,
    pub voucher_solana: PeerVoucherSolanaCase,
    pub prepare: PreparePairCase,
    pub prepare_no_claim: PreparePairCase,
    pub zero_value_challenge: PeerZeroValueCase,
    pub fulfill_ack_accepted: PeerAnswerCase,
    pub fulfill_ack_rejected: PeerAnswerCase,
    pub ack_rejected_reasons: Vec<PeerAnswerCase>,
    pub reject_with_cost: PeerAnswerCase,
    pub ack_absent: PeerAnswerCase,
    pub ack_malformed: PeerMalformedAckCase,
    pub forwarded_data_unchanged: PeerForwardedDataCase,
}

fn headers_pairs(headers: &Headers) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn prepare_fields(prepare: &Prepare) -> PreparePacketFields {
    PreparePacketFields {
        amount: prepare.amount,
        expires_at: prepare
            .expires_at
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        greeting: prepare.greeting,
        destination: prepare.destination.clone(),
        data_hex: hex_of(&prepare.data),
    }
}

/// The paying peer's settlement address, and the receiving peer's.
const PEER_PAYER_KEY_START: u8 = 0x31;
const PEER_RECEIVER: Address = [0x77; 20];

/// A voucher's two transfer encodings, each read back by the real reader
/// and parsed back through the real claim parser (§4, I1, I4).
fn voucher_encodings(json: &str) -> (String, String) {
    let parsed = claim_json::parse(json.as_bytes()).expect("the rendered voucher parses back");
    assert_eq!(
        client_claim::parse_client_claim(json).expect("the client edge parses it too"),
        parsed,
        "one parser serves both edges (I4)"
    );
    let raw_entry = claim_json::protocol_data(json);
    let http_value = claim_header_value(json);
    let mut headers = Headers::new();
    headers.push(CLAIM_HEADER, &http_value);
    assert_eq!(
        http_claim_json(&headers)
            .expect("a claim rode")
            .expect("valid base64"),
        json.as_bytes()
    );
    (hex_of(&raw_entry.data), http_value)
}

fn generate_peer_voucher_evm_case() -> (PeerVoucherEvmCase, EvmPresentation) {
    let (signer, payer) = node_settlement_signer("vector-fixture-peer-payer", PEER_PAYER_KEY_START);
    let config = outbound_channel_config(payer, PEER_RECEIVER, 0x88);
    let (facts, presentation) = sign_evm_voucher(&signer, &config, 250_000);
    let json = voucher_json(
        &presentation.presentation,
        &Voucher {
            cumulative_amount: 250_000,
            signature: presentation.signature.to_vec(),
        },
        &format!("0x{}", hex_of(&config.payer_authorizer)),
        FIXTURE_TIMESTAMP,
    );
    let ClientClaim::EvmVoucher(voucher) =
        claim_json::parse(json.as_bytes()).expect("the peer voucher parses")
    else {
        panic!("an EVM peer voucher");
    };
    assert_eq!(
        voucher.channel_id,
        format!("0x{}", hex_of(&presentation.channel_id))
    );
    assert!(
        voucher.channel_config.is_some(),
        "a peer voucher always carries its channelConfig"
    );
    let (btp_raw_hex, http_base64) = voucher_encodings(&json);
    (
        PeerVoucherEvmCase {
            name: "peer_voucher_evm",
            facts,
            json,
            btp_raw_hex,
            http_base64,
        },
        presentation,
    )
}

fn generate_peer_voucher_solana_case() -> PeerVoucherSolanaCase {
    let seed = seq_bytes::<32>(0xd1);
    let signer = LocalEd25519Signer::from_secret_bytes(seed).expect("fixture seed is 32 bytes");
    let channel_account: [u8; 32] = [0xd4; 32];
    let amount: u64 = 250_000;
    let message = solana_voucher_message(&channel_account, amount, 0);
    let signature = signer.sign(&message);
    assert!(verify_solana_voucher(
        &channel_account,
        amount,
        0,
        &signature,
        &signer.public_key()
    ));
    let channel_account_base58 = bs58::encode(channel_account).into_string();
    // As the paying half renders it: `senderId` is a label, the channel
    // account on Solana (`connector_runtime`'s `voucher_sender`).
    let json = voucher_json(
        &ChannelPresentation::Solana {
            channel: SettlementChannelId(channel_account_base58.clone()),
        },
        &Voucher {
            cumulative_amount: u128::from(amount),
            signature: signature.to_vec(),
        },
        &channel_account_base58,
        FIXTURE_TIMESTAMP,
    );
    assert!(matches!(
        claim_json::parse(json.as_bytes()),
        Ok(ClientClaim::SolanaVoucher(_))
    ));
    let (btp_raw_hex, http_base64) = voucher_encodings(&json);
    PeerVoucherSolanaCase {
        name: "peer_voucher_solana",
        channel_account_base58,
        authorized_signer_base58: bs58::encode(signer.public_key()).into_string(),
        signer_secret_hex: hex_of(&seed),
        max_claimable_amount: amount,
        signed_message_hex: hex_of(&message),
        signature_base58: bs58::encode(signature).into_string(),
        json,
        btp_raw_hex,
        http_base64,
    }
}

fn fixture_prepare(amount: u64) -> Prepare {
    let prepare = Prepare {
        amount,
        expires_at: "2030-01-01T00:01:00Z".parse().expect("fixed literal"),
        greeting: false,
        destination: "g.toon.store-box.settle".to_string(),
        data: b"vector-fixture-prepare-data".to_vec(),
    };
    assert_eq!(
        Prepare::decode(&prepare.encode()).expect("self-generated prepare decodes"),
        prepare
    );
    prepare
}

/// One PREPARE on both carriages with `claim` and/or `challenge` riding
/// beside it, each read back through the role gate's own evidence readers
/// (`role_gate::btp_evidence`, `connector_peer_http::evidence_on`).
fn prepare_pair_case(
    name: &'static str,
    request_id: u32,
    prepare: &Prepare,
    claim: Option<&str>,
    challenge: Option<&str>,
) -> PreparePairCase {
    let prepare_bytes = prepare.encode();
    let mut entries = Vec::new();
    let mut headers = Headers::new();
    if let Some(json) = claim {
        entries.push(claim_json::protocol_data(json));
        headers.push(CLAIM_HEADER, claim_header_value(json));
    }
    if let Some(json) = challenge {
        entries.push(ProtocolData {
            name: PEER_CHALLENGE_PROTOCOL.to_string(),
            content_type: CONTENT_TYPE_TEXT,
            data: json.as_bytes().to_vec(),
        });
        headers.push(PEER_CHALLENGE_HEADER, peer_challenge_header_value(json));
    }
    let btp = encode_message(request_id, &entries, &prepare_bytes);
    let decoded = decode_frame(&btp).expect("self-generated frame decodes");
    assert_eq!(decoded.ilp_packet, prepare_bytes);

    let via_btp = role_gate::btp_evidence(&decoded).expect("unambiguous evidence");
    let via_http = connector_peer_http::evidence_on(&PeerRequest {
        headers: headers.clone(),
        body: prepare_bytes.clone(),
    })
    .expect("unambiguous evidence");
    for evidence in [&via_btp, &via_http] {
        assert_eq!(evidence.claim.is_some(), claim.is_some());
        assert_eq!(evidence.challenge.is_some(), challenge.is_some());
        assert_eq!(evidence.moves_no_value, prepare.amount == 0);
    }
    assert_eq!(via_btp.claim, via_http.claim, "I1: one voucher, both ways");
    assert_eq!(via_btp.challenge, via_http.challenge);

    PreparePairCase {
        name,
        prepare: prepare_fields(prepare),
        claim_json: claim.map(str::to_string),
        challenge_json: challenge.map(str::to_string),
        btp_message_hex: hex_of(&btp),
        http_headers: headers_pairs(&headers),
        http_body_hex: hex_of(&prepare_bytes),
    }
}

/// The peer-role challenge a paying node signs for a zero-value packet on
/// its outbound channel (ADR 0075 decision 5), rendered by the paying side's
/// own `connector_runtime::challenge_entry`.
fn generate_zero_value_case(voucher: &EvmPresentation) -> PeerZeroValueCase {
    let (signer, signer_address) =
        node_settlement_signer("vector-fixture-peer-payer", PEER_PAYER_KEY_START);
    let domain = BatchSettlementDomain::x402(VOUCHER_EVM_CHAIN_ID);
    let expires = VOUCHER_CLAIM_STATE_EXPIRES;
    let digest = evm_voucher_claim_state_challenge_digest(&domain, &voucher.channel_id, expires);
    let mut signature = signer
        .sign(&digest)
        .expect("fixture signer signs its own digest")
        .to_bytes();
    signature[64] += 27;
    assert!(verify_evm_voucher_claim_state_challenge(
        &domain,
        &voucher.channel_id,
        expires,
        &signature,
        &signer_address,
    ));
    // A challenge is never a voucher: its signature does not verify as one.
    assert!(!verify_evm_voucher(
        &domain,
        &voucher.channel_id,
        u128::from(expires),
        &signature,
        &signer_address,
    ));
    let json = challenge_entry(&voucher.presentation, expires, &signature).to_string();
    let parsed = challenge_json::parse(json.as_bytes()).expect("the challenge parses");
    assert_eq!(parsed.expires(), expires);
    assert_eq!(
        parsed.channel(),
        format!("0x{}", hex_of(&voucher.channel_id))
    );

    let packet = prepare_pair_case(
        "peer_prepare_zero_value_challenge",
        9_003,
        &fixture_prepare(0),
        None,
        Some(&json),
    );
    PeerZeroValueCase {
        name: "peer_zero_value_challenge",
        channel_id_hex: hex_of(&voucher.channel_id),
        expires,
        digest_hex: hex_of(&digest),
        signer_address_hex: hex_of(&signer_address),
        signature_hex: hex_of(&signature),
        packet,
    }
}

fn fixture_fulfill() -> Fulfill {
    Fulfill {
        fulfillment: seq_bytes::<32>(0x51),
        data: b"vector-fixture-fulfill-data".to_vec(),
    }
}

fn fixture_reject() -> Reject {
    Reject {
        code: RejectCode::t04_insufficient_liquidity(),
        triggered_by: "g.toon.store-box".to_string(),
        message: "vector fixture reject".to_string(),
        data: Vec::new(),
        // Never part of `Reject::encode`'s wire bytes (its own doc); the
        // real value this vector pins travels as the carriage's own
        // `accumulated-cost` entry/header, built separately below.
        accumulated_cost: 0,
    }
}

/// [`answer_case`]'s inputs, grouped so the function itself takes one
/// argument instead of a long positional list.
struct AnswerCaseSpec {
    name: String,
    request_id: u32,
    packet: &'static str,
    packet_bytes: Vec<u8>,
    protocol_data: Vec<ProtocolData>,
    http_headers: Vec<(String, String)>,
    ack: Option<AckFields>,
    accumulated_cost: Option<u64>,
}

/// One judged-voucher RESPONSE, on both carriages: `protocol_data` rides the
/// BTP RESPONSE beside `ilp_packet`, and the same fields ride as HTTP
/// headers beside the same body -- always status `200` (§6.2).
fn answer_case(spec: AnswerCaseSpec) -> PeerAnswerCase {
    let btp = encode_response(spec.request_id, &spec.protocol_data, &spec.packet_bytes);
    let decoded = decode_frame(&btp).expect("self-generated frame decodes");
    assert_eq!(decoded.ilp_packet, spec.packet_bytes);

    let mut headers = Headers::new();
    for (name, value) in &spec.http_headers {
        headers.push(name.clone(), value.clone());
    }

    let expected_ack = spec.ack.as_ref().map(|fields| match fields.reason {
        None => ClaimAckOutcome::Accepted,
        Some(reason) => ClaimAckOutcome::Rejected(
            ack::reason_from_name(reason).expect("a name this module itself just wrote"),
        ),
    });
    assert_eq!(
        ack::from_protocol_data(&decoded.protocol_data),
        expected_ack
    );
    assert_eq!(http_claim_ack(&headers), expected_ack);
    if let Some(cost) = spec.accumulated_cost {
        assert_eq!(fields::accumulated_cost(&decoded.protocol_data), cost);
        assert_eq!(http_accumulated_cost(&headers), cost);
    }

    PeerAnswerCase {
        name: spec.name,
        packet: spec.packet,
        packet_hex: hex_of(&spec.packet_bytes),
        ack: spec.ack,
        accumulated_cost: spec.accumulated_cost,
        btp_response_hex: hex_of(&btp),
        http_status: 200,
        http_headers: spec.http_headers,
        http_body_hex: hex_of(&spec.packet_bytes),
    }
}

/// [`generate_answer_cases`]'s output, named so its same-typed
/// [`PeerAnswerCase`] values can't be silently transposed by position.
struct AnswerCases {
    fulfill_ack_accepted: PeerAnswerCase,
    fulfill_ack_rejected: PeerAnswerCase,
    ack_rejected_reasons: Vec<PeerAnswerCase>,
    reject_with_cost: PeerAnswerCase,
    ack_absent: PeerAnswerCase,
}

fn generate_answer_cases() -> AnswerCases {
    let fulfill_bytes = fixture_fulfill().encode();

    // Item 7: a FULFILL, its voucher acknowledged accepted.
    let ack_entry = ack::protocol_data(ClaimAckOutcome::Accepted).expect("a judged voucher");
    let ack_header = claim_ack_header_value(ClaimAckOutcome::Accepted).expect("a judged voucher");
    let fulfill_ack_accepted = answer_case(AnswerCaseSpec {
        name: "peer_fulfill_ack_accepted".to_string(),
        request_id: 9_101,
        packet: "fulfill",
        packet_bytes: fulfill_bytes.clone(),
        protocol_data: vec![ack_entry],
        http_headers: vec![(CLAIM_ACK_HEADER.to_string(), ack_header)],
        ack: Some(AckFields {
            result: "accepted",
            reason: None,
        }),
        accumulated_cost: None,
    });

    // Item 8: **the single most important vector in this set** (§10.2) --
    // a FULFILL answer carrying a *rejected* claim-ack on the same
    // response, pinning §6.2's independence of the two verdicts.
    let rejected_signature_invalid = ClaimAckOutcome::Rejected(ClaimRejectReason::SignatureInvalid);
    let ack_entry = ack::protocol_data(rejected_signature_invalid).expect("a judged voucher");
    let ack_header = claim_ack_header_value(rejected_signature_invalid).expect("a judged voucher");
    let fulfill_ack_rejected = answer_case(AnswerCaseSpec {
        name: "peer_fulfill_ack_rejected".to_string(),
        request_id: 9_102,
        packet: "fulfill",
        packet_bytes: fulfill_bytes.clone(),
        protocol_data: vec![ack_entry],
        http_headers: vec![(CLAIM_ACK_HEADER.to_string(), ack_header)],
        ack: Some(AckFields {
            result: "rejected",
            reason: Some("signature_invalid"),
        }),
        accumulated_cost: None,
    });

    // Item 9: one pair per reason a voucher's verdict can carry (§6.1): a
    // signature that does not recover to the channel's voucher signer, an
    // amount that does not strictly exceed the watermark (or does not cover
    // the packet, or is above the channel's collateral), and a channel the
    // receiving half does not admit. `nonce_not_advancing` stays in the
    // ack's vocabulary for a pre-ADR 0075 decoder, but no voucher verdict
    // produces it, so it is no longer pinned.
    let mut ack_rejected_reasons = Vec::new();
    for (index, reason) in [
        ClaimRejectReason::SignatureInvalid,
        ClaimRejectReason::AmountNotAdvancing,
        ClaimRejectReason::UnknownChannel,
    ]
    .into_iter()
    .enumerate()
    {
        let outcome = ClaimAckOutcome::Rejected(reason);
        let reason_name = ack::reason_name(reason);
        let ack_entry = ack::protocol_data(outcome).expect("a judged voucher");
        let ack_header = claim_ack_header_value(outcome).expect("a judged voucher");
        ack_rejected_reasons.push(answer_case(AnswerCaseSpec {
            name: format!("peer_ack_rejected_{reason_name}"),
            request_id: 9_110 + index as u32,
            packet: "fulfill",
            packet_bytes: fulfill_bytes.clone(),
            protocol_data: vec![ack_entry],
            http_headers: vec![(CLAIM_ACK_HEADER.to_string(), ack_header)],
            ack: Some(AckFields {
                result: "rejected",
                reason: Some(reason_name),
            }),
            accumulated_cost: None,
        }));
    }

    // Item 10: a REJECT carrying accumulated-cost **and** a claim-ack, both
    // on one response.
    let reject_bytes = fixture_reject().encode();
    let accumulated_cost = 4_200u64;
    let cost_entry = fields::accumulated_cost_protocol_data(accumulated_cost);
    let cost_header = accumulated_cost.to_string();
    let ack_entry = ack::protocol_data(ClaimAckOutcome::Accepted).expect("a judged voucher");
    let ack_header = claim_ack_header_value(ClaimAckOutcome::Accepted).expect("a judged voucher");
    let reject_with_cost = answer_case(AnswerCaseSpec {
        name: "peer_reject_with_cost".to_string(),
        request_id: 9_120,
        packet: "reject",
        packet_bytes: reject_bytes,
        protocol_data: vec![cost_entry, ack_entry],
        http_headers: vec![
            (ACCUMULATED_COST_HEADER.to_string(), cost_header),
            (CLAIM_ACK_HEADER.to_string(), ack_header),
        ],
        ack: Some(AckFields {
            result: "accepted",
            reason: None,
        }),
        accumulated_cost: Some(accumulated_cost),
    });

    // Item 11: a response answering a voucher-bearing request with **no**
    // ack at all -- pinned as NOT ACKNOWLEDGED (§6.3), not a verdict.
    let ack_absent = answer_case(AnswerCaseSpec {
        name: "peer_ack_absent".to_string(),
        request_id: 9_130,
        packet: "fulfill",
        packet_bytes: fulfill_bytes,
        protocol_data: Vec::new(),
        http_headers: Vec::new(),
        ack: None,
        accumulated_cost: None,
    });

    AnswerCases {
        fulfill_ack_accepted,
        fulfill_ack_rejected,
        ack_rejected_reasons,
        reject_with_cost,
        ack_absent,
    }
}

fn generate_ack_malformed_case() -> PeerMalformedAckCase {
    let malformed = r#"{"result":"maybe"}"#.to_string();
    assert_eq!(
        ack::decode(malformed.as_bytes()),
        None,
        "§6.3: an unknown result is not acknowledged, never a verdict"
    );
    let entry = ProtocolData {
        name: CLAIM_ACK_PROTOCOL.to_string(),
        content_type: CONTENT_TYPE_TEXT,
        data: malformed.as_bytes().to_vec(),
    };
    assert_eq!(ack::from_protocol_data(&[entry]), None);

    let http_value = BASE64.encode(&malformed);
    let mut headers = Headers::new();
    headers.push(CLAIM_ACK_HEADER, &http_value);
    assert_eq!(http_claim_ack(&headers), None);

    PeerMalformedAckCase {
        name: "peer_ack_malformed",
        malformed_json: malformed.clone(),
        btp_raw_hex: hex_of(malformed.as_bytes()),
        http_base64: http_value,
    }
}

fn generate_forwarded_data_case(giftwrap: &GiftwrapVectors) -> PeerForwardedDataCase {
    let sealed = hex::decode(&giftwrap.cases[0].request_wrap_hex)
        .expect("hex from this run's own giftwrap section");
    let prepare = Prepare {
        amount: 250_000,
        expires_at: "2030-01-01T00:01:00Z".parse().expect("fixed literal"),
        greeting: false,
        destination: "g.toon.store-box.settle".to_string(),
        data: sealed.clone(),
    };
    let prepare_bytes = prepare.encode();
    let decoded = Prepare::decode(&prepare_bytes).expect("self-generated prepare decodes");
    assert_eq!(
        decoded.data, sealed,
        "§8.1: a forwarding hop must carry `data` byte-for-byte unchanged"
    );

    let btp = encode_message(9_199, &[], &prepare_bytes);
    let via_btp = decode_frame(&btp).expect("self-generated frame decodes");
    assert_eq!(via_btp.ilp_packet, prepare_bytes);

    PeerForwardedDataCase {
        name: "peer_forwarded_data_unchanged",
        sealed_data_hex: hex_of(&sealed),
        btp_ilp_packet_prepare_hex: hex_of(&btp),
        http_body_hex: hex_of(&prepare_bytes),
    }
}

fn generate_peer_carriage_vectors(giftwrap: &GiftwrapVectors) -> PeerCarriageVectors {
    let (voucher_evm, evm_presentation) = generate_peer_voucher_evm_case();
    let voucher_solana = generate_peer_voucher_solana_case();
    let prepare_with_voucher = fixture_prepare(250_000);
    let prepare = prepare_pair_case(
        "peer_prepare",
        9_001,
        &prepare_with_voucher,
        Some(&voucher_evm.json),
        None,
    );
    let prepare_no_claim = prepare_pair_case(
        "peer_prepare_no_claim",
        9_002,
        &prepare_with_voucher,
        None,
        None,
    );
    let zero_value_challenge = generate_zero_value_case(&evm_presentation);
    let AnswerCases {
        fulfill_ack_accepted,
        fulfill_ack_rejected,
        ack_rejected_reasons,
        reject_with_cost,
        ack_absent,
    } = generate_answer_cases();
    let ack_malformed = generate_ack_malformed_case();
    let forwarded_data_unchanged = generate_forwarded_data_case(giftwrap);

    PeerCarriageVectors {
        voucher_evm,
        voucher_solana,
        prepare,
        prepare_no_claim,
        zero_value_challenge,
        fulfill_ack_accepted,
        fulfill_ack_rejected,
        ack_rejected_reasons,
        reject_with_cost,
        ack_absent,
        ack_malformed,
        forwarded_data_unchanged,
    }
}

// ---------------------------------------------------------------------
// The retired `toon-channel` claim, refused by name (ADR 0075 decision 8,
// issue #1384)
// ---------------------------------------------------------------------
//
// A claim with no `scheme`, or `scheme: "toon-channel"`, is the retired
// claim. Every edge refuses it **by name** rather than as malformed, so a
// straggler learns to upgrade: the client edge's claim parser
// (`ClientClaimError::ToonChannel`), and each peer carriage before the role
// is decided -- a BTP ERROR frame, and an HTTP `400` whose body names it.

/// One retired claim and every edge's refusal of it.
#[derive(Debug, Serialize)]
pub struct ToonChannelRefusedCase {
    pub name: &'static str,
    pub claim_json: String,
    /// The client edge's refusal, as a stable tag: always `"toon_channel"`.
    pub client_edge_error: &'static str,
    /// Its message, which names the retirement (informational: match on
    /// `client_edge_error`).
    pub client_edge_message: String,
    /// The claim riding a PREPARE on the BTP peer carriage.
    pub btp_message_hex: String,
    /// The ERROR frame answering it: `code F00`, `name NotAcceptedError`,
    /// `data` = [`Self::refusal_text`].
    pub btp_error_hex: String,
    /// The same claim in the HTTP peer carriage's `Payment-Channel-Claim`
    /// header, beside the same PREPARE.
    pub http_headers: Vec<(String, String)>,
    pub http_body_hex: String,
    /// The HTTP answer: `400`, with no ILP body, and the refusal as a
    /// `text/plain` body.
    pub http_status: u16,
    pub http_response_body_hex: String,
    /// What both peer carriages say.
    pub refusal_text: String,
}

#[derive(Debug, Serialize)]
pub struct ToonChannelRefusedVectors {
    pub cases: Vec<ToonChannelRefusedCase>,
}

fn toon_channel_refused_case(
    name: &'static str,
    request_id: u32,
    claim: &serde_json::Value,
) -> ToonChannelRefusedCase {
    let claim_json = claim.to_string();
    let client_error =
        client_claim::parse_client_claim(&claim_json).expect_err("a toon-channel claim is refused");
    assert_eq!(client_error, ClientClaimError::ToonChannel);
    assert_eq!(
        claim_json::parse(claim_json.as_bytes()),
        Err(ClaimDecodeError::ToonChannel)
    );
    let refusal = RefusedEvidence::ToonChannelClaim.message();

    let prepare_bytes = fixture_prepare(250_000).encode();
    let btp = encode_message(
        request_id,
        &[claim_json::protocol_data(&claim_json)],
        &prepare_bytes,
    );
    let decoded = decode_frame(&btp).expect("self-generated frame decodes");
    assert_eq!(
        role_gate::btp_evidence(&decoded).map(|_| ()),
        Err(RefusedEvidence::ToonChannelClaim)
    );
    let btp_error = encode_error(request_id, "F00", "NotAcceptedError", &refusal);

    let mut headers = Headers::new();
    headers.push(CLAIM_HEADER, claim_header_value(&claim_json));
    assert_eq!(
        connector_peer_http::evidence_on(&PeerRequest {
            headers: headers.clone(),
            body: prepare_bytes.clone(),
        })
        .map(|_| ()),
        Err(RefusedEvidence::ToonChannelClaim)
    );
    let http = PeerResponse::refused_naming(400, &refusal);
    assert!(!http.answers_the_packet());

    ToonChannelRefusedCase {
        name,
        claim_json,
        client_edge_error: "toon_channel",
        client_edge_message: client_error.to_string(),
        btp_message_hex: hex_of(&btp),
        btp_error_hex: hex_of(&btp_error),
        http_headers: headers_pairs(&headers),
        http_body_hex: hex_of(&prepare_bytes),
        http_status: http.status,
        http_response_body_hex: hex_of(&http.body),
        refusal_text: String::from_utf8(refusal.to_vec()).expect("the refusal is text"),
    }
}

/// The devnet deploy of TOON's retired Solana payment-channel program: what
/// the refused Solana `toon-channel` claim below names as its `programId`,
/// so the refusal is pinned against the claim a straggling devnet payer
/// actually sends. `docs/deployments/devnet-public.md` records it.
const SOLANA_SETTLEMENT_PROGRAM_ID: &str = "2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip";

fn generate_toon_channel_refused_vectors() -> ToonChannelRefusedVectors {
    // The EVM `toon-channel` claim exactly as a pre-ADR 0075 client or peer
    // sends it: no `scheme`, a nonce, a balance proof.
    let evm = serde_json::json!({
        "version": "1.0",
        "blockchain": "evm",
        "messageId": "vector-fixture:toon-channel:evm",
        "timestamp": FIXTURE_TIMESTAMP,
        "senderId": "0x4444444444444444444444444444444444444444",
        "channelId": format!("0x{}", hex_of(&seq_bytes::<32>(0xa0))),
        "nonce": 4,
        "transferredAmount": "12500",
        "lockedAmount": "0",
        "locksRoot": format!("0x{}", "0".repeat(64)),
        "signature": format!("0x{}", hex_of(&seq_bytes::<65>(0x11))),
        "signerAddress": "0x4444444444444444444444444444444444444444",
        "chainId": VOUCHER_EVM_CHAIN_ID,
        "tokenNetworkAddress": format!("0x{}", hex_of(&[0x42; 20])),
    });
    let mut explicit = evm.clone();
    explicit["scheme"] = "toon-channel".into();
    let solana = serde_json::json!({
        "version": "1.0",
        "blockchain": "solana",
        "messageId": "vector-fixture:toon-channel:solana",
        "timestamp": FIXTURE_TIMESTAMP,
        "senderId": "11111111111111111111111111111113",
        "programId": SOLANA_SETTLEMENT_PROGRAM_ID,
        "channelAccount": "GDDMwNyyx8uB6zrqwBFHjLLG3TBYk2F1Mh6usnNPUsqk",
        "nonce": 1,
        "transferredAmount": "250000",
        "signature": BASE64.encode(seq_bytes::<64>(0xe1)),
        "signerPublicKey": "11111111111111111111111111111113",
    });
    ToonChannelRefusedVectors {
        cases: vec![
            toon_channel_refused_case("toon_channel_evm_no_scheme", 9_401, &evm),
            toon_channel_refused_case("toon_channel_evm_explicit_scheme", 9_402, &explicit),
            toon_channel_refused_case("toon_channel_solana_no_scheme", 9_403, &solana),
        ],
    }
}

// ---------------------------------------------------------------------
// Client payouts (ADR 0075 decision 7, issue #1381)
// ---------------------------------------------------------------------
//
// A connector pays a client back with a voucher on its own outbound channel
// toward the client's payee key, signed by the chain's settlement key, and
// delivers it as a BTP TRANSFER over the client's session: the voucher JSON
// in a `payout-claim` protocolData entry, the TRANSFER's `amount` the
// voucher's cumulative amount (`client-edge-spec.md` §1.9 step 7). The
// client lands it itself. ADR 0026's #1073 correction records the client BTP
// dialect as uncovered by this contract; this section covers the one entry
// of it a client has to parse to be paid.
//
// The entry is the client-edge claim JSON (§1.3) less its envelope
// (`version`, `messageId`, `timestamp`, `senderId`): `blockchain`, `scheme`,
// `channelId`, `maxClaimableAmount`, `signature`, and on EVM always the
// `channelConfig` `claim` needs, on Solana `expiresAt: 0`. Its bytes are
// reproduced here from `connector-client-edge`'s
// `payout_voucher_protocol_data`, which this crate cannot depend on; the
// self-verification below restores the envelope and parses it with the
// client edge's own parser, and checks the signature.

#[derive(Debug, Serialize)]
pub struct PayoutVoucherEvmCase {
    pub name: &'static str,
    #[serde(flatten)]
    pub facts: EvmVoucherFacts,
    /// The `payout-claim` entry's JSON.
    pub json: String,
    /// The TRANSFER carrying it; its `amount` is `max_claimable_amount`.
    pub btp_transfer_hex: String,
}

#[derive(Debug, Serialize)]
pub struct PayoutVoucherSolanaCase {
    pub name: &'static str,
    pub channel_account_base58: String,
    /// The connector's Solana settlement key: the channel's
    /// `authorized_signer`.
    pub authorized_signer_base58: String,
    pub signer_secret_hex: String,
    pub max_claimable_amount: u64,
    pub signed_message_hex: String,
    pub signature_base58: String,
    pub json: String,
    pub btp_transfer_hex: String,
}

#[derive(Debug, Serialize)]
pub struct PayoutVoucherVectors {
    pub evm: PayoutVoucherEvmCase,
    pub solana: PayoutVoucherSolanaCase,
}

/// Restore a payout entry's envelope and parse it as a client would its own
/// claim (§1.3), through the client edge's parser.
fn parse_payout_entry(json: &str) -> ClientClaim {
    let mut claim: serde_json::Value = serde_json::from_str(json).expect("the entry is JSON");
    claim["version"] = "1.0".into();
    claim["messageId"] = "vector-fixture:payout".into();
    claim["timestamp"] = FIXTURE_TIMESTAMP.into();
    claim["senderId"] = "vector-fixture".into();
    client_claim::parse_client_claim(&claim.to_string()).expect("the payout voucher parses")
}

fn payout_transfer(request_id: u32, amount: u64, json: &str) -> Vec<u8> {
    let entry = ProtocolData {
        name: PAYOUT_CLAIM_PROTOCOL.to_string(),
        content_type: CONTENT_TYPE_TEXT,
        data: json.as_bytes().to_vec(),
    };
    let btp = encode_transfer(request_id, amount, &[entry]);
    let decoded = decode_frame(&btp).expect("self-generated frame decodes");
    assert_eq!(decoded.amount, Some(amount));
    assert!(
        decoded.ilp_packet.is_empty(),
        "a TRANSFER carries no ilpPacket"
    );
    let rode = decoded
        .protocol_data
        .iter()
        .find(|pd| pd.name == PAYOUT_CLAIM_PROTOCOL)
        .expect("the payout entry rode");
    assert_eq!(rode.data, json.as_bytes());
    btp
}

fn generate_payout_voucher_vectors() -> PayoutVoucherVectors {
    // EVM: the connector's settlement key pays a client's payee address.
    let (signer, payer) = node_settlement_signer("vector-fixture-connector", 0x61);
    let client_payee: Address = [0x9a; 20];
    let config = outbound_channel_config(payer, client_payee, 0xab);
    let amount: u64 = 42_000;
    let (facts, presentation) = sign_evm_voucher(&signer, &config, amount);
    let address = |bytes: &[u8; 20]| format!("0x{}", hex_of(bytes));
    let mut evm_json = serde_json::json!({
        "scheme": "batch-settlement",
        "channelId": format!("0x{}", hex_of(&presentation.channel_id)),
        "maxClaimableAmount": amount.to_string(),
    });
    evm_json["blockchain"] = "evm".into();
    evm_json["signature"] = format!("0x{}", hex_of(&presentation.signature)).into();
    evm_json["channelConfig"] = serde_json::json!({
        "payer": address(&config.payer),
        "payerAuthorizer": address(&config.payer_authorizer),
        "receiver": address(&config.receiver),
        "receiverAuthorizer": address(&config.receiver_authorizer),
        "token": address(&config.token),
        "withdrawDelay": config.withdraw_delay,
        "salt": format!("0x{}", hex_of(&config.salt)),
    });
    let evm_json = serde_json::to_string(&evm_json).expect("a json! object serializes");
    let ClientClaim::EvmVoucher(voucher) = parse_payout_entry(&evm_json) else {
        panic!("an EVM payout voucher");
    };
    assert_eq!(voucher.max_claimable_amount, amount);
    assert!(voucher.channel_config.is_some());
    let evm = PayoutVoucherEvmCase {
        name: "payout_voucher_evm",
        facts,
        btp_transfer_hex: hex_of(&payout_transfer(9_501, amount, &evm_json)),
        json: evm_json,
    };

    // Solana: the connector's Solana settlement key is `authorized_signer`.
    let seed = seq_bytes::<32>(0x71);
    let solana_signer =
        LocalEd25519Signer::from_secret_bytes(seed).expect("fixture seed is 32 bytes");
    let channel_account: [u8; 32] = [0x7c; 32];
    let solana_amount: u64 = 42_000;
    let message = solana_voucher_message(&channel_account, solana_amount, 0);
    let signature = solana_signer.sign(&message);
    assert!(verify_solana_voucher(
        &channel_account,
        solana_amount,
        0,
        &signature,
        &solana_signer.public_key()
    ));
    let channel_account_base58 = bs58::encode(channel_account).into_string();
    let mut solana_json = serde_json::json!({
        "scheme": "batch-settlement",
        "channelId": channel_account_base58,
        "maxClaimableAmount": solana_amount.to_string(),
    });
    solana_json["blockchain"] = "solana".into();
    solana_json["expiresAt"] = 0.into();
    solana_json["signature"] = bs58::encode(signature).into_string().into();
    let solana_json = serde_json::to_string(&solana_json).expect("a json! object serializes");
    assert!(matches!(
        parse_payout_entry(&solana_json),
        ClientClaim::SolanaVoucher(_)
    ));
    let solana = PayoutVoucherSolanaCase {
        name: "payout_voucher_solana",
        channel_account_base58,
        authorized_signer_base58: bs58::encode(solana_signer.public_key()).into_string(),
        signer_secret_hex: hex_of(&seed),
        max_claimable_amount: solana_amount,
        signed_message_hex: hex_of(&message),
        signature_base58: bs58::encode(signature).into_string(),
        btp_transfer_hex: hex_of(&payout_transfer(9_502, solana_amount, &solana_json)),
        json: solana_json,
    };

    PayoutVoucherVectors { evm, solana }
}

/// What a route charges for one packet, as a function of that packet's
/// payload length ([ADR 0065](../../../docs/adr/0065-a-price-is-a-schedule-over-payload-length.md)).
///
/// **Not bytes on the wire, and deliberately in the set anyway.** Every other
/// section here pins an encoding; this one pins arithmetic. It earns its place
/// because the arithmetic is a *cross-repo* contract exactly like an encoding
/// is: a sender computes the charge itself before it sends (a payload length is
/// a property of carriage, which is what lets any hop price a packet without
/// opening its wrap), so `toon-client`, `rig` and `swap` each reimplement this
/// function and each must get the same answer as `connector_domain::Price`. One
/// of them did not -- `toon-client`'s `chargeFor` computed
/// `floor(len / 1024) + 1` and overpaid by a whole kibibyte at every exact
/// multiple of 1024 (toon-client#629) -- and nothing caught it, because prose
/// was the only thing binding the two: the client's own docstring stated the
/// correct rule three lines above the code that contradicted it.
#[derive(Debug, Serialize)]
pub struct ChargeVectors {
    pub cases: Vec<ChargeCase>,
}

/// One row of a price schedule: what `base + per_kib/KiB` charges for a packet
/// whose `Prepare.data` is `payload_len` bytes.
#[derive(Debug, Serialize)]
pub struct ChargeCase {
    pub name: &'static str,
    /// **Decimal strings, not JSON numbers** -- unlike `claim`'s
    /// `transferred_amount`, whose fixture is small. These three fields reach
    /// `u64::MAX` on the saturating rows, which is past 2^53 and would be
    /// silently rounded by any reader that parses JSON numbers as IEEE
    /// doubles. It is the same reason `GET /ilp` publishes `price` as a string.
    pub base: String,
    pub per_kib: String,
    /// `Prepare.data.len()`: the length of the sealed gift wrap, never anything
    /// inside it, and never the encoded packet around it.
    pub payload_len: u64,
    /// `ceil(payload_len / 1024)` -- kibibytes *started*, and zero for an empty
    /// payload. Stated separately from `charge` so a replaying SDK that gets
    /// the total right by luck still fails on the unit count.
    pub kib: u64,
    pub charge: String,
    /// Whether `u64` saturation clamped this row, i.e. whether the exact
    /// arithmetic would have exceeded `u64::MAX`. An SDK computing in
    /// arbitrary-precision integers (`BigInt`, `int`) will not clamp on its
    /// own and must be told where the ceiling is: an amount past `u64::MAX`
    /// cannot even be encoded into the packet it would be paying for.
    pub saturated: bool,
}

/// Builds and self-verifies one [`ChargeCase`].
///
/// The charge comes from the real [`Price::charge`], then the rule is applied a
/// second time here in **checked** arithmetic, written out longhand rather than
/// with `div_ceil`, and the two are compared. That is what makes the row
/// evidence rather than a transcript: a typo in either expression -- a `floor`
/// where a `ceil` belongs, most of all -- fails the build instead of being
/// committed as the contract.
fn charge_case(name: &'static str, base: u64, per_kib: u64, payload_len: usize) -> ChargeCase {
    let price = Price::scheduled(base, per_kib);
    let charge = price.charge(payload_len);

    // Whole kibibytes, plus one more if anything is left over. Longhand, so
    // this is not the same expression `Price::charge` uses.
    let kib = payload_len / 1024 + usize::from(!payload_len.is_multiple_of(1024));
    let kib = u64::try_from(kib).expect("a fixture payload length fits in a u64");

    // `None` exactly when the schedule overflows a u64 -- the case
    // `Price::charge` saturates rather than panicking on.
    let exact = per_kib
        .checked_mul(kib)
        .and_then(|slope| base.checked_add(slope));
    let saturated = exact.is_none();
    match exact {
        Some(exact) => assert_eq!(
            charge, exact,
            "{name}: Price::charge disagrees with base + per_kib * ceil(len / 1024)"
        ),
        None => assert_eq!(
            charge,
            u64::MAX,
            "{name}: an overflowing schedule must saturate, not wrap"
        ),
    }

    ChargeCase {
        name,
        base: base.to_string(),
        per_kib: per_kib.to_string(),
        payload_len: u64::try_from(payload_len).expect("a fixture payload length fits in a u64"),
        kib,
        charge: charge.to_string(),
        saturated,
    }
}

/// The metered rows use `1000 + 10/KiB`, which is not an invented figure: it is
/// what the fleet's deployed store node charges, and the row at 5161 bytes is
/// the one independently confirmed against that live node (its x402 greeting
/// quotes `price.charge(prepare.data.len())` for the packet it was handed).
///
/// The lengths are chosen as the boundaries and their neighbours, because that
/// is the only place two plausible readings of "per kibibyte" differ: 0, 1,
/// 1023/1024/1025 and 2048/2049. A vector set that sampled only round-ish
/// middles -- which is what the client had been checked against -- cannot tell
/// `ceil` from `floor + 1` at all.
fn generate_charge_vectors() -> ChargeVectors {
    const BASE: u64 = 1_000;
    const PER_KIB: u64 = 10;

    let mut cases = vec![
        // An empty payload starts no kibibyte and pays the base alone. The row
        // that most often comes out wrong, because "kibibytes started, counting
        // from one" reads as though the floor were one rather than zero.
        charge_case("metered_empty_payload", BASE, PER_KIB, 0),
        charge_case("metered_one_byte", BASE, PER_KIB, 1),
        charge_case("metered_just_under_one_kib", BASE, PER_KIB, 1023),
        // The boundary. A whole kibibyte is ONE kibibyte.
        charge_case("metered_exactly_one_kib", BASE, PER_KIB, 1024),
        // ...and the next byte starts the second.
        charge_case("metered_one_byte_past_one_kib", BASE, PER_KIB, 1025),
        charge_case("metered_exactly_two_kib", BASE, PER_KIB, 2048),
        charge_case("metered_one_byte_past_two_kib", BASE, PER_KIB, 2049),
        // Confirmed against the deployed store node: 5161 bytes is quoted 1060.
        charge_case("metered_live_measured_5161", BASE, PER_KIB, 5161),
    ];

    // A flat price is a schedule whose slope is zero (ADR 0065): the same value,
    // not merely an equivalent one, so it must charge its base at every length
    // -- including the lengths above where the metered rows all differ.
    cases.extend([
        charge_case("flat_empty_payload", BASE, 0, 0),
        charge_case("flat_one_byte", BASE, 0, 1),
        charge_case("flat_exactly_one_kib", BASE, 0, 1024),
        charge_case("flat_one_mib", BASE, 0, 1024 * 1024),
    ]);

    // Saturation, both ways it can arise. An operator can write a schedule that
    // overflows a u64 on a large payload, and the answer is then `u64::MAX` --
    // a charge no claim can cover, which refuses the packet. Wrapping or
    // panicking on the packet path would both be worse.
    cases.extend([
        charge_case("saturating_base", u64::MAX, 1, 1),
        charge_case("saturating_slope", 0, u64::MAX, 2049),
    ]);

    ChargeVectors { cases }
}

// ---------------------------------------------------------------------
// x402 batch-settlement vouchers (ADR 0074 decision 7, issue #1347)
// ---------------------------------------------------------------------
//
// A client-edge claim gains a `scheme` discriminator (issue #1341,
// `connector_domain::client_claim`); under `scheme: "batch-settlement"` it
// is a **voucher** -- no nonce, ordered by its cumulative amount alone
// (`connector_domain::validate_voucher`), verified against a different
// signature scheme per chain (`connector_signer::voucher_signature`).
// Since ADR 0075 it is the only claim, on every edge: this section pins the
// client-edge shape, and `peer_carriage.voucher_evm`/`voucher_solana` pin a
// peer's, with its BTP/HTTP framing pair.
//
// **Live cross-check, 2026-09-25.** The EVM fixture below is not only this
// crate's own arithmetic: `channel_id_hex` and `digest_hex` were
// independently confirmed against the deployed `x402BatchSettlement`
// contract itself on Base Sepolia (chain 84532) at
// `0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003`, block 47289378, over
// `https://sepolia.base.org`:
//
// ```text
// $ cast call 0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003 \
//     "getChannelId((address,address,address,address,address,uint40,bytes32))(bytes32)" \
//     "(0x1111111111111111111111111111111111111111,0x70997970C51812dc3A010C7d01b50e0d17dc79C8,0x3333333333333333333333333333333333333333,0x4444444444444444444444444444444444444444,0x5555555555555555555555555555555555555555,86400,0x6666666666666666666666666666666666666666666666666666666666666666)" \
//     --rpc-url https://sepolia.base.org
// 0x88d37e9be679d5e46c7c1d073e6f41b5ec07cc5099319a49b80ba460f0d8055d
//
// $ cast call 0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003 \
//     "getVoucherDigest(bytes32,uint128)(bytes32)" \
//     0x88d37e9be679d5e46c7c1d073e6f41b5ec07cc5099319a49b80ba460f0d8055d 5000 \
//     --rpc-url https://sepolia.base.org
// 0x0485b41befd093f42efa53a626e86749b97998f0243e2c7f74b179851d880a8a
// ```
//
// Both equal what `evm_batch_channel_id`/`evm_voucher_digest` compute for
// the fixture below, byte for byte -- so this vector's `channel_id_hex` and
// `digest_hex` are the deployed contract's own answer, not merely this
// crate's. (The same call also confirmed the zero-`payerAuthorizer` channel
// id `0xd90283498ac1b4b04c73bf57672c0d51d7f125f0d65c6f5b8e3e0800035867b4`,
// the `u128::MAX`-amount digest
// `0x3fd853e98fcb965aa64d4c55527447057c5e44f673e2c1fb17d10211d8eb569e`, and
// `VOUCHER_TYPEHASH() == 0x1e1bd6ff84c3e0d9029a292b212e039c0ca97ec497c55191a4a5874294609a69`
// -- the same figures `connector-signer`'s own `voucher_signature.rs`
// fixture pins, so neither crate's copy of this fixture has drifted from
// the chain both name.) `cast wallet sign --no-hash` over that same digest
// with anvil's account 1 key (`0x59c6995e…690d`, address
// `0x70997970…79C8`) produced `signature_hex` below -- exactly
// `connector-signer`'s own `cast_signature()`, reused rather than a second,
// independently chosen one.

const VOUCHER_EVM_CHAIN_ID: u64 = 84_532;

fn voucher_evm_fixture_config() -> BatchChannelConfig {
    BatchChannelConfig {
        payer: [0x11; 20],
        payer_authorizer: hex_bytes("70997970C51812dc3A010C7d01b50e0d17dc79C8"),
        receiver: [0x33; 20],
        receiver_authorizer: [0x44; 20],
        token: [0x55; 20],
        withdraw_delay: 86_400,
        salt: [0x66; 32],
    }
}

/// An x402 `ChannelConfig`'s seven fields, as the vector reports them --
/// see [`VoucherEvmCase::json`] for the same fields as they ride the wire.
#[derive(Debug, Serialize)]
pub struct VoucherEvmChannelConfigFields {
    pub payer_hex: String,
    pub payer_authorizer_hex: String,
    pub receiver_hex: String,
    pub receiver_authorizer_hex: String,
    pub token_hex: String,
    pub withdraw_delay: u64,
    pub salt_hex: String,
}

/// `claim_voucher_evm`: an x402 EVM voucher (ADR 0074 decision 4, issue
/// #1341) -- the claim JSON, its `ChannelConfig`, the `channelId` it hashes
/// to, the EIP-712 digest that channel id and amount hash to, and the
/// signature over that digest.
#[derive(Debug, Serialize)]
pub struct VoucherEvmCase {
    pub name: &'static str,
    /// The EIP-712 domain's `chainId` -- Base Sepolia's real id, the same
    /// domain the live cross-check above queried, not a made-up one.
    pub chain_id: u64,
    /// The EIP-712 domain's `verifyingContract`: `x402BatchSettlement`'s
    /// one deployed address, the same on every chain it is deployed to.
    pub verifying_contract_hex: String,
    pub channel_config: VoucherEvmChannelConfigFields,
    /// `getChannelId(channel_config)` -- what the voucher's `channelId`
    /// resolves to, and what a connector must recompute and match before
    /// trusting a channel's first-presented config (ADR 0074 decision 2).
    pub channel_id_hex: String,
    pub max_claimable_amount: u64,
    /// `getVoucherDigest(channel_id_hex, max_claimable_amount)` -- what
    /// `signature_hex` actually signs.
    pub digest_hex: String,
    /// `evm_voucher_signer(channel_config)`: `payerAuthorizer` here, since
    /// it is nonzero (ADR 0074 decision 4).
    pub signer_address_hex: String,
    /// `r ‖ s ‖ v`, 65 bytes -- the wallet-convention signature a real
    /// voucher carries, recovering to `signer_address_hex` over
    /// `digest_hex`.
    pub signature_hex: String,
    /// The full claim, exactly as it rides the `ILP-Payment-Channel-Claim`
    /// header/protocolData entry.
    pub json: String,
}

fn generate_voucher_evm_case() -> VoucherEvmCase {
    let domain = BatchSettlementDomain::x402(VOUCHER_EVM_CHAIN_ID);
    let config = voucher_evm_fixture_config();
    let channel_id = evm_batch_channel_id(&domain, &config);
    let max_claimable_amount: u64 = 5_000;
    let signature: [u8; 65] = hex_bytes(
        "6be416a12f0d5af512c04435315cc4235e915536d73175e05b08b597c160f0ac463b79066298bea17a15716eedbf07231ffbc514295c9057d8c195bebc8566241b",
    );
    let digest = evm_voucher_digest(&domain, &channel_id, u128::from(max_claimable_amount));
    let signer = evm_voucher_signer(&config);
    assert!(
        verify_evm_voucher(
            &domain,
            &channel_id,
            u128::from(max_claimable_amount),
            &signature,
            &signer,
        ),
        "the fixture voucher signature must verify against its own payerAuthorizer"
    );

    let channel_id_hex_0x = format!("0x{}", hex_of(&channel_id));
    let json = serde_json::json!({
        "version": "1.0",
        "blockchain": "evm",
        "scheme": SCHEME_BATCH_SETTLEMENT,
        "messageId": "vector-fixture:voucher:evm:1",
        "timestamp": "2030-01-01T00:00:00.000Z",
        "senderId": format!("0x{}", hex_of(&signer)),
        "channelId": channel_id_hex_0x,
        "maxClaimableAmount": max_claimable_amount.to_string(),
        "signature": format!("0x{}", hex_of(&signature)),
        "channelConfig": {
            "payer": format!("0x{}", hex_of(&config.payer)),
            "payerAuthorizer": format!("0x{}", hex_of(&config.payer_authorizer)),
            "receiver": format!("0x{}", hex_of(&config.receiver)),
            "receiverAuthorizer": format!("0x{}", hex_of(&config.receiver_authorizer)),
            "token": format!("0x{}", hex_of(&config.token)),
            "withdrawDelay": config.withdraw_delay,
            "salt": format!("0x{}", hex_of(&config.salt)),
        },
    })
    .to_string();

    // I4/I1 (in the `peer_carriage` sense): the emitted claim parses back
    // through the real client-edge claim parser -- ADR 0074 decision 2's
    // own discipline, not merely this generator's.
    let parsed = client_claim::parse_client_claim(&json).expect("the emitted voucher parses");
    let ClientClaim::EvmVoucher(voucher) = &parsed else {
        panic!("expected an EVM voucher, got {parsed:?}");
    };
    assert_eq!(voucher.channel_id, channel_id_hex_0x);
    assert_eq!(voucher.max_claimable_amount, max_claimable_amount);
    let parsed_config = voucher
        .channel_config
        .as_ref()
        .expect("a channel's first voucher carries its channelConfig");
    assert_eq!(parsed_config.withdraw_delay, config.withdraw_delay);

    VoucherEvmCase {
        name: "claim_voucher_evm",
        chain_id: VOUCHER_EVM_CHAIN_ID,
        verifying_contract_hex: hex_of(&X402_BATCH_SETTLEMENT_ADDRESS),
        channel_config: VoucherEvmChannelConfigFields {
            payer_hex: hex_of(&config.payer),
            payer_authorizer_hex: hex_of(&config.payer_authorizer),
            receiver_hex: hex_of(&config.receiver),
            receiver_authorizer_hex: hex_of(&config.receiver_authorizer),
            token_hex: hex_of(&config.token),
            withdraw_delay: config.withdraw_delay,
            salt_hex: hex_of(&config.salt),
        },
        channel_id_hex: hex_of(&channel_id),
        max_claimable_amount,
        digest_hex: hex_of(&digest),
        signer_address_hex: hex_of(&signer),
        signature_hex: hex_of(&signature),
        json,
    }
}

/// `claim_voucher_solana`: an x402 SVM voucher (ADR 0074 decision 4, issue
/// #1341) -- the claim JSON, the 50-byte message its signature covers, and
/// the signature itself.
#[derive(Debug, Serialize)]
pub struct VoucherSolanaCase {
    pub name: &'static str,
    pub channel_account_hex: String,
    pub channel_account_base58: String,
    /// The channel account's `authorized_signer` -- the key a voucher on
    /// this channel must be signed by (ADR 0074 decision 4), read from the
    /// chain and never from the claim.
    pub signer_public_key_hex: String,
    pub signer_public_key_base58: String,
    pub max_claimable_amount: u64,
    /// Always `0` (ADR 0074 decision 3); see `invalid[]` for the refusal of
    /// anything else.
    pub expires_at: i64,
    /// [`solana_voucher_message`]'s 50 bytes: `0x5601 ‖ channel_account ‖
    /// cumulative_amount LE ‖ expires_at LE` -- what `signature_hex`
    /// actually covers.
    pub signed_message_hex: String,
    pub signature_hex: String,
    pub signature_base58: String,
    /// The full claim, exactly as it rides the `ILP-Payment-Channel-Claim`
    /// header/protocolData entry.
    pub json: String,
}

fn generate_voucher_solana_case() -> VoucherSolanaCase {
    let channel_account: [u8; 32] = [0xc3; 32];
    let signer_public_key: [u8; 32] =
        hex_bytes("884b8857f4eaa1613c61504db34d4beaf346517a0e31de3cddd4d9b4201d9d0b");
    let max_claimable_amount: u64 = 5_000;
    let expires_at: i64 = 0;
    let signature: [u8; 64] = hex_bytes(
        "347482945bb1d06372454c0f88c48934e7a0ab8042553132d4abb9937125281154f3bf945db285c9dd5bf1e252592b2d8122aa4ad357675f08a3590f0fa9a405",
    );

    let signed_message = solana_voucher_message(&channel_account, max_claimable_amount, expires_at);
    assert!(
        verify_solana_voucher(
            &channel_account,
            max_claimable_amount,
            expires_at,
            &signature,
            &signer_public_key,
        ),
        "the fixture voucher signature must verify against its own authorized_signer"
    );

    let channel_account_base58 = bs58::encode(channel_account).into_string();
    let signer_public_key_base58 = bs58::encode(signer_public_key).into_string();
    let signature_base58 = bs58::encode(signature).into_string();

    let json = serde_json::json!({
        "version": "1.0",
        "blockchain": "solana",
        "scheme": SCHEME_BATCH_SETTLEMENT,
        "messageId": "vector-fixture:voucher:solana:1",
        "timestamp": "2030-01-01T00:00:00.000Z",
        "senderId": signer_public_key_base58.clone(),
        "channelId": channel_account_base58.clone(),
        "maxClaimableAmount": max_claimable_amount.to_string(),
        "expiresAt": expires_at,
        "signature": signature_base58.clone(),
    })
    .to_string();

    let parsed = client_claim::parse_client_claim(&json).expect("the emitted voucher parses");
    let ClientClaim::SolanaVoucher(voucher) = &parsed else {
        panic!("expected a Solana voucher, got {parsed:?}");
    };
    assert_eq!(voucher.channel_id, channel_account_base58);
    assert_eq!(voucher.max_claimable_amount, max_claimable_amount);

    VoucherSolanaCase {
        name: "claim_voucher_solana",
        channel_account_hex: hex_of(&channel_account),
        channel_account_base58,
        signer_public_key_hex: hex_of(&signer_public_key),
        signer_public_key_base58,
        max_claimable_amount,
        expires_at,
        signed_message_hex: hex_of(&signed_message),
        signature_hex: hex_of(&signature),
        signature_base58,
        json,
    }
}

/// One outcome of [`validate_voucher`] (ADR 0074 decision 3): the
/// amount-only rule a voucher's freshness is judged by, in place of the
/// nonce the retired `toon-channel` claim used.
#[derive(Debug, Serialize)]
pub struct VoucherWatermarkCase {
    pub name: &'static str,
    /// `None` for a channel that has never accepted a voucher; otherwise
    /// the amount and signature of the voucher that set the watermark.
    pub watermark_amount: Option<u64>,
    pub watermark_signature_hex: Option<String>,
    pub presented_amount: u64,
    pub presented_signature_hex: String,
    pub charge: u64,
    /// `"advances"`, `"retransmission"`, `"amount_not_advancing"` or
    /// `"underpayment"`.
    pub outcome: &'static str,
    /// Set only when `outcome` is `"advances"`.
    pub advanced: Option<u64>,
}

/// Builds and self-verifies one [`VoucherWatermarkCase`]: calls the real
/// [`validate_voucher`] and asserts its result is `expected_outcome`
/// (`advanced` too, on `"advances"`) before emitting the case -- this
/// generator cannot silently commit a row its own domain rule disagrees
/// with.
#[allow(clippy::too_many_arguments)]
fn voucher_watermark_case(
    name: &'static str,
    watermark: Option<(u64, &[u8])>,
    presented_amount: u64,
    presented_signature: &[u8],
    charge: u64,
    expected_outcome: &'static str,
    expected_advanced: Option<u64>,
) -> VoucherWatermarkCase {
    let domain_watermark = watermark.map(|(amount, signature)| VoucherWatermark {
        cumulative_amount: amount,
        signature,
    });
    let result = validate_voucher(
        domain_watermark,
        presented_amount,
        presented_signature,
        charge,
    );

    match (expected_outcome, expected_advanced, &result) {
        ("advances", Some(expected), Ok(VoucherAdmission::Advances { advanced })) => {
            assert_eq!(
                *advanced, expected,
                "vector {name} advanced a different amount than expected"
            );
        }
        ("retransmission", None, Ok(VoucherAdmission::Retransmission)) => {}
        ("amount_not_advancing", None, Err(ClaimError::AmountNotAdvancing { .. })) => {}
        ("underpayment", None, Err(ClaimError::Underpayment { .. })) => {}
        _ => panic!(
            "vector {name}: validate_voucher returned {result:?}, expected {expected_outcome:?} \
             (advanced {expected_advanced:?})"
        ),
    }

    VoucherWatermarkCase {
        name,
        watermark_amount: watermark.map(|(amount, _)| amount),
        watermark_signature_hex: watermark.map(|(_, signature)| hex_of(signature)),
        presented_amount,
        presented_signature_hex: hex_of(presented_signature),
        charge,
        outcome: expected_outcome,
        advanced: expected_advanced,
    }
}

/// The amount-only-watermark outcomes ADR 0074 decision 7 asks the
/// vectors to pin: an equal amount under a *different* signature is
/// refused, a strictly higher amount is accepted, and a voucher
/// byte-identical to the one at the watermark -- same amount, same
/// signature -- is a retransmission: accepted again, buying nothing
/// new. Because it buys nothing, the same retransmission against a
/// nonzero charge covers none of it, and is refused as an underpayment
/// (decision 3, amended 2026-09-25).
fn generate_voucher_watermark_cases() -> Vec<VoucherWatermarkCase> {
    let first_signature = seq_bytes::<65>(0xf1);
    let second_signature = seq_bytes::<65>(0xf2);

    vec![
        voucher_watermark_case(
            "voucher_amount_equal_to_watermark_is_refused",
            Some((1_000, &first_signature)),
            1_000,
            &second_signature,
            0,
            "amount_not_advancing",
            None,
        ),
        voucher_watermark_case(
            "voucher_amount_above_watermark_is_accepted",
            Some((1_000, &first_signature)),
            1_500,
            &second_signature,
            500,
            "advances",
            Some(500),
        ),
        voucher_watermark_case(
            "byte_identical_voucher_retransmission_is_accepted_again",
            Some((1_000, &first_signature)),
            1_000,
            &first_signature,
            0,
            "retransmission",
            None,
        ),
        voucher_watermark_case(
            "byte_identical_voucher_retransmission_against_a_charge_is_underpayment",
            Some((1_000, &first_signature)),
            1_000,
            &first_signature,
            100,
            "underpayment",
            None,
        ),
    ]
}

/// A claim [`client_claim::parse_client_claim`] structurally refuses under
/// the `batch-settlement` scheme -- alongside `envelope`'s `invalid[]`,
/// same idea: `expected_error` is a stable tag, not `Debug` output a
/// reformat could change.
#[derive(Debug, Serialize)]
pub struct VoucherInvalidClaimCase {
    pub name: &'static str,
    pub claim_json: String,
    pub expected_error: &'static str,
}

/// ADR 0074 decision 3: a Solana voucher's `expiresAt` must be `0`. x402
/// itself requires this, and the program would refuse a nonzero one at
/// `settle` with no state change -- so the connector refuses it
/// structurally, before any signature check, rather than accept value that
/// could lapse before it lands one. Reuses `solana`'s own channel, signer
/// and signature bytes -- only `expiresAt` in the JSON changes, which is
/// exactly what makes this a structural refusal rather than a signature
/// failure: nothing here re-verifies the signature at all.
fn generate_voucher_invalid_cases(solana: &VoucherSolanaCase) -> Vec<VoucherInvalidClaimCase> {
    let expires_at = 1i64;
    let claim_json = serde_json::json!({
        "version": "1.0",
        "blockchain": "solana",
        "scheme": SCHEME_BATCH_SETTLEMENT,
        "messageId": "vector-fixture:voucher:solana:expires",
        "timestamp": "2030-01-01T00:00:00.000Z",
        "senderId": solana.signer_public_key_base58,
        "channelId": solana.channel_account_base58,
        "maxClaimableAmount": solana.max_claimable_amount.to_string(),
        "expiresAt": expires_at,
        "signature": solana.signature_base58,
    })
    .to_string();

    let err = client_claim::parse_client_claim(&claim_json)
        .expect_err("a nonzero expiresAt must be refused");
    assert_eq!(err, ClientClaimError::VoucherExpires { expires_at });

    vec![VoucherInvalidClaimCase {
        name: "claim_voucher_solana_expires_at_nonzero",
        claim_json,
        expected_error: "voucher_expires",
    }]
}

/// ADR 0074 decision 7 / issue #1347: the x402 batch-settlement voucher's
/// wire vectors. See this section's own doc comment above for the live
/// cross-check against the deployed EVM contract.
#[derive(Debug, Serialize)]
pub struct ClaimVoucherVectors {
    pub evm: VoucherEvmCase,
    pub solana: VoucherSolanaCase,
    pub amount_only_watermark: Vec<VoucherWatermarkCase>,
    pub invalid: Vec<VoucherInvalidClaimCase>,
}

fn generate_claim_voucher_vectors() -> ClaimVoucherVectors {
    let evm = generate_voucher_evm_case();
    let solana = generate_voucher_solana_case();
    let amount_only_watermark = generate_voucher_watermark_cases();
    let invalid = generate_voucher_invalid_cases(&solana);

    ClaimVoucherVectors {
        evm,
        solana,
        amount_only_watermark,
        invalid,
    }
}

// ---------------------------------------------------------------------
// Claim state for an x402 batch-settlement channel (issue #1364,
// `docs/protocol/client-edge-spec.md` §1.10)
// ---------------------------------------------------------------------
//
// A `POST /ilp/claim-state` entry under `scheme: "batch-settlement"` is
// proved by the channel's voucher signer over a claim-state challenge kept
// apart from its vouchers (`connector_signer::claim_state_challenge`): on EVM
// `ClaimStateChallenge(bytes32 channelId,uint256 expires)` under
// `x402BatchSettlement`'s EIP-712 domain, on Solana a message tagged
// `toon-voucher-claim-state-challenge-v1`. Each case is on the same channel
// as the `claim_voucher` section's, signed through the real digest and
// self-verified through the real verifier before it is emitted.

/// One EVM batch-settlement claim-state entry.
#[derive(Debug, Serialize)]
pub struct VoucherClaimStateEvmCase {
    pub name: &'static str,
    /// The EIP-712 domain's `chainId`, as in `claim_voucher.evm`.
    pub chain_id: u64,
    /// The EIP-712 domain's `verifyingContract`: `x402BatchSettlement`.
    pub verifying_contract_hex: String,
    /// `claim_voucher.evm`'s channel.
    pub channel_id_hex: String,
    /// Unix seconds, checked against the verifier's clock, not encoded in
    /// the signature's verdict.
    pub expires: u64,
    /// `evm_voucher_signer(channel_config)` -- the key `signature_hex` must
    /// recover to: `claim_voucher.evm.signer_address_hex`.
    pub voucher_signer_address_hex: String,
    pub signer_secret_hex: String,
    pub signer_address_hex: String,
    /// `keccak256(0x1901 ‖ x402DomainSeparator ‖ structHash)` for
    /// `ClaimStateChallenge(bytes32 channelId,uint256 expires)`.
    pub digest_hex: String,
    /// `r ‖ s ‖ v`, 65 bytes, `v` 27 or 28, `0x`-prefixed.
    pub signature_hex: String,
    /// Whether `signature_hex` recovers to `voucher_signer_address_hex`.
    pub signature_verifies: bool,
    /// The entry, byte-for-byte as it rides in the request's `channels[]`.
    /// It carries the channel's `channelConfig`, which a node that has not
    /// yet accepted a voucher on the channel needs and one that has
    /// ignores.
    pub entry_json: String,
}

/// One Solana batch-settlement claim-state entry.
#[derive(Debug, Serialize)]
pub struct VoucherClaimStateSolanaCase {
    pub name: &'static str,
    pub channel_account_base58: String,
    pub expires: u64,
    /// The channel account's `authorized_signer`, as base58.
    pub authorized_signer_base58: String,
    /// The Ed25519 seed of the key that signed.
    pub signer_secret_hex: String,
    pub signer_public_key_base58: String,
    /// `"toon-voucher-claim-state-challenge-v1" ‖ channelAccount ‖ expires
    /// (u64 LE)` -- what `signature_base64` covers.
    pub signed_message_hex: String,
    /// Base64, as every Solana claim-state signature is.
    pub signature_base64: String,
    pub signature_verifies: bool,
    pub entry_json: String,
}

#[derive(Debug, Serialize)]
pub struct VoucherClaimStateChallengeVectors {
    pub evm: Vec<VoucherClaimStateEvmCase>,
    pub solana: Vec<VoucherClaimStateSolanaCase>,
}

/// 2100-01-01T00:00:00Z: far enough ahead to be valid against any clock.
const VOUCHER_CLAIM_STATE_EXPIRES: u64 = 4_102_444_800;

/// anvil's account 1 (`0x59c6995e…690d`), the `payerAuthorizer` of
/// `claim_voucher.evm`'s channel -- a published development key, never a
/// real one.
const VOUCHER_EVM_AUTHORIZER_SECRET: &str =
    "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

fn voucher_claim_state_evm_case(
    name: &'static str,
    signer_secret: [u8; 32],
    expect_verifies: bool,
) -> VoucherClaimStateEvmCase {
    let domain = BatchSettlementDomain::x402(VOUCHER_EVM_CHAIN_ID);
    let config = voucher_evm_fixture_config();
    let channel_id = evm_batch_channel_id(&domain, &config);
    let voucher_signer = evm_voucher_signer(&config);
    let expires = VOUCHER_CLAIM_STATE_EXPIRES;

    let signer = LocalSigner::from_secret_bytes(name, signer_secret)
        .expect("fixture secret is a valid secp256k1 scalar");
    let signer_address = derive_evm_address(&signer.public_key().expect("fixture has a key"));
    let digest = evm_voucher_claim_state_challenge_digest(&domain, &channel_id, expires);
    let mut signature = signer
        .sign(&digest)
        .expect("fixture signer signs its own digest")
        .to_bytes();
    // The wallet convention a voucher's own signature uses.
    signature[64] += 27;

    let signature_verifies = verify_evm_voucher_claim_state_challenge(
        &domain,
        &channel_id,
        expires,
        &signature,
        &voucher_signer,
    );
    assert_eq!(
        signature_verifies, expect_verifies,
        "vector {name} computed the wrong verification verdict"
    );

    let signature_hex = format!("0x{}", hex_of(&signature));
    let entry_json = serde_json::json!({
        "blockchain": "evm",
        "scheme": SCHEME_BATCH_SETTLEMENT,
        "channelId": format!("0x{}", hex_of(&channel_id)),
        "expires": expires,
        "signature": signature_hex,
        "channelConfig": {
            "payer": format!("0x{}", hex_of(&config.payer)),
            "payerAuthorizer": format!("0x{}", hex_of(&config.payer_authorizer)),
            "receiver": format!("0x{}", hex_of(&config.receiver)),
            "receiverAuthorizer": format!("0x{}", hex_of(&config.receiver_authorizer)),
            "token": format!("0x{}", hex_of(&config.token)),
            "withdrawDelay": config.withdraw_delay,
            "salt": format!("0x{}", hex_of(&config.salt)),
        },
    })
    .to_string();

    VoucherClaimStateEvmCase {
        name,
        chain_id: VOUCHER_EVM_CHAIN_ID,
        verifying_contract_hex: hex_of(&X402_BATCH_SETTLEMENT_ADDRESS),
        channel_id_hex: hex_of(&channel_id),
        expires,
        voucher_signer_address_hex: hex_of(&voucher_signer),
        signer_secret_hex: hex_of(&signer_secret),
        signer_address_hex: hex_of(&signer_address),
        digest_hex: hex_of(&digest),
        signature_hex,
        signature_verifies,
        entry_json,
    }
}

fn voucher_claim_state_solana_case(
    name: &'static str,
    authorized_signer: [u8; 32],
    signer_seed: [u8; 32],
    expect_verifies: bool,
) -> VoucherClaimStateSolanaCase {
    let channel_account: [u8; 32] = [0xc3; 32];
    let expires = VOUCHER_CLAIM_STATE_EXPIRES;
    let signer =
        LocalEd25519Signer::from_secret_bytes(signer_seed).expect("fixture seed is 32 bytes");
    let message = solana_voucher_claim_state_challenge_message(&channel_account, expires);
    let signature = signer.sign(&message);

    let signature_verifies = verify_solana_voucher_claim_state_challenge(
        &channel_account,
        expires,
        &signature,
        &authorized_signer,
    );
    assert_eq!(
        signature_verifies, expect_verifies,
        "vector {name} computed the wrong verification verdict"
    );

    let channel_account_base58 = bs58::encode(channel_account).into_string();
    let signature_base64 = BASE64.encode(signature);
    let entry_json = serde_json::json!({
        "blockchain": "solana",
        "scheme": SCHEME_BATCH_SETTLEMENT,
        "channelAccount": channel_account_base58,
        "expires": expires,
        "signature": signature_base64,
    })
    .to_string();

    VoucherClaimStateSolanaCase {
        name,
        channel_account_base58,
        expires,
        authorized_signer_base58: bs58::encode(authorized_signer).into_string(),
        signer_secret_hex: hex_of(&signer_seed),
        signer_public_key_base58: bs58::encode(signer.public_key()).into_string(),
        signed_message_hex: hex_of(&message),
        signature_base64,
        signature_verifies,
        entry_json,
    }
}

fn generate_voucher_claim_state_challenge_vectors() -> VoucherClaimStateChallengeVectors {
    let authorizer = hex_bytes::<32>(VOUCHER_EVM_AUTHORIZER_SECRET);
    let evm = vec![
        voucher_claim_state_evm_case("voucher_claim_state_evm_valid", authorizer, true),
        voucher_claim_state_evm_case(
            "voucher_claim_state_evm_wrong_key",
            seq_bytes::<32>(0x99),
            false,
        ),
    ];

    let seed = seq_bytes::<32>(0xa1);
    let authorized_signer = LocalEd25519Signer::from_secret_bytes(seed)
        .expect("fixture seed is 32 bytes")
        .public_key();
    let solana = vec![
        voucher_claim_state_solana_case(
            "voucher_claim_state_solana_valid",
            authorized_signer,
            seed,
            true,
        ),
        voucher_claim_state_solana_case(
            "voucher_claim_state_solana_wrong_key",
            authorized_signer,
            seq_bytes::<32>(0xb1),
            false,
        ),
    ];

    VoucherClaimStateChallengeVectors { evm, solana }
}

/// Build the full committed vector set. See the module docs for what
/// "generated from the properties" means here, and
/// `docs/protocol/wire-vectors.md` for the invariant each section pins.
pub fn generate() -> WireVectors {
    let envelope = generate_envelope_vectors();
    let (giftwrap, shared_secret) = generate_giftwrap_vectors();
    let fulfilment = generate_fulfilment_vectors(shared_secret);
    let peer_carriage = generate_peer_carriage_vectors(&giftwrap);
    let charge = generate_charge_vectors();
    let claim_voucher = generate_claim_voucher_vectors();
    let voucher_claim_state_challenge = generate_voucher_claim_state_challenge_vectors();
    let toon_channel_refused = generate_toon_channel_refused_vectors();
    let payout_voucher = generate_payout_voucher_vectors();

    WireVectors {
        schema_version: SCHEMA_VERSION,
        envelope,
        giftwrap,
        fulfilment,
        peer_carriage,
        charge,
        claim_voucher,
        voucher_claim_state_challenge,
        toon_channel_refused,
        payout_voucher,
    }
}

/// Pretty-printed JSON, newline-terminated -- the exact bytes both the
/// generator binary writes to disk and the gate test compares against.
pub fn to_json(vectors: &WireVectors) -> String {
    let mut json = serde_json::to_string_pretty(vectors).expect("WireVectors always serializes");
    json.push('\n');
    json
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_does_not_panic() {
        let _ = generate();
    }

    #[test]
    fn generating_twice_produces_byte_identical_output() {
        assert_eq!(to_json(&generate()), to_json(&generate()));
    }
}
