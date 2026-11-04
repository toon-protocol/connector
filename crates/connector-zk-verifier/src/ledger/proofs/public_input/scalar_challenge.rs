use std::array::IntoIter;

use ark_ff::{BigInteger256, Field};

use crate::ledger::proofs::{self, field::FieldWitness, transaction::endos};

#[derive(Clone, Debug)]
pub struct ScalarChallenge {
    pub inner: [u64; 2],
}

impl<F: FieldWitness> From<F> for ScalarChallenge {
    fn from(value: F) -> Self {
        let bigint: BigInteger256 = value.into();
        let bigint = bigint.0;
        Self::new(bigint[0], bigint[1])
    }
}

impl From<[u64; 2]> for ScalarChallenge {
    fn from(value: [u64; 2]) -> Self {
        Self::new(value[0], value[1])
    }
}

impl From<Vec<u64>> for ScalarChallenge {
    fn from(value: Vec<u64>) -> Self {
        Self::new(value[0], value[1])
    }
}

struct ScalarChallengeBitsIterator {
    inner: IntoIter<bool, 128>,
}

impl Iterator for ScalarChallengeBitsIterator {
    type Item = (bool, bool);

    fn next(&mut self) -> Option<Self::Item> {
        let second = self.inner.next()?;
        let first = self.inner.next()?;
        Some((first, second))
    }
}

impl ScalarChallenge {
    pub fn new(a: u64, b: u64) -> Self {
        Self { inner: [a, b] }
    }

    fn iter_bits(&self) -> ScalarChallengeBitsIterator {
        let a: u128 = self.inner[0] as u128;
        let b: u128 = self.inner[1] as u128;
        let num: u128 = (a | (b << 64)).reverse_bits();

        let mut bits = [false; 128];
        for (index, bit) in bits.iter_mut().enumerate() {
            *bit = ((num >> index) & 1) != 0;
        }

        ScalarChallengeBitsIterator {
            inner: bits.into_iter(),
        }
    }

    /// Implemention of `to_field_constant`
    /// <https://github.com/MinaProtocol/mina/blob/32a91613c388a71f875581ad72276e762242f802/src/lib/pickles/scalar_challenge.ml#L139>
    pub fn to_field<F>(&self, endo: &F) -> F
    where
        F: Field + From<i32>,
    {
        let mut a: F = 2.into();
        let mut b: F = 2.into();
        let one: F = 1.into();
        let neg_one: F = -one;

        for (first, second) in self.iter_bits() {
            let s = if first { one } else { neg_one };

            a += a;
            b += b;

            if second {
                a += s;
            } else {
                b += s;
            }
        }

        (a * endo) + b
    }

    pub fn array_to_fields<F: FieldWitness, const N: usize>(array: &[F; N]) -> [F; N] {
        let (_, endo) = endos::<<F as proofs::field::FieldWitness>::Scalar>();
        array.each_ref().map(|v| Self::from(*v).to_field(&endo))
    }

    pub fn limbs_to_field<F: FieldWitness>(limbs: &[u64; 2]) -> F {
        let (_, endo) = endos::<<F as proofs::FieldWitness>::Scalar>();
        Self::from(*limbs).to_field(&endo)
    }
}
