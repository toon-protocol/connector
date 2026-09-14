//! The BTP peer carriage end to end (`docs/protocol/peer-carriage-spec.md`,
//! issue #727): a [`BtpPeerTransport`] **dials**, a [`PeerSession`]
//! **accepts**, and the frames between them are the ones §3's table names.
//!
//! Nothing here is a fake shortcut past the thing under test. The two sides
//! are joined by an in-memory duplex standing in for the websocket and
//! *only* for the websocket: every frame is encoded and decoded by
//! `connector-btp`, every role decision is
//! `connector_peer_btp::role_gate::decide`'s, every claim is judged by the real
//! `ClaimBook` behind a real `Connector`, and the payer reaches the payee
//! only through the `PeerTransport` port. What is not exercised is TLS and
//! the socket itself.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use connector_btp::{
    decode_frame, encode_message, encode_response, BtpFrame, BtpSessionHandle, OutboundRequests,
    ProtocolData, AUTH_PROTOCOL, BTP_ERROR, BTP_RESPONSE, BTP_TRANSFER, CLAIM_PROTOCOL,
    CONTENT_TYPE_TEXT,
};
use connector_config::StaticRoute;
use connector_domain::{EnvelopeRequest, EnvelopeResponse, PacketResponse, Prepare};
use connector_peer_auth::PeerAuthPolicy;
use connector_peer_btp::accept::{PeerAcceptPolicy, PeerSession, SessionEnd};
use connector_peer_btp::dial::{DialError, PeerDialer, PeerRelation};
use connector_peer_btp::{
    ack, AcceptedClaims, BtpPeerTransport, ClaimEnforcementPolicy, PeerCarriageState,
};
use connector_runtime::{
    ChannelDomain, ClaimAckOutcome, ClaimRejectReason, ClaimSignature, ClaimStateDomain,
    ClaimStateSource, ClaimWatermark, Clock, Connector, EvmDomain, FakeAppClient, InMemoryJournal,
    InProcessPeerTransport, Journal, OutboundClientError, OutboundClientLedger, PeerForward,
    PeerRoute, PeerTransport, TestClock, WireClaim,
};
use connector_signer::{
    derive_evm_address, evm_balance_proof_digest, EvmBalanceProof, LocalSigner, Signature, Signer,
};
use tokio::sync::mpsc;
use url::Url;

// ─── fixtures ───

const CHAIN_ID: u64 = 84_532;
const TOKEN_NETWORK: [u8; 20] = [0x33; 20];
const PEER_ID: &str = "peer-b";

fn channel_id() -> String {
    format!("0x{:064x}", 7)
}

/// The channel a FORWARDING fixture pays its own next hop from -- a
/// different channel from [`channel_id`], which is the one the arriving
/// peering's claims are judged against.
fn pay_channel_id() -> String {
    format!("0x{:064x}", 8)
}

/// The next hop reporting where this node's claims on that channel stand.
/// A fake upholding `ClaimStateSource`'s contract, not a stub with
/// expectations (ADR 0007).
struct ReportsAWatermark;

#[async_trait::async_trait]
impl ClaimStateSource for ReportsAWatermark {
    async fn watermark(
        &self,
        _channel: &[u8; 32],
        _domain: &ClaimStateDomain,
    ) -> Result<ClaimWatermark, OutboundClientError> {
        Ok(ClaimWatermark {
            nonce: 0,
            cumulative: 0,
            available: Some(u128::MAX),
        })
    }
}

/// ADR 0042's send half, which issue #1145 made unavoidable: a connector
/// covers every PREPARE it sends, and one it cannot cover it refuses `T00`
/// before the transport is reached. Any fixture here that FORWARDS needs
/// this -- `Config::load` refuses the file that would produce one without it
/// (`ConfigError::PayChannelUnbound`), so a fixture lacking it is testing a
/// node no operator could deploy.
fn covering(connector: Connector, peer_id: &str) -> Connector {
    connector
        .with_signer(Arc::new(LocalSigner::generate("forwarding-settlement")))
        .with_outbound_client_ledger(Arc::new(OutboundClientLedger::in_memory()))
        .with_outbound_client_hop(
            peer_id,
            pay_channel_id(),
            EvmDomain {
                chain_id: domain().chain_id,
                token_network: domain().token_network_address,
            },
            Arc::new(ReportsAWatermark),
        )
        .expect("a bytes32 channel id")
}

fn clock() -> Arc<TestClock> {
    Arc::new(TestClock::new(
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
    ))
}

fn domain() -> ChannelDomain {
    ChannelDomain {
        chain_id: CHAIN_ID,
        token_network_address: TOKEN_NETWORK,
    }
}

/// A claim on [`channel_id`] signed by `signer`, exactly as `ClaimBook`
/// signs one: the EIP-712 `BalanceProof` digest of ADR 0024, with
/// `lockedAmount`/`locksRoot` as zeros.
fn sign_claim(signer: &dyn Signer, nonce: u64, cumulative_amount: u64) -> WireClaim {
    let mut on_chain_id = [0u8; 32];
    on_chain_id[31] = 7;
    let proof = EvmBalanceProof {
        channel_id: on_chain_id,
        nonce,
        transferred_amount: u128::from(cumulative_amount),
        locked_amount: 0,
        locks_root: [0u8; 32],
        chain_id: CHAIN_ID,
        token_network_address: TOKEN_NETWORK,
    };
    WireClaim {
        channel_id: channel_id(),
        nonce,
        cumulative_amount,
        signature: ClaimSignature::Evm(
            signer
                .sign(&evm_balance_proof_digest(&proof))
                .expect("sign"),
        ),
    }
}

/// The same claim on an arbitrary channel: `byte` is the on-chain id's last
/// byte and `channel` its `0x`-padded spelling, so a fixture can name a
/// channel this connector holds no record of without reaching for a second
/// signer.
fn sign_claim_on(
    signer: &dyn Signer,
    channel: &str,
    byte: u8,
    nonce: u64,
    cumulative_amount: u64,
) -> WireClaim {
    let mut on_chain_id = [0u8; 32];
    on_chain_id[31] = byte;
    let proof = EvmBalanceProof {
        channel_id: on_chain_id,
        nonce,
        transferred_amount: u128::from(cumulative_amount),
        locked_amount: 0,
        locks_root: [0u8; 32],
        chain_id: CHAIN_ID,
        token_network_address: TOKEN_NETWORK,
    };
    WireClaim {
        channel_id: channel.to_string(),
        nonce,
        cumulative_amount,
        signature: ClaimSignature::Evm(
            signer
                .sign(&evm_balance_proof_digest(&proof))
                .expect("sign"),
        ),
    }
}

/// The payee: a connector with no routes -- so every packet it is handed
/// answers `F02` and the *claim*'s verdict is visibly independent of the
/// packet's (§6.2) -- and one `[[peer_channels]]`-shaped channel whose
/// counterparty is `payer`.
fn payee(payer: &dyn Signer) -> Arc<Connector> {
    let counterparty = derive_evm_address(&payer.public_key().unwrap());
    Arc::new(
        Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            clock(),
        )
        .with_channel_verification_key(channel_id(), counterparty)
        .with_channel_domain(channel_id(), domain())
        .expect("a bytes32 channel id"),
    )
}

/// As [`payee`], but with one terminated, priced route -- the fixture issue
/// #880's price-coverage gate tests need, since a payee with no routes at
/// all (`payee`) never reaches that gate (§3.1: the gate is scoped to a
/// `Terminated` route's own price, exactly like the pre-existing amount
/// check right below it in `Connector::handle_peer_prepare`).
fn payee_with_route(payer: &dyn Signer, route: StaticRoute) -> Arc<Connector> {
    let counterparty = derive_evm_address(&payer.public_key().unwrap());
    Arc::new(
        Connector::new(
            vec![route],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            clock(),
        )
        .with_channel_verification_key(channel_id(), counterparty)
        .with_channel_domain(channel_id(), domain())
        .expect("a bytes32 channel id"),
    )
}

/// The one priced, terminated route issue #880's gate and issue #1104's
/// restart tests both need.
fn priced_route() -> StaticRoute {
    StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap()
}

/// This payee's identity key, deterministic so that a node built before a
/// restart and the one built after it are the same node to a sender that
/// sealed to it (issue #1104).
fn payee_identity() -> Arc<dyn Signer> {
    Arc::new(LocalSigner::from_secret_bytes("payee-identity", [0x5c; 32]).expect("identity signer"))
}

/// As [`payee_with_route`], but journaling to `journal` and delivering to
/// `app_client`: the fixture a **restart** needs (issue #1104). The journal
/// is the only thing a node keeps across one (ADR 0005), so a second
/// connector built over the same journal -- new `ClaimBook`, new
/// `AcceptedClaims`, new everything else -- is exactly what a restarted
/// payee is, with its inbound watermarks rebuilt by replay.
fn payee_journaling_to(
    payer: &dyn Signer,
    route: StaticRoute,
    app_client: Arc<FakeAppClient>,
    journal: Arc<dyn Journal>,
) -> Arc<Connector> {
    let counterparty = derive_evm_address(&payer.public_key().unwrap());
    Arc::new(
        Connector::new(
            vec![route],
            vec![],
            app_client,
            Arc::new(InProcessPeerTransport::new()),
            clock(),
        )
        .with_channel_verification_key(channel_id(), counterparty)
        .with_channel_domain(channel_id(), domain())
        .expect("a bytes32 channel id")
        .with_identity_signer(payee_identity())
        .with_journal(journal)
        .expect("the journal replays clean"),
    )
}

/// A policy in which `PEER_ID` is configured and its channel is bound --
/// P2 satisfiable, so P3 is the only thing left to decide role.
fn bound_policy() -> Arc<PeerAuthPolicy> {
    let channel = channel_id();
    Arc::new(PeerAuthPolicy::new(
        vec![PEER_ID],
        vec![(channel.as_str(), PEER_ID)],
    ))
}

fn carriage(connector: Arc<Connector>, policy: Arc<PeerAuthPolicy>) -> Arc<PeerCarriageState> {
    carriage_with_enforcement(
        connector,
        policy,
        Arc::new(ClaimEnforcementPolicy::default()),
    )
}

