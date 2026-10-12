//! RFC 9421 HTTP Message Signatures, verify-only (ADR 0008).
//!
//! This is the mechanism ADR 0008 points at a second surface: the same
//! kind of signature that already binds a claim to a client-edge request,
//! now binding an operator write to the request it authorizes. Only
//! verification lives here -- the connector never signs a write, it only
//! checks one presented by an operator's own tooling.
//!
//! Deliberately narrow, matching the MVP subset this repository already
//! established for RFC 9421 (`packages/connector/src/auth/rfc9421`, deleted
//! with the embedded TypeScript node in #457 -- see git history):
//!
//!   - Exactly one covered-component set: `@method`, `@path`,
//!     `content-digest` (RFC 9530, §2.1) -- no `;sf`/`;bs`/`;tr` component
//!     parameters, no field-value canonicalisation beyond OWS-trimming a
//!     single header line.
//!   - `alg="ed25519"` only. `keyid` is the signer's own ed25519 public
//!     key, hex-encoded -- the connector's operator allowlist is a set of
//!     these keys directly, so "is this key allowed to write" is a set
//!     membership check with no separate identity lookup.
//!   - `expires` is REQUIRED (unlike the client-edge MVP, which deferred
//!     it): a write's validity window is what makes replay detection
//!     boundable, per this ticket's AC.
//!
//! `verify_write_signature` is a pure function: given the request facts,
//! the current time and the allowlist, it either returns the write's
//! identity or a specific reason it was rejected. It has no side effects
//! -- replay tracking and the audit log are [`crate::write_auth::WriteAuth`]'s
//! job, one layer up, because they need state this function must not own.

use std::collections::HashSet;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{PublicKey, Signature, Verifier};
use sha2::{Digest, Sha256};

/// The fixed, ordered set of covered component identifiers a write's
/// signature must cover -- exactly this set, no more, no fewer.
pub const COVERED_COMPONENTS: &[&str] = &["@method", "@path", "content-digest"];

/// The only signature algorithm this verifier accepts.
pub const SIGNATURE_ALG: &str = "ed25519";

/// The `@signature-params` values a labelled `Signature-Input` entry
/// carries (RFC 9421 §2.3).
#[derive(Debug, Clone)]
pub struct SignatureParams {
    pub created: u64,
    pub expires: Option<u64>,
    pub keyid: String,
    pub alg: Option<String>,
}

/// Every way a presented write signature can be rejected. Each variant is
/// a specific, actionable reason -- never a bare "unauthorized" -- so a
/// caller can tell "you sent nothing" apart from "you sent something that
/// doesn't check out."
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteAuthError {
    /// `Signature` and/or `Signature-Input` is absent.
    MissingSignature,
    /// A header is present but does not parse as this MVP's grammar.
    MalformedSignature(String),
    /// `alg` is not `"ed25519"`.
    UnsupportedAlg(String),
    /// The signed component set is not exactly [`COVERED_COMPONENTS`].
    CoveredComponentsMismatch,
    /// `Content-Digest` is missing.
    DigestMissing,
    /// `Content-Digest` does not parse.
    DigestMalformed,
    /// `Content-Digest` names an algorithm other than `sha-256`.
    DigestUnsupportedAlg,
    /// `Content-Digest` does not match the request body.
    DigestMismatch,
    /// `@signature-params` has no `expires` -- a write's signature must
    /// carry a bounded validity window.
    MissingExpiry,
    /// `now` is past the signed `expires`.
    Expired,
    /// `keyid` is not hex, or not a 32-byte ed25519 public key.
    InvalidKeyid,
    /// `keyid` decodes, but is not on the operator write allowlist.
    KeyNotAllowlisted,
    /// The signature does not verify against `keyid`.
    SignatureInvalid,
    /// This exact signature has already authenticated a write once.
    /// [`verify_write_signature`] never returns this itself -- replay
    /// tracking is stateful and lives one layer up, in
    /// [`crate::write_auth::WriteAuth`] -- but it is part of this type so
    /// both layers report failure the same way.
    Replayed,
    /// The signature verifies, and the key is a controller's restricted key
    /// whose scope does not include this write (ADR 0078). Like `Replayed`,
    /// raised a layer up, in [`crate::write_auth::WriteAuth`]; unlike every
    /// other variant it is a `403`, not a `401`: the caller is known.
    OutOfScope(connector_config::WriteScope),
    /// The signature verifies, and the key is a controller's restricted key
    /// past its expiry (ADR 0078). A `403`, like `OutOfScope`.
    KeyExpired,
}

