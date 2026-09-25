# EVM devnet deployment: `TokenNetwork` cutovers on Base Sepolia

The committed runbook for redeploying `TokenNetwork` to Base Sepolia devnet and repointing every
place this repo names a settlement contract address. `TokenNetwork` is not upgradeable, so every
change to it is a cutover of this shape, and this file accumulates them: issue #695's meta-tx-aware
deployment (#694, live 2026-08-06 to 2026-08-28) and ADR 0059's derived channel id (broadcast 2026-08-28,
live).
Read this before touching any of the files it names; `docs/devnet-pricing.md` exists for the same
reason on the pricing side (connector#785) -- a hand-edit on one box or one file, unreconciled with
the rest, is exactly the failure mode both documents exist to prevent.

## Third cutover, BROADCAST 2026-09-25: the token (#1337)

The first two cutovers moved `TokenNetwork` and kept the token. This one moves the **token** and
keeps the registry. The devnet USDC is now Circle's **FiatToken v2.2**,
`0x0C996d7c934c79a6255254875607Fe69df25C0E1`, with a fresh `TokenNetwork`,
`0x1B4606218ceE5Bf02B546e416905F4D3FC8a0249`, created through the live registry `0x0c41D9D4…`.

**Why.** The mock ERC-20 it replaces, `0x49beE1…a9Ce`, had no ERC-3009 and no EIP-2612, and could
not be upgraded. So an x402 `batch-settlement` deposit of it (ADR 0074) always needed a
gas-paying Permit2 `approve`, and x402.org's hosted facilitator does not sponsor that step (ADR 0074
prerequisite 2). With ERC-3009, any stock facilitator makes a deposit gasless. That is proven on
chain: tx `0xa25e41ae…ce45`, from a wallet holding 0 ETH. FiatToken v2.2 is also the code Base
mainnet USDC runs, so the devnet now settles in the token production settles in.

**What changed for users.** Minting is **minter-gated**. The faucet key is the one minter, so
devnet USDC comes from the faucet, or from a key the master minter configures. Channels on the old
`TokenNetwork` `0xe9E05dfe…` are stranded, as in the second cutover: this is mock money minted on
demand, so an operational reset, not a loss.

**The record:**

- the deployment: `packages/contracts/deployments/base-sepolia.md`, "Devnet USDC cutover";
- the repoint checklist: issue #1337;
- the keys: `/root/keys/devnet-usdc-{owner,proxy-admin}.key` on the devnet box.

This repository's pins moved in the cutover PR:

- `infra/linode/endpoints.json`;
- the relay and store fixtures, and `swap.config.json`;
- `devnet_configs_load.rs`'s `EXPECTED_SETTLEMENT_TOKEN_ADDRESS` and `FLEET_LIVE_TOKEN_NETWORK`;
- the faucet's default;
- both funded-ops workflows' `TOKEN`;
- the README.

The live boxes run their own repos' deploy bundles (ADR 0068), so each of those moves in its own
repo.

## Status: BOTH cutovers have broadcast; ADR 0059's (2026-08-28) is the live one

This document covers two cutovers of the same shape. The 2026-08-06 ERC-2771 one is recorded
below exactly as it was written and is now **superseded**. ADR 0059's derived-channel-id redeploy
**broadcast 2026-08-28 at block 46055303** — registry `0x0c41D9D424d6B075A3cEa1068a694f7847a8CCa5`,
`TokenNetwork` `0xe9E05dfecfe165266C88d73e61D483612651952a` — and is what every `[settlement.evm] contract_address`
in this repository now names; its record is
["Second cutover"](#second-cutover-broadcast-2026-08-28-adr-0059s-derived-channel-id) below.

### The ERC-2771 cutover (#695), broadcast 2026-08-06

Everything on the **code** side is done and proven against a real Base-Sepolia fork (this repo's
CI, `testnet-cutover-fork-test` job -- no broadcast, no secrets). The **broadcast** itself, and the
box repoint that follows it, are human-only steps that need a funded Base-Sepolia deployer key and
SSH/deploy access to the two devnet boxes -- neither of which this repo's automation holds (same
posture as the mainnet runbook in `packages/contracts/README.md`). This document is the runbook an
operator with that access follows.

