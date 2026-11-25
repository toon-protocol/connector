//! Spec #1684, issue #1714: the client edge finds a voucher's channel on the
//! network the voucher names, through the node's one collection of
//! settlement networks -- however many networks of a chain it holds.
//!
//! A node settling on two EVM networks, one Solana network and one XRPL
//! network, each over the in-memory batch settlement (the fake both of the
//! port's contract suites hold to the real adapters' rules, ADR 0007), is
//! driven through the real router with a real `POST /ilp` per voucher. Each
//! channel is opened on its own network only, so a fulfilled packet is
//! evidence that its voucher reached that network; a voucher naming a network
//! no table is for is refused by name.

use std::sync::Arc;

use base64::Engine;
use chrono::{TimeZone, Utc};
use connector_client_edge::{router_with_gate, ClientClaimGate};
use connector_config::StaticRoute;
use connector_domain::{EnvelopeRequest, EnvelopeResponse, Fulfill, Network, Prepare, Reject};
use connector_runtime::{
    AppOutcome, Connector, FakeAppClient, InMemoryJournal, InMemorySettlementNetwork,
    InProcessPeerTransport, SettlementNetwork, SettlementNetworks, TestClock,
};
use connector_signer::giftwrap::seal_request;
use connector_signer::{
    derive_evm_address, evm_batch_channel_id, evm_voucher_digest, solana_voucher_message,
    xrpl_family_seed, BatchChannelConfig, BatchSettlementDomain, LocalSigner, PublicKeyBytes,
    Signer, XrplAlgorithm, XrplSigner,
};
use ed25519_dalek::Signer as _;
use hyper::{Body, Request};
use libsecp256k1::{Message, PublicKey, SecretKey};
use tower::ServiceExt;

const PRICE: u64 = 100;
const PREFIX: &str = "g.example.app";
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

/// The same client config's channel on EVM chain `chain_id`: an EVM id binds
/// its chain, so the one config names a different channel on each network.
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

