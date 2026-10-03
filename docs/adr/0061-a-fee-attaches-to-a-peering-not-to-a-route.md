# A fee attaches to a peering, not to a route

**Status:** Accepted — **built** (#1159). `PeerConfig` carries `fee`; `PeerRouteConfig` does not; a route's `fee` is a refuse-to-start tombstone on any route, terminated or forwarded. Amends [0010](0010-flat-per-packet-fee-and-minimum-delivery.md)'s surviving half and [0028](0028-a-forwarded-route-is-priced-at-the-client-edge.md) — the fee stays flat and per-packet and is earned the same way; only where it is written changed. Lands beside the cap [0049](0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md) requires there.

**Scope:** connector architecture — internal to this codebase. What a fee _is_ and how it is earned is [0010](0010-flat-per-packet-fee-and-minimum-delivery.md)'s and is protocol law; which table holds the number is not. See the [ADR index](README.md).

**A fee is the price of using this hop, so it belongs to the peering.** `fee` moves from
`[[routes]]` to `[[peers]]`, and from `POST /routes/peers` to `POST /peers`. A route keeps `price`.
One number per peer, whichever destination a packet is headed for.

## The unit was wrong

[ADR 0010](0010-flat-per-packet-fee-and-minimum-delivery.md) fixed what a fee is — flat, per packet,
indifferent to amount — and how it is earned: _the difference between the amount received and the
amount forwarded_. [ADR 0028](0028-a-forwarded-route-is-priced-at-the-client-edge.md) fixed that a
forwarded route is priced at the client edge, so `price` buys the whole path and this hop retains
`fee`. Neither record chose a _granularity_; `fee` landed on `PeerRouteConfig` beside `price` because
that is where `price` had to be.

**The two numbers answer different questions.**

- `price` is _what the whole path to this prefix costs a client_. It must vary by prefix, because
  different destinations are different distances away with different numbers of hops behind them.
- `fee` is _what this connector keeps for its own hop_. That hop does the same work whichever prefix
  the packet is addressed to: accept the frame, verify a claim, look up a route, forward, mint a
  covering claim. The work does not know the destination.

So `price` is properly per route and `fee` is properly per peering, and putting them in the same row
made the second look like the first.

## The expressiveness is unused, and the evidence is the whole tree

`fee` is set in exactly three files:

```
local/two-hop/connector-a.toml:      fee = 100
local/mixed-chain/connector-a.toml:  fee = 100
local/mixed-chain/connector-b.toml:  fee = 50
```

All three are local rehearsal topologies, and they are recent — a non-zero fee only became
expressible when [0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md) retired the
declared floor, which is the record that observed _"every `fee` in every `local/` topology is `0` as
a direct result"_ and set out to fix it.

**No devnet box sets a fee at all.** `infra/linode-relay` and `infra/linode-store` set only `price`.
And the two values that do exist differ across **nodes** — connector-a keeps 100, connector-b keeps
50 — not across peers on one node. **Nothing anywhere charges different fees to different peers, and
nothing has ever charged different fees for different prefixes to the same peer.**

That is not proof the granularity is wrong. It is evidence that the finer unit has never been
reached for, while the cost of it is paid on every peer route written.

## What is given up

Per-route fees let an operator take a larger margin on a route they know is valuable. That lever is
removed, and it cannot be recovered by raising `price`: this hop forwards `price − fee`, so raising
the price without raising the fee hands the difference to the **downstream** peer, not to this node.

This is accepted because the lever prices the wrong thing. Charging more to carry toward an expensive
destination is a claim about the _rest of the path_, and the rest of the path already charges for
itself — its fees accumulate on the reject a probe reads
([0011](0011-rejects-accumulate-fees-and-probes-discover-cost.md)). An operator who wants a premium
on a particular path is describing a commercial relationship with the peer who serves it, and that
relationship now has exactly one number: the peering's fee.

## Rejected: one node-wide fee

A single flat fee for all forwarding, held once in the node's own configuration, was considered. It is
simpler still, and it matches the evidence above even better — nothing in the tree would need more.

Rejected on [ADR 0006](0006-the-connector-is-mechanism-not-policy.md). Pricing is policy, and a
node-wide constant makes a policy question unanswerable by mechanism: an operator with a
settlement-free arrangement with one partner and a commercial one with a stranger could not express
both. Per-peering is the coarsest unit that still lets the operator decide, and _"the connector
decides nothing the operator did not write down"_ cuts against collapsing further.

The reverse is also worth stating: a node-wide fee remains expressible under this record by writing
the same number on every peering. The converse — recovering per-peer pricing from a single constant —
is not.

## The decision

1. **`PeerConfig` gains `fee: u64`**, defaulting to zero, alongside `max_packet_amount`. The two are
   the operator's policy about one counterparty and now live together.
2. **`PeerRouteConfig` loses `fee`.** It keeps `prefix`, `peer_id` and `price`.
3. **`POST /peers` carries `fee`; `POST /routes/peers` does not.**
   `UpsertPeerRouteRequest` becomes `{ prefix, peer_id, price }`.
4. **`[[routes]].fee` becomes a tombstone**, parsed in order to be rejected by name
   ([0009](0009-one-typed-config-file-no-environment-layer.md)). `ConfigError::TerminatedRouteHasFee`
   — which today refuses a fee on a route that terminates — is replaced by a rejection of the key on
   **any** route, terminated or forwarded.
5. **How a fee is earned does not change.** It is still realised on the wire as the difference between
   the amount received and the amount forwarded, still flat, still per packet, still accumulated onto
   a reject travelling back.

## The sweep

**Does not survive:**

- **[0010](0010-flat-per-packet-fee-and-minimum-delivery.md)**'s implicit siting of the fee beside a
  route's price. **Its decision is untouched**: flat, per packet, independent of amount, earned as
  the difference between received and forwarded. Its minimum-delivery half was already retired by
  [0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md).
- **[0028](0028-a-forwarded-route-is-priced-at-the-client-edge.md)**'s clause describing `fee` as a
  field of the forwarded route. **Its decision is untouched**: a forwarded route is priced at the
  client edge, `price` buys the whole path, this hop forwards `price − fee`, and the `F03` over-carry
  cap is unaffected.
- **`ConfigError::TerminatedRouteHasFee`**, replaced by the tombstone. The distinction it drew —
  a fee is meaningless on a route that terminates — becomes structural rather than checked, because
  a terminated route has no peering to hold one.

**Survives unchanged:**

- **[0011](0011-rejects-accumulate-fees-and-probes-discover-cost.md)** — a reject still states the
  cost of the path it travelled, and that is still how a sender discovers what to pay. A probe reads
  the same accumulated number; only where each hop looked its own contribution up has moved.
- **[0049](0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md)** — the
  cap was already per peering. This record puts the fee beside it, which is the shape 0049's
  operator-surface falsifier anticipates.
- **[0006](0006-the-connector-is-mechanism-not-policy.md)** — the operator still writes every number
  down, and the connector still invents none.
- **[0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md)** — a claim bounds erosion, and
  a covering claim is minted for the packet's own forwarded value. Unchanged: the fee is still the
  gap between what arrived and what went on.

## Consequences

**A peering is one row of policy.** `fee` and `max_packet_amount` are set once, when the peer is
added ([0058](0058-a-peering-is-established-from-a-url.md)), and every route through that peer
inherits both. Adding a second prefix to an existing peer becomes a decision about reachability
alone.

**A route's row shrinks to what a route is.** `{ prefix, peer_id, price }` — where it goes, who
carries it, what a client pays. Nothing about the carrier's terms.

**Config change on every box that forwards.** `[[routes]].fee` must move to `[[peers]].fee` before
the tag moves — a `config-change-required: true` release
([0055](0055-a-release-is-one-dispatch-and-the-ordering-rides-as-data.md)). Three `local/` topology
files and no devnet file are affected, since no devnet box sets a fee today.

**`CONTEXT.md`'s Peering entry becomes literally true.** It already reads _"a counterparty key, a
carriage to reach it on, a fee, and a cap"_ — a description of the peering that, until this record,
named two fields the peering did not hold.

## Update (#1466): an operator's own packet pays its node no fee

A packet originated on the operator surface (`POST /packets`) crosses the node's own peering at no
fee. The amount forwarded to the first peer, and covered by the voucher, is the packet's own
amount; an amount below the peering's fee is not refused `R01` by the originating node (an
amount-0 packet is forwarded under a challenge, as any packet moving no value is); and a reject
relayed back to the operator does not gain this node's fee in its `accumulated_cost`. The
per-packet cap still applies, and no denomination boundary is crossed, as before.

**Why.** A fee pays for carrying _someone else's_ packet. The operator is paying its own
connector, and charging itself is a transfer from the operator to the operator that only makes the
packet dearer. A client's packet and a peer's packet are charged exactly as before, and so are
their rejects; every hop after the originating node still takes its fee. Nothing about the wire
changes. The fee-earned metric is likewise not recorded for an originated packet.
