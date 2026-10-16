//! The derivations a payer, a zk edge and the vault's proof program must agree on.
//!
//! Every one is `Poseidon.hashWithPrefix(TAG, …)` over field elements, in the order written.

use crate::poseidon::hash_with_prefix;
use crate::tags::{TAG_ANCHOR, TAG_AUTH, TAG_COMMIT, TAG_CTX, TAG_LEAF, TAG_NULLIFIER};
use crate::{Field, Point};

fn coords(p: &Point) -> [Field; 2] {
    [p.x, p.y]
}

/// The commitment to a note: `Poseidon(TAG_COMMIT, secret, anchor, C.x, C.y)`, where `C` is
/// the note's balance commitment and `anchor` its current state anchor.
#[must_use]
pub fn note_commitment(secret: Field, anchor: Field, balance: &Point) -> Field {
    let [x, y] = coords(balance);
    hash_with_prefix(TAG_COMMIT.as_bytes(), &[secret, anchor, x, y])
}

/// A leaf of the vault's tree: `Poseidon(TAG_LEAF, note commitment)`.
#[must_use]
pub fn leaf(note_commitment: Field) -> Field {
    hash_with_prefix(TAG_LEAF.as_bytes(), &[note_commitment])
}

/// The nullifier `N` a state is spent under: `Poseidon(TAG_NULLIFIER, secret, anchor)`.
#[must_use]
pub fn nullifier(secret: Field, anchor: Field) -> Field {
    hash_with_prefix(TAG_NULLIFIER.as_bytes(), &[secret, anchor])
}

/// The context an allowance is bound to:
/// `Poseidon(TAG_CTX, zk edge identity, session key, b, expiry)`.
#[must_use]
pub fn context(edge_identity: Field, session_key: Field, budget: u64, expiry: u64) -> Field {
    hash_with_prefix(
        TAG_CTX.as_bytes(),
        &[
            edge_identity,
            session_key,
            Field::from(budget),
            Field::from(expiry),
        ],
    )
}

/// The authorization tag: `Poseidon(TAG_AUTH, N, ctx)`. A proof can't be replayed for another
/// zk edge, session key, budget or expiry because the tag changes with each.
#[must_use]
pub fn binding(nullifier: Field, context: Field) -> Field {
    hash_with_prefix(TAG_AUTH.as_bytes(), &[nullifier, context])
}

/// The successor anchor: `Poseidon(TAG_ANCHOR, N)`.
#[must_use]
pub fn successor_anchor(nullifier: Field) -> Field {
    hash_with_prefix(TAG_ANCHOR.as_bytes(), &[nullifier])
}
