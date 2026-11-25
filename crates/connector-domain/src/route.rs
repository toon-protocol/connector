//! Pure route selection: given a destination and a set of route prefixes,
//! decide which one (if any) governs it. No I/O, no knowledge of what a
//! route's payload is -- callers pair the returned index back up with
//! whatever they store alongside each prefix.

use chrono::{DateTime, Utc};

use crate::condition::is_expired;

/// A route with prefix `p` governs `destination` when `destination` is
/// exactly `p`, or starts with `p` followed by a dot (RFC-0015 addresses
/// are dot-separated labels, so `g.example` must not match `g.exampleX`).
fn matches(prefix: &str, destination: &str) -> bool {
    match destination.strip_prefix(prefix) {
        Some(rest) => rest.is_empty() || rest.starts_with('.'),
        None => false,
    }
}

/// Which table a resolved route came from. The derived order is the
/// precedence at equal prefix length, least to most: a lease (issue #427) is
/// TTL-bound and pushed by an automated controller, so everything durable
/// outranks it; a runtime peer route (issue #884) is a deliberate, paid
/// relationship, so it outranks a lease; a config-file peer route outranks a
/// runtime one (a runtime write colliding with a config-file prefix is also
/// refused at write time); an app route terminates here and outranks all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RouteKind {
    Leased,
    RuntimePeer,
    ConfigPeer,
    App,
}

/// The route a destination resolves to: which table, the position of the
/// route in that table's iteration order, and the length of its prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedRoute {
    pub kind: RouteKind,
    pub index: usize,
    pub prefix_len: usize,
}

/// What a destination resolves to, from one look at the tables: where a
/// packet to it is sent, and which route prices it. They differ only when a
/// lease wins the forwarding target -- a lease carries no price, so it must
/// never zero the price of a configured route it outranks (issue #557).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    /// The route forwarding uses, leases included.
    pub target: Option<ResolvedRoute>,
    /// The route pricing uses: the same ranking with leases left out, so it
    /// is never [`RouteKind::Leased`].
    pub price_route: Option<ResolvedRoute>,
}

impl Resolution {
    /// Whether anything carries the destination -- exactly when forwarding
    /// has a target.
    pub fn has_route(&self) -> bool {
        self.target.is_some()
    }
}

/// Resolve `destination` across every route table: the forwarding target
/// and the price route ([`Resolution`]).
///
/// The precedence, which is the one place it is written down: the longest
/// matching prefix wins; at equal length [`RouteKind`] breaks the tie; a
/// lease whose expiry is at or before `now` never matches. Time is an
/// input, not read here.
///
/// Each table is an iterator of prefixes (leases carry their expiry), and
/// `index` counts positions in that iterator, so the caller pairs the answer
/// back up with whatever it stores beside each prefix (a lapsed lease is
/// skipped but still counted). Nothing is collected or copied: a per-packet
/// lookup allocates nothing (ADR 0015). A caller that does not hold one of
/// the tables passes `std::iter::empty()`.
pub fn resolve_route<'a>(
    destination: &str,
    now: DateTime<Utc>,
    app: impl IntoIterator<Item = &'a str>,
    config_peer: impl IntoIterator<Item = &'a str>,
    runtime_peer: impl IntoIterator<Item = &'a str>,
    leased: impl IntoIterator<Item = (&'a str, DateTime<Utc>)>,
) -> Resolution {
    let always = |prefix| (prefix, true);
    let app = best(destination, RouteKind::App, app.into_iter().map(always));
    let config_peer = best(
        destination,
        RouteKind::ConfigPeer,
        config_peer.into_iter().map(always),
    );
    let runtime_peer = best(
        destination,
        RouteKind::RuntimePeer,
        runtime_peer.into_iter().map(always),
    );
    let leased = best(
        destination,
        RouteKind::Leased,
        leased
            .into_iter()
            .map(|(prefix, expires_at)| (prefix, !is_expired(expires_at, now))),
    );
    let strongest = |routes: &[Option<ResolvedRoute>]| {
        routes
            .iter()
            .copied()
            .flatten()
            .max_by_key(|route| (route.prefix_len, route.kind))
    };
    Resolution {
        target: strongest(&[app, config_peer, runtime_peer, leased]),
        price_route: strongest(&[app, config_peer, runtime_peer]),
    }
}

