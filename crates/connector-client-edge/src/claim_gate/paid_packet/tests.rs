//! The paid packet's lifecycle, driven in through both carriages (#1720):
//! ILP-over-HTTP through the client edge router (`POST /ilp`) and BTP
//! through a real websocket session. Every case runs over each, and asserts
//! what a client sees (the response and its accumulated cost), what the
//! journal records, whether the same claim is good again, and how many
//! forwards the operator surface counts in flight -- so a bug in one
//! carriage's framing of the shared pipeline fails a test (ADR 0027).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::Router;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::{TimeZone, Utc};
use connector_config::StaticRoute;
use connector_domain::{
    EnvelopeRequest, EnvelopeResponse, Fulfill, JournalEntry, Reject, VOUCHER_WATERMARK_NONCE,
};
use connector_runtime::{
    AppClient, AppOutcome, FakeAppClient, InMemoryJournal, InProcessPeerTransport, Journal,
    PeerRoute, TestClock,
};
use connector_signer::{LocalSigner, Signer};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tower::ServiceExt;

use crate::claim_gate::allowance_fixtures;
use crate::test_support;
use crate::{router_with_node_facts, ClientClaimGate, DEFAULT_BTP_SESSION_WINDOW};

const PRICE: u64 = 100;
const FEE: u64 = 3;
const PEER_ID: &str = "beta";
const FORWARD_PREFIX: &str = "g.example.beta";
const REMOTE_APP: &str = "g.example.beta.app";
const LOCAL_APP: &str = "g.example.app";

fn clock() -> Arc<TestClock> {
    Arc::new(TestClock::new(
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
    ))
}

fn answered(body: &[u8]) -> AppOutcome {
    AppOutcome::Answered {
        response: EnvelopeResponse {
            status: 200,
            headers: vec![],
            body: body.to_vec(),
        },
    }
}

