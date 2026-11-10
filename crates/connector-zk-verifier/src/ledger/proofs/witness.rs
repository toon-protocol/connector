use super::{field::FieldWitness, to_field_elements::ToFieldElements, transaction::Check};

/// The values a circuit would hand to its prover. The verifier runs the same code the prover
/// does, so it threads one through; nothing here reads them back.
#[derive(Debug)]
pub struct Witness<F: FieldWitness> {
    aux: Vec<F>,
}

impl<F: FieldWitness> Witness<F> {
    pub fn empty() -> Self {
        Self { aux: Vec::new() }
    }

    pub fn exists<T>(&mut self, data: T) -> T
    where
        T: ToFieldElements<F> + Check<F>,
    {
        let data = self.exists_no_check(data);
        data.check(self);
        data
    }

    /// Same as `Self::exists`, but do not call `Check::check` on `data`
    /// We use this wherever `seal` is used in OCaml, or on `if_` conditions
    pub fn exists_no_check<T>(&mut self, data: T) -> T
    where
        T: ToFieldElements<F>,
    {
        data.to_field_elements(&mut self.aux);

        data
    }
}
