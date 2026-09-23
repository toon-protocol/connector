# The node self-description

**Status:** **Normative for its numbered rules.** The endpoint is **built** (#1080): `GET /ilp`
serves this document and the x402 greeting is a projection of the same source. Two rules are still
**not built** and are marked so rather than written in the present tense — ND-15's unsealed reject
carrying a URL (#1083, [ADR 0054](../adr/0054-an-unsealed-termination-reject-answers-where-to-ask.md))
and the route descriptions [ADR 0044](../adr/0044-a-probe-answers-what-a-route-costs-and-what-it-does.md)
adds.

**Coverage:** none of ND-01 – ND-16 is vectored. This **is** a wire surface, so unlike the
configuration and operator documents these rules **do** enter
[ADR 0045](../adr/0045-a-behavioural-rule-is-normative-prose-until-its-vector-lands.md)'s debt ledger,
and the burn-down order is issue #1084's.

**Consumers:** every client SDK, every controller, every operator configuring a peering by hand.

**Vocabulary:** [`CONTEXT.md`](../../CONTEXT.md). MUST, MUST NOT, SHOULD, MAY per RFC 2119.

**Falsifier:** `crates/connector-runtime/src/connector.rs` matching `fn unsealed_termination_reject\([^)]*,` — the second item "Not built" below (#1083, the unsealed reject's URL, [ADR 0054](../adr/0054-an-unsealed-termination-reject-answers-where-to-ask.md)). The reject builder takes a message and nothing else; the URL has to be handed to it.

---

## Why this document exists

A node used to describe itself in **two** places, with **different field sets**, neither
authoritative and neither a superset of the other: the x402 greeting's `extra` block, and a kind:10032
`IlpPeerInfo` announce.

That is not a tidiness problem. `requiredTransport` was **enforced long before it was advertised** —
the devnet relay pinned a route to BTP, its announce said nothing, `toon-client`'s guard read a key
that was not there, fell through to HTTP, and **every relay publish was refused**. Verified live on
2026-08-14: not one announce in the fleet's corpus carried the key in any form.

One authoritative document is what makes that class of failure structural rather than recurring. The
announce is gone ([ADR 0046](../adr/0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md));
the greeting becomes a projection.

**One document was necessary and was not sufficient, and the same failure came back once more.** The
field this document replaced the announce's with was a **per-node scalar**, derived only where every
route covering the node's own addresses agrees. The devnet relay pins `g.toon.relay` to BTP and
leaves `g.toon.relay.ephemeral` unpinned, so there was no agreement, so the document said nothing —
correctly, by its own rule — while the connector refused every HTTP-carried write to the first
prefix before it would even look at the payment. Observed 2026-09-22 (TOON_Network#111): a directory
publisher reading this document found no pin, dialled HTTP, and a provider that was running fine was
absent from the directory for hours. The answer is **ND-05a**: a pin is enforced per route, so it is
published per route, and the scalar is the summary rather than the statement.

---

## 1. The document

### 1.1 Where it lives

**ND-01** `[connector]` — A connector MUST answer `GET` on its **own client-edge URL** with its
self-description. No ILP packet, no encoder, no protocol knowledge required.
([ADR 0050](../adr/0050-a-connectors-url-resolves-to-its-self-description.md))

**ND-02** `[connector]` — It MUST be free and unauthenticated. This is what
[ADR 0022](../adr/0022-a-connector-answers-it-does-not-announce.md) already means by _answering_: it
decides nothing and reaches nobody who did not ask.

**ND-03** `[connector]` — It MUST NOT accept a `POST`, or any other write, **ever**.

> ND-03 is stated rather than implied because the failure mode is obvious in hindsight and slow to
> arrive: a self-description endpoint grows a write, and purchasable peering is back through a side
> door years after [ADR 0043](../adr/0043-purchasable-peering-is-removed.md) removed it. **A peering
> is created by an operator and by nothing else.** This endpoint publishes what an operator needs to
> configure one out of band; it is never where one is requested.

**ND-04** `[connector]` — There is **no caching contract and no TTL**. The document is generated from
live configuration and read when a client wants it. A TTL existed because a _pushed_ copy needed a
shelf life; a pulled one does not.

### 1.2 What it carries

**ND-05** `[connector]` — Everything in the document MUST be true **of this connector**: a fact it
either proved at startup or was configured with, about itself.

The document carries:

| fact                                                                                                                                                                                                                                                   | why a stranger needs it                                                             |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------- |
| ILP address(es)                                                                                                                                                                                                                                        | what to address                                                                     |
| public HTTP and BTP endpoints, and which carriages are exposed                                                                                                                                                                                         | where to reach it, and how                                                          |
| **edge identity** — the key a packet is sealed to                                                                                                                                                                                                      | without it a packet cannot be sealed, so it cannot be delivered                     |
| per chain: chain id, settlement address, token network and its registry, token address, decimals                                                                                                                                                       | what a buyer needs to **open a channel**                                            |
| route prices — the whole schedule, base and per-KiB slope ([ADR 0065](../adr/0065-a-price-is-a-schedule-over-payload-length.md)) — and their descriptions once [ADR 0044](../adr/0044-a-probe-answers-what-a-route-costs-and-what-it-does.md) is built | what a route costs **at any size**, and what it does                                |
| a route's declared **request** shape, where the operator wrote one ([ADR 0067](../adr/0067-a-route-declares-its-request-shape-and-the-connector-never-reads-it.md))                                                                                    | what to **send** to use the route, for a route whose app expects a specific payload |
| the client transport **each route** requires, where it requires one ([ADR 0072](../adr/0072-a-carriage-pin-is-published-on-the-route-that-enforces-it.md)) — and, as a summary, the one its own addresses require when they agree                      | which carriage to dial **before** sending, rather than by being refused             |
| supported client-edge versions, and which one unversioned `POST /ilp` resolves to                                                                                                                                                                      | [ADR 0003](../adr/0003-clean-room-peer-wire-versioned-client-edge.md), issue #1054  |

**ND-06** `[connector]` — The **edge identity** MUST be published. A route whose terminating identity
is unpublished is unreachable: a sender cannot seal to it, so it can never be delivered to.

**ND-05a** `[connector]` — A route that **requires** a client transport MUST name it on that
route's own entry, as `requiredTransport`, spelled `"http"` or `"btp"` — the same two spellings the
greeting's `extra.requiredTransport` uses. A route that accepts either MUST carry **no such key at
all**: not `"both"`, not `null`, so a node that pins nothing publishes byte-for-byte the document it
published before this field existed.
([ADR 0072](../adr/0072-a-carriage-pin-is-published-on-the-route-that-enforces-it.md), TOON_Network
issue #111)

> **Per route, because per node is not where the refusal is decided.** Both client carriages refuse
> a wrong one from `Connector::client_route(destination).transport_policy` — one longest-prefix
> lookup, per packet. The node-wide field below is a **summary** of the routes covering this node's
> own addresses, and it necessarily says nothing when they disagree. They disagree routinely: the
> devnet relay answers to `g.toon.relay`, pinned to BTP, and `g.toon.relay.ephemeral`, which is not,
> so it published no pin at all while refusing every HTTP-carried write to the first. TOON_Network's
> directory publisher reads this field to decide what to dial, found nothing, fell back to HTTP, and
> a healthy provider went missing from the directory for hours.

**ND-05b** `[client]` — A client resolving which carriage a destination needs MUST read the
**longest route entry whose prefix covers that destination** — the router's own rule — and MAY fall
back to the node-wide field only where no entry covers it. A node-wide answer never overrides a
route's own.

**ND-07** `[connector]` — Per-chain settlement facts MUST be derived from the settlement backend the
connector verified against a chain at startup, and MUST NOT be separately declared. **Two declarations
of one fact is how a mainnet node comes to announce itself as devnet.**

> ND-05a is **not** an exception to ND-07: the operator declares `transport` once, on the route, and
> every surface that mentions it — this document, the greeting, `GET /ilp/routes/price` — is a
> projection of that one declaration, read back through the lookup the connector enforces from.
> There is no second value to disagree with.

**ND-07a** `[connector]` — A route's `request` table is the one exception to ND-07's "derived, never
declared" rule, and deliberately so: there is no backend this connector can ask what an arbitrary
app's payload looks like, so declaration is the only mechanism available at all
([ADR 0067](../adr/0067-a-route-declares-its-request-shape-and-the-connector-never-reads-it.md)). A
connector MUST NOT inspect a key inside it, MUST NOT fetch it from anywhere, and MUST publish it
byte-for-byte as the operator wrote it, converted to JSON. Omitted — not `null` — on a route that
configured none.

### 1.3 What it does not carry

**ND-08** `[connector]` — It MUST NOT describe software **behind** the connector. A connector is a
paid reverse proxy; what runs behind it is the app's business.

> This is why a `relayUrl` field was dropped rather than carried forward. It asserted that a Nostr
> relay for free reads sat behind the node — an _application_ fact, and the last place
> [ADR 0046](../adr/0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md)'s removed
> relay assumption survived. Keeping it would have mixed facts the node **proved** with a claim about
> software it does not run, and mixing those provenances is how `requiredTransport` happened.

> ND-07a's `request` table is not an exception to this rule, even though it names an app fact.
> `relayUrl` **asserted** — this node claimed a relay existed, mixed in among facts it had proved.
> `request` is never asserted by the connector at all: it is an operator's opaque declaration, carried
> unread, and the connector claims nothing about whether the app behind it matches. See
> [ADR 0067](../adr/0067-a-route-declares-its-request-shape-and-the-connector-never-reads-it.md).

**ND-09** `[connector]` — It MUST NOT carry **per-peer** facts: peer identities, per-peering fees, or
caps. Publishing them discloses who this node peers with and how far it trusts each — an
operator-private relationship ([ADR 0006](../adr/0006-the-connector-is-mechanism-not-policy.md),
[ADR 0049](../adr/0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md)).

> **A route's carriage pin is not one of these, and ND-10 does not reach it either.** A cap is a
> per-peer trust decision and a moving number; a pin is a fixed property of a published route, at a
> published price, to a published address. Publishing it discloses nothing a reader of this document
> does not already have — it only stops the reader having to guess. ND-05a therefore states it, and
> the refusal stays as the backstop for a client that did not read (ADR 0072).

**ND-10** `[connector]` — A **cap** is discovered by being refused, not by being published. The `T04`
reject's message states the current cap, which is the whole discovery mechanism.

### 1.4 The greeting is a projection

**ND-11** `[connector]` — The x402 greeting's `extra` block MUST be derived from the same source as
this document. Where the two disagree the **document** is authoritative — the point being that they
cannot.

**ND-12** `[connector]` — The greeting keeps its own job: **terms for one specific priced route**, in
band, to a client that just tried to use it. It is not a node description and MUST NOT be treated as
one.

The greeting therefore carries what a client needs _at that moment_ — `payTo`, `maxTimeoutSeconds`,
the route's price, `sessionLeaseTtlMs` — alongside the projected node facts. Fields that exist only to
serve an in-flight transaction stay there and are not promoted.

**ND-12a** `[connector]` — Where a route's price carries a slope
([ADR 0065](../adr/0065-a-price-is-a-schedule-over-payload-length.md)), both surfaces MUST publish
the **schedule** and not only a figure: this document per priced prefix, and the greeting as
`extra.price` + `extra.pricePerKib` beside its own `amount`. The greeting's `amount` stays what the
greeted request costs — that is the field's job — so the schedule is what makes one read answer
every size. The slope is **omitted** where it is zero, so a flat route's document and greeting are
byte-identical to what they were before schedules existed.

---

## 2. Forwarded routes: whose identity?

**ND-13** `[client]` — A client paying a **forwarded** route MUST seal to the **terminating**
connector's identity, not to the first hop's. A packet sealed to the wrong hop cannot be opened at its
destination, and every hop between is by design unable to help.

**ND-14** `[connector]` — A connector MUST NOT relay another node's identity key as if it were an
answer. A client learns an identity **from the node that owns it**.

> This is the sharpest rule in the document, and the reasoning is not stylistic. If a hop supplies the
> key it will forward to, it can supply **its own**: the client seals to it, that hop opens the payload
> and derives the fulfilment itself ([ADR 0019](../adr/0019-a-terminating-connector-derives-the-fulfilment.md)),
> terminates the packet and pockets the payment. The client receives a **valid-looking fulfilment and
> never learns it was robbed**. Sealing exists so that no hop between sender and destination can open a
> payload; letting a hop name the key hands back exactly what sealing took away.

**ND-15** `[connector]` — A termination that cannot open a packet's wrap MUST answer with an unsealed
reject carrying **where to ask** — the terminating connector's URL.
([ADR 0054](../adr/0054-an-unsealed-termination-reject-answers-where-to-ask.md))

**ND-16** `[client]` — A client MUST NOT trust an identity learned from an unsealed reject. It fetches
the identity from the URL, over TLS, from the node itself. **Ask direct, pay through.**
([ADR 0022](../adr/0022-a-connector-answers-it-does-not-announce.md))

A URL is safe where a key is not: a substituted URL yields an identity that produces packets the real
terminating connector cannot open, so a sender finds out on the **next packet** rather than losing
money silently.

### The flow, end to end

1. Client probes the route. It cannot seal, so the termination answers an unsealed reject.
2. That reject names the terminating connector's **URL**.
3. Client `GET`s that URL — the terminating node's own self-description — and reads its **edge identity**.
4. Client seals to it and pays **through the first hop**, which forwards without ever opening anything.

---

## 3. Consistency

Uses exactly the vocabulary of [`CONTEXT.md`](../../CONTEXT.md) and implements
[ADR 0050](../adr/0050-a-connectors-url-resolves-to-its-self-description.md),
[ADR 0022](../adr/0022-a-connector-answers-it-does-not-announce.md),
[ADR 0046](../adr/0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md),
[ADR 0054](../adr/0054-an-unsealed-termination-reject-answers-where-to-ask.md) and
[ADR 0072](../adr/0072-a-carriage-pin-is-published-on-the-route-that-enforces-it.md).

**Built (#1080):** the endpoint. `GET /ilp` answers this document, free and unauthenticated,
projected from live state on each request; the x402 greeting's `extra` node facts are read off the
same value (ND-11); `[announce]` is `[node]` with its three surviving fields and every other key
refused by name; and the announce itself is gone (#1074).

**Not built:** the unsealed reject's URL (#1083, ND-15) and route descriptions
([ADR 0044](../adr/0044-a-probe-answers-what-a-route-costs-and-what-it-does.md)).

**Issue #981 is closed by construction.** There is no `solana_chain_id` in the tree — not defaulted,
not overridable, not compared against anything. A Solana entry's `chain` is what
`SolanaSettlementBackend::connect` reported after proving the program against the chain, and no
consistency check was added because there is no second source to check against.

**Issue #1026 is not.** ND-06 is built — the terminating connector publishes the key a packet is
sealed to — but that is the _publication_ half, and each node's `GET /ilp/identity` already published
the same key before this landed. The half #1026 actually lacks is the _discovery_: how a client
learns the terminating connector's URL without asking a hop, which ND-14 forbids answering. That is
ND-15/#1083. Until it is built, a forwarded route is reachable only by a client that already knows
the terminating node's URL out of band.