/// An app that holds its first delivery -- the forward a client cancels --
/// until the test releases it, and answers every delivery as `inner` does.
/// The hold is released rather than endless because the in-process peer
/// answers its frames in order, as one real stream would: the peer's own
/// task is still carrying the first packet after the client has gone, and a
/// retry queues behind it.
struct HangsOnce {
    inner: FakeAppClient,
    hung: AtomicBool,
    reached: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    /// Notified once the held delivery has answered.
    answered: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl AppClient for HangsOnce {
    async fn deliver(&self, handler_url: &url::Url, request: &EnvelopeRequest) -> AppOutcome {
        if self.hung.swap(true, Ordering::SeqCst) {
            return self.inner.deliver(handler_url, request).await;
        }
        self.reached.notify_one();
        self.release.notified().await;
        let outcome = self.inner.deliver(handler_url, request).await;
        self.answered.notify_one();
        outcome
    }
}

/// A connector whose route to [`REMOTE_APP`] is a peering priced at
/// [`PRICE`], with the peer's own connector -- delivering to `remote_app`
/// -- on the far end. `remote_app` is `None` for a peer that has no route
/// to the app at all, so the forward is never carried.
fn payer_over_a_priced_peering(
    signer: &Arc<dyn Signer>,
    remote_app: Option<Arc<dyn AppClient>>,
) -> Arc<Connector> {
    let (routes, app): (Vec<StaticRoute>, Arc<dyn AppClient>) = match remote_app {
        Some(app) => (
            vec![StaticRoute::new(REMOTE_APP, "http://localhost:4000").unwrap()],
            app,
        ),
        None => (vec![], Arc::new(FakeAppClient::new())),
    };
    let payee = Arc::new(
        Connector::new(
            routes,
            vec![],
            app,
            Arc::new(InProcessPeerTransport::new()),
            clock(),
        )
        .with_identity_signer(Arc::clone(signer)),
    );
    let mut transport = InProcessPeerTransport::new();
    transport.add_peer(PEER_ID, payee);
    Arc::new(connector_runtime::covering_fake::covering(
        Connector::new(
            vec![],
            vec![PeerRoute::new_priced(FORWARD_PREFIX, PEER_ID, PRICE)],
            Arc::new(FakeAppClient::new()),
            Arc::new(transport),
            clock(),
        )
        .with_peer_fees([(PEER_ID.to_string(), FEE)])
        .with_identity_signer(Arc::clone(signer)),
        PEER_ID,
    ))
}

fn remote_app_answering(outcome: AppOutcome) -> Arc<dyn AppClient> {
    let app = FakeAppClient::new();
    app.respond(
        StaticRoute::new(REMOTE_APP, "http://localhost:4000")
            .unwrap()
            .handler_url(),
        outcome,
    );
    Arc::new(app)
}

/// A connector terminating [`LOCAL_APP`] itself, priced at [`PRICE`].
fn terminating(signer: &Arc<dyn Signer>, outcome: AppOutcome) -> Arc<Connector> {
    let route = StaticRoute::new_priced(LOCAL_APP, "http://localhost:4000", PRICE).unwrap();
    let app = FakeAppClient::new();
    app.respond(route.handler_url(), outcome);
    Arc::new(
        Connector::new(
            vec![route],
            vec![],
            Arc::new(app),
            Arc::new(InProcessPeerTransport::new()),
            clock(),
        )
        .with_identity_signer(Arc::clone(signer)),
    )
}

fn edge_router(
    connector: Arc<Connector>,
    signer: Arc<dyn Signer>,
    gate: Arc<ClientClaimGate>,
) -> Router {
    router_with_node_facts(
        connector,
        signer,
        None,
        gate,
        connector_domain::NodeFacts::default(),
        DEFAULT_BTP_SESSION_WINDOW,
        None,
        Arc::from([]),
    )
}

/// A voucher gate journaling to an in-memory journal the test reads back.
fn voucher_gate() -> (Arc<ClientClaimGate>, Arc<InMemoryJournal>) {
    let journal = Arc::new(InMemoryJournal::new());
    let gate = test_support::gate_over(
        Arc::clone(&journal) as Arc<dyn Journal>,
        &Arc::new(test_support::InMemoryNetworks::new(1_000_000_000)),
    );
    (Arc::new(gate), journal)
}

fn voucher(amount: u64) -> String {
    test_support::signed_voucher(u128::from(amount))
}

/// A PREPARE to `destination` at [`PRICE`], sealed to `signer`.
fn paid_prepare(destination: &str, signer: &Arc<dyn Signer>) -> Prepare {
    let envelope = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: b"hello app".to_vec(),
    }
    .encode();
    let (data, _shared_secret) =
        connector_signer::giftwrap::seal_request(&envelope, &signer.public_key().unwrap())
            .expect("seal");
    Prepare {
        amount: PRICE,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: destination.to_string(),
        data,
    }
}

/// A paid `POST /ilp` to `destination`, sealed to `signer`, carrying
/// `claim_json` in the plaintext claim header.
fn paid_request(destination: &str, signer: &Arc<dyn Signer>, claim_json: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/ilp")
        .header(crate::CLAIM_HEADER, BASE64.encode(claim_json.as_bytes()))
        .body(Body::from(paid_prepare(destination, signer).encode()))
        .unwrap()
}

/// What a client reads off one answer.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Fulfill,
    Reject {
        code: String,
        accumulated_cost: Option<String>,
    },
}

async fn answer(response: Response) -> Answer {
    assert_eq!(response.status(), StatusCode::OK);
    let accumulated_cost = response
        .headers()
        .get(crate::ACCUMULATED_COST_HEADER)
        .map(|value| value.to_str().unwrap().to_string());
    let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
    if let Ok(_fulfill) = Fulfill::decode(&bytes) {
        assert_eq!(accumulated_cost, None, "a FULFILL carries no cost header");
        return Answer::Fulfill;
    }
    let reject = Reject::decode(&bytes).expect("a FULFILL or a REJECT");
    Answer::Reject {
        code: reject.code.as_str().to_string(),
        accumulated_cost,
    }
}

