//! The balance commitment: a Pedersen commitment `commit(B, r) = B·G_v + r·G`.
//!
//! `G` is the curve generator. `G_v`, the value generator, is `hashToGroup` of a fixed
//! published seed ([`crate::tags::SEED_VALUE`]), so nobody knows its discrete log relative to
//! `G`. The commitment is homomorphic, which is what lets a zk edge charge Δ without
//! learning the balance: `commit(B, r) − Δ·G_v = commit(B − Δ, r)`.

use crate::group_map::hash_to_group;
use crate::tags::{seed_to_field, SEED_VALUE};
use crate::{Point, Scalar};
use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::PrimeField;

/// The value generator `G_v`.
pub fn value_generator() -> Point {
    hash_to_group(&[seed_to_field(SEED_VALUE)])
}

/// The blinding generator: the curve generator `G`.
pub fn blinding_generator() -> Point {
    Point::generator()
}

/// `commit(B, r) = B·G_v + r·G`.
pub fn commit(balance: u64, blinding: Scalar) -> Point {
    (value_generator().mul_bigint(Scalar::from(balance).into_bigint())
        + blinding_generator().mul_bigint(blinding.into_bigint()))
    .into_affine()
}

/// `C − Δ·G_v`: the commitment to the balance less `delta`, at the same blinding.
pub fn charge(commitment: Point, delta: u64) -> Point {
    (commitment.into_group() - value_generator().mul_bigint(Scalar::from(delta).into_bigint()))
        .into_affine()
}
