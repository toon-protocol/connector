#!/usr/bin/env python3
"""Does the devnet fleet still agree with itself about a FORWARDED route?

Called by `.github/workflows/fleet-health.yml`. See that file's header for
why it exists; the short version is that fleet-health probed container state
and `GET /ilp/identity` only, and both stayed green through two multi-hour
outages on 2026-08-28 that a forwarded route would have shown in minutes.

This is the half of that gap which costs nothing. It reads each node's
PUBLIC `GET /ilp` self-description (ADR 0050) and holds the documents to
each other: no credential, no ssh, no packet, no money. What it therefore
CANNOT do is prove a packet crosses a peering -- only a paid probe does
that, and `fleet-health.yml`'s `paid-probe` job is the other half.

Output is the same `<OK|FAIL>\\t<subject>\\t<check>\\t<detail>` TSV the rest
of that workflow speaks, so the report table needs no second renderer.

    python3 tools/ci/fleet-peering-crosscheck.py <out.tsv>
    python3 tools/ci/fleet-peering-crosscheck.py --legs
    python3 tools/ci/fleet-peering-crosscheck.py --self-test
"""

import json
import sys
import urllib.request

# The ONE hardcoded fact: which URLs make up the fleet. Every prefix, price
# and settlement figure below is read off the documents those URLs serve. A
# hardcoded expectation would go stale the way fleet-ops.yml's dead scp path
# did -- and would then report success against a fleet it had stopped
# describing.
NODES = {
    "relay": "https://proxy.relay.devnet.toonprotocol.dev/ilp",
    "store": "https://proxy.ario.devnet.toonprotocol.dev/ilp",
    "gas": "https://proxy.gas.devnet.toonprotocol.dev/ilp",
}

# Settlement facts that MUST match across a peering, per chain. The
# settlement ADDRESS is deliberately absent: it is meant to differ, being
# each node's own. These are the shared facts a channel identifier is
# derived FROM (ADR 0059), so changing any of them on one box and not the
# other moves every channel id on one side only. That is the 2026-08-28
# 01:27Z outage exactly -- an SPL mint cutover, and Solana seeds the channel
# PDA with the mint (["channel", min(p1, p2), max(p1, p2), mint]).
SHARED_SETTLEMENT_FACTS = (
    "tokenNetworkRegistry",
    "tokenNetwork",
    "tokenAddress",
    "programId",
    "decimals",
)


def fetch_over_https(url):
    with urllib.request.urlopen(url, timeout=20) as answer:
        return json.load(answer)


def segments(address):
    return address.split(".")


def covers(candidate, prefix):
    """Does `candidate` route `prefix` under longest-prefix matching?

    On ILP SEGMENT boundaries, never on characters: `g.toon.relays` is not
    covered by `g.toon.relay`, and treating it as covered would report a
    dangling forward as healthy.
    """
    c, p = segments(candidate), segments(prefix)
    return len(c) <= len(p) and p[: len(c)] == c


def price_of(route):
    """A route row's price as (flat, per_kib). `GET /ilp` spells both as
    decimal STRINGS and omits `pricePerKib` entirely on a flat route."""
    return int(route.get("price", 0)), int(route.get("pricePerKib") or 0)


