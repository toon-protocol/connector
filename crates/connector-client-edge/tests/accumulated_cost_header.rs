//! `TOON-Accumulated-Cost` at the client edge (issue #548,
//! `docs/protocol/client-edge-spec.md` §1.6).
//!
//! ADR 0011 chose fee accumulation over a quoting protocol: a sender sends a
//! packet it expects to be rejected, and the reject reports what the path
//! would have charged. The packet plane has accumulated that figure since
//! issue #426 and the peer role has carried it beside the packet since; this
//! file holds the client edge -- where the only clients are -- to reporting
//! it too, over the surface a client actually speaks.
//!
//! Deliberately an integration test, driving the mounted router from
//! outside the crate over nothing but its public API: what a client sees is
//! precisely what this is about, and none of these assertions can be
//! satisfied by anything short of the real response a real sender receives.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{TimeZone, Utc};
use connector_domain::{Prepare, Reject};
use connector_runtime::{
    ClaimStateDomain, ClaimStateSource, ClaimWatermark, Connector, EvmDomain, FakeAppClient,
    InProcessPeerTransport, OutboundClientError, OutboundClientLedger, PeerRoute, TestClock,
};
use connector_signer::{LocalSigner, Signer};
use tower::ServiceExt;

const ACCUMULATED_COST_HEADER: &str = "toon-accumulated-cost";

fn test_clock() -> Arc<TestClock> {
    Arc::new(TestClock::new(
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
    ))
}

fn test_signer() -> Arc<dyn Signer> {
    Arc::new(LocalSigner::generate("cost-header-test-signer"))
}

/// A next hop reporting where this node's claims on a channel stand -- the
/// authority a covering claim is priced off.
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

/// The `[[pay_channels]]` half of a peering. ADR 0042 has a connector cover
/// every PREPARE it sends, and since issue #1145 a forward it cannot cover
/// is refused rather than carried -- so a hop that forwards at all needs
/// this, and a fixture without it is one no config could produce.
fn covering(connector: Connector, peer_id: &str) -> Connector {
    connector
        .with_signer(Arc::new(LocalSigner::generate("cost-header-settlement")))
        .with_outbound_client_ledger(Arc::new(OutboundClientLedger::in_memory()))
        .with_outbound_client_hop(
            peer_id,
            format!("0x{:064x}", 1),
            EvmDomain {
                chain_id: 84_532,
                token_network: [0x1E; 20],
            },
            Arc::new(ReportsAWatermark),
        )
        .expect("a valid on-chain channel id")
}

/// A PREPARE bound for `destination`, carrying nothing a termination could
/// open -- every packet here is expected to be rejected, which is what a
/// probe is.
fn prepare(destination: &str) -> Prepare {
    Prepare {
        // Enough to cover a hop's fee, so a reject below is the one the
        // path decided on rather than the `R01` a packet too small to pay
        // for its own carriage gets (RFC 0027; ADR 0057 as corrected).
        amount: 100,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: destination.to_string(),
        data: vec![0xff; 40],
    }
}

fn ilp_request(prepare: &Prepare) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/ilp")
        .body(Body::from(prepare.encode()))
        .expect("well-formed request")
}

/// The header a client actually reads, as a `u64` -- absent is `None`, which
/// is what this edge answered with before #548 and what every assertion
/// below distinguishes from a genuine `0`.
async fn reject_and_reported_cost(
    connector: Arc<Connector>,
    prepare: &Prepare,
) -> (Reject, Option<u64>) {
    let response = connector_client_edge::router(connector, test_signer())
        .oneshot(ilp_request(prepare))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let reported = response
        .headers()
        .get(ACCUMULATED_COST_HEADER)
        .map(|value| {
            value
                .to_str()
                .expect("the cost header is ASCII")
                .parse::<u64>()
                .expect("the cost header is a decimal uint64")
        });
    let body = hyper::body::to_bytes(response.into_body())
        .await
        .expect("a response body");
    (Reject::decode(&body).expect("an OER REJECT"), reported)
}

/// client-edge-spec.md §1.6: "the header is present on every REJECT
/// response, `0` when the packet never left this connector". A destination
/// with no route at all is exactly that case -- and `0` present is a
/// different answer from the header being absent, which is all a client
/// could observe before #548.
#[tokio::test]
async fn a_reject_that_never_left_this_connector_reports_zero_rather_than_nothing() {
    let connector = Arc::new(Connector::new(
        vec![],
        vec![],
        Arc::new(FakeAppClient::new()),
        Arc::new(InProcessPeerTransport::new()),
        test_clock(),
    ));

    let (reject, reported) = reject_and_reported_cost(connector, &prepare("g.nowhere")).await;

    assert_eq!(reject.code.as_str(), "F02");
    assert_eq!(reported, Some(0));
}

