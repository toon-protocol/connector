use ark_ff::BigInteger256;
use kimchi::proof::{PointEvaluations, ProofEvaluations};
use mina_curves::pasta::{Fp, Fq, Vesta};
use poly_commitment::{ipa::OpeningProof, PolyComm};

use crate::ledger::{
    lazy::LazyValue,
    proofs::{
        field::{field, Boolean, FieldWitness, ToBoolean},
        public_input::plonk_checks::{ft_eval0, ShiftingValue},
        transaction::InnerCurve,
        util::{challenge_polynomial, proof_evaluation_to_list},
    },
};

use super::{
    field::GroupAffine,
    public_input::plonk_checks::{PlonkMinimal, ScalarsEnv, ShiftedValue},
    step::FeatureFlags,
    to_field_elements::ToFieldElements,
    transaction::Check,
    witness::Witness,
};

pub struct CombinedInnerProductParams<
    'a,
    F: FieldWitness,
    const NROUNDS: usize,
    const NLIMB: usize = 2,
> {
    pub env: &'a ScalarsEnv<F>,
    pub evals: &'a ProofEvaluations<PointEvaluations<Vec<F>>>,
    pub combined_evals: &'a ProofEvaluations<PointEvaluations<F>>,
    pub public: &'a PointEvaluations<Vec<F>>,
    pub minimal: &'a PlonkMinimal<F, NLIMB>,
    pub ft_eval1: F,
    pub r: F,
    pub old_bulletproof_challenges: &'a [[F; NROUNDS]],
    pub xi: F,
    pub zetaw: F,
}

/// <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/pickles/wrap.ml#L37>
pub fn combined_inner_product<F: FieldWitness, const NROUNDS: usize, const NLIMB: usize>(
    params: CombinedInnerProductParams<F, NROUNDS, NLIMB>,
) -> F {
    let CombinedInnerProductParams {
        env,
        old_bulletproof_challenges,
        evals,
        combined_evals,
        minimal,
        r,
        xi,
        zetaw,
        public,
        ft_eval1,
    } = params;

    let ft_eval0 = ft_eval0::<F, NLIMB>(env, combined_evals, minimal, &public.zeta);

    let challenge_polys: Vec<_> = old_bulletproof_challenges
        .iter()
        .map(|v| challenge_polynomial(v))
        .collect();

    let a = proof_evaluation_to_list(evals);

    enum WhichEval {
        First,
        Second,
    }

    let combine = |which_eval: WhichEval, ft: F, pt: F| {
        let f = |PointEvaluations { zeta, zeta_omega }: &PointEvaluations<Vec<F>>| match which_eval
        {
            WhichEval::First => zeta.clone(),
            WhichEval::Second => zeta_omega.clone(),
        };

        challenge_polys
            .iter()
            .map(|f| f(pt))
            .chain(f(public))
            .chain([ft])
            .chain(a.iter().flat_map(|a| f(a)))
            .rev()
            .reduce(|acc, fx| fx + (xi * acc))
            .unwrap()
    };

    combine(WhichEval::First, ft_eval0, minimal.zeta)
        + (r * combine(WhichEval::Second, ft_eval1, zetaw))
}

/// Defined in `plonk_checks.ml`
/// Note: there are other `actual_evaluation`, but they're different
fn actual_evaluation<F: FieldWitness>(pt: F, e: &[F], rounds: usize) -> F {
    let Some((e, es)) = e.split_last() else {
        return F::zero();
    };

    let pt_n = (0..rounds).fold(pt, |acc, _| acc * acc);
    es.iter().rfold(*e, |acc, fx| *fx + (pt_n * acc))
}

pub fn evals_of_split_evals<F: FieldWitness, S: AsRef<[F]>>(
    point_zeta: F,
    point_zetaw: F,
    es: &ProofEvaluations<PointEvaluations<S>>,
    rounds: usize,
) -> ProofEvaluations<PointEvaluations<F>> {
    es.map_ref(&|PointEvaluations { zeta, zeta_omega }| {
        let zeta = actual_evaluation(point_zeta, zeta.as_ref(), rounds);
        let zeta_omega = actual_evaluation(point_zetaw, zeta_omega.as_ref(), rounds);
        PointEvaluations { zeta, zeta_omega }
    })
}

/// Value of `Common.Max_degree.step_log2`
pub const COMMON_MAX_DEGREE_STEP_LOG2: u64 = 16;

impl Check<Fq> for ShiftedValue<Fp> {
    fn check(&self, w: &mut Witness<Fq>) {
        // TODO: Compute those values instead of hardcoded
        const FORBIDDEN_SHIFTED_VALUES: &[Fq; 2] = &[
            ark_ff::MontFp!("91120631062839412180561524743370440705"),
            ark_ff::MontFp!("91120631062839412180561524743370440706"),
        ];

        let bools = FORBIDDEN_SHIFTED_VALUES.map(|forbidden| {
            let shifted: Fq = {
                let ShiftedValue { shifted } = self.clone();
                let f: BigInteger256 = shifted.into();
                f.into() // Never fail, `Fq` is larger than `Fp`
            };
            field::equal(shifted, forbidden, w)
        });
        Boolean::any(&bools, w);
    }
}

