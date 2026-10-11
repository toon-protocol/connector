# A forwarded route is priced at the client edge, and carries no more than it was paid

**Status:** Accepted, amended in place by issue #1012 (a forwarded route charges on the forward's FULFILL, never on its REJECT). Extended by [0029](0029-a-peer-wire-arrival-to-a-priced-termination-must-cover-its-price.md), which closes the terminating-side gap this record left open. **Amended by [0061](0061-a-fee-attaches-to-a-peering-not-to-a-route.md)** — `fee` leaves the route row for the peering; `price` stays, and the `F03` over-carry cap is unaffected.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

A `[[routes]]` entry naming a `peer_id` may — and must — carry a `price`. `price` is what this
connector's own client edge charges a client for a packet to that prefix; `fee` remains what this
hop retains for carriage. The client edge greets, gates and journals a forwarded destination on
exactly the path a terminated one uses, and a priced forwarded route never puts more value on the
peer wire than the client paid for it.

## Context

Before this decision, `resolve_routes` refused `price` alongside `peer_id` outright
(`ConfigError::PeerRouteHasPrice`) and `Connector::app_route_price` consulted terminated routes
only. The two facts compose into one hole: **no configuration could charge a client for a packet
that crosses a peering.** A destination resolving to a peer route answered no x402 greeting
(`client-edge-spec.md` §1.4), required no claim, advanced no watermark, and was forwarded for
free — while `forward_via_peer_route` went on to record a real, signed peer claim against this
connector's own channel with the next hop. The connector paid for carriage it had not been paid
for.

That is why the devnet's store leg was terminated at the apex over a private segment (issue #600)
instead of being routed to the store box's own connector: repointing `g.toon.store` at a `peer_id`
would have turned a claim-gated route into a free one _and_ started the apex paying the store box
out of its own channel. Issue #557's free-gateway guard exists for exactly the shape a peer route
had by construction, and could not see it.

The refusal was not arbitrary. ADR 0010 frames `fee` as buying **carriage over one peering
relation**, deliberately not the far app's work, and issue #520's `price` buys **the terminating
app's work**. Reading a peer route's `fee` as the client-facing charge would conflate the two: a
client's cost for a path is the whole path, while a hop's fee is one link of it, and a hop cannot
know the rest of the path from its own configuration.

## Decision

**A forwarded route carries both numbers, and they answer different questions.**

- `price` — what **this connector's client edge charges a client** for a packet to this prefix.
  Flat per packet, exactly as ADR 0020 makes a terminated route's price flat. It is the whole of
  what the client pays this connector; whatever the rest of the path costs is paid out of it, by
  this connector, and is this operator's problem rather than the client's.
- `fee` — unchanged from ADR 0010: what **this hop retains** for carriage, realized on the wire as
  the difference between the amount received and the amount forwarded (`peer-semantics-pre-868.md` §4).

Neither is derived from the other. A connector cannot compute a path price from its own fee — it
does not know what the next hop charges — so the operator writes down what it has agreed to
charge, exactly as it writes down what a terminated route charges.

**`price` is required on a forwarded route**, mirroring `ConfigError::RouteMissingPrice` on a
terminated one. `ConfigError::PeerRouteMissingPrice` replaces `PeerRouteHasPrice`. A forwarded
route is never _silently_ free: `price = 0` is a deliberate free-carriage declaration by an
operator who wrote it, which is the whole of what issue #557 asks of any route.

**The client edge treats the two kinds identically.** One lookup —
`Connector::client_route` — answers price and transport policy for a destination over the
configured routing table, terminated and forwarded together, using the same longest-prefix,
same-priority selection `handle_prepare` itself routes by. The x402 greeting (§1.4), the
`ClientClaimGate`, the watermark journal, `GET /ilp/routes/price` (§1.7) and the BTP carriage's
mirror of all four therefore cannot tell the two apart, and cannot drift from what the router will
actually do with the packet. Any divergence between "priced because terminated" and "priced
because forwarded" is a defect, not a design.

**A priced forwarded route never carries more than it was paid.** A client-edge PREPARE to a
forwarded destination whose route is priced above zero is refused `F03_INVALID_AMOUNT` when its
declared `amount` exceeds that `price`. The intended arithmetic is `amount == price`: this hop
collects `price` from the client, forwards `price - fee` (`peer-semantics-pre-868.md` §4, unchanged,
including its `R01` minimum-delivery rule), and so earns exactly its `fee` — §4's own "a hop's
earnings are the difference between the cumulative it receives from upstream and the cumulative it
sends downstream", stated at the one hop where "upstream" is a client rather than a peer.

Without the check the client chooses that difference: a claim advancing by `price` would buy a
forward of an arbitrary `amount - fee`, and the peer claim this connector then signs is real
money it never collected. The check is on the priced branch only — an unpriced (`price = 0`)
forwarded route is an operator's explicit free-carriage declaration and keeps its pre-existing
behavior, since bounding a free route's amount to zero would make free carriage impossible rather
than safe.