**It has now been run.** Broadcast 2026-08-06 from deployer
`0xF29fD62C4848B9573C9b90adbF61b664F386d9CF` at block 45126069; both boxes were repointed and
restarted, and the live kind:10032 announce advertises the new `TokenNetwork` as of
2026-08-06T12:49:42Z. The deployed addresses are:

| Contract             | Address                                      |
| -------------------- | -------------------------------------------- |
| ERC2771Forwarder     | `0xf1b0B8BA9CA90A0779C382Fe4212a3D4C5646Ee9` |
| TokenNetworkRegistry | `0x8263BdD4eB4862395Cb4ef5dA5d637F4b047Eea1` |
| TokenNetwork (USDC)  | `0xa79C3b1dbcEA00a6d84735a134395D8eF6D6a478` |

Full record, transaction hashes and post-broadcast on-chain verification:
`packages/contracts/deployments/base-sepolia.md`. The tables below describing the
_pre-cutover_ deployment and the rollback target are kept as written -- the old registry is
deliberately untouched and is exactly what a rollback points back at.

## Second cutover, BROADCAST 2026-08-28: ADR 0059's derived channel id

> **Status: BROADCAST 2026-08-28 at block 46055303** (2026-08-28T01:01:34Z), from a dedicated deployer
> `0x0E1e13d0A87e99F66715441CdFadfCD273134ADc` funded with 0.001 ETH from the gas box's settlement key for the
> purpose (its key lives with the operator, outside every repository, and now owns the registry).
> Every in-repo item of the repoint checklist below is ticked in the same change that flipped this
> line; the node-repo and on-box items are ticked as they land. The full record — addresses,
> transaction hashes, on-chain verification, bytecode provenance — is
> `packages/contracts/deployments/base-sepolia.md`, "ADR 0059 cutover deployment (2026-08-28)".
>
> | Contract             | Address                                      |
> | -------------------- | -------------------------------------------- |
> | ERC2771Forwarder     | `0x350fCd266F95B1f5B84944E0C7e06C16B837FCAA` |
> | TokenNetworkRegistry | `0x0c41D9D424d6B075A3cEa1068a694f7847a8CCa5` |
> | TokenNetwork (USDC)  | `0xe9E05dfecfe165266C88d73e61D483612651952a` |
>
> One correction to the runbook as written, found on the broadcast: `PRIVATE_KEY` must be
> `0x`-prefixed. `vm.envOr("PRIVATE_KEY", uint256(0))` parses a bare 64-hex string as decimal
> and fails, so the script logs the keyless-simulation line and broadcasts nothing — the
> simulation's addresses come out, which is exactly the "worthless as a prediction" trap step 1
> warns about. The command below is corrected.
>
> **Written before [ADR 0068](adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md).**
> `infra/linode-relay/connector-rust.toml` and `infra/linode-store/connector-rust.toml` are now
> fixtures this repo's own tests boot, not what either box runs -- the relay and store boxes deploy
> from `toon-protocol/relay`'s and `toon-protocol/store`'s own `deploy/` bundles. Every row below
> naming one of those two files still has to change, because `devnet_configs_load.rs` still asserts
> against them, but changing them alone no longer repoints a live box: the same address has to land
> in the owning repo's own committed config too, and `:rust-release` is no longer moved from here at
> all (ADR 0068 retired `promote-to-fleet.yml`). This callout is the flag for whoever executes this
> runbook next; the checklist itself is not rewritten past it.

[ADR 0059](adr/0059-a-channel-is-derived-from-its-participants.md) deleted `TokenNetwork`'s global
`channelCounter` and derives `channelId = keccak256(p1, p2, channelEpoch[p1][p2])`, with the epoch
advancing in `settleChannel`. `TokenNetwork` is not upgradeable (see "Why a fresh deployment at all"
below -- the same reasoning, unchanged), so this is a second redeploy of exactly the same shape:
`DeployTestnetCutover.s.sol`, a fresh forwarder and registry, the **same** mock USDC.

**What is live on Base Sepolia now** (read off the chain 2026-08-28, after the broadcast):

| Question                        | The live `TokenNetwork` `0xe9E05dfe…` answers | The superseded `0xa79C3b1d…` |
| ------------------------------- | --------------------------------------------- | ---------------------------- |
| `channelCounter()`              | reverts — the global counter is gone          | `31`                         |
| `channelEpoch(address,address)` | `0` — the function exists                     | reverts                      |

