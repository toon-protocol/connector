//! Spec #1689, issue #1721: startup is connect, then assemble, and assemble
//! takes parts that implement the ports. These tests connect a node whose
//! config names no chain, hand assemble in-memory settlement networks (the
//! fake both batch contract suites hold to the real adapters' rules, ADR
//! 0007) and an in-memory client-edge journal, and drive the node through
//! the router it returns: no chain is spawned.

mod support;

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use connector_config::Config;
use connector_domain::{Fulfill, JournalEntry, Network, Reject};
use connector_runtime::{
    InMemoryJournal, InMemorySettlementNetwork, Journal, JournalError, SettlementNetwork,
    SettlementNetworks,
};
use connector_signer::{
    derive_evm_address, evm_batch_channel_id, evm_voucher_digest, solana_voucher_message,
    xrpl_family_seed, BatchChannelConfig, BatchSettlementDomain, XrplAlgorithm, XrplSigner,
};
use ed25519_dalek::Signer as _;
use libsecp256k1::{Message, PublicKey, SecretKey};

use support::{paid_prepare, post_ilp, spawn_recording_app, RecordedWrite};

const PRICE: u64 = 100;
const ROUTE: &str = "g.assemble.app";
const BASE: u64 = 8_453;
const BASE_SEPOLIA: u64 = 84_532;
const SOLANA_NETWORK: &str = "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1";
const XRPL_NETWORK: &str = "xrpl:1";
const SOLANA_CHANNEL: [u8; 32] = [0xc3; 32];
const XRPL_CHANNEL: [u8; 32] = [0x5d; 32];

fn network(caip2: &str) -> Network {
    caip2.parse().expect("a CAIP-2 network")
}

fn evm_secret() -> SecretKey {
    SecretKey::parse(&[0x0a; 32]).expect("valid secret")
}

fn evm_config() -> BatchChannelConfig {
    BatchChannelConfig {
        payer: [0x11; 20],
        payer_authorizer: derive_evm_address(
            &PublicKey::from_secret_key(&evm_secret()).serialize(),
        ),
        receiver: [0x33; 20],
        receiver_authorizer: [0x33; 20],
        token: [0x55; 20],
        withdraw_delay: 86_400,
        salt: [0x66; 32],
    }
}

fn solana_key() -> ed25519_dalek::Keypair {
    let secret = ed25519_dalek::SecretKey::from_bytes(&[0x21; 32]).expect("seed");
    let public = (&secret).into();
    ed25519_dalek::Keypair { secret, public }
}

fn xrpl_key() -> XrplSigner {
    XrplSigner::from_family_seed(&xrpl_family_seed(XrplAlgorithm::Ed25519, &[0x07; 16]))
        .expect("a family seed")
}