def crosscheck(nodes, fetch):
    """Hold every node's self-description to every other's.

    `fetch` takes a URL and returns the parsed document, or raises. Injected
    so `--self-test` can drive the failure branches without a fleet.
    """
    rows = []

    def row(ok, subject, check, detail):
        rows.append(("OK" if ok else "FAIL", subject, check, detail))

    docs = {}
    for name, url in sorted(nodes.items()):
        try:
            docs[name] = fetch(url)
            row(True, name, f"GET {url}", "200, self-description parsed")
        except Exception as error:  # noqa: BLE001 -- every failure is one verdict
            row(
                False,
                name,
                f"GET {url}",
                f"unreadable ({error}) -- every check about this node is "
                "blind, which is a failure and not a pass",
            )

    # A node whose document did not parse simply cannot be a far side; the
    # FAIL above is what is reported, and the cross-checks skip it.
    terminated = {n: set(d.get("ilpAddresses", [])) for n, d in docs.items()}
    advertised = {
        n: {r["prefix"]: r for r in d.get("routes", [])} for n, d in docs.items()
    }

    def settlements_of(name):
        return {s["chain"]: s for s in docs[name].get("settlements", [])}

    def check_peering(near, far, prefix):
        """The two checks that are about the PEERING rather than the route:
        can the two sides still derive the same channel, and can either of
        them still dial the other."""
        a, b = settlements_of(near), settlements_of(far)
        subject = f"{near} -> {far}"
        shared = sorted(set(a) & set(b))
        if not shared:
            row(
                False,
                subject,
                "a settlement chain in common",
                f"{near} settles on {sorted(a) or 'nothing'} and {far} on "
                f"{sorted(b) or 'nothing'}: no chain in common, so nothing "
                f"can pay for `{prefix}`",
            )
        for chain in shared:
            differing = [
                f"{fact}: {near}={a[chain].get(fact)!r} {far}={b[chain].get(fact)!r}"
                for fact in SHARED_SETTLEMENT_FACTS
                if (fact in a[chain] or fact in b[chain])
                and a[chain].get(fact) != b[chain].get(fact)
            ]
            agreed = [
                fact
                for fact in SHARED_SETTLEMENT_FACTS
                if fact in a[chain] and fact in b[chain]
            ]
            if differing:
                row(
                    False,
                    subject,
                    f"settlement facts agree on `{chain}`",
                    "the two sides derive channel ids from DIFFERENT facts, "
                    "so every channel id between them has moved on one side "
                    "only, and the other answers T00 'would not report the "
                    "claim state of channel ...' -- " + "; ".join(differing),
                )
            elif not agreed:
                row(
                    False,
                    subject,
                    f"settlement facts agree on `{chain}`",
                    "neither document states any of "
                    f"{list(SHARED_SETTLEMENT_FACTS)} for `{chain}`, so this "
                    "check is blind rather than passing",
                )
            else:
                row(
                    True,
                    subject,
                    f"settlement facts agree on `{chain}`",
                    "agree on " + ", ".join(agreed)
                    + " -- channel ids derive alike on both sides",
                )

        # Exactly one side of a peering dials; the other only accepts, and
        # advertises no carriage. So an empty `peerCarriages` on one side is
        # normal and says nothing. BOTH empty is not: once that session
        # drops, neither side can re-establish it and every packet is T01
        # until a human intervenes.
        near_carriage = docs[near].get("peerCarriages") or []
        far_carriage = docs[far].get("peerCarriages") or []
        if not near_carriage and not far_carriage:
            row(
                False,
                subject,
                "someone can dial",
                "NEITHER side advertises a peerCarriage. The peering may be "
                "up now, but nothing can re-establish it after a restart, "
                "and every packet is then T01 until a human re-peers it.",
            )
        else:
            row(
                True,
                subject,
                "someone can dial",
                f"{near} advertises {near_carriage or 'no carriage'}, "
                f"{far} advertises {far_carriage or 'no carriage'} -- only "
                "one side ever dials, so one of these being empty is normal",
            )

    # For every prefix a node ADVERTISES but does not TERMINATE, it is
    # forwarding: somebody else has to route that name, at a price this
    # node's own advertised price can cover, over a peering the two of them
    # still agree about.
    for near in sorted(docs):
        forwards = [p for p in sorted(advertised[near]) if p not in terminated[near]]
        if not forwards:
            row(
                True,
                near,
                "forwarded prefixes",
                "none advertised -- this node terminates everything it sells",
            )
            continue

        for prefix in forwards:
            near_flat, near_kib = price_of(advertised[near][prefix])

            # The far side is whoever routes this name under longest-prefix
            # matching. Never this node itself: `near` FORWARDS this prefix,
            # so its own longest match for it is the very row being checked.
            candidates = [
                (len(segments(q)), far, q)
                for far in sorted(docs)
                if far != near
                for q in sorted(advertised[far])
                if covers(q, prefix)
            ]
            if not candidates:
                row(
                    False,
                    f"{near} -> ?",
                    f"`{prefix}` lands somewhere",
                    "DANGLING FORWARD: this node advertises and forwards "
                    f"`{prefix}`, and no node in the fleet routes that name. "
                    "A payer is charged and then refused -- F02 at the far "
                    "side, or T01 here if the peering it points at cannot "
                    "even be dialled.",
                )
                continue

            _, far, far_prefix = max(candidates)
            row(
                True,
                f"{near} -> {far}",
                f"`{prefix}` lands somewhere",
                f"{far} "
                f"{'terminates' if far_prefix in terminated[far] else 'forwards on'}"
                f" `{far_prefix}`",
            )

            far_flat, far_kib = price_of(advertised[far][far_prefix])
            check = f"`{prefix}` is priced to cover the far side"
            if near_flat < far_flat or near_kib < far_kib:
                row(
                    False,
                    f"{near} -> {far}",
                    check,
                    f"UNDERPRICED FORWARD: {near} sells it at {near_flat}"
                    f"+{near_kib}/KiB but {far} charges {far_flat}"
                    f"+{far_kib}/KiB for `{far_prefix}`. The packet arrives "
                    "short and is refused after the payer has been charged.",
                )
            elif (near_flat, near_kib) == (far_flat, far_kib):
                row(
                    True,
                    f"{near} -> {far}",
                    check,
                    f"{near_flat}+{near_kib}/KiB, exactly the far side's "
                    "price -- covered, but this hop's own per-packet fee "
                    "comes out of the delivered amount rather than on top",
                )
            else:
                row(
                    True,
                    f"{near} -> {far}",
                    check,
                    f"{near_flat}+{near_kib}/KiB covers {far}'s {far_flat}"
                    f"+{far_kib}/KiB",
                )

            check_peering(near, far, prefix)

    # De-duplicated, order preserved: two forwarded prefixes over one
    # peering assert the same settlement agreement, and printing it twice
    # pads the table without saying anything twice.
    seen, unique = set(), []
    for entry in rows:
        if entry not in seen:
            seen.add(entry)
            unique.append(entry)
    return unique


