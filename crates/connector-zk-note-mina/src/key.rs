//! The request program's verification key, which the binary carries (ADR 0080
//! decision 15).
//!
//! The key isn't on chain: the request program is the payer's, not the
//! vault's, and a verifier build serves one Mina era anyway. So the key is
//! zk_toon's published one, compiled in, and a `[zk_note]` table whose
//! `verification_key_hash` names any other refuses boot by name. The file is
//! `verificationKey` from zk_toon's `test-vectors/request-proofs/
//! request-proofs.json` (zk_toon#65), pinned by hash in
//! `tests/fixtures_are_pinned.rs`; the contract suite proves it accepts the
//! proofs o1js made under it.

use std::sync::Arc;

use connector_zk_note::note::field_to_string;
use connector_zk_verifier::{Verifier, VerifierKey};

/// `{"data": <base64 binprot>, "hash": <decimal field>}`: o1js' JSON form of
/// a verification key.
const REQUEST_KEY_JSON: &str = include_str!("../key/request-verification-key.json");

/// Why the node has no verifier for its request proofs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyRefusal {
    /// The table names a key hash this build does not carry.
    #[error(
        "[zk_note] verification_key_hash {configured} is not the request key this build \
         carries ({carried}): this build verifies request proofs under that key only, so \
         use a build that carries the key you mean, or name this one"
    )]
    NotTheCarriedKey { configured: String, carried: String },
    /// The compiled-in key does not hash to the hash it is filed under. A
    /// build defect, caught at boot rather than by a payer's rejected proof.
    #[error("the request key this build carries does not hash to its recorded hash: {0}")]
    Corrupt(String),
}

/// The hash of the request key this build carries, in decimal.
///
/// # Errors
/// [`KeyRefusal::Corrupt`] for a build whose key file is unreadable.
pub fn carried_key_hash() -> Result<String, KeyRefusal> {
    Ok(carried()?.1)
}

fn carried() -> Result<(VerifierKey, String), KeyRefusal> {
    let file: serde_json::Value =
        serde_json::from_str(REQUEST_KEY_JSON).map_err(|e| KeyRefusal::Corrupt(e.to_string()))?;
    let data = file["data"]
        .as_str()
        .ok_or_else(|| KeyRefusal::Corrupt("no 'data'".to_string()))?;
    let recorded = file["hash"]
        .as_str()
        .ok_or_else(|| KeyRefusal::Corrupt("no 'hash'".to_string()))?;
    let key =
        VerifierKey::from_o1js_base64(data).map_err(|e| KeyRefusal::Corrupt(e.to_string()))?;
    let hash = field_to_string(key.hash());
    if hash != recorded {
        return Err(KeyRefusal::Corrupt(format!(
            "the key hashes to {hash}, not {recorded}"
        )));
    }
    Ok((key, hash))
}

/// Check `configured` (the table's `verification_key_hash`) against the key
/// this build carries, then build its verifier. Building the index takes
/// 1.2--1.8 s of CPU, so it runs off the async workers.
///
/// # Errors
/// [`KeyRefusal`], by name, before anything is built when the hash differs.
pub async fn request_verifier(configured: &str) -> Result<Arc<Verifier>, KeyRefusal> {
    let (key, hash) = carried()?;
    if hash != configured {
        return Err(KeyRefusal::NotTheCarriedKey {
            configured: configured.to_string(),
            carried: hash,
        });
    }
    tokio::task::spawn_blocking(move || Arc::new(Verifier::new(key)))
        .await
        .map_err(|e| KeyRefusal::Corrupt(format!("building the verifier failed: {e}")))
}
