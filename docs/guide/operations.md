# Operating it

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
[devnet faucet](https://faucet.devnet.toonprotocol.dev). **Mainnet**: TOON's own
retired contracts and program exist on Base and Solana mainnet-beta (records
under [`docs/deployments/`](../deployments/)), run by a third-party operator;
this build settles on x402's instead, and that operator's node has run it on
both mainnets since 2026-09-29, its TOON channels drained. **The fleet's production tier is still
named and empty** (ADR 0056): no fleet machine, no fleet key.
