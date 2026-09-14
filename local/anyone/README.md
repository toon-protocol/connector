# `local/anyone` — the client edge over a `.anyone` hidden service

A connector reachable **only** at a hidden-service address, and the
[toon client](https://github.com/toon-protocol/toon-client) paying it over that
circuit. Two isolated docker networks, so a fulfilled packet is evidence of a
circuit rather than evidence of a docker network.

```sh
export TOON_CLIENT_REPO=../toon-client     # with `pnpm build` already run
./local/anyone/run.sh verify               # up + pay + down
```

## What this is for, and how it differs from `local/onion`

[`local/onion`](../onion/) proves a **peering**: two connectors that cannot
reach each other, exchanging BTP frames over a circuit. This proves the **client
edge**: the real payer — a different repository's library, not `connector send`
— discovering, pricing, and paying this node over one.

Both now run the **same daemon**, [`local/anon-image`](../anon-image/), and that is the
finding this topology produced.

## The TLD renamed, and this repository was on the wrong side of it

Anyone Protocol's `anon` renamed its hidden-service TLD between the release this
repository pinned and the one `toon-client` pins. Verified on the binaries and
then on the wire:

|                                   | `anon` v0.4.9.7              | `anon` v0.4.10.2                 |
| --------------------------------- | ---------------------------- | -------------------------------- |
| Pinned by                         | this repo, until issue #1284 | `toon-client`, as `ANON_VERSION` |
| Hidden-service TLD                | `.onion`                     | `.anyone`                        |
| The other spelling, in the binary | `.anyone` absent             | `.onion` — **0 occurrences**     |

v0.4.10.2 writes `<56-base32>.anyone` into `HiddenServiceDir/hostname`, routes
it, and **refuses the same address spelled `.onion`** (`SOCKS5 … (4)`, host
unreachable). v0.4.9.7 does the exact opposite.

So for as long as the two sides pinned different releases they could not
interoperate over a hidden service: `toon-client` accepted `.anyone` only, and
this repository `.onion` only — `const ONION_SUFFIX = ".onion"` in
`connector-config/src/peer.rs`, with `.anyone` appearing nowhere in the tree.

**That is fixed** ([issue #1284](https://github.com/toon-protocol/connector/issues/1284),
ADR 0070 as amended). `is_onion_endpoint` now matches a host ending in `.onion`
**or** `.anyone` — one rule, both spellings, because the exemption a
hidden-service host carries is earned by the address being an ed25519 key rather
than by the label after the last dot. `local/onion` runs the same v0.4.10.2
daemon this topology does.

### The workaround this topology used to need

`connector.toml` still sets `peer_allow_plaintext_endpoints = true`, and that is
now about the **image** rather than about the rule. Before the fix it was what
let this node load and advertise `http://<addr>.anyone/ilp` at all, because

```rust
plaintext_permitted = allow_plaintext || is_onion_endpoint(url)
```

and `is_onion_endpoint` did not recognise `.anyone`. The default image below is
a **released** one that predates the fix, so the line stays until this topology
pins a release that carries it — drop it in the same change that bumps
`ANYONE_CONNECTOR_IMAGE`. It is a loopback-and-test switch that must never be
set on a deployed node, and on a node with the fix an onion-only config needs
nothing of the kind.

## Use a released connector image, not `main`

The default image is `ghcr.io/toon-protocol/connector:rust-2026.08.28.1`. That
is deliberate: `main` is ahead of what `toon-client` has vendored, and a packet
built against the older wire vectors is refused with

```
400 Bad Request: invalid packet type byte: expected 12 (PREPARE), 13 (FULFILL) or 14 (REJECT)
```

which reads like a transport fault and is not one — it reproduces with no
circuit at all, over plain loopback. `pnpm --filter @toon-protocol/client
vectors:check` in the client repo reports the drift, and
`fe996af2` (ADR 0069, "the execution condition leaves the wire") is the commit
that caused it. Point `ANYONE_CONNECTOR_IMAGE` at a build of `main` once the
client has vendored that change.

## What the rehearsal asserts

`pay.mjs` runs on `anyone-payer` and goes further than "it fulfilled", for the
reason `local/onion`'s sender does: a fulfil alone cannot see a claim that
repeats itself.

1. **`describe`** over the circuit — the free GET that bootstraps everything,
   before anything is paid for.
2. **`price`** is asked for, never derived.
3. **`channel.open`** on chain, which does _not_ ride the circuit here (see
   `proxyRpc` in `pay.mjs`).
4. **A paid packet** — sealed payload, signed claim, 32-byte fulfilment, and the
   app's answer coming back through the connector.
5. **A second packet**, whose nonce must **strictly advance** the watermark.
6. **The connector's own watermark**, asked over the circuit, must equal what
   the payer signed. Two independent records of one channel, reconciled rather
   than assumed.

## The isolation, and why it is structural

- `connector`, `stub-app` and `anon-b` are on `anyone-app`; the payer and
  `anon-a` are on `anyone-payer`.
- `connector` publishes **no host port**.
- From `anyone-payer`, a direct dial to the connector times out and the name
  `connector` does not resolve — docker's embedded DNS is per network. Both were
  checked.
- `anvil` joins both, because the payer opens a channel on the chain the
  connector settles on. A container on two bridges forwards nothing between
  them.

## Two traps this cost an afternoon to find

**`anon` resolves `HiddenServicePort`'s target when it PARSES its config.** A
container name that does not exist yet aborts the daemon before it runs — and it
aborts by crashing:

```
[warn] Unparseable address in hidden service port configuration.
free(): invalid size
```

That is unavoidable here, because the connector's config names an address only
the daemon can generate, so the daemon must start first. `anonrc-b` names a
**fixed IP** instead, and `compose.yml` pins the connector to it.

**No image is published for v0.4.10.2** — ghcr's `ator-protocol` tags stop at
v0.4.9.7. `../anon-image/` builds one by overlaying the official release binary
(sha256-verified, the same figure `toon-client` records) onto that image. The
binary is statically linked, so nothing else has to come with it.

## Not in `local_topologies_load.rs`, on purpose — revisit when #1284 lands

`connector.toml` here is deliberately **absent** from that test's `EVERY_CONFIG`
list, which is otherwise "every committed config under `local/`". Adding it today
would assert that this config loads, and it only loads because of
`peer_allow_plaintext_endpoints = true`. That would bake the workaround into the
test suite and make the suite go red the moment someone does the right thing and
removes it.

When [#1284](https://github.com/toon-protocol/connector/issues/1284) lands and
`.anyone` is a recognised suffix, drop the opt-in from this config and add it to
`EVERY_CONFIG` — at that point it earns its place there, because it will load for
the same reason `local/onion`'s configs do.

## Why this is not a CI gate

For `local/onion`'s reasons plus one of its own: it needs a **second
repository's build** to be present and current. `local/README.md` is explicit
that these rehearsals are run deliberately, not on every push; do not "fix" this
one's absence from the workflow either.
