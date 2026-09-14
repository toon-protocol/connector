//! Issue #1217: a peering established by `POST /peers` (ADR 0058) can
//! accept a claim but could never pay one -- nothing populated the outbound
//! CLIENT hop `Connector::cover_forward` reads, so every packet originated
//! over the peering was refused `T00` naming a `[[pay_channels]]` row that
//! ADR 0058 says an operator no longer needs to write.
//!
//! This is a sibling of `connector-cli/tests/peering_from_a_url.rs`, not a
//! replacement for it: that file proves the CHANNEL half against a real
//! `anvil` chain (derivation, idempotence, trust-on-first-use). This proves
//! the PAYMENT half -- the thing #1217 found missing -- at the level
//! `pay_channel_claim_state_round_trip.rs` (this file's actual sibling)
//! already works at: two real `connector-runtime::Connector`s, a real
//! `POST /ilp/claim-state` over a real socket, and `InProcessPeerTransport`
//! standing in for the wire so this is about the WIRING inside
//! `connector-runtime`, not about a chain (ADR 0007 tier 1/2) or about which
//! peer carriage `establish_peering` would have dialled.
//!
//! The settlement backend is a small fixed-id fake rather than
//! `InMemorySettlementBackend`: that one's auto-incrementing decimal ids are
//! accepted by `ClaimBook` but refused by `ClientChannelRegistry`, which
//! `POST /ilp/claim-state`'s challenge resolution needs and which requires
//! canonical `0x`-hex -- the shape every real `SettlementBackend` already
//! returns.

use std::net::TcpListener;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use url::Url;

use connector_client_edge::{
    router_with_gate, ClientChannelRegistry, ClientClaimGate, DepositFloor, EvmChannel,
};
use connector_config::StaticRoute;
use connector_domain::x402::{X402ChainSettlementTerms, X402SettlementTerms};
use connector_domain::{
    EnvelopeRequest, EnvelopeResponse, NodeFacts, NodeSelfDescription, PacketResponse, Prepare,
    Price,
};
use connector_runtime::{
    AppOutcome, ChannelBranch, ChannelDomain, Connector, FakeAppClient, InMemoryJournal,
    InProcessPeerTransport, OutboundClientLedger, PeerRouteStore, PeerRouteTableError,
    PeerTransport, RuntimePeerChannel, RuntimePeering, SelfDescriptionError, SelfDescriptionSource,
    SettlementChain, SystemClock,
};
use connector_settlement::{
    ChannelId, ChannelState, ChannelStatus, Claim, SettlementBackend, SettlementError,
};
use connector_signer::giftwrap::{open_response, seal_request};
use connector_signer::{derive_evm_address, to_hex, Address, LocalSigner, PublicKeyBytes, Signer};

const CHAIN_ID: u64 = 31_337;
const TOKEN_NETWORK: [u8; 20] = [0x42; 20];
const ROUTE_PRICE: u64 = 1_000;
const PEER_ID: &str = "payee";
const PREFIX: &str = "g.example.payee.app";

fn channel_hex() -> String {
    format!("0x{}", "ab".repeat(32))
}

/// A [`SettlementBackend`] that always answers the same channel id, however
/// it is asked. `establish_peering` only ever needs `open`/`live_channel_with`
/// on this write; the rest of the port belongs to the channel-lifecycle
/// surface, which this test never touches -- reached, they would be a bug in
/// this test, not a case to make behave plausibly.
struct FixedChannelBackend {
    channel_id: ChannelId,
}

#[async_trait]
impl SettlementBackend for FixedChannelBackend {
    async fn open(
        &self,
        _counterparty: Vec<u8>,
        _settlement_timeout: ChronoDuration,
    ) -> Result<ChannelId, SettlementError> {
        Ok(self.channel_id.clone())
    }

    async fn fund(
        &self,
        _channel: &ChannelId,
        _amount: u128,
    ) -> Result<ChannelState, SettlementError> {
        unreachable!("this test never funds a channel")
    }

    async fn redeem(
        &self,
        _channel: &ChannelId,
        _claim: Claim,
    ) -> Result<ChannelState, SettlementError> {
        unreachable!("this test never redeems a claim")
    }

    async fn close(&self, _channel: &ChannelId) -> Result<ChannelState, SettlementError> {
        unreachable!("this test never closes a channel")
    }

    async fn settle(&self, _channel: &ChannelId) -> Result<ChannelState, SettlementError> {
        unreachable!("this test never settles a channel")
    }