fn reject(code: &str, accumulated_cost: u64) -> Answer {
    Answer::Reject {
        code: code.to_string(),
        accumulated_cost: Some(accumulated_cost.to_string()),
    }
}

/// The journal's claim entries, as (kind, channel, amount).
fn claim_entries(journal: &InMemoryJournal) -> Vec<(&'static str, String, Option<u128>)> {
    journal
        .read_all()
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry {
            JournalEntry::InboundClaimAccepted {
                channel_id,
                nonce,
                cumulative_amount,
                ..
            } => {
                assert_eq!(nonce, VOUCHER_WATERMARK_NONCE);
                Some(("accepted", channel_id, Some(cumulative_amount)))
            }
            JournalEntry::InboundClaimRolledBack {
                channel_id,
                cumulative_amount,
                ..
            } => Some(("rolled back", channel_id, Some(cumulative_amount))),
            JournalEntry::InboundClaimWatermarkReset { channel_id } => {
                Some(("reset", channel_id, None))
            }
            _ => None,
        })
        .collect()
}

fn accepted(amount: u128) -> (&'static str, String, Option<u128>) {
    ("accepted", test_support::channel_key(), Some(amount))
}

fn reset() -> (&'static str, String, Option<u128>) {
    ("reset", test_support::channel_key(), None)
}

/// The two carriages a client pays over. Every case below runs on both.
#[derive(Debug, Clone, Copy)]
enum Carriage {
    /// ILP-over-HTTP: `POST /ilp` through the client edge router.
    Http,
    /// BTP: a MESSAGE on a websocket session to `GET /ilp/btp`.
    Btp,
}

const CARRIAGES: [Carriage; 2] = [Carriage::Http, Carriage::Btp];

type Session =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The client edge, as a client reaches it: the router for HTTP, and the
/// same router served on an ephemeral port for BTP, since `oneshot` cannot
/// carry a websocket upgrade.
struct Edge {
    router: Router,
    addr: SocketAddr,
}

async fn edge(
    connector: Arc<Connector>,
    signer: Arc<dyn Signer>,
    gate: Arc<ClientClaimGate>,
) -> Edge {
    let router = edge_router(connector, signer, gate);
    let server = axum::Server::bind(&"127.0.0.1:0".parse().unwrap())
        .serve(router.clone().into_make_service());
    let addr = server.local_addr();
    tokio::spawn(server);
    Edge { router, addr }
}

/// A paid packet on its way: the request future on HTTP, the open session
/// on BTP.
enum InFlight {
    Http(std::pin::Pin<Box<dyn std::future::Future<Output = Answer> + Send>>),
    Btp(Box<Session>),
}

impl Carriage {
    /// Send a packet to `destination`, sealed to `signer` and paid for by
    /// `claim_json`, without waiting for its answer.
    async fn send(
        self,
        edge: &Edge,
        destination: &str,
        signer: &Arc<dyn Signer>,
        claim_json: &str,
    ) -> InFlight {
        match self {
            Carriage::Http => {
                let request = paid_request(destination, signer, claim_json);
                let router = edge.router.clone();
                InFlight::Http(Box::pin(async move {
                    answer(router.oneshot(request).await.unwrap()).await
                }))
            }
            Carriage::Btp => {
                let (mut session, _) =
                    tokio_tungstenite::connect_async(format!("ws://{}/ilp/btp", edge.addr))
                        .await
                        .expect("the upgrade succeeds");
                let frame = connector_btp::encode_message(
                    1,
                    &[connector_btp::ProtocolData {
                        name: connector_btp::CLAIM_PROTOCOL.to_string(),
                        content_type: connector_btp::CONTENT_TYPE_TEXT,
                        data: claim_json.as_bytes().to_vec(),
                    }],
                    &paid_prepare(destination, signer).encode(),
                );
                session
                    .send(WsMessage::Binary(frame))
                    .await
                    .expect("the frame sends");
                InFlight::Btp(Box::new(session))
            }
        }
    }

