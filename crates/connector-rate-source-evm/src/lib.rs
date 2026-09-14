//! The first real [`RateSource`] (ADR 0071 decision 6, issue #1293): a
//! **Uniswap-v3-compatible `observe()` TWAP reader**, over an EVM RPC
//! endpoint.
//!
//! An operator names a pool; this crate reads the mean price that pool held
//! over the window the operator set, and hands back a [`Rate`] in base units.
//! It is the dialect Uniswap v3 and Aerodrome Slipstream both carry -- the
//! same `observe(uint32[])`, the same tick cumulatives, the same `OLD` revert
//! -- so one reader covers both venues on every chain either is deployed to
//! (`docs/research/token-pair-price-sources.md`).
//!
//! # What this crate refuses to do
//!
//! Three constraints are the feature, not decoration, and every one of them is
//! a way this reader could have been made "more helpful" and less safe:
//!
//! * **TWAP only.** There is no spot read here and no `slot0` call anywhere in
//!   `crates/` -- ADR 0071 states that absence as its own falsifier, and
//!   `connector-bin`'s `records_state_their_own_falsifier` runs it. A ratio
//!   read inside one block is a number a flash loan sets; the window is the
//!   manipulation resistance, so a window this reader cannot serve is a
//!   [`RateSourceError::WindowNotServed`] rather than a nearby window served
//!   quietly instead.
//! * **No pool discovery.** The reader takes a pool address from the operator
//!   and reads that pool. It never asks a factory what pools exist for a pair,
//!   because a connector picking its own pool picks a dust-liquidity decoy
//!   sooner or later.
//! * **No settlement backend.** This crate takes an RPC endpoint and nothing
//!   else. It holds no key, signs nothing, sends no transaction, and cannot
//!   move value if it tried: every call it makes is an `eth_call` at a pinned
//!   block. ADR 0071 decision 6 keeps the settlement backend out of the value
//!   path, and `tests/the_reader_holds_no_settlement_backend.rs` asserts the
//!   dependency edge does not exist.
//!
//! A pool without the `observe()` interface is **not a source**: Uniswap v4
//! core pools are excluded by name, because an oracle there is an optional
//! per-pool hook that cannot be assumed present. Such an address answers
//! [`RateSourceError::PoolNotFound`] -- there is nothing to fall back to.
//!
//! # What a read costs
//!
//! One `eth_getBlockByNumber` and three `eth_call`s -- `token0()`, `token1()`
//! and `observe([window, 0])` -- all four pinned to the same block number, so
//! a leg is read as of one state of the chain rather than across a moving one.
//! The block's own timestamp is the observation's
//! [`LegObservation::observed_at`]: the venue's clock, never the poller's,
//! which is what the `ttl` guard (ADR 0071 decision 5) has to be read against
//! on a pool whose observations are only written by swaps.
//!
//! Nothing here is on the forwarding path. A background poller drives this
//! reader and writes what it observes into the rate table; a packet only ever
//! reads that table.
//!
//! ```no_run
//! use chrono::Duration;
//! use connector_domain::AssetId;
//! use connector_rate_source::{PoolId, QuoteLeg, RateSource};
//! use connector_rate_source_evm::UniswapV3RateSource;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let source = UniswapV3RateSource::connect("https://mainnet.base.org")?;
//! let observation = source
//!     .observe(&QuoteLeg {
//!         pool: PoolId("0xd0b53d9277642d899df5c87a3966a349a798f224".to_string()),
//!         base: AssetId::evm("0x4200000000000000000000000000000000000006"),
//!         quote: AssetId::evm("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"),
//!         window: Duration::seconds(1_800),
//!     })
//!     .await?;
//! println!("{}", observation.rate);
//! # Ok(())
//! # }
//! ```

pub mod tick_math;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use connector_domain::{AssetChain, AssetId};
use connector_rate_source::{LegObservation, PoolId, QuoteLeg, RateSource, RateSourceError};
use ethers::abi::{parse_abi, Abi, Function, ParamType, Token};
use ethers::providers::{Http, Middleware, Provider, ProviderError, RpcError};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Address, BlockNumber, Bytes, TransactionRequest, U256};
use std::sync::OnceLock;

