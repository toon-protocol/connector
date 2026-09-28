//! The fourth routing arm `session_registry.rs`'s own module doc deferred:
//! wiring [`crate::session_registry::SessionRegistry`] into the packet path
//! so a PREPARE addressed to a live, bound client session is delivered
//! through it rather than answered `F02` (issue #736, toon-meta#262's
//! job-dispatch work). `connector_runtime::Connector::handle_prepare` cannot
//! see the registry at all -- `connector-client-edge` depends on
//! `connector-runtime`, never the reverse -- so this arm lives here,
//! wrapped around `handle_prepare` rather than folded into it.
//!
//! **Precedence.** A configured *forwarding* route (peer or leased) always
//! wins: [`route_prepare`] calls `handle_prepare` first, and only falls
//! through to the session registry when that call's own answer is `F02` (no
//! route at all). A session therefore never shadows an operator's own
//! routing table, and -- since `F02` is the only answer this arm ever
//! overrides -- a forwarding route is never silently shadowed by one either.
//!
//! **A locally *terminated* route is different (issue #902).** ADR 0019 has
//! the terminating connector itself derive the fulfilment from a sealed
//! shared secret -- exactly the thing a client destination's own preimage
//! must never let the connector do (a client session holds the preimage;
//! nothing about it is derivable). So a destination that resolves to *both*
//! a live session and a configured app route is never resolved by
//! precedence at all: [`route_prepare`] refuses it outright, before either
//! `handle_prepare` or the session is ever reached, as the configuration
//! error it is -- an operator's route table has overlapped a client's
//! address, and there is no silent-lookup-order answer that is safe to give.
//!
//! **`T01` vs `F02`.** [`crate::session_registry::SessionRegistry::resolve`]
//! decides, cheaply and without side effects, whether this destination is
//! even a session candidate: if nothing is currently bound there, the
//! original `F02` is returned unchanged -- "matches nothing at all" per
//! issue #736's own wording. If something IS bound, delivery is attempted
//! through it, and `SessionRegistry::deliver`'s own contract answers every
//! failure past that point -- including the session disappearing in the
//! interval between this check and the send itself -- with `T01`
//! (`RejectCode::t01_peer_unreachable`), never `F02`.
//!
//! **Charging.** A destination this arm ever delivers through has, by
//! construction, no matching app route -- one that did would already have
//! answered non-`F02` above and never reached here -- so nothing here is
//! ever priced, and a `T01` this arm answers keeps nothing, exactly like
//! `Connector::deliver_to_app`'s own `AppOutcome::Unreachable`. Before issue
//! #1269 this module also priced a mismatched fulfilment the same way a
//! terminated app route's own would (issue #736's charging AC); that check
//! is retired along with the execution condition it verified against.

use connector_btp::{BtpFrame, BTP_RESPONSE};
use connector_domain::{Fulfill, PacketResponse, Prepare, Reject, RejectCode};
use connector_runtime::ClientRouteKind;
use sha2::{Digest, Sha256};

use crate::btp::payout_voucher_protocol_data;
use crate::ClientEdgeState;

/// Route `prepare` through `state`: a configured route (app/peer/leased)
/// first, and -- only if that answers `F02` -- whatever client session
/// [`crate::session_registry::SessionRegistry`] currently has bound to its
/// destination.
///
/// **Issue #770.** A FULFILL from a client session means that client has
/// just earned `prepare`'s own amount: this connector pays it a fresh
/// voucher on its outbound x402 channel toward that client and hands it
/// over the same session (ADR 0075 decision 7, issue #1381). See
/// [`credit_session_earnings`] for both steps, for the job identity that
/// keeps a retry from being paid twice, and for why each step is
/// best-effort past the point the packet's own answer is already decided.
///
/// `client_channel_id` is the channel a covering claim admitted this
/// request on (issue #535, ADR 0036) -- `None` for an unclaimed request to
/// an unpriced/unmatched destination, the only shape that reaches this
/// arm without one. It rides straight through to the `"packet"` span
/// [`connector_runtime::Connector::handle_prepare_with_client_channel`]
/// opens; this function itself does nothing with it beyond forwarding it.
pub(crate) async fn route_prepare(
    state: &ClientEdgeState,
    prepare: Prepare,
    client_channel_id: Option<&str>,
) -> PacketResponse {
    let now = crate::now_unix();
    let Some(lease) = state.session_registry.resolve(&prepare.destination, now) else {
        // No session is bound here right now -- not even a candidate for
        // this arm, so the ordinary three-source answer (most likely `F02`)
        // stands unchanged.
        return state
            .connector
            .handle_prepare_with_client_channel(prepare, client_channel_id)
            .await;
    };

    // Issue #902: a live session and a locally terminated app route can
    // never both match one destination. Checked, and refused, before either
    // `handle_prepare` or the session is touched -- neither "the app route
    // silently wins" (ADR 0019's derivation on a destination whose preimage
    // it does not hold) nor "the session silently wins" (shadowing an
    // operator's own route table, the precedence `handle_prepare` below
    // still protects for a forwarding route) is a safe default here.
    if state
        .connector
        .client_route(&prepare.destination)
        .is_some_and(|route| route.kind == ClientRouteKind::Terminated)
    {
        return reject_overlapping_termination(&prepare.destination);
    }

    let destination = prepare.destination.clone();
    let amount = prepare.amount;
    let encoded = prepare.encode();
    // The payout dedupe key (issue #770 AC3): a hash of the exact PREPARE
    // bytes this connector is about to hand the session, taken *before* the
    // session ever answers. That order is the whole property -- a session's
    // own FULFILL rides home unchecked since ADR 0069, so anything read from
    // it is payee-forgeable and cannot identify "the same job" against a
    // retry. `job_id` is fixed by what THIS connector asked for, exactly the
    // role the sender-minted execution condition played before it left the
    // wire: a genuine retransmission of one job re-sends byte-identical
    // `data` (ADR 0018's sealed wrap fixes its own ciphertext at encryption
    // time), so it hashes to the same `job_id`; a different job's freshly
    // sealed `data` does not.
    let job_id: [u8; 32] = Sha256::digest(&encoded).into();
    let response = state
        .connector
        .handle_prepare_with_client_channel(prepare, client_channel_id)
        .await;
    if !is_unreachable(&response) {
        // A configured route decided this packet -- never silently
        // overridden by a live session (issue #736's precedence AC).
        return response;
    }

    let response = match state
        .session_registry
        .deliver(&destination, Some(lease.generation), &[], &encoded, now)
        .await
    {
        Ok(frame) => session_answer(frame),
        Err(reject) => return PacketResponse::Reject(reject),
    };

    if matches!(response, PacketResponse::Fulfill(_)) {
        credit_session_earnings(state, &destination, lease.generation, &job_id, amount, now).await;
    }

    response
}