    /// Pay for one packet and read its answer.
    async fn pay(
        self,
        edge: &Edge,
        destination: &str,
        signer: &Arc<dyn Signer>,
        claim_json: &str,
    ) -> Answer {
        self.send(edge, destination, signer, claim_json)
            .await
            .answer()
            .await
    }

    /// Pay for one packet and go away -- drop the request, or close the
    /// socket -- once it has reached the app behind the peering.
    async fn pay_and_disconnect_once_forwarded(
        self,
        edge: &Edge,
        signer: &Arc<dyn Signer>,
        claim_json: &str,
        reached: &tokio::sync::Notify,
    ) {
        let mut in_flight = self.send(edge, REMOTE_APP, signer, claim_json).await;
        tokio::select! {
            answer = in_flight.next_answer() => panic!("the held forward answered: {answer:?}"),
            () = reached.notified() => {}
        }
        drop(in_flight);
    }
}

impl InFlight {
    async fn answer(mut self) -> Answer {
        self.next_answer().await
    }

    async fn next_answer(&mut self) -> Answer {
        match self {
            InFlight::Http(request) => request.await,
            InFlight::Btp(session) => loop {
                let message = session
                    .next()
                    .await
                    .expect("the session stays open")
                    .expect("a websocket frame");
                if let WsMessage::Binary(bytes) = message {
                    break btp_answer(&bytes);
                }
            },
        }
    }
}

/// What a client reads off one BTP RESPONSE: the packet, and on a REJECT
/// the accumulated cost its protocolData carries.
fn btp_answer(bytes: &[u8]) -> Answer {
    let frame = connector_btp::decode_frame(bytes).expect("a BTP frame");
    assert_eq!(frame.frame_type, connector_btp::BTP_RESPONSE);
    let accumulated_cost = frame
        .protocol_data
        .iter()
        .find(|entry| entry.name == connector_btp::ACCUMULATED_COST_PROTOCOL)
        .map(|entry| String::from_utf8(entry.data.clone()).unwrap());
    if let Ok(_fulfill) = Fulfill::decode(&frame.ilp_packet) {
        assert_eq!(accumulated_cost, None, "a FULFILL carries no cost");
        return Answer::Fulfill;
    }
    let reject = Reject::decode(&frame.ilp_packet).expect("a FULFILL or a REJECT");
    Answer::Reject {
        code: reject.code.as_str().to_string(),
        accumulated_cost,
    }
}

