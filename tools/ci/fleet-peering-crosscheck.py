#!/usr/bin/env python3
"""Does the devnet's ONE connector still serve what its committed config says?

Called by `.github/workflows/fleet-health.yml`. See that file's header for
why it exists.

The devnet has run one connector since 2026-10-16 (relay#270, infra#57): the
relay's, at `https://toonprotocol.dev/ilp`, terminating every devnet app's
prefix itself. Before that each node ran its own connector and this script
held their `GET /ilp` documents to each other -- does every forwarded prefix
land on a node that routes it, at a price that covers it, over a peering
whose two sides settle in the same token. With one connector there is no
forwarded leg in the fleet left to hold, and reading the old per-node names
(`proxy.{relay,ario,gas}.devnet`) would read the same document three times
-- and stop resolving once infra#58 removes them. The file keeps its name
because the workflow, CI and `fleet_release_gate.rs` call it by that name.

What it holds now, free and with no credential, reading only public URLs:

  * the committed config -- `deploy/connector.toml` on the relay repo's
    `main`, the one file holding every devnet route and price -- against the
    connector's own `GET /ilp` (ADR 0050): every committed route is listed
    at the committed price and transport, the committed addresses are the
    ones it claims, it says it is at the committed endpoints, and it settles
    in the committed tokens. A missing route is a prefix a payer is refused;
    a price that differs is a merge that did not reach the host, or a host
    edit no merge made.
  * the relay's NIP-11 document at `relay.toonprotocol.dev` against the same
    `GET /ilp`: the relay tells every client which connector to pay, which
    key to seal to, at what price and in which tokens. If those disagree with
    the connector, every client is charged and refused -- the 2026-08-28
    01:27Z mint-cutover outage, in its one-connector shape.

What it CANNOT do is prove a packet reaches an app. The host probe asks
whether the connector shares a network with each app it routes to and gets
an answer from it; `paid-probe` sends a packet. This is the free half.

Output is the same `<OK|FAIL>\\t<subject>\\t<check>\\t<detail>` TSV the rest
of that workflow speaks, so the report table needs no second renderer.

    python3 tools/ci/fleet-peering-crosscheck.py <out.tsv>
    python3 tools/ci/fleet-peering-crosscheck.py --legs
    python3 tools/ci/fleet-peering-crosscheck.py --self-test
"""

import json
import sys
import time
import tomllib
import urllib.request
from datetime import datetime

# The hardcoded facts: where the one connector, the relay's NIP-11 document
# and the committed config live. Every route, price, address and token below
# is read off what those URLs serve -- a hardcoded expectation would go stale
# the way fleet-ops.yml's dead scp path did, and would then report success
# against a devnet it had stopped describing. None of them is a transitional
# name infra#58 removes; `--self-test` asserts that.
CONNECTOR_URL = "https://toonprotocol.dev/ilp"
RELAY_INFO_URL = "https://relay.toonprotocol.dev"
COMMITTED_CONFIG_URL = (
    "https://raw.githubusercontent.com/toon-protocol/relay/main/deploy/connector.toml"
)
# When `main` last changed that file. Read only when a route disagrees, so a
# merge still inside its apply window is reported as pending, not as an
# outage. Unauthenticated: it is asked at most once a run, and only then.
COMMITTED_CONFIG_HISTORY_URL = (
    "https://api.github.com/repos/toon-protocol/relay/commits"
    "?sha=main&path=deploy/connector.toml&per_page=1"
)
# Same grace as the host probe's staleness rule: one 5-minute timer tick to
# fetch, three more to apply or retry.
APPLY_GRACE_SECONDS = 20 * 60

TRANSITIONAL_NAMES = (
    "proxy.relay.devnet",
    "proxy.ario.devnet",
    "proxy.gas.devnet",
    "proxy.gateway.devnet",
    "proxy.echo.devnet",
    "relay-ws.devnet",
)


def fetch_json(url, accept="application/json"):
    request = urllib.request.Request(url, headers={"Accept": accept})
    with urllib.request.urlopen(request, timeout=20) as answer:
        return json.load(answer)


