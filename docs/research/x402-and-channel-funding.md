# x402 and channel funding

> **Research note, not a decision record.** Nothing here decides anything; [ADR
> 0022](../adr/0022-a-connector-answers-it-does-not-announce.md) is the record that already
> deferred this question, and [ADR 0042](../adr/0042-a-packet-carries-its-claim.md) and
> [ADR 0052](../adr/0052-permissionless-payment-is-guaranteed-and-a-claim-is-what-authorises.md)
> own the model it would have to fit beside. Primary sources only — the x402 specification text,
> the EIP text, and the deployed contract/program source in this repository. Written 2026-09-23.

The question: a client has money somewhere and no channel. How does it get from "holds USDC in a
wallet" to "holds a funded channel it can sign claims against" — and can x402, or an x402
facilitator, do that work for it?

Three things are kept apart throughout, because collapsing them is what makes this question look
easy:

- **(a) what the x402 specification guarantees** — cited to `specs/` in the x402 repository;
- **(b) what a particular facilitator happens to do** — cited to that facilitator's own docs, and
  never generalised;
- **(c) what TOON would have to build itself** — cited to this tree.

---

## Verdict

**You are about 80% right, and the 20% that is wrong is the load-bearing part.** x402 is a good
fit for the on-ramp — a client with no channel paying once, over plain HTTP, with a signature
instead of a chain connection — and this repository already emits x402 v2 terms as its greeting
([`client-edge-spec.md` §1.4](../protocol/client-edge-spec.md)). But **a facilitator cannot open
or fund a payment channel**, and not because no one has built one that does: the `exact` scheme's
settlement step is defined as a bare token transfer to `payTo` — `transferWithAuthorization` on
the token contract ([`scheme_exact_evm.md` Phase
3](https://github.com/coinbase/x402/blob/main/specs/schemes/exact/scheme_exact_evm.md)) — with no
contract call and no callback anywhere in it. Point `payTo` at `TokenNetwork` and the USDC lands
in the contract crediting nobody, which is precisely the "unprocessed, locked up deposits" failure
[ERC-3009's own Security Considerations](https://eips.ethereum.org/EIPS/eip-3009) warns about.

**Signing claims does not fit inside `exact` at all, and does not need to.** x402 has a scheme for
exactly TOON's shape — `batch-settlement`, whose own summary names "payment channels" and whose
worked example is "each request increments a signed running total (a receipt)"
([`batch_settlement.md`](https://github.com/coinbase/x402/blob/main/specs/schemes/batch-settlement/batch_settlement.md)).
So the right framing is not "x402 instead of claims" and not "x402 underneath claims"; it is
**two x402 schemes in one `accepts[]` list**: `exact` for the first, channel-less packet (the
on-ramp ADR 0022 deferred), and a `batch-settlement` binding for the steady state (what this
connector already does under the bespoke name `toon-channel`). Funding the channel itself is a
**third** thing that x402 never touches, and the good news is this tree already has most of the
machinery for it — `TokenNetwork` is ERC-2771-aware, the gas station already relays
`setTotalDeposit` for free, and the only genuine gap is the very first transaction.

---

## 1. What x402 is, mechanically

**Three roles, four messages.** A _resource server_ answers an unpaid request with
`PaymentRequired`; a _client_ returns a `PaymentPayload`; a _facilitator_ verifies and settles
([`x402-specification-v2.md`
§3](https://github.com/coinbase/x402/blob/main/specs/x402-specification-v2.md)). The core types
are transport-agnostic — "All transports and schemes use these exact data structures, differing
only in how they represent them (transport layer) and what validation/settlement logic they apply
(scheme layer)" (ibid. §5).

**Headers moved in v2.** v1 used `X-PAYMENT` / `X-PAYMENT-RESPONSE`
([`transports-v1/http.md`](https://github.com/coinbase/x402/blob/main/specs/transports-v1/http.md)).
v2 uses three headers, each carrying base64 JSON
([`transports-v2/http.md`, "Header
Summary"](https://github.com/coinbase/x402/blob/main/specs/transports-v2/http.md)):

| Header              | Direction       | Carries              |
| ------------------- | --------------- | -------------------- |
| `PAYMENT-REQUIRED`  | server → client | `PaymentRequired`    |
| `PAYMENT-SIGNATURE` | client → server | `PaymentPayload`     |
| `PAYMENT-RESPONSE`  | server → client | `SettlementResponse` |

**`PaymentRequirements` is the offer.** Required fields: `scheme`, `network` (CAIP-2), `amount`
(atomic units, decimal string), `asset`, `payTo`, `maxTimeoutSeconds`; `extra` is scheme-specific
(§5.1.2). `PaymentPayload` echoes the chosen requirement as `accepted` and adds a scheme-specific
`payload` (§5.2.2).

**Schemes that exist today** — the specification directory has exactly three, plus per-chain
bindings:

- **`exact`** — a fixed amount, known before signing
  ([`scheme_exact.md`](https://github.com/coinbase/x402/blob/main/specs/schemes/exact/scheme_exact.md)).
- **`upto`** — client signs a maximum, server settles the actual amount at settlement time
  ([`scheme_upto.md`](https://github.com/coinbase/x402/blob/main/specs/schemes/upto/scheme_upto.md),
  added 2026-03). Explicitly **not** multi-settlement: its "Out of Scope" section rules out
  "Multi-settlement / streaming: Settling the same authorization multiple times", "Recurring
  payments", and "Open-ended allowances".
- **`batch-settlement`** — "the client provides a cryptographic payment commitment at request
  time, but the transfer of value is not executed synchronously during that request"
  ([`batch_settlement.md`](https://github.com/coinbase/x402/blob/main/specs/schemes/batch-settlement/batch_settlement.md)).
  See §4 — this is TOON's scheme.

The v2 core spec's architecture section also name-drops a "deferred" scheme (§ Architecture,
item 2). **No `deferred` spec file exists** in `specs/schemes/` — treat it as unbuilt.

**`exact` on EVM has three asset-transfer methods**, not one
([`scheme_exact_evm.md`](https://github.com/coinbase/x402/blob/main/specs/schemes/exact/scheme_exact_evm.md)):

1. **EIP-3009** (`transferWithAuthorization`) — the default, for tokens that support it. "In all
   cases, the Facilitator cannot modify the amount or destination. They serve only as the
   transaction broadcaster."
2. **Permit2** — universal ERC-20 fallback, settled through a canonical
   `x402ExactPermit2Proxy` at `0x402085c248EeA27D92E8b30b2C58ed07f9E20001` whose entire job is to
   bind the destination: `transferDetails.to = witness.to`, from a witness hash the user signed.
3. **ERC-7710** delegation, for smart accounts. Even here the facilitator "**Constructs** the
   `executionCallData` encoding an ERC-20 `transfer(payTo, amount)` call" — the scheme pins the
   delegated action to a transfer.

All three end in a token transfer to an address the payer signed. None of them calls a method on
`payTo`.

---

## 2. The facilitator role — what it does, and what it cannot

**Spec-level (a).** "A service that handles payment verification and blockchain settlement"
(v2 spec §3). Three HTTP endpoints (§7):

- `POST /verify` — "Verifies a payment authorization without executing the transaction on the
  blockchain." Returns `{ isValid, invalidReason?, payer? }`.
- `POST /settle` — broadcasts it. Returns `{ success, transaction, network, payer?, amount? }`.
- `GET /supported` — `{ kinds: [{x402Version, scheme, network}], extensions: [], signers: {} }`.

The scope limit is stated flatly in the project's own facilitator page: "The facilitator does not
hold funds or act as a custodian — it performs verification and execution of onchain transactions
based on signed payloads provided by clients"
([`docs/core-concepts/facilitator.md`](https://github.com/coinbase/x402/blob/main/docs/core-concepts/facilitator.md)).

**`/supported` is a closed list.** A facilitator answers for the `(scheme, network)` pairs it
implements; anything else is `unsupported_scheme` (v2 spec §9). A scheme nobody has implemented is
not a scheme a third-party facilitator will settle.

**Facilitator-level (b) — what one specific implementation does.** Coinbase's CDP facilitator
supports `exact`, `upto` and `batch-settlement` on its EVM networks and `exact`/`upto` on Solana;
all ERC-20s via EIP-3009 or Permit2; first 1,000 on-chain settlements per month free, then $0.001
each; it sponsors the Permit2 approval for EIP-2612 tokens so buyers need no native gas
([docs.cdp.coinbase.com/x402/network-support](https://docs.cdp.coinbase.com/x402/network-support)).
That is one vendor's product surface, not a protocol guarantee — do not design against it as
though it were §7.

**The closest thing in the spec to "the facilitator does something extra for you"** is the gas
sponsoring extension pair, and it is worth reading carefully because it is the shape a TOON
sponsored-deposit relayer would take. Under
[`erc20_gas_sponsoring.md`](https://github.com/coinbase/x402/blob/main/specs/extensions/erc20_gas_sponsoring.md)
the facilitator agrees to "Fund the Client's wallet with enough native gas token **if the Client
lacks sufficient funds**", "Broadcast the Client's signed approval transaction", and settle
immediately after — as "an **atomic batch transaction**". So: a facilitator _can_ be made to
broadcast a user-signed arbitrary transaction and pay its gas. It just is not doing that under
`exact`; it is doing it under a named extension, with a client-supplied RLP blob, because the
scheme's own settlement step cannot express it.

---

## 3. Can an x402 payload fund a channel deposit? The crux

### 3.1 What an EIP-3009 authorization actually binds

The signed struct is exactly six fields
([EIP-3009, Specification](https://eips.ethereum.org/EIPS/eip-3009)):

```solidity
// keccak256("TransferWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)")
```

`from`, `to`, `value`, a validity window, and a random nonce. **There is no calldata field, no
target-method field and no post-transfer hook.** The authorization says "move `value` of _this
token_ from `from` to `to`". It cannot say "and then call `setTotalDeposit`". ERC-2612 `permit`
is weaker still: it authorises an _allowance_, and the spend is a separate `transferFrom` by the
spender.

### 3.2 The no-callback problem, stated by the EIP itself

ERC-3009 has **two** entry points, and the difference is the whole answer:

- `transferWithAuthorization(...)` — anyone may submit it.
- `receiveWithAuthorization(...)` — "This has an additional check to ensure that the payee's
  address matches the caller of this function to prevent front-running attacks."

The EIP's Security Considerations say why, in the words that matter here:

> Use `receiveWithAuthorization` instead of `transferWithAuthorization` when calling from other
> smart contracts. It is possible for an attacker watching the transaction pool to extract the
> transfer authorization and front-run the `transferWithAuthorization` call to execute the
> transfer without invoking the wrapper function. **This could potentially result in unprocessed,
> locked up deposits.**

And the EIP ships the wrapper pattern in its own Specification section — a `deposit(address
token, bytes calldata receiveAuthorization)` that decodes `(from, to, amount)`, asserts
`to == address(this)`, and calls `receiveWithAuthorization` by selector `0xef55bec6`. Its
Backwards Compatibility section spells out the forwarder recipe for contracts that use the
allowance pattern: receive to the forwarder, approve the parent, call the parent's method.

**So the honest statement is not "ERC-3009 has no callback, full stop."** It is:

- There is **no receive hook** — a plain ERC-20 credit to a contract triggers nothing, and neither
  `transferWithAuthorization` nor Permit2's `permitWitnessTransferFrom` (nor the
  `x402ExactPermit2Proxy`, which just sets `transferDetails.to = witness.to`) calls anything on
  the recipient.
- A contract **can** be credited safely and atomically — but only through
  `receiveWithAuthorization` from a wrapper that is itself the `to`.
- **x402's `exact` scheme settles with `transferWithAuthorization`, not `receiveWithAuthorization`**
  ([`scheme_exact_evm.md`, Phase 3](https://github.com/coinbase/x402/blob/main/specs/schemes/exact/scheme_exact_evm.md):
  "Settlement is performed via the facilitator calling the `transferWithAuthorization` function").
  Therefore a stock facilitator's settlement of an `exact` payment whose `payTo` is a channel
  contract is precisely the failure mode the EIP warns against.

### 3.3 What TOON's own contracts require

Two facts from this tree decide what is even possible.

**EVM: deposit is delegatable, open is not.**
[`TokenNetwork.setTotalDeposit(bytes32 channelId, address participant, uint256 totalDeposit)`](../../packages/contracts/src/TokenNetwork.sol)
(`packages/contracts/src/TokenNetwork.sol:266`) validates only that `participant` is one of the
two channel participants (`:272-274`) and pulls tokens from the caller:

```solidity
// Pulled from _msgSender() (the forwarded signer), never the forwarder's own balance/allowance
IERC20(token).safeTransferFrom(_msgSender(), address(this), depositAmount);
```

(`TokenNetwork.sol:284`.) **Caller and credited participant are independent parameters** — the
Rust settlement backend says so in as many words at
`crates/connector-settlement-evm/src/lib.rs:890-893` ("a delegate deposit `TokenNetwork` happens
to permit … and `packages/solana-program` deliberately does not"). `openChannel`, by contrast,
takes only `participant2` and derives the pair from `_msgSender()`
(`TokenNetwork.sol:225-239`) — **a third party cannot open a channel between two other parties.**

`TokenNetwork` is `ERC2771Context` (`TokenNetwork.sol:22`, `:202-204`), constructed with a trusted
forwarder (`:191-199`), so `_msgSender()` resolves through a relayer to the user who signed the
`ForwardRequest`. A meta-transactional deposit is already contractually supported. An
`ERC2771Forwarder` is deployed on Base Sepolia (`docs/evm-deployment.md`), and — a caveat worth
carrying — **`trustedForwarder == address(0)` on Base mainnet**
(`packages/contracts/deployments/base-mainnet.md`), i.e. meta-transactions are off in production.

**Solana: deposit is not delegatable at all.** `process_deposit` requires `depositor.is_signer`
(`packages/solana-program/src/processor.rs:309-311`) and decides the credited side purely from the
signer's pubkey (`:356-360`) — there is no participant parameter. The deployment runbook states
the consequence: a deposit "from a counterparty that is not a connector — `rig`, `toon-client`, a
wallet — … is submitted directly against the deployed program under that participant's own key.
There is no operation on any node … that does it for them, **and adding one is not possible
without changing the program**" (`docs/solana-deployment.md`).

**Neither contract touches ERC-3009 or ERC-2612.** Grepping the tree, `receiveWithAuthorization` /
`transferWithAuthorization` appear in **zero** `.sol` and `.rs` files; `permit` / `IERC20Permit`
appear only inside the vendored OpenZeppelin library, never imported by a TOON contract. Both
deposit paths are `approve` + `transferFrom` (`TokenNetwork.sol:284`;
`RollingSwapChannel.sol:483-487`). The devnet token is worse than neutral here: `MockERC20`
(`packages/contracts/test/mocks/MockERC20.sol`) implements `mint`, `transfer`, `approve`,
`transferFrom` and **nothing else** — no `permit`, no `transferWithAuthorization` — so today's
devnet asset cannot even be paid with x402's default `exact` method.

### 3.4 Where this leaves the two halves of the question

- **(a) Can an x402 payload fund a channel deposit?** Not as `exact` is specified. The token would
  arrive at `TokenNetwork` and `participants[channelId][participant].deposit` would not move. It
  would need a custom `to` contract that uses `receiveWithAuthorization`, which no facilitator's
  `exact` implementation will call.
- **(b) Can a facilitator authorise a `deposit`/`openChannel` on the user's behalf?** Not from an
  ERC-3009 or ERC-2612 signature — neither authorises a method call. It _could_ from an ERC-7710
  delegation or an ERC-2771 `ForwardRequest`, but in both cases the authorisation is a **different
  signature over a different structure** that the user produces specifically for that call, and in
  the ERC-7710 case x402's own scheme text pins the executed calldata to `transfer(payTo, amount)`.
  Nothing about being an x402 facilitator helps; the party doing it is a relayer that happens to
  also run a facilitator.

---

## 4. x402 against TOON's model: where they compose, where they fight

**They already compose, at the envelope.** This connector emits an x402 v2 `PaymentRequired`
document as its greeting — as a `402` body and, on BTP, as `payment-required` protocolData on an
`F06` REJECT (`crates/connector-client-edge/src/lib.rs:753`, shape in
`crates/connector-domain/src/x402.rs`, spec in [`client-edge-spec.md`
§1.4](../protocol/client-edge-spec.md)). The peer carriages emit the same document for an
under-covering peer PREPARE (`crates/connector-peer-btp/src/price_gate.rs:1-4`). The `extra`
object already carries per-chain channel-opening facts — `tokenNetwork`, `tokenAddress`,
`settlementAddress`, `decimals`, and the Solana `programId`
(`connector-domain/src/x402.rs`, issues #617/#632). **A payer is already told, in an x402
greeting, everything it needs to open a channel.** It just has to do the opening itself.

**They fight at the settlement cadence, and ADR 0022 already named the fight.** The deferred-work
note in that record is the sharpest statement of it in the tree:

> A connector fronting an app could plausibly accept a plain HTTP request with payment attached —
> the x402 onramp, for a client with no ILP stack and no channel — and answer `402` with terms
> when payment is absent. That is a second architecture with its own payment verification (**a
> one-shot exact payment settles per request, which inverts ADR 0004 and 0005's "claims are
> constant, settlement is rare"**)…
> — [ADR 0022](../adr/0022-a-connector-answers-it-does-not-announce.md)

Concretely, the conflict is three-fold. **Latency**: `exact` settlement waits for a chain
transaction inside the request (facilitator flow steps 9–11), against a claim verification that is
one ECDSA recover. **Cost**: `payment-spec.md` PM-13 is "one claim per packet, never batched", and
the devnet relay write route is priced at **1 base unit** (`toon-client/docs/devnet.md`) — an
on-chain settlement per 1 µUSDC packet is absurd by three or four orders of magnitude. **Model**:
PM-01's "each claim supersedes the last" and PM-08's watermark are a cumulative state machine;
`exact` and `upto` are both single-use by construction (`scheme_upto.md`, Core Property 1: "Each
authorization MUST be settled at most once").

**And there is in-tree precedent for the collision.** The relay once had exactly the thing being
proposed — an x402 `/publish` endpoint that settled EIP-3009 `transferWithAuthorization` per
request, the facilitator paying gas (`relay` git `b8ec1205^:packages/relay/src/launcher/handlers/x402-settlement.ts`).
It was deleted on 2026-06-22 in favour of "payment … enforced entirely upstream by an external
terminator" (`relay/packages/relay/CHANGELOG.md`). That is not an argument that the on-ramp is
wrong; it is evidence that per-request on-chain settlement and connector-terminated channel
payment did not want to live in the same process.

**The scheme that resolves it exists.** `batch-settlement` is, almost word for word, this
protocol:

> **Payment channel streaming.** A client and provider open a payment channel once. Each request
> increments a signed running total (a receipt). The provider closes the channel periodically,
> collecting accumulated value in one settlement regardless of how many individual requests were
> made.
> — [`batch_settlement.md`, Use
> cases](https://github.com/coinbase/x402/blob/main/specs/schemes/batch-settlement/batch_settlement.md)

Its **capital-backed** trust model ("pre-funded escrow, a payment channel … The trust anchor is
the client's own funds. No network intermediary is required to underwrite access") is TOON's, and
the seven things a network binding MUST specify — commitment format, verification rules, storage,
double-spend prevention, expiry, redemption, trust model — map one-to-one onto
`client-edge-spec.md` §1.3's claim shape and four-step gate, `payment-spec.md` PM-08's watermark,
and the `claimFromChannel` redemption path.

**One interop gap to be honest about (c).** Today's greeting is x402-_shaped_ rather than
x402-_valid_: `scheme` is the bespoke string `"toon-channel"`, `network` and `payTo` are both the
ILP destination rather than a CAIP-2 identifier and an address, and the required `asset` field is
absent entirely (`crates/connector-domain/src/x402.rs:426-433`; the spec's required-field table is
v2 §5.1.2). `client-edge-spec.md` §1.4 says why — the claim gate understands one payment method
and "an `exact` scheme entry would describe a second". That is a defensible local choice, but it
means **no stock x402 client can pay a TOON connector today**, and it is the thing a
`batch-settlement` binding would fix for free.

---

## 5. What a funding flow would actually look like

Assume the worst realistic case: a user holds USDC on Base and **no ETH**, has never opened a
channel, and wants to send one paid packet.

**Step 0 — discovery, already built.** The client sends a claimless, `greeting`-flagged PREPARE
(or a plain unpaid HTTP request) and gets back x402 v2 terms carrying `settlements[]`:
`tokenNetwork`, `tokenAddress`, `settlementAddress`, `decimals`
(`client-edge-spec.md` §1.4; `connector-domain/src/x402.rs`). It now knows who to open a channel
with and on what contract. No key has moved.

**Step 1 — open the channel. This is the wall.** `TokenNetwork.openChannel` derives the pair from
`_msgSender()` (`TokenNetwork.sol:225-239`), so it must be _the user's own address_ that calls it,
directly or through the trusted forwarder. Today `toon-client` sends this transaction itself, from
a raw in-process key, on its own gas — its own module doc says "Everything here is a transaction
or a read the **client** makes, on its own gas, directly against a `TokenNetwork`. A connector has
no endpoint that opens a channel" (`toon-client/packages/client/src/channel/evm/TokenNetworkClient.ts:1-11`),
and a gasless wallet fails with `ChannelFundingError` (`:362-373`). The gas station **deliberately
refuses to help here**: its EVM selector whitelist is exactly `setTotalDeposit`, `closeChannel`,
`settleChannel`, and it excludes `openChannel` and `claimFromChannel` on the stated grounds that
"opening a channel and claiming via a balance proof are not 'an agent reclaiming its own
collateral'" (`gas-station/src/evm-gas-station-handler.ts:35-40`, whitelist at `:218-222`). The
console states the gap without euphemism: "**No faucet on TOON Network gives you native gas.**"
(`console/docs/funding.md`).

**Step 2 — approve.** `setTotalDeposit` pulls with `safeTransferFrom(_msgSender(), …)`
(`TokenNetwork.sol:284`), so the user's address must have an allowance to `TokenNetwork`. That is
a second user-signed transaction, and a second gas cost. `EvmSettlementBackend` approves
`U256::MAX` to avoid repeating it (`crates/connector-settlement-evm/src/lib.rs:519-521`);
`TokenNetworkClient.ensureAllowance` does the same client-side.

**Step 3 — deposit.** Once a channel exists, this one _is_ sponsorable today: the client signs an
ERC-2771 `ForwardRequest` naming itself `from` and `TokenNetwork` `to`, sends it to the gas station
as a NIP-90 kind:5098 job, and the station inspects, simulates, submits its own transaction calling
`forwarder.execute(request)` and pays the gas (`gas-station/src/evm-gas-station-handler.ts:1-12`).
The client-side signing code already exists — `swap/packages/swap/src/gas-station-redeem.ts` signs
exactly this shape against the forwarder's own `eip712Domain()`. **But the gas station is itself a
TOON app behind a connector, paid over a channel** (`gas-station/README.md`), so reaching it
requires the channel you are trying to fund. That is the bootstrap loop.

**Step 4 — steady state.** From here nothing is on-chain. `signBalanceProof` increments nonce and
cumulative, signs the `BalanceProof` EIP-712 struct under domain `("TokenNetwork","1")` with the
resolved `TokenNetwork` as `verifyingContract`
(`toon-client/packages/client/src/signing/evm-signer.ts:40-61`, `:121-158`; typehash at
`TokenNetwork.sol:37-40`), and the claim rides the packet as
`ILP-Payment-Channel-Claim: base64(JSON)` on HTTP or `payment-channel-claim` protocolData on BTP
(`client-edge-spec.md` §1.3). This is `batch-settlement`'s **Commit** and **Accumulate**; the
`claimFromChannel` redemption is its **Redeem**.

### Where x402 actually buys you something

**Option A — `exact` as the bootstrap payment, not as the funding mechanism.** The one packet the
client cannot pay with a claim is the one that breaks the loop. Offer a second `accepts[]` entry
— a real `exact`/EIP-3009 entry with a CAIP-2 `network`, a real USDC `asset`, and `payTo` = the
node's **settlement EOA** (never the contract) — for a narrow, cheap set of routes: the gas
station's `/gas/quote` and `/gas/execute`, and perhaps a hosted "open my channel" job. The
facilitator does what it is specified to do (move USDC from the user to an address), the node gets
paid for a service it renders once, and the _channel funding itself is done by the user's own
signatures being relayed_. This is ADR 0022's deferred on-ramp, scoped to the two or three routes
where per-request settlement is actually the right cadence.

**Option B — a TOON-run facilitator.** Nothing stops TOON running `/verify` + `/settle`.
It buys: control of which `(scheme, network)` pairs are supported, so a
`batch-settlement`/`toon-channel` binding can be advertised and verified through the standard
interface; and the ability to bundle, the way `erc20ApprovalGasSponsoring` already does
("Facilitator batches the following transactions: `from.transfer(gas_amount)` →
`ERC20.approve(Permit2)` → `settle`"). A TOON facilitator could batch
`forwarder.execute(openChannel)` → `forwarder.execute(setTotalDeposit)` in one relayed
transaction. It buys nothing on the crux: it still cannot make an ERC-3009 signature authorise a
contract call.

**Option C — a `receiveWithAuthorization` deposit wrapper.** The cleanest EVM answer, and the one
the EIP itself documents. A small `ChannelDepositForwarder` that takes
`deposit(bytes calldata receiveAuthorization, bytes32 channelId, address participant)`, asserts
`to == address(this)`, calls `receiveWithAuthorization` (selector `0xef55bec6`), approves
`TokenNetwork`, and calls `setTotalDeposit(channelId, participant, newTotal)` — which works
_because_ `setTotalDeposit`'s caller and credited participant are independent (`TokenNetwork.sol:266`,
`:272-274`). One user signature, zero user gas, no allowance transaction, atomic. **What it costs
(c):** a new audited contract; a relayer to submit it (any address may); it does **not** work for
`openChannel`, which still needs the user's own `_msgSender()`; it needs a token with EIP-3009,
which devnet's `MockERC20` is not; and it is **not** something a stock x402 facilitator will ever
call — the payload would have to travel as an x402 _extension_ or as a bespoke scheme, which
means you are running Option B anyway.

**Option D — sponsored open.** Extend the gas station's whitelist to `openChannel`, and reach it
over the `exact` on-ramp from Option A. This is the smallest change that actually closes the loop,
and it is a policy reversal the gas station argued against on purpose
(`gas-station/src/evm-gas-station-handler.ts:35-40`) — so it needs a decision, not a patch.

**Solana is a separate answer.** There is no delegate deposit and there cannot be one without
changing the program (`docs/solana-deployment.md`; `processor.rs:309-311`, `:356-360`). What Solana
_does_ have is fee-payer separation, and the gas station's kind:5096 leg already uses it: the
client authors and signs the transaction, the station co-signs as fee payer and broadcasts. So on
Solana a gasless deposit is already structurally available for an existing channel — the user just
needs the SPL tokens in their own ATA.

---

## 6. What does not work, and what is unverified

**Does not work, with confidence:**

- Setting an x402 `exact` `payTo` to `TokenNetwork` (or any channel contract) and expecting a
  deposit. The funds arrive; no state changes; the EIP names this outcome
  ([EIP-3009, Security Considerations](https://eips.ethereum.org/EIPS/eip-3009)).
- Expecting a third-party facilitator to `openChannel` for a user on EVM. `_msgSender()` decides
  the pair (`TokenNetwork.sol:225-239`).
- Expecting anyone to deposit on a user's behalf on Solana (`processor.rs:309-311`).
- Using `upto` as "a claim". It is single-use and explicitly rules out multi-settlement
  ([`scheme_upto.md`](https://github.com/coinbase/x402/blob/main/specs/schemes/upto/scheme_upto.md)).
- Paying today's devnet with x402 `exact`: `MockERC20` implements neither EIP-3009 nor EIP-2612
  (`packages/contracts/test/mocks/MockERC20.sol`).
- Paying today's connector with a stock x402 client at all: `scheme: "toon-channel"`, non-CAIP-2
  `network`, absent `asset` (`crates/connector-domain/src/x402.rs:426-433` against v2 §5.1.2).

**Open, and deliberately not guessed at:**

- **Is there a published `batch-settlement` binding for on-chain payment channels?** The only
  binding in the repository is `batch_settlement_cloudflare.md`, which is _credit_-backed and
  authenticates with RFC 9421 HTTP Message Signatures. Whether the capital-backed half has any
  reference implementation — **unverified**.
- **Would the CDP facilitator settle a TOON `batch-settlement` binding?** Its docs say it supports
  the `batch-settlement` scheme; whether a third party's network binding can be registered with it
  is not stated anywhere I could find — **unverified**.
- **Does `ERC2771Forwarder` need to be re-enabled on Base mainnet?** `trustedForwarder ==
address(0)` there (`packages/contracts/deployments/base-mainnet.md`), which turns off the whole
  meta-transaction path in production. Whether that was a deliberate mainnet posture or an
  oversight — **not answered by any record I found**.
- **Does the gas station's refusal to relay `openChannel` survive contact with a real on-ramp?**
  Its stated reason is about scope, not safety, and `openChannel` moves no tokens. Reopening it is
  a decision for a record, not a note.
- **Cost of the on-ramp.** A CDP settlement is $0.001 after the free tier
  ([network-support](https://docs.cdp.coinbase.com/x402/network-support)) against a 1 µUSDC relay
  write. Whether an `exact` on-ramp can ever be priced above its own settlement cost on the routes
  that need it — worth arithmetic before any of this is built.

**One documentation bug found in passing**, worth a separate fix: several Rust comments cite
`TokenNetwork.sol:252 / :255 / :273 / :282` for the delegate-deposit property
(`crates/connector-settlement-evm/src/lib.rs:890-893`, `crates/connector-settlement/src/port.rs`,
`crates/connector-bin/tests/base_sepolia_redeem_proof.rs`). The claims are correct; the line
numbers are stale by 11–14 lines — the current coordinates are `:266` (signature), `:272-274`
(participant check), `:284` (`safeTransferFrom`).
