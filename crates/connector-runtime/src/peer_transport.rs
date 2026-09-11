//! The peer transport port: forwards a [`Prepare`] to another connector for
//! the next hop, optionally carrying a claim (issue #423), and flushes a
//! claim on its own when nothing else is going out (peer-semantics-pre-868.md
//! §3.3).
//!
//! **This port is the seam ADR 0027 rests on.** The raw-TCP transport that
//! used to implement it was deleted in issue #679 -- it never carried a
//! production packet -- and the replacement carriages (BTP over `wss://`
//! and ILP-over-HTTP over `https://`, issue #676) will be built behind this
//! same trait. Everything above the port -- [`crate::Connector`]'s peer
//! forwarding, [`crate::ClaimBook`], fees, routing -- is carriage-agnostic
//! and was untouched by that deletion.
//!
//! Until a carriage lands, [`InProcessPeerTransport`] is the only
//! implementation: the in-process stand-in for composing multi-connector
//! tests without a socket, and -- registered with no peers -- what a
//! production node holds, so a `peer_id`-targeted route answers `T01`
//! rather than silently dropping. It is held to the contract suite in this
//! module's `tests::contract`, which each new carriage joins.

/// Why an onion endpoint cannot be dialed on a node that configured no
/// `socks_proxy` (ADR 0070 decision 3).
///
/// A dial failure and nothing more exotic: a `.onion` or `.anyone` name
/// resolves nowhere without a proxy to resolve it, so this reads like every
/// other "could not reach that host", and packets routed to the peer reject
/// `T01` with the peer and the endpoint named -- never `T00` and never a
/// silent drop (`peer-carriage-spec.md` §2.2).
///
/// **One string, shared by both carriages**, because `peer-carriage-spec.md`
/// §9 holds that peer behaviour an operator can observe on one carriage and
/// not the other is a defect -- and "what this node says when it cannot
/// reach an onion peer" is exactly such a behaviour. The two carriage crates
/// do not depend on each other, but both depend on this one, so this is
/// where a sentence they must not diverge on belongs.
pub const NO_SOCKS_PROXY: &str =
    "the endpoint's host ends in .onion or .anyone and this node configured no socks_proxy, so \
     there is nothing that can resolve or reach it (ADR 0070 decision 3)";

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use crate::peer_route_store::RuntimePeering;

use connector_domain::x402::X402PaymentRequired;
use connector_domain::{PacketResponse, Prepare, Reject, RejectCode};

use crate::claim::{ClaimAckOutcome, WireClaim};
use crate::connector::Connector;

/// What one forward to a peer produced.
///
/// A named struct rather than a tuple because of its fourth field: the
/// terms a peer quoted when it refused to carry the packet unpaid (issue
/// #874). A carriage that read a `payment-required` greeting and then
/// flattened it into a bare [`PacketResponse::Reject`] would hand the
/// forwarding path a refusal it cannot act on -- and acting on it, by
/// covering the packet and retrying once, is exactly what issue #875 adds
/// above this port.
#[derive(Debug)]
pub struct PeerForward {
    /// Whatever the peer decided, unchanged -- a reject originated at the
    /// far end reaches the caller exactly as that peer sent it.
    pub response: PacketResponse,
    /// [`ClaimAckOutcome::NotSent`] when no claim rode along, or when the
    /// peer could not be reached to answer at all.
    pub ack: ClaimAckOutcome,
    /// Whether this peer was actually reached -- `true` for any real answer
    /// (fulfil or reject) the peer itself decided on, `false` when this
    /// transport could not deliver the PREPARE at all, in which case
    /// `response` is this transport's own synthesized `T01` (see
    /// [`peer_unreachable`]). The caller (issue #426, ADR 0011) needs this
    /// to decide whether its own fee belongs on an outgoing REJECT: only a
    /// hop that actually forwarded earns one (peer-semantics-pre-868.md §5.2).
    pub reached_peer: bool,
    /// The x402 terms the peer quoted while refusing the packet (issue
    /// #874), or `None` when it quoted none. **Absence means "no terms were
    /// offered", never "the terms could not be read"**: a carriage that
    /// found a greeting it could not parse must answer with its own reject
    /// and no terms rather than degrade an unreadable greeting into a free
    /// ride (see `connector_domain::x402::GreetingError`).
    pub payment_required: Option<Box<X402PaymentRequired>>,
}

