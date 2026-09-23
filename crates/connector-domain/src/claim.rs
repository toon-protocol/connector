//! Claim validation: nonce, watermark and value-binding rules (ADR 0004, ADR
//! 0005, `docs/protocol/peer-semantics-pre-868.md` §3.2-§3.5,
//! `docs/protocol/client-edge-spec.md` §1.3, issues #423, #522). Pure, no
//! I/O -- a claim's signature is chain-specific (peer-semantics-pre-868.md §3.5,
//! ADR 0024) and is both produced and verified in `connector-signer`
//! (`evm_balance_proof_digest`), which this crate deliberately has no
//! dependency on (ADR 0001): a digest computed over on-chain data belongs
//! next to the chain-specific code that produces and checks it, not here.
//! What this module owns is the rules every claim must satisfy regardless
//! of chain or signature scheme: its nonce must strictly advance the
//! payee's watermark, its cumulative amount must never decrease
//! (`CONTEXT.md` "Nonce", "Watermark"), and -- for a locally-terminated,
//! priced route -- it must advance value by at least that route's price
//! ([`validate_price`]). Deliberately cheaper than cryptographic
//! verification and run before it, so a replay or an underpayment never
//! spends a signature check.

use thiserror::Error;

/// The highest nonce and cumulative amount a payee has accepted on a
/// channel so far (`CONTEXT.md` "Watermark"). Absent (`None`) before any
/// claim has ever been accepted on that channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watermark {
    pub nonce: u64,
    pub cumulative_amount: u64,
}

impl Watermark {
    /// The higher of two watermarks on the **same** channel, taken field by
    /// field (issues #1257/#1258): the highest nonce and the highest
    /// cumulative amount either has seen.
    ///
    /// A cumulative amount is a property of the on-chain channel, not of
    /// the book that happened to journal it -- the chain settles one
    /// running total per direction, whichever of a node's two books
    /// accepted each claim. So when both books hold a watermark for one
    /// channel, the channel stands at the higher of the two, and a claim
    /// that advances past this answer advances past both. `None` on
    /// either side is simply the other side's answer.
    #[must_use]
    pub fn highest(this: Option<Self>, other: Option<Self>) -> Option<Self> {
        match (this, other) {
            (Some(a), Some(b)) => Some(Self {
                nonce: a.nonce.max(b.nonce),
                cumulative_amount: a.cumulative_amount.max(b.cumulative_amount),
            }),
            (a, b) => a.or(b),
        }
    }
}

/// Why a claim was rejected at the watermark layer -- mirrors
/// peer-semantics-pre-868.md §3.4's `nonce_not_advancing`/`amount_not_advancing`
/// CLAIM_ACK rejection reasons (`signature_invalid` and `unknown_channel`
/// are not this module's concern: they depend on a verification key and a
/// channel registry, neither of which is pure domain state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ClaimError {
    #[error("claim nonce {claimed} does not advance past the watermark's {watermark}")]
    NonceNotAdvancing { claimed: u64, watermark: u64 },

    #[error("claim amount {claimed} is less than the watermark's already-accepted {watermark}")]
    AmountNotAdvancing { claimed: u64, watermark: u64 },

    #[error(
        "claim advances value by {advanced}, less than the terminated route's price of {price}"
    )]
    Underpayment { advanced: u64, price: u64 },
}

/// Whether a claim of `nonce`/`cumulative_amount` may advance `watermark`.
/// A `None` watermark -- no claim ever accepted on this channel -- accepts
/// any nonce and amount as the channel's first watermark; there is nothing
/// yet for a first claim to fail to advance past.
pub fn validate_claim(
    watermark: Option<Watermark>,
    nonce: u64,
    cumulative_amount: u64,
) -> Result<(), ClaimError> {
    let Some(watermark) = watermark else {
        return Ok(());
    };
    if nonce <= watermark.nonce {
        return Err(ClaimError::NonceNotAdvancing {
            claimed: nonce,
            watermark: watermark.nonce,
        });
    }
    if cumulative_amount < watermark.cumulative_amount {
        return Err(ClaimError::AmountNotAdvancing {
            claimed: cumulative_amount,
            watermark: watermark.cumulative_amount,
        });
    }
    Ok(())
}

