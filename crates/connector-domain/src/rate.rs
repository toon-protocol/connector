//! The terms one connector crosses one denomination boundary on
//! ([ADR 0071](../../../docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md),
//! issue #1288): a **rational over base units**, and nothing else.
//!
//! `numerator` outgoing base units for every `denominator` incoming ones. Both
//! `u64`, both private, and the whole scale difference between the two tokens
//! folded into the ratio -- 6-decimals USDC to 18-decimals ANYONE at one USDC
//! per four ANYONE is `4_000_000_000_000 / 1`, not a price of 4 with a
//! `decimals` correction applied somewhere else. `[settlement] decimals` stays
//! exactly what it is today, a boot-time assertion against the chain, and never
//! becomes an input to value arithmetic (decision 4).
//!
//! # What is deliberately absent
//!
//! **There is no `Rate::IDENTITY` and no `Rate::ONE`.** A 1:1 rate is not a
//! constant this type withholds as a matter of taste; it is a value that must
//! not exist, because ADR 0071 decision 2 makes silent conversion structurally
//! impossible by making *absence* the refusal. A pair with no declared rate is
//! refused with a reject. If a 1:1 rate were nameable, the fix for "this pair
//! has no rate" would be to write one down, and an unconverted 18-vs-6-decimals
//! pass-through -- a 10^12x error, the single failure this record exists to
//! prevent -- would be one plausible-looking config line away.
//!
//! A rate is also not [`Default`], for the same reason: a defaulted rate is a
//! 1:1 rate wearing a different name.
//!
//! **There are no floats.** Not on the conversion path, not in a helper, not in
//! a `Display`. That is ADR 0071's own first falsifier, and
//! `tests/the_absences_adr_0071_relies_on.rs` fails the build on one rather
//! than leaving it to review, and on a 1:1 constant besides.
//!
//! # One value, one spelling
//!
//! `2/4` and `1/2` are the same terms, so [`Rate::new`] reduces by the greatest
//! common divisor and they are the same *value*. That is what lets [`Eq`],
//! [`Hash`] and [`Ord`] agree: a rate table comparing a refreshed observation
//! against the one it holds (ADR 0071 decision 5's `max_move`) is comparing
//! terms, not spellings, and two rates that order as equal must also be equal.

use std::cmp::Ordering;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// How many of the outgoing channel's base units one connector's own
/// declaration says the incoming channel's base units buy.
///
/// One connector's posted term, never a network fact -- the network learns it
/// the way it learns every cost, by probing the route.
///
/// [`Copy`], because it is two `u64`s and every consumer passes it by value
/// into arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawRate")]
pub struct Rate {
    numerator: u64,
    denominator: u64,
}