/// The longest-prefix match within one table, over `(prefix, live)` pairs.
/// `index` is the position in the table as iterated, dead entries counted,
/// so it means the same thing to the caller whether or not any lapsed.
fn best<'a>(
    destination: &str,
    kind: RouteKind,
    prefixes: impl Iterator<Item = (&'a str, bool)>,
) -> Option<ResolvedRoute> {
    longest_match(
        destination,
        prefixes.map(|(prefix, live)| live.then_some(prefix)),
    )
    .map(|(index, prefix_len)| ResolvedRoute {
        kind,
        index,
        prefix_len,
    })
}

/// The longest-prefix step [`select_route`] and [`resolve_route`] share:
/// the position and prefix length of the most specific entry governing
/// `destination`. A `None` entry is skipped but still counted.
fn longest_match<'a>(
    destination: &str,
    prefixes: impl Iterator<Item = Option<&'a str>>,
) -> Option<(usize, usize)> {
    prefixes
        .enumerate()
        .filter_map(|(index, prefix)| prefix.map(|prefix| (index, prefix)))
        .filter(|(_, prefix)| matches(prefix, destination))
        .max_by_key(|(_, prefix)| prefix.len())
        .map(|(index, prefix)| (index, prefix.len()))
}

/// Select the most specific (longest-prefix) route governing `destination`,
/// or `None` if no `prefixes` entry governs it.
pub fn select_route(destination: &str, prefixes: &[&str]) -> Option<usize> {
    longest_match(destination, prefixes.iter().copied().map(Some)).map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_the_only_matching_route() {
        let prefixes = ["g.example.app"];
        assert_eq!(select_route("g.example.app", &prefixes), Some(0));
    }

    #[test]
    fn selects_a_route_for_a_deeper_destination() {
        let prefixes = ["g.example.app"];
        assert_eq!(select_route("g.example.app.sub", &prefixes), Some(0));
    }

    #[test]
    fn does_not_match_a_sibling_label_sharing_a_prefix() {
        let prefixes = ["g.example.app"];
        assert_eq!(select_route("g.example.appendix", &prefixes), None);
    }

    #[test]
    fn returns_none_when_nothing_matches() {
        let prefixes = ["g.example.app"];
        assert_eq!(select_route("g.other", &prefixes), None);
    }

    #[test]
    fn prefers_the_longest_matching_prefix() {
        let prefixes = ["g.example", "g.example.app"];
        assert_eq!(select_route("g.example.app.sub", &prefixes), Some(1));
    }

    #[test]
    fn returns_none_for_an_empty_route_table() {
        let prefixes: [&str; 0] = [];
        assert_eq!(select_route("g.example.app", &prefixes), None);
    }

    mod resolve {
        use super::super::*;
        use chrono::TimeZone;
        use proptest::prelude::*;

        fn t(secs: i64) -> DateTime<Utc> {
            Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
        }

        fn prefix() -> impl Strategy<Value = String> {
            proptest::collection::vec("[ab]{1,2}", 1..4).prop_map(|l| format!("g.{}", l.join(".")))
        }

        fn table() -> impl Strategy<Value = Vec<String>> {
            proptest::collection::vec(prefix(), 0..5)
        }

        fn leases() -> impl Strategy<Value = Vec<(String, i64)>> {
            proptest::collection::vec((prefix(), -5i64..5), 0..5)
        }

        fn refs(v: &[String]) -> impl Iterator<Item = &str> {
            v.iter().map(String::as_str)
        }

        fn run(
            dest: &str,
            app: &[String],
            cfg: &[String],
            rt: &[String],
            ls: &[(String, i64)],
        ) -> Resolution {
            resolve_route(
                dest,
                t(0),
                refs(app),
                refs(cfg),
                refs(rt),
                ls.iter().map(|(p, e)| (p.as_str(), t(*e))),
            )
        }

        proptest! {
            #[test]
            fn a_longer_matching_prefix_always_wins(
                dest in prefix(), app in table(), cfg in table(), rt in table(), ls in leases()
            ) {
                let got = run(&dest, &app, &cfg, &rt, &ls).target;
                let longest = app.iter().chain(&cfg).chain(&rt)
                    .map(String::as_str)
                    .chain(ls.iter().filter(|(_, e)| *e > 0).map(|(p, _)| p.as_str()))
                    .filter(|p| matches(p, &dest))
                    .map(str::len)
                    .max();
                prop_assert_eq!(got.map(|r| r.prefix_len), longest);
            }

            #[test]
            fn at_equal_length_the_ranking_holds(dest in prefix()) {
                let one = vec![dest.clone()];
                let live = [(dest.clone(), 1i64)];
                let pick = |app: bool, cfg: bool, rt: bool, lease: bool| {
                    run(
                        &dest,
                        if app { &one } else { &[] },
                        if cfg { &one } else { &[] },
                        if rt { &one } else { &[] },
                        if lease { &live } else { &[] },
                    ).target.map(|r| r.kind)
                };
                prop_assert_eq!(pick(true, true, true, true), Some(RouteKind::App));
                prop_assert_eq!(pick(false, true, true, true), Some(RouteKind::ConfigPeer));
                prop_assert_eq!(pick(false, false, true, true), Some(RouteKind::RuntimePeer));
                prop_assert_eq!(pick(false, false, false, true), Some(RouteKind::Leased));
                prop_assert_eq!(pick(false, false, false, false), None);
            }

            #[test]
            fn a_runtime_route_never_shadows_a_config_route_at_equal_length(
                dest in prefix(), app in table(), ls in leases()
            ) {
                let one = vec![dest.clone()];
                let got = run(&dest, &app, &one, &one, &ls).target.unwrap();
                prop_assert_ne!(got.kind, RouteKind::RuntimePeer);
            }

            #[test]
            fn an_expired_lease_is_never_the_target(
                dest in prefix(), app in table(), cfg in table(), rt in table(), ls in leases()
            ) {
                let got = run(&dest, &app, &cfg, &rt, &ls).target;
                if let Some(ResolvedRoute { kind: RouteKind::Leased, index, .. }) = got {
                    prop_assert!(ls[index].1 > 0);
                }
            }

            #[test]
            fn the_price_route_is_never_a_lease(
                dest in prefix(), app in table(), cfg in table(), rt in table(), ls in leases()
            ) {
                let got = run(&dest, &app, &cfg, &rt, &ls).price_route;
                prop_assert!(got.is_none_or(|r| r.kind != RouteKind::Leased));
            }

            #[test]
            fn has_route_exactly_when_forwarding_has_a_target(
                dest in prefix(), app in table(), cfg in table(), rt in table(), ls in leases()
            ) {
                let got = run(&dest, &app, &cfg, &rt, &ls);
                prop_assert_eq!(got.has_route(), got.target.is_some());
                let live = app.iter().chain(&cfg).chain(&rt).any(|p| matches(p, &dest))
                    || ls.iter().any(|(p, e)| *e > 0 && matches(p, &dest));
                prop_assert_eq!(got.has_route(), live);
            }

            #[test]
            fn with_no_leases_the_price_route_is_the_target(
                dest in prefix(), app in table(), cfg in table(), rt in table()
            ) {
                let got = run(&dest, &app, &cfg, &rt, &[]);
                prop_assert_eq!(got.price_route, got.target);
            }

            #[test]
            fn adding_a_non_matching_route_never_changes_the_answer(
                dest in prefix(), app in table(), cfg in table(), rt in table(),
                ls in leases(), extra in prefix(), which in 0usize..4
            ) {
                prop_assume!(!matches(&extra, &dest));
                let before = run(&dest, &app, &cfg, &rt, &ls).target;
                // Appended last, so existing indices are undisturbed.
                let (mut a, mut c, mut r, mut l) = (app.clone(), cfg.clone(), rt.clone(), ls.clone());
                match which {
                    0 => a.push(extra),
                    1 => c.push(extra),
                    2 => r.push(extra),
                    _ => l.push((extra, 1)),
                }
                prop_assert_eq!(run(&dest, &a, &c, &r, &l).target, before);
            }
        }

        #[test]
        fn a_lease_expiring_exactly_now_has_lapsed() {
            let got = resolve_route("g.a", t(0), [], [], [], [("g.a", t(0))]);
            assert_eq!(got.target, None);
        }
    }
}
