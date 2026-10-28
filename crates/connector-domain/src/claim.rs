//! Claim validation: the voucher watermark and value-binding rules (ADR
//! 0074 decision 3, ADR 0075 decision 8, `docs/protocol/client-edge-spec.md`
//! §1.3). Pure, no I/O -- a voucher's signature is chain-specific and is
//! both produced and verified in `connector-signer`, which this crate
//! deliberately has no dependency on (ADR 0001).
//!
//! What this module owns is the rules every claim must satisfy regardless
//! of chain: a voucher's cumulative amount must strictly exceed the payee's
//! watermark, unless it is the very voucher that set it, resent
//! (`CONTEXT.md` "Watermark"), and -- for a priced route -- it must advance
//! value by at least that route's price ([`validate_price`]). Deliberately
//! cheaper than cryptographic verification and run before it, so a replay
//! or an underpayment never spends a signature check.
//!
//! **There is no nonce rule.** A `toon-channel` claim was ordered by a
//! nonce, which could advance at an unchanged amount; ADR 0075 retired that
//! claim scheme, and `Watermark { nonce, .. }`, `validate_claim` and
//! `advance_watermark` went with it (issue #1384). A voucher's amount alone
//! orders it.

use thiserror::Error;

/// The highest cumulative amount a payee has accepted on a channel so far
/// (`CONTEXT.md` "Watermark"). Absent (`None`) before any voucher has ever
/// been accepted on that channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watermark {
    /// `u128` (ADR 0074 decision 3, amended 2026-09-30 #1429): EVM's
    /// cumulative amount is `u128`; a Solana voucher's own `u64` amount is
    /// widened into this without loss.
    pub cumulative_amount: u128,
}

/// Why a claim was rejected at the watermark layer (`signature_invalid`
/// and `unknown_channel` are not this module's concern: they depend on a
/// verification key and a channel record, neither of which is pure domain
/// state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ClaimError {
    #[error("voucher amount {claimed} does not strictly exceed the watermark's {watermark}")]
    AmountNotAdvancing { claimed: u128, watermark: u128 },

    #[error(
        "claim advances value by {advanced}, less than the terminated route's price of {price}"
    )]
    Underpayment { advanced: u128, price: u64 },

    /// An allowance claim above its allowance's budget `b` (ADR 0080
    /// decision 2): the cap is on the cumulative amount signed.
    #[error("allowance claim amount {claimed} exceeds the allowance's budget of {budget}")]
    OverBudget { claimed: u128, budget: u64 },
}

/// Whether a claim of `cumulative_amount` advances value past `watermark` by
/// at least `price` -- the value-binding step of `client-edge-spec.md` §1.3,
/// run after freshness ([`validate_voucher`]) and before cryptographic
/// verification (issue #522). `price` of `0` always passes -- a route
/// documented as deliberately free (`connector_config::StaticRoute::price`)
/// charges nothing and rejects nothing here.
///
/// `cumulative_amount` is `u128` (ADR 0074 decision 3, amended 2026-09-30
/// #1429); `price` stays the per-packet `u64` ADR 0071 already fixed --
/// only the channel's cumulative total widened, not a route's price. The
/// comparison runs in `u128`, so it cannot overflow or wrap at the `u64`
/// boundary.
pub fn validate_price(
    watermark: Option<Watermark>,
    cumulative_amount: u128,
    price: u64,
) -> Result<(), ClaimError> {
    let prior = watermark.map_or(0, |watermark| watermark.cumulative_amount);
    let advanced = cumulative_amount.saturating_sub(prior);
    if advanced < u128::from(price) {
        return Err(ClaimError::Underpayment { advanced, price });
    }
    Ok(())
}

/// Whether a peer arrival at a **payment route** (ADR 0083) is covered: its
/// claim advances the channel's watermark by at least the packet's own
/// `amount` ([`validate_price`] with the amount as the figure, as ADR 0042
/// item 3 measures a forwarded arrival) and the amount is at least the route's
/// `charge` at this payload. An `amount` under the charge is ADR 0029's `F03`
/// whatever the claim covers, so the claim never buys a packet the route would
/// refuse.
pub fn payment_route_covers(
    watermark: Option<Watermark>,
    cumulative_amount: u128,
    amount: u64,
    charge: u64,
) -> bool {
    amount >= charge && validate_price(watermark, cumulative_amount, amount).is_ok()
}

