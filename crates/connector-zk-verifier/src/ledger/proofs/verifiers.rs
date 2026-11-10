use std::sync::Arc;

use ark_poly::{EvaluationDomain, Radix2EvaluationDomain};
use kimchi::{
    circuits::{
        constraints::FeatureFlags,
        lookup::lookups::{LookupFeatures, LookupPatterns},
        polynomials::permutation::{permutation_vanishing_polynomial, zk_w},
    },
    linearization::expr_linearization,
    mina_curves::pasta::Pallas,
};
use mina_curves::pasta::{Fp, Fq};
use poly_commitment::{ipa::SRS, SRS as _};

use crate::ledger::{proofs::BACKEND_TOCK_ROUNDS_N, VerificationKey};

use super::{
    transaction::{endos, InnerCurve},
    wrap::{Domain, Domains},
    VerifierIndex,
};

/// <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/crypto/kimchi_bindings/stubs/src/pasta_fq_plonk_verifier_index.rs#L213>
/// <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/pickles/common.ml#L16C1-L25C58>
pub fn make_shifts(
    domain: &Radix2EvaluationDomain<Fq>,
) -> kimchi::circuits::polynomials::permutation::Shifts<Fq> {
    // let value = 1 << log2_size;
    // let domain = Domain::<Fq>::new(value).unwrap();
    kimchi::circuits::polynomials::permutation::Shifts::new(domain)
}

// <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/pickles/common.ml#L27>
pub fn wrap_domains(proofs_verified: usize) -> Domains {
    let h = match proofs_verified {
        0 => 13,
        1 => 14,
        2 => 15,
        _ => unreachable!(),
    };

    Domains {
        h: Domain::Pow2RootsOfUnity(h),
    }
}

/// <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/pickles/side_loaded_verification_key.ml#L206>
pub fn make_zkapp_verifier_index(vk: &VerificationKey) -> VerifierIndex<Fq> {
    let d = wrap_domains(vk.actual_wrap_domain_size.to_int());
    let log2_size = d.h.log2_size();

    let public = 40; // Is that constant ?

    let domain: Radix2EvaluationDomain<Fq> = Radix2EvaluationDomain::new(1 << log2_size).unwrap();

    let srs = {
        let degree = 1 << BACKEND_TOCK_ROUNDS_N;
        let srs = SRS::<Pallas>::create(degree);
        srs.get_lagrange_basis(domain);
        srs
    };

    let make_poly = |poly: &InnerCurve<Fp>| poly_commitment::PolyComm {
        chunks: vec![poly.to_affine()],
    };

    let feature_flags = FeatureFlags {
        range_check0: false,
        range_check1: false,
        foreign_field_add: false,
        foreign_field_mul: false,
        rot: false,
        xor: false,
        lookup_features: LookupFeatures {
            patterns: LookupPatterns {
                xor: false,
                lookup: false,
                range_check: false,
                foreign_field_mul: false,
            },
            joint_lookup_used: false,
            uses_runtime_tables: false,
        },
    };

    let (endo_q, _endo_r) = endos::<Fq>();
    let (linearization, powers_of_alpha) = expr_linearization(Some(&feature_flags), true);

    let shift = make_shifts(&domain);

    // <https://github.com/MinaProtocol/mina/blob/047375688f93546d4bdd58c75674394e3faae1f4/src/lib/pickles/side_loaded_verification_key.ml#L232>
    let zk_rows = 3;

    // Note: Verifier index is converted from OCaml here:
    // <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/crypto/kimchi_bindings/stubs/src/pasta_fq_plonk_verifier_index.rs#L58>

    VerifierIndex::<Fq> {
        domain,
        max_poly_size: 1 << BACKEND_TOCK_ROUNDS_N,
        srs: Arc::new(srs),
        public,
        prev_challenges: 2,
        sigma_comm: vk.wrap_index.sigma.each_ref().map(make_poly),
        coefficients_comm: vk.wrap_index.coefficients.each_ref().map(make_poly),
        generic_comm: make_poly(&vk.wrap_index.generic),
        psm_comm: make_poly(&vk.wrap_index.psm),
        complete_add_comm: make_poly(&vk.wrap_index.complete_add),
        mul_comm: make_poly(&vk.wrap_index.mul),
        emul_comm: make_poly(&vk.wrap_index.emul),
        endomul_scalar_comm: make_poly(&vk.wrap_index.endomul_scalar),
        range_check0_comm: None,
        range_check1_comm: None,
        foreign_field_add_comm: None,
        foreign_field_mul_comm: None,
        xor_comm: None,
        rot_comm: None,
        shift: *shift.shifts(),
        permutation_vanishing_polynomial_m: std::sync::OnceLock::from(
            permutation_vanishing_polynomial(domain, zk_rows),
        ),
        w: { std::sync::OnceLock::from(zk_w(domain, zk_rows)) },
        endo: endo_q,
        lookup_index: None,
        linearization,
        powers_of_alpha,
        zk_rows,
    }
}
