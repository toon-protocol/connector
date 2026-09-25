# Source tree

The map of this repository: what each directory is, what it is for, and — for the
directories that are not the connector — why it is here at all.

The connector is the Cargo workspace under `crates/`, built as the `connector` binary.
Nothing else in this repository is the connector. The npm-workspace layout this page once
described (`packages/connector`, `packages/shared`, `tools/send-packet`) went with the
TypeScript prototype — [ADR 0017](../adr/0017-the-typescript-connector-is-a-prototype.md).

## Top level

```
connector/
  crates/          the connector — eighteen crates producing the `connector` binary
  packages/        not the connector: Solidity contracts, the Solana program, devnet tooling
  tools/           scripts: CI guards, contract and chain helpers, the RFC vendoring script
  local/           the shipped image run against real containerised chains
  infra/           the devnet boxes and local chain provisioning
  deploy/          the image and the deployment recipe
  docs/            ADRs, protocol specs, operator runbooks, vendored RFCs
  vectors/         wire-vectors.json, the normative cross-repo contract (ADR 0021)
  Cargo.toml       the workspace manifest — crates/* plus packages/solana-program
```

## `crates/` — the connector

Layered, and the layering is enforced by which crate may depend on which
([ADR 0001](../adr/0001-rust-workspace-library-first.md)). Nothing below depends on anything
above it. Two of those constraints are load-bearing and are asserted by the crates' own
dependency lists rather than by convention:

- **`connector-domain` is pure.** No async, no I/O, no clock, no keys. Everything in it is
  property-testable without a filesystem or a runtime, and that is why the packet rules,
  the claim rules and the balance projection live there rather than beside the code that
  performs the I/O.
- **`connector-signer` is the only crate that touches key material.** No other crate in the
  workspace holds a key or performs a signing operation; anything that needs to sign takes
  a `&dyn Signer`. Key material is referenced by location, never by value
  ([ADR 0009](../adr/0009-one-typed-config-file-no-environment-layer.md),
  [ADR 0012](../adr/0012-a-signer-and-a-treasury-not-a-wallet.md)) — `connector-config`
  validates that a `SecretLocation` exists and never reads its contents.

