use crate::p2p_messages::{array::ArrayN16, list::List, pseq::PaddedSeq};
use binprot_derive::{BinProtRead, BinProtWrite};
use derive_more::Deref;
use rsexp_derive::{OfSexp, SexpOf};

/// Derived name: `Pickles__Proof.Proofs_verified_2.Repr.Stable.V2.statement.proof_state.deferred_values.plonk.feature_flags`
///
/// Gid: `464`
/// Location: [src/lib/pickles_types/plonk_types.ml:194:6](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles_types/plonk_types.ml#L194)
/// Args: bool
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesProofProofsVerified2ReprStableV2StatementProofStateDeferredValuesPlonkFeatureFlags
{
    pub range_check0: bool,
    pub range_check1: bool,
    pub foreign_field_add: bool,
    pub foreign_field_mul: bool,
    pub xor: bool,
    pub rot: bool,
    pub lookup: bool,
    pub runtime_tables: bool,
}

/// Derived name: `Pickles__Proof.Proofs_verified_2.Repr.Stable.V2.prev_evals.evals.evals`
///
/// Gid: `465`
/// Location: [src/lib/pickles_types/plonk_types.ml:363:6](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles_types/plonk_types.ml#L363)
/// Args: (ArrayN16 < crate :: bigint :: BigInt > , ArrayN16 < crate :: bigint :: BigInt > ,)
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesProofProofsVerified2ReprStableV2PrevEvalsEvalsEvals {
    pub w: PaddedSeq<
        (
            ArrayN16<crate::p2p_messages::bigint::BigInt>,
            ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ),
        15,
    >,
    pub coefficients: PaddedSeq<
        (
            ArrayN16<crate::p2p_messages::bigint::BigInt>,
            ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ),
        15,
    >,
    pub z: (
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    ),
    pub s: PaddedSeq<
        (
            ArrayN16<crate::p2p_messages::bigint::BigInt>,
            ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ),
        6,
    >,
    pub generic_selector: (
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    ),
    pub poseidon_selector: (
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    ),
    pub complete_add_selector: (
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    ),
    pub mul_selector: (
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    ),
    pub emul_selector: (
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    ),
    pub endomul_scalar_selector: (
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    ),
    pub range_check0_selector: Option<(
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    )>,
    pub range_check1_selector: Option<(
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    )>,
    pub foreign_field_add_selector: Option<(
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    )>,
    pub foreign_field_mul_selector: Option<(
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    )>,
    pub xor_selector: Option<(
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    )>,
    pub rot_selector: Option<(
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    )>,
    pub lookup_aggregation: Option<(
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    )>,
    pub lookup_table: Option<(
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    )>,
    pub lookup_sorted: PaddedSeq<
        Option<(
            ArrayN16<crate::p2p_messages::bigint::BigInt>,
            ArrayN16<crate::p2p_messages::bigint::BigInt>,
        )>,
        5,
    >,
    pub runtime_lookup_table: Option<(
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    )>,
    pub runtime_lookup_table_selector: Option<(
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    )>,
    pub xor_lookup_selector: Option<(
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    )>,
    pub lookup_gate_lookup_selector: Option<(
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    )>,
    pub range_check_lookup_selector: Option<(
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    )>,
    pub foreign_field_mul_lookup_selector: Option<(
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
        ArrayN16<crate::p2p_messages::bigint::BigInt>,
    )>,
}

/// Derived name: `Pickles__Proof.Proofs_verified_2.Repr.Stable.V2.prev_evals.evals`
///
/// Gid: `466`
/// Location: [src/lib/pickles_types/plonk_types.ml:1057:8](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles_types/plonk_types.ml#L1057)
/// Args: (crate :: bigint :: BigInt , crate :: bigint :: BigInt ,) , (ArrayN16 < crate :: bigint :: BigInt > , ArrayN16 < crate :: bigint :: BigInt > ,)
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesProofProofsVerified2ReprStableV2PrevEvalsEvals {
    pub public_input: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub evals: PicklesProofProofsVerified2ReprStableV2PrevEvalsEvalsEvals,
}

