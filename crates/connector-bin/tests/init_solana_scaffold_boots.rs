//! `connector init`'s one new config shape, `[settlement.solana]`, booted by
//! the real binary against a disposable chain (#1647). The operator's steps, in
//! order: run `init`, fund the address its report names, start the node. The
//! one substitution an operator never makes is the mint, which does not exist
//! on a chain spawned a moment ago, and the bind address, so tests can share a
//! host.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::str::FromStr;

use connector_settlement_solana::test_support::{
    create_mint, fund, require_solana_test_validator, SolanaValidator,
};
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

/// The Solana devnet row's mint, as the spec publishes it.
const DEVNET_MINT: &str = "34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU";

fn connector() -> Command {
    Command::new(env!("CARGO_BIN_EXE_connector"))
}

/// `connector init --devnet --chain solana`, returning its report.
fn init(dir: &Path, rpc_url: &str) -> String {
    let output = connector()
        .args([
            "init",
            "--devnet",
            "--chain",
            "solana",
            "--rpc-url",
            rpc_url,
        ])
        .arg("--dir")
        .arg(dir)
        .output()
        .expect("run connector init");
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("a UTF-8 report")
}

/// The address the report tells the operator to fund.
fn reported_address(report: &str) -> Pubkey {
    let line = report
        .lines()
        .find_map(|line| line.strip_prefix("Settlement address: "))
        .unwrap_or_else(|| panic!("the report names no settlement address: {report}"));
    Pubkey::from_str(line.trim())
        .unwrap_or_else(|error| panic!("{line:?} is not a Solana address: {error}"))
}

/// The written config with its container paths pointed at `dir`, the mint
/// swapped for one on this chain and an OS-picked port.
fn local_config(dir: &Path, mint: &Pubkey) -> std::path::PathBuf {
    let written = std::fs::read_to_string(dir.join("config/connector.toml")).expect("read config");
    assert!(written.contains(DEVNET_MINT), "{written}");
    assert!(written.contains("0.0.0.0:3000"), "{written}");
    let local = written
        .replace(DEVNET_MINT, &mint.to_string())
        .replace("0.0.0.0:3000", "127.0.0.1:0")
        .replace("/app/", &format!("{}/", dir.display()));
    let path = dir.join("local.toml");
    std::fs::write(&path, local).expect("write local config");
    path
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `true` once the node logs that it is listening; `false` if it exits first.
fn reaches_listening(node: &mut KillOnDrop) -> bool {
    let mut stdout = BufReader::new(node.0.stdout.take().expect("piped stdout"));
    let mut line = String::new();
    loop {
        line.clear();
        if stdout.read_line(&mut line).expect("read stdout") == 0 {
            return false;
        }
        if line.contains("connector listening") {
            return true;
        }
    }
}

/// Story 2a's node, and story 2c's reason the report says to fund it first.
#[tokio::test]
async fn a_solana_scaffold_serves_once_its_reported_address_holds_sol() {
    if !require_solana_test_validator() {
        return;
    }
    let validator = SolanaValidator::spawn().await;
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let authority = Keypair::new();
    fund(&rpc, &authority.pubkey()).await;
    let mint = create_mint(&rpc, &authority, 6).await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("node");
    let report = init(&dir, &validator.rpc_url);
    let config = local_config(&dir, &mint);

    // Unfunded: the failure the report warns of, naming the key to fund.
    let unfunded = connector().arg(&config).output().expect("run connector");
    assert!(
        !unfunded.status.success(),
        "an unfunded node exits non-zero"
    );
    let stderr = String::from_utf8_lossy(&unfunded.stderr);
    assert!(stderr.contains("holds no lamports"), "{stderr}");

    fund(&rpc, &reported_address(&report)).await;

    let mut node = KillOnDrop(
        connector()
            .arg(&config)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn connector"),
    );
    assert!(
        reaches_listening(&mut node),
        "the funded node exited before it was listening"
    );
}