def legs(nodes, fetch):
    """Every forwarded prefix in the fleet, as the paid probe needs it.

    `fleet-health.yml`'s `paid-probe` job drives `connector send` against
    these. It reads them from here rather than keeping a table of its own
    so there is ONE list of what the fleet forwards, derived from live
    documents -- a second copy is a copy that goes stale, and a paid probe
    aimed at a prefix nobody routes any more would report a real-looking
    failure about nothing.

    Yields `near`, `prefix`, `far`, `far url`, `amount`, tab-separated.
    `amount` is the near side's OWN advertised price for the prefix, one
    KiB's worth of slope included -- so it covers the far side's price plus
    every fee on the way, and tracks a repricing without an edit here.
    """
    docs = {name: fetch(url) for name, url in sorted(nodes.items())}
    out = []
    for near, doc in docs.items():
        terminates = set(doc.get("ilpAddresses", []))
        for route in doc.get("routes", []):
            prefix = route["prefix"]
            if prefix in terminates:
                continue
            flat, kib = price_of(route)
            # Longest-prefix matching, exactly as `crosscheck` resolves a
            # far side -- picking the FIRST node that merely covers the
            # prefix would seal a `g.toon.relay.store` probe to whichever
            # node happens to advertise the shorter `g.toon.relay`, and the
            # packet would then be refused for a reason that is this
            # script's fault rather than the fleet's.
            candidates = [
                (len(segments(q["prefix"])), far)
                for far, far_doc in docs.items()
                if far != near
                for q in far_doc.get("routes", [])
                if covers(q["prefix"], prefix)
            ]
            if candidates:
                _, far = max(candidates)
                out.append((near, prefix, far, nodes[far], str(flat + kib)))
    return out


# ─────────────────────────────────────────────────────────────────────────
# Self-test. A monitor whose FAIL branches have never run is a green tick
# over nothing, and this file's failure paths are unreachable from a healthy
# fleet by construction -- so they are driven here, against synthetic
# documents, and CI runs this before the workflow ever trusts the script.
# ─────────────────────────────────────────────────────────────────────────

def _doc(addresses, routes, settlements, carriages):
    return {
        "ilpAddresses": addresses,
        "routes": [
            dict({"prefix": p, "price": str(f)}, **({"pricePerKib": str(k)} if k else {}))
            for p, f, k in routes
        ],
        "settlements": settlements,
        "peerCarriages": carriages,
    }


_EVM = {
    "chain": "evm:84532",
    "tokenNetworkRegistry": "0xreg",
    "tokenNetwork": "0xnet",
    "tokenAddress": "0xusdc",
    "decimals": 6,
}
_SOL = {
    "chain": "solana",
    "programId": "prog",
    "tokenAddress": "mintA",
    "decimals": 6,
}


def _fleet(**overrides):
    fleet = {
        "relay": _doc(
            ["g.toon.relay"],
            [("g.toon.relay", 1, 0), ("g.toon.relay.store", 1001, 10)],
            [dict(_EVM), dict(_SOL)],
            [],
        ),
        "store": _doc(
            ["g.toon.store", "g.toon.relay.store"],
            [("g.toon.store", 1000, 10), ("g.toon.relay.store", 1000, 10)],
            [dict(_EVM), dict(_SOL)],
            ["btp"],
        ),
    }
    fleet.update(overrides)
    return fleet


def _run(fleet):
    def fetch(url):
        doc = fleet[url]
        if isinstance(doc, Exception):
            raise doc
        return doc

    return crosscheck({name: name for name in fleet}, fetch)