    async fn channel_state(&self, channel: &ChannelId) -> Result<ChannelState, SettlementError> {
        // `Connector::open_channel` reads this back immediately after
        // `open` to build its `ChannelView` answer -- never reached for any
        // other reason in this test.
        Ok(ChannelState {
            id: channel.clone(),
            counterparty: Vec::new(),
            status: ChannelStatus::Open,
            counterparty_deposited: 0,
            own_deposited: 0,
            redeemed: 0,
        })
    }

    async fn live_channel_with(
        &self,
        _counterparty: Vec<u8>,
    ) -> Result<Option<ChannelId>, SettlementError> {
        // Always "no channel yet" -- so `establish_peering` always takes the
        // `Created` branch through `open` above, deterministically.
        Ok(None)
    }
}

/// `establish_peering` fetches this instead of dialling a real host -- the
/// same seam `connector-runtime`'s own
/// `a_write_that_cannot_land_is_refused_before_the_fetch` exercises.
struct FixedSelfDescription(NodeSelfDescription);

#[async_trait]
impl SelfDescriptionSource for FixedSelfDescription {
    async fn fetch(&self, _url: &Url) -> Result<NodeSelfDescription, SelfDescriptionError> {
        Ok(self.0.clone())
    }
}

fn sealed_prepare_data(body: &[u8], receiver_public: &PublicKeyBytes) -> (Vec<u8>, [u8; 32]) {
    let plaintext = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: body.to_vec(),
    }
    .encode();
    seal_request(&plaintext, receiver_public).expect("seal")
}

fn sample_prepare(
    destination: &str,
    amount: u64,
    data: Vec<u8>,
    _shared_secret: &[u8; 32],
) -> Prepare {
    Prepare {
        amount,
        expires_at: Utc::now() + ChronoDuration::minutes(5),
        greeting: false,
        destination: destination.to_string(),
        data,
    }
}

/// The payee: a real `Connector` bound in the PEER role to `channel_hex()`
/// (so it accepts `payer_address`'s claims on it) and terminating one priced
/// app route with a canned answer, plus a real `POST /ilp/claim-state`
/// server over a real socket -- `pay_channel_claim_state_round_trip.rs`'s
/// own `spawn_payee` shape, with an app route added so a covered forward
/// can actually be delivered and fulfilled rather than merely claim-verified.
async fn spawn_payee(
    payer_address: Address,
) -> (std::net::SocketAddr, PublicKeyBytes, Arc<Connector>) {
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

    let identity_signer = LocalSigner::generate("payee-edge-identity");
    let identity_public_key = identity_signer
        .public_key()
        .expect("a secp256k1 signer produces a public key");
    let identity_signer: Arc<dyn Signer> = Arc::new(identity_signer);

    let connector = Arc::new(
        Connector::new(
            vec![app_route],
            vec![],
            app_client,
            Arc::new(InProcessPeerTransport::new()),
            Arc::new(SystemClock),
        )
        .with_identity_signer(identity_signer)
        .with_channel_verification_key(channel_hex(), payer_address)
        .with_channel_domain(
            channel_hex(),
            ChannelDomain {
                chain_id: CHAIN_ID,
                token_network_address: TOKEN_NETWORK,
            },
        )
        .expect("a well-formed channel id"),
    );

    let mut registry = ClientChannelRegistry::new();
    registry
        .record_evm(
            &channel_hex(),
            EvmChannel {
                counterparty: payer_address,
                chain_id: CHAIN_ID,
                token_network_address: TOKEN_NETWORK,
                deposit_floor: DepositFloor::Unknown,
            },
        )
        .expect("a 32-byte channel id");
    let gate = ClientClaimGate::restore(registry, Arc::new(InMemoryJournal::new()))
        .expect("a fresh in-memory journal has nothing to replay");

    let router_signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("payee-router-identity"));
    let app = router_with_gate(Arc::clone(&connector), router_signer, None, gate);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind payee socket");
    let addr = listener.local_addr().expect("payee addr");
    let server = axum::Server::from_tcp(listener)
        .expect("axum server from tcp listener")
        .serve(app.into_make_service());
    tokio::spawn(server);

    (addr, identity_public_key, connector)
}