pub use tick_math::PoolDirection;

/// The three functions this reader calls, and the whole of the interface it
/// requires a pool to have. `slot0` is deliberately absent (ADR 0071's second
/// falsifier), and so is everything else a pool exposes: liquidity, fees,
/// ticks, positions. A source is a price over a window or it is not a source.
const POOL_INTERFACE: [&str; 3] = [
    "function token0() external view returns (address)",
    "function token1() external view returns (address)",
    "function observe(uint32[] secondsAgos) external view returns (int56[], uint160[])",
];

fn pool_interface() -> &'static Abi {
    static ABI: OnceLock<Abi> = OnceLock::new();
    ABI.get_or_init(|| {
        parse_abi(&POOL_INTERFACE).expect("the pool interface above is a valid human-readable ABI")
    })
}

fn pool_function(name: &str) -> &'static Function {
    pool_interface()
        .function(name)
        .expect("the pool interface above declares every function this reader calls")
}

/// A [`RateSource`] that reads Uniswap-v3-compatible `observe()` TWAPs over an
/// EVM JSON-RPC endpoint.
///
/// Holds an endpoint and nothing else -- no key, no channel, no settlement
/// backend, no cached observation. Every [`observe`](RateSource::observe) call
/// goes to the chain, because the alternative to reaching the chain is
/// answering with a number that was true once, and ADR 0071 is explicit that
/// staleness is an outage rather than a fallback.
pub struct UniswapV3RateSource {
    provider: Provider<Http>,
    /// Kept for error messages only. An operator debugging a dead pair needs
    /// to know *which* endpoint stopped answering, and a node may hold one
    /// reader per settlement chain.
    endpoint: String,
}

impl UniswapV3RateSource {
    /// Point a reader at an EVM JSON-RPC endpoint.
    ///
    /// Touches no chain: a reader that probed its endpoint at construction
    /// would be a reader whose *construction* can fail for a reason that has
    /// nothing to do with any pool, and the port already has one honest way to
    /// say an endpoint is unreachable -- saying it when a leg is asked for.
    pub fn connect(rpc_url: &str) -> Result<Self, RateSourceError> {
        let provider = Provider::<Http>::try_from(rpc_url)
            .map_err(|error| RateSourceError::Unreachable(format!("{rpc_url}: {error}")))?;
        Ok(Self {
            provider,
            endpoint: rpc_url.to_string(),
        })
    }

    /// The endpoint this reader was built against.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The chain's own head: the block every call in one read is pinned to,
    /// and the instant that read is dated by.
    async fn head(&self) -> Result<(u64, DateTime<Utc>), RateSourceError> {
        let block = self
            .provider
            .get_block(BlockNumber::Latest)
            .await
            .map_err(|error| self.unreachable(error))?
            .ok_or_else(|| {
                RateSourceError::Unreachable(format!("{}: no latest block", self.endpoint))
            })?;

        let number = block
            .number
            .ok_or_else(|| {
                RateSourceError::Unreachable(format!(
                    "{}: the latest block has no number",
                    self.endpoint
                ))
            })?
            .as_u64();

        let timestamp = Some(block.timestamp)
            .filter(|timestamp| timestamp.bits() <= 63)
            .and_then(|timestamp| i64::try_from(timestamp.low_u64()).ok())
            .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
            .ok_or_else(|| {
                RateSourceError::Unreachable(format!(
                    "{}: block {number} is timestamped {}, which is not an instant",
                    self.endpoint, block.timestamp
                ))
            })?;

        Ok((number, timestamp))
    }

    /// One `eth_call` against `pool`, pinned to `block`.
    async fn call(
        &self,
        pool: Address,
        block: u64,
        calldata: Vec<u8>,
    ) -> Result<Bytes, CallFailure> {
        let request: TypedTransaction = TransactionRequest::new()
            .to(pool)
            .data(Bytes::from(calldata))
            .into();

        self.provider
            .call(&request, Some(BlockNumber::Number(block.into()).into()))
            .await
            .map_err(|error| self.classify(error))
    }