/// Let the carriage that outlived its client run to its outcome, once the
/// held packet's app has answered.
async fn settle(answered: &tokio::sync::Notify) {
    answered.notified().await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn a_valid_voucher_is_fulfilled_and_stays_counted() {
    for carriage in CARRIAGES {
        let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
        let (gate, journal) = voucher_gate();
        let connector =
            payer_over_a_priced_peering(&signer, Some(remote_app_answering(answered(b"carried"))));
        let edge = edge(connector, Arc::clone(&signer), Arc::clone(&gate)).await;

        let answer = carriage
            .pay(&edge, REMOTE_APP, &signer, &voucher(PRICE))
            .await;

        assert_eq!(answer, Answer::Fulfill, "{carriage:?}");
        assert_eq!(claim_entries(&journal), vec![accepted(100)], "{carriage:?}");
        assert_eq!(gate.forwards_in_flight(), 0, "{carriage:?}");
        assert!(
            gate.last_claim_time(&test_support::channel_key()).is_some(),
            "{carriage:?}"
        );
    }
}

/// The app behind the peering answers nothing: the forward's reject comes
/// back, and a forwarded route never charges on a reject (ADR 0028).
#[tokio::test]
async fn an_app_rejection_across_a_forward_rolls_the_claim_back() {
    for carriage in CARRIAGES {
        let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
        let (gate, journal) = voucher_gate();
        let connector = payer_over_a_priced_peering(
            &signer,
            Some(remote_app_answering(AppOutcome::Unreachable {
                message: "the app is down".into(),
            })),
        );
        let edge = edge(connector, Arc::clone(&signer), Arc::clone(&gate)).await;

        let first = carriage
            .pay(&edge, REMOTE_APP, &signer, &voucher(PRICE))
            .await;
        assert!(
            matches!(first, Answer::Reject { .. }),
            "{carriage:?}: {first:?}"
        );
        assert_eq!(
            claim_entries(&journal),
            vec![accepted(100), reset()],
            "{carriage:?}"
        );
        assert_eq!(
            gate.watermark(&test_support::channel_key()),
            None,
            "{carriage:?}"
        );
        assert_eq!(gate.forwards_in_flight(), 0, "{carriage:?}");

        // The same voucher is good again: judged afresh, not as a resend.
        let second = carriage
            .pay(&edge, REMOTE_APP, &signer, &voucher(PRICE))
            .await;
        assert_eq!(second, first, "{carriage:?}");
    }
}

/// The peer has no route to the app: the packet is never carried.
#[tokio::test]
async fn an_uncarried_forward_rolls_the_claim_back() {
    for carriage in CARRIAGES {
        let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
        let (gate, journal) = voucher_gate();
        let connector = payer_over_a_priced_peering(&signer, None);
        let edge = edge(connector, Arc::clone(&signer), Arc::clone(&gate)).await;

        let answer = carriage
            .pay(&edge, REMOTE_APP, &signer, &voucher(PRICE))
            .await;

        // The next hop's refusal, priced at what reaching it cost: this
        // hop's fee on the peering.
        assert_eq!(answer, reject("F02", FEE), "{carriage:?}");
        assert_eq!(
            claim_entries(&journal),
            vec![accepted(100), reset()],
            "{carriage:?}"
        );
        assert_eq!(
            gate.watermark(&test_support::channel_key()),
            None,
            "{carriage:?}"
        );
        assert_eq!(gate.forwards_in_flight(), 0, "{carriage:?}");
    }
}

/// A reject from an app this connector terminates is that app's answer,
/// which consumed real work (ADR 0020): it stays counted.
#[tokio::test]
async fn a_terminated_apps_rejection_stays_counted() {
    for carriage in CARRIAGES {
        let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
        let (gate, journal) = voucher_gate();
        let connector = terminating(
            &signer,
            AppOutcome::Unreachable {
                message: "the app is down".into(),
            },
        );
        let edge = edge(connector, Arc::clone(&signer), Arc::clone(&gate)).await;

        let answer = carriage
            .pay(&edge, LOCAL_APP, &signer, &voucher(PRICE))
            .await;

        assert!(matches!(answer, Answer::Reject { .. }), "{carriage:?}");
        assert_eq!(claim_entries(&journal), vec![accepted(100)], "{carriage:?}");
        assert_eq!(
            gate.watermark(&test_support::channel_key())
                .map(|watermark| watermark.cumulative_amount),
            Some(100),
            "{carriage:?}"
        );
    }
}

/// A voucher already admitted for a carried packet buys nothing a second
/// time: refused before anything is routed, and nothing journaled.
#[tokio::test]
async fn an_already_admitted_claim_is_refused() {
    for carriage in CARRIAGES {
        let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
        let (gate, journal) = voucher_gate();
        let connector =
            payer_over_a_priced_peering(&signer, Some(remote_app_answering(answered(b"carried"))));
        let edge = edge(connector, Arc::clone(&signer), Arc::clone(&gate)).await;
        let first = carriage
            .pay(&edge, REMOTE_APP, &signer, &voucher(PRICE))
            .await;
        assert_eq!(first, Answer::Fulfill, "{carriage:?}");

        let replayed = carriage
            .pay(&edge, REMOTE_APP, &signer, &voucher(PRICE))
            .await;

        assert_eq!(replayed, reject("F03", PRICE), "{carriage:?}");
        assert_eq!(claim_entries(&journal), vec![accepted(100)], "{carriage:?}");
        assert_eq!(gate.forwards_in_flight(), 0, "{carriage:?}");
    }
}

/// An app behind the peering that holds the first packet until released,
/// with `reached` notified once that packet is at the app and `answered`
/// once the app has answered it.
fn held_remote_app() -> (Arc<HangsOnce>, HeldApp) {
    let held = HeldApp {
        reached: Arc::new(tokio::sync::Notify::new()),
        release: Arc::new(tokio::sync::Notify::new()),
        answered: Arc::new(tokio::sync::Notify::new()),
    };
    let inner = FakeAppClient::new();
    inner.respond(
        StaticRoute::new(REMOTE_APP, "http://localhost:4000")
            .unwrap()
            .handler_url(),
        answered(b"carried"),
    );
    let app = Arc::new(HangsOnce {
        inner,
        hung: AtomicBool::new(false),
        reached: Arc::clone(&held.reached),
        release: Arc::clone(&held.release),
        answered: Arc::clone(&held.answered),
    });
    (app, held)
}

struct HeldApp {
    reached: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    answered: Arc<tokio::sync::Notify>,
}

/// A client that goes away -- drops its `POST /ilp`, or closes its BTP
/// socket -- once its packet has been forwarded is still charged when the
/// next hop fulfils: the packet is carried in its own task, so the
/// disconnect cancels nothing, and the outcome decides the charge.
#[tokio::test]
async fn a_client_that_disconnects_after_the_forward_is_charged_when_it_fulfils() {
    for carriage in CARRIAGES {
        let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
        let (gate, journal) = voucher_gate();
        let (remote_app, held) = held_remote_app();
        let connector = payer_over_a_priced_peering(&signer, Some(remote_app));
        let edge = edge(connector, Arc::clone(&signer), Arc::clone(&gate)).await;

        carriage
            .pay_and_disconnect_once_forwarded(&edge, &signer, &voucher(PRICE), &held.reached)
            .await;
        held.release.notify_one();
        settle(&held.answered).await;

        assert_eq!(claim_entries(&journal), vec![accepted(100)], "{carriage:?}");
        assert_eq!(
            gate.watermark(&test_support::channel_key())
                .map(|watermark| watermark.cumulative_amount),
            Some(100),
            "{carriage:?}"
        );
        let replayed = carriage
            .pay(&edge, REMOTE_APP, &signer, &voucher(PRICE))
            .await;
        assert_eq!(
            replayed,
            reject("F03", PRICE),
            "{carriage:?}: the fulfilled packet's voucher stays spent"
        );
    }
}

/// The same on a zk edge, where the forward's completion is observable:
/// the allowance's forward stays in flight after the client has gone, is
/// released only once the next hop fulfils, and the claim stays counted
/// (ADR 0080 decision 4).
#[tokio::test]
async fn a_zk_edge_claims_forward_outlives_the_client_and_is_released_by_its_outcome() {
    for (seed, carriage) in (21..).zip(CARRIAGES) {
        let dir = tempfile::tempdir().unwrap();
        let session = allowance_fixtures::keypair(seed);
        let (gate, id) = allowance_fixtures::edge_with_allowance(dir.path(), &session).await;
        let gate = Arc::new(gate);
        let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
        let (remote_app, held) = held_remote_app();
        let connector = payer_over_a_priced_peering(&signer, Some(remote_app));
        let edge = edge(connector, Arc::clone(&signer), Arc::clone(&gate)).await;
        let claim = allowance_fixtures::claim(&session, id, PRICE);

        carriage
            .pay_and_disconnect_once_forwarded(&edge, &signer, &claim, &held.reached)
            .await;
        assert_eq!(
            gate.forwards_in_flight(),
            1,
            "{carriage:?}: still in flight once the client has gone"
        );

        held.release.notify_one();
        while gate.forwards_in_flight() > 0 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            gate.watermark(&id.claim_key())
                .map(|watermark| watermark.cumulative_amount),
            Some(u128::from(PRICE)),
            "{carriage:?}: charged by its outcome"
        );
    }
}
/// A client edge state over `connector` and `gate`, for driving the
/// pipeline's halves directly.
fn state_over(connector: Arc<Connector>, gate: Arc<ClientClaimGate>) -> ClientEdgeState {
    ClientEdgeState {
        connector,
        signer: Arc::new(LocalSigner::generate("paid-packet")),
        claim_gate: gate,
        wrap_receiver_secret: None,
        node: Arc::new(connector_domain::NodeFacts::default()),
        btp_session_window: DEFAULT_BTP_SESSION_WINDOW,
        session_registry: Arc::new(crate::session_registry::SessionRegistry::new()),
        peers: None,
        identities: Arc::from([]),
    }
}

