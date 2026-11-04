use ark_ec::{short_weierstrass::Projective, AffineRepr, CurveGroup};
use ark_ff::{BigInteger256, FftField, Field, PrimeField};
use kimchi::curve::KimchiCurve;
use mina_curves::pasta::{
    Fp, Fq, PallasParameters, ProjectivePallas, ProjectiveVesta, VestaParameters,
};
use mina_poseidon::{constants::PlonkSpongeConstantsKimchi, sponge::DefaultFqSponge};

use crate::poseidon::SpongeParamsForField;

use crate::ledger::proofs;

use super::{
    public_input::plonk_checks::{self, ShiftedValue},
    to_field_elements::ToFieldElements,
    transaction::Check,
    witness::Witness,
    BACKEND_TICK_ROUNDS_N, BACKEND_TOCK_ROUNDS_N,
};

pub type GroupAffine<F> = ark_ec::short_weierstrass::Affine<<F as FieldWitness>::Parameters>;

/// All the generics we need during witness generation
pub trait FieldWitness
where
    Self: Field
        + Send
        + Sync
        + Into<BigInteger256>
        + From<BigInteger256>
        + Into<crate::p2p_messages::bigint::BigInt>
        + From<i64>
        + From<i32>
        + ToFieldElements<Self>
        + Check<Self>
        + FromFpFq
        + PrimeField
        + FftField
        + SpongeParamsForField<Self>
        + std::fmt::Debug
        + 'static,
{
    type Scalar: FieldWitness<Scalar = Self>;
    type Affine: AffineRepr<
            Group = Self::Projective,
            BaseField = Self,
            ScalarField = <Self as proofs::field::FieldWitness>::Scalar,
        > + Into<GroupAffine<Self>>
        + KimchiCurve<{ crate::ledger::proofs::FULL_ROUNDS }>
        + std::fmt::Debug;
    type Projective: CurveGroup<
            Affine = Self::Affine,
            BaseField = Self,
            ScalarField = <Self as proofs::field::FieldWitness>::Scalar,
        > + From<Projective<Self::Parameters>>
        + std::fmt::Debug;
    type Parameters: ark_ec::short_weierstrass::SWCurveConfig<
            BaseField = Self,
            ScalarField = <Self as proofs::field::FieldWitness>::Scalar,
        > + Clone
        + std::fmt::Debug;
    type Shifting: plonk_checks::ShiftingValue<Self> + Clone + std::fmt::Debug;
    type OtherCurve: KimchiCurve<
        { crate::ledger::proofs::FULL_ROUNDS },
        ScalarField = Self,
        BaseField = <Self as proofs::field::FieldWitness>::Scalar,
    >;
    type FqSponge: Clone
        + mina_poseidon::FqSponge<
            <Self as proofs::field::FieldWitness>::Scalar,
            Self::OtherCurve,
            Self,
            { crate::ledger::proofs::FULL_ROUNDS },
        >;

    const NROUNDS: usize;
    const SRS_DEPTH: usize;
}

impl FieldWitness for Fp {
    type Scalar = Fq;
    type Parameters = PallasParameters;
    type Affine = GroupAffine<Self>;
    type Projective = ProjectivePallas;
    type Shifting = ShiftedValue<Fp>;
    type OtherCurve = GroupAffine<Fq>;
    type FqSponge = DefaultFqSponge<
        VestaParameters,
        PlonkSpongeConstantsKimchi,
        { crate::ledger::proofs::FULL_ROUNDS },
    >;

    const NROUNDS: usize = BACKEND_TICK_ROUNDS_N;
    const SRS_DEPTH: usize = 32768;
}

impl FieldWitness for Fq {
    type Scalar = Fp;
    type Parameters = VestaParameters;
    type Affine = GroupAffine<Self>;
    type Projective = ProjectiveVesta;
    type Shifting = ShiftedValue<Fq>;
    type OtherCurve = GroupAffine<Fp>;
    type FqSponge = DefaultFqSponge<
        PallasParameters,
        PlonkSpongeConstantsKimchi,
        { crate::ledger::proofs::FULL_ROUNDS },
    >;

    const NROUNDS: usize = BACKEND_TOCK_ROUNDS_N;
    const SRS_DEPTH: usize = 65536;
}

/// Trait helping converting generics into concrete types
pub trait FromFpFq {
    fn from_fp(fp: Fp) -> Self;
    fn from_fq(fp: Fq) -> Self;
}