impl PeerForward {
    /// The peer answered for itself, quoting no terms.
    pub fn answered(response: PacketResponse, ack: ClaimAckOutcome) -> PeerForward {
        PeerForward {
            response,
            ack,
            reached_peer: true,
            payment_required: None,
        }
    }

    /// The peer refused the packet **and quoted terms** for carrying it.
    pub fn quoted(
        response: PacketResponse,
        ack: ClaimAckOutcome,
        terms: X402PaymentRequired,
    ) -> PeerForward {
        PeerForward {
            response,
            ack,
            reached_peer: true,
            payment_required: Some(Box::new(terms)),
        }
    }

    /// This transport never delivered the PREPARE at all: a synthesized
    /// `T01`, nothing acknowledged, and no fee of the caller's on it.
    pub fn unreachable(peer_id: &str) -> PeerForward {
        PeerForward {
            response: peer_unreachable(peer_id),
            ack: ClaimAckOutcome::NotSent,
            reached_peer: false,
            payment_required: None,
        }
    }

    /// The peer answered something this carriage could not read as an ILP
    /// packet -- a `T01` like [`PeerForward::unreachable`], except that an
    /// acknowledgement may still have been read off the frame.
    pub fn undecodable(peer_id: &str, ack: ClaimAckOutcome) -> PeerForward {
        PeerForward {
            ack,
            ..PeerForward::unreachable(peer_id)
        }
    }
}

/// Forwards a [`Prepare`] to the connector reachable at `peer_id` and
/// returns whatever that peer answered, unchanged -- a reject originated at
/// the far end reaches the caller exactly as that peer sent it. `claim`
/// piggybacks whatever this connector currently owes `peer_id`
/// (peer-semantics-pre-868.md §3.2), and is what bounds erosion across the
/// path now that no declared floor rides beside the packet (ADR 0057,
/// issue #1143).
///
/// See [`PeerForward`] for what comes back, and in particular for why a
/// carriage that read x402 terms off a refusal must report them rather than
/// flatten them into the reject.
#[async_trait]
pub trait PeerTransport: Send + Sync {
    async fn forward(
        &self,
        peer_id: &str,
        prepare: Prepare,
        claim: Option<WireClaim>,
    ) -> PeerForward;

    /// Send `claim` with no packet to ride -- the flush mechanism
    /// (peer-semantics-pre-868.md §3.3) that covers the case traffic to `peer_id`
    /// has stopped. Returns [`ClaimAckOutcome::NotSent`] if `peer_id`
    /// could not be reached.
    async fn flush(&self, peer_id: &str, claim: WireClaim) -> ClaimAckOutcome;
}

/// Adds and removes a **carriage** while the process serves (ADR 0058).
///
/// A dial transport built once at boot from `config.peers()` is what made a
/// runtime peer row hollow: the row could name a peering the node had no
/// way to reach. This port is the other half -- a peering established at
/// runtime registers its carriage here, and a removed one deregisters,
/// without a restart.
///
/// Separate from [`PeerTransport`] rather than a method on it because the
/// two have different callers and different frequencies: forwarding is the
/// packet path, and this is an operator write. An implementation may of
/// course be the same value behind both, and
/// `connector-cli`'s `ConfiguredPeerTransport` is.
pub trait PeerRegistrar: Send + Sync {
    /// Make `peering` dialable under `peer_id`, replacing whatever was
    /// registered under that id. A peering whose endpoint selects no
    /// carriage this node dials registers nothing and is left unreachable
    /// -- which is `T01` with the peer named, the answer an undialable
    /// peering has always had (`peer-carriage-spec.md` §2.2).
    fn register(&self, peer_id: &str, peering: &RuntimePeering);

    /// Stop dialing `peer_id`. A no-op for an id that was never
    /// registered.
    fn deregister(&self, peer_id: &str);
}

/// §2.2, §5.1 of `peer-semantics-pre-868.md`: a peer this connector could not reach
/// rejects `T01`. Never `T00`, and never a silent drop. Every carriage
/// reaches this through [`PeerForward::unreachable`], so the refusal an
/// unreachable peer produces has one definition rather than one per wire.
pub(crate) fn peer_unreachable(peer_id: &str) -> PacketResponse {
    PacketResponse::Reject(Reject {
        code: RejectCode::t01_peer_unreachable(),
        triggered_by: String::new(),
        message: format!("peer '{peer_id}' unreachable"),
        data: Vec::new(),
        accumulated_cost: 0,
    })
}

