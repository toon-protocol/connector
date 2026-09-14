//! Fulfilment and expiry (RFC-0022) -- pure, no I/O.
//!
//! Issue #1269 / ADR 0069 removed the execution condition from the wire: it
//! was invariant across every hop and distinctive per packet -- a perfect
//! join key for any two hops on a path -- while protecting nothing a hop was
//! paid to check (a hop is paid on arrival, ADR 0042; a mismatch still
//! charged; a termination's own check was a tautology against a condition it
//! minted from the same secret it derives the fulfilment from). No packet
//! carries a condition any more, and nothing in this crate mints one.
//! [`fulfillment_matches_condition`] stays exported regardless: RFC-0022's
//! relation is still real math, useful to a caller that independently holds
//! a condition to check a fulfilment against (a bridge to a chain that still
//! uses one, say). It has no production caller in this workspace today --
//! `connector send`'s own end-to-end check compares two fulfilments
//! directly (`fulfill.fulfillment == derive_fulfillment(&shared_secret)`)
//! and needs no intermediate condition to do it. `derive_condition` --
//! `sha256` of a fulfilment -- stays private as this function's own
//! arithmetic; nothing here exposes a function that goes the other way,
//! from a condition to a fulfilment.

use chrono::{DateTime, TimeDelta, Utc};
use sha2::{Digest, Sha256};

/// The condition a `fulfillment` satisfies: `sha256(fulfillment)`. Hashing
/// only ever runs in this direction -- from a chosen preimage to the
/// condition it produces -- never reversed. Private: nothing in this crate
/// mints a condition to put anywhere any more (issue #1269), so this is
/// purely [`fulfillment_matches_condition`]'s own arithmetic.
fn derive_condition(fulfillment: &[u8; 32]) -> [u8; 32] {
    Sha256::digest(fulfillment).into()
}

/// Whether `fulfillment` is the real preimage of `condition`, per RFC-0022's
/// `condition = sha256(fulfillment)`. No packet in this connector carries a
/// condition any more (issue #1269 / ADR 0069), so nothing here calls this
/// today -- kept exported as the one piece of that relation this crate
/// still states plainly, for a caller that holds a condition from elsewhere
/// and needs to check a fulfilment against it.
pub fn fulfillment_matches_condition(condition: &[u8; 32], fulfillment: &[u8; 32]) -> bool {
    derive_condition(fulfillment) == *condition
}

/// Whether a packet due to expire at `expires_at` is expired as of `now`.
/// `now == expires_at` counts as expired: a packet must fulfil strictly
/// before its deadline, not up to and including it.
pub fn is_expired(expires_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    now >= expires_at
}

/// How much of a packet's remaining expiry a hop keeps back for itself when
/// it forwards -- the *message window* of `packet-flow-spec.md` PF-19.
///
/// A hop that copied `expires_at` through verbatim would hand the packet's
/// whole remaining budget to the hop after it, and so on down to the
/// termination, which could then answer at the last instant before the
/// deadline. That fulfilment still has to travel back up the path, and by
/// the time it reaches hop *n* that hop's own deadline has already fired:
/// it has paid its peer to carry the packet -- the covering claim rides out
/// *with* the PREPARE (ADR 0042), so the money is gone the moment the
/// packet leaves -- and can no longer be paid for it upstream. Shortening
/// at every hop is what buys the return leg its own time, and is the race
/// RFC 0018 asks a connector to mitigate.
///
/// One second, and per hop rather than per path. It has to cover the return
/// leg of a single crossing -- one ILP-over-HTTP or BTP round trip to the
/// next hop and back -- which is a wide margin at real network latencies,
/// and it is the same figure ILPv4's reference connector has always used
/// for its default minimum message window, so a TOON hop in a mixed path
/// keeps back what its neighbours expect it to. It is deliberately not
/// generous: a sender's whole budget is finite (`connector send` defaults
/// to 300 seconds), the cost is paid again at every hop, and a window large
/// enough to matter to a deep path would be a window this connector is
/// spending on nothing.
pub const FORWARDING_MESSAGE_WINDOW: TimeDelta = TimeDelta::seconds(1);

