//! ADR 0073's missing measurement, as a probe anyone can run: the connector's
//! own settlement backends, submitting and confirming on Base Sepolia and
//! Solana devnet **through a real onion-routing circuit**.
//!
//! Everything else that tests ADR 0073 does so against fakes on loopback: a
//! scripted RPC, a SOCKS5 server fake. That proves where a dial goes and what
//! the backends do with a bad answer, but not what a real circuit does to a
//! real confirmation. This file is that half. It drives the production
//! constructors over `RpcTransport::through`, which is exactly what
//! `rpc_via_socks_proxy = true` builds.
//!
//! ## LOCAL / DEV ONLY, and inert unless driven
//!
//! Every test returns at once unless `SETTLEMENT_CIRCUIT_SOCKS` names a
//! `socks5h://` proxy, which is a local `anon` (or Tor) client such as the one
//! ADR 0073's Appendix starts. A plain `cargo test`, and CI, never touch the
//! network here. The CI gate must never depend on a third-party anonymity
//! network (ADR 0070).
//!
//! With only the proxy set, each chain's half runs **unfunded**. The node
//! boots over the circuit, doing every read `connect` makes. On Solana that
//! ends at the refusal of a payer holding no lamports, which is itself a
//! read over the circuit.
//!
//! To run the **funded** half, name throwaway keys by location (never by
//! value, ADR 0009):
//!
//! - `SETTLEMENT_CIRCUIT_EVM_KEY_FILE`: a file holding a hex secp256k1 key
//!   with a little Base Sepolia ETH for gas. The probe mints itself mock USDC
//!   (the fleet's Base Sepolia token has an ungated `mint()`), opens a
//!   channel to a random counterparty, funds it to a total twice (the second
//!   is a no-op, ADR 0073 decision 5), and closes it.
//! - `SETTLEMENT_CIRCUIT_SOLANA_KEY_FILE`: a `solana-keygen` JSON keypair
//!   with a little devnet SOL. The probe boots (creating the key's token
//!   account on first run, a real submit and confirm), opens a channel to a
//!   random counterparty, and closes it.
//!
//! Never point either at a fleet or devnet-box key.
//!
//! ```text
//! SETTLEMENT_CIRCUIT_SOCKS=socks5h://127.0.0.1:19050 \
//! SETTLEMENT_CIRCUIT_EVM_KEY_FILE=/path/to/throwaway-evm.key \
//! SETTLEMENT_CIRCUIT_SOLANA_KEY_FILE=/path/to/throwaway-solana.json \
//!   cargo test -p connector --test settlement_over_a_circuit -- --nocapture --test-threads=1
//! ```

use std::str::FromStr;
use std::time::Instant;

use chrono::Duration;
use connector_chain_rpc::{Circuit, RpcTransport};
use connector_settlement::{ChannelStatus, SettlementBackend};
use connector_settlement_evm::EvmSettlementBackend;
use connector_settlement_solana::SolanaSettlementBackend;
use ethers::signers::{LocalWallet, Signer as _};
use ethers::types::Address;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer as _};

/// The fleet's committed `[settlement.evm]` (`infra/linode-relay/connector-rust.toml`).
const EVM_RPC: &str = "https://base-sepolia-rpc.publicnode.com";
const EVM_REGISTRY: &str = "0x0c41D9D424d6B075A3cEa1068a694f7847a8CCa5";
const EVM_TOKEN: &str = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce";

/// The fleet's committed `[settlement.solana]`.
const SOLANA_RPC: &str = "https://api.devnet.solana.com";
const SOLANA_PROGRAM: &str = "2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip";
const SOLANA_MINT: &str = "34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU";

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn proxy() -> Option<url::Url> {
    env("SETTLEMENT_CIRCUIT_SOCKS")
        .map(|value| url::Url::parse(&value).expect("SETTLEMENT_CIRCUIT_SOCKS: not a URL"))
}

fn timed<T>(what: &str, started: Instant, value: T) -> T {
    eprintln!("[circuit] {what}: {:.2}s", started.elapsed().as_secs_f64());
    value
}