/// One message handed to a peer's owning task: either a [`Prepare`] to
/// forward (optionally carrying a claim), or a claim to flush on its own.
/// Both travel the same channel so the two ways a frame reaches a peer stay
/// ordered relative to each other, exactly as they would interleaved on one
/// real duplex stream.
enum PeerMessage {
    Prepare {
        prepare: Prepare,
        claim: Option<WireClaim>,
        respond_to: oneshot::Sender<(PacketResponse, ClaimAckOutcome)>,
    },
    Flush {
        claim: WireClaim,
        respond_to: oneshot::Sender<ClaimAckOutcome>,
    },
}

/// A handle to a peer [`Connector`], reachable only by message -- the
/// in-process stand-in for the peer semantics's persistent duplex stream. The
/// `Connector` behind a `PeerLink` is owned exclusively by the task spawned
/// in [`PeerLink::connect`]; nothing outside that task ever touches it
/// directly, so there is no lock on this path, on either side of it.
#[derive(Clone)]
struct PeerLink {
    sender: mpsc::Sender<PeerMessage>,
}

impl PeerLink {
    /// Spawn the task that owns `connector` for the lifetime of this link,
    /// answering every forwarded [`Prepare`] by calling
    /// [`Connector::handle_peer_prepare`] and every flush by calling
    /// [`Connector::handle_peer_claim`] -- the same claim-acceptance path a
    /// claim piggybacked on a PREPARE reaches, so a flushed claim is judged
    /// identically.
    fn connect(connector: Arc<Connector>) -> PeerLink {
        let (sender, mut receiver) = mpsc::channel::<PeerMessage>(64);
        tokio::spawn(async move {
            while let Some(message) = receiver.recv().await {
                match message {
                    PeerMessage::Prepare {
                        prepare,
                        claim,
                        respond_to,
                    } => {
                        // No arriving peering is named: this stand-in
                        // models a link, not an identity, and the
                        // `Connector` behind it has no way to know which
                        // of its peerings this `PeerLink` stands for.
                        // ADR 0071's converting arm therefore never fires
                        // through this transport -- which is the safe
                        // direction for a stand-in to be wrong in only
                        // because a node that declares no `[[tokens]]`
                        // has no crossing to miss, and a dealing node is
                        // reached over a real carriage that does
                        // authenticate the peer (issue #1295).
                        let result = connector.handle_peer_prepare(None, prepare, claim).await;
                        let _ = respond_to.send(result);
                    }
                    PeerMessage::Flush { claim, respond_to } => {
                        let ack = connector.handle_peer_claim(claim);
                        let _ = respond_to.send(ack);
                    }
                }
            }
        });
        PeerLink { sender }
    }

    async fn forward(
        &self,
        peer_id: &str,
        prepare: Prepare,
        claim: Option<WireClaim>,
    ) -> PeerForward {
        let (respond_to, receiver) = oneshot::channel();
        if self
            .sender
            .send(PeerMessage::Prepare {
                prepare,
                claim,
                respond_to,
            })
            .await
            .is_err()
        {
            return PeerForward::unreachable(peer_id);
        }
        match receiver.await {
            // An in-process peer is a `Connector`, and a `Connector` quotes
            // no terms of its own: greeting a claimless peer PREPARE is the
            // client edge's job (issue #880), above this port on the
            // receiving side.
            Ok((response, ack)) => PeerForward::answered(response, ack),
            Err(_) => PeerForward::unreachable(peer_id),
        }
    }

    async fn flush(&self, claim: WireClaim) -> ClaimAckOutcome {
        let (respond_to, receiver) = oneshot::channel();
        if self
            .sender
            .send(PeerMessage::Flush { claim, respond_to })
            .await
            .is_err()
        {
            return ClaimAckOutcome::NotSent;
        }
        receiver.await.unwrap_or(ClaimAckOutcome::NotSent)
    }
}