impl Check<Fp> for ShiftedValue<Fq> {
    fn check(&self, w: &mut Witness<Fp>) {
        // TODO: Compute those values instead of hardcoded
        #[rustfmt::skip]
        const FORBIDDEN_SHIFTED_VALUES: &[(Fp, Boolean); 4] = &[
            (ark_ff::MontFp!("45560315531506369815346746415080538112"), Boolean::False),
            (ark_ff::MontFp!("45560315531506369815346746415080538113"), Boolean::False),
            (ark_ff::MontFp!("14474011154664524427946373126085988481727088556502330059655218120611762012161"), Boolean::True),
            (ark_ff::MontFp!("14474011154664524427946373126085988481727088556502330059655218120611762012161"), Boolean::True),
        ];

        fn of_bits<F: FieldWitness>(bs: &[bool; 254]) -> F {
            bs.iter().rev().fold(F::zero(), |acc, b| {
                let acc = acc + acc;
                if *b {
                    acc + F::one()
                } else {
                    acc
                }
            })
        }
        // `Fq` is larger than `Fp` so we have to split the field (low & high bits)
        // See:
        // <https://github.com/MinaProtocol/mina/blob/e85cf6969e42060f69d305fb63df9b8d7215d3d7/src/lib/pickles/impls.ml#L94C1-L105C45>

        let to_high_low = |fq: Fq| {
            let [low, high @ ..] = crate::ledger::proofs::transaction::field_to_bits::<Fq, 255>(fq);
            (of_bits::<Fp>(&high), low.to_boolean())
        };

        let bools = FORBIDDEN_SHIFTED_VALUES.map(|(x2, b2)| {
            let (x1, b1) = to_high_low(self.shifted);
            let x_eq = field::equal(x1, x2, w);
            let b_eq = match b2 {
                Boolean::True => b1,
                Boolean::False => b1.neg(),
            };
            x_eq.and(&b_eq, w)
        });
        Boolean::any(&bools, w);
    }
}

impl<F: FieldWitness> Check<F> for ShiftedValue<F> {
    fn check(&self, _w: &mut Witness<F>) {
        // Same field, no check
    }
}

pub mod pseudo {}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Domain {
    Pow2RootsOfUnity(u64),
}

impl Domain {
    pub fn log2_size(&self) -> u64 {
        let Self::Pow2RootsOfUnity(k) = self;
        *k
    }
}

#[derive(Debug)]
pub struct Domains {
    pub h: Domain,
}

impl Domains {}

#[derive(Debug)]
pub struct AllFeatureFlags<F: FieldWitness> {
    pub lookup_tables: LazyValue<Boolean, Witness<F>>,
    pub table_width_at_least_1: LazyValue<Boolean, Witness<F>>,
    pub table_width_at_least_2: LazyValue<Boolean, Witness<F>>,
    pub table_width_3: LazyValue<Boolean, Witness<F>>,
    pub lookups_per_row_3: LazyValue<Boolean, Witness<F>>,
    pub lookups_per_row_4: LazyValue<Boolean, Witness<F>>,
    pub lookup_pattern_xor: LazyValue<Boolean, Witness<F>>,
    pub lookup_pattern_range_check: LazyValue<Boolean, Witness<F>>,
    pub features: FeatureFlags<Boolean>,
}

