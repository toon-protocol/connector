use super::{
    field::{Boolean, FieldWitness},
    public_input::plonk_checks::PlonkMinimal,
    to_field_elements::ToFieldElements,
    unfinalized::AllEvals,
};
use crate::ledger::proofs::{
    public_input::{
        plonk_checks::{derive_plonk, InCircuit, ShiftingValue},
        prepared_statement::{DeferredValues, Plonk},
    },
    transaction::endos,
    util::{challenge_polynomial, four_u64_to_field, proof_evaluation_to_absorption_sequence},
    verification::make_scalars_env,
    wrap::{
        combined_inner_product, evals_of_split_evals, CombinedInnerProductParams,
        COMMON_MAX_DEGREE_STEP_LOG2,
    },
    BACKEND_TICK_ROUNDS_N,
};
use crate::p2p_messages::{bigint::InvalidBigInt, v2};
use ark_ff::BigInteger256;
use ark_poly::{EvaluationDomain, Radix2EvaluationDomain};
use kimchi::proof::PointEvaluations;
use mina_curves::pasta::{Fp, Fq};

#[derive(Clone, Debug)]
pub struct FeatureFlags<Bool> {
    pub range_check0: Bool,
    pub range_check1: Bool,
    pub foreign_field_add: Bool,
    pub foreign_field_mul: Bool,
    pub xor: Bool,
    pub rot: Bool,
    pub lookup: Bool,
    pub runtime_tables: Bool,
}

impl FeatureFlags<Boolean> {}

impl<F: FieldWitness> ToFieldElements<F> for FeatureFlags<bool> {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        let Self {
            range_check0,
            range_check1,
            foreign_field_add,
            foreign_field_mul,
            xor,
            rot,
            lookup,
            runtime_tables,
        } = self;

        [
            range_check0,
            range_check1,
            foreign_field_add,
            foreign_field_mul,
            xor,
            rot,
            lookup,
            runtime_tables,
        ]
        .to_field_elements(fields);
    }
}

impl FeatureFlags<bool> {
    pub fn to_boolean(&self) -> FeatureFlags<Boolean> {
        use super::field::ToBoolean;

        let Self {
            range_check0,
            range_check1,
            foreign_field_add,
            foreign_field_mul,
            xor,
            rot,
            lookup,
            runtime_tables,
        } = self;

        FeatureFlags {
            range_check0: range_check0.to_boolean(),
            range_check1: range_check1.to_boolean(),
            foreign_field_add: foreign_field_add.to_boolean(),
            foreign_field_mul: foreign_field_mul.to_boolean(),
            xor: xor.to_boolean(),
            rot: rot.to_boolean(),
            lookup: lookup.to_boolean(),
            runtime_tables: runtime_tables.to_boolean(),
        }
    }
}

pub struct StatementDeferredValues {
    pub plonk: PlonkMinimal<Fp, 2>,
    pub bulletproof_challenges: [[u64; 2]; 16],
    pub branch_data: v2::CompositionTypesBranchDataStableV1,
}

pub struct StatementProofState {
    pub deferred_values: StatementDeferredValues,
    pub sponge_digest_before_evaluations: [u64; 4],
}

impl TryFrom<&v2::PicklesProofProofsVerified2ReprStableV2StatementProofState>
    for StatementProofState
{
    type Error = InvalidBigInt;

    fn try_from(
        value: &v2::PicklesProofProofsVerified2ReprStableV2StatementProofState,
    ) -> Result<Self, Self::Error> {
        let v2::PicklesProofProofsVerified2ReprStableV2StatementProofState {
            deferred_values:
                v2::PicklesProofProofsVerified2ReprStableV2StatementProofStateDeferredValues {
                    plonk,
                    bulletproof_challenges,
                    branch_data,
                },
            sponge_digest_before_evaluations,
            messages_for_next_wrap_proof: _,
        } = value;

        Ok(Self {
            deferred_values: StatementDeferredValues {
                plonk: plonk.try_into()?,
                bulletproof_challenges: bulletproof_challenges
                    .each_ref()
                    .map(|challenge| challenge.prechallenge.inner.each_ref().map(|v| v.as_u64())),
                branch_data: branch_data.clone(),
            },
            sponge_digest_before_evaluations: sponge_digest_before_evaluations
                .each_ref()
                .map(|v| v.as_u64()),
        })
    }
}

pub mod step_verifier {

    use super::*;

    use kimchi::circuits::wires::PERMUTS;

    pub trait PlonkDomain<F: FieldWitness> {
        fn generator(&self) -> F;
        fn shifts(&self) -> &[F; PERMUTS];
    }

    // TODO: This assumes that feature flags are never enabled (we make this assumption everywhere else too)

    // TODO: Dedup with the one in `wrap_verifier`

    // TODO: Dedup with the one above and in `wrap_verifier`

    // TODO: Dedup with the one in `wrap_verifier`

    // TODO: Dedup with the one in `wrap_verifier`
}

fn to_bytes(f: Fp) -> [u64; 4] {
    let bigint: BigInteger256 = f.into();
    bigint.0
}

fn to_4limbs(v: [u64; 2]) -> [u64; 4] {
    [v[0], v[1], 0, 0]
}

pub struct ExpandDeferredParams<'a> {
    pub evals: &'a AllEvals<Fp>,
    pub old_bulletproof_challenges: &'a Vec<[Fp; 16]>,
    pub proof_state: &'a StatementProofState,
    pub zk_rows: u64,
}

