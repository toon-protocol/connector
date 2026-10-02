//! Client session registry (issue #698, toon-meta#262 decision 12):
//! "the socket is the lease." Once a connector can originate a MESSAGE to
//! a client (issue #697's symmetric BTP grammar), it needs to answer a
//! question it never had to before: which socket, right now, is the live
//! session for a given client-edge address? [`ClientEdgeState`] had no
//! such record; the routing table's [`connector_runtime::PeerRoute`] and
//! app routes both name something this connector *dials* -- neither can
//! name a socket that dialed *it*.
//!
//! One fact, not two: there is deliberately no separate route record with
//! its own TTL alongside this registry. A route record and a socket can
//! disagree, and during the disagreement this connector would route paid
//! work into a hole. The registry's entries -- see [`SessionRegistry::bind`]
//! and [`SessionRegistry::unbind`] -- come and go with the socket itself,
//! driven by `btp::btp_session`'s own lifecycle: bound at auth, cleared
//! when that same session's read loop ends. [`SESSION_LEASE_BACKSTOP_TTL`]
//! exists only for the case that lifecycle cannot see -- a socket that
//! looks alive at the TCP layer but has stopped producing frames -- never
//! as the primary mechanism.
//!
//! **Fencing generations.** Each bind for an address gets the next number
//! from one monotonic counter shared by the whole registry (not one
//! per-address counter -- a single shared sequence gives "does a higher
//! number mean later" for free, with no per-address high-water mark to
//! maintain separately). The highest generation for an address always
//! wins, and [`SessionRegistry::unbind`] cannot remove a binding at a
//! generation newer than the one it names. This is buzz's own fencing law
//! (`buzz-relay-mesh/src/wire.rs`): *"membership is a hint; the fenced
//! generation is the arbiter. The mesh may say 'don't dial' -- it may
//! never say 'take over.'"* Applied here to a socket instead of a mesh
//! peer, it is what stops a reconnect race from producing two claimants
//! for one address -- the failure mode the issue calls "silent,
//! intermittent, and misroutes paid work."
//!
//! **T-class rejection, never R00.** [`SessionRegistry::deliver`] answers
//! every failure path -- no live session at all, or one that died or
//! timed out mid-delivery -- with [`RejectCode::t01_peer_unreachable`]:
//! the packet itself is fine, there is currently no way to reach this
//! peer, and the sender should retry. `R00` (`Transfer Timed Out`) would
//! tell the sender its packet's own expiry passed, which is not what
//! happened when a laptop's Wi-Fi drops mid-flight.
//!
//! [`SessionRegistry::deliver`]'s production caller (issue #736,
//! toon-meta#262's job-dispatch work this module deferred) is
//! [`crate::session_route::route_prepare`], wrapped around
//! `Connector::handle_prepare` at both client-edge ingresses (`lib.rs`'s
//! `POST /ilp`, `btp.rs`'s BTP carriage) rather than folded into
//! `handle_prepare` itself -- `connector-runtime` depends on nothing in
//! this crate and so cannot see this registry at all. `bind`/`touch`/
//! `unbind` were already live in production before that: every BTP
//! session's auth, per-frame liveness and close run through them
//! (`btp::btp_session`), so the registry itself, and its fencing
//! invariant, are exercised by every real session.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use connector_domain::{Reject, RejectCode};

use connector_btp::{BtpFrame, BtpSessionHandle, OriginateError, ProtocolData};

/// Backstop TTL for a half-open socket (issue #698 AC5): the primary
/// liveness signal is the socket's own read loop ending, which unbinds a
/// session immediately (`btp::btp_session`) -- this only catches a socket
/// that still looks alive at the TCP layer but has stopped producing
/// frames. Checked lazily on read (`SessionRegistry::resolve`), not by a
/// background sweep, since the primary signal already clears the
/// overwhelming majority of cases before this ever matters.
///
/// **Cross-plane invariant (toon-meta#262 decision 12):** buzz#84's relay
/// side advertises a provider's reachability and must never advertise for
/// longer than this connector actually honors a session as live, or a
/// buyer pays for a job that cannot land. This constant is where that
/// value lives -- but it is a Rust `pub const`, and buzz's desktop is
/// TypeScript, so there is no path by which it imports this directly
/// (issue #722). A consumer in any language reads it off the wire instead:
/// every x402 greeting this client edge answers (`extra.sessionLeaseTtlMs`,
/// `client-edge-spec.md` §1.4/§1.9) carries exactly this value, derived
/// here rather than typed a second time.
pub const SESSION_LEASE_BACKSTOP_TTL: Duration = Duration::from_secs(120);

