# 4. Peer with another node

Terminating your own routes earns from callers who know your address. Peering
puts you on paths that start somewhere else — you carry someone's packet one hop
further, and keep a fee for doing it.

A peering is created by an **operator** and by nothing else. It cannot be bought,
learned or announced into existence
([ADR 0043](../adr/0043-purchasable-peering-is-removed.md)). What you give it is
one thing: **the counterparty's self-description URL**.

## Being peerable

Step 1's config boots and serves, but nobody can peer _with_ it: its
self-description has no endpoints and `"peerCarriages": []`, so a counterparty's
`POST /peers` at it answers `502`. Three more keys, none of them shown above,
close that gap:

```toml
# Top level, so it goes above every table — beside step 1's client_edge_addr.
peer_expose = "http"             # "btp", "http", "both", or "neither" (default)

[node]
addresses     = ["g.your.node"]
http_endpoint = "https://your-node.example/ilp"
```

`peer_expose` says which carriage(s) _this_ node opens a peer listener for.
`[node]` publishes where clients reach it — both listeners are served whatever
`peer_expose` says, so publishing either endpoint is always allowed. What
`peer_expose` decides is what you may _omit_: `btp_endpoint` is required only when
`"btp"` or `"both"` is exposed, and `http_endpoint` is required whenever anything
is exposed, because a peer pays you by asking your client edge over HTTP
whichever carriage its packets ride. So an HTTP-only node writes `http_endpoint`
and simply leaves `btp_endpoint` out; a BTP node writes both; and with `"neither"`
(the default) a `[node]` naming only `addresses` is legal and still answers
`GET /ilp`, it is just not dialable.

Published endpoints are **absolute** URLs, and a payer dials them exactly as written,
so each must be reachable from wherever the payers are. A loopback address
(`127.0.0.1`, `localhost`), `0.0.0.0`, or a bare container name like `node-a` works
only for payers on the same host or docker network; the node logs a warning at boot
for each such endpoint, and still starts. For payers elsewhere, publish the public
`https://` (or `wss://`) URL in front of the node.

For a local or pre-TLS trial only, add `peer_allow_plaintext_endpoints = true` at
the top level so `http://`/`ws://` endpoints are accepted too — every deployed
config should stay on `https://`/`wss://`.

## Read the other node first

`GET /ilp` on the counterparty's URL is free, unauthenticated, and the whole of
your homework. Do it by hand before you write anything:

```bash
curl -s https://their-node.example/ilp | jq
```

```json
{
  "ilpAddresses": ["g.their.node"],
  "httpEndpoint": "https://their-node.example/ilp",
  "peerCarriages": ["http"],
  "edgeIdentity": {
    "keyId": "connector-signer",
    "publicKey": "0x04…"
  },
  "batchSettlements": [
    {
      "network": "eip155:84532",
      "asset": "0x0C996d7c934c79a6255254875607Fe69df25C0E1",
      "payTo": "0x…",
      "receiverAuthorizer": "0x…",
      "withdrawDelay": 86400,
      "name": "USDC",
      "version": "2"
    }
  ],
  "voucherSigners": [{ "network": "eip155:84532", "signer": "0x…" }],
  "routes": [{ "prefix": "g.their.node.app", "price": "1000" }],
  "supportedVersions": [1],
  "defaultVersion": 1
}
```

Four fields decide whether a peering is possible at all:

| Field                          | What it settles                                                                                                          |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------ |
| `peerCarriages`                | Empty means not dialable — they have not set `peer_expose`, and your write will answer `502`.                            |
| `httpEndpoint` / `btpEndpoint` | Where you dial, and which carriage: `wss://` is BTP, `https://` is ILP-over-HTTP. BTP wins where both are published.     |
| `batchSettlements[]`           | You need x402 terms on a network **in common**. No overlap is a `502`.                                                   |
| `voucherSigners[]`             | The key their vouchers are signed with. Without one for the shared network the EVM write is a `502`.                     |
| `routes[].price`               | What their terminating route charges — the number your `--amount` has to cover. A decimal **string**, not a JSON number. |

