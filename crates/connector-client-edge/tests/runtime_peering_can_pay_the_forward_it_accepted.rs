//! Issue #1217, on vouchers (ADR 0075 decision 6, issue #1378): a peering
//! established by `POST /peers` can pay the forward it accepted -- every
//! forwarded PREPARE covered by a voucher on this node's own outbound x402
//! channel, from the first packet and from the first packet after a
//! restart.
//!
//! A sibling of `connector-cli/tests/peering_from_a_url.rs`, not a
//! replacement: that file drives two config-built nodes over both
//! carriages. This one proves the **paying half's wiring inside
//! `connector-runtime`** -- `establish_peering` registering a payable hop,
//! `POST /routes/peers`'s guard accepting a route to it, `cover_forward`
//! signing and journaling a voucher on every forward, and a restart
//! rehydrating a payable hop rather than a name -- at the level this file
//! always worked at:
//!
//! * the payer is a real `Connector` whose outbound channel is opened and
//!   signed on by x402's real `x402BatchSettlement` on a disposable `anvil`,
//!   so the vouchers it signs are genuine and the payee verifies them;
//! * the payee is a real client edge answering a real `POST /ilp` and a real
//!   `POST /ilp/claim-state` over a real socket, its claim gate over a fake
//!   of the batch-settlement **seam** that admits a channel paying it -- the
//!   kind of fake `voucher_claims.rs` runs the real gate over (ADR 0007: it
//!   holds which channels exist, and asserts no call);
//! * the wire between them is the real ILP-over-HTTP peer carriage.

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use url::Url;

use connector_client_edge::{
    router_with_gate, AdmittedEvmVoucherChannel, AdmittedSolanaVoucherChannel,
    BatchSettlementChannels, ChannelResolutionError, ClientClaimGate,
};
use connector_config::StaticRoute;
use connector_domain::x402::{X402BatchSettlementEvmTerms, X402BatchSettlementTerms};
use connector_domain::{
    EnvelopeRequest, EnvelopeResponse, NodeFacts, NodeSelfDescription, PacketResponse, Prepare,
    Price, VoucherSignerFact,
};
use connector_peer_http::{HttpPeerTransport, PeerRelation, ReqwestPeerClient};
use connector_runtime::{
    AppOutcome, ChannelBranch, Connector, FakeAppClient, FileJournal, InMemoryJournal,
    InProcessPeerTransport, Journal, OutboundChannels, PeerRouteStore, PeerRouteTableError,
    PeerTransport, RuntimePeerChannel, RuntimePeering, SelfDescriptionError, SelfDescriptionSource,
    SettlementChain, SystemClock,
};
use connector_settlement::batch::BatchSettlementPayer;
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{
    require_anvil, Anvil, COUNTERPARTY_PRIVATE_KEY, DEPLOYER_PRIVATE_KEY,
};
use connector_settlement_evm::EvmSettlementBackend;
use connector_signer::giftwrap::{open_response, seal_request};
use connector_signer::{
    BatchChannelConfig, BatchSettlementDomain, LocalSigner, PublicKeyBytes, Signer,
};
use ethers::signers::{LocalWallet, Signer as _};
use ethers::types::Address;

/// This binary's own base port for [`Anvil::spawn`], clear of every other
/// anvil binary's range.
const ANVIL_BASE_PORT: u16 = 23_300;
const ROUTE_PRICE: u64 = 1_000;
const DEPOSIT: u128 = 10_000;
const PEER_ID: &str = "payee";
const PREFIX: &str = "g.example.payee.app";

fn key_bytes(key: &str) -> [u8; 32] {
    let key = key.trim_start_matches("0x");
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&key[2 * i..2 * i + 2], 16).expect("hex");
    }
    out
}

fn address_of(key: &str) -> Address {
    LocalWallet::from_bytes(&key_bytes(key))
        .expect("key")
        .address()
}

fn spelled(address: Address) -> String {
    format!("{address:#x}")
}

/// The payee's batch-settlement seam: it admits any channel a presented
/// config names **this payee** as receiver of, backed by [`DEPOSIT`]. The
/// gate above it re-hashes the config to the voucher's channel id and
/// verifies the voucher against the config's `payerAuthorizer`, so what it
/// accepts is exactly what a real backend's admission would let through.
#[derive(Debug)]
struct PaysThisNode {
    domain: BatchSettlementDomain,
    receiver: [u8; 20],
    admitted: std::sync::Mutex<HashMap<[u8; 32], BatchChannelConfig>>,
}

