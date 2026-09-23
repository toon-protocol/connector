# A carriage pin is published on the route that enforces it

**Status:** Accepted — **built** (TOON_Network#111). Extends [0050](0050-a-connectors-url-resolves-to-its-self-description.md): the self-description stays a node's one description of itself, and this record moves one of its facts from the node to the route. Completes the half of issue #701 [0046](0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md) claimed was closed by construction.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

**Falsifier:** `crates/connector-domain/src/node.rs` matching `^\s*Some\(policy\.to_string\(\)\)\s*$` — the _publish `"both"` rather than omit the field_ option, rejected below. `published_required_transport` answers `None` for the permissive default, which is the whole of what keeps an unpinned node's document byte-for-byte what it was; an unconditional `Some` there is that option having shipped after all.

**A route that requires a client carriage MUST name it on that route's own entry in the node
self-description**, under `requiredTransport`, in the same two spellings (`"http"`, `"btp"`) the
x402 greeting's `extra.requiredTransport` uses. A route that accepts either carries **no such key
at all** — not `"both"`, not `null`.

The per-node `requiredTransport` scalar stays exactly as it is: a summary for a node where one
answer covers every address it answers to. It is no longer the only place a pin is stated.

## The gap

[ADR 0046](0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md) said _"the
`requiredTransport` defect closes by construction"_, and [CF-15](../protocol/configuration-spec.md)
says a connector that pins a carriage MUST publish the requirement. Both were written against a
**per-node scalar**, derived by `agreed_required_transport` over the routes covering the node's own
`[node] addresses`. That scalar answers `None` — say nothing — whenever those routes disagree.

**Routes covering one node's own addresses disagree as a matter of course.** A node with a paid
apex and a free sub-lane has two policies the moment it pins the paid one, and that is not an exotic
deployment: it is the devnet relay. `GET /ilp` there advertised:

```
"ilpAddresses": ["g.toon.relay", "g.toon.relay.ephemeral"]
"peerCarriages": []
"routes": [{"prefix": "g.toon.relay", "price": "1"}, {"prefix": "g.toon.relay.ephemeral", "price": "0"}, …]
```

`g.toon.relay` is pinned to BTP; `g.toon.relay.ephemeral` is not. Two policies, no agreement, no
scalar — and so a document that named no carriage while the connector refused every HTTP-carried
write to `g.toon.relay` before it would even look at the payment. Confirmed by greeting each
destination unpaid on 2026-09-22: `g.toon.relay` answers `402` with
`extra.requiredTransport: "btp"`, the other three answer `402` with no such key. The pin was
enforced, per route, and stated nowhere a client reads before sending.

What that cost: TOON_Network's directory publisher runs `TOON_TRANSPORT=auto`, which exists to read
this field and dial what it asks for. With nothing to read it fell back to an HTTP one-shot, every
Profile, Listing and Liveness write was refused, and a provider that was running perfectly was
invisible in the directory for hours. The fix available at the time was to name `btp` by hand in
that provider's deploy bundle — configuration that repeats, per deployment, a fact the node already
knows.

## Why per route, and not a better scalar

Because **that is the granularity the refusal is decided at.** Both enforcement points —
`handle_ilp` on the HTTP carriage, `btp.rs` on the websocket — take
`Connector::client_route(&prepare.destination).transport_policy`: one longest-prefix lookup, per
packet, per destination. No node-wide value is consulted, so no node-wide value can describe the
answer except by accident.

Three alternatives were on the table:

- **Widen the scalar to every route, not just the node's own addresses.** Strictly worse: it makes
  disagreement _more_ likely, so the field falls silent on more nodes than it does today.
- **Publish `"both"` instead of omitting it.** Puts a key on the wire to say nothing, and still
  gives one answer for a node with two policies. It also breaks the byte-identical-document
  property an unpinned node has today.
- **Leave it, and let clients learn the pin from the `402`.** This is what happens now, and the
  refusal _is_ legible — once the connector can parse the client's wire at all. It costs a round
  trip on a good day, and on a bad one (an older build answering `OER length determinant wider
than 8 bytes`) it costs the client any idea of what is wrong. A backstop is not a discovery
  mechanism.

## Derived, never declared

The field comes off `Connector::client_route_prices`, which reads each prefix back through
`Connector::client_route` — the same call, returning the same `ClientRouteFacts`, that both
carriages refuse from. `ClientRoutePrice` carries the `TransportPolicy` beside the `Price` for
exactly the reason it carries the `Price`: so there is one value and nothing to keep in step.
`published_required_transport` is the single place that knows `"both"` means silence, and
`agreed_required_transport` defers to it, so the per-route entry and the per-node scalar cannot
reach different conclusions about one policy.

This satisfies [ND-07](../protocol/self-description-spec.md)'s "derived, never separately declared"
without an exception: the operator declares `transport` once, on the route, and every surface that
mentions it — the document, the greeting, `GET /ilp/routes/price` — projects that one declaration.

## ND-09 does not cover a pin

A pin is not a per-peer fact and publishing it discloses nothing. It says what a stranger must do
to use a route that is already published, at a price that is already published, to an address that
is already published. Withholding it keeps no secret; it only makes the route unusable to anyone
who did not guess.

## `GET /ilp/routes/price` too

The same lookup answers that endpoint, and a caller told what a destination costs and not what it
takes to reach it can pay in full and still be refused. It carries `requiredTransport` on the same
terms — present on a pinned destination, absent otherwise.

## What does not change

- **The refusal stays.** A client that ignores the document is still answered `402` with
  `extra.requiredTransport` on HTTP and `F02` with the same terms over BTP, before payment is
  considered. Advertising a rule is not the same as trusting everyone to have read it.
- **A node that pins nothing is untouched.** Every `requiredTransport` key is omitted rather than
  emitted as `"both"`, so its document is byte-for-byte what it was.
- **The per-node scalar keeps its meaning and its name.** A reader that only knows the scalar reads
  the same value it always did on a node where the scalar was ever true.

## Consequences

- A client SHOULD resolve a destination's carriage by the longest-prefix route entry that covers it
  and fall back to the per-node scalar, which is the order this document states the facts in.
- A pin becomes discoverable on the one free `GET` that bootstrapping already makes, so `auto`
  dials the pinned carriage on its first attempt.
- The fleet does not pick this up until a release and a pin bump reach each box; until then the
  devnet relay still publishes no pin, and a deployment against it still names its carriage by hand.
