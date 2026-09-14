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

use std::collections::HashMap;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use connector_btp::{
    decode_frame, encode_message, encode_response, encode_transfer, ProtocolData,
    ACCUMULATED_COST_HEADER, AUTH_PROTOCOL, CLAIM_ACK_HEADER, CLAIM_ACK_PROTOCOL, CLAIM_HEADER,
    CONTENT_TYPE_TEXT, FLUSH_REQUESTED_HEADER,
};
use connector_domain::{
    EnvelopeError, EnvelopeRequest, EnvelopeResponse, Fulfill, Prepare, Reject, RejectCode,
};
use connector_peer_btp::{ack, claim_json, fields, AcceptedClaims, PeerClaimDomain};
use connector_peer_http::headers::{
    accumulated_cost as http_accumulated_cost, claim_ack as http_claim_ack, claim_ack_header_value,
    claim_header_value, claim_json as http_claim_json, flush_requested as http_flush_requested,
    Headers,
};
use connector_runtime::{
    ChannelDomain, ClaimAckOutcome, ClaimBook, ClaimRejectReason, ClaimSignature, WireClaim,
};
use connector_signer::giftwrap::{
    derive_fulfillment, open_request, open_response, seal_request_with_randomness,
    seal_response_with_randomness,
};
use connector_signer::{
    derive_evm_address, evm_balance_proof_digest, evm_claim_state_challenge_digest,
    verify_evm_balance_proof, verify_evm_claim_state_challenge, Address, EvmBalanceProof,
    EvmClaimStateChallenge, LocalSigner, Signer,
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
pub const SCHEMA_VERSION: u32 = 5;

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

#[derive(Debug, Serialize)]
pub struct WireVectors {
    pub schema_version: u32,
    pub envelope: EnvelopeVectors,
    pub giftwrap: GiftwrapVectors,
    pub fulfilment: FulfilmentVectors,
    pub claim: ClaimVectors,
    pub peer_carriage: PeerCarriageVectors,
    pub channel_control_declaration: ChannelControlDeclarationVectors,
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

#[derive(Debug, Serialize)]
pub struct ClaimVectors {
    pub cases: Vec<ClaimCase>,
}

/// A signed EIP-712 `BalanceProof` (ADR 0024, issue #575): the same struct
/// and digest both a peer claim (`ClaimBook::accept_inbound`) and a
/// client-edge claim (`client-edge-spec.md` §1.3 step 4) are checked
/// against -- `connector_signer::claim_signature` has exactly one such
/// scheme, shared by both wires, so one vector section covers both.
#[derive(Debug, Serialize)]
pub struct ClaimCase {
    pub name: &'static str,
    /// The EIP-712 domain's `chainId` -- configured per channel
    /// (`ClaimBook::set_channel_domain`), never a global default, since a
    /// vector hardcoding one real chain would be unusable against another.
    pub chain_id: u64,
    /// The EIP-712 domain's `verifyingContract` -- the `TokenNetwork`
    /// deployment this channel's claims are redeemed against.
    pub token_network_address_hex: String,
    /// The channel id in its on-chain `bytes32` form -- what the signed
    /// struct actually hashes, not whatever string label a peering
    /// relation happens to know the channel by.
    pub channel_id_hex: String,
    pub nonce: u64,
    pub transferred_amount: u64,
    /// Always zero on the wire (ADR 0004) but still part of the signed
    /// struct -- omitting it computes a different digest than the one a
    /// real signer signs.
    pub locked_amount: u64,
    /// Always zero on the wire (ADR 0004); same reason as `locked_amount`.
    pub locks_root_hex: String,
    /// `keccak256(0x1901 || domainSeparator || structHash)` -- the exact
    /// bytes a real signer signs and `TokenNetwork.sol` recovers against on
    /// redemption.
    pub digest_hex: String,
    pub signer_secret_hex: String,
    pub signer_address_hex: String,
    /// `r || s || recovery_id` (recovery_id `0`/`1`, not the `27`/`28` a
    /// wallet's own signature carries) -- 65 bytes, recovering to
    /// `signer_address_hex` over `digest_hex`.
    pub signature_hex: String,
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

/// The Rust-typed values behind [`ClaimCase`]'s hex-string fields, passed
/// on to [`generate_peer_carriage_vectors`] so item 2's claim and item 3's
/// digest are built from -- and pinned equal to -- this section's own
/// fixture rather than a second, independently chosen one (§10.2 item 3:
/// "pinned **unchanged** against the existing claim section").
struct ClaimFixture {
    chain_id: u64,
    token_network_address: Address,
    channel_id: [u8; 32],
    nonce: u64,
    transferred_amount: u64,
    signer_address: Address,
    signature: connector_signer::Signature,
    digest_hex: String,
}

fn generate_claim_vectors() -> (ClaimVectors, ClaimFixture) {
    let signer_secret = seq_bytes::<32>(0xa1);
    let signer = LocalSigner::from_secret_bytes("vector-fixture-claim-key", signer_secret)
        .expect("fixture claim secret is a valid secp256k1 scalar");
    let signer_address = derive_evm_address(&signer.public_key().expect("fixture has a key"));

    let chain_id: u64 = 84_532; // Base Sepolia -- an example domain, not the only one a channel can be configured with.
    let token_network_address: Address = seq_bytes::<20>(0xc1);
    let channel_id: [u8; 32] = seq_bytes::<32>(0xd1);
    let nonce: u64 = 7;
    let transferred_amount: u64 = 1_500_000;
    let locked_amount: u64 = 0;
    let locks_root = [0u8; 32];

    let proof = EvmBalanceProof {
        channel_id,
        nonce,
        transferred_amount: u128::from(transferred_amount),
        locked_amount: u128::from(locked_amount),
        locks_root,
        chain_id,
        token_network_address,
    };
    let digest = evm_balance_proof_digest(&proof);
    let signature = signer
        .sign(&digest)
        .expect("fixture signer signs its own digest");
    let signature_bytes = signature.to_bytes();

    assert!(
        verify_evm_balance_proof(&proof, &signature_bytes, &signer_address),
        "the fixture signature must recover to the fixture signer's own address"
    );

    let digest_hex = hex_of(&digest);
    (
        ClaimVectors {
            cases: vec![ClaimCase {
                name: "evm_balance_proof_digest_and_signature",
                chain_id,
                token_network_address_hex: hex_of(&token_network_address),
                channel_id_hex: hex_of(&channel_id),
                nonce,
                transferred_amount,
                locked_amount,
                locks_root_hex: hex_of(&locks_root),
                digest_hex: digest_hex.clone(),
                signer_secret_hex: hex_of(&signer_secret),
                signer_address_hex: hex_of(&signer_address),
                signature_hex: hex_of(&signature_bytes),
            }],
        },
        ClaimFixture {
            chain_id,
            token_network_address,
            channel_id,
            nonce,
            transferred_amount,
            signer_address,
            signature,
            digest_hex,
        },
    )
}

// ---------------------------------------------------------------------
// Peer carriage (issue #729, `docs/protocol/peer-carriage-spec.md` §10)
// ---------------------------------------------------------------------
//
// Twenty items, most of them *(pair)*: one BTP encoding and one HTTP
// encoding of the same fixture value, generated in one pass and asserted
// decoded-equal (§10.1's pairing rule, spec I1). Every function here calls
// the real carriage code -- `connector_peer_btp`'s codec and
// `connector_peer_http::headers`' wrappers over it -- never a
// hand-rolled parallel encoder, so a vector this module emits is a vector
// its own implementation would also accept.

/// A peer claim JSON and its two transfer encodings (§10.2 items 2, 4):
/// raw UTF-8 on BTP, `base64` in the HTTP header -- the same JSON both
/// ways (§4).
#[derive(Debug, Serialize)]
pub struct PeerClaimCase {
    pub name: &'static str,
    /// The exact bytes this claim's signature covers, hex-encoded.
    ///
    /// For Solana this is [`connector_signer::solana_balance_proof_message`]'s
    /// 96 bytes -- domain tag, settlement program id, channel account, nonce,
    /// transferred amount (ADR 0053, issue #1082). It is the cross-repo
    /// contract for that format, and the only thing keeping its **three**
    /// in-tree copies in step: this crate's, `connector-settlement-solana`'s
    /// `wire::balance_proof_message`, and the on-chain program's own
    /// `expected_message`.
    ///
    /// Empty for EVM, whose signed bytes are an EIP-712 digest already
    /// pinned by `claim_digest_hex`.
    pub signed_message_hex: String,
    pub blockchain: &'static str,
    pub json: String,
    pub btp_raw_hex: String,
    pub http_base64: String,
    pub wire_channel_id: String,
    pub wire_nonce: u64,
    pub wire_cumulative_amount: u64,
    pub wire_signature_hex: String,
}

/// The OER `Prepare` a claim-bearing PREPARE carries, and the two carriage
/// framings around it (§10.2 items 5, 6).
#[derive(Debug, Serialize)]
pub struct PreparePacketFields {
    pub amount: u64,
    pub expires_at: String,
    pub greeting: bool,
    pub destination: String,
    pub data_hex: String,
}

#[derive(Debug, Serialize)]
pub struct PreparePairCase {
    pub name: &'static str,
    pub prepare: PreparePacketFields,
    pub claim_json: Option<String>,
    pub btp_message_hex: String,
    pub http_headers: Vec<(String, String)>,
    pub http_body_hex: String,
}

/// A judged claim's verdict, as it rides a RESPONSE (§10.2 items 7-9).
#[derive(Debug, Serialize)]
pub struct AckFields {
    pub result: &'static str,
    pub reason: Option<&'static str>,
}

/// A RESPONSE answering a claim-bearing frame: the packet it answers, and
/// the claim-ack riding beside it -- independently (§6.2), including the
/// combinations item 8, item 10, item 11 and item 14 each name (§10.2
/// items 7, 8, 9, 10, 11, 14).
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

/// FLUSH: a TRANSFER carrying the claim's new cumulative as its amount, and
/// the HTTP standalone-claim POST (§10.2 item 13).
#[derive(Debug, Serialize)]
pub struct PeerFlushCase {
    pub name: &'static str,
    pub claim_json: String,
    pub transfer_amount: u64,
    pub btp_transfer_hex: String,
    pub http_headers: Vec<(String, String)>,
    pub http_body_hex: String,
}

/// §6.3's idempotent re-ack and its boundary (§10.2 items 15, 16): a
/// byte-identical retransmission is accepted again, and a same-nonce claim
/// that differs in any other field is refused `nonce_not_advancing`.
#[derive(Debug, Serialize)]
pub struct PeerRetransmitCase {
    pub name: &'static str,
    pub first_claim_json: String,
    pub second_claim_json: String,
    pub first_ack: &'static str,
    pub second_ack: &'static str,
    pub second_ack_reason: Option<&'static str>,
}

/// `Toon-Flush-Requested` -- HTTP only, no BTP counterpart (§6.4, §10.2
/// item 17).
#[derive(Debug, Serialize)]
pub struct PeerFlushRequestedCase {
    pub name: &'static str,
    pub channel_id: String,
    pub http_header_value: String,
    pub note: &'static str,
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

/// There is no `credential` entry, and there was one: a `{peerId, secret}`
/// in a BTP `auth` entry and a `Toon-Peer-Auth` header, pinned as §10.2's
/// item 1. ADR 0060 deleted the credential from both carriages, so there is
/// no encoding left to pin -- a replayer that still has the old vector will
/// find nothing to compare it against, which is the point.
#[derive(Debug, Serialize)]
pub struct PeerCarriageVectors {
    pub claim_evm: PeerClaimCase,
    /// §10.2 item 3: pinned equal to [`ClaimCase::digest_hex`] of this same
    /// run's `claim` section -- demonstrating ADR 0024's digest is
    /// untouched by carriage, not a second, independently computed one.
    pub claim_digest_hex: String,
    pub claim_solana: PeerClaimCase,
    pub prepare: PreparePairCase,
    pub prepare_no_claim: PreparePairCase,
    pub fulfill_ack_accepted: PeerAnswerCase,
    pub fulfill_ack_rejected: PeerAnswerCase,
    pub ack_rejected_reasons: Vec<PeerAnswerCase>,
    pub reject_with_cost: PeerAnswerCase,
    pub ack_absent: PeerAnswerCase,
    pub ack_malformed: PeerMalformedAckCase,
    pub flush: PeerFlushCase,
    pub flush_ack: PeerAnswerCase,
    pub claim_retransmit: PeerRetransmitCase,
    pub claim_same_nonce_different_bytes: PeerRetransmitCase,
    pub flush_requested: PeerFlushRequestedCase,
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

fn generate_peer_claim_evm_case(fixture: &ClaimFixture) -> PeerClaimCase {
    let claim = WireClaim {
        channel_id: format!("0x{}", hex::encode(fixture.channel_id)),
        nonce: fixture.nonce,
        cumulative_amount: fixture.transferred_amount,
        signature: ClaimSignature::Evm(fixture.signature),
    };
    let domain = PeerClaimDomain {
        chain_id: fixture.chain_id,
        token_network: fixture.token_network_address,
    };
    let json = claim_json::encode(
        &claim,
        &fixture.signer_address,
        // No Solana signer and no Solana program id: which arm renders is
        // decided by `claim.signature`'s discriminant, and this fixture's
        // is `ClaimSignature::Evm`. Both are read only in the
        // `ClaimSignature::Solana` arm, so a value here would be inert.
        None,
        None,
        Some(domain),
        "vector-fixture:evm:1",
        "2030-01-01T00:00:00.000Z",
    );

    // I4/I1: the emitted claim parses back through the client edge's own
    // validator, and both carriage encodings hand that parser the same
    // bytes.
    let reparsed = claim_json::parse(json.as_bytes()).expect("the emitted claim parses back");
    assert_eq!(reparsed, claim);

    let raw_entry = claim_json::protocol_data(&json);
    let http_value = claim_header_value(&json);
    let mut headers = Headers::new();
    headers.push(CLAIM_HEADER, &http_value);
    let via_http = http_claim_json(&headers)
        .expect("a claim rode")
        .expect("valid base64");
    assert_eq!(via_http, json.as_bytes());

    PeerClaimCase {
        name: "peer_claim_evm",
        signed_message_hex: String::new(),
        blockchain: "evm",
        json,
        btp_raw_hex: hex_of(&raw_entry.data),
        http_base64: http_value,
        wire_channel_id: claim.channel_id,
        wire_nonce: claim.nonce,
        wire_cumulative_amount: claim.cumulative_amount,
        wire_signature_hex: hex_of(&claim.signature.to_bytes()),
    }
}

/// Decode a base58 32-byte fixture, panicking on a bad literal -- these are
/// hardcoded in this file, so a failure here is a typo, not input.
fn bs58_32(value: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    let decoded = bs58::decode(value).into_vec().expect("a base58 fixture");
    assert_eq!(decoded.len(), 32, "fixture is not 32 bytes: {value}");
    out.copy_from_slice(&decoded);
    out
}

/// The settlement program the `claim_solana` fixture's channel lives under.
///
/// This is the deployed public-devnet payment-channel program
/// (`packages/solana-program/deployments/devnet-public.md`) -- an example
/// settlement program, not the only one a channel can live under, exactly as
/// `generate_claim_vectors`'s `chain_id` is Base Sepolia's real id rather than
/// a made-up one. A claim fixture's *domain* half carries a real deployment's
/// value for that reason; its account and key halves stay fixture bytes.
///
/// It deliberately replaces the system program
/// (`11111111111111111111111111111111`) this fixture carried until issue
/// #1127. A Solana claim's `programId` MUST name the settlement program its
/// `channelAccount` lives under (`client-edge-spec.md` §1.3) -- the same 32
/// bytes ADR 0053 binds into the signed balance proof at offset 16 -- and a
/// normative fixture declaring the system program taught every payer reading
/// it that any value would do.
const SOLANA_SETTLEMENT_PROGRAM_ID: &str = "2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip";

/// §10.2 item 4: marked **aspirational**, exactly as `peer-semantics-pre-868.md`
/// §3.5 marks the Solana claim row -- pinning the shape before an emitting
/// implementation exists on this connector (outbound peer claims are
/// EVM-only, `claim_json::encode`'s own doc). What *does* exist and is
/// exercised here for real is the inbound half: `claim_json::parse`
/// already accepts a Solana claim (issue #732).
fn generate_peer_claim_solana_case() -> PeerClaimCase {
    let channel_account = "GDDMwNyyx8uB6zrqwBFHjLLG3TBYk2F1Mh6usnNPUsqk";
    let program_id = SOLANA_SETTLEMENT_PROGRAM_ID;
    let signer_public_key = "11111111111111111111111111111113";
    let signature_bytes = seq_bytes::<64>(0xe1);
    let json = serde_json::json!({
        "version": "1.0",
        "blockchain": "solana",
        "messageId": "vector-fixture:solana:1",
        "timestamp": "2030-01-01T00:00:00.000Z",
        "senderId": signer_public_key,
        // One literal, read twice: the declared label here and the signed
        // domain at offset 16 of `signed_message_hex` below are the same
        // `program_id`, so this fixture cannot drift into declaring one
        // program while signing under another (issue #1127).
        "programId": program_id,
        "channelAccount": channel_account,
        "nonce": 1,
        "transferredAmount": "250000",
        "signature": BASE64.encode(signature_bytes),
        "signerPublicKey": signer_public_key,
    })
    .to_string();

    let parsed = claim_json::parse(json.as_bytes()).expect("the aspirational solana shape parses");
    assert_eq!(
        parsed,
        WireClaim {
            channel_id: channel_account.to_string(),
            nonce: 1,
            cumulative_amount: 250_000,
            signature: ClaimSignature::Solana(signature_bytes),
        }
    );

    let raw_entry = claim_json::protocol_data(&json);
    let http_value = claim_header_value(&json);
    let mut headers = Headers::new();
    headers.push(CLAIM_HEADER, &http_value);
    assert_eq!(
        http_claim_json(&headers)
            .expect("a claim rode")
            .expect("valid base64"),
        json.as_bytes()
    );

    PeerClaimCase {
        name: "peer_claim_solana",
        signed_message_hex: hex_of(&connector_signer::solana_balance_proof_message(
            &bs58_32(program_id),
            &bs58_32(channel_account),
            1,
            250_000,
        )),
        blockchain: "solana",
        json,
        btp_raw_hex: hex_of(&raw_entry.data),
        http_base64: http_value,
        wire_channel_id: channel_account.to_string(),
        wire_nonce: 1,
        wire_cumulative_amount: 250_000,
        wire_signature_hex: hex_of(&signature_bytes),
    }
}

fn generate_prepare_pair_cases(evm_claim: &PeerClaimCase) -> (PreparePairCase, PreparePairCase) {
    let prepare = Prepare {
        amount: 250_000,
        expires_at: "2030-01-01T00:01:00Z".parse().expect("fixed literal"),
        greeting: false,
        destination: "g.toon.store-box.settle".to_string(),
        data: b"vector-fixture-prepare-data".to_vec(),
    };
    let prepare_bytes = prepare.encode();
    assert_eq!(
        Prepare::decode(&prepare_bytes).expect("self-generated prepare decodes"),
        prepare
    );

    let claim_entry = claim_json::protocol_data(&evm_claim.json);

    // Item 5: a claim riding one PREPARE.
    let btp_with_claim = encode_message(9_001, &[claim_entry], &prepare_bytes);
    let decoded = decode_frame(&btp_with_claim).expect("self-generated frame decodes");
    assert_eq!(decoded.ilp_packet, prepare_bytes);
    assert_eq!(
        claim_json::from_protocol_data(&decoded.protocol_data),
        Some(evm_claim.json.as_bytes())
    );

    let mut headers_with_claim = Headers::new();
    headers_with_claim.push(CLAIM_HEADER, claim_header_value(&evm_claim.json));
    assert_eq!(
        http_claim_json(&headers_with_claim)
            .expect("a claim rode")
            .expect("valid base64"),
        evm_claim.json.as_bytes()
    );

    let with_claim = PreparePairCase {
        name: "peer_prepare",
        prepare: prepare_fields(&prepare),
        claim_json: Some(evm_claim.json.clone()),
        btp_message_hex: hex_of(&btp_with_claim),
        http_headers: headers_pairs(&headers_with_claim),
        http_body_hex: hex_of(&prepare_bytes),
    };

    // Item 6: the same PREPARE with no claim entry/header -- "claimless is
    // legal" pinned rather than assumed.
    let btp_no_claim = encode_message(9_002, &[], &prepare_bytes);
    let decoded_no_claim = decode_frame(&btp_no_claim).expect("self-generated frame decodes");
    assert!(claim_json::from_protocol_data(&decoded_no_claim.protocol_data).is_none());

    let headers_no_claim = Headers::new();
    assert!(http_claim_json(&headers_no_claim).is_none());

    let without_claim = PreparePairCase {
        name: "peer_prepare_no_claim",
        prepare: prepare_fields(&prepare),
        claim_json: None,
        btp_message_hex: hex_of(&btp_no_claim),
        http_headers: headers_pairs(&headers_no_claim),
        http_body_hex: hex_of(&prepare_bytes),
    };

    (with_claim, without_claim)
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

/// One judged-claim RESPONSE, on both carriages: `protocol_data` rides the
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

    // I1, on the ack and the accumulated-cost fields alike: whatever this
    // case pins is read back identically off both encodings by the real
    // decoders.
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

/// [`generate_answer_cases`]'s output, named so its four same-typed
/// [`PeerAnswerCase`] values can't be silently transposed by position the
/// way a same-typed tuple return could be -- the return-side counterpart
/// to [`AnswerCaseSpec`] grouping the argument side.
struct AnswerCases {
    fulfill_ack_accepted: PeerAnswerCase,
    fulfill_ack_rejected: PeerAnswerCase,
    ack_rejected_reasons: Vec<PeerAnswerCase>,
    reject_with_cost: PeerAnswerCase,
    ack_absent: PeerAnswerCase,
}

fn generate_answer_cases() -> AnswerCases {
    let fulfill_bytes = fixture_fulfill().encode();

    // Item 7: a FULFILL, acknowledged accepted.
    let ack_entry = ack::protocol_data(ClaimAckOutcome::Accepted).expect("a judged claim");
    let ack_header = claim_ack_header_value(ClaimAckOutcome::Accepted).expect("a judged claim");
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
    let ack_entry = ack::protocol_data(rejected_signature_invalid).expect("a judged claim");
    let ack_header = claim_ack_header_value(rejected_signature_invalid).expect("a judged claim");
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

    // Item 9: one pair per §6.1 reason.
    let mut ack_rejected_reasons = Vec::new();
    for (index, reason) in [
        ClaimRejectReason::SignatureInvalid,
        ClaimRejectReason::NonceNotAdvancing,
        ClaimRejectReason::AmountNotAdvancing,
        ClaimRejectReason::UnknownChannel,
    ]
    .into_iter()
    .enumerate()
    {
        let outcome = ClaimAckOutcome::Rejected(reason);
        let reason_name = ack::reason_name(reason);
        let ack_entry = ack::protocol_data(outcome).expect("a judged claim");
        let ack_header = claim_ack_header_value(outcome).expect("a judged claim");
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
    let ack_entry = ack::protocol_data(ClaimAckOutcome::Accepted).expect("a judged claim");
    let ack_header = claim_ack_header_value(ClaimAckOutcome::Accepted).expect("a judged claim");
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

    // Item 11: a response answering a claim-bearing request with **no**
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

/// Item 14: the answer to a FLUSH -- an **empty** packet, acknowledged
/// accepted. Shares [`answer_case`]'s machinery with items 7-11 over a
/// zero-length `packet_bytes`.
fn generate_flush_ack_case() -> PeerAnswerCase {
    let ack_entry = ack::protocol_data(ClaimAckOutcome::Accepted).expect("a judged claim");
    let ack_header = claim_ack_header_value(ClaimAckOutcome::Accepted).expect("a judged claim");
    answer_case(AnswerCaseSpec {
        name: "peer_flush_ack".to_string(),
        request_id: 9_140,
        packet: "none",
        packet_bytes: Vec::new(),
        protocol_data: vec![ack_entry],
        http_headers: vec![(CLAIM_ACK_HEADER.to_string(), ack_header)],
        ack: Some(AckFields {
            result: "accepted",
            reason: None,
        }),
        accumulated_cost: None,
    })
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

fn generate_flush_case(evm_claim: &PeerClaimCase) -> PeerFlushCase {
    let claim_entry = claim_json::protocol_data(&evm_claim.json);
    let amount = evm_claim.wire_cumulative_amount;

    let btp = encode_transfer(9_150, amount, &[claim_entry]);
    let decoded = decode_frame(&btp).expect("self-generated frame decodes");
    assert_eq!(decoded.amount, Some(amount));
    assert!(
        decoded.ilp_packet.is_empty(),
        "a TRANSFER carries no ilpPacket"
    );
    assert_eq!(
        claim_json::from_protocol_data(&decoded.protocol_data),
        Some(evm_claim.json.as_bytes())
    );

    let mut headers = Headers::new();
    headers.push(CLAIM_HEADER, claim_header_value(&evm_claim.json));
    assert_eq!(
        http_claim_json(&headers)
            .expect("a claim rode")
            .expect("valid base64"),
        evm_claim.json.as_bytes()
    );

    PeerFlushCase {
        name: "peer_flush",
        claim_json: evm_claim.json.clone(),
        transfer_amount: amount,
        btp_transfer_hex: hex_of(&btp),
        http_headers: headers_pairs(&headers),
        http_body_hex: String::new(),
    }
}

/// Items 15 and 16: §6.3's idempotent re-ack and its boundary, exercised
/// against the *real* gate a carriage checks -- [`AcceptedClaims`] first
/// (in-process, per relation), falling through to [`ClaimBook`]'s
/// strictly-advancing rule exactly as `connector-peer-btp`'s and
/// `connector-peer-http`'s own accept paths do (`accept.rs`'s
/// `judge_claim`).
fn generate_retransmit_cases() -> (PeerRetransmitCase, PeerRetransmitCase) {
    let signer =
        LocalSigner::from_secret_bytes("vector-fixture-retransmit-key", seq_bytes::<32>(0xb1))
            .expect("fixture retransmit secret is a valid secp256k1 scalar");
    let signer_address = derive_evm_address(&signer.public_key().expect("fixture has a key"));
    let channel_id = seq_bytes::<32>(0xb5);
    let chain_id = 84_532u64;
    let token_network_address: Address = seq_bytes::<20>(0xb9);
    let peer_domain = PeerClaimDomain {
        chain_id,
        token_network: token_network_address,
    };

    let sign = |nonce: u64, amount: u64| -> WireClaim {
        let proof = EvmBalanceProof {
            channel_id,
            nonce,
            transferred_amount: u128::from(amount),
            locked_amount: 0,
            locks_root: [0u8; 32],
            chain_id,
            token_network_address,
        };
        let signature = signer
            .sign(&evm_balance_proof_digest(&proof))
            .expect("fixture signer signs its own digest");
        WireClaim {
            channel_id: format!("0x{}", hex::encode(channel_id)),
            nonce,
            cumulative_amount: amount,
            signature: ClaimSignature::Evm(signature),
        }
    };
    let render = |claim: &WireClaim| {
        claim_json::encode(
            claim,
            &signer_address,
            // EVM fixture, as above -- no Solana signer or program id to
            // render with.
            None,
            None,
            Some(peer_domain),
            "vector-fixture:retransmit",
            "2030-01-01T00:00:00.000Z",
        )
    };

    let first = sign(11, 800_000);
    let first_json = render(&first);

    let watermark = AcceptedClaims::new();
    assert!(!watermark.is_at_watermark("peer-b", &first));
    watermark.record("peer-b", &first);
    assert!(
        watermark.is_at_watermark("peer-b", &first),
        "a byte-identical retransmission must be recognised at the watermark"
    );

    let retransmit = PeerRetransmitCase {
        name: "peer_claim_retransmit",
        first_claim_json: first_json.clone(),
        second_claim_json: first_json.clone(),
        first_ack: "accepted",
        second_ack: "accepted",
        second_ack_reason: None,
    };

    // Item 16: a genuinely different, validly signed claim at the *same*
    // nonce is not the one at the watermark, and falls through to
    // `ClaimBook`'s own strictly-advancing rule -- exactly as the carriage
    // falls through once `is_at_watermark` says no.
    let different = sign(11, 950_000);
    assert!(!watermark.is_at_watermark("peer-b", &different));

    let mut counterparties = HashMap::new();
    counterparties.insert(first.channel_id.clone(), signer_address);
    let book = ClaimBook::new(None, HashMap::new(), counterparties);
    book.set_channel_domain(
        &first.channel_id,
        ChannelDomain {
            chain_id,
            token_network_address,
        },
    )
    .expect("a fixed on-chain channel id is always valid");
    assert_eq!(book.accept_inbound(&first), ClaimAckOutcome::Accepted);
    assert_eq!(
        book.accept_inbound(&different),
        ClaimAckOutcome::Rejected(ClaimRejectReason::NonceNotAdvancing)
    );

    let different_bytes = PeerRetransmitCase {
        name: "peer_claim_same_nonce_different_bytes",
        first_claim_json: first_json,
        second_claim_json: render(&different),
        first_ack: "accepted",
        second_ack: "rejected",
        second_ack_reason: Some("nonce_not_advancing"),
    };

    (retransmit, different_bytes)
}

fn generate_flush_requested_case() -> PeerFlushRequestedCase {
    let channel_id = format!("0x{}", hex::encode(seq_bytes::<32>(0xc5)));
    let mut headers = Headers::new();
    headers.push(FLUSH_REQUESTED_HEADER, &channel_id);
    assert_eq!(http_flush_requested(&headers), vec![channel_id.clone()]);

    PeerFlushRequestedCase {
        name: "peer_flush_requested",
        channel_id: channel_id.clone(),
        http_header_value: channel_id,
        note: "HTTP only -- BTP has no counterpart (peer-carriage-spec.md §6.4): the payee can \
               originate a request of its own",
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

fn generate_peer_carriage_vectors(
    claim_fixture: &ClaimFixture,
    giftwrap: &GiftwrapVectors,
) -> PeerCarriageVectors {
    let claim_evm = generate_peer_claim_evm_case(claim_fixture);
    let claim_solana = generate_peer_claim_solana_case();
    let (prepare, prepare_no_claim) = generate_prepare_pair_cases(&claim_evm);
    let AnswerCases {
        fulfill_ack_accepted,
        fulfill_ack_rejected,
        ack_rejected_reasons,
        reject_with_cost,
        ack_absent,
    } = generate_answer_cases();
    let flush_ack = generate_flush_ack_case();
    let ack_malformed = generate_ack_malformed_case();
    let flush = generate_flush_case(&claim_evm);
    let (claim_retransmit, claim_same_nonce_different_bytes) = generate_retransmit_cases();
    let flush_requested = generate_flush_requested_case();
    let forwarded_data_unchanged = generate_forwarded_data_case(giftwrap);

    PeerCarriageVectors {
        claim_digest_hex: claim_fixture.digest_hex.clone(),
        claim_evm,
        claim_solana,
        prepare,
        prepare_no_claim,
        fulfill_ack_accepted,
        fulfill_ack_rejected,
        ack_rejected_reasons,
        reject_with_cost,
        ack_absent,
        ack_malformed,
        flush,
        flush_ack,
        claim_retransmit,
        claim_same_nonce_different_bytes,
        flush_requested,
        forwarded_data_unchanged,
    }
}

// ---------------------------------------------------------------------
// Client channel-control declaration (issue #792,
// `docs/protocol/client-edge-spec.md` §1.9 step 1, issue #790)
// ---------------------------------------------------------------------
//
// The BTP auth entry's `channelId`/`expires`/`signature` fields, binding a
// client session to a channel *before* it has ever presented a claim
// (issue #790) -- the identical domain-separated `ClaimStateChallenge`
// signature `POST /ilp/claim-state` already verifies for a read
// (`connector_signer::claim_state_challenge`), reused rather than a
// claim's own balance-proof scheme. Generated through the real digest and
// verification functions, never a hand-rolled parallel signer, and
// self-verified against them before being emitted -- exactly the
// discipline `verify_and_record_declared_channel`
// (`connector-client-edge::btp`) itself applies.

/// One `auth` entry carrying a channel-control declaration, and the BTP
/// MESSAGE frame it rides in -- the same `{peerId, secret, channelId,
/// expires, signature}` shape `auth_channel_proof`
/// (`connector-client-edge::btp`) extracts.
#[derive(Debug, Serialize)]
pub struct ChannelControlDeclarationCase {
    pub name: &'static str,
    pub peer_id: &'static str,
    /// The EIP-712 domain's `chainId` -- the channel's own registered
    /// domain, never a self-declared one (same rule as [`ClaimCase`]).
    pub chain_id: u64,
    pub token_network_address_hex: String,
    pub channel_id_hex: String,
    /// Unix seconds. Compared by the verifier as `expires <= now` ->
    /// rejected -- a fact about wall-clock time at verification time, not
    /// something this static vector can itself encode, which is why the
    /// `channel_control_declaration_expired` case picks an `expires` far
    /// enough in the past (`1`, 1970-01-01T00:00:01Z) to be expired against
    /// any reasonable clock, and the valid cases pick one far enough in the
    /// future (2030/2100) to still be valid against any reasonable clock.
    pub expires: u64,
    /// The channel's registered counterparty -- what `signature` must
    /// recover to for [`signature_verifies`] to be `true`.
    ///
    /// [`signature_verifies`]: ChannelControlDeclarationCase::signature_verifies
    pub counterparty_address_hex: String,
    pub signer_secret_hex: String,
    pub signer_address_hex: String,
    /// `keccak256(0x1901 || domainSeparator || structHash)` for
    /// `ClaimStateChallenge(bytes32 channelId,uint256 expires)` -- the
    /// exact bytes `signature` covers.
    pub digest_hex: String,
    /// `r || s || recovery_id` (recovery_id `0`/`1`), `0x`-prefixed, 65
    /// bytes -- matching `EvmSigner.signClaimStateChallenge`'s wire form.
    pub signature_hex: String,
    /// The auth entry's JSON body, byte-for-byte what rides as the BTP
    /// `auth` protocolData entry's `data`.
    pub auth_json: String,
    pub btp_message_hex: String,
    /// Whether `signature` recovers to `counterparty_address_hex` under
    /// `connector_signer::verify_evm_claim_state_challenge` -- independent
    /// of `expires`, which the verifier checks separately (see that
    /// field's own doc). `false` only for the wrong-key case: the expired
    /// case's signature is genuine and this is still `true` for it.
    pub signature_verifies: bool,
}

#[derive(Debug, Serialize)]
pub struct ChannelControlDeclarationVectors {
    pub cases: Vec<ChannelControlDeclarationCase>,
}

/// The fields shared by every [`ChannelControlDeclarationCase`] this module
/// emits -- one channel, one registered counterparty, varied only by which
/// key signs and what `expires` says.
struct ChannelControlFixture {
    channel_id: [u8; 32],
    chain_id: u64,
    token_network_address: Address,
    peer_id: &'static str,
    counterparty_address: Address,
}

/// Builds and self-verifies one [`ChannelControlDeclarationCase`]: signs the
/// real EIP-712 digest, checks the resulting signature against
/// `fixture.counterparty_address` through the real verifier (asserting the
/// result matches `expect_verifies`, so this module cannot silently commit
/// a case whose own verdict it got wrong), round-trips the JSON through
/// `serde_json` the same way `auth_channel_proof` reads it, and round-trips
/// the BTP frame through `encode_message`/`decode_frame`.
fn channel_control_case(
    fixture: &ChannelControlFixture,
    name: &'static str,
    expires: u64,
    signer_secret: [u8; 32],
    signer_label: &'static str,
    expect_verifies: bool,
) -> ChannelControlDeclarationCase {
    let signer = LocalSigner::from_secret_bytes(signer_label, signer_secret)
        .expect("fixture secret is a valid secp256k1 scalar");
    let signer_address = derive_evm_address(&signer.public_key().expect("fixture has a key"));

    let challenge = EvmClaimStateChallenge {
        channel_id: fixture.channel_id,
        expires,
        chain_id: fixture.chain_id,
        token_network_address: fixture.token_network_address,
    };
    let digest = evm_claim_state_challenge_digest(&challenge);
    let signature = signer
        .sign(&digest)
        .expect("fixture signer signs its own digest");
    let signature_bytes = signature.to_bytes();

    let signature_verifies = verify_evm_claim_state_challenge(
        &challenge,
        &signature_bytes,
        &fixture.counterparty_address,
    );
    assert_eq!(
        signature_verifies, expect_verifies,
        "vector {name} computed the wrong verification verdict"
    );

    let channel_id_hex = format!("0x{}", hex::encode(fixture.channel_id));
    let signature_hex = format!("0x{}", hex::encode(signature_bytes));
    let auth_json = serde_json::json!({
        "peerId": fixture.peer_id,
        "secret": "",
        "channelId": channel_id_hex,
        "expires": expires,
        "signature": signature_hex,
    })
    .to_string();

    // Same field access `auth_channel_proof` performs: this vector cannot
    // commit a JSON shape that function would fail to parse.
    let reparsed: serde_json::Value =
        serde_json::from_str(&auth_json).expect("this module's own json! output parses");
    assert_eq!(
        reparsed["channelId"].as_str(),
        Some(channel_id_hex.as_str())
    );
    assert_eq!(reparsed["expires"].as_u64(), Some(expires));
    assert_eq!(reparsed["signature"].as_str(), Some(signature_hex.as_str()));

    let entry = ProtocolData {
        name: AUTH_PROTOCOL.to_string(),
        content_type: CONTENT_TYPE_TEXT,
        data: auth_json.clone().into_bytes(),
    };
    let btp_message = encode_message(9_500, &[entry], &[]);
    let decoded = decode_frame(&btp_message).expect("self-generated frame decodes");
    let decoded_entry = decoded
        .protocol_data
        .iter()
        .find(|pd| pd.name == AUTH_PROTOCOL)
        .expect("the auth entry rides the frame it was just encoded into");
    assert_eq!(decoded_entry.data, auth_json.as_bytes());

    ChannelControlDeclarationCase {
        name,
        peer_id: fixture.peer_id,
        chain_id: fixture.chain_id,
        token_network_address_hex: hex_of(&fixture.token_network_address),
        channel_id_hex,
        expires,
        counterparty_address_hex: hex_of(&fixture.counterparty_address),
        signer_secret_hex: hex_of(&signer_secret),
        signer_address_hex: hex_of(&signer_address),
        digest_hex: hex_of(&digest),
        signature_hex,
        auth_json,
        btp_message_hex: hex_of(&btp_message),
        signature_verifies,
    }
}

fn generate_channel_control_declaration_vectors() -> ChannelControlDeclarationVectors {
    let counterparty_secret = seq_bytes::<32>(0x91);
    let counterparty_label = "vector-fixture-channel-control-counterparty";
    let counterparty = LocalSigner::from_secret_bytes(counterparty_label, counterparty_secret)
        .expect("fixture counterparty secret is a valid secp256k1 scalar");
    let counterparty_address =
        derive_evm_address(&counterparty.public_key().expect("fixture has a key"));

    let fixture = ChannelControlFixture {
        channel_id: seq_bytes::<32>(0xe5),
        chain_id: 84_532, // Base Sepolia -- an example domain, as in `generate_claim_vectors`.
        token_network_address: seq_bytes::<20>(0xe9),
        peer_id: "g.toon.vector-agent",
        counterparty_address,
    };

    let valid = channel_control_case(
        &fixture,
        "channel_control_declaration_valid",
        4_102_444_800, // 2100-01-01T00:00:00Z
        counterparty_secret,
        counterparty_label,
        true,
    );

    let wrong_key = channel_control_case(
        &fixture,
        "channel_control_declaration_wrong_key",
        4_102_444_800, // 2100-01-01T00:00:00Z
        seq_bytes::<32>(0x99),
        "vector-fixture-channel-control-forger",
        false,
    );

    let expired = channel_control_case(
        &fixture,
        "channel_control_declaration_expired",
        1, // 1970-01-01T00:00:01Z -- the signature itself is genuine; only
        // the caller's own wall-clock check (not this function) treats it
        // as expired.
        counterparty_secret,
        counterparty_label,
        true,
    );

    ChannelControlDeclarationVectors {
        cases: vec![valid, wrong_key, expired],
    }
}

/// Build the full committed vector set. See the module docs for what
/// "generated from the properties" means here, and
/// `docs/protocol/wire-vectors.md` for the invariant each section pins.
pub fn generate() -> WireVectors {
    let envelope = generate_envelope_vectors();
    let (giftwrap, shared_secret) = generate_giftwrap_vectors();
    let fulfilment = generate_fulfilment_vectors(shared_secret);
    let (claim, claim_fixture) = generate_claim_vectors();
    let peer_carriage = generate_peer_carriage_vectors(&claim_fixture, &giftwrap);
    let channel_control_declaration = generate_channel_control_declaration_vectors();

    WireVectors {
        schema_version: SCHEMA_VERSION,
        envelope,
        giftwrap,
        fulfilment,
        claim,
        peer_carriage,
        channel_control_declaration,
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
