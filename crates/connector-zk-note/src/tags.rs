//! The domain tags and generator seeds. **Wire law.**
//!
//! Every value in this module is normative and is listed here and nowhere else. The vault's
//! proof program, which lives outside this repository, has to hardcode the same strings, and
//! the wallet and the challenger have to agree with them. Changing one is a breaking change
//! to every proof ever made against the old value.
//!
//! A tag is exactly 20 bytes: `toon-zk-note-`, a name, and `*` padding. The length is not a
//! style choice. o1js zero-pads a shorter prefix (`prefixToField`) while Rust's `mina-hasher`
//! pads it with `*`, so a short prefix hashes to different values on the two sides (spike
//! #1512, convention 1). [`Tag`] can only be built at its full length, and at compile time
//! for a `const`, so a short tag fails the build rather than the vault.

use crate::Field;
use o1_utils::FieldHelpers;

/// The length every domain prefix is written out to.
pub const PREFIX_LEN: usize = 20;

/// What every tag starts with.
pub const TAG_PREFIX: &str = "toon-zk-note-";

/// A domain tag: 20 bytes, `toon-zk-note-` first, `*` padding after the name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tag([u8; PREFIX_LEN]);

impl Tag {
    /// Build a tag. Panics (at compile time in a `const`) unless `s` is exactly 20 bytes,
    /// starts `toon-zk-note-`, is followed by a non-empty name, and any `*` padding is
    /// trailing only.
    #[must_use]
    pub const fn new(s: &str) -> Self {
        let b = s.as_bytes();
        let p = TAG_PREFIX.as_bytes();
        assert!(b.len() == PREFIX_LEN, "a tag is exactly 20 bytes");
        let mut out = [0u8; PREFIX_LEN];
        let mut i = 0;
        while i < PREFIX_LEN {
            if i < p.len() {
                assert!(b[i] == p[i], "a tag starts `toon-zk-note-`");
            }
            out[i] = b[i];
            i += 1;
        }
        assert!(b[p.len()] != b'*', "a tag has a name after the prefix");
        // `*` may only pad on the right.
        let mut seen_star = false;
        let mut j = p.len();
        while j < PREFIX_LEN {
            if b[j] == b'*' {
                seen_star = true;
            } else {
                assert!(!seen_star, "a tag is padded with `*` on the right only");
            }
            j += 1;
        }
        Self(out)
    }

    /// The 20 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; PREFIX_LEN] {
        &self.0
    }

    /// The prefix as the field element Poseidon absorbs first.
    #[must_use]
    pub fn to_field(&self) -> Field {
        prefix_to_field(&self.0)
    }
}

/// A 20-byte prefix as a field element: the bytes little-endian, zero-extended.
#[must_use]
pub fn prefix_to_field(prefix: &[u8; PREFIX_LEN]) -> Field {
    let mut bytes = prefix.to_vec();
    bytes.resize(Field::size_in_bytes(), 0);
    Field::from_bytes(&bytes).expect("20 bytes are far below the field modulus")
}

/// Tags the commitment of a note.
pub const TAG_COMMIT: Tag = Tag::new("toon-zk-note-commit*");
/// Tags a leaf of the vault's tree.
pub const TAG_LEAF: Tag = Tag::new("toon-zk-note-leaf***");
/// Tags a nullifier.
pub const TAG_NULLIFIER: Tag = Tag::new("toon-zk-note-nullify");
/// Tags the authorization binding `Poseidon(TAG_AUTH, N, ctx)`.
pub const TAG_AUTH: Tag = Tag::new("toon-zk-note-auth***");
/// Tags the context `Poseidon(TAG_CTX, zk edge identity, session key, b, expiry)`.
pub const TAG_CTX: Tag = Tag::new("toon-zk-note-ctx****");
/// Tags the successor anchor `Poseidon(TAG_ANCHOR, N)`.
pub const TAG_ANCHOR: Tag = Tag::new("toon-zk-note-anchor*");

/// The seed of the value generator `G_v`. The generator is `hashToGroup([seed_to_field(SEED_VALUE)])`.
/// The balance commitment's other generator is the curve generator, which needs no seed.
pub const SEED_VALUE: &str = "toon-zk-note-gen-value";

/// A seed (at most 31 bytes) as a field element: the bytes little-endian.
///
/// # Panics
/// If the seed is longer than 31 bytes.
#[must_use]
pub fn seed_to_field(seed: &str) -> Field {
    assert!(seed.len() <= 31, "a seed fits in one field element");
    let mut bytes = seed.as_bytes().to_vec();
    bytes.resize(Field::size_in_bytes(), 0);
    Field::from_bytes(&bytes).expect("31 bytes are below the field modulus")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tag_is_twenty_bytes_and_distinct() {
        let tags = [
            TAG_COMMIT,
            TAG_LEAF,
            TAG_NULLIFIER,
            TAG_AUTH,
            TAG_CTX,
            TAG_ANCHOR,
        ];
        for (i, t) in tags.iter().enumerate() {
            assert_eq!(t.as_bytes().len(), 20);
            assert!(t.as_bytes().starts_with(TAG_PREFIX.as_bytes()));
            assert!(tags[i + 1..].iter().all(|u| u != t));
        }
        assert_eq!(TAG_AUTH.as_bytes(), b"toon-zk-note-auth***");
        assert_eq!(TAG_CTX.as_bytes(), b"toon-zk-note-ctx****");
        assert_eq!(TAG_ANCHOR.as_bytes(), b"toon-zk-note-anchor*");
    }

    #[test]
    #[should_panic(expected = "exactly 20 bytes")]
    fn a_short_tag_cannot_be_built() {
        let _ = Tag::new("toon-zk-note-auth");
    }

    #[test]
    #[should_panic(expected = "starts `toon-zk-note-`")]
    fn a_foreign_tag_cannot_be_built() {
        let _ = Tag::new("toon-zk-note********");
    }
}
