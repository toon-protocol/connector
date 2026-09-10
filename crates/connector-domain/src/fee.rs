//! Flat per-packet fee arithmetic
//! ([ADR 0010](../../../docs/adr/0010-flat-per-packet-fee-and-minimum-delivery.md)):
//! what a hop earns forwarding one packet. The fee is a flat amount
//! subtracted once per packet, never a share of `amount` -- there is no
//! percentage or basis-point arithmetic anywhere in this module, which is
//! what keeps a packet of any size charged rather than rounding to zero.
//!
//! What bounds erosion across a path is not arithmetic here but the claim
//! covering each crossing: `cover_forward` mints for the packet's forwarded
//! value, so every hop holds a claim for at least what it passes on (ADR
//! 0057, issue #1143). There is no declared floor for this module to check.
//! What survives is the plain "was there anything left at all" question,
//! whose answer a hop reports as RFC 0027's `R01`.
//!
//! # The converting arm
//!
//! [`ADR 0071`](../../../docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)
//! makes a hop whose two channels hold different tokens cross that
//! **denomination boundary** at a rate it has declared, so this module has a
//! second pair of functions -- [`amount_after_rate_and_fee`] going down the
//! path and [`cost_before_rate_and_fee`] coming back up it. They are a
//! **sibling** of [`amount_after_fee`], never a replacement: a forward that
//! crosses no boundary has no rate to cross at (decision 2 gives a same-asset
//! pair no rate row *by definition*) and runs the one subtraction above,
//! unchanged.
//!
//! The fee is the same flat fee, and it is in the **outgoing** leg's unit --
//! the peering ADR 0061 already attaches it to (ADR 0071 decision 1). So the
//! forward converts and then subtracts, and the reject path adds and then
//! un-converts, which is why the two are not each other's mirror image
//! line for line.
//!
//! *Before* and *after* in those two names are positions on the **path**, not
//! moments in the arithmetic: the amount *after* this hop's rate and fee is
//! what leaves it downstream, and the cost *before* them is what a prober
//! upstream must send.

/// The amount this hop forwards downstream once its own flat `fee` (agreed
/// bilaterally for the peering relation, per ADR 0010) is taken from
/// `amount`, or `None` if the fee alone exceeds what arrived.
///
/// A hop that gets `None` here must reject
/// ([`crate::RejectCode::r01_insufficient_source_amount`], RFC 0027's
/// "too little to forward") rather than forward a smaller amount and hope a
/// downstream hop makes up the difference: no downstream hop ever increases
/// an amount, so the shortfall would only grow. That reject is unaffected by
/// ADR 0057, which retired the *declared floor* this function used to check
/// and not the arithmetic below.
pub fn amount_after_fee(amount: u64, fee: u64) -> Option<u64> {
    amount.checked_sub(fee)
}

/// The amount this hop forwards across a **denomination boundary**:
/// `floor(amount * rate) - fee`, with `amount` in the incoming leg's unit and
/// both the answer and `fee` in the outgoing leg's
/// ([ADR 0071](../../../docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)
/// decision 1).
///
/// The multiplication goes through `u128`, where `u64 * u64` always fits, and
/// the division **rounds down** -- in the connector's favour, always, because
/// the connector is the party that mints the covering claim for whatever this
/// answers.
///
/// # When this answers `None`
///
/// Two cases, and a caller that reports both as `R01`
/// ([`crate::RejectCode::r01_insufficient_source_amount`]) is right about the
/// first and defensible about the second:
///
/// * **The fee alone exceeds the converted amount** -- the same "was there
///   anything left at all" question [`amount_after_fee`] asks, now asked in
///   the outgoing unit, after conversion, which is where ADR 0071 decision 1
///   puts the fee.
/// * **The converted amount does not fit the outgoing leg's `u64`.** A rate
///   that scales up -- 6 decimals to 18 is `10^12` before any market price --
///   can carry a perfectly ordinary incoming amount past `u64::MAX`, which is
///   the real ceiling ADR 0071's consequences name and which an operator
///   expresses as `max_packet_amount` on the outgoing peering. Refusing is the
///   only safe answer: saturating would forward a figure this connector then
///   has to mint a covering claim for, so the overflow would be paid for by
///   the connector rather than reported to the sender.
///
/// There is no third case, and in particular no rate that makes this
/// arithmetic panic: [`Rate`](crate::Rate) has no zero denominator to divide
/// by.
pub fn amount_after_rate_and_fee(amount: u64, rate: crate::Rate, fee: u64) -> Option<u64> {
    let converted =
        u128::from(amount) * u128::from(rate.numerator()) / u128::from(rate.denominator());
    u64::try_from(converted).ok()?.checked_sub(fee)
}

