# Draining a node with live TOON channels

[ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)
retired TOON's own `TokenNetwork` (EVM) and payment-channel program (Solana)
from the connector. From the first release that ships it onward, the binary
holds no `SettlementBackend` for either, resolves no `toon-channel` claim, and
**refuses to boot on a `state_dir` whose journal still holds one**
(`peer-claims.log` or `client-edge-claims.log`), by name, pointing at this
file. Nothing is lost by that refusal — the contracts stay on chain forever,
and the release before this one can still drive them — but a node with a live
TOON channel has to be drained **before** it upgrades, not after.

## Who this is for

**The fleet needs none of this.** Its production tier is a named, empty tier
(ADR 0056) and no devnet node's committed config carries a `[[peer_channels]]`
or `[[client_channels]]` row that names a `toon-channel` channel. This
procedure exists for **one third-party operator's mainnet node** — the only
node ADR 0075 knew of with a live TOON channel, on the Base mainnet
`TokenNetwork` and the Solana mainnet-beta payment-channel program — and for
anyone else who deployed this connector against either of those contracts
before this release.

If your node's journal holds no `toon-channel` entry, this procedure does not
apply to you: upgrade normally.

## What the boot refusal looks like

```
ERROR: peer-claims.log holds a toon-channel entry for channel 0x… . This build
       no longer settles TOON channels (ADR 0075). Drain the node on the last
       TOON-capable release before upgrading -- see
       docs/operators/draining-toon-channels.md.
```

The same refusal fires on `client-edge-claims.log`. Either way, the node does
not start, and nothing is deleted — the fix is to go back to the release
before this one, finish draining, then come forward again.

## The last TOON-capable release

**`2026.09.27.2`** (image tag `ghcr.io/toon-protocol/connector:rust-2026.09.27.2`)
is the last release cut before the first ADR 0075 step landed on `main`. It
predates every part of the x402-only build: its operator surface still has
`redeem`, `redeem-latest`, `settle`, `close` and `cooperative-close`, and its
`[settlement.<chain>]` tables still take `contract_address` / `program_id`.
**Run every step below on this release**, not on anything after it — a later
release's binary is the one refusing to boot on the journal this procedure
clears.

```bash
docker pull ghcr.io/toon-protocol/connector:rust-2026.09.27.2
```

If your node is already running something later than `2026.09.27.2` and
earlier than the release that shipped ADR 0075's journal refusal, downgrade to
`2026.09.27.2` for the duration of this procedure — every release in between
is equally TOON-capable, but pinning to the last one keeps this runbook
unambiguous.

## Before you start

- **A funded settlement key for gas** on whichever chain(s) hold a live
  channel — closing and settling both spend gas, on top of whatever `redeem`
  costs.
- **Read access to a block explorer** for the chain(s) involved (Base mainnet,
  Solana mainnet-beta), to confirm each step landed independently of the
  node's own view.
- **`docs/operators/sign-write.sh`** (or your own RFC 9421 signer) and the
  node's operator write key, exactly as for any other signed write — see
  [`signing-a-write.md`](signing-a-write.md).
- **Time.** Closing a channel starts its on-chain challenge period
  (`settlementTimeout` on EVM, the channel's own settlement window on
  Solana) before it can be settled. Plan for that window, not just the API
  calls.

## Order

### 1. List every channel this node holds

```bash
curl -s -H "Authorization: Bearer $(cat /app/data/operator-bearer-token)" \
  https://your-node.example/channels | jq
```

Each row is `{"id", "counterparty", "status", "deposited", "own_deposited",
"redeemed"}`. `status` is `"open"`, `"closed"` (challenge period running or
elapsed but not yet settled) or `"settled"` (terminal). Work through every row
that is not already `"settled"`.

### 2. Land the latest claim on every channel, both directions

Before touching a channel's lifecycle, make sure every claim already accepted
is actually redeemed on chain — a claim signed but never submitted is value
`settle` would otherwise hand back to the wrong side.

```bash
docs/operators/sign-write.sh -k operator-write.key -X POST \
  -p /channels/<channel-id>/redeem-latest -u https://your-node.example
```

