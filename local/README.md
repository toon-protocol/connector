# `local/` — the shipped image, run against real chains

One connector image, real containerised chains, a real packet. That is the
whole scope.

```sh
make local-up        # build the image, start the chains, provision keys and
                     #   channels, run it
make local-rehearse  # send real packets; non-zero unless they fulfil AND, on a
                     #   peered topology, the payee's journal says it was paid
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
doctrine exists to avoid making. (`promote-to-fleet.yml`, which used to run a
config-compatibility check against a _candidate_ image before moving a fleet
tag, is retired — ADR 0068: neither devnet box deploys the connector from this
repository any more.)

## Connector layer only

No relay, no store, no faucet. Composition of a connector with a real app lives
in that app's repository; this repo builds only the connector image. The thing
behind the route here is `stub-app`, the image's second binary: it answers
`POST /`, holds no secret and does no cryptography, so it contributes nothing
to a packet's fulfilment — the connector derives that itself (ADR 0019).

A `deploy/connector-rust/local-stack/` bundle used to do a bigger version of
this with the published relay image. It is deleted: it was app-layer by
construction, it pinned a relay sha that would rot, and its chain ran on the
_host_ behind a hand-run Python TCP forwarder because `anvil` binds loopback.
Here the chains are the same compose services `make anvil-up` starts, merged
into one project, so the connector reaches them by service name and there is
nothing left to forward.

## Topologies

| Topology                       | Nodes | What it proves                                                                                                                                                                                                                                                                                                                                                                |
| ------------------------------ | ----- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`solo/`](solo/)               | 1     | The image boots on a mounted config with **both** settlement backends live at once, and a real packet reaches the app behind its one route.                                                                                                                                                                                                                                   |
| [`two-hop/`](two-hop/)         | 2     | Two images peered over ILP-over-HTTP. B **prices** the route it terminates; A covers each crossing before sending it, with a real EIP-712 claim on a real funded channel on the local anvil.                                                                                                                                                                                  |
| [`mixed-chain/`](mixed-chain/) | 3     | A↔B settles on EVM **over BTP**, B↔C on Solana **over ILP-over-HTTP**, and B holds both backends. One packet crosses two chains _and_ two carriages — the only place a shipped image carries a packet over BTP at all — and B **enforces** on the arrival it forwards, the only place ADR 0042 item 3's enforcing path runs.                                                  |
| [`dealing/`](dealing/)         | 3     | The same three-node, two-chain shape as `mixed-chain`, with the middle node **dealing**: 6-decimal mock USDC in on the EVM leg, a **9-decimal** mock SPL token out on the Solana leg, converted at a rate B declares and a spread B earns (ADR 0071). The only place a shipped image converts anything, and the only committed fixture here that declares `[[tokens]]`.       |
| [`onion/`](onion/)             | 2     | The peering rides a **real onion network**: B is reachable only at a `.anyone` address its `anon` sidecar generates, and the two connectors are on separate docker networks with **no route between them**, so a direct dial is impossible rather than merely unobserved. The only topology where an endpoint's HOST decides how it is dialed (ADR 0070). Not on the CI gate. |
| [`anyone/`](anyone/)           | 1     | The **client edge** over the same overlay, with the real payer: [`toon-client`](https://github.com/toon-protocol/toon-client) discovering, pricing and paying this node over a circuit. Needs that repository built, so it is run by its own `run.sh` rather than by `LOCAL_TOPOLOGY`. Not on the CI gate.                                                                    |

`two_ledgers_never_merge.rs` is named for the both-chains concern and proves it
in-process; `solo` is the only place a node is actually stood up with an EVM and
a Solana backend attached simultaneously.

`two-hop` is the containerised counterpart of
`crates/connector-bin/tests/two_connectors_peer.rs`, which proves the same
peering in-process against an `anvil` it spawns per test — and therefore says
nothing about the image, the mounted config, or two nodes finding each other
over a network. Its own header is the reference for what a peering has to
assert; this is that path with the containers left in.

They diverge in exactly one place, and it is deliberate. `two_connectors_peer.rs`
terminates at `price = 0`; `two-hop`'s payee charges for the route it terminates.
Both payers now hold a `[[pay_channels]]` row — the fixture's was added when the
row became required (issue #1145), and until then it had none (#1107) — so what
is left of the divergence is only the price. The fixture is still the reference
for what a peering must assert, and this is the only place a **priced** peer
termination is stood up and paid at all.

`mixed-chain` is one node settling with different peers on different chains,
and the only thing that changes an amount between two hops there is a hop
subtracting its own flat fee — 1200 sent, 1100 across the boundary, 1050
delivered. **It is still not a conversion, and what makes it one has changed.**
It used to be exile: the connector had no exchange rate at all, ADR 0010 having
replaced the spread with a flat per-packet fee, and value conversion was the
`swap` repository's job. ADR 0071 ended that — crossing a denomination is core
forwarding now — and what keeps `mixed-chain` unconverted is the **absence
rule** (decision 2) instead: those three configs declare no `[[tokens]]`, so
the middle node resolves no token for either peering, sits on no boundary, and
runs byte for byte the code it ran before the record. Both of its legs hold
6-decimal USDC anyway, so nothing there wants converting.

`dealing` is the topology where something does. It is deliberately the same
shape — three nodes, anvil on one leg and the local validator on the other, the
middle one holding both backends — so that the difference between the two
directories is exactly the thing under test: `[[tokens]]`, one `[[rates]]` row
and `[rate_guards]` on the middle node's config.

## The dealing topology

`dealing` is the only stack in this repository where a **shipped image converts
an amount**, and the only committed config here that declares `[[tokens]]` at
all. Its middle node holds a 6-decimal mock USDC channel on anvil and a
**9-decimal** mock SPL channel on the local validator — two genuinely different
tokens — so every forward across it sits on a **denomination boundary** and is
refused unless a rate for that ordered pair has been declared (ADR 0071
decision 2).

### One packet's arithmetic, and where each number lives

```
1200  µUSDC  leave the sender                     local/dealing/connector-a.toml, price
- 100                A's flat fee                                                fee on a-b
= 1100  µUSDC  arrive at the boundary             = connector-b.toml's price, exactly
× 4000/1             the declared mid             connector-b.toml, [[rates]]
× 99/100             less B's 1% spread           connector-b.toml, [rate_guards]
= 4356000            floor, the connector's way
-   6000             B's flat fee, in the OUTGOING unit                          fee on b-c
= 4350000  base units of the 9-decimal mock, forwarded and covered
```

The mid folds two independent things into one ratio, which is decision 4's
trick: 1000 of **scale** (10⁶ base units against 10⁹) and 4 of **price** (the
mock is worth a quarter of a USDC here). Nothing in the connector separates
them again, deliberately — scale is not price, and `[settlement] decimals`
stays a boot-time assertion against the chain rather than an input to value
arithmetic.

Two lines of that are worth reading twice. The `fee` on `b-c` is
**6000, in 9-decimal base units**, because ADR 0071 decision 1 amends ADR 0061
in exactly that clause: a fee attaches to a peering, and at a converting hop
the outgoing peering's unit is what the fee is denominated in. And `b-c` writes
out a `max_packet_amount`, which it must: the cap defaults to 1000000 — one
USDC at six decimals — and is checked against the amount actually going out,
post-conversion, so a node that left it unwritten would refuse every crossing
`T04` against a ceiling nobody meant. That is ADR 0071's "an 18-decimal leg has
a real ceiling" consequence, met here at nine.

The static `[[rates]]` row is the arm decision 3 provides for a pair that
cannot self-source: there is no AMM on either disposable chain and no
Solana-side `RateSource` exists, so the operator tends this rate by hand. A
declared row **never goes stale** — `ttl` governs an observation, and a
declaration is not one — which is why this topology rehearses the same an hour
after bring-up. `[rate_guards]` is still required as a set, because the guard
set is all-or-nothing the moment anything is declared.

### Why the rehearsal reads the journal for a _figure_

`--expect-fulfill` is even weaker here than on the peered topologies, and for
one more reason. A boundary converting at the **wrong rate** — or not
converting at all, forwarding the arriving 1100 onto a channel denominated a
thousandfold differently — fulfils exactly as happily: the packet reaches the
app either way, the app is payment-oblivious, and only the number in the
payee's claim journal is different. So the sender crosses twice and then reads
both journals against their own units: **1100 per crossing at B**, in µUSDC,
unconverted because A crosses no boundary; **4350000 per crossing at C**, in
base units of the mock. A rate, a spread or an outgoing fee that moved leaves
the first figure untouched and the second wrong, which is the whole reason the
second one is asserted rather than counted.

Around those, the same on-chain read `mixed-chain` does: the Solana channel
account exists, belongs to the payment-channel program and holds the payer's
collateral. It reads `deposit_a` at offset 104 rather than `deposit_b` at 112,
because which slot the payer holds is decided by the 32-byte sort the channel
PDA is derived from and this topology's indices put B first. A constant that
would read the unfunded side and report zero is not one that can pass by
accident.

### The second mint, and why it is not committed

The 9-decimal token is created by `local/keys.sh dealing`, not by
`infra/solana/create-usdc-mint.sh`: that script seeds the mint every other
config in this repository names, devnet included, and this one exists for one
local topology. Its **address** is committed, in `connector-b.toml` and
`connector-c.toml`; its **keypair** is derived at a fixed index of the same
public anvil mnemonic every settlement key here comes from, so the address is
the same on every machine and after every `--reset` while nothing secret is
written down. Its authority is the `usdc-authority.json` the mock USDC mint
already uses — one allowlisted local-chain key, not a second one. `keys.sh`
asserts every committed config that settles on Solana names the address it
derived, which is the drift guard that makes committing it legitimate.

It also means this topology's Solana **channel account** is not the address the
same two participants would derive on the USDC mint: a channel PDA is
`find_program_address(["channel", min, max, mint])`, and the mint is in the
seeds.

### Why it IS on the CI gate

`.github/workflows/local-topologies.yml` runs it, and the case is the mirror of
the `onion` one below. Everything this topology needs — an `anvil`, a
`solana-test-validator`, a built image — the three topologies already on that
gate need too, so nothing about it can go red for a reason outside this
repository. And the composition it proves exists nowhere else: `cargo test`
covers the conversion arithmetic, the rate table and the config refusals far
better than a container can, but nothing under `crates/` can show a **mounted
config that declares tokens** producing a **real Solana claim for the converted
figure** against a channel derived from a mint that is not USDC. That is what
would rot silently if it only ever ran by hand.

## The onion topology

`onion` is the only place in this repository where an endpoint's **host**
decides how it is dialed. B publishes no clearnet address at all: an `anon`
sidecar generates a hidden-service address for it, A dials
`ws://<addr>.anyone/ilp/btp` through a SOCKS5 proxy on its own side, and the
packet is paid for with an ordinary EIP-712 claim on an ordinary anvil
channel. Nothing about the peering, the claim book or the money changes —
which is ADR 0070's whole claim, stated as a deployment: an onion address is a
**host**, not a carriage.