pub fn expand_deferred(params: ExpandDeferredParams) -> anyhow::Result<DeferredValues<Fp>> {
    let ExpandDeferredParams {
        evals,
        old_bulletproof_challenges,
        proof_state,
        zk_rows,
    } = params;

    use super::public_input::scalar_challenge::ScalarChallenge;

    let (_, endo) = endos::<Fq>();

    let plonk0 = &proof_state.deferred_values.plonk;

    let zeta = ScalarChallenge::limbs_to_field(&plonk0.zeta_bytes);
    let alpha = ScalarChallenge::limbs_to_field(&plonk0.alpha_bytes);
    let step_domain: u8 = proof_state.deferred_values.branch_data.domain_log2.as_u8();

    let Some(num_coeffs) = 1u64.checked_shl(step_domain as u32) else {
        return Err(InvalidBigInt)?;
    };

    let Some(domain) = Radix2EvaluationDomain::<Fp>::new(num_coeffs as usize) else {
        return Err(InvalidBigInt)?;
    };

    let zetaw = zeta * domain.group_gen;

    let plonk_minimal = PlonkMinimal::<Fp, 4> {
        alpha,
        beta: plonk0.beta,
        gamma: plonk0.gamma,
        zeta,
        // joint_combiner: plonk0.joint_combiner,
        joint_combiner: plonk0
            .joint_combiner_bytes
            .as_ref()
            .map(ScalarChallenge::limbs_to_field),
        alpha_bytes: to_bytes(alpha),
        beta_bytes: to_4limbs(plonk0.beta_bytes),
        gamma_bytes: to_4limbs(plonk0.gamma_bytes),
        zeta_bytes: to_bytes(zeta),
        joint_combiner_bytes: plonk0.joint_combiner_bytes.map(to_4limbs),
        feature_flags: plonk0.feature_flags.clone(),
    };

    let combined_evals =
        evals_of_split_evals(zeta, zetaw, &evals.evals.evals, BACKEND_TICK_ROUNDS_N);

    let srs_length_log2 = COMMON_MAX_DEGREE_STEP_LOG2;
    let env = make_scalars_env(&plonk_minimal, step_domain, srs_length_log2, zk_rows);

    let plonk = {
        let InCircuit {
            zeta_to_domain_size,
            zeta_to_srs_length,
            perm,
            lookup,
            feature_flags,
        } = derive_plonk(&env, &combined_evals, &plonk_minimal);

        Plonk {
            alpha: plonk0.alpha_bytes,
            beta: plonk0.beta_bytes,
            gamma: plonk0.gamma_bytes,
            zeta: plonk0.zeta_bytes,
            zeta_to_srs_length,
            zeta_to_domain_size,
            perm,
            feature_flags,
            lookup: lookup.map(|_| plonk0.joint_combiner_bytes.unwrap()),
        }
    };

    let xs = proof_evaluation_to_absorption_sequence(&evals.evals.evals);
    let (x1, x2) = &evals.evals.public_input;

    let old_bulletproof_challenges: Vec<_> = old_bulletproof_challenges
        .iter()
        .map(ScalarChallenge::array_to_fields)
        .collect();

    let challenges_digest = {
        let mut sponge = crate::poseidon::Sponge::<Fp>::default();
        for old_bulletproof_challenges in &old_bulletproof_challenges {
            sponge.absorb(old_bulletproof_challenges);
        }
        sponge.squeeze()
    };

    let mut sponge = crate::poseidon::FqSponge::default();
    sponge.absorb_fq(&[four_u64_to_field(
        &proof_state.sponge_digest_before_evaluations,
    )?]);
    sponge.absorb_fq(&[challenges_digest]);
    sponge.absorb_fq(&[evals.ft_eval1]);
    sponge.absorb_fq(x1);
    sponge.absorb_fq(x2);
    xs.iter().for_each(|PointEvaluations { zeta, zeta_omega }| {
        sponge.absorb_fq(zeta);
        sponge.absorb_fq(zeta_omega);
    });
    let xi_chal: [u64; 2] = sponge.squeeze_limbs();
    let r_chal: [u64; 2] = sponge.squeeze_limbs();

    let xi = ScalarChallenge::from(xi_chal).to_field(&endo);
    let r = ScalarChallenge::from(r_chal).to_field(&endo);

    let public_input = &evals.evals.public_input;

    let es = &evals.evals.evals;
    let tick_combined_evals = evals_of_split_evals(zeta, zetaw, es, super::BACKEND_TICK_ROUNDS_N);
    let public = PointEvaluations {
        zeta: public_input.0.clone(),
        zeta_omega: public_input.1.clone(),
    };

    let combined_inner_product_actual =
        combined_inner_product(CombinedInnerProductParams::<_, { Fp::NROUNDS }, 4> {
            env: &env,
            evals: es,
            combined_evals: &tick_combined_evals,
            public: &public,
            minimal: &plonk_minimal,
            ft_eval1: evals.ft_eval1,
            r,
            old_bulletproof_challenges: &old_bulletproof_challenges,
            xi,
            zetaw,
        });

    let bulletproof_challenges: Vec<_> = proof_state
        .deferred_values
        .bulletproof_challenges
        .iter()
        .map(ScalarChallenge::limbs_to_field)
        .collect();

    let b_actual = {
        let challenge_polys = challenge_polynomial(&bulletproof_challenges);
        challenge_polys(zeta) + (r * challenge_polys(zetaw))
    };

    let to_shifted = |f: Fp| <Fp as FieldWitness>::Shifting::of_field(f);

    Ok(DeferredValues {
        plonk,
        combined_inner_product: to_shifted(combined_inner_product_actual),
        b: to_shifted(b_actual),
        xi: xi_chal,
        bulletproof_challenges,
        branch_data: proof_state.deferred_values.branch_data.clone(),
    })
}