impl FromFpFq for Fp {
    fn from_fp(fp: Fp) -> Self {
        fp
    }
    fn from_fq(_fq: Fq) -> Self {
        // `Fq` cannot be converted into `Fp`.
        // Caller must first split `Fq` into 2 parts (high & low bits)
        // See `impl<F: FieldWitness> ToFieldElements<F> for Fq`
        panic!("Attempt to convert a `Fq` into `Fp`")
    }
}

impl FromFpFq for Fq {
    fn from_fp(fp: Fp) -> Self {
        // `Fp` is smaller than `Fq`, so the conversion is fine
        let bigint: BigInteger256 = fp.into();
        Self::from(bigint)
    }
    fn from_fq(fq: Fq) -> Self {
        fq
    }
}

/// Trait helping converting concrete types into generics
pub trait IntoGeneric<F: FieldWitness> {
    fn into_gen(self) -> F;
}

impl<F: FieldWitness> IntoGeneric<F> for Fp {
    fn into_gen(self) -> F {
        F::from_fp(self)
    }
}

impl<F: FieldWitness> IntoGeneric<F> for Fq {
    fn into_gen(self) -> F {
        F::from_fq(self)
    }
}

#[allow(clippy::module_inception)]
pub mod field {

    use super::*;

    // <https://github.com/o1-labs/snarky/blob/7edf13628872081fd7cad154de257dad8b9ba621/src/base/utils.ml#L99>
    pub fn square<F>(field: F, w: &mut Witness<F>) -> F
    where
        F: FieldWitness,
    {
        w.exists(field.square())
        // TODO: Rest of the function doesn't modify witness
    }

    pub fn mul<F>(x: F, y: F, w: &mut Witness<F>) -> F
    where
        F: FieldWitness,
    {
        w.exists(x * y)
    }

    // TODO: Do we need `div` above ?
    pub fn div_by_inv<F>(x: F, y: F, w: &mut Witness<F>) -> F
    where
        F: FieldWitness,
    {
        let y_inv = w.exists(y.inverse().unwrap_or_else(F::zero));
        mul(x, y_inv, w)
    }

    pub fn equal<F: FieldWitness>(x: F, y: F, w: &mut Witness<F>) -> Boolean {
        let z = x - y;

        let (boolean, r, inv) = if x == y {
            (Boolean::True, F::one(), F::zero())
        } else {
            (Boolean::False, F::zero(), z.inverse().unwrap())
        };
        w.exists([r, inv]);

        boolean
    }
}

pub trait ToBoolean {
    fn to_boolean(&self) -> Boolean;
}

impl ToBoolean for bool {
    fn to_boolean(&self) -> Boolean {
        if *self {
            Boolean::True
        } else {
            Boolean::False
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boolean {
    True = 1,
    False = 0,
}

impl Boolean {
    pub fn as_bool(&self) -> bool {
        match self {
            Boolean::True => true,
            Boolean::False => false,
        }
    }

    pub fn neg(&self) -> Self {
        match self {
            Boolean::False => Boolean::True,
            Boolean::True => Boolean::False,
        }
    }

    pub fn to_field<F: FieldWitness>(self) -> F {
        F::from(self.as_bool())
    }

    fn mul<F: FieldWitness>(&self, other: &Self, w: &mut Witness<F>) -> Self {
        let result: F = self.to_field::<F>() * other.to_field::<F>();
        w.exists(result);
        if result.is_zero() {
            Self::False
        } else {
            assert_eq!(result, F::one());
            Self::True
        }
    }

    pub fn and<F: FieldWitness>(&self, other: &Self, w: &mut Witness<F>) -> Self {
        self.mul(other, w)
    }

    pub fn or<F: FieldWitness>(&self, other: &Self, w: &mut Witness<F>) -> Self {
        let both_false = self.neg().and(&other.neg(), w);
        both_false.neg()
    }

    // For non-constant

    pub fn any<F: FieldWitness>(x: &[Self], w: &mut Witness<F>) -> Self {
        match x {
            [] => Self::False,
            [b1] => *b1,
            [b1, b2] => b1.or(b2, w),
            bs => {
                let sum = bs.iter().fold(0u64, |acc, b| {
                    acc + match b {
                        Boolean::True => 1,
                        Boolean::False => 0,
                    }
                });
                field::equal(F::from(sum), F::zero(), w).neg()
            }
        }
    }

    // Part of utils.inv
}

impl<F: FieldWitness> Check<F> for Boolean {
    fn check(&self, _w: &mut Witness<F>) {
        // Does not modify the witness
    }
}