/// Derived name: `Pickles__Proof.Proofs_verified_2.Repr.Stable.V2.prev_evals`
///
/// Gid: `467`
/// Location: [src/lib/pickles_types/plonk_types.ml:1092:6](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles_types/plonk_types.ml#L1092)
/// Args: crate :: bigint :: BigInt , ArrayN16 < crate :: bigint :: BigInt >
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesProofProofsVerified2ReprStableV2PrevEvals {
    pub evals: PicklesProofProofsVerified2ReprStableV2PrevEvalsEvals,
    pub ft_eval1: crate::p2p_messages::bigint::BigInt,
}

/// Derived name: `Pickles__Wrap_wire_proof.Stable.V1.bulletproof`
///
/// Gid: `468`
/// Location: [src/lib/pickles_types/plonk_types.ml:1141:8](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles_types/plonk_types.ml#L1141)
/// Args: (crate :: bigint :: BigInt , crate :: bigint :: BigInt ,) , crate :: bigint :: BigInt
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesWrapWireProofStableV1Bulletproof {
    pub lr: ArrayN16<(
        (
            crate::p2p_messages::bigint::BigInt,
            crate::p2p_messages::bigint::BigInt,
        ),
        (
            crate::p2p_messages::bigint::BigInt,
            crate::p2p_messages::bigint::BigInt,
        ),
    )>,
    pub z_1: crate::p2p_messages::bigint::BigInt,
    pub z_2: crate::p2p_messages::bigint::BigInt,
    pub delta: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub challenge_polynomial_commitment: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
}

/// Derived name: `Mina_base__Verification_key_wire.Stable.V1.wrap_index`
///
/// Gid: `476`
/// Location: [src/lib/pickles_types/plonk_verification_key_evals.ml:7:4](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles_types/plonk_verification_key_evals.ml#L7)
/// Args: (crate :: bigint :: BigInt , crate :: bigint :: BigInt ,)
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite)]
pub struct MinaBaseVerificationKeyWireStableV1WrapIndex {
    pub sigma_comm: PaddedSeq<
        (
            crate::p2p_messages::bigint::BigInt,
            crate::p2p_messages::bigint::BigInt,
        ),
        7,
    >,
    pub coefficients_comm: PaddedSeq<
        (
            crate::p2p_messages::bigint::BigInt,
            crate::p2p_messages::bigint::BigInt,
        ),
        15,
    >,
    pub generic_comm: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub psm_comm: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub complete_add_comm: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub mul_comm: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub emul_comm: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub endomul_scalar_comm: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
}

/// Derived name: `Pickles__Reduced_messages_for_next_proof_over_same_field.Wrap.Challenges_vector.Stable.V2.a.challenge`
///
/// Gid: `482`
/// Location: [src/lib/crypto/kimchi_backend/common/scalar_challenge.ml:6:4](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/crypto/kimchi_backend/common/scalar_challenge.ml#L6)
/// Args: PaddedSeq < LimbVectorConstantHex64StableV1 , 2 >
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2AChallenge {
    pub inner: PaddedSeq<LimbVectorConstantHex64StableV1, 2>,
}

/// **OCaml name**: `Pickles_base__Proofs_verified.Stable.V1`
///
/// Gid: `514`
/// Location: [src/lib/pickles_base/proofs_verified.ml:8:4](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles_base/proofs_verified.ml#L8)
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf)]
pub enum PicklesBaseProofsVerifiedStableV1 {
    N0,
    N1,
    N2,
}

/// **OCaml name**: `Limb_vector__Constant.Hex64.Stable.V1`
///
/// Gid: `523`
/// Location: [src/lib/pickles/limb_vector/constant.ml:60:6](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/limb_vector/constant.ml#L60)
///
///
/// Gid: `125`
/// Location: [src/int64.ml:6:6](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/int64.ml#L6)
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, Deref)]
pub struct LimbVectorConstantHex64StableV1(pub crate::p2p_messages::number::UInt64);

/// **OCaml name**: `Composition_types__Branch_data.Make_str.Domain_log2.Stable.V1`
///
/// Gid: `524`
/// Location: [src/lib/pickles/composition_types/branch_data.ml:24:8](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/composition_types/branch_data.ml#L24)
///
///
/// Gid: `161`
/// Location: [src/std_internal.ml:113:2](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/std_internal.ml#L113)
///
///
/// Gid: `89`
/// Location: [src/char.ml:8:6](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/char.ml#L8)
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, Deref)]
pub struct CompositionTypesBranchDataDomainLog2StableV1(pub crate::p2p_messages::char::Char);

