# A route declares its request shape, and the connector never reads it

**Status:** Accepted — **built** (#1210); amended by #1479 (the runtime write carries `request` too — see the Amendment below). Extends [0050](0050-a-connectors-url-resolves-to-its-self-description.md) and [0046](0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md): the self-description and the greeting stay the client's only surface, and this record adds one more fact to what they publish. Does not build [0044](0044-a-probe-answers-what-a-route-costs-and-what-it-does.md), which remains **not yet built** — see "Not the same as a description" below.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

**Falsifier:** `crates/connector-config/src/route.rs` matching `\bkinds\s*:` — this record's mechanism is one opaque table, never a typed field naming a job kind or any other app concept. A struct field spelled `kinds` would mean the first-class-field option (rejected below) shipped instead.

**A route's `[[routes]]` entry MAY carry `request`, an arbitrary TOML table.** The connector
validates only that it **is** a table — nothing about its keys, shape or meaning — converts it to
JSON once at load, and publishes it verbatim on that route's entry in the self-description and on
the x402 greeting for that destination. It never inspects a key inside it, never fetches anything to
produce or check it, and never varies its own behaviour based on what it contains.

## The gap

A client that discovers a node through `GET /ilp` or probes a route learns what that route costs
and nothing about what to send it. For a route whose app expects a specific payload shape — the
devnet gas station's NIP-90 `kind:5096`/`kind:5098` jobs are the concrete case that surfaced this —
that has to travel out of band, or the client makes a second call to the app's own health endpoint,
which means learning a second hostname exists at all.

## Options considered, and why the shape below won

Filed as issue #1210 with four shapes on the table, ranked by how much they cost the connector's
app-agnosticism:

- **A pointer, not the content** (`app_descriptor = "https://…/health"`). Cheapest for the
  connector — it publishes a URL it never dereferences — but strictly worse for the client: still
  two round trips, now merely discoverable ones.
- **An opaque blob the operator fills in** — this record's shape.
- **A first-class field** (`kinds = [5096, 5098]`). Simplest to consume, and the first thing in the
  self-description that is about the app rather than about money. The next app along wants a field
  that is not `kinds`, and the connector would grow one per app forever.
- **The connector proxies the app's own descriptor at request time.** One document, no drift, but a
  live dependency on the app inside a surface that otherwise answers from memory — a slow or down
  app would make `/ilp` slow or partial.

The issue thread's own first pass leaned toward the pointer, on the reasoning that the connector
should stay entirely ignorant of app semantics. That reasoning doesn't survive contact with
[0050](0050-a-connectors-url-resolves-to-its-self-description.md)'s own argument: `relay_url` was
deleted specifically because it was a second declaration of an app fact nothing compared against
the app that supposedly had it (issue #981's shape). Fetching from the app — this record's fourth
option — looked like the fix for that until the app side priced it out: a live dependency
(fetch-at-boot, refresh-on-interval, cache-the-last-good-answer) for a fact an operator can simply
write down.

## Why declaring it doesn't reintroduce issue #981

Issue #981 was two declarations of one fact with **nothing comparing them** — `[announce].solana_chain_id`
defaulted to `solana:devnet`, was never checked against `[settlement.solana]`, and a mainnet node
described itself as devnet. The lesson generalizes to: a fact this connector can **derive** — by
asking a backend it already verified against a chain at startup — must never also be **declared**,
because the declaration drifts and nothing catches it.

`request` is not that shape, for a reason specific to it: **this connector has no way to derive it.**
There is no settlement backend, no on-chain program and no protocol the connector already speaks that
tells it what an arbitrary app's payload looks like. The only two sources are (a) an operator writing
it down, or (b) fetching it from the app — and (b) is a live dependency this record explicitly declines,
for the same reason [0050](0050-a-connectors-url-resolves-to-its-self-description.md) never made the
self-description depend on anything but this node's own config and its own verified chain state.

That leaves declaration as the only mechanism available, and the drift issue #981 warns about is real
but lands somewhere else: **the app's own repository**, where the composition of app + connector is
assembled (`deploy/README.md`'s job, not this repo's). An app whose bundle names a `request` table its
own code does not register at boot is a bug in that repository's CI to catch, the same way a
`connector.toml` naming a settlement chain the connector cannot reach is a bug this repository's own
config validation catches. This connector is not the party that can check the comparison, because it
is not a party to either side of it.

## Not the same as a description

[ADR 0044](0044-a-probe-answers-what-a-route-costs-and-what-it-does.md) is a different, still-unbuilt
decision: a short, **operator-written free-text** description of what a route's work is, riding on a
probe's reject. `request` is **structured** and says what to **send**, not what the route **does**;
neither subsumes the other, and building this one does not build 0044. `CONTEXT.md`'s **Request** term
says so explicitly.

## Where it rides, and why beside `resource` rather than inside `accepts[]`

Two surfaces, both already carrying route facts:

- **The self-description's route entry** (`RoutePrice` in `connector-domain`), beside `prefix` and
  `price`. A whole-node view, one entry per route.
- **The x402 greeting** (`X402PaymentRequired` in the same crate), as a **top-level** member sitting
  beside `resource`, not nested inside `accepts[]`. It describes the **resource** the greeting is
  about — what this route is, independent of which payment method a payer ends up satisfying — not a
  property of one particular payment option. `accepts[]` today holds exactly one entry (the
  `toon-channel` scheme); a second scheme entering `accepts[]` later must not have to repeat this
  field, which nesting it there would force.

Both surfaces already source from one place — `connector_runtime::Connector::client_route` /
`client_route_prices` — so a route with `request` configured cannot show one value in `GET /ilp` and
a different one in the greeting for the same prefix; there is exactly one lookup either surface reads.

## Consequences

**Omitted, not `null`, when unconfigured.** `RoutePrice::request` and
`X402PaymentRequired::request` are both `#[serde(skip_serializing_if = "Option::is_none")]`. A node
with no `request` anywhere in its config publishes byte-for-byte what it published before this
record — the same discipline [0065](0065-a-price-is-a-schedule-over-payload-length.md) held for
`pricePerKib`, and for the same reason: an existing parser must be unaffected by a field it has
never seen.

**Not `deny_unknown_fields` inside.** The guarantee `deny_unknown_fields` gives — a typo is refused
rather than silently dropped — belongs to the `[[routes]]` row itself, whose keys this connector
does define the meaning of. It does not extend into a blob whose keys are the app's business; the
connector has no basis to call any key inside `request` a typo.

**A forwarded route can carry one too, for the identical reason a terminated one can.** The
connector doesn't interpret either kind's `request` any more than it interprets a terminated one's,
so there is no argument for allowing it on one branch of `[[routes]]` and not the other.

**This is a breaking config-shape change under [0009](0009-one-typed-config-file-no-environment-layer.md)'s
own rule.** Adding a recognized key to a `deny_unknown_fields` schema needs the config landed before
the binary that requires it moves past a node's pin — the usual rule for any new key, and `request`
is not an exception to it. No fleet node's config is changed by this record; that is deliberately out
of scope (issue #1210's own scope note) and happens per node when its repository bumps its pin.

## Amendment (issue #1479, 2026-10-07): the runtime write carries `request` too

`POST /routes/peers` accepts an optional `request`, with the meaning and opacity above: the connector
checks only that it is a JSON object (anything else is a `400`) and never reads inside it. It is
stored on the durable runtime peer/route row (omitted when absent, so a route without one is
persisted and published as before), replaced whole by the next write to the prefix, and published by
`GET /ilp` and the x402 greeting through `Connector::client_route`. `GET /routes/peers` reports it
on config and runtime rows alike. This makes "a forwarded route can carry one too" true of a route
through a peering made with `POST /peers`, which a config `[[routes]]` row cannot name.