/// Issue #770's wiring point: `destination` (the address `prepare` was
/// just genuinely fulfilled at) is a client session's own bound *ILP
/// address* (issue #736/toon-client#503) -- never a channel id.
/// [`crate::claim_gate::ClientClaimGate::credit_session_payout`] resolves
/// it to the session's payee, the key an earlier verified voucher or
/// channel-control proof on this session taught the gate (issues #787,
/// #1381).
///
/// Both steps are best-effort once reached, and neither holds up
/// `route_prepare`'s own answer (already decided by the time this runs):
///
/// 1. [`crate::claim_gate::ClientClaimGate::credit_session_payout`] signs
///    `amount` more as a voucher on this node's outbound x402 channel toward
///    that payee (ADR 0075 decision 7), deduped against `job_id` (issue
///    #770's AC3) so a duplicate or retransmitted fulfilment of the same job
///    cannot pay twice. A node with no payout ledger, a destination with no
///    payee yet, or a payee with no open channel toward it, pays nothing.
///
///    `job_id` is a hash of the PREPARE this connector sent the session
///    ([`route_prepare`]'s own doc), never anything read from the session's
///    answer: since ADR 0069 made a session's FULFILL ride home unchecked,
///    keying on anything the session supplies (its `fulfillment`, say)
///    would let a dishonest session answer a retried job with fresh bytes
///    each time and collect a fresh payout on every retry. The identity of
///    "the same job" is decided by what was asked, not by what was
///    answered.
/// 2. [`deliver_pending_claim`] flushes whatever voucher toward this
///    session's payee is still unacknowledged -- called unconditionally,
///    whether or not step 1 itself signed one (issue #779). A voucher is
///    cumulative, so the latest one carries forward anything an earlier
///    delivery failed to hand off: this is how a payout whose delivery
///    failed gets resent rather than stranded.
async fn credit_session_earnings(
    state: &ClientEdgeState,
    destination: &str,
    generation: u64,
    job_id: &[u8; 32],
    amount: u64,
    now: u64,
) {
    let _ = state
        .claim_gate
        .credit_session_payout(destination, job_id, amount)
        .await;
    deliver_pending_claim(state, destination, Some(generation), now).await;
}

/// Issue #779: resend whatever payout voucher
/// [`crate::outbound_ledger::ClientPayoutLedger::pending_for`] still holds
/// unacknowledged for `destination`'s payee, over its currently bound
/// session, fenced against `expected_generation` exactly like every other
/// delivery this module makes -- one TRANSFER per payout channel.
///
/// The TRANSFER's own `amount` field carries the voucher's cumulative
/// amount rather than any one job's increment: this call has no specific
/// job to attach to (a stranded voucher from an earlier failed delivery, or
/// a bare reconnect), and the voucher itself -- not this field -- is what a
/// client lands.
///
/// Best-effort like every step this module takes past a packet's own
/// answer: no live session, no payee yet, no payout ledger configured, or
/// nothing pending all leave the ledger exactly where it was. Only a
/// RESPONSE acknowledges a voucher; every other outcome (no session, a
/// write that never lands, a timeout, or the client's own ERROR) leaves it
/// pending to be resent.
///
/// Two production call sites: [`credit_session_earnings`] above (every
/// fulfilled delivery, deduped or not) and `crate::btp::handle_frame`'s auth
/// branch, once a session (re)establishes. The two can run at once, which
/// costs at worst one duplicate TRANSFER of the *same* voucher: it is
/// cumulative, so the client lands the same figure either way.
pub(crate) async fn deliver_pending_claim(
    state: &ClientEdgeState,
    destination: &str,
    expected_generation: Option<u64>,
    now: u64,
) {
    let Some((payee, ledger)) = state.claim_gate.payout_for_session(destination) else {
        return;
    };
    for payout in ledger.pending_for(&payee) {
        let answer = state
            .session_registry
            .deliver_transfer(
                destination,
                expected_generation,
                u64::try_from(payout.cumulative_amount()).unwrap_or(u64::MAX),
                &[payout_voucher_protocol_data(&payout)],
                now,
            )
            .await;
        // Only a RESPONSE is an acknowledgement. `deliver_transfer` answers
        // `Ok` with whatever frame the client correlated back, and
        // RFC-0023's ERROR is "could not accept this request" -- clearing
        // the voucher on one would strand the very payout this function
        // exists to resend.
        if answer.is_ok_and(|frame| frame.frame_type == BTP_RESPONSE) {
            ledger.acknowledge(payout.channel_id(), payout.cumulative_amount());
        }
    }
}

