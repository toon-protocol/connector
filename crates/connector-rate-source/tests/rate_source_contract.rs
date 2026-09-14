//! The `RateSource` contract suite, reached the way a real reader's crate
//! will reach it (ADR 0007, issue #1291): from another crate's
//! `[dev-dependencies]`, through `features = ["test-util"]`, rather than
//! from inside the port crate's own test build.
//!
//! An integration test is a separate crate, so this file compiles against
//! `connector-rate-source` exactly as `connector-rate-source-evm`'s own
//! tests will (issue #1293) -- if the feature gate ever stopped exporting
//! the suite, this stops compiling here rather than in somebody else's
//! repository-shaped surprise.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use connector_domain::{AssetId, Rate};
use connector_rate_source::contract::{assert_upholds_the_contract, ContractFixture};
use connector_rate_source::{InMemoryRateSource, PoolContents, PoolId, QuoteLeg, RateSource};

const ANYONE_WETH: &str = "0x1111111111111111111111111111111111111111";
const WETH_USDC: &str = "0x2222222222222222222222222222222222222222";

fn anyone() -> AssetId {
    AssetId::evm("0x0000000000000000000000000000000000009876")
}

fn weth() -> AssetId {
    AssetId::evm("0x4200000000000000000000000000000000000006")
}

fn usdc() -> AssetId {
    AssetId::evm("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913")
}

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(seconds, 0).expect("a timestamp")
}

#[tokio::test]
async fn the_in_memory_rate_source_upholds_the_contract() {
    assert_upholds_the_contract(|| async {
        let source = Arc::new(InMemoryRateSource::new());
        let window = Duration::seconds(600);

        // A thin token quoted in an intermediate, the intermediate quoted
        // in the numeraire: ADR 0071 decision 3's two-pool quote, which is
        // the shape ANYONE's own only honest venue forces.
        source.insert_pool(
            PoolId(ANYONE_WETH.to_string()),
            PoolContents {
                base: anyone(),
                quote: weth(),
                rate: Rate::new(1, 250_000).expect("a rate"),
                observed_at: at(1_757_499_400),
                shortest_window: Duration::seconds(120),
                longest_window: Duration::seconds(3_600),
            },
        );
        source.insert_pool(
            PoolId(WETH_USDC.to_string()),
            PoolContents {
                base: weth(),
                quote: usdc(),
                rate: Rate::new(3_500_000_000, 1_000_000_000_000_000_000).expect("a rate"),
                observed_at: at(1_757_500_000),
                shortest_window: Duration::seconds(120),
                longest_window: Duration::seconds(3_600),
            },
        );

        ContractFixture {
            source: Arc::clone(&source) as Arc<dyn RateSource>,
            leg: QuoteLeg {
                pool: PoolId(ANYONE_WETH.to_string()),
                base: anyone(),
                quote: weth(),
                window,
            },
            next_leg: QuoteLeg {
                pool: PoolId(WETH_USDC.to_string()),
                base: weth(),
                quote: usdc(),
                window,
            },
            absent_pool: PoolId("0x3333333333333333333333333333333333333333".to_string()),
            asset_the_pool_does_not_hold: usdc(),
            window_shorter_than_the_source_can_serve: Duration::seconds(60),
            make_unreachable: {
                let source = Arc::clone(&source);
                Box::new(move || {
                    let source = Arc::clone(&source);
                    Box::pin(async move { source.cut_off() })
                })
            },
        }
    })
    .await;
}
