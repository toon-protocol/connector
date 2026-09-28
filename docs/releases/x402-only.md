# The x402-only build

[ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)
("Every channel is an x402 channel; a peering is two of them") retires TOON's
own `TokenNetwork`/`TokenNetworkRegistry` (EVM) and payment-channel program
(Solana) from the connector. Every channel — client, peer and payout — is now
an x402 `batch-settlement` channel, on both chains, and a peering is **two**
of them: one opened by each side. This is **the largest breaking change since
ADR 0042**: it breaks the config, the journal, the wire (`schema_version` 7)
and the self-description. Read this whole page before bumping a node
repository's pin past it.

If your node has a live TOON channel — this repository's own fleet does not,
but a third-party mainnet node may — **drain it first**, on the last
TOON-capable release, before upgrading:
[`docs/operators/draining-toon-channels.md`](../operators/draining-toon-channels.md).
The last TOON-capable release is **`2026.09.27.2`**.

## Before and after: `connector.toml`

Two nodes, peered, one settling on EVM and one on Solana — enough to show
every shape that changed. **Before** (still boots on `2026.09.27.2` and
earlier; refused by name from this build onward):

```toml
# ── EVM node ──────────────────────────────────────────────────────────────
client_edge_addr = "0.0.0.0:3000"
state_dir        = "/app/state"
peer_expose      = "http"

[node]
addresses     = ["g.example.a"]
http_endpoint = "https://a.example/ilp"

[signer]
key_file = "/app/data/signer.key"   # also signed peer claims and client payouts

[settlement.evm]
rpc_url          = "https://base-sepolia-rpc.publicnode.com"
contract_address = "0x0c41D9D424d6B075A3cEa1068a694f7847a8CCa5"   # TokenNetworkRegistry
token_address    = "0x0C996d7c934c79a6255254875607Fe69df25C0E1"
decimals         = 6

[settlement.evm.key]
key_file = "/app/data/settlement.key"

[settlement.evm.batch_settlement]     # x402 was opt-in, on top of TOON
asset_eip712_name    = "USDC"
asset_eip712_version = "2"

[[client_channels]]
channel_id            = "0x1234…"
chain_id              = 84532
token_network_address = "0x0c41D9D424d6B075A3cEa1068a694f7847a8CCa5"
counterparty          = "0xabcd…"

[[peer_channels]]
peer_id          = "b"
channel_id       = "0x5678…"
chain_id         = 84532
token_network    = "0x0c41D9D424d6B075A3cEa1068a694f7847a8CCa5"
counterparty_key = "0xef01…"

[[pay_channels]]
peer_id         = "b"
channel_id      = "0x5678…"    # the SAME channel paid AND was paid on
chain_id        = 84532
token_network   = "0x0c41D9D424d6B075A3cEa1068a694f7847a8CCa5"
client_edge_url = "https://b.example/ilp"

[[peers]]
id                 = "b"
url                = "https://b.example/ilp"
fee                = 100
max_packet_amount  = 5000
claim_ack_timeout_ms = 5000
```

**After** (this build; both nodes now need `[node]`/`peer_expose` since each
opens its own outbound channel toward the other):

```toml
# ── EVM node ──────────────────────────────────────────────────────────────
client_edge_addr = "0.0.0.0:3000"
state_dir        = "/app/state"
peer_expose      = "http"

[node]
addresses     = ["g.example.a"]
http_endpoint = "https://a.example/ilp"

[signer]
key_file = "/app/data/signer.key"   # identity only: gift-wrap and the self-description

[settlement.evm]
rpc_url               = "https://base-sepolia-rpc.publicnode.com"
token_address          = "0x0C996d7c934c79a6255254875607Fe69df25C0E1"
decimals               = 6
asset_eip712_name      = "USDC"        # required now
asset_eip712_version   = "2"           # required now
# min_withdraw_delay_secs = 86400      # optional, default one day, 900..=2592000

[settlement.evm.key]
key_file = "/app/data/settlement.key"  # signs THIS chain's vouchers too

# [[client_channels]] is gone: a client channel resolves from the chain and
# the voucher, never from config.

[[peer_channels]]
peer_id        = "b"
voucher_signer = "0xef01…"   # b's settlement address -- what b's self-description publishes
# inbound_channel = "0x…"    # optional: pin to one specific channel

[[pay_channels]]
peer_id           = "b"
outbound_channel  = "0x5678…"   # THIS node's own channel, opened first via POST /channels
client_edge_url   = "https://b.example/ilp"

[[peers]]
id                = "b"
url               = "https://b.example/ilp"
fee               = 100
max_packet_amount = 5000
# claim_ack_timeout_ms is gone: peer_answer_timeout_ms bounds the ack instead
```

