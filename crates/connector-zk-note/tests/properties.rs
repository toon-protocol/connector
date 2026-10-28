//! Tier 1: properties of the zk-note primitives. No I/O, no clock.

use ark_ec::AffineRepr;
use ark_ff::{BigInteger, PrimeField};
use connector_zk_note::commitment::{charge, commit, value_generator};
use connector_zk_note::derive::{binding, context, nullifier, successor_anchor};
use connector_zk_note::state::{sign_state, verify_state, StateMessage};
use connector_zk_note::tiers::{Tiers, TiersError};
use connector_zk_note::{Field, Point, Scalar};
use mina_signer::{Keypair, SecKey};
use proptest::prelude::*;

fn field(n: u64) -> Field {
    Field::from(n) * Field::from(0x9E37_79B9_7F4A_7C15_u64) + Field::from(n)
}

fn keypair(seed: u64) -> Keypair {
    Keypair::try_from(SecKey::new(Scalar::from(seed.max(1)))).unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn charging_delta_is_committing_to_the_balance_less_delta(
        balance in any::<u64>(), delta_frac in any::<u64>(), r in any::<[u8; 32]>(),
    ) {
        let delta = if balance == 0 { 0 } else { delta_frac % (balance + 1) };
        let r = Scalar::from_le_bytes_mod_order(&r);
        prop_assert_eq!(charge(commit(balance, r), delta), commit(balance - delta, r));
    }

    #[test]
    fn the_anchor_is_deterministic_and_differs_from_its_nullifier(secret in any::<u64>(), anchor in any::<u64>()) {
        let n = nullifier(field(secret), field(anchor));
        prop_assert_eq!(successor_anchor(n), successor_anchor(n));
        prop_assert_ne!(successor_anchor(n), n);
    }

    #[test]
    fn a_state_signature_verifies_and_fails_under_any_changed_field(
        seed in 1u64..u64::MAX, vault in any::<u64>(), balance in any::<u64>(), r in any::<u64>(), anchor in any::<u64>(),
    ) {
        let kp = keypair(seed);
        let state = StateMessage {
            vault: field(vault),
            commitment: commit(balance, Scalar::from(r)),
            anchor: field(anchor),
        };
        let sig = sign_state(&kp, &state);
        prop_assert!(verify_state(&sig, &kp.public, &state));

        let mut other = state.clone();
        other.vault += Field::from(1u64);
        prop_assert!(!verify_state(&sig, &kp.public, &other));
        let mut other = state.clone();
        other.anchor += Field::from(1u64);
        prop_assert!(!verify_state(&sig, &kp.public, &other));
        let mut other = state.clone();
        other.commitment = charge(other.commitment, 1);
        prop_assert!(!verify_state(&sig, &kp.public, &other));
        // y alone: a commitment with the same x and the other y.
        let mut other = state.clone();
        other.commitment = (-other.commitment.into_group()).into();
        prop_assert!(!verify_state(&sig, &kp.public, &other));
        // x alone: the signed fields, not the curve, are what is checked.
        let mut other = state.clone();
        other.commitment = Point::new_unchecked(other.commitment.x + Field::from(1u64), other.commitment.y);
        prop_assert!(!verify_state(&sig, &kp.public, &other));
        // another key
        prop_assert!(!verify_state(&sig, &keypair(seed + 1).public, &state));
    }

    #[test]
    fn a_binding_changes_with_every_input(
        n in any::<u64>(), edge in any::<u64>(), key in any::<u64>(), b in 1u64.., exp in any::<u64>(),
    ) {
        let ctx = context(field(edge), field(key), b, exp);
        let tag = binding(field(n), ctx);
        prop_assert_ne!(tag, binding(field(n) + Field::from(1u64), ctx));
        prop_assert_ne!(tag, binding(field(n), context(field(edge) + Field::from(1u64), field(key), b, exp)));
        prop_assert_ne!(tag, binding(field(n), context(field(edge), field(key) + Field::from(1u64), b, exp)));
        prop_assert_ne!(tag, binding(field(n), context(field(edge), field(key), b ^ 1, exp)));
        prop_assert_ne!(tag, binding(field(n), context(field(edge), field(key), b, exp ^ 1)));
    }
}

#[test]
fn the_value_generator_is_on_the_curve_and_is_not_the_curve_generator() {
    let g = value_generator();
    assert!(g.is_on_curve());
    assert_ne!(g, <connector_zk_note::Point as AffineRepr>::generator());
    assert!(!g.y.into_bigint().is_odd());
}

#[test]
fn tiers_are_a_validated_published_set() {
    assert_eq!(Tiers::new([]).unwrap_err(), TiersError::Empty);
    assert_eq!(Tiers::new([5, 0]).unwrap_err(), TiersError::Zero);
    assert_eq!(Tiers::new([5, 5]).unwrap_err(), TiersError::Duplicate);
    let t = Tiers::new([100, 10, 1000]).unwrap();
    let budgets: Vec<u64> = t.as_slice().iter().map(|t| t.budget()).collect();
    assert_eq!(budgets, [10, 100, 1000]);
    assert!(t.get(100).is_some());
    assert!(t.get(101).is_none());
}