/// The `nonce` every voucher's [`crate::JournalEntry::InboundClaimAccepted`]
/// carries (ADR 0074 decision 3).
///
/// A voucher has no nonce: its cumulative amount alone orders it. The
/// journal entry's `nonce` field predates vouchers and is kept so an older
/// journal still decodes; a voucher's entry holds this constant there, and
/// nothing reads it back (ADR 0075 decision 8, issue #1384).
pub const VOUCHER_WATERMARK_NONCE: u64 = 0;

/// What a voucher's freshness is judged against: the cumulative amount of
/// the voucher that set the channel's watermark, and that voucher's
/// signature bytes -- the two things a byte-identical retransmission must
/// match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoucherWatermark<'a> {
    pub cumulative_amount: u128,
    pub signature: &'a [u8],
}

/// A voucher [`validate_voucher`] admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoucherAdmission {
    /// The voucher strictly advances the watermark, by `advanced`, and that
    /// advance covers the charge. Accepting it advances and journals the
    /// watermark ([`advance_voucher_watermark`]).
    Advances { advanced: u128 },
    /// The voucher is byte-identical to the one that set the watermark: a
    /// retransmission, not a new claim. It is not an error and it buys
    /// nothing new, so nothing advances and nothing is recorded. Returned
    /// only where the charge is zero: something that buys nothing cannot
    /// cover a charge.
    Retransmission,
}

/// client-edge-spec.md §1.3 steps 2 and 3 for a **voucher** (ADR 0074
/// decision 3): freshness without a nonce, then value binding.
///
/// * A voucher byte-identical to the one at `watermark` -- same amount,
///   same signature bytes -- is [`VoucherAdmission::Retransmission`] where
///   `charge` is zero, and [`ClaimError::Underpayment`] (advancing by `0`)
///   where it is not. Byte identity is the test because an equal amount
///   under a different signature is not the same voucher.
/// * Otherwise it must **strictly** exceed the watermark's amount, else
///   [`ClaimError::AmountNotAdvancing`] -- which for a voucher means _not
///   strictly greater_.
/// * The advance must then be at least `charge`, else
///   [`ClaimError::Underpayment`].
///
/// A `None` watermark is a channel never paid on: the advance is measured
/// from zero, so even a first voucher must name more than zero. Pure, and
/// cheap enough to run before any signature is checked -- a replay or an
/// underpayment never pays for cryptography. It is the only freshness rule
/// (ADR 0075 decision 8).
pub fn validate_voucher(
    watermark: Option<VoucherWatermark<'_>>,
    cumulative_amount: u128,
    signature: &[u8],
    charge: u64,
) -> Result<VoucherAdmission, ClaimError> {
    let prior = watermark.map_or(0, |watermark| watermark.cumulative_amount);
    let identical = watermark.is_some_and(|watermark| {
        watermark.cumulative_amount == cumulative_amount && watermark.signature == signature
    });
    if identical {
        return if charge == 0 {
            Ok(VoucherAdmission::Retransmission)
        } else {
            Err(ClaimError::Underpayment {
                advanced: 0,
                price: charge,
            })
        };
    }
    if cumulative_amount <= prior {
        return Err(ClaimError::AmountNotAdvancing {
            claimed: cumulative_amount,
            watermark: prior,
        });
    }
    let advanced = cumulative_amount - prior;
    if advanced < u128::from(charge) {
        return Err(ClaimError::Underpayment {
            advanced,
            price: charge,
        });
    }
    Ok(VoucherAdmission::Advances { advanced })
}

