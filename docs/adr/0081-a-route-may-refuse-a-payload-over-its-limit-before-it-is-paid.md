# A route may refuse a payload over its limit before it is paid

**Status:** Accepted (owner decision, 2026-10-22, #1530) — **built**. Decides what [0065-price](0065-a-price-is-a-schedule-over-payload-length.md) deferred under _Not decided here_: a per-route **payload limit**, `max_bytes` (that record called it a ceiling, a word `CONTEXT.md` retires). **Narrows** [0020](0020-a-price-is-flat-and-attaches-to-a-handler.md)'s "a price buys an answer": an answer the operator has said it will not give at this size is not sold. Left [#1529](https://github.com/toon-protocol/connector/issues/1529) — raising the 2 MiB body limit — to a record of its own; **amended by [0082](0082-a-route-may-raise-the-request-limit-to-carry-its-payload-limit.md)**, which took it: decision 4's upper bound is now 64 MiB.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

**Falsifier:** none — the record claims nothing absent that a pattern over the tree could find: what it left undecided (a route that _raises_ the carriage's limit) was an absence of behaviour, which no symbol names, and [0082](0082-a-route-may-raise-the-request-limit-to-carry-its-payload-limit.md) has since decided it.

**A route may declare `max_bytes`. A packet whose sealed payload is longer is refused `F00` before
any claim for it is looked at, so its sender is charged nothing, and the limit is published wherever
the route's price is.** A packet exactly `max_bytes` long is priced and delivered as it always was,
and a route that declares no limit behaves and publishes byte for byte as it did before this record.

## Context

ADR 0065-price made a price a schedule over payload length, which removed the cheap tier a large
packet used to abuse. What it left is the case this record is about: an operator who will not do
the work above a size **at any price**.

`toon-protocol/blossom` sells blob storage per KiB on `g.toon.blossom.upload`, and its app refuses a
blob larger than it stores. Before this record the connector had no way to say so. A request under
the client edge's 2 MiB HTTP body limit but over the app's own limit was charged in full at the
route's schedule, delivered, and answered `413`, and the `413` rode home on a `FULFILL` — a refusal
the sender paid for (ADR 0020: a price buys an answer, and a `413` is one). Blossom's free
`HEAD /upload` preflight lets a careful client avoid that, but nothing made it avoid it.

## Decision

### 1. The length measured is the sealed payload's

`max_bytes` bounds `Prepare.data.len()` — the sealed gift wrap — which is the figure ADR 0065-price
charges on. It is a property of **carriage**: every gate that charges a price already has it, needs
to open nothing to read it, and so can refuse on it before it does anything else. A sender sealed its
own wrap, so it knows the length before it sends.

The decoded body's length was considered, because it is what an app's own limit is written against.
It was rejected for the reason 0065-price rejected pricing on it: only the termination can decode,
so the client edge would have to gate a claim before the one figure that decides the refusal exists
— the exact window this record closes. The cost falls on the operator: a `max_bytes` written
against an app's body limit must allow for the wrap's overhead — the envelope's framing, the
encryption's padding and the wrap itself — which the sender can compute exactly, having sealed it.

### 2. Every gate that charges the route's price refuses on its limit

There is one rule, applied wherever a price is applied: **a packet longer than the matched route's
`max_bytes` is refused `F00`.** The matched route is the one `Connector::client_route` selects —
the same lookup the price comes from — so a packet is refused under the limit of the route it would
have been charged by.

- **The client edge**, on both carriages, refuses after decoding the PREPARE and checking its
  transport policy, and **before** the x402 greeting, the over-carry bound or any claim header is
  read. An oversize packet's claim never reaches the claim journal and its watermark does not move,
  so the same claim then buys a packet within the limit. An oversize request is not greeted: no
  amount could buy it, and a greeting would invite a payment that cannot succeed.
- **A forwarded route** may declare `max_bytes` as well, and a hop forwarding under it refuses on
  it. The length is carriage, so a forwarding hop needs nothing of the packet it does not already
  have. A runtime route written by `POST /routes/peers`, and a lease, carry no limit.
- **A peer arrival** is refused `F00` by `Connector::handle_peer_prepare` before its amount is judged
  — an `F03` would tell the peer to pay more for something it cannot have — and the peer price gate
  passes an oversize packet through to that refusal rather than greeting it `F06`.
- **A probe** is answered with the same `F00` rather than with a cost: the refusal is the answer a
  real packet would get.

On a peer arrival the limit cannot come **before the voucher**. A peer's voucher is judged
inline, in arrival order, before the packet is decoded (ADR 0075 decision 6, `peer-carriage-spec.md`
§7.1), and its verdict and the packet's are independent there as they are for every other reject on
that path. What a peer arrival's refusal guarantees is that the app is never asked. An upstream hop
that should not forward an oversize packet at all says so with a `max_bytes` of its own on its
forwarded route, set from the limit this node publishes.

### 3. The refusal is `F00`, and it names both figures

ILPv4 has no "payload too large" code, and `F08` is the _amount_ being too large. `F00` Bad Request
is final, which is right: no retry of the same bytes can succeed. Its message is
`'<destination>' accepts at most <max_bytes> payload bytes; this packet carries <len>`, so a sender
learns the limit from the refusal as well as from the published terms, and its `accumulated_cost` is
`0` — nothing was charged and nothing on the path was paid for.

The limit is published as `maxBytes`, a JSON number (a length, never above 2 MiB, rather than an
amount), wherever the route's price is:

- the x402 greeting's `extensions.toon.info.maxBytes`;
- the route's entry in the node self-description (`GET /ilp`, ADR 0050);
- `GET /ilp/routes/price`.

It is **absent**, not `null` or `0`, on a route with no limit, so every document such a route
publishes is byte-identical to what it published before this record — the rule 0065-price applied
to `pricePerKib`. For the same reason the greeting's `extensions.toon.schema`, which rides every
greeting, does not list it: that schema admits properties it does not name. The peer carriages' own `F06` greeting quotes the figure alone, as it always has,
and does not carry it.

Under [ADR 0045](0045-a-behavioural-rule-is-normative-prose-until-its-vector-lands.md) the reject's code and
message and the new `maxBytes` field are wire surface, and they are **normative prose until a vector
covers them**. `vectors/wire-vectors.json` is unchanged: it pins packet bytes and reject sums, and
neither the REJECT encoding nor `accumulated_cost`'s meaning moved.

### 4. `max_bytes` lowers the carriage's limit, and never raises it

A route's `max_bytes` must be at least `1` — zero refuses every packet with a payload, which is a
route nobody can use rather than a limit — and at most **2 MiB**, the client edge's HTTP body limit
(axum's own `DefaultBodyLimit`, which `POST /ilp` does not override). `Config::load` refuses either
by name. The same bound holds on a BTP-only route, whose carriage (tungstenite's defaults: 64 MiB a
message, 16 MiB a frame) could in principle take more, because one figure for every route is simpler
to reason about than a bound that moves with `transport`, and raising it is the other decision.

That other decision is #1529's: a per-route limit that **raises** the 2 MiB body limit, so a route can
sell a blob larger than any request the edge accepts today. It is deliberately not taken here.
Raising a body limit is a decision about memory held per request and about exposure to a client that
sends large bodies it never pays for — not about refusal — and it gets its own record. When it is
taken, it may widen the upper bound above without changing anything else here.

## Considered options

**Measure the decoded body.** Matches the app's limit exactly. Rejected under decision 1: only the
termination can measure it, after the client edge has already gated the claim.

**Enforce only at the termination.** The smallest change. Rejected: the termination is reached after
the client edge has validated and journaled the claim, which is the charge this record exists to
prevent; and a forwarding hop would carry, and be paid for carrying, a packet its destination is
certain to refuse.

**`F99` Application Error.** It reads as the app refusing. Rejected: the app is never asked, and the
code would imply it was.

**One key that lowers or raises the limit (#1529 in the same record).** Rejected for decision 4's
reason: raising the limit is a different risk, decided on different grounds.

## Consequences

**An oversize request costs its sender nothing**, on either client carriage, and the app behind the
route never sees it. ADR 0020's "a `404` costs the same as a `200`" keeps holding for every answer
the app gives; what changes is that a request the operator has declared too large is not delivered
to be answered at all.

**Adding `max_bytes` to a committed config is not a breaking deploy for a config without it**, since
the key is optional, but a binary predating this record refuses a config that writes it, by
`deny_unknown_fields` — the image lands before the config, in the usual order.

**A `[[children]]` entry cannot carry `max_bytes`.** Like `request`, the key lives on `[[routes]]`
only; a child that needs a limit is written as a route.

**The operator owns the arithmetic between the app's limit and `max_bytes`.** A gift wrap adds
overhead to every body; an operator who sets `max_bytes` to the app's own body limit refuses a few
bodies the app would have taken, and one who sets it above lets a few through that the app will
`413`.

## Update (issue #1529)

[ADR 0082](0082-a-route-may-raise-the-request-limit-to-carry-its-payload-limit.md) decides what
decision 4 left open. A route's `max_bytes` may now be up to **64 MiB**, not 2 MiB, and one above
2 MiB raises the node's request limit, the figure a request body is read at, far enough to carry
it; every BTP socket reads at a fixed 64 MiB plus 64 KiB. Decision 3's `maxBytes` is therefore
never above 64 MiB. Everything else here stands: `max_bytes` is still checked on the decoded
PREPARE, before any claim, and refused `F00`.