/// The in-process [`PeerTransport`]: every peer is another [`Connector`] in
/// this process, reached through a [`PeerLink`] rather than a socket. Peers
/// are registered once via [`InProcessPeerTransport::add_peer`] before the
/// transport is shared; the peer map itself is never mutated after that, so
/// reading it on the packet path takes no lock.
#[derive(Default)]
pub struct InProcessPeerTransport {
    peers: HashMap<String, PeerLink>,
}

impl InProcessPeerTransport {
    pub fn new() -> InProcessPeerTransport {
        InProcessPeerTransport::default()
    }

    /// Register `connector` as reachable under `peer_id`, spawning the task
    /// that owns it for the lifetime of this transport.
    pub fn add_peer(&mut self, peer_id: impl Into<String>, connector: Arc<Connector>) {
        self.peers
            .insert(peer_id.into(), PeerLink::connect(connector));
    }
}

#[async_trait]
impl PeerTransport for InProcessPeerTransport {
    async fn forward(
        &self,
        peer_id: &str,
        prepare: Prepare,
        claim: Option<WireClaim>,
    ) -> PeerForward {
        match self.peers.get(peer_id) {
            Some(link) => link.forward(peer_id, prepare, claim).await,
            None => PeerForward::unreachable(peer_id),
        }
    }