```
connector-domain                 pure logic: no async, no I/O, no clock, no keys
  ├─ packet.rs, oer.rs           ILPv4 packets (RFC-0027) and their canonical OER
  │                               encoding (RFC-0030, ADR 0023)
  ├─ address.rs, route.rs        ILP address validation (RFC-0015); longest-prefix selection
  ├─ fee.rs                      flat per-packet fee arithmetic (ADR 0010), and the
  │                               converting forward and its inverse (ADR 0071)
  ├─ rate.rs, rate_table.rs      a rate as a rational over base units, and the table a
  │                               forward reads it from under three guards (ADR 0071)
  ├─ price.rs                    what a terminated route charges for one packet: a
  │                               schedule over payload length, flat when its slope
  │                               is zero (ADR 0065)
  ├─ condition.rs                condition / fulfilment / expiry rules
  ├─ claim.rs, client_claim.rs   nonce, watermark and value rules (ADR 0004, ADR 0005),
  │                               and a voucher's amount-only watermark (ADR 0074)
  ├─ projection.rs               balances folded from journal entries (ADR 0005)
  ├─ envelope.rs                 the request/response envelope a terminated packet
  │                               carries to and from the app (ADR 0018)
  ├─ x402.rs                     the `payment-required` greeting's wire shape and its reader
  ├─ node.rs                     the node self-description (ADR 0050)
  └─ identity.rs                 client-edge sender identity resolution

connector-signer                 the only crate that touches key material
  ├─ signer.rs, local.rs, kms.rs the `Signer` port and its backends
  ├─ ed25519_signer.rs           the Solana counterpart port, deliberately not folded in
  ├─ giftwrap.rs                 seal/open, and the derived fulfilment (ADR 0018, ADR 0019)
  ├─ claim_signature.rs          EIP-712 and Ed25519 balance-proof verification
  │                               (ADR 0024, ADR 0053)
  ├─ voucher_signature.rs        x402 batch-settlement voucher verification (ADR 0074)
  ├─ claim_state_challenge.rs    "prove you hold this channel", moving no value
  ├─ nip59.rs                    the wrapped-claim transport-privacy envelope
  └─ contract.rs                 the `Signer` contract suite

connector-config                 one typed TOML file and every refuse-to-start error
                                  (ADR 0009); a removed key is parsed in order to be
                                  refused by name, never silently ignored

connector-chain-rpc              the HTTP transport every settlement table's RPC clients
                                  share (ADR 0073): the timeouts, the 403/429 retries, and
                                  the optional pinned circuit through `socks_proxy`. Holds
                                  no key and knows no channel

connector-settlement             the chain-agnostic settlement port + its contract suite
  ├─ port.rs, contract.rs        the port, and the one suite every backend is run against
  ├─ in_memory.rs                the fake — the first implementation to pass that suite
  └─ batch/                      a second, receive-only port for x402 batch-settlement
                                  channels a client opens (ADR 0074 decision 9): admit,
                                  read collateral and lifecycle, land a voucher. Its own
                                  port.rs, contract.rs and in_memory.rs, in the same shape,
                                  and held.rs: where the watchers read the latest voucher
connector-settlement-evm         real EVM backend: TokenNetworkRegistry → TokenNetwork,
                                  holding no local channel state; every method reads the
                                  chain fresh
  ├─ batch_settlement.rs         the batch-settlement port over x402's
  │                               x402BatchSettlement (ADR 0074): admits a presented
  │                               ChannelConfig, `claim`s from the settlement key.
  │                               contracts/x402/ holds its ABI and the pinned bytecode
  │                               the tests place on anvil, with PROVENANCE.md
  └─ batch_watch.rs              its watcher and sweep (ADR 0074 decision 5): claims at
                                  once on a WithdrawInitiated, and periodically claims
                                  every channel in one `claim`, then `settle`s
connector-settlement-solana      real Solana backend, speaking packages/solana-program's
                                  own wire directly (that crate builds for SBF only and
                                  exports no client SDK)
  ├─ batch/                      the batch-settlement port on solana-foundation's
  │                               payment-channels (ADR 0074): admission, settle and
  │                               settle_and_seal, and that program's own wire; sweep.rs
  │                               is its watcher, which rediscovers every sponsored
  │                               channel and seals, distributes and reclaims it
  └─ fixtures/payment_channels.so  that program's mainnet-beta binary, which tier-3
                                  tests load into genesis at its canonical id

connector-rate-source            the chain-agnostic rate-source port + its contract suite
                                  (ADR 0071): what reading a token's price off a market
                                  means, with no chain, venue or RPC in it
  ├─ port.rs, contract.rs        the port, and the one suite every reader is run against
  └─ in_memory.rs                the fake — the first implementation to pass that suite
connector-rate-source-evm        the first real reader: Uniswap-v3-compatible `observe()`
                                  TWAPs over an EVM RPC endpoint, TWAP-only and over a
                                  pool the operator named. Holds no key, sends no
                                  transaction, and is no part of any settlement path

connector-runtime                the packet plane and its ports
  ├─ connector.rs                Connector — routing, delivery, fees, rejects
  ├─ peer_transport.rs           the peer transport port (ADR 0027's seam)
  ├─ app_client.rs               the port to the payment-oblivious app behind a
  │                               terminated route
  ├─ claim.rs, journal.rs        ClaimBook, exposure and the durable journal (ADR 0005)
  ├─ route.rs, peer_route_store.rs  leased routes (memory-only, TTL) and the durable
  │                               runtime-mutable peer/route table
  ├─ peering.rs, self_description.rs  establishing a peering from a URL (ADR 0058) by
  │                               reading the other node's self-description (ADR 0050)
  ├─ outbound_client.rs          paying a next hop as an ordinary client of it
  ├─ attribution.rs              what a terminating connector tells the app about the
  │                               payment (ADR 0040)
  ├─ rate_table.rs, rate_poller.rs  the handle a converting forward reads its rate off,
  │                               and the background poller that refreshes it — the
  │                               packet path does no I/O for a rate (ADR 0071)
  ├─ clock.rs                    the clock as an injected port, so expiry is tested by
  │                               advancing rather than sleeping
  └─ metrics.rs, operator_view.rs   ADR 0014's metrics; ADR 0008's read models

connector-btp                    the BTP frame codec and session framing (RFC-0023),
                                  transport- and role-neutral; depends on no other
                                  connector crate and knows nothing of claims, routes,
                                  prices or refusals
connector-peer-auth              role-by-authentication: peer or client, decided from a
                                  verified claim on a configured `[[peer_channels]]` row
                                  and configuration alone. Two states, no `Unknown`
                                  (ADR 0027's stop-ship invariant; ADR 0060 deleted the
                                  shared secret that used to decide it)
connector-peer-btp               the BTP peer carriage: dial/accept a peering over wss://
connector-peer-http              the ILP-over-HTTP peer carriage: dial/accept over https://

connector-client-edge            axum Router, mountable rather than a server:
                                  POST /ilp, GET /ilp (the self-description, ADR 0050),
                                  GET /ilp/btp, POST /ilp/probe, GET /ilp/identity,
                                  GET /ilp/routes/price, POST /ilp/claim-state
connector-operator               axum Router: bearer-gated reads, RFC 9421-signed writes
                                  (ADR 0008), each accepted signature retained as its
                                  write's audit record; GET /dashboard, the operator
                                  dashboard embedded from dashboard.html (ADR 0066)
connector-cli                    config → runtime → merged routers → bound listeners,
                                  plus the `send` verb; the binary itself branches on
                                  nothing
connector-bin                    bin/connector, bin/stub-app — and the workspace's
                                  cross-cutting integration tests (see below)
connector-vectors                bin/generate-vectors → vectors/wire-vectors.json
```