Addresses come back lowercased, so compare them case-insensitively against
anything you derived yourself. Fields a node has nothing to say about are
**omitted entirely** rather than sent empty or null — a node with no `[node]`
table has no `ilpAddresses`, `httpEndpoint` or `routes` key at all — so parse
this document for absent keys, not empty ones.

Note what is _not_ there: no channel id, no peer list, no fee, no cap. On either
chain a peering is **two one-way x402 channels**, one opened by each side
([ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)):
you open and fund yours toward their `payTo`, they open theirs toward yours, and
each node recognises the other's channel by the `voucherSigners` key the other
publishes — so no identifier is ever exchanged. Both nodes need that chain's
`[settlement.<chain>]` table configured. On Solana your node opens its channel
**through their sponsor endpoint**, so they hold the `payee` seat and can always
land your latest voucher, even after you ask to close. On XRPL there is no sponsor:
your node signs and submits its own `PaymentChannelCreate`. There is no shared secret
either — a peer's role is proved per packet by its voucher signature
([ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md)).

## The three writes

Peering is three authenticated writes against **your own** operator surface, in
this order. All three are signed —
[`docs/operators/sign-write.sh`](../operators/sign-write.sh) is the shipped
signer, and [Signing a write](operator-surface.md#signing-a-write) is how it works.

**1. Establish the peering.** This is the write that reads their URL:

```bash
docs/operators/sign-write.sh -k operator-write.key -X POST -p /peers \
  -u https://your-node.example \
  -b '{"id":"their-node","url":"https://their-node.example/ilp","fee":100,"max_packet_amount":5000,"deposit":100000}'
```

The script prints the three headers it computed, then the node's answer:

```json
{
  "id": "their-node",
  "fee": 100,
  "max_packet_amount": 5000,
  "source": "runtime",
  "channel": { "id": "0x…", "status": "created", "chain": "evm", "network": "eip155:8453" }
}
```

Your node fetches that URL, picks the carriage from their endpoint's scheme,
finds the settlement chain you share, and **opens and funds your own outbound
x402 channel toward them** with `deposit` (base units of the shared token) — on
Solana by posting the payer-signed `open` to their sponsor endpoint, through
your `socks_proxy` if they are an onion host, on XRPL by submitting its own
transaction — or finds the one you already have
open, spending nothing. `"channel"`
is that channel, and `"status"` says which branch it took — `"created"` or
`"found"` — so an unintended second channel shows up in your own output rather
than on a block explorer later. `deposit` is needed only for `"created"`. Every
packet you forward to them from then on carries a voucher on this channel. They
do the same with your URL, and their channel toward you is recognised by the
key they publish; until they do, their packets reach you as a client's.

`id` is your own local label for the relation; nothing puts it on the wire. `fee`
is what you keep for carrying one packet over this peering — flat, per packet,
never a share of the amount. `max_packet_amount` is the largest single packet you
will carry for them, which is the most they can cost you at once. Both are counted
in the base units of the channel this peering settles on — the one you pay them
from — so neither figure carries over to a peering holding a different token, and
`5000` is half a cent of 6-decimals USDC and a rounding error of an 18-decimals
one. Neither can come from a document, which is why they are in the request.

A peering is on **one network**, both of its channels. Two nodes that settle on
**more than one network in common** — Base and Arbitrum, say, or Base and Solana
— must say which: add `"network": "eip155:8453"` (CAIP-2). Without it the write
is refused as `AmbiguousNetwork`, naming the candidates, rather than resolved
silently. To carry value between two networks, make two peerings and forward
between them.

**2. Keep your own collateral behind your own vouchers.** The opening deposit
came with the write above; top your channel up when it runs low. `fund` takes an
**increment**, so posting the same write twice deposits twice:

```bash
docs/operators/sign-write.sh -k operator-write.key -X POST -p /channels/0x…/fund \
  -u https://your-node.example -b '{"amount":3000}'
```

It answers the channel, with its `collateral` raised by that amount.

That is also the cure when a channel runs out of headroom and starts refusing
packets `T00`, with one caveat worth knowing before you conclude the fund
failed: the response and the chain both show the larger deposit immediately, but
the router's view of it refreshes on its own schedule, so a packet sent seconds
later can still be refused. Give it a minute or two rather than funding again.

**3. Route through the peering.** A peering and a route are different decisions,
and either can exist without the other:

```bash
docs/operators/sign-write.sh -k operator-write.key -X POST -p /routes/peers \
  -u https://your-node.example \
  -b '{"prefix":"g.their.node","peer_id":"their-node","price":1100}'
```

`price` here is what **your** caller pays you for the whole path. What you retain
is the `fee` on the peering; the rest goes onward. So the arithmetic that has to
hold, and that nothing checks for you, is:

```
your price − your fee  ≥  the next hop's price
      1100 −       100  =  1000  ✓
```

A connector cannot know what the next hop charges, so this is yours to keep true.
One unit short and the far side rejects the packet outright — a refusal, not a
discount:

```
REJECT F03 -- claim rejected: advances value by 950, less than this route's price of 1000
```

`F03` is the code for a claim that arrives and does not cover the price. A caller
carrying **no claim at all** is a different case and never reaches this: on the
client edge it is answered `402` with the route's greeting, quoting the same
price a paid request would be charged.

`GET /routes/peers` reads the table back. `DELETE /peers/:id` is the kill switch:
it takes the carriage away with the row, so it is immediate and needs no restart.
A peering still referenced by a runtime route is refused until the route goes.

> A peering committed to a **config file** is the same two channels, recorded
> differently: `[[peers]]`; a `[[peer_channels]]` row naming the key the peer
> signs its vouchers with (its settlement address on EVM, its settlement key on
> Solana and XRPL), which binds its channel toward you at boot; and — for a peering you
> forward to — a `[[pay_channels]]` row naming the outbound x402 channel you
> opened toward it beforehand with `POST /channels`, and its client edge's URL.
> Config always wins a collision: it refuses the runtime write outright rather
> than shadowing it.
>
> What the far side sends back depends on whether it has bound **your** key. Until
> it has — by its own `POST /peers` naming your URL, or a `[[peer_channels]]` row
> naming your signer — your packets reach its ordinary client edge, which is why
> the refusal quoted above is `F03` (and a packet with no claim a `402`) rather
> than the peer carriage's `F06`
> ([`peer-carriage-spec.md`](../protocol/peer-carriage-spec.md) §3.1).

## Try it locally

[Try it locally: two connectors, one packet, one chain](local-peering-lab.md) is the lab
for this page: two nodes, a disposable chain, and one packet across the peering.

## Before you point this at a real chain

- **`POST /peers` spends gas**, because it may open a channel and wait for
  confirmation. It is safe to retry — the same request against an established
  peering finds the same channel rather than opening a second one.
- **`fee` and `max_packet_amount` are yours to choose.** No document can supply
  them; they are your policy about this counterparty.
- **A `502` is about them, a `400` is about you.** Unreachable, redirecting, or
  describing a node you share no settlement chain with is `502` — go look at the
  URL. A common one: `POST /peers` takes a self-description URL, so an origin
  with no `/ilp` on the end answers `502` naming that fix.
- **Their identity is trust-on-first-use over TLS, pinned by nothing.** Whatever
  that URL serves is who the peering is with, and that choice determines the
  channel address. Vet the URL; it is the whole of the assurance.
- **A hop can take your claim and decline to carry.** Every `PREPARE` you forward
  carries its own covering claim, so a `REJECT` comes back with that claim already
  spent. What the channel buys you is that this can only ever happen to the one
  packet in flight — nobody holds your deposit. So size the risk rather than
  checking it: keep packets small, and let the amount grow with a path's record.
- **A new peering starts at a conservative cap**, and nothing raises it
  automatically. Post the same `id` again with a larger `max_packet_amount` to
  raise it; a cap is discovered by a `T04` reject naming it, never published.
  There is one ceiling you cannot raise: an amount is a 64-bit integer, so one
  packet on an 18-decimals leg tops out around 18.4 tokens whatever you write
  here.
