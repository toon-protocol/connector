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
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use connector_config::{ControllerKey, ControllerKeysFile, WriteScope};
use serde::Serialize;

use crate::day_totals::{CountKind, CountRefusal, DayTotals, Figures};
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
    /// The spending limit and what the key has counted against it today
    /// (ADR 0078 decision 11); `None` for a key with no limit.
    pub spending_limit: Option<LimitView>,
    /// The deposit limit and what the key has counted against it today
    /// (ADR 0078 decision 11); `None` for a key with no limit.
    pub deposit_limit: Option<LimitView>,
}

/// A restricted key's spending or deposit limit, as `GET /write-keys` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LimitView {
    pub per_write: u64,
    pub per_day: u64,
    /// Counted against `per_day` so far in the current UTC day.
    pub counted_today: u64,
}

/// A restricted key's deposit count: what the channel writes ask before they
/// journal or send a deposit.
pub struct KeyDeposits {
    key: String,
    limit: Option<connector_config::DepositLimit>,
    totals: Option<Arc<DayTotals>>,
}

impl connector_runtime::DepositGate for KeyDeposits {
    fn count(&self, amount: u128) -> Result<(), String> {
        let refusal = match (&self.totals, self.limit) {
            (Some(totals), Some(limit)) => totals
                .count(CountKind::Deposit, &self.key, amount, limit)
                .err(),
            (None, _) => Some(CountRefusal::Unrecorded {
                kind: CountKind::Deposit,
                error: "this node keeps no day totals".to_string(),
            }),
            (_, None) => Some(CountRefusal::Unrecorded {
                kind: CountKind::Deposit,
                error: "this key states no deposit limit".to_string(),
            }),
        };
        refusal.map_or(Ok(()), |refusal| Err(refusal.to_string()))
    }
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

/// The restricted keys (ADR 0078), by key.
type Restricted = HashMap<[u8; 32], ControllerKey>;

fn by_key(controller_keys: Vec<ControllerKey>) -> Arc<Restricted> {
    Arc::new(
        controller_keys
            .into_iter()
            .map(|controller| (*controller.key(), controller))
            .collect(),
    )
}

/// Where the operator surface's restricted keys come from.
pub enum ControllerKeySource {
    /// A list fixed for the process lifetime: a node whose config names no
    /// `controller_keys_file` has the empty one.
    Fixed(Vec<ControllerKey>),
    /// `[operator] controller_keys_file`, read again on every signed request
    /// (ADR 0079), so that an entry removed from it is refused at its key's
    /// next request with no restart.
    File(ControllerKeysFile),
}

impl From<Vec<ControllerKey>> for ControllerKeySource {
    fn from(controller_keys: Vec<ControllerKey>) -> ControllerKeySource {
        ControllerKeySource::Fixed(controller_keys)
    }
}

impl From<ControllerKeysFile> for ControllerKeySource {
    fn from(file: ControllerKeysFile) -> ControllerKeySource {
        ControllerKeySource::File(file)
    }
}

/// [`ControllerKeySource`] as [`WriteAuth`] holds it.
enum RestrictedKeys {
    Fixed(Arc<Restricted>),
    File {
        file: ControllerKeysFile,
        last: Mutex<LastReading>,
    },
}

/// What the last reading of `controller_keys_file` found.
enum LastReading {
    Unread,
    /// The contents and what they parsed to, so an unchanged file is read
    /// and compared but not parsed again.
    Parsed(String, Arc<Restricted>),
    /// Why the file was unusable, so one breakage is one line in the log
    /// however many requests arrive while it lasts.
    Unusable(String),
}

impl RestrictedKeys {
    /// The restricted keys as they stand now. A file that cannot be read or
    /// no longer parses yields **none**: every restricted key is then an
    /// unlisted key until the file is repaired, because the alternative --
    /// serving the last list that parsed -- would keep honouring a key its
    /// operator may have just tried to revoke.
    fn now(&self) -> Arc<Restricted> {
        let (file, last) = match self {
            RestrictedKeys::Fixed(keys) => return Arc::clone(keys),
            RestrictedKeys::File { file, last } => (file, last),
        };
        let mut last = last.lock().expect("controller keys lock poisoned");
        let parsed = file.read().and_then(|contents| match &*last {
            LastReading::Parsed(seen, keys) if *seen == contents => Ok(Arc::clone(keys)),
            _ => {
                let keys = by_key(file.parse(&contents)?);
                *last = LastReading::Parsed(contents, Arc::clone(&keys));
                Ok(keys)
            }
        });
        parsed.unwrap_or_else(|error| {
            let error = error.to_string();
            if !matches!(&*last, LastReading::Unusable(logged) if *logged == error) {
                tracing::error!(
                    path = %file.path().display(),
                    "controller_keys_file is unusable, so every restricted key is refused: {error}"
                );
                *last = LastReading::Unusable(error);
            }
            Arc::new(Restricted::new())
        })
    }
}

/// The operator write surface's authentication state: which keys may
/// write, which signatures have already been spent, and the audit trail
/// of every write accepted so far.
pub struct WriteAuth {
    /// The operator's keys, which keep every write (ADR 0008), in the order
    /// configured.
    full: Vec<[u8; 32]>,
    /// The restricted keys (ADR 0078).
    restricted: RestrictedKeys,
    /// Where spending limits count (ADR 0078 decision 8).
    day_totals: Option<Arc<DayTotals>>,
    seen_signatures: Mutex<HashMap<Vec<u8>, u64>>,
    audit_log: Mutex<Vec<AuditRecord>>,
}

impl WriteAuth {
    /// Construct write authentication over `write_keys` -- the ed25519
    /// public keys permitted to sign any write -- and `controller_keys`, the
    /// restricted keys of controllers (ADR 0078), each permitted only the
    /// writes its scope names, until its expiry. Revoking a key on
    /// `write_keys` is a deploy-time change (ADR 0009: config is immutable
    /// for the process lifetime). Revoking a restricted key is an edit to
    /// `controller_keys_file`, which binds at the key's next request when
    /// `controller_keys` is [`ControllerKeySource::File`] (ADR 0079).
    pub fn with_controller_keys(
        write_keys: Vec<[u8; 32]>,
        controller_keys: impl Into<ControllerKeySource>,
        day_totals: Option<Arc<DayTotals>>,
    ) -> WriteAuth {
        WriteAuth {
            full: write_keys,
            restricted: match controller_keys.into() {
                ControllerKeySource::Fixed(keys) => RestrictedKeys::Fixed(by_key(keys)),
                ControllerKeySource::File(file) => RestrictedKeys::File {
                    file,
                    last: Mutex::new(LastReading::Unread),
                },
            },
            day_totals,
            seen_signatures: Mutex::new(HashMap::new()),
            audit_log: Mutex::new(Vec::new()),
        }
    }