```toml
# ── Solana node ───────────────────────────────────────────────────────────
# BEFORE:
[settlement.solana]
rpc_url    = "https://api.devnet.solana.com"
program_id = "2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip"   # TOON's own program
token_address = "34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU"
decimals   = 6

[settlement.solana.key]
key_file = "/app/data/settlement-solana.key"

[[peer_channels]]
peer_id          = "c"
channel_account  = "7xKX…"     # a channel on TOON's own program
counterparty_key = "9WzD…"     # c's key

[[pay_channels]]
peer_id         = "c"
channel_account = "7xKX…"      # the SAME channel, both roles
client_edge_url = "https://c.example/ilp"

# AFTER: `program_id` is gone -- `payment-channels` is a fixed constant of
# the binary (CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX), the same on
# devnet and mainnet-beta.
[settlement.solana]
rpc_url               = "https://api.devnet.solana.com"
token_address         = "34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU"
decimals              = 6
min_sponsored_deposit = 1000000        # required now: base units, bounds the sponsor endpoint
# min_grace_period_secs = 86400        # optional, default one day, >= 900

[settlement.solana.key]
key_file = "/app/data/settlement-solana.key"   # signs THIS chain's vouchers too

[[peer_channels]]
peer_id        = "c"
voucher_signer = "9WzD…"       # c's authorized_signer: its Solana settlement key, base58
# inbound_channel = "…"        # optional: pin to one payment-channels channel

[[pay_channels]]
peer_id          = "c"
outbound_channel = "4qPr…"     # THIS node's own channel toward c, opened first via POST /channels
client_edge_url  = "https://c.example/ilp"
```

## Every key this build removes or requires

**Refused by name — a node naming any of these does not boot:**

| Key                                                                                      | Where                                            | Why                                                                                                |
| ---------------------------------------------------------------------------------------- | ------------------------------------------------ | -------------------------------------------------------------------------------------------------- |
| `contract_address`                                                                       | `[settlement.evm]`                               | The x402 contract is a constant of the binary, never config.                                       |
| `program_id`                                                                             | `[settlement.solana]`                            | Same, for `payment-channels`.                                                                      |
| `channel_index_from_block`                                                               | `[settlement.evm]`                               | Tuned the EVM `TokenNetwork` log index, which is deleted.                                          |
| `channel_index_confirmations`                                                            | `[settlement.evm]`                               | Same.                                                                                              |
| the flat `[settlement]` shape                                                            | top level                                        | Every settlement table is per chain now.                                                           |
| the `[settlement.<chain>.batch_settlement]` sub-table                                    | `[settlement.evm]` / `[settlement.solana]`       | Its keys moved up a level — x402 is not an opt-in any more, it is the only shape.                  |
| `[[client_channels]]`                                                                    | top level                                        | A client's x402 channel resolves from the chain and the voucher; nothing is declared in advance.   |
| `channel_liveness_ttl_secs`, `channel_serve_stale_secs`, `channel_reattempt_interval_ms` | client edge                                      | Tuned the TOON client-channel liveness sweep, which is deleted.                                    |
| `claim_ack_timeout_ms`                                                                   | `[[peers]]`                                      | Bounded the TOON flush, which is deleted; `peer_answer_timeout_ms` bounds a voucher's ack instead. |
| `channel_id`, `token_network`, `chain_id`, `counterparty_key`                            | `[[peer_channels]]`, `[[pay_channels]]` (EVM)    | The old row named a shared, derived channel; there is no such thing under two one-way channels.    |
| `channel_account` (TOON meaning), `counterparty_key`, `program_id`                       | `[[peer_channels]]`, `[[pay_channels]]` (Solana) | Same.                                                                                              |

**Newly required:**

| Key                     | Where                                             |
| ----------------------- | ------------------------------------------------- |
| `asset_eip712_name`     | `[settlement.evm]`, wherever that table exists    |
| `asset_eip712_version`  | `[settlement.evm]`, wherever that table exists    |
| `min_sponsored_deposit` | `[settlement.solana]`, wherever that table exists |

**Moved up a level, same bounds as before:** `min_withdraw_delay_secs` (EVM,
optional, default one day, `900..=2592000`) and `min_grace_period_secs`
(Solana, optional, default one day, `>= 900`) — both used to live under
`[settlement.<chain>.batch_settlement]`; they live directly under
`[settlement.<chain>]` now.

**New shape, both required fields:**

- `[[peer_channels]]`: `peer_id` + `voucher_signer` (the peer's settlement
  address on EVM, its `authorized_signer` on Solana — the spelling picks the
  chain) + optional `inbound_channel` to pin one specific channel.
- `[[pay_channels]]`: `peer_id` + `outbound_channel` (this node's **own**
  channel, which must already be open — via `POST /channels` — and
  journaled under `state_dir` before boot names it) + `client_edge_url`.

A channel can no longer be shared between the peer book and the pay book (a
peering is two channels now, never one), and a peering channel can no longer
also be a `[[client_channels]]` row.

## The journal

