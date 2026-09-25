# Migrating the apex↔store peer channel to the new TokenNetwork

> **Superseded by issue #872** (toon-meta#310 / toon-meta#313's live cutover): the apex box is
> destroyed and the apex↔store peering this runbook migrates — the `[[peers]]`/`[[peer_channels]]`
> rows it names, on both sides — no longer exists in any committed config. There is no channel left
> to migrate. Kept as the historical record of the ERC-2771 TokenNetwork split-brain and how it was
> resolved while the peering was still live.

Operator runbook for [issue #822](https://github.com/toon-protocol/connector/issues/822): the
apex↔store `[[peer_channels]]` row still settles on the OLD `TokenNetwork`
(`0x1E95493fEF46707E034b4a1945f25a8C76A1823D`) after the ERC-2771 cutover (#695/#811) repointed
every other settlement path on the fleet at the new one
(`0xa79C3b1dbcEA00a6d84735a134395D8eF6D6a478`, via registry `0x8263BdD4eB4862395Cb4ef5dA5d637F4b047Eea1`).
That split was **deliberate** at cutover time (AC4 — `docs/evm-deployment.md`'s "What this does NOT
touch" section): the channel predates the cutover, `token_network` is half its EIP-712 signing
domain (ADR 0024), and rewriting the literal under a live channel would invalidate every claim
already exchanged on it. Leaving it there afterwards is not deliberate — it is a standing
two-settlement-contract split-brain, and this is the follow-up that ends it.

Relies on [ADR 0024](../adr/0024-peer-wire-claims-sign-the-eip-712-balance-proof.md) (the
domain a channel's claims are signed under) and [ADR 0028](../adr/0028-a-forwarded-route-is-priced-at-the-client-edge.md)
(the apex↔store peering carries priced, claim-settled traffic, not a free relay). Modelled on
[`btp-peer-transport-bringup.md`](btp-peer-transport-bringup.md)'s "Order"/"Gates" shape. See also
`docs/evm-deployment.md` for the cutover this migration completes, and
`crates/connector-bin/tests/devnet_configs_load.rs` for the assertions that hold the committed
copies of both files to the new TokenNetwork and to the sentinel channel fields.

## What is actually deployed today

Both `infra/linode-node/connector-rust.toml` and `infra/linode-store/connector-rust.toml` on the
two boxes are **hand-tuned, bind-mounted files that lead the repo copies** — merging this issue's
PR rolls nothing on either box. Verify the live files before touching anything; a stale bind mount
looks identical to a repointed one until you diff it. Backups from the #695/#811 cutover exist as
`connector-rust.toml.bak-pre-erc2771-cutover-20260806T124811Z` on both boxes — do the equivalent
before this migration (`connector-rust.toml.bak-pre-peer-channel-migration-<UTC-timestamp>`).

The repo diff for this issue updates `[[peer_channels]] token_network` on both files to the new
TokenNetwork and replaces `channel_id`/`counterparty_key` with a clearly-marked sentinel
(`0xdead…dead`, never a real value — `devnet_configs_load.rs` asserts both the new
`token_network` and the sentinel fields are present, so an edit that forgets either fails CI). The
steps below are what turns that sentinel into a real, funded, live channel. **None of them can be
done from this repo** — they need SSH/deploy access to both boxes, a Base Sepolia RPC, and a
funded settlement key for each box's identity (apex `0xF29fD62C4848B9573C9b90adbF61b664F386d9CF`,
store `0x6B6c2DACf7Ac1F1273F72beF2E6084F9Ee6D3bff` — the same two addresses the retired channel
already names as participants, since this migrates the _channel_, not the identities).

## Constraints that are easy to get wrong

- **Both boxes change together, and only after the new channel is proven.** A claim one box
  accepts against a channel the other cannot resolve is unrecoverable — this is why the Order
  below opens and proves the new channel _before_ either box's config is touched, and why neither
  box's edit ships without the other's.
- **`docker compose up -d` does NOT reload a bind-mounted config.** It reports `Running` and
  changes nothing. The restart step is `docker compose restart connector-rust`, on both boxes,
  every time.
- **The new channel needs its own funding.** It is not a transfer of the old channel's collateral
  — that collateral is stuck in the old channel until closed and settled, which happens last, not
  first (see "Why closing the old channel is the last step" below).
- **Do not strand the old channel's collateral.** Once the new channel is live and proven, the old
  channel must be closed and settled so each side's deposit (minus whatever the counterparty
  already claimed) comes back. Skipping this leaves real USDC locked in a contract nothing points
  at any more.

## Why closing the old channel is the last step, not the first

It is tempting to read "close the old channel, settle it, fund the new one, open it" as a literal
sequence. Doing it in that order is the wrong shape: `closeChannel` starts the old
`TokenNetwork`'s challenge period (`settlementTimeout`, minimum 1 hour, whatever value the channel
was actually opened with — read it back on-chain per Order step 4 rather than assuming the
minimum), and once closed the channel can never reopen. If the new channel then failed to fund,
failed to open, or failed its own verification, there would be **no live apex↔store channel of any
kind** for the length of that window, and no config edit could roll that back — the old contract
would still resolve (AC5 of the original cutover), but the specific channel that config named would
not.

So the new channel is opened and funded with **fresh collateral**, fully independent of the old
channel's state, proven end to end while the old channel is still live and untouched, and only
_then_ is the old channel closed and settled — purely to reclaim its collateral, not because
anything downstream depends on it being gone. At every point before the final config edit and
restart (Order step 6), the rollback in this document is genuinely "do nothing further"; after it,
rollback means reverting the edit, not the on-chain state.

## Order

1. **Fund the new channel's collateral.** Mint or acquire devnet USDC for both participant
   addresses on the live `TokenNetwork`'s token (`0x0C996d7c934c79a6255254875607Fe69df25C0E1`, Circle's FiatToken v2.2 since the
   2026-09-25 token cutover, #1337, `docs/evm-deployment.md`). The faucet
   (`https://faucet.devnet.toonprotocol.dev/api/base-sepolia/request`) is rate-limited per address
   (24h cooldown) and may not cover a realistic channel deposit. Minting is **minter-gated** now,
   unlike the mock it replaced: for a larger amount, mint from the faucet key on the devnet box
   (the token's one minter), or have the token's master minter
   (`/root/keys/devnet-usdc-owner.key`) `configureMinter` a key for the job.
2. **Open the new channel.** Either participant calls
   `openChannel(address participant2, uint256 settlementTimeout)` on the live `TokenNetwork`
   (`0x1B4606218ceE5Bf02B546e416905F4D3FC8a0249` since the 2026-09-25 token cutover, #1337; `docs/evm-deployment.md`) naming the _other_ participant's settlement
   address, with a `settlementTimeout` at least as long as the retired channel's — read that value
   off the OLD `TokenNetwork`'s `channels(bytes32)` rather than assuming the 1-hour contract
   minimum. Record the returned `channelId`: it is emitted in `ChannelOpened` and is the value both
   boxes' `channel_id` placeholder gets replaced with.
3. **Deposit on the new channel.** Each participant calls
   `setTotalDeposit(channelId, participant, totalDeposit)` for its own address, approving the
   `TokenNetwork` for the token first. Both sides need a deposit, or one direction of the peering
   can never be claimed against.
4. **Verify the new channel off to the side, before either box's config changes.** Confirm on-chain
   (or on a block explorer) that the channel is `Opened` with both deposits recorded — the state
   and the timeout come from `cast call <TokenNetwork> "channels(bytes32)" <channelId>`; each
   side's deposit is a separate read, `participants(bytes32,address)` for that participant, since
   `channels` carries no deposit of its own. Nothing in either box's live config points at it yet,
   so this step is zero-risk to the live peering.
5. **Quiesce, then edit both configs together.** Let in-flight peer-forwarded traffic drain —
   `flush_interval_ms` is retired (ADR 0033, issue #882; the committed configs no longer set it), so
   there is no periodic flush to wait out, only whatever traffic is genuinely in flight — then, on
   **both** boxes, back up the live file and replace the `[[peer_channels]]` row's three
   values: `token_network` is already the new address if the repo diff was applied; set
   `channel_id` to step 2's value and `counterparty_key` to the _other_ box's settlement address
   (apex names store's `0x6B6c2DACf7Ac1F1273F72beF2E6084F9Ee6D3bff`; store names apex's
   `0xF29fD62C4848B9573C9b90adbF61b664F386d9CF` — unchanged from the retired channel, since the two
   participants are the same, only the channel is new). Do not restart either box until both files
   are edited. **Rotating `peer-claims.log` is no longer a step here.** Issue #832 found that,
   before its fix, `ClaimBook::record_fulfillment` kept signing against whatever `channel_id` the
   journal replay left in the outbound ledger, ignoring the freshly edited config entirely — the
   live workaround was to move the payer's `peer-claims.log` aside so the ledger rebuilt empty. As
   of the fix landing, `record_fulfillment` itself detects that config now names a different
   channel than the ledger's and rebinds to it at a fresh nonce/amount before signing, so the old
   journal can be left in place; it is still read for its inbound-side (received-claim) history.
6. **Restart both connectors.** `docker compose restart connector-rust` on each box —
   **`docker compose up -d` is a no-op against a bind-mounted config file** and will report success
   while changing nothing.
7. **Verify claims exchange end to end** — see Gates below. If any gate fails, the rollback in this
   document (revert the two edited files, restart again) is still available, because the old
   channel has not been touched.
8. **Confirm nothing will be stranded.** Still before touching the old channel, confirm its
   `claimedAmounts(bytes32,address)` on each side reflect every claim either box's `ClaimBook`
   journal recorded against it (peer claim logs on the Rust state volume). A claim signed but never
   submitted on-chain is unclaimed value that the settle in step 10 returns to the wrong side, so
   submit it before going any further.
9. **Only once steps 7 and 8 are fully green: close the old channel.** Either participant calls
   `closeChannel(oldChannelId)` on the OLD `TokenNetwork`
   (`0x1E95493fEF46707E034b4a1945f25a8C76A1823D`). This starts that channel's own challenge period
   and is the point of no return for this runbook — after this step, the rollback below no longer
   applies.
10. **Settle the old channel.** Once its challenge period has elapsed (`closedAt` plus the
    channel's `settlementTimeout`, both readable on-chain the same way as step 4), call
    `settleChannel(oldChannelId)` — callable by anyone. Each side's remaining deposit (total
    deposit minus whatever the counterparty already claimed via `claimFromChannel`) is returned to
    that participant.

## Gates — in order

- **(a) The new channel is funded and `Opened`** before either config is touched (Order steps 1-4).
  Nothing on the peer semantics catches this for you: its four reject reasons (`signature_invalid`,
  `nonce_not_advancing`, `amount_not_advancing`, `unknown_channel`) all judge the claim, not the
  chain, so both boxes will happily sign and accept claims against a channel that does not exist or
  cannot cover them, and the failure only surfaces when `claimFromChannel` reverts — after the
  value has already moved.
- **(b) Both boxes' edited configs agree.** Each names the _other's_ settlement address as
  `counterparty_key`, both name the _same_ `channel_id` and the _same_ new `token_network`. A
  mismatch here is exactly the split-brain this migration exists to end, just moved to a new
  contract instead of resolved.
- **(c) Both connectors restarted and healthy.** `GET /ilp/identity` (or the announcer sidecar's
  poll of it) returns 200 on both boxes after the restart in Order step 6.
- **(d) Routing intact.** Both apex-served prefixes that forward across this peering
  (`g.toon.ario`, and `g.toon.relay` if it forwards at the time of migration) still answer x402
  greetings at their committed prices — a broken peer binding fails closed at the connector's own
  routing layer, not silently.
- **(e) The first claim on the new channel journals under the new channel id.** Concretely: the
  payer's `peer-claims.log` records `outbound_claim_signed <peer> <new_channel_id> 1 <amount>` for
  the first forwarded write after restart — nonce 1, not a nonce continuing the old channel's
  sequence, and naming the new `channel_id`, not the old one. Checking the journal line directly
  (rather than assuming the config edit was sufficient) is what catches the old-channel-id failure
  mode issue #832 found: applied without the `record_fulfillment` fix, the config edit alone left
  the payer signing claims that still named the old channel, which the receiver's
  `verify_signature` cannot resolve (`unknown_channel`) — silently, since neither
  `Rejected(UnknownChannel)` nor an un-acked outbound claim journaled anything before issue #832's
  observability fix landed. A forwarded write is charged at the apex client edge, carries a peer
  claim signed under the **new** EIP-712 domain (`chainId` 84532, `verifyingContract` = the new
  `TokenNetwork`), is fulfilled, and the store side's claim watermark advances. A claimless peer
  PREPARE to the same route is still rejected — this is issue #620's gate, and migrating the
  channel must not accidentally regress it.
- **(f) Claim exchange completes.** A FLUSH sent when traffic quiesces is acknowledged with a
  `claim-ack` entry on its RESPONSE, and a deliberately stale-nonce claim is rejected
  (`nonce_not_advancing`) without rejecting the PREPARE it rode on — the same claim-ack contract
  `btp-peer-transport-bringup.md` gate (d) proves for the transport, now proven again for the new
  signing domain.
- **(g) The old channel's collateral is fully reclaimed.** After Order steps 9-10, both
  participants' remaining deposits have actually landed back in their wallets (`ChannelSettled`
  event, or a balance check before/after `settleChannel`) and every claim journaled against the old
  channel was submitted before it was settled (Order step 8).

If (a) through (f) all hold, the migration is complete for live traffic; (g) can lag briefly behind
it (bounded by the old channel's challenge period) without putting anything at risk, since the old
channel is no longer named by either box's config once step 6 has run.

## Rollback

**Before Order step 9 (closing the old channel): revert the two edited `.toml` files to their
backed-up pre-migration content and restart both connectors.** The old channel was never touched,
so this is a complete rollback — the same posture `docs/evm-deployment.md`'s own rollback relies on
for the registry-level cutover ("the old deployment is never touched"). The new channel and its
deposits are simply unused, not lost; the collateral can be withdrawn or reused for a retry once
whatever failed the gates above is fixed.

**After Order step 9: there is no rollback.** The old channel's challenge period is running (or has
run), and once its collateral is returned in step 10 that channel cannot accept new deposits again.
This is exactly why the Order places the close last and gates it on every other gate passing first
— do not close the old channel until Gates (a) through (f) are all green.
