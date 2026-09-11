//! The rate table as a running node shares it: one slow writer, many hot
//! readers ([ADR 0071](../../../docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)
//! decision 6, issue #1294).
//!
//! [`connector_domain::RateTable`] is the decision-making half and has no
//! clock, no I/O and no opinion about who holds it. This module is the other
//! half: the handle the background poller writes through
//! ([`rate_poller`](crate::rate_poller)) and the forwarding path reads
//! through, built once at boot from the immutable declaration in
//! `[[tokens]]`, `[[rates]]` and `[rate_guards]`.
//!
//! # Why an [`ArcSwap`] and not a lock
//!
//! The two callers are lopsided on purpose. A forward reads the table for
//! **every packet that crosses a denomination boundary**, and ADR 0071
//! decision 6 says in as many words that the forwarding path does no I/O --
//! so a read must not block, must not wait behind a writer, and must not
//! leave a guard alive across an `await` in the middle of a forward. A
//! refresh happens once per `ttl`-derived cadence per quoted token, holds a
//! whole cloned table for the length of one `refresh` call, and can afford
//! anything.
//!
//! So a read is [`ArcSwap::load_full`]: one atomic, an `Arc` the caller owns
//! outright and may hold across as many `await` points as it likes, and no
//! lock anywhere in it. A write clones the table, mutates the clone and swaps
//! it in, serialised against other writers by a mutex the readers never
//! touch. The table is a handful of `BTreeMap` rows -- one per quoted token
//! plus one per `[[rates]]` row -- so the clone costs less than the RPC round
//! trip that produced the value being written.
//!
//! # "The forwarding path issues no I/O", asserted
//!
//! The whole reader API here is **synchronous**: [`SharedRateTable::lookup`]
//! and [`SharedRateTable::read`] are `fn`, not `async fn`, and neither takes
//! nor returns anything a runtime could be reached through. A caller cannot
//! await what has no future, so the forwarding path could not issue I/O
//! through this handle even by accident -- and the compile-time coercion at
//! the bottom of this file fails the build on the day one of them stops
//! being a plain function. The tests add the runtime half of the same claim:
//! a read taken with no reactor running at all.

use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use connector_config::DenominationConfig;
use connector_domain::{AssetId, GuardOverride, RateLookup, RateTable};

/// The rate table this node forwards against, shared between the background
/// poller that refreshes it and every packet that reads it.
///
/// Cheap to clone (two `Arc`s), and every clone is the same table: cloning
/// the handle shares the rows, it does not copy them.
#[derive(Clone)]
pub struct SharedRateTable {
    current: Arc<ArcSwap<RateTable>>,
    /// Serialises writers against each other so a read-modify-swap cannot
    /// lose a concurrent one. Readers never take it -- that is the whole
    /// reason the table is behind an [`ArcSwap`] rather than an `RwLock`.
    /// Held only across synchronous work, never across an `await`, which
    /// [`SharedRateTable::write`] being a plain `fn` enforces by
    /// construction.
    writing: Arc<Mutex<()>>,
}

impl SharedRateTable {
    /// Share an already-built table.
    pub fn new(table: RateTable) -> SharedRateTable {
        SharedRateTable {
            current: Arc::new(ArcSwap::from_pointee(table)),
            writing: Arc::new(Mutex::new(())),
        }
    }

