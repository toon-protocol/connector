# A price is a schedule over payload length

**Status:** Accepted — **built** (#984). Amends [0020](0020-a-price-is-flat-and-attaches-to-a-handler.md): "a price is flat per packet" becomes "a price is a schedule over the packet's payload length", of which a flat price is the case whose slope is zero. Everything else in 0020 stands, its handler-granularity rule included. Extends [0011](0011-rejects-accumulate-fees-and-probes-discover-cost.md) — cacheability is preserved by publishing the schedule, not by the reject. Narrows [0040](0040-a-verified-payment-is-stated-to-the-app.md): `X-TOON-Amount` is the charge for that packet. [0010](0010-flat-per-packet-fee-and-minimum-delivery.md) and [0061](0061-a-fee-attaches-to-a-peering-not-to-a-route.md) are untouched — a **fee** is still flat. **The number 0065 is shared** with [_Mina leaves the repository_](0065-mina-leaves-the-repository.md), which landed an hour after this record from a branch that could not see it; cite this one as **0065-price** or by title, and see the [index](README.md) for why neither is renumbered (#1249).

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

**Falsifier:** `crates/connector-config/src/route.rs` matching `\bmax_bytes\b` — this record defers a per-route payload ceiling to its own decision and says no such field exists. The field cannot be spelled anything else in a route's schema, so its appearance here means the deferral was quietly resolved and this record's "Not decided here" section is stale.

A terminated route's price is **`base + per_kib × ceil(payload_len / 1024)`**, where
`payload_len` is the length of the packet's own `data` — the sealed gift wrap, whose
_contents_ stay opaque to everyone but the termination. A price with a zero slope is flat, and
that is what every route ADR 0020 could express already was. The unit an operator writes is
unchanged for a flat route: `price = 1000` still means what it meant.

## Context

ADR 0020 fixed a price as flat per packet and recorded the consequence in as many words:
_"A 100-byte and a 100 KB write to the same handler cost the same."_ That was correct for
the app the decision was taken against — a relay, whose work is nearly the same at any size.

Issue #984 measured it against one whose work is not. A live third-party node
(`g.drew.ario`) fronts ArDrive Turbo, whose upstream charges by the byte and is itself an
x402 resource server. At the committed `price = 1000` the node loses money on every job
above roughly 100 KB: break-even runs from ~3,000 base units at 100 KB to ~60,900 at the
2 MiB body ceiling. **A 61× span, and one scalar cannot express it.**

The deployed workaround is the one ADR 0020 prescribes — a second route at a second handler,
`g.drew.ario.xl` at 61,000 against a second backend container. It does not work. The tier is
advisory: nothing stops a client sending 2 MiB to the 1,000-unit route, and a client sending
100 KB to the expensive one overpays by up to 61×. `insert_consistent_handler_price` keys on
`handler_url`, so a size tier is a whole second deployment rather than a second price, and
the operator is running two of everything to approximate one number.

This is not the relay's pricing engine coming back. That engine priced by **Nostr kind**,
which requires reading what the packet carries, and ADR 0006 forbids this connector learning
what a kind is. Size is a different kind of fact, and the rest of this record is about why.

## Decision

### The schedule

A price is `base` plus `per_kib` for every **started** kibibyte of payload. Both figures are
in the settlement asset's base units, like every other amount on the value path; nothing
scales by `decimals`. The arithmetic saturates rather than overflowing, so a schedule an
operator writes carelessly answers `u64::MAX` — a charge no claim can cover, which refuses
the packet — instead of panicking on the packet path.

Per **kibibyte** and not per byte, because per byte cannot express the thing this record
exists for: the slope #984 measured is ~0.03 base units per byte at 6-decimal USDC, which
rounds to zero in the integer units amounts are counted in. That is the same defect ADR 0010
removed when it deleted the basis-point fee, and it would have been reintroduced by the
finer unit, not avoided by it.

An empty payload is charged `base` alone. A flat price is `per_kib = 0`, and is _the same
value_ as the bare integer — not a separate case — which is why one handler priced `1000` by
one route and `{ base = 1000, per_kib = 0 }` by another is agreement rather than a conflict.

### The length is the sealed wrap's, which is carriage rather than content

ADR 0020 rejected per-byte pricing partly because pricing at a termination "looks as though
it must be content-sensitive". **It is not, if the measured quantity is the length of the
wrap rather than anything inside it.**

`Prepare.data.len()` is a property of **carriage**, in exactly the sense ADR 0016 gives the
word: every hop already handles those bytes, moves them, and counts them against a frame
limit. Reading their length requires opening nothing. That is what makes the rule uniform
where a content-sensitive one could not be —

- the **client edge** charges it on a forwarded route (ADR 0028), where the payload is sealed
  to somebody else entirely and this node could not read it if it wanted to;
- the **peer price gate** charges it on arrival (ADR 0029), likewise;
- the **termination** charges the same figure for the same bytes, and computes it _before_
  it opens the wrap, so there is no second length inside to disagree with.

And it is a figure the **sender** already has. A sender seals its own envelope, so it knows
the payload length before it sends and can compute the charge itself. Nothing about the
connector's answer is a surprise it has to discover by being refused.

The envelope's _decoded_ length was considered and rejected: it would price the same packet
differently at a forwarding hop and at its termination, because only one of them can see it.

### Cacheability moves to the greeting, and is stronger for it

ADR 0011's second ground against per-byte pricing was cacheability: _"a probe would report
only the cost of a packet its own size, so a sender would have to probe with a same-size
dummy before every write."_

That is true of the reject, and the reject is unchanged: `accumulated_cost` stays a single
sum, evaluated at the probe's own payload length, with no per-hop breakdown and no split
between fees and price. What changes is that a sender no longer has to read the cost off a
reject at all, because **the schedule itself is published**:

- the **x402 greeting** carries `extra.price` (the base) and `extra.pricePerKib` (the slope)
  beside `amount`, which remains what _this_ request costs;
- the **node self-description** (`GET /ilp`, ADR 0050) carries both per priced prefix;
- `GET /ilp/routes/price` carries both.

So one free, unauthenticated read answers **every** size, where before one probe answered
every size only because there was one number to answer with. A sender computes
`cost(len) = probe_cost − charge(probe_len) + charge(len)` for the terminating leg without a
second round trip. The property ADR 0011 wanted — do not make a sender probe per size — is
kept, by a surface that was already free.

`extra.pricePerKib` is **absent**, not `"0"`, on a flat route. Every document a flat route
publishes is therefore byte-identical to what it published before this record, and a reader
written against the flat greeting is unaffected.

### Every gate charges the same figure

There is one rule and it is applied everywhere a price was applied before: **what this packet
costs is the route's schedule evaluated at this packet's payload length.** The client edge's
claim gate, both carriages' greetings, the forwarded route's over-carry bound, the peer
arrival's `F03`, a probe's reject, the envelope-decode `F01`, the deadline `R00`, and a
mismatched fulfilment's reject all read that one figure. `validate_price` itself is
unchanged — its callers compute the charge and hand it in.

### Handler granularity is unchanged

ADR 0020's rule stands word for word, with "price" reading as "schedule": one handler, one
schedule, and an operator charges differently for different _work_ by exposing a handler for
each. What this record adds is that charging differently for the same work at different
**sizes** is no longer a reason to publish a second route at a second handler. The two axes
are separate, and only the size one moves here.

`Config::load` still refuses two routes naming one handler at different prices, comparing
whole schedules — the reason is the one ADR 0020 gave: the app cannot tell which request
arrived under which, so the cheaper would always win.

### What the app is told

`X-TOON-Amount` is the charge for **this** delivery (ADR 0040), which is what its own record
already says it is — _"the price this connector charged, never the amount field of the
arriving packet"_. For a flat route that is the flat price, unchanged. The app is still told
nothing else about the payment, is still payment-oblivious, and still receives no length it
did not already have in its own request body.

## Not decided here

**A per-route payload ceiling** (`max_bytes`). #984 proposes one, and a schedule removes most
of its motive: with a slope there is no longer a cheap tier for a large packet to abuse,
because a large packet simply costs more. What remains — an operator wanting to refuse work
above a size at any price — is a different decision about refusal rather than about pricing,
and it gets its own record. Today the only ceiling is the HTTP body limit.

**App-quoted pricing.** For an upstream whose price moves under the operator's feet — an
Arweave or ArNS buy priced in AR at the moment of purchase — a schedule an operator commits
to a config file is an estimate, and the node wears the difference. #984 names the
alternative: the handler returns a price and the edge collects against it. ADR 0020's ground
for rejecting it stands unchanged here (it makes the app payment-aware and adds a round trip
inside the packet's own ADR 0064 deadline), so this record does not take it. It narrows what
that future record would have to be about: not "prices vary", which is now expressible, but
"prices vary in ways the operator cannot know in advance".

## Considered options

**Per byte rather than per KiB.** The obvious unit. Rejected: unrepresentable in integer base
units at the slope actually observed, as above.

**Two routes and two handlers, as ADR 0020 prescribes.** The status quo, deployed. Rejected
because it is deployed and does not work: the boundary is advisory, so the cheap route takes
the large packets anyway, and the expensive route overcharges the small ones. It also costs a
second backend deployment per tier.

**Price on the decoded envelope's length.** More precise — it excludes the wrap's overhead.
Rejected: only the termination can decode, so the client edge and every peer gate would have
to charge a different figure from the one finally taken, which is the property this record
most needs to keep.

**Leave the reject to answer per size and publish nothing.** The smallest change. Rejected:
it is exactly the defect ADR 0011 named, and it would have made every sender probe before
every differently-sized write.

## Consequences

**A sender pays for the wrap's overhead**, not only for its own content, since the sealed
length is what is measured. It is a constant per packet, and it is a cost the connector
genuinely carries.

**A committed config that gains a schedule is a breaking deploy.** A binary predating this
record refuses the table form by `deny_unknown_fields`, so the image lands before the config,
per the usual ordering. The devnet fleet stays flat by choice, so nothing about it changes.

**The ADR 0028 path invariant now has two halves.** A hop collects `price`, retains a flat
`fee` and forwards the rest, and the path adds up only while every hop's `price − fee` is at
least the next hop's price. With a slope that must hold at _every_ length, which means the
bases must clear the fee **and** each hop's slope must be at least the next hop's — otherwise
a large enough packet erodes to a shortfall that the small ones never revealed. No code
enforces this, for the reason ADR 0028 gives (a connector cannot know what the next hop
charges); `local_topologies_load.rs` holds it for the committed topologies and now asserts
their flatness rather than reading past a slope.

**A runtime peer-route snapshot carrying a schedule cannot be read by an image predating this
record.** A flat one can, in both directions: a flat price serialises as the bare integer it
always was.

**`extra.pricePerKib` and the self-description's `pricePerKib` are new wire surface**, and
under ADR 0045 they are normative prose until a vector covers them. The wire vectors
themselves are unchanged: they carry reject sums and packet bytes, and neither the reject
encoding nor `accumulated_cost`'s meaning moved.

**`docs/devnet-pricing.md` stays the price list**, and the store leg stays at 1000. What this
record changes for the fleet is that pricing the store leg by size is now a config edit
rather than a second box.

## Update (issue #1250) — the fleet took the schedule, and the price list stopped being a list

Both sentences above were true for one day and are now history, in opposite directions.

**The store leg is a schedule.** Probed 2026-08-28, `g.toon.store` and `g.toon.relay.store` are
both `{ base = 1000, per_kib = 10 }` on the store box, and the relay's forward to
`g.toon.relay.store` carries the same slope beneath a base of `1001`. The paragraph above called
the adoption "a config edit rather than a second box", and that is exactly what it cost: two lines
in `toon-protocol/store`'s `deploy/connector.toml.template`, no second backend, and a store box
that answers one prefix at one price at every size. The breaking-deploy ordering this record
predicted held — the image landed before the config, as it must.

**`docs/devnet-pricing.md` is no longer the price list**, because
[0068](0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md) moved
deploy ownership into the node repositories a fortnight later. A box's schedule is now committed in
that box's own repository, guarded by that repository's own bundle test; a table in this one could
only ever be a copy going stale. That file now says where each box's authority lives and keeps only
what is genuinely fleet-wide — the unit, the path arithmetic, and how to ask a box directly.

Nothing in the decision moves. What moved is which repository the numbers live in.
