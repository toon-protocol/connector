use field::FieldWitness;
use poly_commitment::ipa::OpeningProof;

pub mod accumulator_check;
pub mod constants;
mod conv;
pub mod field;
pub mod group_map;
pub mod opt_sponge;
mod prover;
pub mod public_input;
pub mod step;
pub mod to_field_elements;
pub mod transaction;
pub mod unfinalized;
pub mod util;
pub mod verification;
pub mod verifiers;
pub mod witness;
pub mod wrap;

pub const BACKEND_TICK_ROUNDS_N: usize = 16;
pub const BACKEND_TOCK_ROUNDS_N: usize = 15;

pub use mina_poseidon::pasta::FULL_ROUNDS;

pub type VerifierIndex<F> = kimchi::verifier_index::VerifierIndex<
    FULL_ROUNDS,
    <F as FieldWitness>::OtherCurve,
    poly_commitment::ipa::SRS<<F as FieldWitness>::OtherCurve>,
>;
pub type ProverProof<F> = kimchi::proof::ProverProof<
    <F as FieldWitness>::OtherCurve,
    OpeningProof<<F as FieldWitness>::OtherCurve, FULL_ROUNDS>,
    FULL_ROUNDS,
>;