`test/DeployTestnetCutover.fork.t.sol` asserts both columns against a live fork
(`testFork_Cutover_LiveDeploymentDerivesChannelIds` for the live one,
`testFork_Cutover_LiveDeploymentStillCarriesTheGlobalCounter` for the superseded one, and
`testFork_Cutover_DeploysDerivedChannelIdsAndLeavesTheCounterBehind` for a fresh simulated deploy).

### The ordering hazard: contract, both configs and the image tag are one matched set

This is not the 2026-08-06 cutover's hazard. That one repointed a contract whose interface the
connector already spoke, so a stale box was merely pointed at the wrong deployment. This one changes
**how a channel id comes into existence**, and the connector computes it:

- **A new image against the old contract is broken.** A connector built from `main` derives
  `keccak256(p1, p2, channelEpoch[p1][p2])` and reads `channelEpoch` to do it. The deployed
  `0xa79C3b1d…` has no such function, so the read reverts.
- **An old image against the new contract is broken.** A pre-ADR-0059 connector expects `openChannel`
  to mint a counter-derived id and learns that id from the `ChannelOpened` log. Its second
  `openChannel` for a pair it already has a live channel with now reverts `ChannelAlreadyExists` — a
  refusal that could never fire before — and nothing in it knows why.

So the order is forced: land and apply the new registry address in both boxes' own configs
**before** either box runs an image built from a `main` that derives the new channel id. Under
[ADR 0055](adr/0055-a-release-is-one-dispatch-and-the-ordering-rides-as-data.md) that ordering rode
as the release's `config-change-required` field and a `promote-to-fleet.yml` dispatch;
[ADR 0068](adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md)
retired that mechanism, so the ordering is now enforced by whichever repo pins each box's
connector tag — land the config in `toon-protocol/relay` / `toon-protocol/store` first, confirm it
applied, and only then bump that repo's own pinned tag to a build carrying this cutover. Getting it
backwards is the swap#134 shape regardless of which mechanism carries the ordering.

**The deploy itself is additive and safe to broadcast early.** It touches no existing contract.
Channels on the current `TokenNetwork` `0xa79C3b1d…` keep settling there with their counter-minted
ids, exactly as the 2026-07-18 deployment's channels kept settling after 2026-08-06 (AC4, and
`testFork_Cutover_DoesNotDisturbTheOldLiveDeployment`). **The cutover is the repoint, not the
deploy** — nothing changes for anyone until the configs move.

### Before the broadcast

1. **Rehearse it keyless against live chain state.** No key, no `--broadcast`, nothing sent:

   ```shell
   cd packages/contracts
   forge script script/DeployTestnetCutover.s.sol --fork-url https://sepolia.base.org
   forge test --match-path 'test/DeployTestnetCutover.fork.t.sol' --fork-url https://sepolia.base.org
   ```

   The script logs `PRIVATE_KEY not set -- running keyless simulation (no broadcast)` and prints
   `BASE_FORWARDER_ADDRESS` / `BASE_REGISTRY_ADDRESS` / `BASE_TOKEN_NETWORK_ADDRESS`. **Those
   addresses are the simulation's, not the broadcast's** — they come from Foundry's default sender,
   not the real deployer, and are worthless as a prediction. Do not write them anywhere.

2. **Check the deployer.** The 2026-08-06 broadcast ran from
   `0xF29fD62C4848B9573C9b90adbF61b664F386d9CF`, which held `0.0102 ETH` at nonce `27` on 2026-08-26.
   That same broadcast used roughly 5M gas across four transactions (forwarder deploy, registry
   deploy, `setTrustedForwarder`, `createTokenNetwork`) at a 0.006 gwei effective price, so the
   balance is ample — but read it again rather than trusting this line.

3. **Settle, or accept the loss of, live channels on `0xa79C3b1d…`.** They are not migrated and not
   destroyed; they simply stop being where new channels are opened. The token is a mock USDC minted
   on demand, so this is an operational reset, not a loss
   ([ADR 0059](adr/0059-a-channel-is-derived-from-its-participants.md), "The cost, stated").

### The broadcast

Human-only, exactly as the 2026-08-06 one was:

