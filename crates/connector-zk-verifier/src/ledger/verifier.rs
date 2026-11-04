use std::sync::{Arc, OnceLock};

use mina_curves::pasta::{Fp, Vesta};
use poly_commitment::{ipa::SRS, SRS as _};

use crate::ledger::proofs::field::FieldWitness;

/// The Structured Reference String (SRS) of the accumulator check, built once.
///
/// The SRS is derived (hash to curve) from its depth alone: it reads no file and touches
/// no network.
pub fn get_srs() -> Arc<SRS<Vesta>> {
    static SRS_VESTA: OnceLock<Arc<SRS<Vesta>>> = OnceLock::new();
    Arc::clone(SRS_VESTA.get_or_init(|| {
        Arc::new(SRS::<Vesta>::create(
            <Fp as FieldWitness>::Scalar::SRS_DEPTH,
        ))
    }))
}
