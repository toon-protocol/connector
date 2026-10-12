# An operator announces a node; the node still does not

**Status:** **Retired by [0046](0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md)** — an announce assumes a relay, and a network of pure connectors has none. Kept for its reasoning about _who_ may announce, which is right about the question it asks and is what stops a sidecar being rebuilt. **One part survives:** issue #807's amendment, under which the packet path reads `[announce]`'s `addresses` and `btp_endpoint` to _answer_ with in the x402 greeting. That is answering, not announcing; those two fields are re-homed by 0046 rather than removed. Its `--via-own-routing` guard went with the verb; the hazard it guarded is now held off by a lock on `state_dir` itself (issue #1318's update, below).

**Scope:** fleet and operations — not connector-internal, not wire law. See the [ADR index](README.md).

A running connector never pushes facts about itself into a network. An **operator** may, by running
`connector announce <relay-discovery-url>` on the node being announced — a one-shot command, from the
box that holds the identity key, paid for through that node's own routing like any other write.

## Context

[ADR 0022](0022-a-connector-answers-it-does-not-announce.md) draws a line that is still the right
line:

> **Announcing** is pushing facts about yourself into a network unprompted — `announcePrice`,
> kind:10032 self-announce. A connector never does this. Deciding to participate in a discovery
> network is the controller's business, and ADR 0006 stands unchanged.

Taken as a rule about the _process_, that sentence forbids a kind:10032 announce outright, and
issue #784 would contradict it. Taken as written — including its own second clause — it does not,
because the clause names who the decision belongs to: **the controller**. What ADR 0022 refuses is a
daemon that decides, on its own schedule, to broadcast. It says nothing about an operator deciding
to, once, deliberately.

That distinction stopped being academic on 2026-08-04, when the store box's TypeScript connector
exited in the cutover and took its `selfAnnounce` block with it. `g.toon.ario` has had no publisher
since, so it is undiscoverable — the last unmet acceptance criterion on #714. The cutover runbook's
stopgap was to stand up a second `packages/announcer` instance on the **apex** box holding the
**store's** identity key, because that is where a free relay is. That works, and it moves a key to
where the convenience is, which is the wrong direction for a key to move.

Three things are needed to make an announce honestly, and only the announced node has all three: the
**identity key** the event is signed with, the **settlement facts** the announce advertises, and a
**channel** with somebody who can carry the packet. A sidecar can be given at most one of them, and
the one it is easiest to give it is the key.

## Decision

**The verb belongs to the operator, and the binary is how they say it.**

