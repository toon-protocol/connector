# `local/` — the shipped image, run against real chains

One connector image, real containerised chains, a real packet. That is the
whole scope.

```sh
make local-up        # build the image, start the chains, provision keys, run
                     #   it, and establish the peerings
make local-rehearse  # send real packets; non-zero unless they fulfil AND, on a
                     #   peered topology, the chain and the payee's journal say
                     #   it was paid
make local-down      # and remove the state volumes with it — see below for why
```

All of them work in one compose project, `connector`, named in
`docker-compose.yml` rather than taken from the directory. So there is one
stack per machine and `make local-down` reaches it from any checkout of this
repository; `make local-preflight` says whether it is free.

Or `make local-verify` for the four that need nothing but this machine, which
is what CI runs (`.github/workflows/local-topologies.yml`). `onion` needs a
working third-party anonymity network and is deliberately not on that gate —
see below.

`LOCAL_TOPOLOGY` picks which one; `solo` is the default.

```sh
make local-verify LOCAL_TOPOLOGY=mixed-chain
```

## What this is for, and what it is not

`cargo test` covers the connector's behaviour far better than a container can.
It spawns its own `anvil` and `solana-test-validator` **per test**, deploys into
them and throws them away (ADR 0007) — nothing under `crates/` dials
`localhost:8545` or `localhost:8899`, and `make anvil-up` before `cargo test`
changes nothing.

What `cargo test` structurally cannot check is the thing every deploy depends
on: that **the image**, running as uid 10001, with a mounted `connector.toml`,
mounted key files and a real volume at `/app/state`, boots and moves a packet.
That is this, and only this.

`devnet_configs_load.rs` boots the fleet's own committed `connector-rust.toml` fixtures
through the real binary, which is the half a GitHub runner can check without a
chain — ADR 0009 makes an unreachable settlement RPC a refuse-to-start. Here
there is a real chain, so this is an assertion rather than a boot-only check.
This one deliberately does **not** use the fleet's configs — its own name
local container URLs, which is exactly the substitution ADR 0041's config-boot
doctrine exists to avoid making.

## Connector layer only

No relay, no store, no faucet. Composition of a connector with a real app lives
in that app's repository; this repo builds only the connector image. The thing
behind the route here is `stub-app`, the image's second binary: it answers
`POST /`, holds no secret and does no cryptography, so it contributes nothing
to a packet's fulfilment — the connector derives that itself (ADR 0019).

The chains are the same compose services `make anvil-up` and `make solana-up`
start, merged into one project, so the connector reaches them by service name.

## The chains

**Every channel here is an x402 `batch-settlement` channel** (ADR 0075). The two
local chains hold exactly what such a channel lives on, placed from the same
committed bytes the tier-3 tests place:

- **anvil** — `infra/anvil/seed.sh` puts `x402BatchSettlement`,
  `ERC3009DepositCollector`, `Permit2DepositCollector`, Uniswap's Permit2 and
  Circle's `SignatureChecker` at their canonical addresses by `anvil_setCode`,
  and deploys **Circle's FiatToken v2.2 as USDC** (named "USDC", version "2", 6
  decimals) at `0xe7f1725E…0512`, with anvil's account 1 as its minter. The
  bytecode is `crates/connector-settlement-evm/contracts/x402/`, mounted
  read-only, exactly what `X402Chain::place` places. Nothing is compiled; the
  service mounts only what it reads.