/// [`carriage`], with an explicit [`ClaimEnforcementPolicy`] rather than the
/// default (empty, so every peer reads
/// `ForwardedClaimEnforcement::Observe`). Only ADR 0042's forwarded rule
/// answers to this policy: the terminated rule's own knob was deleted with
/// its escape hatch (issue #1077) and refuses unconditionally.
fn carriage_with_enforcement(
    connector: Arc<Connector>,
    policy: Arc<PeerAuthPolicy>,
    enforcement: Arc<ClaimEnforcementPolicy>,
) -> Arc<PeerCarriageState> {
    Arc::new(PeerCarriageState::new(
        connector,
        policy,
        Arc::new(AcceptedClaims::new()),
        enforcement,
        PeerAcceptPolicy::default(),
    ))
}

/// The next hop a forwarded arrival is carried to (ADR 0042's item 3), and
/// the destination that resolves to it.
const NEXT_HOP_ID: &str = "next-hop";
const FORWARDED_DESTINATION: &str = "g.example.onward";

/// This peering's flat fee, and the client-edge `price` its forwarded route
/// carries. Both are deliberately non-zero and deliberately *not* what a
/// forwarded arrival must cover -- ADR 0042 requires the packet's own
/// `amount`, so a claim advancing either of these figures is short.
const FORWARD_FEE: u64 = 3;
const FORWARD_ROUTE_PRICE: u64 = 5;

/// The amount every forwarded-arrival test sends, matching [`prepare`].
const ARRIVING_AMOUNT: u64 = 100;

/// As [`payee`], but **forwarding**: one `peer_id` route over which
/// [`FORWARDED_DESTINATION`] reaches a real second connector that terminates
/// it. The fixture ADR 0042's item 3 needs, since neither `payee` (no
/// routes) nor `payee_with_route` (a termination) ever reaches a
/// `ClientRouteKind::Forwarded` arrival.
///
/// Returns the next hop's own app client and identity signer too, so a test
/// can seal a packet the far end can actually fulfil and then prove the
/// packet really was carried rather than merely not refused.
fn forwarding_payee(payer: &dyn Signer) -> (Arc<Connector>, Arc<FakeAppClient>, Arc<dyn Signer>) {
    let next_hop_route = StaticRoute::new(FORWARDED_DESTINATION, "http://localhost:4100").unwrap();
    let app_client = Arc::new(FakeAppClient::new());
    app_client.respond(
        next_hop_route.handler_url(),
        connector_runtime::AppOutcome::Answered {
            response: EnvelopeResponse {
                status: 200,
                headers: vec![],
                body: b"delivered by the next hop".to_vec(),
            },
        },
    );
    let identity: Arc<dyn Signer> = Arc::new(LocalSigner::generate("next-hop-identity"));
    let next_hop = Arc::new(
        Connector::new(
            vec![next_hop_route],
            vec![],
            app_client.clone(),
            Arc::new(InProcessPeerTransport::new()),
            clock(),
        )
        .with_identity_signer(Arc::clone(&identity)),
    );
    let mut onward = InProcessPeerTransport::new();
    onward.add_peer(NEXT_HOP_ID, next_hop);

    let counterparty = derive_evm_address(&payer.public_key().unwrap());
    let connector = Arc::new(covering(
        Connector::new(
            vec![],
            vec![PeerRoute::new_priced(
                FORWARDED_DESTINATION,
                NEXT_HOP_ID,
                FORWARD_ROUTE_PRICE,
            )],
            Arc::new(FakeAppClient::new()),
            Arc::new(onward),
            clock(),
        )
        .with_peer_fees([(NEXT_HOP_ID.to_string(), FORWARD_FEE)])
        .with_channel_verification_key(channel_id(), counterparty)
        .with_channel_domain(channel_id(), domain())
        .expect("a bytes32 channel id"),
        NEXT_HOP_ID,
    ));
    (connector, app_client, identity)
}

/// A PREPARE sealed to `identity`'s public key (ADR 0018/0019) so the hop
/// that finally terminates it can fulfil, plus the shared secret needed to
/// open the answer. Sealing is orthogonal to every gate here and is what
/// makes "the packet was carried" provable rather than inferred.
fn sealed_prepare_to(identity: &dyn Signer, destination: &str, amount: u64) -> (Prepare, [u8; 32]) {
    let envelope = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: b"hello".to_vec(),
    };
    let identity_public = identity.public_key().expect("identity public key");
    let (data, shared_secret) =
        connector_signer::giftwrap::seal_request(&envelope.encode(), &identity_public)
            .expect("seal");
    (
        Prepare {
            amount,
            expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
            greeting: false,
            destination: destination.to_string(),
            data,
        },
        shared_secret,
    )
}

/// A policy in which `PEER_ID` enforces ADR 0042's forwarded rule. There is
/// no terminated setting to leave alone: ADR 0029's rule always enforces
/// (issue #1077 deleted `claim_enforcement`).
fn forwarded_enforcing() -> Arc<ClaimEnforcementPolicy> {
    Arc::new(ClaimEnforcementPolicy::of(vec![(
        PEER_ID,
        connector_config::ForwardedClaimEnforcement::Enforce,
    )]))
}

fn prepare(destination: &str) -> Prepare {
    Prepare {
        amount: 100,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: destination.to_string(),
        data: b"sealed to whoever terminates this route".to_vec(),
    }
}

// ─── the in-memory duplex standing in for the websocket ───

/// Runs one accepting [`PeerSession`] per dial, joined to the dialing side
/// by two channels. Every byte between them goes through the real codec.
struct LoopbackDialer {
    state: Arc<PeerCarriageState>,
    /// Every frame the dialing side wrote, in order -- so a test can assert
    /// what actually went on the wire (§3's table) and not merely what came
    /// back.
    sent: Arc<Mutex<Vec<BtpFrame>>>,
    /// How many sockets were opened. Counted here because there is nothing
    /// else left to count: session reuse used to be provable from the one
    /// `auth` frame a session sent, and ADR 0060 deleted it.
    dials: Arc<AtomicUsize>,
}

impl LoopbackDialer {
    fn new(state: Arc<PeerCarriageState>) -> Arc<LoopbackDialer> {
        Arc::new(LoopbackDialer {
            state,
            sent: Arc::new(Mutex::new(Vec::new())),
            dials: Arc::new(AtomicUsize::new(0)),
        })
    }
}

#[async_trait]
impl PeerDialer for LoopbackDialer {
    async fn dial(&self, _peer_id: &str, _endpoint: &Url) -> Result<BtpSessionHandle, DialError> {
        self.dials.fetch_add(1, Ordering::SeqCst);
        let (to_peer, mut to_peer_rx) = mpsc::channel::<Vec<u8>>(32);
        let (from_peer, mut from_peer_rx) = mpsc::channel::<Vec<u8>>(32);
        let outbound = Arc::new(OutboundRequests::new());
        let handle = BtpSessionHandle::new(to_peer, Arc::clone(&outbound));

        // The accepting side, reading exactly the bytes the dialing side
        // wrote.
        let (tap, sent) = (mpsc::channel::<Vec<u8>>(32), Arc::clone(&self.sent));
        let (tapped, tapped_rx) = tap;
        tokio::spawn(async move {
            while let Some(bytes) = to_peer_rx.recv().await {
                sent.lock()
                    .expect("sent frames lock")
                    .push(decode_frame(&bytes).expect("our own encoder"));
                if tapped.send(bytes).await.is_err() {
                    break;
                }
            }
        });
        let session = PeerSession::new(Arc::clone(&self.state), from_peer);
        tokio::spawn(session.run(tapped_rx));

        // The answer path: a RESPONSE/ERROR resolves whichever outbound
        // request it names (§7.3), which is the only correlation either
        // carriage has or needs.
        tokio::spawn(async move {
            while let Some(bytes) = from_peer_rx.recv().await {
                if let Ok(frame) = decode_frame(&bytes) {
                    outbound.resolve(frame);
                }
            }
        });
        Ok(handle)
    }
}

/// A dialer that never connects -- the "the remote does not expose what we
/// dial" case §2.2 says is not locally detectable and must surface as an
/// ordinary dial failure.
struct DeadDialer;

#[async_trait]
impl PeerDialer for DeadDialer {
    async fn dial(&self, peer_id: &str, endpoint: &Url) -> Result<BtpSessionHandle, DialError> {
        Err(DialError {
            peer_id: peer_id.to_string(),
            endpoint: endpoint.to_string(),
            reason: "connection refused".to_string(),
        })
    }
}

/// The Solana channel account this relation's `[[peer_channels]]` row binds
/// (issue #759) -- a real base58-encoded 32-byte account, reused only as a
/// well-formed fixture.
fn solana_channel_account() -> String {
    "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin".to_string()
}

/// The program id that channel was opened under -- the deployed SPL Token
/// program's, reused here only as a well-formed base58 32-byte fixture.
const SOLANA_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

fn relation() -> PeerRelation {
    let mut domains = HashMap::new();
    domains.insert(
        channel_id(),
        connector_peer_btp::PeerClaimDomain {
            chain_id: CHAIN_ID,
            token_network: TOKEN_NETWORK,
        },
    );
    let mut solana_program_ids = HashMap::new();
    solana_program_ids.insert(solana_channel_account(), SOLANA_PROGRAM_ID.to_string());
    PeerRelation::new(
        PEER_ID,
        Url::parse("wss://peer.example:443/btp").unwrap(),
        domains,
        solana_program_ids,
        Duration::from_millis(30_000),
        Duration::from_millis(30_000),
    )
}

fn transport(dialer: Arc<dyn PeerDialer>, payer: &dyn Signer) -> BtpPeerTransport {
    let transport = BtpPeerTransport::new(
        dialer,
        derive_evm_address(&payer.public_key().unwrap()),
        clock() as Arc<dyn Clock>,
    );
    transport.add_peer(relation());
    transport
}

// ─── §3, §6: a claim rides a PREPARE and is acknowledged ───

/// §6.2, the property whose loss would silently destroy ADR 0024's
/// semantics: **one RESPONSE carries two independent answers**. The packet
/// is rejected (the payee has no route for it) and the claim that rode it
/// is accepted, on the same frame.
#[tokio::test]
async fn a_claim_riding_a_prepare_is_judged_independently_of_the_packet() {
    let payer_signer = LocalSigner::generate("payer");
    let state = carriage(payee(&payer_signer), bound_policy());
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);
    let claim = sign_claim(&payer_signer, 1, 500);

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport
        .forward(PEER_ID, prepare("g.nowhere"), Some(claim))
        .await;

    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F02"),
        other => panic!("expected the payee's own reject, got {other:?}"),
    }
    assert_eq!(ack, ClaimAckOutcome::Accepted);
    assert!(reached, "the peer answered, so this hop forwarded");
}

