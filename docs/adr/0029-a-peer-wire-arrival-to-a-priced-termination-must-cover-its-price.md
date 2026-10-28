# A peer-wire arrival to a priced termination must cover its price, per packet, before delivery

**Status:** Accepted in part. The per-packet `F03` price-coverage check **stands and is live**. Every citation of the exposure ceiling and `T04` as separate, still-live machinery is retired by [0033](0033-the-exposure-machinery-is-retired-not-restated.md), and the claim-exchange premise it reasons from — a claim rides the _next_ packet — is superseded by [0042](0042-a-packet-carries-its-claim.md). The decision survives both, because it needs only that a hop cannot increase an amount while forwarding. **Amended by [0083](0083-a-payment-route-is-paid-whatever-arrives.md)** (built, #1543): at a route with `payment = true`, a peer arrival's claim must cover the packet's own `amount`, which includes this record's check.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

> **The exposure ceiling and `T04` this ADR references throughout as already-existing, unaffected
> machinery are retired** by [ADR 0033](0033-the-exposure-machinery-is-retired-not-restated.md)
> (issue #882). This ADR's own decision — the per-packet `F03` price-coverage check — is untouched
> and still stands; only its citations of the ceiling as a _separate, still-live_ mechanism are now
> historical.

A peer-role PREPARE that resolves to one of this connector's own priced `handler_url` routes is
now checked against that route's `price` before the app is ever consulted: `amount >= price`, or
the connector rejects `F03_INVALID_AMOUNT` with `accumulatedCost = 0` and never opens the wrap.

## Context

Issue #620 gave a `peer_id` route a client-facing `price`, closed by ADR 0028 — but ADR 0028's own
"What this does not change" named a second gap and left it open: **a connector whose priced
_terminated_ route is reached over the peer wire still serves it without charging.**
`StaticRoute::price` was consulted in exactly one place, `Connector::deliver_to_app` /
`deliver_opened_envelope`, and only to fill in a REJECT's `accumulatedCost` — never to decide
whether to deliver at all. `ClaimBook::accept_inbound` verifies a peer's claim signature and its
nonce/amount monotonicity, but deliberately never calls `connector_domain::validate_price` (its own
code comment said so); it has no notion of which route a claim is paying for in the first place.
The result: a peer forwarding into a priced termination — with a stale claim, an unadvancing one, or
none at all — got the app's work for free, discoverable only by the fact that nothing ever refused
it.

Issue #752 (dispatched off the back of #714's Shape A decision — one connector per box, each fronting
its own app) made this concrete rather than theoretical: under Shape A the store box's own priced
termination is reached from the apex over the peer wire, and ADR 0028's stated workaround — "the
first hop's price covers the whole path" — collapses the fee split back into Shape B (the apex
collects everything, the store earns nothing) if the store box cannot itself verify it was paid.

Two questions were explicitly left open by the issue for whoever closed it:

1. **Mechanism.** The peer wire's claim exchange is not per-packet (`peer-semantics-pre-868.md` §3.2 — a
   claim rides the _next_ packet, trailing the fulfilment that created the obligation) while a
   price is inherently per-delivery. Checking a price against a running, trailing balance is not the
   same operation as checking it against a single claim the way the client edge's `ClientClaimGate`
   does.
2. **Per-packet refusal, or relation-level throttle?** `peer-semantics-pre-868.md` §5.3's `T04` ceiling
   already throttles a whole peering relation once its unclaimed exposure grows too large. The issue
   asked whether an underpriced arrival should be answered the same way, as a property of the
   relation, rather than of the one packet.

## Decision

**The check is per-packet, and it is answered from the PREPARE already in hand — not from the claim
exchange at all.**

Value on the peer path moved, when this was written, by
[ADR 0004](0004-value-moves-on-fulfilment.md) — superseded twice since, and the reasoning below
survives both: [ADR 0033](0033-the-exposure-machinery-is-retired-not-restated.md) deleted the
exposure accounting this paragraph names, and
[ADR 0042](0042-a-packet-carries-its-claim.md) retired the "moves on fulfilment" headline for a
packet that carries its own claim. What the argument actually needs is only that a hop cannot
increase an amount while forwarding, which is untouched by either. As originally written: a
PREPARE's `amount` is exactly what becomes owed to this connector, as exposure, the moment it
fulfils
(`peer-semantics-pre-868.md` §3.2, `Connector::handle_peer_prepare`'s `record_inbound_delivery`). That
`amount` already carries the answer this ticket needs — a hop cannot increase it forwarding
(`peer-semantics-pre-868.md` §4: outgoing amount is always incoming amount minus fee), so if the amount
arriving at a termination is at least that route's price, whatever exposure this delivery creates is
guaranteed to be at least that route's price too, and the ordinary claim exchange that later covers
that exposure (§3.2–§3.4) is guaranteed to cover the route's price as a consequence — with no new
claim-level machinery, no change to how a claim is signed or verified, and no attempt to tie one
specific claim to one specific packet the way the client edge does.

`Connector::handle_peer_prepare` therefore gates on `Connector::client_route` — the same
longest-prefix, same-priority lookup ADR 0028 already uses to price the client edge — immediately
after the exposure-ceiling check and before calling `Connector::handle_prepare`:

- Only `ClientRouteKind::Terminated` routes are gated. A `Forwarded` (`peer_id`) route reached over
  the peer wire is ordinary multi-hop forwarding, fee-metered exactly as it always was; `price` on
  such a route is a client-edge fact this connector's own peer-role handling has no reason to
  consult.
- `price = 0` never gates. An operator's deliberate free termination (ADR 0020) stays free reached
  from a peer, exactly as it stays free reached from a client.
- A rejected arrival never opens the wrap, never reaches the app, and records no exposure — the app
  did no work, so nothing accumulates (`peer-semantics-pre-868.md` §5.2's existing "no value added" rule,
  which this decision adds a new member to) and the sending peer is not charged for a delivery that
  never happened.

**This answers question 2 directly: per packet, not a relation throttle.** `T04`'s ceiling and this
check answer different questions and are kept separate rather than merged into one code path:

- `T04` (§5.3) is about **how much unclaimed value this connector tolerates being owed by a peer in
  total**, independent of what any individual packet costs. It already existed, already fires first
  in `handle_peer_prepare`, and is unaffected by this change.
- `F03` (this decision, §5.4) is about **whether one specific arrival brought enough value for the
  specific route it reached.** A relation can be nowhere near its ceiling and still send a PREPARE
  that underpays a route's price — the two failures are independent, and conflating them (e.g.
  throttling the whole relation over one underpriced packet, or waiting for the ceiling to catch an
  underpriced pattern statistically) would either overreact to an isolated misconfiguration or fail
  to catch one at all. A per-packet reject is also the shape every other amount-driven refusal on
  this wire already takes (`R01`'s minimum-delivery check, ADR 0028's own `F03` over-carry cap at
  the client edge) — this decision keeps that precedent rather than introducing a second kind of
  amount enforcement.