def fetch_text(url):
    with urllib.request.urlopen(url, timeout=20) as answer:
        return answer.read().decode("utf-8")


def committed_config_age(fetch=fetch_json, now=time.time):
    """Seconds since `main` last changed the committed config, or None."""
    commits = fetch(COMMITTED_CONFIG_HISTORY_URL)
    stamp = commits[0]["commit"]["committer"]["date"]
    when = datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp()
    return now() - when


def committed_price(route):
    """A committed route's price as (flat, per_kib). The TOML spells it as a
    bare integer or as `{ base, per_kib }`."""
    price = route.get("price", 0)
    if isinstance(price, dict):
        return int(price.get("base", 0)), int(price.get("per_kib", 0))
    return int(price), 0


def served_price(route):
    """A `GET /ilp` route row's price as (flat, per_kib). The document spells
    both as decimal STRINGS and omits `pricePerKib` on a flat route."""
    return int(route.get("price", 0)), int(route.get("pricePerKib") or 0)


def chain_of(network):
    """`eip155:84532` -> `evm`, `solana:Et...` -> `solana`."""
    return "evm" if network.startswith("eip155:") else network.split(":")[0]


def same_asset(chain, a, b):
    # An EVM address is the same address in any case; a Solana mint is not.
    if chain == "evm":
        return str(a).lower() == str(b).lower()
    return a == b


