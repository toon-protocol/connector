use std::path::PathBuf;

use serde::Deserialize;

use crate::error::ConfigError;

use thiserror::Error;

/// Every way the secret rules refuse a config. Composed into
/// [`ConfigError`] (issue #1690), so the binary still handles one error and the
/// messages are the ones the operator has always seen.
#[derive(Debug, Error)]
#[allow(clippy::enum_variant_names)]
pub enum SecretRefusal {
    #[error("signer config must set exactly one of 'key_file' or 'kms_key_id', but {reason}")]
    SignerLocationAmbiguous { reason: &'static str },

    #[error("signer key_file does not exist or is not a file: {0}")]
    SignerKeyFileNotFound(PathBuf),

    #[error("signer kms_key_id must not be empty")]
    SignerKmsIdEmpty,
}

/// The `[signer]` section as written in the config file: a location, not a
/// secret. Exactly one of the two fields must be set.
///
/// `deny_unknown_fields` (issue #556): both fields are optional and their
/// absence is meaningful, so a mistyped `key_fle` would otherwise parse as
/// "neither location set" -- an error whose message points at the wrong
/// thing -- and a mistyped `kms_key_i` alongside a real `key_file` would
/// silently disappear. A key written down is honoured or refused, never
/// dropped.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSignerConfig {
    #[serde(default)]
    key_file: Option<PathBuf>,
    #[serde(default)]
    kms_key_id: Option<String>,
}

/// Where the connector's signing key material lives -- never the key itself.
///
/// A [`SecretLocation`] is a pointer: a path to a local key file, or a key
/// management service identifier. Config loading resolves and validates the
/// pointer only; reading the file's bytes or calling out to a KMS is left to
/// whatever later constructs a `Signer` (see the `connector-signer` crate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretLocation {
    /// A local file holding raw key material.
    File(PathBuf),
    /// An identifier a key management service backend resolves.
    Kms { key_id: String },
}

impl SecretLocation {
    pub(crate) fn resolve(raw: RawSignerConfig) -> Result<SecretLocation, ConfigError> {
        match (raw.key_file, raw.kms_key_id) {
            (Some(path), None) => {
                if !path.is_file() {
                    return Err(ConfigError::Secret(SecretRefusal::SignerKeyFileNotFound(
                        path,
                    )));
                }
                Ok(SecretLocation::File(path))
            }
            (None, Some(key_id)) => {
                if key_id.trim().is_empty() {
                    return Err(ConfigError::Secret(SecretRefusal::SignerKmsIdEmpty));
                }
                Ok(SecretLocation::Kms { key_id })
            }
            (None, None) => Err(ConfigError::Secret(
                SecretRefusal::SignerLocationAmbiguous {
                    reason: "neither 'key_file' nor 'kms_key_id' is set",
                },
            )),
            (Some(_), Some(_)) => Err(ConfigError::Secret(
                SecretRefusal::SignerLocationAmbiguous {
                    reason: "both 'key_file' and 'kms_key_id' are set",
                },
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn resolves_a_key_file_that_exists() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(b"secret bytes").expect("write");

        let resolved = SecretLocation::resolve(RawSignerConfig {
            key_file: Some(file.path().to_path_buf()),
            kms_key_id: None,
        })
        .expect("resolve");

        assert_eq!(resolved, SecretLocation::File(file.path().to_path_buf()));
    }

    #[test]
    fn rejects_a_key_file_that_does_not_exist() {
        let result = SecretLocation::resolve(RawSignerConfig {
            key_file: Some(PathBuf::from("/nonexistent/does-not-exist.key")),
            kms_key_id: None,
        });

        assert!(matches!(
            result,
            Err(ConfigError::Secret(SecretRefusal::SignerKeyFileNotFound(_)))
        ));
    }

    #[test]
    fn resolves_a_non_empty_kms_key_id() {
        let resolved = SecretLocation::resolve(RawSignerConfig {
            key_file: None,
            kms_key_id: Some("arn:aws:kms:us-east-1:123:key/abc".to_string()),
        })
        .expect("resolve");

        assert_eq!(
            resolved,
            SecretLocation::Kms {
                key_id: "arn:aws:kms:us-east-1:123:key/abc".to_string()
            }
        );
    }

    #[test]
    fn rejects_an_empty_kms_key_id() {
        let result = SecretLocation::resolve(RawSignerConfig {
            key_file: None,
            kms_key_id: Some("   ".to_string()),
        });

        assert!(matches!(
            result,
            Err(ConfigError::Secret(SecretRefusal::SignerKmsIdEmpty))
        ));
    }

    #[test]
    fn rejects_neither_location_set() {
        let result = SecretLocation::resolve(RawSignerConfig {
            key_file: None,
            kms_key_id: None,
        });

        assert!(matches!(
            result,
            Err(ConfigError::Secret(
                SecretRefusal::SignerLocationAmbiguous { .. }
            ))
        ));
    }

    #[test]
    fn rejects_both_locations_set() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(b"secret bytes").expect("write");

        let result = SecretLocation::resolve(RawSignerConfig {
            key_file: Some(file.path().to_path_buf()),
            kms_key_id: Some("arn:aws:kms:us-east-1:123:key/abc".to_string()),
        });

        assert!(matches!(
            result,
            Err(ConfigError::Secret(
                SecretRefusal::SignerLocationAmbiguous { .. }
            ))
        ));
    }
}
