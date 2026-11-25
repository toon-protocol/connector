//! The paid packet's lifecycle, driven in through the client edge router
//! (`POST /ilp`): what a client sees (the response and its accumulated-cost
//! header), what the journal records, whether the same claim is good again,
//! and how many forwards the operator surface counts in flight.

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
}

#[async_trait::async_trait]
impl AppClient for HangsOnce {
    async fn deliver(&self, handler_url: &url::Url, request: &EnvelopeRequest) -> AppOutcome {
        if !self.hung.swap(true, Ordering::SeqCst) {
            self.reached.notify_one();
            self.release.notified().await;
        }
        self.inner.deliver(handler_url, request).await
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

fn edge(connector: Arc<Connector>, signer: Arc<dyn Signer>, gate: Arc<ClientClaimGate>) -> Router {
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

/// A paid `POST /ilp` to `destination`, sealed to `signer`, carrying
/// `claim_json` in the plaintext claim header.
fn paid_request(destination: &str, signer: &Arc<dyn Signer>, claim_json: &str) -> Request<Body> {
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
    let prepare = Prepare {
        amount: PRICE,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: destination.to_string(),
        data,
    };
    Request::builder()
        .method("POST")
        .uri("/ilp")
        .header(crate::CLAIM_HEADER, BASE64.encode(claim_json.as_bytes()))
        .body(Body::from(prepare.encode()))
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

#[tokio::test]
async fn a_valid_voucher_is_fulfilled_and_stays_counted() {
    let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
    let (gate, journal) = voucher_gate();
    let connector =
        payer_over_a_priced_peering(&signer, Some(remote_app_answering(answered(b"carried"))));
    let app = edge(connector, Arc::clone(&signer), Arc::clone(&gate));

    let response = app
        .oneshot(paid_request(REMOTE_APP, &signer, &voucher(PRICE)))
        .await
        .unwrap();

    assert_eq!(answer(response).await, Answer::Fulfill);
    assert_eq!(claim_entries(&journal), vec![accepted(100)]);
    assert_eq!(gate.forwards_in_flight(), 0);
    assert!(gate.last_claim_time(&test_support::channel_key()).is_some());
}

/// The app behind the peering answers nothing: the forward's reject comes
/// back, and a forwarded route never charges on a reject (ADR 0028).
#[tokio::test]
async fn an_app_rejection_across_a_forward_rolls_the_claim_back() {
    let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
    let (gate, journal) = voucher_gate();
    let connector = payer_over_a_priced_peering(
        &signer,
        Some(remote_app_answering(AppOutcome::Unreachable {
            message: "the app is down".into(),
        })),
    );
    let app = edge(connector, Arc::clone(&signer), Arc::clone(&gate));

    let first = answer(
        app.clone()
            .oneshot(paid_request(REMOTE_APP, &signer, &voucher(PRICE)))
            .await
            .unwrap(),
    )
    .await;
    assert!(matches!(first, Answer::Reject { .. }), "{first:?}");
    assert_eq!(claim_entries(&journal), vec![accepted(100), reset()]);
    assert_eq!(gate.watermark(&test_support::channel_key()), None);
    assert_eq!(gate.forwards_in_flight(), 0);

    // The same voucher is good again: judged afresh, not as a resend.
    let second = answer(
        app.oneshot(paid_request(REMOTE_APP, &signer, &voucher(PRICE)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(second, first);
}

/// The peer has no route to the app: the packet is never carried.
#[tokio::test]
async fn an_uncarried_forward_rolls_the_claim_back() {
    let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
    let (gate, journal) = voucher_gate();
    let connector = payer_over_a_priced_peering(&signer, None);
    let app = edge(connector, Arc::clone(&signer), Arc::clone(&gate));

    let response = app
        .clone()
        .oneshot(paid_request(REMOTE_APP, &signer, &voucher(PRICE)))
        .await
        .unwrap();

    // The next hop's refusal, priced at what reaching it cost: this hop's
    // fee on the peering.
    assert_eq!(answer(response).await, reject("F02", FEE));
    assert_eq!(claim_entries(&journal), vec![accepted(100), reset()]);
    assert_eq!(gate.watermark(&test_support::channel_key()), None);
    assert_eq!(gate.forwards_in_flight(), 0);
}

/// A reject from an app this connector terminates is that app's answer,
/// which consumed real work (ADR 0020): unchanged by #1719, it stays
/// counted.
#[tokio::test]
async fn a_terminated_apps_rejection_stays_counted() {
    let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
    let (gate, journal) = voucher_gate();
    let connector = terminating(
        &signer,
        AppOutcome::Unreachable {
            message: "the app is down".into(),
        },
    );
    let app = edge(connector, Arc::clone(&signer), Arc::clone(&gate));

    let response = app
        .oneshot(paid_request(LOCAL_APP, &signer, &voucher(PRICE)))
        .await
        .unwrap();

    assert!(matches!(answer(response).await, Answer::Reject { .. }));
    assert_eq!(claim_entries(&journal), vec![accepted(100)]);
    assert_eq!(
        gate.watermark(&test_support::channel_key())
            .map(|watermark| watermark.cumulative_amount),
        Some(100)
    );
}

/// The client drops its request while the forward is in flight: the handle
/// is dropped without an outcome, which counts as not carried.
#[tokio::test]
async fn a_cancelled_forward_rolls_the_claim_back_and_the_voucher_is_good_again() {
    let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
    let (gate, journal) = voucher_gate();
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let inner = FakeAppClient::new();
    inner.respond(
        StaticRoute::new(REMOTE_APP, "http://localhost:4000")
            .unwrap()
            .handler_url(),
        answered(b"carried the second time"),
    );
    let remote_app = Arc::new(HangsOnce {
        inner,
        hung: AtomicBool::new(false),
        reached: Arc::clone(&reached),
        release: Arc::clone(&release),
    });
    let connector = payer_over_a_priced_peering(&signer, Some(remote_app));
    let app = edge(connector, Arc::clone(&signer), Arc::clone(&gate));

    let mut forward = Box::pin(app.clone().oneshot(paid_request(
        REMOTE_APP,
        &signer,
        &voucher(PRICE),
    )));
    tokio::select! {
        _ = &mut forward => panic!("the first forward never answers"),
        () = reached.notified() => {}
    }
    assert_eq!(
        gate.watermark(&test_support::channel_key())
            .map(|watermark| watermark.cumulative_amount),
        Some(100),
        "counted while in flight"
    );
    drop(forward);

    assert_eq!(gate.watermark(&test_support::channel_key()), None);
    assert_eq!(gate.forwards_in_flight(), 0);
    release.notify_one();

    let retried = app
        .oneshot(paid_request(REMOTE_APP, &signer, &voucher(PRICE)))
        .await
        .unwrap();
    assert_eq!(answer(retried).await, Answer::Fulfill);
    assert_eq!(
        claim_entries(&journal),
        vec![accepted(100), reset(), accepted(100)]
    );
}

/// A voucher already admitted for a carried packet buys nothing a second
/// time: refused before anything is routed, and nothing journaled.
#[tokio::test]
async fn an_already_admitted_claim_is_refused() {
    let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
    let (gate, journal) = voucher_gate();
    let connector =
        payer_over_a_priced_peering(&signer, Some(remote_app_answering(answered(b"carried"))));
    let app = edge(connector, Arc::clone(&signer), Arc::clone(&gate));
    let first = app
        .clone()
        .oneshot(paid_request(REMOTE_APP, &signer, &voucher(PRICE)))
        .await
        .unwrap();
    assert_eq!(answer(first).await, Answer::Fulfill);

    let replayed = app
        .oneshot(paid_request(REMOTE_APP, &signer, &voucher(PRICE)))
        .await
        .unwrap();

    assert_eq!(answer(replayed).await, reject("F03", PRICE));
    assert_eq!(claim_entries(&journal), vec![accepted(100)]);
    assert_eq!(gate.forwards_in_flight(), 0);
}

/// ADR 0080 decision 4: an allowance claim's forward is in flight while
/// the packet is, and released once it resolves -- fulfilled or cancelled.
#[tokio::test]
async fn a_zk_edge_claims_in_flight_forward_is_released() {
    let dir = tempfile::tempdir().unwrap();
    let session = allowance_fixtures::keypair(21);
    let (gate, id) = allowance_fixtures::edge_with_allowance(dir.path(), &session).await;
    let gate = Arc::new(gate);
    let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("paid-packet"));
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let inner = FakeAppClient::new();
    inner.respond(
        StaticRoute::new(REMOTE_APP, "http://localhost:4000")
            .unwrap()
            .handler_url(),
        answered(b"carried"),
    );
    let remote_app = Arc::new(HangsOnce {
        inner,
        hung: AtomicBool::new(false),
        reached: Arc::clone(&reached),
        release: Arc::clone(&release),
    });
    let connector = payer_over_a_priced_peering(&signer, Some(remote_app));
    let app = edge(connector, Arc::clone(&signer), Arc::clone(&gate));
    let claim = allowance_fixtures::claim(&session, id, PRICE);

    let mut forward = Box::pin(
        app.clone()
            .oneshot(paid_request(REMOTE_APP, &signer, &claim)),
    );
    tokio::select! {
        _ = &mut forward => panic!("the first forward never answers"),
        () = reached.notified() => {}
    }
    assert_eq!(
        gate.forwards_in_flight(),
        1,
        "in flight while the packet is"
    );
    drop(forward);
    assert_eq!(gate.forwards_in_flight(), 0, "released when cancelled");
    assert_eq!(gate.watermark(&id.claim_key()), None, "and rolled back");
    release.notify_one();

    let carried = app
        .oneshot(paid_request(REMOTE_APP, &signer, &claim))
        .await
        .unwrap();
    assert_eq!(answer(carried).await, Answer::Fulfill);
    assert_eq!(gate.forwards_in_flight(), 0, "released when fulfilled");
    assert_eq!(
        gate.watermark(&id.claim_key())
            .map(|watermark| watermark.cumulative_amount),
        Some(u128::from(PRICE))
    );
}