def crosscheck(fetch_doc, fetch_config, fetch_info, config_age):
    """Hold the one connector to its committed config and to the relay.

    Every input is injected so `--self-test` can drive the failure branches
    without a devnet. `config_age()` returns seconds since the committed
    config last changed, or raises when that cannot be read.
    """
    rows = []

    def row(ok, subject, check, detail):
        rows.append(("OK" if ok else "FAIL", subject, check, detail))

    def read(subject, url, fetch, what):
        try:
            value = fetch()
            row(True, subject, f"GET {url}", f"200, {what} parsed")
            return value
        except Exception as error:  # noqa: BLE001 -- every failure is one verdict
            row(
                False,
                subject,
                f"GET {url}",
                f"unreadable ({error}) -- every check that needs it is blind, "
                "which is a failure and not a pass",
            )
            return None

    doc = read("one connector", CONNECTOR_URL, fetch_doc, "self-description")
    config = read("relay connector.toml", COMMITTED_CONFIG_URL, fetch_config, "committed config")
    info = read("relay", RELAY_INFO_URL, fetch_info, "NIP-11 document")

    served = {}
    if doc is not None:
        for route in doc.get("routes", []):
            served.setdefault(route["prefix"], []).append(route)

    # ── The committed config against the connector ────────────────────────
    if doc is not None and config is not None:
        node = config.get("node", {})

        # A merge to the relay's `main` reaches the host within one timer
        # tick plus an apply. Inside that window a disagreement is the merge
        # still landing, and reporting it as an outage opens an alert that
        # closes itself on the next run. Asked once, only on a disagreement.
        age = []

        def pending():
            if not age:
                try:
                    age.append(config_age())
                except Exception:  # noqa: BLE001 -- unknown age means no grace
                    age.append(None)
            seconds = age[0]
            if seconds is not None and 0 <= seconds <= APPLY_GRACE_SECONDS:
                return f"main changed connector.toml {int(seconds // 60)}m ago, within the {APPLY_GRACE_SECONDS // 60}m apply grace"
            return None

        def judge(subject, check, ok, ok_detail, fail_detail):
            if ok:
                row(True, subject, check, ok_detail)
                return
            why = pending()
            if why:
                row(True, subject, check, f"pending: {why} -- {fail_detail}")
            else:
                row(False, subject, check, fail_detail)

        want_http = node.get("http_endpoint")
        judge(
            "one connector",
            "is where the committed config says",
            doc.get("httpEndpoint") == want_http == CONNECTOR_URL
            and doc.get("btpEndpoint") == node.get("btp_endpoint"),
            f"httpEndpoint {doc.get('httpEndpoint')}, btpEndpoint {doc.get('btpEndpoint')}",
            f"the connector says it is at {doc.get('httpEndpoint')!r} / "
            f"{doc.get('btpEndpoint')!r}, the committed config says "
            f"{want_http!r} / {node.get('btp_endpoint')!r}, and this check "
            f"reads {CONNECTOR_URL!r}. A client that follows the self-description "
            "lands somewhere else.",
        )

        want_addresses = list(node.get("addresses", []))
        have_addresses = list(doc.get("ilpAddresses", []))
        missing = sorted(set(want_addresses) - set(have_addresses))
        extra = sorted(set(have_addresses) - set(want_addresses))
        primary_ok = bool(want_addresses) and have_addresses[:1] == want_addresses[:1]
        judge(
            "one connector",
            "claims the committed addresses",
            not missing and not extra and primary_ok,
            f"{len(have_addresses)} addresses, primary {have_addresses[:1]}",
            f"missing {missing}, not committed {extra}, primary "
            f"{have_addresses[:1]} vs committed {want_addresses[:1]}",
        )

        committed = {}
        for route in config.get("routes", []):
            committed[route["prefix"]] = route

        for prefix in sorted(committed):
            route = committed[prefix]
            subject = f"`{prefix}`"
            check = "listed at the committed price"
            rows_for = served.get(prefix, [])
            want_flat, want_kib = committed_price(route)
            want_transport = route.get("transport")
            if not rows_for:
                judge(
                    subject,
                    check,
                    False,
                    "",
                    f"MISSING ROUTE: committed at {want_flat}+{want_kib}/KiB to "
                    f"{route.get('handler_url')}, and the connector does not list it. "
                    "A payer for this prefix is refused.",
                )
                continue
            if len(rows_for) > 1:
                row(
                    False,
                    subject,
                    check,
                    f"SHADOWED ROUTE: listed {len(rows_for)} times "
                    f"({[served_price(r) for r in rows_for]}): a runtime route "
                    "(POST /routes/peers) carries the same prefix as a committed "
                    "one, and only one of them can win",
                )
                continue
            have = rows_for[0]
            have_flat, have_kib = served_price(have)
            have_transport = have.get("requiredTransport")
            judge(
                subject,
                check,
                (have_flat, have_kib) == (want_flat, want_kib)
                and have_transport == want_transport,
                f"{have_flat}+{have_kib}/KiB"
                + (f", {have_transport} only" if have_transport else ""),
                f"PRICE DIFFERS: committed {want_flat}+{want_kib}/KiB"
                f"{f' ({want_transport} only)' if want_transport else ''}, "
                f"served {have_flat}+{have_kib}/KiB"
                f"{f' ({have_transport} only)' if have_transport else ''}. "
                "The host is not running what relay's main says.",
            )

        # A route the connector lists that the config does not hold is a
        # runtime route -- `POST /routes/peers`, a forward to a peer outside
        # the fleet (a node that joined the relay, relay#268). Nothing free
        # can see where it lands, so it is named rather than judged.
        for prefix in sorted(set(served) - set(committed)):
            row(
                True,
                f"`{prefix}`",
                "a runtime route, not in the committed config",
                f"listed at {'+'.join(map(str, served_price(served[prefix][0])))}/KiB "
                "by a POST /routes/peers this check cannot follow; the devnet's "
                "own apps are all committed routes",
            )

        settlement = config.get("settlement", {})
        offered = {}
        for entry in doc.get("batchSettlements", []):
            offered.setdefault(chain_of(entry.get("network", "")), []).append(entry)
        for chain in sorted(settlement):
            want = settlement[chain].get("token_address")
            have = [e.get("asset") for e in offered.get(chain, [])]
            judge(
                "one connector",
                f"settles in the committed `{chain}` token",
                any(same_asset(chain, want, a) for a in have),
                f"{want}",
                f"committed {want!r}, offered {have or 'nothing'}: a client "
                "deposits into a token the committed config does not name",
            )

    # ── The relay's NIP-11 document against the connector ─────────────────
    if doc is not None and info is not None:
        toon = info.get("toon") or {}
        subject = "relay -> one connector"
        if not toon:
            row(
                False,
                subject,
                "the relay names a connector",
                "the NIP-11 document carries no `toon` block: no client can "
                "find out where to pay",
            )
        else:
            url = toon.get("connector_url")
            row(
                url == doc.get("httpEndpoint") == CONNECTOR_URL,
                subject,
                "the relay names the one connector",
                f"connector_url {url!r}"
                + ("" if url == CONNECTOR_URL else f", the one connector is {CONNECTOR_URL!r}"),
            )
            key = toon.get("connector_seal_key")
            have_key = (doc.get("edgeIdentity") or {}).get("publicKey")
            row(
                bool(key) and str(key).lower() == str(have_key).lower(),
                subject,
                "the relay names the connector's seal key",
                "the same key"
                if key and str(key).lower() == str(have_key).lower()
                else f"relay says {key!r}, the connector publishes {have_key!r}: "
                "every sealed payload is one the connector cannot open",
            )
            address = toon.get("ilp_address")
            row(
                address in doc.get("ilpAddresses", []),
                subject,
                "the relay's address is the connector's",
                f"{address!r}"
                + ("" if address in doc.get("ilpAddresses", []) else " is not claimed by the connector"),
            )
            relay_route = (served.get(address) or [{}])[0]
            want_price = toon.get("price")
            row(
                want_price is not None
                and relay_route
                and int(want_price) == served_price(relay_route)[0],
                subject,
                "the relay's price is the connector's",
                f"relay says {want_price}, the connector lists "
                f"{served_price(relay_route)[0] if relay_route else 'no route'} for `{address}`",
            )
            offered = {e.get("network"): e.get("asset") for e in doc.get("batchSettlements", [])}
            told = {s.get("network"): s.get("asset") for s in toon.get("settlement", [])}
            differing = [
                f"{net}: relay={asset!r} connector={offered.get(net)!r}"
                for net, asset in sorted(told.items())
                if not same_asset(chain_of(net or ""), asset, offered.get(net))
            ]
            row(
                bool(told) and not differing,
                subject,
                "the relay and the connector settle in the same tokens",
                "agree on " + ", ".join(sorted(told))
                if told and not differing
                else ("; ".join(differing) or "the relay names no settlement")
                + " -- a client pays in a token the connector will not admit",
            )

    return rows