**There is no route between the two connectors, and that is the evidence.** A
fulfilled packet proves nothing about the circuit — a SOCKS dial that silently
fell back to a direct connection would fulfil exactly as happily — so a direct
dial has to be structurally impossible rather than merely unobserved. Each
connector sits on its own docker network with its own sidecar; docker does not
route between two user-defined bridge networks and its embedded DNS is per
network, so A cannot reach `connector-b` and cannot even resolve the name.
`anvil` joins both, because both nodes settle on it, and a container attached
to two bridges forwards nothing between them. B publishes no host port either:
its only door is the circuit.

The peering is **BTP**, deliberately. The HTTP carriage asks its own client for
a proxy and gets one; the websocket library the BTP carriage speaks has none at
all, so its onion path establishes a SOCKS5 stream itself and hands the
already-established stream to the websocket client. That is the larger of the
two carriage changes and the one a loopback SOCKS5 fake can least stand in for.
The client edge rides the same onion service on the same port, because it is
the same listener — one hidden service, both surfaces.

### `.anyone`, and the daemon this builds

The sidecar is **built here**, from [`local/anon-image/`](anon-image/), rather than
pulled: `anon` renamed its hidden-service TLD between v0.4.9.7 (`.onion`) and v0.4.10.2
(`.anyone`), ghcr publishes no image for the newer one, and the rename is total — neither
release resolves the other's spelling (issue #1284). That Dockerfile overlays the official
release binary, sha256-verified, onto the last published image, and both hidden-service
topologies here use it.