impl Rate {
    /// `numerator` outgoing base units per `denominator` incoming ones,
    /// reduced to lowest terms.
    ///
    /// Refuses a zero **denominator**: there is no rational with one, and a
    /// rate that could hold one would push the division-by-zero out of
    /// construction and onto the packet path, where the only remaining move is
    /// a panic. Refuses a zero **numerator** for the reason it is not obvious:
    /// ADR 0071 decision 7 makes the reject path the exact *inverse* of the
    /// forward, `ceil((cost + fee) / rate)`, so a rate this connector cannot
    /// invert is a rate whose probe cannot be answered. A rate that buys
    /// nothing is not a cheap rate; it is not a rate.
    ///
    /// This is the **only** way to make one, which is what "a zero denominator
    /// is unconstructable" means: the fields are private and there is no other
    /// door.
    pub fn new(numerator: u64, denominator: u64) -> Result<Rate, RateError> {
        if denominator == 0 {
            return Err(RateError::ZeroDenominator { numerator });
        }
        if numerator == 0 {
            return Err(RateError::ZeroNumerator { denominator });
        }
        let divisor = gcd(numerator, denominator);
        Ok(Rate {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    /// The outgoing side of the ratio, in lowest terms.
    pub const fn numerator(&self) -> u64 {
        self.numerator
    }

    /// The incoming side of the ratio, in lowest terms. Never zero.
    pub const fn denominator(&self) -> u64 {
        self.denominator
    }

    /// The same terms read the other way: what the outgoing unit buys of the
    /// incoming one.
    ///
    /// Total, and infallible by construction -- both fields are non-zero, so
    /// swapping them is again a rate. This is how ADR 0071 decision 3's cross
    /// rate composes through the numeraire (`X -> Y = (X/numeraire) /
    /// (Y/numeraire)`) without a second arithmetic.
    pub const fn inverted(&self) -> Rate {
        // Already in lowest terms, and gcd is symmetric, so the swap needs no
        // second reduction.
        Rate {
            numerator: self.denominator,
            denominator: self.numerator,
        }
    }
}

impl Ord for Rate {
    /// By value, cross-multiplied through `u128` -- `1/3 < 1/2` even though
    /// `3 > 2`, which is exactly what a field-by-field comparison would get
    /// wrong. `u64 * u64` always fits a `u128`, so this is total.
    fn cmp(&self, other: &Rate) -> Ordering {
        let mine = u128::from(self.numerator) * u128::from(other.denominator);
        let theirs = u128::from(other.numerator) * u128::from(self.denominator);
        mine.cmp(&theirs)
    }
}

impl PartialOrd for Rate {
    fn partial_cmp(&self, other: &Rate) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Rate {
    /// `4000000000000/1` -- both halves, always, because the pair is the
    /// declaration and a rate printed as one number would be a number in no
    /// unit.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

/// A rate as an operator writes one: `{ numerator = ..., denominator = ... }`.
///
/// Its only job is to give [`Rate`]'s refusals a way to reach config load, so
/// a zero denominator in a static rate row is refused at boot, in this
/// module's own words, rather than deserialized into a value that cannot
/// exist. `deny_unknown_fields` for ADR 0009's standing reason.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRate {
    numerator: u64,
    denominator: u64,
}

impl TryFrom<RawRate> for Rate {
    type Error = RateError;

    fn try_from(raw: RawRate) -> Result<Rate, RateError> {
        Rate::new(raw.numerator, raw.denominator)
    }
}

/// What can be wrong with a declared rate. Both variants name the half that
/// was written, so an operator reading a boot refusal can find the row.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RateError {
    /// No rational has a zero denominator.
    #[error("a rate's denominator is never zero, and '{numerator}/0' is not a rate")]
    ZeroDenominator { numerator: u64 },

    /// A rate that buys nothing cannot be inverted, and a reject crossing this
    /// boundary must invert it.
    #[error(
        "a rate's numerator is never zero, and '0/{denominator}' buys nothing in the outgoing \
         unit -- a rate a reject cannot convert a cost back through is not a rate"
    )]
    ZeroNumerator { denominator: u64 },
}

/// Euclid, on `u64`. `b` is non-zero at every call site, so this never
/// answers zero and never divides by one.
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// One 6-decimals USDC base unit in 18-decimals ANYONE base units, at
    /// four ANYONE to the USDC: `4 * 10^12` outgoing per incoming.
    const USDC_TO_ANYONE: u64 = 4_000_000_000_000;

    fn rate(numerator: u64, denominator: u64) -> Rate {
        Rate::new(numerator, denominator).expect("a rate with two non-zero halves")
    }

    #[test]
    fn a_zero_denominator_is_refused_by_name() {
        let error = Rate::new(3, 0).expect_err("there is no rational over zero");
        assert_eq!(error, RateError::ZeroDenominator { numerator: 3 });
        assert!(error.to_string().contains("denominator"), "got: {error}");
    }

    #[test]
    fn a_zero_numerator_is_refused_by_name() {
        let error = Rate::new(0, 3).expect_err("a rate that buys nothing is not a rate");
        assert_eq!(error, RateError::ZeroNumerator { denominator: 3 });
        assert!(error.to_string().contains("numerator"), "got: {error}");
    }

    #[test]
    fn the_same_terms_written_twice_are_one_value() {
        assert_eq!(rate(2, 4), rate(1, 2));
        assert_eq!(rate(2, 4).numerator(), 1);
        assert_eq!(rate(2, 4).denominator(), 2);
    }