```shell
cd packages/contracts
PRIVATE_KEY=0x<funded-deployer-key> \
  forge script script/DeployTestnetCutover.s.sol --rpc-url https://sepolia.base.org --broadcast
```

Record `BASE_FORWARDER_ADDRESS`, `BASE_REGISTRY_ADDRESS` and `BASE_TOKEN_NETWORK_ADDRESS` from the
run's output before doing anything else. Every `<NEW …>` below means one of those three.

### The repoint checklist

Each line names the exact key in the exact file. Nothing here may be filled in ahead of the
broadcast: an address written before it exists is a guess, and a guess in a fleet config boots a box
pointed at nothing.

**The two box configs.** Both MUST agree: a claim one box accepts against a channel on the new
contract is unresolvable by a box still pointed at the old registry. Since ADR 0068, changing the
files below is necessary but **not sufficient** — they are fixtures `devnet_configs_load.rs` boots,
and the file that actually changes what a box runs is the matching one in `toon-protocol/relay` /
`toon-protocol/store`'s own `deploy/` bundle. Change both.

- [x] `infra/linode-relay/connector-rust.toml` → `[settlement.evm] contract_address` = `<NEW REGISTRY>`
- [x] `infra/linode-store/connector-rust.toml` → `[settlement.evm] contract_address` = `<NEW REGISTRY>`
- [ ] the matching field in `toon-protocol/relay`'s, `toon-protocol/store`'s and `toon-protocol/gas-station`'s own committed configs (each its own PR; the gas box holds the third connector on this chain since gas-station#5)

**Test literals that pin the live fleet identity.** These fail the Rust gate the moment the configs
above change, which is the point — they are the gate that says the two boxes agree with each other
and with this record.

- [x] `crates/connector-bin/tests/devnet_configs_load.rs` → `FLEET_LIVE_REGISTRY` = `<NEW REGISTRY>`
- [x] `crates/connector-bin/tests/devnet_configs_load.rs` → `FLEET_LIVE_TOKEN_NETWORK` = `<NEW TOKEN NETWORK>`
- [x] `crates/connector-settlement-evm/tests/channel_index_sync.rs` → `DEVNET_TOKEN_NETWORK` = `<NEW TOKEN NETWORK>`

**Live-chain workflows that name the registry.** Both drive real Base Sepolia and would otherwise
keep passing against the old deployment while quietly proving nothing about the one the fleet uses:

- [x] `.github/workflows/base-sepolia-redeem-gate.yml` → the job's `env: REGISTRY` = `<NEW REGISTRY>`
- [x] `.github/workflows/funded-ops.yml` → the base-sepolia job's `env: REGISTRY` = `<NEW REGISTRY>`
- [x] `crates/connector-bin/tests/base_sepolia_redeem_proof.rs` → `DEFAULT_REGISTRY` = `<NEW REGISTRY>`

**The swap node's own config and the published endpoint list.** The maker signs leg-A claims against
whatever `tokenNetworkAddress` says; leaving it on the old contract is the swap#134 failure with the
value merely stale instead of missing.

- [x] `infra/linode-relay/swap.config.json` → `chainProviders[0].registryAddress` = `<NEW REGISTRY>`
- [x] `infra/linode-relay/swap.config.json` → `chainProviders[0].tokenNetworkAddress` = `<NEW TOKEN NETWORK>`
- [x] `infra/linode/endpoints.json` → `evm.registryAddress` and `baseSepolia.registryAddress` = `<NEW REGISTRY>`
- [x] `infra/linode/endpoints.json` → `evm.tokenNetworkUsdc` and `baseSepolia.tokenNetworkUsdc` =
      `<NEW TOKEN NETWORK>`. These two are **already stale**: they still name `0x1E95493f…`, the
      2026-07-18 `TokenNetwork`, and were missed by the 2026-08-06 repoint. Fix them to the new
      value rather than carrying the error forward.

**Bookkeeping — the record of what is deployed.**

- [x] `packages/contracts/deployments/base-sepolia.md` → a new section in the shape of the existing
      "ERC-2771 cutover deployment (2026-08-06)" one: network, RPC, date, deployer, block, script,
      the three addresses with their deploy tx hashes, the `setTrustedForwarder` tx, and the
      post-broadcast on-chain verification actually run — including `channelEpoch(address,address)`
      answering and `channelCounter()` reverting on the new `TokenNetwork`. Mark the 2026-08-06
      section **superseded**, not deleted.
