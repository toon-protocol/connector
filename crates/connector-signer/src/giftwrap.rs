//! A packet payload sealed to the terminating connector's identity (ADR
//! 0018, issue #524): what actually rides in `Prepare.data`,
//! `Fulfill.data`, and a REJECT raised at the termination.
//!
//! **Request direction**: the sender ECDHs a fresh, per-packet ephemeral
//! key against the terminating connector's identity public key, and seals
//! the plaintext -- a fresh random shared secret followed by the encoded
//! envelope -- under a key derived from that ECDH result. Only the holder
//! of the matching identity secret key can recover the shared secret and
//! open it; a forwarding hop, which never holds that key, sees only opaque
//! bytes.
//!
//! **Response direction**: no second key exchange. The shared secret
//! carried inside the request wrap seals the answer directly (a
//! [`crate::signer::Signer`] is not needed to open a sealed response -- the
//! caller already has the secret from opening the request). "No second
//! exchange is needed; the secret is bidirectional by construction."
//!
//! A sealed request and a sealed response use distinct type bytes so
//! neither can be fed to the other's `open_*` by mistake, and unopenable
//! bytes ([`GiftWrapError`]) are a different Rust type entirely from a
//! wrap that opens cleanly but decodes to a malformed envelope --
//! `connector_domain::EnvelopeError` -- so the two failure modes stay
//! distinguishable at every call site by construction, not by convention.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use libsecp256k1::{PublicKey, SecretKey};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::Sha256;
use thiserror::Error;

use crate::crypto::ecdh_x_coordinate;
use crate::signer::{PublicKeyBytes, Signer};

const NONCE_LEN: usize = 12;
const SECRET_LEN: usize = 32;
const PUBLIC_KEY_LEN: usize = 65;
const REQUEST_INFO: &[u8] = b"toon-giftwrap-request";
const RESPONSE_INFO: &[u8] = b"toon-giftwrap-response";
const FULFILLMENT_INFO: &[u8] = b"toon-giftwrap-fulfillment";
const TYPE_GIFTWRAP_REQUEST: u8 = 1;
const TYPE_GIFTWRAP_RESPONSE: u8 = 2;

/// Why a gift wrap could not be opened. Never carries decrypted plaintext.
/// Distinct from a wrap that opens cleanly but whose recovered plaintext
/// fails to decode as an envelope -- that is a
/// `connector_domain::EnvelopeError`, a different type entirely, raised
/// above this module rather than by it (issue #524's own acceptance
/// criterion: a wrap that cannot be opened rejects distinguishably from one
/// that opens to a malformed envelope).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GiftWrapError {
    #[error("invalid gift wrap type byte: expected {0}")]
    InvalidType(u8),

    #[error("gift wrap is truncated")]
    Truncated,

    #[error("invalid ephemeral or peer public key")]
    InvalidKey,

    #[error("gift wrap failed to decrypt")]
    OpenFailed,
}

fn hkdf_key(shared_secret: &[u8; 32], info: &[u8]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, shared_secret);
    let mut okm = [0u8; 32];
    hk.expand(info, &mut okm)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    okm
}

/// Encrypt with an explicit AEAD nonce rather than drawing one from
/// [`OsRng`] internally -- both [`seal_request_with_randomness`] and
/// [`seal_response_with_randomness`] call this directly (their [`OsRng`]-
/// drawing counterparts generate a nonce and delegate), so there is exactly
/// one place this module turns a key and plaintext into a ciphertext.
fn encrypt_with_nonce(key: &[u8; 32], plaintext: &[u8], nonce_bytes: [u8; NONCE_LEN]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new(&Key::from(*key));
    let ciphertext = cipher
        .encrypt(&Nonce::from(nonce_bytes), plaintext)
        .expect("chacha20poly1305 encryption of a bounded packet payload cannot fail");
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    out
}

fn decrypt(key: &[u8; 32], nonce_and_ciphertext: &[u8]) -> Result<Vec<u8>, GiftWrapError> {
    if nonce_and_ciphertext.len() < NONCE_LEN {
        return Err(GiftWrapError::Truncated);
    }
    let (nonce_bytes, ciphertext) = nonce_and_ciphertext.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce_bytes.try_into().expect("checked length above");
    let cipher = ChaCha20Poly1305::new(&Key::from(*key));
    cipher
        .decrypt(&Nonce::from(nonce), ciphertext)
        .map_err(|_| GiftWrapError::OpenFailed)
}

