//! Poseidon with the kimchi parameters over the Pallas base field, as o1js's `Poseidon`.

use crate::tags::{prefix_to_field, PREFIX_LEN};
use crate::Field;
use mina_poseidon::{
    constants::PlonkSpongeConstantsKimchi,
    pasta::{fp_kimchi, FULL_ROUNDS},
    poseidon::{ArithmeticSponge, Sponge},
};

type KimchiSponge = ArithmeticSponge<Field, PlonkSpongeConstantsKimchi, FULL_ROUNDS>;

fn sponge() -> KimchiSponge {
    KimchiSponge::new(fp_kimchi::static_params())
}

/// `Poseidon.hash`: absorb the inputs, squeeze one field element. No domain.
#[must_use]
pub fn hash(input: &[Field]) -> Field {
    let mut s = sponge();
    s.absorb(input);
    s.squeeze()
}

/// `Poseidon.hashWithPrefix`: the prefix is absorbed and squeezed first, as `mina-hasher`
/// initialises a domain, and the inputs are absorbed after it.
///
/// The prefix is 20 bytes, never fewer: see [`crate::tags`].
#[must_use]
pub fn hash_with_prefix(prefix: &[u8; PREFIX_LEN], input: &[Field]) -> Field {
    let mut s = sponge();
    s.absorb(&[prefix_to_field(prefix)]);
    s.squeeze();
    s.absorb(input);
    s.squeeze()
}