/// §3's table, on the wire: the claim rides a `payment-channel-claim`
/// entry as **raw UTF-8 JSON** on a MESSAGE whose `ilpPacket` is the OER
/// PREPARE -- and it is the session's **first** frame, because ADR 0060
/// deleted the credential that used to ride ahead of it. Nothing
/// authenticates the peering but the claim, so nothing precedes it.
#[tokio::test]
async fn the_frames_a_dialed_peering_puts_on_the_wire_are_the_ones_section_3_names() {
    let payer_signer = LocalSigner::generate("payer");
    let state = carriage(payee(&payer_signer), bound_policy());
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);

    let _ = transport
        .forward(
            PEER_ID,
            prepare("g.nowhere"),
            Some(sign_claim(&payer_signer, 1, 500)),
        )
        .await;

    let sent = dialer.sent.lock().expect("sent frames lock").clone();
    assert!(
        sent.iter().all(|frame| frame
            .protocol_data
            .iter()
            .all(|pd| pd.name != AUTH_PROTOCOL)),
        "a dialed peering put a peer credential on the wire"
    );

    let message = &sent[0];
    assert_eq!(message.frame_type, connector_btp::BTP_MESSAGE);
    assert!(
        Prepare::decode(&message.ilp_packet).is_ok(),
        "the OER PREPARE rides ilpPacket"
    );
    let claim = message
        .protocol_data
        .iter()
        .find(|pd| pd.name == CLAIM_PROTOCOL)
        .expect("the claim rode as protocolData");
    let claim_json: serde_json::Value =
        serde_json::from_slice(&claim.data).expect("raw UTF-8 JSON, no base64 layer");
    assert_eq!(claim_json["blockchain"], "evm");
    assert_eq!(claim_json["nonce"], 1);
}

// ─── §3, §6: FLUSH is a TRANSFER ───

/// §3's FLUSH row: a **TRANSFER (type 7)** whose `amount` is the claim's
/// new cumulative, carrying the claim in `payment-channel-claim` and **no
/// `ilpPacket`**; answered by a RESPONSE carrying the `claim-ack`.
#[tokio::test]
async fn a_flush_is_a_transfer_whose_amount_is_the_claims_new_cumulative() {
    let payer_signer = LocalSigner::generate("payer");
    let state = carriage(payee(&payer_signer), bound_policy());
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);

    let ack = transport
        .flush(PEER_ID, sign_claim(&payer_signer, 1, 900))
        .await;

    assert_eq!(ack, ClaimAckOutcome::Accepted);
    let sent = dialer.sent.lock().expect("sent frames lock").clone();
    let flush = sent
        .iter()
        .find(|frame| frame.frame_type == BTP_TRANSFER)
        .expect("the flush rode a TRANSFER, not a MESSAGE");
    assert_eq!(flush.amount, Some(900), "amount is the new cumulative");
    assert!(
        flush.ilp_packet.is_empty(),
        "a FLUSH carries no ILP packet at all"
    );
    assert!(flush
        .protocol_data
        .iter()
        .any(|pd| pd.name == CLAIM_PROTOCOL));
}

/// Issue #759's AC, exercised on the real BTP wire: a Solana claim flushed
/// over this carriage carries the `[[peer_channels]]`-configured
/// `programId` -- never the deleted `PLACEHOLDER_SOLANA_PROGRAM_ID` -- and
/// the same bytes round-trip through the client edge's own validator
/// (`claim_json::parse`), the same proof
/// `the_frames_a_dialed_peering_puts_on_the_wire_are_the_ones_section_3_names`
/// gives the EVM shape.
#[tokio::test]
async fn a_flushed_solana_claim_carries_its_configured_program_id_on_the_wire() {
    let payer_signer = LocalSigner::generate("payer");
    let state = carriage(payee(&payer_signer), bound_policy());
    let dialer = LoopbackDialer::new(state);
    let mut transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);
    transport.set_solana_signer_public_key([0x77; 32]);

    let claim = WireClaim {
        channel_id: solana_channel_account(),
        nonce: 1,
        cumulative_amount: 500,
        signature: ClaimSignature::Solana([0x5a; 64]),
    };

    let _ = transport.flush(PEER_ID, claim.clone()).await;

    let sent = dialer.sent.lock().expect("sent frames lock").clone();
    let flush = sent
        .iter()
        .find(|frame| frame.frame_type == BTP_TRANSFER)
        .expect("the flush rode a TRANSFER");
    let entry = flush
        .protocol_data
        .iter()
        .find(|pd| pd.name == CLAIM_PROTOCOL)
        .expect("the claim rode as protocolData");
    let claim_json: serde_json::Value =
        serde_json::from_slice(&entry.data).expect("raw UTF-8 JSON");
    assert_eq!(claim_json["blockchain"], "solana");
    assert_eq!(claim_json["programId"], SOLANA_PROGRAM_ID);
    assert_eq!(claim_json["channelAccount"], solana_channel_account());

    let parsed = connector_peer_btp::claim_json::parse(&entry.data).expect("round trip");
    assert_eq!(parsed, claim);
}

/// The same wire assertion, but with the relation built by
/// [`BtpPeerTransport::add_peers_from_config`] from a **loaded config**
/// rather than by hand -- because the value under test is precisely the one
/// this carriage must not choose for itself. The HTTP twin is
/// `a_solana_claim_flushed_from_a_loaded_config_declares_the_settlement_tables_program`
/// in `connector-peer-http` (§9: a behaviour on one carriage and not the
/// other is a defect, and the two relations are built by duplicated code).
///
/// Since issue #1128 a Solana `[[peer_channels]]` row MUST NOT restate a
/// `program_id`; it resolves it from `[settlement.solana] program_id`, the
/// only program this node can redeem a claim through. This closes the hop
/// between that table and the `programId` a peer claim carries, and it is
/// half of what `peer-carriage-spec.md` §4.1 relies on when it says a
/// peer-edge check of the declared field would have nothing to find: one
/// configured value renders the label here and keys the `SolanaChannel` an
/// inbound claim is verified against. The other half is
/// `tests/a_peer_claims_declared_program_is_not_consulted.rs`.
#[tokio::test]
async fn a_solana_claim_flushed_from_a_loaded_config_declares_the_settlement_tables_program() {
    use std::io::Write;

    let state_dir = tempfile::tempdir().expect("temp state dir");
    let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
    key_file.write_all(b"not a real key").expect("write key");
    let toml = format!(
        r#"
client_edge_addr = "127.0.0.1:3000"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[[peers]]
id = "{PEER_ID}"
endpoint = "wss://peer.example:443/btp"

[[peer_channels]]
peer_id = "{PEER_ID}"
channel_account = "{channel_account}"
counterparty_key = "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin"

[settlement.solana]
rpc_url = "https://api.devnet.solana.com"
program_id = "{SOLANA_PROGRAM_ID}"
token_address = "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin"
decimals = 6

[settlement.solana.key]
key_file = "{key_file}"
"#,
        state_dir = state_dir.path().display(),
        key_file = key_file.path().display(),
        channel_account = solana_channel_account(),
    );
    let mut config_file = tempfile::Builder::new()
        .suffix(".toml")
        .tempfile()
        .expect("temp config file");
    config_file
        .write_all(toml.as_bytes())
        .expect("write config");
    let config = connector_config::Config::load(config_file.path()).expect("load");

    let payer_signer = LocalSigner::generate("payer");
    let state = carriage(payee(&payer_signer), bound_policy());
    let dialer = LoopbackDialer::new(state);
    let mut transport = BtpPeerTransport::new(
        Arc::clone(&dialer) as Arc<dyn PeerDialer>,
        derive_evm_address(&payer_signer.public_key().unwrap()),
        clock() as Arc<dyn Clock>,
    );
    transport.add_peers_from_config(config.peers(), config.peer_channels());
    transport.set_solana_signer_public_key([0x77; 32]);

    let _ = transport
        .flush(
            PEER_ID,
            WireClaim {
                channel_id: solana_channel_account(),
                nonce: 1,
                cumulative_amount: 500,
                signature: ClaimSignature::Solana([0x5a; 64]),
            },
        )
        .await;

    let sent = dialer.sent.lock().expect("sent frames lock").clone();
    let entry = sent
        .iter()
        .find(|frame| frame.frame_type == BTP_TRANSFER)
        .expect("the flush rode a TRANSFER")
        .protocol_data
        .iter()
        .find(|pd| pd.name == CLAIM_PROTOCOL)
        .expect("the claim rode as protocolData")
        .clone();
    let claim_json: serde_json::Value =
        serde_json::from_slice(&entry.data).expect("raw UTF-8 JSON");
    assert_eq!(
        claim_json["programId"], SOLANA_PROGRAM_ID,
        "the declared program is `[settlement.solana] program_id` and nothing else (#1128)"
    );
}

// ─── §6.3: the idempotent re-ack ───

/// §6.3, the rule that stands between a lost ack and a permanently wedged
/// peering: a byte-identical retransmission of the claim already at the
/// watermark is answered **`accepted`**, never `nonce_not_advancing`, and
/// nothing is advanced or recorded.
#[tokio::test]
async fn a_byte_identical_retransmission_at_the_watermark_is_accepted_again() {
    let payer_signer = LocalSigner::generate("payer");
    let state = carriage(payee(&payer_signer), bound_policy());
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);
    let claim = sign_claim(&payer_signer, 3, 900);

    let first = transport.flush(PEER_ID, claim.clone()).await;
    let retransmitted = transport.flush(PEER_ID, claim.clone()).await;

    assert_eq!(first, ClaimAckOutcome::Accepted);
    assert_eq!(
        retransmitted,
        ClaimAckOutcome::Accepted,
        "a lost ack must not wedge the peering"
    );

    // And the retransmission really was byte-identical: the payer reused
    // the exact JSON it emitted, timestamp included (§6.3).
    let sent = dialer.sent.lock().expect("sent frames lock").clone();
    let claims: Vec<&ProtocolData> = sent
        .iter()
        .filter(|frame| frame.frame_type == BTP_TRANSFER)
        .filter_map(|frame| {
            frame
                .protocol_data
                .iter()
                .find(|pd| pd.name == CLAIM_PROTOCOL)
        })
        .collect();
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[0].data, claims[1].data);
}