/// Seal `plaintext` (an encoded envelope) to `receiver_public` -- a fresh
/// ephemeral key pair is generated for this call alone, so no two sealed
/// requests, even to the same receiver, share an ephemeral key. Returns the
/// wire bytes to carry as `Prepare.data`, and the freshly generated shared
/// secret this packet's fulfilment derives from (ADR 0019/#525) and that
/// [`seal_response`] uses to seal the answer.
pub fn seal_request(
    plaintext: &[u8],
    receiver_public: &PublicKeyBytes,
) -> Result<(Vec<u8>, [u8; 32]), GiftWrapError> {
    let mut ephemeral_secret_bytes = [0u8; 32];
    loop {
        OsRng.fill_bytes(&mut ephemeral_secret_bytes);
        if SecretKey::parse(&ephemeral_secret_bytes).is_ok() {
            break;
        }
    }

    let mut shared_secret = [0u8; SECRET_LEN];
    OsRng.fill_bytes(&mut shared_secret);

    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);

    let wrapped = seal_request_with_randomness_impl(
        plaintext,
        receiver_public,
        &ephemeral_secret_bytes,
        &shared_secret,
        &nonce_bytes,
    )?;

    Ok((wrapped, shared_secret))
}

/// The deterministic core [`seal_request`] wraps, parameterized on the
/// ephemeral key, shared secret and AEAD nonce it would otherwise draw from
/// [`OsRng`]. Gated behind the `test-util` feature (issue #587): reusing a
/// (key, nonce) pair under ChaCha20-Poly1305 is catastrophic, so a caller-
/// supplied nonce/shared-secret is a footgun this crate does not hand out on
/// its ordinary public surface. `connector-vectors` (issue #527) is the
/// sanctioned consumer -- it needs this module's own sealing logic to
/// reproduce exactly what a genuine seal produces for a fixed fixture,
/// instead of a second implementation of it that could drift from this one.
/// `ephemeral_secret_bytes` must parse as a valid secp256k1 scalar --
/// [`seal_request`] retries internally until it draws one; a caller that
/// already controls its own fixture is expected to pick one that parses.
#[cfg(any(test, feature = "test-util"))]
pub fn seal_request_with_randomness(
    plaintext: &[u8],
    receiver_public: &PublicKeyBytes,
    ephemeral_secret_bytes: &[u8; 32],
    shared_secret: &[u8; SECRET_LEN],
    nonce_bytes: &[u8; NONCE_LEN],
) -> Result<Vec<u8>, GiftWrapError> {
    seal_request_with_randomness_impl(
        plaintext,
        receiver_public,
        ephemeral_secret_bytes,
        shared_secret,
        nonce_bytes,
    )
}

fn seal_request_with_randomness_impl(
    plaintext: &[u8],
    receiver_public: &PublicKeyBytes,
    ephemeral_secret_bytes: &[u8; 32],
    shared_secret: &[u8; SECRET_LEN],
    nonce_bytes: &[u8; NONCE_LEN],
) -> Result<Vec<u8>, GiftWrapError> {
    let receiver_public =
        PublicKey::parse(receiver_public).map_err(|_| GiftWrapError::InvalidKey)?;
    let ephemeral_secret =
        SecretKey::parse(ephemeral_secret_bytes).map_err(|_| GiftWrapError::InvalidKey)?;
    let ephemeral_public = PublicKey::from_secret_key(&ephemeral_secret);

    let ecdh_secret =
        ecdh_x_coordinate(&ephemeral_secret, &receiver_public).ok_or(GiftWrapError::InvalidKey)?;
    let aead_key = hkdf_key(&ecdh_secret, REQUEST_INFO);

    let mut inner = Vec::with_capacity(SECRET_LEN + plaintext.len());
    inner.extend_from_slice(shared_secret);
    inner.extend_from_slice(plaintext);
    let ciphertext = encrypt_with_nonce(&aead_key, &inner, *nonce_bytes);

    let mut out = Vec::with_capacity(1 + PUBLIC_KEY_LEN + ciphertext.len());
    out.push(TYPE_GIFTWRAP_REQUEST);
    out.extend_from_slice(&ephemeral_public.serialize());
    out.extend_from_slice(&ciphertext);

    Ok(out)
}

