//! The ILP-over-HTTP peer carriage end to end
//! (`docs/protocol/peer-carriage-spec.md`, issue #728): an
//! [`HttpPeerTransport`] **dials**, a [`PeerHttpState`] **accepts**, and the
//! requests and responses between them are the ones §3's table names.
//!
//! Nothing here is a fake shortcut past the thing under test. The two sides
//! are joined by an in-process client standing in for the socket and *only*
//! for the socket: every header is the one the shared name table declares,
//! every role decision is `connector_peer_auth::decide_role`'s, every claim
//! is judged by the real `ClaimBook` behind a real `Connector`, and the payer
//! reaches the payee only through the `PeerTransport` port. What is not
//! exercised is TLS and the socket itself.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use connector_btp::{
    ACCUMULATED_COST_HEADER, CLAIM_ACK_HEADER, CLAIM_HEADER, FLUSH_REQUESTED_HEADER,
    PAYMENT_REQUIRED_HEADER,
};
use connector_config::StaticRoute;
use connector_domain::x402::parse_greeting;
use connector_domain::{PacketResponse, Prepare};
use connector_peer_auth::PeerAuthPolicy;
use connector_peer_btp::{AcceptedClaims, ClaimEnforcementPolicy};
use connector_peer_http::accept::{FlushHints, PeerHttpPolicy, PeerHttpState};
use connector_peer_http::dial::{HttpDialError, PeerHttpClient, PeerRelation};
use connector_peer_http::headers::{Headers, PeerRequest, PeerResponse};
use connector_peer_http::{HttpPeerTransport, NAT_NOTE};
use connector_runtime::{
    ChannelDomain, ClaimAckOutcome, ClaimRejectReason, ClaimSignature, ClaimStateDomain,
    ClaimStateSource, ClaimWatermark, Clock, Connector, EvmDomain, FakeAppClient, InMemoryJournal,
    InProcessPeerTransport, Journal, OutboundClientError, OutboundClientLedger, PeerForward,
    PeerRoute, PeerTransport, TestClock, WireClaim,
};
use connector_signer::{
    derive_evm_address, evm_balance_proof_digest, EvmBalanceProof, LocalSigner, Signer,
};
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
/// `lockedAmount`/`locksRoot` as zeros. Unchanged by carriage (§3.1).
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

fn accepting(connector: Arc<Connector>, policy: Arc<PeerAuthPolicy>) -> Arc<PeerHttpState> {
    accepting_with(connector, policy, Arc::new(FlushHints::new()))
}

fn accepting_with(
    connector: Arc<Connector>,
    policy: Arc<PeerAuthPolicy>,
    hints: Arc<FlushHints>,
) -> Arc<PeerHttpState> {
    accepting_with_enforcement(
        connector,
        policy,
        Arc::new(ClaimEnforcementPolicy::default()),
        hints,
    )
}