/// §6.3's other half: the *same nonce* with any other field changed is a
/// different claim, refused `nonce_not_advancing` exactly as §3.2's
/// strictly-advancing rule requires. Together with the test above this
/// pins the whole boundary.
#[tokio::test]
async fn the_same_nonce_with_different_bytes_is_refused_nonce_not_advancing() {
    let payer_signer = LocalSigner::generate("payer");
    let state = carriage(payee(&payer_signer), bound_policy());
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);

    let accepted = transport
        .flush(PEER_ID, sign_claim(&payer_signer, 3, 900))
        .await;
    let same_nonce_more_money = transport
        .flush(PEER_ID, sign_claim(&payer_signer, 3, 1_500))
        .await;

    assert_eq!(accepted, ClaimAckOutcome::Accepted);
    assert_eq!(
        same_nonce_more_money,
        ClaimAckOutcome::Rejected(ClaimRejectReason::NonceNotAdvancing)
    );
}

// ─── §6.3: absence, malformation and the timeout ───

/// A payee that answers the claim-bearing frame but carries **no**
/// `claim-ack`. §6.3: not acknowledged -- never accepted, never rejected,
/// never inferred from the packet's own verdict.
#[tokio::test]
async fn a_response_carrying_no_ack_leaves_the_claim_not_acknowledged() {
    let payer_signer = LocalSigner::generate("payer");
    let transport = transport(
        Arc::new(SilentPayee { ack: None }) as Arc<dyn PeerDialer>,
        &payer_signer,
    );

    let ack = transport
        .flush(PEER_ID, sign_claim(&payer_signer, 1, 500))
        .await;

    assert_eq!(ack, ClaimAckOutcome::NotSent);
}

/// §6.3: a malformed ack is likewise not acknowledged, and must not be
/// read as either verdict.
#[tokio::test]
async fn a_malformed_ack_leaves_the_claim_not_acknowledged() {
    let payer_signer = LocalSigner::generate("payer");
    let transport = transport(
        Arc::new(SilentPayee {
            ack: Some(br#"{"result":"probably"}"#.to_vec()),
        }) as Arc<dyn PeerDialer>,
        &payer_signer,
    );

    let ack = transport
        .flush(PEER_ID, sign_claim(&payer_signer, 1, 500))
        .await;

    assert_eq!(ack, ClaimAckOutcome::NotSent);
}

/// A payee that answers every request with an empty RESPONSE, optionally
/// carrying `ack` bytes verbatim -- for the absence and malformation cases
/// a well-behaved `PeerSession` will not produce.
struct SilentPayee {
    ack: Option<Vec<u8>>,
}

#[async_trait]
impl PeerDialer for SilentPayee {
    async fn dial(&self, _peer_id: &str, _endpoint: &Url) -> Result<BtpSessionHandle, DialError> {
        let (to_peer, mut to_peer_rx) = mpsc::channel::<Vec<u8>>(32);
        let outbound = Arc::new(OutboundRequests::new());
        let handle = BtpSessionHandle::new(to_peer, Arc::clone(&outbound));
        let ack = self.ack.clone();
        tokio::spawn(async move {
            while let Some(bytes) = to_peer_rx.recv().await {
                let frame = decode_frame(&bytes).expect("our own encoder");
                let entries: Vec<ProtocolData> = ack
                    .iter()
                    .map(|data| ProtocolData {
                        name: connector_btp::CLAIM_ACK_PROTOCOL.to_string(),
                        content_type: CONTENT_TYPE_TEXT,
                        data: data.clone(),
                    })
                    .collect();
                let answer = encode_response(frame.request_id, &entries, &[]);
                let _ = outbound.resolve(decode_frame(&answer).expect("our own encoder"));
            }
        });
        Ok(handle)
    }
}

// ─── §2.2: a peer that cannot be dialed ───

/// §2.2: whether the remote exposes what we dial is not locally
/// detectable, so it surfaces as an ordinary dial failure -- and a packet
/// routed there rejects **`T01`**, never `T00` and never a silent drop.
/// `reached` is false, so no fee of this hop's belongs on the reject that
/// goes back (ADR 0011).
#[tokio::test]
async fn a_peer_that_cannot_be_dialed_rejects_t01_and_was_never_reached() {
    let payer_signer = LocalSigner::generate("payer");
    let transport = transport(Arc::new(DeadDialer) as Arc<dyn PeerDialer>, &payer_signer);

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport
        .forward(PEER_ID, prepare("g.somewhere"), None)
        .await;

    match response {
        PacketResponse::Reject(reject) => {
            assert_eq!(reject.code.as_str(), "T01");
            assert!(reject.message.contains(PEER_ID));
        }
        other => panic!("expected T01, got {other:?}"),
    }
    assert_eq!(ack, ClaimAckOutcome::NotSent);
    assert!(!reached);
}

#[tokio::test]
async fn a_peer_id_this_connector_does_not_dial_rejects_t01() {
    let payer_signer = LocalSigner::generate("payer");
    let transport = transport(Arc::new(DeadDialer) as Arc<dyn PeerDialer>, &payer_signer);

    let PeerForward {
        response,
        reached_peer: reached,
        ..
    } = transport
        .forward("nowhere", prepare("g.somewhere"), None)
        .await;

    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "T01"),
        other => panic!("expected T01, got {other:?}"),
    }
    assert!(!reached);
}

// ─── §1: role is decided by authentication ───

/// A session driver for the accept side alone: feed frames in, read the
/// answers out.
struct Accepting {
    frames: mpsc::Sender<Vec<u8>>,
    answers: mpsc::Receiver<Vec<u8>>,
    session: tokio::task::JoinHandle<SessionEnd>,
}

fn accepting(state: Arc<PeerCarriageState>) -> Accepting {
    let (frames, frames_rx) = mpsc::channel::<Vec<u8>>(32);
    let (replies, answers) = mpsc::channel::<Vec<u8>>(32);
    let session = tokio::spawn(PeerSession::new(state, replies).run(frames_rx));
    Accepting {
        frames,
        answers,
        session,
    }
}

impl Accepting {
    async fn send(&self, frame: Vec<u8>) {
        self.frames.send(frame).await.expect("the session is live");
    }

    async fn answer(&mut self) -> BtpFrame {
        let bytes = self
            .answers
            .recv()
            .await
            .expect("the session answered the request");
        decode_frame(&bytes).expect("our own encoder")
    }
}

fn claim_frame(request_id: u32, claim_json: &str) -> Vec<u8> {
    encode_message(
        request_id,
        &[ProtocolData {
            name: CLAIM_PROTOCOL.to_string(),
            content_type: CONTENT_TYPE_TEXT,
            data: claim_json.as_bytes().to_vec(),
        }],
        &prepare("g.nowhere").encode(),
    )
}

fn claim_as_json(claim: &WireClaim, payer: &dyn Signer) -> String {
    connector_peer_btp::claim_json::encode(
        claim,
        &derive_evm_address(&payer.public_key().unwrap()),
        None,
        None,
        Some(connector_peer_btp::PeerClaimDomain {
            chain_id: CHAIN_ID,
            token_network: TOKEN_NETWORK,
        }),
        "message-1",
        "2030-01-01T00:00:00.000Z",
    )
}

/// **The named regression (§1.9).** `toon-sandbox` admitted an anonymous
/// BTP session with `btp_auth … success:true mode:"no-auth"` and then
/// treated it as a quasi-peer. Each of the five frames below is classified
/// `client` and reaches **no peer handling whatsoever** -- testable, per
/// §1.9, as: no `claim-ack` was emitted, and the claim they carried moved
/// no peer watermark (proved by a subsequent *genuine* peer claim at nonce
/// 1 being accepted, which it could not be if any of these had advanced
/// anything).
#[tokio::test]
async fn the_named_regression_no_frame_becomes_a_peer_without_p2_and_p3() {
    let payer_signer = LocalSigner::generate("payer");
    let stranger = LocalSigner::generate("stranger");
    // A channel this policy binds to `PEER_ID` but the connector holds no
    // verification key for: P2 failing, which is config and chain
    // disagreeing rather than a caller's doing.
    let unrecorded = format!("0x{:064x}", 11);
    // A channel bound to a peer id no `[[peers]]` entry configures. The
    // policy drops the row, so its channel binds nothing -- the runtime
    // analogue of `PeerChannelOrphaned`.
    let orphaned = format!("0x{:064x}", 12);
    let bound = channel_id();
    let policy = Arc::new(PeerAuthPolicy::new(
        vec![PEER_ID],
        vec![
            (bound.as_str(), PEER_ID),
            (unrecorded.as_str(), PEER_ID),
            (orphaned.as_str(), "stranger"),
        ],
    ));
    let connector = payee(&payer_signer);
    let state = carriage(Arc::clone(&connector), policy);

    let asserted: Vec<Option<String>> = vec![
        // 1. no claim at all
        None,
        // 2. a claim on a channel no `[[peer_channels]]` row configures
        Some(claim_as_json(
            &sign_claim_on(&payer_signer, &format!("0x{:064x}", 13), 13, 1, 500),
            &payer_signer,
        )),
        // 3. a claim on a configured channel whose signature does not
        //    recover to the counterparty key that row configures (P3)
        Some(claim_as_json(&sign_claim(&stranger, 1, 500), &stranger)),
        // 4. a claim on a bound channel this node holds no record of (P2)
        Some(claim_as_json(
            &sign_claim_on(&payer_signer, &unrecorded, 11, 1, 500),
            &payer_signer,
        )),
        // 5. a claim on a channel whose row names an unconfigured peer id
        Some(claim_as_json(
            &sign_claim_on(&payer_signer, &orphaned, 12, 1, 500),
            &payer_signer,
        )),
    ];

    for (index, claim_json) in asserted.into_iter().enumerate() {
        let mut session = accepting(Arc::clone(&state));
        let frame = match claim_json.as_deref() {
            Some(json) => claim_frame(1, json),
            None => encode_message(1, &[], &prepare("g.nowhere").encode()),
        };
        session.send(frame).await;
        let answer = session.answer().await;

        // §1.6: not refused for the assertion alone -- refusing would make
        // the check an oracle for which channels are configured.
        assert_eq!(
            answer.frame_type, BTP_RESPONSE,
            "case {index}: an asserted role is not refused on the wire (§1.6)"
        );
        assert!(
            ack::from_protocol_data(&answer.protocol_data).is_none(),
            "case {index}: a client frame gets no claim-ack (§1.7)"
        );
    }

    // Nothing above moved a peer watermark: a genuine peer's claim at
    // nonce 1 is still fresh.
    let genuine = claim_as_json(&sign_claim(&payer_signer, 1, 500), &payer_signer);
    let mut peer = accepting(Arc::clone(&state));
    peer.send(claim_frame(2, &genuine)).await;
    let answer = peer.answer().await;
    assert_eq!(
        ack::from_protocol_data(&answer.protocol_data),
        Some(ClaimAckOutcome::Accepted),
        "no client frame had advanced this channel's peer watermark"
    );
}