/// One address's current binding: the fencing generation it was installed
/// under, the session handle to reach it through, and when it was last
/// heard from (for the backstop TTL).
struct SessionBinding {
    generation: u64,
    handle: BtpSessionHandle,
    last_seen: u64,
}

/// One [`SessionRegistry::resolve`] answer: the live session for an
/// address and the fencing generation it is currently bound under. A
/// caller that must retry a delivery across a reconnect keeps the
/// generation it saw and passes it back to [`SessionRegistry::deliver`] as
/// `expected_generation`, so a rebind that happened in between fences the
/// stale attempt off rather than letting it race the session that
/// superseded it.
#[derive(Clone)]
pub(crate) struct SessionLease {
    pub(crate) generation: u64,
    handle: BtpSessionHandle,
}

/// ILP address -> live BTP session (issue #698). One instance is shared by
/// every session `ClientEdgeState` serves, so a reconnect on a different
/// socket is visible to every other session immediately -- this is what
/// makes "the socket is the lease" a single fact rather than one this
/// registry could disagree with a route table over.
pub(crate) struct SessionRegistry {
    next_generation: AtomicU64,
    bindings: Mutex<HashMap<String, SessionBinding>>,
}

impl SessionRegistry {
    pub(crate) fn new() -> Self {
        Self {
            // Start at 1, matching `btp::OutboundRequests::next_id`'s own
            // rationale -- 0 is an ordinary value elsewhere and this
            // sequence is free to start anywhere, so it does not overlap
            // needlessly with a caller's habit of treating 0 as "unset".
            next_generation: AtomicU64::new(1),
            bindings: Mutex::new(HashMap::new()),
        }
    }

    /// Install `handle` as the live session for `address`, allocating and
    /// returning the next fencing generation. Always supersedes whatever
    /// was bound before -- there is no refusal path, because "the socket
    /// is the lease" means the most recently authenticated session for an
    /// address is definitionally the current one (issue #698's decision
    /// 12).
    pub(crate) fn bind(
        &self,
        address: impl Into<String>,
        handle: BtpSessionHandle,
        now: u64,
    ) -> u64 {
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        let mut bindings = self.bindings.lock().expect("not poisoned");
        bindings.insert(
            address.into(),
            SessionBinding {
                generation,
                handle,
                last_seen: now,
            },
        );
        generation
    }

    /// Note that `address`'s session at `generation` is still alive,
    /// resetting the backstop TTL clock. A no-op if `generation` has since
    /// been superseded, so a frame a dying session is still finishing
    /// cannot resurrect a binding a reconnect already replaced.
    pub(crate) fn touch(&self, address: &str, generation: u64, now: u64) {
        let mut bindings = self.bindings.lock().expect("not poisoned");
        if let Some(binding) = bindings.get_mut(address) {
            if binding.generation == generation {
                binding.last_seen = now;
            }
        }
    }

    /// Clear `address`'s binding, but only if it is still at `generation`
    /// -- the fencing law itself, applied to closing a session rather than
    /// opening one: a session already superseded by a reconnect must not
    /// be able to clear the newer binding out from under it when its own
    /// read loop finally notices the socket is gone.
    pub(crate) fn unbind(&self, address: &str, generation: u64) {
        let mut bindings = self.bindings.lock().expect("not poisoned");
        if bindings
            .get(address)
            .is_some_and(|binding| binding.generation == generation)
        {
            bindings.remove(address);
        }
    }

    /// The live session for `address`, if any -- always the newest
    /// generation ever bound for it, since [`bind`](Self::bind) never
    /// installs a lower generation over a higher one and
    /// [`unbind`](Self::unbind) cannot remove one either. `None` is issue
    /// #698 AC4's "fail fast when absent": there is no live session, and a
    /// caller should refuse before any money moves rather than wait on
    /// one. Also `None` -- and the stale entry is dropped -- when the
    /// binding has gone quiet longer than [`SESSION_LEASE_BACKSTOP_TTL`]
    /// (AC5).
    pub(crate) fn resolve(&self, address: &str, now: u64) -> Option<SessionLease> {
        let mut bindings = self.bindings.lock().expect("not poisoned");
        let stale = bindings.get(address).is_some_and(|binding| {
            now.saturating_sub(binding.last_seen) > SESSION_LEASE_BACKSTOP_TTL.as_secs()
        });
        if stale {
            bindings.remove(address);
            return None;
        }
        bindings.get(address).map(|binding| SessionLease {
            generation: binding.generation,
            handle: binding.handle.clone(),
        })
    }

