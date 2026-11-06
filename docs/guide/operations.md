# Operating it

**Logs** are structured JSON on stdout. Every line emitted while handling a
packet carries the same `correlation_id` — the packet's execution condition — and
because that value is invariant across hops, the same id appears in every
connector that handled it. `RUST_LOG=debug` for more.

**Settlement RPC down.** A node refuses to start without it and refuses vouchers
while it is out; see [when the settlement RPC is unreachable](../operators/x402-batch-settlement.md#when-the-settlement-rpc-is-unreachable).

**Releases.** A release is one dispatch of `release-connector.yml`: it builds the
image, cuts a dated handle and opens a GitHub Release. It does not deploy, and
nothing here moves a tag onto a box (ADR 0068) — a node repository pins the
connector image it runs, by release handle, in its own `deploy/` bundle. Because
the binary and a box's mounted TOML are a matched pair in both directions,
**adding a required config key is a breaking deploy**: land the config first, then
bump that pin. A breaking release may have a note under
[`docs/releases/`](../releases/) with the upgrade order and what cannot be rolled back —
read it, and snapshot the state directory, before bumping (the multi-EVM-network build's is
[`multi-evm-networks.md`](../releases/multi-evm-networks.md)).

**Devnet** settles on Base Sepolia and Solana devnet; test funds come from the
[devnet faucet](https://faucet.devnet.toonprotocol.dev). **Mainnet**: TOON's own
retired contracts and program exist on Base and Solana mainnet-beta (records
under [`docs/deployments/`](../deployments/)), run by a third-party operator;
this build settles on x402's instead, and that operator's node has run it on
both mainnets since 2026-09-29, its TOON channels drained. **The fleet's production tier is still
named and empty** (ADR 0056): no fleet machine, no fleet key.
