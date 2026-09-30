//! Throughput and latency measurement for [`ClientClaimGate::ingest`]
//! (issue #686): how many fully verified, durably journaled claims per
//! second the gate admits, and what one claim's admission latency looks
//! like at huddle-shaped load. Written against the gate's public surface
//! only, so the same measurement runs unchanged before and after #686's
//! group-commit restructuring -- the numbers it prints are the before/after
//! evidence, not a pass/fail gate.
//!
//! `#[ignore]`d because it is a measurement, not a test: it runs for tens
//! of seconds, its numbers depend on the disk under `TMPDIR` (the fsync
//! floor is the whole subject), and it asserts nothing a CI box could
//! promise. Run it by hand:
//!
//! ```sh
//! cargo test -p connector-client-edge --test claim_gate_throughput \
//!     --release -- --ignored --nocapture
//! ```
//!
//! The workload mirrors `prototypes/tigerbeetle-claim-gate` in toon-meta
//! (branch `proto/tigerbeetle-claim-gate`): per session, strictly advancing
//! vouchers on that session's own x402 channel, each genuinely EIP-712
//! signed and verified -- nothing is stubbed out of the admission path but
//! the chain, which a fake backend stands in for, so a printed claims/sec is
//! what a fleet box's gate would sustain on this disk (ADR 0075: every claim
//! is a voucher).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use connector_client_edge::{
    AdmittedEvmVoucherChannel, AdmittedSolanaVoucherChannel, BatchSettlementChannels,
    ChannelResolutionError, ClientClaimGate,
};
use connector_runtime::FileJournal;
use connector_signer::{
    derive_evm_address, evm_batch_channel_id, evm_voucher_digest, BatchChannelConfig,
    BatchSettlementDomain,
};
use libsecp256k1::{Message, PublicKey, SecretKey};

/// What each voucher advances its channel by -- the huddle measurement's
/// per-frame price, so batches fill the way real load fills them.
const PRICE: u64 = 20;

fn domain() -> BatchSettlementDomain {
    BatchSettlementDomain::x402(84_532)
}

/// One deterministic payer keypair, shared by every session: the gate
/// verifies per channel, not per key, and one key keeps setup cheap.
fn signer() -> (SecretKey, [u8; 20]) {
    let secret = SecretKey::parse(&[9u8; 32]).unwrap();
    let public = PublicKey::from_secret_key(&secret);
    (secret, derive_evm_address(&public.serialize()))
}

/// Session `index`'s channel: distinct per session, by salt, so sessions
/// contend only on the gate's shared state, never on one watermark.
fn config(index: u32) -> BatchChannelConfig {
    let mut salt = [0xab; 32];
    salt[..4].copy_from_slice(&index.to_be_bytes());
    BatchChannelConfig {
        payer: [0x11; 20],
        payer_authorizer: signer().1,
        receiver: [0x33; 20],
        receiver_authorizer: [0x33; 20],
        token: [0x55; 20],
        withdraw_delay: 86_400,
        salt,
    }
}

/// A backend admitting every session's channel with unbounded collateral,
/// so no chain is ever consulted -- the measurement is of the gate and its
/// journal, not of an RPC endpoint.
#[derive(Debug)]
struct Channels(HashMap<[u8; 32], BatchChannelConfig>);

#[async_trait]
impl BatchSettlementChannels for Channels {
    fn evm_domain(&self) -> Option<BatchSettlementDomain> {
        Some(domain())
    }

    fn accepts_solana(&self) -> bool {
        false
    }

    async fn evm(
        &self,
        channel_id: &[u8; 32],
        _presented_config: Option<&BatchChannelConfig>,
    ) -> Result<Option<AdmittedEvmVoucherChannel>, ChannelResolutionError> {
        Ok(self
            .0
            .get(channel_id)
            .map(|config| AdmittedEvmVoucherChannel {
                config: *config,
                max_cumulative: u128::MAX,
            }))
    }

    async fn solana(
        &self,
        _channel_account: &[u8; 32],
    ) -> Result<Option<AdmittedSolanaVoucherChannel>, ChannelResolutionError> {
        Ok(None)
    }
}

fn gate_over(journal: FileJournal, sessions: u32) -> Arc<ClientClaimGate> {
    let channels = (0..sessions)
        .map(|index| {
            let config = config(index);
            (evm_batch_channel_id(&domain(), &config), config)
        })
        .collect();
    Arc::new(
        ClientClaimGate::restore(Arc::new(journal))
            .expect("a fresh journal has nothing to replay")
            .with_batch_settlement(Arc::new(Channels(channels))),
    )
}

