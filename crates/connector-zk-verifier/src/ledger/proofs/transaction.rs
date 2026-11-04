use ark_ec::{short_weierstrass::Projective, AffineRepr, CurveGroup};
use ark_ff::{BigInteger256, Field};
use mina_curves::pasta::{Fp, Fq};
use mina_signer::CompressedPubKey;

use crate::ledger::{
    proofs::{self, unfinalized::AllEvals},
    AppendToInputs,
};

use super::{
    field::{field, FieldWitness, GroupAffine},
    witness::Witness,
};

pub trait Check<F: FieldWitness> {
    fn check(&self, w: &mut Witness<F>);
}

struct FieldBitsIterator {
    index: usize,
    bigint: [u64; 4],
}

impl Iterator for FieldBitsIterator {
    type Item = bool;

    fn next(&mut self) -> Option<Self::Item> {
        let index = self.index;
        self.index += 1;

        let limb_index = index / 64;
        let bit_index = index % 64;

        let limb = self.bigint.get(limb_index)?;
        Some(limb & (1 << bit_index) != 0)
    }
}

pub fn bigint_to_bits<const NBITS: usize>(bigint: BigInteger256) -> [bool; NBITS] {
    let mut bits = FieldBitsIterator {
        index: 0,
        bigint: bigint.0,
    }
    .take(NBITS);
    std::array::from_fn(|_| bits.next().unwrap())
}

pub fn field_to_bits<F, const NBITS: usize>(field: F) -> [bool; NBITS]
where
    F: Field + Into<BigInteger256>,
{
    let bigint: BigInteger256 = field.into();
    bigint_to_bits(bigint)
}

fn bits_msb<F, const NBITS: usize>(field: F) -> [bool; NBITS]
where
    F: Field + Into<BigInteger256>,
{
    let mut bits = field_to_bits::<F, NBITS>(field);
    bits.reverse();
    bits
}

pub fn endos<F>() -> (F, <F as proofs::field::FieldWitness>::Scalar)
where
    F: FieldWitness,
{
    use poly_commitment::ipa::endos;

    // Let's keep them in cache since they're used everywhere
    cache!(
        (F, <F as proofs::field::FieldWitness>::Scalar),
        endos::<GroupAffine<F>>()
    )
}

pub fn make_group<F>(x: F, y: F) -> GroupAffine<F>
where
    F: FieldWitness,
{
    if x == F::ZERO && y == F::ZERO {
        GroupAffine::<F>::zero()
    } else {
        GroupAffine::<F>::new(x, y)
    }
}

pub mod scalar_challenge {

    use super::*;

    // TODO: `scalar` might be a `F::Scalar` here
    // <https://github.com/MinaProtocol/mina/blob/357144819e7ce5f61109d23d33da627be28024c7/src/lib/pickles/scalar_challenge.ml#L12>
    pub fn to_field_checked_prime<F, const NBITS: usize>(scalar: F, w: &mut Witness<F>) -> (F, F, F)
    where
        F: FieldWitness,
    {
        let zero = F::zero();
        let one = F::one();
        let neg_one = one.neg();

        let a_array = [zero, zero, neg_one, one];
        let a_func = |n: u64| a_array[n as usize];

        let b_array = [neg_one, one, zero, zero];
        let b_func = |n: u64| b_array[n as usize];

        let bits_msb: [bool; NBITS] = bits_msb::<_, NBITS>(scalar);

        let nybbles_per_row = 8;
        let bits_per_row = 2 * nybbles_per_row;
        assert_eq!((NBITS % bits_per_row), 0);
        let rows = NBITS / bits_per_row;

        // TODO: Use arrays when const feature allows it
        // <https://github.com/rust-lang/rust/issues/76560>
        let nybbles_by_row: Vec<Vec<u64>> = (0..rows)
            .map(|i| {
                (0..nybbles_per_row)
                    .map(|j| {
                        let bit = (bits_per_row * i) + (2 * j);
                        let b0 = bits_msb[bit + 1] as u64;
                        let b1 = bits_msb[bit] as u64;
                        b0 + (2 * b1)
                    })
                    .collect()
            })
            .collect();

        let two: F = 2u64.into();
        let mut a = two;
        let mut b = two;
        let mut n = F::zero();

        for nybbles_by_row in nybbles_by_row.iter().take(rows) {
            let n0 = n;
            let a0 = a;
            let b0 = b;

            let xs: Vec<F> = (0..nybbles_per_row)
                .map(|j| w.exists(F::from(nybbles_by_row[j])))
                .collect();

            let n8: F = w.exists(xs.iter().fold(n0, |accum, x| accum.double().double() + x));

            let a8: F = w.exists(
                nybbles_by_row
                    .iter()
                    .fold(a0, |accum, x| accum.double() + a_func(*x)),
            );

            let b8: F = w.exists(
                nybbles_by_row
                    .iter()
                    .fold(b0, |accum, x| accum.double() + b_func(*x)),
            );

            n = n8;
            a = a8;
            b = b8;
        }

        (a, b, n)
    }

