//! The client BTP websocket carriage, end-to-end (client-edge-spec.md §1.9,
//! ADR 0026): a real websocket session against a served router, since
//! `tower::oneshot` cannot carry an upgrade and none of these assertions can
//! be satisfied by anything short of the frames a real client sends and
//! receives. The client half here is written against §1.9's grammar
//! independently of the server's own codec -- the `@toon-protocol/client`
//! dialect, mirrored byte-for-byte -- so these tests hold the server to the
//! wire, not to itself.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use connector_client_edge::{
    ClientChannelRegistry, ClientClaimGate, DepositFloor, EvmChannel, SESSION_LEASE_BACKSTOP_TTL,
};
use connector_config::{StaticRoute, TransportPolicy};
use connector_domain::{EnvelopeRequest, EnvelopeResponse, Fulfill, Prepare, Reject};
use connector_runtime::{
    AppOutcome, Connector, FakeAppClient, InMemoryJournal, InProcessPeerTransport, TestClock,
};
use connector_signer::{
    derive_evm_address, evm_balance_proof_digest, to_hex, EvmBalanceProof, LocalSigner,
    PublicKeyBytes, Signer,
};
use futures_util::{SinkExt, StreamExt};
use hyper::{Body as HttpBody, Client as HttpClient, Request as HttpRequest, StatusCode};
use libsecp256k1::{Message as SecpMessage, PublicKey, SecretKey};
use tokio_tungstenite::tungstenite::Message as WsMessage;

const PRICE: u64 = 100;
const EVM_CHAIN_ID: u64 = 8453;
const EVM_TOKEN_NETWORK_ADDRESS: [u8; 20] = [0x42; 20];

// ─── client-side §1.9 frame grammar, written independently of the server ───

const BTP_RESPONSE: u8 = 1;
const BTP_ERROR: u8 = 2;
const BTP_MESSAGE: u8 = 6;
const BTP_TRANSFER: u8 = 7;

/// Append `protocolData count` + each entry, the layout `btp_message` and
/// `btp_transfer` both carry, the way `@toon-protocol/client`'s
/// `serializeBtpMessage` writes it.
fn write_protocol_data(out: &mut Vec<u8>, protocol_data: &[(&str, &[u8])]) {
    out.push(protocol_data.len() as u8);
    for (name, data) in protocol_data {
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&1u16.to_be_bytes()); // contentType, as the client sends
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(data);
    }
}

/// Serialize a MESSAGE the way `@toon-protocol/client`'s
/// `serializeBtpMessage` does.
fn btp_message(request_id: u32, protocol_data: &[(&str, &[u8])], ilp_packet: &[u8]) -> Vec<u8> {
    let mut out = vec![BTP_MESSAGE];
    out.extend_from_slice(&request_id.to_be_bytes());
    write_protocol_data(&mut out, protocol_data);
    out.extend_from_slice(&(ilp_packet.len() as u32).to_be_bytes());
    out.extend_from_slice(ilp_packet);
    out
}

/// Serialize a TRANSFER (issue #697, RFC-0023 `Transfer ::= SEQUENCE {
/// amount, protocolData }`): amount immediately after requestId, then the
/// protocolData list, with no ILP-packet trailer.
fn btp_transfer(request_id: u32, amount: u64, protocol_data: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = vec![BTP_TRANSFER];
    out.extend_from_slice(&request_id.to_be_bytes());
    out.extend_from_slice(&amount.to_be_bytes());
    write_protocol_data(&mut out, protocol_data);
    out
}

/// Serialize a bare RESPONSE frame -- what a client answers a
/// server-originated MESSAGE with. A client is also free to send a
/// RESPONSE/ERROR the connector never asked for, and the session must not
/// choke on it.
fn btp_response(request_id: u32) -> Vec<u8> {
    btp_response_with_packet(request_id, &[])
}

/// As [`btp_response`], but carrying `ilp_packet` -- what a session-bound
/// client answers a server-originated PREPARE with (issue #736): a FULFILL
/// or REJECT, OER-encoded, exactly as it would answer any other PREPARE.
fn btp_response_with_packet(request_id: u32, ilp_packet: &[u8]) -> Vec<u8> {
    let mut out = vec![BTP_RESPONSE];
    out.extend_from_slice(&request_id.to_be_bytes());
    out.push(0); // no protocolData
    out.extend_from_slice(&(ilp_packet.len() as u32).to_be_bytes());
    out.extend_from_slice(ilp_packet);
    out
}

/// A parsed server answer: `(type, requestId, protocolData, ilpPacket)`,
/// parsed the way the client's `parseBtpMessage` parses it.
struct Answer {
    frame_type: u8,
    request_id: u32,
    protocol_data: Vec<(String, Vec<u8>)>,
    ilp_packet: Vec<u8>,
}

fn parse_answer(buf: &[u8]) -> Answer {
    assert!(buf.len() >= 5, "answer shorter than a frame header");
    let frame_type = buf[0];
    let request_id = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]);
    if frame_type == BTP_ERROR {
        // code / name / triggeredAt as 1-byte-length strings, then u32 data.
        let mut pos = 5;
        let mut fields = Vec::new();
        for _ in 0..3 {
            let len = usize::from(buf[pos]);
            pos += 1;
            fields.push(String::from_utf8(buf[pos..pos + len].to_vec()).unwrap());
            pos += len;
        }
        let data_len = u32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
        pos += 4;
        let data = buf[pos..pos + data_len as usize].to_vec();
        return Answer {
            frame_type,
            request_id,
            protocol_data: vec![
                ("code".to_string(), fields[0].clone().into_bytes()),
                ("name".to_string(), fields[1].clone().into_bytes()),
            ],
            ilp_packet: data,
        };
    }
    let mut pos = 5;
    let count = usize::from(buf[pos]);
    pos += 1;
    let mut protocol_data = Vec::new();
    for _ in 0..count {
        let name_len = usize::from(buf[pos]);
        pos += 1;
        let name = String::from_utf8(buf[pos..pos + name_len].to_vec()).unwrap();
        pos += name_len;
        pos += 2; // contentType
        let data_len =
            u32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
        pos += 4;
        let data = buf[pos..pos + data_len].to_vec();
        pos += data_len;
        protocol_data.push((name, data));
    }
    let ilp_len = u32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
    pos += 4;
    Answer {
        frame_type,
        request_id,
        protocol_data: protocol_data.clone(),
        ilp_packet: buf[pos..pos + ilp_len].to_vec(),
    }
}