def _fails(rows):
    return [r for r in rows if r[0] == "FAIL"]


def _self_test():
    cases = []

    def case(name, rows, want_fail, want_in=""):
        failures = _fails(rows)
        ok = bool(failures) == want_fail and (
            not want_in or any(want_in in f[3] for f in failures)
        )
        cases.append((ok, name, failures))

    case("a healthy fleet passes", _run(_fleet()), False)

    # The 2026-08-28 01:27Z outage: an SPL mint cutover on one box only.
    cut = _fleet()
    cut["store"]["settlements"][1] = dict(_SOL, tokenAddress="mintB")
    case("an SPL mint cutover on one side fails", _run(cut), True, "tokenAddress")

    # A prefix forwarded to nobody -- the shape found live on the store box.
    dangling = _fleet()
    dangling["store"]["routes"].append({"prefix": "g.toon.store.relay", "price": "2"})
    case("a dangling forward fails", _run(dangling), True, "DANGLING FORWARD")

    # A far-side price rise the near side did not follow.
    under = _fleet()
    under["store"]["routes"][1]["price"] = "2000"
    case("an underpriced forward fails", _run(under), True, "UNDERPRICED FORWARD")

    # A per-KiB slope the near side did not follow, at an equal flat price.
    slope = _fleet()
    slope["store"]["routes"][1]["pricePerKib"] = "99"
    case("an unfollowed per-KiB slope fails", _run(slope), True, "UNDERPRICED FORWARD")

    # Nobody left who can dial: the peering cannot survive a restart.
    deaf = _fleet()
    deaf["store"]["peerCarriages"] = []
    case("neither side able to dial fails", _run(deaf), True, "NEITHER side")

    # No chain in common: nothing can pay.
    apart = _fleet()
    apart["store"]["settlements"] = [dict(_SOL)]
    apart["relay"]["settlements"] = [dict(_EVM)]
    case("no settlement chain in common fails", _run(apart), True, "no chain in common")

    # An unreachable node is a failure, never a skip.
    down = _fleet()
    down["store"] = OSError("connection refused")
    case("an unreadable self-description fails", _run(down), True, "blind")

    # `--legs` must name the same forwards the checks above cross-check,
    # priced at the near side's own advertised figure. A leg list that
    # drifted from the route list would aim the paid probe at the wrong
    # prefix, or pay the wrong amount, and report the miss as an outage.
    healthy = _fleet()
    found = legs({n: n for n in healthy}, lambda url: healthy[url])
    cases.append(
        (
            found == [("relay", "g.toon.relay.store", "store", "store", "1011")],
            "--legs names each forward once, at the near side's own price",
            found,
        )
    )

    # A third node advertising a SHORTER covering prefix must not win the
    # leg. `gas` routes `g.toon.relay`, which covers `g.toon.relay.store`;
    # sealing the probe to gas would have it refused for this script's
    # reason rather than the fleet's. Live shape, and the bug this case
    # was written against.
    three = _fleet(
        gas=_doc(
            ["g.toon.gas"],
            [("g.toon.gas", 1000, 0), ("g.toon.relay", 2, 0)],
            [dict(_EVM), dict(_SOL)],
            ["btp"],
        )
    )
    found = legs({n: n for n in three}, lambda url: three[url])
    sealed = dict((prefix, far) for _, prefix, far, _, _ in found)
    cases.append(
        (
            sealed.get("g.toon.relay.store") == "store",
            "--legs seals to the LONGEST match, not the first that covers",
            found,
        )
    )

    # Segment-boundary matching: `g.toon.relays` must NOT satisfy a forward
    # of `g.toon.relay.store`, or a dangling forward reads as healthy.
    assert covers("g.toon.relay", "g.toon.relay.store")
    assert not covers("g.toon.relays", "g.toon.relay.store")
    assert not covers("g.toon.relay.store", "g.toon.relay")

    for ok, name, failures in cases:
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            for failure in failures:
                print(f"        {failure}")
    if not all(ok for ok, _, _ in cases):
        raise SystemExit("self-test failed")
    print(f"\n{len(cases)} cases passed.")


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "--self-test":
        _self_test()
    elif len(sys.argv) == 2 and sys.argv[1] == "--legs":
        for leg in legs(NODES, fetch_over_https):
            print("\t".join(leg))
    elif len(sys.argv) == 2:
        with open(sys.argv[1], "w", encoding="utf-8") as out:
            for entry in crosscheck(NODES, fetch_over_https):
                out.write("\t".join(str(field) for field in entry) + "\n")
    else:
        raise SystemExit(__doc__)