    /// Which of two kinds of failure an RPC error is: the node executed the
    /// call and the contract refused, or the node was not reached at all.
    ///
    /// The distinction carries the whole of the port's third promise. A
    /// revert is the pool telling this reader something true about itself --
    /// `OLD` means the window reaches back past the observations it holds. An
    /// unreachable node tells it nothing, and must never be mistaken for one.
    fn classify(&self, error: ProviderError) -> CallFailure {
        let Some(response) = RpcError::as_error_response(&error) else {
            return CallFailure::Unreachable(format!("{}: {error}", self.endpoint));
        };
        match response.as_revert_data() {
            Some(data) => CallFailure::Reverted(
                revert_reason(&data).unwrap_or_else(|| response.message.clone()),
            ),
            // The node answered, and the answer was an error that is not a
            // revert -- an unsupported method, a pruned block, a rate limit.
            // Not this pool's fault, and not a price.
            None => CallFailure::Unreachable(format!("{}: {response}", self.endpoint)),
        }
    }

    fn unreachable(&self, error: ProviderError) -> RateSourceError {
        match self.classify(error) {
            CallFailure::Unreachable(message) => RateSourceError::Unreachable(message),
            CallFailure::Reverted(reason) => {
                RateSourceError::Unreachable(format!("{}: {reason}", self.endpoint))
            }
        }
    }

    /// `token0()` or `token1()`. A pool that cannot answer either is not a
    /// pool this reader can read -- a v4 core pool, a contract of some other
    /// shape, or an address with no code at all.
    async fn read_token(
        &self,
        pool: Address,
        id: &PoolId,
        block: u64,
        name: &str,
    ) -> Result<Address, RateSourceError> {
        let function = pool_function(name);
        let calldata = function
            .encode_input(&[])
            .expect("token0/token1 take no arguments");

        let returned = match self.call(pool, block, calldata).await {
            Ok(returned) => returned,
            Err(CallFailure::Unreachable(message)) => {
                return Err(RateSourceError::Unreachable(message))
            }
            Err(CallFailure::Reverted(_)) => return Err(RateSourceError::PoolNotFound(id.clone())),
        };

        match function.decode_output(&returned).as_deref() {
            Ok([Token::Address(address)]) => Ok(*address),
            // Includes the empty return an `eth_call` to an address with no
            // code produces: nothing was decoded because nothing ran.
            _ => Err(RateSourceError::PoolNotFound(id.clone())),
        }
    }

    /// `observe([window, 0])`, reduced to the arithmetic mean tick over the
    /// window.
    ///
    /// This is `OracleLibrary.consult`'s arithmetic, ported rather than called
    /// (ADR 0071 decision 6 keeps I/O off the forwarding path, and an on-chain
    /// helper would be a second call for arithmetic this crate can do): the
    /// difference of the two cumulatives divided by the window, rounded
    /// **toward negative infinity** rather than toward zero. That correction
    /// is not decoration -- Rust's integer division truncates, so without it
    /// every negative mean tick would come out one tick too high, which is one
    /// part in ten thousand of the price, in the same direction, every time.
    async fn read_mean_tick(
        &self,
        pool: Address,
        id: &PoolId,
        block: u64,
        window: u32,
    ) -> Result<i32, RateSourceError> {
        let function = pool_function("observe");
        let calldata = function
            .encode_input(&[Token::Array(vec![
                Token::Uint(U256::from(window)),
                Token::Uint(U256::zero()),
            ])])
            .expect("observe takes one uint32[]");

        let returned = match self.call(pool, block, calldata).await {
            Ok(returned) => returned,
            Err(CallFailure::Unreachable(message)) => {
                return Err(RateSourceError::Unreachable(message))
            }
            // v3's oracle reverts `OLD` when the window reaches back past the
            // observations the pool holds -- which is every window on a pool
            // whose observation cardinality nobody has grown, since it is
            // initialised with an array of one. Anything else a pool reverts
            // is a pool this reader cannot read.
            Err(CallFailure::Reverted(reason)) if reason.contains("OLD") => {
                return Err(RateSourceError::WindowNotServed {
                    pool: id.clone(),
                    window: Duration::seconds(i64::from(window)),
                })
            }
            Err(CallFailure::Reverted(_)) => return Err(RateSourceError::PoolNotFound(id.clone())),
        };

        let decoded = function
            .decode_output(&returned)
            .map_err(|_| RateSourceError::PoolNotFound(id.clone()))?;
        let Some(Token::Array(cumulatives)) = decoded.first() else {
            return Err(RateSourceError::PoolNotFound(id.clone()));
        };
        let [older, newer] = cumulatives.as_slice() else {
            return Err(RateSourceError::PoolNotFound(id.clone()));
        };

        let older =
            tick_cumulative(older).ok_or_else(|| RateSourceError::PoolNotFound(id.clone()))?;
        let newer =
            tick_cumulative(newer).ok_or_else(|| RateSourceError::PoolNotFound(id.clone()))?;

        let elapsed = i128::from(window);
        let accumulated = newer - older;
        let mut mean = accumulated / elapsed;
        if accumulated < 0 && accumulated % elapsed != 0 {
            mean -= 1;
        }

        i32::try_from(mean).map_err(|_| RateSourceError::PoolNotFound(id.clone()))
    }
}