/// §1.4: a receiver **ignores** an arriving `auth` entry rather than
/// refusing it (ADR 0060), so the two ends of a peering may be upgraded in
/// either order without the peering going dark mid-flight. The frame is
/// answered the way any claimless MESSAGE is -- an empty RESPONSE -- and
/// nothing about the entry is evaluated.
#[tokio::test]
async fn an_arriving_auth_entry_is_ignored_rather_than_refused() {
    let payer_signer = LocalSigner::generate("payer");
    let state = carriage(payee(&payer_signer), bound_policy());
    let mut session = accepting(Arc::clone(&state));

    session
        .send(encode_message(
            1,
            &[ProtocolData {
                name: AUTH_PROTOCOL.to_string(),
                content_type: CONTENT_TYPE_TEXT,
                data: br#"{"peerId":"peer-b","secret":"whatever"}"#.to_vec(),
            }],
            &[],
        ))
        .await;
    let answer = session.answer().await;

    assert_eq!(answer.frame_type, BTP_RESPONSE);
    assert!(answer.protocol_data.is_empty());
    // And it decided nothing: the very next claim still stands on itself,
    // and a claim signed by a stranger is still a client frame.
    let stranger = LocalSigner::generate("stranger");
    let forged = claim_as_json(&sign_claim(&stranger, 1, 500), &stranger);
    session.send(claim_frame(2, &forged)).await;
    let answer = session.answer().await;
    assert!(ack::from_protocol_data(&answer.protocol_data).is_none());
}

/// §1.5: more than one claim entry on one frame is **refused, not
/// resolved** -- never the first, never the last, never a concatenation.
/// This is the smuggling defence, and its absence is how "which claim did
/// we verify?" becomes unanswerable. It guarded the `auth` entry until ADR
/// 0060 deleted it; the defect it names is a property of duplicated
/// authentication material rather than of the credential in particular.
#[tokio::test]
async fn two_claim_entries_on_one_frame_are_refused_rather_than_resolved() {
    let payer_signer = LocalSigner::generate("payer");
    let state = carriage(payee(&payer_signer), bound_policy());
    let mut session = accepting(Arc::clone(&state));
    let first = claim_as_json(&sign_claim(&payer_signer, 1, 500), &payer_signer);
    let second = claim_as_json(&sign_claim(&payer_signer, 2, 600), &payer_signer);
    let entry = |json: &str| ProtocolData {
        name: CLAIM_PROTOCOL.to_string(),
        content_type: CONTENT_TYPE_TEXT,
        data: json.as_bytes().to_vec(),
    };

    session
        .send(encode_message(1, &[entry(&first), entry(&second)], &[]))
        .await;
    let answer = session.answer().await;

    assert_eq!(answer.frame_type, BTP_ERROR);
    // Neither claim was adopted: nonce 1 is still fresh, so nothing was
    // resolved behind the refusal.
    session.send(claim_frame(2, &first)).await;
    let answer = session.answer().await;
    assert_eq!(
        ack::from_protocol_data(&answer.protocol_data),
        Some(ClaimAckOutcome::Accepted)
    );
}

/// §1.5: role is a property of the **frame**, not of the session. A frame
/// whose claim does not verify is a client frame, and a verifying frame
/// after it does not reach back and reclassify it -- nor does the verifying
/// frame inherit anything from the one before.
#[tokio::test]
async fn a_frame_whose_claim_does_not_verify_stays_a_client_frame() {
    let payer_signer = LocalSigner::generate("payer");
    let stranger = LocalSigner::generate("stranger");
    let state = carriage(payee(&payer_signer), bound_policy());
    let mut session = accepting(Arc::clone(&state));
    let forged = claim_as_json(&sign_claim(&stranger, 1, 500), &stranger);
    let genuine = claim_as_json(&sign_claim(&payer_signer, 1, 500), &payer_signer);

    session.send(claim_frame(1, &forged)).await;
    let before = session.answer().await;
    assert!(
        ack::from_protocol_data(&before.protocol_data).is_none(),
        "a frame whose claim does not verify is a client frame"
    );

    // The forged claim was never judged, so nonce 1 is still fresh -- which
    // is exactly what "not retroactively reclassified" means here.
    session.send(claim_frame(2, &genuine)).await;
    let after = session.answer().await;
    assert_eq!(
        ack::from_protocol_data(&after.protocol_data),
        Some(ClaimAckOutcome::Accepted)
    );

    // And the peer frame does not make the socket a peer socket: the very
    // next frame carrying no claim is a client frame again.
    session
        .send(encode_message(3, &[], &prepare("g.nowhere").encode()))
        .await;
    let claimless = session.answer().await;
    assert!(ack::from_protocol_data(&claimless.protocol_data).is_none());
}

/// §1.10's bounded escape hatch: on a **dedicated peer listener with
/// mandatory authentication** a frame that fails P2 or P3 is refused
/// outright rather than downgraded -- safe only because such a listener
/// serves no clients. Role is still decided by P2 and P3; the listener
/// never becomes the decider.
#[tokio::test]
async fn a_dedicated_peer_listener_refuses_rather_than_downgrades() {
    let payer_signer = LocalSigner::generate("payer");
    let stranger = LocalSigner::generate("stranger");
    let state = Arc::new(PeerCarriageState::new(
        payee(&payer_signer),
        bound_policy(),
        Arc::new(AcceptedClaims::new()),
        Arc::new(ClaimEnforcementPolicy::default()),
        PeerAcceptPolicy {
            mandatory_auth: true,
            ..PeerAcceptPolicy::default()
        },
    ));
    let mut session = accepting(state);
    let forged = claim_as_json(&sign_claim(&stranger, 1, 500), &stranger);

    session.send(claim_frame(1, &forged)).await;
    let answer = session.answer().await;

    assert_eq!(answer.frame_type, BTP_ERROR);
    assert_eq!(
        session.session.await.expect("the session task"),
        SessionEnd::Refused
    );
}

// ─── issue #880 (owner decision #868): every peer PREPARE to a priced
// terminated route carries a covering claim, or is refused with the client
// edge's own x402 greeting ───

/// A claimless arrival reaches **no peer handling at all** (§1.2, ADR
/// 0060). It used to reach this gate: a credential made a session a peering
/// without a claim, and the gate then refused it `F06` with the x402
/// greeting. With role read from the claim there is no such state -- a
/// frame carrying none is a client frame, and the greeting a client gets is
/// the client edge's own, covered by `connector-client-edge`'s
/// `a_claimless_prepare_to_a_priced_route_is_refused_with_the_terms` (BTP)
/// and its `POST /ilp` `402` unit test (HTTP).
///
/// What survives here is the property that matters at this layer: the app
/// never sees it, and nothing is acknowledged.
#[tokio::test]
async fn a_claimless_arrival_at_a_priced_route_reaches_no_peer_handling() {
    let payer_signer = LocalSigner::generate("payer");
    let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
    let state = carriage(payee_with_route(&payer_signer, route), bound_policy());
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);

    let PeerForward {
        response,
        ack,
        payment_required,
        ..
    } = transport
        .forward(PEER_ID, prepare("g.example.app"), None)
        .await;

    match response {
        PacketResponse::Reject(reject) => assert_eq!(
            reject.code.as_str(),
            "F02",
            "a client-role packet reaches no peer route"
        ),
        other => panic!("expected an F02 reject, got {other:?}"),
    }
    assert!(payment_required.is_none());
    assert_eq!(
        ack,
        ClaimAckOutcome::NotSent,
        "no claim-ack to a client (§1.7)"
    );
}

/// A claim rides the PREPARE, but its own advance over the watermark falls
/// short of the route's price: refused exactly the same way as no claim at
/// all (issue #880's second acceptance case). The claim's own nonce/amount
/// validity is unaffected by this gate -- it is still acknowledged (§6.2).
#[tokio::test]
async fn a_claim_that_does_not_cover_the_routes_price_is_refused_the_same_way() {
    let payer_signer = LocalSigner::generate("payer");
    let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
    let state = carriage(payee_with_route(&payer_signer, route), bound_policy());
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);
    let claim = sign_claim(&payer_signer, 1, 10); // advances only 10, price is 25

    let PeerForward {
        response,
        ack,
        payment_required,
        ..
    } = transport
        .forward(PEER_ID, prepare("g.example.app"), Some(claim))
        .await;

    assert_eq!(ack, ClaimAckOutcome::Accepted);
    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F06"),
        other => panic!("expected an F06 reject, got {other:?}"),
    }
    assert!(payment_required.is_some());
}