impl WriteAuthError {
    /// The reason a verified key was refused (`"scope"` or `"expiry"`), or
    /// `None` for a signature that did not verify. It labels the refused-write
    /// metric and decides `403` over `401`.
    pub fn refusal_reason(&self) -> Option<&'static str> {
        match self {
            WriteAuthError::OutOfScope(_) => Some("scope"),
            WriteAuthError::KeyExpired => Some("expiry"),
            _ => None,
        }
    }
}

impl std::fmt::Display for WriteAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteAuthError::MissingSignature => {
                write!(f, "missing Signature / Signature-Input header")
            }
            WriteAuthError::MalformedSignature(detail) => {
                write!(f, "malformed signature: {detail}")
            }
            WriteAuthError::UnsupportedAlg(alg) => write!(f, "unsupported alg '{alg}'"),
            WriteAuthError::CoveredComponentsMismatch => {
                write!(f, "signed components do not match the required set")
            }
            WriteAuthError::DigestMissing => write!(f, "missing Content-Digest header"),
            WriteAuthError::DigestMalformed => write!(f, "malformed Content-Digest header"),
            WriteAuthError::DigestUnsupportedAlg => {
                write!(f, "Content-Digest uses an unsupported algorithm")
            }
            WriteAuthError::DigestMismatch => {
                write!(f, "Content-Digest does not match the request body")
            }
            WriteAuthError::MissingExpiry => write!(f, "signature has no 'expires' parameter"),
            WriteAuthError::Expired => write!(f, "signature has expired"),
            WriteAuthError::InvalidKeyid => write!(f, "keyid is not a valid ed25519 public key"),
            WriteAuthError::KeyNotAllowlisted => {
                write!(f, "keyid is not on the operator write allowlist")
            }
            WriteAuthError::SignatureInvalid => write!(f, "signature does not verify"),
            WriteAuthError::Replayed => write!(f, "signature has already been used"),
            WriteAuthError::OutOfScope(write) => write!(
                f,
                "scope: this key's scope does not include '{write}' (ADR 0078)"
            ),
            WriteAuthError::KeyExpired => write!(f, "expiry: this key has expired (ADR 0078)"),
        }
    }
}

/// A write whose signature has verified: who signed it, and the exact
/// signature bytes -- the audit record ADR 0012 calls for retaining.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedWrite {
    pub keyid: String,
    pub signature: Vec<u8>,
    pub created: u64,
    pub expires: u64,
}

