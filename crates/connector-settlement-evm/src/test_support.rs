//! A real, disposable `anvil` chain harness other crates' tests reuse
//! rather than reimplementing (issue #542): `connector-bin`'s devnet-config
//! test and `connector-cli`'s settlement-construction tests all need
//! exactly what this crate's own `tests/support/mod.rs` already provides
//! its own integration tests. Gated behind the `test-util` feature for the
//! same reason `connector-operator`'s own `test_support` module is: a
//! downstream crate's tests cannot see anything behind `#[cfg(test)]`,
//! since that cfg is only active while this crate compiles its own test
//! binary.

pub mod x402;

use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ethers::providers::{Http, Middleware, Provider};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::{Address, U256};

mod mock_erc20 {
    // A plain, mintable ERC-20 with no EIP-3009 (`contracts/MockERC20.sol`):
    // the token a test needs when its subject is the Permit2 deposit route,
    // or any token that is not Circle's FiatToken.
    ethers::contract::abigen!(MockErc20, "./contracts/MockERC20.json");
}

/// Anvil's first well-known dev account -- the same one `infra/anvil/seed.sh`
/// uses as its deployer, so this test harness's choice of key is not a new
/// convention.
pub const DEPLOYER_PRIVATE_KEY: &str =
    "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

/// Anvil's *second* well-known dev account (`0x7099…79C8`), genesis-funded
/// with ETH exactly like [`DEPLOYER_PRIVATE_KEY`]: the other node of a test
/// that needs two, each signing for itself. Two backends built for one
/// address count two independent local nonces over one nonce sequence, so
/// a test's counterparty is a different key, as it is on a real chain.
pub const COUNTERPARTY_PRIVATE_KEY: &str =
    "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

/// True if `anvil --version` runs successfully.
pub fn anvil_available() -> bool {
    Command::new("anvil")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// The "fail loudly in CI, skip locally" gate (issue #471): a real chain is
/// genuinely under test wherever this is called, so a CI run that lacks
/// `anvil` must fail loudly rather than silently skip and report success.
/// A local run without Foundry installed still skips.
pub fn require_anvil() -> bool {
    if anvil_available() {
        return true;
    }
    if std::env::var_os("CI").is_some() {
        panic!(
            "anvil is not on PATH, but CI is set -- the Rust Workspace Gate must install \
             Foundry (foundry-rs/foundry-toolchain) before this test runs. Refusing to \
             silently skip and report success here; see issue #471."
        );
    }
    eprintln!(
        "skipping: anvil is not on PATH (install Foundry: https://getfoundry.sh) -- this test \
         needs a real chain and only skips because this is not a CI run"
    );
    false
}

/// Deploy a plain, mintable 6-decimal ERC-20 with no EIP-3009
/// (`contracts/MockERC20.sol`) from `private_key`, minting
/// `mint_to_deployer` of it to that key's own address, and return its
/// address. Never used against a real chain.
pub async fn deploy_plain_token(
    rpc_url: &str,
    private_key: &str,
    mint_to_deployer: u128,
) -> Address {
    let provider = Provider::<Http>::try_from(rpc_url).expect("provider");
    let chain_id = provider.get_chainid().await.expect("chain id").as_u64();
    let wallet: LocalWallet = private_key.parse().expect("key");
    let wallet = wallet.with_chain_id(chain_id);
    let owner = wallet.address();
    let client = Arc::new(ethers::middleware::SignerMiddleware::new(provider, wallet));
    let token = mock_erc20::MockErc20::deploy(
        Arc::clone(&client),
        ("USD Coin (mock)".to_string(), "USDC".to_string(), 6u8),
    )
    .expect("deploy transaction")
    .send()
    .await
    .expect("deploy MockERC20");
    if mint_to_deployer > 0 {
        token
            .mint(owner, U256::from(mint_to_deployer))
            .send()
            .await
            .expect("send mint")
            .await
            .expect("mint");
    }
    token.address()
}

/// Mint `amount` of a [`deploy_plain_token`] token to `owner`, signed by
/// any funded key (the mock's `mint` is ungated).
pub async fn mint_plain_token(
    rpc_url: &str,
    private_key: &str,
    token: Address,
    owner: Address,
    amount: u128,
) {
    let provider = Provider::<Http>::try_from(rpc_url).expect("provider");
    let chain_id = provider.get_chainid().await.expect("chain id").as_u64();
    let wallet: LocalWallet = private_key.parse().expect("key");
    let client = Arc::new(ethers::middleware::SignerMiddleware::new(
        provider,
        wallet.with_chain_id(chain_id),
    ));
    mock_erc20::MockErc20::new(token, client)
        .mint(owner, U256::from(amount))
        .send()
        .await
        .expect("send mint")
        .await
        .expect("mint");
}

static NEXT_PORT_OFFSET: AtomicU16 = AtomicU16::new(0);

/// A freshly spawned `anvil` instance, killed when dropped.
pub struct Anvil {
    child: Child,
    pub rpc_url: String,
}

impl Anvil {
    /// Spawn `anvil` bound to a port derived from `base_port`, this
    /// process's pid, and a per-call atomic counter. Callers should pick a
    /// `base_port` distinct from other test binaries' so that binaries
    /// running concurrently under `cargo test --workspace` don't contend
    /// for the same port range; the atomic counter means multiple calls
    /// within the same test binary don't collide with each other either.
    pub async fn spawn(base_port: u16) -> Self {
        Self::spawn_with_chain_id(base_port, 31_337).await
    }

    /// [`spawn`](Self::spawn), under `chain_id` instead of anvil's default
    /// 31337: for a test whose subject is bound to a real chain's id, such
    /// as an EIP-712 digest a deployed contract computes under Base
    /// Sepolia's 84532.
    pub async fn spawn_with_chain_id(base_port: u16, chain_id: u64) -> Self {
        let offset = NEXT_PORT_OFFSET.fetch_add(1, Ordering::SeqCst);
        let port = base_port
            .wrapping_add((std::process::id() as u16) % 1_000)
            .wrapping_add(offset);
        let rpc_url = format!("http://127.0.0.1:{port}");

        let child = Command::new("anvil")
            .args(["--host", "127.0.0.1", "--port"])
            .arg(port.to_string())
            .arg("--chain-id")
            .arg(chain_id.to_string())
            .args([
                // Two genesis accounts, not one: `DEPLOYER_PRIVATE_KEY` and
                // `COUNTERPARTY_PRIVATE_KEY`, so a test's two nodes each
                // hold ETH for their own gas.
                "--accounts",
                "2",
                "--balance",
                "10000",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn anvil (is `anvil` on PATH? see foundryup)");

        let provider = Provider::<Http>::try_from(rpc_url.as_str()).expect("build provider");
        for _ in 0..200 {
            if provider.get_chainid().await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        Self { child, rpc_url }
    }
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
