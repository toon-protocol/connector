//! zk_toon's **Slot**: a global slot counted from the network's original genesis, never from the
//! Mesa fork (ADR 0080 decision 16).
//!
//! zk_toon's `zk-note` crate takes slots as bare `u64`s, beside millisecond values that are `u64`
//! too. [`Slot`] keeps the two apart: a slot is built from a `u64` only where one is read off the
//! wire or a chain, and turned back into one only where zk_toon's functions, the [`Field`]
//! encoding of a public input or the wire require it. The edge calls the wrappers here, never
//! zk_toon's `u64` `subsidy_period` and `is_request_time_fresh`.
//!
//! [`Field`]: crate::Field

use crate::request::RequestPublic;
use std::fmt;
use zk_note::policy;

/// A zk_toon global slot, counted from the network's original genesis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slot(u64);

impl Slot {
    /// The slot numbered `slot`.
    #[must_use]
    pub const fn new(slot: u64) -> Slot {
        Slot(slot)
    }

    /// The slot's number, for zk_toon's functions, a field element and the wire.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// This slot `slots` later, or `None` on overflow.
    #[must_use]
    pub fn checked_add(self, slots: u64) -> Option<Slot> {
        self.0.checked_add(slots).map(Slot)
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The subsidy period `slot` falls in, counted from the vault's `creation` slot: zk_toon's
/// `policy::subsidy_period`, or `None` before the vault was created.
#[must_use]
pub fn subsidy_period(creation: Slot, slot: Slot) -> Option<u64> {
    policy::subsidy_period(creation.get(), slot.get())
}

/// Whether a request time is fresh against the current slot: zk_toon's
/// `policy::is_request_time_fresh`.
#[must_use]
pub fn is_request_time_fresh(request_time: Slot, current: Slot) -> bool {
    policy::is_request_time_fresh(request_time.get(), current.get())
}

/// The slot a request proof's public input says the request was made in.
#[must_use]
pub fn request_time(public: &RequestPublic) -> Slot {
    Slot(public.request_time)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrappers_are_zk_toons_policy() {
        let creation = Slot::new(100);
        assert_eq!(subsidy_period(creation, Slot::new(99)), None);
        assert_eq!(
            subsidy_period(creation, Slot::new(100)),
            policy::subsidy_period(100, 100)
        );
        assert!(is_request_time_fresh(Slot::new(993), Slot::new(1000)));
        assert!(!is_request_time_fresh(Slot::new(992), Slot::new(1000)));
        assert_eq!(Slot::new(7).to_string(), "7");
        assert_eq!(Slot::new(u64::MAX).checked_add(1), None);
    }
}
