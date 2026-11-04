//! Conversions between the wire proof types and the verifier's own.

use crate::ledger::proofs::{
    field::FieldWitness, public_input::plonk_checks::PlonkMinimal, util::two_u64_to_field,
};
use crate::p2p_messages::{bigint::InvalidBigInt, v2};

impl From<&v2::PicklesProofProofsVerified2ReprStableV2StatementProofStateDeferredValuesPlonkFeatureFlags> for crate::ledger::proofs::step::FeatureFlags::<bool> {
    fn from(value: &v2::PicklesProofProofsVerified2ReprStableV2StatementProofStateDeferredValuesPlonkFeatureFlags) -> Self {
        let v2::PicklesProofProofsVerified2ReprStableV2StatementProofStateDeferredValuesPlonkFeatureFlags {
            range_check0,
            range_check1,
            foreign_field_add,
            foreign_field_mul,
            xor,
            rot,
            lookup,
            runtime_tables,
        } = value;

        Self {
            range_check0: *range_check0,
            range_check1: *range_check1,
            foreign_field_add: *foreign_field_add,
            foreign_field_mul: *foreign_field_mul,
            xor: *xor,
            rot: *rot,
            lookup: *lookup,
            runtime_tables: *runtime_tables,
        }
    }
}

impl From<&crate::ledger::proofs::step::FeatureFlags::<bool>> for v2::PicklesProofProofsVerified2ReprStableV2StatementProofStateDeferredValuesPlonkFeatureFlags {
    fn from(value: &crate::ledger::proofs::step::FeatureFlags::<bool>) -> Self {
        let crate::ledger::proofs::step::FeatureFlags::<bool> {
            range_check0,
            range_check1,
            foreign_field_add,
            foreign_field_mul,
            xor,
            rot,
            lookup,
            runtime_tables,
        } = value;

        Self {
            range_check0: *range_check0,
            range_check1: *range_check1,
            foreign_field_add: *foreign_field_add,
            foreign_field_mul: *foreign_field_mul,
            xor: *xor,
            rot: *rot,
            lookup: *lookup,
            runtime_tables: *runtime_tables,
        }
    }
}

impl<F: FieldWitness>
    TryFrom<&v2::PicklesProofProofsVerified2ReprStableV2StatementProofStateDeferredValuesPlonk>
    for PlonkMinimal<F>
{
    type Error = InvalidBigInt;

    fn try_from(
        value: &v2::PicklesProofProofsVerified2ReprStableV2StatementProofStateDeferredValuesPlonk,
    ) -> Result<Self, Self::Error> {
        let v2::PicklesProofProofsVerified2ReprStableV2StatementProofStateDeferredValuesPlonk {
            alpha,
            beta,
            gamma,
            zeta,
            joint_combiner,
            feature_flags,
        } = value;

        let to_bytes = |v: &v2::LimbVectorConstantHex64StableV1| v.as_u64();

        let alpha_bytes = alpha.inner.each_ref().map(to_bytes);
        let beta_bytes = beta.each_ref().map(to_bytes);
        let gamma_bytes = gamma.each_ref().map(to_bytes);
        let zeta_bytes = zeta.inner.each_ref().map(to_bytes);

        Ok(PlonkMinimal::<F, 2> {
            alpha: two_u64_to_field(&alpha_bytes),
            beta: two_u64_to_field(&beta_bytes),
            gamma: two_u64_to_field(&gamma_bytes),
            zeta: two_u64_to_field(&zeta_bytes),
            joint_combiner: joint_combiner
                .as_ref()
                .map(|f| two_u64_to_field(&f.inner.each_ref().map(to_bytes))),
            alpha_bytes,
            beta_bytes,
            gamma_bytes,
            zeta_bytes,
            joint_combiner_bytes: joint_combiner
                .as_ref()
                .map(|f| f.inner.each_ref().map(to_bytes)),
            feature_flags: feature_flags.into(),
        })
    }
}
