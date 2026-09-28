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
//! ends at the refusal of a key holding no lamports, which is itself a read
//! over the circuit.
//!
//! To run the **funded** EVM half, name a throwaway key by location (never by
//! value, ADR 0009):
//!
//! - `SETTLEMENT_CIRCUIT_EVM_KEY_FILE`: a file holding a hex secp256k1 key
//!   with a little Base Sepolia ETH for gas and some of the fleet's devnet
//!   USDC (the faucet mints it). The probe opens an x402 channel toward a
//!   random receiver, tops it up, and starts its withdrawal -- every write a
//!   real submit and confirm over the circuit.
//! - `SETTLEMENT_CIRCUIT_SOLANA_KEY_FILE`: a `solana-keygen` JSON keypair
//!   with a little devnet SOL. The probe boots, every read over the circuit.
//!   (A Solana `open` needs a counterparty's sponsor endpoint, which this
//!   probe does not stand up.)
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

use connector_chain_rpc::{Circuit, RpcTransport};
use connector_settlement::batch::{BatchSettlementPayer, EvmReceiverTerms, ReceiverTerms};
use connector_settlement_evm::EvmBatchSettlementBackend;
use connector_settlement_solana::batch::SolanaBatchSettlement;
use ethers::signers::{LocalWallet, Signer as _};
use ethers::types::Address;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;

/// The fleet's committed `[settlement.evm]` (`infra/linode-relay/connector-rust.toml`).
const EVM_RPC: &str = "https://base-sepolia-rpc.publicnode.com";
const EVM_TOKEN: &str = "0x0C996d7c934c79a6255254875607Fe69df25C0E1";

/// The fleet's committed `[settlement.solana]`.
const SOLANA_RPC: &str = "https://api.devnet.solana.com";
const SOLANA_MINT: &str = "34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU";

const ONE_DAY: u64 = 86_400;

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
    let token = Address::from_str(EVM_TOKEN).expect("token");

    let started = Instant::now();
    let backend = EvmBatchSettlementBackend::connect(&transport, &key, token, 6, ONE_DAY)
        .await
        .expect("boot over the circuit");
    timed(
        "evm boot (chain id, x402 code, decimals, channel-id probe)",
        started,
        (),
    );
    assert_eq!(backend.domain().chain_id, 84_532, "Base Sepolia");
    if !funded {
        return;
    }

    let receiver = LocalWallet::new(&mut ethers::core::rand::thread_rng()).address();
    let terms = ReceiverTerms::Evm(EvmReceiverTerms {
        receiver: receiver.to_fixed_bytes(),
        token: token.to_fixed_bytes(),
        min_withdraw_delay_secs: ONE_DAY,
    });
    let started = Instant::now();
    let opened = backend
        .open(terms, 1_000)
        .await
        .expect("open over the circuit");
    timed("evm open (deposit, submit + confirm)", started, ());
    let channel = opened.presentation.channel().clone();

    let started = Instant::now();
    backend
        .top_up(&channel, 1_000)
        .await
        .expect("top up over the circuit");
    timed("evm top-up (submit + confirm)", started, ());

    let started = Instant::now();
    backend
        .start_withdrawal(&channel)
        .await
        .expect("start the withdrawal over the circuit");
    timed("evm initiateWithdraw (submit + confirm)", started, ());
    eprintln!(
        "[circuit] evm channel {} opened, topped up and withdrawing",
        channel.0
    );
}

#[tokio::test]
async fn the_solana_backend_boots_over_a_real_circuit() {
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
    let mint = Pubkey::from_str(SOLANA_MINT).expect("mint");

    let started = Instant::now();
    let connected =
        SolanaBatchSettlement::connect(&transport, &seed, mint, 6, ONE_DAY, 1_000_000).await;
    timed("solana boot", started, ());
    if !funded {
        let error = connected
            .err()
            .expect("an unfunded key is refused")
            .to_string();
        assert!(
            error.contains("holds no lamports"),
            "every boot read went over the circuit and the key was read as unfunded: {error}"
        );
        return;
    }
    let backend = connected.expect("boot over the circuit");
    assert_eq!(backend.cluster(), Some("devnet"));
}