pub fn expand_feature_flags<F: FieldWitness>(
    features: &FeatureFlags<Boolean>,
) -> AllFeatureFlags<F> {
    let FeatureFlags::<Boolean> {
        range_check0,
        range_check1,
        foreign_field_add: _,
        foreign_field_mul,
        xor,
        rot,
        lookup,
        runtime_tables: _,
    } = features.clone();

    let lookup_pattern_range_check = LazyValue::make(move |w: &mut Witness<F>| {
        let first = range_check0.or(&range_check1, w);
        first.or(&rot, w)
    });

    let lookup_pattern_xor = LazyValue::make(move |_w: &mut Witness<F>| xor);

    let lookup_pattern_xor_clone = lookup_pattern_xor.clone();
    let table_width_3 = LazyValue::make(move |w: &mut Witness<F>| *lookup_pattern_xor_clone.get(w));

    let table_width_3_clone = table_width_3.clone();
    let table_width_at_least_2 = LazyValue::make(move |w: &mut Witness<F>| {
        let table_width_3 = table_width_3_clone.get(w);
        table_width_3.or(&lookup, w)
    });

    let table_width_at_least_2_clone = table_width_at_least_2.clone();
    let lookup_pattern_range_check_clone = lookup_pattern_range_check.clone();
    let table_width_at_least_1 = LazyValue::make(move |w: &mut Witness<F>| {
        let table_width_at_least_2 = *table_width_at_least_2_clone.get(w);
        let lookup_pattern_range_check = *lookup_pattern_range_check_clone.get(w);
        Boolean::any(
            &[
                table_width_at_least_2,
                lookup_pattern_range_check,
                foreign_field_mul,
            ],
            w,
        )
    });

    let lookup_pattern_xor_clone = lookup_pattern_xor.clone();
    let lookup_pattern_range_check_clone = lookup_pattern_range_check.clone();
    let lookups_per_row_4 = LazyValue::make(move |w: &mut Witness<F>| {
        let lookup_pattern_xor = *lookup_pattern_xor_clone.get(w);
        let lookup_pattern_range_check = *lookup_pattern_range_check_clone.get(w);
        Boolean::any(
            &[
                lookup_pattern_xor,
                lookup_pattern_range_check,
                foreign_field_mul,
            ],
            w,
        )
    });

    let lookups_per_row_4_clone = lookups_per_row_4.clone();
    let lookups_per_row_3 = LazyValue::make(move |w: &mut Witness<F>| {
        let lookups_per_row_4 = *lookups_per_row_4_clone.get(w);
        lookups_per_row_4.or(&lookup, w)
    });

    AllFeatureFlags {
        lookup_tables: lookups_per_row_3.clone(),
        table_width_at_least_1,
        table_width_at_least_2,
        table_width_3,
        lookups_per_row_3,
        lookups_per_row_4,
        lookup_pattern_xor,
        lookup_pattern_range_check,
        features: features.clone(),
    }
}

pub mod pcs_batch {}

pub mod wrap_verifier {

    // TODO: Here we pick the verifier index directly from the prover index
    //       but OCaml does it differently

    // TODO: We might have to use F::Scalar here

    // TODO: We might have to use F::Scalar here

    pub mod split_commitments {}
}

pub mod one_hot_vector {}

impl Check<Fq> for OpeningProof<Vesta, { crate::ledger::proofs::FULL_ROUNDS }> {
    fn check(&self, w: &mut Witness<Fq>) {
        let Self {
            lr,
            delta,
            z1,
            z2,
            sg,
        } = self;

        let to_curve = |c: &Vesta| InnerCurve::<Fq>::of_affine(*c);
        let shift = |f: Fp| <Fp as FieldWitness>::Shifting::of_field(f);

        lr.iter().for_each(|(a, b)| {
            to_curve(a).check(w);
            to_curve(b).check(w);
        });
        shift(*z1).check(w);
        shift(*z2).check(w);
        to_curve(delta).check(w);
        to_curve(sg).check(w);
    }
}

impl ToFieldElements<Fq> for OpeningProof<Vesta, { crate::ledger::proofs::FULL_ROUNDS }> {
    fn to_field_elements(&self, fields: &mut Vec<Fq>) {
        let Self {
            lr,
            delta,
            z1,
            z2,
            sg,
        } = self;

        let push = |c: &Vesta, fields: &mut Vec<Fq>| {
            let GroupAffine::<Fq> { x, y, .. } = c;
            x.to_field_elements(fields);
            y.to_field_elements(fields);
        };
        let shift = |f: Fp| <Fp as FieldWitness>::Shifting::of_field(f);

        lr.iter().for_each(|(a, b)| {
            push(a, fields);
            push(b, fields);
        });
        shift(*z1).shifted.to_field_elements(fields);
        shift(*z2).shifted.to_field_elements(fields);
        push(delta, fields);
        push(sg, fields);
    }
}

impl ToFieldElements<Fq> for kimchi::proof::ProverCommitments<Vesta> {
    fn to_field_elements(&self, fields: &mut Vec<Fq>) {
        let Self {
            w_comm,
            z_comm,
            t_comm,
            lookup,
        } = self;

        let mut push_poly = |poly: &PolyComm<Vesta>| {
            let PolyComm { chunks } = poly;
            for GroupAffine::<Fq> { x, y, .. } in chunks {
                x.to_field_elements(fields);
                y.to_field_elements(fields);
            }
        };
        for poly in w_comm.iter().chain([z_comm, t_comm]) {
            push_poly(poly);
        }
        assert!(lookup.is_none());
    }
}

impl Check<Fq> for kimchi::proof::ProverCommitments<Vesta> {
    fn check(&self, w: &mut Witness<Fq>) {
        let Self {
            w_comm,
            z_comm,
            t_comm,
            lookup,
        } = self;
        assert!(lookup.is_none());

        let mut check_poly = |poly: &PolyComm<Vesta>| {
            let PolyComm { chunks } = poly;
            for affine in chunks {
                InnerCurve::of_affine(*affine).check(w);
            }
        };

        for poly in w_comm.iter().chain([z_comm, t_comm]) {
            check_poly(poly);
        }
    }
}