/// The boundary this gate exists to leave open: a claim whose advance
/// exactly meets the route's price is admitted precisely as it was before
/// this issue -- delivered to the app, no greeting.
#[tokio::test]
async fn a_covering_claim_is_admitted_exactly_as_today() {
    let payer_signer = LocalSigner::generate("payer");
    let identity_signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("payee-identity"));
    let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
    let app_client = Arc::new(FakeAppClient::new());
    let response_body = b"irrelevant".to_vec();
    app_client.respond(
        route.handler_url(),
        connector_runtime::AppOutcome::Answered {
            response: EnvelopeResponse {
                status: 200,
                headers: vec![],
                body: response_body.clone(),
            },
        },
    );
    let counterparty = derive_evm_address(&payer_signer.public_key().unwrap());
    let connector = Arc::new(
        Connector::new(
            vec![route],
            vec![],
            app_client.clone(),
            Arc::new(InProcessPeerTransport::new()),
            clock(),
        )
        .with_channel_verification_key(channel_id(), counterparty)
        .with_channel_domain(channel_id(), domain())
        .expect("a bytes32 channel id")
        .with_identity_signer(Arc::clone(&identity_signer)),
    );
    let state = carriage(connector, bound_policy());
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);
    let claim = sign_claim(&payer_signer, 1, 25); // advances exactly the price

    // A genuinely sealed envelope (ADR 0018/0019), so this termination can
    // actually fulfil rather than being refused for want of sealing --
    // orthogonal to this gate, but needed to prove delivery reached the app.
    let envelope = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: b"hello".to_vec(),
    };
    let identity_public = identity_signer.public_key().expect("identity public key");
    let (data, shared_secret) =
        connector_signer::giftwrap::seal_request(&envelope.encode(), &identity_public)
            .expect("seal");
    let sealed_prepare = Prepare {
        amount: 25,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: "g.example.app".to_string(),
        data,
    };

    let PeerForward {
        response,
        ack,
        payment_required,
        ..
    } = transport
        .forward(PEER_ID, sealed_prepare, Some(claim))
        .await;

    assert_eq!(ack, ClaimAckOutcome::Accepted);
    assert!(
        payment_required.is_none(),
        "an admitted packet carries no greeting"
    );
    let fulfill = match response {
        PacketResponse::Fulfill(fulfill) => fulfill,
        other => panic!("expected a fulfil, got {other:?}"),
    };
    let opened = connector_signer::giftwrap::open_response(&shared_secret, &fulfill.data)
        .expect("open the sealed fulfil");
    let opened = EnvelopeResponse::decode(&opened).expect("decode envelope");
    assert_eq!(opened.body, response_body);
    assert_eq!(app_client.deliveries().len(), 1);
}

/// PR #913 review finding: a claim signed by a non-counterparty key still
/// *decodes* and can declare any `cumulative_amount` it likes -- coverage
/// judged off that declared amount, ignoring the claim book's own verdict,
/// let an unlimited-value, never-verified claim buy service.
///
/// Since ADR 0060 that claim fails P3, so the frame is a **client** frame
/// and there is no `claim-ack` to carry `signature_invalid` back: §1.7
/// forbids one on a client interaction, and the sender learns from the
/// greeting the client edge gives it, not from an ack. What this test still
/// pins is the finding itself -- the declared amount buys nothing, and the
/// app never sees the packet.
#[tokio::test]
async fn a_forged_claim_declaring_a_large_amount_does_not_buy_coverage() {
    let payer_signer = LocalSigner::generate("payer");
    let impostor = LocalSigner::generate("impostor");
    let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
    let app_client = Arc::new(FakeAppClient::new());
    let counterparty = derive_evm_address(&payer_signer.public_key().unwrap());
    let connector = Arc::new(
        Connector::new(
            vec![route],
            vec![],
            app_client.clone(),
            Arc::new(InProcessPeerTransport::new()),
            clock(),
        )
        .with_channel_verification_key(channel_id(), counterparty)
        .with_channel_domain(channel_id(), domain())
        .expect("a bytes32 channel id"),
    );
    let state = carriage(connector, bound_policy());
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);
    // Signed by an impostor, not the channel's configured counterparty --
    // declares far more than the route's price, but never verifies.
    let claim = sign_claim(&impostor, 1, 1_000_000);

    let PeerForward {
        response,
        ack,
        payment_required,
        ..
    } = transport
        .forward(PEER_ID, prepare("g.example.app"), Some(claim))
        .await;

    assert_eq!(
        ack,
        ClaimAckOutcome::NotSent,
        "a claim that fails P3 makes the frame a client's, and a client \
         interaction never carries a claim-ack (§1.7)"
    );
    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F02"),
        other => panic!("expected an F02 reject, got {other:?}"),
    }
    assert!(
        payment_required.is_none(),
        "the greeting a client gets is the client edge's, not this gate's"
    );
    assert!(
        app_client.deliveries().is_empty(),
        "a forged claim must never reach the app"
    );
}

/// PR #913 review finding, second case: a genuinely signed claim replayed
/// at an already-used nonce also decodes and can declare any amount, and
/// its verdict is `nonce_not_advancing` -- not `signature_invalid`, but
/// equally not `accepted`. The refusal must repeat on every retransmission
/// and the watermark must never move off the last genuinely accepted
/// claim.
#[tokio::test]
async fn a_claim_replayed_at_a_used_nonce_never_buys_coverage() {
    let payer_signer = LocalSigner::generate("payer");
    let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
    let app_client = Arc::new(FakeAppClient::new());
    let counterparty = derive_evm_address(&payer_signer.public_key().unwrap());
    let connector = Arc::new(
        Connector::new(
            vec![route],
            vec![],
            app_client.clone(),
            Arc::new(InProcessPeerTransport::new()),
            clock(),
        )
        .with_channel_verification_key(channel_id(), counterparty)
        .with_channel_domain(channel_id(), domain())
        .expect("a bytes32 channel id"),
    );
    let accepted = Arc::new(AcceptedClaims::new());
    let state = Arc::new(PeerCarriageState::new(
        connector,
        bound_policy(),
        Arc::clone(&accepted),
        Arc::new(ClaimEnforcementPolicy::default()),
        PeerAcceptPolicy::default(),
    ));
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);

    // A first claim at nonce 1 genuinely covers the price and advances the
    // watermark to 25.
    let first = sign_claim(&payer_signer, 1, 25);
    let first_ack = transport.flush(PEER_ID, first).await;
    assert_eq!(first_ack, ClaimAckOutcome::Accepted);

    // Replayed at the same nonce, declaring an amount that would cover many
    // more packets if it were judged by amount alone.
    let replayed = sign_claim(&payer_signer, 1, 1_000_000);

    for attempt in 0..2 {
        let PeerForward {
            response,
            ack,
            payment_required,
            ..
        } = transport
            .forward(PEER_ID, prepare("g.example.app"), Some(replayed.clone()))
            .await;

        assert_eq!(
            ack,
            ClaimAckOutcome::Rejected(ClaimRejectReason::NonceNotAdvancing),
            "attempt {attempt}"
        );
        match response {
            PacketResponse::Reject(reject) => {
                assert_eq!(reject.code.as_str(), "F06", "attempt {attempt}")
            }
            other => panic!("expected an F06 reject on attempt {attempt}, got {other:?}"),
        }
        assert!(payment_required.is_some(), "attempt {attempt}");
    }
    assert!(
        app_client.deliveries().is_empty(),
        "the replayed claim must never reach the app"
    );
    let watermark = accepted
        .watermark(PEER_ID, &channel_id())
        .expect("a watermark was recorded by the first, genuine claim");
    assert_eq!(
        watermark.cumulative_amount, 25,
        "the replayed claim's declared amount must never advance the watermark"
    );
}

// ─── issue #1104: coverage is the claim's advance past the **durable**
// watermark, so a payee restart never credits a claim with its whole
// cumulative amount ───

/// A PREPARE genuinely sealed to [`payee_identity`] (ADR 0018/0019), with
/// the shared secret its answer can be opened with. Orthogonal to the price
/// gate, but what lets an admitted packet actually fulfil rather than be
/// refused for want of sealing -- so "admitted" can be proved by the app
/// having been reached. [`sealed_prepare_to`] at this node's own priced
/// termination.
fn sealed_prepare(amount: u64) -> (Prepare, [u8; 32]) {
    sealed_prepare_to(payee_identity().as_ref(), "g.example.app", amount)
}

/// An app that actually answers `route`'s handler, so a packet the gate
/// admits visibly **fulfils**: without the fix, issue #1104's packet is
/// served for free rather than merely getting past one check.
fn serving_app(route: &StaticRoute, body: &[u8]) -> Arc<FakeAppClient> {
    let app_client = Arc::new(FakeAppClient::new());
    app_client.respond(
        route.handler_url(),
        connector_runtime::AppOutcome::Answered {
            response: EnvelopeResponse {
                status: 200,
                headers: vec![],
                body: body.to_vec(),
            },
        },
    );
    app_client
}

/// Carries this channel to cumulative 50 000 on a payee journaling to
/// `journal`, then drops that whole node -- the state a restarted payee
/// replays from.
async fn journal_at_fifty_thousand(payer: &LocalSigner, journal: Arc<dyn Journal>) {
    let state = carriage(
        payee_journaling_to(
            payer,
            priced_route(),
            Arc::new(FakeAppClient::new()),
            journal,
        ),
        bound_policy(),
    );
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, payer);
    assert_eq!(
        transport.flush(PEER_ID, sign_claim(payer, 1, 50_000)).await,
        ClaimAckOutcome::Accepted,
        "the pre-restart claim is what the journal records"
    );
}

/// The bug: after a restart `ClaimBook` has replayed its journal and is at
/// cumulative 50 000, while `AcceptedClaims` -- in-memory and per-process
/// -- is empty. A claim at cumulative 50 001 is one unit of genuinely new
/// money and cannot buy a packet priced at 25. Measured against the empty
/// per-process record it would be credited with all 50 001 and buy it
/// (issue #1104).
#[tokio::test]
async fn a_restart_does_not_credit_a_claim_with_the_amount_it_already_paid() {
    let payer_signer = LocalSigner::generate("payer");
    let journal: Arc<dyn Journal> = Arc::new(InMemoryJournal::new());
    journal_at_fifty_thousand(&payer_signer, Arc::clone(&journal)).await;

    // The restart: a second node over the same journal and nothing else.
    let route = priced_route();
    let app_client = serving_app(&route, b"free service");
    let state = carriage(
        payee_journaling_to(
            &payer_signer,
            route,
            Arc::clone(&app_client),
            Arc::clone(&journal),
        ),
        bound_policy(),
    );
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);

    let claim = sign_claim(&payer_signer, 2, 50_001); // advances 1, the price is 25
    let (prepare, _) = sealed_prepare(25);
    let PeerForward {
        response,
        ack,
        payment_required,
        ..
    } = transport.forward(PEER_ID, prepare, Some(claim)).await;

    assert_eq!(
        ack,
        ClaimAckOutcome::Accepted,
        "the claim itself is good -- its nonce and amount both advance the durable watermark, \
         which is why the book's verdict cannot catch this on its own"
    );
    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F06"),
        other => panic!("expected an F06 reject, got {other:?} -- the app served this for free"),
    }
    assert!(payment_required.is_some(), "the x402 greeting rides it");
    assert!(
        app_client.deliveries().is_empty(),
        "one unit of new money must not buy a packet priced at 25"
    );
}