def legs(fetch_doc, fetch_config):
    """Every committed route the connector lists, as the paid probe needs it.

    `fleet-health.yml`'s `paid-probe` job drives `connector send` against
    these. It reads them from here rather than keeping a table of its own
    so there is ONE list of what the devnet routes -- a second copy is a copy
    that goes stale, and a probe aimed at a prefix nobody routes any more
    would report a real-looking failure about nothing.

    Yields `prefix`, `amount`, `handler_url`, tab-separated. `amount` is the
    committed flat price plus one KiB's worth of slope, so it covers the
    route and tracks a repricing without an edit here.
    """
    doc, config = fetch_doc(), fetch_config()
    listed = {r["prefix"] for r in doc.get("routes", [])}
    out = []
    for route in sorted(config.get("routes", []), key=lambda r: r["prefix"]):
        if route["prefix"] not in listed:
            continue
        flat, kib = committed_price(route)
        out.append((route["prefix"], str(flat + kib), route.get("handler_url", "")))
    return out


# ─────────────────────────────────────────────────────────────────────────
# Self-test. A monitor whose FAIL branches have never run is a green tick
# over nothing, and this file's failure paths are unreachable from a healthy
# devnet by construction -- so they are driven here, against synthetic
# documents, and CI runs this before the workflow ever trusts the script.
# ─────────────────────────────────────────────────────────────────────────

_CONFIG = """
[node]
addresses = ["g.toon.relay", "g.toon.store", "g.toon.echo"]
http_endpoint = "https://toonprotocol.dev/ilp"
btp_endpoint = "wss://toonprotocol.dev/ilp/btp"

[[routes]]
prefix = "g.toon.relay"
handler_url = "http://relay:3100/write"
price = 1
transport = "btp"

[[routes]]
prefix = "g.toon.store"
handler_url = "http://store-dvm:3300/store"
price = { base = 1000, per_kib = 10 }

[[routes]]
prefix = "g.toon.echo"
handler_url = "http://echo-app:3000"
price = { base = 1, per_kib = 1 }

[settlement.evm]
token_address = "0x0C996d7c934c79a6255254875607Fe69df25C0E1"

[settlement.solana]
token_address = "34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU"
"""