- [x] `packages/contracts/deployments.json` → `networks["base-sepolia"].contracts` gains the new
      `ERC2771Forwarder` / `TokenNetworkRegistry` / `TokenNetwork` entries alongside the existing
      `*_ERC2771` ones, each with `address`, `deployer`, `deployTxHash`, `blockNumber`, `deployedAt`.
- [x] `docs/evm-deployment.md` (this file) → this section's status line flips from **NOT BROADCAST**
      to the date and block it broadcast at, and the live-state table above is rewritten against the
      new deployment.
- [x] `docs/adr/0059-a-channel-is-derived-from-its-participants.md` → the Status line's sentence
      "**The redeploy this needs has not happened**" is replaced by the date it did.
- [x] `crates/connector-settlement-evm/contracts/BYTECODE-PROVENANCE.md` → redone against the new
      registry and `TokenNetwork` addresses. Its own text requires this: "If either contract is ever
      redeployed, this file must be redone against the new address."
- [x] `docs/operators/swap-node-bringup.md` and `docs/operators/peer-channel-migration.md` → the
      registry/`TokenNetwork` addresses quoted in their prose. Both are operator instructions; a
      stale address in one sends a human to the wrong contract by hand.

**On the boxes themselves, at restart.**

- [x] ~~Delete `evm-channel-index.json` from each box's `state_dir` volume before restarting.~~ No
      longer a manual step, since #1282: the snapshot records the chain id and the resolved
      `TokenNetwork`, and `EvmChannelIndex::open` refuses to resume one whose identity does not
      match what this node now indexes. A repoint therefore discards the old index by itself, logs a
      WARN naming the mismatch, and backfills clean. The old file is ignored, never repaired — so
      deleting it is still harmless, just unnecessary.
- [ ] Set `[settlement.evm] channel_index_from_block` in both configs to the new `TokenNetwork`'s
      deploy block. Unset means backfill from genesis — correct, but slow on a public chain — and
      since #1282 it is also the second, independent guard: a checkpoint below it is discarded even
      when the chain id and `TokenNetwork` both match.

### What NOT to repoint

The 2026-08-06 section's "what NOT to repoint" reasoning holds unchanged, and two of its items are
now dead rather than merely excluded:

- **No forwarder address goes in any config.** It is baked immutably into the deployed
  `TokenNetwork`'s bytecode (`ERC2771Context`) and the connector never acts as a relayer. Record it;
  do not configure it.
- **No connector config names a `TokenNetwork` directly.** `[settlement.evm] contract_address` is the
  **registry**, resolved to a `TokenNetwork` at connect time. The `TokenNetwork` literals in the
  checklist above are test pins and app configs, never the connector's own settlement config.
- **Mock USDC `0x49beE1Bca5d15Fb0963117923403F9498119a9Ce` does not move.** The script reuses it by
  default, so no balance and no faucet distribution is disturbed. Changing `token_address` anywhere
  is out of scope and would strand every funded account.
- **Do not touch the Solana leg.** `packages/solana-program` is unchanged by ADR 0059 — the PDA
  derivation it already had is what ADR 0059 gives the EVM leg — and
  [ADR 0053](adr/0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md) binds the program
  id into a claim's signed message, so a new program id would invalidate every claim already signed.
  `[settlement.solana] program_id` stays exactly as it is.
- **Do not fill in `deploy/connector-rust/connector.production.toml`.** It is a deliberately inert
  skeleton ([ADR 0056](adr/0056-production-is-a-named-empty-tier.md)) and
  `crates/connector-bin/tests/production_skeleton_is_inert.rs` fails the build if it stops being one.
- **The `[[peer_channels]] token_network` literal** the 2026-08-06 cutover deliberately left on the
  old contract no longer exists in any committed config: issue #872 removed the apex and with it both
  of this fleet's peerings. There is nothing left for this rule to protect.
- **`connector announce` is not a step.** The 2026-08-06 runbook ended with re-running it so the
  kind:10032 event advertised the new address.
  [ADR 0046](adr/0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md) removed that
  verb and the whole announce; the binary refuses it by name. Nothing advertises the address any
  more, so nothing needs re-announcing.

### Rollback: now TWO steps, not one

