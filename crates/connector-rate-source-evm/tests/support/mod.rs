//! A real Uniswap-v3-shaped venue on a real, disposable chain: what both of
//! this crate's tier-3 test binaries stand up before they read a price.
//!
//! Tier 3 applies here because chain behaviour is the subject (ADR 0007).
//! What is under test is not "does this arithmetic work" -- `src/tick_math.rs`
//! settles that without a chain -- but the things only a chain can answer:
//! that `observe(uint32[])` encodes and decodes the way this reader thinks it
//! does, that a two's-complement `int56` comes back with its sign, that a
//! window reaching past a pool's observations arrives as a revert this reader
//! recognises, and that an address with no code is a refusal rather than a
//! number.
//!
//! So this spawns its own `anvil` (the shared harness from
//! `connector-settlement-evm`, issue #542 -- never the `docker-compose`
//! containers, which have nothing to do with the test gate), builds
//! `contracts/OracleMockPool.sol` with a real `forge`, deploys it, drives it
//! through a scripted timeline of swaps, and hands the tests a reader pointed
//! at the result.

// Each integration-test binary compiles this module separately and uses a
// different subset of it; under clippy's `-D warnings` on `--all-targets`
// whatever a given binary does not touch is a hard `dead_code` error.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use connector_rate_source::PoolId;
use connector_settlement_evm::test_support::{Anvil, DEPLOYER_PRIVATE_KEY};
use ethers::abi::{Abi, Token};
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Middleware, Provider};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::{Address, Bytes, TransactionRequest, U256};

/// `Anvil::spawn`'s own chain id.
const CHAIN_ID: u64 = 31_337;

/// The three token identities the fixture pools hold. Never deployed: this
/// reader reads a pool's `token0()`/`token1()` to decide which way round a leg
/// reads, and never calls the tokens themselves -- not even `decimals()`,
/// because a v3 tick is already a ratio of base units (ADR 0071 decision 4).
/// Addresses that sort in this order, so `token0 < token1` holds the way a
/// real pool's does.
pub const ANYONE: &str = "0x00000000000000000000000000000000000000a1";
pub const USDC: &str = "0x00000000000000000000000000000000000000b2";
pub const WETH: &str = "0x00000000000000000000000000000000000000c3";

/// A parseable address with no contract at it.
pub const NO_POOL_HERE: &str = "0x000000000000000000000000000000000000dead";

/// The tick the ANYONE/WETH pool opens at, and the one a swap moves it to
/// partway through the window. Both 18-decimal tokens, so the base-unit ratio
/// is the human one: about 1/300100 of an ether per ANYONE.
pub const ANYONE_WETH_TICK_BEFORE: i32 = -126_115;
pub const ANYONE_WETH_TICK_AFTER: i32 = -126_134;

/// The tick the USDC/WETH pool holds throughout. USDC is that pool's `token0`,
/// so read as WETH-in-USDC -- the direction the quote path needs -- it is the
/// reciprocal: about 3001 USDC to the ether at 18-against-6 decimals.
pub const USDC_WETH_TICK: i32 = 196_253;

/// The window both fixture legs are read over.
pub const WINDOW_SECONDS: i64 = 120;

/// The mean tick the ANYONE/WETH pool's timeline produces over that window --
/// 60 seconds at each of the two ticks above, with the window's edges falling
/// two seconds inside each end, and the division rounded toward negative
/// infinity the way `OracleLibrary.consult` rounds it.
pub const ANYONE_WETH_MEAN_TICK: i32 = -126_125;

/// A stood-up venue: a disposable chain with three pools on it, and the
/// timeline that put observations in them.
pub struct Venue {
    /// Held in an `Option` so a test can take the chain away mid-life --
    /// which is the only shape that proves the port's "an unreachable source
    /// is an error, not a stale value" promise, since a source that was never
    /// reachable has no stale value to hand back.
    chain: Arc<Mutex<Option<Anvil>>>,
    pub rpc_url: String,
    /// ANYONE/WETH: `token0` is ANYONE. Grown to four observations and swapped
    /// twice, so it serves the fixture window.
    pub anyone_weth: PoolId,
    /// USDC/WETH: `token0` is USDC. Grown and swapped the same way.
    pub usdc_weth: PoolId,
    /// ANYONE/WETH again, deployed late and never swapped: its one observation
    /// is younger than the fixture window's far edge, so that window reaches
    /// back past everything it holds.
    pub too_young: PoolId,
    /// The timestamp of the block every read below is pinned to -- the chain's
    /// own clock, which is what a leg observed here must be dated by.
    pub head: DateTime<Utc>,
}