#[async_trait]
impl BatchSettlementChannels for PaysThisNode {
    fn evm_domain(&self) -> Option<BatchSettlementDomain> {
        Some(self.domain)
    }

    fn accepts_solana(&self) -> bool {
        false
    }

    async fn evm(
        &self,
        channel_id: &[u8; 32],
        presented_config: Option<&BatchChannelConfig>,
    ) -> Result<Option<AdmittedEvmVoucherChannel>, ChannelResolutionError> {
        let mut admitted = self.admitted.lock().expect("lock");
        let config = match presented_config {
            Some(config) if config.receiver == self.receiver => *config,
            Some(_) => return Ok(None),
            None => match admitted.get(channel_id) {
                Some(config) => *config,
                None => return Ok(None),
            },
        };
        admitted.insert(*channel_id, config);
        Ok(Some(AdmittedEvmVoucherChannel {
            config,
            max_cumulative: u64::try_from(DEPOSIT).expect("small"),
        }))
    }

    async fn solana(
        &self,
        _channel_account: &[u8; 32],
    ) -> Result<Option<AdmittedSolanaVoucherChannel>, ChannelResolutionError> {
        Ok(None)
    }
}

/// `establish_peering` fetches this instead of dialling a host.
struct FixedSelfDescription(NodeSelfDescription);

#[async_trait]
impl SelfDescriptionSource for FixedSelfDescription {
    async fn fetch(&self, _url: &Url) -> Result<NodeSelfDescription, SelfDescriptionError> {
        Ok(self.0.clone())
    }
}

fn sealed_prepare(body: &[u8], receiver: &PublicKeyBytes) -> (Prepare, [u8; 32]) {
    let plaintext = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: body.to_vec(),
    }
    .encode();
    let (data, shared_secret) = seal_request(&plaintext, receiver).expect("seal");
    (
        Prepare {
            amount: ROUTE_PRICE,
            expires_at: Utc::now() + ChronoDuration::minutes(5),
            greeting: false,
            destination: PREFIX.to_string(),
            data,
        },
        shared_secret,
    )
}

/// The payee: a real `Connector` terminating one priced app route, behind
/// a real client edge on a real socket, its claim gate over
/// [`PaysThisNode`].
fn spawn_payee(domain: BatchSettlementDomain) -> (std::net::SocketAddr, PublicKeyBytes) {
    let app_route = StaticRoute::new_priced(PREFIX, "http://app.example/", ROUTE_PRICE)
        .expect("a valid priced route");
    let app_client = Arc::new(FakeAppClient::new());
    app_client.respond(
        app_route.handler_url(),
        AppOutcome::Answered {
            response: EnvelopeResponse {
                status: 200,
                headers: vec![],
                body: b"delivered".to_vec(),
            },
        },
    );
    let identity = LocalSigner::generate("payee-edge-identity");
    let identity_public_key = identity.public_key().expect("a public key");
    let connector = Arc::new(
        Connector::new(
            vec![app_route],
            vec![],
            app_client,
            Arc::new(InProcessPeerTransport::new()),
            Arc::new(SystemClock),
        )
        .with_identity_signer(Arc::new(identity)),
    );
    let seam = Arc::new(PaysThisNode {
        domain,
        receiver: address_of(COUNTERPARTY_PRIVATE_KEY).to_fixed_bytes(),
        admitted: std::sync::Mutex::new(HashMap::new()),
    });
    let gate = ClientClaimGate::restore(Arc::new(InMemoryJournal::new()))
        .expect("a fresh journal")
        .with_batch_settlement(seam as Arc<dyn BatchSettlementChannels>);
    let router_signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("payee-router"));
    let app = router_with_gate(connector, router_signer, None, gate);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind payee socket");
    let addr = listener.local_addr().expect("payee addr");
    tokio::spawn(
        axum::Server::from_tcp(listener)
            .expect("serve")
            .serve(app.into_make_service()),
    );
    (addr, identity_public_key)
}