/// The client's channel on EVM chain `chain_id`: an EVM id binds its chain,
/// so the one config names a different channel on each network.
fn evm_voucher(chain_id: u64, amount: u64) -> String {
    let domain = BatchSettlementDomain::x402(chain_id);
    let config = evm_config();
    let channel_id = evm_batch_channel_id(&domain, &config);
    let digest = evm_voucher_digest(&domain, &channel_id, u128::from(amount));
    let (signature, recovery) = libsecp256k1::sign(&Message::parse(&digest), &evm_secret());
    let mut signature = signature.serialize().to_vec();
    signature.push(recovery.serialize() + 27);
    let address = |bytes: &[u8; 20]| format!("0x{}", hex::encode(bytes));
    serde_json::json!({
        "version": "1.0",
        "blockchain": "evm",
        "network": format!("eip155:{chain_id}"),
        "scheme": "batch-settlement",
        "messageId": format!("voucher-{chain_id}-{amount}"),
        "timestamp": "2026-11-25T12:00:00.000Z",
        "senderId": "client",
        "channelId": format!("0x{}", hex::encode(channel_id)),
        "maxClaimableAmount": amount.to_string(),
        "signature": format!("0x{}", hex::encode(signature)),
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

fn solana_voucher(amount: u64) -> String {
    let signature = solana_key().sign(&solana_voucher_message(&SOLANA_CHANNEL, amount, 0));
    serde_json::json!({
        "version": "1.0",
        "blockchain": "solana",
        "network": SOLANA_NETWORK,
        "scheme": "batch-settlement",
        "messageId": format!("voucher-{amount}"),
        "timestamp": "2026-11-25T12:00:00Z",
        "senderId": "client",
        "channelId": bs58::encode(SOLANA_CHANNEL).into_string(),
        "maxClaimableAmount": amount.to_string(),
        "expiresAt": 0,
        "signature": bs58::encode(signature.to_bytes()).into_string(),
    })
    .to_string()
}

fn xrpl_voucher(amount: u64) -> String {
    let signature = xrpl_key()
        .sign_claim(&XRPL_CHANNEL, u128::from(amount))
        .expect("an amount the ledger holds");
    serde_json::json!({
        "version": "1.0",
        "blockchain": "xrpl",
        "network": XRPL_NETWORK,
        "scheme": "batch-settlement",
        "messageId": format!("voucher-{amount}"),
        "timestamp": "2026-11-25T12:00:00Z",
        "senderId": xrpl_key().classic_address(),
        "channelId": hex::encode_upper(XRPL_CHANNEL),
        "maxClaimableAmount": amount.to_string(),
        "signature": hex::encode_upper(signature),
    })
    .to_string()
}

fn settlement_networks(held: Vec<InMemorySettlementNetwork>) -> SettlementNetworks {
    let mut networks = SettlementNetworks::new();
    for held in held {
        networks
            .insert(Arc::new(held) as Arc<dyn SettlementNetwork>)
            .expect("distinct networks");
    }
    networks
}

/// Two EVM networks, one Solana network and one XRPL network, each holding
/// the client's channel on it alone.
fn four_networks() -> SettlementNetworks {
    let base_sepolia = InMemorySettlementNetwork::fresh(Network::Eip155(BASE_SEPOLIA));
    base_sepolia.open_evm(&evm_config(), 1_000_000);
    let base = InMemorySettlementNetwork::fresh(Network::Eip155(BASE));
    base.open_evm(&evm_config(), 1_000_000);
    let solana = InMemorySettlementNetwork::fresh(network(SOLANA_NETWORK));
    solana.open_solana(SOLANA_CHANNEL, solana_key().public.to_bytes(), 1_000_000);
    let xrpl = InMemorySettlementNetwork::fresh(network(XRPL_NETWORK));
    xrpl.open_xrpl(XRPL_CHANNEL, xrpl_key().public_key(), 1_000_000);
    settlement_networks(vec![base_sepolia, base, solana, xrpl])
}

/// A node's config naming one priced route to `app_addr` and no chain:
/// connect dials nothing, and the networks are handed to assemble.
struct NodeConfig {
    config: Config,
    _signer_key: tempfile::NamedTempFile,
}

fn node_config(app_addr: &str) -> NodeConfig {
    let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
    signer_key.write_all(&[0x07; 32]).expect("write");
    let config = support::load_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"

[signer]
key_file = "{signer_key}"

[[routes]]
prefix = "{ROUTE}"
handler_url = "http://{app_addr}"
price = {PRICE}
"#,
        signer_key = signer_key.path().display(),
    ));
    NodeConfig {
        config,
        _signer_key: signer_key,
    }
}

/// Connect `config`, then assemble it over `networks` and `journal`.
async fn assemble(
    config: &Config,
    networks: SettlementNetworks,
    journal: Arc<dyn Journal>,
) -> connector_cli::Runtime {
    let mut parts = connector_cli::connect(config).await.expect("connect");
    parts.settlement_networks = networks;
    parts.journals.client_edge = journal;
    connector_cli::assemble(parts).await.expect("assemble")
}

enum Outcome {
    Fulfilled,
    Rejected(Reject),
}

async fn pay(node: &connector_cli::Runtime, voucher: &str) -> Outcome {
    let receiver = node.signer.public_key().expect("the node's wrap key");
    let mut prepare = paid_prepare(ROUTE, &receiver);
    prepare.amount = PRICE;
    let response = post_ilp(&node.router(), voucher, prepare).await;
    match Fulfill::decode(&response) {
        Ok(_) => Outcome::Fulfilled,
        Err(_) => Outcome::Rejected(Reject::decode(&response).expect("a fulfil or a reject")),
    }
}

fn networks_seen(recorded: &Mutex<Vec<RecordedWrite>>) -> Vec<String> {
    recorded
        .lock()
        .unwrap()
        .iter()
        .map(|write| {
            write
                .header("x-toon-network")
                .expect("every paid write names its network")
                .to_string()
        })
        .collect()
}

/// The claim gate accepts a voucher on every network handed to assemble,
/// and the app is told the network each one was verified on -- on EVM,
/// where the node holds two networks, that is the connector asking the gate
/// (ADR 0086 decision 14), so it holds the gate from the moment it is built.
#[tokio::test]
async fn the_claim_gate_accepts_a_voucher_on_every_configured_network() {
    let (app_addr, recorded) = spawn_recording_app().await;
    let node_config = node_config(&app_addr);
    let node = assemble(
        &node_config.config,
        four_networks(),
        Arc::new(InMemoryJournal::new()),
    )
    .await;

    for (name, voucher) in [
        ("eip155:84532", evm_voucher(BASE_SEPOLIA, PRICE)),
        ("eip155:8453", evm_voucher(BASE, PRICE)),
        (SOLANA_NETWORK, solana_voucher(PRICE)),
        (XRPL_NETWORK, xrpl_voucher(PRICE)),
    ] {
        if let Outcome::Rejected(reject) = pay(&node, &voucher).await {
            panic!("a voucher on {name} is refused: {reject:?}");
        }
    }
    assert_eq!(
        networks_seen(&recorded),
        ["eip155:84532", "eip155:8453", SOLANA_NETWORK, XRPL_NETWORK]
    );
}

/// A [`Journal`] that counts how often it is read back whole.
#[derive(Default)]
struct CountingJournal {
    inner: InMemoryJournal,
    reads: AtomicUsize,
}

impl Journal for CountingJournal {
    fn append(&self, entry: &JournalEntry) -> Result<(), JournalError> {
        self.inner.append(entry)
    }

    fn append_batch(&self, entries: &[JournalEntry]) -> Result<(), JournalError> {
        self.inner.append_batch(entries)
    }

    fn read_all(&self) -> Result<Vec<JournalEntry>, JournalError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read_all()
    }
}