### Test layout

There is no separate test tree for unit tests. Following Rust convention:

- **Unit tests** live in a `#[cfg(test)] mod tests` at the bottom of the module they test.
  `proptest` properties live beside them.
- **Contract suites** — the one place a port's behaviour is defined — live with the port and
  are run against every implementation, fake and real
  ([ADR 0007](../adr/0007-testing-doctrine-fakes-yes-mocks-no.md)).
  `connector-settlement`'s `assert_upholds_the_contract` is the model.
- **Integration tests** are their own binaries under `crates/<crate>/tests/`. Several drive a
  real chain, spawning a disposable `anvil` or `solana-test-validator` per test rather than
  dialling a container. They skip when the binary is absent locally but **panic** when `CI`
  is set, so the gate cannot go green without one.
- **Repository-wide assertions** live in `crates/connector-bin/tests/`, because they are
  properties of committed files rather than of a library: the devnet and `local/` configs
  load through the real `Config::load`; the production skeleton stays inert
  (`production_skeleton_is_inert.rs`); a vendored RFC body is unmodified
  (`vendored_rfcs_are_unmodified.rs`); the release pipeline holds its shape
  (`fleet_release_gate.rs`).
- **Vectors** are generated, not written: `crates/connector-vectors` emits
  `vectors/wire-vectors.json`, and `crates/connector-vectors/tests/vectors_up_to_date.rs`
  fails the gate if the committed file is stale.

## What is not the connector

### `packages/`

| Directory        | What it is                                                                                                                                                                                                                                                                                                                                                                                                             |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `contracts`      | Solidity, Foundry. `TokenNetwork`, `TokenNetworkRegistry`, `RollingSwapChannel` — what `connector-settlement-evm` binds to. Its own `forge test` job (`.github/workflows/contracts.yml`); no `make` target runs it. The repository's only git submodules (OpenZeppelin, forge-std) live here.                                                                                                                          |
| `solana-program` | The SPL-token, PDA-addressed payment-channel program `connector-settlement-solana` drives. Crate name `payment-channel`; a Cargo workspace member **excluded** from the workspace test gate, with its own `cargo test-sbf` CI job and a separate build-reproducibility job.                                                                                                                                            |
| `faucet`         | Devnet token faucet service (plain JavaScript).                                                                                                                                                                                                                                                                                                                                                                        |
| `announcer`      | A standalone `kind:10032` announcer sidecar. It is not the connector and never was: it never links against connector crates, never reads connector config and never runs in the connector process. It only asks the client edge's already-public answers and republishes them ([ADR 0022](../adr/0022-a-connector-answers-it-does-not-announce.md), [ADR 0006](../adr/0006-the-connector-is-mechanism-not-policy.md)). |

**Mina is not in this repository at all.**
[ADR 0002](../adr/0002-drop-mina-from-the-rust-connector.md) dropped it as a settlement chain —
the zkApp's methods need proof generation through o1js, which exists only in JavaScript, and a
Node sidecar beside the binary was refused — and
[ADR 0065](../adr/0065-mina-leaves-the-repository.md) then deleted the zkApp, the browser faucet
dApp and the Mina tooling that record had left standing. The Cargo workspace has no Mina crate
and the npm workspaces have no Mina package. What remains is the connector refusing a `mina`
claim by name, which is wire behaviour, not Mina support.

### `tools/`

Scripts, none of them part of the binary.

- `ci/check-tracked-secrets.sh` — the tracked-key guard, by filename **and** by content
  (a Solana keypair is a bare 64-byte JSON array and can be called anything).
- `contracts/init-libs.sh` — the Foundry submodules.
- `solana/build-sbf.sh`, `solana/deploy.sh` — building and deploying the payment-channel
  program.