/// [`accepting_with`], with an explicit [`ClaimEnforcementPolicy`] rather
/// than the default (empty, so every peer reads
/// `ForwardedClaimEnforcement::Observe`). Only ADR 0042's forwarded rule
/// answers to this policy: the terminated rule's own knob was deleted with
/// its escape hatch (issue #1077) and refuses unconditionally.
fn accepting_with_enforcement(
    connector: Arc<Connector>,
    policy: Arc<PeerAuthPolicy>,
    enforcement: Arc<ClaimEnforcementPolicy>,
    hints: Arc<FlushHints>,
) -> Arc<PeerHttpState> {
    Arc::new(PeerHttpState::new(
        connector,
        policy,
        Arc::new(AcceptedClaims::new()),
        enforcement,
        hints,
        PeerHttpPolicy::default(),
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
/// `ClientRouteKind::Forwarded` arrival. The BTP twin of the same fixture.
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
            response: connector_domain::EnvelopeResponse {
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
    let envelope = connector_domain::EnvelopeRequest {
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

// ─── the in-process client standing in for the socket ───

/// Hands each request straight to an accepting [`PeerHttpState`], recording
/// what actually went on the wire so a test can assert §3's table rather than
/// only what came back.
struct Loopback {
    peer: Arc<PeerHttpState>,
    sent: Mutex<Vec<PeerRequest>>,
    /// §7.2: the high-water mark of claim-bearing requests in flight at once.
    concurrent_claims: AtomicUsize,
    peak_concurrent_claims: AtomicUsize,
    /// How long each request dwells inside the peer, so overlapping requests
    /// would actually overlap if the in-flight rule were not enforced.
    dwell: Duration,
}

impl Loopback {
    fn new(peer: Arc<PeerHttpState>) -> Arc<Loopback> {
        Arc::new(Loopback {
            peer,
            sent: Mutex::new(Vec::new()),
            concurrent_claims: AtomicUsize::new(0),
            peak_concurrent_claims: AtomicUsize::new(0),
            dwell: Duration::ZERO,
        })
    }

    fn with_dwell(peer: Arc<PeerHttpState>, dwell: Duration) -> Arc<Loopback> {
        let mut loopback = Loopback::new(peer);
        Arc::get_mut(&mut loopback).expect("sole owner").dwell = dwell;
        loopback
    }

    fn sent(&self) -> Vec<PeerRequest> {
        self.sent.lock().expect("sent lock").clone()
    }

    fn last(&self) -> PeerRequest {
        self.sent().pop().expect("a request went out")
    }
}

#[async_trait]
impl PeerHttpClient for Loopback {
    async fn post(
        &self,
        _endpoint: &Url,
        request: PeerRequest,
    ) -> Result<PeerResponse, HttpDialError> {
        self.sent.lock().expect("sent lock").push(request.clone());
        let carries_claim = request.headers.get(CLAIM_HEADER).is_some();
        if carries_claim {
            let in_flight = self.concurrent_claims.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak_concurrent_claims
                .fetch_max(in_flight, Ordering::SeqCst);
        }
        if !self.dwell.is_zero() {
            tokio::time::sleep(self.dwell).await;
        }
        let response = self.peer.handle(request).await;
        if carries_claim {
            self.concurrent_claims.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(response)
    }
}

/// A client that never reaches anybody -- the "the remote does not expose
/// what we dial" case §2.2 says is not locally detectable and must surface as
/// an ordinary dial failure.
struct Unreachable;

#[async_trait]
impl PeerHttpClient for Unreachable {
    async fn post(
        &self,
        endpoint: &Url,
        _request: PeerRequest,
    ) -> Result<PeerResponse, HttpDialError> {
        Err(HttpDialError {
            peer_id: PEER_ID.to_string(),
            endpoint: endpoint.to_string(),
            reason: "connection refused".to_string(),
        })
    }
}

/// A client that answers a status carrying no ILP body at all (§6.2's
/// `4xx`/`5xx`).
struct Status(u16);

#[async_trait]
impl PeerHttpClient for Status {
    async fn post(
        &self,
        _endpoint: &Url,
        _request: PeerRequest,
    ) -> Result<PeerResponse, HttpDialError> {
        Ok(PeerResponse::refused(self.0))
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
        Url::parse("https://peer.example:443/ilp").unwrap(),
        domains,
        solana_program_ids,
        Duration::from_millis(30_000),
        Duration::from_millis(30_000),
    )
}

fn transport(client: Arc<dyn PeerHttpClient>, payer: &dyn Signer) -> HttpPeerTransport {
    let transport = HttpPeerTransport::new(
        client,
        derive_evm_address(&payer.public_key().unwrap()),
        clock() as Arc<dyn Clock>,
    );
    transport.add_peer(relation());
    transport
}

/// One request, as a peer would send it by hand -- for the accept-side tests
/// that have no dialing transport in front of them.
fn request(claim_json: Option<&str>, body: Vec<u8>) -> PeerRequest {
    let mut headers = Headers::new();
    if let Some(json) = claim_json {
        headers.push(
            CLAIM_HEADER,
            connector_peer_http::headers::claim_header_value(json),
        );
    }
    PeerRequest { headers, body }
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

fn ack_on(response: &PeerResponse) -> Option<ClaimAckOutcome> {
    connector_peer_http::headers::claim_ack(&response.headers)
}

// ─── §3, §6: a claim rides a PREPARE and is acknowledged ───

/// §6.2, the property whose loss would silently destroy ADR 0024's
/// semantics: **the body answers the packet, the header answers the claim,
/// and the status is `200` regardless**. The packet is rejected (the payee
/// has no route for it) and the claim that rode it is accepted, on the one
/// response.
#[tokio::test]
async fn a_claim_riding_a_prepare_is_judged_independently_of_the_packet() {
    let payer_signer = LocalSigner::generate("payer");
    let peer = accepting(payee(&payer_signer), bound_policy());
    let client = Loopback::new(peer);
    let transport = transport(
        Arc::clone(&client) as Arc<dyn PeerHttpClient>,
        &payer_signer,
    );
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

/// §3's table, as bytes: the credential and the claim are `base64(JSON)`
/// headers, and the body is the OER PREPARE unchanged (§8.1).
#[tokio::test]
async fn the_request_a_dialed_peering_puts_on_the_wire_is_the_one_section_3_names() {
    let payer_signer = LocalSigner::generate("payer");
    let peer = accepting(payee(&payer_signer), bound_policy());
    let client = Loopback::new(peer);
    let transport = transport(
        Arc::clone(&client) as Arc<dyn PeerHttpClient>,
        &payer_signer,
    );
    let claim = sign_claim(&payer_signer, 1, 500);
    let prepare = prepare("g.nowhere");

    let _ = transport
        .forward(PEER_ID, prepare.clone(), Some(claim.clone()))
        .await;

    let sent = client.last();
    // §1.4: nothing authenticates the peering but the claim. A dialer sends
    // no credential, because there is none to send (ADR 0060) -- and the
    // header it used to ride in must not reappear under any spelling.
    assert!(
        sent.headers
            .iter()
            .all(|(name, _)| !name.eq_ignore_ascii_case("toon-peer-auth")),
        "a dialed peering put a peer credential on the wire"
    );
    // §4: `base64(JSON)` over exactly the JSON the BTP entry carries raw.
    let claim_json = base64_decode(sent.headers.get(CLAIM_HEADER).expect("a claim rode"));
    assert_eq!(
        connector_peer_btp::claim_json::parse(&claim_json).expect("the client edge's validator"),
        claim
    );
    // §8.1: `data` rides byte-for-byte unchanged, in the same OER encoding
    // every other carriage puts on a wire.
    assert_eq!(sent.body, prepare.encode());
    // §3: a peer connector MUST NOT invent additional headers. One, now
    // that the credential is gone (ADR 0060): the claim.
    assert_eq!(sent.headers.len(), 1, "got {:?}", sent.headers);
}

/// §10.2 item 6: a claimless PREPARE is legal, and carries no claim header
/// rather than an empty one.
#[tokio::test]
async fn a_claimless_prepare_carries_no_claim_header() {
    let payer_signer = LocalSigner::generate("payer");
    let peer = accepting(payee(&payer_signer), bound_policy());
    let client = Loopback::new(peer);
    let transport = transport(
        Arc::clone(&client) as Arc<dyn PeerHttpClient>,
        &payer_signer,
    );

    let _ = transport.forward(PEER_ID, prepare("g.nowhere"), None).await;

    let sent = client.last();
    assert_eq!(sent.headers.get(CLAIM_HEADER), None);
}

/// FLUSH (§3): **a POST with an empty ILP body plus the claim header** --
/// the standalone-claim shape of `client-edge-spec.md` §1.9 step 5 -- and
/// the ack rides the response that answers it.
#[tokio::test]
async fn a_flush_is_a_post_with_an_empty_body_and_the_claim_header() {
    let payer_signer = LocalSigner::generate("payer");
    let peer = accepting(payee(&payer_signer), bound_policy());
    let client = Loopback::new(peer);
    let transport = transport(
        Arc::clone(&client) as Arc<dyn PeerHttpClient>,
        &payer_signer,
    );
    let claim = sign_claim(&payer_signer, 3, 900);

    let ack = transport.flush(PEER_ID, claim.clone()).await;

    assert_eq!(ack, ClaimAckOutcome::Accepted);
    let sent = client.last();
    assert!(sent.body.is_empty(), "a FLUSH carries no ILP packet");
    let carried = base64_decode(sent.headers.get(CLAIM_HEADER).expect("the claim rode"));
    assert_eq!(
        connector_peer_btp::claim_json::parse(&carried)
            .expect("valid")
            .cumulative_amount,
        claim.cumulative_amount
    );
}

/// Issue #759's AC, exercised on the real ILP-over-HTTP wire: a Solana
/// claim flushed over this carriage carries the `[[peer_channels]]`-
/// configured `programId` -- never the deleted
/// `PLACEHOLDER_SOLANA_PROGRAM_ID` -- and the same bytes round-trip through
/// the client edge's own validator (`claim_json::parse`), the HTTP
/// counterpart of `connector-peer-btp`'s own
/// `a_flushed_solana_claim_carries_its_configured_program_id_on_the_wire`
/// (I4: one codec serves both carriages).
#[tokio::test]
async fn a_flushed_solana_claim_carries_its_configured_program_id_on_the_wire() {
    let payer_signer = LocalSigner::generate("payer");
    let peer = accepting(payee(&payer_signer), bound_policy());
    let client = Loopback::new(peer);
    let mut transport = transport(
        Arc::clone(&client) as Arc<dyn PeerHttpClient>,
        &payer_signer,
    );
    transport.set_solana_signer_public_key([0x77; 32]);

    let claim = WireClaim {
        channel_id: solana_channel_account(),
        nonce: 1,
        cumulative_amount: 500,
        signature: ClaimSignature::Solana([0x5a; 64]),
    };

    let _ = transport.flush(PEER_ID, claim.clone()).await;

    let sent = client.last();
    assert!(sent.body.is_empty(), "a FLUSH carries no ILP packet");
    let carried = base64_decode(sent.headers.get(CLAIM_HEADER).expect("the claim rode"));
    let claim_json: serde_json::Value =
        serde_json::from_slice(&carried).expect("raw UTF-8 JSON, base64 of it on this carriage");
    assert_eq!(claim_json["blockchain"], "solana");
    assert_eq!(claim_json["programId"], SOLANA_PROGRAM_ID);
    assert_eq!(claim_json["channelAccount"], solana_channel_account());

    let parsed = connector_peer_btp::claim_json::parse(&carried).expect("round trip");
    assert_eq!(parsed, claim);
}

/// The same wire assertion, but with the relation built by
/// [`HttpPeerTransport::add_peers_from_config`] from a **loaded config**
/// rather than by hand -- because the value under test is precisely the one
/// this carriage must not choose for itself.
///
/// Since issue #1128 a Solana `[[peer_channels]]` row MUST NOT restate a
/// `program_id`; the row resolves it from `[settlement.solana] program_id`,
/// the only program this node can redeem a claim through. This closes the
/// hop between that table and the `programId` a peer claim carries, and it
/// is half of what `peer-carriage-spec.md` §4.1 relies on when it says a
/// peer-edge check of the declared field would have nothing to find: the
/// same configured value renders the label here and keys the
/// `SolanaChannel` an inbound claim is verified against. The other half --
/// that a claim declaring one program and signed under another is accepted
/// on its signature, silently -- is
/// `connector-peer-btp`'s `a_peer_claims_declared_program_is_not_consulted`.
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
endpoint = "https://peer.example:443/ilp"

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
    let peer = accepting(payee(&payer_signer), bound_policy());
    let client = Loopback::new(peer);
    let mut transport = HttpPeerTransport::new(
        Arc::clone(&client) as Arc<dyn PeerHttpClient>,
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

    let carried = base64_decode(
        client
            .last()
            .headers
            .get(CLAIM_HEADER)
            .expect("the claim rode"),
    );
    let claim_json: serde_json::Value = serde_json::from_slice(&carried).expect("raw UTF-8 JSON");
    assert_eq!(
        claim_json["programId"], SOLANA_PROGRAM_ID,
        "the declared program is `[settlement.solana] program_id` and nothing else (#1128)"
    );
}

// ─── §6.3: retransmission, and the idempotent re-ack ───

/// §6.3, the rule standing between a lost ack and a permanently wedged
/// peering -- **on both halves at once**.
///
/// The payer's half: the claim JSON carries a `timestamp`, so a payer that
/// re-rendered it with a fresh `now` could never produce a byte-identical
/// retransmission, and every one of its retransmissions would be a
/// *different* claim at the same nonce -- which a payee MUST refuse. The
/// payee's half: a claim byte-identical to the one already at the watermark
/// is answered `accepted`, and nothing is advanced or recorded.
#[tokio::test]
async fn a_byte_identical_retransmission_at_the_watermark_is_accepted_again() {
    let payer_signer = LocalSigner::generate("payer");
    let peer = accepting(payee(&payer_signer), bound_policy());
    let client = Loopback::new(peer);
    let transport = transport(
        Arc::clone(&client) as Arc<dyn PeerHttpClient>,
        &payer_signer,
    );
    let claim = sign_claim(&payer_signer, 1, 500);

    // The first flush is answered; the payer then retransmits the same
    // pending claim, as it must when an ack goes missing.
    assert_eq!(
        transport.flush(PEER_ID, claim.clone()).await,
        ClaimAckOutcome::Accepted
    );
    // The clock moves. A payer that re-rendered the JSON here would emit
    // different bytes at the same nonce.
    let ack = transport.flush(PEER_ID, claim).await;

    assert_eq!(
        ack,
        ClaimAckOutcome::Accepted,
        "a retransmission at the watermark is accepted, never nonce_not_advancing"
    );
    let sent = client.sent();
    assert_eq!(
        sent[0].headers.get(CLAIM_HEADER),
        sent[1].headers.get(CLAIM_HEADER),
        "the retransmission was byte-identical (§6.3)"
    );
}

/// §6.3's other half: the **same nonce with any other field changed** is a
/// different claim, and is refused `nonce_not_advancing` exactly as §3.2's
/// strictly-advancing rule requires. Together with the test above this pins
/// the boundary.
#[tokio::test]
async fn the_same_nonce_with_different_bytes_is_refused_nonce_not_advancing() {
    let payer_signer = LocalSigner::generate("payer");
    let peer = accepting(payee(&payer_signer), bound_policy());
    let client = Loopback::new(Arc::clone(&peer));
    let transport = transport(client as Arc<dyn PeerHttpClient>, &payer_signer);

    assert_eq!(
        transport
            .flush(PEER_ID, sign_claim(&payer_signer, 1, 500))
            .await,
        ClaimAckOutcome::Accepted
    );
    // Same nonce, a different cumulative: a different claim.
    let response = peer
        .handle(request(
            Some(&claim_as_json(
                &sign_claim(&payer_signer, 1, 900),
                &payer_signer,
            )),
            Vec::new(),
        ))
        .await;

    assert_eq!(
        ack_on(&response),
        Some(ClaimAckOutcome::Rejected(
            ClaimRejectReason::NonceNotAdvancing
        ))
    );
}

/// §6.3: **absence means NOT ACKNOWLEDGED** -- never accepted, never
/// rejected, never inferred from the packet's verdict.
#[tokio::test]
async fn a_response_carrying_no_ack_header_leaves_the_claim_not_acknowledged() {
    struct Silent;

    #[async_trait]
    impl PeerHttpClient for Silent {
        async fn post(
            &self,
            _endpoint: &Url,
            _request: PeerRequest,
        ) -> Result<PeerResponse, HttpDialError> {
            Ok(PeerResponse::ok(Vec::new()))
        }
    }

    let payer_signer = LocalSigner::generate("payer");
    let transport = transport(Arc::new(Silent), &payer_signer);

    let ack = transport
        .flush(PEER_ID, sign_claim(&payer_signer, 1, 500))
        .await;

    assert_eq!(ack, ClaimAckOutcome::NotSent);
}

/// §6.3: a malformed ack -- undecodable base64, undecodable JSON, an unknown
/// `result`, a `rejected` with no `reason` -- is likewise **not
/// acknowledged**, and MUST NOT be read as either verdict.
#[tokio::test]
async fn a_malformed_ack_header_leaves_the_claim_not_acknowledged() {
    struct Garbled(&'static str);

    #[async_trait]
    impl PeerHttpClient for Garbled {
        async fn post(
            &self,
            _endpoint: &Url,
            _request: PeerRequest,
        ) -> Result<PeerResponse, HttpDialError> {
            let mut response = PeerResponse::ok(Vec::new());
            response.headers.push(CLAIM_ACK_HEADER, self.0);
            Ok(response)
        }
    }

    for garbled in [
        "!!! not base64 !!!",
        "bm90IGpzb24=",                 // "not json"
        "eyJyZXN1bHQiOiJtYXliZSJ9",     // {"result":"maybe"}
        "eyJyZXN1bHQiOiJyZWplY3RlZCJ9", // {"result":"rejected"}
    ] {
        let payer_signer = LocalSigner::generate("payer");
        let transport = transport(Arc::new(Garbled(garbled)), &payer_signer);

        let ack = transport
            .flush(PEER_ID, sign_claim(&payer_signer, 1, 500))
            .await;

        assert_eq!(
            ack,
            ClaimAckOutcome::NotSent,
            "read a verdict from {garbled}"
        );
    }
}

/// §6.2: `4xx`/`5xx` are reserved for a request there is no ILP answer to.
/// A response like that is not an ILP verdict, is not a claim verdict, and
/// leaves the claim not acknowledged.
#[tokio::test]
async fn a_non_200_answer_is_no_ilp_answer_at_all() {
    let payer_signer = LocalSigner::generate("payer");
    let transport = transport(Arc::new(Status(400)), &payer_signer);

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport
        .forward(
            PEER_ID,
            prepare("g.nowhere"),
            Some(sign_claim(&payer_signer, 1, 500)),
        )
        .await;

    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "T01"),
        other => panic!("expected T01, got {other:?}"),
    }
    assert_eq!(ack, ClaimAckOutcome::NotSent);
    assert!(
        !reached,
        "no fee of ours belongs on a hop that never forwarded"
    );
}

// ─── §2.2, §2.4, §6.4(1): what cannot be reached, and why ───

/// §2.2: whether the remote exposes what we dial is not locally detectable,
/// so it surfaces as an ordinary dial failure and the packet rejects `T01` --
/// never `T00`, and never a silent drop.
#[tokio::test]
async fn a_peer_that_cannot_be_reached_rejects_t01_and_was_never_reached() {
    let payer_signer = LocalSigner::generate("payer");
    let transport = transport(Arc::new(Unreachable), &payer_signer);

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport.forward(PEER_ID, prepare("g.nowhere"), None).await;

    match response {
        PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "T01"),
        other => panic!("expected T01, got {other:?}"),
    }
    assert_eq!(ack, ClaimAckOutcome::NotSent);
    assert!(!reached);
}

/// §6.4(1) and §2.4, the two things an operator hits first and diagnoses
/// last. A peer this connector does not dial over HTTP can never be
/// originated to -- packets flow only in the dialing direction -- and the
/// `T01` says so, including that an HTTP-only peer can neither reach nor be
/// reached by a NAT'd peer.
#[tokio::test]
async fn a_peer_this_connector_cannot_originate_to_says_why_in_its_t01() {
    let payer_signer = LocalSigner::generate("payer");
    let transport = transport(Arc::new(Unreachable), &payer_signer);

    let PeerForward {
        response,
        ack,
        reached_peer: reached,
        ..
    } = transport
        .forward("accept-only", prepare("g.nowhere"), None)
        .await;

    match response {
        PacketResponse::Reject(reject) => {
            assert_eq!(reject.code.as_str(), "T01");
            assert!(
                reject.message.contains("§6.4(1)"),
                "an operator must not have to infer unidirectional packet flow: {}",
                reject.message
            );
            assert!(
                reject.message.contains("NAT'd peer"),
                "the NAT consequence is the least obvious thing here: {}",
                reject.message
            );
        }
        other => panic!("expected T01, got {other:?}"),
    }
    assert_eq!(ack, ClaimAckOutcome::NotSent);
    assert!(!reached);
    assert!(NAT_NOTE.contains("must be BTP"));
}

/// An accept-only peering cannot flush either (§6.4(2)): the claim stays
/// pending until this connector can dial again, and its counterparty's
/// protection in that window is that counterparty's own ceiling.
#[tokio::test]
async fn an_accept_only_peering_cannot_flush() {
    let payer_signer = LocalSigner::generate("payer");
    let transport = transport(Arc::new(Unreachable), &payer_signer);

    let ack = transport
        .flush("accept-only", sign_claim(&payer_signer, 1, 500))
        .await;

    assert_eq!(ack, ClaimAckOutcome::NotSent);
}

// ─── §1: role is decided by authentication ───

/// **The named regression (§1.9).** `toon-sandbox` admitted an anonymous BTP
/// session with `btp_auth … success:true mode:"no-auth"` and then treated it
/// as a quasi-peer. Each of the five interactions below is classified
/// `client` and reaches **no peer handling whatsoever** -- testable, per
/// §1.9, as: no `Toon-Claim-Ack` was emitted, and the claim they carried
/// moved no peer watermark (proved by a subsequent *genuine* peer claim at
/// nonce 1 being accepted, which it could not be if any of these had
/// advanced anything).
#[tokio::test]
async fn the_named_regression_no_request_becomes_a_peer_without_p2_and_p3() {
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
    let peer = accepting(payee(&payer_signer), policy);

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
        let response = peer
            .handle(request(
                claim_json.as_deref(),
                prepare("g.nowhere").encode(),
            ))
            .await;

        // §1.6: not refused for the assertion alone -- refusing would make
        // the check an oracle for which channels are configured.
        assert_eq!(response.status, 200, "case {index}");
        assert!(
            ack_on(&response).is_none(),
            "case {index}: a client interaction gets no claim-ack (§1.7)"
        );
        assert!(
            response.headers.get(FLUSH_REQUESTED_HEADER).is_none(),
            "case {index}: a client interaction is never a peering relation (§6.4)"
        );
    }

    // Nothing above moved a peer watermark: a genuine peer's claim at nonce
    // 1 is still fresh.
    let genuine = claim_as_json(&sign_claim(&payer_signer, 1, 500), &payer_signer);
    let response = peer.handle(request(Some(&genuine), Vec::new())).await;

    assert_eq!(
        ack_on(&response),
        Some(ClaimAckOutcome::Accepted),
        "no client interaction had advanced this channel's peer watermark"
    );
}

/// §1.5's header-smuggling defence, now over the material that actually
/// decides role: **more than one claim header on one request is refused, not
/// resolved** -- `400`, with no ILP body, and never the first, the last or a
/// concatenation. Its absence is how "which claim did we verify?" becomes
/// unanswerable. It guarded the `Toon-Peer-Auth` header until ADR 0060
/// deleted it; the defect it names is a property of duplicated
/// authentication material rather than of the credential in particular.
#[tokio::test]
async fn two_claims_on_one_request_are_refused_rather_than_resolved() {
    let payer_signer = LocalSigner::generate("payer");
    let peer = accepting(payee(&payer_signer), bound_policy());
    let first = claim_as_json(&sign_claim(&payer_signer, 1, 500), &payer_signer);
    let second = claim_as_json(&sign_claim(&payer_signer, 2, 600), &payer_signer);
    let mut headers = Headers::new();
    headers.push(
        CLAIM_HEADER,
        connector_peer_http::headers::claim_header_value(&first),
    );
    headers.push(
        CLAIM_HEADER,
        connector_peer_http::headers::claim_header_value(&second),
    );

    let response = peer
        .handle(PeerRequest {
            headers,
            body: prepare("g.nowhere").encode(),
        })
        .await;

    assert_eq!(response.status, 400);
    assert!(response.body.is_empty(), "a 400 carries no ILP body (§1.5)");
    assert!(ack_on(&response).is_none());

    // Neither claim was adopted: nonce 1 is still fresh, so nothing was
    // resolved behind the refusal.
    let fresh = peer.handle(request(Some(&first), Vec::new())).await;
    assert_eq!(ack_on(&fresh), Some(ClaimAckOutcome::Accepted));
}

/// §1.4: because HTTP has no session, a request is judged on its own claim.
/// One request proving a peering says nothing about the next -- a request
/// carrying no claim is a client request, whatever the previous request from
/// the same connection carried.
#[tokio::test]
async fn a_request_without_a_claim_is_a_client_however_the_last_one_was_judged() {
    let payer_signer = LocalSigner::generate("payer");
    let peer = accepting(payee(&payer_signer), bound_policy());
    let json = claim_as_json(&sign_claim(&payer_signer, 1, 500), &payer_signer);

    let proven = peer.handle(request(Some(&json), Vec::new())).await;
    let next = peer.handle(request(None, Vec::new())).await;

    assert_eq!(ack_on(&proven), Some(ClaimAckOutcome::Accepted));
    assert!(
        ack_on(&next).is_none(),
        "the previous request's role does not carry over (§1.4)"
    );
}

/// §5.2: a REJECT this carriage sends always carries the running cost,
/// **even at zero**, so "absent" never has to carry meaning in the
/// direction that matters. Kept when §5.1's malformed-floor case went with
/// minimum delivery (ADR 0057, issue #1143), which is where this assertion
/// used to ride.
#[tokio::test]
async fn a_peers_reject_always_carries_the_accumulated_cost_even_at_zero() {
    let payer_signer = LocalSigner::generate("payer");
    let peer = accepting(payee(&payer_signer), bound_policy());
    let json = claim_as_json(&sign_claim(&payer_signer, 1, 500), &payer_signer);

    let response = peer
        .handle(request(Some(&json), prepare("g.nowhere").encode()))
        .await;

    assert_eq!(response.status, 200);
    let reject = connector_domain::Reject::decode(&response.body).expect("a reject");
    assert_eq!(reject.code.as_str(), "F02");
    assert_eq!(response.headers.get(ACCUMULATED_COST_HEADER), Some("0"));
    // §6.2: the two verdicts are independent, so the claim that rode the
    // refused packet is still judged and still acknowledged.
    assert_eq!(ack_on(&response), Some(ClaimAckOutcome::Accepted));
}

/// §1.10's bounded escape hatch: on a **dedicated** peer listener a request
/// that fails P2 or P3 is refused outright rather than downgraded, because
/// such a listener serves no clients -- there is no client to downgrade to
/// and no oracle to leak. Role is still decided by P2 and P3; the listener
/// never becomes the decider.
#[tokio::test]
async fn a_dedicated_peer_listener_refuses_rather_than_downgrades() {
    let payer_signer = LocalSigner::generate("payer");
    let impostor = LocalSigner::generate("impostor");
    let peer = Arc::new(PeerHttpState::new(
        payee(&payer_signer),
        bound_policy(),
        Arc::new(AcceptedClaims::new()),
        Arc::new(ClaimEnforcementPolicy::default()),
        Arc::new(FlushHints::new()),
        PeerHttpPolicy {
            mandatory_auth: true,
        },
    ));
    let forged = claim_as_json(&sign_claim(&impostor, 1, 500), &impostor);
    let proven = claim_as_json(&sign_claim(&payer_signer, 1, 500), &payer_signer);

    let no_claim = peer
        .handle(request(None, prepare("g.nowhere").encode()))
        .await;
    let unverified = peer
        .handle(request(Some(&forged), prepare("g.nowhere").encode()))
        .await;
    let admitted = peer
        .handle(request(Some(&proven), prepare("g.nowhere").encode()))
        .await;

    assert_eq!(no_claim.status, 401);
    assert!(no_claim.body.is_empty());
    assert_eq!(
        unverified.status, 401,
        "a claim that fails P3 is refused too"
    );
    assert_eq!(admitted.status, 200, "P2 and P3 still decide the role");
}

// ─── §6.4: the flush prompt, and only a prompt ───

/// §6.4: a payee that cannot originate MAY prompt a payer, one channel id
/// per occurrence, and a payer holding that pending claim sends it on its
/// next request. It **creates no obligation**: nothing is refused, rejected
/// or re-accounted because a hint went unanswered.
#[tokio::test]
async fn a_flush_prompt_is_read_by_the_payer_and_obliges_nothing() {
    let payer_signer = LocalSigner::generate("payer");
    let hints = Arc::new(FlushHints::new());
    let peer = accepting_with(payee(&payer_signer), bound_policy(), Arc::clone(&hints));
    let client = Loopback::new(Arc::clone(&peer));
    let transport = transport(
        Arc::clone(&client) as Arc<dyn PeerHttpClient>,
        &payer_signer,
    );

    // The payer has a pending, unacknowledged claim: the payee answered it
    // with nothing at all, which §6.3 makes "not acknowledged".
    let response = peer
        .handle(request(None, prepare("g.nowhere").encode()))
        .await;
    assert!(
        response.headers.get(FLUSH_REQUESTED_HEADER).is_none(),
        "no hint was requested yet"
    );

    // A claim goes out and is accepted, so nothing is pending; a hint for it
    // is then ignored, exactly as §6.4 requires of a channel the payer holds
    // no pending claim on.
    let _ = transport
        .flush(PEER_ID, sign_claim(&payer_signer, 1, 500))
        .await;
    hints.request(PEER_ID, &channel_id());
    let _ = transport.forward(PEER_ID, prepare("g.nowhere"), None).await;

    assert!(
        transport.flush_hints(PEER_ID).is_empty(),
        "a hint for a channel with no pending claim is ignored (§6.4)"
    );
}

/// §6.4: the prompt rides a response to a **peer**, one occurrence per
/// channel, and is drained rather than repeated for ever.
#[tokio::test]
async fn a_payee_names_one_channel_per_occurrence_and_only_to_a_peer() {
    let payer_signer = LocalSigner::generate("payer");
    let hints = Arc::new(FlushHints::new());
    let peer = accepting_with(payee(&payer_signer), bound_policy(), Arc::clone(&hints));
    hints.request(PEER_ID, &channel_id());
    hints.request(PEER_ID, &format!("0x{:064x}", 9));
    // A prompt rides a response to a **peer**, so these two requests have
    // to be peer requests -- which, since ADR 0060, means each carries its
    // own verifying claim.
    let first = claim_as_json(&sign_claim(&payer_signer, 1, 500), &payer_signer);
    let second = claim_as_json(&sign_claim(&payer_signer, 2, 600), &payer_signer);

    let prompted = peer
        .handle(request(Some(&first), prepare("g.nowhere").encode()))
        .await;
    let again = peer
        .handle(request(Some(&second), prepare("g.nowhere").encode()))
        .await;

    let named = prompted.headers.get_all(FLUSH_REQUESTED_HEADER);
    assert_eq!(
        named.len(),
        2,
        "one channel id per occurrence, no list form"
    );
    assert!(named.contains(&channel_id().as_str()));
    assert!(
        again.headers.get(FLUSH_REQUESTED_HEADER).is_none(),
        "a hint is drained when it is emitted"
    );

    // §6.4: never on a response to a client interaction -- here, a request
    // carrying no claim at all.
    hints.request(PEER_ID, &channel_id());
    let client_response = peer
        .handle(request(None, prepare("g.nowhere").encode()))
        .await;
    assert!(client_response
        .headers
        .get(FLUSH_REQUESTED_HEADER)
        .is_none());
}

// ─── issue #880 (owner decision #868): every peer PREPARE to a priced
// terminated route carries a covering claim, or is refused with the client
// edge's own x402 greeting ───

/// A claimless arrival reaches **no peer handling at all** (§1.2, ADR
/// 0060). It used to reach this gate: a credential made an interaction a
/// peering without a claim, and the gate then refused it `F06` with the
/// x402 greeting. With role read from the claim there is no such state --
/// a request carrying none is a client request, and the greeting a client
/// gets is the client edge's own, covered by
/// `connector-client-edge`'s `a_claimless_prepare_to_a_priced_route_is_refused_with_the_terms`
/// (BTP) and its `POST /ilp` `402` unit test (HTTP).
///
/// What survives here is the property that matters at this layer: the app
/// never sees it, and nothing is acknowledged.
#[tokio::test]
async fn a_claimless_arrival_at_a_priced_route_reaches_no_peer_handling() {
    let payer_signer = LocalSigner::generate("payer");
    let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
    let peer = accepting(payee_with_route(&payer_signer, route), bound_policy());

    let response = peer
        .handle(request(None, prepare("g.example.app").encode()))
        .await;

    assert_eq!(
        response.status, 200,
        "a packet verdict, not a transport 4xx (§6.2)"
    );
    let reject = connector_domain::Reject::decode(&response.body).expect("a reject");
    assert_eq!(
        reject.code.as_str(),
        "F02",
        "a client-role packet reaches no peer route"
    );
    assert!(
        ack_on(&response).is_none(),
        "no claim-ack to a client (§1.7)"
    );
}

/// A claim rides the request, but its own advance over the watermark falls
/// short of the route's price: refused exactly the same way as no claim at
/// all (issue #880's second acceptance case) -- the claim's own nonce/amount
/// validity is irrelevant to this gate, which judges coverage, not validity.
#[tokio::test]
async fn a_claim_that_does_not_cover_the_routes_price_is_refused_the_same_way() {
    let payer_signer = LocalSigner::generate("payer");
    let route = StaticRoute::new_priced("g.example.app", "http://localhost:4000", 25).unwrap();
    let peer = accepting(payee_with_route(&payer_signer, route), bound_policy());
    let claim = sign_claim(&payer_signer, 1, 10); // advances only 10, price is 25

    let response = peer
        .handle(request(
            Some(&claim_as_json(&claim, &payer_signer)),
            prepare("g.example.app").encode(),
        ))
        .await;

    // The claim itself is perfectly valid and is acknowledged -- the two
    // verdicts stay independent (§6.2) even though this gate is new.
    assert_eq!(ack_on(&response), Some(ClaimAckOutcome::Accepted));
    let reject = connector_domain::Reject::decode(&response.body).expect("a reject");
    assert_eq!(reject.code.as_str(), "F06");
    assert!(response.headers.get(PAYMENT_REQUIRED_HEADER).is_some());
}

/// ADR 0065 (issue #984) at the peer gate: what a peer arrival must cover is
/// the schedule at THAT packet's payload length, and the greeting it gets
/// back publishes the schedule so the peer can price its next packet.
///
/// The specific failure this rules out: a gate that checks the route's base
/// while the termination charges the full schedule would admit a large
/// packet across the peering -- banking the covering claim -- and only then
/// refuse it.
#[tokio::test]
async fn a_peer_claim_must_cover_the_schedule_at_this_packets_length() {
    let payer_signer = LocalSigner::generate("payer");
    let route = StaticRoute::new_scheduled(
        "g.example.app",
        "http://localhost:4000",
        connector_domain::Price::scheduled(25, 1),
    )
    .unwrap();
    let peer = accepting(payee_with_route(&payer_signer, route), bound_policy());

    // ~2 KiB of sealed payload, so the slope actually bites: 25 + 1*2 = 27.
    let mut big = prepare("g.example.app");
    big.data = vec![0xab; 2000];
    let expected = connector_domain::Price::scheduled(25, 1).charge(big.data.len());
    assert_eq!(expected, 27);

    // A claim covering the BASE is no longer enough for this packet.
    let claim = sign_claim(&payer_signer, 1, 25);
    let response = peer
        .handle(request(
            Some(&claim_as_json(&claim, &payer_signer)),
            big.encode(),
        ))
        .await;

    assert_eq!(ack_on(&response), Some(ClaimAckOutcome::Accepted));
    let reject = connector_domain::Reject::decode(&response.body).expect("a reject");
    assert_eq!(reject.code.as_str(), "F06");

    // The greeting quotes what THIS packet costs, and publishes the rule
    // beside it so the peer need not be refused twice to learn it.
    let terms_header = response
        .headers
        .get(PAYMENT_REQUIRED_HEADER)
        .expect("a refused arrival is greeted");
    let raw = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        terms_header.as_bytes(),
    )
    .expect("the greeting is base64");
    let terms = connector_domain::x402::parse_greeting(&raw).expect("well-formed terms");
    assert_eq!(terms.price(), Some(expected));
    assert_eq!(
        terms.schedule(),
        Some(connector_domain::Price::scheduled(25, 1))
    );
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
            response: connector_domain::EnvelopeResponse {
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
    let peer = accepting(connector, bound_policy());
    let claim = sign_claim(&payer_signer, 1, 25); // advances exactly the price

    // A genuinely sealed envelope (ADR 0018/0019), so this termination can
    // actually fulfil rather than being refused for want of sealing --
    // orthogonal to this gate, but needed to prove delivery reached the app.
    let envelope = connector_domain::EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: b"hello".to_vec(),
    };
    let identity_public = identity_signer.public_key().expect("identity public key");
    let (data, shared_secret) =
        connector_signer::giftwrap::seal_request(&envelope.encode(), &identity_public)
            .expect("seal");
    let prepare = Prepare {
        amount: 25,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: "g.example.app".to_string(),
        data,
    };

    let response = peer
        .handle(request(
            Some(&claim_as_json(&claim, &payer_signer)),
            prepare.encode(),
        ))
        .await;

    assert_eq!(ack_on(&response), Some(ClaimAckOutcome::Accepted));
    assert!(
        response.headers.get(PAYMENT_REQUIRED_HEADER).is_none(),
        "an admitted packet carries no greeting"
    );
    let fulfill = connector_domain::Fulfill::decode(&response.body).expect("a fulfil");
    let opened = connector_signer::giftwrap::open_response(&shared_secret, &fulfill.data)
        .expect("open the sealed fulfil");
    let opened = connector_domain::EnvelopeResponse::decode(&opened).expect("decode envelope");
    assert_eq!(opened.body, response_body);
    assert_eq!(app_client.deliveries().len(), 1);
}

// ─── issue #1104: coverage is the claim's advance past the **durable**
// watermark, so a payee restart never credits a claim with its whole
// cumulative amount. §0.1's one pipeline: the BTP twin of each of these
// lives in `connector-peer-btp`'s own `peer_carriage.rs` ───

/// An app that actually answers `route`'s handler, so a packet the gate
/// admits visibly **fulfils**: without the fix, issue #1104's packet is
/// served for free rather than merely getting past one check.
fn serving_app(route: &StaticRoute, body: &[u8]) -> Arc<FakeAppClient> {
    let app_client = Arc::new(FakeAppClient::new());
    app_client.respond(
        route.handler_url(),
        connector_runtime::AppOutcome::Answered {
            response: connector_domain::EnvelopeResponse {
                status: 200,
                headers: vec![],
                body: body.to_vec(),
            },
        },
    );
    app_client
}

/// A PREPARE genuinely sealed to [`payee_identity`] (ADR 0018/0019), with
/// the shared secret its answer can be opened with. Orthogonal to the price
/// gate, but what lets an admitted packet actually fulfil rather than be
/// refused for want of sealing -- so "admitted" can be proved by the app
/// having been reached. [`sealed_prepare_to`] at this node's own priced
/// termination.
fn sealed_prepare(amount: u64) -> (Prepare, [u8; 32]) {
    sealed_prepare_to(payee_identity().as_ref(), "g.example.app", amount)
}

/// Carries this channel to cumulative 50 000 on a payee journaling to
/// `journal`, then drops that whole node -- the state a restarted payee
/// replays from. A FLUSH (§3): the claim header with an empty ILP body.
async fn journal_at_fifty_thousand(payer: &LocalSigner, journal: Arc<dyn Journal>) {
    let peer = accepting(
        payee_journaling_to(
            payer,
            priced_route(),
            Arc::new(FakeAppClient::new()),
            journal,
        ),
        bound_policy(),
    );
    let claim = sign_claim(payer, 1, 50_000);
    let response = peer
        .handle(request(Some(&claim_as_json(&claim, payer)), Vec::new()))
        .await;
    assert_eq!(
        ack_on(&response),
        Some(ClaimAckOutcome::Accepted),
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
    let peer = accepting(
        payee_journaling_to(
            &payer_signer,
            route,
            Arc::clone(&app_client),
            Arc::clone(&journal),
        ),
        bound_policy(),
    );

    let claim = sign_claim(&payer_signer, 2, 50_001); // advances 1, the price is 25
    let (prepare, _) = sealed_prepare(25);
    let response = peer
        .handle(request(
            Some(&claim_as_json(&claim, &payer_signer)),
            prepare.encode(),
        ))
        .await;

    assert_eq!(
        ack_on(&response),
        Some(ClaimAckOutcome::Accepted),
        "the claim itself is good -- its nonce and amount both advance the durable watermark, \
         which is why the book's verdict cannot catch this on its own"
    );
    let reject = connector_domain::Reject::decode(&response.body)
        .expect("a reject -- anything else means the app served this for free");
    assert_eq!(reject.code.as_str(), "F06");
    assert!(
        response.headers.get(PAYMENT_REQUIRED_HEADER).is_some(),
        "the x402 greeting rides it"
    );
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
    let peer = accepting(
        payee_journaling_to(
            &payer_signer,
            route,
            Arc::clone(&app_client),
            Arc::clone(&journal),
        ),
        bound_policy(),
    );

    let claim = sign_claim(&payer_signer, 2, 50_025); // advances exactly the price
    let (prepare, shared_secret) = sealed_prepare(25);
    let response = peer
        .handle(request(
            Some(&claim_as_json(&claim, &payer_signer)),
            prepare.encode(),
        ))
        .await;

    assert_eq!(ack_on(&response), Some(ClaimAckOutcome::Accepted));
    assert!(
        response.headers.get(PAYMENT_REQUIRED_HEADER).is_none(),
        "an admitted packet carries no greeting"
    );
    let fulfill = connector_domain::Fulfill::decode(&response.body).expect("a fulfil");
    let opened = connector_signer::giftwrap::open_response(&shared_secret, &fulfill.data)
        .expect("open the sealed fulfil");
    let opened = connector_domain::EnvelopeResponse::decode(&opened).expect("decode envelope");
    assert_eq!(opened.body, response_body);
    assert_eq!(app_client.deliveries().len(), 1);
}

/// PR #913 review finding: a claim signed by a non-counterparty key still
/// *decodes* and can declare any `cumulative_amount` it likes -- coverage
/// judged off that declared amount, ignoring the claim book's own verdict,
/// let an unlimited-value, never-verified claim buy service.
///
/// Since ADR 0060 that claim fails P3, so the request is a **client**
/// request and there is no `Toon-Claim-Ack` to carry `signature_invalid`
/// back: §1.7 forbids one on a client interaction, and the sender learns
/// from the greeting the client edge gives it, not from an ack. What this
/// test still pins is the finding itself -- the declared amount buys
/// nothing, and the app never sees the packet.
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
    let peer = accepting(connector, bound_policy());
    // Signed by an impostor, not the channel's configured counterparty --
    // declares far more than the route's price, but never verifies.
    let claim = sign_claim(&impostor, 1, 1_000_000);

    let response = peer
        .handle(request(
            Some(&claim_as_json(&claim, &impostor)),
            prepare("g.example.app").encode(),
        ))
        .await;

    assert!(
        ack_on(&response).is_none(),
        "a claim that fails P3 makes the request a client's, and a client \
         interaction never carries a claim-ack (§1.7)"
    );
    let reject = connector_domain::Reject::decode(&response.body).expect("a reject");
    assert_eq!(reject.code.as_str(), "F02");
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
    let peer = Arc::new(PeerHttpState::new(
        connector,
        bound_policy(),
        Arc::clone(&accepted),
        Arc::new(ClaimEnforcementPolicy::default()),
        Arc::new(FlushHints::new()),
        PeerHttpPolicy::default(),
    ));

    // A first claim at nonce 1 genuinely covers the price and advances the
    // watermark to 25 (a FLUSH: an empty body plus the claim header).
    let first = sign_claim(&payer_signer, 1, 25);
    let flush_response = peer
        .handle(request(
            Some(&claim_as_json(&first, &payer_signer)),
            Vec::new(),
        ))
        .await;
    assert_eq!(ack_on(&flush_response), Some(ClaimAckOutcome::Accepted));

    // Replayed at the same nonce, declaring an amount that would cover many
    // more packets if it were judged by amount alone.
    let replayed = sign_claim(&payer_signer, 1, 1_000_000);
    let replayed_json = claim_as_json(&replayed, &payer_signer);

    for attempt in 0..2 {
        let response = peer
            .handle(request(
                Some(&replayed_json),
                prepare("g.example.app").encode(),
            ))
            .await;

        assert_eq!(
            ack_on(&response),
            Some(ClaimAckOutcome::Rejected(
                ClaimRejectReason::NonceNotAdvancing
            )),
            "attempt {attempt}"
        );
        let reject = connector_domain::Reject::decode(&response.body).expect("a reject");
        assert_eq!(reject.code.as_str(), "F06", "attempt {attempt}");
        assert!(
            response.headers.get(PAYMENT_REQUIRED_HEADER).is_some(),
            "attempt {attempt}"
        );
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

// ─── ADR 0042 item 3: a forwarded arrival must cover its own `amount`,
// behind a per-peer knob that defaults to observing. The HTTP twins of the
// BTP carriage's own tests of the same names -- §0.1's one pipeline cannot
// admit over one carriage what it refuses over the other ───

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
/// rather than no claim at all: since ADR 0060 a claimless arrival is a
/// client's and never reaches this gate. `observe` still selects between
/// admitting and refusing everything else it always did.
#[tokio::test]
async fn a_forwarded_arrival_that_undercovers_is_admitted_by_default() {
    let payer_signer = LocalSigner::generate("payer");
    let (connector, next_hop_app, next_hop_identity) = forwarding_payee(&payer_signer);
    // The default policy: no entry for this peering at all, exactly as an
    // unconfigured `[[peers]]` row resolves.
    let peer = accepting(connector, bound_policy());
    let (sealed, shared_secret) = sealed_prepare_to(
        next_hop_identity.as_ref(),
        FORWARDED_DESTINATION,
        ARRIVING_AMOUNT,
    );
    let short = sign_claim(&payer_signer, 1, ARRIVING_AMOUNT - 1);

    let response = peer
        .handle(request(
            Some(&claim_as_json(&short, &payer_signer)),
            sealed.encode(),
        ))
        .await;

    assert_eq!(response.status, 200);
    assert!(
        response.headers.get(PAYMENT_REQUIRED_HEADER).is_none(),
        "an admitted packet carries no greeting"
    );
    let fulfill = connector_domain::Fulfill::decode(&response.body).expect("a fulfil");
    let opened = connector_signer::giftwrap::open_response(&shared_secret, &fulfill.data)
        .expect("open the sealed fulfil");
    let opened = connector_domain::EnvelopeResponse::decode(&opened).expect("decode envelope");
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
    let peer = accepting_with_enforcement(
        connector,
        bound_policy(),
        forwarded_enforcing(),
        Arc::new(FlushHints::new()),
    );
    let (sealed, _) = sealed_prepare_to(
        next_hop_identity.as_ref(),
        FORWARDED_DESTINATION,
        ARRIVING_AMOUNT,
    );
    let short = sign_claim(&payer_signer, 1, ARRIVING_AMOUNT - 1);

    let response = peer
        .handle(request(
            Some(&claim_as_json(&short, &payer_signer)),
            sealed.encode(),
        ))
        .await;

    assert_eq!(
        response.status, 200,
        "a packet verdict, not a transport 4xx (§6.2)"
    );
    let reject = connector_domain::Reject::decode(&response.body).expect("a reject");
    assert_eq!(reject.code.as_str(), "F06");
    let terms_header = response
        .headers
        .get(PAYMENT_REQUIRED_HEADER)
        .expect("the x402 greeting rode the response");
    let terms = parse_greeting(&base64_decode(terms_header)).expect("readable terms");
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
        let peer = accepting_with_enforcement(
            connector,
            bound_policy(),
            enforcement,
            Arc::new(FlushHints::new()),
        );
        let (sealed, shared_secret) = sealed_prepare_to(
            next_hop_identity.as_ref(),
            FORWARDED_DESTINATION,
            ARRIVING_AMOUNT,
        );
        let claim = sign_claim(&payer_signer, 1, ARRIVING_AMOUNT);

        let response = peer
            .handle(request(
                Some(&claim_as_json(&claim, &payer_signer)),
                sealed.encode(),
            ))
            .await;

        assert_eq!(ack_on(&response), Some(ClaimAckOutcome::Accepted));
        assert!(
            response.headers.get(PAYMENT_REQUIRED_HEADER).is_none(),
            "an admitted packet carries no greeting"
        );
        let fulfill = connector_domain::Fulfill::decode(&response.body).expect("a fulfil");
        let opened = connector_signer::giftwrap::open_response(&shared_secret, &fulfill.data)
            .expect("open the sealed fulfil");
        let opened = connector_domain::EnvelopeResponse::decode(&opened).expect("decode envelope");
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
        let peer = accepting_with_enforcement(
            connector,
            bound_policy(),
            forwarded_enforcing(),
            Arc::new(FlushHints::new()),
        );
        let (sealed, _) = sealed_prepare_to(
            next_hop_identity.as_ref(),
            FORWARDED_DESTINATION,
            ARRIVING_AMOUNT,
        );
        let claim = sign_claim(&payer_signer, 1, advance);

        let response = peer
            .handle(request(
                Some(&claim_as_json(&claim, &payer_signer)),
                sealed.encode(),
            ))
            .await;

        // The claim is perfectly valid and is still acknowledged: the two
        // verdicts stay independent (§6.2).
        assert_eq!(
            ack_on(&response),
            Some(ClaimAckOutcome::Accepted),
            "advance {advance}"
        );
        let reject = connector_domain::Reject::decode(&response.body).expect("a reject");
        assert_eq!(reject.code.as_str(), "F06", "advance {advance}");
        assert!(
            response.headers.get(PAYMENT_REQUIRED_HEADER).is_some(),
            "advance {advance}"
        );
        assert!(
            next_hop_app.deliveries().is_empty(),
            "advance {advance} was never carried"
        );
    }
}

/// ADR 0029's rule is **untouched** by ADR 0042, and since issue #1077 it
/// has no escape hatch at all: an arrival at a priced termination that does
/// not cover the route's price is refused under **every** setting a peering
/// can carry -- `forwarded_claim_enforcement` selects nothing here, because
/// it is the *forwarded* rule's knob. The HTTP twin of the BTP carriage's
/// own test of the same name.
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
                response: connector_domain::EnvelopeResponse {
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
        let peer = accepting_with_enforcement(
            connector,
            bound_policy(),
            Arc::new(ClaimEnforcementPolicy::of(vec![(PEER_ID, forwarded)])),
            Arc::new(FlushHints::new()),
        );
        let (sealed, _) = sealed_prepare_to(identity.as_ref(), "g.example.app", 25);
        let short = sign_claim(&payer_signer, 1, 1); // the price is 25

        let response = peer
            .handle(request(
                Some(&claim_as_json(&short, &payer_signer)),
                sealed.encode(),
            ))
            .await;

        assert!(
            connector_domain::Reject::decode(&response.body)
                .is_ok_and(|reject| reject.code.as_str() == "F06"),
            "forwarded_claim_enforcement = {forwarded}"
        );
        assert!(
            response.headers.get(PAYMENT_REQUIRED_HEADER).is_some(),
            "forwarded_claim_enforcement = {forwarded}"
        );
    }
}

// ─── §7.2: the claim race, and its mitigation ───

/// §7.2: **no more than one claim-bearing request in flight to a peer per
/// channel.** The race is a property of the carriage an operator chose, and
/// this is the normative mitigation the client edge already ships; without it
/// parallel requests at nonces *n* and *n+1* reach the payee's watermark lock
/// in either order and the loser is refused `nonce_not_advancing` for
/// nothing.
#[tokio::test]
async fn only_one_claim_bearing_request_is_in_flight_per_channel() {
    let payer_signer = LocalSigner::generate("payer");
    let peer = accepting(payee(&payer_signer), bound_policy());
    let client = Loopback::with_dwell(peer, Duration::from_millis(20));
    let transport = Arc::new(transport(
        Arc::clone(&client) as Arc<dyn PeerHttpClient>,
        &payer_signer,
    ));

    let flushes = (1..=4).map(|nonce| {
        let transport = Arc::clone(&transport);
        let claim = sign_claim(&payer_signer, nonce, nonce * 100);
        tokio::spawn(async move { transport.flush(PEER_ID, claim).await })
    });
    let acks: Vec<ClaimAckOutcome> = futures_join(flushes).await;

    assert_eq!(
        client.peak_concurrent_claims.load(Ordering::SeqCst),
        1,
        "two claim-bearing requests were in flight to one channel at once (§7.2)"
    );
    assert!(
        acks.iter().all(|ack| *ack == ClaimAckOutcome::Accepted),
        "serialized claims cannot race each other into nonce_not_advancing: {acks:?}"
    );
}

async fn futures_join<T>(handles: impl IntoIterator<Item = tokio::task::JoinHandle<T>>) -> Vec<T> {
    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await.expect("the task did not panic"));
    }
    results
}

fn base64_decode(value: &str) -> Vec<u8> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    STANDARD.decode(value).expect("standard base64")
}