Repeat for every channel `GET /channels` listed as `"open"` or `"closed"`.
`redeem-latest` is idempotent against a channel with nothing new to redeem —
running it again is safe, and cheap compared with skipping it and finding out
later.

### 3. Close and settle each channel

`close` and `cooperative-close` do the same on-chain work here: despite its
name, `cooperative-close` is not a two-party protocol and does not skip the
challenge period — it is exactly `redeem-latest` immediately followed by the
same unilateral `close` the other endpoint calls, on this node's own
best-known claim. Either way the channel lands in `"closed"`, not
`"settled"`, and still needs its challenge window to run before `settle` can
finish it. Since step 2 already redeemed everything there was to redeem,
plain `close` (no request body) is enough:

```bash
docs/operators/sign-write.sh -k operator-write.key -X POST \
  -p /channels/<channel-id>/close -u https://your-node.example

# ... wait for the channel's settlementTimeout / settlement window to elapse ...

docs/operators/sign-write.sh -k operator-write.key -X POST \
  -p /channels/<channel-id>/settle -u https://your-node.example
```

Do this for **every channel on every chain** the node settles on — a node
carrying both an EVM and a Solana `[settlement]` table drains both, not
whichever one you remember first.

### 4. Confirm on chain

Do not trust the node's own `GET /channels` alone — it is exactly the view
that is about to stop existing. For each channel, confirm independently, on a
block explorer or with a direct RPC call, that:

- its status is terminal (`Settled` on EVM's `TokenNetwork`, the equivalent
  terminal state on Solana's program);
- each participant's remaining deposit has actually landed back in their
  wallet;
- no `redeem`-able claim is outstanding — re-check `GET /channels`'
  `redeemed` figure against your own claim journal before moving on.

Only once every channel this node's journal names is independently confirmed
closed and settled should you continue. A channel still open when you upgrade
is a channel this node can no longer redeem, close or settle at all — the
upgraded binary holds no code path for any of the three.

### 5. Move the journal out of `state_dir`

Once every TOON channel is drained, the journal files still hold the history
of that now-finished channel — and the upgraded binary refuses to boot on
them regardless of whether anything in them is still actionable (decision 8:
"never silently skipped: a skipped entry is a claim somebody could still
redeem that this node has forgotten it signed"). Move them out, do not
delete them:

```bash
mkdir -p /app/state/pre-adr-0075-archive
mv /app/state/peer-claims.log /app/state/pre-adr-0075-archive/ 2>/dev/null || true
mv /app/state/client-edge-claims.log /app/state/pre-adr-0075-archive/ 2>/dev/null || true
```

Keep the archive — it is the only remaining record of what this node signed
and redeemed against TOON's channels, and nothing recreates it.

### 6. Upgrade and rewrite the config

With the journal clear, pull the release you actually want to run and rewrite
`connector.toml` to the x402-only shape:

- Delete `[settlement.<chain>] contract_address` / `program_id` — the x402
  contract and program are fixed constants of the binary now, never config.
- Delete any `[settlement.<chain>.batch_settlement]` sub-table and move its
  keys up into `[settlement.<chain>]` directly.
- Add the newly required keys: `asset_eip712_name` and `asset_eip712_version`
  wherever `[settlement.evm]` exists; `min_sponsored_deposit` wherever
  `[settlement.solana]` exists.
- Delete any `[[client_channels]]` table.
- Rewrite every `[[peer_channels]]` row to the new shape (`peer_id` +
  `voucher_signer`, chain read from the spelling, optional
  `inbound_channel`) and every `[[pay_channels]]` row to the new shape
  (`peer_id` + `outbound_channel`, already opened via `POST /channels` and
  journaled + `client_edge_url`) — see the top-level
  [`README.md`](../../README.md#4-peer-with-another-node) and
  [`configuration-spec.md`](../protocol/configuration-spec.md) §2.4.

See [the release notes](../releases/x402-only.md) for the full before-and-after
config and every key this build removes or requires.

```bash
docker pull ghcr.io/toon-protocol/connector:rust-<your-target-handle>
```

Boot the rewritten config against the new image. A node that skipped step 5
finds its journal refused at boot, by name, pointing back at this file — that
refusal is not data loss, it means come back here and finish draining first.