A `peer-claims.log` or `client-edge-claims.log` under `state_dir` holding a
`toon-channel` entry is **refused at boot, by name**. It is never silently
skipped — a skipped entry would be a claim somebody could still redeem that
this node has forgotten it signed. If you are upgrading a node with a live
TOON channel, drain it first on the last TOON-capable release, then **move**
the journal out of `state_dir` (do not delete it — it is the only remaining
record of what the node signed and redeemed):
[`docs/operators/draining-toon-channels.md`](../operators/draining-toon-channels.md).
The fleet holds no such journal; this only applies if you deployed against
TOON's own `TokenNetwork` or Solana program before this build.

## The wire: `schema_version` 7

[`vectors/wire-vectors.json`](../../vectors/wire-vectors.json) is the
normative contract (ADR 0021); this is orientation only.

- **Every claim is an x402 `batch-settlement` voucher.** `scheme` is
  required; an absent `scheme` or `scheme: "toon-channel"` is refused by
  name — at the client edge, on both peer carriages, and by
  `POST /ilp/claim-state` — the same way `blockchain: "mina"` always was.
  There is no `toon-channel` claim shape left to parse.
- **`channelChallenge` replaces `auth_channel_proof`.** The BTP `auth`
  entry's channel declaration is now the same voucher claim-state challenge
  object a peer's zero-value packet carries (`scheme: "batch-settlement"`
  required), signed by the channel's voucher signer under the x402 domain
  (EVM) or as Ed25519 over a fixed string (Solana). The old
  `auth_channel_proof` (top-level `channelId`/`expires`/`signature`, signed
  under the `TokenNetwork` domain) is refused by name, and the session is
  **not** bound if it is sent.
- **The greeting's TOON terms move into x402 v2's `extensions.toon`.** The
  `402` body and the BTP `payment-required` frame's `accepts[]` holds only
  valid x402 `batch-settlement` entries now — the `toon-channel` entry, and
  `extra.settlement(s)`, are gone. TOON's own facts (the amount, the
  endpoint, the price, the node's addresses) ride in
  `extensions.toon.info`, with `extensions.toon.schema` describing it. Read
  the price from `extensions.toon.info.amount`; a reader that finds no
  `accepts[]` entry must not treat the route as free.
- **The self-description drops `settlements` and adds `voucherSigners`.**
  `GET /ilp` keeps `batchSettlements` (the x402 terms) and now publishes,
  per chain, the node's own voucher signer — the address or key a peer
  binds an inbound channel by.

## Operator surface

**Gone:** `redeem`, `redeem-latest`, `settle`, `close`, `cooperative-close`.
**New:** `POST /channels/:id/withdraw` (start a withdrawal on EVM or a
close-request on Solana; call again once due to finish) and
`POST /channels/:id/land` (land the latest voucher held on an inbound
channel right now — the manual lever for planned maintenance; the watchers
still land automatically). `POST /channels` (open an outbound channel) and
`POST /channels/:id/fund` (top it up by an increment) keep their names and
shapes.

## Keys

Each chain's settlement key now signs that chain's vouchers — client payouts
included — as well as its settlement transactions. `[signer]` is identity
only: gift-wrap and the self-description, never a value-moving signature.
This corrects an interim note that shipped briefly during the rollout
(`[signer]` was said to sign client-payout claims; it does not).

## For node repositories

`relay`, `store`, `gas-station` and `gateway` each pin the connector image
they run, by release handle, in their own `deploy/` bundle (ADR 0068).
**Bumping that pin past this build is a breaking deploy for that box**: the
binary and the box's bind-mounted `connector.toml` are a matched pair in
both directions (ADR 0009), and this build boots on no config in the old
shape. Land the box's config change in the same reviewed change as the pin
bump — not before, since the old binary does not understand the new shape
either, and not after, since the new binary refuses the old one.

## The local stack (#1383)

`local/`'s anvil now hosts `x402BatchSettlement` and its deposit collectors
at their canonical addresses, plus Circle's FiatToken v2.2 as USDC — no
`TokenNetworkRegistry`, `TokenNetwork`, `MockERC20` or ERC-2771 forwarder is
deployed any more. The local Solana validator loads `payment-channels` at
`CHNLx…` and mainnet's p-token from the pinned fixtures under
`crates/connector-settlement-solana/fixtures/`; nothing of TOON's Solana
program is loaded. `local/keys.sh`'s channel stage now opens every local
channel through the running nodes' own `POST /peers` and `POST /channels`
(through the sponsor endpoint on Solana) rather than `cast` or
`open-solana-channel.py`, and tops it up with `POST /channels/:id/fund`. No
committed `local/` config holds a `[[peer_channels]]` or `[[pay_channels]]`
row naming a TOON channel field. `make local-verify`'s peered topologies
(`two-hop`, `mixed-chain`, `dealing`) each cross their peering more than
once and read the payee's own claim journal, and `dealing` still asserts
the converted figure rather than merely a fulfilled packet.