/// The other side of the same boundary: after the same restart, a claim
/// that genuinely advances the durable watermark by the price is admitted
/// and reaches the app. The fix must not make a restart refuse real money.
#[tokio::test]
async fn a_restart_still_admits_a_claim_that_genuinely_advances_by_the_price() {
    let payer_signer = LocalSigner::generate("payer");
    let journal: Arc<dyn Journal> = Arc::new(InMemoryJournal::new());
    journal_at_fifty_thousand(&payer_signer, Arc::clone(&journal)).await;

    let route = priced_route();
    let response_body = b"served after the restart".to_vec();
    let app_client = serving_app(&route, &response_body);
    let state = carriage(
        payee_journaling_to(
            &payer_signer,
            route,
            Arc::clone(&app_client),
            Arc::clone(&journal),
        ),
        bound_policy(),
    );
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);

    let claim = sign_claim(&payer_signer, 2, 50_025); // advances exactly the price
    let (prepare, shared_secret) = sealed_prepare(25);
    let PeerForward {
        response,
        ack,
        payment_required,
        ..
    } = transport.forward(PEER_ID, prepare, Some(claim)).await;

    assert_eq!(ack, ClaimAckOutcome::Accepted);
    assert!(
        payment_required.is_none(),
        "an admitted packet carries no greeting"
    );
    let fulfill = match response {
        PacketResponse::Fulfill(fulfill) => fulfill,
        other => panic!("expected a fulfil, got {other:?}"),
    };
    let opened = connector_signer::giftwrap::open_response(&shared_secret, &fulfill.data)
        .expect("open the sealed fulfil");
    let opened = EnvelopeResponse::decode(&opened).expect("decode envelope");
    assert_eq!(opened.body, response_body);
    assert_eq!(app_client.deliveries().len(), 1);
}

// ─── ADR 0042 item 3: a forwarded arrival must cover its own `amount`,
// behind a per-peer knob that defaults to observing ───

/// **The default is still `observe`, and this is what that means.** A
/// peering that configures nothing carries an **under-covered** forwarded
/// arrival -- admitted, logged, and actually forwarded to the next hop. It
/// was written as a fleet-safety guard when both devnet boxes forwarded to
/// each other; issue #872 removed both peerings, so what it guards now is
/// the migration default itself, which ADR 0042 item 3 keeps for a
/// counterparty on an older binary. Note what this fixture must ALSO have
/// since issue #1145: its own `[[pay_channels]]` row (`covering`).
/// Admitting an arrival for free says nothing about what this node then
/// sends, and what it sends is covered unconditionally.
///
/// The arrival carries a claim that verifies but advances too little,
/// rather than no claim at all: since ADR 0060 a claimless one is a
/// client's and never reaches this gate. `observe` still selects between
/// admitting and refusing everything else it always did.
#[tokio::test]
async fn a_forwarded_arrival_that_undercovers_is_admitted_by_default() {
    let payer_signer = LocalSigner::generate("payer");
    let (connector, next_hop_app, next_hop_identity) = forwarding_payee(&payer_signer);
    // The default policy: no entry for this peering at all, exactly as an
    // unconfigured `[[peers]]` row resolves.
    let state = carriage(connector, bound_policy());
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);
    let (sealed, shared_secret) = sealed_prepare_to(
        next_hop_identity.as_ref(),
        FORWARDED_DESTINATION,
        ARRIVING_AMOUNT,
    );

    let short = sign_claim(&payer_signer, 1, ARRIVING_AMOUNT - 1);

    let PeerForward {
        response,
        payment_required,
        ..
    } = transport.forward(PEER_ID, sealed, Some(short)).await;

    assert!(
        payment_required.is_none(),
        "an admitted packet carries no greeting"
    );
    let fulfill = match response {
        PacketResponse::Fulfill(fulfill) => fulfill,
        other => panic!("expected a fulfil from the next hop, got {other:?}"),
    };
    let opened = connector_signer::giftwrap::open_response(&shared_secret, &fulfill.data)
        .expect("open the sealed fulfil");
    let opened = EnvelopeResponse::decode(&opened).expect("decode envelope");
    assert_eq!(opened.body, b"delivered by the next hop");
    assert_eq!(
        next_hop_app.deliveries().len(),
        1,
        "the packet was really carried, not merely not refused"
    );
}

/// The same arrival on a peering an operator has flipped: refused `F06`
/// with the x402 greeting, quoting the packet's own `amount` -- and never
/// carried, so the next hop does no work this connector was not paid for.
#[tokio::test]
async fn a_forwarded_arrival_that_undercovers_is_refused_once_this_peering_enforces() {
    let payer_signer = LocalSigner::generate("payer");
    let (connector, next_hop_app, next_hop_identity) = forwarding_payee(&payer_signer);
    let state = carriage_with_enforcement(connector, bound_policy(), forwarded_enforcing());
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);
    let (sealed, _) = sealed_prepare_to(
        next_hop_identity.as_ref(),
        FORWARDED_DESTINATION,
        ARRIVING_AMOUNT,
    );

    let short = sign_claim(&payer_signer, 1, ARRIVING_AMOUNT - 1);

    let PeerForward {
        response,
        payment_required,
        ..
    } = transport.forward(PEER_ID, sealed, Some(short)).await;

    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F06"),
        other => panic!("expected an F06 reject, got {other:?}"),
    }
    let terms = payment_required.expect("the x402 greeting rode the reject");
    assert_eq!(
        terms.price(),
        Some(ARRIVING_AMOUNT),
        "a forwarded arrival is quoted the packet's own amount, not the route's price"
    );
    assert_eq!(terms.pay_to(), Some(FORWARDED_DESTINATION));
    assert!(
        next_hop_app.deliveries().is_empty(),
        "a refused arrival is never carried"
    );
}

/// A claim advancing the full arriving `amount` is admitted under **either**
/// setting: enforcing changes what an uncovered packet gets, never what a
/// covered one gets.
#[tokio::test]
async fn a_claim_covering_the_arriving_amount_is_admitted_under_either_setting() {
    for enforcement in [
        Arc::new(ClaimEnforcementPolicy::default()),
        forwarded_enforcing(),
    ] {
        let payer_signer = LocalSigner::generate("payer");
        let (connector, next_hop_app, next_hop_identity) = forwarding_payee(&payer_signer);
        let state = carriage_with_enforcement(connector, bound_policy(), enforcement);
        let dialer = LoopbackDialer::new(state);
        let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);
        let (sealed, shared_secret) = sealed_prepare_to(
            next_hop_identity.as_ref(),
            FORWARDED_DESTINATION,
            ARRIVING_AMOUNT,
        );
        let claim = sign_claim(&payer_signer, 1, ARRIVING_AMOUNT);

        let PeerForward {
            response,
            ack,
            payment_required,
            ..
        } = transport.forward(PEER_ID, sealed, Some(claim)).await;

        assert_eq!(ack, ClaimAckOutcome::Accepted);
        assert!(
            payment_required.is_none(),
            "an admitted packet carries no greeting"
        );
        let fulfill = match response {
            PacketResponse::Fulfill(fulfill) => fulfill,
            other => panic!("expected a fulfil from the next hop, got {other:?}"),
        };
        let opened = connector_signer::giftwrap::open_response(&shared_secret, &fulfill.data)
            .expect("open the sealed fulfil");
        let opened = EnvelopeResponse::decode(&opened).expect("decode envelope");
        assert_eq!(opened.body, b"delivered by the next hop");
        assert_eq!(next_hop_app.deliveries().len(), 1);
    }
}

/// **Which figure must be covered**, stated as the three near misses: not
/// the forwarded route's client-edge `price` (ADR 0028 says that is a fact
/// about this node's *client* edge), not the post-fee amount this hop passes
/// on (that is what this hop covers to the next hop, and the difference it
/// keeps is its fee, ADR 0010), and not one unit short. Only the arriving
/// `amount` covers an arriving packet.
#[tokio::test]
async fn a_claim_advancing_less_than_the_arriving_amount_never_covers_it() {
    for advance in [
        FORWARD_ROUTE_PRICE,
        ARRIVING_AMOUNT - FORWARD_FEE,
        ARRIVING_AMOUNT - 1,
    ] {
        let payer_signer = LocalSigner::generate("payer");
        let (connector, next_hop_app, next_hop_identity) = forwarding_payee(&payer_signer);
        let state = carriage_with_enforcement(connector, bound_policy(), forwarded_enforcing());
        let dialer = LoopbackDialer::new(state);
        let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);
        let (sealed, _) = sealed_prepare_to(
            next_hop_identity.as_ref(),
            FORWARDED_DESTINATION,
            ARRIVING_AMOUNT,
        );
        let claim = sign_claim(&payer_signer, 1, advance);

        let PeerForward {
            response,
            ack,
            payment_required,
            ..
        } = transport.forward(PEER_ID, sealed, Some(claim)).await;

        // The claim is perfectly valid and is still acknowledged: the two
        // verdicts stay independent (§6.2).
        assert_eq!(ack, ClaimAckOutcome::Accepted, "advance {advance}");
        match response {
            PacketResponse::Reject(reject) => {
                assert_eq!(reject.code.as_str(), "F06", "advance {advance}");
            }
            other => panic!("expected an F06 reject for advance {advance}, got {other:?}"),
        }
        assert!(payment_required.is_some(), "advance {advance}");
        assert!(
            next_hop_app.deliveries().is_empty(),
            "advance {advance} was never carried"
        );
    }
}