/// The client-edge journal is replayed once at startup, and that one replay
/// is both what restores the channel and what the gate resumes from: a
/// voucher accepted before a restart is not accepted again after it, and
/// the next one is.
#[tokio::test]
async fn a_claim_journaled_before_a_restart_is_honoured_after_it() {
    let (app_addr, recorded) = spawn_recording_app().await;
    let node_config = node_config(&app_addr);
    let chain = InMemorySettlementNetwork::fresh(Network::Eip155(BASE_SEPOLIA));
    chain.open_evm(&evm_config(), 1_000_000);
    let networks = settlement_networks(vec![chain]);
    let journal = Arc::new(CountingJournal::default());

    let before = assemble(
        &node_config.config,
        networks.clone(),
        Arc::clone(&journal) as Arc<dyn Journal>,
    )
    .await;
    assert_eq!(journal.reads.load(Ordering::SeqCst), 1, "replayed once");
    assert!(matches!(
        pay(&before, &evm_voucher(BASE_SEPOLIA, PRICE)).await,
        Outcome::Fulfilled
    ));
    drop(before);

    let after = assemble(
        &node_config.config,
        networks,
        Arc::clone(&journal) as Arc<dyn Journal>,
    )
    .await;
    assert_eq!(
        journal.reads.load(Ordering::SeqCst),
        2,
        "replayed once more, by the restart"
    );
    let Outcome::Rejected(_) = pay(&after, &evm_voucher(BASE_SEPOLIA, PRICE)).await else {
        panic!("the voucher spent before the restart is accepted again after it");
    };
    assert!(matches!(
        pay(&after, &evm_voucher(BASE_SEPOLIA, 2 * PRICE)).await,
        Outcome::Fulfilled
    ));
    assert_eq!(
        recorded.lock().unwrap().len(),
        2,
        "two writes were paid for"
    );
}