The 2026-08-06 rollback was one step because the old and new contracts spoke the same interface. This
one does not have that property, for the reason stated in the ordering hazard above: reverting the
configs alone leaves a derived-id image pointed at a counter-based contract, which is the broken
combination.

1. Revert `[settlement.evm] contract_address` to `0x8263BdD4eB4862395Cb4ef5dA5d637F4b047Eea1` in
   both boxes' committed configs (`toon-protocol/relay`, `toon-protocol/store` — ADR 0068; this
   repo's own `infra/linode-*/connector-rust.toml` fixtures too, so `devnet_configs_load.rs` keeps
   asserting the live value), and apply it.
2. Roll each box back to the last pre-ADR-0059 connector build, by that repo's own pin.

Both, in that order, or the fleet is in the broken state either way round. There is still no on-chain
action to undo: the new registry, `TokenNetwork` and forwarder simply stop being referenced, and the
2026-08-06 deployment was never touched.

## Why a fresh deployment at all (both cutovers)

`TokenNetwork` is **not upgradeable** -- there is no proxy pattern anywhere in
`packages/contracts/src` -- so ERC-2771 support (#694) can only ship as a new deployment.
`TokenNetworkRegistry.createTokenNetwork(token)` also reverts `TokenNetworkAlreadyExists` for a
token it has already registered, and the live registry already has a `TokenNetwork` registered for
devnet's mock USDC. So the cutover deploys a **new** `TokenNetworkRegistry` too (wired to a new
`ERC2771Forwarder`), and creates the new `TokenNetwork` through it for the **same** USDC token --
never a new token, so no existing balance or faucet distribution is disturbed. Channels on the old
registry/`TokenNetwork` are a separate contract and are not migrated; they keep settling and closing
exactly where they always did (AC4).

## The 2026-07-18 deployment, which #695 cut over from

From `packages/contracts/deployments/base-sepolia.md`, deployed 2026-07-18:

| Contract               | Address                                      |
| ---------------------- | -------------------------------------------- |
| TokenNetworkRegistry   | `0xcC9079adE929b168B54145f6d25262b64FAB9D5b` |
| Mock USDC (6 decimals) | `0x49beE1Bca5d15Fb0963117923403F9498119a9Ce` |
| TokenNetwork (USDC)    | `0x1E95493fEF46707E034b4a1945f25a8C76A1823D` |

This deployment has **no** trusted forwarder (`address(0)`) -- it predates #694 and cannot be
upgraded to add one.

## The #695 cutover deploy (2026-08-06)

`packages/contracts/script/DeployTestnetCutover.s.sol`, broadcast per
`packages/contracts/README.md`'s "Devnet ERC-2771 cutover runbook" section:

```shell
cd packages/contracts
PRIVATE_KEY=<funded-deployer-key-no-0x-prefix> \
  forge script script/DeployTestnetCutover.s.sol --rpc-url base_sepolia --broadcast
```

(`base_sepolia` is a `foundry.toml` alias for `$BASE_SEPOLIA_RPC_URL`, so that variable must be set
too -- see `packages/contracts/.env.example`.)

Deploys, in order: an `ERC2771Forwarder("TokenNetworkForwarder")`, a fresh
`TokenNetworkRegistry`, `registry.setTrustedForwarder(forwarder)`, then
`registry.createTokenNetwork(0x49beE1…)` -- the same mock USDC as above, now behind a
forwarder-aware `TokenNetwork`. The script logs `BASE_FORWARDER_ADDRESS`,
`BASE_REGISTRY_ADDRESS`, and `BASE_TOKEN_NETWORK_ADDRESS`; record them (this document's
"After a real broadcast" section below) before doing anything else.

## The #695 repoint (2026-08-06): what was repointed, and what NOT

The connector announces whatever `TokenNetwork` its `[settlement.evm]` config resolves through the
registry at boot (`crates/connector-config/src/announce.rs`, `crates/connector-settlement-evm/src/
lib.rs::connect`) -- there is no separate config field naming a `TokenNetwork` address directly, and
**no Rust config carries the forwarder address at all**: it is baked immutably into the deployed
`TokenNetwork`'s bytecode (`ERC2771Context`), and the connector itself never acts as a relayer, so
nothing needs to be told about it. Repointing the announce is therefore exactly one config value, in
exactly two files -- the fleet's surviving two boxes as of issue #872 (the apex, and its own
`infra/linode-node/connector-rust.toml`, are retired):

