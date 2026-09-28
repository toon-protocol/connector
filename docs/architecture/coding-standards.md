# Coding Standards

The connector is Rust. The TypeScript rules this page used to carry went with the prototype
([ADR 0017](../adr/0017-the-typescript-connector-is-a-prototype.md)); they still apply to the
faucet tooling under `packages/`, which is linted by ESLint and formatted by Prettier, but not to
anything in `crates/`.

## What the gate enforces

Nothing here is a matter of taste — CI runs exactly this, in this order, and each step is
blocking:

```bash
cargo fmt --all -- --check
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

There is no `rustfmt.toml` and no `clippy.toml`: both tools run on their defaults, so formatting
and lint arguments are settled by the tool rather than by review. `-D warnings` means a clippy
warning is a build failure, including in tests and benches (`--all-targets`).

CI additionally fails the build if any `tests/*.rs` harness reports "running 0 tests" — an
integration test that compiles but is fully skipped is treated as a broken test, not a passing
one.

## Structure

- **Layers are dependency edges.** `connector-domain` depends on nothing in the workspace and does
  no I/O, holds no clock and touches no keys. Adding an async dependency to it is a design error,
  not a convenience ([ADR 0001](../adr/0001-rust-workspace-library-first.md)).
- **One crate owns key material.** `connector-signer` is the only place a secret key is read,
  held or used. Elsewhere a key is a _location_ — a `key_file` path or a `kms_key_id` — never a
  value ([ADR 0012](../adr/0012-a-signer-and-a-treasury-not-a-wallet.md)).
- **A port is a trait plus one contract suite.** Where an abstraction exists (`SettlementBackend`,
  `PeerTransport`, `AppClient`, `Signer`), its behaviour is defined by a single suite that every
  implementation is run against — the fake and the real one alike. **Fakes, never mocks**
  ([ADR 0007](../adr/0007-testing-doctrine-fakes-yes-mocks-no.md)): a fake is a working
  implementation with a simpler substrate; a mock asserts on calls and freezes the caller's
  internals into the test.
- **Routers do not own ports.** `connector-client-edge` and `connector-operator` expose
  `axum::Router` values; `connector-cli` merges them and binds. This is what lets a whole
  connector be driven in a test without a socket.

## Errors

- One `thiserror` enum per crate, with `#[error("…")]` messages written for the person reading a
  log, not for a developer reading the enum. No `anyhow`.
- **Keep distinguishable failures distinct.** Where a caller must be able to tell two refusals
  apart, they get separate variants — `ClaimIngestRejection` splits `Mina`, `Underpayment` and
  `SignatureInvalid` from plain `Malformed` precisely so a client can act on the difference. A
  variant that exists only to be collapsed at the call site is the wrong variant.
- **Refuse to start rather than start wrong.** Every configuration mistake is a named
  `ConfigError` raised at load
  ([ADR 0009](../adr/0009-one-typed-config-file-no-environment-layer.md)). A surface that would be
  silently open — an `[operator]` section with an empty bearer token — is a load failure, not a
  default.
- `.unwrap()` does not appear on a production path. `.expect()` does, but only where the string
  argues why the case is impossible (`"32 bytes is a valid HKDF-SHA256 output length"`).

## Comments and naming

- **Every module opens with a `//!` block that says why it exists**, not what it contains. These
  cite the ADR or issue that decided the design, and they are the primary documentation of this
  codebase — a change that invalidates one is expected to update it in the same commit.
- Doc comments on public items describe the contract, including what is deliberately _not_
  guaranteed.
- **Test names are sentences.** `a_self_originated_reject_carries_zero_accumulated_cost`, not
  `test_reject_cost`. The name states the invariant, so a failure reads as the claim that broke.
- Standard Rust naming otherwise: `snake_case` items, `PascalCase` types, `SCREAMING_SNAKE_CASE`
  constants. No Hungarian prefixes, no `I` on traits.

## Logging

- `tracing`, structured, JSON to stdout. `tracing::info!(field = %value, "message")` — the message
  is a short constant, the detail goes in fields.
- Packet handling runs inside an `info_span!("packet", correlation_id, destination)`, so every
  line emitted while handling a packet is correlated automatically
  ([ADR 0014](../adr/0014-metrics-surface-and-packet-correlated-logs.md)). Do not thread a
  correlation id through call signatures. The same span carries `client_channel_id` whenever a
  client claim admitted the packet, joinable to `state_dir/client-edge-claims.log`'s
  `InboundClaimAccepted` entries and the channel's chain-resolved record
  ([ADR 0036](../adr/0036-a-paid-deliverys-attribution-stays-on-the-connector.md)) — the same
  channel key the delivery's own `X-TOON-Payer` carries, per the bullet below.
- **Never log a private key, a mnemonic, a bearer token or a decrypted payload.** A gift wrap's
  failure to open is reported by kind, never by content.
- **A terminating connector tells the app about a payment it verified itself, and about no other**
  ([ADR 0040](../adr/0040-a-verified-payment-is-stated-to-the-app.md), superseding ADR 0036's
  conclusion). The delivery to a route's `handler_url` carries `X-TOON-Payer` (the admitted client
  channel key), `X-TOON-Amount` (the route's own flat price) and `X-TOON-Chain` (that key's
  namespace) — and carries none of them when no client claim admitted the packet or the route is
  free. Do not source any of the three from anywhere else: not the previous hop, not the
  destination address, not the packet's own `amount` field, and never a sentinel in place of an
  absent value. All three live in `crates/connector-runtime/src/attribution.rs`; a caller's
  spelling of those names is stripped there on every delivery, so add a name to that list or do
  not add it at all.

## Documentation

- Where a spec in `docs/protocol/` and an ADR disagree, the ADR wins and the spec is reconciled to
  it. Where a spec and the code disagree, the code wins and the spec is a bug
  ([ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md)).
- A change to the envelope, the gift wrap, the fulfilment derivation or the claim signing scheme
  means regenerating `vectors/wire-vectors.json` in the same change. The gate fails if you do not.