_EVM = "0x0c996d7c934c79a6255254875607fe69df25c0e1"
_SOL = "34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU"
_KEY = "0x04915d29"


def _doc():
    return {
        "ilpAddresses": ["g.toon.relay", "g.toon.store", "g.toon.echo"],
        "httpEndpoint": "https://toonprotocol.dev/ilp",
        "btpEndpoint": "wss://toonprotocol.dev/ilp/btp",
        "edgeIdentity": {"publicKey": _KEY},
        "routes": [
            {"prefix": "g.toon.echo", "price": "1", "pricePerKib": "1"},
            {"prefix": "g.toon.relay", "price": "1", "requiredTransport": "btp"},
            {"prefix": "g.toon.store", "price": "1000", "pricePerKib": "10"},
        ],
        "batchSettlements": [
            {"network": "eip155:84532", "asset": _EVM},
            {"network": "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1", "asset": _SOL},
        ],
    }


def _info():
    return {
        "toon": {
            "ilp_address": "g.toon.relay",
            "connector_url": "https://toonprotocol.dev/ilp",
            "connector_seal_key": _KEY,
            "price": 1,
            "settlement": [
                {"network": "eip155:84532", "asset": _EVM},
                {"network": "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1", "asset": _SOL},
            ],
        }
    }


def _old_config():
    raise OSError("no commit history")


def _run(doc=None, config=_CONFIG, info=None, age=_old_config):
    doc = _doc() if doc is None else doc
    info = _info() if info is None else info

    def give(value):
        def fetch():
            if isinstance(value, Exception):
                raise value
            return value

        return fetch

    def parse():
        if isinstance(config, Exception):
            raise config
        return tomllib.loads(config)

    return crosscheck(give(doc), parse, give(info), age)


def _fails(rows):
    return [r for r in rows if r[0] == "FAIL"]