/// The whole point of the header: a figure the packet plane computed and
/// this edge previously discarded. One forwarding hop charging 7, relaying a
/// reject its peer genuinely decided on, is the smallest path whose cost is
/// not zero.
#[tokio::test]
async fn a_reject_relayed_through_a_paying_hop_reports_that_hops_fee() {
    let second_hop = Arc::new(Connector::new(
        vec![],
        vec![],
        Arc::new(FakeAppClient::new()),
        Arc::new(InProcessPeerTransport::new()),
        test_clock(),
    ));
    let mut peer_transport = InProcessPeerTransport::new();
    peer_transport.add_peer("second-hop", second_hop);
    let connector = Arc::new(covering(
        Connector::new(
            vec![],
            vec![PeerRoute::new("g.example", "second-hop")],
            Arc::new(FakeAppClient::new()),
            Arc::new(peer_transport),
            test_clock(),
        )
        .with_peer_fees([("second-hop".to_string(), 7)]),
        "second-hop",
    ));

    let (reject, reported) =
        reject_and_reported_cost(connector, &prepare("g.example.remote")).await;

    // The far hop had no route either, so what comes back is its F02 plus
    // this hop's own fee -- one figure, no breakdown (ADR 0011).
    assert_eq!(reject.code.as_str(), "F02");
    assert_eq!(reported, Some(7));
}

/// ADR 0065 (issue #984) over the surface a client actually reads: two unpaid
/// requests of different sizes to ONE schedule-priced route are greeted with
/// two different `amount`s, and both greetings publish the same schedule.
///
/// Not the cost header, deliberately. An unpaid request to a **priced** route
/// is answered with its terms rather than a reject (ADR 0020), so `402` is
/// the thing a client actually meets here and `amount` is where the figure
/// is. The reject-side twin -- a probe on an open channel, whose
/// `accumulated_cost` is the schedule at its own length -- lives beside
/// `handle_probe` in `connector-runtime`, which is the ingress that has one.
///
/// The `amount`/schedule split is the whole of how ADR 0011's cacheability
/// survives a slope: `amount` answers for this request, and `extra.price` +
/// `extra.pricePerKib` answer for every request the client has not sent yet.
#[tokio::test]
async fn a_schedule_route_greets_each_size_with_its_own_amount() {
    let route = connector_config::StaticRoute::new_scheduled(
        "g.example.app",
        "http://localhost:4000",
        connector_domain::Price::scheduled(25, 4),
    )
    .expect("a valid scheduled route");
    let connector = Arc::new(Connector::new(
        vec![route],
        vec![],
        Arc::new(FakeAppClient::new()),
        Arc::new(InProcessPeerTransport::new()),
        test_clock(),
    ));

    let schedule = connector_domain::Price::scheduled(25, 4);

    async fn greeted_terms(
        connector: Arc<Connector>,
        data: Vec<u8>,
    ) -> connector_domain::x402::X402PaymentRequired {
        let mut packet = prepare("g.example.app");
        packet.data = data;
        let response = connector_client_edge::router(connector, test_signer())
            .oneshot(ilp_request(&packet))
            .await
            .expect("the router answers");
        // An unpaid request to a priced route is answered with its terms.
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
        let body = hyper::body::to_bytes(response.into_body())
            .await
            .expect("a response body");
        connector_domain::x402::parse_greeting(&body).expect("well-formed terms")
    }

    let small = greeted_terms(Arc::clone(&connector), vec![0xff; 40]).await;
    let large = greeted_terms(connector, vec![0xff; 2000]).await;

    // `amount` is per-request, and differs because the requests differ.
    assert_eq!(small.price(), Some(schedule.charge(40)));
    assert_eq!(large.price(), Some(schedule.charge(2000)));
    assert_eq!(small.price(), Some(25 + 4));
    assert_eq!(large.price(), Some(25 + 8));

    // The schedule is per-route, and is the same in both -- which is what
    // lets one greeting answer for a size the client has not sent yet.
    assert_eq!(small.schedule(), Some(schedule));
    assert_eq!(large.schedule(), Some(schedule));
    assert_eq!(
        schedule.charge(1024 * 1024),
        small.schedule().unwrap().charge(1024 * 1024)
    );
}