**A probe short-circuits at a priced forwarded route.** `handle_probe` already answers a
destination that terminates here with that route's price as `accumulated_cost` rather than
delivering (issue #548). A destination that _forwards_ from here under a price is answered the same
way, for both of that rule's reasons: the figure is exactly what a real request would be charged
and is known locally, so no traversal is needed to discover it; and free traversal must not become
a way to make this connector sign a peer claim it was not paid for. An unpriced forwarded route
still traverses and accumulates fees, which is ADR 0011's mechanism and is untouched.

**A forwarded route charges on the forward's FULFILL, never on its REJECT** (issue #1012). The
client edge admits the client's covering claim and only then forwards — it has to: whether the
next hop will carry the packet at all is not knowable before trying it, so "refuse before
admitting" (the seam issues #869/#944 and #887 already use for every refusal this connector can
predict from the packet alone) cannot cover a refusal only the peer wire itself produces. When the
next hop terminally rejects — `F06` after a covered retry, `T01` unreachable, or any other genuine
peer-wire refusal `forward_via_peer_route` relays rather than retries — the claim that admitted at
this edge is rolled back: `ClientClaimGate::roll_back` restores the channel's watermark to what it
held immediately before that claim, durably, via its own `InboundClaimRolledBack` journal entry
(a direct overwrite on replay, unlike `InboundClaimAccepted`'s componentwise-max fold, since a
rollback exists specifically to move a watermark down). The client is told the same refusal it
would have been told anyway; only the fact that nothing was charged reaching it is new. A forward
that FULFILLs is unaffected and still advances the watermark by exactly `price`, as it always has.
This is the same "charged for an attempt the connector itself decided not to render" defect issues
#869/#944 closed one hop earlier — here the hop is the hand-off to the next connector rather than
this one's own app, and the claim can only be evaluated after the fact rather than predicted before
it, so the fix is a rollback rather than a pre-admission refusal. The same rule now holds on a peering, and an unrouted destination is refused before admitting too: [0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md), issue #1446 update.

## What this does not change

**The peer-facing direction.** `peer-carriage-spec.md` §3.1 stands verbatim: a connector MUST NOT
answer a **peer-role** PREPARE with the x402 greeting. Everything above is the _client-facing_
direction of the same node. Peer fees remain bilateral configuration, never a negotiation, and a
peer-role arrival is priced by the claim exchange of `peer-semantics-pre-868.md` §3, not by a 402.

**Charging for a peer-wire arrival at the terminating connector** — issue #620's second gap — was
left open here. This decision made the first hop's client leg payable, which is what a
client-facing deployment needs, and left the terminating side to its own change.
[ADR 0029](0029-a-peer-wire-arrival-to-a-priced-termination-must-cover-its-price.md) (issue #752)
closes it: a peer-role PREPARE reaching a priced terminated route is now refused per packet,
before the app is consulted, when its `amount` does not cover that route's price.

**Leased routes (issue #427) stay unpriced at the client edge.** `client_route` reads the
configured table only. A lease is pushed over the bearer-gated operator surface and its API
carries no price field, so folding leases into the client-edge lookup would let an operator-pushed
longer-prefix lease silently zero a configured route's price — the exact free-gateway failure
issue #557 exists to prevent — rather than close a hole. When the lease surface grows a price, it
joins the same lookup.

**`transport` on a forwarded route stays refused.** `ConfigError::PeerRouteHasTransport` remains,
but its stated reason does not survive this change: a forwarded route _is_ now reached over a
client transport, so "restricting it to `http` or `btp` means nothing" is no longer true. It is
refused because the policy is not implemented for it, not because it is meaningless, and a
forwarded route accordingly accepts both transports — the default every route had before issue
#701.

## Consequences

A devnet route may now move from `handler_url` to `peer_id` without becoming free. That is the
single blocker issue #620 named on retiring the TypeScript connector, and the reason this decision
exists.

An operator who wants a forwarded route to break even sets `price` to at least what the rest of
the path charges plus their own `fee`. The connector does not check that — it cannot, without
knowing the far end's price — and ADR 0006 keeps it mechanism rather than policy. What the
connector does enforce is the half it can see: it never forwards more value than it collected.

Every existing configuration with a `peer_id` route fails to load until a `price` is written. No
configuration committed in this repository has one, and a hard failure is what ADR 0009 asks of a
config whose meaning changed.

## Update (issue #1143) — the `R01` minimum-delivery clause is deleted; the `F03` cap is not

[0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md) retires minimum delivery, and
issue #1143 deletes it. The parenthetical _"including its `R01` minimum-delivery rule"_ in the
forwards-`price - fee` clause is dead: a forwarding hop subtracts its fee and forwards the rest,
with no floor to check and no `R01` to raise. It still earns exactly its `fee`, which is the point
that clause was making.

**Untouched:** the `F03` over-carry cap — a priced forwarded route refusing `amount > price`, so a
hop never carries more value than it was paid for — along with everything about probes
short-circuiting at a priced forwarded route and rollback on reject. The over-carry cap bounds this
hop against carrying more than it collected; the retired floor bounded a _downstream_ hop against
eroding what it passed on, and that job now belongs to the claim covering each crossing.

## Update (issue #1143, corrected) — there is still an `R01` to raise, for RFC 0027's reason

The sentence _"with no floor to check and no `R01` to raise"_ above overstates its second half.
There is no floor to check — that stands. But a forwarding hop whose **own fee exceeds the arriving
amount** forwards nothing, and answers RFC 0027's `R01`, _"too little to forward"_
([0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md)'s corrected Update,
[0051](0051-a-reject-code-binds-where-a-sender-must-act-differently.md)'s corrected row). What died
is the code's minimum-delivery meaning, not the code.

The rest of that update is unaffected: the `F03` over-carry cap still bounds `amount > price` at the
top, and `R01` now bounds the same route at the bottom — a hop carries between its fee and its price
or it carries nothing.
