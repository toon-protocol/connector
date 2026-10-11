use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::ConfigError;

/// The `[operator]` section as written in the config file. Its presence
/// enables the operator surface (ADR 0008); its absence means the surface
/// is not started at all, and none of the fields below are read.
///
/// Each of the two settings the surface needs is spelled as **exactly one
/// of** a literal or a path to a file holding it (issue #1003):
///
/// * `bearer_token` / `bearer_token_file`
/// * `write_keys` / `write_keys_file`
///
/// The file forms are the deployed forms, and the reason they exist is the
/// reason `[[peers]] credential.secret_file` exists (issue #750): this
/// fleet's `connector-rust.toml` files are committed to a **public**
/// repository, so a section written with literals cannot be committed at
/// all. The store box's operator surface was configured on the box only for
/// exactly that reason -- untracked drift that any `fleet-ops` reconcile of
/// the committed tree would have deleted without a word, since `main`
/// carried no `[operator]` section to reconcile *to*. Every other secret in
/// those same files is already a path (`[signer] key_file`,
/// `[settlement.*.key] key_file`, `credential.secret_file`); this makes the
/// operator surface's the same shape, and the box reproducible.
///
/// `write_keys_file` holds no secret -- an allowlist entry is an ed25519
/// PUBLIC key -- and it is a file for the second half of the same reason:
/// ADR 0008 revokes write authority by removing a key and restarting, and
/// config is immutable for the process lifetime (ADR 0009). Behind a file,
/// a revocation is an edit on the box; behind a committed literal it is a
/// pull request, a green CI run and a promotion.
///
/// `deny_unknown_fields` (issue #556): a mistyped `bearer_tokn` or
/// `write_key` would otherwise be dropped and the section resolved as
/// "present but unauthenticated", which `resolve_operator` then reports as
/// a missing token rather than as the typo it is. The surface's own
/// fail-closed guarantee (ADR 0008) is only as good as the parse beneath it.
/// It is also why a hard break here would be an outage: an image that does
/// not know `bearer_token_file` refuses the config outright rather than
/// ignoring it, so the file forms are ADDED beside the literals and the
/// literals stay accepted (see [`resolve_operator`]).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawOperatorConfig {
    #[serde(default)]
    bearer_token: Option<String>,
    #[serde(default)]
    bearer_token_file: Option<PathBuf>,
    #[serde(default)]
    write_keys: Option<Vec<String>>,
    #[serde(default)]
    write_keys_file: Option<PathBuf>,
    /// How many recent packets to keep in memory for `GET /packets`
    /// (ADR 0077). Absent or `0` keeps none; not a required key.
    #[serde(default)]
    packet_history: Option<usize>,
}

/// The literal bearer token never reaches a [`fmt::Debug`] rendering, for
/// the same reason [`RawPeerCredential`](crate::peer)'s secret does not: a
/// raw config is a whole-value thing, `RawConfig` derives `Debug`, and a
/// derived `Debug` anywhere on the path from file to [`OperatorConfig`] is
/// enough to put the credential that gates every operator read into a log
/// aggregator. The write keys are public and the paths are not secret, so
/// both render as themselves -- a redaction that hides which *file* was
/// read would only make the missing-file case harder to diagnose.
impl fmt::Debug for RawOperatorConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawOperatorConfig")
            .field(
                "bearer_token",
                &self.bearer_token.as_ref().map(|_| "<redacted>"),
            )
            .field("bearer_token_file", &self.bearer_token_file)
            .field("write_keys", &self.write_keys)
            .field("write_keys_file", &self.write_keys_file)
            .field("packet_history", &self.packet_history)
            .finish()
    }
}

/// The operator surface's authentication, fully validated (ADR 0008): a
/// bearer token that gates every read, and an allowlist of ed25519 public
/// keys that gate every write via an RFC 9421 signature. Constructed only
/// by [`resolve_operator`], so a value that exists is never
/// unauthenticated -- there is no way to enable the surface without both.
#[derive(Clone, PartialEq, Eq)]
pub struct OperatorConfig {
    bearer_token: String,
    write_keys: Vec<[u8; 32]>,
    packet_history: usize,
}

