# A hop charges a flat per-packet fee, and packets declare a minimum delivery

**Status:** Accepted in part. Amended by [0042](0042-a-packet-carries-its-claim.md): a fee is earned when the packet is paid for, not on fulfilment. **The minimum-delivery half is retired by [0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md)** — a claim bounds erosion, not a declared floor — which also moots the #1072 update's peer-versus-client asymmetry below. **Flat-per-packet, the earnings rule and cost discoverability are unchanged**, and are what this record is now for. **Amended by [0061](0061-a-fee-attaches-to-a-peering-not-to-a-route.md)** — a fee attaches to a peering rather than to a route; what a fee is and how it is earned is untouched. **Amended by [0071](0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)** — a hop may cross a denomination at a declared rate: the flat fee, its earnings rule and cost discoverability all stand, and the spread returns as a distinct dealing earning at a denomination boundary, never as a fee shape — a fee is still never proportional.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

Each peering relation has a flat fee charged once per forwarded packet, replacing the
percentage spread. Every packet declares the amount that must reach its destination, and a hop
that cannot meet that figure after taking its fee rejects the packet rather than forwarding a
smaller one.

## Why flat rather than proportional

A hop's real cost to forward a packet is one signature verification, one signature, and some
bandwidth. That cost is constant; it does not vary with the value being carried. The
proportional-risk argument that justifies a spread elsewhere is weak here too, because a
payee's exposure is capped at a single packet by ADR 0004 rather than accumulating with volume.

The percentage model also failed quietly at the scale this network actually operates. Fees are
computed in basis points with integer arithmetic, so at the default 0.1% the fee is
`amount / 1000` and every packet below 1000 units is carried free. Route prices are denominated
at exactly that scale — `price: '1000'` is 0.001 USDC — so a meaningful share of real traffic
fell into the rounding gap.

## Why minimum delivery rather than quoting

Without it, each hop silently reduces the amount and the sender learns what arrived only after
the fact. There is no quoting protocol in the system today and the only reference to one is a
comment advising the sender to "re-quote and retry" against nothing. Interledger removed its
quoting protocol for good reasons, and reintroducing one to solve this would be a large amount
of machinery for a guarantee that a single field provides.

Declaring the minimum inverts the failure: under-delivery becomes an explicit reject that the
sender can act on, rather than a shortfall it discovers at the destination. Every hop can check
it locally, with no knowledge of the rest of the path.

## Consequences

A hop's earnings are the difference between the cumulative it receives from upstream and the
cumulative it sends downstream, which falls out of the claim exchange without separate accounting.
That arithmetic is unchanged by _when_ the exchange happens: this record originally said fees are
earned only on fulfilment, following ADR 0004, and
[ADR 0042](0042-a-packet-carries-its-claim.md) retired that headline — a packet carries its claim,
so a fee is earned when the packet is paid for rather than when it fulfils. The difference-of-two-
cumulatives above is what a hop earns either way.

Choosing a flat fee is what makes cost discoverable in one shot. Because the figure does not
vary with the amount carried, a path's cost is a constant a client can learn once and cache;
see ADR 0011, which replaces the price discovery lost with `announcePrice` and the x402
greeting.

## Update (issue #1072) — minimum delivery is a peer-path field, and a client role MUST ignore it

This record says _"every packet declares the amount that must reach its destination."_ That was
written when the peer path was the whole story, and it generalises a **peer** rule into a universal.
It is narrowed here to what the design actually is, and always was.

### A client-originated packet declares none, deliberately

`connector-client-edge` calls `handle_prepare_with_client_channel(prepare, 0, …)` — a floor of zero —
and says why: _"client-edge-spec v1 carries no minimum-delivery field… a client-originated packet
declares no guarantee yet, so this hop enforces none."_

**A client's guarantee is the price, not a floor.** [0028](0028-a-forwarded-route-is-priced-at-the-client-edge.md)
prices a forwarded route at the client edge and requires it carry no more than it was paid, and the
invariant `price − fee >= next hop price` is what protects a client across the whole path. A client has
already bought a stated route at a stated price.

A **peer** needs minimum delivery for the opposite reason: it is carrying on someone else's behalf,
with no price of its own to lean on, and must be able to bound its own erosion. That asymmetry is the
whole content of the field, and stating it as universal obscured it.