#[tokio::test]
async fn the_evm_backend_submits_and_confirms_over_a_real_circuit() {
    let Some(proxy) = proxy() else {
        return;
    };
    let transport =
        RpcTransport::through(EVM_RPC, &proxy, Circuit::EvmSettlement).expect("transport");
    let key = match env("SETTLEMENT_CIRCUIT_EVM_KEY_FILE") {
        Some(path) => std::fs::read_to_string(path)
            .expect("read SETTLEMENT_CIRCUIT_EVM_KEY_FILE")
            .trim()
            .to_string(),
        // Unfunded: boot only. EVM boot sends nothing.
        None => format!(
            "{:x}",
            LocalWallet::new(&mut ethers::core::rand::thread_rng())
                .signer()
                .to_bytes()
        ),
    };
    let funded = env("SETTLEMENT_CIRCUIT_EVM_KEY_FILE").is_some();

    let started = Instant::now();
    let backend = EvmSettlementBackend::connect(
        &transport,
        &key,
        Address::from_str(EVM_REGISTRY).expect("registry"),
        Address::from_str(EVM_TOKEN).expect("token"),
        6,
    )
    .await
    .expect("boot over the circuit");
    timed(
        "evm boot (chain id, getTokenNetwork, decimals)",
        started,
        (),
    );
    assert_eq!(backend.chain_id(), 84_532, "Base Sepolia");
    if !funded {
        return;
    }

    let started = Instant::now();
    backend
        .mint_mock_tokens_to(backend.own_address(), 10_000)
        .await
        .expect("mint mock USDC over the circuit");
    timed("evm mint (submit + confirm)", started, ());

    let counterparty = LocalWallet::new(&mut ethers::core::rand::thread_rng()).address();
    let started = Instant::now();
    let channel = backend
        .open(counterparty.as_bytes().to_vec(), Duration::seconds(3_600))
        .await
        .expect("open over the circuit");
    timed("evm open (submit + confirm)", started, ());

    let started = Instant::now();
    let state = backend.fund_to(&channel, 1_000).await.expect("fund_to");
    timed("evm fund_to (approve + setTotalDeposit)", started, ());
    assert_eq!(state.own_deposited, 1_000);
    let started = Instant::now();
    let state = backend
        .fund_to(&channel, 1_000)
        .await
        .expect("fund_to again");
    timed("evm fund_to repeated (a read, nothing sent)", started, ());
    assert_eq!(state.own_deposited, 1_000);

    let started = Instant::now();
    let state = backend.close(&channel).await.expect("close");
    timed("evm close (submit + confirm)", started, ());
    assert_eq!(state.status, ChannelStatus::Closed);
    eprintln!(
        "[circuit] evm channel {} opened, funded and closed",
        channel.0
    );
}

#[tokio::test]
async fn the_solana_backend_submits_and_confirms_over_a_real_circuit() {
    let Some(proxy) = proxy() else {
        return;
    };
    let transport =
        RpcTransport::through(SOLANA_RPC, &proxy, Circuit::SolanaSettlement).expect("transport");
    let seed: [u8; 32] = match env("SETTLEMENT_CIRCUIT_SOLANA_KEY_FILE") {
        Some(path) => {
            let bytes: Vec<u8> = serde_json::from_str(
                &std::fs::read_to_string(path).expect("read SETTLEMENT_CIRCUIT_SOLANA_KEY_FILE"),
            )
            .expect("a solana-keygen JSON keypair");
            bytes[..32].try_into().expect("64 bytes, seed first")
        }
        None => Keypair::new().to_bytes()[..32]
            .try_into()
            .expect("a keypair's first 32 bytes are its seed"),
    };
    let funded = env("SETTLEMENT_CIRCUIT_SOLANA_KEY_FILE").is_some();
    let program = Pubkey::from_str(SOLANA_PROGRAM).expect("program");
    let mint = Pubkey::from_str(SOLANA_MINT).expect("mint");

    let started = Instant::now();
    let connected = SolanaSettlementBackend::connect(&transport, &seed, program, mint, 6).await;
    timed("solana boot", started, ());
    if !funded {
        let error = connected
            .err()
            .expect("an unfunded payer is refused")
            .to_string();
        assert!(
            error.contains("holds no lamports"),
            "every boot read went over the circuit and the payer was read as unfunded: {error}"
        );
        return;
    }
    let backend = connected.expect("boot over the circuit, creating the ATA on a first run");
    assert_eq!(backend.cluster(), Some("devnet"));

    let counterparty = Keypair::new().pubkey();
    let started = Instant::now();
    let channel = backend
        .open(counterparty.to_bytes().to_vec(), Duration::seconds(3_600))
        .await
        .expect("open over the circuit");
    timed("solana open (submit + confirm)", started, ());

    let started = Instant::now();
    let state = backend.close(&channel).await.expect("close");
    timed("solana close (submit + confirm)", started, ());
    assert_eq!(state.status, ChannelStatus::Closed);
    eprintln!("[circuit] solana channel {} opened and closed", channel.0);
}