impl Venue {
    /// Spawn a chain, deploy the pools, and drive the timeline.
    ///
    /// Every block's timestamp is set explicitly with
    /// `evm_setNextBlockTimestamp`. A test that let anvil timestamp its own
    /// blocks from the wall clock would be asserting a TWAP against a window
    /// whose length it does not know, and would pass or fail on how busy the
    /// runner was.
    pub async fn stand_up(base_port: u16) -> Venue {
        let chain = Anvil::spawn(base_port).await;
        let rpc_url = chain.rpc_url.clone();
        let client = client(&rpc_url);
        let (abi, bytecode) = oracle_mock_pool();

        let base = latest_timestamp(&client).await + 10;

        // The two pools the quote path runs through, opened before the window
        // it will be read over.
        set_next_block_timestamp(&client, base).await;
        let anyone_weth = deploy(
            &client,
            &abi,
            &bytecode,
            address(ANYONE),
            address(WETH),
            ANYONE_WETH_TICK_BEFORE,
        )
        .await;

        set_next_block_timestamp(&client, base + 1).await;
        let usdc_weth = deploy(
            &client,
            &abi,
            &bytecode,
            address(USDC),
            address(WETH),
            USDC_WETH_TICK,
        )
        .await;

        // Somebody pays for the observation slots. A pool is initialised with
        // an array of one, and until this is called every swap overwrites the
        // same slot and no window is servable at all.
        set_next_block_timestamp(&client, base + 2).await;
        grow(&client, &abi, anyone_weth, 4).await;
        set_next_block_timestamp(&client, base + 3).await;
        grow(&client, &abi, usdc_weth, 4).await;

        // Sixty seconds in, a swap moves ANYONE/WETH's tick. The window read
        // later straddles this, which is what makes its mean tick something
        // neither of the two ticks is.
        set_next_block_timestamp(&client, base + 60).await;
        swap(&client, &abi, anyone_weth, ANYONE_WETH_TICK_AFTER).await;
        set_next_block_timestamp(&client, base + 61).await;
        swap(&client, &abi, usdc_weth, USDC_WETH_TICK).await;

        set_next_block_timestamp(&client, base + 62).await;
        let too_young = deploy(
            &client,
            &abi,
            &bytecode,
            address(ANYONE),
            address(WETH),
            ANYONE_WETH_TICK_BEFORE,
        )
        .await;

        set_next_block_timestamp(&client, base + 120).await;
        swap(&client, &abi, anyone_weth, ANYONE_WETH_TICK_AFTER).await;
        set_next_block_timestamp(&client, base + 121).await;
        swap(&client, &abi, usdc_weth, USDC_WETH_TICK).await;

        // One last block, so every read below is pinned to a head nothing
        // moves afterwards.
        set_next_block_timestamp(&client, base + 122).await;
        mine(&client).await;

        Venue {
            chain: Arc::new(Mutex::new(Some(chain))),
            rpc_url,
            anyone_weth: pool_id(anyone_weth),
            usdc_weth: pool_id(usdc_weth),
            too_young: pool_id(too_young),
            head: instant(base + 122),
        }
    }

    /// Take the chain away. The reader keeps its endpoint and its knowledge of
    /// every pool on it, and must now refuse rather than answer.
    pub fn outage(&self) -> Arc<Mutex<Option<Anvil>>> {
        Arc::clone(&self.chain)
    }
}

/// A chain holding one pool that is being swapped without ever having had its
/// observation cardinality grown -- the state a freshly created pool is in,
/// and the reason naming a pool is not on its own enough to read one
/// (`docs/research/token-pair-price-sources.md`).
pub struct UngrownPool {
    _chain: Anvil,
    pub rpc_url: String,
    pub pool: PoolId,
    client: Client,
    abi: Abi,
    address: Address,
    timestamp: u64,
}

