//! Properties of what this crate owns: the allowance claim's signature and the tiers.

use connector_zk_note::note::policy::TIERS;
use connector_zk_note::session::{
    sign_allowance_claim, signature_from_be_bytes, signature_to_hex, verify_allowance_claim,
    SIGNING_NETWORK,
};
use connector_zk_note::tiers::{Tiers, TiersError};
use connector_zk_note::{Keypair, NetworkId, Scalar, SecKey};
use proptest::prelude::*;

fn session(seed: u64) -> Keypair {
    Keypair::try_from(SecKey::new(Scalar::from(seed + 1))).expect("a keypair")
}

fn hex_to_bytes(hex: &str) -> [u8; 64] {
    let mut out = [0u8; 64];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex");
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// A claim the session key signed verifies, and verifies only for its own id and amount
    /// and under its own key.
    #[test]
    fn an_allowance_claim_verifies_only_as_signed(
        seed in 0u64..1_000_000,
        id in any::<[u8; 16]>(),
        amount in any::<u64>(),
        other_amount in any::<u64>(),
    ) {
        let key = session(seed);
        let signature = sign_allowance_claim(&key, &id, amount);
        let public = key.public.into_compressed();
        prop_assert!(verify_allowance_claim(&signature, &public, &id, amount));
        if other_amount != amount {
            prop_assert!(!verify_allowance_claim(&signature, &public, &id, other_amount));
        }
        let mut other_id = id;
        other_id[0] ^= 1;
        prop_assert!(!verify_allowance_claim(&signature, &public, &other_id, amount));
        let stranger = session(seed + 1).public.into_compressed();
        prop_assert!(!verify_allowance_claim(&signature, &stranger, &id, amount));
    }

    /// The wire spelling of a signature reads back as the same signature.
    #[test]
    fn a_signature_round_trips_its_wire_spelling(seed in 0u64..1_000_000, amount in any::<u64>()) {
        let signature = sign_allowance_claim(&session(seed), &[7; 16], amount);
        let hex = signature_to_hex(&signature);
        prop_assert_eq!(hex.len(), 128);
        prop_assert_eq!(signature_from_be_bytes(&hex_to_bytes(&hex)), Some(signature));
    }
}

#[test]
fn an_allowance_claim_is_signed_under_the_devnet_domain() {
    // The mainnet refusal itself is pinned by zk_toon's vectors (tests/zk_toon_parity.rs).
    assert_eq!(SIGNING_NETWORK, NetworkId::TESTNET);
}

#[test]
fn a_signature_half_outside_its_field_is_refused() {
    assert_eq!(signature_from_be_bytes(&[0xff; 64]), None);
}

#[test]
fn tiers_are_a_validated_subset_of_zk_toons() {
    assert_eq!(Tiers::new([]).unwrap_err(), TiersError::Empty);
    assert_eq!(Tiers::new([TIERS[0], 0]).unwrap_err(), TiersError::Zero);
    assert_eq!(
        Tiers::new([TIERS[0], TIERS[0]]).unwrap_err(),
        TiersError::Duplicate
    );
    assert_eq!(
        Tiers::new([TIERS[1], 12_345]).unwrap_err(),
        TiersError::NotPublished(12_345)
    );
    assert!(TiersError::NotPublished(12_345)
        .to_string()
        .contains("12345"));
    let t = Tiers::new([TIERS[2], TIERS[0]]).unwrap();
    let budgets: Vec<u64> = t.as_slice().iter().map(|t| t.budget()).collect();
    assert_eq!(budgets, [TIERS[0], TIERS[2]]);
    assert!(t.get(TIERS[0]).is_some());
    assert!(t.get(TIERS[1]).is_none());
}

// -- Closing an allowance and its successor (ADR 0080 decision 4, #1523) --

mod close_and_successor {
    use super::*;
    use connector_zk_note::note::balance::commit;
    use connector_zk_note::note::state::StateMessage;
    use connector_zk_note::session::{sign_allowance_close, verify_allowance_close};
    use connector_zk_note::successor::successor_state;
    use connector_zk_note::Field;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(8))]

        /// A close request verifies only for the allowance it names and under its own session
        /// key, and a claim's signature is never a close's.
        #[test]
        fn a_close_request_verifies_only_as_signed(seed in 0u64..1_000_000, id in any::<[u8; 16]>()) {
            let key = session(seed);
            let public = key.public.into_compressed();
            let close = sign_allowance_close(&key, &id);
            prop_assert!(verify_allowance_close(&close, &public, &id));
            let mut other = id;
            other[15] ^= 1;
            prop_assert!(!verify_allowance_close(&close, &public, &other));
            let stranger = session(seed + 1).public.into_compressed();
            prop_assert!(!verify_allowance_close(&close, &stranger, &id));
            for amount in [0u64, 1] {
                let claim = sign_allowance_claim(&key, &id, amount);
                prop_assert!(!verify_allowance_close(&claim, &public, &id));
            }
        }

        /// The successor of `commit(B, r)` charged Δ is `commit(B − Δ, r)` at `next_anchor(N)`,
        /// and its message is zk_toon's state message under the state key.
        #[test]
        fn a_successor_is_the_old_balance_less_the_charge_under_the_state_key(
            balance in 1u64..1_000_000_000_000,
            blinding in 1u64..u64::MAX,
            charge_seed in any::<u64>(),
            nullifier in any::<u64>(),
        ) {
            let charge = charge_seed % balance;
            let state = session(9_999).public.into_compressed();
            let before = commit(Scalar::from(balance), Scalar::from(blinding));
            let next = successor_state(&state, before, Field::from(nullifier), charge)
                .expect("signable");
            prop_assert_eq!(
                next.commitment,
                commit(Scalar::from(balance - charge), Scalar::from(blinding))
            );
            prop_assert_eq!(
                next.anchor,
                connector_zk_note::note::next_anchor(Field::from(nullifier))
            );
            prop_assert_eq!(
                next.message.to_fields(),
                StateMessage::new(&state, next.commitment, next.anchor).to_fields()
            );
        }
    }
}