/// **OCaml name**: `Composition_types__Branch_data.Make_str.Stable.V1`
///
/// Gid: `525`
/// Location: [src/lib/pickles/composition_types/branch_data.ml:51:6](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/composition_types/branch_data.ml#L51)
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct CompositionTypesBranchDataStableV1 {
    pub proofs_verified: PicklesBaseProofsVerifiedStableV1,
    pub domain_log2: CompositionTypesBranchDataDomainLog2StableV1,
}

/// Derived name: `Pickles__Reduced_messages_for_next_proof_over_same_field.Wrap.Challenges_vector.Stable.V2.a`
///
/// Gid: `526`
/// Location: [src/lib/pickles/composition_types/bulletproof_challenge.ml:4:4](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/composition_types/bulletproof_challenge.ml#L4)
/// Args: PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2AChallenge
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2A {
    pub prechallenge:
        PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2AChallenge,
}

/// **OCaml name**: `Composition_types__Digest.Constant.Stable.V1`
///
/// Gid: `527`
/// Location: [src/lib/pickles/composition_types/digest.ml:13:6](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/composition_types/digest.ml#L13)
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, Deref)]
pub struct CompositionTypesDigestConstantStableV1(
    pub PaddedSeq<LimbVectorConstantHex64StableV1, 4>,
);

/// Derived name: `Pickles__Proof.Proofs_verified_2.Repr.Stable.V2.statement.proof_state.deferred_values.plonk`
///
/// Gid: `528`
/// Location: [src/lib/pickles/composition_types/composition_types.ml:45:14](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/composition_types/composition_types.ml#L45)
/// Args: PaddedSeq < LimbVectorConstantHex64StableV1 , 2 > , PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2AChallenge , bool
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesProofProofsVerified2ReprStableV2StatementProofStateDeferredValuesPlonk {
    pub alpha:
        PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2AChallenge,
    pub beta: PaddedSeq<LimbVectorConstantHex64StableV1, 2>,
    pub gamma: PaddedSeq<LimbVectorConstantHex64StableV1, 2>,
    pub zeta: PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2AChallenge,
    pub joint_combiner: Option<
        PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2AChallenge,
    >,
    pub feature_flags:
        PicklesProofProofsVerified2ReprStableV2StatementProofStateDeferredValuesPlonkFeatureFlags,
}

/// Derived name: `Pickles__Proof.Proofs_verified_2.Repr.Stable.V2.statement.proof_state.deferred_values`
///
/// Gid: `530`
/// Location: [src/lib/pickles/composition_types/composition_types.ml:275:12](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/composition_types/composition_types.ml#L275)
/// Args: PaddedSeq < LimbVectorConstantHex64StableV1 , 2 > , PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2AChallenge , PicklesProofProofsVerified2ReprStableV2StatementFp , bool , PaddedSeq < PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2A , 16 > , CompositionTypesBranchDataStableV1
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesProofProofsVerified2ReprStableV2StatementProofStateDeferredValues {
    pub plonk: PicklesProofProofsVerified2ReprStableV2StatementProofStateDeferredValuesPlonk,
    pub bulletproof_challenges:
        PaddedSeq<PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2A, 16>,
    pub branch_data: CompositionTypesBranchDataStableV1,
}

/// Derived name: `Pickles__Proof.Proofs_verified_2.Repr.Stable.V2.messages_for_next_wrap_proof`
///
/// Gid: `531`
/// Location: [src/lib/pickles/composition_types/composition_types.ml:397:10](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/composition_types/composition_types.ml#L397)
/// Args: (crate :: bigint :: BigInt , crate :: bigint :: BigInt ,) , PaddedSeq < PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2 , 2 >
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesProofProofsVerified2ReprStableV2MessagesForNextWrapProof {
    pub challenge_polynomial_commitment: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub old_bulletproof_challenges:
        PaddedSeq<PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2, 2>,
}