1. **`infra/linode-store/connector-rust.toml`** -- `[settlement.evm] contract_address` ->
   `BASE_REGISTRY_ADDRESS` from the broadcast.
2. **`infra/linode-relay/connector-rust.toml`** -- same field, same new value. Both boxes MUST agree
   -- a claim one box accepts against a channel opened on the new contract is unresolvable by a box
   still pointed at the old registry.

Then redeploy/restart both boxes and re-run `connector announce` (issue #784) so the live kind:10032
event advertises the new `TokenNetwork` address -- the next announce picks it up automatically, with
no announce-side config change needed.

> **"Exactly one config value" is true of the connector and false of the repository.** Because a
> node resolves the `TokenNetwork` at boot, none of its own config carries one -- which makes it
> easy to read the sentence above as "the registry is the only address a repoint moves". It is not.
> Four committed files publish the **resolved** `TokenNetwork` as a literal, and a literal cannot
> re-derive itself when the registry moves. They are in "Bookkeeping" below; do not stop at the two
> `.toml` files. This is not hypothetical: the 2026-08-06 broadcast repointed
> `infra/linode/endpoints.json`'s `registryAddress` and left the `tokenNetworkUsdc` immediately
> below it naming the retired contract, in both of that file's blocks, for three weeks. Nothing on
> the fleet broke, because nothing on the fleet reads that field -- which is exactly why nobody
> noticed, and why the miss belongs on a checklist rather than to an operator's memory.

### What this did NOT touch: the apex↔store peer channel (retired, issue #872)

> **History.** This subsection describes a `[[peer_channels]]` row that no committed config carries
> any more: issue #872 removed the apex and with it both of this fleet's peerings, so there is no
> peer channel left for a repoint to touch or to leave alone. Kept because it records why the field
> was deliberately excluded from #695's scope.

Both `.toml` files also had a `[[peer_channels]] token_network` literal
(`0x1E95493fEF46707E034b4a1945f25a8C76A1823D` at the time) -- the EIP-712 signing domain for the
apex↔store peer channel. That channel was opened **before** cutover, so per AC4 it kept settling
against the **old** deployment; this field is deliberately left unchanged by the cutover. Migrating
that specific peering to the new contract (closing it and opening a fresh one) is a separate
operational decision -- new channel funding and coordination between both box operators -- not part
of this repoint.

**Update (issue #822):** that migration is no longer deferred -- it is the standing split-brain
issue #822 exists to close. The repo-side diff (new `token_network`, placeholder `channel_id`/
`counterparty_key`) and the live open/fund/close/settle runbook now live at
[`docs/operators/peer-channel-migration.md`](operators/peer-channel-migration.md); the paragraph
above is kept as written because it correctly describes this document's own scope (#695's repoint),
not because the peer channel is still unmigrated.

### Bookkeeping that must be updated alongside the repoint

This is the whole list. A file is on it because it names a `TokenNetworkRegistry` or a resolved
`TokenNetwork` as a **literal** -- something that goes on describing the old deployment until a
human retypes it. Anything you add here, add with the guard that makes forgetting it loud.

**The registry moved, so these move:**

- **`crates/connector-bin/tests/devnet_configs_load.rs`** -- `FLEET_LIVE_REGISTRY` asserts the live
  registry address as a literal against both `.toml` files; update it to the new registry or this
  test fails the moment the boxes are repointed.
- **`infra/linode-relay/swap.config.json`** -- `chainProviders[].registryAddress`. Not a connector
  config; the rolling-swap maker reads it (swap#134, issue #983), and it settles on this same
  deployment. `devnet_configs_load.rs`'s
  `the_makers_leg_a_token_network_is_the_fleets_and_is_not_its_leg_b_channel` holds it to
  `FLEET_LIVE_REGISTRY`.

**The resolved `TokenNetwork` moved, so these move too** -- the ones the "exactly one config value"
note above warns about:

- **`infra/linode/endpoints.json`** -- `tokenNetworkUsdc` **and** `registryAddress`, in BOTH the
  `evm` block and its `baseSepolia` mirror: four values, not two. This is the hand-maintained
  document a third party configures itself from (`infra/linode/README.md`), and the only committed
  file that publishes the resolved address to people who are not this fleet. It is also the file
  the 2026-08-06 repoint half-finished. Held now by `devnet_configs_load.rs`'s
  `the_public_endpoints_document_names_the_fleets_live_evm_deployment` (against the constants, on
  every push) and by `.github/workflows/base-sepolia-redeem-gate.yml`'s first step (against the
  chain, on dispatch).
