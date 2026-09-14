# Base mainnet deployment record (chainId 8453)

Mainnet deployment of the TOON payment-channel contracts against Circle's native USDC. Real funds.
Mirrors the format of `base-sepolia.md`.

The broadcast was delegated by the maintainer on 2026-08-31 and executed on 2026-09-01 by Drew
Pierson, the third-party operator whose node is the only one that uses it, from his own machine.
The public record with the verification transcript is
[toon-meta#341](https://github.com/toon-protocol/toon-meta/issues/341#issuecomment-5497203242);
this file is the in-repo copy. Every address and transaction below was re-read from chain on
2026-09-03 before this file was written.

- **Network:** Base mainnet (`chainId 8453`, chain key `evm:8453`)
- **RPC used for the broadcast:** https://mainnet.base.org
- **Deployed:** 2026-09-01. Registry and the script's capped `TokenNetwork` at block 50745567
  (2026-09-01T16:34:41Z); the registry-created `TokenNetwork` at block 50745815 (16:42:57Z).
- **Script:** `packages/contracts/script/DeployMainnet.s.sol` at connector commit `deded9f9`
  (broadcast record: `packages/contracts/broadcast/DeployMainnet.s.sol/8453/`)
- **Deployer, registry owner:** `0x5D6a47acD20750d4D1b54024B57BA739b7b6550A`, a dedicated key
  generated for this broadcast and held by the operator outside every repository
- **Explorer:** https://basescan.org

## Deployed contracts

| Contract                                        | Address                                      | Tx                                                                                    |
| ----------------------------------------------- | -------------------------------------------- | ------------------------------------------------------------------------------------- |
| TokenNetworkRegistry                            | `0x61d31e7Fd9a57A0611e29Bd7eB162f15AC8B3427` | `0x8499658d7afa0196fd2e8d8f9a508c8dbdce442e6db2ddb940fbd57a74bb140b` (block 50745567) |
| TokenNetwork (USDC), capped, from the script    | `0xF795B07Aea3CB86bf30Dfc52C01DEd6c08B43057` | `0x1d0afc440354617a699ecb5c3d5514a73d3b447e0ce635487752af7a26cd9839` (block 50745567) |
| TokenNetwork (USDC), registry-created, **LIVE** | `0xc24a18F16C589A1F7187840FB26211Aa4ec60Fa8` | `0x36e039d8688e2989b6cda56f566c2bbe429cc76264634fd326373ad5ed68fcb4` (block 50745815) |
| Circle native USDC (6 decimals), not ours       | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` |                                                                                       |

## Two TokenNetworks, and why the live one is not the script's

`DeployMainnet.s.sol` deploys its `TokenNetwork` directly, with soak caps (`maxChannelDeposit`
1,000 USDC, `maxChannelLifetime` 30 days), and deliberately does not register it. The Rust
connector binds `[settlement.evm] contract_address` only through
`TokenNetworkRegistry.getTokenNetwork(token)` (ADR 0059), so no node can be pointed at the capped
contract. That skew is [#1264](https://github.com/toon-protocol/connector/issues/1264), found the
same afternoon and still open.

To bring the node up the operator called `registry.createTokenNetwork(USDC)`, which produces what
the registry's factory path always produces: `maxChannelDeposit` of `1e24` base units (no
effective cap for a 6-decimal token), `maxChannelLifetime` 365 days, and `owner()` equal to the
registry itself. State the consequences plainly:

- Every mainnet channel lives in `0xc24a18F1…`. The capped `0xF795B07A…` holds nothing and nothing
  points at it.
- `TokenNetwork` is not upgradeable. Its owner holds `pause()` and, while paused,
  `emergencyWithdraw`. On the live contract the owner is the registry, which exposes neither
  call, so **nobody holds emergency powers over mainnet channel funds.** On the capped contract
  the owner is the deployer key.
- The registry's `owner()` is the deployer key. `trustedForwarder()` is the zero address: no
  ERC-2771 forwarder was deployed on mainnet.

Verified on-chain 2026-09-03 with `cast call` against `https://base-rpc.publicnode.com`:

- `registry.getTokenNetwork(0x833589…) == 0xc24a18F1…`
- `registry.owner() == 0x5D6a47ac…`
- `registry.trustedForwarder() == 0x0000000000000000000000000000000000000000`
- live `0xc24a18F1…`: `owner() == 0x61d31e7F…` (the registry), `token() == 0x833589…`,
  `maxChannelDeposit() == 1000000000000000000000000`, `maxChannelLifetime() == 31536000`
- capped `0xF795B07A…`: `owner() == 0x5D6a47ac…`, `maxChannelDeposit() == 1000000000`,
  `maxChannelLifetime() == 2592000`
- `USDC.decimals() == 6`

Not done: a byte-for-byte match of either runtime bytecode against a local `forge build` of
`deded9f9`, and Basescan source verification (needs an Etherscan API key).

## First use, 2026-09-01, same day

The operator's node (`g.drew`) moved `[settlement.evm]` from Base Sepolia to this registry the same
afternoon and retired its Sepolia leg, making it the first connector settling on two mainnets
(its Solana leg is `packages/solana-program/deployments/mainnet-beta.md`).

| Step             | Value                                                                                                                                                                                                                                                                       |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Channel          | `0x90914b41a7680aa3fd7a20167bea8168ab477290a44526bd2bde4467dbf4c868` on `0xc24a18F1…`; payer `0x5D6a47ac…` (the deployer key), node settlement key `0x37b44bD509Fcf24a55b8097894DF6fd332ee4379`, timeout 3600 s, deposit 5 USDC                                             |
| First paid write | kind:5094 two-hop write through the org's `devnet_store_leg_probe`: EVM claim nonce 3, cumulative 5,030 units; the edge charged 1,030 and the store hop 930, both exact ADR 0065 schedules; Arweave `CPAHw8IZhTgtFX4BEeeRxz8XNgRed2E0qZy0Hlr8-Bg`, read back byte-identical |
| First redemption | `POST /channels/<id>/redeem-latest` on the node, tx `0xbde536ddd0f1cc97a2b30fbcb0a51eaa04f18fae9287b6fb733666886b4a6523` (block 50746573, 2026-09-01T17:08:13Z): 5,030 USDC units moved from the channel to the node's settlement key, that key's first mainnet transaction |

The redeem receipt reads `to == 0xc24a18F1…`, carries a USDC `Transfer` log, and names the channel
id in the TokenNetwork's own log.

Two defects that write surfaced are in the tracker:
[#1265](https://github.com/toon-protocol/connector/issues/1265) (the probe's `fetch_price` ignored
`price_per_kib`; fixed by #1266) and a comment on
[#1027](https://github.com/toon-protocol/connector/issues/1027) (the store attributed the EVM
payment to its Solana peer channel).

## What has not happened on this deployment

- No channel has been closed, cooperatively closed or rescued on Base mainnet. The soak §1 rows for
  those paths are still open.
- No second operator. Payer and node belong to the same operator, so the hop-2 fee is circular.
- Basescan source verification.
- The `base-mainnet` preset promotion in the `toon` repo.

## Connector config (`[settlement.evm]`)

What the operator's node runs. `contract_address` is the **registry**; the node resolves the
`TokenNetwork` itself.

```toml
[settlement.evm]
rpc_url          = "https://base-rpc.publicnode.com"
contract_address = "0x61d31e7Fd9a57A0611e29Bd7eB162f15AC8B3427"  # TokenNetworkRegistry
token_address    = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"  # Circle native USDC
decimals         = 6
channel_index_from_block = 50745815
```

publicnode refuses the channel index's archive `eth_getLogs` backfill ("archive requests require a
personal token"); the node logs a WARN and falls back to direct chain reads. Cosmetic at the
current channel count; pick an archive-capable RPC if it matters.
