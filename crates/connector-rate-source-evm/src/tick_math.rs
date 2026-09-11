//! `1.0001^tick`, in integers, the way Uniswap v3-core computes it.
//!
//! ADR 0071 decision 4 says the conversion arithmetic is a rational over base
//! units and that no float exists on the money path. A tick is a logarithm, so
//! this is the one place in the reader where that could quietly stop being
//! true -- `(1.0001f64).powi(tick)` is one line, and it is wrong in a way
//! nobody notices until two nodes disagree about a price by a few base units.
//!
//! So this is a port of
//! [`TickMath.getSqrtRatioAtTick`](https://github.com/Uniswap/v3-core/blob/main/contracts/libraries/TickMath.sol):
//! a product of twenty precomputed Q128.128 constants, one per bit of
//! `|tick|`, each of them `floor(2^128 / sqrt(1.0001)^(2^i))`, shifted back
//! down after each multiply. The result is `sqrt(1.0001^tick)` as a Q64.96
//! fixed-point integer -- exact to the last bit the format holds, identical to
//! what the chain itself computes, and reached without a floating-point
//! operation anywhere.
//!
//! The price the reader wants is that ratio squared:
//! `1.0001^tick = (sqrtRatioX96 / 2^96)^2`, which is **token1 per token0 in
//! base units** -- the decimals difference between the pair is already folded
//! into it (ADR 0071 decision 4: scale is not price, and nothing downstream
//! applies a second one). Squaring a 160-bit number needs 320 bits, so the
//! product is carried in a [`U512`] and only then reduced -- once, off the
//! money path, before the table write -- to the `u64/u64` [`Rate`] the rest of
//! the connector deals in.

use connector_domain::Rate;
use ethers::types::{U256, U512};
use thiserror::Error;

/// The lowest tick a Uniswap v3 pool can hold, from v3-core's `TickMath`.
pub const MIN_TICK: i32 = -887_272;

/// The highest tick a Uniswap v3 pool can hold, from v3-core's `TickMath`.
pub const MAX_TICK: i32 = 887_272;

/// Which way round a leg reads the pool's own token ordering.
///
/// A pool has no opinion about which of its two tokens is "the" base: it holds
/// one tick, and that tick is the price of `token0` in `token1`. The leg is
/// what says which direction was asked for, and reading it the other way round
/// is the reciprocal -- a different number, on the other side of one, which is
/// what the port's anti-substitution promise turns on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolDirection {
    /// The leg's base is the pool's `token0`: the rate is the pool's own
    /// price, `token1` per `token0`.
    Token1PerToken0,
    /// The leg's base is the pool's `token1`: the rate is that price inverted.
    Token0PerToken1,
}

/// What can go wrong turning a tick into a [`Rate`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TickMathError {
    /// A tick outside the range a Uniswap v3 pool can hold. Unreachable from a
    /// real pool; reachable from a contract that answers `observe` with
    /// something else, which is the case this exists to refuse rather than
    /// compute over.
    #[error("tick {tick} is outside [{MIN_TICK}, {MAX_TICK}]")]
    TickOutOfRange { tick: i32 },

    /// `1.0001^tick` does not fit a `u64/u64` rational in the direction asked.
    /// A pair priced past `u64::MAX` base units of one token per base unit of
    /// the other is beyond what a [`Rate`] can spell, and a made-up nearby
    /// number is worse than a refusal.
    ///
    /// The two terms are the true ratio divided down until both fit a `u128`,
    /// so the message names the magnitude that could not be represented rather
    /// than a 320-bit product nobody can read.
    #[error("1.0001^{tick} is {numerator}/{denominator}, which is not a u64 rational")]
    Unrepresentable {
        tick: i32,
        numerator: u128,
        denominator: u128,
    },
}