    // TODO: `scalar` might be a `F::Scalar` here

    // TODO: Use `F::Scalar` instead of `F2`

    // TODO: Remove

    // TODO: Use `F::Scalar` for `chal`
}

pub mod plonk_curve_ops {

    // TODO: `scalar` is a `F::Scalar` here

    // TODO: `scalar` is a `F::Scalar` here
    // <https://github.com/openmina/mina/blob/8f83199a92faa8ff592b7ae5ad5b3236160e8c20/src/lib/pickles/plonk_curve_ops.ml#L140>
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlonkVerificationKeyEvals<F: FieldWitness> {
    pub sigma: [InnerCurve<F>; 7],
    pub coefficients: [InnerCurve<F>; 15],
    pub generic: InnerCurve<F>,
    pub psm: InnerCurve<F>,
    pub complete_add: InnerCurve<F>,
    pub mul: InnerCurve<F>,
    pub emul: InnerCurve<F>,
    pub endomul_scalar: InnerCurve<F>,
}

// Here cvars are not used correctly, but it's just temporary

impl PlonkVerificationKeyEvals<Fp> {}

impl crate::ledger::ToInputs for PlonkVerificationKeyEvals<Fp> {
    fn to_inputs(&self, inputs: &mut crate::poseidon::hash::Inputs) {
        let Self {
            sigma,
            coefficients,
            generic,
            psm,
            complete_add,
            mul,
            emul,
            endomul_scalar,
        } = self;

        let mut to_input = |v: &InnerCurve<Fp>| {
            let GroupAffine::<Fp> { x, y, .. } = v.to_affine();
            inputs.append(&x);
            inputs.append(&y);
        };

        for s in sigma.iter() {
            to_input(s);
        }
        for c in coefficients.iter() {
            to_input(c);
        }
        to_input(generic);
        to_input(psm);
        to_input(complete_add);
        to_input(mul);
        to_input(emul);
        to_input(endomul_scalar);
    }
}

// Implementation for references
impl<F: FieldWitness, T: Check<F>> Check<F> for &T {
    fn check(&self, w: &mut Witness<F>) {
        (*self).check(w)
    }
}

impl<F: FieldWitness, T: Check<F> + Clone> Check<F> for std::borrow::Cow<'_, T> {
    fn check(&self, w: &mut Witness<F>) {
        let this: &T = self.as_ref();
        this.check(w)
    }
}

impl<F: FieldWitness> Check<F> for PlonkVerificationKeyEvals<F> {
    fn check(&self, w: &mut Witness<F>) {
        let Self {
            sigma,
            coefficients,
            generic,
            psm,
            complete_add,
            mul,
            emul,
            endomul_scalar,
        } = self;

        sigma.iter().for_each(|s| s.check(w));
        coefficients.iter().for_each(|c| c.check(w));
        generic.check(w);
        psm.check(w);
        complete_add.check(w);
        mul.check(w);
        emul.check(w);
        endomul_scalar.check(w);
    }
}

impl<F: FieldWitness> Check<F> for bool {
    fn check(&self, _w: &mut Witness<F>) {
        // Does not modify the witness
    }
}

impl<F: FieldWitness> Check<F> for mina_signer::Signature {
    fn check(&self, _w: &mut Witness<F>) {
        // Does not modify the witness
    }
}

impl<F: FieldWitness> Check<F> for Fp {
    fn check(&self, _w: &mut Witness<F>) {
        // Does not modify the witness
    }
}

impl<F: FieldWitness> Check<F> for Fq {
    fn check(&self, _w: &mut Witness<F>) {
        // Does not modify the witness
    }
}

impl<F: FieldWitness, T: Check<F>, const N: usize> Check<F> for [T; N] {
    fn check(&self, w: &mut Witness<F>) {
        self.iter().for_each(|v| v.check(w));
    }
}

impl<F: FieldWitness> Check<F> for u32 {
    fn check(&self, w: &mut Witness<F>) {
        const NBITS: usize = u32::BITS as usize;

        let number: u32 = *self;
        assert_eq!(NBITS, std::mem::size_of_val(&number) * 8);

        let number: F = number.into();
        scalar_challenge::to_field_checked_prime::<F, NBITS>(number, w);
    }
}

/// Rust calls:
/// <https://github.com/openmina/mina/blob/8f83199a92faa8ff592b7ae5ad5b3236160e8c20/src/lib/crypto/kimchi_bindings/stubs/src/projective.rs>
/// Conversion to/from OCaml:
/// <https://github.com/openmina/mina/blob/8f83199a92faa8ff592b7ae5ad5b3236160e8c20/src/lib/crypto/kimchi_bindings/stubs/src/arkworks/group_projective.rs>
/// Typ:
/// <https://github.com/o1-labs/snarky/blob/7edf13628872081fd7cad154de257dad8b9ba621/snarky_curve/snarky_curve.ml#L219-L229>
///
#[derive(
    Clone,
    derive_more::Add,
    derive_more::Sub,
    derive_more::Neg,
    derive_more::Mul,
    derive_more::Div,
    PartialEq,
    Eq,
)]
pub struct InnerCurve<F: FieldWitness> {
    // ProjectivePallas
    // ProjectiveVesta
    inner: F::Projective,
}