fn pd<'a>(answer: &'a Answer, name: &str) -> Option<&'a [u8]> {
    answer
        .protocol_data
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, d)| d.as_slice())
}

// ─── server + wallet fixtures (the shapes `claim_gate`'s own tests use) ───

fn evm_signer() -> (SecretKey, connector_signer::Address) {
    let secret = SecretKey::parse(&[9u8; 32]).unwrap();
    let public = PublicKey::from_secret_key(&secret);
    (secret, derive_evm_address(&public.serialize()))
}

fn channel_hex() -> String {
    "ab".repeat(32)
}

/// An EVM claim JSON with a genuine EIP-712 signature over its own fields,
/// exactly the JSON `JSON.stringify(claim)` produces client-side -- raw,
/// not base64: §1.9's protocolData carriage.
fn evm_claim_json(nonce: u64, transferred_amount: u64) -> String {
    let (secret, address) = evm_signer();
    let mut channel_id = [0u8; 32];
    hex::decode_to_slice(channel_hex(), &mut channel_id).unwrap();
    let proof = EvmBalanceProof {
        channel_id,
        nonce,
        transferred_amount: u128::from(transferred_amount),
        locked_amount: 0,
        locks_root: [0u8; 32],
        chain_id: EVM_CHAIN_ID,
        token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
    };
    let digest = evm_balance_proof_digest(&proof);
    let (signature, recovery_id) = libsecp256k1::sign(&SecpMessage::parse(&digest), &secret);
    let mut sig_bytes = signature.serialize().to_vec();
    let recovery_byte: u8 = recovery_id.into();
    sig_bytes.push(recovery_byte + 27);
    format!(
        r#"{{"version":"1.0","blockchain":"evm","messageId":"msg-{nonce}","timestamp":"2026-02-02T12:00:00.000Z","senderId":"btp-test","channelId":"0x{channel}","nonce":{nonce},"transferredAmount":"{transferred_amount}","lockedAmount":"0","locksRoot":"0x{zeros}","signature":"0x{signature}","signerAddress":"{address}","chainId":{EVM_CHAIN_ID},"tokenNetworkAddress":"{token_network}"}}"#,
        channel = channel_hex(),
        zeros = "0".repeat(64),
        signature = hex::encode(&sig_bytes),
        address = to_hex(&address),
        token_network = to_hex(&EVM_TOKEN_NETWORK_ADDRESS),
    )
}

fn test_channels() -> ClientChannelRegistry {
    let (_secret, counterparty) = evm_signer();
    let mut channels = ClientChannelRegistry::new();
    channels
        .record_evm(
            &channel_hex(),
            EvmChannel {
                counterparty,
                chain_id: EVM_CHAIN_ID,
                token_network_address: EVM_TOKEN_NETWORK_ADDRESS,
                deposit_floor: DepositFloor::Unknown,
            },
        )
        .expect("a 32-byte hex channel id");
    channels
}

fn test_clock() -> Arc<TestClock> {
    Arc::new(TestClock::new(
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
    ))
}

/// A PREPARE whose `data` is sealed to `receiver_public` (ADR 0018) -- a
/// packet the termination genuinely fulfils, deriving its fulfilment from
/// this same sealed secret (ADR 0019).
fn sealed_prepare(destination: &str, receiver_public: &PublicKeyBytes) -> Prepare {
    sealed_prepare_with_target(destination, "/", receiver_public)
}

/// As [`sealed_prepare`], but with the envelope's own `target` (issue
/// #596/#869) set to `target` rather than hard-coded to `"/"` -- for a test
/// asserting on what happens when that target does, or does not, resolve
/// under the matched route's handler path.
fn sealed_prepare_with_target(
    destination: &str,
    target: &str,
    receiver_public: &PublicKeyBytes,
) -> Prepare {
    let envelope = EnvelopeRequest {
        method: "POST".to_string(),
        target: target.to_string(),
        headers: vec![],
        body: b"hello app over btp".to_vec(),
    }
    .encode();
    let (data, _shared_secret) =
        connector_signer::giftwrap::seal_request(&envelope, receiver_public).expect("seal");
    Prepare {
        amount: PRICE,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: destination.to_string(),
        data,
    }
}

/// Serve a priced-route edge on an ephemeral port; returns the address and
/// the signer whose key prepares must seal to.
async fn serve_edge() -> (SocketAddr, Arc<dyn Signer>) {
    let route = StaticRoute::new_priced("g.test.app", "http://localhost:4000", PRICE).unwrap();
    serve_edge_with_route(route, Arc::new(FakeAppClient::new())).await
}

