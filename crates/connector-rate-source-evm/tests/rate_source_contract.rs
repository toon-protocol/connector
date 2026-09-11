//! The `RateSource` contract suite (ADR 0007), run **unmodified** against the
//! Uniswap-v3-compatible reader and a real chain (issue #1293).
//!
//! The in-memory fake in `connector-rate-source` passes the same four
//! assertions from a `HashMap`. That a chain-backed reader passes them from an
//! `eth_call` is the point of defining the port once: the poller above it, and
//! the rate table above that, cannot tell the two apart, and a second reader
//! -- Raydium CLMM on Solana, when somebody writes it -- joins on the same
//! terms or does not join.
//!
//! Tier 3, so it spawns its own `anvil` and compiles its own Solidity, and
//! panics rather than skipping when the chain binaries are missing under `CI`.

mod support;

use std::sync::Arc;

use chrono::Duration;
use connector_domain::AssetId;
use connector_rate_source::contract::{assert_upholds_the_contract, ContractFixture};
use connector_rate_source::{PoolId, QuoteLeg, RateSource};
use connector_rate_source_evm::UniswapV3RateSource;
use connector_settlement_evm::test_support::require_anvil;

/// Distinct from every other test binary's base port, so binaries running
/// concurrently do not contend for one range.
const ANVIL_BASE_PORT: u16 = 19_200;

#[tokio::test]
async fn the_uniswap_v3_twap_reader_upholds_the_rate_source_contract() {
    if !require_anvil() || !support::require_forge() {
        return;
    }

    assert_upholds_the_contract(|| async {
        let venue = support::Venue::stand_up(ANVIL_BASE_PORT).await;
        let source = Arc::new(
            UniswapV3RateSource::connect(&venue.rpc_url).expect("a reader for a spawned chain"),
        );
        let window = Duration::seconds(support::WINDOW_SECONDS);

        // The chain itself, so the last promise can take it away. Held past
        // `venue`'s own lifetime: an `Anvil` is killed when it drops, and this
        // is what decides when that happens.
        let chain = venue.outage();

        ContractFixture {
            source: source as Arc<dyn RateSource>,
            // ANYONE quoted in WETH, then WETH quoted in USDC: ADR 0071
            // decision 3's two-pool quote, which is the shape a thin token's
            // only honest venue forces.
            leg: QuoteLeg {
                pool: venue.anyone_weth.clone(),
                base: AssetId::evm(support::ANYONE),
                quote: AssetId::evm(support::WETH),
                window,
            },
            next_leg: QuoteLeg {
                pool: venue.usdc_weth.clone(),
                base: AssetId::evm(support::WETH),
                quote: AssetId::evm(support::USDC),
                window,
            },
            // An address this reader parses perfectly well and finds nothing
            // at -- not a label it would have refused before looking.
            absent_pool: PoolId(support::NO_POOL_HERE.to_string()),
            asset_the_pool_does_not_hold: AssetId::evm(support::USDC),
            // A zero-second window is a spot read by another name, and this
            // reader has no spot read (ADR 0071 decision 3).
            window_shorter_than_the_source_can_serve: Duration::zero(),
            make_unreachable: Box::new(move || {
                let chain = Arc::clone(&chain);
                Box::pin(async move {
                    // Dropping the harness kills the `anvil` process, so the
                    // endpoint this reader still holds stops answering --
                    // an outage midway through a source's life, with a
                    // perfectly good last observation it must not hand back.
                    drop(chain.lock().expect("the chain is not poisoned").take());
                })
            }),
        }
    })
    .await;
}
