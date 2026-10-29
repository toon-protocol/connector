//! A real, disposable stand-alone `xrpld` (issue #1610).
//!
//! The repository's first container-backed tier-3 chain. `cargo test` spawns
//! one per test (ADR 0084's default: XRPL is "ready in 1-3 s and 72-322 MB",
//! under the 10 s / 1 GB line that would move it to the shared tier) and
//! removes it on drop. Nothing here dials a hand-started container.
//!
//! - **The image is pinned by digest**, not by tag: [`IMAGE`].
//! - **Each chain has its own ports** (Docker picks the host port), its own
//!   config and its own database, which lives in the container and goes with
//!   it.
//! - **A chain left by a killed run is cleared by the next spawn.** Every
//!   container carries the `toon.test-chain.owner` label holding the PID of
//!   the test process that started it, and [`Xrpld::spawn`] first removes any
//!   container whose owner is no longer running (ADR 0084 decision 4).
//! - **No chain, no green.** [`require_xrpld`] panics when `CI` is set and
//!   skips locally without Docker or when the image cannot be pulled.
//!
//! Funding is a plain `Payment` from the genesis account, whose secret is
//! public ([`GENESIS_SECRET`]): it is the stand-alone chain's built-in
//! faucet, worth nothing anywhere else.

use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use connector_chain_rpc::xrpl::{TxFate, TxWait, XrplRpc};
use connector_chain_rpc::RpcTransport;
use serde_json::json;

/// `rippleci/xrpld` 3.4.1, pinned by the digest of its multi-arch index
/// (amd64 only). The tag is for the reader; Docker resolves the digest.
pub const IMAGE: &str =
    "rippleci/xrpld:3.4.1@sha256:e77ed5a956a32842929d0a2f2c581cab88e2e72e1d9af7df7330938231364d03";

/// The label every harness-spawned container carries, holding its owner's PID.
pub const OWNER_LABEL: &str = "toon.test-chain.owner";

/// The stand-alone network id the harness configures: testnet-class (at most
/// 1024, so a transaction carries no `NetworkID`) and none of 0, 1 or 2,
/// which are mainnet, testnet and devnet. Fixed by the XRPL ADR (#1609),
/// which this constant follows.
pub const STANDALONE_NETWORK_ID: u32 = 1000;

/// The genesis account every stand-alone ledger starts with, holding all
/// 100 billion XRP.
pub const GENESIS_ADDRESS: &str = "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh";

/// The genesis account's secret. Public: it is the `masterpassphrase` seed
/// printed in XRPL's own documentation, and it only controls an account that
/// exists on a chain this harness throws away.
pub const GENESIS_SECRET: &str = "snoPBrXtMeMyMHUVTgbuqAfg1SUTb";

/// The RPC port inside the container.
const RPC_PORT: u16 = 5005;

/// How long a spawn waits for a first answered `server_info` before it
/// fails, measured from `docker run`.
const STARTUP_DEADLINE: Duration = Duration::from_secs(60);

/// Held from `docker run` to the first answered `server_info`, so a test
/// binary starts its chains one at a time. Eight started at once take 18–22 s
/// each to answer where one alone takes under 4 s: without this, `startup`
/// times the host's contention rather than the chain ADR 0084's line is about.
static SPAWNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What [`require_xrpld`] does about a missing chain: a pure function of the
/// facts, so the CI rule is tested without Docker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Run,
    /// Skip, locally only.
    Skip,
    /// Fail loudly: this is CI.
    Fail,
}

pub fn availability(chain_ready: bool, ci: bool) -> Availability {
    match (chain_ready, ci) {
        (true, _) => Availability::Run,
        (false, true) => Availability::Fail,
        (false, false) => Availability::Skip,
    }
}