/// As [`serve_edge`], but for a caller that wants to inspect the app
/// client's deliveries -- `AppClient::deliver` was never called, or was.
async fn serve_edge_with_route(
    route: StaticRoute,
    app_client: Arc<FakeAppClient>,
) -> (SocketAddr, Arc<dyn Signer>) {
    app_client.respond(
        route.handler_url(),
        AppOutcome::Answered {
            response: EnvelopeResponse {
                status: 200,
                headers: vec![],
                body: b"app said yes".to_vec(),
            },
        },
    );
    let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("btp-session-test"));
    let connector = Arc::new(
        Connector::new(
            vec![route],
            vec![],
            app_client,
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        )
        .with_identity_signer(signer.clone()),
    );
    let gate = ClientClaimGate::restore(test_channels(), Arc::new(InMemoryJournal::new()))
        .expect("a fresh in-memory journal has nothing to replay");
    let app = connector_client_edge::router_with_gate(connector, signer.clone(), None, gate);
    let server = axum::Server::bind(&"127.0.0.1:0".parse().unwrap()).serve(app.into_make_service());
    let addr = server.local_addr();
    tokio::spawn(server);
    (addr, signer)
}

type Session =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(addr: SocketAddr) -> Session {
    let (session, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/ilp/btp"))
        .await
        .expect("the upgrade succeeds");
    session
}

async fn next_answer(session: &mut Session) -> Answer {
    loop {
        let message = session
            .next()
            .await
            .expect("the session stays open")
            .expect("a websocket frame");
        if let WsMessage::Binary(bytes) = message {
            return parse_answer(&bytes);
        }
    }
}

async fn send(session: &mut Session, frame: Vec<u8>) {
    session
        .send(WsMessage::Binary(frame))
        .await
        .expect("the frame sends");
}

// ─── the tests ───

/// §1.9 steps 1 and 2, and the ordering contract in one session: auth is
/// acknowledged, and five paid writes pipelined back-to-back -- their claims
/// sent in nonce order on the one socket, none waiting for the previous
/// response -- all fulfil. On the HTTP carriage this exact traffic can race
/// itself into `NonceNotAdvancing`; the in-order claim admission is the fix,
/// and this assertion is the transport's reason to exist. Responses
/// correlate by requestId and may return in any order (issue #688) -- the
/// dialect's own contract, and what the deployed client's pendingRequests
/// map implements -- so the assertion is on the *set* of ids answered, and
/// on every strictly-advancing claim fulfilling, never on arrival order.
#[tokio::test(flavor = "multi_thread")]
async fn an_authenticated_session_pipelines_paid_writes_in_claim_order() {
    let (addr, signer) = serve_edge().await;
    let mut session = connect(addr).await;

    send(
        &mut session,
        btp_message(1, &[("auth", br#"{"peerId":"p","secret":""}"#)], &[]),
    )
    .await;
    let auth = next_answer(&mut session).await;
    assert_eq!(auth.frame_type, BTP_RESPONSE);
    assert_eq!(auth.request_id, 1);

    let receiver = signer.public_key().unwrap();
    // All five frames written before any response is read.
    for nonce in 1..=5u64 {
        let claim = evm_claim_json(nonce, nonce * PRICE);
        let prepare = sealed_prepare("g.test.app", &receiver);
        send(
            &mut session,
            btp_message(
                10 + nonce as u32,
                &[("payment-channel-claim", claim.as_bytes())],
                &prepare.encode(),
            ),
        )
        .await;
    }
    let mut answered = std::collections::HashSet::new();
    for _ in 1..=5u64 {
        let answer = next_answer(&mut session).await;
        assert_eq!(answer.frame_type, BTP_RESPONSE);
        let fulfill = Fulfill::decode(&answer.ilp_packet).unwrap_or_else(|_| {
            let reject = Reject::decode(&answer.ilp_packet).expect("an OER packet");
            panic!(
                "write {} was refused {} {:?} instead of fulfilling",
                answer.request_id,
                reject.code.as_str(),
                reject.message
            );
        });
        assert!(!fulfill.data.is_empty(), "a sealed answer rides home");
        assert!(
            answered.insert(answer.request_id),
            "each request is answered exactly once"
        );
    }
    assert_eq!(
        answered,
        (11..=15u32).collect(),
        "every pipelined write was answered, whatever the completion order"
    );
}

/// §1.9 step 3: the x402 greeting, BTP-shaped -- an F06 REJECT whose
/// `payment-required` protocolData is the same terms JSON the HTTP 402
/// serves, with the running cost total riding as `toon-accumulated-cost`.
#[tokio::test(flavor = "multi_thread")]
async fn a_claimless_prepare_to_a_priced_route_is_refused_with_the_terms() {
    let (addr, signer) = serve_edge().await;
    let mut session = connect(addr).await;

    let prepare = sealed_prepare("g.test.app", &signer.public_key().unwrap());
    send(&mut session, btp_message(2, &[], &prepare.encode())).await;

    let answer = next_answer(&mut session).await;
    assert_eq!(answer.frame_type, BTP_RESPONSE);
    assert_eq!(answer.request_id, 2);
    let reject = Reject::decode(&answer.ilp_packet).expect("an OER REJECT");
    assert_eq!(reject.code.as_str(), "F06");
    assert_eq!(reject.message, "No payment channel claim attached");
    assert_eq!(pd(&answer, "toon-accumulated-cost"), Some(b"0".as_slice()));
    let terms: serde_json::Value =
        serde_json::from_slice(pd(&answer, "payment-required").expect("the terms ride along"))
            .expect("the terms are the §1.4 JSON");
    assert_eq!(terms["x402Version"], 2);
    assert_eq!(terms["accepts"][0]["amount"], PRICE.to_string());
    // Issue #722: the same greeting also carries the session lease backstop
    // TTL the client session registry actually enforces, over BTP exactly
    // as over HTTP -- both carriages share `x402_terms_body`.
    assert_eq!(
        terms["accepts"][0]["extra"]["sessionLeaseTtlMs"],
        SESSION_LEASE_BACKSTOP_TTL.as_millis() as u64
    );
}

/// Issue #874: the other half of the greeting above -- a connector that
/// **dials** an edge like this one reads the terms back off the very bytes
/// it just emitted. Round-tripped through the real emitter on purpose:
/// `connector-peer-btp`'s reader and this edge's writer share one wire type
/// (`connector_domain::x402`), and this is the test that fails if they ever
/// stop agreeing -- a fixture copied into the reader's own unit tests could
/// not tell.
#[tokio::test(flavor = "multi_thread")]
async fn a_dialing_peer_reads_the_terms_off_the_greeting_the_edge_emits() {
    let (addr, signer) = serve_edge().await;
    let mut session = connect(addr).await;

    let prepare = sealed_prepare("g.test.app", &signer.public_key().unwrap());
    send(&mut session, btp_message(2, &[], &prepare.encode())).await;
    let answer = next_answer(&mut session).await;

    // The frame as the dialing side receives it, entries and all -- not a
    // hand-picked field lifted out of it.
    let frame = connector_btp::BtpFrame {
        frame_type: answer.frame_type,
        request_id: answer.request_id,
        amount: None,
        protocol_data: answer
            .protocol_data
            .iter()
            .map(|(name, data)| connector_btp::ProtocolData {
                name: name.clone(),
                content_type: connector_btp::CONTENT_TYPE_TEXT,
                data: data.clone(),
            })
            .collect(),
        ilp_packet: answer.ilp_packet.clone(),
    };

    let Some(connector_peer_btp::PeerAnswer::PaymentRequired { reject, terms }) =
        connector_peer_btp::decode_answer(&frame)
    else {
        panic!("a claimless dial must learn the terms, not an opaque refusal");
    };
    assert_eq!(reject.code.as_str(), "F06");
    assert_eq!(
        terms.price(),
        Some(PRICE),
        "the price the dialer must cover is the one this route charges"
    );
    assert_eq!(terms.pay_to(), Some("g.test.app"));
    assert_eq!(terms.required_transport(), None);
    assert_eq!(
        terms.offer().unwrap().extra.session_lease_ttl_ms,
        SESSION_LEASE_BACKSTOP_TTL.as_millis() as u64
    );
}

/// The greeting reused for issue #701's wrong-transport refusal is read the
/// same way, so a dialer that picked the wrong carriage learns which one to
/// use instead of guessing.
#[tokio::test(flavor = "multi_thread")]
async fn a_dialing_peer_reads_the_required_transport_off_the_same_greeting() {
    let route = StaticRoute::new_priced_with_transport(
        "g.test.app",
        "http://localhost:4000",
        PRICE,
        TransportPolicy::Http,
    )
    .unwrap();
    let (addr, signer) = serve_edge_with_route(route, Arc::new(FakeAppClient::new())).await;
    let mut session = connect(addr).await;

    let prepare = sealed_prepare("g.test.app", &signer.public_key().unwrap());
    send(&mut session, btp_message(2, &[], &prepare.encode())).await;
    let answer = next_answer(&mut session).await;

    let greeting = pd(&answer, "payment-required").expect("the terms ride along");
    let entries = vec![connector_btp::ProtocolData {
        name: "payment-required".to_string(),
        content_type: connector_btp::CONTENT_TYPE_TEXT,
        data: greeting.to_vec(),
    }];
    let terms = connector_peer_btp::fields::payment_required(&entries)
        .expect("the entry is there")
        .expect("and the shared wire type reads it");
    assert_eq!(terms.required_transport(), Some("http"));
}

/// Issue #701 (toon-meta#262 decision 11): a route restricted to HTTP
/// refuses a PREPARE arriving over this BTP session -- F02 (Unreachable,
/// from this carriage's own point of view), with the same terms JSON the
/// F06 greeting above carries, self-diagnosing via `extra.requiredTransport`
/// rather than a bare reject. This fires even though the PREPARE carries no
/// claim at all -- transport is checked before payment is considered.
#[tokio::test(flavor = "multi_thread")]
async fn a_prepare_to_an_http_only_route_is_refused_over_btp_with_the_required_transport() {
    let route = StaticRoute::new_priced_with_transport(
        "g.test.app",
        "http://localhost:4000",
        PRICE,
        TransportPolicy::Http,
    )
    .unwrap();
    let app_client = Arc::new(FakeAppClient::new());
    let (addr, signer) = serve_edge_with_route(route, app_client.clone()).await;
    let mut session = connect(addr).await;

    let prepare = sealed_prepare("g.test.app", &signer.public_key().unwrap());
    send(&mut session, btp_message(2, &[], &prepare.encode())).await;

    let answer = next_answer(&mut session).await;
    assert_eq!(answer.frame_type, BTP_RESPONSE);
    assert_eq!(answer.request_id, 2);
    let reject = Reject::decode(&answer.ilp_packet).expect("an OER REJECT");
    assert_eq!(reject.code.as_str(), "F02");
    let terms: serde_json::Value =
        serde_json::from_slice(pd(&answer, "payment-required").expect("the terms ride along"))
            .expect("the terms are the §1.4 JSON, reused");
    assert_eq!(terms["accepts"][0]["extra"]["requiredTransport"], "http");

    assert!(
        app_client.deliveries().is_empty(),
        "a request over the wrong transport must never reach the app"
    );
}

/// The mirror case: a claim that would otherwise fully pay for the route
/// does not make an HTTP-only route reachable over BTP either -- paying
/// over the wrong transport does not fix it.
#[tokio::test(flavor = "multi_thread")]
async fn a_paid_prepare_to_an_http_only_route_is_still_refused_over_btp() {
    let route = StaticRoute::new_priced_with_transport(
        "g.test.app",
        "http://localhost:4000",
        PRICE,
        TransportPolicy::Http,
    )
    .unwrap();
    let app_client = Arc::new(FakeAppClient::new());
    let (addr, signer) = serve_edge_with_route(route, app_client.clone()).await;
    let mut session = connect(addr).await;

    let claim = evm_claim_json(1, PRICE);
    let prepare = sealed_prepare("g.test.app", &signer.public_key().unwrap());
    send(
        &mut session,
        btp_message(
            2,
            &[("payment-channel-claim", claim.as_bytes())],
            &prepare.encode(),
        ),
    )
    .await;

    let answer = next_answer(&mut session).await;
    assert_eq!(answer.frame_type, BTP_RESPONSE);
    let reject = Reject::decode(&answer.ilp_packet).expect("an OER REJECT");
    assert_eq!(reject.code.as_str(), "F02");

    assert!(
        app_client.deliveries().is_empty(),
        "a valid claim over the wrong transport must never reach the app"
    );
}

/// §1.3 over the new carriage: a non-advancing nonce is refused exactly as
/// HTTP refuses it (F01, the shared taxonomy) -- same gate, same watermark,
/// so the first claim's nonce is spent for both carriages at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_claim_is_refused_with_the_http_taxonomy() {
    let (addr, signer) = serve_edge().await;
    let mut session = connect(addr).await;
    let receiver = signer.public_key().unwrap();

    let claim = evm_claim_json(1, PRICE);
    send(
        &mut session,
        btp_message(
            1,
            &[("payment-channel-claim", claim.as_bytes())],
            &sealed_prepare("g.test.app", &receiver).encode(),
        ),
    )
    .await;
    let first = next_answer(&mut session).await;
    Fulfill::decode(&first.ilp_packet).expect("the fresh claim pays");

    // The same claim again: structurally and cryptographically fine, its
    // nonce simply does not advance.
    send(
        &mut session,
        btp_message(
            2,
            &[("payment-channel-claim", claim.as_bytes())],
            &sealed_prepare("g.test.app", &receiver).encode(),
        ),
    )
    .await;
    let second = next_answer(&mut session).await;
    let reject = Reject::decode(&second.ilp_packet).expect("an OER REJECT");
    assert_eq!(reject.code.as_str(), "F01");
    assert_eq!(pd(&second, "toon-accumulated-cost"), Some(b"0".as_slice()));
}

/// Issue #869, BTP-shaped: a packet refused for its envelope's own target
/// shape (F00) must never advance the covering claim's watermark, on this
/// carriage exactly as on HTTP (§9's no-drift invariant). Proven the same
/// way [`a_replayed_claim_is_refused_with_the_http_taxonomy`] proves the
/// opposite direction: the identical claim, resent with a target that
/// resolves cleanly, still pays -- only possible if the first, refused
/// attempt left the watermark untouched.
#[tokio::test(flavor = "multi_thread")]
async fn a_claim_covering_a_packet_refused_for_envelope_shape_is_never_spent_over_btp() {
    let (addr, signer) = serve_edge().await;
    let mut session = connect(addr).await;
    let receiver = signer.public_key().unwrap();
    let claim = evm_claim_json(1, PRICE);

    send(
        &mut session,
        btp_message(
            1,
            &[("payment-channel-claim", claim.as_bytes())],
            &sealed_prepare_with_target("g.test.app", "/write", &receiver).encode(),
        ),
    )
    .await;
    let first = next_answer(&mut session).await;
    let reject = Reject::decode(&first.ilp_packet).expect("an OER REJECT");
    assert_eq!(reject.code.as_str(), "F00");
    assert_eq!(pd(&first, "toon-accumulated-cost"), Some(b"0".as_slice()));

    // The identical claim, resent with a target that resolves cleanly,
    // must still be accepted.
    send(
        &mut session,
        btp_message(
            2,
            &[("payment-channel-claim", claim.as_bytes())],
            &sealed_prepare("g.test.app", &receiver).encode(),
        ),
    )
    .await;
    let second = next_answer(&mut session).await;
    Fulfill::decode(&second.ilp_packet).expect("the unspent claim is still accepted");
}

/// §1.9 step 4: a standalone claim is ingested fire-and-forget -- no
/// response frame -- and genuinely advances the shared watermark: the auth
/// frame sent after it is answered first (nothing answered the claim), and
/// a following paid write must present nonce 2, not 1.
#[tokio::test(flavor = "multi_thread")]
async fn a_standalone_claim_registers_silently_and_advances_the_watermark() {
    let (addr, signer) = serve_edge().await;
    let mut session = connect(addr).await;

    let claim = evm_claim_json(1, PRICE);
    send(
        &mut session,
        btp_message(1, &[("payment-channel-claim", claim.as_bytes())], &[]),
    )
    .await;
    send(
        &mut session,
        btp_message(2, &[("auth", br#"{"peerId":"p","secret":""}"#)], &[]),
    )
    .await;
    let answer = next_answer(&mut session).await;
    assert_eq!(
        answer.request_id, 2,
        "the standalone claim is answered with nothing; the auth ack is the first frame back"
    );

    // Nonce 1 is spent: a packet claim reusing it is refused, and nonce 2
    // fulfils -- the fire-and-forget claim reached the same watermark.
    let receiver = signer.public_key().unwrap();
    let replay = evm_claim_json(1, PRICE);
    send(
        &mut session,
        btp_message(
            3,
            &[("payment-channel-claim", replay.as_bytes())],
            &sealed_prepare("g.test.app", &receiver).encode(),
        ),
    )
    .await;
    let refused = next_answer(&mut session).await;
    let reject = Reject::decode(&refused.ilp_packet).expect("an OER REJECT");
    assert_eq!(reject.code.as_str(), "F01");

    let fresh = evm_claim_json(2, 2 * PRICE);
    send(
        &mut session,
        btp_message(
            4,
            &[("payment-channel-claim", fresh.as_bytes())],
            &sealed_prepare("g.test.app", &receiver).encode(),
        ),
    )
    .await;
    let fulfilled = next_answer(&mut session).await;
    Fulfill::decode(&fulfilled.ilp_packet).expect("nonce 2 pays");
}

/// §1.9 step 5: an undecodable frame whose requestId was readable is
/// answered with an ERROR frame carrying the parse failure, and the session
/// survives to serve the next frame.
#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_frame_is_answered_with_an_error_and_the_session_survives() {
    let (addr, _signer) = serve_edge().await;
    let mut session = connect(addr).await;

    // Claims one protocolData entry, provides nothing after the count.
    send(&mut session, vec![6, 0, 0, 0, 9, 1]).await;
    let error = next_answer(&mut session).await;
    assert_eq!(error.frame_type, BTP_ERROR);
    assert_eq!(error.request_id, 9);
    assert_eq!(pd(&error, "code"), Some(b"F00".as_slice()));

    send(
        &mut session,
        btp_message(10, &[("auth", br#"{"peerId":"p","secret":""}"#)], &[]),
    )
    .await;
    let auth = next_answer(&mut session).await;
    assert_eq!(auth.frame_type, BTP_RESPONSE);
    assert_eq!(auth.request_id, 10);
}

// ─── issue #697: RFC-0023's symmetric grammar -- TRANSFER, and tolerance
// of an unsolicited RESPONSE/ERROR ───

/// A client-originated TRANSFER is answered with an empty RESPONSE under
/// the same requestId -- RFC-23's "the responder should always send back a
/// response to a request with the same requestId", satisfied at the
/// protocol level. The settlement/netting accounting this frame will
/// eventually drive is a separate ticket (toon-meta#262's payout ledger);
/// this connector acknowledges receipt today and nothing more.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_originated_transfer_is_acknowledged_with_an_empty_response() {
    let (addr, _signer) = serve_edge().await;
    let mut session = connect(addr).await;

    send(
        &mut session,
        btp_transfer(1, 250_000, &[("payout-claim", b"{}")]),
    )
    .await;
    let answer = next_answer(&mut session).await;
    assert_eq!(answer.frame_type, BTP_RESPONSE);
    assert_eq!(answer.request_id, 1);
    assert!(answer.protocol_data.is_empty());
    assert!(answer.ilp_packet.is_empty());

    // The session survives and keeps serving ordinary traffic afterward.
    send(
        &mut session,
        btp_message(2, &[("auth", br#"{"peerId":"p","secret":""}"#)], &[]),
    )
    .await;
    let auth = next_answer(&mut session).await;
    assert_eq!(auth.frame_type, BTP_RESPONSE);
    assert_eq!(auth.request_id, 2);
}

/// A TRANSFER with no protocolData at all -- the minimal legal frame --
/// still gets acknowledged rather than treated as malformed.
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_transfer_is_still_acknowledged() {
    let (addr, _signer) = serve_edge().await;
    let mut session = connect(addr).await;

    send(&mut session, btp_transfer(7, 0, &[])).await;
    let answer = next_answer(&mut session).await;
    assert_eq!(answer.frame_type, BTP_RESPONSE);
    assert_eq!(answer.request_id, 7);
}

/// A RESPONSE the connector never asked for (this dialect "never
/// originates a requestId" today over a real socket -- issue #697's
/// session registry is a separate, later ticket) answers nothing and does
/// not disturb the session: byte-identical to the pre-#697 behavior where
/// any non-MESSAGE frame was simply dropped.
#[tokio::test(flavor = "multi_thread")]
async fn an_unsolicited_response_is_silently_dropped_and_the_session_survives() {
    let (addr, _signer) = serve_edge().await;
    let mut session = connect(addr).await;

    send(&mut session, btp_response(99)).await;
    send(
        &mut session,
        btp_message(1, &[("auth", br#"{"peerId":"p","secret":""}"#)], &[]),
    )
    .await;
    let auth = next_answer(&mut session).await;
    assert_eq!(
        auth.request_id, 1,
        "the stray RESPONSE produced no answer of its own"
    );
}

// ─── issue #736: a bound BTP client session answers a PREPARE addressed to it ───

/// The exact scenario issue #736 reports: a provider's BTP session is bound
/// under its own address, and a buyer's PREPARE addressed there -- sent over
/// the *other* carriage, `POST /ilp` -- is delivered through that session
/// and fulfilled, rather than F02ing at the router because
/// `Connector::handle_prepare` has no fourth arm for a live session.
#[tokio::test(flavor = "multi_thread")]
async fn a_prepare_over_http_addressed_to_a_bound_btp_session_is_delivered_and_fulfilled() {
    let (addr, _signer) = serve_edge().await;

    // The provider connects over BTP and binds "g.toon.provider" as its
    // own address -- issue #698's session registry, exercised for real.
    let mut provider = connect(addr).await;
    send(
        &mut provider,
        btp_message(
            1,
            &[("auth", br#"{"peerId":"g.toon.provider","secret":""}"#)],
            &[],
        ),
    )
    .await;
    let ack = next_answer(&mut provider).await;
    assert_eq!(ack.frame_type, BTP_RESPONSE);

    // The buyer's PREPARE, over the HTTP carriage entirely -- no static
    // route names "g.toon.provider" and no claim is required, since a
    // dynamic session address like this is unpriced today (issue #736's
    // charging AC: only a *configured* route is ever priced).
    let fulfillment = [42u8; 32];
    let prepare = Prepare {
        amount: 0,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: "g.toon.provider".to_string(),
        data: Vec::new(),
    };
    let body = prepare.encode();

    let buyer = tokio::spawn(async move {
        let client = HttpClient::new();
        let request = HttpRequest::builder()
            .method("POST")
            .uri(format!("http://{addr}/ilp"))
            .body(HttpBody::from(body))
            .expect("well-formed request");
        let response = client
            .request(request)
            .await
            .expect("the connector answers");
        assert_eq!(response.status(), StatusCode::OK);
        hyper::body::to_bytes(response.into_body())
            .await
            .expect("a response body")
    });

    // The provider's own socket receives the forwarded PREPARE as a
    // server-originated MESSAGE (issue #697's symmetric grammar) and
    // answers it exactly as it would answer any other PREPARE: a FULFILL
    // that rides straight home, unchecked (issue #1269 / ADR 0069).
    let forwarded = next_answer(&mut provider).await;
    assert_eq!(forwarded.frame_type, BTP_MESSAGE);
    let forwarded_prepare =
        Prepare::decode(&forwarded.ilp_packet).expect("the connector forwards a real PREPARE");
    assert_eq!(forwarded_prepare.destination, "g.toon.provider");

    send(
        &mut provider,
        btp_response_with_packet(
            forwarded.request_id,
            &Fulfill {
                fulfillment,
                data: Vec::new(),
            }
            .encode(),
        ),
    )
    .await;

    let answer_body = buyer.await.expect("the buyer's request task");
    let fulfill = Fulfill::decode(&answer_body).expect("decode fulfill");
    assert_eq!(fulfill.fulfillment, fulfillment);
}

/// The mirror of the test above, entirely over BTP: both the provider's
/// bound session and the buyer's PREPARE ride the same carriage, on two
/// separate sockets -- session registration is per-address, not per-socket
/// pair.
#[tokio::test(flavor = "multi_thread")]
async fn a_prepare_over_btp_addressed_to_a_bound_btp_session_is_delivered_and_fulfilled() {
    let (addr, _signer) = serve_edge().await;

    let mut provider = connect(addr).await;
    send(
        &mut provider,
        btp_message(
            1,
            &[("auth", br#"{"peerId":"g.toon.other-provider","secret":""}"#)],
            &[],
        ),
    )
    .await;
    let ack = next_answer(&mut provider).await;
    assert_eq!(ack.frame_type, BTP_RESPONSE);

    let mut buyer = connect(addr).await;
    let fulfillment = [43u8; 32];
    let prepare = Prepare {
        amount: 0,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: "g.toon.other-provider".to_string(),
        data: Vec::new(),
    };
    send(&mut buyer, btp_message(1, &[], &prepare.encode())).await;

    let forwarded = next_answer(&mut provider).await;
    assert_eq!(forwarded.frame_type, BTP_MESSAGE);
    send(
        &mut provider,
        btp_response_with_packet(
            forwarded.request_id,
            &Fulfill {
                fulfillment,
                data: Vec::new(),
            }
            .encode(),
        ),
    )
    .await;

    let answer = next_answer(&mut buyer).await;
    assert_eq!(answer.frame_type, BTP_RESPONSE);
    assert_eq!(answer.request_id, 1);
    let fulfill = Fulfill::decode(&answer.ilp_packet).expect("decode fulfill");
    assert_eq!(fulfill.fulfillment, fulfillment);
}

/// A destination that has never had a session bound to it, and matches no
/// configured route either, still answers `F02` -- issue #736's "matches
/// nothing at all" case, proven over a real BTP session rather than only at
/// the router.
#[tokio::test(flavor = "multi_thread")]
async fn a_prepare_to_a_never_bound_destination_still_answers_f02_over_btp() {
    let (addr, _signer) = serve_edge().await;
    let mut session = connect(addr).await;

    let prepare = Prepare {
        amount: 0,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: "g.toon.nobody-ever-bound-this".to_string(),
        data: Vec::new(),
    };
    send(&mut session, btp_message(1, &[], &prepare.encode())).await;

    let answer = next_answer(&mut session).await;
    assert_eq!(answer.frame_type, BTP_RESPONSE);
    let reject = Reject::decode(&answer.ilp_packet).expect("decode reject");
    assert_eq!(reject.code.as_str(), "F02");
}

// ─── issue #688: the pipelined session's throughput, measured for real ───

/// An app that takes a fixed, real amount of time to answer -- ADR 0007's
/// fake, not a mock: a relay POST has a round-trip, and the in-flight
/// window exists precisely because it does. Wraps the same
/// [`FakeAppClient`] every other test uses; the delay is the only behavior
/// added.
struct SlowApp {
    inner: Arc<FakeAppClient>,
    delay: std::time::Duration,
    /// How many deliveries are between `sleep` and answer right now --
    /// the pipelining signal itself. A lockstep session never gets above
    /// `1`; a windowed one overlaps up to `btp_session_window` of them.
    in_flight: Arc<AtomicUsize>,
    /// The high-water mark of `in_flight`, read back once the run is done.
    max_in_flight: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl connector_runtime::AppClient for SlowApp {
    async fn deliver(
        &self,
        handler_url: &url::Url,
        request: &connector_domain::EnvelopeRequest,
    ) -> AppOutcome {
        let now_in_flight = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight
            .fetch_max(now_in_flight, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        let outcome = self.inner.deliver(handler_url, request).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        outcome
    }
}

/// [`serve_edge`], with the downstream taking `delay` per delivery and the
/// claim gate journaling to `journal` -- the two real costs (a downstream
/// round-trip, an fsync) the lockstep session serialized per frame. The
/// returned [`AtomicUsize`] is [`SlowApp`]'s high-water mark of concurrent
/// deliveries, the pipelining signal a caller reads back once done.
async fn serve_slow_edge(
    delay: std::time::Duration,
    journal: Arc<dyn connector_runtime::Journal>,
) -> (SocketAddr, Arc<dyn Signer>, Arc<AtomicUsize>) {
    let route = StaticRoute::new_priced("g.test.app", "http://localhost:4000", PRICE).unwrap();
    let inner = Arc::new(FakeAppClient::new());
    inner.respond(
        route.handler_url(),
        AppOutcome::Answered {
            response: EnvelopeResponse {
                status: 200,
                headers: vec![],
                body: b"app said yes".to_vec(),
            },
        },
    );
    let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("btp-throughput-test"));
    let max_in_flight = Arc::new(AtomicUsize::new(0));
    let connector = Arc::new(
        Connector::new(
            vec![route],
            vec![],
            Arc::new(SlowApp {
                inner,
                delay,
                in_flight: Arc::new(AtomicUsize::new(0)),
                max_in_flight: max_in_flight.clone(),
            }),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        )
        .with_identity_signer(signer.clone()),
    );
    let gate =
        ClientClaimGate::restore(test_channels(), journal).expect("a fresh journal replays empty");
    let app = connector_client_edge::router_with_gate(connector, signer.clone(), None, gate);
    let server = axum::Server::bind(&"127.0.0.1:0".parse().unwrap()).serve(app.into_make_service());
    let addr = server.local_addr();
    tokio::spawn(server);
    (addr, signer, max_in_flight)
}

/// Issue #688's own property, demonstrated rather than estimated: with the
/// downstream taking a real 20 ms per delivery and the journal a real
/// fsync (a `FileJournal` on disk, group-committed per #686), one session's
/// admission is **pipelined, not serialized** -- deliveries overlap up to
/// `DEFAULT_BTP_SESSION_WINDOW` at once instead of one completing before
/// the next starts.
///
/// This asserts concurrency actually observed in flight rather than a
/// wall-clock rate (issue #747): a wall-clock threshold placed at the
/// measured ceiling (the original form of this test asserted
/// `>150 writes/s`, the number issue #685 documents as that ceiling) has no
/// headroom against CI contention and goes red on a busy runner with no
/// regression underneath it. Whether deliveries overlapped at all is a
/// property of the session's own scheduling, not of how fast the runner
/// happened to be while it ran, so it stays green under load and still
/// catches a real regression to lockstep (fixed at max-observed = 1).
#[tokio::test(flavor = "multi_thread")]
async fn a_single_session_pipelines_admission_instead_of_serializing_it() {
    const WRITES: u64 = 300;
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = connector_runtime::FileJournal::open(dir.path().join("claims.log"))
        .expect("open the journal");
    let (addr, signer, max_in_flight) =
        serve_slow_edge(std::time::Duration::from_millis(20), Arc::new(journal)).await;
    let session = connect(addr).await;
    let receiver = signer.public_key().unwrap();

    let started = std::time::Instant::now();
    let (mut sink, mut stream) = session.split();
    let reader = tokio::spawn(async move {
        let mut answered = std::collections::HashSet::new();
        while answered.len() < WRITES as usize {
            let message = stream
                .next()
                .await
                .expect("the session stays open")
                .expect("a websocket frame");
            let WsMessage::Binary(bytes) = message else {
                continue;
            };
            let answer = parse_answer(&bytes);
            assert_eq!(answer.frame_type, BTP_RESPONSE);
            let fulfill = Fulfill::decode(&answer.ilp_packet).unwrap_or_else(|_| {
                let reject = Reject::decode(&answer.ilp_packet).expect("an OER packet");
                panic!(
                    "write {} was refused {} {:?} instead of fulfilling",
                    answer.request_id,
                    reject.code.as_str(),
                    reject.message
                );
            });
            assert!(!fulfill.data.is_empty(), "a sealed answer rides home");
            assert!(
                answered.insert(answer.request_id),
                "each response correlates to exactly one request"
            );
        }
        answered
    });

    for nonce in 1..=WRITES {
        let claim = evm_claim_json(nonce, nonce * PRICE);
        let prepare = sealed_prepare("g.test.app", &receiver);
        sink.send(WsMessage::Binary(btp_message(
            nonce as u32,
            &[("payment-channel-claim", claim.as_bytes())],
            &prepare.encode(),
        )))
        .await
        .expect("the frame sends");
    }

    let answered = reader.await.expect("the reader task");
    let elapsed = started.elapsed();
    assert_eq!(
        answered,
        (1..=WRITES as u32).collect(),
        "every write was answered exactly once, correlated by requestId"
    );
    let per_second = WRITES as f64 / elapsed.as_secs_f64();
    let observed_max = max_in_flight.load(Ordering::SeqCst);
    let window = connector_client_edge::DEFAULT_BTP_SESSION_WINDOW.get() as usize;
    println!(
        "single-session pipelined admission: {WRITES} paid writes in {elapsed:?} = \
         {per_second:.0}/s, max {observed_max} of {window} concurrently in flight"
    );
    // A lockstep session (issue #688's regression case) is pinned at
    // `max_in_flight == 1`: the next delivery cannot start before the
    // previous one's window slot is released, ever. `>= 2` is already a
    // strict, binary line between pipelined and serialized -- the
    // property issue #747 asks this test to prove, not an estimate of how
    // much overlap "normal" looks like -- and it is set no higher:
    // measured under artificial CPU contention on a 4-core box (multiple
    // `yes` loops fighting the test for cores, well past what a busy CI
    // neighbor does), max observed concurrency fell as low as 2-3 while
    // still genuinely pipelined, at a wall-clock rate under 40/s that
    // would have failed the old >150/s assertion many times over. This
    // floor stays green through exactly the contention that made that
    // assertion flaky.
    const PIPELINING_FLOOR: usize = 2;
    assert!(
        observed_max >= PIPELINING_FLOOR,
        "one session peaked at only {observed_max} of {window} deliveries concurrently in \
         flight, short of the {PIPELINING_FLOOR} floor ({WRITES} writes in {elapsed:?} = \
         {per_second:.0}/s) -- that is admission serializing rather than pipelining, not a slow \
         runner (a busy runner does not lower observed concurrency, only wall-clock rate)"
    );
}