    async fn flush(&self, peer_id: &str, claim: WireClaim) -> ClaimAckOutcome {
        match self.peers.get(peer_id) {
            Some(link) => link.flush(claim).await,
            None => ClaimAckOutcome::NotSent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_client::FakeAppClient;
    use crate::clock::TestClock;
    use crate::test_support::{
        answered, expected_fulfillment, fulfill_envelope, identity_signer, open_sealed_envelope,
        sealed_envelope_request_data, sign_wire_claim, with_test_channel,
    };
    use chrono::{TimeZone, Utc};
    use connector_config::StaticRoute;
    use connector_signer::{LocalSigner, Signer};

    /// Seals a fixed body (issue #524) -- what a genuine sender does before
    /// ever transmitting a packet, so a termination reached through this
    /// derives a fulfilment for the wrap's own secret and this is, by
    /// construction, one that fulfils if it reaches an app that answers at
    /// all. A test that also needs the secret back uses [`sealed_prepare`]
    /// instead.
    fn prepare(destination: &str) -> Prepare {
        let (data, _shared_secret) = sealed_envelope_request_data(b"hello");
        Prepare {
            amount: 0,
            // Comfortably after `test_clock()`'s instant (2030-01-01).
            expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
            greeting: false,
            destination: destination.to_string(),
            data,
        }
    }

    /// A `Prepare` addressed to `"g.example.app"`, sealed to
    /// [`identity_signer`]'s identity and carrying `body` (issue #524).
    /// Returns the shared secret alongside, to open the sealed
    /// `Fulfill`/termination-`Reject` this produces, or to compute the
    /// expected fulfilment via `expected_fulfillment`.
    fn sealed_prepare(body: &[u8]) -> (Prepare, [u8; 32]) {
        let (data, shared_secret) = sealed_envelope_request_data(body);
        (
            Prepare {
                data,
                ..prepare("g.example.app")
            },
            shared_secret,
        )
    }

    fn test_clock() -> Arc<TestClock> {
        Arc::new(TestClock::new(
            Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
        ))
    }

    #[tokio::test]
    async fn forwards_to_the_registered_peer_and_returns_its_response() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(route.handler_url(), answered(b"delivered by the peer"));
        let peer = Arc::new(
            Connector::new(
                vec![route],
                vec![],
                app_client,
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(identity_signer()),
        );
        let mut transport = InProcessPeerTransport::new();
        transport.add_peer("peer-b", peer);
        let (sealed, shared_secret) = sealed_prepare(b"hello");

        let PeerForward {
            response,
            ack,
            reached_peer: reached,
            ..
        } = transport.forward("peer-b", sealed, None).await;

        match response {
            PacketResponse::Fulfill(fulfill) => {
                assert_eq!(fulfill.fulfillment, expected_fulfillment(&shared_secret));
                assert_eq!(
                    open_sealed_envelope(&shared_secret, &fulfill.data),
                    fulfill_envelope(b"delivered by the peer")
                );
            }
            other => panic!("expected a fulfill, got {other:?}"),
        }
        assert_eq!(ack, ClaimAckOutcome::NotSent);
        assert!(reached);
    }

    #[tokio::test]
    async fn returns_peer_unreachable_for_an_unregistered_peer_id() {
        let transport = InProcessPeerTransport::new();

        let PeerForward {
            response,
            ack,
            reached_peer: reached,
            ..
        } = transport
            .forward("nowhere", prepare("g.example.app"), None)
            .await;

        match response {
            PacketResponse::Reject(reject) => {
                assert_eq!(reject.code.as_str(), "T01");
                assert!(reject.message.contains("nowhere"));
            }
            other => panic!("expected a reject, got {other:?}"),
        }
        assert_eq!(ack, ClaimAckOutcome::NotSent);
        // Never reached: nothing forwarded, so no fee ever belongs to this
        // hop (ADR 0011, peer-semantics-pre-868.md §5.2).
        assert!(!reached);
    }

    #[tokio::test]
    async fn a_reject_originated_by_the_peer_is_relayed_unchanged() {
        let peer = Arc::new(Connector::new(
            vec![],
            vec![],
            Arc::new(FakeAppClient::new()),
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        ));
        let mut transport = InProcessPeerTransport::new();
        transport.add_peer("peer-b", peer);

        let PeerForward {
            response,
            reached_peer: reached,
            ..
        } = transport
            .forward("peer-b", prepare("g.nowhere-on-peer-b"), None)
            .await;

        match response {
            PacketResponse::Reject(reject) => {
                assert_eq!(reject.code.as_str(), "F02");
                assert!(reject.message.contains("g.nowhere-on-peer-b"));
            }
            other => panic!("expected a reject, got {other:?}"),
        }
        // The peer *was* reached -- it answered with its own reject.
        assert!(reached);
    }

    /// Issue #423: a claim piggybacked on a forwarded PREPARE is verified
    /// by the accepting peer independently of the packet itself.
    #[tokio::test]
    async fn a_piggybacked_claim_is_verified_and_acknowledged() {
        let signer = LocalSigner::generate("claim-key");
        let counterparty = connector_signer::derive_evm_address(&signer.public_key().unwrap());
        let peer = Arc::new(with_test_channel(
            Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ),
            1,
            counterparty,
        ));
        let mut transport = InProcessPeerTransport::new();
        transport.add_peer("peer-b", peer);
        let claim = sign_wire_claim(&signer, 1, 1, 50);

        let PeerForward { response, ack, .. } = transport
            .forward("peer-b", prepare("g.nowhere"), Some(claim))
            .await;

        // The claim is judged independently of the packet: no route exists
        // for this PREPARE, but the claim it carried is still accepted.
        match response {
            PacketResponse::Reject(reject) => assert_eq!(reject.code.as_str(), "F02"),
            other => panic!("expected a reject, got {other:?}"),
        }
        assert_eq!(ack, ClaimAckOutcome::Accepted);
    }

    #[tokio::test]
    async fn flushing_a_claim_to_an_unregistered_peer_reports_not_sent() {
        let transport = InProcessPeerTransport::new();
        let claim = WireClaim {
            channel_id: "channel-a".to_string(),
            nonce: 1,
            cumulative_amount: 50,
            signature: crate::claim::ClaimSignature::Evm(connector_signer::Signature {
                r: [0u8; 32],
                s: [0u8; 32],
                recovery_id: 0,
            }),
        };

        let ack = transport.flush("nowhere", claim).await;

        assert_eq!(ack, ClaimAckOutcome::NotSent);
    }

    /// Establishes that a peer link is owned by exactly one spawned task
    /// rather than shared behind a lock: several concurrent forwards over
    /// the same link are all answered correctly, which would only be
    /// possible if the owning task serialized them itself.
    #[tokio::test]
    async fn a_single_peer_link_answers_several_concurrent_forwards() {
        let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
        let app_client = Arc::new(FakeAppClient::new());
        app_client.respond(route.handler_url(), answered(b"ok"));
        let peer = Arc::new(
            Connector::new(
                vec![route],
                vec![],
                app_client,
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            )
            .with_identity_signer(identity_signer()),
        );
        let mut transport = InProcessPeerTransport::new();
        transport.add_peer("peer-b", peer);
        let transport = Arc::new(transport);

        let mut handles = Vec::new();
        for _ in 0..8 {
            let transport = transport.clone();
            handles.push(tokio::spawn(async move {
                transport
                    .forward("peer-b", prepare("g.example.app"), None)
                    .await
            }));
        }

        for handle in handles {
            let forward = handle.await.expect("task");
            assert!(matches!(forward.response, PacketResponse::Fulfill(_)));
        }
    }

    /// Contract suite (ADR 0007): every [`PeerTransport`] implementation
    /// upholds the same statement about the port -- a registered peer's
    /// response comes back unchanged (fulfill or reject), and an
    /// unregistered peer id produces a `T01` reject -- so nothing above
    /// this port can tell which implementation is in use. Issue #426: all
    /// also agree on the `reached` signal -- `true` for any registered
    /// peer's own answer, `false` only when this transport never delivered
    /// the PREPARE at all.
    ///
    /// The suite is deliberately generic over how a peer is wired up even
    /// though [`InProcessPeerTransport`] is, since issue #679 deleted the
    /// raw-TCP wire, its only member: ADR 0027's two carriages (#676) join
    /// it by adding an arm, and the shared statement is what stops them
    /// drifting from each other.
    mod contract {
        use super::*;
        use std::future::Future;

        /// `deliverer` fulfills any destination under `g.example.app`;
        /// `rejecter` has no routes at all, so it produces the same F02 a
        /// direct client would get. Both are registered with `build`, which
        /// wires the implementation under test up its own way and returns a
        /// transport that can reach both.
        async fn assert_upholds_the_contract<F, Fut>(build: F)
        where
            F: FnOnce(Vec<(&'static str, Arc<Connector>)>) -> Fut,
            Fut: Future<Output = Arc<dyn PeerTransport>>,
        {
            let route = StaticRoute::new("g.example.app", "http://localhost:4000").unwrap();
            let app_client = Arc::new(FakeAppClient::new());
            app_client.respond(route.handler_url(), answered(b"delivered by the peer"));
            let deliverer = Arc::new(
                Connector::new(
                    vec![route],
                    vec![],
                    app_client,
                    Arc::new(InProcessPeerTransport::new()),
                    test_clock(),
                )
                .with_identity_signer(identity_signer()),
            );
            let rejecter = Arc::new(Connector::new(
                vec![],
                vec![],
                Arc::new(FakeAppClient::new()),
                Arc::new(InProcessPeerTransport::new()),
                test_clock(),
            ));

            let transport = build(vec![("peer-b", deliverer), ("peer-c", rejecter)]).await;
            let (sealed, shared_secret) = sealed_prepare(b"hello");

            let PeerForward {
                response,
                reached_peer: reached,
                ..
            } = transport.forward("peer-b", sealed, None).await;
            match response {
                PacketResponse::Fulfill(fulfill) => {
                    assert_eq!(fulfill.fulfillment, expected_fulfillment(&shared_secret));
                    assert_eq!(
                        open_sealed_envelope(&shared_secret, &fulfill.data),
                        fulfill_envelope(b"delivered by the peer")
                    );
                }
                other => panic!("expected a fulfill, got {other:?}"),
            }
            assert!(reached);

            let PeerForward {
                response,
                reached_peer: reached,
                ..
            } = transport
                .forward("peer-c", prepare("g.nowhere-on-peer-c"), None)
                .await;
            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "F02");
                    assert!(reject.message.contains("g.nowhere-on-peer-c"));
                }
                other => panic!("expected a reject, got {other:?}"),
            }
            assert!(reached);

            let PeerForward {
                response,
                reached_peer: reached,
                ..
            } = transport
                .forward("nowhere", prepare("g.example.app"), None)
                .await;
            match response {
                PacketResponse::Reject(reject) => {
                    assert_eq!(reject.code.as_str(), "T01");
                    assert!(reject.message.contains("nowhere"));
                }
                other => panic!("expected a reject, got {other:?}"),
            }
            assert!(!reached);
        }

        #[tokio::test]
        async fn in_process_peer_transport_upholds_the_contract() {
            assert_upholds_the_contract(|peers| async move {
                let mut transport = InProcessPeerTransport::new();
                for (peer_id, connector) in peers {
                    transport.add_peer(peer_id, connector);
                }
                Arc::new(transport) as Arc<dyn PeerTransport>
            })
            .await;
        }
    }
}
