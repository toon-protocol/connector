# The operator names the facilitator and pays its gas

**Status:** Accepted (owner decision, 2026-09-29, toon-client#695; recorded here by #1423) — **built** (#1419): `[settlement.evm] facilitator_url` and `asset_transfer_method`, published as the EVM `batch-settlement` entry's `facilitator` and `assetTransferMethod` in the greeting's `accepts[].extra` and in the self-description's `batchSettlements`. Until this record the decision stood only in [0074](0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md)'s `Amended 2026-09-29 (toon-client#695)` paragraph and decision 8; this record is now where it stands. It **amends** [0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md): the considered option "Run an x402 facilitator" said clients keep using stock facilitators on EVM, and they now use the one the operator names; its other half, that a node's own deposit involves no facilitator, stands. It **extends** 0074 decision 8 (the wire fields), and is the EVM counterpart of 0074 decision 5's Solana sponsor rule, which it leaves unchanged. It leaves [0021](0021-vectors-are-normative-prose-is-not.md) undisturbed: no vector covers the greeting or the self-description, so `schema_version` stays 7.

**Scope:** protocol law. It binds every implementation, because it adds a field to the greeting's EVM `batch-settlement` entry and decides whom a payer asks to relay its deposit. That the connector never calls the facilitator is connector architecture. See the [ADR index](README.md).

**Falsifier:** `crates/**/*.rs` matching `"/(settle|verify|supported)"` — the connector addresses an x402 facilitator's `/settle`, `/verify` or `/supported` endpoint. It publishes the facilitator and never calls it (decision 1), so a match means the connector has become a client of one and this record is wrong.

**On EVM the operator names the x402 facilitator its payers deposit through, and pays that
facilitator's gas as a cost of the sale.** The operator publishes it as
`[settlement.evm] facilitator_url`, which the greeting and the self-description carry as
`facilitator`, and it runs that facilitator or picks one. The connector never calls it: the payer
does. `x402BatchSettlement` has no fee field, so nothing on chain pays for a deposit's gas, and the
operator, as the seller, is the party that wants the deposit to happen. A token without ERC-3009 deposits through Permit2, which is gasless for
the payer only when the named facilitator offers one of x402's gas-sponsoring extensions. Otherwise
the payer pays a one-time approval from its own ETH, and a payer that holds ETH may always pay its
own gas.

## Context

[0074](0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md) let a client pay over an x402
`batch-settlement` channel and promised that it could onboard with no native gas: _"on EVM a stock
x402 facilitator relays the deposit"_. [0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)
made every channel an x402 channel and put running a facilitator out of scope: _"A node pays its own
gas when it deposits, and clients keep using stock facilitators on EVM."_

Neither record said **which** facilitator, or **who pays** it. Two facts made that an open question
rather than a detail:

- **In x402 the seller calls its facilitator. In TOON the payer must.** A stock x402 client hands
  its payment to the seller with the request, and the seller forwards it to a facilitator the client
  never addresses. A TOON deposit comes before the channel exists and travels outside the packet
  path: the vouchers ride inside ILP and only the one-time deposit leaves it (0074 decision 8). So
  the payer posts the deposit to a facilitator itself, and it can only find one if someone names it.
- **Nothing on chain pays for gas.** `x402BatchSettlement` has no fee field (0074, "Prerequisites,
  as found"). A facilitator that relays a deposit pays its gas from its own key and is repaid, if at
  all, off chain. CDP bills per on-chain transaction past a free tier. x402.org charges nothing and
  does not list Base mainnet (0075, Consequences), so a mainnet payer had no default at all.

0074's prerequisite 2 found a second gap. A deposit is gasless on any facilitator for an ERC-3009
token. A token without ERC-3009 goes through Permit2, which needs a one-time `approve` from the payer.
x402.org advertised `erc20ApprovalGasSponsoring` but broadcast that approval without first funding
the payer, so the deposit failed until the payer approved Permit2 from its own ETH.

The owner decided on 2026-09-29, in toon-client#695, that the connector names its own facilitator
and the seller pays deposit gas as a cost of the sale. #1419 built the connector's half. It was
recorded in 0074's amendment of that date. This record gives it a record of its own, so that 0075
no longer contradicts it (#1423).

## Decision

### 1. The operator names the facilitator, and the connector never calls it

- **`[settlement.evm] facilitator_url`** is optional. It must be an absolute `http` or `https` URL,
  and anything else is refused by name at load (`configuration-spec.md`).
- It is published verbatim as **`facilitator`** in the EVM `batch-settlement` entry's `extra`: in
  the greeting's `accepts[]` and in the self-description's `batchSettlements`, which carry the same
  facts (0074 decision 8). When it is unset, the field is **absent**, not empty.
- `facilitator` is **TOON's own addition**, as the Solana entry's `sponsorEndpoint` is. x402 has no
  field for it, because a stock seller never tells a client where its facilitator is.
- **The connector never calls it.** It does not verify, settle, probe or health-check the URL. A
  facilitator that is down, or refuses, is between the payer and the facilitator. The connector
  learns of a deposit the way it always has: a voucher names a channel, and admission reads the
  channel from the chain (0074 decision 2).
- **The operator runs a facilitator or picks one.** Either way the choice is the operator's, and
  the facilitator must never be the channel's `receiverAuthorizer`. The connector always names its
  own settlement address there, whatever a facilitator advertises (0074 decision 5).

### 2. The operator pays, because gas is a cost of the sale

`x402BatchSettlement` has no fee field. Who pays for a deposit's gas is therefore a business
arrangement outside the protocol, and TOON settles it this way: **the party selling pays to be
paid**. The operator funds its facilitator's gas key, or pays a hosted facilitator's bill, and
recovers that cost in its prices.

This is the EVM counterpart of the Solana rule that already held. There the operator's settlement
key is the **sponsor**: it pays the fee and floats the rent of every channel opened toward it, and
it gets the rent back at `reclaim` (0074 decisions 5 and 9). On EVM the node cannot take that seat
itself, because the deposit is a transaction the payer's authorization moves through one of x402's
collectors and some key has to send it. The facilitator the operator names is that key.

What bounds a stranger's ability to make the operator spend is the facilitator's own business. The
connector adds nothing on EVM, because it never sees the request. The devnet Onboarder bounds a
sponsored approval by gas, by fee and by token, funds a payer only once, and is rate-limited at the
edge (toon-protocol/infra#40).

### 3. Tokens without ERC-3009: `assetTransferMethod` and the gas-sponsoring extensions

- **`[settlement.evm] asset_transfer_method`**, default `eip3009`, is published as x402's own
  **`assetTransferMethod`**. It is always written, even at x402's default, so a reader has nothing to
  infer (0074 decision 8, as amended 2026-09-29).
  - `eip3009`: the token implements ERC-3009 `receiveWithAuthorization`, as Circle's USDC and the
    devnet's USDC do. The payer signs one authorization and sends nothing. Any facilitator makes
    the deposit gasless.
  - `permit2`: any other ERC-20. The deposit is a Permit2 witness transfer, and Permit2 first needs
    a one-time `approve` from the payer.
- **The approval is gasless for the payer only when the named facilitator offers one of x402's two
  gas-sponsoring extensions:**
  - **`eip2612GasSponsoring`**, for a token with an EIP-2612 `permit`. The payer's permit for
    Permit2 rides inside the deposit, and the payer sends no transaction at all.
  - **`erc20ApprovalGasSponsoring`**, for a plain ERC-20 with neither ERC-3009 nor a permit. The
    facilitator funds the payer's shortfall for the approval's fee, broadcasts the approval the
    payer signed, and then sends the deposit.
- A facilitator advertises what it offers on its `/supported`. Advertising an extension is not
  proof that it works: x402.org advertised `erc20ApprovalGasSponsoring` and skipped the funding step
  (0074, prerequisite 2).
- Nothing in the connector checks that `asset_transfer_method` suits the token. A wrong value fails
  every deposit on chain (#1422, open).

### 4. The payer's own-gas fallback

A payer that holds ETH may always pay its own gas, and one that has no facilitator available must.
Without a sponsoring extension it sends the one-time Permit2 approval from its own ETH. With no
facilitator at all, or one that is down or refuses, it can send the deposit itself: the same signed
authorization serves either path, and its nonce is single-use on chain, so a deposit cannot land
twice.

toon-client exposes this as **`depositGas`** (toon-client#695): relay through the facilitator and
fall back to the payer's own ETH, relay only, or pay only from the payer's own ETH. Its option names
and its exact fallback conditions are toon-client's to decide, and #695 was still open when this
record was written. This record decides only that the fallback is the payer's and costs the operator
nothing: nothing in the greeting requires a payer to use the facilitator it names.

### 5. What stays out of scope

- **This repository runs no facilitator, calls none and bundles none.** The devnet's is
  toon-protocol/infra's Onboarder. 0075's statement that running one is out of scope still holds of
  the connector; what changed is that the operator names one.
- **A node's own outbound deposit involves no facilitator.** When a node pays a peer or a client
  (0075 decision 3), it sends `deposit` itself from its settlement key and pays its own gas. It holds
  ETH anyway, so a one-time `approve` costs it nothing that matters.
- **Solana is unchanged.** The payer posts its `open` to the operator's sponsor endpoint, and the
  operator is the fee payer. No facilitator was found offering SVM `batch-settlement` on Solana devnet
  (`docs/research/x402-devnet-facilitators.md`, 2026-09-25), and a third-party sponsor is refused
  (0074 decision 5).

## As found, 2026-09-29

- **The devnet Onboarder offers both extensions.** toon-protocol/infra#40 (merged 2026-09-29)
  registers `eip2612GasSponsoring`, and makes the Onboarder the signer `erc20ApprovalGasSponsoring`
  needs, for the tokens in `ONBOARDER_SPONSORED_TOKENS`. infra#41 pinned the image built from it.
  The devnet allowlists one token, the first mock USDC `0x49beE1Bc…a9Ce`, which has neither
  ERC-3009 nor a permit and is the token 0074's prerequisite 2 ran with. Its `/supported`, read
  live on 2026-09-29, lists both extensions.
- **The approval-funding path is proved on anvil.** infra#40 ran toon-client's
  `batch-settlement-deposit-gas` suite against the real x402 contracts and the Onboarder: a WETH9
  deposit from a wallet with no ETH, a FiatToken Permit2 deposit from a wallet with no ETH that sent
  no transaction at all, and three self-paid cases.
- **On Base Sepolia, the permit path is proved against the live Onboarder.** toon-client#695 records
  it: devnet USDC through Permit2, from a wallet with no ETH, the permit riding inside the deposit.
  The plain-ERC-20 case there needs a funded key to mint the token first. No live Base Sepolia run
  of `erc20ApprovalGasSponsoring` against the Onboarder is recorded in #695 or infra#40.
- **x402.org still advertises `erc20ApprovalGasSponsoring`** on its `/supported` (read live on
  2026-09-29). Whether it now funds the approval is not known: nothing has been sent to it since
  the 2026-09-25 run.

## Considered options

- **Leave facilitators to the client (0075 as written).** Rejected. A client would have to find a
  facilitator on its own, and on mainnet x402.org lists none. Each client would pick a different
  third party to relay real money, and the seller, who wants the deposit, would pay nothing towards
  it.
- **Have the connector call the facilitator itself, as a stock x402 seller does.** Rejected. The
  deposit comes before the channel and outside the packet path, so there is no paid request for the
  seller to forward it with. A connector that relayed deposits would also become a public endpoint
  that spends on a stranger's request, which on EVM is the facilitator's job and its risk to bound.
- **Charge the payer for gas.** Rejected. `x402BatchSettlement` has no fee field, so the charge
  would be a second, off-chain payment beside the voucher, and a payer with no ETH could not make
  it.
- **Run a facilitator inside the connector.** Rejected, and out of scope. It would put a second hot
  key and a public spending endpoint in the binary for a job x402's published facilitator already
  does. 0075 rejected it for the same reason.

## Consequences

- **A payer with the token and no ETH can deposit on EVM whenever the operator names a facilitator
  that sponsors that token.** For an ERC-3009 token any facilitator does. For any other ERC-20 the
  facilitator must offer the matching extension.
- **An operator's EVM costs include its payers' deposit gas.** It is paid off chain, at whatever the
  facilitator charges, or from the gas key of the facilitator the operator runs.
- **An operator that names none still gets paid**, but only by payers that bring their own
  facilitator or hold ETH.
- **The greeting names a URL the connector does not check.** A stale or wrong `facilitator_url`
  surfaces as failed deposits on the payer's side, never as a refusal at boot.
- **0075's out-of-scope line is amended, not reversed.** The connector still runs and calls no
  facilitator, and a node still pays its own gas for its own deposits.
