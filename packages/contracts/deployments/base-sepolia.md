# Base Sepolia deployment record (chainId 84532)

Public testnet deployment of the TOON payment-channel contracts + a 6-decimal
mock USDC, for the devnet nodes' EVM settlement to point at.

> **This deployment has no ERC-2771 trusted forwarder** and predates #694/#695. The devnet cutover
> to a meta-tx-aware `TokenNetwork` (deployed alongside a fresh registry + forwarder, reusing this
> same mock USDC) is tracked in `docs/evm-deployment.md` -- read that first if you are looking for
> the CURRENT live addresses; this file is the historical record of the deployment below and gets a
> new section once the cutover actually broadcasts.

- **Network:** Base Sepolia (`chainId 84532`)
- **RPC:** https://sepolia.base.org
- **Deployed:** 2026-07-18
- **Script:** `packages/contracts/script/DeployTestnet.s.sol`
- **Explorer:** https://sepolia.basescan.org

## Deployed contracts

| Contract                                           | Address                                      | Explorer                                                                        |
| -------------------------------------------------- | -------------------------------------------- | ------------------------------------------------------------------------------- |
| TokenNetworkRegistry                               | `0xcC9079adE929b168B54145f6d25262b64FAB9D5b` | https://sepolia.basescan.org/address/0xcC9079adE929b168B54145f6d25262b64FAB9D5b |
| Mock USDC ("USD Coin (mock)" / `USDC`, 6 decimals) | `0x49beE1Bca5d15Fb0963117923403F9498119a9Ce` | https://sepolia.basescan.org/address/0x49beE1Bca5d15Fb0963117923403F9498119a9Ce |
| TokenNetwork (USDC)                                | `0x1E95493fEF46707E034b4a1945f25a8C76A1823D` | https://sepolia.basescan.org/address/0x1E95493fEF46707E034b4a1945f25a8C76A1823D |

The TokenNetwork was created **through the registry** via
`registry.createTokenNetwork(usdc)`, so `registry.getTokenNetwork(usdc)` resolves
it — the connector needs only `registryAddress` + `tokenAddress` at runtime.
Verified on-chain: `getTokenNetwork(0x49beE1…) == 0x1E95493f…`.

## Transactions

| Step                              | Tx hash                                                              |
| --------------------------------- | -------------------------------------------------------------------- |
| Deploy TokenNetworkRegistry       | `0x3db004967999e24a51c61251534a1bd507e679d4db94cf71eb1a4b08de2f1e49` |
| Deploy Mock USDC (MockERC20)      | `0x60bf2264a0f543593e155732e194f50855a38e1d2d33b9ff3d21a426a0019b08` |
| registry.createTokenNetwork(USDC) | `0xb066cf35dd118d21ff269c60466b5bd5a922d56a4f38a968c6c012d2199046c5` |
| Mock USDC mint → deployer         | `0xf2855eea2a81157ffbd832cefd05528c62aeeac0945203661dd314a36a4c1ed5` |

## Deployer / distributor

- **Address:** `0x6bafedaF18FF62f0a63dd0148bafa163204627F6` (fresh, testnet-only)
- **USDC balance:** `101,000,000 USDC` (`101000000000000` base units) —
  1,000,000 from the MockERC20 constructor + 100,000,000 from the deploy-script mint.
  Held for distribution to node settlement identities / clients.