/// Open a sealed request addressed to `signer`'s active identity key,
/// recovering the plaintext envelope bytes and the shared secret carried
/// alongside them. `signer.ecdh` never exposes secret key material -- a
/// [`crate::KmsSigner`] backend can open a wrap without its private key
/// ever leaving its own boundary.
pub fn open_request(
    bytes: &[u8],
    signer: &dyn Signer,
) -> Result<(Vec<u8>, [u8; 32]), GiftWrapError> {
    if bytes.is_empty() {
        return Err(GiftWrapError::Truncated);
    }
    if bytes[0] != TYPE_GIFTWRAP_REQUEST {
        return Err(GiftWrapError::InvalidType(TYPE_GIFTWRAP_REQUEST));
    }
    if bytes.len() < 1 + PUBLIC_KEY_LEN {
        return Err(GiftWrapError::Truncated);
    }
    let mut ephemeral_public_key = [0u8; PUBLIC_KEY_LEN];
    ephemeral_public_key.copy_from_slice(&bytes[1..1 + PUBLIC_KEY_LEN]);
    let ciphertext = &bytes[1 + PUBLIC_KEY_LEN..];

    let ecdh_secret = signer
        .ecdh(&ephemeral_public_key)
        .map_err(|_| GiftWrapError::InvalidKey)?;
    let aead_key = hkdf_key(&ecdh_secret, REQUEST_INFO);
    let plaintext = decrypt(&aead_key, ciphertext)?;

    if plaintext.len() < SECRET_LEN {
        return Err(GiftWrapError::Truncated);
    }
    let (secret_bytes, envelope_bytes) = plaintext.split_at(SECRET_LEN);
    let mut shared_secret = [0u8; SECRET_LEN];
    shared_secret.copy_from_slice(secret_bytes);

    Ok((envelope_bytes.to_vec(), shared_secret))
}

/// Seal `plaintext` (an encoded envelope, or a reject's diagnostic bytes)
/// with `shared_secret` -- the request's own secret, no second key
/// exchange. Returns the wire bytes to carry as `Fulfill.data`, or as
/// `Reject.data` for a reject raised at the termination.
pub fn seal_response(shared_secret: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    seal_response_with_randomness_impl(shared_secret, plaintext, &nonce_bytes)
}

/// The deterministic core [`seal_response`] wraps, parameterized on the AEAD
/// nonce it would otherwise draw from [`OsRng`] -- the response-direction
/// counterpart to [`seal_request_with_randomness`], for the same reason
/// (issue #527's vector generation). Gated behind the `test-util` feature
/// (issue #587) for the same footgun reason: see
/// [`seal_request_with_randomness`]'s doc comment.
#[cfg(any(test, feature = "test-util"))]
pub fn seal_response_with_randomness(
    shared_secret: &[u8; 32],
    plaintext: &[u8],
    nonce_bytes: &[u8; NONCE_LEN],
) -> Vec<u8> {
    seal_response_with_randomness_impl(shared_secret, plaintext, nonce_bytes)
}

fn seal_response_with_randomness_impl(
    shared_secret: &[u8; 32],
    plaintext: &[u8],
    nonce_bytes: &[u8; NONCE_LEN],
) -> Vec<u8> {
    let aead_key = hkdf_key(shared_secret, RESPONSE_INFO);
    let ciphertext = encrypt_with_nonce(&aead_key, plaintext, *nonce_bytes);
    let mut out = Vec::with_capacity(1 + ciphertext.len());
    out.push(TYPE_GIFTWRAP_RESPONSE);
    out.extend_from_slice(&ciphertext);
    out
}

/// Open a sealed response with the same shared secret [`seal_request`]
/// returned for the request it answers.
pub fn open_response(shared_secret: &[u8; 32], bytes: &[u8]) -> Result<Vec<u8>, GiftWrapError> {
    if bytes.is_empty() {
        return Err(GiftWrapError::Truncated);
    }
    if bytes[0] != TYPE_GIFTWRAP_RESPONSE {
        return Err(GiftWrapError::InvalidType(TYPE_GIFTWRAP_RESPONSE));
    }
    let aead_key = hkdf_key(shared_secret, RESPONSE_INFO);
    decrypt(&aead_key, &bytes[1..])
}

