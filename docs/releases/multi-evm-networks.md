# The multi-EVM-network build

[ADR 0086](../adr/0086-a-node-settles-on-more-than-one-evm-network.md) ("A node
settles on more than one EVM network") lets one node hold a settlement table per
EVM network. To do that it names the network on every claim, every asset id and
every channel row. The first release to contain it is **`2026.11.02.1`** (#1607);
the last one before it is **`2026.10.23.1`**.

This build breaks three things at once, **even on a node that settles on one
network**: the wire, the config and the journal. Read this whole page, and
snapshot your state directory, before bumping a node repository's pin past it.

1. [The wire](#the-wire-network-is-required): a voucher with no `network` is refused.
2. [The upgrade order](#the-upgrade-order): every payer upgrades before the node it pays.
3. [The journal](#the-journal-upgrade-is-one-way): after first boot an older build cannot start.
4. [Config, before and after](#before-and-after-connectortoml).
5. [Mixed fleets](#mixed-fleets-do-not-add-a-second-evm-network-yet).

## The wire: `network` is required

`network` arrives with `schema_version` 11 of `vectors/wire-vectors.json` (ADR 0086
decision 3). `2026.11.02.1` itself ships 12, because XRPL landed in the same release (see
[`xrpl.md`](xrpl.md)); `2026.10.23.1` was at 9.

- **Claims, claim-state challenges and payout vouchers** — every EVM and Solana
  voucher, whether it comes from a peer or from a client — carry a required CAIP-2
  `network`: `eip155:<chainId>` (`eip155:8453` for Base) or `solana:<genesis prefix>`.
  `blockchain` stays and still picks the parser.
- A voucher with no `network` is **refused by name**, never guessed, even on a node
  with one table. ADR 0086 considered inferring it for a single-network node and refused:
  the answer would change the day the operator added a second table. A node logs a warning,
  `refusing a voucher with no 'network'`, carrying the refusal it sends back, which reads:

  ```text
  claim carries no 'network': since wire schema_version 11 every voucher names the CAIP-2 network it is on (eip155:<chainId>, solana:<genesis prefix> or xrpl:<id>), and one without it is refused rather than guessed (ADR 0086) -- a payer that sends none is probably on a build from before schema_version 11, and every payer of a node (its clients and any peer connector that pays it) must upgrade before the node does
  ```

  A peer-role claim-state challenge with no `network` is refused with the same text,
  prefixed `peer-role challenge: `. A peer gets the refusal back in BTP's `F00` error frame
  or ILP-over-HTTP's `400` body; a client gets it after `claim rejected: `.

- **Asset ids are CAIP-19**: `eip155:<chainId>/erc20:0x…` and
  `solana:<genesis prefix>/token:<mint>`. The old `evm:0x…` and `solana:<mint>` spellings
  are refused by name, and so are `evm:<chainId>:0x…` and `<chain>:native` (now
  `<network>/slip44:<coin type>`). This reaches `[[tokens]]` and
  anything else in your config that spells an asset.
- Apps behind a route get a new header, `X-TOON-Network`, carrying the CAIP-2 network.
  `X-TOON-Chain` and `X-TOON-Payer` keep their values.

## The upgrade order

**Every payer of a node must be on a build that sends `network` before the node itself
upgrades.** A payer is anything that sends it a voucher: a peer, and a client
(`toon-client`, `rig`, `swap`). A build that sends `network` is safe against an older
node, which ignores a field it does not know; an older payer against this build is the
case that stops payments.

1. **Clients first.** Upgrade `toon-client` (and `rig`, `swap`) to a release that
   speaks `schema_version` 11 or later before any node you operate that clients pay.
2. **Peers, leaf to root.** For every node, upgrade the peers that pay it first.
3. **A peering is two channels, so it has two payers.** A pays B on one channel and B pays A
   on the other. Whichever node upgrades first refuses the other's vouchers until the other
   catches up, because the other is still an older payer. There is no order that avoids this
   on a two-way peering: upgrade both ends in one window and expect that gap to be as short as
   the window. In it, the half still on the older build is refused with the text above and is
   not credited.

If you skip the order, payments do not degrade — they stop, and the log is the line above on
every voucher. An upgraded payer is the fix; rolling the node back is the other one, see below.

## The journal upgrade is one-way

At boot, this build journals every outbound channel record again as **version 2**, under its
unchanged key, which records the network the channel is on (ADR 0086 decision 10); replay reads
the version-2 copy in place of the first. A version-1 EVM record gets its network by proof: its
channel id is recomputed under each configured table's EIP-712 domain, and the one table that
reproduces it is its network. A Solana record is placed on the node's one Solana network.
Nothing is guessed.

- A **build from before this release cannot read a version-2 record, and refuses to boot**:

  ```text
  corrupt journal entry: outbound channel '…' has a record this build cannot read
  ```

  This build prints the same line if a record is damaged.

- **Snapshot the state directory before the first boot — at least `outbound-channels.log`**.
  Rolling back means restoring that snapshot under the old build. Any channel opened since
  the upgrade is not in the snapshot and is lost **to the old build**; the chain still holds
  its collateral, so keep the new build's journal aside to withdraw from it later.
- The restore **refuses to boot, by name**, in two further cases, both of which leave the
  journal untouched:

  ```text
  the outbound channel … is on …, and this node has no settlement table for it: the node still holds value in that channel. Restore the table for the network it was opened on, at least until the channel is withdrawn (ADR 0086 decision 10)
  ```

  when you removed a network that still holds one of your channels (a version-1 record no table
  reproduces prints `no network this node settles on …` in place of the network), and

  ```text
  the outbound channel … is reproduced by more than one settlement table (…), so which network it is on cannot be proved: state is never assigned to a network by guess (ADR 0086 decision 10)
  ```

  when two tables on chains that report the same id prove the same channel. The fix is the
  config, never the journal.

- The channel's **journal key does not change** (`evm:0x…`, `solana:…`), so neither does the
  `X-TOON-Payer` value an app receives.

## Before and after: `connector.toml`

**Before** (refused by name from this build onward):

```toml
client_edge_addr = "0.0.0.0:3000"
state_dir        = "/app/state"

[node]
addresses     = ["g.example.a"]
http_endpoint = "https://a.example/ilp"

[signer]
key_file = "/app/data/signer.key"

[settlement.evm]
rpc_url              = "https://base-sepolia-rpc.publicnode.com"
token_address        = "0x0C996d7c934c79a6255254875607Fe69df25C0E1"
decimals             = 6
asset_eip712_name    = "USDC"
asset_eip712_version = "2"

[settlement.evm.key]
key_file = "/app/data/settlement.key"

[[peers]]
id       = "b"
endpoint = "wss://b.example:443/btp"
fee      = 100

[[peer_channels]]
peer_id        = "b"
voucher_signer = "0x3333333333333333333333333333333333333333"
```

**After:**

```toml
client_edge_addr = "0.0.0.0:3000"
state_dir        = "/app/state"

[node]
addresses     = ["g.example.a"]
http_endpoint = "https://a.example/ilp"

[signer]
key_file = "/app/data/signer.key"

[settlement.eip155.84532]               # was [settlement.evm]: the chain id eth_chainId answers
rpc_url              = "https://base-sepolia-rpc.publicnode.com"
token_address        = "0x0C996d7c934c79a6255254875607Fe69df25C0E1"
decimals             = 6
asset_eip712_name    = "USDC"
asset_eip712_version = "2"

[settlement.eip155.84532.key]           # was [settlement.evm.key]
key_file = "/app/data/settlement.key"

[[peers]]
id       = "b"
endpoint = "wss://b.example:443/btp"
fee      = 100

[[peer_channels]]
peer_id        = "b"
network        = "eip155:84532"         # new: the row names its network
voucher_signer = "0x3333333333333333333333333333333333333333"
```

What changed, and what is refused by name:

| Before                                             | After                                                                           | Notes                                                                                                                                                                       |
| -------------------------------------------------- | ------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `[settlement.evm]`, `[settlement.evm.key]`         | `[settlement.eip155.<chainId>]`, `[settlement.eip155.<chainId>.key]`            | Every key inside the table is unchanged. `[settlement.evm]` is refused as `[settlement.evm] was removed: EVM settlement is one table per network (ADR 0086). …`             |
| a table heading that is not a decimal chain id     | —                                                                               | `[settlement.eip155.{key}] is not keyed by a chain id: …`. No leading zero.                                                                                                 |
| —                                                  | `network = "eip155:<chainId>"` on `[[peer_channels]]` / `[[pay_channels]]` rows | Optional with one EVM table; **required** with several (`… names an EVM channel or signer but no network, and this node settles on several EVM networks …`). Never guessed. |
| a row's `network` naming no table of this node     | —                                                                               | `… has network '…', and this node has no settlement table for that network …`                                                                                               |
| a peering whose two rows are on different networks | —                                                                               | `peering '…' has its '[[peer_channels]]' row on … and its '[[pay_channels]]' row on …: both channels of a peering are on one network …`                                     |
| `evm:0x…`, `solana:<mint>` asset ids               | `eip155:<chainId>/erc20:0x…`, `solana:<genesis prefix>/token:<mint>`            | In `[[tokens]]` and anywhere else an asset is spelled.                                                                                                                      |

At boot each table's chain id is checked against `eth_chainId`, and two tables that report the
same id are refused. Several tables may name one `key_file`. `facilitator_url`,
`asset_transfer_method` and the EIP-712 name and version are per table, and so is
`rpc_via_socks_proxy`, with a circuit of its own per network.

## Mixed fleets: do not add a second EVM network yet

A node on `2026.10.23.1` or earlier filters a peer's self-description by `"evm"`, not by
network, and refuses a peer that publishes two EVM networks as `AmbiguousChain`. It cannot be
patched. **Do not add a second `[settlement.eip155.<chainId>]` table to any node until every
node it peers with has upgraded.** A node with one EVM table publishes one entry and peers with
an older node as before. A peering is on one network, both channels of it (ADR 0086 decision
11); moving value from one network to another is two peerings and a forward between them, which
is a forward between assets (ADR 0071, as 0086 decision 13 amends it).

## Upgrade checklist

1. Every client and every peer that pays this node sends `network` (`schema_version` 11 or later).
2. `cp -a` the state directory, or at least `outbound-channels.log`, somewhere the upgrade
   does not touch.
3. Rewrite `[settlement.evm]` to `[settlement.eip155.<chainId>]`, spell every asset in CAIP-19,
   and add `network` to channel rows where the node holds more than one EVM table.
4. Bump the pin. On first boot, expect no refusal above; a refusal at boot is a config problem
   with a name in it.
5. Only then add a second network, and only after every peer has done step 1–4.