def _self_test():
    cases = []

    def case(name, rows, want_fail, want_in=""):
        failures = _fails(rows)
        ok = bool(failures) == want_fail and (
            not want_in or any(want_in in f[3] for f in failures)
        )
        cases.append((ok, name, failures if want_fail or failures else rows))

    healthy = _run()
    case("a healthy one-connector devnet passes", healthy, False)
    cases.append(
        (
            sum(1 for r in healthy if r[2] == "listed at the committed price") == 3,
            "every committed route gets its own row",
            healthy,
        )
    )

    gone = _doc()
    gone["routes"] = [r for r in gone["routes"] if r["prefix"] != "g.toon.echo"]
    case("a committed route the connector does not list fails", _run(gone), True, "MISSING ROUTE")

    cheap = _doc()
    cheap["routes"][2]["price"] = "999"
    case("a flat price that differs fails", _run(cheap), True, "PRICE DIFFERS")

    slope = _doc()
    slope["routes"][2]["pricePerKib"] = "99"
    case("a per-KiB slope that differs fails", _run(slope), True, "PRICE DIFFERS")

    either = _doc()
    del either["routes"][1]["requiredTransport"]
    case("a transport pin the host dropped fails", _run(either), True, "PRICE DIFFERS")

    # A merge still inside its apply window is pending, not an outage, and
    # one past it is an outage.
    case("a disagreement inside the apply grace is pending", _run(cheap, age=lambda: 300), False)
    case("a disagreement past the apply grace fails", _run(cheap, age=lambda: 3600), True, "PRICE DIFFERS")

    moved = _doc()
    moved["httpEndpoint"] = "https://proxy.relay.devnet.toonprotocol.dev/ilp"
    case("a connector advertising another endpoint fails", _run(moved), True, "says it is at")

    lost = _doc()
    lost["ilpAddresses"].remove("g.toon.store")
    case("an address the connector stopped claiming fails", _run(lost), True, "missing ['g.toon.store']")

    swapped = _doc()
    swapped["ilpAddresses"] = list(reversed(swapped["ilpAddresses"]))
    case("a different primary address fails", _run(swapped), True, "primary")

    # The 2026-08-28 01:27Z shape: a mint cutover on one side only.
    cut = _doc()
    cut["batchSettlements"][1]["asset"] = "mintB"
    case("a token the committed config does not name fails", _run(cut), True, "committed")

    runtime = _doc()
    runtime["routes"].append({"prefix": "g.toon.b4ad2718.echo", "price": "3"})
    rows = _run(runtime)
    cases.append(
        (
            not _fails(rows)
            and any(r[1] == "`g.toon.b4ad2718.echo`" and "runtime route" in r[2] for r in rows),
            "a runtime route to a joined node is named, not failed",
            rows,
        )
    )

    shadow = _doc()
    shadow["routes"].append({"prefix": "g.toon.echo", "price": "2", "pricePerKib": "1"})
    case("a runtime route shadowing a committed one fails", _run(shadow), True, "SHADOWED ROUTE")

    elsewhere = _info()
    elsewhere["toon"]["connector_url"] = "https://proxy.relay.devnet.toonprotocol.dev/ilp"
    case("a relay naming another connector fails", _run(info=elsewhere), True, "connector_url")

    rekeyed = _info()
    rekeyed["toon"]["connector_seal_key"] = "0x04deadbeef"
    case("a relay naming another seal key fails", _run(info=rekeyed), True, "cannot open")

    mint = _info()
    mint["toon"]["settlement"][1]["asset"] = "mintB"
    case("a relay settling in another token fails", _run(info=mint), True, "will not admit")

    priced = _info()
    priced["toon"]["price"] = 2
    case("a relay advertising another price fails", _run(info=priced), True, "relay says 2")

    case("a relay with no toon block fails", _run(info={"name": "x"}), True, "no `toon` block")

    case("an unreadable self-description fails", _run(doc=OSError("refused")), True, "blind")
    case("an unreadable committed config fails", _run(config=OSError("404")), True, "blind")
    case("an unreadable NIP-11 document fails", _run(info=OSError("refused")), True, "blind")

    # `--legs` names each committed route the connector lists, once, at the
    # committed flat price plus one KiB.
    found = legs(lambda: _doc(), lambda: tomllib.loads(_CONFIG))
    cases.append(
        (
            found
            == [
                ("g.toon.echo", "2", "http://echo-app:3000"),
                ("g.toon.relay", "1", "http://relay:3100/write"),
                ("g.toon.store", "1010", "http://store-dvm:3300/store"),
            ],
            "--legs names every committed route once, at base plus one KiB",
            found,
        )
    )

    # infra#58 removes the transitional names; nothing here may depend on one.
    urls = (CONNECTOR_URL, RELAY_INFO_URL, COMMITTED_CONFIG_URL, COMMITTED_CONFIG_HISTORY_URL)
    cases.append(
        (
            not any(name in url for name in TRANSITIONAL_NAMES for url in urls),
            "no URL here is a transitional name infra#58 removes",
            urls,
        )
    )

    for ok, name, detail in cases:
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            for entry in detail if isinstance(detail, (list, tuple)) else [detail]:
                print(f"        {entry}")
    if not all(ok for ok, _, _ in cases):
        raise SystemExit("self-test failed")
    print(f"\n{len(cases)} cases passed.")


def _live():
    return (
        lambda: fetch_json(CONNECTOR_URL),
        lambda: tomllib.loads(fetch_text(COMMITTED_CONFIG_URL)),
        lambda: fetch_json(RELAY_INFO_URL, accept="application/nostr+json"),
        committed_config_age,
    )


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "--self-test":
        _self_test()
    elif len(sys.argv) == 2 and sys.argv[1] == "--legs":
        doc, config, _, _ = _live()
        for leg in legs(doc, config):
            print("\t".join(leg))
    elif len(sys.argv) == 2:
        with open(sys.argv[1], "w", encoding="utf-8") as out:
            for entry in crosscheck(*_live()):
                out.write("\t".join(str(field) for field in entry) + "\n")
    else:
        raise SystemExit(__doc__)
