//! Proves the CI/local anvil gate end to end (issue #471): when anvil is available, this test
//! spawns a real instance and answers a real JSON-RPC call, taking non-zero time -- not a
//! 0.00s early return that reports `passed` without exercising anything. The real settlement
//! suites (`tests/batch_settlement.rs`, `tests/batch_settlement_paying.rs`) are their own
//! scope; this only proves the gate they depend on.

mod support;

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct AnvilHandle(Child);

impl Drop for AnvilHandle {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Reserves an ephemeral port by binding to it and immediately releasing it, matching the
/// `127.0.0.1:0` pattern connector-runtime's own transport tests already use for this.
fn reserve_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("binding an ephemeral localhost port")
        .local_addr()
        .expect("bound listener has a local address")
        .port()
}

fn spawn_anvil(port: u16) -> AnvilHandle {
    let child = Command::new("anvil")
        .arg("--port")
        .arg(port.to_string())
        .arg("--silent")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("require_anvil() already confirmed anvil runs, so spawning it must succeed");
    AnvilHandle(child)
}

fn chain_id(port: u16) -> Option<String> {
    let output = Command::new("cast")
        .args(["chain-id", "--rpc-url", &format!("http://127.0.0.1:{port}")])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn wait_for_rpc(port: u16) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(id) = chain_id(port) {
            return id;
        }
        assert!(
            Instant::now() < deadline,
            "anvil on port {port} never answered eth_chainId within 10s"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn anvil_spawns_and_answers_a_real_json_rpc_call() {
    if !support::require_anvil() {
        return;
    }

    let port = reserve_port();
    let _anvil = spawn_anvil(port);

    assert_eq!(wait_for_rpc(port), "31337", "anvil's default chain id");
}
