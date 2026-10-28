//! The largest payload a route accepts
//! ([ADR 0081](../../../docs/adr/0081-a-route-may-refuse-a-payload-over-its-limit-before-it-is-paid.md),
//! issue #1530): an operator's `max_bytes`, refused before any claim is
//! taken, so a request the app would only answer `413` costs its sender
//! nothing.
//!
//! The measured quantity is the one [`crate::Price`] charges on:
//! `Prepare.data.len()`, the length of the sealed gift wrap (ADR 0065). It
//! is a property of carriage, so every gate that charges a price can refuse
//! on the same figure, before it opens anything -- and the sender, which
//! sealed the wrap, knows it before it sends.
//!
//! [`RequestLimit`] is the other half (ADR 0082, issue #1529): the largest
//! request either client carriage *reads* before it can see which route a
//! packet is for, derived from those same limits.

use std::num::NonZeroU64;

use crate::{Reject, RejectCode};

/// The most payload bytes one packet to a route may carry: a packet whose
/// `data` is longer is refused `F00`, a packet exactly this long is not.
///
/// Never zero. A limit of zero would refuse every packet with a payload,
/// which is a route nobody can use rather than a limit, so it is not a value
/// of this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PayloadLimit(NonZeroU64);

impl PayloadLimit {
    /// A limit of `max_bytes`, or `None` for zero.
    pub const fn new(max_bytes: u64) -> Option<PayloadLimit> {
        match NonZeroU64::new(max_bytes) {
            Some(max_bytes) => Some(PayloadLimit(max_bytes)),
            None => None,
        }
    }

    /// The limit itself, in bytes -- what the config file wrote and what the
    /// greeting and the self-description publish as `maxBytes`.
    pub const fn max_bytes(&self) -> u64 {
        self.0.get()
    }

    /// Whether a packet whose payload is `payload_len` bytes is within this
    /// limit. A payload exactly at the limit is.
    pub fn admits(&self, payload_len: usize) -> bool {
        u64::try_from(payload_len).is_ok_and(|len| len <= self.max_bytes())
    }
}

/// The largest `max_bytes` a route may declare (ADR 0082): 64 MiB. A route
/// above the 2 MiB a node accepts by default raises the node's
/// [`RequestLimit`] to carry it, and this bounds how far one route can raise
/// it.
pub const MAX_PAYLOAD_LIMIT: u64 = 64 * 1024 * 1024;

/// The largest request body a node reads -- a `POST /ilp` or
/// `POST /ilp/probe` body, from a client or a peer (ADR 0082).
///
/// It is read before the packet is decoded, so before the route is known,
/// and is therefore one figure for the node: the default 2 MiB, or enough to
/// carry the largest packet any of the node's routes admits, whichever is
/// larger. A node whose routes declare no `max_bytes` above 2 MiB reads
/// exactly what it read before the record.
///
/// A BTP socket does not read at this figure: requests and responses share
/// it, so it reads at [`BTP_MESSAGE_LIMIT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestLimit(usize);

impl RequestLimit {
    /// 2 MiB: axum's own `DefaultBodyLimit`, the `POST /ilp` limit of every
    /// node before ADR 0082.
    pub const DEFAULT: RequestLimit = RequestLimit(2 * 1024 * 1024);

    /// What a request carries beyond its packet's payload. On HTTP that is
    /// the PREPARE's own OER framing -- at most about 1.1 KiB, most of it a
    /// 1023-byte destination. On BTP it is that plus the frame's header and
    /// its other protocol-data entries, a claim among them, which run to a
    /// few KiB. 64 KiB covers both with room to spare.
    pub const FRAMING_ALLOWANCE: usize = 64 * 1024;

    /// The request limit a node whose routes declare `limits` reads at.
    pub fn covering(limits: impl IntoIterator<Item = PayloadLimit>) -> RequestLimit {
        let largest = limits
            .into_iter()
            .map(|limit| usize::try_from(limit.max_bytes()).unwrap_or(usize::MAX))
            .max()
            .map_or(0, |max_bytes| {
                max_bytes.saturating_add(Self::FRAMING_ALLOWANCE)
            });
        RequestLimit(largest.max(Self::DEFAULT.0))
    }

    /// The limit, in bytes.
    pub const fn bytes(&self) -> usize {
        self.0
    }
}

