# The multi-network build

[ADR 0086](../adr/0086-a-node-settles-on-more-than-one-evm-network.md) ("A node
settles on more than one EVM network") first shipped in release
**`2026.11.02.1`**. It breaks the config, the wire (`schema_version` 11) and, for
a node holding an outbound channel, the journal. The GitHub Release body for
`2026.11.02.1` names only the `connector init` change as its reason; this page is
the rest.

## Upgrade order: every payer first

> **Upgrade every payer of a node before you upgrade the node.** A payer is
> anything that sends the node a voucher: its **clients** (`toon-client`, `rig`,
> `swap`, anything speaking the client edge) **and every connector that peers
> with it and pays it** — a peering is two channels, and the peer that opens one
> toward this node is a payer of it.

From this build onward a voucher with no `network` is refused **by name**, even on
a node that settles on a single network (ADR 0086 decision 1). A node upgraded
before one of its payers therefore refuses every voucher that payer sends, and
that payer's payments stop until the payer upgrades. Nothing is guessed and
nothing is carried over a window.

Upgrading payers first is safe: a node on an earlier release ignores an unknown
`network`, and a voucher for another network still fails there because its
channel id does not recompute under that node's domain.

How the refusal shows up, so you can recognise it:

- **Client edge:** the payer gets the reject `claim rejected: claim carries no
'network': …` with the explanation and the upgrade order.
- **Peer carriage (BTP and ILP-over-HTTP):** nothing is sent back to the peer.
  The _paid_ node logs a warning, `refusing a voucher with no 'network'`, followed
  by the same explanation. Seeing it means a peer that pays you is on a
  pre-`schema_version`-11 build: have its operator upgrade first.

## Before and after: `connector.toml`

**Before** (refused by name from this build onward):

```toml
[settlement.evm]
rpc_url          = "https://base-sepolia-rpc.publicnode.com"
token_address    = "0x036CbD53842c5426634e7929541eC2318f3dCF7e"
decimals         = 6

[settlement.evm.key]
key_file = "/app/data/settlement.key"

[[peer_channels]]
peer_id         = "b"
voucher_signer  = "0xabcd…"

[[tokens]]
asset = "evm:0x036CbD53842c5426634e7929541eC2318f3dCF7e"
```

**After**:

```toml
# One table per network, keyed by its decimal EIP-155 chain id. Every key inside
# is unchanged. The id is checked against eth_chainId at boot.
[settlement.eip155.84532]
rpc_url          = "https://base-sepolia-rpc.publicnode.com"
token_address    = "0x036CbD53842c5426634e7929541eC2318f3dCF7e"
decimals         = 6

[settlement.eip155.84532.key]
key_file = "/app/data/settlement.key"

# A channel row names its CAIP-2 network. With one EVM table it is inferred;
# with several, a 0x row without one is refused (never guessed).
[[peer_channels]]
peer_id         = "b"
voucher_signer  = "0xabcd…"
network         = "eip155:84532"

# Asset ids are CAIP-19.
[[tokens]]
asset = "eip155:84532/erc20:0x036cbd53842c5426634e7929541ec2318f3dcf7e"
```

Every key this build removes or newly requires, from ADR 0086 and the config
crate:

| Was                                                                              | Now                                                                                                                                                                                                                                                   |
| -------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `[settlement.evm]`, `[settlement.evm.key]`                                       | `[settlement.eip155.<chainId>]`, `[settlement.eip155.<chainId>.key]`. `[settlement.evm]` is refused by name with this rewrite. A key that is not a decimal chain id, and an empty `[settlement.eip155]`, are refused too                              |
| `chain_id` on a channel row (parsed only to be refused)                          | `network = "eip155:<chainId>"` (EVM) or `network = "solana:<genesis prefix>"` on `[[peer_channels]]` and `[[pay_channels]]` rows. Required on a `0x` row when the node holds more than one EVM table; a row naming a network with no table is refused |
| `evm:0x…`, `evm:<chainId>:0x…`, `solana:<mint>`, `<chain>:native` as an asset id | CAIP-19: `eip155:<chainId>/erc20:0x…`, `solana:<genesis prefix>/token:<mint>`, `<network>/slip44:<coin type>`. The old spellings are refused by name, each with its rewrite                                                                           |
| `[[tokens]]` on a network the node has no table for                              | refused (`TokenWithoutSettlement`); a token off the numeraire's network needs a static `[[rates]]` row                                                                                                                                                |
| a peering whose two channel rows are on different networks                       | refused: both channels of a peering are on one network                                                                                                                                                                                                |

Several `[settlement.eip155.<chainId>]` tables may name the same key file. The
per-table keys `facilitator_url`, `asset_transfer_method`, `asset_eip712_name` and
`asset_eip712_version` are per network, and under `rpc_via_socks_proxy` each EVM
table gets its own circuit.

## The wire: `schema_version` 11

- Every EVM and Solana **voucher, claim-state challenge and payout voucher**
  carries a required CAIP-2 `network` (`eip155:<chainId>`,
  `solana:<genesis prefix>`). `blockchain` stays and still picks the parser.
- Asset ids are CAIP-19 (above).
- Apps behind a route receive a new header, `X-TOON-Network`, carrying the CAIP-2
  network. `X-TOON-Chain` and `X-TOON-Payer` are unchanged.
- `POST /peers`'s `chain` filter is a CAIP-2 network; a peer publishing several
  networks this node has tables for, with no filter given, is refused as
  `AmbiguousNetwork`.

`vectors/wire-vectors.json` is the normative contract
([ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md)).

## The journal

The channel key stays `evm:0x…` / `solana:…`. A version-1 outbound channel record
is upgraded in place on first boot, by proving which configured table reproduces
its channel id; a record no table reproduces (the operator removed a network that
still holds value) is refused at boot by name. Keep every network that holds a
channel configured until it is closed. The upgrade is one-way: take a snapshot of
all of `state_dir` first (#1667 states the rollback rule in full).

## For node repositories

Bumping a pin to `2026.11.02.1` or later is a change to the node's
`connector.toml` as well as to the image — do both in one reviewed change, and
only once every payer of that node, clients and peering connectors alike, runs a
`schema_version` 11 build.