    #[test]
    fn rates_order_by_value_rather_than_by_field() {
        // The comparison a field-by-field ordering gets backwards.
        assert!(rate(1, 3) < rate(1, 2));
        assert!(rate(3, 1) > rate(2, 1));
        assert_eq!(rate(2, 4).cmp(&rate(1, 2)), Ordering::Equal);
    }

    #[test]
    fn inverting_reads_the_same_terms_the_other_way() {
        let usdc_to_anyone = rate(USDC_TO_ANYONE, 1);
        assert_eq!(usdc_to_anyone.inverted(), rate(1, USDC_TO_ANYONE));
        assert_eq!(usdc_to_anyone.inverted().inverted(), usdc_to_anyone);
    }

    #[test]
    fn a_rate_prints_both_halves() {
        assert_eq!(rate(USDC_TO_ANYONE, 1).to_string(), "4000000000000/1");
    }

    #[test]
    fn a_rate_parses_from_a_table() {
        let parsed: Rate =
            serde_json::from_str(r#"{"numerator":3,"denominator":2}"#).expect("a table is a rate");
        assert_eq!(parsed, rate(3, 2));
    }

    #[test]
    fn a_declared_zero_denominator_is_refused_at_load() {
        let error = serde_json::from_str::<Rate>(r#"{"numerator":3,"denominator":0}"#)
            .expect_err("a zero denominator never becomes a value");
        assert!(error.to_string().contains("denominator"), "got: {error}");
    }

    #[test]
    fn an_unknown_key_in_a_rate_table_is_refused_by_name() {
        let error = serde_json::from_str::<Rate>(r#"{"numerator":3,"denominator":2,"spread":1}"#)
            .expect_err("an unknown key is refused");
        assert!(error.to_string().contains("spread"), "got: {error}");
    }

    proptest! {
        #[test]
        fn a_rate_is_always_in_lowest_terms(
            numerator in 1u64..,
            denominator in 1u64..,
        ) {
            let declared = rate(numerator, denominator);
            prop_assert_eq!(gcd(declared.numerator(), declared.denominator()), 1);
        }

        #[test]
        fn reducing_never_changes_the_terms(
            numerator in 1u64..u32::MAX as u64,
            denominator in 1u64..u32::MAX as u64,
            factor in 1u64..1_000,
        ) {
            // The same ratio scaled up is the same rate.
            prop_assert_eq!(
                rate(numerator * factor, denominator * factor),
                rate(numerator, denominator)
            );
        }

        #[test]
        fn a_denominator_is_never_zero(
            numerator in 1u64..,
            denominator in 1u64..,
        ) {
            prop_assert!(rate(numerator, denominator).denominator() > 0);
            prop_assert!(rate(numerator, denominator).inverted().denominator() > 0);
        }

        #[test]
        fn equal_rates_compare_equal_and_hash_alike(
            numerator in 1u64..u32::MAX as u64,
            denominator in 1u64..u32::MAX as u64,
            factor in 1u64..1_000,
        ) {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};

            let scaled = rate(numerator * factor, denominator * factor);
            let plain = rate(numerator, denominator);
            prop_assert_eq!(scaled.cmp(&plain), Ordering::Equal);

            let hash_of = |value: &Rate| {
                let mut hasher = DefaultHasher::new();
                value.hash(&mut hasher);
                hasher.finish()
            };
            prop_assert_eq!(hash_of(&scaled), hash_of(&plain));
        }

        #[test]
        fn ordering_is_total_over_the_whole_range(
            first_numerator in 1u64..,
            first_denominator in 1u64..,
            second_numerator in 1u64..,
            second_denominator in 1u64..,
        ) {
            // u64 * u64 always fits a u128, so no pair of rates can panic
            // the comparison.
            let first = rate(first_numerator, first_denominator);
            let second = rate(second_numerator, second_denominator);
            prop_assert_eq!(first.cmp(&second), second.cmp(&first).reverse());
        }

        #[test]
        fn a_rate_round_trips_through_serde(
            numerator in 1u64..,
            denominator in 1u64..,
        ) {
            let declared = rate(numerator, denominator);
            let json = serde_json::to_string(&declared).expect("serializes");
            let read: Rate = serde_json::from_str(&json).expect("reads back");
            prop_assert_eq!(read, declared);
        }
    }
}
