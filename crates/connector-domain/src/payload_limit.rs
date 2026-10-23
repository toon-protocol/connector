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

    proptest! {
        #[test]
        fn a_payload_at_the_limit_is_admitted(max_bytes in 1u64..=(64 * 1024 * 1024)) {
            let limit = PayloadLimit::new(max_bytes);
            let at = usize::try_from(max_bytes).unwrap();
            prop_assert_eq!(refuse_oversize(limit, "g.a", at), None);
        }

        #[test]
        fn a_payload_one_under_the_limit_is_admitted(max_bytes in 1u64..=(64 * 1024 * 1024)) {
            let limit = PayloadLimit::new(max_bytes);
            let under = usize::try_from(max_bytes - 1).unwrap();
            prop_assert_eq!(refuse_oversize(limit, "g.a", under), None);
        }

        #[test]
        fn a_payload_one_over_the_limit_is_refused_f00(max_bytes in 1u64..=(64 * 1024 * 1024)) {
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