- A serving connector announces nothing. There is no timer, no `selfAnnounce` config block, and no
  startup broadcast. ADR 0022's rule about the _process_ is unchanged and unweakened.
  (Amended by [issue #807](https://github.com/toon-protocol/connector/issues/807): the packet path
  now reads two fields of the `[announce]` section — `addresses` and `btp_endpoint` — to _answer_
  with, carrying them in the x402 greeting so a client with a stale or missing genesis seed can
  bootstrap against an edge it can reach. That is a reply to a request that asked, over the
  connection that asked it, never an unprompted push: the rule this bullet states is untouched,
  only the sentence that said no serving code path reads the section at all.)
- `connector announce` is a **subcommand**: it runs, publishes one kind:10032 event, and exits. It
  is an operator action with an operator's intent behind it, in the same category as opening a
  channel.
- It publishes **from the node being announced**. The identity key never leaves the box, and the
  settlement facts announced are unambiguously that node's own, read from the `[settlement.*]` tables
  it verified against a chain at startup rather than polled off whichever edge happened to be asked.
- **The URL means one thing: the node you pay.** The paid PREPARE is POSTed back to it with a
  payment-channel claim in the `ilp-payment-channel-claim` header — this node arriving at that node's
  client edge as an ordinary buyer, which is what "paying like any other client" means. The
  announcing node's routing table is not consulted at all: no `[[routes]]` entry reaching the relay,
  no peering to originate over.
- The announce is **paid for**, at the price that edge quotes. There is no free path and no special
  case.

### Where a client claim's parts come from

Only one of them is configured, and that is the point — everything else has exactly one correct
source, and choosing any other source is a bug that is invisible on a fleet where all nodes point at
one deployment:

| what              | source                                            | why                                                                                                                                                  |
| ----------------- | ------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| signing key       | `[settlement.evm]`                                | the channel's on-chain participant **is** this node's settlement address — the same key ADR 0024's peer claims use. **No second key is introduced.** |
| EIP-712 domain    | the **target's** x402 greeting                    | its gate recovers the signer under the domain **it** resolved for the channel                                                                        |
| nonce, cumulative | the target's `POST /ilp/claim-state` (issue #693) | the receiver is the authority on its own watermark; a guessed one replays (refused) or overpays (silent)                                             |
| channel id        | `[announce] pay_channel`                          | the only fact neither side can derive                                                                                                                |

`pay_channel` is deliberately not a `[[client_channels]]` row: that table is channels this node
_receives_ on, and this is one it _pays_ from. One channel in two roles is the same collision
`Config::load` already refuses between the peer and client books.

### Routing it yourself stays available, behind a flag

`--via-own-routing` originates through the node's own routing table instead, via
`Connector::handle_prepare` — the same call `POST /packets` makes. It is coherent (it pays over an
existing peering rather than a client channel) but it is not the default, because it makes the URL
argument mean two things at once: who you _ask_, and — only if the local routing table happens to
reach them — who you _pay_.

This also widens ADR 0001's "load configuration, construct the runtime, merge routers, serve — and
nothing else", which is a real change and is made deliberately. The binary itself still branches on
nothing: `connector_cli::run` returns a `Command` saying whether a socket is to be held open or the
work is already done.

### Not the operator surface

`POST /packets` already originates a packet outward "exactly as the client edge does for an external
caller", so an announce could have been an operator-surface call. Issue #753 is why it is not:
enabling `[operator]` to expose one endpoint publishes the **whole write surface** — `POST /packets`,
`POST /channels`, `/channels/:id/close` — because the bearer `route_layer` wraps reads only and the
Rust operator paths carry no `/admin` prefix for nginx to deny. A subcommand needs no operator
section, no bearer token, no write keys, and no second HTTP surface. It calls the same function
directly.

## Considered options

**A second `packages/announcer` instance holding the store's key** (the cutover runbook's stopgap).
Ships today and needs no code. Rejected as the answer, though not as a stopgap: it puts a node's
identity key on a different box from the node, permanently, to work around the fact that the sidecar
cannot pay.

**Teaching the sidecar to pay.** It would need a channel, a claim signer and a durable watermark —
i.e. it would need to become a connector. Rejected: that is the thing it is standing next to.

**A `[announce] publish_to` block the serving process acts on, on a timer.** Closest to what the
retired TypeScript connector's `selfAnnounce` did, and it would keep `g.toon.ario` announced without
anyone running anything. Rejected here because it is exactly the daemon-decides shape ADR 0022
refuses, and because it is a strictly larger commitment than the one this issue needs. It remains
the obvious next step if operator-driven announces prove too manual, and it would be a change to
this ADR, made on purpose, rather than a drift.

## Consequences

**An announce needs a funded channel with the node it pays, and nothing else about topology.** No
route, no peering, no `[[routes]]` entry. That is what makes this reachable from a node whose only
peering is accept-only. Under `--via-own-routing` the old requirements return: a route reaching the
relay's connector, and the ability to originate over the peering it names.

**The carriage is negotiated; the BTP URL is supplied.** Issue #701 lets a route require one
transport, and `handle_ilp` checks transport _before_ payment — so a route pinned to `btp` answers a
paid HTTP request with the same x402 terms it answers an unpaid one, however correct the claim. **The
devnet apex pins `g.toon.relay` to `transport = "btp"`**, so this is the live case, not a
hypothetical. The subcommand therefore reads the greeting's `extra.requiredTransport` and _picks_ the
carriage — HTTP for an unrestricted route, BTP for a pinned one — rather than being told which to
use.