/// Same redaction as [`RawOperatorConfig`]'s, one step further along: a
/// `Config` derives `Debug` and is exactly the kind of value that gets
/// logged whole at startup.
impl fmt::Debug for OperatorConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OperatorConfig")
            .field("bearer_token", &"<redacted>")
            .field(
                "write_keys",
                &format_args!("{} key(s)", self.write_keys.len()),
            )
            .field("packet_history", &self.packet_history)
            .finish()
    }
}

impl OperatorConfig {
    /// The bearer token every read on the operator surface must present.
    pub fn bearer_token(&self) -> &str {
        &self.bearer_token
    }

    /// The ed25519 public keys allowed to sign a write. Removing a key
    /// from this list and restarting is how an operator's write authority
    /// is revoked (ADR 0008) -- config is immutable for the process
    /// lifetime (ADR 0009), so revocation is never a live operation.
    pub fn write_keys(&self) -> &[[u8; 32]] {
        &self.write_keys
    }

    /// How many recent packets the node keeps in memory for the operator to
    /// watch (ADR 0077); `0` keeps none.
    pub fn packet_history(&self) -> usize {
        self.packet_history
    }
}

fn parse_write_key(value: &str) -> Result<[u8; 32], ConfigError> {
    hex_decode_32(value).ok_or_else(|| ConfigError::OperatorInvalidWriteKey {
        value: value.to_string(),
    })
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

/// Read one of the `[operator]` section's `*_file` settings, whole.
///
/// The file is read **here, at config load**, and not left as a pointer to
/// be resolved when the surface first authenticates something, for the same
/// reason `credential.secret_file` is read at load: the alternative is a
/// node that starts, serves, and only discovers at the first operator
/// request that the file it was pointed at is not there -- a surface that
/// reads as enabled and authenticates nobody. ADR 0009 puts that failure at
/// load instead, so a missing or unreadable file is a refuse-to-start error
/// naming the setting, exactly as [`ConfigError::SignerKeyFileNotFound`] is
/// for `[signer] key_file`. The path is resolved the same way those are (by
/// the OS, against the process's working directory), which is why the
/// committed fleet configs write absolute container paths.
fn read_operator_file(setting: &'static str, path: &Path) -> Result<String, ConfigError> {
    if !path.is_file() {
        return Err(ConfigError::OperatorFileNotFound {
            setting,
            path: path.to_path_buf(),
        });
    }
    std::fs::read_to_string(path).map_err(|source| ConfigError::OperatorFileUnreadable {
        setting,
        path: path.to_path_buf(),
        source,
    })
}

/// Where the bearer token comes from: exactly one of the literal or the
/// file. See [`resolve_operator`] for why "both" is an error rather than a
/// precedence rule.
///
/// A file's contents are **trimmed**. Operators write these with `echo` and
/// `openssl rand -hex 32 >`, both of which append a newline, and a token
/// that failed to match because of one invisible byte is a 401 with no
/// evidence at all -- the same reasoning, and the same asymmetry, as
/// `credential.secret_file`: the literal form is deliberately NOT trimmed,
/// it is byte-for-byte what it was before this field existed.
fn resolve_bearer_token(
    literal: Option<String>,
    file: Option<PathBuf>,
) -> Result<String, ConfigError> {
    match (literal, file) {
        (Some(_), Some(_)) => Err(ConfigError::OperatorSettingAmbiguous {
            literal: "bearer_token",
            file: "bearer_token_file",
        }),
        (Some(token), None) if !token.trim().is_empty() => Ok(token),
        (None, Some(path)) => {
            let token = read_operator_file("bearer_token_file", &path)?
                .trim()
                .to_string();
            if token.is_empty() {
                return Err(ConfigError::OperatorFileEmpty {
                    setting: "bearer_token_file",
                    path,
                });
            }
            Ok(token)
        }
        // Neither field set, or `bearer_token = ""`. One condition, because
        // it is one outcome: an enabled surface with no read authentication.
        (_, None) => Err(ConfigError::OperatorMissingBearerToken),
    }
}

/// The write-key allowlist, from exactly one of the inline array or the
/// file. See [`parse_write_keys_file`] for the file's format.
fn resolve_write_keys(
    literal: Option<Vec<String>>,
    file: Option<PathBuf>,
) -> Result<Vec<[u8; 32]>, ConfigError> {
    match (literal, file) {
        (Some(_), Some(_)) => Err(ConfigError::OperatorSettingAmbiguous {
            literal: "write_keys",
            file: "write_keys_file",
        }),
        (Some(keys), None) if !keys.is_empty() => {
            keys.iter().map(|key| parse_write_key(key)).collect()
        }
        (None, Some(path)) => {
            let contents = read_operator_file("write_keys_file", &path)?;
            let keys = parse_write_keys_file(&contents, &path)?;
            if keys.is_empty() {
                return Err(ConfigError::OperatorFileEmpty {
                    setting: "write_keys_file",
                    path,
                });
            }
            Ok(keys)
        }
        // Neither field set, or `write_keys = []`. One outcome: an enabled
        // surface that would accept a write from no one.
        (_, None) => Err(ConfigError::OperatorNoWriteKeys),
    }
}

/// One 64-character hex ed25519 public key per line.
///
/// Blank lines are skipped and everything from a `#` to end of line is a
/// comment, because this is the one operator file a human edits by hand
/// rather than generates: ADR 0008's revocation story is "remove a key and
/// restart", and an allowlist you cannot annotate with whose key each entry
/// is, is one you revoke from by guessing. A `#` can never appear inside a
/// hex key, so stripping on it cannot eat one.
///
/// A malformed entry names its line, not just its value: a hand-edited file
/// is where an off-by-one paste actually happens, and "which of these nine
/// lines" is the whole question.
fn parse_write_keys_file(contents: &str, path: &Path) -> Result<Vec<[u8; 32]>, ConfigError> {
    let mut keys = Vec::new();
    for (index, raw_line) in contents.lines().enumerate() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let key =
            hex_decode_32(line).ok_or_else(|| ConfigError::OperatorWriteKeysFileInvalidKey {
                path: path.to_path_buf(),
                line: index + 1,
                value: line.to_string(),
            })?;
        keys.push(key);
    }
    Ok(keys)
}