impl<F: FieldWitness> std::fmt::Debug for InnerCurve<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // OCaml uses `to_affine_exn` when those are printed using `sexp`
        // <https://github.com/openmina/mina/blob/8f83199a92faa8ff592b7ae5ad5b3236160e8c20/src/lib/snark_params/snark_params.ml#L149>
        let GroupAffine::<F> { x, y, .. } = self.to_affine();
        f.debug_struct("InnerCurve")
            .field("x", &x)
            .field("y", &y)
            .finish()
    }
}

impl crate::ledger::ToInputs for InnerCurve<Fp> {
    fn to_inputs(&self, inputs: &mut crate::poseidon::hash::Inputs) {
        let GroupAffine::<Fp> { x, y, .. } = self.to_affine();
        inputs.append_field(x);
        inputs.append_field(y);
    }
}

impl<F: FieldWitness> From<(F, F)> for InnerCurve<F> {
    fn from((x, y): (F, F)) -> Self {
        Self::of_affine(make_group(x, y))
    }
}

impl<F: FieldWitness> InnerCurve<F> {
    pub fn to_affine(&self) -> GroupAffine<F> {
        // Both `affine` below are the same type, but we use `into()` to make it non-generic
        let affine: F::Affine = self.inner.into_affine();
        let affine: GroupAffine<F> = affine.into();
        // OCaml panics on infinity
        // <https://github.com/MinaProtocol/mina/blob/3e58e92ea9aeddb41ad3b6e494279891c5f9aa09/src/lib/crypto/kimchi_backend/common/curve.ml#L180>
        assert!(!affine.infinity);
        affine
    }

    pub fn of_affine(affine: GroupAffine<F>) -> Self {
        // Both `inner` below are the same type, but we use `into()` to make it generic
        let inner: Projective<F::Parameters> = affine.into_group();
        let inner: F::Projective = inner.into();
        Self { inner }
    }
}

use std::cell::RefCell;

thread_local! {
    static LATEST_RANDOM: RefCell<Box<dyn Fn() -> String>> = RefCell::new(Box::new(String::new));
}

impl InnerCurve<Fp> {
    // TODO: Remove this
}

impl<F: FieldWitness> Check<F> for InnerCurve<F> {
    // <https://github.com/openmina/mina/blob/8f83199a92faa8ff592b7ae5ad5b3236160e8c20/src/lib/snarky_curves/snarky_curves.ml#L167>
    fn check(&self, w: &mut Witness<F>) {
        self.to_affine().check(w);
    }
}

impl<F: FieldWitness> Check<F> for GroupAffine<F> {
    // <https://github.com/openmina/mina/blob/8f83199a92faa8ff592b7ae5ad5b3236160e8c20/src/lib/snarky_curves/snarky_curves.ml#L167>
    fn check(&self, w: &mut Witness<F>) {
        let GroupAffine::<F> { x, .. } = self;
        let x2 = field::square(*x, w);
        let _x3 = field::mul(x2, *x, w);
        // Rest of the function doesn't modify witness
    }
}

impl<F: FieldWitness, T: Check<F>> Check<F> for Vec<T> {
    fn check(&self, w: &mut Witness<F>) {
        self.iter().for_each(|v| v.check(w))
    }
}

impl<F: FieldWitness, T: Check<F>> Check<F> for Box<[T]> {
    fn check(&self, w: &mut Witness<F>) {
        self.iter().for_each(|v| v.check(w))
    }
}

impl<F: FieldWitness, A: Check<F>, B: Check<F>> Check<F> for (A, B) {
    fn check(&self, w: &mut Witness<F>) {
        let (a, b) = self;
        a.check(w);
        b.check(w);
    }
}

impl<F: FieldWitness> Check<F> for CompressedPubKey {
    fn check(&self, _w: &mut Witness<F>) {
        // Does not modify the witness
    }
}

impl<F: FieldWitness> Check<F> for &[AllEvals<F>] {
    fn check(&self, _w: &mut Witness<F>) {
        // Does not modify the witness
    }
}

pub mod legacy_input {

    pub struct BitsIterator<const N: usize> {
        pub index: usize,
        pub number: [u8; N],
    }

    impl<const N: usize> Iterator for BitsIterator<N> {
        type Item = bool;

        fn next(&mut self) -> Option<Self::Item> {
            let index = self.index;
            self.index += 1;

            let limb_index = index / 8;
            let bit_index = index % 8;

            let limb = self.number.get(limb_index)?;
            Some(limb & (1 << bit_index) != 0)
        }
    }
}

pub mod poseidon {}

// Used as the _if method

pub mod transaction_snark {

    mod user_command_failure {}
}

// TODO: Find better name

pub mod debug {

    // Manual implementation because String does not implement binprot traits (because unbounded)
    // Manual implementation because String does not implement binprot traits (because unbounded)
}