    /// Every key that may sign a request: `write_keys` and `restricted`.
    fn allowlist(&self, restricted: &Restricted) -> HashSet<[u8; 32]> {
        self.full.iter().chain(restricted.keys()).copied().collect()
    }

    /// Verify a signature presented on a **read**, in place of the bearer
    /// token (ADR 0079): any key that may sign a write, restricted or not,
    /// may read, and a scope says nothing about reads. A restricted key
    /// past its expiry or gone from `controller_keys_file` is refused here
    /// as it is on a write.
    ///
    /// A read's signature is neither spent nor retained: it is good until
    /// its own `expires`, and the audit log records accepted writes. A
    /// replayed read tells its replayer what the bearer token would.
    pub fn authenticate_read(
        &self,
        method: &str,
        path: &str,
        signature_input: Option<&str>,
        signature: Option<&str>,
        content_digest: Option<&str>,
    ) -> Result<(), WriteAuthError> {
        // A request that presents no signature is refused before the file is
        // read: only a signed request reaches it.
        if signature_input.is_none() || signature.is_none() {
            return Err(WriteAuthError::MissingSignature);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is before the Unix epoch")
            .as_secs();
        let restricted = self.restricted.now();
        let verified = verify_write_signature(
            method,
            path,
            signature_input,
            signature,
            content_digest,
            b"",
            now,
            &self.allowlist(&restricted),
        )?;
        let expired = verified_key(&verified.keyid)
            .and_then(|key| restricted.get(&key))
            .and_then(ControllerKey::expires)
            .is_some_and(|expires| now >= expires);
        if expired {
            return Err(WriteAuthError::KeyExpired);
        }
        Ok(())
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

        if signature_input.is_none() || signature.is_none() {
            return Err(WriteAuthError::MissingSignature);
        }
        // One reading of the list serves this whole check: the key that
        // verified is judged by the entry it verified under.
        let restricted = self.restricted.now();
        let verified = verify_write_signature(
            method,
            path,
            signature_input,
            signature,
            content_digest,
            body,
            now,
            &self.allowlist(&restricted),
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
                verified_key(&verified.keyid).and_then(|key| restricted.get(&key))
            {
                let key_hex = to_hex(controller.key());
                if controller.expires().is_some_and(|expires| now >= expires) {
                    return Err(WriteAuthError::KeyExpired);
                }
                if !controller.scope().contains(&write) {
                    return Err(WriteAuthError::OutOfScope(write));
                }
                // A packet counts at its full amount once accepted, whatever
                // becomes of it (ADR 0078 decision 3), and the count is
                // journaled before the packet is handed on. The replay lock
                // is held across the check and the append, so two packets
                // arriving together cannot both pass a check one of them
                // would fail. A body that is not a `Prepare` is not a
                // packet: the handler answers it `400` and nothing is sent.
                if write == WriteScope::Packets {
                    if let (Some(limit), Ok(prepare)) = (
                        controller.spending_limit(),
                        connector_domain::Prepare::decode(body),
                    ) {
                        let refusal = match &self.day_totals {
                            Some(totals) => totals
                                .count(
                                    CountKind::Spending,
                                    &key_hex,
                                    u128::from(prepare.amount),
                                    limit,
                                )
                                .err(),
                            None => Some(CountRefusal::Unrecorded {
                                kind: CountKind::Spending,
                                error: "this node keeps no day totals".to_string(),
                            }),
                        };
                        if let Some(refusal) = refusal {
                            return Err(WriteAuthError::LimitRefused(refusal));
                        }
                    }
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

    fn limit_view(
        &self,
        controller: &ControllerKey,
        kind: CountKind,
        figures: Figures,
    ) -> LimitView {
        LimitView {
            per_write: figures.per_write,
            per_day: figures.per_day,
            counted_today: self.day_totals.as_ref().map_or(0, |totals| {
                totals.counted_today(kind, &to_hex(controller.key()))
            }),
        }
    }

    /// Where the deposits of the write just authenticated under `keyid` are
    /// counted (ADR 0078 decision 4): `None` for a key on `write_keys`, which
    /// is never counted or limited. A restricted key that states no deposit
    /// limit gets a gate that refuses -- config makes that unreachable for any
    /// scope that deposits, and a deposit nothing bounds is not made. So does
    /// a key that left `controller_keys_file` after its write authenticated
    /// (ADR 0079): only a key on `write_keys` is ever unlimited.
    pub fn deposit_gate(&self, keyid: &str) -> Option<KeyDeposits> {
        let key = verified_key(keyid)?;
        if self.full.contains(&key) {
            return None;
        }
        Some(KeyDeposits {
            key: to_hex(&key),
            limit: self
                .restricted
                .now()
                .get(&key)
                .and_then(ControllerKey::deposit_limit),
            totals: self.day_totals.clone(),
        })
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
                spending_limit: None,
                deposit_limit: None,
            })
            .collect();
        let restricted = self.restricted.now();
        let mut controllers: Vec<&ControllerKey> = restricted.values().collect();
        controllers.sort_by(|a, b| a.name().cmp(b.name()));
        views.extend(controllers.into_iter().map(|controller| {
            WriteKeyView {
                keyid: to_hex(controller.key()),
                restricted: true,
                name: Some(controller.name().to_string()),
                scope: controller.scope().iter().map(|s| s.name()).collect(),
                spending_limit: controller
                    .spending_limit()
                    .map(|limit| self.limit_view(controller, CountKind::Spending, limit.into())),
                deposit_limit: controller
                    .deposit_limit()
                    .map(|limit| self.limit_view(controller, CountKind::Deposit, limit.into())),
                expires: controller.expires().and_then(|at| {
                    chrono::DateTime::from_timestamp(i64::try_from(at).ok()?, 0)
                        .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
                }),
            }
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
        let auth = WriteAuth::with_controller_keys(vec![keypair.public.to_bytes()], vec![], None);
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
        let auth = WriteAuth::with_controller_keys(vec![], vec![], None); // key not allowlisted
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
        let auth = WriteAuth::with_controller_keys(vec![keypair.public.to_bytes()], vec![], None);
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
        let auth = WriteAuth::with_controller_keys(vec![], vec![controller], None);
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

    /// `[name]` holding `keypair`, scoped to `packets` with a spending limit
    /// and to `channels-fund` with a deposit limit.
    fn limited_entry(keypair: &Keypair, per_day: u64) -> String {
        format!(
            "[agent]\nkey = \"{}\"\nscope = [\"packets\", \"channels-fund\"]\n\
             spending_limit = {{ per_write = 100, per_day = {per_day} }}\n\
             deposit_limit = {{ per_write = 100, per_day = {per_day} }}\n",
            keyid_hex(keypair)
        )
    }

    fn day_totals(dir: &tempfile::TempDir) -> Arc<DayTotals> {
        Arc::new(
            DayTotals::open(
                &dir.path().join(crate::DAY_TOTALS_JOURNAL),
                Arc::new(connector_runtime::SystemClock),
            )
            .expect("open day totals"),
        )
    }

    #[test]
    fn removing_an_entry_and_adding_it_back_does_not_reset_its_day() {
        use connector_runtime::DepositGate;

        let keypair = keypair();
        let dir = tempfile::tempdir().expect("state dir");
        let file = dir.path().join("controllers");
        std::fs::write(&file, limited_entry(&keypair, 150)).expect("write");
        let auth = WriteAuth::with_controller_keys(
            vec![],
            ControllerKeysFile::new(file.clone(), vec![]),
            Some(day_totals(&dir)),
        );
        let keyid = keyid_hex(&keypair);
        let deposit = |amount| auth.deposit_gate(&keyid).expect("restricted").count(amount);
        assert!(deposit(100).is_ok());

        std::fs::write(&file, "").expect("revoke");
        // Gone from the file after its write authenticated: refused, never
        // treated as an operator's unlimited key.
        assert!(deposit(1).is_err());

        std::fs::write(&file, limited_entry(&keypair, 150)).expect("restore");
        assert!(deposit(100).is_err(), "the day's 100 is still counted");
        assert!(deposit(50).is_ok());

        // A limit raised in the file applies against what is already counted.
        std::fs::write(&file, limited_entry(&keypair, 250)).expect("raise");
        assert!(deposit(100).is_ok());
        assert!(deposit(1).is_err());
    }

    #[test]
    fn an_unsigned_request_is_refused_without_reading_the_file() {
        let dir = tempfile::tempdir().expect("state dir");
        let auth = WriteAuth::with_controller_keys(
            vec![],
            // A directory is not a readable file: reading it would mark the
            // list unusable.
            ControllerKeysFile::new(dir.path().to_path_buf(), vec![]),
            None,
        );
        assert_eq!(
            auth.authenticate_read("GET", "/peers", None, None, None),
            Err(WriteAuthError::MissingSignature)
        );
        let RestrictedKeys::File { last, .. } = &auth.restricted else {
            panic!("file source");
        };
        assert!(matches!(*last.lock().unwrap(), LastReading::Unread));
    }
}
