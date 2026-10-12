//! A controller's write key (ADR 0078): an ed25519 public key that may make
//! the writes its **scope** names and no others, until its optional expiry.
//!
//! Restricted keys live in a file of their own, `[operator]
//! controller_keys_file`, one TOML table per key. They are kept apart from
//! `write_keys` / `write_keys_file` on purpose: a key listed there keeps
//! every write (ADR 0008), and a key present in both places is a refusal to
//! start rather than a question of which entry wins.
//!
//! ```toml
//! [agent-1]
//! key = "<64 hex>"
//! scope = ["channels-land", "routes-peers"]
//! expires = "2026-12-31T00:00:00Z"   # optional, RFC 3339
//! ```
//!
//! The spending and deposit limits are parsed and validated here, but this
//! build does not count them, so an entry that states one is refused by name
//! ([`ConfigError::ControllerLimitsNotEnforced`]): no release accepts a limit
//! it does not hold.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde::Deserialize;

use crate::error::ConfigError;

/// One named write of the operator surface (ADR 0078 decision 1). A `DELETE`
/// belongs to the name of its `POST`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WriteScope {
    /// `POST /packets`
    Packets,
    /// `POST /peers`, `DELETE /peers/:id`
    Peers,
    /// `POST /routes/peers`, `DELETE /routes/peers/:prefix`
    RoutesPeers,
    /// `POST /routes/leased`
    RoutesLeased,
    /// `POST /channels`
    ChannelsOpen,
    /// `POST /channels/:id/fund`
    ChannelsFund,
    /// `POST /channels/:id/withdraw`
    ChannelsWithdraw,
    /// `POST /channels/:id/land`
    ChannelsLand,
}

impl WriteScope {
    /// Every name the binary knows, in the order the docs list them.
    pub const ALL: [WriteScope; 8] = [
        WriteScope::Packets,
        WriteScope::Peers,
        WriteScope::RoutesPeers,
        WriteScope::RoutesLeased,
        WriteScope::ChannelsOpen,
        WriteScope::ChannelsFund,
        WriteScope::ChannelsWithdraw,
        WriteScope::ChannelsLand,
    ];

    /// The name as written in a scope list and rendered by `GET /write-keys`.
    pub fn name(self) -> &'static str {
        match self {
            WriteScope::Packets => "packets",
            WriteScope::Peers => "peers",
            WriteScope::RoutesPeers => "routes-peers",
            WriteScope::RoutesLeased => "routes-leased",
            WriteScope::ChannelsOpen => "channels-open",
            WriteScope::ChannelsFund => "channels-fund",
            WriteScope::ChannelsWithdraw => "channels-withdraw",
            WriteScope::ChannelsLand => "channels-land",
        }
    }

    fn parse(name: &str) -> Option<WriteScope> {
        WriteScope::ALL
            .into_iter()
            .find(|scope| scope.name() == name)
    }
}

impl fmt::Display for WriteScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A restricted write key. Every boot refusal is checked where the file is
/// parsed; [`ControllerKey::new`] itself validates nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerKey {
    name: String,
    key: [u8; 32],
    scope: Vec<WriteScope>,
    expires: Option<u64>,
}

impl ControllerKey {
    pub fn new(
        name: impl Into<String>,
        key: [u8; 32],
        mut scope: Vec<WriteScope>,
        expires: Option<u64>,
    ) -> ControllerKey {
        scope.sort();
        scope.dedup();
        ControllerKey {
            name: name.into(),
            key,
            scope,
            expires,
        }
    }

    /// The entry's table name in the file: what a boot refusal and a log
    /// line call it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The ed25519 public key.
    pub fn key(&self) -> &[u8; 32] {
        &self.key
    }

    /// The writes this key may make, sorted and without repeats.
    pub fn scope(&self) -> &[WriteScope] {
        &self.scope
    }

    /// Unix seconds from which every write from the key is refused, if the
    /// entry states one.
    pub fn expires(&self) -> Option<u64> {
        self.expires
    }
}

/// A limit as written: both figures, or the entry is refused.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLimit {
    #[serde(default)]
    per_write: Option<u64>,
    #[serde(default)]
    per_day: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawControllerKey {
    key: String,
    #[serde(default)]
    scope: Vec<String>,
    #[serde(default)]
    expires: Option<String>,
    #[serde(default)]
    spending_limit: Option<RawLimit>,
    #[serde(default)]
    deposit_limit: Option<RawLimit>,
}

fn entry_error(file: &Path, entry: &str, reason: String) -> ConfigError {
    ConfigError::ControllerKeyInvalid {
        path: file.to_path_buf(),
        entry: entry.to_string(),
        reason,
    }
}

fn check_limit(
    path: &Path,
    entry: &str,
    which: &'static str,
    limit: &RawLimit,
) -> Result<(), ConfigError> {
    if limit.per_write.is_some() != limit.per_day.is_some() {
        return Err(ConfigError::ControllerLimitIncomplete {
            path: path.to_path_buf(),
            entry: entry.to_string(),
            limit: which,
        });
    }
    Ok(())
}