What it cannot pick is the BTP endpoint. **A target's greeting need not carry a BTP URL**: before
[issue #807](https://github.com/toon-protocol/connector/issues/807) none did — `extra` keys exactly
`endpoint` (the HTTP one), `ilpAddress`, `price`, `requiredTransport`, `sessionLeaseTtlMs`,
`settlement`, `settlements` — and since #807 only a target that configures its own `[announce]`
carries `extra.btpEndpoint` there. So the BTP endpoint is explicit input (`--btp-url` /
`[announce] publish_btp_url`), and a BTP-only route with neither supplied is refused by name, naming
where an operator finds it: the target's own greeting or kind:10032 announce, both spelled
`btpEndpoint`. Deriving it from the HTTP URL
by swapping scheme and appending a path is exactly the class of guess `relay_url` and `payTo` have
already punished — right on this fleet, wrong for anyone whose deployment does not mirror it.

On BTP the claim rides as a `payment-channel-claim` protocolData entry as **raw JSON**, where the
HTTP carriage base64s the identical bytes into a header; the frame bytes come from `connector-btp`,
the one codec both roles share. No `auth` frame is sent: the client edge trusts nothing from the
handshake, and an `auth` MESSAGE only binds a session-registry entry so the connector can push to it
later — which a one-shot buyer has no use for.

**A second process must not share a serving node's `state_dir`.** A node's outbound peer-claim ledger
is replayed from the journal at startup and held in memory, and the journal has no lock. Two
processes over one `state_dir` both resume at nonce N, both sign N+1 against different cumulative
amounts, and the counterparty refuses one as a replay — after which the serving node's claims never
advance the far side's watermark again and the peering silently stops being paid.

This is why the client path is the default rather than merely an alternative. `--via-own-routing`
therefore refuses when all three of these hold: the config names a `state_dir`, the destination
resolves to a `peer_id` route (so the announce would **forward**, which is the only thing that signs
an outbound peer claim), and something is already listening on this config's client edge. An announce
to a route this node _terminates_ writes no journal entry and is not blocked; neither is `--dry-run`.
A guard that refused more than the hazard would be a guard operators route around.

**The default client path needs no such guard, and the reasoning is worth stating.** It signs a
_client_ claim by hand, not through `ClientPayoutLedger` — which is assembled in `router()`, and
`router()` is never called by this subcommand. Its watermark authority is the receiver, asked over
`POST /ilp/claim-state` rather than remembered. So there is no local mutable money state for a second
process to fork. (`build()` does open the peer journal to replay it, which is a read; nothing on this
path appends.)

The accept-only shape is the reason this matters. The devnet store box's peering has no `endpoint` —
the apex dials in — and an accept-only peering can only be originated over by the process holding the
accepted session, which is the serving process. `--via-own-routing` is therefore permanently
unavailable there. The client path does not care: it needs a funded channel with the apex and nothing
else.

**A node with a KMS-held identity cannot announce.** A Nostr signature is BIP-340 Schnorr over the
event's own id, which needs the scalar itself rather than a `Signer`'s recoverable ECDSA `sign`. Said
by name at the point of use, not discovered as a panic.

**`relay_url` stays optional, and stays operator-supplied.** It is where clients **read** this node
for free, which is neither the URL the announce was published _through_ nor anything derivable from
`[[routes]]` — a relay route's `handler_url` is the relay's private write ingress on a container
network. A node that fronts no relay omits the field. An `http(s)://` value is refused at config
load: that spelling is the write ingress, and announcing it publishes an unauthenticated write door
to the network.

## Update (issue #1318) — a `state_dir` lock replaces the deleted guard (2026-10-12)

The `--via-own-routing` guard under "A second process must not share a serving node's `state_dir`"
above went with the verb ([0046](0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md))
and nothing replaced it for a while. Its successor is a lock, not a refusal: a serving node takes an exclusive OS advisory
lock on a file inside `state_dir` before it opens anything there, the operating system releases it
when the process exits for any reason, and a second process over the same directory exits at startup
naming it. It is scoped to the directory, not to a subcommand. The hazard also has three directions,
not one: outbound (the nonce fork described above), inbound (a nonce one process accepted is accepted
again by the other, which is free service) and settlement (both send transactions from one settlement
key and collide on its nonce).
