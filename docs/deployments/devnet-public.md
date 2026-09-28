# Solana Payment-Channel — Public Devnet Deployment Record

**Cluster:** Solana **public devnet** — `https://api.devnet.solana.com` (real devnet, NOT the
self-hosted local `solana-test-validator`). Devnet SOL only; no mainnet, no real funds.

**Deployed:** 2026-07-18

This deployment replaces the self-hosted local-validator Solana settlement target for the
devnet connector nodes. The connector settles `solana:devnet` channels against this program
using the mock-USDC SPL mint below.

> Restored 2026-07-30 (issue #567): this file was deleted along with the TypeScript connector
> package (`git show eb5ecea:packages/solana-program/deployments/devnet-public.md` is the
> original) and is checked back in verbatim below, with one amendment section appended --
> `packages/solana-program/src` has changed since this record was written (commit `cedd0170`,
> issue #581, added claim/settlement-destination validation), so the original "Binary" section's
> 105,128-byte figure describes the _original_ deploy, not what a build of the _current_ source
> produces or what is live on chain today. See "Provenance amendment" below.

## On-chain addresses

| Item                    | Value                                                     |
| ----------------------- | --------------------------------------------------------- |
| Program ID              | `2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip`            |
| ProgramData account     | `omq3fmopaCbHHr4tT2UJyU46NztSCeA76UYQWDbprJ9`             |
| Upgrade authority       | `AEPoA5xTTJY9SR8c5CfsemFGC5TmxQBe6Xf6wewEtnYa` (deployer) |
| Mock USDC mint (6 dp)   | `xyc5J8MgKFiEN13PnfftdXxUzYH34FEvw1LCrFwN7in`             |
| Mint authority          | `AEPoA5xTTJY9SR8c5CfsemFGC5TmxQBe6Xf6wewEtnYa` (deployer) |
| Treasury ATA (deployer) | `9JvfXPAVox3EvYHV1CNTHeZ3h7yFkWgHuRey58JtwYBL`            |
| Treasury supply         | 100,000,000 USDC (100000000000000 base units, 6 dp)       |
| Deployer / fee-payer    | `AEPoA5xTTJY9SR8c5CfsemFGC5TmxQBe6Xf6wewEtnYa`            |

> Note: the program ID is **non-deterministic per `cargo build-sbf`** (a fresh program keypair
> was generated for this public-devnet deploy), so it differs from the local-validator id.

## Binary

- Source: `packages/solana-program/` (native Rust, non-Anchor), built with `cargo build-sbf`.
- `payment_channel.so` size: **105,128 bytes**.
- Program account data length on-chain: 105,128 bytes; `executable: true`.

## Transaction signatures

| Action                    | Signature                                                                                  |
| ------------------------- | ------------------------------------------------------------------------------------------ |
| Program deploy            | `p9qhAfyghEi59xvYfHU9Gvbnaf49cYWsAtinbwoJR4YfxppAeQyJRj1Y7u1ePiCFiDAZtd6j2y7NtgguX1FXcEX`  |
| Create USDC mint          | `5ujTr7fyUfPgoQrNYcXNXtmPj5RozAJxQKj21iqZsPUu9ETfMnBujvELre4kdnCYyAn4omAwGio4oHaj3tR9rTBz` |
| Create treasury ATA       | `4xMPwBikFHLVpF71YceJs6dRximSKvDxs9NdLAdqACeMX14N5qWEPGpTLUSDDAp2SgueLeJLMgGG558Tb1UeCxG5` |
| Mint 100M USDC → treasury | `4mf3BcqmpPegyZUechWHn6SnoTmYtcAxqHKS4EKW5Ts9d1MMv9nuU4VFGq9ZaQX5cVxmxpRrLGMFx8RGMW6PbKQv` |

## Explorer links (cluster=devnet)

- Program: <https://explorer.solana.com/address/2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip?cluster=devnet>
- Mock USDC mint: <https://explorer.solana.com/address/xyc5J8MgKFiEN13PnfftdXxUzYH34FEvw1LCrFwN7in?cluster=devnet>
- Deploy tx: <https://explorer.solana.com/tx/p9qhAfyghEi59xvYfHU9Gvbnaf49cYWsAtinbwoJR4YfxppAeQyJRj1Y7u1ePiCFiDAZtd6j2y7NtgguX1FXcEX?cluster=devnet>

## Cost

- Program rent (held in ProgramData account): ~0.733 SOL for the 105,128-byte binary deployed
  2026-07-18. Reconciles exactly with Solana's rent-exemption formula:
  `(105,128 + 45-byte ProgramData header + 128) * 6,960 lamports/byte = 732,894,960 lamports`.
- Total deploy + mint spend: ~0.74 SOL (deployer went 2.0 → 1.2619 SOL).
- This figure describes the 2026-07-18 deploy only, not the current source: per the "Provenance
  amendment" below, `packages/solana-program/src` has since grown to a 109,401-byte binary,
  which implies `(109,401 + 45 + 128) * 6,960 = 762,635,040` lamports, i.e. ~0.76 SOL. See
  `docs/solana-deployment.md`'s "Deployment Cost Estimates" for the up-to-date figure and the
  formula to recompute it after any future size change.

## Verification

```console
$ solana program show 2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip --url https://api.devnet.solana.com
Program Id: 2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip
Owner: BPFLoaderUpgradeab1e11111111111111111111111
ProgramData Address: omq3fmopaCbHHr4tT2UJyU46NztSCeA76UYQWDbprJ9
Authority: AEPoA5xTTJY9SR8c5CfsemFGC5TmxQBe6Xf6wewEtnYa
Data Length: 105128 (0x19aa8) bytes

$ spl-token supply xyc5J8MgKFiEN13PnfftdXxUzYH34FEvw1LCrFwN7in --url https://api.devnet.solana.com
100000000
```

## Connector chainProvider config (`solana:devnet`)

Matches the `infra/linode-node/connector.yaml` box format. `keyId` is overwritten from the
node's `TOON_MNEMONIC` at boot; `SOLANA_PRIVATE_KEY` env may also supply the raw key.

```yaml
- chainType: solana
  chainId: solana:devnet
  rpcUrl: https://api.devnet.solana.com
  wsUrl: wss://api.devnet.solana.com
  programId: '2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip'
  tokenMint: 'xyc5J8MgKFiEN13PnfftdXxUzYH34FEvw1LCrFwN7in'
  keyId: 'placeholder-overwritten-by-mnemonic'
  cluster: devnet
  settlementOptions:
    threshold: '5000'
    pollingIntervalMs: 100
    settlementTimeoutSecs: 3600
    initialDepositMultiplier: 2
    ledgerSnapshotPath: /app/data/ledger-solana.json
```

## Reproduce / redeploy

Keypairs used for this deploy live outside the repo (scratchpad, not committed). To redeploy
or upgrade:

```bash
# ../../../tools/solana/build-sbf.sh, never a bare `cargo build-sbf`: the bare
# form takes the CLI's default platform-tools line, which is the 112,513-byte
# binary the comparison table below calls the wrong one. `make
# solana-deploy-devnet` (tools/solana/deploy.sh) does this whole block pinned
# and is the supported path; these commands are what it runs.
cd packages/solana-program && ../../../tools/solana/build-sbf.sh
solana program deploy target/deploy/payment_channel.so \
  --program-id <program-keypair.json> \
  --upgrade-authority <deployer.json> \
  --url https://api.devnet.solana.com
# mock USDC:
spl-token --url https://api.devnet.solana.com create-token --decimals 6 <mint-keypair.json>
spl-token --url https://api.devnet.solana.com create-account <MINT>
spl-token --url https://api.devnet.solana.com mint <MINT> 100000000
```

## Provenance amendment (2026-07-30, issue #567)

The "one gap" issue #567 named -- "nobody checked in a hash of the deployed 105,128-byte binary,
so the repo proves _which source tree_ was built, not _which commit of it_" -- is not fully
closed by this restoration. What was actually established, read-only, against public devnet
(no transaction submitted, no funds spent):

```console
$ curl -s https://api.devnet.solana.com -X POST -H 'Content-Type: application/json' -d '
  {"jsonrpc":"2.0","id":1,"method":"getAccountInfo",
   "params":["omq3fmopaCbHHr4tT2UJyU46NztSCeA76UYQWDbprJ9", {"encoding":"base64"}]}'
```

reports the **live** `ProgramData` account is **115,413 bytes** total (a 45-byte
`UpgradeableLoaderState::ProgramData { slot, upgrade_authority_address: Some(_) }` header, then
**115,368 bytes** of ELF bytecode, possibly including trailing zero padding from the loader's own
account sizing) -- not the 105,128 bytes this record's original "Binary" section states. That is
consistent with an upgrade having landed since the 2026-07-18 deploy: `packages/solana-program/src`
changed in commit `cedd0170` (issue #581, "validate settlement destinations, bound claims by
deposit") after `eb5ecea` introduced this record, adding code (new error variants, the
`validate_settlement_destination` check) that would grow the binary. The upgrade authority
(`AEPoA5xTTJY9SR8c5CfsemFGC5TmxQBe6Xf6wewEtnYa`) ~~can upgrade in place without changing the
program id~~ **could have, on 2026-07-30 when this amendment was written**; that key is now lost,
so an in-place upgrade is no longer available -- see "The program cannot be upgraded in place"
under the Mint amendment below. The size growth is still expected and not itself a discrepancy --
but it does mean the original 105,128-byte figure is **stale** and must not be treated as still
describing the live program.

**Reproducible-build comparison against the live bytes (2026-07-30, issue #567).** `cargo
build-sbf` had stopped resolving entirely -- crates.io supply-chain drift, not a source change:
several unpinned transitive dependencies (`blake3`'s `constant_time_eq` chain,
`zeroize`/`zeroize_derive`, `proc-macro-crate`'s `toml_edit`/`toml_datetime` chain, `indexmap`)
published versions requiring Rust's 2024 edition, which the SBF toolchain's frozen `cargo
1.79.0` cannot parse. Fixed on this same branch by pinning all of them back to
pre-edition-2024 versions in the workspace `Cargo.lock` (`blake3 1.5.5`, `zeroize 1.8.1`,
`zeroize_derive 1.4.2`, `proc-macro-crate 3.1.0`, `indexmap 2.11.4`), so `cargo build-sbf`
resolves deterministically again from a clean checkout.

With the build working, the comparison (live bytes fetched read-only as above; `raw[45:]`
stripped of the loader's trailing zero padding):

| Build                                                          | `.so` size (zero-stripped) | sha256 (zero-stripped)                                                |
| -------------------------------------------------------------- | -------------------------- | --------------------------------------------------------------------- |
| live ProgramData (`raw[45:]`, stripped)                        | 109,401                    | `b49087ce3304b0516c6f869e590118b50b9bf227a2ee4f15651a1a78377125ae`    |
| `cargo build-sbf --tools-version v1.52` of current source      | 109,401                    | `573c12ad6e6de6dfbe106fd75f9e19f2dc0b41cb96a46dcb4dd72c61ac15615e`    |
| `cargo build-sbf` (v2.1.0 CLI default tools) of current source | 112,513                    | (differs -- wrong toolchain line, listed to show version sensitivity) |

The platform-tools **v1.52** build of the _current_ `packages/solana-program/src` matches the
live program **exactly in size** and is **99.66% byte-identical** (375 of 109,401 bytes differ,
in small, scattered runs -- the signature of residual SBF build nondeterminism such as embedded
paths or a transitive-dependency micro-version drift in the deploy-time lockfile, not of
different program logic; a code change moves sizes and offsets wholesale, as the default-tools
row shows). So the provenance chain now says: the live
`2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip` was built from this source tree, post-`cedd0170`
(issue #581), with the platform-tools v1.52 line -- the same `--tools-version v1.52` CI's
`solana-program` job pins. A byte-for-byte zero-diff would additionally need the exact
deploy-time lockfile and build path, which were not preserved; what closes the gap functionally
is `crates/connector-settlement-solana`'s integration tests, which load the artifact this source
builds into a real validator and prove the Rust backend's claims redeem against it.

---

## Mint amendment (2026-08-27) — the mock USDC mint moved, and why

**The mint above is retired.** `xyc5J8MgKFiEN13PnfftdXxUzYH34FEvw1LCrFwN7in` is still on chain
and still holds its 100M supply, but **its mint authority is lost.** This record said from the
start that "keypairs used for this deploy live outside the repo (scratchpad, not committed)"; the
scratchpad is gone. `AEPoA5xTTJY9SR8c5CfsemFGC5TmxQBe6Xf6wewEtnYa` is in no repository, on no
machine, and in no scratchpad that survives. Nobody can mint that token, and nobody can refill a
treasury holding it — which is what left the devnet faucet's Solana leg answering `503` from the
day the faucet moved to its own box (#919) with no repair path.

The fix was to change the mechanism rather than hunt for the key. The faucet now **mints on
demand** from a mint it is itself the authority of, exactly as its Base Sepolia leg already did
through an ungated `mint()`. Nothing about the arrangement depends on a key that only one place
holds.

### The live devnet settlement token

| Item                  | Value                                                                                                     |
| --------------------- | --------------------------------------------------------------------------------------------------------- |
| Mock USDC mint (6 dp) | `34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU`                                                            |
| Mint authority        | `Bg5YF6nCKe8aeJwoyovYpGr7Qj9ViGSXiH9JHE7tH98F` — the faucet box's own treasury                            |
| Freeze authority      | none                                                                                                      |
| Initial supply        | 0 — every token in circulation has been dripped                                                           |
| Created               | 2026-08-27, tx `3D5gX28jrXAA7ohr67XbTH3GGtvDsqejugLgegi7WAtoPWPUUd87dJ3wZBPaRmssa13M4d2pnahv3Vb3KaQxnrJy` |

Created by `infra/linode-faucet/create-devnet-usdc-mint.sh`, run on the faucet box, against the
treasury `infra/linode-faucet/generate-solana-treasury.sh` had generated there. The private half
has never left that box. If the box is lost, the recovery is to run both scripts again on its
replacement and re-pin the new address — cheap, and the reason this shape was chosen.

Explorer: <https://explorer.solana.com/address/34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU?cluster=devnet>

### The program cannot be upgraded in place

The lost key is also this program's **upgrade authority** (see "On-chain addresses" above:
`Upgrade authority: AEPoA5x… (deployer)`). So any change to `packages/solana-program/src` —
[#1036](https://github.com/toon-protocol/connector/issues/1036)'s lock, preimage release and
expiry refund among them — is a **fresh deploy at a new program id**, not an upgrade of this one.
That is not a small consequence: [ADR 0053](../../../docs/adr/0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md)
binds the settlement program into a claim's signed message, so a new program id is a new claim
domain and every open channel on the old one has to be drained or abandoned first. Plan that
deploy as a migration, not as a release.
