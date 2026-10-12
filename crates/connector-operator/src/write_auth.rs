//! The stateful half of write authentication (ADR 0008): the operator
//! allowlist, a replay cache, and the audit log a verified write is
//! retained into.
//!
//! [`crate::rfc9421::verify_write_signature`] is pure -- it has no memory
//! of a request it has already seen. [`WriteAuth`] is what gives the
//! surface memory: [`WriteAuth::authenticate`] rejects a signature it has
//! already accepted once (replay), and on success retains the signature
//! itself as the write's audit record (ADR 0012) rather than a log line
//! asserting that something happened.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use connector_config::{ControllerKey, WriteScope};
use serde::Serialize;

use crate::rfc9421::{verify_write_signature, VerifiedWrite, WriteAuthError};

/// A retained record of one authenticated write: the signature itself,
/// plus enough request context to make it meaningful. This is the audit
/// record ADR 0012 calls for -- non-repudiable, and naming a specific key
/// -- rather than a log line asserting that something happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditRecord {
    pub keyid: String,
    pub signature: String,
    pub method: String,
    pub path: String,
    pub created: u64,
    pub expires: u64,
}

/// One row of `GET /write-keys`: a key allowed to sign a write. An
/// unrestricted key (`write_keys`) lists every write as its scope; a
/// restricted one (`controller_keys_file`) lists its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WriteKeyView {
    pub keyid: String,
    pub restricted: bool,
    pub name: Option<String>,
    pub scope: Vec<&'static str>,
    pub expires: Option<String>,
}