/// The payee's self-description: its x402 terms and its voucher signer.
fn payee_document(
    addr: std::net::SocketAddr,
    chain_id: u64,
    token: Address,
) -> NodeSelfDescription {
    let payee = spelled(address_of(COUNTERPARTY_PRIVATE_KEY));
    let network = format!("eip155:{chain_id}");
    NodeSelfDescription::describe(
        &NodeFacts {
            ilp_addresses: vec!["g.example.payee".to_string()],
            http_endpoint: Some(format!("http://{addr}/ilp")),
            btp_endpoint: None,
            peer_carriages: vec!["http".to_string()],
            batch_settlements: vec![X402BatchSettlementTerms::Evm(X402BatchSettlementEvmTerms {
                network: network.clone(),
                asset: spelled(token),
                pay_to: payee.clone(),
                receiver_authorizer: payee.clone(),
                min_withdraw_delay_secs: 86_400,
                name: "USDC".to_string(),
                version: "2".to_string(),
            })],
            voucher_signers: vec![VoucherSignerFact {
                network,
                signer: payee,
            }],
        },
        None,
        Vec::new(),
        None,
    )
}

/// The payer's carriage: ILP-over-HTTP to the payee, as a peering's
/// registrar would dial it.
fn transport_to(payee: std::net::SocketAddr) -> Arc<dyn PeerTransport> {
    let transport =
        HttpPeerTransport::new(Arc::new(ReqwestPeerClient::new(reqwest::Client::new())));
    transport.add_peer(PeerRelation::new(
        PEER_ID,
        Url::parse(&format!("http://{payee}/ilp")).expect("url"),
        std::time::Duration::from_secs(10),
    ));
    Arc::new(transport)
}

/// The payer's x402 channels, over the real paying half on `anvil` and a
/// journal file in `state_dir`: what a booting node restores.
async fn outbound_channels(
    rpc_url: &str,
    registry: Address,
    token: Address,
    state_dir: &std::path::Path,
) -> Arc<OutboundChannels> {
    let payer = EvmSettlementBackend::connect(
        &connector_settlement_evm::RpcTransport::direct(rpc_url).expect("rpc transport"),
        DEPLOYER_PRIVATE_KEY,
        registry,
        token,
        6,
    )
    .await
    .expect("connect the payer's settlement key")
    .batch_settlement(86_400)
    .await
    .expect("the payer's x402 half");
    let journal: Arc<dyn Journal> = Arc::new(
        FileJournal::open(state_dir.join("outbound-channels.log")).expect("open the journal"),
    );
    Arc::new(
        OutboundChannels::restore(
            journal,
            vec![(
                SettlementChain::Evm,
                Arc::new(payer) as Arc<dyn BatchSettlementPayer>,
            )],
        )
        .await
        .expect("the journal replays"),
    )
}