fn is_unreachable(response: &PacketResponse) -> bool {
    matches!(
        response,
        PacketResponse::Reject(reject) if reject.code == RejectCode::f02_unreachable()
    )
}

/// Turn a session's own RESPONSE frame into this hop's [`PacketResponse`]:
/// its `ilp_packet` is a FULFILL or a REJECT, the same as any answer to a
/// PREPARE this connector originated would be. A candidate FULFILL rides
/// home unchecked (issue #1269 / ADR 0069) -- the same as
/// `Connector::forward_via_peer_route` now trusts a peer's relayed
/// fulfilment outright, since a client session is exactly as untrusted as a
/// peer is and checking it here protected nothing this connector owns: the
/// sender's own end-to-end check (`connector send` against its own
/// `derive_fulfillment`) is what a forged fulfilment actually meets. A
/// REJECT the session raised itself rides home unchanged. Content this
/// carriage cannot decode as either is treated as unreachable (`T01`), the
/// same as no answer at all.
fn session_answer(frame: BtpFrame) -> PacketResponse {
    if let Ok(fulfill) = Fulfill::decode(&frame.ilp_packet) {
        return PacketResponse::Fulfill(fulfill);
    }
    match Reject::decode(&frame.ilp_packet) {
        Ok(reject) => PacketResponse::Reject(reject),
        Err(_) => PacketResponse::Reject(undecodable_session_answer()),
    }
}

/// Issue #902's guard: `destination` matches both a live client session and
/// a configured app route, which -- unlike an overlap with a forwarding
/// route -- has no safe precedence rule, only two silently-wrong ones (the
/// app route deriving a fulfilment the client's preimage should have
/// produced, or the session shadowing the operator's own routing table).
/// `T00` (retryable) rather than a final code: the packet is not at fault,
/// only this connector's own route table is, and the sender should retry
/// once an operator has resolved the overlap. `accumulated_cost` is `0` --
/// like `F02`/a dead-session `T01`, nothing was ever delivered anywhere.
fn reject_overlapping_termination(destination: &str) -> PacketResponse {
    tracing::error!(
        destination,
        "a live client session and a locally terminated app route both match this \
         destination -- refusing rather than silently deriving a fulfilment for a \
         client-held preimage (issue #902)"
    );
    PacketResponse::Reject(Reject {
        code: RejectCode::t00_internal_error(),
        triggered_by: String::new(),
        message: "destination matches both a client session and a locally terminated route"
            .to_string(),
        data: Vec::new(),
        accumulated_cost: 0,
    })
}