/// Verify a presented write signature against THIS request.
///
/// `now` and `allowlist` are supplied by the caller rather than read from
/// process state, so this function stays pure and its expiry/allowlist
/// behavior is exercised directly in tests without any clock or shared
/// state.
#[allow(clippy::too_many_arguments)]
pub fn verify_write_signature(
    method: &str,
    path: &str,
    signature_input_header: Option<&str>,
    signature_header: Option<&str>,
    content_digest_header: Option<&str>,
    body: &[u8],
    now: u64,
    allowlist: &HashSet<[u8; 32]>,
) -> Result<VerifiedWrite, WriteAuthError> {
    let (sig_input_header, sig_header) = match (signature_input_header, signature_header) {
        (Some(a), Some(b)) => (a, b),
        _ => return Err(WriteAuthError::MissingSignature),
    };

    let mut inputs = parse_signature_input(sig_input_header)
        .ok_or_else(|| WriteAuthError::MalformedSignature("Signature-Input".to_string()))?;
    let mut sigs = parse_signature(sig_header)
        .ok_or_else(|| WriteAuthError::MalformedSignature("Signature".to_string()))?;

    if inputs.len() != 1 {
        return Err(WriteAuthError::MalformedSignature(
            "ambiguous signature label".to_string(),
        ));
    }
    let (label, input) = inputs.pop().expect("checked len == 1");
    let signature_bytes = match sigs.iter().position(|(l, _)| *l == label) {
        Some(index) => sigs.swap_remove(index).1,
        None => {
            return Err(WriteAuthError::MalformedSignature(format!(
                "no signature for label '{label}'"
            )))
        }
    };

    let alg = input.params.alg.as_deref().unwrap_or(SIGNATURE_ALG);
    if alg != SIGNATURE_ALG {
        return Err(WriteAuthError::UnsupportedAlg(alg.to_string()));
    }

    if !same_components(&input.components, COVERED_COMPONENTS) {
        return Err(WriteAuthError::CoveredComponentsMismatch);
    }

    verify_content_digest(content_digest_header, body)?;
    let content_digest = content_digest_header.expect("verify_content_digest checked Some");

    let expires = input.params.expires.ok_or(WriteAuthError::MissingExpiry)?;
    if now > expires {
        return Err(WriteAuthError::Expired);
    }

    let keyid_bytes = hex_decode_32(&input.params.keyid).ok_or(WriteAuthError::InvalidKeyid)?;
    if !allowlist.contains(&keyid_bytes) {
        return Err(WriteAuthError::KeyNotAllowlisted);
    }

    let base = build_signature_base(method, path, content_digest, &input.params);
    let public_key =
        PublicKey::from_bytes(&keyid_bytes).map_err(|_| WriteAuthError::InvalidKeyid)?;
    let signature =
        Signature::from_bytes(&signature_bytes).map_err(|_| WriteAuthError::SignatureInvalid)?;
    public_key
        .verify(base.as_bytes(), &signature)
        .map_err(|_| WriteAuthError::SignatureInvalid)?;

    Ok(VerifiedWrite {
        keyid: input.params.keyid,
        signature: signature_bytes,
        created: input.params.created,
        expires,
    })
}

/// Render the RFC 9421 §2.3 `@signature-params` inner-list value.
pub(crate) fn serialize_signature_params(components: &[&str], params: &SignatureParams) -> String {
    let inner = components
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = format!("({inner})");
    out.push_str(&format!(";created={}", params.created));
    if let Some(expires) = params.expires {
        out.push_str(&format!(";expires={expires}"));
    }
    out.push_str(&format!(";keyid=\"{}\"", params.keyid));
    out.push_str(&format!(
        ";alg=\"{}\"",
        params.alg.as_deref().unwrap_or(SIGNATURE_ALG)
    ));
    out
}

/// Build the canonical signature-base string for [`COVERED_COMPONENTS`]
/// over THIS request. The component set is fixed, so unlike a general
/// RFC 9421 implementation this never fails on a missing component --
/// `@method` and `@path` are always request facts, and `content-digest`
/// has already been confirmed present by [`verify_content_digest`] before
/// this is called.
pub(crate) fn build_signature_base(
    method: &str,
    path: &str,
    content_digest: &str,
    params: &SignatureParams,
) -> String {
    let mut lines = vec![
        format!("\"@method\": {}", method.to_uppercase()),
        format!("\"@path\": {path}"),
        format!("\"content-digest\": {}", content_digest.trim()),
    ];
    lines.push(format!(
        "\"@signature-params\": {}",
        serialize_signature_params(COVERED_COMPONENTS, params)
    ));
    lines.join("\n")
}

