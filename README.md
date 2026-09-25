# Connector

[![License](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**A paid reverse proxy.** You put it in front of an ordinary HTTP app, you set a
price, and it collects that price from whoever calls — in tokens, per request,
without your app knowing payment exists.

It does that by being an [Interledger](https://interledger.org) connector: value
arrives wrapped in a protocol your app never speaks, and at the last hop this
binary unwraps it, verifies it was paid for, and hands the app a plain HTTP
request. **It terminates payments the way nginx terminates SSL.**

You do not need to know anything about Interledger to run one.

|       | Step                                                | You end up with                                     |
| ----- | --------------------------------------------------- | --------------------------------------------------- |
| **1** | [Run a node](#1-run-a-node)                         | One binary, one config file, answering on a port.   |
| **2** | [Put your app behind it](#2-put-your-app-behind-it) | Your app served through it, unchanged.              |
| **3** | [Get paid](#3-get-paid)                             | A settlement chain, so anyone can pay what you ask. |
| **4** | [Peer with another node](#4-peer-with-another-node) | A path onward, and a packet that crosses it.        |

Then [the operator surface](#the-operator-surface) for inspecting a node and
moving its money, and [operating it](#operating-it) day to day.

---

## 1. Run a node

The connector is one static binary that reads one TOML file. Two ways to get it.

### With Docker

The published image is how this is meant to be deployed. It runs as uid `10001`
and creates `/app/state` owned by that uid, so a fresh named volume inherits the
ownership.

Pin a **release handle** — `rust-<handle>`, a UTC date and that day's ordinal.
Handles are immutable and never moved:

```bash
docker pull ghcr.io/toon-protocol/connector:rust-2026.08.28.1
mkdir -p node/config node/data && cd node
openssl rand -hex 32 > data/signer.key && chmod 600 data/signer.key
```

`rust-sha-<short-sha>` pins an exact commit and is equally immutable. `rust-main`
floats on every green build of `main` — fine for a scratch trial, never for a
deployment. [`deploy/connector-rust/README.md`](deploy/connector-rust/README.md)
has the full tag table.

> [!NOTE]
> **The published image is `linux/amd64` only.** On Apple Silicon (or any other
> arm64 host), pull and run it under emulation:
>
> ```bash
> docker pull --platform linux/amd64 ghcr.io/toon-protocol/connector:rust-2026.08.28.1
> ```
>
> and add `platform: linux/amd64` next to `image:` on the `connector` service in
> `compose.yml` below.

`compose.yml`:

```yaml
services:
  connector:
    image: ghcr.io/toon-protocol/connector:rust-2026.08.28.1
    command: ['/app/config/connector.toml']
    volumes:
      - ./config/connector.toml:/app/config/connector.toml:ro
      - ./data:/app/data:ro
      # A NAMED volume, not a bind mount. A host bind mount arrives root-owned
      # and the connector refuses to start.
      - connector-state:/app/state
    ports:
      # Loopback to begin with. The client edge is the paid surface; put a
      # TLS-terminating reverse proxy in front of it before it faces the world.
      - '127.0.0.1:3000:3000'
    restart: unless-stopped
    healthcheck:
      # Free and unauthenticated. Answering it means the config loaded, every
      # settlement backend connected, and the router is serving. `docker ps`
      # showing "Up" proves none of that.
      test: ['CMD', 'wget', '-qO-', 'http://127.0.0.1:3000/ilp/identity']
      interval: 10s
      timeout: 3s
      retries: 5

  quotes:
    image: your-quotes-app:latest
  search:
    image: your-search-app:latest

volumes:
  connector-state:
```

```bash
docker compose up -d
```

### From source

For changing the connector rather than running it. Rust stable, and clone with
submodules — `packages/contracts` vendors OpenZeppelin and forge-std:

```bash
git clone --recurse-submodules https://github.com/toon-protocol/connector.git
cd connector && cargo build --workspace
./target/debug/connector path/to/connector.toml
```

Paths in the config are then yours, not the image's: `state_dir = "./state"`,
`key_file = "./signer.key"`. [`CONTRIBUTING.md`](CONTRIBUTING.md) has the test
gate and the chain binaries it needs.

### The config

```toml
client_edge_addr = "0.0.0.0:3000"
state_dir        = "/app/state"

[signer]
key_file = "/app/data/signer.key"   # 32 raw bytes, or 64 hex characters

# One route per thing you serve. Longest matching prefix wins, so a more
# specific prefix can sit beneath a broader one and take precedence.
#
# `price` is a whole number of the SMALLEST UNIT of the token you settle in
# (step 3) — the way a card terminal counts in cents, never in dollars. How
# many of those units make one token is the token's `decimals`. USDC has 6,
# so 1,000,000 units are one USDC, and:
#
#     price = 1000        0.001 USDC   a tenth of a cent
#     price = 100000      0.10  USDC   ten cents
#     price = 1000000     1.00  USDC   one dollar
#
# A wallet or a dashboard shows the whole-token figure. This file never does.
[[routes]]
prefix      = "g.example.quotes"
handler_url = "http://quotes:8080/"
price       = 1000                       # 0.001 USDC per request

[[routes]]
prefix      = "g.example.search"
handler_url = "http://search:8080/"
price       = 2500                       # 0.0025 USDC

# Same app, deeper prefix, different price. This wins over g.example.search
# for g.example.search.bulk because it matches more labels.
[[routes]]
prefix      = "g.example.search.bulk"
handler_url = "http://search:8080/bulk/"
price       = 10000                      # 0.01 USDC, a cent

# If your app's own costs go up with the size of what it is handed — storage,
# uploads, anything you pay an upstream by the byte for — price it by size
# instead of picking one number and losing money at one end of the range:
#
#     base     what every request pays, whatever it carries
#     per_kib  added for each started kibibyte of the request's payload
#
# The route's PRICE is this schedule. What one packet actually costs under it
# is that packet's CHARGE: `base + per_kib × ceil(payload_size / 1024)`. Both
# figures are published, so a caller can work out a charge before sending.
# Leave `price` a plain number when one number is right — that is still what
# most routes want.
[[routes]]
prefix      = "g.example.store"
handler_url = "http://store:8080/"
price       = { base = 1000, per_kib = 30 }   # 0.001 USDC + 0.00003 per KiB
```

Then ask the node what it is:

```bash
curl http://localhost:3000/ilp
```

That free, unauthenticated `GET` returns the node's **self-description** — its
addresses, endpoints, identity key and settlement facts. A connector answers; it
never announces. It is also the whole of what another operator needs to peer with
you, once [`[node]` and `peer_expose`](#being-peerable) are set — this minimal
config's own self-description has no endpoints and nothing to dial.

> [!IMPORTANT]
> **One TOML file, read once, immutable for the process lifetime.** There is no
> environment-variable layer — `CONFIG_FILE` and friends do nothing, and the only
> variable read is `RUST_LOG`. An unknown key is a hard load failure and a removed
> key is refused **by name**, so a stale config says so at boot instead of quietly
> doing nothing.
>
> **Every key is a path, never a value.** No inline keys, no mnemonic, nowhere to
> smuggle one through.
>
> **`state_dir` is where this node records which claims it has already been
> paid.** In a container it must be a mounted volume; a watermark that dies with
> the container hands every payer their spent claims back as free service.

## 2. Put your app behind it

A route with a `handler_url` **terminates** there. The connector opens the sealed
payload, makes exactly that HTTP request of your app, and seals the app's
complete response back.

Your app is payment-oblivious. It receives an ordinary HTTP request, holds no key
on your behalf, and supplies nothing toward the packet's fulfilment — the
connector derives that itself. So "the app answered" and "the packet was paid
for" stay separable, and an app that knows nothing about payment cannot leak,
forge or withhold one.

The one thing this connector adds is attribution, on a request it took the
payment for itself: `X-TOON-Payer` (the paying channel), `X-TOON-Amount` (what
that request was charged) and `X-TOON-Chain`. Your app is free to ignore all
three — it is handed them so it can log or rate-limit by payer if it wants to,
not so it can decide anything about the payment. They are absent on a request
this node did not take the payment for, so treat them as optional. Whatever a
caller writes under those names is stripped before your app sees it.

Two things decide what you can charge for:

- **You are paid for an answer, not the answer the caller wanted.** A `404` from
  your app is a real answer: it rides home on a `FULFILL` and costs the same as a
  `200`. Only unreachability or a refused target produces a reject.
- **The trailing slash on `handler_url` is load-bearing**, and a request's target
  is resolved _beneath_ the handler's path — an absolute path, a `..` segment, a
  scheme or an authority is refused before your app is touched.

A terminated route **must** carry a `price`. Write `price = 0` if free is
deliberate, because it is never silently free.

## 3. Get paid

A price makes a route cost something. A **settlement backend** is what lets
anyone actually pay it. There are two chains. A node may carry either table or
both — with both, it accepts claims on both at once.

```toml
# EVM — Base Sepolia. These are live addresses, not placeholders.
[settlement.evm]
rpc_url          = "https://base-sepolia-rpc.publicnode.com"
contract_address = "0x0c41D9D424d6B075A3cEa1068a694f7847a8CCa5"  # the TokenNetworkRegistry, not a TokenNetwork
token_address    = "0x0C996d7c934c79a6255254875607Fe69df25C0E1"  # the token every price on this node is in
decimals         = 6              # units per token: 6 means 1,000,000 = 1.00

[settlement.evm.key]
key_file = "/app/data/settlement.key"

# Solana — public devnet. Note `program_id` where EVM has `contract_address`:
# there is no registry to resolve a channel contract through, so this names the
# payment-channel program itself, and `token_address` is an SPL mint.
[settlement.solana]
rpc_url       = "https://api.devnet.solana.com"
program_id    = "2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip"
token_address = "34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU"
decimals      = 6

[settlement.solana.key]
key_file = "/app/data/settlement-solana.key"
```

Copy those addresses rather than picking your own: a claim resolves against
**one** deployment, so every node that might accept a given claim has to name the
same one.

|                  | EVM                                                                                                                                                                                   | Solana                                                                                                                                                                          |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Chain            | Base Sepolia, chain id `84532`                                                                                                                                                        | public devnet (`solana:devnet`)                                                                                                                                                 |
| RPC              | `https://base-sepolia-rpc.publicnode.com`                                                                                                                                             | `https://api.devnet.solana.com`                                                                                                                                                 |
| Channels live in | [`0x0c41D9D424d6B075A3cEa1068a694f7847a8CCa5`](https://sepolia.basescan.org/address/0x0c41D9D424d6B075A3cEa1068a694f7847a8CCa5) — `TokenNetworkRegistry`                              | [`2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip`](https://explorer.solana.com/address/2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip?cluster=devnet) — the payment-channel program |
| Token            | [`0x0C996d7c934c79a6255254875607Fe69df25C0E1`](https://sepolia.basescan.org/address/0x0C996d7c934c79a6255254875607Fe69df25C0E1) — devnet USDC (Circle FiatToken v2.2, ERC-3009), 6 dp | [`34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU`](https://explorer.solana.com/address/34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU?cluster=devnet) — mock USDC mint, 6 dp        |
| Funding the key  | Base Sepolia ETH for gas; devnet USDC from the [devnet faucet](https://faucet.devnet.toonprotocol.dev)                                                                                | devnet SOL (`solana airdrop 1 <address> -u devnet`); mock USDC from the same faucet                                                                                             |
| Full record      | [`packages/contracts/deployments/base-sepolia.md`](packages/contracts/deployments/base-sepolia.md)                                                                                    | [`packages/solana-program/deployments/devnet-public.md`](packages/solana-program/deployments/devnet-public.md)                                                                  |

There is no Solana _testnet_ deployment: the program is on devnet and on
mainnet-beta, nowhere else. **Mainnet exists on both chains**, deployed by hand
and used by one third-party operator's node, not by this repository's fleet:
the Base mainnet contracts
([`packages/contracts/deployments/base-mainnet.md`](packages/contracts/deployments/base-mainnet.md),
2026-09-01) and the Solana mainnet-beta program
([`packages/solana-program/deployments/mainnet-beta.md`](packages/solana-program/deployments/mainnet-beta.md),
2026-08-14, upgraded in place 2026-08-29). The devnet table above is what the
fleet and the faucet serve; a node on mainnet funds itself. Because a Solana
claim's signed message binds the settlement program, a node pointing
`[settlement.solana]` at a mainnet RPC while naming the devnet program id would
take money for claims it can never redeem: take the program id from the mainnet
record. The fleet's production tier is still a **named, empty tier**
([ADR 0056](docs/adr/0056-production-is-a-named-empty-tier.md)): no fleet
machine, no fleet key, and
[`connector.production.toml`](deploy/connector-rust/connector.production.toml) is
a skeleton in which every value fails to load on purpose. Do not fill it in.

`decimals` is what turns a `price` into money. Every `price` in the config is a
count of the token's smallest unit, and `decimals` says how many of those make
one whole token — so with `decimals = 6`, `price = 1000` is 0.001 of the token,
and a route meant to cost ten cents of USDC is `price = 100000`. The node reads
the token's own decimals at boot and **refuses to start** if the config
disagrees, because a wrong `decimals` is not a rounding error: it misprices
every route by a factor of ten or more.

Every other settlement value is verified against the chain the same way, which is
why there is no `--network` flag: which chain a node is on **is** these values and
nothing else. The EVM backend reads the chain id off the RPC and calls
`getTokenNetwork()` to prove the address really is a registry; the Solana backend
proves `program_id` is executable _and_ behaves like the payment-channel program,
that `token_address` is an SPL mint, and asks the chain its own genesis hash so a
claim declaring the wrong cluster is refused.

**You do not list the channels your payers will use, and you could not** — a
client's channel does not exist until that client opens it on chain, long after
your node booted. The settlement section gives this node its on-chain identity,
and it is where a claim naming a channel you have never heard of is **resolved
from chain** and accepted, which is what makes paying you permissionless rather
than an arrangement.

> [!WARNING]
> **Fund the settlement key _before_ you start the node**, and know that
> **booting a config is not a dry run**. A Solana backend submits a real
> transaction at `connect`; with no gas the connector exits 1 on a chain error
> that reads like a config bug. With a funded key, starting a node "just to see
> whether the TOML parses" spends real money.

You do not need to build the payer —
[`toon-client`](https://github.com/toon-protocol/toon-client) is that — but two
things help when debugging "why is nobody paying me":

- **An ILP outcome is never an HTTP one.** A `FULFILL` and a `REJECT` both come
  back at HTTP **200**.
- **A caller with no claim on a priced route gets `402`**, carrying the route's
  **greeting**: a document quoting the same price a real request would be
  charged.

Turning the claims you collect into money on chain is
[the operator surface](#the-operator-surface)'s job.

---

## 4. Peer with another node

Terminating your own routes earns from callers who know your address. Peering
puts you on paths that start somewhere else — you carry someone's packet one hop
further, and keep a fee for doing it.

A peering is created by an **operator** and by nothing else. It cannot be bought,
learned or announced into existence
([ADR 0043](docs/adr/0043-purchasable-peering-is-removed.md)). What you give it is
one thing: **the counterparty's self-description URL**.

### Being peerable

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

For a local or pre-TLS trial only, add `peer_allow_plaintext_endpoints = true` at
the top level so `http://`/`ws://` endpoints are accepted too — every deployed
config should stay on `https://`/`wss://`.

### Read the other node first

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
  "settlements": [
    {
      "chain": "evm:84532",
      "settlementAddress": "0x…",
      "tokenNetworkRegistry": "0x0c41D9D424d6B075A3cEa1068a694f7847a8CCa5",
      "tokenNetwork": "0x…",
      "tokenAddress": "0x0C996d7c934c79a6255254875607Fe69df25C0E1",
      "decimals": 6
    }
  ],
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
| `settlements[]`                | You need a chain **in common**. No overlap is a `502`.                                                                   |
| `routes[].price`               | What their terminating route charges — the number your `--amount` has to cover. A decimal **string**, not a JSON number. |

Addresses come back lowercased, so compare them case-insensitively against
anything you derived yourself. Fields a node has nothing to say about are
**omitted entirely** rather than sent empty or null — a node with no `[node]`
table has no `ilpAddresses`, `httpEndpoint` or `routes` key at all — so parse
this document for absent keys, not empty ones.

Note what is _not_ there: no channel id, no peer list, no fee, no cap. The channel
is **derived** from the two settlement addresses, so neither side publishes one
and no identifier is ever exchanged
([ADR 0059](docs/adr/0059-a-channel-is-derived-from-its-participants.md)). There is
no shared secret either — a peer's role is proved per packet by its claim
signature ([ADR 0060](docs/adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md)).

### The three writes

Peering is three authenticated writes against **your own** operator surface, in
this order. All three are signed —
[`docs/operators/sign-write.sh`](docs/operators/sign-write.sh) is the shipped
signer, and [Signing a write](#signing-a-write) is how it works.

**1. Establish the peering.** This is the write that reads their URL:

```bash
docs/operators/sign-write.sh -k operator-write.key -X POST -p /peers \
  -u https://your-node.example \
  -b '{"id":"their-node","url":"https://their-node.example/ilp","fee":100,"max_packet_amount":5000}'
```

The script prints the three headers it computed, then the node's answer:

```json
{
  "id": "their-node",
  "fee": 100,
  "max_packet_amount": 5000,
  "source": "runtime",
  "channel": { "id": "0x…", "status": "created", "chain": "evm" }
}
```

Your node fetches that URL, picks the carriage from their endpoint's scheme,
finds the settlement chain you share, derives the channel from the two settlement
addresses, and **opens it on chain if it is absent**. `"status"` says which branch
it took — `"created"` or `"found"` — so an unintended second channel shows up in
your own output rather than on a block explorer later.

`id` is your own local label for the relation; nothing puts it on the wire. `fee`
is what you keep for carrying one packet over this peering — flat, per packet,
never a share of the amount. `max_packet_amount` is the largest single packet you
will carry for them, which is the most they can cost you at once. Both are counted
in the base units of the channel this peering settles on — the one you pay them
from — so neither figure carries over to a peering holding a different token, and
`5000` is half a cent of 6-decimals USDC and a rounding error of an 18-decimals
one. Neither can come from a document, which is why they are in the request.

Two nodes that settle on **more than one chain in common** must say which: add
`"chain": "evm"` or `"chain": "solana"`. Without it the write is refused by name
rather than resolved silently.

**2. Put your own collateral behind your own claims.** Opening a channel does not
fund it. `fund` is a self-deposit — the chain credits strictly by signer, so only
you can back your side:

```bash
docs/operators/sign-write.sh -k operator-write.key -X POST -p /channels/0x…/fund \
  -u https://your-node.example -b '{"amount":3000}'
```

It answers the channel, with `own_deposited` raised by that amount. On EVM this
is an absolute `setTotalDeposit` under the hood; the request itself takes an
**increment** on both chains, so posting the same write twice deposits twice —
3000 then 3000 leaves `own_deposited` at 6000, not 3000.

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

> A peering committed to a **config file** is the same object, recorded
> differently: it needs `[[peers]]`, `[[peer_channels]]` and — for a peering you
> forward to — `[[pay_channels]]`, with the channel id and the counterparty's
> settlement address derived by hand beforehand. `local/two-hop/` is a worked pair
> of those files. Config always wins a collision: it refuses the runtime write
> outright rather than shadowing it.
>
> It also changes what the far side sends back. Only a node whose **own** config
> declares those tables mounts a peer carriage, so a runtime peering is answered
> by the counterparty's ordinary client edge — which is why the refusals quoted
> below are `402` and `F03` rather than the peer carriage's `F06`
> ([`peer-carriage-spec.md`](docs/protocol/peer-carriage-spec.md) §3.1).

### Try it locally: two connectors, one packet, one chain

Everything above, on a disposable chain, in about five minutes. Two nodes — **A**
terminates a priced route, **B** peers with A and forwards to it — and one packet
that crosses the peering and comes back fulfilled.

On the host you need **Docker**, **`bash`**, **`curl`**, **`openssl`** and
**`jq`**, and a clone of this repository (the chain's contracts are deployed from
it). You do **not** need Rust, Foundry or the Solana CLI: every chain command
below runs inside a container.

On arm64 — Apple Silicon included — [step 1's amd64 note](#with-docker) applies
here too, and to more services: pull with `--platform linux/amd64`, add
`platform: linux/amd64` beside `image:` on all three services in the lab's
`compose.yml` below, and pass `--platform linux/amd64` to step 7's `docker run`.

Two paths, used throughout — the lab lives outside the checkout so nothing here
lands in `git status`:

```bash
export REPO=/path/to/connector       # this clone
export LAB=~/peering-lab             # anywhere else
export IMAGE=ghcr.io/toon-protocol/connector:rust-2026.08.28.1
mkdir -p "$LAB"

docker pull $IMAGE     # ~1 min the first time; everything below assumes it is here
```

#### 1. A chain, with the settlement contracts on it

```bash
cd "$REPO"
docker compose --profile evm up -d --wait anvil
```

That starts `anvil` on `127.0.0.1:8545` and deploys the settlement topology into
it. `--wait anvil` is doing two jobs: it blocks until anvil reports **healthy**,
which for this service means the deploy has actually landed rather than that the
process started — and naming the service keeps the `evm` profile's other member,
a devnet `faucet` container irrelevant to this walkthrough, out of the way.

Three addresses, deterministic on every fresh anvil, which is what lets them be
written down here:

| What                                        | Address                                      |
| ------------------------------------------- | -------------------------------------------- |
| `TokenNetworkRegistry` (`contract_address`) | `0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512` |
| Mock USDC, 6 dp (`token_address`)           | `0x5FbDB2315678afecb367f032d93F642f64180aa3` |
| Chain id                                    | `31337`                                      |

Anvil's account 0 — `0xf39F…2266`, private key
`0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80` — is the
deployer, holds 10,000 ETH, and can mint that USDC to anyone. It is a public test
key; it is not a secret and nothing outside a local anvil will accept it.

#### 2. Keys

```bash
cd "$LAB"
mkdir -p node-a/data node-b/data

for n in node-a node-b; do
  openssl rand -hex 32 > $n/data/signer.key
  openssl rand -hex 32 > $n/data/settlement.key
  openssl rand -hex 32 > $n/data/operator-bearer-token
  openssl rand -hex 32 > $n/data/operator-send.key
done

# The image runs as uid 10001 and mounts these read-only, so it has to be able
# to read them. That is why this lab does NOT chmod 600: these are disposable
# keys on a disposable chain. On a real node the key files are owned by uid
# 10001 instead, and stay unreadable to everyone else.
chmod -R a+rX node-a/data node-b/data
```

`operator-send.key` is the **private** half of an operator write key, and it never
goes on a node. What the node holds is its public half, which the binary that will
do the signing derives for you:

```bash
for n in node-a node-b; do
  docker run --rm -v "$PWD/$n/data:/data:ro" $IMAGE \
    send --operator-key /data/operator-send.key --print-keyid > $n/data/operator-write-keys
done
cat node-b/data/operator-write-keys      # 64 hex characters

# `operator-write-keys` was created after the chmod above, with whatever your
# umask gives it, and both configs name it. Under `umask 077` a node started
# without this line refuses to boot on a file you believe you already fixed.
chmod -R a+rX node-a/data node-b/data
```

#### 3. Fund the settlement keys

Only **B** pays: it opens the channel (gas) and deposits the collateral (USDC).
A needs a little ETH so its backend has a funded account, and no USDC at all.

```bash
cd "$REPO"
DEPLOYER=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
USDC=0x5FbDB2315678afecb367f032d93F642f64180aa3
RPC=http://localhost:8545

# every `cast` below runs inside the anvil container -- nothing is installed here
cast() { docker compose --profile evm exec -T anvil cast "$@"; }

# `cast wallet address` is a local computation and takes no --rpc-url
A_ADDR=$(cast wallet address --private-key 0x$(cat "$LAB"/node-a/data/settlement.key) | tr -d '\r')
B_ADDR=$(cast wallet address --private-key 0x$(cat "$LAB"/node-b/data/settlement.key) | tr -d '\r')
echo "A $A_ADDR   B $B_ADDR"

cast send --rpc-url $RPC --private-key $DEPLOYER --value 10ether  "$A_ADDR"
cast send --rpc-url $RPC --private-key $DEPLOYER --value 100ether "$B_ADDR"
cast send --rpc-url $RPC --private-key $DEPLOYER "$USDC" "mint(address,uint256)" "$B_ADDR" 1000000000

# check it landed: 100 ETH, and 1000 USDC at 6 decimals
cast balance --rpc-url $RPC "$B_ADDR"
cast call --rpc-url $RPC "$USDC" "balanceOf(address)(uint256)" "$B_ADDR"
```

Fund **before** you boot. A settlement backend that cannot pay looks exactly like
a config error in the logs.

#### 4. The two configs

`$LAB/node-a/connector.toml` — the payee. It is the only side that needs
`[node]` and `peer_expose`, because it is the side being read:

```toml
client_edge_addr               = "0.0.0.0:3000"
state_dir                      = "/app/state"
peer_expose                    = "http"
peer_allow_plaintext_endpoints = true      # http:// endpoints — local trial only

[node]
addresses     = ["g.lab.a"]
http_endpoint = "http://node-a:3000/ilp"

[signer]
key_file = "/app/data/signer.key"

[[routes]]
prefix      = "g.lab.a.app"
handler_url = "http://stub-app:3100/"
price       = 1000

[operator]
bearer_token_file = "/app/data/operator-bearer-token"
write_keys_file   = "/app/data/operator-write-keys"

[settlement.evm]
rpc_url          = "http://anvil:8545"
contract_address = "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"
token_address    = "0x5FbDB2315678afecb367f032d93F642f64180aa3"
decimals         = 6

[settlement.evm.key]
key_file = "/app/data/settlement.key"
```

`$LAB/node-b/connector.toml` — the payer. Note what is **absent**: no
`[[peers]]`, no channel tables, no `[node]`. The peering has not been written yet,
and when it is, it is written at runtime rather than here:

```toml
client_edge_addr               = "0.0.0.0:3000"
state_dir                      = "/app/state"
peer_allow_plaintext_endpoints = true

[signer]
key_file = "/app/data/signer.key"

[operator]
bearer_token_file = "/app/data/operator-bearer-token"
write_keys_file   = "/app/data/operator-write-keys"

[settlement.evm]
rpc_url          = "http://anvil:8545"
contract_address = "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"
token_address    = "0x5FbDB2315678afecb367f032d93F642f64180aa3"
decimals         = 6

[settlement.evm.key]
key_file = "/app/data/settlement.key"
```

#### 5. Bring the nodes up

`$LAB/compose.yml`. `stub-app` is the connector image's second binary — it
answers `POST /` and holds no secret, which is the point: it contributes nothing
to the packet's fulfilment. Both nodes join the network anvil is already on:

```yaml
services:
  stub-app:
    image: ghcr.io/toon-protocol/connector:rust-2026.08.28.1
    entrypoint: ['/usr/local/bin/stub-app']
    command: ['0.0.0.0:3100']

  node-a:
    image: ghcr.io/toon-protocol/connector:rust-2026.08.28.1
    command: ['/app/config/connector.toml']
    volumes:
      - ./node-a/connector.toml:/app/config/connector.toml:ro
      - ./node-a/data:/app/data:ro
      - a-state:/app/state
    ports: ['127.0.0.1:3010:3000']
    # Same probe as step 1's compose.yml, and for the same reason: "Up" says a
    # container exists, not that a node is serving. The next thing you do is
    # curl it, so the difference is the whole race.
    healthcheck:
      test: ['CMD', 'wget', '-qO-', 'http://127.0.0.1:3000/ilp/identity']
      interval: 2s
      timeout: 3s
      retries: 15

  node-b:
    image: ghcr.io/toon-protocol/connector:rust-2026.08.28.1
    command: ['/app/config/connector.toml']
    volumes:
      - ./node-b/connector.toml:/app/config/connector.toml:ro
      - ./node-b/data:/app/data:ro
      - b-state:/app/state
    ports: ['127.0.0.1:3011:3000']
    depends_on:
      node-a: { condition: service_healthy }
    healthcheck:
      test: ['CMD', 'wget', '-qO-', 'http://127.0.0.1:3000/ilp/identity']
      interval: 2s
      timeout: 3s
      retries: 15

volumes:
  a-state:
  b-state:

networks:
  default:
    name: connector_default # the network `docker compose --profile evm` made
    external: true
```

That network name does not depend on what you called your clone. Compose usually
derives a project name from the directory, but this repository's
`docker-compose.yml` pins `name: connector`, so its network is
`connector_default` whatever the checkout is called — which is what makes it safe
to write down here, and why step 7's `docker run --network connector_default`
works too.

The two `connector.toml` files sit outside `data/`, so step 2's `chmod` never
reached them, and uid 10001 has to read them too:

```bash
chmod a+r "$LAB"/node-a/connector.toml "$LAB"/node-b/connector.toml
cd "$LAB" && docker compose up -d --wait
```

`--wait` returns only once both nodes answer their healthcheck, so when it comes
back they are serving and the `curl` below cannot race them.

If it returns non-zero, or a node exits immediately, `docker compose logs node-a`
says why in one line — a config that does not load, or a chain that disagreed with
it, stops the node deliberately rather than serving something half-configured.

Now read A the way B is about to:

```bash
curl -s http://127.0.0.1:3010/ilp | jq
```

`"peerCarriages": ["http"]`, an `httpEndpoint`, an `edgeIdentity` and one
`settlements` entry on `evm:31337` means A is peerable. B's own document, at
`http://127.0.0.1:3011/ilp`, has `"peerCarriages": []` — B dials, it is not
dialed, and that asymmetry is fine.

#### 6. Peer them

Three signed writes against **B**. `sign-write.sh` lives in this repository:

```bash
SW="$REPO"/docs/operators/sign-write.sh
K="$LAB"/node-b/data/operator-send.key
B=http://127.0.0.1:3011

# 1. Establish. B reads A's self-description and opens the channel on chain.
$SW -k $K -X POST -p /peers -u $B \
  -b '{"id":"a","url":"http://node-a:3000/ilp","fee":100,"max_packet_amount":5000}'
```

```json
{
  "id": "a",
  "fee": 100,
  "max_packet_amount": 5000,
  "source": "runtime",
  "channel": { "id": "0x…", "status": "created", "chain": "evm" }
}
```

The channel id is the one value you carry between commands. Lift it rather than
retyping 66 hex characters — `sign-write.sh` prints its three headers first, so
the response body is the last line:

```bash
CH=$($SW -k $K -X POST -p /peers -u $B \
      -b '{"id":"a","url":"http://node-a:3000/ilp","fee":100,"max_packet_amount":5000}' \
     | tail -1 | jq -r .channel.id)
echo "$CH"
```

Re-running `POST /peers` like this is safe: the peering is already established, so
this second call finds the same channel and answers `"status":"found"`.

```bash

# 2. Fund it — B's own collateral behind B's own claims. Ten packets' worth:
#    every packet spends collateral INCLUDING one that comes back REJECT, so
#    leave yourself room to experiment. Running out reads as
#    "T00 ... has 0 base units of headroom left".
$SW -k $K -X POST -p /channels/$CH/fund -u $B -b '{"amount":10000}'

# 3. Route through it. 1100 in, 100 kept, 1000 forwarded — exactly A's price.
$SW -k $K -X POST -p /routes/peers -u $B \
  -b '{"prefix":"g.lab.a","peer_id":"a","price":1100}'
```

Read the table back, with the bearer token this time — reads need no signature:

```bash
curl -s -H "Authorization: Bearer $(cat "$LAB"/node-b/data/operator-bearer-token)" \
  $B/routes/peers | jq
```

#### 7. Send a packet across it

`connector send` is the binary's second verb. It forms a real packet, seals it,
signs the write and reports the outcome:

```bash
echo '{"hello":"from a paid packet"}' > "$LAB"/node-b/data/payload.json
chmod a+r "$LAB"/node-b/data/payload.json

docker run --rm --network connector_default \
  -v "$LAB/node-b/data:/data:ro" \
  $IMAGE send \
    --operator     http://node-b:3000 \
    --operator-key /data/operator-send.key \
    --to           g.lab.a.app \
    --seal-to      http://node-a:3000/ilp \
    --target       / \
    --amount       1100 \
    --body         /data/payload.json \
    --expect-fulfill
```

Three flags decide whether this works, and each fails quietly if you get it
wrong:

- **`--operator` is B, `--seal-to` is A.** You hand the packet to B, but the
  payload is sealed to the node that **terminates** it
  ([ADR 0018](docs/adr/0018-a-payload-is-sealed-to-the-terminating-connector.md)),
  and B cannot open it. Sealing to B instead comes back as a
  `REJECT F01 -- gift wrap could not be opened: …`, which is the one mistake
  here that announces itself.
- **`--seal-to` takes a self-description URL**, ending in `/ilp`, not an origin.
- **`--amount` is the path's cost**, not the route's price: every hop's fee plus
  the terminating price. 100 + 1000 = 1100. One unit short and the packet comes
  back `REJECT F03 -- claim rejected: advances value by 999, less than this
route's price of 1000`.

`--expect-fulfill` is what makes this a test rather than a report: without it a
`REJECT` is printed and the process still exits 0.

#### 8. Prove it was actually paid for

A fulfilment does **not** prove the peering was paid. The verdict on a peer's claim
travels back beside the packet's own answer and never gates it, so a peering whose
every claim was refused still fulfils — `--expect-fulfill` would stay green over a
peering carrying your traffic for free. The evidence is on the payee, in A's own
claim journal:

```bash
curl -s -H "Authorization: Bearer $(cat "$LAB"/node-a/data/operator-bearer-token)" \
  http://127.0.0.1:3010/claims | jq
```

```json
[
  {
    "peer_id": null,
    "channel_id": "evm:0x…",
    "direction": "inbound",
    "nonce": 1,
    "cumulative_amount": 1000,
    "pending": false,
    "book": "client"
  }
]
```

`cumulative_amount` is 1000, not 1100: B collected 1100 and kept its 100 fee.
Send a second packet and the nonce becomes 2 and the cumulative 2000 — a claim
that merely repeats the same cumulative at a fresh nonce advances nothing and
buys nothing, so **the watermark is the thing to watch**, not the claim count.

#### 9. Tear it down

```bash
cd "$LAB"  && docker compose down -v
cd "$REPO" && docker compose --profile evm down -v
```

Both chains and both state volumes are disposable, and `-v` matters: a claim
journal left behind satisfies the next run's money check without the next run
paying for anything.

### Before you point this at a real chain

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

---

## The operator surface

How you inspect a running node and how you move its money.

It mounts **only** when `[operator]` is configured, and merges onto
`client_edge_addr` — there is no second port and no second listener.

```toml
[operator]
bearer_token_file = "/app/data/operator-bearer-token"
write_keys_file   = "/app/data/operator-write-keys"
```

Each setting is spelled as **exactly one of** a literal or a path:
`bearer_token` / `bearer_token_file`, `write_keys` / `write_keys_file`. The file
forms are the deployed forms, because a fleet's config files are committed to a
public repository and a literal cannot be.

### Read and write are different authorities

The rules are numbered in
[`operator-spec.md`](docs/protocol/operator-spec.md) and follow from
[ADR 0008](docs/adr/0008-operator-surface-splits-read-from-write.md):

- **Reads** take `Authorization: Bearer <token>` and nothing more.
- **Writes** take an RFC 9421 HTTP Message Signature from an ed25519 key on
  `write_keys`, with the body bound by an RFC 9530 `Content-Digest`.
- **A bearer token is never sufficient to move value** (OP-03). Read authority
  must not confer write authority.
- **Every write is attributable and individually revocable** (OP-02). A shared
  secret is neither: it cannot say which operator did a thing, and losing it
  loses everything at once.
- **An accepted write cannot be replayed** (OP-05). Signatures carry `created`
  and `expires`, and an accepted signature is remembered until its own expiry.
- **A surface with neither half authenticated refuses to start** (OP-04).

`write_keys` holds only **public** halves. The private half lives with whoever is
calling and never on the node. `connector send --operator-key <file>
--print-keyid` prints the value that goes in the allowlist, derived by the binary
that will do the signing.

### Reads

| Endpoint             | Answers                                                   |
| -------------------- | --------------------------------------------------------- |
| `GET /peers`         | The peerings this node holds, config and runtime alike.   |
| `GET /routes`        | The full routing table, with each row's source.           |
| `GET /routes/leased` | TTL-bound pushed routes that lapse on their own.          |
| `GET /routes/peers`  | The durable runtime peer-route table.                     |
| `GET /channels`      | Every channel this node knows, with deposits and status.  |
| `GET /claims`        | The claim journal — what you have been paid, and by whom. |
| `GET /identity`      | This node's operator-facing identity.                     |
| `GET /audit-log`     | Every accepted write, with the key that made it.          |
| `GET /metrics`       | Prometheus text.                                          |

`/metrics` is a bearer-gated read like any other, and there is no unauthenticated
metrics path: absent `[operator]`, `/metrics` is not mounted and answers 404
rather than 401. A _public_ status page — one strangers load — therefore needs a
server-side holder for the token, never the token embedded in the page.

There is also no dedicated health route. What a container healthcheck probes
instead is `GET /ilp/identity` on the client edge, which is free and
unauthenticated and answers only once the config loaded, every settlement backend
connected and the router is serving.

The counters are `toon_packets_total`, `toon_packets_rejected_total`,
`toon_fees_earned_total`, `toon_settlement_total`, and `toon_exposure`, which is
always zero and kept only so scrape configs do not break.

### The dashboard

`GET /dashboard` is the operator's own view of all of the above on one page the
node serves (ADR 0066): packet traffic and rejects by code, fees earned, inbound
and outbound claims, peerings and channels, every route with its source, and
the audit log. It needs no token to load, because it holds nothing. Paste the
bearer token in and it reads; paste an operator key in and it can peer, write a
runtime route or lease one, signing each write in your browser exactly as
`connector send` would. The key stays in the tab's memory — never stored, never
sent — and config-file rows are shown with no button, because a price or a fee
still changes by editing the file and restarting. Reach the page the way you
reach `/metrics` on that box: on the fleet, an SSH tunnel to `client_edge_addr`.

### Writes

| Endpoint                               | Does                                                                      |
| -------------------------------------- | ------------------------------------------------------------------------- |
| `POST /packets`                        | Originate a packet from this node.                                        |
| `POST /peers`                          | Establish a peering from a URL. `DELETE /peers/:id` removes it.           |
| `POST /routes/peers`                   | Write a durable runtime route. `DELETE /routes/peers/:prefix` removes it. |
| `POST /routes/leased`                  | Push a TTL-bound route that lapses on its own.                            |
| `POST /channels`                       | Open a payment channel.                                                   |
| `POST /channels/:id/fund`              | **Self-deposit** — put your own collateral behind your own claims.        |
| `POST /channels/:id/redeem`            | Redeem a specific claim on chain.                                         |
| `POST /channels/:id/redeem-latest`     | Redeem the latest claim — **this is how you get paid**.                   |
| `POST /channels/:id/settle`            | Settle the channel.                                                       |
| `POST /channels/:id/close`             | Close it. `cooperative-close` is the agreed variant.                      |
| `POST /channels/:id/cooperative-close` | Close by agreement with the counterparty.                                 |

Channel operations answer **503** when no `[settlement]` backend is configured.

`POST`/`DELETE` on `/peers*` and `/routes/peers*` are the durable runtime table.
Unlike a leased route, they survive a restart — and they are **refused outright**,
never silently accepted as a shadow, when they would collide with a row the
config file already owns.

### Signing a write

The signature covers exactly three components — `@method`, `@path` and
`content-digest` — with `alg="ed25519"` and `keyid` set to the signer's own
ed25519 public key in hex.

[`docs/operators/sign-write.sh`](docs/operators/sign-write.sh) is the shipped
signer: `bash` and `openssl`, no other dependency. `-u <base-url>` makes it send
the signed request and print the response; omit `-u` and it prints only the three
headers, for a caller assembling its own request.

```bash
docs/operators/sign-write.sh -k operator-write.key -X POST -p /peers \
    -b "$BODY" -u https://your-node.example
```

[`docs/operators/signing-a-write.md`](docs/operators/signing-a-write.md) is the
worked example and explains what each computed value is.

`connector send` signs one write of its own — `POST /packets` — and
`--expect-fulfill` makes a non-fulfilled packet a non-zero exit, which is what
turns a rehearsal into a gate.

---

## Operating it

**Logs** are structured JSON on stdout. Every line emitted while handling a
packet carries the same `correlation_id` — the packet's execution condition — and
because that value is invariant across hops, the same id appears in every
connector that handled it. `RUST_LOG=debug` for more.

**Releases.** A release is one dispatch of `release-connector.yml`: it builds the
image, cuts a dated handle and opens a GitHub Release. It does not deploy, and
nothing here moves a tag onto a box (ADR 0068) — a node repository pins the
connector image it runs, by release handle, in its own `deploy/` bundle. Because
the binary and a box's mounted TOML are a matched pair in both directions,
**adding a required config key is a breaking deploy**: land the config first, then
bump that pin.

**Devnet** settles on Base Sepolia and Solana devnet; test funds come from the
[devnet faucet](https://faucet.devnet.toonprotocol.dev). **Mainnet** contracts and
program exist on Base and Solana mainnet-beta (records under
`packages/*/deployments/`), run by a third-party operator. **The fleet's production
tier is still named and empty** (ADR 0056): no fleet machine, no fleet key.

---

## Where to go next

| Path                                                                               | What it is                                                                                  |
| ---------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| [`docs/the-yellow-brick-road.md`](docs/the-yellow-brick-road.md)                   | **The idea.** Why you pay a path and not a destination, and why the road earns the traffic. |
| [`docs/rfcs/`](docs/rfcs/README.md)                                                | **The protocol.** Interledger, the ten vendored RFCs, and where TOON departs from each.     |
| [`docs/protocol/configuration-spec.md`](docs/protocol/configuration-spec.md)       | Every config key, and what each one binds.                                                  |
| [`docs/protocol/operator-spec.md`](docs/protocol/operator-spec.md)                 | The operator surface's rules, numbered.                                                     |
| [`docs/protocol/self-description-spec.md`](docs/protocol/self-description-spec.md) | What `GET /ilp` must and must not carry, rule by rule.                                      |
| [`docs/operators/`](docs/operators/)                                               | Runbooks: box bring-up, key rotation, fleet release and health, signing a write.            |
| [`deploy/connector-rust/README.md`](deploy/connector-rust/README.md)               | The container path in full, including the image tag table.                                  |
| [`local/`](local/README.md)                                                        | The shipped image against real chains — `make local-verify`.                                |
| [`CONTEXT.md`](CONTEXT.md)                                                         | The vocabulary. Read before writing docs or naming anything.                                |
| [`docs/adr/`](docs/adr/README.md)                                                  | Why any of this is the way it is. The tiebreaker for everything.                            |
| [`CONTRIBUTING.md`](CONTRIBUTING.md)                                               | Building from source, the test gate, the chain binaries it needs.                           |

## License

MIT — see [`LICENSE`](LICENSE). Except [`docs/rfcs/`](docs/rfcs/README.md), which
is CC BY-SA 4.0: it holds the Interledger Foundation's RFCs, reproduced
unmodified.