/// The full claim: `establish_peering` derives and registers a payable
/// hop, `POST /routes/peers`'s guard accepts a route to it, a packet
/// originated over that peering actually fulfils (twice, so the payee's
/// watermark genuinely advances rather than replaying ADR 0004's retired
/// postpay claim), and a restart's rehydrated row still pays.
#[tokio::test]
async fn a_runtime_established_peering_can_pay_the_forward_it_accepted() {
    let payer_settlement_signer = LocalSigner::generate("payer-settlement");
    let payer_address = derive_evm_address(
        &payer_settlement_signer
            .public_key()
            .expect("a secp256k1 signer"),
    );
    let payer_settlement_signer: Arc<dyn Signer> = Arc::new(payer_settlement_signer);

    let (payee_addr, payee_identity, payee) = spawn_payee(payer_address).await;

    let fixed_channel_id = ChannelId(channel_hex());
    let backend = FixedChannelBackend {
        channel_id: fixed_channel_id.clone(),
    };

    // Whoever the payee's self-description says it is -- a placeholder
    // settlement address, since `FixedChannelBackend` ignores it, and the
    // real point under test is what happens once a channel exists, not how
    // its counterparty bytes were chosen.
    let document = NodeSelfDescription::describe(
        &NodeFacts {
            ilp_addresses: vec!["g.example.payee".to_string()],
            http_endpoint: Some(format!("http://{payee_addr}/ilp")),
            btp_endpoint: None,
            peer_carriages: vec!["http".to_string()],
            settlements: vec![X402ChainSettlementTerms::Evm(X402SettlementTerms {
                chain: format!("evm:{CHAIN_ID}"),
                settlement_address: to_hex(&[0x22u8; 20]),
                token_network_registry: to_hex(&[0x99u8; 20]),
                token_network: to_hex(&TOKEN_NETWORK),
                token_address: to_hex(&[0x77u8; 20]),
                decimals: 6,
            })],
        },
        None,
        Vec::new(),
        None,
    );

    let mut transport = InProcessPeerTransport::new();
    transport.add_peer(PEER_ID, Arc::clone(&payee));
    let transport: Arc<dyn PeerTransport> = Arc::new(transport);

    let state_dir = tempfile::tempdir().expect("temp state dir");
    let store_path = state_dir.path().join("runtime_peers.json");
    let (store, peers, routes) = PeerRouteStore::open(&store_path).expect("open a fresh store");

    let payer = Connector::new(
        vec![],
        vec![],
        Arc::new(FakeAppClient::new()),
        Arc::clone(&transport),
        Arc::new(SystemClock),
    )
    .with_settlement(SettlementChain::Evm, Arc::new(backend))
    .with_signer(Arc::clone(&payer_settlement_signer))
    .with_outbound_client_ledger(Arc::new(OutboundClientLedger::in_memory()))
    .with_self_description_source(Arc::new(FixedSelfDescription(document)))
    // The payee's endpoint is a loopback `http://` socket, same as any
    // `local/` topology's own opt-in for a TLS terminator this test does
    // not have.
    .with_peer_allow_plaintext_endpoints(true)
    .with_runtime_peer_route_store(store, peers, routes);

    // ── The write ADR 0058 promises: accept AND pay ─────────────────────
    let established = payer
        .establish_peering(
            PEER_ID,
            &Url::parse("http://ignored.example/ilp").expect("url"),
            0,
            0,
            Some(SettlementChain::Evm),
        )
        .await
        .expect("establishing a peering against a reachable, payable document must succeed");
    assert_eq!(established.channel.status, ChannelBranch::Created);
    assert_eq!(established.channel.id, fixed_channel_id.0);

    // Issue #1217's guard fix: a peering `establish_peering` just wired a
    // CLIENT-role hop for is payable, so `POST /routes/peers` to it must
    // not be refused `PeerHasNoPayChannel` -- the guard that, before this
    // fix, tested the PEER-role bindings (always non-empty here) and so
    // never caught a peering that could accept a claim but sign none.
    payer
        .upsert_runtime_peer_route(PREFIX, PEER_ID, Price::FREE)
        .expect("a payable peering must be routable");

    // ── First crossing: the peering can actually pay ────────────────────
    let (data, shared_secret) = sealed_prepare_data(b"first crossing", &payee_identity);
    let prepare = sample_prepare(PREFIX, ROUTE_PRICE, data, &shared_secret);
    let response = payer.handle_prepare(prepare).await;
    let PacketResponse::Fulfill(fulfill) = response else {
        panic!(
            "expected a fulfil -- issue #1217's bug answers a T00 naming a missing \
             '[[pay_channels]]' row here instead: {response:?}"
        );
    };
    let opened = open_response(&shared_secret, &fulfill.data).expect("open the sealed response");
    let envelope = EnvelopeResponse::decode(&opened).expect("decode envelope response");
    assert_eq!(envelope.status, 200);
    assert_eq!(envelope.body, b"delivered");

    assert_eq!(
        payee
            .peer_channel_watermark(&fixed_channel_id.0)
            .map(|w| (w.nonce, w.cumulative_amount)),
        Some((1, ROUTE_PRICE)),
        "the payee's own peer book must show the payer's claim, advanced by the forward"
    );

    // ── Second crossing: genuinely covered, not stuck replaying ─────────
    // ADR 0004's retired postpay claim signed the SAME cumulative amount at
    // a fresh nonce every time -- a nonce that advances while the amount
    // does not, which `pay_channel_claim_state_round_trip.rs` measured as
    // the exact shape of that defect.
    let (data2, shared_secret2) = sealed_prepare_data(b"second crossing", &payee_identity);
    let prepare2 = sample_prepare(PREFIX, ROUTE_PRICE, data2, &shared_secret2);
    let response2 = payer.handle_prepare(prepare2).await;
    assert!(
        matches!(response2, PacketResponse::Fulfill(_)),
        "expected a second fulfil: {response2:?}"
    );
    assert_eq!(
        payee
            .peer_channel_watermark(&fixed_channel_id.0)
            .map(|w| (w.nonce, w.cumulative_amount)),
        Some((2, 2 * ROUTE_PRICE)),
        "each crossing must advance the payee's watermark by what it forwards"
    );

    // ── A restart rehydrates a payable hop, not a name ───────────────────
    let (store2, peers2, routes2) = PeerRouteStore::open(&store_path).expect("reopen the store");
    assert!(
        peers2.contains_key(PEER_ID),
        "the peering itself must have survived the restart"
    );
    let payer_after_restart = Connector::new(
        vec![],
        vec![],
        Arc::new(FakeAppClient::new()),
        Arc::clone(&transport),
        Arc::new(SystemClock),
    )
    .with_signer(payer_settlement_signer)
    .with_outbound_client_ledger(Arc::new(OutboundClientLedger::in_memory()))
    .with_runtime_peer_route_store(store2, peers2, routes2);

    let (data3, shared_secret3) = sealed_prepare_data(b"after a restart", &payee_identity);
    let prepare3 = sample_prepare(PREFIX, ROUTE_PRICE, data3, &shared_secret3);
    let response3 = payer_after_restart.handle_prepare(prepare3).await;
    let PacketResponse::Fulfill(fulfill3) = response3 else {
        panic!(
            "expected a fulfil after rehydration -- a restart must not turn a payable peering \
             back into an accept-only one: {response3:?}"
        );
    };
    let opened3 = open_response(&shared_secret3, &fulfill3.data).expect("open sealed response");
    let envelope3 = EnvelopeResponse::decode(&opened3).expect("decode envelope response");
    assert_eq!(envelope3.status, 200);
    assert_eq!(
        payee
            .peer_channel_watermark(&fixed_channel_id.0)
            .map(|w| (w.nonce, w.cumulative_amount)),
        Some((3, 3 * ROUTE_PRICE)),
        "the same channel's watermark keeps advancing after the payer's restart"
    );
}