The connector accepts **both** suffixes — one rule, `is_onion_endpoint`, ADR 0070 as
amended — so nothing under `crates/` turns on which daemon is running. What does turn on
it is every address in this directory: the committed placeholders are spelled `.anyone`
because that is what this sidecar writes, and `local_topologies_load.rs` holds the
placeholder's TLD and the image's pinned version to one fact so the two cannot drift.

### The address does not exist until the daemon has run

ADR 0070 decision 7: the daemon generates the address into its
`HiddenServiceDir/hostname` and **the operator copies it** into `[node]` and
into the dialing node's peer `endpoint`. The connector never reads that file
and never speaks the daemon's control protocol.

That collides with the committed-config discipline the rest of this directory
keeps, and the resolution is a **placeholder and a render**:

- `local/onion/connector-a.toml` and `connector-b.toml` are committed with a
  placeholder `.anyone` host. A placeholder still ends in a hidden-service
  suffix, so the
  committed files load through the real parser and stay meaningful to
  `local_topologies_load.rs` — which is the whole reason for committing a
  config rather than generating one.
- `local/keys.sh onion` plays the operator. It starts the sidecars, reads the
  address out of `hostname`, and writes both configs out again with the address
  substituted, into `local/.keys/onion/<node>/connector.toml`.
- Compose mounts **the rendered copy**. Mounting the committed one would boot a
  node whose peer endpoint is the placeholder.