    /// Originate a MESSAGE on whichever session is currently bound to
    /// `address`, answering a T-class reject on every failure path (issue
    /// #698 AC3/AC4): no live session at all (AC4, fail fast, no money
    /// moved) and a session that died or timed out mid-delivery (AC3) are
    /// both cases where the *packet* is fine and the sender should simply
    /// retry -- never `R00` (`Transfer Timed Out`), which the issue's own
    /// text singles out as the wrong answer for "the provider's Wi-Fi
    /// dropped."
    ///
    /// `expected_generation` fences a retry against a rebind that happened
    /// in between: a caller holding a lease from before a reconnect passes
    /// the generation it saw, and if the address has since moved to a
    /// higher one, the attempt is discarded with the same T-class reject
    /// rather than silently sent to the new session under the old
    /// caller's assumptions (issue #698's explicit test: "old generation
    /// sends after a new one is established, and must be discarded"). Pass
    /// `None` for a delivery with no prior lease to fence against.
    pub(crate) async fn deliver(
        &self,
        address: &str,
        expected_generation: Option<u64>,
        protocol_data: &[ProtocolData],
        ilp_packet: &[u8],
        now: u64,
    ) -> Result<BtpFrame, Reject> {
        let lease = self.fenced_lease(address, expected_generation, now)?;
        lease
            .handle
            .send_message(protocol_data, ilp_packet)
            .await
            .map_err(|error| match error {
                OriginateError::SessionGone
                | OriginateError::Timeout
                | OriginateError::ClosedBeforeAnswer => no_live_session_reject(address),
            })
    }

    /// As [`Self::deliver`], but originating a TRANSFER rather than a
    /// MESSAGE (issue #770): how a payout claim reaches the client session
    /// it is owed to (client-edge-spec.md §1.9 step 7). Same fencing, same
    /// T-class-never-R00 answer on every failure path -- a session gone by
    /// the time a payout would go out is not a reason to undo the credit
    /// already recorded, only to fail this delivery attempt; the claim
    /// stays pending for the next one.
    pub(crate) async fn deliver_transfer(
        &self,
        address: &str,
        expected_generation: Option<u64>,
        amount: u64,
        protocol_data: &[ProtocolData],
        now: u64,
    ) -> Result<BtpFrame, Reject> {
        let lease = self.fenced_lease(address, expected_generation, now)?;
        lease
            .handle
            .send_transfer(amount, protocol_data)
            .await
            .map_err(|error| match error {
                OriginateError::SessionGone
                | OriginateError::Timeout
                | OriginateError::ClosedBeforeAnswer => no_live_session_reject(address),
            })
    }

    /// The live, fencing-checked session for `address` -- [`Self::resolve`]
    /// plus the same `expected_generation` check both [`Self::deliver`] and
    /// [`Self::deliver_transfer`] apply before originating anything.
    fn fenced_lease(
        &self,
        address: &str,
        expected_generation: Option<u64>,
        now: u64,
    ) -> Result<SessionLease, Reject> {
        let Some(lease) = self.resolve(address, now) else {
            return Err(no_live_session_reject(address));
        };
        if expected_generation.is_some_and(|expected| expected != lease.generation) {
            return Err(no_live_session_reject(address));
        }
        Ok(lease)
    }
}

