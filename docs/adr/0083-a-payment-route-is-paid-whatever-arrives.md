# A payment route is paid whatever arrives

**Status:** Accepted (owner decision, 2026-10-26, #1543), **built** (#1543): `payment` is a `[[routes]]` key in `connector-config`, refused by name with a `peer_id` (`PaymentRouteForwards`); the peer price gate in `connector-peer-btp/src/price_gate.rs` judges coverage, `connector-runtime/src/attribution.rs` states the headers, and `GET /ilp` marks the route. **Amends** [0029](0029-a-peer-wire-arrival-to-a-priced-termination-must-cover-its-price.md) (at a payment route, a peer arrival's claim must cover the packet's `amount`, not just the charge), [0040](0040-a-verified-payment-is-stated-to-the-app.md) (a peer arrival at a payment route is a second case where something was verified, so the headers are stated) and [0050](0050-a-connectors-url-resolves-to-its-self-description.md) (the self-description marks a payment route).

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

**Falsifier:** none — the record no longer claims anything absent: every decision in it is built (#1543), so there is no unbuilt half left to check the tree against.

**A `[[routes]]` entry may set `payment = true`. A payment route still ends at an app, the
operator's own, but its job is to be paid: the payer chooses the amount, at or above the route's
charge, and the whole amount is the operator's. When a packet arrives from a peer, its covering
claim must advance by the packet's own `amount`, or the packet is refused `F03`. The app is told
that amount in `X-TOON-Amount`, and the self-description lists the route as a payment route.**

## Context

A connector delivers value only to an app, and every route that ends at a node sells work: it has a
price, and the price is what the app is told it earned (ADR 0020, ADR 0040). TOON has no route
whose job is "pay this person". Tips on TOON (toon-protocol/relay#311) need one. The flow there is
payer node → hub → recipient node, over peerings, with the hub keeping its peering fee (ADR 0061).
The sealed payload is a tip receipt the payer signed. The recipient's node publishes a signed
acknowledgement of what arrived, and readers count only acknowledgements.

Most of that works already:

- A route that ends at the node with `price = 0` accepts any amount from a peer. ADR 0029's `F03`
  check is a floor with no upper bound, and anything paid above the charge is kept.
- The fulfilment is derived from the sealed secret whatever the app answers (ADR 0019).
- The hub forwards as an ordinary peer, and the claim travels with the packet (ADR 0042).

Three things are missing.

1. **The recipient isn't told what arrived.** ADR 0040 states its headers only when a client claim
   admitted the packet and the price is above zero. A peer arrival at a `price = 0` route matches
   neither condition, so the app is told nothing.
2. **What arrived isn't checked against the claim.** ADR 0029 measures a peer arrival's claim
   against the charge, not the packet's `amount`. A hub could declare `amount = 1000`, back it with
   a claim that advances only by the charge, and be admitted. Telling the app that `amount` would be
   telling it a figure the sender declared, which ADR 0040 refused to do.
3. **A payer can't tell a payment route from an app route that happens to be free.** The payer
   reads the recipient's connector at `GET /ilp` and must be able to refuse an address that isn't a
   payment route.

## Decision

### 1. `payment = true` marks the route, and needs a `handler_url`

`payment` is an optional boolean on a `[[routes]]` entry, false when absent. It is valid only on a
route that sets `handler_url`. It is refused by name on a route that sets `peer_id`, because a
forward is never the end of a payment. A payment route keeps every other key a route that ends at
the node has: `price`, `max_bytes`, `request`, and the transport pin.

The app stays. The recipient's node already has to run something that reads the payload and signs
the acknowledgement, and that is an app. Delivering to it uses the one way a packet already ends at a
node, so there is no second way to design.

### 2. The minimum is the charge

A payment route's minimum is its **charge**, the route's price evaluated at the packet's payload
length (ADR 0065-price), and `price = 0` means any amount. There is no separate minimum key, so two
figures on one route can never disagree.

### 3. A peer arrival's claim must cover the packet's own amount

When a packet arrives from a peer at a payment route, its covering claim must advance the inbound
channel's watermark by at least the packet's `amount`, measured the way ADR 0042 item 3 measures a
forwarded arrival. Otherwise the packet is refused `F03`, the wrap is never opened, and the app is
never called. This applies whatever the peering's `forwarded_claim_enforcement` says: there is no
observe mode at a payment route.

Because the amount is at or above the charge, this check includes ADR 0029's. On a route without
`payment = true`, ADR 0029 is unchanged.

### 4. The app is told the amount, the channel and the chain

On a delivery that decision 3 admitted, the three ADR 0040 headers are stated:

| Header          | Value at a payment route, reached from a peer                                                            |
| --------------- | -------------------------------------------------------------------------------------------------------- |
| `X-TOON-Amount` | the packet's `amount` that decision 3 checked, decimal, in the base units of the inbound channel's token |
| `X-TOON-Payer`  | the inbound peering channel's key, in ADR 0040's spelling                                                |
| `X-TOON-Chain`  | that channel key's namespace                                                                             |

`X-TOON-Amount` keeps its meaning, "what this connector took for this packet". At a payment route
that is everything the claim covered. The figure is after every upstream hop's fee and any ADR 0071
conversion, because it is what the last hop's voucher actually paid this node.

**`X-TOON-Payer` names the last hop, not the person who paid.** It is the channel this connector
verified, as it always was ("a channel, not a wallet"), and at a payment route reached through a hub
that channel is the hub's. Who paid is whatever the sealed payload says, relay's signed receipt for
a tip, and the connector neither reads nor vouches for it.

No header names the asset. The channel key identifies one channel, which holds one token, and an
app that needs the code and scale looks the channel up. ADR 0040 made the same call about wallet
addresses, keeping lookups off the packet path.

The strip list doesn't change: these are the same three names.

### 5. The self-description lists the route as a payment route

The route's entry in `GET /ilp` gains `"payment": true`. The field is absent, not `false`, on any
other route, so a node with no payment route publishes exactly the document it published before this
record. A payer checks a recipient's advertised address against it and refuses one that isn't
listed.

## What this does not change

- **The client edge.** A client that pays a payment route pays the charge, as at any route that ends
  at the node, and ADR 0040's client-claim case states the charge as it always has. Client payers
  paying a chosen amount, and client recipients online or offline, are out of scope (#1543).
- **The fulfilment.** It is derived as ADR 0019 says, whatever the app answers.
- **Leased routes.** ADR 0028 and 0029 exclude them from the price gate, and a lease can't be a
  payment route.
- **The fee.** A hub keeps its peering's flat fee (ADR 0061). Nothing here takes value off a path.

## Considered options

**A payment route with no app.** The connector would deliver the payload and the amount through a
new surface: a callback, reading the journal, or a stream on the operator surface. Rejected: the
recipient's node runs an app anyway, to sign its acknowledgement, and a second way for a packet to
end at a node would duplicate delivery, the header strip, and the reject path.

**A separate `min_amount` key.** Rejected: ADR 0029 already makes the charge a floor on a peer
arrival. Two figures on one route can disagree, and the charge already has a schedule (ADR
0065-price).

**No marker: every route that ends at the node is payable.** Rejected: the payer has to tell an
address meant for payment from an app route that happens to be free, and nothing else in the
self-description says so.

**A new header for the amount, such as `X-TOON-Received`.** Rejected: it would be a second name for
the same fact, "what this connector took for this packet", and a second entry in the strip list.

**State the packet's `amount` without the stricter claim check.** Rejected: that is the
sender-declared figure ADR 0040 refused to report, and a recipient's signed acknowledgement would be
attesting to a number nobody checked.

## Consequences

- An operator turns a route into a tip address by adding one key. The app behind it reads
  `X-TOON-Amount` and the sealed payload, and publishes its acknowledgement.
- A hub that underfunds a forward toward a payment route is refused there, not believed. The payer
  is refused too, and not charged.
- `vectors/wire-vectors.json` doesn't change: the envelope, gift wrap, fulfilment and claim signing
  are untouched.
- Adding an optional key isn't a breaking deploy. A node that doesn't set it boots and publishes as
  before.
