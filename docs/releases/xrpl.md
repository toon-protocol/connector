# XRPL settlement

[ADR 0087](../adr/0087-xrpl-settles-over-the-ledgers-payment-channels.md) ("XRPL
settles over the ledger's payment channels") gives the connector a third settlement
family. A node may now hold `[settlement.xrpl.<id>]` tables beside its EVM and Solana
ones, accept vouchers over the ledger's own `PayChannel`, open its own channels toward
a peer that publishes an XRPL entry, and land a voucher the moment a payer begins to
leave. **Nothing already running is broken by it**: a config with no XRPL table
behaves as before. Two things are refused that were merely unspoken before, and the
vectors move.

The operator's walkthrough is [Get paid → XRPL](../guide/get-paid.md#xrpl).

## Before and after: `connector.toml`

A node that settles in USDC on Base and now also takes XRP on XRPL testnet, dealing
between them.

**Before:**

```toml
[settlement.eip155.84532]
rpc_url              = "https://base-sepolia-rpc.publicnode.com"
token_address        = "0x0C996d7c934c79a6255254875607Fe69df25C0E1"
decimals             = 6
asset_eip712_name    = "USDC"
asset_eip712_version = "2"

[settlement.eip155.84532.key]
key_file = "/app/data/settlement.key"
```

**After** (the first table is untouched; everything below it is new):

```toml
[settlement.eip155.84532]
rpc_url              = "https://base-sepolia-rpc.publicnode.com"
token_address        = "0x0C996d7c934c79a6255254875607Fe69df25C0E1"
decimals             = 6
asset_eip712_name    = "USDC"
asset_eip712_version = "2"

[settlement.eip155.84532.key]
key_file = "/app/data/settlement.key"

[settlement.xrpl.1]                        # keyed by the ledger's network_id
rpc_url = "https://s.altnet.rippletest.net:51234"
# min_settle_delay_secs  = 86400           # default one day; must exceed 901
# redemption_safety_secs = 60              # default 60; below the delay
# max_fee_drops          = 100000          # default; never 0
# rpc_via_socks_proxy    = false

[settlement.xrpl.1.key]
key_file = "/app/data/settlement-xrpl.key" # an XRPL family seed, NOT 32 bytes

# Required on every node with an XRPL table, one row per settlement table.
# `numeraire` is not required; this node sets one for its rate below:
[[tokens]]
asset     = "eip155:84532/erc20:0x0C996d7c934c79a6255254875607Fe69df25C0E1"
numeraire = true

[[tokens]]
asset = "xrpl:1/slip44:144"

# Only if this node forwards between XRP and anything else:
[[rates]]
from = "eip155:84532/erc20:0x0C996d7c934c79a6255254875607Fe69df25C0E1"
to   = "xrpl:1/slip44:144"
rate = { numerator = 2, denominator = 1 }

[rate_guards]
spread   = { numerator = 1, denominator = 100 }
ttl_secs = 300
max_move = { numerator = 5, denominator = 100 }
```

The networks are `0` (mainnet), `1` (testnet), `2` (devnet) and `1000` (the stand-alone
ledger the tests and `local/` use). Any other key is refused.

## New refusals, by name

At config load:

- `[settlement.xrpl]` holding no network, or holding `rpc_url`/`key` directly under
  it (`SettlementXrplEmpty`, `SettlementXrplBareTable`).
- A network this build has no row for (`SettlementXrplUnknownNetwork`).
- A key written inline rather than as `[settlement.xrpl.<id>.key]`
  (`SettlementXrplInlineKey`), and any unknown or mistyped key
  (`SettlementXrplTableInvalid`).
- `min_settle_delay_secs` at or below 901 (`SettlementXrplSettleDelayBelowFloor`),
  `redemption_safety_secs` of zero or not below the delay
  (`SettlementXrplRedemptionSafetyOutOfRange`), `max_fee_drops = 0`
  (`SettlementXrplZeroMaxFee`).

Still refused, not new: **`xrpl:native`, and every other `<chain>:native` asset
spelling**, wherever an asset id is parsed (`AssetIdError::LegacyNative`, since #1607
and ADR 0086). XRP is `xrpl:<id>/slip44:144` (ADR 0087 decision 7).

At boot, before the node serves:

- A key file that cannot be read or is not an XRPL family seed — 32 random bytes from
  `openssl rand` is not one (`XrplKeyUnusable`).
- A table whose `rpc_url` reports a different `network_id` (`XrplNetworkMismatch`).
- A settlement account that does not exist on the ledger, or holds less than its
  base reserve (`XrplAccountMissing`, `XrplAccountUnderfunded`), or an endpoint with
  no validated ledger to read the reserve from (`XrplReserveUnreported`).
- **Two XRPL tables that resolve to one account** (`XrplAccountOnTwoNetworks`). An XRPL
  claim binds no network id, so use a different account on testnet than on mainnet.

At admission of a channel, on its first voucher: a `Destination` that is not this
node's account, a `SettleDelay` below the table's minimum, an `Expiration` already
set, **any `CancelAfter`** (a new `AdmissionRefusal::CancelAfter`), and a key that is
not a valid channel key. A voucher above 10¹⁷ drops is refused as
`AmountExceedsChain`.

## The wire: `schema_version` 12

`vectors/wire-vectors.json` moves from `schema_version` 11 to **12**. XRPL entries
join `claim_voucher` (including `xrpl_above_ceiling`), `payout_voucher`,
`voucher_claim_state_challenge` and `client_auth_channel_challenge`. A voucher is
`CLM\0 ‖ channelId ‖ u64 drops`; the claim-state challenge has its own domain tag,
`toon-xrpl-voucher-claim-state-challenge-v1`, and binds the CAIP-2 network. No existing
EVM or Solana bytes change. `toon-client`, `rig` and `swap` read the new rows from
the vectors, not from this page (ADR 0021).

The greeting gains one `batch-settlement` entry per XRPL table, `network: xrpl:<id>`,
`asset: xrpl:<id>/slip44:144`, `payTo` the classic address, and
`extra: {binding: "xrpl-paychan-v1", minSettleDelay, redemptionSafetySeconds}`.

## Behaviour to know about

- **A payer's exit is answered with `tfClose`.** The watcher lands the latest held
  voucher and closes the channel in one transaction, so a payer that began an exit and
  then wanted to renew loses that chance (ADR 0087 decision 8).
- **A peering costs each side an owner reserve** (0.2 XRP on 2026-10-28) until its
  channel is deleted, and each party pays its own fees in XRP. There is no sponsor.
- **A node with an XRPL table must declare `[[tokens]]`.** Boot refuses it by name
  otherwise (`XrplSettlementWithoutTokens`), even on one network holding only XRP. Declare
  `xrpl:<id>/slip44:144`, and a token for every other settlement table the node holds. No
  rate is needed at boot.
- **An XRPL leg is a dealing boundary whenever the other side holds anything else**
  (ADR 0071): without a `[[rates]]` row there is no such forward. A path that holds XRP
  end to end forwards unconverted (ADR 0087 decision 6.1).

## For node repositories

Nothing is required: adopting this build needs no config change. An XRPL table is
an addition, and the pair stays a matched pair as always.
Tests that exercise XRPL need Docker (`CONTRIBUTING.md`).