- **`infra/linode-relay/swap.config.json`** -- `chainProviders[].tokenNetworkAddress`, held to
  `FLEET_LIVE_TOKEN_NETWORK` by the same maker test. Leave `channelAddress` alone: it is the leg-B
  `RollingSwapChannel`, a different contract with a different ABI, and this cutover does not touch
  it.
- **`crates/connector-bin/tests/devnet_configs_load.rs`** -- `FLEET_LIVE_TOKEN_NETWORK`, the
  constant both of the above are held to. It is the one number a human types after reading the
  chain, so type it from a `cast call`, not from another document:

  ```shell
  cast call --rpc-url https://base-sepolia-rpc.publicnode.com \
    <BASE_REGISTRY_ADDRESS> "getTokenNetwork(address)(address)" \
    0x49beE1Bca5d15Fb0963117923403F9498119a9Ce
  ```

- **`crates/connector-settlement-evm/tests/channel_index_sync.rs`** -- `DEVNET_TOKEN_NETWORK`, the
  contract that test's `eth_getLogs` names (issue #970).

**Records of the deploy, which gain the new addresses without losing the old:**

- **`packages/contracts/deployments.json`** and **`packages/contracts/deployments/base-sepolia.md`**
  -- add the new forwarder/registry/TokenNetwork addresses, transaction hashes, and deploy date,
  the same way the pre-cutover deployment is recorded there today.
- **`crates/connector-settlement-evm/contracts/BYTECODE-PROVENANCE.md`** -- its own text says "If
  either contract is ever redeployed, this file must be redone against the new address." Redo it
  against the new registry/`TokenNetwork` addresses once they exist on chain.

**Deliberately NOT repointed**, so that a future reader does not "fix" them: the pre-cutover tables
in this document, `packages/contracts/test/DeployTestnetCutover.fork.t.sol`'s `OLD_REGISTRY` /
`OLD_TOKEN_NETWORK` (the fork test proves the old deployment is undisturbed), the captured-greeting
fixtures in `crates/connector-cli/src/announce.rs` and `crates/connector-client-edge/src/lib.rs`
(parser inputs recorded from a real pre-cutover greeting -- the addresses in them decide nothing),
and `docs/operators/peer-channel-migration.md`, which is about a channel that lives on the old
contract on purpose.

## Rollback from #695: one step

Because the old deployment is never touched (not destroyed, not paused, nothing migrated out of
it), rollback is exactly what AC5 asks for: revert `[settlement.evm] contract_address` back to
`0xcC9079adE929b168B54145f6d25262b64FAB9D5b` in both `.toml` files and redeploy/restart. The next
`connector announce` reverts the advertised address automatically -- there is no on-chain action to
undo, since the new registry/`TokenNetwork`/forwarder simply stop being referenced.

## #695's acceptance criteria, mapped to this document

- Meta-tx-aware `TokenNetwork` + forwarder deployed, addresses recorded -- the broadcast + "After a
  real broadcast" bookkeeping above.
- The live kind:10032 announce advertises the new address -- "After a real broadcast" step 1-2
  above; nothing else to configure.
- A channel opened after cutover settles through the forwarder from an EOA holding zero native gas
  -- proven against real forked chain state (with real forked USDC, funded by impersonating the
  live distributor, not minted) by
  `test/DeployTestnetCutover.fork.t.sol::testFork_Cutover_GaslessChannelLifecycleOnRealForkedUsdc`,
  and against local chain state by `test/TokenNetworkERC2771.t.sol` (#694).
- Channels opened before cutover still settle and close against the old deployment -- the old
  registry/`TokenNetwork` are never touched, proven by
  `testFork_Cutover_DoesNotDisturbTheOldLiveDeployment`; the apex↔store peer channel was the worked
  example of one deliberately left pointed at the old contract (see above -- that peering is gone
  as of issue #872, but the property it demonstrated is a property of the deployment, not of it).
- Rollback is one documented step -- "Rollback: one step" above.