/// Admit `claim_json` for a packet to [`REMOTE_APP`] and drop the handle
/// before anything is forwarded: what a pipeline cancelled or panicking
/// between admission and its forward leaves behind.
async fn admit_and_drop(state: &ClientEdgeState, signer: &Arc<dyn Signer>, claim_json: &str) {
    let prepare = paid_prepare(REMOTE_APP, signer);
    let admitted = admit(
        state,
        &prepare,
        Some(|| Ok(claim_json.to_string())),
        PRICE,
        &SessionRole::Client,
    )
    .await
    .expect("the claim admits")
    .expect("a claim was presented");
    drop(admitted);
}

/// A packet whose handle is dropped before anything is forwarded leaves no
/// value counted and no hold held, and the same voucher is good again.
#[tokio::test]
async fn a_packet_dropped_before_its_forward_leaves_nothing_counted() {
    let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
    let (gate, journal) = voucher_gate();
    let connector =
        payer_over_a_priced_peering(&signer, Some(remote_app_answering(answered(b"carried"))));
    let state = state_over(Arc::clone(&connector), Arc::clone(&gate));

    admit_and_drop(&state, &signer, &voucher(PRICE)).await;

    assert_eq!(gate.watermark(&test_support::channel_key()), None);
    assert_eq!(gate.forwards_in_flight(), 0);
    let app = edge_router(connector, Arc::clone(&signer), Arc::clone(&gate));
    let retried = app
        .oneshot(paid_request(REMOTE_APP, &signer, &voucher(PRICE)))
        .await
        .unwrap();
    assert_eq!(answer(retried).await, Answer::Fulfill);
    // The rollback's entry is journaled once the committer gets to it; by
    // the time a later acceptance is durable, it is.
    assert_eq!(
        claim_entries(&journal),
        vec![accepted(100), reset(), accepted(100)]
    );
}

/// The same on a zk edge: no hold is left held.
#[tokio::test]
async fn a_zk_edge_packet_dropped_before_its_forward_holds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let session = allowance_fixtures::keypair(22);
    let (gate, id) = allowance_fixtures::edge_with_allowance(dir.path(), &session).await;
    let gate = Arc::new(gate);
    let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
    let connector =
        payer_over_a_priced_peering(&signer, Some(remote_app_answering(answered(b"carried"))));
    let state = state_over(Arc::clone(&connector), Arc::clone(&gate));

    admit_and_drop(
        &state,
        &signer,
        &allowance_fixtures::claim(&session, id, PRICE),
    )
    .await;

    assert_eq!(gate.forwards_in_flight(), 0);
    assert_eq!(gate.watermark(&id.claim_key()), None);
    let app = edge_router(connector, Arc::clone(&signer), Arc::clone(&gate));
    let carried = app
        .oneshot(paid_request(
            REMOTE_APP,
            &signer,
            &allowance_fixtures::claim(&session, id, PRICE),
        ))
        .await
        .unwrap();
    assert_eq!(answer(carried).await, Answer::Fulfill);
    assert_eq!(gate.forwards_in_flight(), 0);
}