/// ADR 0029's rule is **untouched** by ADR 0042, and since issue #1077 it
/// has no escape hatch at all: an arrival at a priced termination that does
/// not cover the route's price is refused under **every** setting a peering
/// can carry. The forwarded knob is the only one left, and neither of its
/// values admits one -- it is the *forwarded* rule's knob and selects
/// nothing here.
///
/// The arrival carries an under-covering claim rather than none: since ADR
/// 0060 a claimless one is a client's and never reaches this gate at all.
#[tokio::test]
async fn no_peering_setting_admits_an_uncovered_arrival_at_a_priced_termination() {
    for forwarded in [
        connector_config::ForwardedClaimEnforcement::Observe,
        connector_config::ForwardedClaimEnforcement::Enforce,
    ] {
        let payer_signer = LocalSigner::generate("payer");
        let identity: Arc<dyn Signer> = Arc::new(LocalSigner::generate("payee-identity"));
        let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(
            route.handler_url(),
            connector_runtime::AppOutcome::Answered {
                response: EnvelopeResponse {
                    status: 200,
                    headers: vec![],
                    body: b"terminated here".to_vec(),
                },
            },
        );
        let counterparty = derive_evm_address(&payer_signer.public_key().unwrap());
        let connector = Arc::new(
            Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                clock(),
            )
            .with_channel_verification_key(channel_id(), counterparty)
            .with_channel_domain(channel_id(), domain())
            .expect("a bytes32 channel id")
            .with_identity_signer(Arc::clone(&identity)),
        );
        let state = carriage_with_enforcement(
            connector,
            bound_policy(),
            Arc::new(ClaimEnforcementPolicy::of(vec![(PEER_ID, forwarded)])),
        );
        let dialer = LoopbackDialer::new(state);
        let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);
        let (sealed, _) = sealed_prepare_to(identity.as_ref(), "g.example.app", 25);
        let short = sign_claim(&payer_signer, 1, 1); // the price is 25

        let PeerForward {
            response,
            payment_required,
            ..
        } = transport.forward(PEER_ID, sealed, Some(short)).await;

        assert!(
            matches!(&response, PacketResponse::Reject(reject) if reject.code.as_str() == "F06"),
            "forwarded_claim_enforcement = {forwarded}"
        );
        assert!(
            payment_required.is_some(),
            "forwarded_claim_enforcement = {forwarded}"
        );
    }
}

// ─── §6.1: the four reasons reach the wire ───

/// A claim signed by somebody who is not the channel's configured
/// counterparty is **not acknowledged at all**, and that is ADR 0060's
/// doing: the signature is what decides role (§1.2's P3), so a claim that
/// does not verify makes the frame a *client* frame, and §1.7 forbids a
/// `claim-ack` on a client interaction.
///
/// It used to come back `signature_invalid`, because a credential made the
/// session a peering and the claim was then judged inside it. That state no
/// longer exists, and with it `signature_invalid` and `unknown_channel`
/// become verdicts a **peer** frame can no longer reach -- structurally,
/// not by omission. Their JSON is still pinned, in `ack.rs`'s own round
/// trip and in the `ack_rejected_reasons` vectors, because the client edge
/// still reaches them.
///
/// The refusal is not silent: the peering is named to an operator through
/// §1.6's rate-limited `peer_auth_refused`, which is where a real
/// counterparty with a rotated key gets diagnosed.
#[tokio::test]
async fn a_claim_from_the_wrong_signer_is_not_acknowledged_at_all() {
    let payer_signer = LocalSigner::generate("payer");
    let impostor = LocalSigner::generate("impostor");
    let state = carriage(payee(&payer_signer), bound_policy());
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);

    let ack = transport
        .flush(PEER_ID, sign_claim(&impostor, 1, 500))
        .await;

    assert_eq!(ack, ClaimAckOutcome::NotSent);
}

/// A claim naming a channel this connector has no record of is not
/// acknowledged either, and for the same reason: it satisfies neither P2
/// nor P3, so the frame carrying it is a client frame.
#[tokio::test]
async fn a_claim_on_an_unconfigured_channel_is_not_acknowledged_at_all() {
    let payer_signer = LocalSigner::generate("payer");
    let state = carriage(payee(&payer_signer), bound_policy());
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);
    let claim = WireClaim {
        channel_id: format!("0x{:064x}", 99),
        ..sign_claim(&payer_signer, 1, 500)
    };

    let ack = transport.flush(PEER_ID, claim).await;

    assert_eq!(ack, ClaimAckOutcome::NotSent);
}

// ─── §7.1: ordering ───

/// §7.1: claims on one session are judged **strictly sequentially, in
/// arrival order**, so claims sent in order on one socket cannot race each
/// other into `nonce_not_advancing`. Sixteen advancing claims are sent
/// back to back without waiting for any answer; every one is accepted.
#[tokio::test]
async fn claims_sent_in_order_on_one_session_never_race_each_other() {
    let payer_signer = LocalSigner::generate("payer");
    let state = carriage(payee(&payer_signer), bound_policy());
    let mut session = accepting(state);

    for nonce in 1..=16u64 {
        let json = claim_as_json(
            &sign_claim(&payer_signer, nonce, nonce * 100),
            &payer_signer,
        );
        session
            .send(encode_message(
                nonce as u32 + 1,
                &[ProtocolData {
                    name: CLAIM_PROTOCOL.to_string(),
                    content_type: CONTENT_TYPE_TEXT,
                    data: json.into_bytes(),
                }],
                &[],
            ))
            .await;
    }

    for _ in 1..=16 {
        let answer = session.answer().await;
        assert_eq!(
            ack::from_protocol_data(&answer.protocol_data),
            Some(ClaimAckOutcome::Accepted)
        );
    }
}

// ─── §2.3: BTP is symmetric once established ───

/// §2.3: after auth either side may originate on the one session -- the
/// whole of the difference between the two carriages, and why BTP needs no
/// `Toon-Flush-Requested` analogue (§6.4). The accepting side's handle
/// originates a MESSAGE and the dialing side answers it.
#[tokio::test]
async fn the_accepting_side_can_originate_on_the_session_it_accepted() {
    let payer_signer = LocalSigner::generate("payer");
    let state = carriage(payee(&payer_signer), bound_policy());
    let (frames, frames_rx) = mpsc::channel::<Vec<u8>>(32);
    let (replies, mut answers) = mpsc::channel::<Vec<u8>>(32);
    let session = PeerSession::new(state, replies);
    let handle = session.handle();
    let driver = tokio::spawn(session.run(frames_rx));

    let counterparty = tokio::spawn(async move {
        let bytes = answers.recv().await.expect("the originated MESSAGE");
        let frame = decode_frame(&bytes).expect("our own encoder");
        assert_eq!(frame.frame_type, connector_btp::BTP_MESSAGE);
        frames
            .send(encode_response(frame.request_id, &[], b"answered"))
            .await
            .expect("the session is live");
    });

    let answer = handle
        .send_message(&[], &[])
        .await
        .expect("the counterparty answered");

    assert_eq!(answer.ilp_packet, b"answered".to_vec());
    counterparty.await.expect("the counterparty task");
    drop(driver);
}

// ─── the port's own contract (spec I5) ───

/// Establishing that nothing above the port can tell which carriage
/// delivered a packet: the `PeerTransport` contract statements, restated
/// against the BTP carriage. A registered peer's own answer comes back
/// unchanged, and an unregistered peer id produces `T01` with
/// `reached == false`.
///
/// The first forward carries a covering claim, because since ADR 0060 that
/// is what makes it a peer frame at all -- without one the answer would be
/// the client-role `F02` and the peer's *own* routing verdict would never
/// be reached, so there would be nothing to assert came back unchanged.
#[tokio::test]
async fn the_btp_carriage_upholds_the_peer_transport_contract() {
    let payer_signer = LocalSigner::generate("payer");
    let state = carriage(payee(&payer_signer), bound_policy());
    let dialer = LoopbackDialer::new(state);
    let transport = transport(Arc::clone(&dialer) as Arc<dyn PeerDialer>, &payer_signer);

    let PeerForward {
        response,
        reached_peer: reached,
        ..
    } = transport
        .forward(
            PEER_ID,
            prepare("g.nowhere-on-the-peer"),
            Some(sign_claim(&payer_signer, 1, 500)),
        )
        .await;
    match response {
        PacketResponse::Reject(reject) => {
            assert_eq!(reject.code.as_str(), "F02");
            assert!(reject.message.contains("g.nowhere-on-the-peer"));
        }
        other => panic!("expected the peer's own reject, got {other:?}"),
    }
    assert!(reached);

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport
        .forward("unregistered", prepare("g.anything"), None)
        .await;
    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "T01"),
        other => panic!("expected T01, got {other:?}"),
    }
    assert_eq!(ack, ClaimAckOutcome::NotSent);
    assert!(!reached);
}

/// A dialed session is established once and reused: eight concurrent
/// forwards to one peer do not open eight sessions.
#[tokio::test]
async fn concurrent_forwards_share_one_dialed_session() {
    let payer_signer = LocalSigner::generate("payer");
    let state = carriage(payee(&payer_signer), bound_policy());
    let dialer = LoopbackDialer::new(state);
    let transport = Arc::new(transport(
        Arc::clone(&dialer) as Arc<dyn PeerDialer>,
        &payer_signer,
    ));

    let mut handles = Vec::new();
    for _ in 0..8 {
        let transport = Arc::clone(&transport);
        handles.push(tokio::spawn(async move {
            transport.forward(PEER_ID, prepare("g.nowhere"), None).await
        }));
    }
    for handle in handles {
        let PeerForward {
            response,
            reached_peer: reached,
            ..
        } = handle.await.expect("task");
        assert!(matches!(response, PacketResponse::Reject(_)));
        assert!(reached);
    }

    assert_eq!(
        dialer.dials.load(Ordering::SeqCst),
        1,
        "eight concurrent forwards opened one socket"
    );
}

/// A signature is 65 bytes and its recovery id rides raw (§4.2) -- proved
/// here against a claim that actually verifies at the far end, so the
/// byte layout is not merely asserted but *used*.
#[test]
fn a_signed_claim_carries_a_65_byte_signature() {
    let payer_signer = LocalSigner::generate("payer");
    let claim = sign_claim(&payer_signer, 1, 500);
    let ClaimSignature::Evm(Signature { recovery_id, .. }) = claim.signature else {
        unreachable!("this connector signs peer claims on secp256k1 only")
    };

    assert_eq!(claim.signature.to_bytes().len(), 65);
    assert!(
        recovery_id <= 1,
        "libsecp256k1's {{0, 1}}, never {{27, 28}}"
    );
}