/// The reason the chain is not available, or `None` when Docker answers and
/// the pinned image is present. Pulled at most once per test process.
fn unavailable_reason() -> Option<&'static str> {
    static REASON: OnceLock<Option<String>> = OnceLock::new();
    REASON
        .get_or_init(|| {
            let docker = Command::new("docker")
                .args(["version", "--format", "{{.Server.Version}}"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            if !docker {
                return Some("no Docker daemon answers `docker version`".to_string());
            }
            match Command::new("docker")
                .args(["pull", "--quiet", IMAGE])
                .stdout(Stdio::null())
                .output()
            {
                Ok(out) if out.status.success() => None,
                Ok(out) => Some(format!(
                    "`docker pull {IMAGE}` failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )),
                Err(e) => Some(format!("could not run `docker pull`: {e}")),
            }
        })
        .as_deref()
}

/// The "fail loudly in CI, skip locally" gate (issues #471, #1610): a real
/// chain is under test wherever this is called, so a CI run without Docker,
/// or whose pull failed, must not report `passed` in 0.00 s.
pub fn require_xrpld() -> bool {
    let reason = unavailable_reason();
    match availability(reason.is_none(), std::env::var_os("CI").is_some()) {
        Availability::Run => true,
        Availability::Fail => panic!(
            "the xrpld test chain is unavailable ({}), but CI is set -- the Rust Workspace \
             Gate runs on a runner with Docker. Refusing to silently skip and report success.",
            reason.unwrap_or("unknown")
        ),
        Availability::Skip => {
            eprintln!(
                "skipping: the xrpld test chain is unavailable ({}) -- this test needs a real \
                 chain and only skips because this is not a CI run",
                reason.unwrap_or("unknown")
            );
            false
        }
    }
}

fn docker(args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("docker").args(args).output()
}

/// Whether `pid` names a live process. `/proc` where there is one; `ps -p`
/// where there is not (macOS), since a missing `/proc` would make every
/// owner, this process included, look dead and have each spawn remove its
/// siblings' chains.
fn process_is_running(pid: u32) -> bool {
    if std::path::Path::new("/proc/self").exists() {
        return std::path::Path::new(&format!("/proc/{pid}")).exists();
    }
    Command::new("ps")
        .args(["-p", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        // Unable to tell: keep the container rather than remove a live chain.
        .unwrap_or(true)
}

/// Remove every harness container whose owning process is gone. A container
/// whose label does not parse as a PID is removed too: nothing live owns it.
fn clear_orphans() {
    let format = format!("{{{{.ID}}}} {{{{.Label \"{OWNER_LABEL}\"}}}}");
    let Ok(listed) = docker(&[
        "ps",
        "-a",
        "--filter",
        &format!("label={OWNER_LABEL}"),
        "--format",
        &format,
    ]) else {
        return;
    };
    for line in String::from_utf8_lossy(&listed.stdout).lines() {
        let mut parts = line.split_whitespace();
        let (Some(id), owner) = (parts.next(), parts.next()) else {
            continue;
        };
        let live = owner
            .and_then(|pid| pid.parse::<u32>().ok())
            .is_some_and(process_is_running);
        if !live {
            let _ = docker(&["rm", "-f", id]);
        }
    }
}

/// The config of a stand-alone chain. `[voting]` is explicit because the
/// code's defaults (10 XRP base reserve, 2 XRP owner reserve) are not the
/// network's (1 XRP, 0.2 XRP), and a settlement backend's reserve arithmetic
/// must be tested against the network's.
fn config() -> String {
    format!(
        "\
[server]
port_rpc_admin_local

[port_rpc_admin_local]
port = {RPC_PORT}
ip = 0.0.0.0
admin = 0.0.0.0
protocol = http

[node_db]
type=NuDB
path=/tmp/xrpld/db/nudb

[database_path]
/tmp/xrpld/db

[debug_logfile]
/tmp/xrpld/debug.log

[network_id]
{STANDALONE_NETWORK_ID}

[voting]
reference_fee = 10
account_reserve = 1000000
owner_reserve = 200000

[ssl_verify]
0
"
    )
}

/// A freshly spawned stand-alone `xrpld`, removed when dropped.
pub struct Xrpld {
    container: String,
    /// The RPC endpoint, on a host port Docker chose.
    pub rpc_url: String,
    /// How long the spawn took to a first answered `server_info`: ADR 0084's
    /// 10 s line is read off this.
    pub startup: Duration,
    // Held for the container's life: the mounted config.
    _config: tempfile::TempDir,
    // Funding signs with the genesis account, whose sequence is one counter.
    funder: tokio::sync::Mutex<()>,
}

/// A funded account: its classic address and family seed (`s…`).
#[derive(Debug, Clone)]
pub struct Wallet {
    pub address: String,
    pub seed: String,
}

impl Xrpld {
    /// Start a chain and wait for its first `server_info`. Call
    /// [`require_xrpld`] first.
    pub async fn spawn() -> Xrpld {
        use std::os::unix::fs::PermissionsExt;

        let _spawning = SPAWNING.lock().await;
        clear_orphans();
        let dir = tempfile::tempdir().expect("a temp dir for the xrpld config");
        let config_path = dir.path().join("xrpld.cfg");
        std::fs::write(&config_path, config()).expect("write xrpld.cfg");
        // The container's user is not ours: the file must be world-readable.
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o644))
            .expect("chmod xrpld.cfg");

        let started = Instant::now();
        let run = docker(&[
            "run",
            "-d",
            "--rm",
            "--label",
            &format!("{OWNER_LABEL}={}", std::process::id()),
            "-p",
            &format!("127.0.0.1::{RPC_PORT}"),
            "-v",
            &format!("{}:/etc/xrpld/xrpld.cfg:ro", config_path.display()),
            IMAGE,
            "-a",
            "--start",
            "--conf",
            "/etc/xrpld/xrpld.cfg",
        ])
        .expect("run docker");
        assert!(
            run.status.success(),
            "docker run failed: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        let container = String::from_utf8_lossy(&run.stdout).trim().to_string();
        // From here a panic must not leak the container.
        let mut chain = Xrpld {
            container,
            rpc_url: String::new(),
            startup: Duration::ZERO,
            _config: dir,
            funder: tokio::sync::Mutex::new(()),
        };

        let port =
            docker(&["port", &chain.container, &format!("{RPC_PORT}/tcp")]).expect("docker port");
        let mapped = String::from_utf8_lossy(&port.stdout);
        let port = mapped
            .lines()
            .find_map(|line| line.trim().rsplit(':').next()?.parse::<u16>().ok())
            .unwrap_or_else(|| panic!("no host port for the xrpld container: {mapped:?}"));
        chain.rpc_url = format!("http://127.0.0.1:{port}");

        let rpc = chain.rpc();
        let deadline = started + STARTUP_DEADLINE;
        let mut answered = false;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Ok(Ok(_)) = tokio::time::timeout(remaining, rpc.server_info()).await {
                answered = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        chain.startup = started.elapsed();
        assert!(
            answered,
            "xrpld did not answer server_info within {STARTUP_DEADLINE:?}; its log:\n{}",
            chain.logs()
        );
        chain
    }

    /// An RPC client over a fresh direct [`RpcTransport`]: the connector's
    /// own transport, the one a settlement table would build.
    pub fn rpc(&self) -> XrplRpc {
        XrplRpc::new(RpcTransport::direct(&self.rpc_url).expect("the harness url is http"))
    }

    /// The container's id.
    pub fn container_id(&self) -> &str {
        &self.container
    }

    /// The container's stdout and stderr, for a failure message.
    pub fn logs(&self) -> String {
        docker(&["logs", "--tail", "40", &self.container])
            .map(|out| {
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                )
            })
            .unwrap_or_default()
    }

    /// The container's memory use in bytes, as `docker stats` reports it,
    /// for ADR 0084's 1 GB line. `None` when it cannot be read.
    pub fn memory_bytes(&self) -> Option<u64> {
        let out = docker(&[
            "stats",
            "--no-stream",
            "--format",
            "{{.MemUsage}}",
            &self.container,
        ])
        .ok()?;
        parse_mem_usage(&String::from_utf8_lossy(&out.stdout))
    }

    /// Pay `drops` from the genesis account to `address` and close the
    /// ledger. Serialised per chain: concurrent funders would race the
    /// genesis account's sequence.
    pub async fn fund(&self, rpc: &XrplRpc, address: &str, drops: u64) {
        let _funder = self.funder.lock().await;
        let current = rpc.fee().await.expect("fee").ledger_current_index;
        let last_ledger_sequence = current + 20;
        let submitted = rpc
            .sign_and_submit(
                json!({
                    "TransactionType": "Payment",
                    "Account": GENESIS_ADDRESS,
                    "Destination": address,
                    "Amount": drops.to_string(),
                    "LastLedgerSequence": last_ledger_sequence,
                }),
                GENESIS_SECRET,
            )
            .await
            .expect("submit the funding payment");
        assert_eq!(submitted.engine_result, "tesSUCCESS", "{submitted:?}");
        rpc.ledger_accept().await.expect("close the ledger");
        let fate = rpc
            .wait_for_tx(
                &submitted.hash,
                last_ledger_sequence,
                TxWait {
                    poll: Duration::from_millis(50),
                    give_up_after: Duration::from_secs(30),
                },
            )
            .await
            .expect("wait for the funding payment");
        match fate {
            TxFate::Landed {
                transaction_result, ..
            } => assert_eq!(transaction_result, "tesSUCCESS"),
            other => panic!("the funding payment did not land: {other:?}"),
        }
    }

    /// A fresh account holding `drops`, with its seed.
    pub async fn funded_wallet(&self, rpc: &XrplRpc, drops: u64) -> Wallet {
        let (address, seed) = rpc.wallet_propose().await.expect("wallet_propose");
        self.fund(rpc, &address, drops).await;
        Wallet { address, seed }
    }
}

/// The used half of `docker stats`' `MemUsage` (`72.5MiB / 7.7GiB`), in
/// bytes.
fn parse_mem_usage(mem_usage: &str) -> Option<u64> {
    let used = mem_usage.split('/').next()?.trim();
    let split = used.find(|c: char| c.is_ascii_alphabetic())?;
    let (number, unit) = used.split_at(split);
    let scale: f64 = match unit {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((number.trim().parse::<f64>().ok()? * scale) as u64)
}

impl Drop for Xrpld {
    fn drop(&mut self) {
        let _ = docker(&["rm", "-f", &self.container]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_chain_skips_locally_and_fails_in_ci() {
        assert_eq!(availability(true, true), Availability::Run);
        assert_eq!(availability(true, false), Availability::Run);
        assert_eq!(availability(false, false), Availability::Skip);
        assert_eq!(availability(false, true), Availability::Fail);
    }

    #[test]
    fn docker_stats_memory_usage_reads_as_bytes() {
        assert_eq!(parse_mem_usage("72.5MiB / 7.7GiB"), Some(76_021_760));
        assert_eq!(parse_mem_usage("1.2GiB / 7.7GiB"), Some(1_288_490_188));
        assert_eq!(parse_mem_usage("512KiB / 1GiB"), Some(524_288));
        assert_eq!(parse_mem_usage("--"), None);
    }

    #[test]
    fn the_image_is_pinned_by_digest() {
        assert!(IMAGE.contains("@sha256:"), "{IMAGE}");
    }
}