/// An **allowance claim** under a voucher's rules (ADR 0080 decision 2):
/// exactly [`validate_voucher`] -- strictly above the watermark, an advance
/// covering `charge`, a byte-identical resend at no charge buying nothing --
/// and then a cumulative amount no greater than the allowance's `budget`,
/// else [`ClaimError::OverBudget`]. Pure, and run before the session key's
/// signature is checked, so a stale or over-budget claim never pays for
/// one.
pub fn validate_allowance_claim(
    watermark: Option<VoucherWatermark<'_>>,
    cumulative_amount: u128,
    signature: &[u8],
    charge: u64,
    budget: u64,
) -> Result<VoucherAdmission, ClaimError> {
    let admission = validate_voucher(watermark, cumulative_amount, signature, charge)?;
    if cumulative_amount > u128::from(budget) {
        return Err(ClaimError::OverBudget {
            claimed: cumulative_amount,
            budget,
        });
    }
    Ok(admission)
}

/// **Δ**, the charge an allowance's close journals (ADR 0080 decisions 2
/// and 4): its final watermark, once every forward in flight has resolved
/// and every uncarried one rolled back -- `0` when no claim stands. A
/// watermark above the budget is refused rather than charged: admission
/// never leaves one there, so one that is there was not admitted, and the
/// successor it would sign would take more than the proof granted.
pub fn allowance_close_charge(
    final_watermark: Option<u128>,
    budget: u64,
) -> Result<u64, ClaimError> {
    let claimed = final_watermark.unwrap_or(0);
    u64::try_from(claimed)
        .ok()
        .filter(|delta| *delta <= budget)
        .ok_or(ClaimError::OverBudget { claimed, budget })
}