/// `sqrt(1.0001^tick) * 2^96`, as v3-core's `TickMath.getSqrtRatioAtTick`
/// computes it.
///
/// The running `ratio` is Q128.128 throughout, so every product is under
/// `2^256` and the trailing shift brings it back -- the same bound v3-core
/// relies on to do the multiply unchecked. A positive tick is the reciprocal
/// of the negative one, taken as `U256::MAX / ratio` exactly as the Solidity
/// does rather than from a second constant table, so the two directions cannot
/// drift apart.
pub fn sqrt_ratio_at_tick(tick: i32) -> Result<U256, TickMathError> {
    if !(MIN_TICK..=MAX_TICK).contains(&tick) {
        return Err(TickMathError::TickOutOfRange { tick });
    }

    let absolute = tick.unsigned_abs();

    // Bit `0x1`'s constant seeds the product rather than multiplying into it,
    // which is where v3-core puts it too.
    let mut ratio = if absolute & 0x1 != 0 {
        U256::from(0xfffc_b933_bd6f_ad37_aa2d_162d_1a59_4001u128)
    } else {
        U256::one() << 128
    };

    for (bit, constant) in RECIPROCAL_SQRT_CONSTANTS {
        if absolute & bit != 0 {
            ratio = (ratio * U256::from(constant)) >> 128;
        }
    }

    if tick > 0 {
        ratio = U256::MAX / ratio;
    }

    // Q128.128 down to Q64.96, rounding up -- v3-core's own last line.
    let remainder = ratio & ((U256::one() << 32) - 1);
    let rounding = if remainder.is_zero() {
        U256::zero()
    } else {
        U256::one()
    };
    Ok((ratio >> 32) + rounding)
}

/// `1.0001^tick` as a [`Rate`], in the direction the leg asked for.
///
/// The exact ratio is `sqrtRatioX96^2 / 2^192`, or its reciprocal, carried at
/// full width and reduced only at the end. Reduction floors **both** terms --
/// the rule `connector_rate_source::compose_rates` states, for the reason it
/// states: dealing margin is the `spread` guard's job (ADR 0071 decision 5),
/// not a rounding rule's, so rounding favours neither side of the trade.
///
/// Precision is whatever a `u64/u64` rational holds, which for an extreme
/// ratio is less than the tick carries: a price around `10^-14` leaves the
/// numerator about eighteen bits once the denominator has taken sixty-four.
/// That is a property of the representation ADR 0071 decision 4 chose, not of
/// this reduction -- no `u64/u64` pair does better.
pub fn rate_at_tick(tick: i32, direction: PoolDirection) -> Result<Rate, TickMathError> {
    let sqrt_ratio = sqrt_ratio_at_tick(tick)?;
    let squared = sqrt_ratio.full_mul(sqrt_ratio);
    let two_to_the_192 = U512::one() << 192;

    let (numerator, denominator) = match direction {
        PoolDirection::Token1PerToken0 => (squared, two_to_the_192),
        PoolDirection::Token0PerToken1 => (two_to_the_192, squared),
    };

    narrow_to_rate(numerator, denominator).ok_or_else(|| {
        let (numerator, denominator) = narrow_for_report(numerator, denominator);
        TickMathError::Unrepresentable {
            tick,
            numerator,
            denominator,
        }
    })
}

/// Divide a wide rational down until both terms fit a `u64`, preserving the
/// ratio as closely as `u64/u64` allows.
///
/// One shift, sized so the *larger* term lands on sixty-four bits: shifting
/// further throws away precision the pair could still have held, and shifting
/// less leaves the larger term unrepresentable. [`Rate::new`] then reduces by
/// gcd, so the accessors return the reduced pair and an error message echoing
/// a rate prints its lowest terms.
///
/// `None` where the result is not a rate at all -- a ratio so extreme that one
/// term floors to zero, which [`Rate::new`] refuses at both ends anyway.
fn narrow_to_rate(numerator: U512, denominator: U512) -> Option<Rate> {
    let widest = numerator.bits().max(denominator.bits());
    let shift = widest.saturating_sub(64);
    let numerator = numerator >> shift;
    let denominator = denominator >> shift;

    if numerator.is_zero() || denominator.is_zero() {
        return None;
    }

    // The only thing `Rate::new` refuses is a zero term at either end, and the
    // check above has ruled both out -- so this arm is unreachable rather than
    // a second way for a rate to be missing.
    Rate::new(numerator.low_u64(), denominator.low_u64()).ok()
}