/// Parse `controller_keys_file`'s contents. `write_keys` is every key already
/// allowed to make every write, so a restricted key among them is refused.
pub(crate) fn parse_controller_keys(
    contents: &str,
    path: &Path,
    write_keys: &[[u8; 32]],
) -> Result<Vec<ControllerKey>, ConfigError> {
    let table: BTreeMap<String, RawControllerKey> =
        toml::from_str(contents).map_err(|error| ConfigError::ControllerKeysFileInvalid {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

    let mut out: Vec<ControllerKey> = Vec::new();
    for (name, raw) in table {
        let key = crate::operator::hex_decode_32(&raw.key).ok_or_else(|| {
            entry_error(
                path,
                &name,
                format!(
                    "key '{}' must be 64 hex characters (a 32-byte ed25519 public key)",
                    raw.key
                ),
            )
        })?;

        if raw.scope.is_empty() {
            return Err(entry_error(
                path,
                &name,
                "its scope is empty: name the writes it may make".into(),
            ));
        }
        let mut scope = Vec::new();
        for item in &raw.scope {
            let parsed = WriteScope::parse(item).ok_or_else(|| {
                let known: Vec<&str> = WriteScope::ALL.iter().map(|s| s.name()).collect();
                entry_error(
                    path,
                    &name,
                    format!(
                        "scope names '{item}', which is not a write this binary knows (known: {})",
                        known.join(", ")
                    ),
                )
            })?;
            scope.push(parsed);
        }

        if write_keys.contains(&key) {
            return Err(ConfigError::ControllerKeyAlsoWriteKey {
                path: path.to_path_buf(),
                entry: name,
            });
        }
        if let Some(first) = out.iter().find(|existing| existing.key == key) {
            return Err(ConfigError::ControllerKeyDuplicated {
                path: path.to_path_buf(),
                entry: name,
                first: first.name.clone(),
            });
        }

        if let Some(limit) = &raw.spending_limit {
            check_limit(path, &name, "spending_limit", limit)?;
        }
        if let Some(limit) = &raw.deposit_limit {
            check_limit(path, &name, "deposit_limit", limit)?;
        }

        if scope.contains(&WriteScope::Packets) && raw.spending_limit.is_none() {
            return Err(ConfigError::ControllerLimitRequired {
                path: path.to_path_buf(),
                entry: name,
                write: WriteScope::Packets,
                limit: "spending_limit",
            });
        }
        if let Some(write) = [
            WriteScope::Peers,
            WriteScope::ChannelsOpen,
            WriteScope::ChannelsFund,
        ]
        .into_iter()
        .find(|write| scope.contains(write))
        {
            if raw.deposit_limit.is_none() {
                return Err(ConfigError::ControllerLimitRequired {
                    path: path.to_path_buf(),
                    entry: name,
                    write,
                    limit: "deposit_limit",
                });
            }
        }

        if raw.spending_limit.is_some() || raw.deposit_limit.is_some() {
            return Err(ConfigError::ControllerLimitsNotEnforced {
                path: path.to_path_buf(),
                entry: name,
            });
        }

        let expires = match &raw.expires {
            None => None,
            Some(text) => {
                let at = chrono::DateTime::parse_from_rfc3339(text).map_err(|error| {
                    entry_error(
                        path,
                        &name,
                        format!("expires '{text}' is not an RFC 3339 timestamp: {error}"),
                    )
                })?;
                Some(u64::try_from(at.timestamp()).unwrap_or(0))
            }
        };

        out.push(ControllerKey::new(name, key, scope, expires));
    }
    Ok(out)
}

/// Read and parse the file named by `controller_keys_file`.
pub(crate) fn resolve_controller_keys(
    file: Option<&Path>,
    write_keys: &[[u8; 32]],
) -> Result<Vec<ControllerKey>, ConfigError> {
    let Some(path) = file else {
        return Ok(Vec::new());
    };
    let contents = crate::operator::read_operator_file("controller_keys_file", path)?;
    parse_controller_keys(&contents, path, write_keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    fn parse(contents: &str) -> Result<Vec<ControllerKey>, ConfigError> {
        parse_controller_keys(contents, Path::new("controllers.toml"), &[])
    }

    #[test]
    fn an_entry_resolves_with_its_scope_sorted_and_its_expiry() {
        let keys = parse(&format!(
            "[agent]\nkey = \"{KEY}\"\nscope = [\"channels-land\", \"routes-peers\", \"channels-land\"]\nexpires = \"2030-01-01T00:00:00Z\"\n"
        ))
        .expect("parses");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].name(), "agent");
        assert_eq!(keys[0].key()[0], 0x01);
        assert_eq!(
            keys[0].scope(),
            &[WriteScope::RoutesPeers, WriteScope::ChannelsLand]
        );
        assert_eq!(keys[0].expires(), Some(1_893_456_000));
    }

    #[test]
    fn an_expiry_in_the_past_still_resolves() {
        let keys = parse(&format!(
            "[a]\nkey = \"{KEY}\"\nscope = [\"channels-land\"]\nexpires = \"2001-01-01T00:00:00Z\"\n"
        ))
        .expect("an expired entry is not a refusal to start");
        assert!(keys[0].expires().is_some());
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let result = parse(&format!(
            "[a]\nkey = \"{KEY}\"\nscope = [\"channels-land\"]\nscop = []\n"
        ));
        assert!(matches!(
            result,
            Err(ConfigError::ControllerKeysFileInvalid { .. })
        ));
    }

    #[test]
    fn an_empty_scope_is_refused_naming_the_entry() {
        let result = parse(&format!("[a]\nkey = \"{KEY}\"\nscope = []\n"));
        match result {
            Err(ConfigError::ControllerKeyInvalid { entry, reason, .. }) => {
                assert_eq!(entry, "a");
                assert!(reason.contains("empty"));
            }
            other => panic!("expected ControllerKeyInvalid, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_scope_name_is_refused_naming_the_entry() {
        let result = parse(&format!(
            "[a]\nkey = \"{KEY}\"\nscope = [\"channels-burn\"]\n"
        ));
        match result {
            Err(ConfigError::ControllerKeyInvalid { entry, reason, .. }) => {
                assert_eq!(entry, "a");
                assert!(reason.contains("channels-burn"));
            }
            other => panic!("expected ControllerKeyInvalid, got {other:?}"),
        }
    }

    #[test]
    fn a_key_that_is_not_64_hex_is_refused() {
        let result = parse("[a]\nkey = \"abcd\"\nscope = [\"channels-land\"]\n");
        assert!(matches!(
            result,
            Err(ConfigError::ControllerKeyInvalid { .. })
        ));
    }

    #[test]
    fn the_same_key_twice_in_the_file_is_refused() {
        let result = parse(&format!(
            "[a]\nkey = \"{KEY}\"\nscope = [\"channels-land\"]\n[b]\nkey = \"{KEY}\"\nscope = [\"channels-land\"]\n"
        ));
        assert!(
            matches!(result, Err(ConfigError::ControllerKeyDuplicated { entry, first, .. }) if entry == "b" && first == "a")
        );
    }

    #[test]
    fn a_key_also_on_the_write_keys_is_refused() {
        let key = crate::operator::hex_decode_32(KEY).unwrap();
        let result = parse_controller_keys(
            &format!("[a]\nkey = \"{KEY}\"\nscope = [\"channels-land\"]\n"),
            Path::new("controllers.toml"),
            &[key],
        );
        assert!(
            matches!(result, Err(ConfigError::ControllerKeyAlsoWriteKey { entry, .. }) if entry == "a")
        );
    }

    #[test]
    fn packets_without_a_spending_limit_is_refused() {
        let result = parse(&format!("[a]\nkey = \"{KEY}\"\nscope = [\"packets\"]\n"));
        assert!(matches!(
            result,
            Err(ConfigError::ControllerLimitRequired {
                write: WriteScope::Packets,
                limit: "spending_limit",
                ..
            })
        ));
    }

    #[test]
    fn peers_open_and_fund_without_a_deposit_limit_are_refused() {
        for write in ["peers", "channels-open", "channels-fund"] {
            let result = parse(&format!("[a]\nkey = \"{KEY}\"\nscope = [\"{write}\"]\n"));
            assert!(
                matches!(
                    result,
                    Err(ConfigError::ControllerLimitRequired {
                        limit: "deposit_limit",
                        ..
                    })
                ),
                "{write}: {result:?}"
            );
        }
    }

    #[test]
    fn a_limit_stating_one_figure_is_refused() {
        let result = parse(&format!(
            "[a]\nkey = \"{KEY}\"\nscope = [\"packets\"]\nspending_limit = {{ per_day = 5 }}\n"
        ));
        assert!(matches!(
            result,
            Err(ConfigError::ControllerLimitIncomplete {
                limit: "spending_limit",
                ..
            })
        ));
    }

    #[test]
    fn a_stated_limit_is_refused_because_limits_are_not_yet_enforced() {
        let result = parse(&format!(
            "[a]\nkey = \"{KEY}\"\nscope = [\"packets\"]\nspending_limit = {{ per_write = 1, per_day = 5 }}\n"
        ));
        assert!(
            matches!(result, Err(ConfigError::ControllerLimitsNotEnforced { entry, .. }) if entry == "a")
        );
    }

    #[test]
    fn two_entries_resolve_in_name_order() {
        let keys = parse(&format!(
            "[b]\nkey = \"{OTHER}\"\nscope = [\"channels-withdraw\"]\n[a]\nkey = \"{KEY}\"\nscope = [\"channels-land\"]\n"
        ))
        .expect("parses");
        assert_eq!(
            keys.iter().map(|k| k.name()).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }
}
