//! The `[[pay_channels]]` money round trip on x402 vouchers (ADR 0042 item
//! 2, as ADR 0075 decision 6 amends it; issue #1380), both halves, nothing
//! faked between them: a payer covering its forwards with vouchers on its
//! **own outbound x402 channel**, registered the way a `[[pay_channels]]`
//! row registers it (`Connector::with_config_pay_channel`), and a payee
//! answering a real `POST /ilp` and a real `POST /ilp/claim-state` over a
//! real socket.
//!
//! # What this is for
//!
//! Before ADR 0075 this file held the `toon-channel` version of the same
//! defect: a payer that asked the wrong book where its claims stood re-signed
//! the same cumulative amount at a fresh nonce on every packet, and a priced
//! termination refused every packet after the first. A voucher has no nonce,
//! so the equivalent failure is sharper: a voucher that does not advance the
//! payee's one watermark is refused outright. Two properties hold that off,
//! and this file asserts both as the payer experiences them:
//!
//! 1. **Successive covered packets each advance the payee's watermark** by
//!    exactly what they forward, and the payee's `POST /ilp/claim-state`
//!    (`scheme: "batch-settlement"`) reports where the channel stands.
//! 2. **A journal that lost its latest vouchers is recovered from the next
//!    hop's claim-state** (decision 6): the payer restores its outbound
//!    channel at a watermark below the payee's, asks the payee before it
//!    signs again, and its next voucher advances past what the payee holds
//!    -- rather than signing one the payee refuses as not advancing.
//!
//! # Where the chain comes in
//!
//! The payer's vouchers are signed by x402's real `x402BatchSettlement`
//! paying half on a disposable `anvil`, so they are genuine and the payee
//! verifies them against the channel's `payerAuthorizer`. The payee's claim
//! gate runs over a fake of the batch-settlement **seam** that admits a
//! channel paying it -- the kind of fake `voucher_claims.rs` runs the real
//! gate over (ADR 0007: it holds which channels exist, and asserts no call).
//!
//! The Solana half this file once had was the `toon-channel` Solana claim;
//! a Solana peering's vouchers, and their claim-state restore, are held by
//! `connector-cli/tests/solana_peering_from_a_url.rs` against a real
//! validator.

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use url::Url;

use connector_client_edge::{
    router_with_gate, AdmittedEvmVoucherChannel, AdmittedSolanaVoucherChannel,
    BatchSettlementChannels, ChannelResolutionError, ClientChannelRegistry, ClientClaimGate,
};
use connector_config::StaticRoute;
use connector_domain::{EnvelopeRequest, EnvelopeResponse, PacketResponse, Prepare};
use connector_peer_http::{HttpPeerTransport, PeerRelation, ReqwestPeerClient};
use connector_runtime::{
    AppOutcome, Connector, FakeAppClient, FileJournal, HttpVoucherState, InMemoryJournal,
    InProcessPeerTransport, Journal, OutboundChannels, PeerRoute, PeerTransport, SettlementChain,
    SystemClock, VoucherStateSource,
};
use connector_settlement::batch::{BatchSettlementPayer, EvmReceiverTerms, ReceiverTerms};
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
const ANVIL_BASE_PORT: u16 = 23_500;
/// What the payee's route charges, and so what every covering voucher must
/// advance its watermark by.
const ROUTE_PRICE: u64 = 1_000;
/// The payer's opening deposit on its outbound channel.
const DEPOSIT: u128 = 10_000;
/// The next hop, as the payer's `[[peers]]` names it.
const NEXT_HOP: &str = "a-b";
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

