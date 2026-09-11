// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

/// @title OracleMockPool
/// @notice The Uniswap-v3-compatible oracle surface, and nothing else: the
/// three functions `UniswapV3RateSource` actually calls -- `token0()`,
/// `token1()` and `observe(uint32[])` -- backed by the same observation ring
/// v3-core's `Oracle` library keeps, so this crate's tier-3 tests can read a
/// TWAP off a real chain without vendoring v3-core (GPL-2.0-or-later, which
/// this MIT repository cannot carry) and without pointing a test at a
/// deployment it does not control.
///
/// Deployed only inside a test's disposable `anvil`. It is a *fixture*, not a
/// pool: there is no swap math, no liquidity, no fee accounting and no token
/// transfer anywhere in it, because the reader never asks a pool for any of
/// those. `swap(int24)` here is only "a swap happened, and left the tick
/// here" -- the one effect a swap has on the oracle.
///
/// Two faithfulnesses matter, and are why this is a ring rather than a list:
///
/// 1. **A pool is initialized with an observation array of length 1**, and
///    every write overwrites that single slot, so `observe` over any real
///    window reverts `OLD` until somebody pays to grow the array. That is the
///    cardinality prerequisite `docs/research/token-pair-price-sources.md`
///    records, and the tier-3 test proves the reader reports it as
///    `WindowNotServed` rather than guessing.
/// 2. **`observe` interpolates a counterfactual observation** for a target
///    time no stored observation matches, and extrapolates past the newest one
///    at the current tick -- which is what makes the `tickCumulative` delta a
///    mean over exactly the window asked for.
///
/// One deliberate simplification against v3-core: the 32-bit timestamp
/// wraparound comparisons (`Oracle.lte`) are left out, and the surrounding
/// observations are found by a linear scan rather than a binary search. A
/// disposable chain lives for seconds and holds a handful of observations, so
/// neither the wrap nor the gas matters here, and both would be code with no
/// test able to reach it.
contract OracleMockPool {
    struct Observation {
        uint32 blockTimestamp;
        int56 tickCumulative;
        bool initialized;
    }

    // forge-lint: disable-start(screaming-snake-case-immutable)
    // Named as a real v3 pool names them: the ABI is the point, and
    // `TOKEN0()` is not a function any pool has.
    address public immutable token0;
    address public immutable token1;
    // forge-lint: disable-end(screaming-snake-case-immutable)

    /// The current tick -- the price of one base unit of `token0` in base
    /// units of `token1`, as `1.0001^tick`. The decimals difference between
    /// the two tokens is already inside it, which is why the reader never
    /// asks either token for its `decimals()`.
    int24 public tick;

    uint16 public observationIndex;
    uint16 public observationCardinality;
    uint16 public observationCardinalityNext;

    Observation[65535] public observations;

    constructor(address _token0, address _token1, int24 _tick) {
        token0 = _token0;
        token1 = _token1;
        tick = _tick;

        observations[0] = Observation({blockTimestamp: uint32(block.timestamp), tickCumulative: 0, initialized: true});
        observationIndex = 0;
        observationCardinality = 1;
        observationCardinalityNext = 1;
    }

    /// @notice Pay for the slots a longer window needs. Anyone may call it on
    /// a real pool, and an operator naming a fresh pool has to.
    function increaseObservationCardinalityNext(uint16 next) external {
        uint16 current = observationCardinalityNext;
        if (next <= current) {
            return;
        }
        for (uint16 i = current; i < next; i++) {
            // v3 writes a non-zero timestamp into each new slot so the SSTORE
            // is paid here rather than by whoever first swaps into it.
            // `initialized` stays false, so `observe` still ignores it.
            observations[i].blockTimestamp = 1;
        }
        observationCardinalityNext = next;
    }

    /// @notice Record that a swap moved the tick to `newTick`. The observation
    /// written covers the interval that just *ended*, at the tick that was in
    /// force across it -- so a pool nobody trades against writes nothing, and
    /// its newest observation ages.
    function swap(int24 newTick) external {
        Observation memory last = observations[observationIndex];
        uint32 timestamp = uint32(block.timestamp);

        if (last.blockTimestamp != timestamp) {
            uint16 cardinality = observationCardinality;
            if (observationCardinalityNext > cardinality && observationIndex == cardinality - 1) {
                cardinality = observationCardinalityNext;
                observationCardinality = cardinality;
            }
            uint16 next = (observationIndex + 1) % cardinality;
            observations[next] = Observation({
                blockTimestamp: timestamp,
                tickCumulative: last.tickCumulative + int56(tick) * int56(uint56(timestamp - last.blockTimestamp)),
                initialized: true
            });
            observationIndex = next;
        }

        tick = newTick;
    }

    /// @notice The one read the reader makes: cumulative ticks as of each
    /// `secondsAgos` entry. The second return value is v3's
    /// seconds-per-liquidity accumulator, which this fixture does not model
    /// and the reader does not look at -- it is present so the ABI tuple is
    /// the one a real pool returns.
    function observe(uint32[] calldata secondsAgos)
        external
        view
        returns (int56[] memory tickCumulatives, uint160[] memory secondsPerLiquidityCumulativeX128s)
    {
        tickCumulatives = new int56[](secondsAgos.length);
        secondsPerLiquidityCumulativeX128s = new uint160[](secondsAgos.length);
        for (uint256 i = 0; i < secondsAgos.length; i++) {
            tickCumulatives[i] = observeSingle(secondsAgos[i]);
        }
    }

    function observeSingle(uint32 secondsAgo) private view returns (int56) {
        uint32 time = uint32(block.timestamp);
        require(secondsAgo <= time, "OLD");
        uint32 target = time - secondsAgo;

        Observation memory newest = observations[observationIndex];
        if (newest.blockTimestamp <= target) {
            // Nothing has been written since `target`: the counterfactual
            // observation is the newest one carried forward at the tick that
            // has been in force ever since.
            return newest.tickCumulative + int56(tick) * int56(uint56(target - newest.blockTimestamp));
        }

        Observation memory beforeOrAt;
        Observation memory atOrAfter;
        bool haveBefore = false;
        bool haveAfter = false;
        for (uint16 i = 0; i < observationCardinality; i++) {
            Observation memory candidate = observations[i];
            if (!candidate.initialized) {
                continue;
            }
            if (
                candidate.blockTimestamp <= target
                    && (!haveBefore || candidate.blockTimestamp > beforeOrAt.blockTimestamp)
            ) {
                beforeOrAt = candidate;
                haveBefore = true;
            }
            if (
                candidate.blockTimestamp >= target
                    && (!haveAfter || candidate.blockTimestamp < atOrAfter.blockTimestamp)
            ) {
                atOrAfter = candidate;
                haveAfter = true;
            }
        }

        // The window reaches back past the oldest observation this pool still
        // holds. v3 reverts with exactly this string, and a fresh pool -- one
        // observation, overwritten by every swap -- reverts it for every
        // window there is.
        require(haveBefore, "OLD");
        if (beforeOrAt.blockTimestamp == target) {
            return beforeOrAt.tickCumulative;
        }

        // `newest.blockTimestamp > target` was established above, so an
        // observation at or after the target always exists here.
        require(haveAfter, "OLD");
        uint32 span = atOrAfter.blockTimestamp - beforeOrAt.blockTimestamp;
        uint32 elapsed = target - beforeOrAt.blockTimestamp;
        // Divided before it is multiplied, which is v3-core's own order in
        // `Oracle.observeSingle`: the reader under test has to see the
        // rounding a real pool's interpolation produces, not a better one.
        int56 perSecond = (atOrAfter.tickCumulative - beforeOrAt.tickCumulative) / int56(uint56(span));
        return beforeOrAt.tickCumulative + perSecond * int56(uint56(elapsed));
    }
}