/// The expiry a hop puts on the packet it forwards: `expires_at` less
/// [`FORWARDING_MESSAGE_WINDOW`] -- or `None` when that leaves nothing,
/// meaning the packet MUST NOT be forwarded at all (PF-19).
///
/// `None` is the rule that makes the shortening safe rather than merely
/// arithmetic. A hop with less than a window left cannot both forward and
/// be answered in time, so forwarding would spend a claim on a crossing
/// whose fulfilment can no longer come home; refusing costs the sender a
/// retry and costs this hop nothing. The boundary is [`is_expired`]'s, and
/// for the same reason: a packet must fulfil strictly before its deadline,
/// so a shortened expiry landing exactly on `now` is already dead.
pub fn forwarded_expiry(expires_at: DateTime<Utc>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let shortened = expires_at.checked_sub_signed(FORWARDING_MESSAGE_WINDOW)?;
    (!is_expired(shortened, now)).then_some(shortened)
}

/// How long a *termination* may wait for its app: everything the packet has
/// left, and not one instant more -- `None` when it has nothing left, meaning
/// the app MUST NOT be asked at all (`packet-flow-spec.md` PF-25, ADR 0064).
///
/// This is [`forwarded_expiry`]'s counterpart for the last hop, and the two
/// differ in exactly one way: a forward keeps a [`FORWARDING_MESSAGE_WINDOW`]
/// back and a termination keeps nothing back. That is not an oversight, it is
/// what PF-19 bought. The hop above already shortened this packet by its own
/// window before handing it here, precisely so the answer has time to travel
/// back up; a termination that shortened *again* would be spending a second
/// window on a return leg somebody else has already paid for, and would refuse
/// packets it could have served. Where nobody shortened -- a packet a client
/// posted straight to this connector's own edge -- the deadline is the payer's
/// own, unmediated, and waiting all of it is doing exactly what the payer
/// asked.
///
/// The boundary is [`is_expired`]'s, for [`forwarded_expiry`]'s reason: a
/// packet must fulfil strictly before its deadline, so a budget of exactly
/// zero is no budget. The returned span is therefore always strictly
/// positive, which is what makes it safe to hand to a timer.
///
/// It is deliberately a *budget* rather than a verdict. The rule this
/// implements is that the deadline bounds the wait, not that it censors the
/// answer: an app that answers inside the budget is answered for, however
/// close to the line it came, and the caller never asks this question a
/// second time about work already done (ADR 0064). Charging is not what is
/// being decided here and cannot be -- the claim that pays for this packet
/// was taken before the app was called, and no verdict returned afterwards
/// gives it back.
pub fn delivery_budget(expires_at: DateTime<Utc>, now: DateTime<Utc>) -> Option<TimeDelta> {
    (!is_expired(expires_at, now)).then(|| expires_at - now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use proptest::prelude::*;

    fn at(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, hh, mm, ss).unwrap()
    }

    #[test]
    fn the_real_preimage_matches_the_condition_it_derives() {
        let fulfillment = [7u8; 32];
        let condition = derive_condition(&fulfillment);
        assert!(fulfillment_matches_condition(&condition, &fulfillment));
    }

    #[test]
    fn a_different_preimage_does_not_match() {
        let condition = derive_condition(&[7u8; 32]);
        assert!(!fulfillment_matches_condition(&condition, &[9u8; 32]));
    }

    #[test]
    fn a_packet_at_exactly_its_expiry_is_expired() {
        let deadline = at(2030, 1, 1, 0, 0, 0);
        assert!(is_expired(deadline, deadline));
    }

    #[test]
    fn a_packet_a_second_before_its_expiry_is_not_expired() {
        let expires_at = at(2030, 1, 1, 0, 0, 1);
        let now = at(2030, 1, 1, 0, 0, 0);
        assert!(!is_expired(expires_at, now));
    }

    #[test]
    fn a_packet_a_second_past_its_expiry_is_expired() {
        let expires_at = at(2030, 1, 1, 0, 0, 0);
        let now = expires_at + Duration::seconds(1);
        assert!(is_expired(expires_at, now));
    }

    #[test]
    fn a_forwarded_expiry_is_strictly_earlier_than_the_one_that_arrived() {
        let now = at(2030, 1, 1, 0, 0, 0);
        let expires_at = now + Duration::seconds(30);

        let forwarded = forwarded_expiry(expires_at, now).expect("30s leaves room to forward");

        assert!(forwarded < expires_at);
        assert_eq!(forwarded, expires_at - FORWARDING_MESSAGE_WINDOW);
    }

    #[test]
    fn a_packet_with_exactly_one_window_left_is_not_forwardable() {
        let now = at(2030, 1, 1, 0, 0, 0);
        // Shortening lands exactly on `now`, which `is_expired` already
        // calls expired: a packet must fulfil strictly before its deadline.
        assert_eq!(forwarded_expiry(now + FORWARDING_MESSAGE_WINDOW, now), None);
    }

    #[test]
    fn a_packet_with_less_than_a_window_left_is_not_forwardable() {
        let now = at(2030, 1, 1, 0, 0, 0);
        assert_eq!(
            forwarded_expiry(
                now + FORWARDING_MESSAGE_WINDOW - Duration::milliseconds(1),
                now
            ),
            None
        );
    }

    #[test]
    fn a_packet_with_a_hair_over_a_window_left_is_forwardable() {
        let now = at(2030, 1, 1, 0, 0, 0);
        let expires_at = now + FORWARDING_MESSAGE_WINDOW + Duration::milliseconds(1);

        let forwarded = forwarded_expiry(expires_at, now).expect("a hair over a window is enough");

        assert_eq!(forwarded, now + Duration::milliseconds(1));
    }

    #[test]
    fn an_already_expired_packet_is_not_forwardable() {
        let now = at(2030, 1, 1, 0, 0, 0);
        assert_eq!(forwarded_expiry(now - Duration::seconds(1), now), None);
    }

    #[test]
    fn a_delivery_budget_is_everything_the_packet_has_left() {
        let now = at(2030, 1, 1, 0, 0, 0);
        assert_eq!(
            delivery_budget(now + Duration::seconds(30), now),
            Some(Duration::seconds(30))
        );
    }

    #[test]
    fn a_termination_keeps_no_window_back_where_a_forward_would() {
        // PF-25 against PF-19 on the same packet: the last hop may spend
        // the whole remaining budget waiting for its app, because the hop
        // above already kept a window back for the answer's journey home.
        let now = at(2030, 1, 1, 0, 0, 0);
        let expires_at = now + Duration::seconds(30);

        let budget = delivery_budget(expires_at, now).expect("30s is a budget");
        let forwarded = forwarded_expiry(expires_at, now).expect("30s leaves room to forward");

        assert_eq!(budget, expires_at - now);
        assert_eq!(budget - (forwarded - now), FORWARDING_MESSAGE_WINDOW);
    }

    #[test]
    fn a_packet_with_a_millisecond_left_still_has_a_delivery_budget() {
        // The counterpart of `forwarded_expiry`'s refusal at a whole
        // window: a termination has nobody to hand the packet on to and no
        // window to keep, so any strictly positive remainder is a budget it
        // is entitled to spend.
        let now = at(2030, 1, 1, 0, 0, 0);
        assert_eq!(
            delivery_budget(now + Duration::milliseconds(1), now),
            Some(Duration::milliseconds(1))
        );
    }

    #[test]
    fn a_packet_at_exactly_its_expiry_has_no_delivery_budget() {
        let deadline = at(2030, 1, 1, 0, 0, 0);
        assert_eq!(delivery_budget(deadline, deadline), None);
    }

    #[test]
    fn an_already_expired_packet_has_no_delivery_budget() {
        let now = at(2030, 1, 1, 0, 0, 0);
        assert_eq!(delivery_budget(now - Duration::seconds(1), now), None);
    }

    proptest! {
        /// PF-19, both halves at once: a forward either keeps a whole
        /// message window back -- landing strictly before the expiry that
        /// arrived and strictly after `now`, so the packet that goes out is
        /// alive and shorter-lived than the one that came in -- or is
        /// refused outright. Which of the two happens is decided by one
        /// thing only: whether more than a window remained.
        #[test]
        fn a_forward_keeps_a_window_back_or_is_refused(
            expires_at_secs in 0i64..1_000_000_000,
            remaining_millis in -5_000i64..5_000,
        ) {
            let expires_at = Utc.timestamp_opt(expires_at_secs, 0).unwrap();
            let now = expires_at - Duration::milliseconds(remaining_millis);
            let remaining = expires_at - now;

            match forwarded_expiry(expires_at, now) {
                Some(forwarded) => {
                    prop_assert!(remaining > FORWARDING_MESSAGE_WINDOW);
                    prop_assert!(forwarded < expires_at);
                    prop_assert!(!is_expired(forwarded, now));
                    prop_assert_eq!(expires_at - forwarded, FORWARDING_MESSAGE_WINDOW);
                }
                None => prop_assert!(remaining <= FORWARDING_MESSAGE_WINDOW),
            }
        }

        /// PF-25: a delivery budget is exactly the packet's own remaining
        /// time whenever the packet is alive, and is refused outright
        /// whenever it is not. It is never zero or negative, which is the
        /// property that makes it safe to hand straight to a timer. Which
        /// of the two happens is decided by [`is_expired`] and by nothing
        /// else, so the app is asked on exactly the packets PF-02 would
        /// still admit and on no others.
        #[test]
        fn a_delivery_budget_is_the_whole_remainder_or_nothing(
            expires_at_secs in 0i64..1_000_000_000,
            remaining_millis in -5_000i64..5_000,
        ) {
            let expires_at = Utc.timestamp_opt(expires_at_secs, 0).unwrap();
            let now = expires_at - Duration::milliseconds(remaining_millis);

            match delivery_budget(expires_at, now) {
                Some(budget) => {
                    prop_assert!(!is_expired(expires_at, now));
                    prop_assert!(budget > Duration::zero());
                    prop_assert_eq!(budget, expires_at - now);
                }
                None => prop_assert!(is_expired(expires_at, now)),
            }
        }

        /// Only the exact preimage a condition was derived from ever matches
        /// it -- a claimed fulfilment cannot be forged by picking any other
        /// 32 bytes.
        #[test]
        fn only_the_derived_preimage_matches_its_condition(
            fulfillment in proptest::array::uniform32(any::<u8>()),
            other in proptest::array::uniform32(any::<u8>()),
        ) {
            let condition = derive_condition(&fulfillment);
            prop_assert!(fulfillment_matches_condition(&condition, &fulfillment));
            if other != fulfillment {
                prop_assert!(!fulfillment_matches_condition(&condition, &other));
            }
        }

        /// Expiry is a total order on the offset between `now` and
        /// `expires_at`: expired for every non-negative offset, not expired
        /// for every negative one.
        #[test]
        fn expiry_follows_the_sign_of_now_minus_expires_at(
            expires_at_secs in 0i64..1_000_000_000,
            delta_secs in -1_000_000i64..1_000_000,
        ) {
            let expires_at = Utc.timestamp_opt(expires_at_secs, 0).unwrap();
            let now = expires_at + Duration::seconds(delta_secs);
            prop_assert_eq!(is_expired(expires_at, now), delta_secs >= 0);
        }
    }
}