impl Default for RequestLimit {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The largest websocket message, and frame, any BTP socket reads, accepted
/// or dialed (ADR 0082): the largest [`RequestLimit`] any node can have.
///
/// A fixed figure rather than the node's own request limit, because a socket
/// cannot tell a request from a response until it has read it, and a
/// FULFILL carries the app's whole response, which no `max_bytes` bounds.
/// It equals tungstenite's 64 MiB message default plus the framing
/// allowance, so no session that read a message before the record refuses
/// it now; what changes is that the figure is chosen here.
pub const BTP_MESSAGE_LIMIT: usize = MAX_PAYLOAD_LIMIT as usize + RequestLimit::FRAMING_ALLOWANCE;

// Never below tungstenite's own 64 MiB message default, which every BTP
// socket read at before the record.
const _: () = assert!(BTP_MESSAGE_LIMIT >= 64 * 1024 * 1024);

/// The refusal for a packet to `destination` whose payload is `payload_len`
/// bytes, under a route whose limit is `limit` -- `None` when the route sets
/// no limit or the packet is within it.
///
/// `F00`, because the packet is malformed for this route in a way no retry
/// of the same bytes can cure (ILPv4 has no "payload too large" code, and
/// `F08` is about the *amount*). Its message names both figures, so a sender
/// learns the limit from the refusal as well as from the published terms.
/// `accumulated_cost` is zero: nothing was charged and nothing on the path
/// was paid for.
pub fn refuse_oversize(
    limit: Option<PayloadLimit>,
    destination: &str,
    payload_len: usize,
) -> Option<Reject> {
    let limit = limit?;
    if limit.admits(payload_len) {
        return None;
    }
    Some(Reject {
        code: RejectCode::f00_bad_request(),
        triggered_by: String::new(),
        message: format!(
            "'{destination}' accepts at most {} payload bytes; this packet carries {payload_len}",
            limit.max_bytes()
        ),
        data: Vec::new(),
        accumulated_cost: 0,
        reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn a_zero_limit_is_not_a_limit() {
        assert_eq!(PayloadLimit::new(0), None);
    }

    #[test]
    fn the_refusal_names_the_limit_and_the_length() {
        let limit = PayloadLimit::new(1024);
        let reject = refuse_oversize(limit, "g.blossom.upload", 1025).expect("one over is refused");
        assert_eq!(reject.code, RejectCode::f00_bad_request());
        assert_eq!(
            reject.message,
            "'g.blossom.upload' accepts at most 1024 payload bytes; this packet carries 1025"
        );
        assert_eq!(reject.accumulated_cost, 0);
    }

    #[test]
    fn a_route_without_a_limit_refuses_nothing() {
        assert_eq!(refuse_oversize(None, "g.any", usize::MAX), None);
    }

    /// A PREPARE carrying exactly `max_bytes` of payload to the longest
    /// destination RFC 0015 allows -- the largest packet a route with that
    /// limit admits.
    fn largest_admitted_prepare(max_bytes: u64) -> Vec<u8> {
        crate::Prepare {
            amount: u64::MAX,
            expires_at: chrono::Utc::now(),
            greeting: false,
            destination: "g".repeat(1023),
            data: vec![0; usize::try_from(max_bytes).unwrap()],
        }
        .encode()
    }

    #[test]
    fn a_node_whose_routes_raise_nothing_keeps_the_two_mib_body_limit() {
        assert_eq!(RequestLimit::covering([]).bytes(), 2 * 1024 * 1024);
        let small = PayloadLimit::new(4096).unwrap();
        assert_eq!(RequestLimit::covering([small]).bytes(), 2 * 1024 * 1024);
    }

    #[test]
    fn the_largest_route_limit_sets_the_request_limit() {
        let small = PayloadLimit::new(4096).unwrap();
        let large = PayloadLimit::new(8 * 1024 * 1024).unwrap();
        let limit = RequestLimit::covering([small, large]);
        assert!(limit.bytes() > 8 * 1024 * 1024);
        assert_eq!(limit, RequestLimit::covering([large, small]));
    }

    #[test]
    fn the_largest_packet_a_route_admits_fits_its_request_limit() {
        for max_bytes in [1, 2 * 1024 * 1024, 5 * 1024 * 1024 + 7, MAX_PAYLOAD_LIMIT] {
            let limit = RequestLimit::covering([PayloadLimit::new(max_bytes).unwrap()]);
            let packet = largest_admitted_prepare(max_bytes);
            assert!(
                packet.len() <= limit.bytes(),
                "a {}-byte PREPARE for max_bytes = {max_bytes} exceeds {}",
                packet.len(),
                limit.bytes()
            );
        }
    }

    #[test]
    fn a_btp_socket_reads_the_largest_request_any_node_can_take() {
        let largest = RequestLimit::covering([PayloadLimit::new(MAX_PAYLOAD_LIMIT).unwrap()]);
        assert_eq!(BTP_MESSAGE_LIMIT, largest.bytes());
    }

    proptest! {
        #[test]
        fn a_request_limit_never_falls_below_two_mib(
            limits in proptest::collection::vec(1u64..=MAX_PAYLOAD_LIMIT, 0..8),
        ) {
            let limits = limits.into_iter().filter_map(PayloadLimit::new);
            prop_assert!(RequestLimit::covering(limits).bytes() >= 2 * 1024 * 1024);
        }

        #[test]
        fn a_request_limit_covers_every_route_limit(
            limits in proptest::collection::vec(1u64..=MAX_PAYLOAD_LIMIT, 1..8),
        ) {
            let request = RequestLimit::covering(limits.iter().copied().filter_map(PayloadLimit::new));
            for max_bytes in limits {
                prop_assert!(request.bytes() as u64 > max_bytes);
            }
        }

        #[test]
        fn a_payload_at_the_limit_is_admitted(max_bytes in 1u64..=MAX_PAYLOAD_LIMIT) {
            let limit = PayloadLimit::new(max_bytes);
            let at = usize::try_from(max_bytes).unwrap();
            prop_assert_eq!(refuse_oversize(limit, "g.a", at), None);
        }

        #[test]
        fn a_payload_one_under_the_limit_is_admitted(max_bytes in 1u64..=MAX_PAYLOAD_LIMIT) {
            let limit = PayloadLimit::new(max_bytes);
            let under = usize::try_from(max_bytes - 1).unwrap();
            prop_assert_eq!(refuse_oversize(limit, "g.a", under), None);
        }

        #[test]
        fn a_payload_one_over_the_limit_is_refused_f00(max_bytes in 1u64..=MAX_PAYLOAD_LIMIT) {
            let limit = PayloadLimit::new(max_bytes);
            let over = usize::try_from(max_bytes + 1).unwrap();
            let reject = refuse_oversize(limit, "g.a", over);
            prop_assert_eq!(reject.map(|reject| reject.code), Some(RejectCode::f00_bad_request()));
        }

        #[test]
        fn refusing_is_total(max_bytes in any::<u64>(), len in any::<usize>()) {
            // No panic at any length, including one beyond a `u64`.
            let _ = refuse_oversize(PayloadLimit::new(max_bytes), "g.a", len);
        }
    }
}