/// The full claim: `establish_peering` opens this node's own channel and
/// registers a payable hop on it, `POST /routes/peers`'s guard accepts a
/// route to it, a packet originated over the peering fulfils -- twice, each
/// voucher advanced by exactly the forward -- and a restart's rehydrated
/// row still pays, above the watermark it restored.
#[tokio::test]
async fn a_runtime_established_peering_can_pay_the_forward_it_accepted() {
    if !require_anvil() {
        return;
    }
    let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
    let mut x402 = X402Chain::place(&anvil.rpc_url).await;
    let token = x402.deploy_fiat_token().await;
    let registry = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("a TokenNetwork registry to connect the settlement key through")
        .registry_address();
    x402.mint(token, address_of(DEPLOYER_PRIVATE_KEY), 1_000_000)
        .await;

    let (payee_addr, payee_identity) = spawn_payee(x402.domain());
    let document = payee_document(payee_addr, x402.chain_id(), token);
    let network = format!("eip155:{}", x402.chain_id());

    let state_dir = tempfile::tempdir().expect("temp state dir");
    let store_path = state_dir.path().join("runtime_peers.json");
    let boot = |outbound: Arc<OutboundChannels>| {
        let (store, peers, routes) = PeerRouteStore::open(&store_path).expect("open the store");
        Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            transport_to(payee_addr),
            Arc::new(SystemClock),
        )
        .with_outbound_channels(outbound, vec![(SettlementChain::Evm, network.clone())])
        .with_self_description_source(Arc::new(FixedSelfDescription(document.clone())))
        // The payee's endpoint is a loopback `http://` socket.
        .with_peer_allow_plaintext_endpoints(true)
        .with_runtime_peer_route_store(store, peers, routes)
    };
    let outbound = outbound_channels(&anvil.rpc_url, registry, token, state_dir.path()).await;
    let payer = boot(Arc::clone(&outbound));

    // ── The write ADR 0058 promises: accept AND pay ─────────────────────
    let established = payer
        .establish_peering(
            PEER_ID,
            &Url::parse("http://ignored.example/ilp").expect("url"),
            0,
            0,
            Some(SettlementChain::Evm),
            Some(DEPOSIT),
        )
        .await
        .expect("establishing a peering against a reachable, payable document must succeed");
    assert_eq!(established.channel.status, ChannelBranch::Created);
    let channel = established.channel.id.clone();
    payer
        .upsert_runtime_peer_route(PREFIX, PEER_ID, Price::FREE)
        .expect("a peering paid over its own outbound channel is routable");

    // ── Two crossings, each covered by a voucher ────────────────────────
    for (crossing, body) in [(1u64, b"first".as_slice()), (2, b"second".as_slice())] {
        let (prepare, shared_secret) = sealed_prepare(body, &payee_identity);
        let response = payer.handle_prepare(prepare).await;
        let PacketResponse::Fulfill(fulfill) = response else {
            panic!("crossing {crossing} must fulfil: {response:?}");
        };
        let opened = open_response(&shared_secret, &fulfill.data).expect("open");
        assert_eq!(
            EnvelopeResponse::decode(&opened).expect("envelope").body,
            b"delivered"
        );
        assert_eq!(
            outbound.signed(&channel),
            Some(u128::from(crossing * ROUTE_PRICE)),
            "each crossing signs a voucher advanced by exactly what it forwards"
        );
    }

    // ── A restart rehydrates a payable hop, not a name ──────────────────
    drop(payer);
    drop(outbound);
    let outbound = outbound_channels(&anvil.rpc_url, registry, token, state_dir.path()).await;
    let payer = boot(Arc::clone(&outbound));
    let (prepare, shared_secret) = sealed_prepare(b"after a restart", &payee_identity);
    let response = payer.handle_prepare(prepare).await;
    let PacketResponse::Fulfill(fulfill) = response else {
        panic!("a restart must not turn a payable peering accept-only: {response:?}");
    };
    assert!(open_response(&shared_secret, &fulfill.data).is_ok());
    assert_eq!(
        outbound.signed(&channel),
        Some(u128::from(3 * ROUTE_PRICE)),
        "the restored watermark is where the next voucher is signed from"
    );
}

/// The exact shape issue #1217 found, on the x402 row: a runtime peering
/// written with a binding but no hop ever registered for it cannot take a
/// route -- the guard checks the paying hop, never the row.
#[tokio::test]
async fn a_peering_with_a_binding_but_no_paying_hop_cannot_pay_a_route_to_it() {
    let connector = Connector::new(
        vec![],
        vec![],
        Arc::new(FakeAppClient::new()),
        Arc::new(InProcessPeerTransport::new()),
        Arc::new(SystemClock),
    );
    let peering = RuntimePeering {
        fee: 0,
        max_packet_amount: 0,
        endpoint: Some("https://peer.example/ilp".to_string()),
        edge_identity: None,
        client_edge_url: Some("https://peer.example/ilp".to_string()),
        channels: vec![RuntimePeerChannel::EvmVoucher {
            outbound_channel_id: format!("0x{}", "ab".repeat(32)),
            voucher_signer: format!("0x{}", "aa".repeat(20)),
            network: "eip155:31337".to_string(),
        }],
    };
    connector
        .upsert_runtime_peer("half-bound", peering)
        .expect("a peering with a binding is accepted at write time");

    let error = connector
        .upsert_runtime_peer_route("g.example.half", "half-bound", Price::FREE)
        .expect_err("no paying hop was ever registered for this peering");
    assert!(
        matches!(error, PeerRouteTableError::PeerHasNoPayChannel { .. }),
        "expected the pay-channel guard to fire, got {error:?}"
    );
}
