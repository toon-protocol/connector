# Technology Stack Overview

The connector is a **Rust** Cargo workspace producing one binary. It was previously a
TypeScript/Node.js project; that implementation was a prototype and has been removed
([ADR 0017](../adr/0017-the-typescript-connector-is-a-prototype.md)). Nothing on this page
describes it — for what it used, read this file's history.

## Core

- **Language:** Rust, edition 2021, stable toolchain (CI pins nothing tighter than
  `dtolnay/rust-toolchain@stable`).
- **Workspace:** one Cargo workspace, `crates/*` alone (issue #1386 removed the one other
  member, `packages/solana-program`) ([ADR 0001](../adr/0001-rust-workspace-library-first.md) —
  library-first: the binary is a thin shell over crates that are usable and testable without it).
- **Async runtime:** `tokio` 1.x. Each crate enables only the features it needs; only
  `connector-bin` takes `rt-multi-thread`.
- **Errors:** `thiserror` 2 — every crate defines its own typed error enum. No `anyhow` anywhere.

## HTTP and networking

- **Server:** `axum` 0.6. The client-edge and operator routers are `axum::Router` values, mounted
  and merged by `connector-cli`, so neither owns a port or a process.
- **Client:** `reqwest` 0.11 with `rustls-tls` and `default-features = false` — no OpenSSL in the
  build, which is what lets the container image be an Alpine build.
- **Peer transport:** none shipped. The raw-TCP transport was deleted in issue #679; per
  [ADR 0027](../adr/0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md)
  peers ride the carriages the client edge already serves — BTP (RFC-0023) over `wss://` or
  ILP-over-HTTP over `https://` — so peering adds no transport dependency of its own. The
  `PeerTransport` port remains; the semantics it carries are
  [`docs/protocol/peer-semantics-pre-868.md`](../protocol/peer-semantics-pre-868.md) §3–§6.

## Cryptography

All of it lives in `connector-signer`; no other crate takes a crypto dependency except
`connector-operator`, which verifies RFC 9421 write signatures.

- **secp256k1:** `libsecp256k1` 0.6 — identity keys, ECDH for the gift wrap, EVM claim signatures.
- **Ed25519:** `ed25519-dalek` 1 — Solana claim signatures, and operator-write signatures.
- **Hashes:** `sha2` 0.10 (conditions, RFC 9530 digests), `sha3` 0.10 (keccak for EIP-712).
- **Sealing:** `chacha20poly1305` 0.10 + `hkdf` 0.12 — the gift wrap
  ([ADR 0018](../adr/0018-a-payload-is-sealed-to-the-terminating-connector.md)).

## Chains

- **EVM:** `ethers` 2 (`rustls`, no OpenSSL), bound to x402's `x402BatchSettlement`
  ([ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)) — a
  constant of the binary, not a contract this repository builds or deploys. Local chain:
  `anvil`, seeded with committed bytecode; `forge` (Foundry, pinned to `v1.7.1` in CI) is still
  used to build `connector-rate-source-evm`'s TWAP test fixture.
- **Solana:** `solana-client` / `solana-sdk`, both pinned to `=2.1.0`. Local chain:
  `solana-test-validator`.
- **Mina:** not supported ([ADR 0002](../adr/0002-drop-mina-from-the-rust-connector.md)).

## Configuration, logging, metrics

- **Config:** `toml` 0.8 + `serde`. One file, `deny_unknown_fields`, validated at boot, immutable
  after ([ADR 0009](../adr/0009-one-typed-config-file-no-environment-layer.md)). No env-var layer;
  `RUST_LOG` is the only variable the binary reads.
- **Logging:** `tracing` + `tracing-subscriber` with the `json` and `env-filter` features.
  Structured JSON on stdout, packet-correlated by spans
  ([ADR 0014](../adr/0014-metrics-surface-and-packet-correlated-logs.md)).
- **Metrics:** `prometheus` 0.13, served as text from the bearer-gated `GET /metrics`.
- **Read-mostly state:** `arc-swap` — the routing table is a swapped snapshot rather than a lock
  ([ADR 0015](../adr/0015-read-mostly-state-is-a-swapped-snapshot.md)).

## Testing

- **Unit and integration:** the built-in `cargo test` harness. No test framework crate.
- **Properties:** `proptest` 1, in `connector-domain`, `connector-runtime` and `connector-signer`
  — the crates whose invariants generate `vectors/wire-vectors.json`
  ([ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md)).
- **Fakes:** `hyper` 0.14 and `tempfile` as dev-dependencies, for in-process app servers and
  scratch key files. Fakes, never mocks
  ([ADR 0007](../adr/0007-testing-doctrine-fakes-yes-mocks-no.md)).
- **Lint and format:** `cargo fmt` and `cargo clippy -- -D warnings`, both gating.

## What is still JavaScript

`packages/faucet` (plain JS), `packages/announcer` (TypeScript) and `tools/fund-peers`
(TypeScript) are devnet tooling, not the connector. They are why `package.json`, ESLint,
Prettier and Husky still exist at the root. `npm test` runs those; it does not touch the
connector.