fn verify_content_digest(header_value: Option<&str>, body: &[u8]) -> Result<(), WriteAuthError> {
    let value = header_value.ok_or(WriteAuthError::DigestMissing)?;
    let (alg, b64) = parse_content_digest(value).ok_or(WriteAuthError::DigestMalformed)?;
    if alg != "sha-256" {
        return Err(WriteAuthError::DigestUnsupportedAlg);
    }
    let expected = BASE64.encode(Sha256::digest(body));
    if b64 != expected {
        return Err(WriteAuthError::DigestMismatch);
    }
    Ok(())
}

/// Compute the RFC 9530 `Content-Digest` field value for `body`.
///
/// This used to be gated behind `test-util`, on the reasoning that "the
/// connector itself never signs a write -- it only verifies one an
/// operator's own tooling already signed". That stopped being true when
/// `connector send` landed: the binary now *is* an operator's own tooling,
/// originating a packet through `POST /packets` on another node. Signing is
/// therefore ordinary shipped code, and gating it behind a test feature
/// would mean the released binary could not do the one thing the verb is
/// for. Verification stays where it was; only the signing half moved.
pub fn compute_content_digest(body: &[u8]) -> String {
    format!("sha-256=:{}:", BASE64.encode(Sha256::digest(body)))
}

/// Hex-encode a keypair's public key the way `keyid` is written on the
/// wire -- and the way an operator writes it into a node's
/// `[operator] write_keys` allowlist, so a caller can print the value it
/// must be allowlisted under. See [`compute_content_digest`] for why this
/// is no longer behind `test-util`.
pub fn keyid_hex(keypair: &ed25519_dalek::Keypair) -> String {
    keypair
        .public
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Sign a well-formed write request, returning the three headers a
/// caller needs (`signature-input`, `signature`, `content-digest`).
///
/// The inverse of this crate's own verification path, and held to it by
/// that path's tests -- so a change to either side that breaks the pair
/// fails here rather than at a node that refuses a real write. See
/// [`compute_content_digest`] for why this is no longer behind `test-util`.
pub fn sign_request(
    keypair: &ed25519_dalek::Keypair,
    method: &str,
    path: &str,
    body: &[u8],
    created: u64,
    expires: Option<u64>,
) -> (String, String, String) {
    let content_digest = compute_content_digest(body);
    let params = SignatureParams {
        created,
        expires,
        keyid: keyid_hex(keypair),
        alg: Some(SIGNATURE_ALG.to_string()),
    };
    let base = build_signature_base(method, path, &content_digest, &params);
    let expanded = ed25519_dalek::ExpandedSecretKey::from(&keypair.secret);
    let signature = expanded.sign(base.as_bytes(), &keypair.public);
    let signature_input = format!(
        "sig1={}",
        serialize_signature_params(COVERED_COMPONENTS, &params)
    );
    let sig_value = format!("sig1=:{}:", BASE64.encode(signature.to_bytes()));
    (signature_input, sig_value, content_digest)
}

fn parse_content_digest(value: &str) -> Option<(String, String)> {
    let value = value.trim();
    let eq = value.find('=')?;
    let alg = value[..eq].trim().to_lowercase();
    let rest = value[eq + 1..].trim();
    let inner = rest.strip_prefix(':')?.strip_suffix(':')?;
    Some((alg, inner.to_string()))
}

#[derive(Debug, Clone)]
struct ParsedSignatureInput {
    components: Vec<String>,
    params: SignatureParams,
}

/// Parse a `Signature-Input` header into its labelled entries, in order.
/// The grammar is the RFC 8941 Dictionary `label=(...);params`. This MVP
/// parser handles the subset our verifier needs: quoted inner-list
/// members, and the parameters `created`/`expires` (integers) and
/// `keyid`/`alg` (strings).
fn parse_signature_input(value: &str) -> Option<Vec<(String, ParsedSignatureInput)>> {
    let mut out = Vec::new();
    for member in split_top_level(value, ',') {
        let eq = member.find('=')?;
        let label = member[..eq].trim().to_string();
        let rest = member[eq + 1..].trim();
        if label.is_empty() || !rest.starts_with('(') {
            return None;
        }

        let close = rest.find(')')?;
        let inner = rest[1..close].trim();
        let mut components = Vec::new();
        if !inner.is_empty() {
            for tok in inner.split_whitespace() {
                components.push(parse_quoted_string(tok)?);
            }
        }

        let mut created = None;
        let mut expires = None;
        let mut keyid = None;
        let mut alg = None;
        for p in rest[close + 1..].split(';') {
            let t = p.trim();
            if t.is_empty() {
                continue;
            }
            let pe = t.find('=')?;
            let k = t[..pe].trim();
            let v = t[pe + 1..].trim();
            match k {
                "created" => created = Some(v.parse::<u64>().ok()?),
                "expires" => expires = Some(v.parse::<u64>().ok()?),
                "keyid" => keyid = Some(strip_quotes_lenient(v)),
                "alg" => alg = Some(strip_quotes_lenient(v)),
                _ => {}
            }
        }

        out.push((
            label,
            ParsedSignatureInput {
                components,
                params: SignatureParams {
                    created: created?,
                    expires,
                    keyid: keyid?,
                    alg,
                },
            },
        ));
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Parse a `Signature` header (`label=:base64:`) into label/bytes pairs,
/// in order.
fn parse_signature(value: &str) -> Option<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    for member in split_top_level(value, ',') {
        let eq = member.find('=')?;
        let label = member[..eq].trim().to_string();
        let rest = member[eq + 1..].trim();
        if label.is_empty() {
            return None;
        }
        let inner = rest.strip_prefix(':')?.strip_suffix(':')?;
        let bytes = BASE64.decode(inner).ok()?;
        out.push((label, bytes));
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Split `s` on `sep` at top level, ignoring separators inside `(...)` or
/// `"..."`.
fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_quote = false;
    let mut start = 0usize;
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '"' {
            in_quote = !in_quote;
        } else if !in_quote && ch == '(' {
            depth += 1;
        } else if !in_quote && ch == ')' {
            depth -= 1;
        } else if !in_quote && depth == 0 && ch == sep {
            parts.push(chars[start..i].iter().collect::<String>());
            start = i + 1;
        }
    }
    parts.push(chars[start..].iter().collect::<String>());
    parts
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Strict quoted-string parse: `"foo"` -> `Some("foo")`, anything else
/// (including an unquoted token) -> `None`. Used for inner-list members,
/// which RFC 8941 requires to be quoted Strings.
fn parse_quoted_string(tok: &str) -> Option<String> {
    if tok.len() >= 2 && tok.starts_with('"') && tok.ends_with('"') {
        Some(tok[1..tok.len() - 1].to_string())
    } else {
        None
    }
}

/// Lenient quoted-string unwrap for a parameter value: strips surrounding
/// quotes if present, otherwise returns the token unchanged.
fn strip_quotes_lenient(v: &str) -> String {
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

fn same_components(a: &[String], b: &[&str]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut sa: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
    sa.sort_unstable();
    let mut sb: Vec<&str> = b.to_vec();
    sb.sort_unstable();
    sa == sb
}

fn hex_decode_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{ExpandedSecretKey, Keypair};
    use rand::rngs::OsRng;

    fn keypair() -> Keypair {
        Keypair::generate(&mut OsRng)
    }

    fn allowlist_of(keypair: &Keypair) -> HashSet<[u8; 32]> {
        let mut set = HashSet::new();
        set.insert(keypair.public.to_bytes());
        set
    }

    #[test]
    fn a_validly_signed_write_verifies() {
        let keypair = keypair();
        let body = b"{\"prefix\":\"g.example\"}";
        let (sig_input, sig, digest) =
            sign_request(&keypair, "POST", "/routes", body, 1_000, Some(2_000));

        let result = verify_write_signature(
            "POST",
            "/routes",
            Some(&sig_input),
            Some(&sig),
            Some(&digest),
            body,
            1_500,
            &allowlist_of(&keypair),
        );

        let verified = result.expect("verify");
        assert_eq!(verified.keyid, keyid_hex(&keypair));
        assert_eq!(verified.created, 1_000);
        assert_eq!(verified.expires, 2_000);
    }

    #[test]
    fn a_write_with_no_signature_headers_is_rejected() {
        let result = verify_write_signature(
            "POST",
            "/routes",
            None,
            None,
            None,
            b"body",
            1_000,
            &HashSet::new(),
        );
        assert_eq!(result, Err(WriteAuthError::MissingSignature));
    }

    #[test]
    fn a_malformed_signature_input_is_rejected() {
        let keypair = keypair();
        let body = b"body";
        let (_, sig, digest) = sign_request(&keypair, "POST", "/routes", body, 1_000, Some(2_000));

        let result = verify_write_signature(
            "POST",
            "/routes",
            Some("not a valid structured field"),
            Some(&sig),
            Some(&digest),
            body,
            1_500,
            &allowlist_of(&keypair),
        );
        assert!(matches!(result, Err(WriteAuthError::MalformedSignature(_))));
    }

    #[test]
    fn a_signature_from_a_key_not_on_the_allowlist_is_rejected() {
        let signer = keypair();
        let allowlisted = keypair();
        let body = b"body";
        let (sig_input, sig, digest) =
            sign_request(&signer, "POST", "/routes", body, 1_000, Some(2_000));

        let result = verify_write_signature(
            "POST",
            "/routes",
            Some(&sig_input),
            Some(&sig),
            Some(&digest),
            body,
            1_500,
            &allowlist_of(&allowlisted),
        );
        assert_eq!(result, Err(WriteAuthError::KeyNotAllowlisted));
    }

    #[test]
    fn removing_a_key_from_the_allowlist_revokes_it_with_no_other_change() {
        let keypair = keypair();
        let body = b"body";
        let (sig_input, sig, digest) =
            sign_request(&keypair, "POST", "/routes", body, 1_000, Some(2_000));

        // Same request, same signature -- only the allowlist changes.
        let with_key = verify_write_signature(
            "POST",
            "/routes",
            Some(&sig_input),
            Some(&sig),
            Some(&digest),
            body,
            1_500,
            &allowlist_of(&keypair),
        );
        assert!(with_key.is_ok());

        let without_key = verify_write_signature(
            "POST",
            "/routes",
            Some(&sig_input),
            Some(&sig),
            Some(&digest),
            body,
            1_500,
            &HashSet::new(),
        );
        assert_eq!(without_key, Err(WriteAuthError::KeyNotAllowlisted));
    }

    #[test]
    fn a_tampered_body_fails_the_content_digest_binding() {
        let keypair = keypair();
        let original_body = b"{\"amount\":1}";
        let (sig_input, sig, digest) = sign_request(
            &keypair,
            "POST",
            "/routes",
            original_body,
            1_000,
            Some(2_000),
        );

        let tampered_body = b"{\"amount\":999999}";
        let result = verify_write_signature(
            "POST",
            "/routes",
            Some(&sig_input),
            Some(&sig),
            Some(&digest),
            tampered_body,
            1_500,
            &allowlist_of(&keypair),
        );
        assert_eq!(result, Err(WriteAuthError::DigestMismatch));
    }

    #[test]
    fn a_signature_with_no_expires_is_rejected() {
        let keypair = keypair();
        let body = b"body";
        let (sig_input, sig, digest) = sign_request(&keypair, "POST", "/routes", body, 1_000, None);

        let result = verify_write_signature(
            "POST",
            "/routes",
            Some(&sig_input),
            Some(&sig),
            Some(&digest),
            body,
            1_500,
            &allowlist_of(&keypair),
        );
        assert_eq!(result, Err(WriteAuthError::MissingExpiry));
    }

    #[test]
    fn a_signature_past_its_expiry_is_rejected() {
        let keypair = keypair();
        let body = b"body";
        let (sig_input, sig, digest) =
            sign_request(&keypair, "POST", "/routes", body, 1_000, Some(2_000));

        let result = verify_write_signature(
            "POST",
            "/routes",
            Some(&sig_input),
            Some(&sig),
            Some(&digest),
            body,
            2_001,
            &allowlist_of(&keypair),
        );
        assert_eq!(result, Err(WriteAuthError::Expired));
    }

    #[test]
    fn a_signature_covering_the_wrong_component_set_is_rejected() {
        let keypair = keypair();
        let body = b"body";
        let content_digest = compute_content_digest(body);
        let params = SignatureParams {
            created: 1_000,
            expires: Some(2_000),
            keyid: keyid_hex(&keypair),
            alg: Some(SIGNATURE_ALG.to_string()),
        };
        // Sign over only `@method` and `@path` -- missing `content-digest`.
        let base = build_signature_base("POST", "/routes", &content_digest, &params);
        let short_components: &[&str] = &["@method", "@path"];
        let base_missing_digest = base
            .lines()
            .filter(|l| !l.starts_with("\"content-digest\""))
            .collect::<Vec<_>>()
            .join("\n")
            .replace(
                &serialize_signature_params(COVERED_COMPONENTS, &params),
                &serialize_signature_params(short_components, &params),
            );
        let expanded = ExpandedSecretKey::from(&keypair.secret);
        let signature = expanded.sign(base_missing_digest.as_bytes(), &keypair.public);
        let signature_input = format!(
            "sig1={}",
            serialize_signature_params(short_components, &params)
        );
        let sig_value = format!("sig1=:{}:", BASE64.encode(signature.to_bytes()));

        let result = verify_write_signature(
            "POST",
            "/routes",
            Some(&signature_input),
            Some(&sig_value),
            Some(&content_digest),
            body,
            1_500,
            &allowlist_of(&keypair),
        );
        assert_eq!(result, Err(WriteAuthError::CoveredComponentsMismatch));
    }

    #[test]
    fn an_unsupported_algorithm_is_rejected() {
        let keypair = keypair();
        let body = b"body";
        let content_digest = compute_content_digest(body);
        let params = SignatureParams {
            created: 1_000,
            expires: Some(2_000),
            keyid: keyid_hex(&keypair),
            alg: Some("rsa-pss-sha512".to_string()),
        };
        let base = build_signature_base("POST", "/routes", &content_digest, &params);
        let expanded = ExpandedSecretKey::from(&keypair.secret);
        let signature = expanded.sign(base.as_bytes(), &keypair.public);
        let signature_input = format!(
            "sig1={}",
            serialize_signature_params(COVERED_COMPONENTS, &params)
        );
        let sig_value = format!("sig1=:{}:", BASE64.encode(signature.to_bytes()));

        let result = verify_write_signature(
            "POST",
            "/routes",
            Some(&signature_input),
            Some(&sig_value),
            Some(&content_digest),
            body,
            1_500,
            &allowlist_of(&keypair),
        );
        assert_eq!(
            result,
            Err(WriteAuthError::UnsupportedAlg("rsa-pss-sha512".to_string()))
        );
    }

    #[test]
    fn a_signature_bound_to_a_different_path_does_not_verify_here() {
        let keypair = keypair();
        let body = b"body";
        let (sig_input, sig, digest) =
            sign_request(&keypair, "POST", "/routes", body, 1_000, Some(2_000));

        // A captured request replayed against a different write endpoint.
        let result = verify_write_signature(
            "POST",
            "/channels",
            Some(&sig_input),
            Some(&sig),
            Some(&digest),
            body,
            1_500,
            &allowlist_of(&keypair),
        );
        assert_eq!(result, Err(WriteAuthError::SignatureInvalid));
    }

    #[test]
    fn compute_and_verify_content_digest_round_trip() {
        let body = b"hello operator";
        let digest = compute_content_digest(body);
        assert!(verify_content_digest(Some(&digest), body).is_ok());
        assert_eq!(
            verify_content_digest(Some(&digest), b"different body"),
            Err(WriteAuthError::DigestMismatch)
        );
    }
}
