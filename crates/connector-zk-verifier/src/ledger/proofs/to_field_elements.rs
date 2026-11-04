use std::borrow::Cow;

use crate::p2p_messages::string::ByteString;
use ark_ff::Field;
use kimchi::proof::{PointEvaluations, ProofEvaluations};
use mina_curves::pasta::{Fp, Fq};
use mina_signer::CompressedPubKey;

use super::{
    field::{Boolean, FieldWitness, GroupAffine},
    transaction::{field_to_bits, InnerCurve, PlonkVerificationKeyEvals},
    unfinalized::{AllEvals, EvalsWithPublicInput},
};

pub trait ToFieldElements<F: Field> {
    fn to_field_elements(&self, fields: &mut Vec<F>);
}

// impl ToFieldElements for Statement<SokDigest> {
//     fn to_field_elements(&self) -> Vec<Fp> {
//         let mut inputs = crate::ledger::Inputs::new();
//         inputs.append(self);

//         inputs.to_fields()
//     }
// }

impl<F: Field> ToFieldElements<F> for () {
    fn to_field_elements(&self, _fields: &mut Vec<F>) {}
}

impl<F: FieldWitness, T: ToFieldElements<F>> ToFieldElements<F> for Vec<T> {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        self.iter().for_each(|v| v.to_field_elements(fields));
    }
}

impl<F: FieldWitness, T: ToFieldElements<F>> ToFieldElements<F> for Box<[T]> {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        self.iter().for_each(|v| v.to_field_elements(fields));
    }
}

impl<F: FieldWitness> ToFieldElements<F> for Fp {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        use crate::ledger::proofs::field::IntoGeneric;
        fields.push(self.into_gen());
    }
}

impl<F: FieldWitness, T: ToFieldElements<F> + Clone> ToFieldElements<F> for Cow<'_, T> {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        let this: &T = self.as_ref();
        this.to_field_elements(fields)
    }
}

// pack
pub fn field_of_bits<F: FieldWitness, const N: usize>(bs: &[bool; N]) -> F {
    bs.iter().rev().fold(F::zero(), |acc, b| {
        let acc = acc + acc;
        if *b {
            acc + F::one()
        } else {
            acc
        }
    })
}

impl<F: FieldWitness> ToFieldElements<F> for Fq {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        use crate::ledger::proofs::field::IntoGeneric;
        use std::any::TypeId;

        // TODO: Refactor when specialization is stable
        if TypeId::of::<F>() == TypeId::of::<Fq>() {
            fields.push(self.into_gen());
        } else {
            // `Fq` is larger than `Fp` so we have to split the field (low & high bits)
            // See:
            // <https://github.com/MinaProtocol/mina/blob/e85cf6969e42060f69d305fb63df9b8d7215d3d7/src/lib/pickles/impls.ml#L94C1-L105C45>

            let to_high_low = |fq: Fq| {
                let [low, high @ ..] = field_to_bits::<Fq, 255>(fq);
                [field_of_bits(&high), F::from(low)]
            };
            fields.extend(to_high_low(*self));
        }
    }
}

impl<F: FieldWitness, T: ToFieldElements<F>, const N: usize> ToFieldElements<F> for [T; N] {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        self.iter().for_each(|v| v.to_field_elements(fields));
    }
}

impl<F: FieldWitness> ToFieldElements<F> for ByteString {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        let slice: &[u8] = self;
        slice.to_field_elements(fields);
    }
}

impl<F: FieldWitness> ToFieldElements<F> for GroupAffine<F> {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        let Self { x, y, .. } = self;
        y.to_field_elements(fields);
        x.to_field_elements(fields);
    }
}

impl<F: FieldWitness> ToFieldElements<F> for &'_ [u8] {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        const BITS: [u8; 8] = [1, 2, 4, 8, 16, 32, 64, 128];
        fields.extend(
            self.iter()
                .flat_map(|byte| BITS.iter().map(|bit| F::from((*byte & bit != 0) as u64))),
        );
    }
}

impl<F: FieldWitness> ToFieldElements<F> for &'_ [bool] {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        fields.reserve(self.len());
        fields.extend(self.iter().copied().map(F::from))
    }
}

impl<F: FieldWitness> ToFieldElements<F> for bool {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        F::from(*self).to_field_elements(fields)
    }
}

impl<F: FieldWitness> ToFieldElements<F> for u64 {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        F::from(*self).to_field_elements(fields)
    }
}

impl<F: FieldWitness> ToFieldElements<F> for u32 {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        F::from(*self).to_field_elements(fields)
    }
}

impl<F: FieldWitness, T: ToFieldElements<F>> ToFieldElements<F> for PointEvaluations<T> {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        let Self { zeta, zeta_omega } = self;
        zeta.to_field_elements(fields);
        zeta_omega.to_field_elements(fields);
    }
}