/// A voucher JSON with a genuine EIP-712 signature -- the same shape a
/// stock x402 client sends, so the gate spends real cryptographic work on
/// every admission it counts.
fn signed_claim_json(secret: &SecretKey, session: u32, amount: u64) -> String {
    let config = config(session);
    let channel_id = evm_batch_channel_id(&domain(), &config);
    let message = Message::parse(&evm_voucher_digest(
        &domain(),
        &channel_id,
        u128::from(amount),
    ));
    let (signature, recovery_id) = libsecp256k1::sign(&message, secret);
    let mut signature_bytes = signature.serialize().to_vec();
    signature_bytes.push(recovery_id.serialize() + 27);
    let address = |bytes: &[u8; 20]| format!("0x{}", hex::encode(bytes));
    serde_json::json!({
        "version": "1.0",
        "blockchain": "evm",
        "scheme": "batch-settlement",
        "messageId": format!("msg-{amount}"),
        "timestamp": "2026-02-02T12:00:00.000Z",
        "senderId": "bench",
        "channelId": format!("0x{}", hex::encode(channel_id)),
        "maxClaimableAmount": amount.to_string(),
        "signature": format!("0x{}", hex::encode(signature_bytes)),
        "channelConfig": {
            "payer": address(&config.payer),
            "payerAuthorizer": address(&config.payer_authorizer),
            "receiver": address(&config.receiver),
            "receiverAuthorizer": address(&config.receiver_authorizer),
            "token": address(&config.token),
            "withdrawDelay": config.withdraw_delay,
            "salt": format!("0x{}", hex::encode(config.salt)),
        },
    })
    .to_string()
}

/// Sustained throughput: `sessions` concurrent clients each submitting
/// strictly advancing claims as fast as the gate admits them, for
/// `seconds`. Prints aggregate accepted claims/sec.
async fn measure_throughput(gate: Arc<ClientClaimGate>, sessions: u32, seconds: u64) -> f64 {
    let stop = Arc::new(AtomicBool::new(false));
    let accepted = Arc::new(AtomicU64::new(0));
    let started = Instant::now();
    let mut tasks = Vec::new();
    for session in 0..sessions {
        let gate = gate.clone();
        let stop = stop.clone();
        let accepted = accepted.clone();
        tasks.push(tokio::spawn(async move {
            let (secret, _) = signer();
            let mut nonce = 0u64;
            while !stop.load(Ordering::Relaxed) {
                nonce += 1;
                let claim = signed_claim_json(&secret, session, nonce * PRICE);
                gate.ingest(&claim, PRICE)
                    .await
                    .expect("strictly advancing vouchers are always admissible");
                accepted.fetch_add(1, Ordering::Relaxed);
                // An admission that never awaited anything pending (the
                // pre-#686 gate blocks synchronously) would otherwise pin
                // its worker forever and starve the stop timer.
                tokio::task::yield_now().await;
            }
        }));
    }
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    stop.store(true, Ordering::Relaxed);
    for task in tasks {
        task.await.expect("session task");
    }
    let elapsed = started.elapsed().as_secs_f64();
    accepted.load(Ordering::Relaxed) as f64 / elapsed
}

/// Paced latency: `sessions` clients each submitting `rate` claims/sec --
/// the huddle-audio shape -- for `seconds`, measuring each admission's
/// wall-clock time. Returns every latency in milliseconds, sorted.
async fn measure_latency(
    gate: Arc<ClientClaimGate>,
    sessions: u32,
    rate: u64,
    seconds: u64,
) -> Vec<f64> {
    let per_session = (rate * seconds) as usize;
    let interval = Duration::from_nanos(1_000_000_000 / rate);
    let mut tasks = Vec::new();
    for session in 0..sessions {
        let gate = gate.clone();
        tasks.push(tokio::spawn(async move {
            let (secret, _) = signer();
            let started = Instant::now();
            let mut latencies = Vec::with_capacity(per_session);
            for tick in 0..per_session {
                let target = interval * tick as u32;
                let elapsed = started.elapsed();
                if target > elapsed {
                    tokio::time::sleep(target - elapsed).await;
                }
                let nonce = tick as u64 + 1;
                let claim = signed_claim_json(&secret, session, nonce * PRICE);
                let submitted = Instant::now();
                gate.ingest(&claim, PRICE)
                    .await
                    .expect("strictly advancing vouchers are always admissible");
                latencies.push(submitted.elapsed().as_secs_f64() * 1000.0);
            }
            latencies
        }));
    }
    let mut all = Vec::new();
    for task in tasks {
        all.extend(task.await.expect("session task"));
    }
    all.sort_by(|a, b| a.partial_cmp(b).expect("finite latencies"));
    all
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() as f64 * p / 100.0) as usize).min(sorted.len() - 1)]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "a measurement, not a test -- run by hand with --ignored --nocapture (see module doc)"]
async fn claim_admission_throughput_and_latency() {
    for sessions in [1u32, 16, 64] {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal = FileJournal::open(dir.path().join("claims.log")).expect("journal");
        let gate = gate_over(journal, sessions);
        let claims_per_sec = measure_throughput(gate, sessions, 10).await;
        println!("throughput sessions={sessions} claims_per_sec={claims_per_sec:.0}");
    }

    let sessions = 10u32;
    let rate = 50u64;
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = FileJournal::open(dir.path().join("claims.log")).expect("journal");
    let gate = gate_over(journal, sessions);
    let latencies = measure_latency(gate, sessions, rate, 10).await;
    println!(
        "latency sessions={sessions} rate={rate}/s p50_ms={:.1} p95_ms={:.1} p99_ms={:.1} max_ms={:.1}",
        percentile(&latencies, 50.0),
        percentile(&latencies, 95.0),
        percentile(&latencies, 99.0),
        latencies.last().copied().unwrap_or(f64::NAN),
    );
}