/// The exact shape issue #1217 found, reproduced directly: a runtime
/// peering with a PEER-role channel binding (`RuntimePeering::channels`,
/// non-empty for every peering `establish_peering` ever writes) but no
/// CLIENT-role outbound hop ever registered for it -- the old guard
/// (`peering.channels.is_empty()`) never caught this, and this is the
/// unit test proving the fixed one (testing `outbound_client_hops`
/// instead) does.
#[tokio::test]
async fn a_peering_with_a_peer_role_channel_but_no_client_role_hop_cannot_pay_a_route_to_it() {
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
        channels: vec![RuntimePeerChannel::Evm {
            channel_id: channel_hex(),
            counterparty_key: to_hex(&[0xaa; 20]),
            chain_id: CHAIN_ID,
            token_network: to_hex(&TOKEN_NETWORK),
        }],
    };
    connector
        .upsert_runtime_peer("half-bound", peering)
        .expect("a peering with a peer-role channel binding is accepted at write time");

    let error = connector
        .upsert_runtime_peer_route("g.example.half", "half-bound", Price::FREE)
        .expect_err("no outbound client hop was ever registered for this peering");
    assert!(
        matches!(error, PeerRouteTableError::PeerHasNoPayChannel { .. }),
        "expected the pay-channel guard to fire, got {error:?}"
    );
}
