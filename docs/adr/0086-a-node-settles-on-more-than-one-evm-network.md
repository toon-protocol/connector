# A node settles on more than one EVM network

**Status:** Accepted (owner decision, 2026-10-28, #1566) — **not yet built**; the spec is
#1569. It **amends** [0071](0071-a-forward-crosses-a-denomination-at-a-declared-rate.md) (the asset
spelling, and decision 2's same-asset sentence),
[0073](0073-settlement-rpc-may-ride-the-circuit-once-every-wait-on-it-is-bounded.md) decision 3 (one
circuit per network), [0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)
decision 3 (one settlement key per network) and
[0076](0076-the-operator-names-the-facilitator-and-pays-its-gas.md) (the facilitator is named per
network). It **extends** [0040](0040-a-verified-payment-is-stated-to-the-app.md) with one header and
moves [0021](0021-vectors-are-normative-prose-is-not.md)'s `schema_version` from 10 to 11.

**Scope:** protocol law — binds every implementation, because a claim, an asset id and the vectors
change. See the [ADR index](README.md).

**Falsifier:** none — the record is not yet built, and what it refuses (`[settlement.evm]`,
`evm:0x…` as an asset, a claim with no `network`) is refused by name, which is a presence the tests
will hold rather than an absence a pattern can find.

**A node may hold one settlement table per EVM network, keyed by chain id, and every identifier that
says which network a value is on says so in CAIP form.** A claim names its network, an asset id is
CAIP-19, and the channel key stays as it is because an EVM channel id already binds its chain.

## Context

x402's `x402BatchSettlement` sits at one constant address on about eighteen EVM networks, each with
Circle USDC: Base, Arbitrum, Optimism, Polygon, Avalanche, Celo, Linea, Unichain, World Chain, Monad,
Ethereum and several of their testnets (`docs/research/multi-evm-networks.md` §10). The existing
backend and contract suites apply to every one of them unchanged. What stops a node using more than
one is the configuration and the identifiers:

- `[settlement.evm]` is one table, and `SettlementChain::Evm` one chain.
- An asset id is `evm:0x…` (0071), which does not say which network.
- A client claim dispatches on `blockchain: "evm"` and does not say which network either, so a node
  on Base and Arbitrum could not tell which backend a voucher is for.

Two facts narrowed the change (research §1, §4):

- **An EVM channel id already binds its chain.** `getChannelId` runs the channel config through an
  EIP-712 hash whose domain includes the chain id (`connector_signer::evm_batch_channel_id`). Two
  networks with different chain ids cannot produce the same id. The only collision is two tables on
  chains that report the same id, such as two anvils left at 31337.
- **What is not unique is the asset.** Every anvil `infra/anvil/seed.sh` seeds puts USDC at the same
  address, and Circle uses one USDC address on Arc mainnet and Arc testnet.

Hedera was the other reason to ask (research `x402-pending-chain-bindings.md` D4). It is not folded
in. Its x402 binding names its network `hedera:mainnet`, carries `0.0.x` entity ids on the wire,
routes signature checks through `0x16a` and has a deposit route of its own. It stays parked (#1571),
and gets a table of its own if it returns.

## Decision

### The wire

1. **Every EVM and Solana claim, claim-state challenge and payout voucher carries a required
   CAIP-2 `network`**: `eip155:<chainId>`, or `solana:<genesis prefix>` as the greeting already
   spells it. `blockchain` stays, and still picks the parser. A claim with no `network` is refused
   **by name**, even on a node that runs one network, rather than guessed. This is the field x402
   itself carries (`accepted.network`). A bare numeric chain id was refused: it repeats the shape of
   the retired `toon-channel` claim.
2. **An asset id is CAIP-19**: `eip155:<chainId>/erc20:0x…` and `solana:<genesis prefix>/token:<mint>`.
   `evm:0x…` and `solana:<mint>` are refused by name. `evm:<chainId>:0x…` was refused because today's
   parser would read it as a different token rather than refuse it.
3. **The vectors move to `schema_version` 11** in one bump that covers both chains, and `toon-client`,
   `rig` and `swap` ship before any node does. That order is safe: a node on the current release
   ignores an unknown `network`, and a claim for another network still fails safe there, because its
   channel id does not recompute under that node's domain.

### Configuration and keys

_Amended 2026-10-28 (merge of #1607):_ this record was written as a move from 9 to 10. ADR 0080's
`zk_note` section reached `main` first as schema 10 (#1608), so this bump is 11. 4. **One table per network: `[settlement.eip155.<chainId>]`.** `[settlement.evm]` is refused by name
(0009). At boot each table's chain id is checked against `eth_chainId`, and two tables that
report the same id are refused. Each is checked for x402's code on its own, as 0075 decision 1
requires of every table. 5. **Each table names its own `key`.** Several tables may name the same file: nonces are per chain
and each backend has its own sender, so a key shared across networks is safe. 0075 decision 3's
"one settlement key per chain" reads "per network". 6. **`facilitator_url`, `asset_transfer_method` and the token's EIP-712 name and version are per
table.** USDC's EIP-712 name differs by network ("USD Coin" on Base, "USDC" on Base Sepolia). The
greeting already gives one `accepts[]` entry per table, so 0076 holds network by network. 7. **Each table's RPC circuit is its own.** Under `rpc_via_socks_proxy`, each EVM table
authenticates to the SOCKS port with a username naming its network, so two EVM networks never
share a circuit (amending 0073 decision 3's "one pinned circuit per chain"). 8. **Channel rows in config name their network** with a new `network` key. `chain_id` is not reused:
it is already parsed on those rows only to be refused.

### State

9. **The journal's channel key does not change.** It stays `evm:0x…` and `solana:…`. A channel id
   is already unique across networks (Context), so the key needs no chain id, and the
   `X-TOON-Payer` value apps receive does not change.
10. **The outbound channel record moves to version 2, which records its network.** A version-1 record
    is upgraded in place at boot: its channel id is recomputed under each configured table's domain,
    and the one table that reproduces it is its network. A record that no table reproduces — the
    node still holds value on a network its operator removed — or that more than one does, is
    refused at boot **by name**. 0075 refused its legacy state outright, but that was a scheme with
    no successor; these are live x402 channels.

### Peering, forwarding and apps

11. **Both channels of a peering are on one network.** A peering whose halves would be on two
    networks is refused. A node that moves value from Base to Arbitrum does it with two peerings and
    a forward between them.
12. **`POST /peers` selects by `network`.** Its `chain` filter becomes a CAIP-2 network, and a peer
    that publishes several networks the node holds tables for, with no filter given, is refused as
    `AmbiguousNetwork`.
13. **A forward between networks is a forward between assets** (amending 0071 decision 2). USDC on
    Base and USDC on Arbitrum are different asset ids. A node that declares no `[[tokens]]` forwards
    them unconverted, as `mixed-chain` forwards Base to Solana today (XRPL is the exception: a node with
    an XRPL table must declare tokens, [0087](0087-xrpl-settles-over-the-ledgers-payment-channels.md)'s update for #1636). A node that declares tokens
    needs a declared rate for the pair, and 1:1 or a spread is the operator's statement of what
    holding its USDC on the other network is worth. 0071 decision 2's sentence calling USDC on Base
    and on Solana a same-asset pair is replaced by this one, which is what the code already does
    (`peering_asset.rs`'s `crosses` is plain asset-id inequality). A quote path cannot leave its
    token's network, so only tokens on the numeraire's own network quote themselves; every other
    network's tokens take a static rate. Rate sources are keyed by network, one per EVM table.
14. **Apps are told the network in a new header, `X-TOON-Network`**, carrying the CAIP-2 network.
    `X-TOON-Chain` keeps its two values. Without it an app on a multi-network node could not tell
    whose base units `X-TOON-Amount` is in.

### Tests and scope

15. **`solo` gains a second anvil on a different chain id** and stays on the CI gate, so a
    multi-network node is checked by `make local-verify` without a sixth topology. `cargo test` uses
    `Anvil::spawn_with_chain_id`.
16. **Which networks the devnet fleet runs is not decided here.** That is each node repository's pin
    and the faucet's work: every added testnet needs a FiatToken the faucet can mint.

## Considered options

- **A separate `[settlement.hedera]` table, leaving `[settlement.evm]` single** (research D4 option
  b). It answers a question #1563 parked, and leaves a node on one canonical EVM network, which is
  the thing this record exists to change.
- **Put the chain id in the journal key** (`eip155:<chainId>:0x…`). One rule for every identifier,
  bought with a journal migration and a change to `X-TOON-Payer` that apps can see, to add
  information the channel id already carries.
- **Refuse version-1 state at boot.** Forces a drain of channels this build can settle. Recomputation
  proves each record's network, so there is nothing to guess.
- **Infer a missing `network` on a single-network node.** Kinder to an old client for one release,
  and a node whose answer changes when its operator adds a table. Refused by decision 1.
- **Change only EVM now, Solana later.** Two `schema_version` bumps for every downstream repository,
  and two asset-id grammars in the meantime, in which the legacy `solana:<mint>` reads as a CAIP-2
  network with its asset missing.
- **Widen `X-TOON-Chain` to the CAIP-2 network.** Changes a value apps already read.
- **A sixth `multi-evm` topology.** A second anvil in `solo` checks the same thing without another
  image boot in the CI job.

## Consequences

- **A breaking deploy, and a wire change.** Every committed `[settlement.evm]` table (13 TOML files
  here, and each node repository's) is rewritten, and `local/dealing/connector-b.toml`'s
  `evm:0x…` asset with it. The release gets a note in `docs/releases/` with the before and after.
- **Mixed fleets.** A node on the current release cannot peer with one that publishes two EVM
  networks: it filters by `"evm"`, not by network, and refuses as `AmbiguousChain`. Old nodes cannot
  be patched, so the release note says: do not add a second EVM network until every node you peer
  with has upgraded. A node with one EVM table publishes one entry, as today, and peers as before.
- **A dealing node hand-tends rates across networks.** Only tokens on the numeraire's network quote
  themselves by TWAP.
- **`SettlementChain::Evm` stops being a key.** The runtime's one EVM backend slot, its transport
  slot, `x402_network`, `ClientChannelAssets` and `RateSources` become lookups by network (research
  §11).
- **Hedera is untouched.** Decisions 4–8 apply to `eip155` networks only.