/// The fulfilment a terminating connector derives from a request's shared
/// secret (ADR 0019, issue #525) -- `HKDF-SHA256(shared_secret,
/// "toon-giftwrap-fulfillment")`. The sender checks its own end-to-end
/// delivery by comparing a returned fulfilment against this same derivation
/// (`connector send`, issue #1269 / ADR 0069 -- the packet itself no longer
/// carries a condition to mint), so recovering `shared_secret` (via
/// [`open_request`]) is sufficient to derive a fulfilment that verifies, with
/// no app participation. Domain-separated from
/// [`REQUEST_INFO`]/[`RESPONSE_INFO`] by its own HKDF `info` string, so it
/// can never collide with either AEAD key the same secret also derives.
pub fn derive_fulfillment(shared_secret: &[u8; 32]) -> [u8; 32] {
    hkdf_key(shared_secret, FULFILLMENT_INFO)
}

/// Whether `bytes` is shaped like a sealed response (issue #524's fourth
/// acceptance criterion: a sender can tell a sealed reject from an
/// unsealed one without needing the shared secret to do so). An empty
/// `Reject.data` -- what every reject raised short of a termination
/// carries -- is never mistaken for one.
pub fn looks_like_sealed_response(bytes: &[u8]) -> bool {
    bytes.first() == Some(&TYPE_GIFTWRAP_RESPONSE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalSigner;
    use proptest::prelude::*;

    #[test]
    fn a_sealed_request_opens_with_the_receivers_signer() {
        let receiver = LocalSigner::generate("receiver");
        let plaintext = b"GET / envelope bytes";

        let (sealed, shared_secret) =
            seal_request(plaintext, &receiver.public_key().unwrap()).unwrap();
        let (opened, opened_secret) = open_request(&sealed, &receiver).unwrap();

        assert_eq!(opened, plaintext);
        assert_eq!(opened_secret, shared_secret);
    }

    #[test]
    fn a_sealed_request_does_not_open_under_a_different_identity() {
        let receiver = LocalSigner::generate("receiver");
        let forwarding_hop = LocalSigner::generate("forwarding-hop");
        let (sealed, _secret) = seal_request(b"payload", &receiver.public_key().unwrap()).unwrap();

        let result = open_request(&sealed, &forwarding_hop);

        assert_eq!(result, Err(GiftWrapError::OpenFailed));
    }

    #[test]
    fn each_seal_uses_a_fresh_ephemeral_key_and_shared_secret() {
        let receiver = LocalSigner::generate("receiver");
        let receiver_public = receiver.public_key().unwrap();

        let (first, first_secret) = seal_request(b"payload", &receiver_public).unwrap();
        let (second, second_secret) = seal_request(b"payload", &receiver_public).unwrap();

        assert_ne!(first, second);
        assert_ne!(first_secret, second_secret);
    }

    #[test]
    fn a_tampered_ciphertext_fails_to_open() {
        let receiver = LocalSigner::generate("receiver");
        let (mut sealed, _secret) =
            seal_request(b"payload", &receiver.public_key().unwrap()).unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0xff;

        assert_eq!(
            open_request(&sealed, &receiver),
            Err(GiftWrapError::OpenFailed)
        );
    }

    #[test]
    fn open_request_rejects_the_wrong_type_byte() {
        let receiver = LocalSigner::generate("receiver");
        let (mut sealed, _secret) =
            seal_request(b"payload", &receiver.public_key().unwrap()).unwrap();
        sealed[0] = TYPE_GIFTWRAP_RESPONSE;

        assert_eq!(
            open_request(&sealed, &receiver),
            Err(GiftWrapError::InvalidType(TYPE_GIFTWRAP_REQUEST))
        );
    }

    #[test]
    fn open_request_rejects_truncated_bytes() {
        let receiver = LocalSigner::generate("receiver");
        assert_eq!(
            open_request(&[TYPE_GIFTWRAP_REQUEST], &receiver),
            Err(GiftWrapError::Truncated)
        );
        assert_eq!(open_request(&[], &receiver), Err(GiftWrapError::Truncated));
    }

    #[test]
    fn a_response_opens_with_the_shared_secret_from_its_request() {
        let receiver = LocalSigner::generate("receiver");
        let (sealed_request, shared_secret) =
            seal_request(b"request", &receiver.public_key().unwrap()).unwrap();
        let (_opened, secret_from_request) = open_request(&sealed_request, &receiver).unwrap();

        let sealed_response = seal_response(&secret_from_request, b"response envelope bytes");
        let opened_response = open_response(&shared_secret, &sealed_response).unwrap();

        assert_eq!(opened_response, b"response envelope bytes");
    }

    #[test]
    fn a_response_does_not_open_under_the_wrong_shared_secret() {
        let sealed_response = seal_response(&[1u8; 32], b"response");

        assert_eq!(
            open_response(&[2u8; 32], &sealed_response),
            Err(GiftWrapError::OpenFailed)
        );
    }

    #[test]
    fn a_sealed_response_is_distinguishable_from_an_unsealed_empty_reject() {
        let sealed = seal_response(&[3u8; 32], b"");
        assert!(looks_like_sealed_response(&sealed));
        assert!(!looks_like_sealed_response(&[]));
    }

    #[test]
    fn open_response_rejects_the_wrong_type_byte() {
        assert_eq!(
            open_response(&[9u8; 32], &[TYPE_GIFTWRAP_REQUEST, 0, 0]),
            Err(GiftWrapError::InvalidType(TYPE_GIFTWRAP_RESPONSE))
        );
    }

    #[test]
    fn derive_fulfillment_is_deterministic_for_the_same_secret() {
        let secret = [5u8; 32];
        assert_eq!(derive_fulfillment(&secret), derive_fulfillment(&secret));
    }

    #[test]
    fn derive_fulfillment_differs_across_secrets() {
        assert_ne!(
            derive_fulfillment(&[1u8; 32]),
            derive_fulfillment(&[2u8; 32])
        );
    }

    #[test]
    fn derive_fulfillment_is_domain_separated_from_the_aead_keys() {
        let secret = [7u8; 32];
        let fulfillment = derive_fulfillment(&secret);
        assert_ne!(fulfillment, hkdf_key(&secret, REQUEST_INFO));
        assert_ne!(fulfillment, hkdf_key(&secret, RESPONSE_INFO));
    }

    /// The premise ADR 0019/#525 relies on: `open_request` recovers exactly
    /// the shared secret `seal_request` generated, so deriving a fulfilment
    /// from either side's copy of that secret produces the same value.
    #[test]
    fn a_terminating_connector_derives_the_same_fulfillment_the_sender_would() {
        let receiver = LocalSigner::generate("receiver");
        let (sealed, sender_secret) =
            seal_request(b"payload", &receiver.public_key().unwrap()).unwrap();
        let (_opened, recovered_secret) = open_request(&sealed, &receiver).unwrap();

        assert_eq!(
            derive_fulfillment(&sender_secret),
            derive_fulfillment(&recovered_secret)
        );
    }

    proptest! {
        /// Issue #524's round-trip property, generalized past the fixed
        /// byte strings the worked examples above use: sealing any
        /// plaintext to a receiver's public key and opening it with that
        /// receiver's own signer recovers exactly that plaintext, whatever
        /// it is.
        #[test]
        fn any_plaintext_round_trips_through_seal_and_open_request(
            plaintext in proptest::collection::vec(any::<u8>(), 0..256)
        ) {
            let receiver = LocalSigner::generate("receiver");
            let (sealed, shared_secret) =
                seal_request(&plaintext, &receiver.public_key().unwrap()).unwrap();
            let (opened, opened_secret) = open_request(&sealed, &receiver).unwrap();

            prop_assert_eq!(opened, plaintext);
            prop_assert_eq!(opened_secret, shared_secret);
        }

        /// Same property, response direction: any plaintext sealed under a
        /// shared secret opens with that same secret to exactly itself.
        #[test]
        fn any_plaintext_round_trips_through_seal_and_open_response(
            secret in proptest::array::uniform32(any::<u8>()),
            plaintext in proptest::collection::vec(any::<u8>(), 0..256)
        ) {
            let sealed = seal_response(&secret, &plaintext);
            let opened = open_response(&secret, &sealed).unwrap();

            prop_assert_eq!(opened, plaintext);
        }
    }
}
