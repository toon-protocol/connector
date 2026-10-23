//! `EvmBatchWatcher` in front of an endpoint that refuses every call
//! (#1534): the watcher says so once, naming the endpoint and never its
//! keyed URL, and says once more when calls succeed again -- not once per
//! cycle.
//!
//! The endpoint is a real `anvil` behind a `FakeRpc` that answers HTTP 429
//! while a switch is on, and counts the requests it refuses.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use connector_chain_rpc::{FakeRpc, RpcReply, REFUSAL_RETRIES};
use connector_settlement::batch::{HeldVoucher, HeldVouchers};
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};
use connector_settlement_evm::{EvmBatchSettlementBackend, EvmBatchWatcher, RpcTransport};

/// This binary's own base port for [`Anvil::spawn`], clear of every other
/// anvil binary's range.
const ANVIL_BASE_PORT: u16 = 24_900;

/// Everything the watcher logs, as text.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Captured {
        self.clone()
    }
}

#[tokio::test]
async fn an_endpoint_refusing_every_call_is_warned_about_once_and_its_recovery_logged_once() {
    if !require_anvil() {
        return;
    }
    let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
    let mut x402 = X402Chain::place(&anvil.rpc_url).await;
    let token = x402.deploy_fiat_token().await;

    let refusing = Arc::new(AtomicBool::new(false));
    let refused = Arc::new(Mutex::new(0usize));
    let fake = {
        let refusing = Arc::clone(&refusing);
        let refused = Arc::clone(&refused);
        FakeRpc::spawn_in_front_of(&anvil.rpc_url, move |_| {
            if refusing.load(Ordering::SeqCst) {
                *refused.lock().unwrap() += 1;
                RpcReply::Status(429)
            } else {
                RpcReply::Forward
            }
        })
        .await
    };
    // A keyed URL, the way a hosted RPC is configured: the key is the path.
    let keyed = format!("{}/v2/SECRET-API-KEY", fake.url());
    let backend = EvmBatchSettlementBackend::connect(
        &RpcTransport::direct(&keyed).expect("transport"),
        DEPLOYER_PRIVATE_KEY,
        token,
        6,
        86_400,
    )
    .await
    .expect("bound to x402BatchSettlement");
    let held: Arc<RwLock<Vec<HeldVoucher>>> = Arc::new(RwLock::new(Vec::new()));
    let watcher = EvmBatchWatcher::new(Arc::new(backend), held as Arc<dyn HeldVouchers>);

    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let run = watcher.run(Duration::from_millis(100), Duration::from_millis(100));
    tokio::pin!(run);

    refusing.store(true, Ordering::SeqCst);
    let per_call = 1 + REFUSAL_RETRIES as usize;
    tokio::select! {
        _ = &mut run => unreachable!("the watcher runs for the life of the process"),
        _ = async {
            // Three refused passes, each one call the transport gave up on.
            while *refused.lock().unwrap() < 3 * per_call {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        } => {}
    }
    refusing.store(false, Ordering::SeqCst);
    tokio::select! {
        _ = &mut run => unreachable!("the watcher runs for the life of the process"),
        _ = tokio::time::sleep(Duration::from_secs(8)) => {}
    }

    let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let lines = |level: &str| {
        text.lines()
            .filter(|line| line.contains(level) && line.contains("settlement RPC endpoint"))
            .count()
    };
    assert_eq!(
        lines(" WARN "),
        1,
        "one warn for the run of refusals:\n{text}"
    );
    assert!(lines(" DEBUG ") >= 2, "later refusals at debug:\n{text}");
    assert_eq!(
        lines(" INFO "),
        1,
        "one line when calls succeed again:\n{text}"
    );
    assert!(
        !text
            .lines()
            .any(|line| line.contains(" WARN ") && line.contains("failed; retrying")),
        "a refusal is not also logged as a failed step:\n{text}"
    );
    assert!(text.contains(&fake.url()), "the endpoint is named:\n{text}");
    assert!(text.contains("429"), "the status is named:\n{text}");
    assert!(
        !text.contains("SECRET-API-KEY"),
        "never the keyed URL:\n{text}"
    );
}