- `fund-peers/` — devnet peer funding tooling (TypeScript).
- `bench/peer-claim-journal-fsyncs.sh` — a one-off measurement script.
- `vendor-rfc.sh` — re-vendors an Interledger RFC into `docs/rfcs/`
  ([ADR 0062](../adr/0062-an-rfc-is-vendored-verbatim-and-profiled-never-forked.md)).

### Why npm still exists

`packages/announcer`, `packages/faucet` and `tools/fund-peers` are the npm workspaces named in
`package.json`, and they are the only reason npm and `package.json` are still in this
repository. **`npm test` does not test the connector** — it runs those packages, each with its
own runner (`node --test` for the faucet, `tsx --test` for the announcer; the one Jest project
went with `packages/mina-zkapp`, [ADR 0065](../adr/0065-mina-leaves-the-repository.md)). The
connector's gate is `cargo test --workspace --exclude payment-channel`.

## `local/` — the shipped image against real chains

One connector image, real containerised chains, a real packet. It is a **separate gate from
`cargo test`**, not a duplicate of it: nothing under `crates/` dials `localhost:8545` or
`localhost:8899`, and starting a chain container before `cargo test` changes nothing. What
`cargo test` structurally cannot check is that **the image** — as uid 10001, with a mounted
`connector.toml`, mounted key files and a real volume at `/app/state` — boots and moves a
packet. That is this, and only this.

| Topology       | Nodes | What it proves                                                                                                                                                                                                                                          |
| -------------- | ----- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `solo/`        | 1     | The image boots on a mounted config with **both** settlement backends live at once, and a real packet reaches the app behind its route.                                                                                                                 |
| `two-hop/`     | 2     | Two images peered over ILP-over-HTTP on anvil. B prices the route it terminates; A covers each crossing with a real EIP-712 claim on a real funded channel.                                                                                             |
| `mixed-chain/` | 3     | A↔B on EVM over BTP, B↔C on Solana over ILP-over-HTTP, B holding both backends. One packet crosses two chains and two carriages.                                                                                                                        |
| `dealing/`     | 3     | `mixed-chain`'s shape with the middle node **dealing**: 6-decimal mock USDC in, a 9-decimal mock SPL token out, converted at a declared rate (ADR 0071). The only place a shipped image converts, and the only fixture here that declares `[[tokens]]`. |
| `onion/`       | 2     | B reachable only at a hidden-service address a real `anon` sidecar generates, the two on separate docker networks with no route between them. Not on the CI gate.                                                                                       |
| `anyone/`      | 1     | The **client edge** over the same overlay: `toon-client` — a different repository's payer — discovering, pricing and paying this node over a circuit. Needs that repository built; not on the CI gate.                                                  |