fn to_hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The 32 bytes a verified `keyid` names.
fn verified_key(keyid: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    if keyid.len() != 64 {
        return None;
    }
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(keyid.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// The operator write surface's authentication state: which keys may
/// write, which signatures have already been spent, and the audit trail
/// of every write accepted so far.
pub struct WriteAuth {
    /// Every key that may sign a write: `write_keys` and the controllers'.
    allowlist: HashSet<[u8; 32]>,
    /// The operator's keys, which keep every write (ADR 0008), in the order
    /// configured.
    full: Vec<[u8; 32]>,
    /// The restricted keys (ADR 0078), by key.
    restricted: HashMap<[u8; 32], ControllerKey>,
    seen_signatures: Mutex<HashMap<Vec<u8>, u64>>,
    audit_log: Mutex<Vec<AuditRecord>>,
}

impl WriteAuth {
    /// Construct write authentication over `write_keys` -- the ed25519
    /// public keys permitted to sign any write -- and `controller_keys`, the
    /// restricted keys of controllers (ADR 0078), each permitted only the
    /// writes its scope names, until its expiry. Revoking a key is a
    /// deploy-time change to these lists (ADR 0009: config is immutable for
    /// the process lifetime), not a runtime operation.
    pub fn with_controller_keys(
        write_keys: Vec<[u8; 32]>,
        controller_keys: Vec<ControllerKey>,
    ) -> WriteAuth {
        let allowlist = write_keys
            .iter()
            .copied()
            .chain(controller_keys.iter().map(|controller| *controller.key()))
            .collect();
        WriteAuth {
            allowlist,
            full: write_keys,
            restricted: controller_keys
                .into_iter()
                .map(|controller| (*controller.key(), controller))
                .collect(),
            seen_signatures: Mutex::new(HashMap::new()),
            audit_log: Mutex::new(Vec::new()),
        }
    }

    /// Verify a presented write signature against THIS request, reject a
    /// replay of a signature already accepted, and -- on success --
    /// retain the signature as this write's audit record.
    ///
    /// A restricted key's scope and expiry are checked once the signature
    /// has verified and **before** it is remembered or retained: a refused
    /// write leaves no audit record and does not spend the signature.
    #[allow(clippy::too_many_arguments)]
    pub fn authenticate(
        &self,
        write: WriteScope,
        method: &str,
        path: &str,
        signature_input: Option<&str>,
        signature: Option<&str>,
        content_digest: Option<&str>,
        body: &[u8],
    ) -> Result<VerifiedWrite, WriteAuthError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is before the Unix epoch")
            .as_secs();

        let verified = verify_write_signature(
            method,
            path,
            signature_input,
            signature,
            content_digest,
            body,
            now,
            &self.allowlist,
        )?;

        {
            let mut seen = self
                .seen_signatures
                .lock()
                .expect("write-auth replay cache lock poisoned");
            // A signature's own `expires` bounds how long it needs to be
            // remembered: once it can no longer verify as unexpired, it
            // can never be replayed successfully, so it is safe to forget.
            seen.retain(|_, expires_at| *expires_at >= now);
            // A replay is a `401` whatever the key's authority now is: a
            // signature accepted before its key expired and presented again
            // after is a replay first.
            if seen.contains_key(&verified.signature) {
                return Err(WriteAuthError::Replayed);
            }
            if let Some(controller) =
                verified_key(&verified.keyid).and_then(|key| self.restricted.get(&key))
            {
                if controller.expires().is_some_and(|expires| now >= expires) {
                    return Err(WriteAuthError::KeyExpired);
                }
                if !controller.scope().contains(&write) {
                    return Err(WriteAuthError::OutOfScope(write));
                }
            }
            seen.insert(verified.signature.clone(), verified.expires);
        }

        self.audit_log
            .lock()
            .expect("write-auth audit log lock poisoned")
            .push(AuditRecord {
                keyid: verified.keyid.clone(),
                signature: BASE64.encode(&verified.signature),
                method: method.to_string(),
                path: path.to_string(),
                created: verified.created,
                expires: verified.expires,
            });

        Ok(verified)
    }

    /// Every write key, for `GET /write-keys`: the operator's first, then
    /// the controllers' in name order.
    pub fn keys(&self) -> Vec<WriteKeyView> {
        let mut views: Vec<WriteKeyView> = self
            .full
            .iter()
            .map(|key| WriteKeyView {
                keyid: to_hex(key),
                restricted: false,
                name: None,
                scope: WriteScope::ALL.iter().map(|s| s.name()).collect(),
                expires: None,
            })
            .collect();
        let mut controllers: Vec<&ControllerKey> = self.restricted.values().collect();
        controllers.sort_by(|a, b| a.name().cmp(b.name()));
        views.extend(controllers.into_iter().map(|controller| WriteKeyView {
            keyid: to_hex(controller.key()),
            restricted: true,
            name: Some(controller.name().to_string()),
            scope: controller.scope().iter().map(|s| s.name()).collect(),
            expires: controller.expires().and_then(|at| {
                chrono::DateTime::from_timestamp(i64::try_from(at).ok()?, 0)
                    .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            }),
        }));
        views
    }

    /// Every write authenticated so far, in the order it was accepted.
    pub fn audit_log(&self) -> Vec<AuditRecord> {
        self.audit_log
            .lock()
            .expect("write-auth audit log lock poisoned")
            .clone()
    }
}

/// Authenticate a write request against `write_auth`, reading the three
/// RFC 9421 headers out of `headers`. Every future write endpoint (route
/// CRUD, channel lifecycle, packet origination -- ADR 0008) calls this
/// first, before doing anything else with the request, and maps an `Err`
/// to `401 Unauthorized` itself -- kept a plain [`WriteAuthError`] here
/// rather than a pre-built HTTP response, so this stays a small,
/// `Copy`-free but cheaply-sized value rather than embedding a whole
/// response type in every call site's `Result`.
pub(crate) fn authenticate_write(
    write_auth: &WriteAuth,
    write: WriteScope,
    method: &str,
    path: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<VerifiedWrite, WriteAuthError> {
    let header_str = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    write_auth.authenticate(
        write,
        method,
        path,
        header_str("signature-input"),
        header_str("signature"),
        header_str("content-digest"),
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rfc9421::{keyid_hex, sign_request};
    use ed25519_dalek::Keypair;
    use rand::rngs::OsRng;

    fn keypair() -> Keypair {
        Keypair::generate(&mut OsRng)
    }

    fn sign(
        keypair: &Keypair,
        method: &str,
        path: &str,
        body: &[u8],
        expires: u64,
    ) -> (String, String, String) {
        sign_request(keypair, method, path, body, 1_000, Some(expires))
    }

    #[test]
    fn a_valid_write_is_authenticated_and_retained_as_an_audit_record() {
        let keypair = keypair();
        let auth = WriteAuth::with_controller_keys(vec![keypair.public.to_bytes()], vec![]);
        let body = b"{\"prefix\":\"g.example\"}";
        let (sig_input, sig, digest) = sign(&keypair, "POST", "/routes", body, 9_999_999_999);

        let result = auth.authenticate(
            WriteScope::Packets,
            "POST",
            "/routes",
            Some(&sig_input),
            Some(&sig),
            Some(&digest),
            body,
        );
        assert!(result.is_ok());

        let log = auth.audit_log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].keyid, keyid_hex(&keypair));
        assert_eq!(log[0].method, "POST");
        assert_eq!(log[0].path, "/routes");
    }

    #[test]
    fn a_rejected_write_is_not_retained_as_an_audit_record() {
        let keypair = keypair();
        let auth = WriteAuth::with_controller_keys(vec![], vec![]); // key not allowlisted
        let body = b"body";
        let (sig_input, sig, digest) = sign(&keypair, "POST", "/routes", body, 9_999_999_999);

        let result = auth.authenticate(
            WriteScope::Packets,
            "POST",
            "/routes",
            Some(&sig_input),
            Some(&sig),
            Some(&digest),
            body,
        );
        assert!(result.is_err());
        assert!(auth.audit_log().is_empty());
    }

    #[test]
    fn replaying_the_same_signature_is_rejected_the_second_time() {
        let keypair = keypair();
        let auth = WriteAuth::with_controller_keys(vec![keypair.public.to_bytes()], vec![]);
        let body = b"body";
        let (sig_input, sig, digest) = sign(&keypair, "POST", "/routes", body, 9_999_999_999);

        let first = auth.authenticate(
            WriteScope::Packets,
            "POST",
            "/routes",
            Some(&sig_input),
            Some(&sig),
            Some(&digest),
            body,
        );
        assert!(first.is_ok());

        let second = auth.authenticate(
            WriteScope::Packets,
            "POST",
            "/routes",
            Some(&sig_input),
            Some(&sig),
            Some(&digest),
            body,
        );
        assert_eq!(second, Err(WriteAuthError::Replayed));

        // Exactly one write was recorded -- the replay never reached the
        // audit log.
        assert_eq!(auth.audit_log().len(), 1);
    }

    #[test]
    fn a_signature_replayed_after_its_key_expires_is_a_replay_not_an_expiry() {
        let keypair = keypair();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs();
        let controller = ControllerKey::new(
            "agent",
            keypair.public.to_bytes(),
            vec![WriteScope::ChannelsLand],
            Some(now + 2),
        );
        let auth = WriteAuth::with_controller_keys(vec![], vec![controller]);
        let body = b"body";
        let (sig_input, sig, digest) =
            sign(&keypair, "POST", "/channels/c/land", body, 9_999_999_999);
        let attempt = || {
            auth.authenticate(
                WriteScope::ChannelsLand,
                "POST",
                "/channels/c/land",
                Some(&sig_input),
                Some(&sig),
                Some(&digest),
                body,
            )
        };

        assert!(attempt().is_ok());
        std::thread::sleep(std::time::Duration::from_secs(3));
        // `401` stays the answer to a replayed signature (#1487).
        assert_eq!(attempt(), Err(WriteAuthError::Replayed));
        assert_eq!(auth.audit_log().len(), 1);
    }
}