/// Derived name: `Pickles__Proof.Proofs_verified_2.Repr.Stable.V2.statement.proof_state`
///
/// Gid: `533`
/// Location: [src/lib/pickles/composition_types/composition_types.ml:466:10](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/composition_types/composition_types.ml#L466)
/// Args: PaddedSeq < LimbVectorConstantHex64StableV1 , 2 > , PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2AChallenge , PicklesProofProofsVerified2ReprStableV2StatementFp , bool , PicklesProofProofsVerified2ReprStableV2MessagesForNextWrapProof , CompositionTypesDigestConstantStableV1 , PaddedSeq < PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2A , 16 > , CompositionTypesBranchDataStableV1
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesProofProofsVerified2ReprStableV2StatementProofState {
    pub deferred_values: PicklesProofProofsVerified2ReprStableV2StatementProofStateDeferredValues,
    pub sponge_digest_before_evaluations: CompositionTypesDigestConstantStableV1,
    pub messages_for_next_wrap_proof:
        PicklesProofProofsVerified2ReprStableV2MessagesForNextWrapProof,
}

/// Derived name: `Pickles__Proof.Proofs_verified_2.Repr.Stable.V2.statement`
///
/// Gid: `535`
/// Location: [src/lib/pickles/composition_types/composition_types.ml:714:10](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/composition_types/composition_types.ml#L714)
/// Args: PaddedSeq < LimbVectorConstantHex64StableV1 , 2 > , PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2AChallenge , PicklesProofProofsVerified2ReprStableV2StatementFp , bool , PicklesProofProofsVerified2ReprStableV2MessagesForNextWrapProof , CompositionTypesDigestConstantStableV1 , PicklesProofProofsVerified2ReprStableV2MessagesForNextStepProof , PaddedSeq < PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2A , 16 > , CompositionTypesBranchDataStableV1
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesProofProofsVerified2ReprStableV2Statement {
    pub proof_state: PicklesProofProofsVerified2ReprStableV2StatementProofState,
    pub messages_for_next_step_proof:
        PicklesProofProofsVerified2ReprStableV2MessagesForNextStepProof,
}

/// **OCaml name**: `Pickles__Wrap_wire_proof.Commitments.Stable.V1`
///
/// Gid: `537`
/// Location: [src/lib/pickles/wrap_wire_proof.ml:17:6](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/wrap_wire_proof.ml#L17)
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesWrapWireProofCommitmentsStableV1 {
    pub w_comm: PaddedSeq<
        (
            crate::p2p_messages::bigint::BigInt,
            crate::p2p_messages::bigint::BigInt,
        ),
        15,
    >,
    pub z_comm: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub t_comm: PaddedSeq<
        (
            crate::p2p_messages::bigint::BigInt,
            crate::p2p_messages::bigint::BigInt,
        ),
        7,
    >,
}

/// **OCaml name**: `Pickles__Wrap_wire_proof.Evaluations.Stable.V1`
///
/// Gid: `538`
/// Location: [src/lib/pickles/wrap_wire_proof.ml:55:6](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/wrap_wire_proof.ml#L55)
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesWrapWireProofEvaluationsStableV1 {
    pub w: PaddedSeq<
        (
            crate::p2p_messages::bigint::BigInt,
            crate::p2p_messages::bigint::BigInt,
        ),
        15,
    >,
    pub coefficients: PaddedSeq<
        (
            crate::p2p_messages::bigint::BigInt,
            crate::p2p_messages::bigint::BigInt,
        ),
        15,
    >,
    pub z: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub s: PaddedSeq<
        (
            crate::p2p_messages::bigint::BigInt,
            crate::p2p_messages::bigint::BigInt,
        ),
        6,
    >,
    pub generic_selector: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub poseidon_selector: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub complete_add_selector: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub mul_selector: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub emul_selector: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
    pub endomul_scalar_selector: (
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    ),
}

/// **OCaml name**: `Pickles__Wrap_wire_proof.Stable.V1`
///
/// Gid: `539`
/// Location: [src/lib/pickles/wrap_wire_proof.ml:175:4](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/wrap_wire_proof.ml#L175)
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesWrapWireProofStableV1 {
    pub commitments: PicklesWrapWireProofCommitmentsStableV1,
    pub evaluations: PicklesWrapWireProofEvaluationsStableV1,
    pub ft_eval1: crate::p2p_messages::bigint::BigInt,
    pub bulletproof: PicklesWrapWireProofStableV1Bulletproof,
}