/// Validate an optional `[operator]` section. Presence enables the
/// operator surface; if enabled, a non-empty bearer token and at least
/// one write key are both required. A surface that is enabled but would
/// have no authentication -- an empty token, or no allowlisted write key
/// -- must refuse to start rather than run open (ADR 0009's "refuse to
/// start" contract, applied to ADR 0008's auth requirement).
///
/// **Inline and file are mutually exclusive, and neither wins** (issue
/// #1003). "The file wins" was the other candidate, and it is the wrong
/// one here for the same reason `credential`'s `secret`/`secret_file` pair
/// and `[signer]`'s `key_file`/`kms_key_id` pair both refuse rather than
/// pick: two answers to "where does the credential that gates this surface
/// come from" is not a merge, it is an unanswerable question, and the
/// losing literal stays in the file looking authoritative. The config that
/// motivated this change is the exact hazard -- a box whose committed file
/// says one thing and whose live file says another -- so a rule that lets
/// both be written and quietly ignores one of them would reintroduce it in
/// a new spelling.
///
/// The literal forms are NOT removed and NOT an error. `RawOperatorConfig`
/// is `deny_unknown_fields` in both directions: an old image refuses a
/// config carrying `bearer_token_file`, and a new image that dropped
/// `bearer_token` would refuse the config the store box is running right
/// now. Deleting the literals would therefore take the live surface down at
/// the first promotion, which is precisely the outage this change exists to
/// prevent. They remain the right form for a test fixture, and for any
/// config that is never committed.
pub(crate) fn resolve_operator(
    raw: Option<RawOperatorConfig>,
) -> Result<Option<OperatorConfig>, ConfigError> {
    let Some(raw) = raw else {
        return Ok(None);
    };

    let bearer_token = resolve_bearer_token(raw.bearer_token, raw.bearer_token_file)?;
    let write_keys = resolve_write_keys(raw.write_keys, raw.write_keys_file)?;

    Ok(Some(OperatorConfig {
        bearer_token,
        write_keys,
        packet_history: raw.packet_history.unwrap_or(0),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;

    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const OTHER_KEY: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    fn raw(bearer_token: &str, write_keys: &[&str]) -> RawOperatorConfig {
        RawOperatorConfig {
            bearer_token: Some(bearer_token.to_string()),
            bearer_token_file: None,
            write_keys: Some(write_keys.iter().map(|k| k.to_string()).collect()),
            write_keys_file: None,
            packet_history: None,
        }
    }

    #[test]
    fn packet_history_is_off_unless_configured() {
        let off = resolve_operator(Some(raw("token", &[KEY])))
            .expect("resolve")
            .expect("some");
        assert_eq!(off.packet_history(), 0);

        let mut on = raw("token", &[KEY]);
        on.packet_history = Some(500);
        let on = resolve_operator(Some(on)).expect("resolve").expect("some");
        assert_eq!(on.packet_history(), 500);
    }

    /// A temp file holding `contents`, kept alive by the caller -- dropping
    /// the handle deletes the file, and every one of these is read during
    /// the `resolve_operator` call it is passed to.
    fn file_holding(contents: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(contents.as_bytes()).expect("write");
        file.flush().expect("flush");
        file
    }

    #[test]
    fn absent_operator_section_resolves_to_none() {
        let resolved = resolve_operator(None).expect("resolve");
        assert_eq!(resolved, None);
    }

    #[test]
    fn a_fully_configured_section_resolves() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let resolved = resolve_operator(Some(raw("secret-token", &[key])))
            .expect("resolve")
            .expect("some");

        assert_eq!(resolved.bearer_token(), "secret-token");
        assert_eq!(resolved.write_keys().len(), 1);
        assert_eq!(resolved.write_keys()[0][0], 0x01);
    }

    #[test]
    fn rejects_an_empty_bearer_token() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let result = resolve_operator(Some(raw("", &[key])));
        assert!(matches!(
            result,
            Err(ConfigError::OperatorMissingBearerToken)
        ));
    }

    #[test]
    fn rejects_a_blank_bearer_token() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let result = resolve_operator(Some(raw("   ", &[key])));
        assert!(matches!(
            result,
            Err(ConfigError::OperatorMissingBearerToken)
        ));
    }

    #[test]
    fn rejects_no_write_keys() {
        let result = resolve_operator(Some(raw("secret-token", &[])));
        assert!(matches!(result, Err(ConfigError::OperatorNoWriteKeys)));
    }

    #[test]
    fn rejects_a_write_key_of_the_wrong_length() {
        let result = resolve_operator(Some(raw("secret-token", &["abcd"])));
        assert!(matches!(
            result,
            Err(ConfigError::OperatorInvalidWriteKey { .. })
        ));
    }

    #[test]
    fn rejects_a_non_hex_write_key() {
        let non_hex = "zz23456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let result = resolve_operator(Some(raw("secret-token", &[non_hex])));
        assert!(matches!(
            result,
            Err(ConfigError::OperatorInvalidWriteKey { .. })
        ));
    }

    // ── The file forms (issue #1003) ──────────────────────────────────────
    //
    // The deployed spelling: the store box's committed config names two
    // paths, and the files behind them are gitignored and placed on the box.
    // Everything below is about that pair being at least as strict as the
    // literals it replaces -- a surface that starts unauthenticated is the
    // one failure ADR 0008 does not tolerate.

    #[test]
    fn both_settings_resolve_from_files() {
        let token = file_holding("file-token\n");
        let keys = file_holding(&format!("{KEY}\n"));

        let resolved = resolve_operator(Some(RawOperatorConfig {
            bearer_token: None,
            bearer_token_file: Some(token.path().to_path_buf()),
            write_keys: None,
            write_keys_file: Some(keys.path().to_path_buf()),
            packet_history: None,
        }))
        .expect("resolve")
        .expect("some");

        assert_eq!(resolved.bearer_token(), "file-token");
        assert_eq!(resolved.write_keys().len(), 1);
        assert_eq!(resolved.write_keys()[0][0], 0x01);
    }

    /// One file form beside one literal is fine: they are two independent
    /// settings, and a box mid-migration should not have to move both at
    /// once.
    #[test]
    fn a_file_backed_token_composes_with_inline_write_keys() {
        let token = file_holding("file-token");

        let resolved = resolve_operator(Some(RawOperatorConfig {
            bearer_token: None,
            bearer_token_file: Some(token.path().to_path_buf()),
            write_keys: Some(vec![KEY.to_string()]),
            write_keys_file: None,
            packet_history: None,
        }))
        .expect("resolve")
        .expect("some");

        assert_eq!(resolved.bearer_token(), "file-token");
        assert_eq!(resolved.write_keys().len(), 1);
    }

    /// `openssl rand -hex 32 > token` and `echo … > token` both append a
    /// newline; a token that fails to match on one invisible byte is a 401
    /// with no evidence at all.
    #[test]
    fn a_token_file_is_trimmed_of_surrounding_whitespace() {
        let token = file_holding("  file-token \t\r\n\n");

        let resolved = resolve_operator(Some(RawOperatorConfig {
            bearer_token: None,
            bearer_token_file: Some(token.path().to_path_buf()),
            write_keys: Some(vec![KEY.to_string()]),
            write_keys_file: None,
            packet_history: None,
        }))
        .expect("resolve")
        .expect("some");

        assert_eq!(resolved.bearer_token(), "file-token");
    }

    /// The inline form is byte-for-byte what it was before the file form
    /// existed -- deliberately not trimmed, so no config that loads today
    /// authenticates differently tomorrow.
    #[test]
    fn an_inline_token_is_not_trimmed() {
        let resolved = resolve_operator(Some(raw(" padded-token ", &[KEY])))
            .expect("resolve")
            .expect("some");

        assert_eq!(resolved.bearer_token(), " padded-token ");
    }

    #[test]
    fn a_write_keys_file_takes_one_key_per_line_and_ignores_blanks_and_comments() {
        let keys = file_holding(&format!(
            "# the operators allowed to write, as of the 2026-08 rotation\n\
             \n\
             {KEY}   # alice\n\
             \t{OTHER_KEY}\n\
             \n"
        ));

        let resolved = resolve_operator(Some(RawOperatorConfig {
            bearer_token: Some("secret-token".to_string()),
            bearer_token_file: None,
            write_keys: None,
            write_keys_file: Some(keys.path().to_path_buf()),
            packet_history: None,
        }))
        .expect("resolve")
        .expect("some");

        assert_eq!(resolved.write_keys().len(), 2);
        assert_eq!(resolved.write_keys()[0][0], 0x01);
        assert_eq!(resolved.write_keys()[1][0], 0xfe);
    }

    /// Two answers to "where does this come from" is not a merge. Both
    /// settings refuse the same way, and the error names both spellings so
    /// the fix does not need a source read.
    #[test]
    fn setting_both_a_token_and_a_token_file_is_refused_by_name() {
        let token = file_holding("file-token");

        let result = resolve_operator(Some(RawOperatorConfig {
            bearer_token: Some("inline-token".to_string()),
            bearer_token_file: Some(token.path().to_path_buf()),
            write_keys: Some(vec![KEY.to_string()]),
            write_keys_file: None,
            packet_history: None,
        }));

        let message = result.expect_err("ambiguous").to_string();
        assert!(message.contains("bearer_token"), "{message}");
        assert!(message.contains("bearer_token_file"), "{message}");
    }

    #[test]
    fn setting_both_write_keys_and_a_write_keys_file_is_refused_by_name() {
        let keys = file_holding(KEY);

        let result = resolve_operator(Some(RawOperatorConfig {
            bearer_token: Some("secret-token".to_string()),
            bearer_token_file: None,
            write_keys: Some(vec![OTHER_KEY.to_string()]),
            write_keys_file: Some(keys.path().to_path_buf()),
            packet_history: None,
        }));

        let message = result.expect_err("ambiguous").to_string();
        assert!(message.contains("write_keys"), "{message}");
        assert!(message.contains("write_keys_file"), "{message}");
    }

    /// The whole point of reading at load: a surface pointed at a file that
    /// is not there must refuse to start, not start and reject everything.
    #[test]
    fn a_missing_token_file_is_refused_by_name() {
        let result = resolve_operator(Some(RawOperatorConfig {
            bearer_token: None,
            bearer_token_file: Some(PathBuf::from("/nonexistent/operator-bearer-token")),
            write_keys: Some(vec![KEY.to_string()]),
            write_keys_file: None,
            packet_history: None,
        }));

        let message = result.expect_err("missing file").to_string();
        assert!(message.contains("bearer_token_file"), "{message}");
        assert!(
            message.contains("/nonexistent/operator-bearer-token"),
            "{message}"
        );
    }

    #[test]
    fn a_missing_write_keys_file_is_refused_by_name() {
        let result = resolve_operator(Some(RawOperatorConfig {
            bearer_token: Some("secret-token".to_string()),
            bearer_token_file: None,
            write_keys: None,
            write_keys_file: Some(PathBuf::from("/nonexistent/operator-write-keys")),
            packet_history: None,
        }));

        let message = result.expect_err("missing file").to_string();
        assert!(message.contains("write_keys_file"), "{message}");
        assert!(
            message.contains("/nonexistent/operator-write-keys"),
            "{message}"
        );
    }

    /// A directory stands in for "unreadable": it exists as a path and is
    /// not a file, which is the shape a bind mount takes when the file
    /// behind it was never created (Docker creates a directory).
    #[test]
    fn a_token_file_that_is_a_directory_is_refused_by_name() {
        let dir = tempfile::tempdir().expect("temp dir");

        let result = resolve_operator(Some(RawOperatorConfig {
            bearer_token: None,
            bearer_token_file: Some(dir.path().to_path_buf()),
            write_keys: Some(vec![KEY.to_string()]),
            write_keys_file: None,
            packet_history: None,
        }));

        let message = result.expect_err("not a file").to_string();
        assert!(message.contains("bearer_token_file"), "{message}");
    }

    #[test]
    fn a_token_file_that_is_not_text_is_refused_by_name() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(&[0xff, 0xfe, 0x00]).expect("write");
        file.flush().expect("flush");

        let result = resolve_operator(Some(RawOperatorConfig {
            bearer_token: None,
            bearer_token_file: Some(file.path().to_path_buf()),
            write_keys: Some(vec![KEY.to_string()]),
            write_keys_file: None,
            packet_history: None,
        }));

        let message = result.expect_err("unreadable").to_string();
        assert!(message.contains("bearer_token_file"), "{message}");
    }

    #[test]
    fn an_empty_or_whitespace_only_token_file_is_refused_by_name() {
        for contents in ["", "   ", "\n", "\t\r\n "] {
            let token = file_holding(contents);

            let result = resolve_operator(Some(RawOperatorConfig {
                bearer_token: None,
                bearer_token_file: Some(token.path().to_path_buf()),
                write_keys: Some(vec![KEY.to_string()]),
                write_keys_file: None,
                packet_history: None,
            }));

            let message = result
                .expect_err("empty token file should be refused")
                .to_string();
            assert!(message.contains("bearer_token_file"), "{message}");
        }
    }

    /// A file of nothing but comments is empty in the only sense that
    /// matters: the allowlist it produces admits no one.
    #[test]
    fn a_write_keys_file_with_no_keys_is_refused_by_name() {
        for contents in ["", "\n\n", "# every operator was revoked\n"] {
            let keys = file_holding(contents);

            let result = resolve_operator(Some(RawOperatorConfig {
                bearer_token: Some("secret-token".to_string()),
                bearer_token_file: None,
                write_keys: None,
                write_keys_file: Some(keys.path().to_path_buf()),
                packet_history: None,
            }));

            let message = result
                .expect_err("keyless allowlist should be refused")
                .to_string();
            assert!(message.contains("write_keys_file"), "{message}");
        }
    }

    #[test]
    fn a_malformed_write_keys_file_entry_names_its_line() {
        let keys = file_holding(&format!("{KEY}\n\n# bob\nnot-a-key\n"));

        let result = resolve_operator(Some(RawOperatorConfig {
            bearer_token: Some("secret-token".to_string()),
            bearer_token_file: None,
            write_keys: None,
            write_keys_file: Some(keys.path().to_path_buf()),
            packet_history: None,
        }));

        let error = result.expect_err("malformed entry");
        assert!(matches!(
            error,
            ConfigError::OperatorWriteKeysFileInvalidKey { line: 4, .. }
        ));
        assert!(error.to_string().contains(":4"), "{error}");
    }

    /// The bearer token is the credential that gates every operator read.
    /// `RawConfig` and `Config` both derive `Debug` and both get logged
    /// whole; neither may carry it.
    #[test]
    fn neither_the_raw_nor_the_resolved_debug_rendering_carries_the_token() {
        let raw_config = raw("super-secret-token", &[KEY]);
        let rendered = format!("{raw_config:?}");
        assert!(!rendered.contains("super-secret-token"), "{rendered}");

        let resolved = resolve_operator(Some(raw_config))
            .expect("resolve")
            .expect("some");
        let rendered = format!("{resolved:?}");
        assert!(!rendered.contains("super-secret-token"), "{rendered}");
    }
}