/// The payee: a real `Connector` terminating one priced app route, behind a
/// real client edge on a real socket, its claim gate over [`PaysThisNode`].
fn spawn_payee(domain: BatchSettlementDomain) -> (std::net::SocketAddr, PublicKeyBytes) {
    let app_route =
        StaticRoute::new_priced(PREFIX, "http://app.example/", ROUTE_PRICE).expect("a route");
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
    let gate = ClientClaimGate::restore(
        ClientChannelRegistry::new(),
        Arc::new(InMemoryJournal::new()),
    )
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

/// One chain for a test: anvil, x402 placed, a FiatToken USDC the payer
/// holds, and the `TokenNetwork` registry the settlement key still connects
/// through until #1385.
struct Chain {
    anvil: Anvil,
    x402: X402Chain,
    token: Address,
    registry: Address,
}

impl Chain {
    async fn spawn() -> Chain {
        let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
        let mut x402 = X402Chain::place(&anvil.rpc_url).await;
        let token = x402.deploy_fiat_token().await;
        let registry = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
            .await
            .expect("a TokenNetwork registry to connect the settlement key through")
            .registry_address();
        x402.mint(token, address_of(DEPLOYER_PRIVATE_KEY), 1_000_000)
            .await;
        Chain {
            anvil,
            x402,
            token,
            registry,
        }
    }

    /// The payer's x402 channels, over the real paying half and a journal
    /// file at `journal`: what a booting node restores.
    async fn outbound_channels(&self, journal: &std::path::Path) -> Arc<OutboundChannels> {
        let payer = EvmSettlementBackend::connect(
            &connector_settlement_evm::RpcTransport::direct(&self.anvil.rpc_url)
                .expect("rpc transport"),
            DEPLOYER_PRIVATE_KEY,
            self.registry,
            self.token,
            6,
        )
        .await
        .expect("connect the payer's settlement key")
        .batch_settlement(86_400)
        .await
        .expect("the payer's x402 half");
        let journal: Arc<dyn Journal> =
            Arc::new(FileJournal::open(journal).expect("open the journal"));
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

    /// The payee's terms, as its self-description publishes them.
    fn payee_terms(&self) -> ReceiverTerms {
        ReceiverTerms::Evm(EvmReceiverTerms {
            receiver: address_of(COUNTERPARTY_PRIVATE_KEY).to_fixed_bytes(),
            token: self.token.to_fixed_bytes(),
            min_withdraw_delay_secs: 86_400,
        })
    }
}

/// The payer: a node whose one route forwards to [`NEXT_HOP`] over the real
/// ILP-over-HTTP carriage, covering every forward on `channel` -- registered
/// exactly as `connector-cli` wires a `[[pay_channels]]` row.
fn payer(
    outbound: Arc<OutboundChannels>,
    network: &str,
    channel: &str,
    payee: std::net::SocketAddr,
) -> Connector {
    let transport =
        HttpPeerTransport::new(Arc::new(ReqwestPeerClient::new(reqwest::Client::new())));
    transport.add_peer(PeerRelation::new(
        NEXT_HOP,
        Url::parse(&format!("http://{payee}/ilp")).expect("url"),
        std::time::Duration::from_secs(10),
    ));
    Connector::new(
        vec![],
        vec![PeerRoute::new(PREFIX, NEXT_HOP)],
        Arc::new(FakeAppClient::new()),
        Arc::new(transport) as Arc<dyn PeerTransport>,
        Arc::new(SystemClock),
    )
    .with_config_peer_ids([NEXT_HOP.to_string()])
    .with_outbound_channels(outbound, vec![(SettlementChain::Evm, network.to_string())])
    .with_config_pay_channel(
        NEXT_HOP,
        channel,
        &Url::parse(&format!("http://{payee}/ilp")).expect("url"),
        std::time::Duration::from_secs(10),
    )
    .expect("the pay channel is this node's own journaled outbound channel")
}

/// One crossing: a packet forwarded to the payee must fulfil with the app's
/// own answer.
async fn crossing(payer: &Connector, payee_identity: &PublicKeyBytes, body: &[u8]) {
    let (prepare, shared_secret) = sealed_prepare(body, payee_identity);
    let response = payer.handle_prepare(prepare).await;
    let PacketResponse::Fulfill(fulfill) = response else {
        panic!("a covered forward must fulfil: {response:?}");
    };
    let opened = open_response(&shared_secret, &fulfill.data).expect("open");
    assert_eq!(
        EnvelopeResponse::decode(&opened).expect("envelope").body,
        b"delivered"
    );
}

/// Where the payee's `POST /ilp/claim-state` says `channel` stands: the
/// answer the payer restores from, asked exactly as the payer asks it.
async fn claim_state(
    outbound: &OutboundChannels,
    channel: &str,
    payee: std::net::SocketAddr,
) -> u128 {
    let expires = u64::try_from(Utc::now().timestamp()).expect("after 1970") + 60;
    let signature = outbound
        .sign_challenge(channel, expires)
        .await
        .expect("sign the claim-state challenge");
    HttpVoucherState::new(reqwest::Client::new(), format!("http://{payee}/ilp"))
        .watermark(
            &outbound.presentation(channel).expect("a journaled channel"),
            expires,
            &signature,
        )
        .await
        .expect("the payee answers")
}

#[tokio::test]
async fn successive_covered_packets_each_advance_the_payees_watermark() {
    // `require_anvil`, not a bare availability check: it panics when `CI` is
    // set and skips only on a developer machine without Foundry.
    if !require_anvil() {
        return;
    }
    let chain = Chain::spawn().await;
    let (payee, payee_identity) = spawn_payee(chain.x402.domain());
    let network = format!("eip155:{}", chain.x402.chain_id());
    let state = tempfile::tempdir().expect("state dir");
    let outbound = chain
        .outbound_channels(&state.path().join("outbound-channels.log"))
        .await;
    let (opened, _) = outbound
        .open(chain.payee_terms(), DEPOSIT)
        .await
        .expect("open the payer's own channel toward the payee");
    let channel = opened.on_chain.id.0.clone();
    let payer = payer(Arc::clone(&outbound), &network, &channel, payee);

    for crossing_number in 1..=2u64 {
        crossing(&payer, &payee_identity, b"covered").await;
        assert_eq!(
            outbound.signed(&channel),
            Some(u128::from(crossing_number * ROUTE_PRICE)),
            "crossing {crossing_number} signs a voucher advanced by exactly what it forwards"
        );
    }
    assert_eq!(
        claim_state(&outbound, &channel, payee).await,
        u128::from(2 * ROUTE_PRICE),
        "and the payee's claim-state agrees with the payer's journal"
    );
}

#[tokio::test]
async fn a_lost_journal_is_recovered_from_the_next_hops_claim_state() {
    if !require_anvil() {
        return;
    }
    let chain = Chain::spawn().await;
    let (payee, payee_identity) = spawn_payee(chain.x402.domain());
    let network = format!("eip155:{}", chain.x402.chain_id());
    let state = tempfile::tempdir().expect("state dir");
    let journal = state.path().join("outbound-channels.log");
    let outbound = chain.outbound_channels(&journal).await;
    let (opened, _) = outbound
        .open(chain.payee_terms(), DEPOSIT)
        .await
        .expect("open");
    let channel = opened.on_chain.id.0.clone();
    // What a backup taken right after the open holds: the channel, and no
    // voucher signed on it.
    let snapshot = std::fs::read(&journal).expect("read the journal");

    let first = payer(Arc::clone(&outbound), &network, &channel, payee);
    crossing(&first, &payee_identity, b"one").await;
    crossing(&first, &payee_identity, b"two").await;
    assert_eq!(outbound.signed(&channel), Some(u128::from(2 * ROUTE_PRICE)));
    drop(first);
    drop(outbound);

    // The journal loses every voucher it recorded: the node restores its
    // channel from the backup, below where the payee's watermark stands.
    std::fs::write(&journal, &snapshot).expect("restore the backup");
    let outbound = chain.outbound_channels(&journal).await;
    let restored = outbound.signed(&channel).expect("the channel is journaled");
    assert!(
        restored < u128::from(2 * ROUTE_PRICE),
        "the restored journal is behind the payee: {restored}"
    );

    // The payer asks the payee before it signs again, so its next voucher
    // advances past the payee's watermark rather than being refused as not
    // advancing -- and the packet it covers is delivered.
    let restarted = payer(Arc::clone(&outbound), &network, &channel, payee);
    crossing(&restarted, &payee_identity, b"after the journal was lost").await;
    assert_eq!(
        outbound.signed(&channel),
        Some(u128::from(3 * ROUTE_PRICE)),
        "the next voucher is the payee's watermark plus exactly what this packet forwards"
    );
    assert_eq!(
        claim_state(&outbound, &channel, payee).await,
        u128::from(3 * ROUTE_PRICE)
    );
}