## What this does not change

**The claim exchange itself (§3.2–§3.4) is untouched.** `ClaimBook::accept_inbound` still verifies
only signature and nonce/amount monotonicity, exactly as before; it still has no notion of which
route a claim is paying for, and none is added. This decision verifies the _packet_, not the claim.

**The exposure ceiling (§5.3, `T04`) is untouched**, checked first, and this decision's own gate
never touches exposure bookkeeping — a rejected arrival never calls `record_inbound_delivery`.

**No upper bound.** Unlike ADR 0028's client-edge cap (a priced _forwarded_ route refuses an
`amount` greater than `price`, because the excess would be real money this connector forwards and
never collected), a priced _termination_ has nowhere to forward the excess — this connector is the
end of the path. A peer that overpays a termination is not refused; the money is simply not returned
either, the same as any other overpayment this design has never tried to detect.

**No x402 greeting.** `peer-carriage-spec.md` §3.1 stands verbatim: a peer-role PREPARE is never
greeted. A peer that sends too little is simply rejected, the same way `R01` refuses a PREPARE this
hop cannot forward at the declared minimum delivery — told the packet failed, not invited to retry
with different terms.

(Amended by
[ADR 0031](0031-a-peer-prepare-arrives-with-its-covering-claim-or-it-is-greeted.md), owner decision
2026-08-07: a peer-role PREPARE carrying **no claim at all** is now greeted, because §3.1's "PREPAREs
never carry claims to gate at PREPARE time" no longer holds. This paragraph is otherwise unchanged
and still governs the case this ADR decides — a peer that carried a claim but too small an `amount`
for a priced termination is rejected `F03`, not greeted.)

**Leased routes are unaffected.** `Connector::client_route` already excludes them (ADR 0028); they
carry no price and this decision adds none.

## Consequences

Under #714's Shape A, the store box's own priced termination is now verified on its own terms when
reached from the apex over the peer wire: an apex misconfigured to forward less than the store's
`price`, or a store-facing peering that stops claiming altogether, is now visible as `F03` rejects
at the store rather than as silent free service. The apex's own workaround — pricing the first hop
to cover the whole path — is no longer the only thing standing between the store box and working for
free.

An operator who terminates the same route behind two doors at two different prices (the exact shape
issue #557's free-gateway guard and the live `g.toon.relay` incident both name) is unaffected by this
decision specifically — it does not reconcile two prices for one handler, only whether one path's own
declared price was met by what it received.

## Update (issue #1143) — the two `R01` citations are deleted; this decision is strengthened

[0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md) retires minimum delivery, and
issue #1143 deletes it. Two sentences here cite it and are dead: `R01`'s minimum-delivery check in
the list of refusals this wire already takes, and the reject-taxonomy case comparing a peer that
sends too little to _"a PREPARE this hop cannot forward at the declared minimum delivery"_. `R01`
has left the reject vocabulary entirely; the surviving members of that list — ADR 0028's `F03`
over-carry cap, and this record's own `F03` — are unaffected.

**The decision itself is strengthened, not disturbed.** "A peer arrival at a priced termination
covers that price" is precisely what 0057 generalises to the forwarded case: every crossing is
covered by a claim, so what a hop passes on is bounded by what it was paid rather than by a figure
it was handed and trusted to check.

## Update (issue #1143, corrected) — `R01` did not leave the vocabulary

_"`R01` has left the reject vocabulary entirely"_ above is wrong; see
[0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md)'s corrected Update and
[0051](0051-a-reject-code-binds-where-a-sender-must-act-differently.md)'s corrected row. Its
minimum-delivery meaning left; RFC 0027's own — a hop whose fee alone exceeds the arriving amount has
_"too little to forward"_ — did not, and a forwarding hop still raises it.

**Both deletions this record made are still right**, because both cited the floor rather than the
code: the minimum-delivery check in the list of refusals this wire takes, and the reject-taxonomy
comparison to _"a PREPARE this hop cannot forward at the declared minimum delivery"_. The list is
simply longer than that update said — ADR 0028's `F03` over-carry cap, this record's own `F03`, and
`R01` in its RFC 0027 sense. This record's decision is untouched either way.