#[async_trait]
impl RateSource for UniswapV3RateSource {
    async fn observe(&self, leg: &QuoteLeg) -> Result<LegObservation, RateSourceError> {
        let pool = pool_address(&leg.pool)?;
        let window = window_seconds(leg)?;

        let (block, observed_at) = self.head().await?;
        let token0 = self.read_token(pool, &leg.pool, block, "token0").await?;
        let token1 = self.read_token(pool, &leg.pool, block, "token1").await?;
        let direction = direction(leg, token0, token1)?;

        let mean_tick = self.read_mean_tick(pool, &leg.pool, block, window).await?;
        let rate = tick_math::rate_at_tick(mean_tick, direction).map_err(|error| match error {
            // A tick a v3 pool cannot hold did not come from a v3 pool.
            tick_math::TickMathError::TickOutOfRange { .. } => {
                RateSourceError::PoolNotFound(leg.pool.clone())
            }
            tick_math::TickMathError::Unrepresentable {
                numerator,
                denominator,
                ..
            } => RateSourceError::Unrepresentable {
                numerator,
                denominator,
            },
        })?;

        Ok(LegObservation {
            leg: leg.clone(),
            rate,
            observed_at,
        })
    }
}

/// The two shapes an `eth_call` can fail in, which the port turns into three
/// different errors depending on which call was being made.
enum CallFailure {
    /// The node executed the call and the contract refused, with this reason.
    Reverted(String),
    /// The node was not reached, or answered with something that is not a
    /// revert.
    Unreachable(String),
}

/// A pool identifier is an EVM contract address. A name this reader cannot
/// parse is a pool it does not have -- refused where it is read rather than
/// carried until some later call fails for a stranger reason.
fn pool_address(id: &PoolId) -> Result<Address, RateSourceError> {
    id.0.parse()
        .map_err(|_| RateSourceError::PoolNotFound(id.clone()))
}

/// The window in whole seconds, which is the only window `observe` can be
/// asked for.
///
/// Refused at both ends of "not a window at all": zero or negative is a spot
/// read by another name, and a fraction of a second is a window this reader
/// would have to round to serve. Rounding it would substitute the reader's
/// judgement for the operator's guard, and the operator set that guard
/// deliberately.
fn window_seconds(leg: &QuoteLeg) -> Result<u32, RateSourceError> {
    let not_served = || RateSourceError::WindowNotServed {
        pool: leg.pool.clone(),
        window: leg.window,
    };

    let seconds = leg.window.num_seconds();
    if leg.window != Duration::seconds(seconds) {
        return Err(not_served());
    }
    u32::try_from(seconds)
        .ok()
        .filter(|seconds| *seconds > 0)
        .ok_or_else(not_served)
}