fn solana_voucher(network: &str, amount: u64) -> String {
    let signature = solana_key().sign(&solana_voucher_message(&SOLANA_CHANNEL, amount, 0));
    serde_json::json!({
        "version": "1.0",
        "blockchain": "solana",
        "network": network,
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

fn xrpl_voucher(network: &str, amount: u64) -> String {
    let signature = xrpl_key()
        .sign_claim(&XRPL_CHANNEL, u128::from(amount))
        .expect("an amount the ledger holds");
    serde_json::json!({
        "version": "1.0",
        "blockchain": "xrpl",
        "network": network,
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

/// Two EVM networks, one Solana network and one XRPL network, each holding
/// one channel of the client's, opened on that network alone. Base Sepolia
/// holds the EVM config's channel; Base holds that config's own channel only
/// when `base_holds_a_channel`.
fn four_networks(base_holds_a_channel: bool) -> SettlementNetworks {
    let base_sepolia = InMemorySettlementNetwork::fresh(Network::Eip155(BASE_SEPOLIA));
    base_sepolia.open_evm(&evm_config(), 1_000_000);
    let base = InMemorySettlementNetwork::fresh(Network::Eip155(BASE));
    if base_holds_a_channel {
        base.open_evm(&evm_config(), 1_000_000);
    }
    let solana = InMemorySettlementNetwork::fresh(network(SOLANA_NETWORK));
    solana.open_solana(SOLANA_CHANNEL, solana_key().public.to_bytes(), 1_000_000);
    let xrpl = InMemorySettlementNetwork::fresh(network(XRPL_NETWORK));
    xrpl.open_xrpl(XRPL_CHANNEL, xrpl_key().public_key(), 1_000_000);
    let mut networks = SettlementNetworks::new();
    for held in [base_sepolia, base, solana, xrpl] {
        networks
            .insert(Arc::new(held) as Arc<dyn SettlementNetwork>)
            .expect("four distinct networks");
    }
    networks
}

/// A node terminating one priced route, its client edge's gate settling on
/// `networks`.
struct Node {
    router: axum::Router,
    identity: PublicKeyBytes,
    app: Arc<FakeAppClient>,
}

fn node(networks: SettlementNetworks) -> Node {
    let route = StaticRoute::new_priced(PREFIX, "http://localhost:4000", PRICE).expect("a route");
    let app = Arc::new(FakeAppClient::new());
    app.respond(
        route.handler_url(),
        AppOutcome::Answered {
            response: EnvelopeResponse {
                status: 200,
                headers: vec![],
                body: b"delivered".to_vec(),
            },
        },
    );
    let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("settlement-networks"));
    let identity = signer.public_key().expect("a public key");
    let connector = Arc::new(
        Connector::new(
            vec![route],
            vec![],
            app.clone(),
            Arc::new(InProcessPeerTransport::new()),
            Arc::new(TestClock::new(
                Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
            )),
        )
        .with_identity_signer(Arc::clone(&signer)),
    );
    let gate = ClientClaimGate::restore(Arc::new(InMemoryJournal::new()))
        .expect("a fresh journal")
        .with_settlement_networks(networks);
    Node {
        router: router_with_gate(connector, signer, None, gate),
        identity,
        app,
    }
}

enum Outcome {
    Fulfilled,
    Rejected(Reject),
}

/// One `POST /ilp` paying `PRICE` with `voucher` in the claim header.
async fn pay(node: &Node, voucher: &str) -> Outcome {
    let plaintext = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: b"hello".to_vec(),
    }
    .encode();
    let (data, _) = seal_request(&plaintext, &node.identity).expect("seal");
    let prepare = Prepare {
        amount: PRICE,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: PREFIX.to_string(),
        data,
    };
    let request = Request::builder()
        .method("POST")
        .uri("/ilp")
        .header(
            connector_btp::CLAIM_HEADER,
            base64::engine::general_purpose::STANDARD.encode(voucher.as_bytes()),
        )
        .body(Body::from(prepare.encode()))
        .expect("a request");
    let response = node.router.clone().oneshot(request).await.expect("served");
    let bytes = hyper::body::to_bytes(response.into_body())
        .await
        .expect("a body");
    match Fulfill::decode(&bytes) {
        Ok(_) => Outcome::Fulfilled,
        Err(_) => Outcome::Rejected(Reject::decode(&bytes).expect("a fulfil or a reject")),
    }
}

#[tokio::test]
async fn a_voucher_on_each_settled_network_reaches_that_network() {
    let node = node(four_networks(false));
    for (name, voucher) in [
        ("eip155:84532", evm_voucher(BASE_SEPOLIA, PRICE)),
        (SOLANA_NETWORK, solana_voucher(SOLANA_NETWORK, PRICE)),
        (XRPL_NETWORK, xrpl_voucher(XRPL_NETWORK, PRICE)),
    ] {
        match pay(&node, &voucher).await {
            Outcome::Fulfilled => {}
            Outcome::Rejected(reject) => panic!("a voucher on {name} is refused: {reject:?}"),
        }
    }
    assert_eq!(node.app.deliveries().len(), 3);
}

/// Two networks of one chain are two entries: a voucher on Base is asked of
/// Base alone, which holds none of the client's channels until one is opened
/// there -- the channel on Base Sepolia is no help to it.
#[tokio::test]
async fn a_voucher_on_one_evm_network_is_never_found_on_the_other() {
    let node_without = node(four_networks(false));
    let Outcome::Rejected(reject) = pay(&node_without, &evm_voucher(BASE, PRICE)).await else {
        panic!("Base holds no channel of the client's");
    };
    assert_eq!(reject.code.as_str(), "F01", "{reject:?}");
    assert!(node_without.app.deliveries().is_empty());

    let node_with = node(four_networks(true));
    for (name, voucher) in [
        ("eip155:8453", evm_voucher(BASE, PRICE)),
        ("eip155:84532", evm_voucher(BASE_SEPOLIA, PRICE)),
    ] {
        match pay(&node_with, &voucher).await {
            Outcome::Fulfilled => {}
            Outcome::Rejected(reject) => panic!("a voucher on {name} is refused: {reject:?}"),
        }
    }
}

/// ADR 0086 decision 1: a voucher naming a network no settlement table is
/// for is refused, naming that network, on a chain the node settles on.
#[tokio::test]
async fn a_voucher_on_an_unsettled_network_is_refused_by_name() {
    let node = node(four_networks(false));
    for (unsettled, voucher) in [
        ("eip155:1", evm_voucher(1, PRICE)),
        (
            "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            solana_voucher("solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp", PRICE),
        ),
        ("xrpl:0", xrpl_voucher("xrpl:0", PRICE)),
    ] {
        let Outcome::Rejected(reject) = pay(&node, &voucher).await else {
            panic!("a voucher on {unsettled} is accepted");
        };
        let message = reject.message.clone();
        assert!(
            message.contains(&format!("network '{unsettled}'"))
                && message.contains("settles nothing"),
            "{unsettled}: {message}"
        );
    }
    assert!(node.app.deliveries().is_empty());
}