It is the manual step automated exactly as far as it goes, and no further:
what changes is who does the copying, not what the connector reads.

**No hidden-service key is ever committed.** The daemon's key lives in the
named volume beside the `hostname` file and nowhere else — nothing on the host
filesystem holds it, `tools/ci/check-tracked-secrets.sh` would refuse it if
anything tried, and a fixed onion key in a public repository is an address
anyone who cloned the repository could impersonate. The volume is what makes
the address survive a **restart**, which is ADR 0070's operational point: an
unpersisted `HiddenServiceDir` changes the node's address on every start and
every counterparty's configuration goes stale silently. `make local-down`
removes it with every other state volume, which is a different thing — the next
bring-up reads a new address and renders it in.

The daemon needs `AgreeToTerms 1` in its `anonrc` or it does not start: in a
container it fails fast rather than prompting. Both `local/onion/anonrc-a` and
`anonrc-b` carry it, and both carry a `Nickname` line for a less obvious
reason — the image's entrypoint appends one when it finds none, and these files
are mounted read-only.

A third trap of the same shape: `HiddenServicePort`'s target is resolved when the
daemon **parses** its config, not when a stream arrives, so a container name that
does not exist yet kills it — `Unparseable address in hidden service port
configuration`, followed by an abort. That is unavoidable here, because the daemon
has to start before the config naming its address can be rendered. `anonrc-b`
therefore names a **fixed IP** and compose pins `connector-b` to it, held to one
literal by `local_topologies_load.rs`.
`docs/operators/onion-endpoint-bringup.md` carries the same warning for a real
deployment.

### What is still dialed direct

Settlement RPC and the app's `handler_url` (ADR 0070 decision 4). Both nodes
reach `anvil` from their real addresses, and no key anywhere says so: the
host-selected rule reads the answer off each endpoint. An onion endpoint hides
**where a node is reachable** and nothing else, and this topology makes no
anonymity claim beyond that sentence.