/// Which way round the leg reads this pool's own token ordering, or a refusal
/// naming the pair the caller asked for.
///
/// A pool holds its pair in one order; a leg names the order it wants. Both of
/// the leg's tokens must be the pool's two, and refusing otherwise is what
/// keeps an operator who named their ANYONE/WETH pool for an ANYONE/USDC leg
/// from being handed a perfectly plausible-looking WETH number.
fn direction(
    leg: &QuoteLeg,
    token0: Address,
    token1: Address,
) -> Result<PoolDirection, RateSourceError> {
    let not_in_pool = || RateSourceError::PairNotInPool {
        pool: leg.pool.clone(),
        base: leg.base.clone(),
        quote: leg.quote.clone(),
    };

    let base = evm_address(&leg.base).ok_or_else(not_in_pool)?;
    let quote = evm_address(&leg.quote).ok_or_else(not_in_pool)?;

    if base == token0 && quote == token1 {
        Ok(PoolDirection::Token1PerToken0)
    } else if base == token1 && quote == token0 {
        Ok(PoolDirection::Token0PerToken1)
    } else {
        Err(not_in_pool())
    }
}

/// An [`AssetId`]'s contract address, where it names one. A Solana mint names
/// no address on this chain, and neither does an EVM identity this reader
/// cannot parse -- both are simply not in the pool.
fn evm_address(asset: &AssetId) -> Option<Address> {
    (asset.chain() == AssetChain::Evm)
        .then(|| asset.token().parse().ok())
        .flatten()
}

/// One `int56` out of an ABI word.
///
/// `decode_output` hands back the raw 256-bit word for an `int56`, so the sign
/// lives in bit 55 and the bits above it are the ABI's sign extension. Masked
/// and re-signed here rather than trusted, because a contract answering
/// `observe` with a word that is not a sign-extended `int56` is not a pool
/// this reader should compute a price from.
fn tick_cumulative(token: &Token) -> Option<i128> {
    let Token::Int(raw) = token else {
        return None;
    };

    const WIDTH: u32 = 56;
    let magnitude = raw.low_u64() & ((1u64 << WIDTH) - 1);
    let negative = magnitude & (1u64 << (WIDTH - 1)) != 0;

    // The sign extension the word must carry for the masked value to be the
    // whole of it: every bit above 55 set for a negative, clear for a
    // positive.
    let expected = if negative {
        (U256::MAX << WIDTH) | U256::from(magnitude)
    } else {
        U256::from(magnitude)
    };
    if *raw != expected {
        return None;
    }

    Some(if negative {
        i128::from(magnitude) - (1i128 << WIDTH)
    } else {
        i128::from(magnitude)
    })
}