    /// Build this node's table from what its config declares, or `None` for a
    /// node that declares nothing to deal.
    ///
    /// `None` is the ordinary case and the easy path: every node that
    /// predates ADR 0071 takes it, holds no table, starts no poller, and
    /// forwards exactly as it did before. Two configurations produce it, and
    /// both mean the same thing --
    ///
    /// * no numeraire, because no `[[tokens]]` row claimed one. A table
    ///   without one has nothing to compose cross rates through, which is why
    ///   [`RateTable::new`] requires it rather than modelling its absence.
    /// * a numeraire but no `[rate_guards]`, which config only permits when
    ///   there is no `[[rates]]` row and no quote path anywhere in the file
    ///   (a node declaring tokens for the same-asset cross-chain hop and
    ///   nothing else). The table it would build holds no row, so every
    ///   lookup answers `NotDeclared` either way -- and the alternative,
    ///   inventing a default `Guards` to satisfy the constructor, would be
    ///   this code declaring a dealing policy no operator wrote.
    ///
    /// Everything declared is loaded here, at boot, once: the pair guards
    /// from every `[[rates]]` row -- including the rows that carry no rate
    /// and exist only to tighten a pair -- and then the static rows
    /// themselves. Nothing observed is loaded, because nothing has been
    /// observed yet: a quote path's first reading is the poller's first tick.
    pub fn from_config(denomination: &DenominationConfig) -> Option<SharedRateTable> {
        let numeraire = denomination.numeraire()?;
        let defaults = denomination.guards()?;
        let mut table = RateTable::new(numeraire.clone(), defaults);

        // Guards before rates, so that a `[[rates]]` row which both declares
        // a rate and tightens the pair is in force as one thing the moment
        // the table exists -- and so a poller's first refresh of a pair is
        // already measured against the `max_move` the operator wrote for it.
        for row in denomination.rates() {
            let over = row.guard_override();
            if over != GuardOverride::default() {
                table.set_guards(row.from().clone(), row.to().clone(), over);
            }
        }
        for row in denomination.rates() {
            if let Some(rate) = row.rate() {
                table.declare(row.from().clone(), row.to().clone(), rate);
            }
        }

        Some(SharedRateTable::new(table))
    }

    /// What this connector converts `from -> to` at as of `now`, or why it
    /// will not -- the forwarding path's whole question, answered without a
    /// lock, without an allocation of its own and without anything to await.
    pub fn lookup(&self, from: &AssetId, to: &AssetId, now: DateTime<Utc>) -> RateLookup {
        self.current.load().lookup(from, to, now)
    }

    /// The table as it stands, owned.
    ///
    /// For a reader that asks more than one question of one consistent
    /// snapshot -- the operator status page enumerating
    /// [`RateTable::declared_pairs`] and looking each up (issue #1297), or a
    /// forward that wants the numeraire beside the rate. The value is an
    /// `Arc`, not a guard: holding it across an `await` blocks no writer and
    /// keeps no lock, it only means this reader goes on seeing the rows it
    /// started with while a refresh installs newer ones.
    pub fn read(&self) -> Arc<RateTable> {
        self.current.load_full()
    }

    /// Apply `change` to the table and publish the result, returning whatever
    /// the change answered -- the `Refresh` verdict, for the one caller that
    /// matters.
    ///
    /// Read-modify-swap under a writer mutex, so two writers cannot each
    /// clone the same table and have the later swap discard the earlier
    /// one's row. Readers are untouched throughout: they go on seeing the
    /// previous table until the swap, and the new one immediately after.
    pub fn write<R>(&self, change: impl FnOnce(&mut RateTable) -> R) -> R {
        let _writing = self
            .writing
            .lock()
            .expect("rate table writer lock poisoned");
        let mut next = RateTable::clone(&self.current.load());
        let answer = change(&mut next);
        self.current.store(Arc::new(next));
        answer
    }
}

impl std::fmt::Debug for SharedRateTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SharedRateTable")
            .field(&self.current.load())
            .finish()
    }
}

/// ADR 0071 decision 6's "the forwarding path does **no I/O**", asserted at
/// compile time rather than trusted.
///
/// Coercing the reader's whole API to plain `fn` pointers only type-checks
/// while these are synchronous functions: an `async fn` returns a future and
/// would not coerce, and neither would a signature that grew a handle to
/// anything a packet could wait on. It is a stronger argument than watching a
/// forward for sockets, because it holds for every future caller rather than
/// for the one a test exercised.
const _: fn(&SharedRateTable, &AssetId, &AssetId, DateTime<Utc>) -> RateLookup =
    SharedRateTable::lookup;
