use crate::p2p_messages::{bigint::InvalidBigInt, v2};
use kimchi::proof::{PointEvaluations, ProofEvaluations};
use mina_curves::pasta::Fq;

use crate::ledger::proofs::field::FieldWitness;

use super::{verification::prev_evals_from_p2p, BACKEND_TOCK_ROUNDS_N};

pub mod ro {

    use crate::ledger::proofs::{
        public_input::scalar_challenge::ScalarChallenge, transaction::legacy_input::BitsIterator,
    };

    pub fn bits_random_oracle<const N: usize>(s: &str) -> [bool; N] {
        use blake2::{
            digest::{Update, VariableOutput},
            Blake2sVar,
        };

        let mut hasher = Blake2sVar::new(32).unwrap();
        hasher.update(s.as_bytes());
        let hash = hasher.finalize_boxed();

        let mut bits = BitsIterator::<32> {
            index: 0,
            number: (&*hash).try_into().unwrap(),
        }
        .take(N);

        std::array::from_fn(|_| bits.next().unwrap())
    }

    fn ro<T, F, const N: usize>(n: usize, label: &str, fun: F) -> T
    where
        F: FnOnce([bool; N]) -> T,
    {
        let s = format!("{}_{}", label, n);
        fun(bits_random_oracle::<N>(&s))
    }

    pub fn chal(n: usize) -> ScalarChallenge {
        fn of_bits(bits: [bool; 128]) -> [u64; 2] {
            let mut limbs = bits.chunks(64).map(|chunk| {
                chunk.iter().enumerate().fold(
                    0u64,
                    |acc, (i, b)| {
                        if *b {
                            acc | (1 << i)
                        } else {
                            acc
                        }
                    },
                )
            });
            std::array::from_fn(|_| limbs.next().unwrap())
        }

        let [a, b] = ro(n, "chal", of_bits);
        ScalarChallenge::new(a, b)
    }
}

#[derive(Clone, Debug)]
pub struct EvalsWithPublicInput<F: FieldWitness> {
    pub evals: ProofEvaluations<PointEvaluations<Vec<F>>>,
    pub public_input: (Vec<F>, Vec<F>),
}

#[derive(Clone, Debug)]
pub struct AllEvals<F: FieldWitness> {
    pub ft_eval1: F,
    pub evals: EvalsWithPublicInput<F>,
}

impl AllEvals<Fq> {}

impl<F: FieldWitness> TryFrom<&v2::PicklesProofProofsVerified2ReprStableV2PrevEvals>
    for AllEvals<F>
{
    type Error = InvalidBigInt;

    fn try_from(
        value: &v2::PicklesProofProofsVerified2ReprStableV2PrevEvals,
    ) -> Result<Self, Self::Error> {
        let v2::PicklesProofProofsVerified2ReprStableV2PrevEvals {
            evals:
                v2::PicklesProofProofsVerified2ReprStableV2PrevEvalsEvals {
                    public_input: (p0, p1),
                    evals,
                },
            ft_eval1,
        } = value;

        Ok(Self {
            ft_eval1: ft_eval1.to_field()?,
            evals: EvalsWithPublicInput {
                evals: prev_evals_from_p2p::<F>(evals)?,
                public_input: (vec![p0.to_field()?], vec![p1.to_field()?]),
            },
        })
    }
}

/// Value of `Dummy.Ipa.Wrap.challenges`
pub fn dummy_ipa_wrap_challenges() -> [[u64; 2]; BACKEND_TOCK_ROUNDS_N] {
    cache_one!([[u64; 2]; BACKEND_TOCK_ROUNDS_N], {
        std::array::from_fn(|i| ro::chal(15 - i).inner)
    })
}
