# connector-rust — build and run

The Rust connector (ADR 0001) as a container image: one static binary, one
config file, no JavaScript runtime. This is the complete, no-undocumented-steps
path from a clone of this repository to a running node answering `POST /ilp`
and an authenticated `GET /metrics`.

All commands below run from the repository root.

## Two templates live here

| File                        | For                                                                                                                                                                    |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `connector.toml`            | **This walkthrough.** Fill it in and run a node. Its placeholders are invalid on purpose so a half-edited file refuses to start rather than starting wrong (ADR 0009). |
| `connector.production.toml` | **A skeleton for a tier that does not exist.** Do not fill it in. It documents what standing production up would require, and every value in it is invalid on purpose. |

The second one needs a word of warning, because it looks like the first. There
is no production tier: no machine, no mainnet contract, no key, no deploy
([ADR 0056](../../docs/adr/0056-production-is-a-named-empty-tier.md)). Two of
its settings cannot be filled in even in principle — `packages/contracts` has
never been deployed to an EVM mainnet, so there is no `TokenNetworkRegistry`
to name, and the Solana payment-channel program
(`2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip`) exists on devnet only, which
matters because ADR 0053 binds the settlement program into a claim's signed
message. Copying a devnet address across to "make it valid" produces a node
that boots, looks healthy, and cannot redeem a claim.
`crates/connector-bin/tests/production_skeleton_is_inert.rs` fails the build if
that happens.

## Pulling the published image

`.github/workflows/publish-connector-rust-image.yml` publishes this image to
`ghcr.io/toon-protocol/connector` on every push to `main` that touches
`crates/**`, `Cargo.toml`/`Cargo.lock`, `packages/solana-program/**`, or this
Dockerfile (also runnable manually via `workflow_dispatch`). The package is
public, so no registry login is required to pull it.

It shares the `connector` package with the retired TypeScript node (last tag
`4.0.0`, no longer published); the `rust-` tag prefix keeps the two disjoint.
This is a reversal of #487's original `connector-rust` package — see the
workflow header for why the Rust binary reclaimed the canonical name.

Tags:

| Tag                    | Meaning                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `rust-sha-<short-sha>` | Immutable — pins the exact commit the binary was built from. Use this for any deployment that needs a reproducible pin (e.g. #490's devnet overlay).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `rust-main`            | Floating — always the most recent build off `main`. Convenience only; do not pin a deployment to it.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `rust-release`         | Floating, and **not a build output** — historically a PROMOTION tag, moved only by an explicit `promote-to-fleet.yml` dispatch after checking it still booted both devnet boxes' committed `connector-rust.toml`. [ADR 0068](../../docs/adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md) retired that mechanism: neither devnet box deploys the connector from this repository any more, so nothing here moves this tag — it is frozen at whatever digest it last held. A node repository that adopts a build now pins `rust-sha-<short-sha>` or a release's `rust-<handle>` alias in its own `deploy/` bundle instead. It is still deliberately **not** pushed by the publish workflow: #990 made it move on every green `main` once, which made every merge an unvalidated deploy to the live client edge on two machines, and that is not a mistake worth repeating even with no promotion left to guard against it. See [ADR 0041](../../docs/adr/0041-a-moving-tag-carries-the-fleets-committed-config-or-it-does-not-move.md). |
| `rust-2026.08.21.1`    | Immutable — a **release handle** alias for the `rust-sha-` digest a release was cut from, applied by [`release-connector.yml`](../../.github/workflows/release-connector.yml) and never moved. UTC date, then that day's ordinal. Deliberately not semver (see below). This is the tag a node repository pins to adopt a specific release — see [ADR 0055](../../docs/adr/0055-a-release-is-one-dispatch-and-the-ordering-rides-as-data.md) and [ADR 0068](../../docs/adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |

There is no semver tag series here: no crate under `crates/` has a release
process yet, and inventing one for the image alone would claim a stability
contract the binary hasn't earned. The retired TypeScript image had a
semver/cosign contract; that image is no longer published, and this one does
not (yet) carry an equivalent.

The release handle is what a version number would otherwise have been, minus
the promise: `2026.08.21.1` says when this state of the world was cut and in
what order, and nothing about compatibility. [ADR 0055](../../docs/adr/0055-a-release-is-one-dispatch-and-the-ordering-rides-as-data.md)
is the record for the handle shape; its deploy-ordering mechanism
(`config-change-required: true|false` on the release, read by
`promote-to-fleet.yml`) is retired by
[ADR 0068](../../docs/adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md) —
a node repository now owns its own deploy ordering when it bumps its pin.
`package.json`'s `"version": "3.3.0"` is TypeScript-era residue belonging to
`packages/`, not this binary's version.

```bash
docker pull ghcr.io/toon-protocol/connector:rust-sha-<short-sha>
```

A `local-stack/` bundle used to sit here — the connector plus the published
relay image plus a host `anvil`, driven end to end. It is deleted. It was
app-layer by construction (its subject was a paid write landing on a _relay_),
it pinned a relay image by sha that would rot, and its chain lived on the host
behind a hand-run Python TCP forwarder because `anvil` binds loopback. Local
composition of a connector with an app belongs in the app's own repository;
this repo builds only the connector image. `git log --diff-filter=D --
deploy/connector-rust/local-stack` if you need to read what it did.

What replaced it is [`local/`](../../local/README.md), and the difference is
the point: it is connector-layer only — the app behind its routes is the
image's own `stub-app` — its chains are ordinary compose services the
connector reaches by name, and it is a CI gate rather than a demonstration.
`make local-verify LOCAL_TOPOLOGY=<solo|two-hop|mixed-chain|dealing>` builds
this image, runs it against those chains, sends a real packet and asserts the
outcome. `LOCAL_TOPOLOGY=onion` is a fifth, run by hand rather than on the
gate: it needs a real onion daemon and a working anonymity network (ADR 0070).

Skip to [step 6](#6-run-it) to run it — the config/key setup in steps 1-4
below is identical whether you built the image locally or pulled it.

### Cutting a release

One dispatch, and everything after it is automated — build, handle, image tags,
GitHub Release:

```bash
gh workflow run release-connector.yml \
  -f reason="claim-state fix, verified on the relay"
```

It is `workflow_dispatch` **only**, and must stay that way. A green merge to
`main` reaches GHCR and stops there: the connector is the client edge on both
devnet boxes, so one bad digest reaching either unreviewed takes that box's
paid-write path dark (ADR 0041 Decision 3). Auto-on-green for this image
shipped once (#990) and was reverted.

**A release does not deploy.** It ends at the GitHub Release, which names the
`rust-sha-<short-sha>` tag and its `rust-<handle>` alias. A node repository
(`toon-protocol/relay`, `toon-protocol/store`) adopts a build by bumping its
own pinned connector tag, in one place in its own `deploy/` bundle, as its own
reviewed change ([ADR 0068](../../docs/adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md)).
That is also where the deploy ordering lives now: when a build needs a config
key the box does not yet have, the node repo lands the config and bumps the pin
in that order. `docs/operators/fleet-release-and-health.md` is the procedure.

## 1. Generate a signer key

The connector signs claims and settlement transactions with this key
(ADR 0012). `key_file` accepts either 32 raw bytes or 64 hex characters:

```bash
openssl rand -hex 32 > deploy/connector-rust/signer.key
chmod 600 deploy/connector-rust/signer.key
chown 10001:10001 deploy/connector-rust/signer.key   # see below
```

The image runs as `connector` (uid/gid **10001**, set by the Dockerfile's
`adduser -D -u 10001` / `USER connector`), and a bind-mounted file keeps its
_host_ ownership inside the container -- a `:ro` mount does not make it
readable. So a key written by root at the natural `0600` is unreadable to the
process that needs it, and the container restart-loops on:

```text
failed to read signer key_file at /app/data/signer.key: Permission denied (os error 13)
```

which is what happened deploying the devnet apex in #492. `chown 10001:10001`
fixes it without widening the mode. Do not reach for `chmod 644` instead:
this is the key the connector signs claims and settlement transactions with
(ADR 0012), and it should stay readable by exactly one uid.

This applies to the key file wherever it lives, under whatever name. The
devnet overlays under `infra/linode-store/` and `infra/linode-relay/` mount
`./signer-rust.key` rather than this directory's `signer.key` -- same
generation, same `chmod`, same `chown`, different path:

```bash
openssl rand -hex 32 > infra/linode-store/signer-rust.key
chmod 600 infra/linode-store/signer-rust.key
chown 10001:10001 infra/linode-store/signer-rust.key
```

## 2. Generate an operator bearer token

Every read on the operator surface (peers, routes, channels, claims,
exposure, identity, the audit log, and `GET /metrics`) requires this token
(ADR 0008):

```bash
openssl rand -hex 32
```

Paste the output into `deploy/connector-rust/connector.toml`'s
`operator.bearer_token`, replacing the `CHANGE-ME-...` placeholder.

> **If the config you are writing will be committed, use the file form
> instead** (issue #1003): write the token to a file, name it as
> `bearer_token_file = "/app/data/operator-bearer-token"`, and bind-mount the
> file at that path (`chmod 600`, `chown 10001:10001`). This quickstart's
> `connector.toml` keeps the literal because it is a local template that
> never holds a real token; every deployed config in this repo is public, and
> a literal there is a credential in a public repository. Setting both forms
> is refused at load — exactly one of them says where the token comes from.
> See `infra/linode-store/connector-rust.toml`'s `[operator]` header for the
> deployed shape.

## 3. Generate an operator write key

Every write (currently `POST /packets`) requires an RFC 9421 signature from
an ed25519 key on this allowlist -- no bearer token is ever sufficient to
move value. Generate a key pair and extract its raw 32-byte public key as
64 hex characters:

```bash
openssl genpkey -algorithm ed25519 -out deploy/connector-rust/operator.pem
openssl pkey -in deploy/connector-rust/operator.pem -pubout -outform DER \
  | tail -c 32 | od -An -tx1 | tr -d ' \n'
```

Paste that hex string into `connector.toml`'s `operator.write_keys` array,
replacing the placeholder (which is deliberately not 64 hex characters, so
the file refuses to load until you do this). Keep `operator.pem` -- it is
the private key an operator client signs writes with; nothing in this
directory reads it automatically.

> The file form is `write_keys_file = "/app/data/operator-write-keys"`, one
> 64-hex public key per line with `#` comments (issue #1003). Public key
> material, so this one is not about secrecy: ADR 0008 revokes write
> authority by removing a key and restarting, and behind a file that is an
> edit on the box rather than a pull request and a promotion. Use it for
> anything deployed.

## 4. Edit the route(s)

`connector.toml`'s `[[routes]]` block points at an example app
(`http://app:3100`). Replace `prefix` and `handler_url` with your own app's
ILP address and handler URL, or delete the block if this node only peers.
`price` is required on a terminated route — write `price = 0` if free is
deliberate, because it is never silently free.

To peer, set `peer_expose` (which peer carriages this node accepts _peer_
traffic on — it opens no port, and a node that leaves it at its `neither`
default still serves clients over BTP exactly as before), add a `[[peers]]`
entry with a `wss://` or `https://` `endpoint` and a `credential`, a
`[[peer_channels]]` row binding it to a channel, and a
`[[routes]]` entry that names the peer's `id` instead of a `handler_url`.
The template's commented peering block annotates every field. ADR 0027
deleted the raw-TCP transport that preceded this (issue #679), along with
`peer_wire_addr` and the `SocketAddr`-shaped `[[peers]].addr`; a config
still setting either now fails config load by name.

**A `[[routes]]` entry naming a peer also needs a `[[pay_channels]]` row for
that peer** (issue #1145). A connector covers every PREPARE it sends (ADR
0042), and there is no longer an uncovered path for a forward to take, so a
route to a peering with no channel to pay it from is refused at config load
by name — the node does not start. A peering this node only _accepts_ on
needs no such row; the requirement is keyed on the route.

That key became required rather than optional, which by ADR 0009 makes it a
**breaking deploy**: land the config carrying the row first, then move the
image tag. Neither box on this fleet forwards to a peering today (issue
#872 removed both peerings), so nothing deployed is affected — but the
ordering rule holds the moment one is added back.

Write the credential as a **`secret_file`**, not a literal:

```toml
credential = { secret_file = "/app/data/store-peer.secret" }
```

```sh
openssl rand -hex 32 > /app/data/store-peer.secret   # both sides need the same bytes
chmod 600 /app/data/store-peer.secret
```

That keeps the peering itself in a config you can commit while the secret
stays on the box — the same shape `[signer] key_file` and the settlement
keys already use, and `deploy/connector-rust/*.secret` is gitignored for it.
The path is resolved the way `key_file` is, so make it absolute and make it
a path _inside_ the container. It is read at startup and trimmed of trailing
whitespace; missing, unreadable or empty stops the node by name
(`PeerSecretFileNotFound` / `PeerSecretFileUnreadable` /
`PeerSecretFileEmpty`). A literal `secret` still works and is fine for a
config nobody commits; setting both is `PeerCredentialAmbiguous`.

Two things to get right, both of which fail silently rather than loudly:

- **`[[peers]].id` is one string both operators write.** It is the `peerId`
  the dialing side presents, and the accepting side proves it against its
  own `[[peers]]` table — so an id the far side does not have is admitted
  as an ordinary client, with nothing in the log.
- **There is no peer port.** A peer's `endpoint` is
  `wss://<host>/ilp/btp` or `https://<host>/ilp` — this node's own client
  edge — because peer carriages ride the listener it already serves and
  role is decided by the credential, not by the port. Those are the same
  two URLs an ordinary client uses, and a client uses them **without a
  credential**: `GET /ilp/btp` accepts a session that presents none (or an
  empty `secret`) and keeps it a client, and what pays for a write is the
  signed claim on each frame, never the session
  (`docs/protocol/client-edge-spec.md` §1.9 step 1). The `credential` here
  buys peer role, not entry.

A packet routed to a peer this node cannot dial is still answered
`T01 peer unreachable`, never dropped. See
`docs/operators/btp-peer-transport-bringup.md`.

## 4b. (Optional) settlement

Omit `[settlement]` entirely and the node routes and charges normally, but
every channel operation on the operator surface answers `503` — there is no
backend to run it. Configure it and the node resolves the
`TokenNetworkRegistry` at `contract_address` **at startup**, so a wrong RPC
URL or a registry that does not answer `getTokenNetwork(token)` is an
exit-1, not a runtime surprise. Only `chain = "evm"` is accepted today.

`infra/linode-store/connector-rust.toml` carries a commented, annotated
`[settlement]` block to copy from; `crates/connector-bin/tests/devnet_configs_load.rs`
boots exactly that block against a freshly deployed registry on anvil, so it
is a template that is known to load.

## 4c. Durable claim state

`connector.toml` sets `state_dir = "/app/state"`. That directory holds the
append-only claim journals — `client-edge-claims.log` and `peer-claims.log` —
whose replay is what makes a claim's **replay watermark** survive a restart.
Without it the watermarks live only in process memory: a restart resets every
channel to "no claim ever seen", and a channel with no watermark accepts any
nonce, so every claim a client has already spent becomes free service again
(issue #605).

It must be a **volume**, not a path inside the container's writable layer — a
watermark that dies with the container is the same defect one indirection
down. The image runs as uid `10001`, and it ships `/app/state` already owned
by that uid — that pre-existing path is what a fresh **named volume** inherits
its ownership from on first mount, so a named volume needs no manual `chown`.
(An image built before this path existed initializes the volume root-owned
instead, and the node refuses to start on it.) A host bind mount inherits
nothing: `chown 10001:10001` it first, exactly like `signer.key` in step 1.

Two things fail closed here rather than degrading quietly:

- a config with `[[client_channels]]` but no `state_dir` **does not load**;
- a `state_dir` the node cannot write, or a journal it cannot replay (a
  corrupt line), is an **exit 1 at startup**, naming the path.

## 5. Build the image

```bash
docker build -f deploy/connector-rust/Dockerfile -t connector-rust:local .
```

For both target platforms:

```bash
docker buildx build --platform linux/amd64,linux/arm64 \
  -f deploy/connector-rust/Dockerfile -t connector-rust:local .
```

## 6. Run it

```bash
docker run --rm \
  -p 3000:3000 \
  -v "$(pwd)/deploy/connector-rust/connector.toml:/app/config/connector.toml:ro" \
  -v "$(pwd)/deploy/connector-rust/signer.key:/app/data/signer.key:ro" \
  -v connector_rust_state:/app/state \
  connector-rust:local
```

(The image's `CMD` already points at `/app/config/connector.toml` --
nothing more to pass on the command line.)

`-v connector_rust_state:/app/state` is the `state_dir` from step 4c. Drop it
and the node still starts, but every claim watermark it holds dies with the
container — which is the whole of issue #605. In a compose file the equivalent
is a `connector_rust_state:/app/state` entry under `volumes:` for the service,
plus a top-level `volumes: { connector_rust_state: }` — see
`infra/linode-store/docker-compose.store.rust.yml`.

## 7. Verify

```bash
# The client edge answered (400: an empty body isn't a valid PREPARE).
curl -s -o /dev/null -w '%{http_code}\n' -X POST http://localhost:3000/ilp

# The identity a sender seals a packet's payload to (ADR 0018).
curl -s http://localhost:3000/ilp/identity
# → {"keyId":"...","publicKey":"0x04..."}

# The price of the route you configured in step 4 (404 if nothing matches).
curl -s 'http://localhost:3000/ilp/routes/price?destination=g.example.connector.app'
# → {"destination":"g.example.connector.app","price":100}

# The operator surface requires the token you generated in step 2. It shares
# this one port -- there is no separate admin port, and no health endpoint.
curl -s -o /dev/null -w '%{http_code}\n' http://localhost:3000/metrics        # 401
curl -s http://localhost:3000/metrics -H "Authorization: Bearer <token-from-step-2>"
```

The last command prints the decided metrics surface (ADR 0014):
`toon_packets_total`, `toon_packets_rejected_total`, `toon_fees_earned_total`,
`toon_exposure` and `toon_settlement_total`.

## Logs

The connector logs structured JSON (one object per line) to stdout. Every
line logged while handling a packet -- received, routed, delivered or
forwarded, fulfilled or rejected -- carries the same `correlation_id`: the
packet's execution condition, hex-encoded. Because that condition is
invariant across every hop a packet passes through, the same
`correlation_id` appears in the logs of every connector that handled the
packet, so `jq 'select(.fields.correlation_id == "...")'` against either
node's log stream (or both, once aggregated) recovers that packet's whole
path. Set `RUST_LOG` (e.g. `RUST_LOG=debug`) to change verbosity; it is not
a config file field (ADR 0009 has no environment-variable layer for
anything the connector's _behavior_ depends on, but log verbosity is an
operational knob, not a behavioral one).
