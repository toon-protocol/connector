//! The side-loaded verification key and its hash.

use ark_ff::Field;
use mina_curves::pasta::Fp;

use crate::{
    ledger::{
        hash::{AppendToInputs, ToInputs},
        proofs::{
            field::FieldWitness,
            transaction::{make_group, InnerCurve, PlonkVerificationKeyEvals},
        },
        util::try_array_into_with,
    },
    p2p_messages::{
        bigint::{BigInt, InvalidBigInt},
        v2::{
            MinaBaseVerificationKeyWireStableV1, MinaBaseVerificationKeyWireStableV1WrapIndex,
            PicklesBaseProofsVerifiedStableV1,
        },
    },
    poseidon::hash::{params::MINA_SIDELOADED_VK, Inputs},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProofVerified {
    N0,
    N1,
    N2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationKey {
    pub max_proofs_verified: ProofVerified,
    pub actual_wrap_domain_size: ProofVerified,
    pub wrap_index: Box<PlonkVerificationKeyEvals<Fp>>,
    // `wrap_vk` is not used for hash inputs
    pub wrap_vk: Option<()>,
}

impl ProofVerified {
    /// <https://github.com/MinaProtocol/mina/blob/47a269c2e917775b34a83775b8a55fcc44830831/src/lib/pickles_base/proofs_verified.ml#L17>
    pub fn to_int(&self) -> usize {
        match self {
            ProofVerified::N0 => 0,
            ProofVerified::N1 => 1,
            ProofVerified::N2 => 2,
        }
    }
}

impl ToInputs for ProofVerified {
    /// <https://github.com/MinaProtocol/mina/blob/436023ba41c43a50458a551b7ef7a9ae61670b25/src/lib/pickles_base/proofs_verified.ml#L125>
    fn to_inputs(&self, inputs: &mut Inputs) {
        let bits = match self {
            ProofVerified::N0 => [true, false, false],
            ProofVerified::N1 => [false, true, false],
            ProofVerified::N2 => [false, false, true],
        };

        for bit in bits {
            inputs.append_bool(bit);
        }
    }
}

impl ToInputs for VerificationKey {
    fn to_inputs(&self, inputs: &mut Inputs) {
        let Self {
            max_proofs_verified,
            actual_wrap_domain_size,
            wrap_index,
            wrap_vk: _,
        } = self;

        let PlonkVerificationKeyEvals {
            sigma,
            coefficients,
            generic,
            psm,
            complete_add,
            mul,
            emul,
            endomul_scalar,
        } = wrap_index.as_ref();

        inputs.append(max_proofs_verified);
        inputs.append(actual_wrap_domain_size);

        for sigma in sigma {
            inputs.append(sigma);
        }
        for coefficients in coefficients {
            inputs.append(coefficients);
        }
        inputs.append(generic);
        inputs.append(psm);
        inputs.append(complete_add);
        inputs.append(mul);
        inputs.append(emul);
        inputs.append(endomul_scalar);
    }
}

impl VerificationKey {
    /// The Pickles verification-key hash (what o1js prints as `verificationKey.hash`).
    pub fn hash(&self) -> Fp {
        self.hash_with_param(&MINA_SIDELOADED_VK)
    }
}

impl TryFrom<&MinaBaseVerificationKeyWireStableV1> for VerificationKey {
    type Error = InvalidBigInt;

    fn try_from(vk: &MinaBaseVerificationKeyWireStableV1) -> Result<Self, Self::Error> {
        let MinaBaseVerificationKeyWireStableV1 {
            max_proofs_verified,
            actual_wrap_domain_size,
            wrap_index,
        } = vk;

        Ok(VerificationKey {
            max_proofs_verified: match max_proofs_verified {
                PicklesBaseProofsVerifiedStableV1::N0 => ProofVerified::N0,
                PicklesBaseProofsVerifiedStableV1::N1 => ProofVerified::N1,
                PicklesBaseProofsVerifiedStableV1::N2 => ProofVerified::N2,
            },
            actual_wrap_domain_size: match actual_wrap_domain_size {
                PicklesBaseProofsVerifiedStableV1::N0 => ProofVerified::N0,
                PicklesBaseProofsVerifiedStableV1::N1 => ProofVerified::N1,
                PicklesBaseProofsVerifiedStableV1::N2 => ProofVerified::N2,
            },
            wrap_index: Box::new(wrap_index.try_into()?),
            wrap_vk: None,
        })
    }
}

impl TryFrom<&MinaBaseVerificationKeyWireStableV1WrapIndex> for PlonkVerificationKeyEvals<Fp> {
    type Error = InvalidBigInt;

    fn try_from(value: &MinaBaseVerificationKeyWireStableV1WrapIndex) -> Result<Self, Self::Error> {
        let MinaBaseVerificationKeyWireStableV1WrapIndex {
            sigma_comm,
            coefficients_comm,
            generic_comm,
            psm_comm,
            complete_add_comm,
            mul_comm,
            emul_comm,
            endomul_scalar_comm,
        } = value;

        let sigma = try_array_into_with(sigma_comm, |s| s.try_into())?;
        let coefficients = try_array_into_with(coefficients_comm, |s| s.try_into())?;

        Ok(PlonkVerificationKeyEvals {
            sigma,
            coefficients,
            generic: generic_comm.try_into()?,
            psm: psm_comm.try_into()?,
            complete_add: complete_add_comm.try_into()?,
            mul: mul_comm.try_into()?,
            emul: emul_comm.try_into()?,
            endomul_scalar: endomul_scalar_comm.try_into()?,
        })
    }
}

impl<F: FieldWitness> TryFrom<&(BigInt, BigInt)> for InnerCurve<F>
where
    F: Field,
{
    type Error = InvalidBigInt;

    fn try_from((x, y): &(BigInt, BigInt)) -> Result<Self, Self::Error> {
        Ok(Self::of_affine(make_group::<F>(
            x.to_field()?,
            y.to_field()?,
        )))
    }
}