- The private key lives outside the repo (in the operator's key store); it is a
  throwaway Base Sepolia testnet key holding only mock funds.

## Connector chainProvider config (`evm:84532`)

Paste into `chainProviders:` in the node `connector.yaml`. `keyId` must be the
node's own EVM settlement key (e.g. derived from `TOON_MNEMONIC`); the value below
is a placeholder.

```yaml
- chainType: evm
  chainId: evm:84532
  rpcUrl: https://sepolia.base.org
  registryAddress: '0xcC9079adE929b168B54145f6d25262b64FAB9D5b'
  tokenAddress: '0x49beE1Bca5d15Fb0963117923403F9498119a9Ce'
  keyId: 'placeholder-overwritten-by-mnemonic'
  settlementOptions:
    threshold: '5000'
    pollingIntervalMs: 100
    settlementTimeoutSecs: 3600
    initialDepositMultiplier: 2
    ledgerSnapshotPath: ./data/ledger-evm-base-sepolia.json
```

> Testnet only. No mainnet, no real funds.

---

## ERC-2771 cutover deployment (2026-08-06) — SUPERSEDED 2026-08-28

> **Superseded by the ADR 0059 cutover below.** This `TokenNetwork` still carries the global
> `channelCounter` and has no `channelEpoch(address,address)`; it is untouched, and every channel
> opened on it keeps settling and closing there. Nothing new is opened on it: the fleet's
> `[settlement.evm] contract_address` moved to the new registry.

Issue #695. Broadcast of `packages/contracts/script/DeployTestnetCutover.s.sol`; the runbook is
`docs/evm-deployment.md`. `TokenNetwork` is not upgradeable, so meta-tx support (#694) shipped as a
fresh registry + forwarder. The **same** mock USDC above is reused, so no balance or faucet
distribution was disturbed.

- **Network:** Base Sepolia (`chainId 84532`)
- **RPC:** https://base-sepolia-rpc.publicnode.com
- **Deployed:** 2026-08-06
- **Deployer:** `0xF29fD62C4848B9573C9b90adbF61b664F386d9CF`
- **Block:** 45126069
- **Script:** `packages/contracts/script/DeployTestnetCutover.s.sol`

| Contract               | Address                                      | Deploy tx                                                            |
| ---------------------- | -------------------------------------------- | -------------------------------------------------------------------- |
| ERC2771Forwarder       | `0xf1b0B8BA9CA90A0779C382Fe4212a3D4C5646Ee9` | `0xcac52ad5e1d4e7ee1f5e167c5364c7c1611a1058a80a8d3ee250578f85c61b13` |
| TokenNetworkRegistry   | `0x8263BdD4eB4862395Cb4ef5dA5d637F4b047Eea1` | `0x060b57dbed552193081817602d8b1d814b6ab5362a8c04d01afddd2f9152273d` |
| TokenNetwork (USDC)    | `0xa79C3b1dbcEA00a6d84735a134395D8eF6D6a478` | `0xa7769bf1c2835d5de2a0d5631fa9abed98b7701caa0aacbe8b8c045253d63748` |
| Mock USDC (6 decimals) | `0x49beE1Bca5d15Fb0963117923403F9498119a9Ce` | unchanged — reused from the 2026-07-18 deployment                    |

`registry.setTrustedForwarder(forwarder)` was tx
`0x30ecdd34e9afd74b8e9ef8b7036b12267a3ce61694e3ddd4082857ed6dcea760`.

Verified on-chain after broadcast:

- `registry.getTokenNetwork(0x49beE1…) == 0xa79C3b1d…`
- `registry.trustedForwarder() == 0xf1b0B8BA…`
- `tokenNetwork.isTrustedForwarder(0xf1b0B8BA…) == true`
- `tokenNetwork.token() == 0x49beE1Bca5…` (same USDC)
- The **old** registry `0xcC9079ad…` still resolves `0x1E95493f…` — untouched, per AC4. Channels
  opened before the cutover keep settling there, including the apex↔store peer channel, whose
  `[[peer_channels]] token_network` literal is deliberately left on the old address.

The live kind:10032 announce advertises the new `TokenNetwork` (`tokenNetworks["evm:84532"]`) as of
2026-08-06T12:49:42Z.

## Devnet USDC cutover (2026-09-25) — CURRENT LIVE TOKEN

Issue #1337. The mock USDC below had no ERC-3009, no EIP-2612, no proxy and no owner, so an x402
`batch-settlement` deposit of it always needed a gas-paying Permit2 `approve` (ADR 0074
prerequisite 2). It is replaced by **Circle's FiatToken v2.2**, the code Base mainnet USDC runs.
It is deployed from the creation bytecode of Base Sepolia USDC's own deployment transactions
(`toon-protocol/infra` `sandbox/artifacts/evm/FiatToken*.creation.hex`, regenerated by that repo's
`scripts/fetch-artifacts.sh`). The registry and forwarder below are unchanged; only the token and
its `TokenNetwork` are new.

- **Deployed:** 2026-09-25, blocks 47284995–47285026
- **Proxy admin:** `0x897B79c495eD8C277efc36fB9C2305377767C702` (`/root/keys/devnet-usdc-proxy-admin.key` on the devnet box)
- **Owner, master minter, pauser, blacklister:** `0x9B965b22EE84e7594BEB4D749590594b245227c2` (`/root/keys/devnet-usdc-owner.key`)
- **Minter:** the faucet key `0x7eC0c44Fcd62042711b7005c5E96E2Ccc7a30dBd`, allowance `type(uint256).max`
- **EIP-712 / metadata:** name `USDC`, symbol `USDC`, currency `USD`, version `2`, 6 decimals
- **Links:** Circle's `SignatureChecker` at `0xbA3b60c21e28C41df4bABd90f228e1D368627DA6`, already deployed on Base Sepolia

| Contract                       | Address                                      | Tx                                                                   |
| ------------------------------ | -------------------------------------------- | -------------------------------------------------------------------- |
| FiatTokenV2_2 (implementation) | `0xfeb1e530cAa508d48f5177097499f056d64e7c8a` | `0x330d5dc2d642b4177e8e4c76125b3ef05f4672feced63d22ddaf32d0e8e23b3f` |
| **USDC (FiatTokenProxy)**      | `0x0C996d7c934c79a6255254875607Fe69df25C0E1` | `0xc20f9bd290bb84f368e9c373220cfd85758ac4419741bf50d9fbaccae199aadd` |
| **TokenNetwork (USDC)**        | `0x1B4606218ceE5Bf02B546e416905F4D3FC8a0249` | `0xba947142c3c9d1101684ea515680d1d476ff80533ca55e7fca5be156b02abeb7` |

Initialisation: `initialize` `0x8d521728…`, `initializeV2` `0xb94a2c0e…`, `initializeV2_1`
`0x952a0392…`, `initializeV2_2` `0x8773b6f3…`, and `configureMinter(faucet, max)` `0xe111f9bf…`.
`createTokenNetwork` was called through the live registry `0x0c41D9D4…`.

Verified on-chain after broadcast:

- `registry.getTokenNetwork(0x0C996d7c…) == 0x1B460621…`
- `tokenNetwork.token() == 0x0C996d7c…`
- `tokenNetwork.isTrustedForwarder(0x350fCd26…) == true`
- `tokenNetwork.channelEpoch(a, b) == 0` (ADR 0059's derivation)
- The proxy's implementation slot names `0xfeb1e530…`, and `admin()` is the proxy admin.
- `name()`, `version()` and `decimals()` return `USDC`, `2` and `6`, and `isMinter(faucet)` is true.
- **A gasless deposit works end to end:** a wallet holding 0 ETH deposited 1 USDC through
  x402.org's facilitator by ERC-3009. That was tx `0xa25e41ae0be66dfcba48b3c30ea174975fb860dd5590dd5f226ae7aff602ce45`,
  block 47285040, into channel `0xf11059cc…705a`.

**Stranded, deliberately:** every channel on `0xe9E05dfe…`, the `TokenNetwork` of the retired mock
USDC. They keep settling and closing where they are; nothing new should open there. The 2026-08-15
`RollingSwapChannel` is bound to the retired token too; see that section below.

## ADR 0059 cutover deployment (2026-08-28) — registry CURRENT, token superseded 2026-09-25

[ADR 0059](../../../docs/adr/0059-a-channel-is-derived-from-its-participants.md): a channel id is
derived from its two participants and a per-pair epoch, never from a global counter, because a
peering established from a URL (ADR 0058) has no channel id to be told and must compute one.
`TokenNetwork` is not upgradeable, so this is the same shape as the 2026-08-06 cutover — a fresh
forwarder + registry + `TokenNetwork` from `packages/contracts/script/DeployTestnetCutover.s.sol`,
the **same** mock USDC reused. The runbook is `docs/evm-deployment.md`, "Second cutover".

- **Network:** Base Sepolia (`chainId 84532`)
- **RPC:** https://sepolia.base.org
- **Deployed:** 2026-08-28 (2026-08-28T01:01:34Z)
- **Deployer:** `0x0E1e13d0A87e99F66715441CdFadfCD273134ADc` — a dedicated key generated for this broadcast; it owns the registry
- **Block:** 46055303
- **Script:** `packages/contracts/script/DeployTestnetCutover.s.sol` (broadcast record:
  `packages/contracts/broadcast/DeployTestnetCutover.s.sol/84532/`)
- **Source:** `packages/contracts/src` at connector commit `c714551a` (`forge build`, solc 0.8.26,
  optimizer 200 runs, via-IR — the committed `foundry.toml`)

| Contract               | Address                                      | Deploy tx                                                            |
| ---------------------- | -------------------------------------------- | -------------------------------------------------------------------- |
| ERC2771Forwarder       | `0x350fCd266F95B1f5B84944E0C7e06C16B837FCAA` | `0x983988504363bfd84549bd3a2c2bcb7e49bad13e074d6448df29dbc5862884f1` |
| TokenNetworkRegistry   | `0x0c41D9D424d6B075A3cEa1068a694f7847a8CCa5` | `0x26299c72e663a5c4f985d90330c0a431336e4ba11ef372c2ff67656ca0a3297e` |
| TokenNetwork (USDC)    | `0xe9E05dfecfe165266C88d73e61D483612651952a` | `0xbd8a0583189f33347b2cd594b86119a7309d0f5b0ea91df393cfd921605a11d0` |
| Mock USDC (6 decimals) | `0x49beE1Bca5d15Fb0963117923403F9498119a9Ce` | unchanged — reused from the 2026-07-18 deployment                    |

`registry.setTrustedForwarder(forwarder)` was tx `0x2017c45b0dfa69209a54dda1c2851c2e658322c9566a57add1f53ec9e5161273`. All four
transactions landed in block 46055303; 4,801,808 gas in total.

Verified on-chain after broadcast (`cast call` against `https://sepolia.base.org`):

- `registry.getTokenNetwork(0x49beE1…) == 0xe9E05dfe…`
- `registry.owner() == 0x0E1e13d0…`
- `registry.trustedForwarder() == 0x350fCd26…`
- `tokenNetwork.isTrustedForwarder(0x350fCd26…) == true`
- `tokenNetwork.token() == 0x49beE1Bca5…` (same USDC)
- `tokenNetwork.channelEpoch(a, b)` answers `0` — the function exists (ADR 0059)
- `tokenNetwork.channelCounter()` **reverts** — the global counter is gone
- The superseded `0xa79C3b1d…` still has no `channelEpoch` (reverts) and the old registry
  `0x8263BdD4…` still resolves it — untouched; channels opened there keep settling there.
- Runtime bytecode of both new contracts matches the local `forge build` byte-for-byte
  (`crates/connector-settlement-evm/contracts/BYTECODE-PROVENANCE.md`, 2026-08-28 section).

## RollingSwapChannel deployment (2026-08-15)

The rolling-swap leg-B settlement contract (connector#973, epic toon-meta#394), deployed via
`funded-ops.yml`'s `deploy-rolling-swap-channel` verb (dry run 31885868413, apply run 31885961037).
See `docs/rolling-swap-channel-deployment.md` for the full runbook and verification detail.

- **Network:** Base Sepolia (`chainId 84532`, chain key `evm:84532`)
- **RPC:** https://base-sepolia-rpc.publicnode.com
- **Deployer:** `0x2B00c21af9926F9222bC29B87f7e03004AbAd43e` (NIP-06 account index 0)
- **Block:** 45515248
- **Script:** `packages/contracts/script/DeployRollingSwapChannel.s.sol` (forge v1.7.1)

| Contract           | Address                                      | Deploy tx                                                            |
| ------------------ | -------------------------------------------- | -------------------------------------------------------------------- |
| RollingSwapChannel | `0xd329aBf86ceae23F904641F992ca90e3721FeF83` | `0x23bbebaf8bea0976861eb51883db3322c5cfafdd69fee82207e83dcb8b06c3a2` |

Constructor args: `token = 0x49beE1Bca5d15Fb0963117923403F9498119a9Ce` (the same mock USDC every
other devnet contract settles), `challengePeriod = 86400s` (the contract's own floor).

Verified on-chain after broadcast (by the apply run itself, never from the receipt alone):

- `token() == 0x49beE1Bca5…` and `challengePeriod() == 86400`
- `domainSeparator()` == the independently computed EIP-712 domain hash for the domain below
- `claimDigest(...)` for the golden-vector sample in `docs/rolling-swap-v2-digest-spec.md` == the
  same struct hashed off-chain via `TypedDataEncoder` for this deployment's real
  `(chainId, address)` pair
- `updateBalance(...)` against a never-opened channel reverts `InvalidChannelState()` — the
  entrypoint is live contract code

```
EIP712Domain(name="RollingSwapChannel", version="2", chainId=84532,
             verifyingContract=0xd329aBf86ceae23F904641F992ca90e3721FeF83)
```

No live announce advertises this address yet: advertising it under `tokenNetworks["evm:84532"]` is
the swap node's job (swap#102), blocked on the signer migration (swap#101), not on this deploy.
