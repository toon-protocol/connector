# A route may raise the request limit to carry its payload limit

**Status:** Accepted (owner decision, 2026-10-22, #1529) — **built**. Takes the decision
[0081](0081-a-route-may-refuse-a-payload-over-its-limit-before-it-is-paid.md) left to a record of
its own, and **amends** that record's decision 4: a route's `max_bytes` may now exceed 2 MiB, up to
64 MiB, and the node reads request bodies large enough to carry it.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

**Falsifier:** `crates/*/src/**/*.rs` matching `tokio_tungstenite::(connect_async|accept_async)\(` — a websocket dialed or accepted with no `WebSocketConfig` reads at tungstenite's defaults, which this record replaces with a chosen figure on every BTP socket. (The axum upgrade on `GET /ilp/btp` has no such tell; its `max_message_size` is the test's to hold.)

**A node reads a request body up to one request limit: 2 MiB, or enough to carry the largest packet
any of its routes' `max_bytes` admits, whichever is larger. There is no key for it. Every BTP socket
reads up to one fixed figure, the largest request limit any node can have.** A route may declare
`max_bytes` up to 64 MiB. A node whose routes declare nothing above 2 MiB reads `POST /ilp` exactly
as it did before this record, and no BTP session refuses a message it read before.

## Context

`toon-protocol/blossom` sells blob storage per KiB on `g.toon.blossom.upload`. Before this record
`POST /ilp` read at axum's own `DefaultBodyLimit` of 2 MiB, which no config key could move, so no
route could sell a request larger than about 1.9 MiB, whatever its operator would charge. Images
fit; video and archives did not.

BTP had a limit too, and a different one: neither the client edge's `GET /ilp/btp` upgrade nor the
peer dialer configured its websocket, so both read at tungstenite 0.20's defaults — 64 MiB a
message, 16 MiB a frame. Nobody had chosen either figure, and a packet the HTTP carriage refused
the BTP one would carry.

## Decision

### 1. The request limit is read before the route is known, so it is one figure for the node

A packet's destination is inside the body. The body has to be read before it can be decoded, and
decoded before a route can be matched, so no per-route figure can bound the read itself. ADR 0081's
`max_bytes` is the per-route check, applied once the PREPARE is decoded and before any claim is
read; the **request limit** is what bounds the read that comes before it.

It applies to every HTTP body that carries a packet: a `POST /ilp` body, from a client or a peer, a
`POST /ilp/probe` body, and an operator's `POST /packets`.

### 2. It is derived from the routes, and is not a key

The request limit is `max(2 MiB, largest max_bytes + 64 KiB)`, over every configured route's
`max_bytes`, terminating and forwarded — a forwarding hop reads the whole packet too. The 64 KiB is
framing: on HTTP the PREPARE's own OER framing, at most about 1.1 KiB with a 1023-byte destination;
on BTP that plus the frame's header and its other protocol-data entries, a claim among them.

A separate key was considered and refused. The only reason to raise the limit is a route that sells
something larger, and that route has to declare `max_bytes` anyway to publish the size it accepts
(0081 decision 3). A second key would be a second number to keep in step with the first, with a
load-time refusal for when it is not. A route written at runtime by `POST /routes/peers`, and a
lease, carry no limit (0081), so neither raises it.

### 3. A BTP socket reads at one fixed figure

A websocket cannot tell a request from a response until it has read the message, and on BTP both
ride the same socket: a peer session this node dials reads the FULFILLs its PREPAREs earn, and a
client session bound as a destination answers with them. A FULFILL carries the app's whole response,
which no `max_bytes` bounds. Reading every message at the request limit would cut off, at 2 MiB, a
response that reads today at tungstenite's 64 MiB.

So every BTP socket — the `GET /ilp/btp` upgrade, client and peer sessions alike, and every session
this node dials — reads a message, and a frame, up to `BTP_MESSAGE_LIMIT`: 64 MiB plus the 64 KiB
framing allowance, the request limit of a node whose route declares the largest `max_bytes` allowed.
A frame gets the whole figure as well as a message, because a BTP sender sends each message as one
frame. A request over BTP that is within this figure but over its route's `max_bytes` is the `F00`
of 0081, as on HTTP; a request to a route with no `max_bytes` is bounded by this figure alone.

Exact parity with HTTP — reading at 64 MiB and refusing, in the frame, a PREPARE over the node's
request limit — was considered. It buys nothing the fixed figure does not: by the time the frame
handler could refuse, the message is already in memory.

### 4. A route may declare up to 64 MiB

`Config::load` refuses a `max_bytes` above 64 MiB by name. It was tungstenite's message default,
so no BTP sender built to that library's defaults is cut off below it, and it is large enough for
the blobs that motivated this record.

### 5. An oversize request is refused at the carriage, not with a packet

A body over the request limit is answered `413` before it is decoded, as before this record; the
figure behind the `413` is now the request limit rather than a library's. A websocket message over
`BTP_MESSAGE_LIMIT` ends the session: the library refuses it while reading, and the session closes with every
in-flight request on it. Neither costs the sender anything, since nothing was decoded, let alone
charged. A packet within the request limit but over its route's `max_bytes` is still the `F00` of 0081.

## Considered options

**One per-route limit that both lowers and raises.** What the issue asked for first. Not possible on
its own terms (decision 1); what survives of it is that the route's `max_bytes` is what raises the
node's limit.

**An explicit node-wide key, with `max_bytes` bounded by it.** Lets a forwarding-only node raise its
limit with no route of its own. Refused under decision 2: a forwarding hop that should carry large
packets declares `max_bytes` on its forwarded route, which also publishes the size.

**Read BTP at the request limit too.** Strict parity with HTTP, and what this record first built.
Refused under decision 3: it bounds responses by a figure derived from requests, and ends a session
on a large response that reads today.

**Publish the request limit in `GET /ilp`.** So a client could find the smallest limit on a path
before paying. Not done: every route that raises it publishes `maxBytes` already, and a route that
does not is bounded by 2 MiB, the figure every node read at before this record.

**Bound how many large bodies are in flight.** The edge buffers a whole body and opens the gift wrap
in memory, so each request in flight holds about twice its size. Not done here: the operator opts
into that exposure by raising `max_bytes`, and a bound is a separate decision if one is needed.

## Consequences

**A node that sets no `max_bytes` above 2 MiB reads `POST /ilp` and `POST /ilp/probe` byte for byte
as before.** Its BTP sessions read at 64 MiB plus 64 KiB rather than tungstenite's 64 MiB message
and 16 MiB frame: no message they read before is refused now. Nothing in this record is a breaking
change for a config that does not use it.

**HTTP and BTP still differ for a request to a route without `max_bytes`,** as they did before this
record: 2 MiB on HTTP on such a node, the BTP figure on BTP. A route that cares about the size says
so with `max_bytes`, which binds both carriages.

**Memory per request scales with the largest route.** A node with one 64 MiB route reads up to
64 MiB on every HTTP request, because the read comes before the route is known, and every BTP
message may be that large on any node. Each request in flight can hold about twice its size.

**Every hop on a path has to allow the size.** A forwarding hop reads at its own request limit, so a
large packet reaches its termination only across hops whose forwarded routes declare a `max_bytes`
at least as large. A path where a hop does not is refused at that hop: with a `413` on HTTP, or the
hop's own `F00` where it declares a smaller `max_bytes`.

**Adding `max_bytes` above 2 MiB to a committed config needs this binary.** A binary predating this
record refuses one by name at load. The image lands before the config, in the usual order.