fn undecodable_session_answer() -> Reject {
    Reject {
        code: RejectCode::t01_peer_unreachable(),
        triggered_by: String::new(),
        message: "client session answered with a packet that is neither a fulfill nor a reject"
            .to_string(),
        data: Vec::new(),
        accumulated_cost: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use connector_btp::{
        decode_frame, BtpSessionHandle, OutboundRequests, BTP_ERROR, BTP_RESPONSE, BTP_TRANSFER,
        PAYOUT_CLAIM_PROTOCOL,
    };
    use connector_config::StaticRoute;
    use connector_runtime::{
        Connector, FakeAppClient, InMemoryJournal, InProcessPeerTransport, TestClock,
    };
    use connector_settlement::batch::VoucherSigner;
    use connector_signer::{LocalSigner, Signer};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    use crate::claim_gate::ClientClaimGate;
    use crate::outbound_ledger::{test_ledger_paying, ClientPayoutLedger};
    use crate::session_registry::SessionRegistry;

    const FULFILLMENT: [u8; 32] = [7u8; 32];

    fn test_clock() -> Arc<TestClock> {
        Arc::new(TestClock::new(
            Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
        ))
    }

    fn test_signer() -> Arc<dyn Signer> {
        Arc::new(LocalSigner::generate("session-route-test"))
    }

    fn test_state(connector: Arc<Connector>, session_registry: SessionRegistry) -> ClientEdgeState {
        test_state_with_gate(
            connector,
            session_registry,
            ClientClaimGate::restore(Arc::new(InMemoryJournal::new()))
                .expect("a fresh in-memory journal has nothing to replay"),
        )
    }

    /// As [`test_state`], but with a caller-supplied [`ClientClaimGate`] --
    /// issue #770's own tests need one carrying a real
    /// [`crate::outbound_ledger::ClientPayoutLedger`], which [`test_state`]
    /// deliberately never configures (every pre-#770 session-routing test
    /// relies on that).
    fn test_state_with_gate(
        connector: Arc<Connector>,
        session_registry: SessionRegistry,
        claim_gate: ClientClaimGate,
    ) -> ClientEdgeState {
        ClientEdgeState {
            connector,
            signer: test_signer(),
            claim_gate: claim_gate.into(),
            wrap_receiver_secret: None,
            node: Arc::new(connector_domain::NodeFacts::default()),
            btp_session_window: crate::DEFAULT_BTP_SESSION_WINDOW,
            session_registry: Arc::new(session_registry),
            // A node that mounts no peer carriage (issue #678): every
            // interaction on its listeners is a client's, which is the only
            // audience session routing has.
            peers: None,
            identities: Arc::from([]),
        }
    }

    fn empty_connector() -> Arc<Connector> {
        Arc::new(Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ))
    }

    fn sample_prepare(destination: &str) -> Prepare {
        Prepare {
            amount: 0,
            expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
            greeting: false,
            destination: destination.to_string(),
            data: Vec::new(),
        }
    }

    /// A handle over a real channel pair, mirroring
    /// `session_registry.rs`'s own `test_handle` -- the reply half a test
    /// can act as "the client" over, and the same `OutboundRequests` the
    /// handle wraps, kept alongside to answer through `outbound.resolve`.
    fn test_handle() -> (
        BtpSessionHandle,
        mpsc::Receiver<Vec<u8>>,
        Arc<OutboundRequests>,
    ) {
        let (replies, reply_rx) = mpsc::channel::<Vec<u8>>(4);
        let outbound = Arc::new(OutboundRequests::new());
        let handle = BtpSessionHandle::new(replies, Arc::clone(&outbound));
        (handle, reply_rx, outbound)
    }

    /// A handle whose send half is already gone -- simulates a session that
    /// died between `resolve` finding it and delivery reaching it.
    fn dead_handle() -> BtpSessionHandle {
        let (replies, reply_rx) = mpsc::channel::<Vec<u8>>(4);
        drop(reply_rx);
        BtpSessionHandle::new(replies, Arc::new(OutboundRequests::new()))
    }

    /// Read the one written MESSAGE off `reply_rx` and answer it with a
    /// RESPONSE carrying `ilp_packet`, exactly what a live client session
    /// does after receiving a forwarded PREPARE.
    async fn answer_next_message(
        reply_rx: &mut mpsc::Receiver<Vec<u8>>,
        outbound: &OutboundRequests,
        ilp_packet: Vec<u8>,
    ) {
        let sent = reply_rx.recv().await.expect("the MESSAGE was written");
        let decoded = decode_frame(&sent).expect("the connector's own encoder");
        outbound.resolve(BtpFrame {
            frame_type: BTP_RESPONSE,
            request_id: decoded.request_id,
            amount: None,
            protocol_data: Vec::new(),
            ilp_packet,
        });
    }

    #[tokio::test]
    async fn a_destination_with_no_session_and_no_route_answers_the_ordinary_f02() {
        let state = test_state(empty_connector(), SessionRegistry::new());
        let prepare = sample_prepare("g.nowhere");

        let response = route_prepare(&state, prepare, None).await;

        let PacketResponse::Reject(reject) = response else {
            panic!("expected a reject");
        };
        assert_eq!(reject.code, RejectCode::f02_unreachable());
        assert_eq!(
            reject.accumulated_cost, 0,
            "an F02 that never reached a session keeps nothing"
        );
    }

    #[tokio::test]
    async fn a_prepare_to_a_bound_session_is_delivered_and_fulfilled() {
        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, outbound) = test_handle();
        registry.bind("g.provider.one", handle, crate::now_unix());
        let state = test_state(empty_connector(), registry);

        let prepare = sample_prepare("g.provider.one");

        let peer = tokio::spawn(async move {
            answer_next_message(
                &mut reply_rx,
                &outbound,
                Fulfill {
                    fulfillment: FULFILLMENT,
                    data: Vec::new(),
                }
                .encode(),
            )
            .await;
        });

        let response = route_prepare(&state, prepare, None).await;
        peer.await.expect("the peer task");

        assert!(
            matches!(response, PacketResponse::Fulfill(fulfill) if fulfill.fulfillment == FULFILLMENT),
            "a live session's own fulfilment rides home"
        );
    }

    /// Issue #1269 / ADR 0069: a client session's FULFILL rides home
    /// unchecked, exactly like a peer's now does
    /// (`connector-runtime`'s `a_peers_fulfillment_rides_home_unchecked`) --
    /// a session is exactly as untrusted as a peer, and verifying its
    /// fulfilment against the packet's execution condition used to be the
    /// one thing standing between this hop and trusting its word outright,
    /// while protecting nothing this hop was paid to check.
    #[tokio::test]
    async fn a_sessions_fulfillment_rides_home_unchecked() {
        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, outbound) = test_handle();
        registry.bind("g.provider.two", handle, crate::now_unix());
        let state = test_state(empty_connector(), registry);

        let prepare = sample_prepare("g.provider.two");
        let bogus_fulfillment = [9u8; 32];

        let peer = tokio::spawn(async move {
            answer_next_message(
                &mut reply_rx,
                &outbound,
                Fulfill {
                    fulfillment: bogus_fulfillment,
                    data: Vec::new(),
                }
                .encode(),
            )
            .await;
        });

        let response = route_prepare(&state, prepare, None).await;
        peer.await.expect("the peer task");

        assert!(
            matches!(response, PacketResponse::Fulfill(fulfill) if fulfill.fulfillment == bogus_fulfillment),
            "whatever the session answers with rides home, unchecked"
        );
    }

    #[tokio::test]
    async fn a_session_that_dies_between_resolve_and_delivery_answers_t01_not_f02() {
        let registry = SessionRegistry::new();
        registry.bind("g.provider.three", dead_handle(), crate::now_unix());
        let state = test_state(empty_connector(), registry);

        let prepare = sample_prepare("g.provider.three");
        let response = route_prepare(&state, prepare, None).await;

        let PacketResponse::Reject(reject) = response else {
            panic!("expected a reject");
        };
        assert_eq!(
            reject.code,
            RejectCode::t01_peer_unreachable(),
            "a destination that looked like a live session but had no live binding is T01, not F02"
        );
        assert_eq!(
            reject.accumulated_cost, 0,
            "no value is kept for an unreachable session"
        );
    }

    /// Issue #902's AC3, the regression test named in the issue's own
    /// wording: "a client-destined PREPARE whose client never answers must
    /// reject; it must not fulfil." This is what would catch a future
    /// refactor that re-introduced ADR 0019's derivation on this path -- if
    /// `route_prepare` ever grew a fallback that synthesized a fulfilment
    /// when a session-bound destination's client stays silent (rather than
    /// genuinely waiting on the client, which alone holds the preimage),
    /// this is the test that would fail. `start_paused` lets the real
    /// [`connector_btp::OUTBOUND_ANSWER_TIMEOUT`] (30s) elapse without the
    /// test itself waiting on it -- tokio fast-forwards a paused clock to
    /// the next timer once nothing else is runnable.
    #[tokio::test(start_paused = true)]
    async fn a_client_destined_prepare_whose_client_never_answers_rejects_and_never_fulfils() {
        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, _outbound) = test_handle();
        registry.bind("g.provider.silent", handle, crate::now_unix());
        let state = test_state(empty_connector(), registry);

        let prepare = sample_prepare("g.provider.silent");

        // Deliberately no peer task: nothing ever answers the forwarded
        // MESSAGE, the exact shape of a client that is bound but has gone
        // quiet (asleep, network-partitioned, or simply slow).
        let response = route_prepare(&state, prepare, None).await;

        assert!(
            reply_rx.try_recv().is_ok(),
            "the PREPARE must actually have been forwarded to the client -- this is not \
             testing an unreachable session, it is testing a reachable one that stays silent"
        );

        let PacketResponse::Reject(reject) = response else {
            panic!(
                "a client destination whose client never answered must never be fulfilled -- \
                 the connector holds no preimage to derive one from"
            );
        };
        assert_eq!(
            reject.code,
            RejectCode::t01_peer_unreachable(),
            "a client that never answers is unreachable, not a reason to invent a fulfilment"
        );
        assert_eq!(
            reject.accumulated_cost, 0,
            "no value is kept for a delivery nobody ever answered"
        );
    }

    /// Issue #902: a configured app route (ADR 0019 -- this connector
    /// derives the fulfilment from the sealed shared secret) and a live
    /// client session (the client itself holds the preimage) can never
    /// both cover the same destination. Before #902 this was resolved by
    /// silent precedence -- the app route always won. That is exactly the
    /// failure #902 exists to close: a seller accidentally wired as a route
    /// termination would still get paid, with the connector deriving a
    /// fulfilment the client's own preimage was supposed to gate, and the
    /// hashlock would silently disappear. Both halves are watched -- the
    /// app client's `deliveries()` and the session's reply half -- so the
    /// test asserts directly that neither was reached: the connector
    /// answers the configuration-error reject itself, before dispatch.
    #[tokio::test]
    async fn a_destination_matching_both_an_app_route_and_a_session_is_a_configuration_error() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(
            route.handler_url(),
            connector_runtime::AppOutcome::Answered {
                response: connector_domain::EnvelopeResponse {
                    status: 200,
                    headers: vec![],
                    body: b"the configured app, not the session".to_vec(),
                },
            },
        );
        let signer = test_signer();
        let connector = Arc::new(
            Connector::new(
                vec![route],
                vec![],
                app_client.clone(),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(signer.clone()),
        );

        // A session is ALSO bound at the exact address the app route
        // covers. Its reply half is kept (rather than dropped) so the
        // assertions below can read it: anything written there would mean
        // `route_prepare` had tried to deliver through the session.
        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, _outbound) = test_handle();
        registry.bind("g.example.app", handle, crate::now_unix());
        let mut state = test_state(Arc::clone(&connector), registry);
        state.signer = signer.clone();

        let envelope = connector_domain::EnvelopeRequest {
            method: "POST".to_string(),
            target: "/".to_string(),
            headers: vec![],
            body: b"hello".to_vec(),
        }
        .encode();
        let (data, _shared_secret) =
            connector_signer::giftwrap::seal_request(&envelope, &signer.public_key().unwrap())
                .expect("seal");
        let prepare = Prepare {
            data,
            ..sample_prepare("g.example.app")
        };

        let response = route_prepare(&state, prepare, None).await;

        let PacketResponse::Reject(reject) = response else {
            panic!("expected a reject, not a fulfilment the connector has no right to derive");
        };
        assert_eq!(
            reject.code,
            RejectCode::t00_internal_error(),
            "an overlapping app route and session is this connector's own configuration \
             error, reported as one -- not a value judgement about the packet"
        );
        assert_eq!(
            reject.accumulated_cost, 0,
            "nothing was delivered anywhere, so nothing is kept"
        );
        assert!(
            app_client.deliveries().is_empty(),
            "the app route must never have been reached -- a delivery here is the derived \
             fulfilment #902 exists to prevent"
        );
        assert!(
            reply_rx.try_recv().is_err(),
            "the session must never have been reached either -- refusing the overlap is not \
             the same as letting the session quietly win it"
        );
    }

    /// The forwarding-route half of #902's precedence note: unlike an app
    /// route (above), a peer route never derives a fulfilment locally, so
    /// an overlapping session is not a configuration error -- the existing
    /// "a configured route always wins" rule (issue #736) still applies,
    /// unchanged by #902.
    #[tokio::test]
    async fn a_configured_peer_route_still_outranks_an_overlapping_session() {
        // ADR 0042: a peering with nothing to pay it from is refused
        // before the transport is ever reached (issue #1145), so the hop
        // has to be covered for this test to be about routing at all.
        let connector = Arc::new(crate::tests::covering(
            Connector::new(
                vec![],
                vec![connector_runtime::PeerRoute::new(
                    "g.example.peer",
                    "peer-a",
                )],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ),
            "peer-a",
        ));

        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, _outbound) = test_handle();
        registry.bind("g.example.peer", handle, crate::now_unix());
        let state = test_state(connector, registry);

        let prepare = sample_prepare("g.example.peer");

        let response = route_prepare(&state, prepare, None).await;

        // `InProcessPeerTransport` with no peer named "peer-a" registered
        // answers `T01` naming that peer -- which is what makes this
        // distinguishable from the session's own `T01`, and from #902's
        // `T00`: the peer route was the one consulted.
        let PacketResponse::Reject(reject) = response else {
            panic!("expected a reject from the (unregistered) peer transport");
        };
        assert_eq!(
            reject.code,
            RejectCode::t01_peer_unreachable(),
            "a peer route overlapping a session is not #902's configuration error -- it is \
             still routed, and answers whatever the peer transport answers"
        );
        assert!(
            reject.message.contains("peer-a"),
            "the peer transport's own answer, not the session's: {}",
            reject.message
        );
        assert!(
            reply_rx.try_recv().is_err(),
            "the configured peer route must have won outright -- the session is never \
             delivered to while a configured route answers (issue #736)"
        );
    }

    #[tokio::test]
    async fn delivery_after_a_reconnect_reaches_only_the_newer_session() {
        let registry = SessionRegistry::new();
        let (old_handle, mut old_rx, _old_outbound) = test_handle();
        registry.bind("g.provider.four", old_handle, crate::now_unix());

        let (new_handle, mut new_rx, new_outbound) = test_handle();
        registry.bind("g.provider.four", new_handle, crate::now_unix());

        let state = test_state(empty_connector(), registry);
        let prepare = sample_prepare("g.provider.four");

        let peer = tokio::spawn(async move {
            answer_next_message(
                &mut new_rx,
                &new_outbound,
                Fulfill {
                    fulfillment: FULFILLMENT,
                    data: Vec::new(),
                }
                .encode(),
            )
            .await;
        });

        let response = route_prepare(&state, prepare, None).await;
        peer.await.expect("the peer task");

        assert!(matches!(response, PacketResponse::Fulfill(_)));
        assert!(
            old_rx.try_recv().is_err(),
            "the superseded session never receives a delivery meant for the newer one"
        );
    }

    // ─── issue #770 / #1381: a fulfilled session delivery pays a voucher ───

    /// The client's payee key in these tests: the receiver of the payout
    /// channel [`test_ledger_paying`] opens.
    const PAYEE: [u8; 20] = [0x2a; 20];

    fn payee() -> VoucherSigner {
        VoucherSigner::Evm(PAYEE)
    }

    /// A gate over `ledger` that already knows `address` is paid at
    /// [`payee`] -- what a verified voucher or channel-control proof on this
    /// session teaches it (`crate::btp::record_accepted_claim`).
    fn gate_paying(address: &str, ledger: &Arc<ClientPayoutLedger>) -> ClientClaimGate {
        let gate = ClientClaimGate::restore(Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay")
            .with_payout_ledger(Arc::clone(ledger));
        gate.record_session_payee(address, payee());
        gate
    }

    /// The production wiring, at the real call site (issue #770's AC4: this
    /// fails if the call in [`route_prepare`] is deleted): a FULFILL from
    /// the session bound at `address` signs a voucher on the payout channel
    /// toward its payee and hands it over the same session as a TRANSFER
    /// carrying the voucher JSON (ADR 0075 decision 7).
    ///
    /// **Issue #787.** The session is bound under an ILP address, never a
    /// channel id -- the shape production actually reaches.
    #[tokio::test]
    async fn a_fulfilled_session_delivery_pays_a_voucher_over_the_session() {
        let address = "g.provider.nine";
        let ledger = test_ledger_paying(PAYEE).await;
        let gate = gate_paying(address, &ledger);

        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, outbound) = test_handle();
        registry.bind(address, handle, crate::now_unix());
        let state = test_state_with_gate(empty_connector(), registry, gate);

        let prepare = Prepare {
            amount: 4_200,
            ..sample_prepare(address)
        };

        let peer = tokio::spawn(async move {
            answer_next_message(
                &mut reply_rx,
                &outbound,
                Fulfill {
                    fulfillment: FULFILLMENT,
                    data: Vec::new(),
                }
                .encode(),
            )
            .await;

            let sent = reply_rx
                .recv()
                .await
                .expect("the payout TRANSFER was written");
            let decoded = decode_frame(&sent).expect("the connector's own encoder");
            assert_eq!(decoded.frame_type, BTP_TRANSFER);
            assert_eq!(decoded.amount, Some(4_200));

            let pd = decoded
                .protocol_data
                .iter()
                .find(|pd| pd.name == PAYOUT_CLAIM_PROTOCOL)
                .expect("the payout voucher rode the TRANSFER");
            let json: serde_json::Value = serde_json::from_slice(&pd.data).expect("valid JSON");
            assert_eq!(json["scheme"], "batch-settlement");
            assert_eq!(json["maxClaimableAmount"], "4200");
            assert_eq!(
                json["channelConfig"]["receiver"],
                format!("0x{}", hex::encode(PAYEE)),
                "the voucher is on the channel that pays this client"
            );

            outbound.resolve(BtpFrame {
                frame_type: BTP_RESPONSE,
                request_id: decoded.request_id,
                amount: None,
                protocol_data: Vec::new(),
                ilp_packet: Vec::new(),
            });
        });

        let response = route_prepare(&state, prepare, None).await;
        peer.await.expect("the peer task");

        assert!(
            matches!(response, PacketResponse::Fulfill(fulfill) if fulfill.fulfillment == FULFILLMENT),
            "the original packet still answers fulfilled -- paying rides alongside it, never blocking it"
        );
        assert_eq!(ledger.signed_toward(&payee()), 4_200);
        assert!(
            ledger.pending_for(&payee()).is_empty(),
            "the client's RESPONSE acknowledged the voucher"
        );
    }

    /// Issue #770's AC3, at the real call site: a job delivered twice to
    /// the same live session (a sender's own retry, byte-identical) is
    /// paid once, though the session answers both deliveries fulfilled.
    #[tokio::test]
    async fn a_retried_delivery_of_the_same_job_is_paid_once() {
        let address = "g.provider.eleven";
        let ledger = test_ledger_paying(PAYEE).await;
        let gate = gate_paying(address, &ledger);

        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, outbound) = test_handle();
        registry.bind(address, handle, crate::now_unix());
        let state = test_state_with_gate(empty_connector(), registry, gate);

        // The session answers whatever it is sent -- a MESSAGE with a
        // genuine FULFILL, a TRANSFER with an empty ack -- exactly as a
        // real client's BTP session would.
        let peer = tokio::spawn(async move {
            for _ in 0..3 {
                let sent = reply_rx.recv().await.expect("a frame was written");
                let decoded = decode_frame(&sent).expect("the connector's own encoder");
                let ilp_packet = if decoded.frame_type == BTP_TRANSFER {
                    Vec::new()
                } else {
                    Fulfill {
                        fulfillment: FULFILLMENT,
                        data: Vec::new(),
                    }
                    .encode()
                };
                outbound.resolve(BtpFrame {
                    frame_type: BTP_RESPONSE,
                    request_id: decoded.request_id,
                    amount: None,
                    protocol_data: Vec::new(),
                    ilp_packet,
                });
            }
        });

        for _ in 0..2 {
            let job = Prepare {
                amount: 5_000,
                ..sample_prepare(address)
            };
            assert!(matches!(
                route_prepare(&state, job, None).await,
                PacketResponse::Fulfill(_)
            ));
        }
        peer.await.expect("the peer task");

        assert_eq!(
            ledger.signed_toward(&payee()),
            5_000,
            "a retried delivery of the same job must not be paid a second time"
        );
    }

    /// Issue #1269 / ADR 0069's regression: a dishonest session answering
    /// the same retried job with a *different* fulfilment each time is still
    /// paid once -- the dedupe key is the job this connector asked for,
    /// never anything the session supplies.
    #[tokio::test]
    async fn a_session_answering_the_same_job_with_a_different_fulfilment_each_time_is_still_deduped(
    ) {
        let address = "g.provider.dishonest";
        let ledger = test_ledger_paying(PAYEE).await;
        let gate = gate_paying(address, &ledger);

        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, outbound) = test_handle();
        registry.bind(address, handle, crate::now_unix());
        let state = test_state_with_gate(empty_connector(), registry, gate);

        let peer = tokio::spawn(async move {
            for fulfillment in [[0xaa_u8; 32], [0xbb_u8; 32], [0xcc_u8; 32]] {
                let sent = reply_rx.recv().await.expect("a frame was written");
                let decoded = decode_frame(&sent).expect("the connector's own encoder");
                let ilp_packet = if decoded.frame_type == BTP_TRANSFER {
                    Vec::new()
                } else {
                    Fulfill {
                        fulfillment,
                        data: Vec::new(),
                    }
                    .encode()
                };
                outbound.resolve(BtpFrame {
                    frame_type: BTP_RESPONSE,
                    request_id: decoded.request_id,
                    amount: None,
                    protocol_data: Vec::new(),
                    ilp_packet,
                });
            }
        });

        for _ in 0..2 {
            let job = Prepare {
                amount: 5_000,
                ..sample_prepare(address)
            };
            assert!(matches!(
                route_prepare(&state, job, None).await,
                PacketResponse::Fulfill(_)
            ));
        }
        peer.await.expect("the peer task");

        assert_eq!(ledger.signed_toward(&payee()), 5_000);
    }

    /// Issue #787's own scenario under ADR 0075 decision 7: a session that
    /// has never proved a payee -- no voucher, no channel-control proof --
    /// is paid nothing and sent no TRANSFER, and the packet still answers
    /// fulfilled.
    #[tokio::test]
    async fn a_destination_with_no_known_payee_is_not_paid() {
        let address = "g.provider.unpaid";
        let ledger = test_ledger_paying(PAYEE).await;
        let gate = ClientClaimGate::restore(Arc::new(InMemoryJournal::new()))
            .expect("a fresh in-memory journal has nothing to replay")
            .with_payout_ledger(Arc::clone(&ledger));

        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, outbound) = test_handle();
        registry.bind(address, handle, crate::now_unix());
        let state = test_state_with_gate(empty_connector(), registry, gate);

        let peer = tokio::spawn(async move {
            answer_next_message(
                &mut reply_rx,
                &outbound,
                Fulfill {
                    fulfillment: FULFILLMENT,
                    data: Vec::new(),
                }
                .encode(),
            )
            .await;
            reply_rx
        });

        let prepare = Prepare {
            amount: 3_000,
            ..sample_prepare(address)
        };
        let response = route_prepare(&state, prepare, None).await;
        let mut reply_rx = peer.await.expect("the peer task");
        assert!(
            reply_rx.try_recv().is_err(),
            "a session with no known payee must never receive a payout TRANSFER"
        );
        assert!(matches!(response, PacketResponse::Fulfill(_)));
        assert_eq!(ledger.signed_toward(&payee()), 0);
    }

    /// A payee with no channel open toward it is paid nothing: this node
    /// never opens or funds a payout channel on its own (ADR 0075 decision
    /// 11), and the packet still answers fulfilled.
    #[tokio::test]
    async fn a_payee_with_no_open_channel_toward_it_is_not_paid() {
        let address = "g.provider.unchanneled";
        let ledger = test_ledger_paying([0x77; 20]).await;
        let gate = gate_paying(address, &ledger);

        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, outbound) = test_handle();
        registry.bind(address, handle, crate::now_unix());
        let state = test_state_with_gate(empty_connector(), registry, gate);

        let peer = tokio::spawn(async move {
            answer_next_message(
                &mut reply_rx,
                &outbound,
                Fulfill {
                    fulfillment: FULFILLMENT,
                    data: Vec::new(),
                }
                .encode(),
            )
            .await;
            reply_rx
        });
        let prepare = Prepare {
            amount: 3_000,
            ..sample_prepare(address)
        };
        let response = route_prepare(&state, prepare, None).await;
        let mut reply_rx = peer.await.expect("the peer task");
        assert!(reply_rx.try_recv().is_err(), "nothing was signed to send");
        assert!(matches!(response, PacketResponse::Fulfill(_)));
    }

    // ─── issue #779: a payout voucher whose delivery fails is resent ───

    /// Through the real call site (`credit_session_earnings`), per the
    /// issue's AC4: a first delivery pays and then loses its TRANSFER (the
    /// session's reply channel is dropped right after it answers the
    /// FULFILL); the voucher stays pending. A retry of the *same job* --
    /// deduped, so nothing new is signed -- still flushes it, proving the
    /// resend runs unconditionally.
    #[tokio::test]
    async fn a_stranded_payout_voucher_is_resent_on_the_next_successful_delivery_even_when_deduped()
    {
        let address = "g.provider.stranded";
        let ledger = test_ledger_paying(PAYEE).await;
        let gate = gate_paying(address, &ledger);

        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, outbound) = test_handle();
        registry.bind(address, handle, crate::now_unix());
        let state = test_state_with_gate(empty_connector(), registry, gate);

        let peer = tokio::spawn(async move {
            answer_next_message(
                &mut reply_rx,
                &outbound,
                Fulfill {
                    fulfillment: FULFILLMENT,
                    data: Vec::new(),
                }
                .encode(),
            )
            .await;
            drop(reply_rx);
        });
        let first = Prepare {
            amount: 5_000,
            ..sample_prepare(address)
        };
        assert!(matches!(
            route_prepare(&state, first, None).await,
            PacketResponse::Fulfill(_)
        ));
        peer.await.expect("the peer task");

        assert_eq!(
            ledger.signed_toward(&payee()),
            5_000,
            "the payout must not depend on its TRANSFER's delivery succeeding"
        );
        assert_eq!(
            ledger.pending_for(&payee()).len(),
            1,
            "a failed delivery leaves the voucher pending for the next caller to find"
        );

        let (handle2, mut reply_rx2, outbound2) = test_handle();
        state
            .session_registry
            .bind(address, handle2, crate::now_unix());
        let peer2 = tokio::spawn(async move {
            answer_next_message(
                &mut reply_rx2,
                &outbound2,
                Fulfill {
                    fulfillment: FULFILLMENT,
                    data: Vec::new(),
                }
                .encode(),
            )
            .await;

            let sent = reply_rx2
                .recv()
                .await
                .expect("the stranded payout TRANSFER was resent");
            let decoded = decode_frame(&sent).expect("the connector's own encoder");
            assert_eq!(decoded.frame_type, BTP_TRANSFER);
            let pd = decoded
                .protocol_data
                .iter()
                .find(|pd| pd.name == PAYOUT_CLAIM_PROTOCOL)
                .expect("the stranded voucher rode this TRANSFER");
            let json: serde_json::Value = serde_json::from_slice(&pd.data).expect("valid JSON");
            assert_eq!(json["maxClaimableAmount"], "5000");

            outbound2.resolve(BtpFrame {
                frame_type: BTP_RESPONSE,
                request_id: decoded.request_id,
                amount: None,
                protocol_data: Vec::new(),
                ilp_packet: Vec::new(),
            });
            reply_rx2
        });

        let retry = Prepare {
            amount: 5_000,
            ..sample_prepare(address)
        };
        assert!(matches!(
            route_prepare(&state, retry, None).await,
            PacketResponse::Fulfill(_)
        ));
        let mut reply_rx2 = peer2.await.expect("the peer task");

        assert_eq!(
            ledger.signed_toward(&payee()),
            5_000,
            "a deduped retry pays nothing more"
        );
        assert!(
            ledger.pending_for(&payee()).is_empty(),
            "the successful resend acknowledged the voucher"
        );
        assert!(
            reply_rx2.try_recv().is_err(),
            "only one payout TRANSFER goes out -- no double send"
        );
    }

    /// A client that answers the payout TRANSFER with an ERROR frame never
    /// took the voucher, so it stays pending for the next delivery or
    /// reconnect to resend.
    #[tokio::test]
    async fn a_payout_transfer_the_client_answers_with_an_error_leaves_the_voucher_pending() {
        let address = "g.provider.refuses";
        let ledger = test_ledger_paying(PAYEE).await;
        let stranded = ledger
            .record_payout_once(payee(), &[1; 32], 4_000)
            .await
            .expect("a channel toward the payee is open");
        let gate = gate_paying(address, &ledger);

        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, outbound) = test_handle();
        registry.bind(address, handle, crate::now_unix());
        let state = test_state_with_gate(empty_connector(), registry, gate);

        let peer = tokio::spawn(async move {
            let sent = reply_rx.recv().await.expect("the TRANSFER was written");
            let decoded = decode_frame(&sent).expect("the connector's own encoder");
            assert_eq!(decoded.frame_type, BTP_TRANSFER);
            outbound.resolve(BtpFrame {
                frame_type: BTP_ERROR,
                request_id: decoded.request_id,
                amount: None,
                protocol_data: Vec::new(),
                ilp_packet: Vec::new(),
            });
        });

        deliver_pending_claim(&state, address, None, crate::now_unix()).await;
        peer.await.expect("the peer task");

        assert_eq!(
            ledger.pending_for(&payee()),
            vec![stranded],
            "a refused TRANSFER leaves the same voucher armed for the next attempt"
        );
    }
}