`LOCAL_TOPOLOGY` picks one (`solo` is the default); `make local-verify` runs the cycle and
`.github/workflows/local-topologies.yml` runs the first four — `onion` needs a third-party
anonymity network and is deliberately off that gate (ADR 0070), and `anyone` is run by its own
`run.sh` rather than by `LOCAL_TOPOLOGY` at all. `anon-image/` builds the daemon both of those
run: ghcr publishes none for the release whose hidden-service TLD is `.anyone` (issue #1284). The peered topologies cross more than
once and then read the payee's own claim journal, because a peer claim's verdict rides back
out of band and never gates the packet — `--expect-fulfill` alone would go green over a
peering carrying traffic for free. `local/README.md` is the long version and is worth reading
before editing anything here. `keys.sh` generates and funds a topology's keys into the
gitignored `local/.keys/`, and has a second stage that opens and tops up Solana channels
through a running node's operator surface, because only a running node can submit one.

## `infra/` and `deploy/`

`infra/` provisions machines. `linode-relay/` and `linode-store/` describe the two connector
devnet boxes — bootstrap script, nginx and Let's Encrypt, compose overlays and
`connector-rust.toml` — but are fixtures now (ADR 0068): each box actually deploys from its
own repository's `deploy/` bundle (`toon-protocol/relay`, `toon-protocol/store`), and these
directories exist so `devnet_configs_load.rs` keeps booting realistic fleet-shaped configs.
`linode-faucet/` is the faucet box, and is not a fixture — it still deploys from here.
`linode/` is a retired chain box that now holds one live artefact, `endpoints.json`.
`solana/` provisions chain-side state: the deterministic mock USDC mint, treasury funding, the
local validator entrypoint.

`deploy/` is the recipe: `connector-rust/` holds the `Dockerfile`, a commented
`connector.toml` to fill in, and a README walking the key material and the first `up -d`.

Beside it, `deploy/connector-rust/connector.production.toml` looks like the same thing and is
not. Production is a **named and empty tier** —
[ADR 0056](../adr/0056-production-is-a-named-empty-tier.md): no machine, no mainnet contract,
no key, no deploy. Every value in the skeleton is invalid on purpose, and
`crates/connector-bin/tests/production_skeleton_is_inert.rs` fails the build if one is
replaced with something plausible or with a devnet value copied in "to have something valid
there".

## `docs/`

| Directory       | What it is                                                                                                                                                                                                                                                 |
| --------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `adr/`          | The numbered decisions, and the tiebreaker for everything else in this repository. `README.md` is the index — grouped by scope, with each record's own `**Status:**` line as the authority for whether it is live. Numbers are permanent and never reused. |
| `protocol/`     | The wire specs the ADRs are implemented against: `client-edge-spec.md`, `peer-carriage-spec.md`, `operator-spec.md`, `configuration-spec.md`, `packet-flow-spec.md`, `payment-spec.md`, `self-description-spec.md`, `wire-vectors.md`.                     |
| `rfcs/`         | The Interledger RFCs this connector implements — see below.                                                                                                                                                                                                |
| `operators/`    | Runbooks: box bringup, key rotation, fleet release and health, peer-channel migration, BTP peer bringup, onion-endpoint bringup, the claim-policy rollout, box reconciliation.                                                                             |
| `agents/`       | Conventions for agents working here: the issue tracker, triage labels, how to consume the domain docs.                                                                                                                                                     |
| `architecture/` | This page, plus [`tech-stack.md`](tech-stack.md) (languages, runtimes, pinned versions) and [`coding-standards.md`](coding-standards.md) (what the gate enforces, in the order it enforces it).                                                            |

Loose files under `docs/` are chain-deployment notes and one-off design records
(`evm-deployment.md`, `solana-deployment.md`, `devnet-pricing.md` and similar). They are point-in-time; the ADRs are not.

Nothing under `docs/` describes the retired TypeScript connector any more
([ADR 0017](../adr/0017-the-typescript-connector-is-a-prototype.md)): its admin-API reference,
load-testing guide, cutover runbooks, stories and changelog were deleted, and are in git history.
The two `protocol/*-pre-868.md` files are frozen records of an earlier _Rust_ money model, kept
because the ADRs that replaced it argue against them by name.

### `docs/rfcs/`

Ten Interledger RFCs — 0001, 0015, 0018, 0019, 0023, 0027, 0030, 0032, 0034, 0035 — vendored
verbatim and pinned to a single upstream commit, so the protocol being run is readable without
leaving the repository. Each file is in two halves: a **TOON profile** written by this project,
recording where this connector departs and which ADR governs the departure; then the
Interledger Foundation's text, untouched, below a `<!-- BEGIN VERBATIM UPSTREAM BODY -->`
marker.

**The body is never edited.** An alignment goes in the profile above the marker.
[ADR 0062](../adr/0062-an-rfc-is-vendored-verbatim-and-profiled-never-forked.md) argues why,
each preface records the SHA-256 of its own body, and
`crates/connector-bin/tests/vendored_rfcs_are_unmodified.rs` recomputes it in the workspace
gate. Re-vendoring from a newer upstream is `tools/vendor-rfc.sh`.

The order of authority is `vectors > ADRs > docs/protocol/ specs > a TOON profile > an RFC
body`.

**This directory is CC BY-SA 4.0, not MIT.** The bodies are © the Interledger Foundation and
contributors; the rest of the repository is MIT ([`LICENSE`](../../LICENSE)). The share-alike
term applies to `docs/rfcs/` and adaptations of it, and to nothing in `crates/` — an
implementation of a specification is not a derivative work of the specification.
[`docs/rfcs/README.md`](../rfcs/README.md) carries the pin, the statement of changes and the
list of RFCs deliberately **not** vendored.

## `vectors/`

`wire-vectors.json` is the normative cross-repo contract for the client-edge termination wire
([ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md)): reproducing these bytes is
what conformance means for `toon-client`, `rig` and `swap`. **Prose is not normative; this
file is.** It is plain JSON so a client SDK can replay it without importing anything from this
repository.

It is generated, never hand-written — each vector is produced by the real implementation and
re-checked against the real validator before being emitted. Regenerate after any change to the
envelope, the gift wrap, the fulfilment derivation or the claim signing scheme:

```bash
cargo run -p connector-vectors --bin generate-vectors
```

`cargo test -p connector-vectors`, part of the workspace gate, fails if the committed file is
stale.
