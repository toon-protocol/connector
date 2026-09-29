# Getting paid over x402 `batch-settlement`

Every payment this connector takes is a **voucher** on an x402 `batch-settlement` channel
([ADR 0074](../adr/0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md),
[ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)). On EVM the
channel lives in x402's audited `x402BatchSettlement` contract; on Solana in solana-foundation's
`payment-channels` program. Neither is TOON's, and both are constants of the binary. The old
`toon-channel` claim is refused by name at the client edge and on both peer carriages.

This page is for an operator with a priced route. It covers what a paying client does end to end,
what your node does in return, every config key involved, who pays gas and why, and which tokens
work. The README's [step 3](../../README.md#3-get-paid) is the short version.

The wire rules are [`client-edge-spec.md`](../protocol/client-edge-spec.md) §1.3, §1.4, §1.9, §1.10
and §1.11. The keys are [`configuration-spec.md`](../protocol/configuration-spec.md). The exact
bytes are `claim_voucher` and `voucher_claim_state_challenge` in
[`vectors/wire-vectors.json`](../../vectors/wire-vectors.json). Where this page and those disagree,
they win. The payer side is [`toon-client`](https://github.com/toon-protocol/toon-client), and its
[`docs/channels.md`](https://github.com/toon-protocol/toon-client/blob/main/docs/channels.md)
describes the same lifecycle from the payer's end.

## The shape

| Step      | Who acts     | What touches the chain                                                                                         |
| --------- | ------------ | -------------------------------------------------------------------------------------------------------------- |
| Terms     | client reads | nothing: `GET /ilp`, or the `402` greeting on an unpaid request                                                |
| Open      | client       | **EVM:** one `deposit`, relayed by an x402 facilitator. **Solana:** one `open`, co-signed by your node         |
| Pay       | client       | nothing: one signed voucher per paid packet, carried inside ILP                                                |
| Land      | your node    | batched `claim` + `settle` (EVM), `settle` (Solana), every ten minutes                                         |
| Exit      | client       | **EVM:** `initiateWithdraw`, then `finalizeWithdraw`. **Solana:** `request_close`, then the payer's withdrawal |
| Exit race | your node    | lands its latest voucher as soon as it sees the exit start                                                     |

Value moves one way on a channel: client to you. Claims are frequent and free, and settlement is
rare (ADR 0004, 0005). The chain sees the deposit, the sweeps, and the exit.

## What a paying client does

### 1. Read your terms

A client learns everything it needs from one of two free answers:

- **`GET /ilp`**, your self-description. `batchSettlements[]` has one entry per configured chain,
  and `voucherSigners[]` the key your own vouchers are signed with (what a peer binds your channel
  by). See [`self-description-spec.md`](../protocol/self-description-spec.md).
- **The `402` greeting** on a request to a priced route that carries no voucher. It is an x402 v2
  `PaymentRequired` document: HTTP `402` on `POST /ilp`, or a `payment-required` protocolData
  entry on BTP. `accepts[]` holds one x402-valid `batch-settlement` entry per chain you settle on,
  and nothing else. TOON's own terms ride in `extensions.toon.info`: the quoted `amount`, `price`,
  `pricePerKib` on a size-priced route, `ilpAddress`, `endpoint` and `sessionLeaseTtlMs`, plus
  your `ilpAddresses`/`btpEndpoint` when `[node]` sets them. A node with no `[settlement]` table
  still greets, with an empty `accepts[]`, and nobody can pay it.

Both are projections of one value, so they never disagree. An EVM entry, as the greeting carries it:

```json
{
  "scheme": "batch-settlement",
  "network": "eip155:84532",
  "amount": "1000",
  "asset": "0x0C996d7c934c79a6255254875607Fe69df25C0E1",
  "payTo": "<your EVM settlement address>",
  "maxTimeoutSeconds": 60,
  "extra": {
    "receiverAuthorizer": "<your EVM settlement address>",
    "withdrawDelay": 86400,
    "name": "USDC",
    "version": "2",
    "assetTransferMethod": "eip3009",
    "facilitator": "https://onboard.example/x402"
  }
}
```

A Solana entry's `extra` is `feePayer` (your Solana settlement key), `withdrawDelay` (your minimum
`grace_period`, under x402's name), `tokenProgram` (always SPL Token), `minDeposit` and
`sponsorEndpoint` (`/ilp/batch-settlement/solana/open`). `payTo` is your Solana settlement key, the
owner of your receiving token account. The full field list is `client-edge-spec.md` §1.4.

### 2. Open a channel toward you

**EVM.** The client builds a `ChannelConfig`. It picks its own `payer`, `payerAuthorizer` and `salt`.
`receiver` and `receiverAuthorizer` are your `payTo`, `token` is your `asset`, and `withdrawDelay`
is at least your minimum. It signs a deposit authorization under the asset's EIP-712 domain
(`extra.name`/`extra.version`), by the method `extra.assetTransferMethod` names. Then it hands the
authorization to an x402 facilitator, which submits `deposit` to `x402BatchSettlement` and pays the
gas. A client holding its own ETH may submit it directly instead. Your node is not involved. It
learns of the channel from the first voucher.

Two things a stock x402 client can get wrong here, and your node refuses:

- **`payerAuthorizer` must be nonzero.** x402 allows a zero one. This connector does not, because
  with a zero one the contract checks each voucher against `payer` through ERC-1271 once `payer`
  has code, which an EOA can gain later through an EIP-7702 delegation. `extra` has no field to say
  so. A channel with a zero one is refused on its first voucher as a channel your node does not
  admit.
- **The deposit's own voucher must be for at least one base unit.** The TypeScript and Python
  reference facilitators refuse a zero voucher on a fresh channel (ADR 0074, prerequisite 1). Your
  watermark still starts at zero, so the client can use its first packet's charge for that voucher
  and present it again with the packet.

**Solana.** The client builds a `payment-channels` `open` in which your settlement key is fee payer,
`rent_payer` and `payee`, the only distribution entry is your receiving account at 10000 bps,
`grace_period` is at least your minimum, and the deposit is at least `minDeposit`. It signs as
payer, leaves the fee payer's signature empty, and posts `{"transaction": "<base64>"}` to your
`sponsorEndpoint`. Your node checks every field, co-signs, submits, waits for confirmation,
re-reads the channel and answers `200` with `channelId`, `transaction`, `payer` and `deposit`. The
payer's canonical token account must already exist and hold the deposit. A Token-2022 mint is
refused. The refusal names are listed in `client-edge-spec.md` §1.11.

Nothing stops one client from holding several channels to you. Each has its own collateral and its
own watermark.

### 3. Pay per packet with a voucher

Each paid packet carries one voucher: a cumulative amount on one channel, signed by the channel's
voucher signer. Over HTTP it rides in `ILP-Payment-Channel-Claim` (base64 JSON), or in
`ILP-Payment-Channel-Claim-Wrapped` if it is gift-wrapped to your `[signer]` key. Over BTP it rides
in a `payment-channel-claim` protocolData entry. The first voucher on an EVM channel your node has
not seen carries the full `channelConfig`, and your node recomputes the channel id from it.

Your node checks, in this order: structure; **freshness**, meaning the amount must be strictly
greater than the channel's watermark; **value**, meaning it must advance the watermark by at least
the route's charge; the **signature**, against the voucher signer the chain records
(`payerAuthorizer` on EVM, `authorized_signer` on Solana); and **collateral**, meaning the amount
must not exceed what the channel can pay right now. A voucher has no nonce, so a replay advances
nothing and buys nothing. A packet to a free route carries no voucher. A Solana voucher with a
nonzero `expiresAt` is refused. Each refusal names its own reason in the REJECT's message.

Before any of that, a voucher on a channel your node has never seen costs it one chain read. The
node bounds reads that resolve nothing with the `unresolvable_lookup_budget_*` knobs (defaults: 20
per declared signer and 600 in total per 60-second window, with lookups held up to 2 seconds for a
slot rather than dropped). On a metered RPC plan, set them from your endpoint's real allowance
(`client-edge-spec.md` §1.3).

### 4. Recover its watermark: `POST /ilp/claim-state`

A client that lost its channel store has no nonce to fall back on, so it asks your node where each
of its channels stands. Each entry names `"scheme": "batch-settlement"`, the channel (`channelId`
on EVM, `channelAccount` on Solana), an `expires`, and a signature by the channel's voucher signer
over a **claim-state challenge**. That is a different message from a voucher, so neither can be
replayed as the other: EIP-712 `ClaimStateChallenge(bytes32 channelId,uint256 expires)` under
`x402BatchSettlement`'s domain on EVM, and Ed25519 over
`"toon-voucher-claim-state-challenge-v1" ‖ channelAccount ‖ expires` on Solana. The answer gives
`cumulativeClaimed` (the watermark the next voucher must beat), `maxCumulative`, `available` and a
best-effort `lastClaimTime`. It is free, unauthenticated and read-only. An entry with no `scheme`
is answered `"toon-channel-refused"`. See `client-edge-spec.md` §1.10.

### 5. Declare its channel at BTP auth

A BTP client may carry the same challenge object as `channelChallenge` on its `auth` entry, with
an `expires` no more than 300 seconds ahead. That tells your node, before the session pays
anything, which voucher signer the session holds. It matters for a client that earns: a payout
goes to the voucher signer a session has proved. The retired flat `auth_channel_proof` fields are
refused with an `ERROR` frame, and that session is not bound (`client-edge-spec.md` §1.9 step 1).

### 6. Leave

Leaving is the payer's own transaction, and the one step that costs it native gas.

- **EVM.** `initiateWithdraw`, then `finalizeWithdraw` once `withdrawDelay` has passed. Only what
  your node has already `claim`ed on chain survives a withdrawal, so your node claims its latest
  voucher the moment it sees `WithdrawInitiated`. Collateral falls when a withdrawal starts, and
  your node re-reads it for every voucher, so nothing above what is left is accepted afterwards.
- **Solana.** `request_close`. From then on only the `payee`, which is your node, can land a
  voucher, with `settle_and_seal`, and only inside the grace period. Your node does that on its
  next tick, or seals at once if there is nothing to land. After the grace period the payer takes
  back what was not claimed. toon-client seals first if your node has not, then calls
  `withdraw_payer`.

Neither chain has a cooperative refund. On EVM it would need your `receiverAuthorizer`'s
signature, and the connector never gives one.

## What your node does

**Admission.** A channel is admitted on its first voucher (EVM) or at the sponsored open (Solana),
and only on your published terms:

| Chain  | The channel must have                                                                                                                                                                                        |
| ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| EVM    | `receiver` **and** `receiverAuthorizer` = your EVM settlement address; `token` = your `token_address`; `withdrawDelay` ≥ `min_withdraw_delay_secs`; a nonzero `payerAuthorizer`                              |
| Solana | status Open; `payee` **and** `rent_payer` = your Solana settlement key; `mint` = your `token_address`; one distribution entry, your receiving account at 10000 bps; `grace_period` ≥ `min_grace_period_secs` |

A channel's first accepted voucher journals the channel itself (`BatchChannelAdmitted`) in
`state_dir`. On EVM that entry holds the `ChannelConfig`, which the chain does not store and which
`claim` cannot be sent without. That is one more reason `state_dir` must be a persisted volume.

**Landing.** Automatic, with no operator write:

| Chain  | Cadence                                                                                                                                                                                                                                                                                                                                  |
| ------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| EVM    | Every **5 s**, read new `WithdrawInitiated` logs and `claim` that channel's latest voucher at once. Every **10 min**, `claim` every held voucher above what the chain records, many channels in one transaction, then `settle` to your address. The sweep also runs at boot, which catches a withdrawal started while the node was down. |
| Solana | Every **10 s**, rediscover every channel you sponsored with `getProgramAccounts` (filtered on `rent_payer`), so the journal is not the index. Open: `settle` the latest voucher every **10 min**. Closing: `settle_and_seal` inside the grace period. Sealed: `distribute`. Distributed: `reclaim` the channel's rent.                   |

`POST /channels/:id/land` lands an inbound channel's latest voucher now. It is the manual lever for
planned maintenance. `GET /channels` and `GET /claims` show each channel with its direction,
collateral and watermark. See the README's [operator surface](../../README.md#the-operator-surface).

The one-day default minimum for `withdrawDelay` and `grace_period` is the window a delayed or
censored landing transaction still has. Lowering it toward the 900-second floor narrows that
window.

## Configuring it

Each chain is one table. Its x402 terms sit directly in it. The retired
`[settlement.<chain>.batch_settlement]` sub-table is refused by name, and so are `contract_address`
(EVM) and `program_id` (Solana), because the contract and program are constants of the binary.

### `[settlement.evm]`

```toml
[settlement.evm]
rpc_url               = "https://base-sepolia-rpc.publicnode.com"
token_address         = "0x0C996d7c934c79a6255254875607Fe69df25C0E1"
decimals              = 6
asset_eip712_name     = "USDC"
asset_eip712_version  = "2"
# min_withdraw_delay_secs = 86400
# asset_transfer_method   = "eip3009"
# facilitator_url         = "https://onboard.example/x402"
# rpc_via_socks_proxy     = false

[settlement.evm.key]
key_file = "/app/data/settlement.key"
```

| Key                       | Default         | Refused at load when                                                           | Published as                                                           |
| ------------------------- | --------------- | ------------------------------------------------------------------------------ | ---------------------------------------------------------------------- |
| `rpc_url`                 | required        | empty, not a URL, or not `http`/`https`                                        | (the chain id, read from it, is `network`)                             |
| `token_address`           | required        | not a 20-byte hex address                                                      | `asset`                                                                |
| `decimals`                | required        | `0`. Boot also refuses a value the token itself disagrees with.                | nothing: it is how you read `price`                                    |
| `asset_eip712_name`       | **required**    | missing or empty                                                               | `extra.name`                                                           |
| `asset_eip712_version`    | **required**    | missing or empty                                                               | `extra.version`                                                        |
| `min_withdraw_delay_secs` | `86400` (a day) | below `900`, or above `2592000` (the contract's 30-day maximum)                | `extra.withdrawDelay`                                                  |
| `asset_transfer_method`   | `"eip3009"`     | anything but `"eip3009"` or `"permit2"`                                        | `extra.assetTransferMethod`, always                                    |
| `facilitator_url`         | none            | not a URL, or not `http`/`https`                                               | `extra.facilitator`, verbatim; absent when unset                       |
| `rpc_via_socks_proxy`     | `false`         | `true` with no root `socks_proxy` (ADR 0073)                                   | nothing                                                                |
| `[settlement.evm.key]`    | required        | not exactly one of `key_file`/`kms_key_id`, or a `key_file` that is not a file | `payTo`, `receiverAuthorizer`, `voucherSigners[].signer` (the address) |

`kms_key_id` parses, but the binary refuses to start with it. Use `key_file`.

**Read the EIP-712 name and version off the token itself**
(`cast call <token> 'name()(string)'` and `'version()(string)'`). Base mainnet's native USDC is
`"USD Coin"` / `"2"`, not the devnet's `"USDC"` / `"2"`. Your node never signs under this domain, so
it boots with a wrong value, and the only symptom is every client's deposit failing.
`asset_transfer_method` is not checked against the token either: `"eip3009"` on a token without
ERC-3009 boots, and every stock client's deposit then fails on chain.

### `[settlement.solana]`

```toml
[settlement.solana]
rpc_url               = "https://api.devnet.solana.com"
token_address         = "34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU"
decimals              = 6
min_sponsored_deposit = 1000000
# min_grace_period_secs = 86400
# rpc_via_socks_proxy   = false

[settlement.solana.key]
key_file = "/app/data/settlement-solana.key"
```

| Key                       | Default         | Refused at load when                                                                      | Published as                                   |
| ------------------------- | --------------- | ----------------------------------------------------------------------------------------- | ---------------------------------------------- |
| `rpc_url`                 | required        | empty, not a URL, or not `http`/`https`                                                   | (the genesis hash, read from it, is `network`) |
| `token_address`           | required        | empty. Boot also refuses a mint the SPL Token program does not own (Token-2022 included). | `asset`                                        |
| `decimals`                | required        | `0`. Boot also refuses a value the mint disagrees with.                                   | nothing                                        |
| `min_sponsored_deposit`   | **required**    | missing, or `0`                                                                           | `extra.minDeposit`                             |
| `min_grace_period_secs`   | `86400` (a day) | below `900`                                                                               | `extra.withdrawDelay`                          |
| `rpc_via_socks_proxy`     | `false`         | `true` with no root `socks_proxy`                                                         | nothing                                        |
| `[settlement.solana.key]` | required        | as on EVM                                                                                 | `payTo`, `feePayer`, `voucherSigners[].signer` |

`asset_transfer_method` and `facilitator_url` are EVM-only, and a Solana table that writes either
fails to load as an unknown field. On Solana your node is its own facilitator. Your receiving
token account for the mint must exist before anyone opens a channel, because the sponsor refuses to
open into an account that would forfeit your payout (`receiving_account_unusable`).

**At boot, before serving,** the node confirms the chain id and that `x402BatchSettlement` has
code at its fixed address (EVM), or that `payment-channels` is deployed and the mint is an SPL
Token mint (Solana). It refuses a chain it is missing from, by name. A Solana key with no lamports
is refused too. Fund both keys before the first boot.

## Who pays gas, and why

x402's `x402BatchSettlement` has **no fee field**. Nothing on chain pays a facilitator, so gas is
paid by whoever wants the transaction to happen. The owner decided on 2026-09-29 that in TOON that
is **the seller**: your node names the facilitator it relays deposits through
(`facilitator_url`), and deposit gas is a cost of the sale. Your node publishes that URL and never
calls it. A stock x402 seller calls its facilitator itself. Here the deposit precedes the channel
and leaves the packet path, so the payer calls it, and you have to name it.

| Step                                           | Who pays                                                                                        |
| ---------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| EVM deposit or top-up                          | the facilitator you name, off chain at your expense; or the payer, from its own ETH             |
| EVM Permit2 approval (once)                    | the facilitator, if it sponsors approvals for this token; otherwise the payer's own ETH         |
| Solana `open`                                  | **you**: the transaction fee and 4,711,920 lamports (about 0.0047 SOL) of rent per channel      |
| A voucher                                      | nobody: a signature                                                                             |
| EVM `claim` and `settle`                       | you, from the EVM settlement key's ETH                                                          |
| Solana `settle`, seal, `distribute`, `reclaim` | you, from the Solana settlement key's SOL. The rent comes back at `reclaim`.                    |
| The payer's exit                               | the payer                                                                                       |
| Your own outbound channels                     | you: when your node pays a peer, it deposits directly and pays its own gas, with no facilitator |

On Solana you pay the rent up front and get it back. What bounds a stranger's ability to make you
spend is `min_sponsored_deposit`: the program already requires a nonzero deposit, so every channel
an attacker makes you sponsor locks at least that much of its own capital for at least the grace
period. The sponsor endpoint adds its own bounds on top: at most 8 sponsorships in flight
(`sponsor_busy`), one per payer (`payer_open_in_flight`), a priority-fee cap per `open`, and a
failure budget of 8 sent-and-failed opens per hour, after which it refuses everything
(`sponsor_paused`) until the oldest ages out.

### Choosing a facilitator

**The facilitator must never be your `receiverAuthorizer`.** That key can refund to the payer
anything you have earned and not yet claimed. Your node always names its own settlement address,
so a facilitator that advertises its own authorizer is simply not used as one.

- **Run a stock x402 facilitator yourself.** toon-protocol/infra's
  [Onboarder](https://github.com/toon-protocol/infra/tree/main/onboarder) is one, and the devnet
  runs it at `https://onboard.devnet.toonprotocol.dev`. It is the published `@x402/evm` facilitator
  with no `receiverAuthorizer`. It registers `eip2612GasSponsoring` (a permit token's approval rides
  inside the deposit) and, for the tokens its `ONBOARDER_SPONSORED_TOKENS` allowlist names,
  `erc20ApprovalGasSponsoring` (it funds and broadcasts a plain ERC-20's one-time approval). Its
  gas key needs ETH, and that ETH is the cost of sale.
- **Use a hosted one.** Coinbase CDP is the credible hosted option for mainnet. It needs an API key,
  bills per on-chain transaction past a free tier, and screens the payer and recipient for KYT and
  OFAC. x402.org's facilitator describes itself as for development and testnet work, not mainnet.
  In a live Base Sepolia run it relayed a deposit to an arbitrary `payTo`, but its advertised
  `erc20ApprovalGasSponsoring` broadcast the approval without funding it. The specifics, with
  sources and what is still unverified, are in
  [`docs/research/x402-devnet-facilitators.md`](../research/x402-devnet-facilitators.md). Check
  current terms with the provider rather than relying on that note's figures.
- **Name none.** Then only a payer that brings its own facilitator, or holds ETH, can deposit.
  toon-client falls back, in order, to the payer's own `facilitatorUrl`, then yours, then the
  devnet Onboarder on Base Sepolia only. With none of those and no ETH, it stops with a
  `ConfigError` rather than pick a third party to relay real money.

Solana needs none of this. The payer posts its `open` to your sponsor endpoint, and your node is the
fee payer.

## Tokens

| Token                                                                 | `asset_transfer_method` | Is the payer's first deposit gasless?                                                                                                               |
| --------------------------------------------------------------------- | ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| ERC-3009 (`receiveWithAuthorization`): Circle's USDC, the devnet USDC | `"eip3009"` (default)   | Yes, through any facilitator. One signature, no approval.                                                                                           |
| EIP-2612 `permit`, no ERC-3009                                        | `"permit2"`             | Yes, if the facilitator offers `eip2612GasSponsoring`. Otherwise the payer sends a one-time Permit2 approval from its own ETH.                      |
| Any other ERC-20                                                      | `"permit2"`             | Yes, if the facilitator offers `erc20ApprovalGasSponsoring` for this token. Otherwise the payer sends a one-time Permit2 approval from its own ETH. |
| Solana: an SPL Token mint                                             | n/a                     | Yes. Your node sponsors the `open`. Token-2022 mints are refused.                                                                                   |

A payer may always pay its own deposit gas instead. toon-client's `depositGas` (`--deposit-gas`)
picks `auto` (the facilitator, else the wallet's own ETH), `facilitator` or `self`. Under `auto`, a
deposit the facilitator refused is sent directly with the same single-use authorization, so it can
never land twice.

Every node that might accept a given voucher must name the same token, so copy the token address
rather than choosing one. On mainnet, point `token_address` at Base mainnet's or Solana
mainnet-beta's USDC, never the devnet one.

## Checking it

```bash
curl -s https://your-node.example/ilp | jq '.batchSettlements, .voucherSigners'
```

Check that there is one `batchSettlements` entry per chain you configured, with your settlement
address as `payTo` (EVM) or your key as `feePayer` (Solana), and the `name`, `version`,
`assetTransferMethod` and `facilitator` you meant to publish. After a client has paid, read the
journal with your bearer token:

```bash
curl -s -H "Authorization: Bearer $TOKEN" https://your-node.example/claims | jq
```

Each paying channel shows `"scheme": "batch-settlement"` and a `cumulative_amount` that rises with
every packet. Watch that watermark, not the count of claims. `nonce` is always `0`.

| Symptom                                                            | Likely cause                                                                                                               |
| ------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------- |
| Every client's EVM deposit fails at the facilitator or on chain    | Wrong `asset_eip712_name`/`asset_eip712_version`, or `"eip3009"` on a token without ERC-3009                               |
| Permit2 payers are told they need ETH for an approval              | Your facilitator does not sponsor approvals for this token: allowlist it, or accept that such payers need ETH              |
| First voucher refused: "does not admit"                            | The client's `ChannelConfig` names a zero `payerAuthorizer`, a short `withdrawDelay`, another token, or another receiver   |
| Refused as "does not strictly exceed this channel's watermark"     | The client lost its store: it should ask `POST /ilp/claim-state`                                                           |
| Refused as "more than the … this channel can pay"                  | The voucher is above what the channel holds, or a withdrawal lowered it: the client tops up and resubmits the same voucher |
| Solana open: `receiving_account_unusable`                          | Your own token account for the mint does not exist (or is frozen): create it                                               |
| Solana open: `deposit_below_minimum`, `grace_period_below_minimum` | The client ignored `extra.minDeposit` or `extra.withdrawDelay`                                                             |
| Solana open: `sponsor_paused`                                      | Eight sponsored opens failed on chain in the last hour: look for a client sabotaging its own opens                         |
| Refused as "could not look up the channel's counterparty"          | Your settlement RPC is not answering, not the client's fault                                                               |

## Trust

Opting in to a chain means trusting x402's code. The EVM contract is ownerless and immutable, so
nothing can change it, and nothing can rescue an escrow that USDC's blacklist has frozen.
`payment-channels` is upgradeable: on mainnet-beta by a 3-of-5 Squads multisig with no time lock,
on devnet by a single keypair. What you risk as a receiver is whatever you have accepted in
vouchers and not yet landed. ADR 0075's trust statement has the details.
