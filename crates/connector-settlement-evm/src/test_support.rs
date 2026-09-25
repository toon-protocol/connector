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
use std::time::Duration;

use ethers::providers::{Http, Middleware, Provider};

/// Anvil's first well-known dev account -- the same one
/// `packages/contracts/script/DeployLocal.s.sol` already uses as its
/// deployer, so this test harness's choice of key is not a new
/// convention.
pub const DEPLOYER_PRIVATE_KEY: &str =
    "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

/// Anvil's *second* well-known dev account (`0x7099…79C8`), genesis-funded
/// with ETH exactly like [`DEPLOYER_PRIVATE_KEY`].
///
/// Needed once `SettlementBackend::fund` became a self-deposit (issue
/// #1118): a test that wants collateral on **both** sides of a channel now
/// needs two identities that can each sign for themselves, and they cannot
/// be the same address. Two `EvmSettlementBackend`s built for one address
/// count two independent local nonces over one nonce sequence, so the
/// second one to write is refused `nonce too low` and has to re-read
/// `pending` (ADR 0073) -- which is not a path a test should lean on for its
/// counterparty, since on a real chain the counterparty is a different
/// party with a different key anyway.
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
        let offset = NEXT_PORT_OFFSET.fetch_add(1, Ordering::SeqCst);
        let port = base_port
            .wrapping_add((std::process::id() as u16) % 1_000)
            .wrapping_add(offset);
        let rpc_url = format!("http://127.0.0.1:{port}");

        let child = Command::new("anvil")
            .args(["--host", "127.0.0.1", "--port"])
            .arg(port.to_string())
            .args([
                "--chain-id",
                "31337",
                // Two genesis accounts, not one: `DEPLOYER_PRIVATE_KEY` and
                // `COUNTERPARTY_PRIVATE_KEY`. A channel is two-sided and
                // `fund` is a self-deposit (issue #1118), so a test that
                // wants collateral on both sides needs both identities to
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