impl UngrownPool {
    /// Deploy a pool and swap it twice, sixty seconds apart, leaving its
    /// single observation slot holding only the most recent of them.
    pub async fn stand_up(base_port: u16) -> UngrownPool {
        let chain = Anvil::spawn(base_port).await;
        let rpc_url = chain.rpc_url.clone();
        let client = client(&rpc_url);
        let (abi, bytecode) = oracle_mock_pool();

        let base = latest_timestamp(&client).await + 10;
        set_next_block_timestamp(&client, base).await;
        let address = deploy(
            &client,
            &abi,
            &bytecode,
            self::address(ANYONE),
            self::address(WETH),
            ANYONE_WETH_MEAN_TICK,
        )
        .await;

        set_next_block_timestamp(&client, base + 40).await;
        swap(&client, &abi, address, ANYONE_WETH_MEAN_TICK).await;
        set_next_block_timestamp(&client, base + 80).await;
        swap(&client, &abi, address, ANYONE_WETH_MEAN_TICK).await;
        set_next_block_timestamp(&client, base + 81).await;
        mine(&client).await;

        UngrownPool {
            _chain: chain,
            rpc_url,
            pool: pool_id(address),
            client,
            abi,
            address,
            timestamp: base + 81,
        }
    }

    /// Pay for the slots, then swap twice more so two of them are filled. The
    /// same window that reached back past everything the pool held now lands
    /// inside its history.
    pub async fn grow_and_keep_swapping(&mut self) {
        set_next_block_timestamp(&self.client, self.timestamp + 1).await;
        grow(&self.client, &self.abi, self.address, 4).await;

        set_next_block_timestamp(&self.client, self.timestamp + 59).await;
        swap(&self.client, &self.abi, self.address, ANYONE_WETH_MEAN_TICK).await;
        set_next_block_timestamp(&self.client, self.timestamp + 119).await;
        swap(&self.client, &self.abi, self.address, ANYONE_WETH_MEAN_TICK).await;
        set_next_block_timestamp(&self.client, self.timestamp + 120).await;
        mine(&self.client).await;
        self.timestamp += 120;
    }
}

type Client = Arc<SignerMiddleware<Provider<Http>, LocalWallet>>;

fn client(rpc_url: &str) -> Client {
    let provider = Provider::<Http>::try_from(rpc_url)
        .expect("build a provider")
        // Anvil mines instantly, and the default seven-second receipt poll
        // would make a scripted timeline of ten transactions take a minute.
        .interval(StdDuration::from_millis(10));
    let wallet = DEPLOYER_PRIVATE_KEY
        .parse::<LocalWallet>()
        .expect("anvil's first dev key")
        .with_chain_id(CHAIN_ID);
    Arc::new(SignerMiddleware::new(provider, wallet))
}

fn instant(timestamp: u64) -> DateTime<Utc> {
    DateTime::from_timestamp(
        i64::try_from(timestamp).expect("a fixture timestamp is an instant"),
        0,
    )
    .expect("a fixture timestamp is an instant")
}

pub fn address(hex: &str) -> Address {
    hex.parse().expect("a 20-byte hex address")
}

fn pool_id(address: Address) -> PoolId {
    PoolId(format!("{address:?}"))
}

/// `contracts/OracleMockPool.sol`, compiled by a real `forge`.
///
/// Compiled once per test process and cached: `cargo test` runs a crate's test
/// binaries one at a time, and within one binary several tests stand up venues
/// concurrently, which would otherwise have them racing in the same `out/`
/// directory.
fn oracle_mock_pool() -> (Abi, Bytes) {
    static ARTIFACT: OnceLock<(Abi, Bytes)> = OnceLock::new();
    ARTIFACT
        .get_or_init(|| {
            let root = contracts_dir();
            let status = Command::new("forge")
                .arg("build")
                .current_dir(&root)
                .stdout(Stdio::null())
                .status()
                .expect("run forge build (is `forge` on PATH? see foundryup)");
            assert!(status.success(), "forge build failed in {}", root.display());

            let path = root.join("out/OracleMockPool.sol/OracleMockPool.json");
            let artifact: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
            )
            .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));

            let abi: Abi = serde_json::from_value(artifact["abi"].clone())
                .expect("the forge artifact carries an ABI");
            let bytecode: Bytes = artifact["bytecode"]["object"]
                .as_str()
                .expect("the forge artifact carries creation bytecode")
                .parse()
                .expect("creation bytecode is 0x-prefixed hex");
            (abi, bytecode)
        })
        .clone()
}