/// The string inside a `revert("...")`, where the returned data carries one.
/// A custom error, or a revert with no data at all, has no string to give.
fn revert_reason(data: &[u8]) -> Option<String> {
    /// `Error(string)`, the selector solc puts in front of every
    /// `revert("...")` and `require(..., "...")` reason.
    const ERROR_STRING: [u8; 4] = [0x08, 0xc3, 0x79, 0xa0];

    let payload = data.strip_prefix(&ERROR_STRING[..])?;
    match ethers::abi::decode(&[ParamType::String], payload).as_deref() {
        Ok([Token::String(reason)]) => Some(reason.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use connector_domain::AssetId;

    fn leg(window: Duration) -> QuoteLeg {
        QuoteLeg {
            pool: PoolId("0x1111111111111111111111111111111111111111".to_string()),
            base: AssetId::evm("0x2222222222222222222222222222222222222222"),
            quote: AssetId::evm("0x3333333333333333333333333333333333333333"),
            window,
        }
    }

    #[test]
    fn a_pool_identifier_that_is_not_an_address_is_a_pool_this_reader_does_not_have() {
        let id = PoolId("the-anyone-weth-pool".to_string());
        assert_eq!(
            pool_address(&id).unwrap_err(),
            RateSourceError::PoolNotFound(id)
        );
    }

    #[test]
    fn a_window_that_is_not_whole_seconds_of_real_time_is_not_served() {
        for window in [
            Duration::zero(),
            Duration::seconds(-60),
            Duration::milliseconds(1_500),
            Duration::seconds(i64::from(u32::MAX) + 1),
        ] {
            let error = window_seconds(&leg(window)).unwrap_err();
            assert!(
                matches!(error, RateSourceError::WindowNotServed { .. }),
                "{window} must not be served, got {error}"
            );
        }

        assert_eq!(window_seconds(&leg(Duration::seconds(1_800))), Ok(1_800));
    }

    #[test]
    fn a_leg_is_read_in_the_direction_it_names() {
        let leg = leg(Duration::seconds(600));
        let base: Address = "0x2222222222222222222222222222222222222222"
            .parse()
            .expect("an address");
        let quote: Address = "0x3333333333333333333333333333333333333333"
            .parse()
            .expect("an address");

        assert_eq!(
            direction(&leg, base, quote),
            Ok(PoolDirection::Token1PerToken0)
        );
        assert_eq!(
            direction(&leg, quote, base),
            Ok(PoolDirection::Token0PerToken1)
        );
    }

    #[test]
    fn a_pair_the_named_pool_does_not_hold_is_refused_rather_than_answered() {
        let leg = leg(Duration::seconds(600));
        let base: Address = "0x2222222222222222222222222222222222222222"
            .parse()
            .expect("an address");
        let stranger: Address = "0x4444444444444444444444444444444444444444"
            .parse()
            .expect("an address");

        assert_eq!(
            direction(&leg, base, stranger),
            Err(RateSourceError::PairNotInPool {
                pool: leg.pool.clone(),
                base: leg.base.clone(),
                quote: leg.quote.clone(),
            })
        );
    }

    #[test]
    fn a_token_on_another_chain_is_not_in_an_evm_pool() {
        let leg = QuoteLeg {
            base: AssetId::solana("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"),
            ..leg(Duration::seconds(600))
        };
        let token0: Address = "0x2222222222222222222222222222222222222222"
            .parse()
            .expect("an address");
        let token1: Address = "0x3333333333333333333333333333333333333333"
            .parse()
            .expect("an address");

        assert!(matches!(
            direction(&leg, token0, token1),
            Err(RateSourceError::PairNotInPool { .. })
        ));
    }

    #[test]
    fn an_int56_word_is_read_with_its_sign() {
        assert_eq!(tick_cumulative(&Token::Int(U256::zero())), Some(0));
        assert_eq!(
            tick_cumulative(&Token::Int(U256::from(15_134_940u64))),
            Some(15_134_940)
        );
        // -15_134_940 as a sign-extended 256-bit word.
        let negative = U256::zero().overflowing_sub(U256::from(15_134_940u64)).0;
        assert_eq!(tick_cumulative(&Token::Int(negative)), Some(-15_134_940));
    }

    #[test]
    fn a_word_that_is_not_a_sign_extended_int56_is_not_a_tick_cumulative() {
        // Bit 56 set: too wide to be an `int56`, whatever contract produced it.
        assert_eq!(tick_cumulative(&Token::Int(U256::one() << 56)), None);
        assert_eq!(tick_cumulative(&Token::Uint(U256::zero())), None);
    }

    #[test]
    fn a_revert_reason_is_read_out_of_the_returned_data() {
        let encoded = ethers::abi::encode(&[Token::String("OLD".to_string())]);
        let mut data = vec![0x08, 0xc3, 0x79, 0xa0];
        data.extend_from_slice(&encoded);
        assert_eq!(revert_reason(&data), Some("OLD".to_string()));

        assert_eq!(revert_reason(&[]), None);
        assert_eq!(revert_reason(&[0x71, 0x38, 0x35, 0x6f]), None);
    }

    #[test]
    fn the_pool_interface_is_the_three_functions_and_no_spot_read() {
        let names: Vec<&str> = pool_interface()
            .functions()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names.len(), 3, "got {names:?}");
        for expected in ["token0", "token1", "observe"] {
            assert!(
                names.contains(&expected),
                "{expected} is missing from {names:?}"
            );
        }
    }
}