/// The same ratio divided down to a pair a human can read in an error message.
/// Lossy on purpose: nothing computes with these.
fn narrow_for_report(numerator: U512, denominator: U512) -> (u128, u128) {
    let widest = numerator.bits().max(denominator.bits());
    let shift = widest.saturating_sub(128);
    (
        (numerator >> shift).low_u128(),
        (denominator >> shift).low_u128(),
    )
}

/// `floor(2^128 / sqrt(1.0001)^(2^i))` for each bit of `|tick|` above the
/// lowest -- v3-core's table, transcribed in the same hex it is written in
/// there so the two can be diffed by eye.
const RECIPROCAL_SQRT_CONSTANTS: [(u32, u128); 19] = [
    (0x2, 0xfff9_7272_373d_4132_59a4_6990_580e_213a),
    (0x4, 0xfff2_e50f_5f65_6932_ef12_357c_f3c7_fdcc),
    (0x8, 0xffe5_caca_7e10_e4e6_1c36_24ea_a094_1cd0),
    (0x10, 0xffcb_9843_d60f_6159_c9db_5883_5c92_6644),
    (0x20, 0xff97_3b41_fa98_c081_472e_6896_dfb2_54c0),
    (0x40, 0xff2e_a164_66c9_6a38_43ec_78b3_26b5_2861),
    (0x80, 0xfe5d_ee04_6a99_a2a8_11c4_61f1_969c_3053),
    (0x100, 0xfcbe_86c7_900a_88ae_dcff_c83b_479a_a3a4),
    (0x200, 0xf987_a725_3ac4_1317_6f2b_074c_f781_5e54),
    (0x400, 0xf339_2b08_22b7_0005_940c_7a39_8e4b_70f3),
    (0x800, 0xe715_9475_a2c2_9b74_43b2_9c7f_a6e8_89d9),
    (0x1000, 0xd097_f3bd_fd20_22b8_845a_d8f7_92aa_5825),
    (0x2000, 0xa9f7_4646_2d87_0fdf_8a65_dc1f_90e0_61e5),
    (0x4000, 0x70d8_69a1_56d2_a1b8_90bb_3df6_2baf_32f7),
    (0x8000, 0x31be_135f_97d0_8fd9_8123_1505_542f_cfa6),
    (0x10000, 0x09aa_508b_5b7a_84e1_c677_de54_f3e9_9bc9),
    (0x20000, 0x005d_6af8_dedb_8119_6699_c329_225e_e604),
    (0x40000, 0x0000_2216_e584_f5fa_1ea9_2604_1bed_fe98),
    (0x80000, 0x0000_0000_048a_1703_91f7_dc42_444e_8fa2),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// v3-core's own `MIN_SQRT_RATIO` and `MAX_SQRT_RATIO` constants, and the
    /// one ratio anybody can check by hand. If the transcribed table above
    /// were wrong in any bit, the two extremes -- twenty multiplies apart from
    /// the seed -- would not land on the published values.
    #[test]
    fn the_tick_extremes_reproduce_v3_cores_published_constants() {
        assert_eq!(
            sqrt_ratio_at_tick(0).expect("tick 0 is in range"),
            U256::one() << 96,
            "tick 0 is a price of exactly 1, so sqrt(1) * 2^96"
        );
        assert_eq!(
            sqrt_ratio_at_tick(MIN_TICK).expect("MIN_TICK is in range"),
            U256::from(4_295_128_739u64),
            "v3-core's MIN_SQRT_RATIO"
        );
        assert_eq!(
            sqrt_ratio_at_tick(MAX_TICK).expect("MAX_TICK is in range"),
            U256::from_dec_str("1461446703485210103287273052203988822378723970342")
                .expect("v3-core's MAX_SQRT_RATIO is a U256"),
            "v3-core's MAX_SQRT_RATIO"
        );
    }

    #[test]
    fn a_tick_outside_the_pool_range_is_refused_rather_than_computed() {
        assert_eq!(
            sqrt_ratio_at_tick(MAX_TICK + 1).unwrap_err(),
            TickMathError::TickOutOfRange { tick: MAX_TICK + 1 }
        );
        assert_eq!(
            sqrt_ratio_at_tick(MIN_TICK - 1).unwrap_err(),
            TickMathError::TickOutOfRange { tick: MIN_TICK - 1 }
        );
    }

    #[test]
    fn tick_zero_is_one_to_one_in_both_directions() {
        assert_eq!(
            rate_at_tick(0, PoolDirection::Token1PerToken0).expect("a rate"),
            Rate::new(1, 1).expect("1/1 is a rate")
        );
        assert_eq!(
            rate_at_tick(0, PoolDirection::Token0PerToken1).expect("a rate"),
            Rate::new(1, 1).expect("1/1 is a rate")
        );
    }

    /// The two ticks the tier-3 fixture's pools sit at, and the rates the
    /// reader must derive from them. Computed independently of this code from
    /// the same v3-core algorithm in exact integer arithmetic; the tier-3 test
    /// asserts the same two figures come back off a real chain.
    ///
    /// `1.0001^-126125` is ANYONE priced in WETH -- both 18 decimals, so the
    /// base-unit ratio is also the human one: about 1/300100 of an ether.
    /// `1.0001^196253` is USDC priced in WETH; read the other way round it is
    /// WETH priced in USDC, which at 18-against-6 decimals is about 3001 USDC
    /// to the ether.
    #[test]
    fn the_fixture_ticks_derive_their_known_good_rates() {
        assert_eq!(
            rate_at_tick(-126_125, PoolDirection::Token1PerToken0).expect("a rate"),
            Rate::new(30_734_378_297_187, 9_223_372_036_854_775_808).expect("a rate"),
            "1 base unit of ANYONE in base units of WETH"
        );
        assert_eq!(
            rate_at_tick(196_253, PoolDirection::Token0PerToken1).expect("a rate"),
            Rate::new(17_179_869_184, 5_724_706_407_302_616_293).expect("a rate"),
            "1 base unit of WETH in base units of USDC"
        );
    }

    /// Direction is the reciprocal, taken before the reduction so the two
    /// readings are exact inverses of each other rather than of two separately
    /// floored pairs.
    #[test]
    fn reading_a_pool_the_other_way_round_inverts_it() {
        let forward = rate_at_tick(-126_125, PoolDirection::Token1PerToken0).expect("a rate");
        let backward = rate_at_tick(-126_125, PoolDirection::Token0PerToken1).expect("a rate");
        assert_eq!(backward, forward.inverted());
        assert!(forward < Rate::new(1, 1).expect("1/1 is a rate"));
        assert!(backward > Rate::new(1, 1).expect("1/1 is a rate"));
    }

    /// The far ends of the tick range price one token past `u64::MAX` base
    /// units of the other, which no `Rate` can spell. Refused, not saturated:
    /// a saturated rate is a made-up price.
    #[test]
    fn a_ratio_past_a_u64_rational_is_refused_not_saturated() {
        let err = rate_at_tick(MAX_TICK, PoolDirection::Token1PerToken0).unwrap_err();
        assert!(
            matches!(err, TickMathError::Unrepresentable { tick, .. } if tick == MAX_TICK),
            "got {err}"
        );
        let err = rate_at_tick(MIN_TICK, PoolDirection::Token1PerToken0).unwrap_err();
        assert!(
            matches!(err, TickMathError::Unrepresentable { tick, .. } if tick == MIN_TICK),
            "got {err}"
        );
    }
}