/// Whether a claim of `cumulative_amount` advances value past `watermark` by
/// at least `price` -- the value-binding step of `client-edge-spec.md` §1.3,
/// run after freshness ([`validate_claim`]) and before cryptographic
/// verification (issue #522): a minimal claim that merely advances the
/// nonce is not, by itself, worth anything, and this is the check that
/// stops it from buying a route it does not cover. `price` of `0` always
/// passes -- a route documented as deliberately free
/// (`connector_config::StaticRoute::price`) charges nothing and rejects
/// nothing here.
pub fn validate_price(
    watermark: Option<Watermark>,
    cumulative_amount: u64,
    price: u64,
) -> Result<(), ClaimError> {
    let prior = watermark.map_or(0, |watermark| watermark.cumulative_amount);
    let advanced = cumulative_amount.saturating_sub(prior);
    if advanced < price {
        return Err(ClaimError::Underpayment { advanced, price });
    }
    Ok(())
}

/// The watermark after accepting a claim of `nonce`/`cumulative_amount`.
/// Callers MUST have already checked [`validate_claim`] -- this does not
/// re-check, matching `condition.rs`'s split between
/// `fulfillment_matches_condition` (the check) and `derive_condition` (the
/// unconditional computation).
pub fn advance_watermark(nonce: u64, cumulative_amount: u64) -> Watermark {
    Watermark {
        nonce,
        cumulative_amount,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn a_first_claim_is_accepted_with_no_watermark_yet() {
        assert!(validate_claim(None, 1, 0).is_ok());
    }

    #[test]
    fn a_claim_with_the_same_nonce_as_the_watermark_is_rejected() {
        let watermark = Watermark {
            nonce: 5,
            cumulative_amount: 100,
        };
        let err = validate_claim(Some(watermark), 5, 200).unwrap_err();
        assert_eq!(
            err,
            ClaimError::NonceNotAdvancing {
                claimed: 5,
                watermark: 5
            }
        );
    }

    #[test]
    fn a_claim_with_a_lower_nonce_than_the_watermark_is_rejected() {
        let watermark = Watermark {
            nonce: 5,
            cumulative_amount: 100,
        };
        let err = validate_claim(Some(watermark), 4, 200).unwrap_err();
        assert_eq!(
            err,
            ClaimError::NonceNotAdvancing {
                claimed: 4,
                watermark: 5
            }
        );
    }

    #[test]
    fn a_higher_nonce_with_a_lower_amount_is_rejected() {
        let watermark = Watermark {
            nonce: 5,
            cumulative_amount: 100,
        };
        let err = validate_claim(Some(watermark), 6, 99).unwrap_err();
        assert_eq!(
            err,
            ClaimError::AmountNotAdvancing {
                claimed: 99,
                watermark: 100
            }
        );
    }

    #[test]
    fn a_higher_nonce_with_the_same_amount_is_accepted() {
        let watermark = Watermark {
            nonce: 5,
            cumulative_amount: 100,
        };
        assert!(validate_claim(Some(watermark), 6, 100).is_ok());
    }

    #[test]
    fn a_higher_nonce_with_a_higher_amount_is_accepted() {
        let watermark = Watermark {
            nonce: 5,
            cumulative_amount: 100,
        };
        assert!(validate_claim(Some(watermark), 6, 150).is_ok());
    }

    #[test]
    fn a_first_claim_advancing_by_exactly_the_price_is_accepted() {
        assert!(validate_price(None, 100, 100).is_ok());
    }

    #[test]
    fn a_first_claim_advancing_by_less_than_the_price_is_underpayment() {
        let err = validate_price(None, 99, 100).unwrap_err();
        assert_eq!(
            err,
            ClaimError::Underpayment {
                advanced: 99,
                price: 100
            }
        );
    }

    #[test]
    fn a_zero_price_route_accepts_a_claim_that_advances_nothing() {
        assert!(validate_price(
            Some(Watermark {
                nonce: 1,
                cumulative_amount: 100
            }),
            100,
            0
        )
        .is_ok());
    }

    #[test]
    fn value_binding_is_measured_against_the_watermark_not_the_raw_cumulative_amount() {
        let watermark = Watermark {
            nonce: 5,
            cumulative_amount: 100,
        };
        // Advances by only 40 -- below a price of 50 -- even though the
        // claim's own cumulative_amount (140) looks larger than the price.
        let err = validate_price(Some(watermark), 140, 50).unwrap_err();
        assert_eq!(
            err,
            ClaimError::Underpayment {
                advanced: 40,
                price: 50
            }
        );
        // Advancing by exactly the price is accepted.
        assert!(validate_price(Some(watermark), 150, 50).is_ok());
    }

    #[test]
    fn a_claim_advancing_by_more_than_the_price_is_accepted() {
        let watermark = Watermark {
            nonce: 5,
            cumulative_amount: 100,
        };
        assert!(validate_price(Some(watermark), 1_000, 50).is_ok());
    }

    #[test]
    fn advancing_the_watermark_records_exactly_the_accepted_claim() {
        let watermark = advance_watermark(7, 250);
        assert_eq!(
            watermark,
            Watermark {
                nonce: 7,
                cumulative_amount: 250
            }
        );
    }

    proptest! {
        /// The property the issue's acceptance criteria calls out by name:
        /// a watermark never moves backwards. Feed an arbitrary sequence of
        /// (nonce, amount) candidate claims through validate-then-advance
        /// exactly as a payee would -- rejecting anything that fails
        /// validation, applying anything that passes -- and check that the
        /// resulting watermark's nonce and amount are never smaller than
        /// they were a step ago, for every prefix of the sequence.
        #[test]
        fn a_watermark_never_moves_backwards(
            candidates in proptest::collection::vec((any::<u64>(), any::<u64>()), 0..64)
        ) {
            let mut watermark: Option<Watermark> = None;
            for (nonce, cumulative_amount) in candidates {
                let before = watermark;
                if validate_claim(watermark, nonce, cumulative_amount).is_ok() {
                    watermark = Some(advance_watermark(nonce, cumulative_amount));
                }
                if let (Some(before), Some(after)) = (before, watermark) {
                    prop_assert!(after.nonce >= before.nonce);
                    prop_assert!(after.cumulative_amount >= before.cumulative_amount);
                }
            }
        }

        /// A claim that validate_claim accepts always has a strictly
        /// greater nonce than any prior watermark -- the mechanism that
        /// makes replay gain nothing (`CONTEXT.md` "Nonce").
        #[test]
        fn an_accepted_claim_always_strictly_advances_the_nonce(
            watermark_nonce in any::<u64>(),
            watermark_amount in any::<u64>(),
            claim_nonce in any::<u64>(),
            claim_amount in any::<u64>(),
        ) {
            let watermark = Watermark { nonce: watermark_nonce, cumulative_amount: watermark_amount };
            if validate_claim(Some(watermark), claim_nonce, claim_amount).is_ok() {
                prop_assert!(claim_nonce > watermark_nonce);
                prop_assert!(claim_amount >= watermark_amount);
            }
        }

        /// Value binding accepts a claim exactly when its advance over the
        /// watermark (or over zero, with none yet) meets the price -- never
        /// less, regardless of how large the claim's own cumulative_amount
        /// looks in isolation.
        #[test]
        fn value_binding_accepts_iff_the_advance_meets_the_price(
            watermark in proptest::option::of((any::<u64>(), any::<u64>()).prop_map(|(nonce, cumulative_amount)| Watermark { nonce, cumulative_amount })),
            cumulative_amount in any::<u64>(),
            price in any::<u64>(),
        ) {
            let prior = watermark.map_or(0, |w| w.cumulative_amount);
            let advanced = cumulative_amount.saturating_sub(prior);
            let result = validate_price(watermark, cumulative_amount, price);
            prop_assert_eq!(result.is_ok(), advanced >= price);
        }

        /// Issues #1257/#1258: a claim that advances past
        /// [`Watermark::highest`] of two books' watermarks is accepted by
        /// **both** books -- so a payer seeded from the combined answer can
        /// never be refused "goes backwards" by whichever book it did not
        /// read, and a redeem never picks a claim the other book has
        /// already superseded.
        #[test]
        fn a_claim_past_the_highest_of_two_watermarks_passes_both(
            a in proptest::option::of((any::<u32>(), any::<u32>()).prop_map(|(nonce, cumulative_amount)| Watermark { nonce: nonce.into(), cumulative_amount: cumulative_amount.into() })),
            b in proptest::option::of((any::<u32>(), any::<u32>()).prop_map(|(nonce, cumulative_amount)| Watermark { nonce: nonce.into(), cumulative_amount: cumulative_amount.into() })),
            amount in any::<u32>(),
        ) {
            let highest = Watermark::highest(a, b);
            let (nonce, cumulative) = highest.map_or((0, 0), |w| (w.nonce, w.cumulative_amount));
            let claim_nonce = nonce + 1;
            let claim_amount = cumulative + u64::from(amount);
            prop_assert!(validate_claim(a, claim_nonce, claim_amount).is_ok());
            prop_assert!(validate_claim(b, claim_nonce, claim_amount).is_ok());
            prop_assert_eq!(Watermark::highest(a, b), Watermark::highest(b, a));
        }
    }
}