Adding a client-side floor as well would give one guarantee two mechanisms that can disagree — a client
declaring a floor contradicting the price it paid has no good resolution. The pricing invariant is the
one that binds.

### A client role MUST **ignore** the field — not reject it, not apply it

This rule binds every implementation and had no record at all (sweep finding F-27). Its only source
was `connector-peer-btp/src/fields.rs`, `connector-peer-http/src/headers.rs` and a spec that is now
frozen history.

> `role` is taken rather than assumed because the field is a **peer** grant: on a client-role
> interaction it MUST be ignored — not rejected and not applied — **so a client SDK that sets an
> unrecognised entry is not broken by a peer feature.**

The reason is forward compatibility, and **ignoring is the only behaviour that achieves it**.
Rejecting looks like the safer choice and is the wrong one: it turns a client's harmless extra header
into a refused packet, which is precisely the breakage the rule exists to prevent. A second
implementer choosing "reject" as the conservative default would be conforming to this record's old
wording and breaking real clients.

### Ruling note

The map's scope default (#1049) says a **protocol law** record wins and the binary has a bug. That
default is argued against here, and the evidence is that the counter-argument was written down in two
crates and a specification — the signature of a deliberate narrowing nobody recorded, rather than
drift. Reading [0028](0028-a-forwarded-route-is-priced-at-the-client-edge.md)'s pricing invariant makes
it conclusive: the client-side guarantee already exists and is not this field.

**Everything else in this record stands:** flat per-packet fees, and a hop that cannot meet a declared
minimum delivery after its fee rejecting (`R01`) rather than forwarding less.

## Update (issue #1143) — the minimum-delivery half is deleted, in code and on both wires

[0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md) is built. What that record
retires is gone from the binary and from both carriages: the `minimumDelivery` field, its
`toon-minimum-delivery` protocolData entry, its `Toon-Minimum-Delivery` header, its two vectors and
the `R01` reject it produced. `amount_after_fee(amount, fee)` no longer takes a floor.

**Dead in this record**, as of that deletion:

- The title's second clause and the opening paragraph's _"Every packet declares the amount that must
  reach its destination"_. No packet declares one.
- **"Why minimum delivery rather than quoting"** in full. The argument was sound under
  [0004](0004-value-moves-on-fulfilment.md)'s postpay, where a rejecting hop earned nothing;
  [0042](0042-a-packet-carries-its-claim.md) inverted that premise, so the reject the section prizes
  now costs the sender exactly what silent under-delivery would have. 0057 is the long form.
- The **#1072 update** above in full — its peer-versus-client asymmetry, its "a client role MUST
  ignore it" rule and its closing line about `R01`. Neither role declares a floor now, so there is
  no asymmetry left to state and nothing for a client to ignore. Its actual finding survives
  elsewhere and unchanged: **a client's guarantee is the price**
  ([0028](0028-a-forwarded-route-is-priced-at-the-client-edge.md)), which is now every sender's.

**Alive and untouched:** the flat per-packet fee and why it is flat rather than proportional; the
earnings rule (a hop earns the difference between the cumulative it receives and the cumulative it
sends); and cost discoverability in one shot, which [0011](0011-rejects-accumulate-fees-and-probes-discover-cost.md)'s
probe now carries alone rather than as one of two mechanisms.

One residual case kept a code: a packet that does not cover **this hop's own fee** cannot be
forwarded at all, and is refused **`F03`** — the amount is wrong for what this hop charges and the
sender's move is to pay it, which is [0051](0051-a-reject-code-binds-where-a-sender-must-act-differently.md)'s
`F03` row rather than a floor to lower. `R01` is not reused for it.

## Update (issue #1143, corrected) — that residual case is `R01`, not `F03`

The last paragraph above is wrong and is replaced by this one.
[0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md)'s corrected Update establishes
why: `R01` carried both this record's minimum-delivery meaning **and** RFC 0027's own — _"the amount
received by a connector in the path was too little to forward (zero or less)"_ — and only the first
dies with the field.

So a packet that does not cover **this hop's own fee** is refused **`R01`**, naming the fee and the
amount it carried. `F03` does not gain the case: that row is an amount wrong against a _price_ a
sender can pay, and here the fee consumed everything with no price in question. The code is reused
for exactly the situation RFC 0027 defines it for, which is not "reuse" at all.

This changes nothing else in the update above. The floor, its two carriage bindings, its two vectors
and `amount_after_fee`'s third parameter are gone and stay gone.