impl<F: FieldWitness, T: ToFieldElements<F>> ToFieldElements<F> for ProofEvaluations<T> {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        let Self {
            public: _,
            w,
            z,
            s,
            coefficients,
            generic_selector,
            poseidon_selector,
            complete_add_selector,
            mul_selector,
            emul_selector,
            endomul_scalar_selector,
            range_check0_selector,
            range_check1_selector,
            foreign_field_add_selector,
            foreign_field_mul_selector,
            xor_selector,
            rot_selector,
            lookup_aggregation,
            lookup_table,
            lookup_sorted,
            runtime_lookup_table,
            runtime_lookup_table_selector,
            xor_lookup_selector,
            lookup_gate_lookup_selector,
            range_check_lookup_selector,
            foreign_field_mul_lookup_selector,
        } = self;

        let mut push = |value: &T| {
            value.to_field_elements(fields);
        };

        w.iter().for_each(&mut push);
        coefficients.iter().for_each(&mut push);
        push(z);
        s.iter().for_each(&mut push);
        push(generic_selector);
        push(poseidon_selector);
        push(complete_add_selector);
        push(mul_selector);
        push(emul_selector);
        push(endomul_scalar_selector);
        range_check0_selector.as_ref().map(&mut push);
        range_check1_selector.as_ref().map(&mut push);
        foreign_field_add_selector.as_ref().map(&mut push);
        foreign_field_mul_selector.as_ref().map(&mut push);
        xor_selector.as_ref().map(&mut push);
        rot_selector.as_ref().map(&mut push);
        lookup_aggregation.as_ref().map(&mut push);
        lookup_table.as_ref().map(&mut push);
        lookup_sorted.iter().for_each(|v| {
            v.as_ref().map(&mut push);
        });
        runtime_lookup_table.as_ref().map(&mut push);
        runtime_lookup_table_selector.as_ref().map(&mut push);
        xor_lookup_selector.as_ref().map(&mut push);
        lookup_gate_lookup_selector.as_ref().map(&mut push);
        range_check_lookup_selector.as_ref().map(&mut push);
        foreign_field_mul_lookup_selector.as_ref().map(&mut push);
    }
}

impl<F: FieldWitness> ToFieldElements<F> for AllEvals<F> {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        let Self {
            ft_eval1,
            evals:
                EvalsWithPublicInput {
                    evals,
                    public_input,
                },
        } = self;

        public_input.to_field_elements(fields);
        evals.to_field_elements(fields);
        ft_eval1.to_field_elements(fields);
    }
}

impl<F: FieldWitness> ToFieldElements<F> for &[AllEvals<F>] {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        self.iter().for_each(|e| e.to_field_elements(fields))
    }
}

impl ToFieldElements<Fp> for CompressedPubKey {
    fn to_field_elements(&self, fields: &mut Vec<Fp>) {
        let Self { x, is_odd } = self;
        x.to_field_elements(fields);
        is_odd.to_field_elements(fields);
    }
}

impl ToFieldElements<Fp> for mina_signer::Signature {
    fn to_field_elements(&self, fields: &mut Vec<Fp>) {
        let Self { rx, s } = self;

        rx.to_field_elements(fields);
        let s_bits = field_to_bits::<_, 255>(*s);
        s_bits.to_field_elements(fields);
    }
}

impl ToFieldElements<Fp> for mina_signer::PubKey {
    fn to_field_elements(&self, fields: &mut Vec<Fp>) {
        let GroupAffine::<Fp> { x, y, .. } = self.point();
        x.to_field_elements(fields);
        y.to_field_elements(fields);
    }
}

// TODO: De-deduplicate with ToInputs

impl<F: FieldWitness, A: ToFieldElements<F>, B: ToFieldElements<F>> ToFieldElements<F> for (A, B) {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        let (a, b) = self;
        a.to_field_elements(fields);
        b.to_field_elements(fields);
    }
}

impl<F: FieldWitness> ToFieldElements<F> for PlonkVerificationKeyEvals<F> {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
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

        sigma.iter().for_each(|s| s.to_field_elements(fields));
        coefficients
            .iter()
            .for_each(|c| c.to_field_elements(fields));
        generic.to_field_elements(fields);
        psm.to_field_elements(fields);
        complete_add.to_field_elements(fields);
        mul.to_field_elements(fields);
        emul.to_field_elements(fields);
        endomul_scalar.to_field_elements(fields);
    }
}

// Implementation for references
impl<F: FieldWitness, T: ToFieldElements<F>> ToFieldElements<F> for &T {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        (*self).to_field_elements(fields);
    }
}

impl<F: FieldWitness> ToFieldElements<F> for InnerCurve<F> {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        let GroupAffine::<F> { x, y, .. } = self.to_affine();
        x.to_field_elements(fields);
        y.to_field_elements(fields);
    }
}

impl<F: FieldWitness> ToFieldElements<F> for Boolean {
    fn to_field_elements(&self, fields: &mut Vec<F>) {
        self.to_field::<F>().to_field_elements(fields);
    }
}