/// What a packet must have arrived with, in the **incoming** leg's unit, for a
/// reject carrying `cost` in the outgoing leg's unit to have been reached:
/// `ceil((cost + fee) / rate)`
/// ([ADR 0071](../../../docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)
/// decision 7).
///
/// This is the exact inverse of [`amount_after_rate_and_fee`], and "exact" is
/// meant literally: the answer is the **smallest** incoming amount that still
/// clears `cost` downstream once this hop has taken its fee. The fee is added
/// in the outgoing unit *first*, because that is the unit ADR 0061's peering
/// fee is denominated in, and only then is the total un-converted.
///
/// The rounding goes **up**, against this connector, so that the pair of
/// roundings can overstate a probed cost by a base unit and can never
/// understate one. A prober that pays the figure this produces always clears
/// the hop; a prober that pays a unit less may not. That asymmetry is the
/// whole point: ADR 0011's probe answers with one number, and a number that
/// might be a unit short would make every probed price a coin toss.
///
/// # Total, and saturating rather than refusing
///
/// A reject is already travelling upstream when this is evaluated, and there
/// is nothing upstream that "no answer" could mean, so this answers a `u64`
/// rather than an `Option`. The two figures that do not fit -- `cost + fee`
/// past `u128` range once scaled, or a quotient past `u64::MAX` -- both
/// saturate to `u64::MAX`, which is the same move [`crate::Price::charge`]
/// makes for the same reason: a cost no claim can cover refuses the packet,
/// and it refuses it without ever understating what the path costs.
pub fn cost_before_rate_and_fee(cost: u64, rate: crate::Rate, fee: u64) -> u64 {
    let with_fee = u128::from(cost) + u128::from(fee);
    match with_fee.checked_mul(u128::from(rate.denominator())) {
        Some(scaled) => {
            u64::try_from(scaled.div_ceil(u128::from(rate.numerator()))).unwrap_or(u64::MAX)
        }
        None => u64::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn subtracts_the_flat_fee() {
        assert_eq!(amount_after_fee(100, 5), Some(95));
    }

    #[test]
    fn rejects_when_the_fee_alone_exceeds_the_amount() {
        assert_eq!(amount_after_fee(3, 5), None);
    }

    #[test]
    fn a_tiny_packet_is_still_charged_the_full_flat_fee() {
        // Regression for the basis-point model this replaces (ADR 0010),
        // where a packet under 1000 units at the default rate was carried
        // for free because amount / 1000 rounded down to zero.
        assert_eq!(amount_after_fee(1, 1), Some(0));
        assert_eq!(amount_after_fee(1, 2), None);
    }

    #[test]
    fn zero_fee_forwards_the_full_amount() {
        assert_eq!(amount_after_fee(42, 0), Some(42));
    }

    proptest! {
        #[test]
        fn never_forwards_more_than_was_received(
            amount in any::<u64>(),
            fee in any::<u64>(),
        ) {
            if let Some(forwarded) = amount_after_fee(amount, fee) {
                prop_assert!(forwarded <= amount);
            }
        }

        #[test]
        fn the_fee_actually_taken_is_always_the_exact_configured_fee(
            amount in any::<u64>(),
            fee in any::<u64>(),
        ) {
            // No rounding, no percentage: whenever a fee can be taken at
            // all, it is taken in full.
            if let Some(forwarded) = amount_after_fee(amount, fee) {
                prop_assert_eq!(amount - forwarded, fee);
            }
        }
    }

    /// One 6-decimals USDC base unit in 18-decimals ANYONE base units, at four
    /// ANYONE to the USDC: the decimals gap (`10^12`) and the market price (4)
    /// folded into one ratio, which is what ADR 0071 decision 4 means by the
    /// rate being the only place a scale difference lives.
    const USDC_TO_ANYONE: u64 = 4_000_000_000_000;

    fn rate(numerator: u64, denominator: u64) -> crate::Rate {
        crate::Rate::new(numerator, denominator).expect("a rate with two non-zero halves")
    }

    #[test]
    fn a_forward_converts_and_then_takes_the_fee_in_the_outgoing_unit() {
        // 1 USDC in, at four ANYONE to the USDC, less a fee of 0.001 ANYONE.
        let forwarded =
            amount_after_rate_and_fee(1_000_000, rate(USDC_TO_ANYONE, 1), 1_000_000_000_000_000);
        assert_eq!(forwarded, Some(3_999_000_000_000_000_000));
    }

    #[test]
    fn the_other_direction_is_the_same_arithmetic_on_the_inverted_rate() {
        // 4 ANYONE in, out at 1 USDC less a fee of 0.001 USDC.
        let forwarded =
            amount_after_rate_and_fee(4_000_000_000_000_000_000, rate(1, USDC_TO_ANYONE), 1_000);
        assert_eq!(forwarded, Some(999_000));
    }

    #[test]
    fn a_forward_rounds_down_and_a_reject_rounds_up() {
        // The pair ADR 0071 decision 7 names, on a rate that cannot divide
        // evenly.
        assert_eq!(amount_after_rate_and_fee(10, rate(1, 3), 0), Some(3));
        assert_eq!(cost_before_rate_and_fee(3, rate(1, 3), 0), 9);
    }

    #[test]
    fn a_probed_cost_is_the_smallest_amount_that_still_clears() {
        // Paying the probed figure clears the hop; paying a base unit less
        // does not. That is the whole promise a prober relies on.
        let quoted = cost_before_rate_and_fee(3, rate(1, 3), 0);
        assert_eq!(amount_after_rate_and_fee(quoted, rate(1, 3), 0), Some(3));
        assert_eq!(
            amount_after_rate_and_fee(quoted - 1, rate(1, 3), 0),
            Some(2)
        );
    }

    #[test]
    fn the_reject_path_adds_the_fee_before_un_converting_it() {
        // A fee of 1 outgoing unit at 1:1000 costs 1000 incoming units, not 1.
        assert_eq!(cost_before_rate_and_fee(0, rate(1, 1000), 1), 1000);
    }

    #[test]
    fn a_forward_refuses_when_the_fee_alone_exceeds_the_converted_amount() {
        // 10 in at 1:3 converts to 3, and a fee of 4 leaves nothing --
        // the R01 question, asked in the outgoing unit after conversion.
        assert_eq!(amount_after_rate_and_fee(10, rate(1, 3), 4), None);
        assert_eq!(amount_after_rate_and_fee(10, rate(1, 3), 3), Some(0));
    }

    #[test]
    fn a_forward_refuses_an_amount_the_outgoing_leg_cannot_hold() {
        // ADR 0071's real ceiling: `u64::MAX / 10^18` is ~18.4 tokens on an
        // 18-decimals leg, and a rate that scales up walks a perfectly
        // ordinary incoming amount past it.
        assert_eq!(
            amount_after_rate_and_fee(u64::MAX, rate(USDC_TO_ANYONE, 1), 0),
            None
        );
    }

    #[test]
    fn a_cost_too_large_to_state_saturates_rather_than_understating() {
        // A cost no claim can cover, which refuses the packet -- the move
        // `Price::charge` makes, for the reason it makes it.
        assert_eq!(
            cost_before_rate_and_fee(u64::MAX, rate(1, u64::MAX), u64::MAX),
            u64::MAX
        );
    }

    proptest! {
        #[test]
        fn a_probed_cost_always_clears_and_is_never_beaten_by_a_smaller_one(
            cost in any::<u64>(),
            fee in any::<u64>(),
            numerator in 1u64..,
            denominator in 1u64..,
        ) {
            let rate = rate(numerator, denominator);
            let quoted = cost_before_rate_and_fee(cost, rate, fee);

            // The clearing half, over the crossings that can happen at all:
            // a quote of `u64::MAX` may be a saturated one, and a saturated
            // quote is by definition an amount no `u64` could have carried,
            // and a forward that answers `None` here has run into the
            // outgoing leg's own ceiling. Neither is a cost this hop
            // understated; both are packets it refuses.
            if quoted < u64::MAX {
                if let Some(forwarded) = amount_after_rate_and_fee(quoted, rate, fee) {
                    prop_assert!(
                        forwarded >= cost,
                        "a probed cost must never understate: quoted {quoted} forwards \
                         {forwarded}, short of {cost}"
                    );
                }
            }

            // The minimality half: it may overstate by a base unit, and it
            // does not overstate by more, because one unit less does not
            // clear.
            if quoted > 0 {
                if let Some(forwarded) = amount_after_rate_and_fee(quoted - 1, rate, fee) {
                    prop_assert!(
                        forwarded < cost,
                        "a probed cost must be the smallest that clears, and {quoted} minus \
                         one still forwards {forwarded} against {cost}"
                    );
                }
            }
        }

        #[test]
        fn a_forward_refuses_exactly_when_nothing_would_be_left(
            amount in any::<u64>(),
            fee in any::<u64>(),
            numerator in 1u64..,
            denominator in 1u64..,
        ) {
            // The `u128` intermediate restated as the specification it is:
            // exact integer arithmetic, and the only two refusals are the
            // outgoing leg's ceiling and the fee.
            let rate = rate(numerator, denominator);
            let converted =
                u128::from(amount) * u128::from(numerator) / u128::from(denominator);

            match u64::try_from(converted) {
                Ok(converted) => prop_assert_eq!(
                    amount_after_rate_and_fee(amount, rate, fee),
                    converted.checked_sub(fee)
                ),
                Err(_) => prop_assert_eq!(amount_after_rate_and_fee(amount, rate, fee), None),
            }
        }

        #[test]
        fn a_forward_never_rounds_up(
            amount in any::<u64>(),
            numerator in 1u64..,
            denominator in 1u64..,
        ) {
            // Rounding is always the connector's way: what leaves is never
            // more than the exact product.
            if let Some(forwarded) = amount_after_rate_and_fee(amount, rate(numerator, denominator), 0) {
                let exact = u128::from(amount) * u128::from(numerator) / u128::from(denominator);
                prop_assert!(u128::from(forwarded) <= exact);
            }
        }

        #[test]
        fn crossing_a_boundary_is_total_over_the_whole_u64_range(
            amount in any::<u64>(),
            cost in any::<u64>(),
            fee in any::<u64>(),
            numerator in 1u64..,
            denominator in 1u64..,
        ) {
            // No panic and no overflow anywhere between `0` and `u64::MAX` on
            // either arm: the only contract the packet path needs, and the
            // reason every intermediate is a `u128`.
            let rate = rate(numerator, denominator);
            let _ = amount_after_rate_and_fee(amount, rate, fee);
            let _ = cost_before_rate_and_fee(cost, rate, fee);
        }

        #[test]
        fn a_rate_that_divides_evenly_converts_exactly(
            units in 0u64..1_000_000,
            scale in 1u64..1_000_000_000_000,
        ) {
            // The decimals-only case, which is the whole of a same-market
            // crossing: scale up and back down and the figure returns.
            let up = rate(scale, 1);
            let converted = amount_after_rate_and_fee(units, up, 0).expect("fits");
            prop_assert_eq!(converted, units * scale);
            prop_assert_eq!(
                amount_after_rate_and_fee(converted, up.inverted(), 0),
                Some(units)
            );
        }
    }
}
