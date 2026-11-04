use mina_curves::pasta::Fp;

use crate::poseidon::hash::{hash_with_kimchi, Inputs, LazyParam};

pub trait ToInputs {
    fn to_inputs(&self, inputs: &mut Inputs);

    fn hash_with_param(&self, param: &LazyParam) -> Fp {
        let mut inputs = Inputs::new();
        self.to_inputs(&mut inputs);
        hash_with_kimchi(param, &inputs.to_fields())
    }
}

impl ToInputs for Fp {
    fn to_inputs(&self, inputs: &mut Inputs) {
        inputs.append_field(*self);
    }
}

impl<const N: usize> ToInputs for [Fp; N] {
    fn to_inputs(&self, inputs: &mut Inputs) {
        for field in self {
            inputs.append(field);
        }
    }
}

impl ToInputs for bool {
    fn to_inputs(&self, inputs: &mut Inputs) {
        inputs.append_bool(*self);
    }
}

pub trait AppendToInputs {
    fn append<T>(&mut self, value: &T)
    where
        T: ToInputs;
}

impl AppendToInputs for Inputs {
    fn append<T>(&mut self, value: &T)
    where
        T: ToInputs,
    {
        value.to_inputs(self);
    }
}