/// Derived name: `Pickles__Proof.Proofs_verified_2.Repr.Stable.V2.messages_for_next_step_proof`
///
/// Gid: `542`
/// Location: [src/lib/pickles/reduced_messages_for_next_proof_over_same_field.ml:16:6](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/reduced_messages_for_next_proof_over_same_field.ml#L16)
/// Args: () , List < (crate :: bigint :: BigInt , crate :: bigint :: BigInt ,) > , List < PaddedSeq < PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2A , 16 > >
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesProofProofsVerified2ReprStableV2MessagesForNextStepProof {
    pub app_state: (),
    pub challenge_polynomial_commitments: List<(
        crate::p2p_messages::bigint::BigInt,
        crate::p2p_messages::bigint::BigInt,
    )>,
    pub old_bulletproof_challenges: List<
        PaddedSeq<PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2A, 16>,
    >,
}

/// **OCaml name**: `Pickles__Reduced_messages_for_next_proof_over_same_field.Wrap.Challenges_vector.Stable.V2`
///
/// Gid: `543`
/// Location: [src/lib/pickles/reduced_messages_for_next_proof_over_same_field.ml:57:8](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/reduced_messages_for_next_proof_over_same_field.ml#L57)
///
///
/// Gid: `488`
/// Location: [src/lib/crypto/kimchi_backend/pasta/basic/kimchi_pasta_basic.ml:32:8](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/crypto/kimchi_backend/pasta/basic/kimchi_pasta_basic.ml#L32)
/// Args: PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2A
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, Deref)]
pub struct PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2(
    pub PaddedSeq<PicklesReducedMessagesForNextProofOverSameFieldWrapChallengesVectorStableV2A, 15>,
);

/// **OCaml name**: `Mina_base__Verification_key_wire.Stable.V1`
///
/// Gid: `544`
/// Location: [src/lib/pickles/side_loaded_verification_key.ml:170:6](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/side_loaded_verification_key.ml#L170)
///
///
/// Gid: `519`
/// Location: [src/lib/pickles_base/side_loaded_verification_key.ml:130:6](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles_base/side_loaded_verification_key.ml#L130)
/// Args: (crate :: bigint :: BigInt , crate :: bigint :: BigInt ,)
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite)]
pub struct MinaBaseVerificationKeyWireStableV1 {
    pub max_proofs_verified: PicklesBaseProofsVerifiedStableV1,
    pub actual_wrap_domain_size: PicklesBaseProofsVerifiedStableV1,
    pub wrap_index: MinaBaseVerificationKeyWireStableV1WrapIndex,
}

/// **OCaml name**: `Pickles__Proof.Proofs_verified_2.Repr.Stable.V2`
///
/// Gid: `547`
/// Location: [src/lib/pickles/proof.ml:342:8](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/proof.ml#L342)
///
///
/// Gid: `546`
/// Location: [src/lib/pickles/proof.ml:47:8](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/proof.ml#L47)
/// Args: PicklesProofProofsVerified2ReprStableV2MessagesForNextWrapProof , PicklesProofProofsVerified2ReprStableV2MessagesForNextStepProof
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesProofProofsVerified2ReprStableV2 {
    pub statement: PicklesProofProofsVerified2ReprStableV2Statement,
    pub prev_evals: PicklesProofProofsVerified2ReprStableV2PrevEvals,
    pub proof: PicklesWrapWireProofStableV1,
}

/// **OCaml name**: `Pickles__Proof.Proofs_verified_max.Stable.V2`
///
/// Gid: `548`
/// Location: [src/lib/pickles/proof.ml:411:8](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/proof.ml#L411)
///
///
/// Gid: `546`
/// Location: [src/lib/pickles/proof.ml:47:8](https://github.com/MinaProtocol/mina/blob/1551e2faaa/src/lib/pickles/proof.ml#L47)
/// Args: PicklesProofProofsVerified2ReprStableV2MessagesForNextWrapProof , PicklesProofProofsVerified2ReprStableV2MessagesForNextStepProof
#[derive(Clone, Debug, PartialEq, BinProtRead, BinProtWrite, SexpOf, OfSexp)]
pub struct PicklesProofProofsVerifiedMaxStableV2 {
    pub statement: PicklesProofProofsVerified2ReprStableV2Statement,
    pub prev_evals: PicklesProofProofsVerified2ReprStableV2PrevEvals,
    pub proof: PicklesWrapWireProofStableV1,
}

// Utils

// FIXME: This workaround ignores values in the JSON that correspond to `()` in the target type.
// The proper solution would involve defining a composite type that accurately reflects
// the structure of the data, including fields with actual values instead of unit (`()`).
