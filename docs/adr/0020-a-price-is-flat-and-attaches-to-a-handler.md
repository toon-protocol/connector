# A price is flat, attaches to a handler, and buys an answer

**Status:** Accepted, narrowed by [0040](0040-a-verified-payment-is-stated-to-the-app.md) — a delivery whose covering client claim this connector verified itself now states `X-TOON-Payer` / `X-TOON-Amount` / `X-TOON-Chain`. Everything else stands, and [0044](0044-a-probe-answers-what-a-route-costs-and-what-it-does.md) extends handler granularity from price to description. Amended by [0064](0064-a-deadline-bounds-the-wait-for-an-app-not-the-answer.md) (#1183): “value moves whenever the app answered” now reads “and answered in time” — an app that does not answer within the packet’s own deadline is abandoned and the packet refused `R00`. Every answer that does arrive in time is untouched, a `404` included. **Amended by [0065-price](0065-a-price-is-a-schedule-over-payload-length.md) (#984):** “a price is flat per packet” becomes “a price is a schedule over the packet’s payload length”, of which flat is the zero-slope case. Handler granularity, the app’s obliviousness, cost accumulation and value-on-answer are all untouched.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

A price is flat per packet, as a fee is. Pricing granularity is handler granularity: one handler,
one price, and an app that wants to charge differently exposes more handlers. The app is told
nothing about the payment that bought its work. A price accumulates into a reject's running total
alongside fees. And value moves whenever the app answered — whatever it answered.

## Context

The Rust connector cannot charge for a termination at all. `StaticRoute` is a prefix and a handler
URL and nothing else (`crates/connector-config/src/route.rs:60`), and the word `price` appears in
`crates/` only in a comment saying "unpriced". Pay-to-write is not unenforced here, it is
unrepresentable.

Pricing the work at a termination looks as though it must be content-sensitive. The relay prices
`basePricePerByte × bytes` with per-kind overrides. But the connector must never learn what a Nostr
kind is (ADR 0006), and the app is payment-oblivious, so it cannot price either. The prototype
resolved this by not resolving it: its route carried a flat price of `1000` and never consulted the
relay's pricing engine.

## Decision

> **Amended by [ADR 0065](0065-a-price-is-a-schedule-over-payload-length.md) (issue
> #984).** A price is a **schedule** over the packet's payload length --
> `base + per_kib × ceil(len / 1024)` -- of which the flat price below is the case whose
> slope is zero, and which is still what every route the fleet runs charges. The paragraph
> below was taken against an app whose work is the same at any size; #984 measured a node
> fronting a per-byte upstream losing money on every job above ~100 KB, across a 61×
> break-even span one number cannot express. What did **not** change: the length measured is
> the sealed wrap's, never anything inside it, so a connector still prices without ever
> interpreting what it carries.

**A price is flat per packet**, exactly as a fee is (ADR 0010). It does not vary with the payload.

**Pricing granularity is handler granularity.** One handler, one price. An app that wants to charge
differently for different work exposes a handler for each, and the operator publishes a route per
handler. The distinction lives in the address space, not in the packet — which is how a connector
prices without ever interpreting what it carries. `Config::load` refuses two differently-priced
routes pointing at one handler, since an app provably cannot tell them apart and the cheaper price
would always win.

> **Narrowed by [ADR 0040](0040-a-verified-payment-is-stated-to-the-app.md).** The paragraph
> below stands for everything the connector cannot honestly assert, and for every delivery it did
> not take the payment for — but a delivery whose covering client claim this connector verified
> itself now states `X-TOON-Payer` / `X-TOON-Amount` / `X-TOON-Chain`, sourced from that claim's
> own chain-namespaced channel key and this route's own price. The objection recorded here was to
> the prototype's _sources_ (the previous hop; the destination's second label), and ADR 0040
> reuses neither. "Not even which destination was addressed" is untouched.

**The app is told nothing about the payment that brought the packet to it** — not who paid, not
how much, not on what chain, and not even which destination was addressed. Not the payer or the
chain: ADR 0017 found both wrong by construction, not merely omittable — `X-TOON-Payer` names the
immediate previous hop rather than the payer on any path longer than one hop, and `X-TOON-Chain`
can carry a payer-supplied value that an app trusts as connector-asserted. Not the amount: that is
this decision's own consequence rather than a separate one — an app that wants to charge
differently for different work already gets that by exposing a different handler, so an amount
header would tell it only what its own route's price already says. Not the destination: the ILP
address routing consumed to reach this handler never travels with the delivery, which is distinct
from the HTTP method and target inside the sealed envelope — exactly what the connector makes of
the app (ADR 0018). Whatever arrives at a handler was paid for, at that handler's one price, and
that is the only fact the app gets.

**A price accumulates into a reject's running total**, alongside the fees of the hops that carried
it, so a probe discovers what a path costs end to end (ADR 0011). Today only the forwarding path
accumulates (`connector.rs:719`) and every reject raised at a termination hardcodes zero. The field
is renamed `accumulated_fee` → `accumulated_cost`; it does not ride the OER encoding, so the rename
is internal. `CONTEXT.md` gains **Cost** for the sum — what a caller must send, and the only figure
ever returned.

> **Amended by [ADR 0064](0064-a-deadline-bounds-the-wait-for-an-app-not-the-answer.md) (issue
> #1183).** The paragraph below gains one word: _in time_. A packet's expiry bounds how long a
> termination waits for its app, so an answer that never arrives before the deadline is abandoned
> and the packet refused `R00` — which is not a new kind of app failure but the existing
> "timed out" case, previously unenforced. Nothing about _which_ answer is untouched: a `404`
> still rides home on a FULFILL, and lateness is the only property of an answer that has ever
> changed a packet's outcome.

**Value moves whenever the app answered**, whatever it answered. An HTTP status is envelope content,
never a packet outcome: a 404 rides home inside a response envelope on a FULFILL. Only the _absence_
of an answer rejects — unreachable, timed out, undecodable, no route. `AppOutcome::Declined` and its
mapping to `f99_application_error` (`connector.rs:740`) go.

**An unpaid request to a priced route is answered with its terms, not with service.** A connector
that receives one returns what it costs and what is needed to pay it (ADR 0022), rather than
performing the work. This is what makes it safe for a connector that sells to be reachable at all:
the failure mode of an unpriced connector in front of a payment-oblivious app — an anonymous free
gateway to that app, which is what #492 discovered — stops existing when the unpaid case has a
defined, useful, unpaid answer.

## Considered options

> **Reversed in part by [ADR 0065](0065-a-price-is-a-schedule-over-payload-length.md).**
> Both grounds below were answered rather than overridden. _Asymmetry with the flat fee_
> stands as written and is now deliberate: a fee buys carriage, whose work does not scale with
> a payload, and only the price gains a slope. _Cacheability_ is preserved by publishing the
> **schedule** on the greeting and the self-description (`extra.pricePerKib`), so one free
> read answers every size and no sender probes with a same-size dummy. The unit is per
> **KiB** and not per byte, for a reason this record's own ADR 0010 lineage supplies: at
> 6-decimal USDC the observed slope is ~0.03 base units a byte, which rounds to zero in
> integer base units exactly as the basis-point fee did.

**Price per byte.** Covers what the relay actually does. Rejected on two counts: it is asymmetric
with ADR 0010's deliberately flat fee, and it breaks ADR 0011's cacheability — a probe would report
only the cost of a packet its own size, so a sender would have to probe with a same-size dummy
before every write.

**The app quotes each request.** Full fidelity to app-specific pricing. Rejected: it makes the app
payment-aware, and adds a round trip to every packet.

**Reject when the app returns an error.** Intuitive — don't charge for a failure. Rejected: it makes
app errors free, so anyone can drive unlimited load through a connector into an app at zero cost by
aiming at paths that error. That is precisely the traffic a price exists to charge for.

## Consequences

> **Reversed by [ADR 0065](0065-a-price-is-a-schedule-over-payload-length.md) (issue #984).**
> The sentence below is the exact consequence that had to go: a 100-byte and a 100 KB write to
> one handler now cost the same only where the operator wrote a flat price, which remains the
> default and the whole of the deployed fleet. Anti-spam by size follows: a large packet costs
> more, so the cheap-tier abuse the two-route workaround could not prevent stops existing.

Byte-proportional pricing is gone, and with it anti-spam by size at the connector. A 100-byte and a
100 KB write to the same handler cost the same.

The relay's pricing engine loses its consumer. `basePricePerByte` and `kindOverrides` in
`relay/packages/bls/src/pricing/config.ts` have no successor: the route table _is_ the price list. A
relay that wants kind:30023 to cost more exposes a second write handler and the operator prices that
route. Today the relay has one write path, so it has one price.

A payer can be charged for a 500 from a running-but-broken app. The mitigation is the operator
watching its own error rate, not the protocol.

Combined with ADR 0019, fabricating an error response is exactly as profitable to a dishonest
terminating connector as fabricating a success. That consequence is recorded there.

An app fronted by this connector cannot log, bill or rate-limit by payer from what a handler
receives — nothing on that path names one. Anything that needs that attribution must get it from
somewhere other than the packet path.

> **Reversed by [ADR 0040](0040-a-verified-payment-is-stated-to-the-app.md).** A handler now
> receives the paying channel key on the deliveries this connector took the payment for, and can
> log, bill and rate-limit by it. On the deliveries it did not, the paragraph above still holds
> exactly — which is why an app must treat the attribution as optional rather than assuming it.