fn contracts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts")
}

/// True if `forge --version` runs. The `anvil` twin of this lives in
/// `connector_settlement_evm::test_support`.
pub fn forge_available() -> bool {
    Command::new("forge")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// The `forge` half of the "fail loudly in CI, skip locally" gate (issue
/// #471). Real Solidity is compiled and deployed here, so a CI run without
/// Foundry must fail rather than report a pass it did not earn; a local run
/// without it skips, because requiring every contributor to install Foundry to
/// run `cargo test` is a cost this crate does not need to impose.
pub fn require_forge() -> bool {
    if forge_available() {
        return true;
    }
    if std::env::var_os("CI").is_some() {
        panic!(
            "forge is not on PATH, but CI is set -- the Rust Workspace Gate must install \
             Foundry (foundry-rs/foundry-toolchain) before this crate's tests run. Refusing to \
             silently skip and report success here; see issue #471."
        );
    }
    eprintln!(
        "skipping: forge is not on PATH (install Foundry: https://getfoundry.sh) -- this test \
         compiles and deploys real Solidity and only skips because this is not a CI run"
    );
    false
}

async fn latest_timestamp(client: &Client) -> u64 {
    client
        .get_block(ethers::types::BlockNumber::Latest)
        .await
        .expect("read the latest block")
        .expect("anvil has a genesis block")
        .timestamp
        .as_u64()
}

async fn set_next_block_timestamp(client: &Client, timestamp: u64) {
    let _: serde_json::Value = client
        .provider()
        .request("evm_setNextBlockTimestamp", serde_json::json!([timestamp]))
        .await
        .expect("anvil sets the next block's timestamp");
}

async fn mine(client: &Client) {
    let _: serde_json::Value = client
        .provider()
        .request("evm_mine", serde_json::json!([]))
        .await
        .expect("anvil mines a block");
}

async fn deploy(
    client: &Client,
    abi: &Abi,
    bytecode: &Bytes,
    token0: Address,
    token1: Address,
    tick: i32,
) -> Address {
    let data = abi
        .constructor()
        .expect("OracleMockPool declares a constructor")
        .encode_input(
            bytecode.to_vec(),
            &[Token::Address(token0), Token::Address(token1), signed(tick)],
        )
        .expect("encode the constructor arguments");

    send(client, TransactionRequest::new().data(data))
        .await
        .contract_address
        .expect("a deployment receipt names the contract's address")
}

async fn grow(client: &Client, abi: &Abi, pool: Address, cardinality: u16) {
    call(
        client,
        abi,
        pool,
        "increaseObservationCardinalityNext",
        &[Token::Uint(U256::from(cardinality))],
    )
    .await;
}

async fn swap(client: &Client, abi: &Abi, pool: Address, tick: i32) {
    call(client, abi, pool, "swap", &[signed(tick)]).await;
}

async fn call(client: &Client, abi: &Abi, pool: Address, name: &str, args: &[Token]) {
    let data = abi
        .function(name)
        .unwrap_or_else(|_| panic!("OracleMockPool declares {name}"))
        .encode_input(args)
        .unwrap_or_else(|error| panic!("encode {name}: {error}"));
    send(client, TransactionRequest::new().to(pool).data(data)).await;
}

async fn send(client: &Client, request: TransactionRequest) -> ethers::types::TransactionReceipt {
    let receipt = client
        .send_transaction(request, None)
        .await
        .expect("submit the transaction")
        .await
        .expect("wait for the receipt")
        .expect("anvil mines instantly, so a receipt is available");
    assert_eq!(
        receipt.status,
        Some(1.into()),
        "the fixture's own transaction reverted"
    );
    receipt
}

/// A signed integer as an ABI word. `int24` is two's complement over 256 bits
/// like every other signed ABI type.
fn signed(value: i32) -> Token {
    let magnitude = U256::from(value.unsigned_abs());
    Token::Int(if value < 0 {
        U256::zero().overflowing_sub(magnitude).0
    } else {
        magnitude
    })
}