const _: fn(&SharedRateTable) -> Arc<RateTable> = SharedRateTable::read;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeDelta, TimeZone};
    use connector_domain::{Guards, MaxMove, Rate, Refresh, Spread, Ttl};

    /// USDC on Base -- the numeraire throughout, as in ADR 0071's own
    /// examples and in `connector-config`'s denomination tests.
    const USDC: &str = "evm:0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";
    /// ANYONE on Base: the dealt token, 18 decimals against USDC's 6.
    const ANYONE: &str = "evm:0x9ff58f4ffb29fa2266ab25e75e2a8b3503311656";

    fn asset(text: &str) -> AssetId {
        text.parse::<AssetId>().expect("a declared asset")
    }

    fn guards() -> Guards {
        Guards::new(
            Spread::none(),
            Ttl::new(TimeDelta::seconds(120)).expect("a positive ttl"),
            // Wide on purpose: these tests are about how the table is
            // shared, and a guard refusing a write would be a different
            // subject (`rate_poller` has that one).
            MaxMove::fraction(50, 100).expect("a max_move"),
        )
    }

    fn table() -> SharedRateTable {
        SharedRateTable::new(RateTable::new(asset(USDC), guards()))
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap() + TimeDelta::seconds(seconds)
    }

    fn rate(numerator: u64) -> Rate {
        Rate::new(numerator, 1).expect("a rate")
    }

    /// The claim this module exists to make good on, in the bluntest form it
    /// can be tested in: a read with **no reactor running at all**. A plain
    /// `#[test]` has no tokio runtime, so anything in the read path that
    /// waited on a socket, a timer or a spawned task would panic here rather
    /// than merely being slow.
    #[test]
    fn a_forwarding_read_needs_no_runtime() {
        let shared = table();
        shared.write(|table| table.refresh(asset(ANYONE), asset(USDC), rate(4), at(0)));

        let looked_up = shared.lookup(&asset(ANYONE), &asset(USDC), at(60));

        assert_eq!(looked_up.rate(), Some(rate(4)));
    }

    /// The other half of the sharing shape: what a read hands back is owned,
    /// so a forward may carry it across an `await` without holding a lock.
    /// This test only compiles if the value is `Send` and borrows nothing
    /// from the handle -- which is exactly the property a guard would not
    /// have.
    #[tokio::test]
    async fn a_read_may_be_held_across_an_await() {
        let shared = table();
        shared.write(|table| table.refresh(asset(ANYONE), asset(USDC), rate(4), at(0)));

        // Taken here, so that the refresh below lands strictly after it.
        let snapshot = shared.read();
        let held = tokio::spawn(async move {
            tokio::task::yield_now().await;
            snapshot.lookup(&asset(ANYONE), &asset(USDC), at(60))
        });
        shared.write(|table| table.refresh(asset(ANYONE), asset(USDC), rate(5), at(60)));

        assert_eq!(
            held.await.expect("the reader task").rate(),
            Some(rate(4)),
            "a held snapshot keeps the rows it was taken with"
        );
        assert_eq!(
            shared.lookup(&asset(ANYONE), &asset(USDC), at(60)).rate(),
            Some(rate(5)),
            "and a read taken afterwards sees the refresh"
        );
    }

    #[test]
    fn a_write_publishes_to_every_clone_of_the_handle() {
        let shared = table();
        let other = shared.clone();

        let verdict =
            other.write(|table| table.refresh(asset(ANYONE), asset(USDC), rate(4), at(0)));

        assert_eq!(verdict, Refresh::Accepted);
        assert_eq!(
            shared.lookup(&asset(ANYONE), &asset(USDC), at(60)).rate(),
            Some(rate(4))
        );
    }
}
