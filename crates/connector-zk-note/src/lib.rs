//! The connector's zk-note boundary (ADR 0080 decision 12).
//!
//! Every zk-note derivation -- the domain tags, the leaf, the anchors, the nullifier, the
//! authorization tag, the balance commitment and the state message -- is defined by
//! toon-protocol/zk_toon's `zk-note` crate, re-exported here as [`note`] and pinned by git commit
//! in this crate's manifest. Nothing in this crate defines one. Its `test-vectors/zk-note.json`
//! is the parity contract, replayed by `tests/zk_toon_parity.rs`.
//!
//! What stays here is what the connector owns:
//!
//! - [`session`]: the allowance claim's and the close request's signed fields, and the
//!   session-key signature over them.
//! - [`successor`]: the successor state, composed from zk_toon's charge, anchor and state
//!   message. The state key that signs it is `connector-signer`'s, never read or held here.
//! - [`tiers`]: the operator's published tiers, validated against zk_toon's.
//! - [`request`]: the request proof's public inputs, until zk_toon#29 adds a Rust
//!   `RequestPublic`.
//! - [`evidence`]: the evidence file's bytes, until zk_toon#29's `zk-evidence` crate exists.
//!
//! Pure: no I/O, no clock, no dependency on any connector crate.

pub use zk_note as note;

pub mod evidence;
pub mod request;
pub mod session;
pub mod successor;
pub mod tiers;

/// The Mina keys and signatures the zk edge reads and signs with.
pub use mina_signer::{CompressedPubKey, Keypair, NetworkId, PubKey, SecKey, Signature};
/// The balance commitment: a point on Pallas, in affine form.
pub use zk_note::balance::{Commitment, Scalar};
/// The Pallas base field, the field every value in a zk note lives in.
pub use zk_note::Field;

/// A field element as 32 bytes, little-endian: the encoding zk_toon#29 fixes for the evidence
/// file, and the one the journal stores.
#[must_use]
pub fn field_to_le_bytes(field: Field) -> [u8; 32] {
    use ark_ff::{BigInteger, PrimeField};
    let bytes = field.into_bigint().to_bytes_le();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

/// The field element 32 little-endian bytes encode, or `None` when they are not its canonical
/// encoding (at or above the modulus).
#[must_use]
pub fn field_from_le_bytes(bytes: &[u8; 32]) -> Option<Field> {
    use ark_ff::PrimeField;
    let field = Field::from_le_bytes_mod_order(bytes);
    (field_to_le_bytes(field) == *bytes).then_some(field)
}

/// A scalar (a signature's `s`) as 32 bytes, little-endian: the journal's encoding.
#[must_use]
pub fn scalar_to_le_bytes(scalar: Scalar) -> [u8; 32] {
    use ark_ff::{BigInteger, PrimeField};
    let bytes = scalar.into_bigint().to_bytes_le();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

/// A scalar as a decimal string: `zk_note::field_to_string`'s encoding, for a signature's `s`.
#[must_use]
pub fn scalar_to_string(scalar: Scalar) -> String {
    use ark_ff::PrimeField;
    scalar.into_bigint().to_string()
}

/// The scalar 32 little-endian bytes encode, or `None` when they are not its canonical encoding.
#[must_use]
pub fn scalar_from_le_bytes(bytes: &[u8; 32]) -> Option<Scalar> {
    use ark_ff::PrimeField;
    let scalar = Scalar::from_le_bytes_mod_order(bytes);
    (scalar_to_le_bytes(scalar) == *bytes).then_some(scalar)
}

/// The commitment two coordinates (32 bytes little-endian each) name, or `None` when they are
/// not canonical or not a point on Pallas.
#[must_use]
pub fn commitment_from_le_bytes(x: &[u8; 32], y: &[u8; 32]) -> Option<Commitment> {
    let point = Commitment::new_unchecked(field_from_le_bytes(x)?, field_from_le_bytes(y)?);
    point.is_on_curve().then_some(point)
}

/// The full public key a compressed one names, or `None` when its x is not on Pallas.
#[must_use]
pub fn decompress(key: &CompressedPubKey) -> Option<PubKey> {
    PubKey::from_address(&key.into_address()).ok()
}