- **the Solana validator** — `infra/solana/entrypoint.sh` loads
  solana-foundation's `payment-channels` at `CHNLx…` and mainnet-beta's Token
  program (p-token) at the SPL Token id into genesis, from the pinned, hash-checked
  dumps under `crates/connector-settlement-solana/fixtures/`, mounted rather than
  copied. p-token is there because the bundled SPL Token refuses the `Batch` a
  two-payout `distribute` sends (#1358).

**One TOON deployment is left on each chain, and no channel is ever opened on
it.** The connector on this tree still BOOTS through `[settlement.evm]
contract_address` (it resolves `getTokenNetwork(token_address)` and refuses to
start when that is zero) and `[settlement.solana] program_id` (it checks the
program is executable). So anvil still carries a `TokenNetworkRegistry` with a
`TokenNetwork` for the FiatToken, deployed from the committed registry bytecode,
and the validator still loads TOON's `payment_channel.so`. Both go with that boot
dependency (#1385). There is no `MockERC20`, no ERC-2771 forwarder and no
`RollingSwapChannel` on the local chain any more, and nothing runs `forge`.

anvil's healthcheck waits on the **last** contract the seed creates, and the
seed runs under `set -e` in its own process, so a healthy anvil is a fully
seeded one. A failed seed leaves anvil serving an empty chain that never turns
healthy, with the reason in its log.

## Topologies

| Topology                       | Nodes | What it proves                                                                                                                                                                                                                                                                                                                                                                        |
| ------------------------------ | ----- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`solo/`](solo/)               | 1     | The image boots on a mounted config with **both** settlement backends live at once, and a real packet reaches the app behind its one route.                                                                                                                                                                                                                                           |
| [`two-hop/`](two-hop/)         | 2     | Two images peered over ILP-over-HTTP on two x402 channels on anvil. B **prices** the route it terminates; A covers each crossing with a voucher before sending it, and the crossing can only reach B's app in the **peer role**.                                                                                                                                                      |
| [`mixed-chain/`](mixed-chain/) | 3     | A↔B settles on EVM **over BTP**, B↔C on Solana (`payment-channels`) **over ILP-over-HTTP**, and B holds both backends. One packet crosses two chains _and_ two carriages — the only place a shipped image carries a packet over BTP at all.                                                                                                                                           |
| [`dealing/`](dealing/)         | 3     | The same three-node, two-chain shape as `mixed-chain`, with the middle node **dealing**: 6-decimal USDC in on the EVM leg, a **9-decimal** mock SPL token out on the Solana leg, converted at a rate B declares and a spread B earns (ADR 0071). The only place a shipped image converts anything, and the only committed fixture here that declares `[[tokens]]`.                    |
| [`onion/`](onion/)             | 2     | The peering rides a **real onion network**: each node is reachable only at a `.anyone` address its `anon` sidecar generates, and the two connectors are on separate docker networks with **no route between them**, so a direct dial is impossible rather than merely unobserved. The only topology where an endpoint's HOST decides how it is dialed (ADR 0070). Not on the CI gate. |
| [`anyone/`](anyone/)           | 1     | The **client edge** over the same overlay, with the real payer: [`toon-client`](https://github.com/toon-protocol/toon-client) discovering, pricing and paying this node over a circuit. Needs that repository built, so it is run by its own `run.sh` rather than by `LOCAL_TOPOLOGY`. Not on the CI gate.                                                                            |

`connector-cli`'s `a_both_chains_config_attaches_and_routes_both_backends` proves
the both-chains concern in-process; `solo` is the only place a node is actually stood up with an EVM and
a Solana backend attached simultaneously.

`two-hop` is the containerised counterpart of
`crates/connector-cli/tests/peering_from_a_url.rs`, which proves the same
peering in-process against an `anvil` it spawns per test — and therefore says
nothing about the image, the mounted config, or two nodes finding each other
over a network.

`mixed-chain` is one node settling with different peers on different chains,
and the only thing that changes an amount between two hops there is a hop
subtracting its own flat fee — 1200 sent, 1100 across the boundary, 1050
delivered. **It is still not a conversion**, and what keeps it unconverted is
ADR 0071's **absence rule** (decision 2): those three configs declare no
`[[tokens]]`, so the middle node sits on no boundary. Both of its legs hold
6-decimal USDC anyway.

`dealing` is the topology where something does convert. It is deliberately the
same shape — so that the difference between the two directories is exactly the
thing under test: `[[tokens]]`, one `[[rates]]` row and `[rate_guards]` on the
middle node's config.

## How a peering is established here

ADR 0075: **a peering is two one-way x402 channels, one opened by each side.**
A→B carries A's vouchers to B, B→A carries B's to A, and each node opens and
funds only its own outbound channel. It is established from a URL (ADR 0058, as
0075 decision 4 amends it), so **no committed config under `local/` holds a
`[[pay_channels]]` row, and only `mixed-chain`'s B holds `[[peers]]` and
`[[peer_channels]]`** (for one peering, below) — there is no channel id for
anybody to paste, because an x402 channel is opened with a fresh salt and its
id is a fact of the run.

`make local-up` runs `local/keys.sh <topology>` before the nodes start (keys and
funding) and `local/keys.sh <topology> channels` after they are serving. The
second stage makes, for every peering in the script's topology table, the
signed operator writes an operator would (ADR 0008, signed by
`docs/operators/sign-write.sh`):

1. **`POST /peers` on the payee**, naming the payer's self-description URL. The
   payee reads the payer's `voucherSigners` and **binds** the payer's channel
   toward it to the peering; it also opens its own channel toward the payer —
   the other half of every peering — which nothing here ever pays on. First,
   so that the payer's very first voucher already arrives on a bound channel.
2. **`POST /peers` on the payer**, naming the payee's. The payer opens its
   channel toward the payee — on EVM an ERC-3009 `deposit` through the
   collector, paid for with its own gas; on Solana a payer-signed `open` posted
   to the payee's **sponsor endpoint** (ADR 0075 decision 3), so the payee holds
   the `payee` and `rent_payer` seats — and binds the payee's.
3. **`POST /channels/:id/fund` on the payer**, topping the channel up from its
   opening deposit to its target. `fund` takes an **increment**, so the stage
   reads the channel's collateral off the payer's own `GET /channels` first and
   funds the shortfall: a second `make local-up` moves no money.
4. **`POST /routes/peers` on each forwarding node**, pointing its prefix at the
   peering at the route's `price`.

Then it **reads every payer channel back off the chain it lives on** — the
EVM one out of `x402BatchSettlement.channels(id)`, the Solana one as a
`payment-channels` account whose `payer`, `payee`, `authorized_signer`, mint,
status and `deposit` must all agree with the peering — and refuses to report
success otherwise. The channel key the payee journals each voucher under
(`evm:0x…` or `solana:<account>`) goes into `local/.keys/<topology>/peerings/<id>`,
which is where the rehearsal learns it.

A node with no host port — `onion`'s B, whose only door is the circuit — is
written to from **inside its own container** over its loopback, with the
image's own busybox `wget`, rather than given a port for this script's sake.

### Where each figure lives

The peering half of every topology's arithmetic — each peering's `fee`, its
`max_packet_amount` and each forwarding route's `price` — is in `local/keys.sh`'s
**topology table**, because that script makes the writes that set them. The
terminating half — the `price` of the route that ends the path — is in the
committed config. `local_topologies_load.rs` parses the table and holds the two
halves and the rehearsal's own figures to one sum; no code anywhere checks that
a path adds up (ADR 0028), and no container can see a fee quietly back at zero.

|                 | collects | keeps       | forwards                                             |
| --------------- | -------- | ----------- | ---------------------------------------------------- |
| `two-hop` A     | 1100     | 100         | 1000, which is B's price exactly                     |
| `onion` A       | 1200     | 200         | 1000, which is B's price exactly                     |
| `mixed-chain` A | 1200     | 100         | 1100, which is B's route price exactly               |
| `mixed-chain` B | 1100     | 50          | 1050, delivered to C's unpriced termination          |
| `dealing` A     | 1200     | 100         | 1100 µUSDC, which is B's route price exactly         |
| `dealing` B     | 1100     | 6000 (9 dp) | 4350000 base units of the 9-decimal mock (see below) |

### The peer-role proof

On an x402 channel the payee journals a voucher in one book whichever role it
arrived under (`peer-carriage-spec.md` §1.8), so the journal alone cannot say
whether a crossing came from the **peering** or from a paying client. What can
is a **carriage pin** (ADR 0072): a client is refused a route pinned to the
carriage it did not arrive on, and a peer is not held to a client's pin. So each
terminating route is pinned to the carriage its peering does **not** ride —
`two-hop`'s B and `mixed-chain`'s and `dealing`'s C to BTP (they are paid over
ILP-over-HTTP), `onion`'s B to HTTP (it is paid over BTP). A crossing that
reaches the app at all arrived on a voucher from a channel the payee bound to
the peering; one from an unbound channel would be admitted as a client's and
refused.

The middle hop of `mixed-chain` and `dealing` needs one more step, because B
forwards and a runtime forwarding route cannot be pinned: a voucher on A's
channel would pay for a forward just as well from a client. So B also
terminates one route of its own, pinned to the carriage A does not pay it over
and priced at exactly what A forwards, and the rehearsal sends a third packet
there on the same channel. Reaching it proves A's channel is bound to the
peering — the channel every one of B's vouchers arrived on — and B's journal is
held to three vouchers of 1100 rather than two.

### The one config-declared peering

A runtime peering has no `forwarded_claim_enforcement` knob and **observes** —
an uncovered forwarded arrival is forwarded and logged rather than refused
`F06`. So `mixed-chain`'s B declares its A peering in its committed config
instead (ADR 0075 decision 9, issue #1380): a `[[peers]]` row with the `a-b`
id `keys.sh`'s table names, at `forwarded_claim_enforcement = "enforce"` (ADR
0042 item 3), the only enforcing peering in the repository, and a
`[[peer_channels]]` row naming A's EVM settlement address as its
`voucher_signer`. That address is committed because it is derived — the same
on every machine and every run — and the `channels` stage holds the row to the
derivation before it skips B's end of `POST /peers` for that peering. A still
opens and funds its own channel toward B with its `POST /peers`; B opens none
toward A, since nothing is paid that way. The rehearsal's journal read is
unchanged — B must show exactly one 1100 voucher per crossing — and now an A
that stopped covering is also refused on the wire.

## The dealing topology

`dealing` is the only stack in this repository where a **shipped image converts
an amount**, and the only committed config here that declares `[[tokens]]` at
all. Its middle node holds a 6-decimal USDC channel on anvil and a
**9-decimal** mock SPL channel on the local validator — two genuinely different
tokens — so every forward across it sits on a **denomination boundary** and is
refused unless a rate for that ordered pair has been declared (ADR 0071
decision 2).

A runtime x402 peering has no `[[peers]]` row to resolve its token from, so each
peering's token is read off its own channel binding (#1382): the `token_address`
of the settlement table on its chain. That is what B's two `[[tokens]]` rows
name.

### One packet's arithmetic, and where each number lives

```
1200  µUSDC  leave the sender                     local/keys.sh, A's route price
- 100                A's flat fee                                  a-b's fee (keys.sh)
= 1100  µUSDC  arrive at the boundary             = B's route price (keys.sh), exactly
× 4000/1             the declared mid             connector-b.toml, [[rates]]
× 99/100             less B's 1% spread           connector-b.toml, [rate_guards]
= 4356000            floor, the connector's way
-   6000             B's flat fee, in the OUTGOING unit            b-c's fee (keys.sh)
= 4350000  base units of the 9-decimal mock, forwarded and covered
```

The mid folds two independent things into one ratio, which is decision 4's
trick: 1000 of **scale** (10⁶ base units against 10⁹) and 4 of **price** (the
mock is worth a quarter of a USDC here). Nothing in the connector separates
them again, deliberately — scale is not price, and `[settlement] decimals`
stays a boot-time assertion against the chain rather than an input to value
arithmetic.

The fee on `b-c` is **6000, in 9-decimal base units**, because ADR 0071
decision 1 amends ADR 0061 in exactly that clause. And `b-c` writes out a
`max_packet_amount` of 1000000000, which it must: the cap defaults to 1000000
— one USDC at six decimals — and is checked against the amount actually going
out, post-conversion, so a peering that left it unwritten would refuse every
crossing `T04`.

The static `[[rates]]` row is the arm decision 3 provides for a pair that
cannot self-source: there is no AMM on either disposable chain and no
Solana-side `RateSource` exists. A declared row **never goes stale** — `ttl`
governs an observation, and a declaration is not one.

### Why the rehearsal reads the journal for a _figure_

A boundary converting at the **wrong rate** — or not converting at all,
forwarding the arriving 1100 onto a channel denominated a thousandfold
differently — fulfils exactly as happily: the packet reaches the app either
way, and only the number in the payee's journal is different. So the sender
crosses twice and then reads both journals against their own units: **exactly
1100 per crossing at B**, in µUSDC, unconverted because A crosses no boundary;
**exactly 4350000 per crossing at C**, in base units of the mock.

Around those, the chain: A's channel read off `x402BatchSettlement`, and B's
channel toward C read as a `payment-channels` account holding at least its
target `deposit` — **a thousand times** the EVM leg's base units for the same
hundred tokens, because a deposit is denominated in the base units of the
channel that holds it.

### The second mint, and why it is not committed

The 9-decimal token is created by `local/keys.sh dealing`, not by
`infra/solana/create-usdc-mint.sh`: that script seeds the mint every other
config in this repository names, devnet included, and this one exists for one
local topology. Its **address** is committed, in `connector-b.toml` and
`connector-c.toml`; its **keypair** is derived at a fixed index of the same
public anvil mnemonic every settlement key here comes from, so the address is
the same on every machine and after every `--reset` while nothing secret is
written down. Its authority is the `usdc-authority.json` the mock USDC mint
already uses. `keys.sh` asserts every committed config that settles on Solana
names the address it derived.

### Why it IS on the CI gate

`.github/workflows/local-topologies.yml` runs it, and the case is the mirror of
the `onion` one below. Everything this topology needs — an `anvil`, a
`solana-test-validator`, a built image — the three topologies already on that
gate need too, so nothing about it can go red for a reason outside this
repository. And the composition it proves exists nowhere else: `cargo test`
covers the conversion arithmetic, the rate table and the config refusals far
better than a container can, but nothing under `crates/` can show a **mounted
config that declares tokens** producing a **real Solana voucher for the
converted figure** on a channel in a mint that is not USDC.

## The onion topology

`onion` is the only place in this repository where an endpoint's **host**
decides how it is dialed. Neither node publishes a clearnet address: each
node's `anon` sidecar generates a hidden-service address for it, and each node
dials the other's through a SOCKS5 proxy on its own side. The peering is paid
for with ordinary EVM vouchers on ordinary anvil channels. Nothing about the
peering or the money changes — which is ADR 0070's whole claim, stated as a
deployment: an onion address is a **host**, not a carriage.

**Both nodes are onion nodes, because a peering is established from both
ends.** A's `POST /peers` reads B's self-description to open its channel toward
B; B's reads A's to bind the voucher signer A's channel is signed by. Each
sidecar therefore does two jobs for its own node: it publishes that node's
hidden service, and it is the proxy that node dials the other's through.

**There is no route between the two connectors, and that is the evidence.** A
fulfilled packet proves nothing about the circuit — a SOCKS dial that silently
fell back to a direct connection would fulfil exactly as happily — so a direct
dial has to be structurally impossible rather than merely unobserved. Each
connector sits on its own docker network with its own sidecar; docker does not
route between two user-defined bridge networks and its embedded DNS is per
network. `anvil` joins both, because both nodes settle on it, and a container
attached to two bridges forwards nothing between them. B publishes no host port
either: its only door is the circuit, which is why `local/keys.sh` writes to B
from inside B's own container.

The peering is **BTP**, deliberately: B publishes a `ws://…anyone/ilp/btp`
endpoint and `POST /peers` prefers BTP. The HTTP carriage asks its own client
for a proxy and gets one; the websocket library the BTP carriage speaks has none
at all, so its onion path establishes a SOCKS5 stream itself and hands the
already-established stream to the websocket client. That is the larger of the
two carriage changes and the one a loopback SOCKS5 fake can least stand in for.

### `.anyone`, and the daemon this builds

The sidecar is **built here**, from [`local/anon-image/`](anon-image/), rather than
pulled: `anon` renamed its hidden-service TLD between v0.4.9.7 (`.onion`) and v0.4.10.2
(`.anyone`), ghcr publishes no image for the newer one, and the rename is total — neither
release resolves the other's spelling (issue #1284). That Dockerfile overlays the official
release binary, sha256-verified, onto the last published image, and both hidden-service
topologies here use it.

The connector accepts **both** suffixes — one rule, `is_onion_endpoint`, ADR 0070 as
amended — so nothing under `crates/` turns on which daemon is running.

### The addresses do not exist until the daemons have run

ADR 0070 decision 7: each daemon generates its address into its
`HiddenServiceDir/hostname` and **the operator copies it** into `[node]`. The
connector never reads that file and never speaks the daemon's control protocol.

That collides with the committed-config discipline the rest of this directory
keeps, and the resolution is a **placeholder and a render**:

- `local/onion/connector-a.toml` and `connector-b.toml` are committed with one
  placeholder `.anyone` host each. A placeholder still ends in a hidden-service
  suffix, so the committed files load through the real parser and stay
  meaningful to `local_topologies_load.rs`.
- `local/keys.sh onion` plays the operator. It starts both sidecars, reads both
  addresses out of their `hostname` files, and writes both configs out again
  with each placeholder substituted, into
  `local/.keys/onion/<node>/connector.toml`. It records each address in
  `local/.keys/onion/<node>/onion-hostname`, which is how the `channels` stage
  names each node's URL and how the rehearsal finds B's.
- Compose mounts **the rendered copy**.

**No hidden-service key is ever committed.** Each daemon's key lives in its
named volume beside the `hostname` file and nowhere else. The volume is what
makes an address survive a **restart**, which is ADR 0070's operational point.
`make local-down` removes it with every other state volume; the next bring-up
reads a new address and renders it in.

Each daemon needs `AgreeToTerms 1` in its `anonrc` or it does not start, and a
`Nickname` line because the image's entrypoint appends one when it finds none
and these files are mounted read-only. And each `HiddenServicePort` names its
connector by a **fixed IP**, because `anon` resolves that target when it
**parses** its config — before the connector container exists, since the
connector's config names an address only the daemon can generate. Compose pins
`connector-a` to 172.29.0.10 and `connector-b` to 172.31.0.10, and
`local_topologies_load.rs` holds each pair to one literal.
`docs/operators/onion-endpoint-bringup.md` carries the same warning for a real
deployment.

### What is still dialed direct

Settlement RPC and the app's `handler_url` (ADR 0070 decision 4). Both nodes
reach `anvil` from their real addresses, and no key anywhere says so: the
host-selected rule reads the answer off each endpoint. An onion endpoint hides
**where a node is reachable** and nothing else, and this topology makes no
anonymity claim beyond that sentence.

The dial worth knowing about because it is easy to forget is the
**claim-state ask**. A covering payer asks the payee where its channel's
watermark stands (ADR 0075 decision 6), at the payee's published
`httpEndpoint` — a hidden-service URL here — so that ask goes through the same
proxy the carriage does.

## Why the onion topology is not on the CI gate

`.github/workflows/local-topologies.yml` runs `solo`, `two-hop`, `mixed-chain`
and `dealing`. It does not run `onion`, and that absence is a decision rather
than an oversight — **please do not fix it**.

This repository's rule is that a test **either runs or fails loudly**.
`require_anvil()` panics when `CI` is set rather than skipping, because a guard
that returns early and reports `passed` in `0.00s` is worse than a missing
test: it claims something. A gate that goes red when a third-party anonymity
network has a bad day is that rule **inverted** rather than honoured. The red
would say nothing about this repository, and the only sustainable responses to
it are the two this rule exists to forbid: retry until green, or add a skip.

The connector's own change is already on the gate and needs no network and no
daemon. "Dial through SOCKS5" and "accept a hidden-service host as a valid endpoint"
are both asserted in `cargo test` against a real SOCKS5 server on loopback —
`Socks5TestServer`, in `connector-runtime`'s test support. What this topology
adds is the **composition**: a real daemon, a real circuit, two containers that
cannot reach each other any other way. That is demonstrated by running it.

Run it by hand, on a machine with a working onion network:

```sh
make local-verify LOCAL_TOPOLOGY=onion
```

Bootstrapping a circuit takes minutes rather than seconds, so the sidecars'
health gates wait on `Bootstrapped 100%` and the rehearsal then waits for a
rendezvous with `connector send --dry-run` — which fetches B's
self-description over the circuit and sends no packet, so waiting costs no
crossing the money assertion would have to account for.

## Keys and money

Both are the same rule: nothing is committed, and nothing is assumed.

`local/keys.sh <topology>` generates every key into
`local/.keys/<topology>/<node>/`, which is gitignored, and then **funds** it.
Nothing it writes is committed, and every `key_file` in a committed
`connector.toml` here is a path (ADR 0009, ADR 0012). One directory per node,
named after that node's compose service; the node's config is
`local/<topology>/<node>.toml`.

Per node it writes `signer.key`, `settlement.key`, `settlement-solana.key`,
`settlement-solana-cli.json`, `operator-bearer-token`, `operator-write-keys`
and `operator-send.key`. The operator two are a pair: the allowlist holds the
**public** half (derived by the same binary that will verify, so the two cannot
disagree), and `connector send` and the `channels` stage hold the private half.
Ask for the allowlist value directly with:

```sh
connector send --operator-key <file> --print-keyid
```

`signer.key`, `operator-send.key` and `operator-bearer-token` are **random**.
The two settlement keys are **derived**, per node, from anvil's own published
test mnemonic at a fixed index. No committed file names their addresses any
more — a peering binds the other side's channel by the voucher signer its
self-description publishes — but an address that is the same on every run is
one fewer thing to disambiguate when a log names it, and the `channels` stage
checks every channel's counterparty against it. EVM and Solana take disjoint
index ranges, so no 32 bytes is ever used on both curves. Each settlement key is
also what signs that node's vouchers on its chain (ADR 0075 decision 3).

Funding involves **no faucet on either chain** — the faucet is an app-layer
service and is not part of the connector:

- **EVM.** anvil's genesis funds account 0 with 10,000 ETH, and ETH is a plain
  transfer from it. USDC is **minted on demand**: the FiatToken's minter is
  anvil's account 1 (`infra/anvil/seed.sh`), so each node's 1000 USDC is a
  `mint` and nobody's balance runs down.
- **Solana.** `solana airdrop` from the validator's genesis, and then mock USDC
  on top of it — SOL pays fees, it is not the asset a channel settles in. The
  mint is seeded by `make solana-mint-usdc`, which **fails** rather than warns
  when it cannot. An SPL mint has one authority, so each node's tokens are a
  `spl-token transfer` out of the treasury that script seeds — with
  `--fund-recipient`, because this runs before any node boots.
- **`dealing`'s Solana leg settles in a different token**, so `keys.sh` creates
  and seeds that one itself, under the same authority, and funds the nodes out
  of it instead.

Each payer's channel is opened at `OPEN_DEPOSIT` and topped up to 100 tokens —
`CHANNEL_DEPOSIT` base units, or `DEALING_CHANNEL_DEPOSIT` on `dealing`'s
9-decimal leg. Each payee's channel back toward its payer stays at the opening
deposit.

Devnet funds completely differently — the faucet and its mints, on public
chains. Do not carry an assumption from here to there.

## Sending a packet

`connector send` is the binary's second verb. It forms the packet the operator
surface cannot form for itself: an OER `Prepare` whose payload is gift-wrapped
to the terminating connector's identity (ADR 0018) under a condition minted
from the fulfilment that wrap derives (ADR 0019), inside an RFC 9421-signed
`POST /packets` (ADR 0008).

```sh
connector send \
  --operator  http://127.0.0.1:3001 \       # whose /packets originates it (two-hop's A)
  --operator-key local/.keys/two-hop/connector-a/operator-send.key \
  --to        g.local.two-hop.b.app \       # the ILP destination
  --seal-to   http://127.0.0.1:3002/ilp \   # the connector that TERMINATES it (B)
  --amount    1100 \
  --body      payload.json \
  --expect-fulfill
```

`--seal-to` is separate from `--operator` because a payload is sealed to the
node that terminates it, which in a multi-hop topology is not the node the
packet is handed to. It takes that node's self-description URL (ADR 0050).

`--expect-fulfill` is what makes the rehearsal a gate. Without it a REJECT is
reported and the process exits 0 — right for an operator probing what a route
does, wrong for CI.

## What `--expect-fulfill` cannot see

A peering's money is not on the packet's answer. A voucher's verdict rides back
in the ack (`Toon-Claim-Ack`, or BTP's `claim-ack`) and never gates the packet,
so a peering whose every voucher was refused still FULFILLs every packet. A
rehearsal that only checked the exit status would go green over a peering
carrying traffic for free.

So every peered topology's sender sources `local/rehearsal.sh` and asks two
more witnesses:

- **The chain**, before anything is sent: every payer's channel read off the
  contract or program it lives in, holding at least the collateral `keys.sh`
  topped it up to. A voucher on an unfunded channel is a signature, not a
  payment, and it verifies exactly as well.
- **The payee's own journal**, after two crossings: `client-edge-claims.log`,
  where the payee records every voucher it accepts as `inbound_claim_accepted`.
  The rehearsal requires **exactly one voucher per crossing** on the peering's
  channel, each advancing the payee's watermark by **exactly** the figure one
  crossing owes it. Fewer vouchers is a crossing carried for free; an advance
  below the figure is a hop keeping more than its fee, above it a hop keeping
  less — ADR 0010's earnings rule ("the difference between the cumulative it
  receives from upstream and the cumulative it sends downstream") measured on a
  running image; on `dealing` it is the converted figure.

**Two crossings, for issue #1102's reason.** A covering payer asks the payee
where its channel's watermark stands (`POST /ilp/claim-state`, ADR 0075
decision 6), and a payee answering out of the wrong book reports zero forever —
so crossing 2 re-signs crossing 1's cumulative amount, advances nothing, and is
refused. One crossing cannot see that. Two can, and did.

`mixed-chain`'s A→B crossing is the only place in this repository a shipped
image carries a packet on BTP, ADR 0027's first carriage. B sets
`peer_expose = "btp"`, so **no ILP-over-HTTP peer carriage is mounted on it at
all**, and B publishes the `ws://` endpoint `POST /peers` prefers. So one packet
crosses two carriages as well as two chains, which is the property `CONTEXT.md`
asserts: below the transport port there is one pipeline.

It is silent on things no container can see. It does not prove the price or the
fee is **enforced** against a figure that dropped to zero — holding each
committed `price`, `keys.sh`'s `fee`s and prices, and the sender's figures to one
arithmetic is `local_topologies_load.rs`'s job. And it does not **land** a
voucher: the receiving half's watchers and `POST /channels/:id/land` do that, and
the tier-3 tests cover them against the same contracts.

## Teardown, and one stack per machine

`make local-down` removes the state volumes. Both local chains wipe their own
state on every start, so keeping a journal across a down/up pairs a live
watermark with a chain that no longer has the history behind it — and,
concretely, a journal left by the last run would satisfy this run's money check
without this run having paid anything. The channel ids `keys.sh` recorded in
`local/.keys/<topology>/peerings/` are removed at the start of the next bring-up
for the same reason.

The compose project name used to follow the directory, so a stack started from
a git worktree was a _different_ project from the same repository's main
checkout: `make local-down` in one could not see the other's containers, network
or state volumes, and a `connector_solo-state` outlived every teardown on one
machine for two days (issue #1122). `docker-compose.yml` now names the project
`connector` outright, so the teardown reaches whatever the bring-up created,
from wherever either is run — and it removes the project's state volumes **by
label**, so a `two-hop-b-state` left behind by another topology goes with it.

One name means one stack per machine. That is not a capability being taken
away: every topology publishes 8545, 8899 and its connectors' client edges, so a
second stack was never going to run anyway. What changed is that the collision
is now **reported**. `make local-up` and `make local-verify` run
`local/stack-guard.sh` first (`make local-preflight` asks the same question by
hand), which refuses and names the directory and topology already holding the
stack, rather than letting this run adopt the other's containers. It also
refuses a start over state volumes left by a run that was killed rather than
torn down.