/// The watermark after accepting a voucher of `cumulative_amount`: the
/// amount. Callers MUST have already checked [`validate_voucher`]; this does
/// not re-check.
pub fn advance_voucher_watermark(cumulative_amount: u128) -> Watermark {
    Watermark { cumulative_amount }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // -- Allowance claims (ADR 0080 decision 2, issue #1522) --

    #[test]
    fn an_allowance_claim_at_its_budget_is_admitted_and_one_above_it_is_not() {
        assert_eq!(
            validate_allowance_claim(None, 100, b"s", 100, 100),
            Ok(VoucherAdmission::Advances { advanced: 100 })
        );
        assert_eq!(
            validate_allowance_claim(None, 101, b"s", 0, 100),
            Err(ClaimError::OverBudget {
                claimed: 101,
                budget: 100
            })
        );
    }

    #[test]
    fn an_allowance_claim_is_held_to_the_voucher_rules_first() {
        let at = voucher_watermark(50, b"a");
        assert_eq!(
            validate_allowance_claim(at, 50, b"b", 0, 100),
            Err(ClaimError::AmountNotAdvancing {
                claimed: 50,
                watermark: 50
            })
        );
        assert_eq!(
            validate_allowance_claim(at, 60, b"b", 20, 100),
            Err(ClaimError::Underpayment {
                advanced: 10,
                price: 20
            })
        );
        assert_eq!(
            validate_allowance_claim(at, 50, b"a", 0, 100),
            Ok(VoucherAdmission::Retransmission)
        );
    }

    proptest! {
        /// Tier 1: whatever sequence of claims arrives, the watermark an
        /// allowance's admitted claims leave only ever rises, and never
        /// exceeds the budget.
        #[test]
        fn an_allowance_watermark_rises_and_never_exceeds_its_budget(
            budget in 1u64..10_000,
            claims in proptest::collection::vec((0u128..12_000, 0u64..50), 1..40),
        ) {
            let mut watermark: Option<u128> = None;
            for (amount, charge) in claims {
                let signature = amount.to_le_bytes();
                let prior = watermark.map(|w| VoucherWatermark {
                    cumulative_amount: w,
                    signature: &[],
                });
                if let Ok(VoucherAdmission::Advances { .. }) =
                    validate_allowance_claim(prior, amount, &signature, charge, budget)
                {
                    prop_assert!(watermark.is_none_or(|w| amount > w));
                    watermark = Some(advance_voucher_watermark(amount).cumulative_amount);
                }
                prop_assert!(watermark.is_none_or(|w| w <= u128::from(budget)));
            }
        }

        /// Tier 1 (ADR 0080 decisions 2 and 4, #1523): admit claims and have
        /// the next hop reject some of them, rolling each rejected one back
        /// to the watermark it advanced from, and the charge the close
        /// journals is the final watermark -- zero if nothing stood -- and
        /// never more than the budget.
        #[test]
        fn the_close_charges_the_final_watermark_and_never_more_than_the_budget(
            budget in 1u64..10_000,
            claims in proptest::collection::vec((0u128..12_000, 0u64..50, any::<bool>()), 1..40),
        ) {
            let mut watermark: Option<u128> = None;
            for (amount, charge, rejected) in claims {
                let prior = watermark.map(|w| VoucherWatermark {
                    cumulative_amount: w,
                    signature: &[],
                });
                if let Ok(VoucherAdmission::Advances { .. }) =
                    validate_allowance_claim(prior, amount, b"s", charge, budget)
                {
                    if !rejected {
                        watermark = Some(amount);
                    }
                }
            }
            let delta = allowance_close_charge(watermark, budget).expect("within budget");
            prop_assert_eq!(u128::from(delta), watermark.unwrap_or(0));
            prop_assert!(delta <= budget);
        }
    }

    #[test]
    fn a_close_charge_above_the_budget_is_refused_rather_than_signed() {
        assert_eq!(allowance_close_charge(None, 10), Ok(0));
        assert_eq!(allowance_close_charge(Some(10), 10), Ok(10));
        assert_eq!(
            allowance_close_charge(Some(11), 10),
            Err(ClaimError::OverBudget {
                claimed: 11,
                budget: 10
            })
        );
    }

    // -- Vouchers (ADR 0074 decision 3, issue #1341) --

    fn voucher_watermark(amount: u128, signature: &[u8]) -> Option<VoucherWatermark<'_>> {
        Some(VoucherWatermark {
            cumulative_amount: amount,
            signature,
        })
    }

    #[test]
    fn a_first_voucher_naming_more_than_zero_advances_from_zero() {
        assert_eq!(
            validate_voucher(None, 5, b"sig", 5),
            Ok(VoucherAdmission::Advances { advanced: 5 })
        );
    }

    #[test]
    fn a_first_voucher_naming_zero_does_not_advance() {
        assert_eq!(
            validate_voucher(None, 0, b"sig", 0),
            Err(ClaimError::AmountNotAdvancing {
                claimed: 0,
                watermark: 0
            })
        );
    }

    /// The case ADR 0074 decision 7 names for the vectors: an equal amount
    /// is refused (under a different signature), a higher one accepted.
    #[test]
    fn an_equal_amount_under_a_different_signature_is_not_advancing() {
        assert_eq!(
            validate_voucher(voucher_watermark(100, b"first"), 100, b"second", 0),
            Err(ClaimError::AmountNotAdvancing {
                claimed: 100,
                watermark: 100
            })
        );
        assert_eq!(
            validate_voucher(voucher_watermark(100, b"first"), 101, b"second", 0),
            Ok(VoucherAdmission::Advances { advanced: 1 })
        );
    }

    #[test]
    fn a_lower_amount_is_not_advancing() {
        assert_eq!(
            validate_voucher(voucher_watermark(100, b"first"), 99, b"older", 0),
            Err(ClaimError::AmountNotAdvancing {
                claimed: 99,
                watermark: 100
            })
        );
    }

    #[test]
    fn a_byte_identical_voucher_at_the_watermark_is_a_retransmission() {
        assert_eq!(
            validate_voucher(voucher_watermark(100, b"same"), 100, b"same", 0),
            Ok(VoucherAdmission::Retransmission)
        );
    }

    /// A retransmission buys nothing new, so it cannot pay for a packet that
    /// costs something -- otherwise one voucher, resent, would pay for every
    /// packet after it.
    #[test]
    fn a_retransmission_covers_no_charge() {
        assert_eq!(
            validate_voucher(voucher_watermark(100, b"same"), 100, b"same", 1),
            Err(ClaimError::Underpayment {
                advanced: 0,
                price: 1
            })
        );
    }

    #[test]
    fn a_voucher_advance_must_cover_the_charge() {
        assert_eq!(
            validate_voucher(voucher_watermark(100, b"a"), 140, b"b", 50),
            Err(ClaimError::Underpayment {
                advanced: 40,
                price: 50
            })
        );
        assert_eq!(
            validate_voucher(voucher_watermark(100, b"a"), 150, b"b", 50),
            Ok(VoucherAdmission::Advances { advanced: 50 })
        );
    }

    #[test]
    fn a_voucher_watermark_is_its_amount() {
        assert_eq!(
            advance_voucher_watermark(250),
            Watermark {
                cumulative_amount: 250
            }
        );
    }

    proptest! {
        /// ADR 0074 decision 3's property: an admitted voucher either
        /// strictly exceeds the watermark by at least the charge, or is the
        /// very voucher that set it, resent at no charge. Nothing else.
        #[test]
        fn an_admitted_voucher_strictly_advances_or_is_the_same_bytes_for_free(
            watermark_amount in any::<u128>(),
            watermark_signature in proptest::collection::vec(any::<u8>(), 0..4),
            amount in any::<u128>(),
            signature in proptest::collection::vec(any::<u8>(), 0..4),
            charge in any::<u64>(),
            has_watermark in any::<bool>(),
        ) {
            let watermark = has_watermark.then_some(VoucherWatermark {
                cumulative_amount: watermark_amount,
                signature: &watermark_signature,
            });
            let prior = watermark.map_or(0, |w| w.cumulative_amount);
            match validate_voucher(watermark, amount, &signature, charge) {
                Ok(VoucherAdmission::Advances { advanced }) => {
                    prop_assert!(amount > prior);
                    prop_assert_eq!(advanced, amount - prior);
                    prop_assert!(advanced >= u128::from(charge));
                }
                Ok(VoucherAdmission::Retransmission) => {
                    prop_assert!(has_watermark);
                    prop_assert_eq!(amount, watermark_amount);
                    prop_assert_eq!(&signature, &watermark_signature);
                    prop_assert_eq!(charge, 0);
                }
                Err(ClaimError::AmountNotAdvancing { claimed, watermark }) => {
                    prop_assert!(amount <= prior);
                    prop_assert_eq!((claimed, watermark), (amount, prior));
                }
                Err(ClaimError::Underpayment { advanced, price }) => {
                    prop_assert_eq!(price, charge);
                    prop_assert!(advanced < u128::from(charge));
                }
                Err(ClaimError::OverBudget { .. }) => {
                    prop_assert!(false, "a voucher has no budget");
                }
            }
        }

        /// Replay gains nothing: fold any sequence of vouchers -- drawn from
        /// a small alphabet so repeats, resends and equal amounts actually
        /// occur -- through validate-then-advance exactly as a payee would.
        /// The watermark never moves backwards, a resend never moves it, and
        /// the total charged never exceeds the amount the watermark reached.
        #[test]
        fn replaying_vouchers_never_moves_the_watermark_back_or_pays_twice(
            candidates in proptest::collection::vec(
                (0u128..32, proptest::collection::vec(0u8..2, 1..2), 0u64..4),
                0..64,
            )
        ) {
            let mut current: Option<(u128, Vec<u8>)> = None;
            let mut paid = 0u128;
            for (amount, signature, charge) in candidates {
                let before = current.as_ref().map(|(amount, _)| *amount);
                let watermark = current.as_ref().map(|(amount, signature)| VoucherWatermark {
                    cumulative_amount: *amount,
                    signature,
                });
                if let Ok(VoucherAdmission::Advances { .. }) =
                    validate_voucher(watermark, amount, &signature, charge)
                {
                    paid += u128::from(charge);
                    current = Some((advance_voucher_watermark(amount).cumulative_amount, signature));
                }
                let after = current.as_ref().map(|(amount, _)| *amount);
                if let (Some(before), Some(after)) = (before, after) {
                    prop_assert!(after >= before);
                }
                prop_assert!(paid <= after.unwrap_or(0));
            }
        }

        /// ADR 0074 decision 3, amended 2026-09-30 (#1429): the `u64`
        /// boundary that used to be the ceiling is now just a point in the
        /// middle of the range. A watermark just below it, then a voucher
        /// just above it, must still have its delta computed exactly in
        /// `u128` and compared against the price without overflowing or
        /// wrapping.
        #[test]
        fn the_delta_across_the_old_u64_ceiling_is_exact(
            below in 0u128..=1000,
            above in 1u128..=1000,
            price in prop_oneof![0u64..=2001, any::<u64>()],
        ) {
            let watermark_amount = u128::from(u64::MAX) - below;
            let cumulative_amount = u128::from(u64::MAX) + above;
            let watermark = Watermark { cumulative_amount: watermark_amount };
            let advanced = cumulative_amount - watermark_amount;
            let result = validate_price(Some(watermark), cumulative_amount, price);
            prop_assert_eq!(result.is_ok(), advanced >= u128::from(price));
            if let Err(ClaimError::Underpayment { advanced: reported, price: reported_price }) = result {
                prop_assert_eq!(reported, advanced);
                prop_assert_eq!(reported_price, price);
            }

            // The voucher path proper: freshness against the amount-only
            // watermark, then value, across the same boundary.
            let admission = validate_voucher(
                voucher_watermark(watermark_amount, &[0x01]),
                cumulative_amount,
                &[0x02],
                price,
            );
            if advanced >= u128::from(price) {
                prop_assert_eq!(admission, Ok(VoucherAdmission::Advances { advanced }));
            } else {
                prop_assert_eq!(admission, Err(ClaimError::Underpayment { advanced, price }));
            }
            // And the other way across it: a voucher at or below a
            // watermark above `u64::MAX` is stale, whatever it pays.
            prop_assert_eq!(
                validate_voucher(
                    voucher_watermark(cumulative_amount, &[0x01]),
                    watermark_amount,
                    &[0x02],
                    0,
                ),
                Err(ClaimError::AmountNotAdvancing {
                    claimed: watermark_amount,
                    watermark: cumulative_amount,
                })
            );
        }
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
            cumulative_amount: 100,
        };
        assert!(validate_price(Some(watermark), 1_000, 50).is_ok());
    }

    proptest! {
        /// Value binding accepts a claim exactly when its advance over the
        /// watermark (or over zero, with none yet) meets the price -- never
        /// less, regardless of how large the claim's own cumulative_amount
        /// looks in isolation.
        #[test]
        fn value_binding_accepts_iff_the_advance_meets_the_price(
            watermark in proptest::option::of(any::<u128>().prop_map(|cumulative_amount| Watermark { cumulative_amount })),
            cumulative_amount in any::<u128>(),
            price in any::<u64>(),
        ) {
            let prior = watermark.map_or(0, |w| w.cumulative_amount);
            let advanced = cumulative_amount.saturating_sub(prior);
            let result = validate_price(watermark, cumulative_amount, price);
            prop_assert_eq!(result.is_ok(), advanced >= u128::from(price));
        }

        /// ADR 0083: at a payment route an arrival is admitted exactly when
        /// the claim's advance covers the packet's own amount AND the amount
        /// covers the charge -- a claim that covers only the charge buys
        /// nothing above it, and an amount under the charge is never admitted.
        #[test]
        fn a_payment_route_admits_iff_the_advance_covers_the_amount_and_the_amount_the_charge(
            watermark in proptest::option::of(any::<u128>().prop_map(|cumulative_amount| Watermark { cumulative_amount })),
            cumulative_amount in any::<u128>(),
            amount in any::<u64>(),
            charge in any::<u64>(),
        ) {
            let prior = watermark.map_or(0, |w| w.cumulative_amount);
            let advanced = cumulative_amount.saturating_sub(prior);
            prop_assert_eq!(
                payment_route_covers(watermark, cumulative_amount, amount, charge),
                advanced >= u128::from(amount) && amount >= charge,
            );
        }

        #[test]
        fn a_claim_covering_only_the_charge_is_refused_above_it(
            prior in 0u128..u128::from(u64::MAX),
            charge in 0u64..1_000_000,
            extra in 1u64..1_000_000,
        ) {
            let watermark = Some(Watermark { cumulative_amount: prior });
            let amount = charge + extra;
            prop_assert!(!payment_route_covers(
                watermark,
                prior + u128::from(charge),
                amount,
                charge,
            ));
            prop_assert!(payment_route_covers(
                watermark,
                prior + u128::from(amount),
                amount,
                charge,
            ));
        }
    }
}