fn no_live_session_reject(address: &str) -> Reject {
    Reject {
        code: RejectCode::t01_peer_unreachable(),
        triggered_by: String::new(),
        message: format!("no live client session for '{address}'"),
        data: Vec::new(),
        accumulated_cost: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_btp::{decode_frame, OutboundRequests};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    /// A handle over a real channel pair, the receiver a test can read the
    /// encoded frame off of, and the same `OutboundRequests` the handle
    /// wraps -- kept alongside so a test can act as the "peer" and answer
    /// through `outbound.resolve`, exactly as `btp.rs`'s own
    /// `BtpSessionHandle` round-trip tests do.
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

    /// A handle whose send half is already gone -- `send_message` on it
    /// fails immediately with `SessionGone`, simulating a session that
    /// died.
    fn dead_handle() -> BtpSessionHandle {
        let (replies, reply_rx) = mpsc::channel::<Vec<u8>>(4);
        drop(reply_rx);
        BtpSessionHandle::new(replies, Arc::new(OutboundRequests::new()))
    }

    /// Read one written frame off `reply_rx` and answer it with an empty
    /// RESPONSE through `outbound`, the same shape `handle_frame`'s real
    /// RESPONSE/ERROR branch delivers on a genuine inbound answer.
    async fn answer_next_message(
        reply_rx: &mut mpsc::Receiver<Vec<u8>>,
        outbound: &OutboundRequests,
    ) {
        let sent = reply_rx.recv().await.expect("the MESSAGE was written");
        let decoded = decode_frame(&sent).expect("the connector's own encoder");
        outbound.resolve(BtpFrame {
            frame_type: 1, // BTP_RESPONSE
            request_id: decoded.request_id,
            amount: None,
            protocol_data: Vec::new(),
            ilp_packet: Vec::new(),
        });
    }

    #[test]
    fn bind_returns_strictly_increasing_generations() {
        let registry = SessionRegistry::new();
        let (handle_a, _rx_a, _outbound_a) = test_handle();
        let (handle_b, _rx_b, _outbound_b) = test_handle();
        let gen_a = registry.bind("g.proxy.agents.one", handle_a, 0);
        let gen_b = registry.bind("g.proxy.agents.two", handle_b, 0);
        assert!(
            gen_b > gen_a,
            "generations increase across every bind, not just per address"
        );
    }

    #[test]
    fn resolve_returns_the_newest_binding_for_an_address() {
        let registry = SessionRegistry::new();
        let (handle_a, _rx_a, _outbound_a) = test_handle();
        let (handle_b, _rx_b, _outbound_b) = test_handle();
        let gen_a = registry.bind("g.proxy.agents.one", handle_a, 0);
        let gen_b = registry.bind("g.proxy.agents.one", handle_b, 0);
        assert!(gen_b > gen_a);

        let lease = registry
            .resolve("g.proxy.agents.one", 0)
            .expect("a session is bound");
        assert_eq!(lease.generation, gen_b, "the newest bind wins");
    }

    #[test]
    fn resolve_of_an_unbound_address_is_none() {
        let registry = SessionRegistry::new();
        assert!(registry.resolve("g.proxy.agents.nobody", 0).is_none());
    }

    #[test]
    fn unbind_with_a_superseded_generation_is_a_no_op() {
        let registry = SessionRegistry::new();
        let (handle_a, _rx_a, _outbound_a) = test_handle();
        let (handle_b, _rx_b, _outbound_b) = test_handle();
        let gen_a = registry.bind("g.proxy.agents.one", handle_a, 0);
        let gen_b = registry.bind("g.proxy.agents.one", handle_b, 0);

        // The old session's own cleanup, running after it has already
        // been superseded by a reconnect, must not evict the session that
        // replaced it -- the fencing law's "may never say take over"
        // applied to closing rather than opening.
        registry.unbind("g.proxy.agents.one", gen_a);

        let lease = registry
            .resolve("g.proxy.agents.one", 0)
            .expect("the newer session is still bound");
        assert_eq!(lease.generation, gen_b);
    }

    #[test]
    fn unbind_with_the_current_generation_clears_the_binding() {
        let registry = SessionRegistry::new();
        let (handle, _rx, _outbound) = test_handle();
        let generation = registry.bind("g.proxy.agents.one", handle, 0);

        registry.unbind("g.proxy.agents.one", generation);

        assert!(registry.resolve("g.proxy.agents.one", 0).is_none());
    }

    #[test]
    fn touch_from_a_superseded_generation_does_not_refresh_the_newer_binding() {
        let registry = SessionRegistry::new();
        let (handle_a, _rx_a, _outbound_a) = test_handle();
        let (handle_b, _rx_b, _outbound_b) = test_handle();
        let gen_a = registry.bind("g.proxy.agents.one", handle_a, 0);
        let _gen_b = registry.bind("g.proxy.agents.one", handle_b, 100);

        // A frame the old (superseded) session is still finishing must not
        // touch the new binding's liveness clock under the old generation.
        registry.touch("g.proxy.agents.one", gen_a, 999);

        // The newer binding's own last_seen (100) is unaffected: it is
        // still stale past the backstop TTL measured from `now` far
        // beyond it, proving the touch above did nothing to it.
        let far_future = 100 + SESSION_LEASE_BACKSTOP_TTL.as_secs() + 1;
        assert!(registry.resolve("g.proxy.agents.one", far_future).is_none());
    }

    #[test]
    fn a_binding_untouched_past_the_backstop_ttl_is_lazily_expired() {
        let registry = SessionRegistry::new();
        let (handle, _rx, _outbound) = test_handle();
        registry.bind("g.proxy.agents.one", handle, 0);

        let just_within = SESSION_LEASE_BACKSTOP_TTL.as_secs();
        assert!(
            registry
                .resolve("g.proxy.agents.one", just_within)
                .is_some(),
            "exactly the TTL boundary is still live"
        );

        let past_ttl = SESSION_LEASE_BACKSTOP_TTL.as_secs() + 1;
        assert!(
            registry.resolve("g.proxy.agents.one", past_ttl).is_none(),
            "a session that has gone quiet past the backstop TTL is treated as gone"
        );
    }

    #[test]
    fn touch_extends_the_backstop_past_bind_time() {
        let registry = SessionRegistry::new();
        let (handle, _rx, _outbound) = test_handle();
        let generation = registry.bind("g.proxy.agents.one", handle, 0);

        let renewed_at = SESSION_LEASE_BACKSTOP_TTL.as_secs() - 1;
        registry.touch("g.proxy.agents.one", generation, renewed_at);

        let would_have_expired_from_bind = SESSION_LEASE_BACKSTOP_TTL.as_secs() + 1;
        assert!(
            registry
                .resolve("g.proxy.agents.one", would_have_expired_from_bind)
                .is_some(),
            "the touch reset the clock, so the binding is still within the TTL of it"
        );
    }

    #[tokio::test]
    async fn deliver_to_an_unbound_address_fails_fast_with_a_t_class_reject() {
        let registry = SessionRegistry::new();
        let reject = registry
            .deliver("g.proxy.agents.nobody", None, &[], &[], 0)
            .await
            .expect_err("no session exists to deliver through");
        assert_eq!(reject.code.as_str(), "T01");
        assert_ne!(
            reject.code.as_str(),
            "R00",
            "an absent session is not the packet's own expiry"
        );
    }

    #[tokio::test]
    async fn deliver_through_a_dead_session_answers_a_t_class_reject_not_r00() {
        let registry = SessionRegistry::new();
        registry.bind("g.proxy.agents.one", dead_handle(), 0);

        let reject = registry
            .deliver("g.proxy.agents.one", None, &[], &[], 0)
            .await
            .expect_err("the session's send half is gone");
        assert_eq!(reject.code.as_str(), "T01");
    }

    #[tokio::test]
    async fn deliver_succeeds_through_the_live_session_and_returns_its_answer() {
        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, outbound) = test_handle();
        registry.bind("g.proxy.agents.one", handle, 0);

        let peer = tokio::spawn(async move {
            answer_next_message(&mut reply_rx, &outbound).await;
        });

        let answer = registry
            .deliver("g.proxy.agents.one", None, &[], b"job", 0)
            .await
            .expect("the session is live");
        assert!(answer.ilp_packet.is_empty());
        peer.await.expect("the peer task");
    }

    /// The stale generation from before a reconnect must be discarded, not
    /// redirected to whichever session now holds the address -- issue
    /// #698's explicit test: "old generation sends after a new one is
    /// established, and must be discarded." Neither the old nor the new
    /// session receives anything from the discarded attempt.
    #[tokio::test]
    async fn a_stale_generation_delivery_is_discarded_once_a_newer_session_is_bound() {
        let registry = SessionRegistry::new();
        let (handle_a, mut rx_a, _outbound_a) = test_handle();
        let gen_a = registry.bind("g.proxy.agents.one", handle_a, 0);

        let (handle_b, mut rx_b, _outbound_b) = test_handle();
        let gen_b = registry.bind("g.proxy.agents.one", handle_b, 0);
        assert!(gen_b > gen_a);

        let reject = registry
            .deliver("g.proxy.agents.one", Some(gen_a), &[], b"stale job", 0)
            .await
            .expect_err("a caller holding the old generation must be fenced off");
        assert_eq!(reject.code.as_str(), "T01");
        assert!(
            rx_a.try_recv().is_err(),
            "the superseded session never receives the discarded attempt"
        );
        assert!(
            rx_b.try_recv().is_err(),
            "the current session is not silently substituted for the stale caller either"
        );
    }

    /// The control case for the test above: a caller with no prior lease
    /// (or one that already knows the current generation) reaches the
    /// live session normally.
    #[tokio::test]
    async fn a_delivery_naming_the_current_generation_reaches_the_live_session() {
        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, outbound) = test_handle();
        let generation = registry.bind("g.proxy.agents.one", handle, 0);

        let peer = tokio::spawn(async move {
            answer_next_message(&mut reply_rx, &outbound).await;
        });

        let result = registry
            .deliver("g.proxy.agents.one", Some(generation), &[], b"job", 0)
            .await;
        assert!(result.is_ok(), "the current generation is never fenced off");
        peer.await.expect("the peer task");
    }

    // ─── issue #770: deliver_transfer, deliver's payout-carrying sibling ───

    #[tokio::test]
    async fn deliver_transfer_to_an_unbound_address_fails_fast_with_a_t_class_reject() {
        let registry = SessionRegistry::new();
        let reject = registry
            .deliver_transfer("g.proxy.agents.nobody", None, 500, &[], 0)
            .await
            .expect_err("no session exists to deliver a payout through");
        assert_eq!(reject.code.as_str(), "T01");
    }

    #[tokio::test]
    async fn deliver_transfer_through_a_dead_session_answers_a_t_class_reject() {
        let registry = SessionRegistry::new();
        registry.bind("g.proxy.agents.one", dead_handle(), 0);

        let reject = registry
            .deliver_transfer("g.proxy.agents.one", None, 500, &[], 0)
            .await
            .expect_err("the session's send half is gone");
        assert_eq!(reject.code.as_str(), "T01");
    }

    #[tokio::test]
    async fn deliver_transfer_succeeds_through_the_live_session_as_a_real_transfer_frame() {
        use connector_btp::BTP_TRANSFER;

        let registry = SessionRegistry::new();
        let (handle, mut reply_rx, outbound) = test_handle();
        registry.bind("g.proxy.agents.one", handle, 0);

        let peer = tokio::spawn(async move {
            let sent = reply_rx.recv().await.expect("the TRANSFER was written");
            let decoded = decode_frame(&sent).expect("the connector's own encoder");
            assert_eq!(decoded.frame_type, BTP_TRANSFER);
            assert_eq!(decoded.amount, Some(500));
            outbound.resolve(BtpFrame {
                frame_type: 1, // BTP_RESPONSE
                request_id: decoded.request_id,
                amount: None,
                protocol_data: Vec::new(),
                ilp_packet: Vec::new(),
            });
        });

        let answer = registry
            .deliver_transfer("g.proxy.agents.one", None, 500, &[], 0)
            .await
            .expect("the session is live");
        assert!(answer.ilp_packet.is_empty());
        peer.await.expect("the peer task");
    }

    /// The same fencing law [`a_stale_generation_delivery_is_discarded_once_a_newer_session_is_bound`]
    /// proves for `deliver`, applied to `deliver_transfer`: a payout must
    /// never be discovered under way and then raced by a reconnect into
    /// the wrong socket.
    #[tokio::test]
    async fn a_stale_generation_transfer_is_discarded_once_a_newer_session_is_bound() {
        let registry = SessionRegistry::new();
        let (handle_a, mut rx_a, _outbound_a) = test_handle();
        let gen_a = registry.bind("g.proxy.agents.one", handle_a, 0);

        let (handle_b, mut rx_b, _outbound_b) = test_handle();
        let gen_b = registry.bind("g.proxy.agents.one", handle_b, 0);
        assert!(gen_b > gen_a);

        let reject = registry
            .deliver_transfer("g.proxy.agents.one", Some(gen_a), 500, &[], 0)
            .await
            .expect_err("a caller holding the old generation must be fenced off");
        assert_eq!(reject.code.as_str(), "T01");
        assert!(rx_a.try_recv().is_err());
        assert!(rx_b.try_recv().is_err());
    }
}