The one dial worth knowing about because it is easy to forget is the
**claim-state ask**. A covering payer asks the payee where its claims stand on
every covered PREPARE (issue #1102), so A's `[[pay_channels]]` row names B's
hidden-service client edge and that ask goes through the same proxy the carriage does —
a hop reachable only over a circuit is not payable otherwise, and the packet
would be refused for want of a covering claim long before the carriage was
asked to carry it.

## Why the onion topology is not on the CI gate

`.github/workflows/local-topologies.yml` runs `solo`, `two-hop` and
`mixed-chain`. It does not run `onion`, and that absence is a decision rather
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
`Socks5TestServer`, in `connector-runtime`'s test support — and the dial is
proved to have traversed it by the target the proxy recorded, not by a call
being counted. What this topology adds is the **composition**: a real daemon, a
real circuit, two containers that cannot reach each other any other way. That
is demonstrated by running it, which is what it is for.

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
and `operator-send.key`. A peering needs nothing here: since ADR 0060 there is
no shared `peer-<id>-secret`, because role is decided by the covering claim's
signature rather than by a string both operators wrote down. The operator two
are a pair: the allowlist holds the
**public** half (derived by the same binary that will sign, so the two cannot
disagree), and `connector send` holds the private half. Ask for the allowlist
value directly with:

```sh
connector send --operator-key <file> --print-keyid
```

### Random and derived, and why the split exists

`signer.key`, `operator-send.key`, `operator-bearer-token` and the peering
secrets are **random**. None of them appears in a committed file, so nothing
depends on their value.

The two settlement keys are **derived**, per node, from anvil's own published
test mnemonic at a fixed index. They have to be: a `[[peer_channels]]` row
names the `counterparty_key` whose signature this node accepts, and a committed
config cannot say "whatever address the other container happened to generate".
The mnemonic is public knowledge — anvil prints it on every start, and account
0's private key was already in `keys.sh` as the local chain's deployer — so
deriving from it introduces no secret that did not already exist, and the
alternative (a fixed throwaway key checked in under `local/`) would introduce
one. EVM and Solana take disjoint index ranges, so no 32 bytes is ever used on
both curves.

Every address a committed config names is then **checked against the chain**:
`keys.sh` derives each settlement address, resolves the deployed
`TokenNetwork`, opens the EVM peering's channel and reads back its id, and
computes the Solana channel PDA — and refuses to provision, naming the value it
computed, if a committed file disagrees. Its `solana-channels` stage then opens
and funds the Solana channel once the nodes are serving and reads _that_
account back too. Those checks are what make committed-not-generated safe here.

Funding involves **no faucet on either chain** — the faucet is an app-layer
service and is not part of the connector:

- **EVM.** anvil's genesis funds account 0 with 10,000 ETH; it is the deployer
  `DeployLocal.s.sol` runs as, so it owns the settlement topology. ETH is a
  plain transfer from it and USDC is a `mint` — `MockERC20` is mintable, so
  nobody's balance runs down.
- **Solana.** `solana airdrop` from the validator's genesis, and then mock USDC
  on top of it — SOL pays fees, it is not the asset a channel settles in. The
  mint is seeded by `make solana-mint-usdc`, which **fails** rather than warns
  when it cannot: a validator without that mint cannot satisfy the committed
  `token_address`, and the node will refuse to start. Unlike anvil's mintable
  `MockERC20`, an SPL mint has one authority, so each node's tokens are a
  `spl-token transfer` out of the treasury that script seeds — with
  `--fund-recipient`, because this runs before any node boots and the
  associated token account it lands in does not exist yet.
- **`dealing`'s Solana leg settles in a different token**, so `keys.sh` creates
  and seeds that one itself, under the same authority, and funds the nodes out
  of it instead. Everything above still applies; what changes is which mint is
  on the other end of the transfer, and that the collateral figure is a
  thousand times larger because base units are what a deposit is denominated
  in. See "The dealing topology" above.

Devnet funds completely differently — the faucet box and its treasuries, on
public chains. Do not carry an assumption from here to there.

## Sending a packet

`connector send` is the binary's third verb. It forms the packet the operator
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
  --amount    1000 \
  --body      payload.json \
  --expect-fulfill
```

`--seal-to` is separate from `--operator` because a payload is sealed to the
node that terminates it, which in a multi-hop topology is not the node the
packet is handed to. It takes that node's self-description URL (ADR 0050) —
the one whose `GET` answers with the identity to seal to, e.g.
`http://127.0.0.1:3002/ilp` — never an origin. ADR 0050 publishes that
identity; it does not yet let a client _discover_ which URL to ask, so the
caller still names it directly.

`--expect-fulfill` is what makes the rehearsal a gate. Without it a REJECT is
reported and the process exits 0 — right for an operator probing what a route
does, wrong for CI, where a run that prints `REJECT F02` and goes green is the
same nothing-asserted success ADR 0007 bans elsewhere.

## What `--expect-fulfill` cannot see

A peering's money is not on the packet's answer. A peer claim's verdict rides
back in the `Toon-Claim-Ack` header and never gates the packet
(`handle_peer_prepare` returns the answer and the ack side by side), so a
peering whose every claim was refused still FULFILLs every packet. A rehearsal
that only checked the exit status would go green over a peering carrying
traffic for free — the same nothing-asserted success ADR 0007 bans elsewhere.

So `two-hop`, `mixed-chain` and `dealing` cross **twice** and then read the
payee's own claim journal — and on `dealing` there is a second thing it cannot
see, since a boundary converting at the wrong rate fulfils every packet just as
happily as one converting at the right one. That topology's section above has
the figure and the reason. **Both now cross twice for the same reason**, and that reason is
issue #1102: a covering payer asks the payee where its claims stand on every
packet, and a payee answering out of the wrong book reports nonce 0 forever — so
crossing 2 re-signs crossing 1's cumulative amount at a fresh nonce and advances
nothing, accepted every time and buying nothing. One crossing cannot see that.
Two can, and did.

**`mixed-chain` used to cross twice for a different reason, and that reason has
stopped existing** (issue #1145). It was postpay: value moved on fulfilment (ADR
0004), so its payer owed nothing until the first crossing had fulfilled and the
claim covering crossing _n_ rode crossing _n + 1_ — one packet proved delivery
and could say nothing at all about payment. That model is deleted from the tree,
not relocated: `cover_forward` has no uncovered arm, no fulfilment arms a peer
claim, and `Config::load` refuses a peering a route forwards to with no
`[[pay_channels]]` row. **The coverage the second crossing used to provide does
not move somewhere else. It stops being needed, because the thing it covered
stops existing.** What the second crossing is worth here now is exactly what it
is worth on `two-hop`, and no more.

Both topologies' payers hold `[[pay_channels]]` rows (ADR 0042 item 2), so
`cover_forward` mints the claim **before** the packet is sent and crossing 1
arrives already paid for. On `two-hop` that is what lets B price the route it
terminates, since a priced peer termination refuses an uncovered arrival. On
`mixed-chain` it is what lets B **enforce** on the arrival from A — see below.

**What reading the journal proves, and what it does not.** `two-hop`'s sender
walks B's journal line by line and fails unless there are at least as many
accepted claims on the peering's channel as crossings sent, each advances the
cumulative amount by at least the price, and the final watermark is at least
crossings × price. That advance is the exact quantity `price_gate::payment_required`
charges against, which is what makes it a measurement rather than a restatement.

Since #1144 the same lines answer the other half of the money question: **what A
kept**. A collected the packet's `AMOUNT` and its claim to B advanced by what it
forwarded, so `AMOUNT - advance` is this hop's earnings, and the rehearsal fails
unless that is A's `fee` exactly — keeping less than it charges is a hop working
for free, keeping more is a hop eroding what it passes on. That is ADR 0010's
earnings rule ("the difference between the cumulative it receives from upstream
and the cumulative it sends downstream") measured on a running image, and it is
not implied by the advance check: an advance of `price + 1` satisfies that one
while leaving A a unit short.

It is silent on four things. It does not prove the price or the fee is
**enforced**: the gate returns early when a route's `price` is `0`, before any
comparison runs, so a price quietly dropped to zero satisfies every one of those
checks, and a `fee` back at zero would leave every claim advancing by the price
and every packet fulfilling — holding B's committed `price`, A's committed `fee`
and the sender's `AMOUNT`/`PRICE`/`FEE` to one arithmetic is
`local_topologies_load.rs`'s job, and no container can do it. It does not say
which claim paid for which packet, only that the totals line up. It says nothing
about whether any of it could be **redeemed on chain**, for the reason the next
section gives: nothing on the peer path reads a chain. And it says nothing about
the BTP carriage, because `two-hop` is ILP-over-HTTP on both sides and stays
that way — that gap moved to `mixed-chain` in issue #1155 rather than closing
here, and the paragraph below is where it closed.

`mixed-chain`'s money check used to be the weaker one, and could not have been
anything else: it greps each payee's journal for an accepted claim on its own
channel, and while the legs were postpay there was no figure to hold that claim
to — after N crossings a journal legitimately held N − 1 claims, because the
last crossing was unpaid by construction. Both legs cover before they send now
(issue #1145), so N crossings owe exactly N advances, and each payee's journal
is checked against **its own** figure: 1100 accepted at B per crossing, 1050 at
C. That difference is B's flat fee, measured on a running image, and it is the
one thing a grep could never see — a fee back at zero, or doubled, leaves both
journals as green as ever while moving one of those two numbers.

It gained a second thing on the other side of the same crossing: B's `a-b`
peering **enforces**, so an arrival from A that carried no covering claim is
refused `F06` and never reaches C at all. A regression that stopped A covering
fails `--expect-fulfill` on the first crossing rather than passing quietly.

And a third, in issue #1155: **that crossing is now carried over BTP**, and it
is the only place in this repository where a shipped image carries a packet on
ADR 0027's first carriage at all. A dials `ws://connector-b:3000/ilp/btp` — the
same listener `client_edge_addr` binds, since BTP rides `GET /ilp/btp` and
needs no port of its own — while B still dials C over `http://`. So one packet
crosses two carriages as well as two chains, which is the property `CONTEXT.md`
asserts and nothing here could previously show: below the transport port there
is one pipeline, and peer behaviour that exists on one carriage and not the
other is a defect rather than a difference.

The journal grep at B is what makes that a measurement rather than a
configuration claim. B sets `peer_expose = "btp"`, so **no ILP-over-HTTP peer
carriage is mounted on it at all** — an A that fell back to `POST /ilp` would
be admitted as an ordinary client, whose claims never reach the peer book. An
accepted peer claim in B's journal therefore cannot have arrived on anything
but a BTP frame, and the 2200 cumulative that check already demanded is
evidence of the carriage for free.

`two-hop` is deliberately **not** converted. It is where a priced peer
termination is proven and it is the containerised counterpart of
`two_connectors_peer.rs`, which it already diverges from in exactly one place;
a second divergence is worth more than a second BTP peering. `peer_expose`'s
default is untouched too — say nothing and a node still exposes no peer
listener at all, which is right, because a peer listener is the surface that
accepts value-bearing traffic. Carriage is operator policy, never a protocol
constant.

This is also why `make local-down` removes the state volumes. Both local chains
wipe their own state on every start, so keeping a claim journal across a
down/up pairs a live watermark with a chain that no longer has the history
behind it — and, concretely, a journal left by the last run satisfies this
run's money check without this run having paid anything.

For a while it did not actually manage that, and the reason is worth knowing
because it is invisible from inside one checkout. The compose project name used
to follow the directory, so a stack started from a git worktree was a
_different_ project from the same repository's main checkout: `make local-down`
in one could not see the other's containers, network or state volumes, and a
`connector_solo-state` outlived every teardown on one machine for two days
(issue #1122). `docker-compose.yml` now names the project `connector` outright,
so the teardown reaches whatever the bring-up created, from wherever either is
run — and it removes the project's state volumes **by label**, so a
`two-hop-b-state` left behind by another topology goes with it rather than
waiting for someone to run the matching `LOCAL_TOPOLOGY`.

One name means one stack per machine. That is not a capability being taken
away: every topology publishes 8545, 8899 and its connectors' client edges, so
a second stack was never going to run anyway — it would half-start, fail on a
port bind, and leave residue the other checkout could not reach. What changed
is that the collision is now **reported**. `make local-up` and `make
local-verify` run `local/stack-guard.sh` first (`make local-preflight` asks the
same question by hand), which refuses and names the directory and topology
already holding the stack, rather than letting this run adopt the other's
containers — an `anvil` with another checkout's `packages/contracts` mounted
into it, say. It also refuses a start over state volumes left by a run that was
killed rather than torn down, for the reason in the paragraph above: those
volumes are the journals the money assertion reads.

Two consequences worth knowing before editing a config here:

- **Every payer here holds a `[[pay_channels]]` row, and none of them has a
  choice about it** (issue #1145). A peering a `[[routes]]` entry forwards to
  with no row is refused at load by name (`ConfigError::PayChannelUnbound`):
  ADR 0042 has a connector cover every PREPARE it sends, and there is no longer
  an uncovered path for a forward to take. A peering a node only _receives_ on
  needs nothing — `mixed-chain`'s B holds one of those (`a-b`), and `two-hop`'s
  B and `mixed-chain`'s C are entirely accept-only.

  What that row buys, beyond paying: **a peer termination can be priced.** A
  route a node both terminates and prices refuses a peer PREPARE that arrives
  without a covering claim (`F06`, issue #880), and under the deleted postpay
  convention the first crossing carried none — so it was refused, never
  fulfilled, left nothing owed, and the second carried none either. The peering
  deadlocked rather than charging. `two-hop`'s payee prices its termination for
  exactly that reason. `mixed-chain`'s C still terminates at `price = 0`, and
  now only because that is not what that topology is about: what a hop there is
  paid is the forwarded amount, on a real Solana claim.
  `local/two-hop/connector-a.toml` and `connector-b.toml` carry the two halves
  of the long version, including the defect the row found the first time it was
  tried here (issue #1102, fixed by #1103, and the reason the rehearsal counts
  what each claim **advanced** rather than that a claim exists).

- **Every forwarding hop charges, and the numbers are kept true by hand.** Each
  forwarded route here has a real `fee` (issue #1144), so the flat per-packet
  fee that is ADR 0010's whole revenue model is finally exercised by a shipped
  image and not by `cargo test` alone. It could not be until #1143: the operator
  surface declared `minimum_delivery = amount` on every packet it originated, so
  `amount_after_fee` refused any hop that would retain anything and a non-zero
  fee turned the rehearsal into an unmeetable floor. That lockout is retired (ADR 0057) — no packet
  declares a floor, and a hop keeps its fee. (`R01` itself is not gone: it still answers RFC 0027's
  own case, a hop whose fee alone exceeds the arriving amount, which no topology here provokes.)

  The arithmetic is ADR 0028's: a hop collects `price`, forwards `price - fee`,
  earns exactly its `fee`, and a path adds up only while every hop's
  `price - fee` is at least the next hop's `price`. **No code anywhere checks
  that** — a connector cannot know what the next hop charges — so it holds here
  because the committed files were written together, and `local_topologies_load.rs`
  is what notices when they stop agreeing:

  |                 | collects | keeps | forwards                                    |
  | --------------- | -------- | ----- | ------------------------------------------- |
  | `two-hop` A     | 1100     | 100   | 1000, which is B's price exactly            |
  | `mixed-chain` A | 1200     | 100   | 1100, which is B's price exactly            |
  | `mixed-chain` B | 1100     | 50    | 1050, delivered to C's unpriced termination |

  The two `mixed-chain` fees differ deliberately: a fee is one peering
  relation's bilateral price for carriage, never a network-wide constant, and
  equal fees would let a hop charging the wrong one pass unnoticed. What the
  operator sends is the path's **cost** (`CONTEXT.md`) — the fees of every hop
  that carries the packet plus what the terminating leg is paid. The first
  hop's own fee sits in that sum without the operator paying anybody: it is
  value that never leaves a node the operator owns.

- **One peering here enforces on a _forwarded_ arrival, and it is the only one
  that turns the setting on.** ADR 0042 item 3 (issue #1142) requires a
  forwarded arrival to carry a claim covering the arriving amount, behind a
  per-peer `forwarded_claim_enforcement` that defaults to `observe`.
  `mixed-chain`'s B is no longer the only node here that forwards a packet which
  arrived from a peer — `dealing`'s B does too, and holds the knob at its
  default on purpose (ADR 0042's `## Update (issue #1303)`) — but `mixed-chain`'s
  `a-b` row is the only one that reads `"enforce"`, which is the first and only
  time that path runs against a running image anywhere.

  It could not be turned on before issue #1145. That peering was postpay, so its
  first crossing was uncovered by construction and enforcing would have
  deadlocked it exactly as a priced postpay termination deadlocked. A now holds
  a `[[pay_channels]]` row and covers `amount_after_fee(1200, 100)` — exactly
  the 1100 that arrives at B, which is the symmetric figure ADR 0042 item 3
  names. Observing is still the default everywhere else, and `two-hop`'s B never
  forwards anything, so the setting would do nothing there.

## What a peer claim does and does not check

`ClaimBook` verifies a peer claim's signature against the `counterparty_key`
its operator configured, and nothing else (CF-23). It reads no chain. So the
channels `keys.sh` opens and funds on anvil are not what makes a crossing
verify — a topology would rehearse green against a channel with a zero deposit,
which was tried. They are opened and funded anyway, for the reason
`two_connectors_peer.rs`'s fixture does the same: a claim naming a channel
nobody could redeem is not a payment, and the difference does not show up until
somebody tries.

The Solana peering's channel is opened **and funded** too, and by a different
route on both counts, because nothing in this repository can submit either
instruction from a shell. `InitializeChannel` is a positional account list
under an 8-byte discriminator, `spl-token` knows only SPL Token, and the Solana
CLI cannot build an arbitrary program instruction. `Deposit` is worse than
inconvenient to build by hand — it credits strictly **by signer**, with no
participant parameter, so only the depositing node can submit its own
collateral at all. The submitter for both is therefore a **running node's
operator surface** — `POST /channels` and `POST /channels/:id/fund`, ADR 0008's
writes — reaching `SolanaSettlementBackend::open` and `::fund` under that
node's own `[settlement.solana]` key. That is the right party as well as the
only available one: the channel's on-chain participant _is_ that settlement
identity, and it is the identity that will sign every claim on the channel.

Which is why `keys.sh` runs twice. `local/keys.sh <topology>` is everything
that has to exist before a node starts — including the mock USDC in each node's
own settlement account, which is what it later has to collateralise _with_;
`local/keys.sh <topology> solana-channels` runs after `--wait`, and `make
local-up` calls both with the containers started in between. The second stage
delegates to `local/open-solana-channel.py`, which makes both signed writes and
then reads the account back off the validator, refusing to report success
unless the deployed program's own layout agrees with the committed config — the
discriminator, both participants, the mint, `Opened` status, and the payer's
own deposit.

It is idempotent on both writes, and the two are idempotent differently. A
channel already at the expected address is left alone and still asserted. The
deposit is a **top-up**: `POST /channels/:id/fund` takes an increment, unlike
the EVM leg's absolute `setTotalDeposit`, so the script reads the payer's own
on-chain deposit first and deposits only the shortfall — nothing at all on a
second `make local-up`. That asymmetry is the one thing about this stage worth
remembering: the same figure reached by an absolute write on one chain and a
relative one on the other.

Neither journal can corroborate any of that, which is why the rehearsal asks the
chain rather than the payee. An accepted claim is a signature check against a
configured key and nothing more, so a journal stays exactly as green against an
address nobody ever created — which is what this topology used to settle
against. `mixed-chain`'s sender therefore reads the channel account off the
validator itself: that the payment-channel program owns it, and that it still
holds the payer's collateral. That runs before anything is sent, so a failure
names the missing channel or the missing deposit rather than a puzzling claim
further down.

**Opened is not funded, and both chains now do both.** They arrive there by
different routes, and the difference is worth keeping straight because it is
about who may submit, not about what the channel ends up holding. The EVM
`TokenNetwork`'s `setTotalDeposit` names the participant being credited
separately from the caller whose tokens are pulled, so `cast` can deposit for
the payer before any node exists — which is what the first stage does. The
Solana program's `Deposit` credits by signer, so only the payer's own node can,
and only after it is serving. Issue #1118 settled which of the two the port
means: `SettlementBackend::fund` is a **self-deposit** on both chains, backing
the claims that node _signs_ (`own_deposited`), never the counterparty's side
(`counterparty_deposited`, which is what bounds a claim this node could
_redeem_). So the honest summary is now: a peering's channel is real on both
chains, its collateral is real on both chains, and on Solana the deposit is
read back out of the program's own account twice — once by the script that
makes it and once by the rehearsal, before it sends.

What is still true is the sentence at the top of this section: none of it is
what makes a crossing verify. A claim is a signature check against a configured
key. The collateral is what makes the claim worth something to whoever holds
it, and `packages/solana-program`'s `ClaimFromChannel` bounds a claim by the
claimer's own deposit — which is precisely why the payer's side is the one
funded here.
