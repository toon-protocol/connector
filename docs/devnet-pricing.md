# Devnet route pricing

**This file is not the price list any more, and re-pinning the numbers in it would not make it
one.** [ADR 0068](adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md)
moved deploy ownership into the node repositories: what a box serves, and at what price, is
committed in **that box's own repository** and guarded by that repository's own bundle test.
Nothing in this repository decides it, and nothing here can notice when it changes.

That is not a small correction to the table this file used to carry. Probed 2026-08-28 (#1250),
that table was wrong in three separate ways at once — a prefix no route answers to, a flat figure
for a route that had taken a slope, and a whole box it did not know existed — and every one of
them had been true when it was written a fortnight earlier. **A copy of a number another
repository owns goes stale without anything failing**, which is the same class of defect
[ADR 0068](adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md)
retired a workflow over: reporting green about a box you no longer have a hand in.

So what this file keeps is what no single node repository owns — the unit every figure is in, the
arithmetic a multi-hop path has to satisfy, **where each box's authority actually lives**, and how
to ask a box what it charges rather than reading it here. Plus the history of the arithmetic that
is cited from elsewhere in this repo, which is history and cannot go stale.

## Where a price is decided

Three tiers, and only the first is committed anywhere.

| What                                                     | Decided in                                                                                  | Guarded by                                     |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------------- | ---------------------------------------------- |
| A box's own **terminating** prefixes and their schedules | that node repository's `deploy/` bundle                                                     | that repository's own bundle test              |
| A **forwarded** leg's price and fee                      | runtime peer-route state on the box, in its state volume — committed nowhere, **by design** | nothing; a runtime row is not a committed fact |
| What any box is charging **right now**                   | the box                                                                                     | the free self-description, below               |

The second tier is the one that surprises people, so it is worth saying plainly: the relay's
`g.toon.relay.store` and `g.toon.relay.gas` legs, and the gas box's route back to `g.toon.relay`,
appear in **no** committed file in any repository. They are established over the operator surface
(`POST /peers`, then `POST /routes/peers` —
[ADR 0058](adr/0058-a-peering-is-established-from-a-url.md),
[ADR 0034](adr/0034-a-runtime-peer-route-table-never-shadows-the-config-file.md)) and live in
`runtime-peers.json` beside the box's state. Each bundle says so in its own comments and explains
why writing them into the config file would take them away from that surface for good. Read them
back with `GET /peers`; a `"source"` of `"runtime"` is that arrangement working.

## The fleet, and whose repository owns each box

Four boxes and no apex — the apex (`toon`) was destroyed 2026-08-14 (#872, toon-meta#313).
`g.toon` remains the namespace root in the wire protocol, and nothing answers at it.

| Box            | Edge                                  | Terminates                               | Authority                                                                                                    |
| -------------- | ------------------------------------- | ---------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| relay          | `proxy.relay.devnet.toonprotocol.dev` | `g.toon.relay`, `g.toon.relay.ephemeral` | [`toon-protocol/relay`](https://github.com/toon-protocol/relay) `deploy/connector.toml`                      |
| store (`ario`) | `proxy.ario.devnet.toonprotocol.dev`  | `g.toon.store`, `g.toon.relay.store`     | [`toon-protocol/store`](https://github.com/toon-protocol/store) `deploy/connector.toml.template`             |
| gas            | `proxy.gas.devnet.toonprotocol.dev`   | `g.toon.gas`, `g.toon.relay.gas`         | [`toon-protocol/gas-station`](https://github.com/toon-protocol/gas-station) `deploy/connector.toml.template` |
| faucet         | `faucet.devnet.toonprotocol.dev`      | — no connector at all                    | `infra/linode-faucet/` in **this** repository                                                                |

`ario` is a **box label and a DNS name**, and since store#109 (2026-08-27) it is nothing else:
that box terminates `g.toon.store`, and a probe for `g.toon.ario` is a `404`. The two are easy to
conflate precisely because the hostname still carries the older name — the box's own
`[node].http_endpoint` is `https://proxy.ario.devnet.toonprotocol.dev/ilp` while its
`[node].addresses` are `g.toon.store` and `g.toon.relay.store`. See "Retired names" below.

The faucet box is the one whose deploy this repository still owns, and
[ADR 0068](adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md)
says why: it has no connector, so it has no route and no price, and `fleet-ops.yml` offers it and
nothing else.

## What is genuinely fleet-wide

### The unit

All prices are in **base units of 6-decimal USDC** (ADR 0010;
`docs/usdc-cross-chain-settlement.md`'s "6 decimals everywhere" is canonical across every chain the
connector settles on, not a TypeScript-only asset config). So `1000` is 0.001 USDC and `1` is
1 µUSDC. Nothing scales by `decimals` on the value path.

### A price is a schedule, and the fleet has taken one

`base + per_kib × ceil(payload_len / 1024)`, flat exactly when the slope is zero
([ADR 0065-price](adr/0065-a-price-is-a-schedule-over-payload-length.md), #984). A flat price is
written as the bare integer it always was, and publishes a greeting byte-identical to the
pre-0065 one.

The store took a slope on 2026-08-27 in store#107 — two lines in its own bundle, no second box and
no second backend, which was the outcome that record predicted. Anything in this repository still
saying "the devnet fleet stays flat by choice" is describing the fleet as it was for one day; see
that record's `## Update (issue #1250)`.

### The path arithmetic, which now has two halves

A hop collects `price`, retains its `fee` and forwards the rest, so a path adds up only while every
hop's `price − fee` is at least the next hop's price
([ADR 0028](adr/0028-a-forwarded-route-is-priced-at-the-client-edge.md)). With a slope that must
hold **at every length**: the bases must clear the fee **and** each hop's slope must be at least the
next hop's, or a large enough packet erodes to a shortfall the small ones never revealed.

The live relay→store leg is the worked example, and it satisfies both halves:

```
relay  g.toon.relay.store   base 1001, per_kib 10   forwards over the peering
store  g.toon.relay.store   base 1000, per_kib 10   terminates

  base:   1001 - fee >= 1000   =>  the relay may keep at most 1
  slope:  10         >= 10     =>  holds at every payload length
```

No code enforces this across a peering, for the reason ADR 0028 gives — a connector cannot know
what the next hop charges. It is checked by the operator who establishes the route, and it is why
the figures above are worth writing down even though this file does not own them.

### `g.toon` answers nothing

It is the namespace root, not an address. Every box terminates its own prefixes directly for its
own clients; there is no hop in front of any of them.

## Ask the box, do not read a table

Both surfaces are free and unauthenticated —
[ADR 0022](adr/0022-a-connector-answers-it-does-not-announce.md) puts configuration
answers on the answering side of the answering/announcing line, and
[ADR 0050](adr/0050-a-connectors-url-resolves-to-its-self-description.md) makes a connector's URL
resolve to its self-description:

```
GET https://<edge>/ilp                                  every prefix, with its whole schedule
GET https://<edge>/ilp/routes/price?destination=<addr>  one prefix, resolved by longest match
```

The first is the one to reach for. It answers with the box's addresses, its settlement identity on
each chain, and `routes[]` carrying `price` and — only where there is a slope — `pricePerKib`.
That is the authority for what a box is charging, at the only moment the question can be answered
truthfully, which is now.

## Probed 2026-08-28T21:55Z

Kept as **evidence that the surfaces above answer**, not as a table to maintain. It is dated
because it is a reading, and a reading is stale the moment an operator edits a bundle.

```
relay  proxy.relay.devnet   g.toon.relay              1
                            g.toon.relay.ephemeral    0
                            g.toon.relay.gas          1001            (runtime, forwarded)
                            g.toon.relay.store        1001 + 10/KiB   (runtime, forwarded)
store  proxy.ario.devnet    g.toon.store              1000 + 10/KiB
                            g.toon.relay.store        1000 + 10/KiB
                            g.toon.ario               404 — no such route
gas    proxy.gas.devnet     g.toon.gas                1000
                            g.toon.relay.gas          1000
                            g.toon.relay              2               (runtime, forwarded)
```

Three things in that reading are worth naming.

**`g.toon.relay` is still 1**, and the reason has not changed — see below. It is the one figure the
retired table got right.

**The relay's two forwarded legs are 1001, not 1000.** The extra unit is the headroom ADR 0028's
arithmetic requires: at 1001 the relay may keep a fee of at most 1 and still deliver the far side
its 1000. The fee itself attaches to the peering rather than to the route
([ADR 0061](adr/0061-a-fee-attaches-to-a-peering-not-to-a-route.md)) and is read from the operator
surface, not from a free probe.

**The gas box went live 2026-08-27**, from `toon-protocol/gas-station`'s own `deploy/` bundle, the
same day both other boxes moved onto theirs. Its `g.toon.relay` route at 2 is its side of the same
peering — a gas station spends real value on a caller's behalf, so it needs a way to pay for the
writes that report the job.

## Why the relay route is 1 and a store write is 1000

**`g.toon.relay` carries buzz huddles**, which is per-audio-frame at 49 fps over BTP
(toon-meta#262). 1 µUSDC is a coherent per-frame price; 0.001 USDC per frame is not. A general-write
price is the wrong frame for that route. This was an owner decision on 2026-08-04, ratifying a value
the live box had already been serving — the repo moved to the box, not the box to the repo, and the
box that holds it is now the relay's own.

**A store write starts at 1000** because it is a one-shot upload from an arbitrary buyer, not a
high-frequency stream: nothing amortises a handshake there. That is also why the relay route pins
`transport = "btp"` (#701) while the store legs keep the default `both`. Since store#107 the store
also charges 10 per started KiB on top, which is the difference between a 1 KB note and a 50 MB
object finally being visible in the price.

## The apex forward (retired, issue #872)

Until issue #872 removed it, the apex sat in front of both boxes and forwarded to them over a paid
peering — this section is the historical record of the arithmetic that governed it, not a
description of anything currently live.

The apex charged its own client `1002` for `g.toon.ario`, kept a `fee` of `2`, and forwarded the
remaining `1000` to the store's own terminating route — ADR 0028's arithmetic (`amount == price` at
each hop) made a short forward an F03 rather than a silent subsidy once #754 made a terminating
connector charge its price on a peer-role arrival, so `1002`/`2` was the only pair that both paid the
store its `1000` and matched that rule.

For `g.toon.relay` the apex charged `1` and kept a `fee` of `0` (owner decision, 2026-08-06): the
relay carries buzz huddles at 49 fps, so any non-zero apex fee would have forced `apex.price` above
`relay.price`, doubling the per-frame client cost for a workload billed 49 times a second. Zero fee
kept that cost flat while the arithmetic still held: `1 - 0 = 1 >= 1`.

The `EXPECTED_APEX_FORWARD_PRICE`/`EXPECTED_APEX_FORWARD_FEE` (and their relay-forward siblings)
constants that guarded this arithmetic in `crates/connector-bin/tests/devnet_configs_load.rs` are
removed by issue #872 along with the peering and the `infra/linode-node/` config that named them —
with no hop, there is no fee and no peer arrival, so the F03 peer-arrival case they guarded against
cannot arise on this fleet any more.

`transport = "btp"` was only ever legal alongside `handler_url` —
`ConfigError::PeerRouteHasTransport` refuses it on a `peer_id` route
(`crates/connector-config/src/error.rs:125`) — so the pin always lived on the relay's own
terminating route, never on the apex's forward. That is unchanged by the apex's removal.

## `announcePrice` 2000

This was the retired TypeScript connector's fixed figure for what the store's self-announce had to
cover: the apex's `g.toon.relay` terminate price plus this box's own forward fee, with headroom. It
was never a route price and was never comparable to any figure above. The TypeScript config that
carried it, `infra/linode-store/connector.yaml`, was deleted by issue #901.

The Rust `connector announce` mechanism that replaced `selfAnnounce`
(`crates/connector-cli/src/announce.rs`) does not configure this figure at all — there is nothing to
repoint the citation at. Each announce run asks the publish target's own x402 greeting for its live
price and pays that; only when it originates through its own routing (`--via-own-routing`) does it
add this box's own `[[routes]]` forwarding fee on top, ADR 0028's arithmetic from the originating
side (`amount_to_pay`). Either way the amount tracks the target's live price instead of needing a
hand-maintained buffer like `2000`.

## Retired names

- **`g.toon.ario`** — retired by store#109 (2026-08-27), which renamed the store box's terminating
  prefix and its `[node].addresses` to `g.toon.store`. It is now a **box label and a DNS name only**:
  the edge is still `proxy.ario.devnet.toonprotocol.dev`, and a price probe for `g.toon.ario` there
  is a `404`. Note the direction of travel — this file's predecessor recorded `g.toon.store` as the
  retired alias and `g.toon.ario` as the survivor, which is what it was between 2026-08-05 and
  store#109. A shipped client compiled against `g.toon.ario`
  (`desktop/src/shared/api/toonTransportConfig.ts`, buzz's `storeDestination`) reaches nothing; the
  name to compile in is `g.toon.store`, or `g.toon.relay.store` for a client that only ever learned
  the relay box.

- **`g.toon.relay.ario`** — retired by #820 alongside the apex-era alias. It was never actually
  reachable: the apex's Rust route table only ever declared `g.toon.relay` and `g.toon.ario`, so
  longest-prefix matching always dropped `g.toon.relay.ario` into the `g.toon.relay` route — which
  the apex TERMINATED (against `relay:3100`) until #820, so every arrival 404'd for free. #820's
  flip of `g.toon.relay` to a peer forward would only have relocated the accident and made it more
  expensive. Its successor is `g.toon.relay.store`, which the store box genuinely terminates
  (store#112) and the relay genuinely forwards.

## The TypeScript fleet

No TypeScript `connector.yaml` is left in this repository at all: the store's copy went with
issue #901, and the apex's went with the rest of `infra/linode-node/` (issue #872). Neither was ever a
second source of pricing truth; the TypeScript retirement itself is tracked in #714.

`infra/linode-relay/` and `infra/linode-store/` still hold a `connector-rust.toml` each, and
**neither is what its box runs** — they are fixtures, boot-tested by
`crates/connector-bin/tests/devnet_configs_load.rs`, and each directory's `README.md` says so
([ADR 0068](adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md),
Decision 6). Their prices, and the `EXPECTED_RELAY_PRICE` / `EXPECTED_STORE_PRICE` constants that
pin them, are properties of those fixtures. They are not a reading of the fleet and must not be
cited as one.

## What this file used to claim

Kept because the failure is more instructive than the correction. Verified 2026-08-14 and
re-probed 2026-08-28 (#1250), the table here was wrong three ways in a fortnight:

| It said                                               | The box said                                               | Because                      |
| ----------------------------------------------------- | ---------------------------------------------------------- | ---------------------------- |
| the store terminates `g.toon.ario`                    | `g.toon.store` and `g.toon.relay.store`; `ario` is a `404` | store#109, store#112         |
| the store leg is flat at `1000`                       | `base 1000, per_kib 10`                                    | store#107                    |
| "the fleet has not taken [ADR 0065-price's] schedule" | it has                                                     | store#107, the same day      |
| the fleet is two boxes                                | three with a connector, plus the faucet                    | the gas box, live 2026-08-27 |

Not one of those was a hand-edit on a box drifting from a committed file — the failure this file
was created to catch (#785). Every one was another repository's reviewed, committed change that
this repository had no way to see. The fix for that is not a fresher copy.
