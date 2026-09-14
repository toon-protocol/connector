# Solana Payment-Channel: Mainnet-Beta Deployment Record

**Cluster:** Solana **mainnet-beta**, `https://api.mainnet-beta.solana.com`. Real SOL, real
funds. This is not devnet and not a local `solana-test-validator`.

**Deployed:** 2026-08-14

This is the first deployment of the payment-channel program to Solana mainnet-beta, closing the
deploy half of [#834](https://github.com/toon-protocol/connector/issues/834). It was performed
by hand per the "Mainnet Deployment Runbook" in `docs/solana-deployment.md`, which
[#954](https://github.com/toon-protocol/connector/issues/954) added. Mirrors the format of
`devnet-public.md`.

Unlike the devnet record, **no mint was created and no treasury exists**. Mainnet channels
settle in Circle's native USDC. `deploy.sh` has no mint-creation path on any network, and
`infra/solana/create-usdc-mint.sh` refuses a mainnet-shaped RPC URL outright.

## On-chain addresses

| Item                    | Value                                                     |
| ----------------------- | --------------------------------------------------------- |
| Program ID              | `8e7BhzydH1EqL486tw6Lp99BXviH3i5JN8qNpMSNmHj3`            |
| ProgramData account     | `29MT16eh1GCdL4JWrJHjyTWu5ZMn217h25ojCvFpx2wc`            |
| Owner (loader)          | `BPFLoaderUpgradeab1e11111111111111111111111`             |
| Upgrade authority       | `DaYDFYeCFr6FFyZLGywZV1FPhfbyeuTW65EZ22meEqAi` (deployer) |
| Circle USDC mint (6 dp) | `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v`            |
| Deployer / fee-payer    | `DaYDFYeCFr6FFyZLGywZV1FPhfbyeuTW65EZ22meEqAi`            |

The USDC mint is a **recorded convention, not an on-chain constraint.** The program takes no
mint at deploy time and is mint-agnostic per channel: each channel names its own SPL mint at
`InitializeChannel`. `deploy.sh` writes the mint into `tools/solana/program-id.mainnet.json`
for operators to read and enforces nothing about it afterwards.

Upgrade authority is the deployer keypair, per the decision recorded on #834 before the
broadcast. That decision was forced rather than chosen: `deploy.sh` derives its
`--upgrade-authority` target with `solana-keygen pubkey <path>`, so the flag requires a keypair
file, and handing authority to a key held by someone else cannot happen during the deploy
itself. Moving it later takes a bare pubkey and one transaction signed by the current authority
alone.

Program IDs are **non-deterministic per deploy** (a fresh program keypair is generated), so this
id is unrelated to the devnet id `2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip`.

## Binary

- Source: `packages/solana-program/` (native Rust, non-Anchor).
- Built with `cargo build-sbf --tools-version v1.52`, the pin `deploy.sh` and CI both use.
- Build host: `aarch64-apple-darwin`, solana-cli **3.1.12** (matching CI's `v3.1.12` pin).
- `payment_channel.so` size: **109,416 bytes**.
- `sha256`: `57619b3068d2e51cc34a47cd3e76a0736e5c7a58b450cbbc72b5b352a44e529e`

### Reproducibility, verified against chain

The deployed bytecode was pulled back down and compared to the local artifact:

```
solana program dump 8e7BhzydH1EqL486tw6Lp99BXviH3i5JN8qNpMSNmHj3 onchain.so --url https://api.mainnet-beta.solana.com
head -c 109416 onchain.so | shasum -a 256
```

which yields `57619b3068d2e51cc34a47cd3e76a0736e5c7a58b450cbbc72b5b352a44e529e`, identical to the
local build. The dump is `max_len` bytes and zero-padded past the binary, hence the truncation.

An arm64 macOS build of this source produces the byte-identical size to CI's ubuntu build. Worth
recording because it was not a given: this file's own devnet predecessor notes a Docker build of
the same source at 112,513 bytes against 109,401 elsewhere.

## Program account sizing and rent

| Item                             | Value                                     |
| -------------------------------- | ----------------------------------------- |
| Binary size                      | 109,416 bytes                             |
| Upgrade headroom allocated (25%) | 27,354 bytes                              |
| `max_len` / on-chain data length | **136,770 bytes**                         |
| Rent-exempt deposit              | **953,123,280 lamports (0.95312328 SOL)** |

Reconciles exactly against the rent formula in `docs/solana-deployment.md`:
`(136,770 + 45 + 128) x 6,960 = 953,123,280`.

The headroom is the `deploy.sh` default for a mainnet initial deploy with no explicit
`--max-len`, taken deliberately: the Solana lifecycle still lacks close, coop-close and rescue,
so the binary is certain to grow, and `solana program extend` later costs more than the headroom
does now.

**Rent is a refundable deposit, not a burn.** It is reclaimable by closing the program, which
requires upgrade authority.

## Cost

| Item                                 | SOL            |
| ------------------------------------ | -------------- |
| Deployer funded                      | 1.3998         |
| Remaining after deploy               | 0.44497028     |
| Total moved                          | 0.95482972     |
| Of which refundable rent deposit     | 0.95312328     |
| **Irrecoverable (transaction fees)** | **0.00170644** |

## Transaction signatures

| Action | Signature                                                                                                   |
| ------ | ----------------------------------------------------------------------------------------------------------- |
| Deploy | `2yXbkgQ2ZC3iuuNneqpqsCakF7C1HefBQEnWDWjaZRhQC4HYYy4AaAcEyuCzojtnSpkdgPAcV7D9pipWDGMKck1b` (slot 439316400) |

## Verification

```
solana program show 8e7BhzydH1EqL486tw6Lp99BXviH3i5JN8qNpMSNmHj3 --url https://api.mainnet-beta.solana.com
```

Confirms `executable`, owner `BPFLoaderUpgradeab1e...`, data length 136,770, and the upgrade
authority above.

## Since deployment (last updated 2026-09-03)

The section that used to close this file said no channel had been opened against this program, no
node was configured to use it, and no channel had been closed on any chain. All three were true on
2026-08-14 and false by the time the record merged on 2026-09-03. What happened, all on this
program id, all re-read from `https://api.mainnet-beta.solana.com` on 2026-09-03:

| Date       | Event                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Evidence                                                                 |
| ---------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| 2026-08-14 | First channel opened and funded with 5 USDC by a third-party operator's client against its own node: `DcW6wGmZChYD674SnibLYwMJSWzdR4rYwrgq5ecc8efz`, vault `AHNgAKW4S6TBxHYQMv2wWZiUATqKGJh7XaSFNxr4JwbG`, challenge window 3600 s                                                                                                                                                                                                                                                                                                                                                | account owner is this program, 178 bytes, still open                     |
| 2026-08-15 | Full lifecycle on that channel, signed by the payer alone: `ClaimFromChannel` (nonce 1, 1,000 units) `3Aeoj2C8WZE7CM6PPBawYUkd1ijXZvMBNdipYyCsDLtBtqPDryoDWHyL2uMwSGCCB8Z6AmfUiDmDF2GUaErA5mFM`, `CloseChannel` `4s3sFwKSdNty4YjwRNje2R5UQ4YiawR2dpaDkWVx9Yy7H38xGSrL4DBCbmHHnMNg9gNiE7bx4Rwpm2QpR3abedJU`, `SettleChannel` `SD4czZTRyy8KLZk6W5zXiDucZhK3ZXGP3DVmGYpEe9T1DdEwcKhs9M9E8ky7DNtu8F4RvpFpxPdVoCujzUts7EW` (A=1,000, B=4,999,000). The first claim redeemed on chain on any TOON settlement family. The channel was then reopened at the same PDA, which surfaced #977 | all three Finalized                                                      |
| 2026-08-16 | First peer channel on any TOON connector, edge to store, 2 USDC: `27XcKjUVe3SbVfkrj72bqMcZf3QuEasGxci4kberf8fu`, tx `F9VnMj7FrMvsv7CBSrKaU9UMpGGZfbdhPvEQCasLc24hGjLTFv1fox4mmu3aqNzATiNsGsonJFUa7GKEHTecAdP`                                                                                                                                                                                                                                                                                                                                                                     | account owner is this program                                            |
| 2026-08-29 | Both outstanding V1 claims redeemed, then the program **upgraded in place to the ADR 0053 / #1082 balance-proof V2 build** (tx `Bys6wM3vWWLqVFWU2Wtwn4GjU3YitgR1EueCMd1sC48vUNHcmXcTG81apWbu7UmbP6fgXnvQJBxfMqQSgSrnsTY`, slot 442712107). Same program id, same authority, `max_len` and rent unchanged. Simulated A/B after the upgrade: a 96-byte V2 claim accepted, a 48-byte V1 claim rejected `Custom:8`                                                                                                                                                                    | `solana program show` reads Last Deployed In Slot 442712107; bytes below |
| 2026-08-29 | Both V2 claims redeemed through the operator surface (`POST /channels/:id/redeem-latest`): `DcW6wGmZ…` nonce_b 20 / transferred_b 500,000; `27XcKjUV…` nonce_a 9 / transferred_a 243,600                                                                                                                                                                                                                                                                                                                                                                                          | chain read-back                                                          |
| 2026-09-01 | The same node moved `[settlement.evm]` to Base mainnet with this Solana leg unchanged, the first connector settling on two mainnets                                                                                                                                                                                                                                                                                                                                                                                                                                               | `packages/contracts/deployments/base-mainnet.md`                         |
| 2026-09-03 | First kind:5096 gas-station jobs on mainnet, paid over `DcW6wGmZ…` (claim nonces into the 50s); one execute tx `4SuuVx7ZLmuEMZ22LYUrJGSpkMcPvNFFczWqMybNovECd95JYZ3ijyTsgPXdgPXz33dyhULd3cWWpNVNzj5eU6Un`                                                                                                                                                                                                                                                                                                                                                                         | Finalized                                                                |

### The bytes on chain are no longer the 2026-08-14 build

The "Binary" and "Reproducibility" sections above describe the V1 build that was deployed on
2026-08-14 (109,416 bytes, `57619b30…`). Since the 2026-08-29 upgrade the program data is the V2
build of `deded9f9`:

```
solana program dump 8e7BhzydH1EqL486tw6Lp99BXviH3i5JN8qNpMSNmHj3 onchain.so --url https://api.mainnet-beta.solana.com
head -c 109400 onchain.so | shasum -a 256
```

yields `c87cf232f211bb226b6960d164121b6ed728e8586e74eadd4676c6e02a1c8cbb` (re-run 2026-09-03), with
every byte past 109,400 zero. The `max_len` of 136,770 bytes from the initial deploy absorbed the
upgrade without `solana program extend`.

### Still not observed on this program

- Cooperative close. Close, claim-redeemed and rescue have each run once (2026-08-15).
- A second operator. Every channel above belongs to one operator's client, edge and store, so peer
  fees are circular and the observations prove routing and charging, not that anyone else earned.
- Soak: per toon-meta's soak criteria a family's clock cannot begin until every lifecycle path has
  one live observation, and the amendment admitting mainnet observations (toon-meta#395) has
  closed, so the remaining gap on this chain is coop-close alone.
