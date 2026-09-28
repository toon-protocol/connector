# EVM devnet deployment: `RollingSwapChannel`

The committed runbook for connector#973 -- deploying the rolling-swap chain-B
settlement contract (`packages/contracts/src/RollingSwapChannel.sol`,
connector#315/#324) to Base Sepolia devnet and recording its address. Read
this before dispatching the workflow it names; `docs/evm-deployment.md` is the
sibling runbook for the `TokenNetwork` ERC-2771 cutover and follows the same
shape.

## Status: BROADCAST 2026-08-15

|                 |                                                                       |
| --------------- | --------------------------------------------------------------------- |
| address         | `0xd329aBf86ceae23F904641F992ca90e3721FeF83`                          |
| deploy tx       | `0x23bbebaf8bea0976861eb51883db3322c5cfafdd69fee82207e83dcb8b06c3a2`  |
| block           | 45515248                                                              |
| deployer        | `0x2B00c21af9926F9222bC29B87f7e03004AbAd43e` (NIP-06 account index 0) |
| chain           | `evm:84532` (Base Sepolia)                                            |
| token           | `0x49beE1Bca5d15Fb0963117923403F9498119a9Ce` (mock USDC, 6 dp)        |
| challengePeriod | 86400s                                                                |
| workflow runs   | dry run 31885868413, apply 31885961037 (`funded-ops.yml`)             |

The apply run's post-deploy verification all passed against the LIVE
contract: `domainSeparator()` matched the independently computed EIP-712
domain hash for `(RollingSwapChannel, "2", 84532, 0xd329aBf8…)` (AC3), the
spec's golden-vector sample hashed via `claimDigest(...)` matched the same
struct hashed off-chain for this real `(chainId, address)` pair (AC5), and
`updateBalance(...)` against a never-opened channel reverted
`InvalidChannelState()` (AC4). The first dry run also caught (and this
branch fixed) a dead key-extraction path: `fromMnemonicFull` exposes the
EVM key as the 32-byte NIP-06 `secretKey`, not an `evmPrivateKey` field.

The epic this closes is toon-meta#394 ("the rolling swap cannot complete a
packet"); this repo's slice (connector#973, "T4") is one of two unblocked
roots, run in parallel with the `swap` repo's signer migration (T1).

## Why a workflow, not a local `forge script --broadcast`

Unlike the mainnet policy in `packages/contracts/README.md` (issues
#388/#405 -- no funded deployer key ever reaches CI, broadcasts stay
human-only), this is a **devnet** deploy against Base Sepolia with a devnet
mock USDC token, the same class of operation `.github/workflows/
funded-ops.yml`'s existing `deposit`/`open-channel` verbs already perform
with a CI-derived key. connector#973's own text asks for exactly this:
"Privileged devnet operations go through the reviewed funded-ops workflow
rather than credentials held by the runner." No deployer private key is ever
generated for or held by an agent; `E2E_DEV_MNEMONIC` reaches only the
GitHub-hosted runner executing the reviewed workflow file, same boundary as
every other write in that file.

## The deploy

`.github/workflows/funded-ops.yml`'s `deploy-rolling-swap-channel` operation
runs the already-committed `packages/contracts/script/
DeployRollingSwapChannel.s.sol` via `forge script` against Base Sepolia
(`RPC_URL`/`CHAIN_ID` are the same job-level constants the other verbs use),
binding it to the fleet's existing devnet mock USDC
(`TOKEN = 0x49beE1Bca5d15Fb0963117923403F9498119a9Ce`) and a challenge period
of `challenge_period` seconds (default `86400` = 1 day, the contract's own
floor).

Dry run first (reads chain state, simulates the deploy, sends nothing):

```shell
gh workflow run funded-ops.yml \
  -f operation=deploy-rolling-swap-channel \
  -f account_index=0 \
  -f challenge_period=86400 \
  -f apply=false
```

Read the run's summary (`gh run view --log`), confirm the deployer address
holds enough Base-Sepolia ETH for gas (the dry run prints its balance), then
re-dispatch with `apply=true` to broadcast:

```shell
gh workflow run funded-ops.yml \
  -f operation=deploy-rolling-swap-channel \
  -f account_index=0 \
  -f challenge_period=86400 \
  -f apply=true
```

### What the apply run verifies before calling the job green

Per this repo's own rule that a transaction receipt is never the authority
(connector#907), the workflow does not stop at a mined deploy transaction. It
reads the deployed address out of the script's own
`broadcast/DeployRollingSwapChannel.s.sol/84532/run-latest.json` (never out
of console text), then calls the **live deployment**:

- `token()` / `challengePeriod()` equal what was requested.
- `domainSeparator()` equals an independently-computed EIP-712 domain hash for
  the domain `name="RollingSwapChannel"`, `version="2"`, `chainId=84532`,
  `verifyingContract=<the deployed address>` -- connector#973 AC3.
- `claimDigest(...)` for the exact sample `docs/rolling-swap-v2-digest-spec.md`'s
  golden vector uses (`channelId=0x5b`, `cumulativeAmount=24_000_000`,
  `nonce=24`, `recipient=0x…DEADBEEF`) equals the same `ClaimBalanceProof`
  struct hashed off-chain by ethers' `TypedDataEncoder` for THIS deployment's
  real `(chainId, address)` pair -- connector#973 AC5. This is deliberately
  not a comparison against the spec's own pinned digest literal: that fixture
  is deployed at a synthetic address on chainId 8453 in
  `packages/contracts/test/RollingSwapChannel.t.sol`, unrelated to any real
  devnet address. What's proven here is that the same algorithm agrees
  on-chain and off-chain for the pair that actually exists.
- `updateBalance(...)` is called (a static call, no state change) against a
  channel id that was never opened and must revert with the contract's own
  `InvalidChannelState()` -- proving the entrypoint is live on-chain, not
  merely present in the compiled ABI -- connector#973 AC4.

A run that reaches "✅ RollingSwapChannel deployed and verified against the
live contract" in its job summary has satisfied AC3, AC4 and AC5 for the
address it prints.

## Where the address is recorded

The broadcast above is written down in three places, and a re-deploy (a new
`apply=true` dispatch supersedes the old contract; nothing migrates) must
update all three together:

1. **`packages/contracts/deployments.json`** -- the `RollingSwapChannel`
   entry under `networks.base-sepolia.contracts`, in the same shape as the
   entries beside it (`address`, `deployer`, `deployTxHash`, `blockNumber`,
   `deployedAt`, a free-text `note` naming the token and challenge period).
2. **`docs/deployments/base-sepolia.md`** -- the dated
   "RollingSwapChannel deployment" section, mirroring the "ERC-2771 cutover
   deployment" section's shape (network, RPC, deployer, block, script, a
   table of the deployed address and its deploy tx, and the on-chain
   verification performed -- the workflow's job summary has all of it).
3. **This document's "Status" section.**

AC2 asks for the address to be keyed by the same chain-key form used
elsewhere -- `evm:84532`, matching `docs/rolling-swap-v2-digest-spec.md`'s
"derive both from one source" note and the live announce's own
`tokenNetworks["evm:84532"]` key. That keying lives in the three records
above. No infra `connector-rust.toml` in this repo carries a
`RollingSwapChannel` field to populate: **no swap node is live**
(toon-meta#394's own honest-notes section: zero swap pairs across 5,000 live
announces), so there is nothing here to repoint. Advertising the address in a
live announce is `swap#102` ("swap node advertises its verifying contract"),
which lives in the `swap` repo and is blocked on `swap#101`, not on this
deploy.

## What this leaves open

connector#973 is satisfied in full: AC1 (deployed via funded-ops run
31885961037), AC2 (recorded above, keyed `evm:84532`), AC3-AC5 (proven by the
apply run's own post-deploy calls against the live contract), AC6 (this
document plus the workflow verb are the reproducible record).

Not in this repo's scope, and still open: a swap node advertising this
address as its `verifyingContract` (`swap#102`, blocked on the signer
migration `swap#101`). Until that lands the contract is deployed and
verifiable but nothing on the network points at it.
