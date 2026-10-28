# How XRPL's channel lifecycle fits the port and the contract suites (#1565, #1570)

> **Research note, not a decision record.** It decides nothing. It gathers the facts behind the
> design grilling of [#1565](https://github.com/toon-protocol/connector/issues/1565) ("Port and
> contract-suite changes for chains whose channel lifecycle differs", map
> [#1563](https://github.com/toon-protocol/connector/issues/1563)), and XRPL's details
> ([#1570](https://github.com/toon-protocol/connector/issues/1570)). It builds on
> [`x402-pending-chain-bindings.md`](x402-pending-chain-bindings.md) (cited as **PENDING**, whose §2
> is XRPL and §8/D3/D7 the port questions) and does not redo that ground. Primary sources only.
> Written 2026-10-28, and every source was read that day.

**Evidence tags.** CONFIRMED-CODE means read in source at a pinned commit, or read live from a
chain's RPC. CONFIRMED-DOCS means read in the maintainer's own documents or specs. UNCONFIRMED means
neither, and says why. A sentence marked **Inference** is this note's reasoning, not a fact.

**Sources pinned.**

- **rippled / xrpld**: tag [`3.4.1`](https://github.com/XRPLF/rippled/tree/00e640751460c137636fdb6dae6b4280de355d81)
  = commit `00e64075` (the version PENDING §2 found on mainnet). Cited as **XRPLD** `path:line`.
- **XRPL docs**: xrpl.org pages [PaymentChannelClaim](https://xrpl.org/docs/references/protocol/transactions/types/paymentchannelclaim),
  [PayChannel](https://xrpl.org/docs/references/protocol/ledger-data/ledger-entry-types/paychannel),
  [ledger_entry](https://xrpl.org/docs/references/http-websocket-apis/public-api-methods/ledger-methods/ledger_entry),
  [transaction metadata](https://xrpl.org/docs/references/protocol/transactions/metadata),
  [advance the ledger in stand-alone mode](https://xrpl.org/docs/infrastructure/testing-and-auditing/advance-the-ledger-in-stand-alone-mode).
- **Mainnet, live**: `feature` and `ledger_entry` against `s1.ripple.com:51234` (a Clio server),
  validated ledger 107960081.
- **x402**: [`x402-foundation/x402@261d0c5e`](https://github.com/x402-foundation/x402/tree/261d0c5e1b3d65c7d41839923399de39cd02cec1);
  XRPL PR [#2952](https://github.com/x402-foundation/x402/pull/2952), head still
  [`Dhali-org/x402@683fb7a6`](https://github.com/Dhali-org/x402/blob/683fb7a6b08b5cc12568b36d6f560ba0d57dcf1e/specs/schemes/batch-settlement/scheme_batch_settlement_xrpl.md)
  (`gh pr view`: OPEN, `REVIEW_REQUIRED`, last update 2026-08-13). Cited as **SPEC** `§n` / `Ln`.
- **This repository**: `HEAD` `9a039468` (branch `adr-0077-zk-note`). Paths are relative to the
  repository root.

---

## Decisions already accepted (owner, 2026-10-28)

These are the context for this note, not its findings:

1. **Scope.** #1565 decides the general rule and settles XRPL's details (#1570). Cardano (#1572) and
   Hedera (#1571) misfits go on their own parked issues. Mina (#1552) applies the rule.
2. **Rule.** The port's enums are the **union**: a variant enters when some chain can report it.
   The suites are the **intersection**: a clause enters only when it holds on every chain.
   Everything else is tested in that chain's own tests, beside the suites. The fake's `PayerExit`
   grows a shape only when a suite clause needs it.
3. **Deleted on end.** A channel deleted when it ends follows Solana: the receiver reports
   `ChannelNotFound` and keeps no copy; the payer half caches its final state. No new terminal status.
4. **Amounts** stay `u128` in the port. Each chain narrows at its boundary and refuses an over-width
   amount by a named error, never truncating. That is tested beside the suite.
5. **`VoucherSigner`** grows one variant per chain, e.g. `Xrpl(XrplKey)` with secp256k1 | Ed25519
   inside.
6. **Clocks.** Chains without a movable clock follow Solana: `let_delay_pass` is a no-op, and the
   paying suite's finish step is reached because the receiver seals. The real time-elapsed exit is a
   chain-only test with a short on-chain delay, waited in real time. The suite's 901 s floor and
   `DelayBelowMinimum` stay.

**Outcome.** Decision 6 did not survive this note (§A.4): the owner replaced it the same day, with
the further decisions this note's open questions asked for. The decisions as accepted are recorded in
[ADR 0075, "Update (issue #1565)"](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md);
the list above is kept as what the note was written against.

---

## Verdict

- **Decision 6 does not carry over to XRPL as written.** On Solana, the receiver's seal and the
  payer's refund are two transactions: `settle_and_seal`, then the payer's `distribute`. On XRPL
  they are **one**. A destination `PaymentChannelClaim` with `tfClose` claims, refunds the source
  and deletes the entry, all in the same transaction (XRPLD `PaymentChannelClaim.cpp:131-192`,
  `PaymentChannelHelpers.cpp:53-66`).
  - So if the receiver seals in `land`, the 1,100 refund reaches the payer **before** the suite
    reads `before` (`contract.rs:681-689`). The delta asserted at `contract.rs:696-700` is then 0.
  - If the receiver does not seal, nothing ends the channel before `Expiration`, and a no-op
    `let_delay_pass` leaves `finish_withdrawal` with `WithdrawalNotDue`.
  - **Inference:** XRPL passes the paying suite unmodified only with a `let_delay_pass` that really
    lets the delay pass. The paying fixture is free to publish a short `SettleDelay`: the 901 s
    floor is asserted only by the receiving suite (`contract.rs:108-111`).
- **A second misfit, not raised before: fees are paid in the asset the channel holds.** The paying
  suite asserts exact payer-balance deltas: `−1,000` (`contract.rs:520-524`), `−2,000` (`:555`),
  `−2,500` (`:561`) and `+1,100` (`:696-700`).
  - On EVM and Solana, `payer_balance` reads a token (USDC, or an SPL mint), and gas is paid in
    ETH or SOL. The Solana harness reads `token_balance` (`batch_settlement_solana_paying.rs:250`).
  - On XRPL the channel holds XRP, and every payer transaction burns its XRP fee from the same
    balance. Each delta is therefore off by the fees.
  - **Inference:** either the XRPL fixture's `payer_balance` adds back fees read from the chain, or
    the clause changes. Either way, this is for the grilling.
- **Decision 3 holds, with one constraint on `land`.** A plain `ledger_entry` cannot tell "ended"
  from "never existed": both give `entryNotFound`. That supports decision 3.
  - But the receiving suite reads `channel_state` after landing on an exiting channel, and expects
    `landed == 400` (`contract.rs:356-369`).
  - So XRPL's `land` on a `Closing` channel must **not** add `tfClose`. Otherwise the entry is gone
    and the read is `ChannelNotFound`.
- **Decision 4 holds, but XRPL's real width is 10¹⁷ drops, not `u64::MAX`.** The claim message
  carries a u64. The ledger, though, holds XRP as `int64` drops and caps native amounts at
  10¹⁷ drops.
- **Decision 5 holds.** A 33-byte XRPL public key already says its own type (`0xED` prefix =
  Ed25519, `0x02`/`0x03` = secp256k1).
- **The exit is reversible only by the payer**, through `tfRenew` or `PaymentChannelFund`. No
  receiver-side code in this repository caches "exiting" as terminal. The only `Closing`-is-final
  shape is the fake's `PayerExit::Close`.

---

## A. The paying suite's finish step, and whether XRPL can reach it

### A.1 What the suite requires

From `assert_upholds_the_paying_contract` (`crates/connector-settlement/src/batch/contract.rs`),
CONFIRMED-CODE:

| Step                                                                                     | Lines     |
| ---------------------------------------------------------------------------------------- | --------- |
| Payer signs 400 and the receiver holds it, unlanded                                      | `630`     |
| `start_withdrawal` → `Withdrawing \| Closing`, collateral 0, `signed` 400                | `631-647` |
| Starting again returns the identical state                                               | `648-655` |
| `receiver.channel_state` equals the payer's `on_chain` ("from the chain alone")          | `656-660` |
| Signing 450 → `VoucherUnbacked { backed: 300 }`                                          | `663-669` |
| `finish_withdrawal` inside the delay → `WithdrawalNotDue`                                | `671-677` |
| `receiver.land(held)` → `landed == 400`                                                  | `681-686` |
| `let_delay_pass()`, then `before = payer_balance()`                                      | `688-689` |
| `finish_withdrawal` → Ok, `on_chain.landed == 400`, `collateral == 0`                    | `690-695` |
| `payer_balance() − before == 1_100`                                                      | `696-700` |
| `finish_withdrawal` again → `NoWithdrawalPending`; a channel never withdrawn from → same | `701-711` |

Nothing after this step reads the finished channel's state: the restart section (`760-945`) only
restores and starts withdrawals. There is no `Sealed` or `ChannelSealed` clause anywhere in
`contract.rs` (grep). The fixture contract for `let_delay_pass` (`contract.rs:399-403`) says that
"the suite only finishes a Solana close after the receiver has sealed it, which is due at once, so
on a validator, whose clock cannot be moved, this may do nothing".

### A.2 How Solana satisfies it with a no-op

CONFIRMED-CODE:

1. **The harness.** `let_delay_pass` is `Box::pin(async {})`
   (`crates/connector-cli/tests/batch_settlement_solana_paying.rs:263`). The terms carry a one-day
   grace (`:54`, `:204`). `payer_balance` reads the SPL token balance (`:250`). EVM, for contrast,
   moves anvil's clock by a day (`crates/connector-settlement-evm/tests/batch_settlement_paying.rs:171-176`).
2. **`start_withdrawal`** sends `request_close`, which takes Open to Closing. It sets `backed` to 0
   (`crates/connector-settlement-solana/src/batch/pay.rs:780-812`).
3. **The receiver's `land` on a Closing channel** sends `settle_and_seal` instead of `settle`
   (`crates/connector-settlement-solana/src/batch/mod.rs:807-818`, branch at `:871-888`). The
   channel is now **Sealed**, its account still allocated, and **no tokens have moved to the payer
   yet**.
4. **`finish_withdrawal`** (`pay.rs:818-894`) handles each status:
   - Sealed: it goes straight to `distribute` (`:830`, `:853`), which pays the receiver's share and
     refunds the payer the rest. **The refund happens here, after `before` was read**, so the delta
     is exactly 1,100.
   - Closing: it checks the on-chain clock against `closure_started_at + grace_period`, returns
     `WithdrawalNotDue` before the deadline (`:831-841`), and cranks `seal` after it (`:843`).
   - Open or Distributed, or an account that is gone: `NoWithdrawalPending` (`:825-828`).
5. **After `distribute`.** If `distribute` deallocated the account, the payer reports a synthesized
   Distributed state, maps it to `Sealed` and caches it in `record.finished` (`:877-889`,
   `pay.rs:213-216`). `outbound_state` falls back to that cache when the read is empty
   (`pay.rs:442-460`).
6. **The receiver keeps no copy.** A deallocated account reports `ChannelNotFound`
   (`mod.rs:794-797`, `:383-391`). Decision 3's precedent is exactly this.

### A.3 What XRPL does when the destination closes

All CONFIRMED-CODE (XRPLD `src/libxrpl/tx/transactors/payment_channel/PaymentChannelClaim.cpp`)
and CONFIRMED-DOCS (PaymentChannelClaim page).

- **Claim and close happen in one transaction.**
  - `doApply` first applies `Balance` (`:131-178`): it moves `reqBalance − chanBalance` drops to
    the destination.
  - It then handles `tfClose`: "Channel will close immediately if dry or the receiver closes"
    (`:188-192`) and calls `closeChannel`.
  - The docs: if the destination uses the flag while XRP remains, "any XRP that remains after
    processing the claim is returned to the source address".
- **The refund and the deletion are in the same transaction.** `closeChannel` does the following
  (`src/libxrpl/ledger/helpers/PaymentChannelHelpers.cpp:24-67`):
  1. removes the channel from both owner directories (`:28-51`);
  2. credits the source with `Amount − Balance` and decrements its owner count (`:53-62`);
  3. `view.erase(slep)`, which deletes the entry (`:64-65`).
- **The source's own `tfClose` with XRP remaining only schedules the close.** It sets
  `Expiration = parentCloseTime + SettleDelay` unless an earlier `Expiration` exists (`:194-203`).
  If the channel is dry (`Balance == Amount`), the close is immediate (`:191`).
- **A claim on an expired channel delivers nothing, yet still succeeds.**
  - If `Expiration` or `CancelAfter` has passed, `doApply` calls `closeChannel` **before** any
    `Balance` is applied, and returns its `tesSUCCESS` (`:121-126`).
  - The docs agree: "Any validly formed `PaymentChannelClaim` transaction has this effect,
    regardless of the contents of the transaction".
  - **Inference:** a `land` must check the delivered amount, not only `tesSUCCESS`. SPEC §11 says
    to submit in time to be validated `redemptionSafetySeconds` before the deadline.
- **Anyone can close an expired channel.** The expiry check comes before the
  `txAccount != src && txAccount != dst` check (`:121-129`). `PaymentChannelFund` is the same
  (`PaymentChannelFund.cpp:55-61`).
- **Afterwards the source sees the entry gone.** The refund appears as the source's
  `AccountRoot` balance change, and the channel as a `DeletedNode` whose `FinalFields` are "the
  content fields of the ledger entry immediately before it was deleted" (metadata docs). A
  `ledger_entry` read returns `entryNotFound` (§E).

### A.4 Can an XRPL backend pass the finish step unmodified with a no-op `let_delay_pass`?

**No.** This is an **Inference** from A.1–A.3. The two ways an XRPL `land` could behave on a
`Closing` channel both fail.

| `land` on `Closing`          | Receiving suite `contract.rs:356-369` (`channel_state` after land → landed 400) | Paying suite `:688-700` with no-op `let_delay_pass`                                                                                         |
| ---------------------------- | ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| **claim + `tfClose`** (seal) | **Fails**: the entry is deleted, so the read is `ChannelNotFound` (decision 3)  | **Fails**: the refund lands inside `land`, before `before` is read; the delta is 0, not 1,100                                               |
| **claim, no flag**           | Passes: the entry persists with `Expiration` set                                | **Fails**: `Expiration` has not passed, the payer cannot close a non-dry channel early (`:188-203`), so `finish` returns `WithdrawalNotDue` |

Two fixture-only paths would pass both suites without changing any suite clause (**Inference**):

1. **`land` does not seal; the fixture's delay really passes.**
   - The XRPL paying fixture publishes a short `SettleDelay`. The paying suite asserts no floor;
     only the receiving suite asserts `> 901` (`contract.rs:108-111`). The protocol allows any
     u32 (§D).
   - `let_delay_pass` advances ledger close time past `Expiration`, by waiting in real time or by
     repeated `ledger_accept` (§B).
   - `finish_withdrawal` then sends any source transaction that touches the channel, for example a
     `PaymentChannelClaim` with no fields beyond `Channel`. That transaction closes the channel and
     refunds 1,100 inside `finish`.
   - The backend caches the final state from the `DeletedNode` `FinalFields` as Solana's
     `record.finished` does. The second `finish` sees `entryNotFound` and returns
     `NoWithdrawalPending`, as Solana does at `pay.rs:825`.
   - This still trips the fee misfit (A.5).
2. **`land` seals, and the fixture lies about the clock.** Not viable: the `before` read comes
   after `land`, whatever `let_delay_pass` does.

So for XRPL, `let_delay_pass` is **not** a no-op. The suite's own words allow this ("this may do
nothing", `contract.rs:401-403`, is permissive, not required). It contradicts decision 6's letter
("finish step is reached because the receiver seals").

What `finish_withdrawal` must do on XRPL (**Inference**):

- If the entry is present with `Expiration` not yet passed, return `WithdrawalNotDue` with
  `Expiration − validated close time`.
- If it has passed, submit a touching transaction, then read the refund and final `Balance` from
  the transaction's metadata, and cache them.
- If the entry is gone and a cached final state exists (a receiver `tfClose`, or someone else's
  touch), report from the cache **once**.
- Otherwise return `NoWithdrawalPending`.
- Open question: what `finish` reports when the receiver's own `tfClose` already refunded the
  payer. The refund came earlier, outside `finish`.

### A.5 The fee misfit

CONFIRMED-CODE for the facts; the consequence is **Inference**.

- XRP is both the channel asset and the fee asset. `PaymentChannelCreate` reserves `Amount` from
  the source's own balance (`PaymentChannelCreate.cpp:80-91`), and `PaymentChannelFund` debits it
  (`PaymentChannelFund.cpp:115`).
- Every transaction's fee is burned from the sender's XRP. PENDING §2.6 read a base fee of 10
  drops on mainnet.
- `PayingContractFixture::payer_balance` is documented as "the payer's settlement account's balance
  of the shared token, read from the chain" (`contract.rs:390-392`).
- On XRPL every `payer_balance` delta in the suite (`:505-507`, `:520-524`, `:555`, `:561`,
  `:696-700`) is off by the payer's own fees.
- The owner reserve (0.2 XRP per channel) is a minimum the balance must keep, not a debit, so it
  does not move the balance. CONFIRMED-CODE (`PaymentChannelCreate.cpp:84-90`).
- Mina, the native-asset chain #1552 plans, has the same coupling. **Inference.**

---

## B. The stand-alone clock

- **`ledger_accept` takes no parameters.** `doLedgerAccept` reads no params. It refuses outside
  stand-alone mode and calls `netOps.acceptLedger()` (XRPLD
  `src/xrpld/rpc/handlers/admin/server_control/LedgerAccept.cpp:14-31`; the role is ADMIN at
  `src/xrpld/rpc/detail/Handler.cpp:179-182`). The docs list no parameters either. CONFIRMED-CODE,
  DOCS.
- **The close time comes from the wall clock.** `acceptLedger` calls
  `consensus_.simulate(getTimeKeeper().closeTime(), …)` (`src/xrpld/app/misc/NetworkOPs.cpp:4521-4535`).
  `TimeKeeper::closeTime()` is `system_clock::now()` plus `closeOffset_`
  (`src/xrpld/core/TimeKeeper.h:47-64`). `closeOffset_` changes only through `adjustCloseTime`,
  which consensus drives (`:76-103`). CONFIRMED-CODE.
- **No command-line or config override exists.** `Main.cpp`'s options (`:379-398`; `--start`,
  `--ledger`, `--net`, `--valid`, `--quorum`, `--force_ledger_present_range`) include none for
  time, and `Config.cpp` has none. The test framework's `ManualTimeKeeper`
  (`src/test/jtx/ManualTimeKeeper.h`) is compiled only into the unit tests. CONFIRMED-CODE.
- **Each ledger moves the close time forward by at least 1 s.** A ledger's close time is
  `max(roundCloseTime(closeTime, resolution), priorCloseTime + 1s)`
  (`include/xrpl/ledger/LedgerTiming.h:145-157`). It is `prevLedger.closeTime() + 1s` when there
  is no agreed close time (`src/xrpld/app/consensus/RCLConsensus.cpp:474-486`). The resolution is
  one of 10/20/30/60/90/120 s, starting at 30 s (`LedgerTiming.h:17-28`). CONFIRMED-CODE.
  - **Inference, not run:** N quick `ledger_accept` calls push the parent close time at least N
    seconds ahead of the wall clock. So a few hundred calls could stand in for a time warp over a
    short `SettleDelay`. No binary was run to check this (PENDING §2.5: xrpld is not installed
    here). UNCONFIRMED in practice.
- **The protocol sets no minimum `SettleDelay`.** It "can be any value that fits in a 32-bit
  unsigned integer (0 to 2^32-1)" (PayChannel docs). `PaymentChannelCreate::preflight` checks no
  delay (`PaymentChannelCreate.cpp:57-70`). CONFIRMED-CODE, DOCS. The only floor is the binding's
  own: SPEC §5 requires `extra.minSettleDelay ≥ maxTimeoutSeconds + redemptionSafetySeconds`, and
  §6.1 requires the channel's `SettleDelay ≥ extra.minSettleDelay`. SPEC sets no absolute number
  (no "900" appears in it).
- **Expiry rule.** A channel is expired when the **parent** ledger's close time is after the
  deadline. With `fixCleanup3_2_0` this is strictly after, otherwise `>=`
  (`PaymentChannelHelpers.cpp:83-91`). `fixCleanup3_2_0` is **enabled on mainnet** (live `feature`,
  ledger 107960081). CONFIRMED-CODE.
- **Real-time cost** (**Inference**): with `SettleDelay = S`, `Expiration = t₀ + S`. Expiry needs
  a ledger whose parent closed after that. In real time that is about `S` seconds, plus up to half
  a close resolution of rounding (≤ 15 s at the 30 s default), plus two `ledger_accept` calls.
  With `S` = 2–5 s a chain-only exit test waits roughly 10–20 s. The suite's 901 s floor applies
  only to the receiving suite's admission clauses (`contract.rs:108-111`, `:159`, `:211`), which
  never wait.

---

## C. `tfRenew`, `CancelAfter`, and whether this repository assumes a monotonic exit

### C.1 XRPL semantics (CONFIRMED-CODE, DOCS)

- **`tfRenew`** clears `Expiration` (`PaymentChannelClaim.cpp:180-186`; docs: "Clear the channel's
  `Expiration` time").
  - Only the source may send it. Otherwise the result is `tecNO_PERMISSION` (`:182-183`; docs:
    "Only the source address of the payment channel can use this flag").
  - It cannot be combined with `tfClose` (`temMALFORMED`, `:60-61`).
  - It cannot rescue an expired channel, because the expiry close at `:121-126` runs first.
- **`PaymentChannelFund` can also move `Expiration`.** Only the source may send it
  (`PaymentChannelFund.cpp:61-65`). A new `Expiration` must be at least
  `min(parentCloseTime + SettleDelay, current Expiration)` (`:67-83`), so it can push the deadline
  later. It cannot clear it.
- **`CancelAfter`** is "the immutable expiration time … set by the transaction that created the
  channel, and cannot be changed" (PayChannel docs).
  - Only `PaymentChannelCreate` writes it (`PaymentChannelCreate.cpp:183`). Neither Claim nor Fund
    writes `sfCancelAfter`.
  - With `fixPayChanCancelAfter`, a create whose `CancelAfter` is already past fails with
    `tecEXPIRED` (`:129-134`). `fixPayChanCancelAfter` is **enabled on mainnet** (live `feature`).
  - At `CancelAfter`, any Claim or Fund closes the channel and refunds the source, the same as at
    `Expiration` (`PaymentChannelClaim.cpp:122-125`, `PaymentChannelFund.cpp:55-58`).
- **SPEC's view.** SPEC verify step 10 takes the earlier of `Expiration` and `CancelAfter` as
  `deadline`, and requires `deadline − validatedLedgerCloseTime ≥ maxTimeoutSeconds +
redemptionSafetySeconds` (`L373-379`). Its settle step 8 redeems at once when `Expiration` is
  present, or when `CancelAfter` is within `minSettleDelay` (`L480-483`).
- **SPEC is silent on reversal.** It never mentions `tfRenew` or `tfClose` (grep).

**Inference:** the exit can be reversed (`Closing` back to `Open`) only by the **payer**. TOON's
own payer half would never do it. A third-party client paying a TOON receiver could.

### C.2 Does any receiver-side code assume the exit is one-way?

Searched: every `BatchChannelStatus::{Closing, Withdrawing, Sealed}` and `accepts_vouchers()` use
outside the backends' own tests. CONFIRMED-CODE.

| Where                                                                                                                     | What it assumes                                                                                                           | Assumes one-way?                                            |
| ------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| `crates/connector-settlement/src/batch/port.rs:23-32`                                                                     | Doc comments: `Withdrawing` "EVM only", `Closing`/`Sealed` "Solana only", `Sealed` "terminal"                             | Documentation only; says nothing about `Closing → Open`     |
| `crates/connector-settlement/src/batch/in_memory.rs:17-27`, `:268-270`, `:682-685`, `:960-962`                            | The fake's `Close` shape: `Closing` only ever goes to `Sealed`                                                            | **Yes, in the fake's `Close` shape**                        |
| `crates/connector-settlement/src/batch/in_memory.rs:950-955`                                                              | The fake's `Withdrawal` shape returns `Withdrawing → Open`                                                                | No: the port **already** carries a reversing exit (EVM)     |
| `crates/connector-cli/src/batch_settlement.rs:173-193`, `:208-209`                                                        | The gate calls `Closing` "**terminal**" (`ChannelTerminal`), but recomputes from a fresh `channel_state` on every voucher | Naming only; nothing is cached                              |
| `crates/connector-client-edge/src/claim_gate.rs:1044-1066`                                                                | Reads the backend's current figure every voucher: "no lower bound to cache"                                               | No                                                          |
| `crates/connector-settlement-evm/src/batch_watch.rs:11-16`                                                                | "No collateral cache to drop: the backend answers every `channel_state` from the chain"                                   | No                                                          |
| `crates/connector-settlement-solana/src/batch/sweep.rs:18-29`                                                             | Rediscovers every channel from the chain each tick; `next_step` is a pure function of the account                         | No                                                          |
| `crates/connector-settlement-solana/src/batch/pay.rs:800-803`, `:209-213`                                                 | The payer sets `backed = 0` on its own close; `backed` moves only on its own actions                                      | Payer-side; on XRPL the payer is the only one who can renew |
| `crates/connector-runtime/src/batch_channels.rs:611-630` (`withdraw`), `:687-707` (`live_toward`)                         | Read `outbound_state` fresh each call                                                                                     | No                                                          |
| Admission: `AdmissionRefusal::NotOpen` (`port.rs:539`), the fake at `in_memory.rs:512`, the sweep doc at `sweep.rs:23-24` | A channel exiting when **presented** is refused                                                                           | Not about reversal                                          |

**Finding:** no receiver-side code caches "exiting" as a one-way fact. The only one-way shape is
the in-memory fake's `PayerExit::Close`. Under decision 2 the fake changes only if a suite clause
needs a reversing `Closing`. No such clause exists today. **Inference.**

- **One behavioural interaction to note.** Solana's receiver watcher seals at once when it sees
  `Closing` with nothing left to land (`sweep.rs:76-78`, `SealEarly`). An XRPL watcher doing the
  same (destination `tfClose` on seeing `Expiration`) would end the channel, making any later
  `tfRenew` moot. It would also refund the payer inside the receiver's transaction (A.3).
  **Inference.**
- **`CancelAfter` at admission.** PENDING §2.2 misfit 3 already proposes refusing a channel whose
  earlier deadline leaves less than the minimum delay, which SPEC step 10 also requires. Today's
  `AdmissionRefusal` has `DelayBelowMinimum { delay_secs, minimum_secs }` (`port.rs:536`) and no
  variant for an absolute deadline. Whether that is a new variant, or `DelayBelowMinimum` with
  `delay_secs = CancelAfter − now`, is open.

---

## D. Amounts, the claim message, keys, and what #2952 specifies

- **Amounts are XRP drops.**
  - Internally they are `XRPAmount` with `value_type = std::int64_t`
    (`include/xrpl/protocol/XRPAmount.h:31-34`).
  - A native amount's maximum is `kMaxNativeN = 100'000'000'000'000'000` drops
    (`include/xrpl/protocol/STAmount.h:72`), equal to `kInitialXrp` = 10¹¹ XRP × 10⁶
    (`include/xrpl/protocol/SystemParameters.h:31-41`).
  - Claim and Fund refuse non-XRP or non-positive amounts with `temBAD_AMOUNT`
    (`PaymentChannelClaim.cpp:48-57`, `PaymentChannelFund.cpp:37-38`).
  - CONFIRMED-CODE.
  - **Inference:** decision 4's narrowing bound for XRPL is 10¹⁷ drops, not `u64::MAX`. A voucher
    between 10¹⁷ and 2⁶⁴−1 fits the signed message but can never be claimed.
- **The claim message is `CLM\0 ‖ channelId ‖ u64 BE`.**
  - `serializePayChanAuthorization` writes `add32(HashPrefix::PaymentChannelClaim)`,
    `addBitString(key)` and `add64(amt.drops())` (`include/xrpl/protocol/PayChan.h:10-16`).
  - The prefix is `makeHashPrefix('C','L','M')` = `'C'<<24 | 'L'<<16 | 'M'<<8`, which is
    `0x434C4D00` (`include/xrpl/protocol/HashPrefix.h:12-16`, `:89`).
  - It binds **no network id**.
  - CONFIRMED-CODE. SPEC §6.2 (`L210-221`) states the same.
- **Signature schemes.** secp256k1 verifies over `sha512Half(message)`; Ed25519 verifies over the
  raw message (`src/libxrpl/protocol/PublicKey.cpp`, `verify`, around `:278-289`). CONFIRMED-CODE.
- **Key types.** `publicKeyType` accepts exactly 33 bytes: `0xED` followed by 32 bytes is Ed25519,
  and `0x02`/`0x03` followed by 32 bytes is compressed secp256k1. Anything else gives `temMALFORMED`
  at create (`PublicKey.cpp:208-221`, `PaymentChannelCreate.cpp:66-67`). CONFIRMED-CODE.
  - **Inference:** a `VoucherSigner::Xrpl([u8; 33])` already carries its type. An inner
    `secp256k1 | Ed25519` enum (decision 5) is a choice of representation, not a necessity.
- **Width of the times.** `SettleDelay`, `Expiration` and `CancelAfter` are UInt32 (PayChannel
  docs). Delays narrow from the port's `u64` seconds the same way Solana's `grace_period` narrows
  to `u32` (`crates/connector-settlement-solana/src/batch/pay.rs:494`).
- **What #2952 specifies** (SPEC at `683fb7a6`), CONFIRMED-DOCS:
  - **Network** `xrpl:<network_id>` from `server_info`; "the facilitator **MUST** confirm that its
    XRPL endpoint reports the requested `network_id`" (§4.1, `L67-79`).
  - **`extra` fields:**
    - `extra.binding = "xrpl-paychan-v1"`;
    - `extra.minSettleDelay`, which must be at least `maxTimeoutSeconds + redemptionSafetySeconds`;
    - `extra.redemptionSafetySeconds`, positive;
    - `destinationTag` and `payoutReference`, both optional.
    - Source: §5, `L152-163`. The example uses `minSettleDelay: 86400` (`L127`).
  - **Voucher** as above (§6.2). **Verify** reads a validated ledger and checks:
    - existence and not expired;
    - `Destination`;
    - the tag;
    - the `PublicKey`;
    - `SettleDelay`;
    - the deadline;
    - `Balance ≤ authorizedAmount ≤ Amount`;
    - the signature.
    - Source: §8, `L362-385`.
  - **Redemption** is `PaymentChannelClaim` from `payTo` with `Amount`/`Balance`/`PublicKey`/
    `Signature` and no flag. It is final only on a validated `tesSUCCESS`, and must be submitted at
    once when `Expiration` appears (§11, `L570-601`).
  - The `invalid_batch_settlement_xrpl_channel` error covers a channel that is "missing, closed,
    expired" (`L796`). These are not told apart.
- **Existing narrowing precedent in this repository** (for decision 4), CONFIRMED-CODE:
  - Solana refuses a voucher amount wider than u64 by name: `land` →
    `VoucherExceedsDeposit` (`mod.rs:848`), `sign_voucher` → `VoucherUnbacked` (`pay.rs:768`).
  - A deposit or top-up that is too wide is refused with a generic
    `Backend(String)` (`pay.rs:489-493`, `:720`), **not** a named variant.
  - A ceiling (a bound, not an amount) **saturates** on purpose (`crates/connector-cli/src/batch_settlement.rs:225-236`).

---

## E. Can a plain read tell "ended" from "never existed"?

- **`ledger_entry`** returns `entryNotFound` ("The requested ledger entry does not exist in the
  ledger") either way (ledger_entry docs).
  - Live check on mainnet, ledger 107960081: `ledger_entry` for a never-existing `payment_channel`
    returns `entryNotFound`, `error_code` 98. It does so both with and without `include_deleted`.
    CONFIRMED-CODE (live RPC).
- **Only history can tell them apart.** The sources (CONFIRMED-DOCS):
  - a deleting transaction's metadata `DeletedNode` (`LedgerEntryType` PayChannel, with
    `FinalFields`);
  - Clio's `ledger_entry` with `include_deleted: true`, which returns the pre-deletion data and a
    `deleted_ledger_index`. It is Clio-only, and the docs are inconsistent about the error name
    when the flag is off.
  - **Inference:** a `ledger_entry` at a ledger index before the deletion, on a server that holds
    that history.
- **Conclusion** (**Inference**): supports decision 3. A receiver reading the current ledger cannot
  tell a closed channel from an unknown one, exactly as on Solana after deallocation. The payer
  half can cache its final state from the `DeletedNode` of the transaction that closed it, but
  only when it sees that transaction. A close by the receiver, or by a third party touching an
  expired channel (A.3), is not the payer's own transaction. The payer then needs an `account_tx`
  or metadata lookup to learn the final `Balance`. Whether that lookup is required, or "gone"
  simply means `NoWithdrawalPending`, is open.

---

## What this means for the grilling

| Decision                       | Status                           | One line                                                                                                                                                                                                                                                                                     |
| ------------------------------ | -------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1. Scope                       | **Supported**                    | Nothing found forces Cardano or Hedera facts into XRPL's settlement.                                                                                                                                                                                                                         |
| 2. Union / intersection        | **Supported, with a caveat**     | The structural clauses hold. But the paying suite's exact `payer_balance` deltas are true only where fees are paid in a different asset, so they are not intersection-safe for XRPL or Mina (A.5).                                                                                           |
| 3. Deleted = not found         | **Supported, with a constraint** | `ledger_entry` cannot tell ended from absent (E). XRPL `land` on `Closing` must not `tfClose`, or the receiving suite's read at `contract.rs:366-369` fails (A.4).                                                                                                                           |
| 4. `u128`, narrow, refuse      | **Supported, with a correction** | XRPL's bound is 10¹⁷ drops (`kMaxNativeN`), not `u64::MAX`. Today's Solana deposit narrowing uses `Backend(String)`, not a named error.                                                                                                                                                      |
| 5. `VoucherSigner` per chain   | **Supported**                    | A 33-byte key is self-typing; an inner enum is optional.                                                                                                                                                                                                                                     |
| 6. No-op clock, receiver seals | **Challenged for XRPL**          | XRPL's seal refunds in the same transaction, before the suite reads `before`. A non-sealing `land` cannot finish without the delay passing. XRPL needs a real `let_delay_pass` over a short fixture `SettleDelay` (real wait, or `ledger_accept` ≥ 1 s per ledger: UNCONFIRMED in practice). |

**Open questions for the next round:**

1. **Fee-denominated balance.** Does the XRPL (and Mina) fixture's `payer_balance` add back fees
   read from the chain, or do the suite's exact deltas become "net of the payer's fees"? A.5 lists
   every affected clause.
2. **Decision 6 for XRPL.** Accept a non-no-op `let_delay_pass` with a short paying-fixture
   `SettleDelay`, keeping the suites unmodified? Or keep decision 6's letter and change the finish
   clause to allow "refund already received"? Should someone run `ledger_accept` in a loop against
   xrpld 3.4.1 to confirm the close-time push (§B)?
3. **Reversible exit.** The port already carries `Withdrawing → Open` (EVM). Is a payer-only
   `Closing → Open` on XRPL just a chain-only test, given no suite clause needs it and no code
   caches one-way status (C.2)? Should the gate's "terminal" wording for `Closing`
   (`batch_settlement.rs:173-178`) change?
4. **`CancelAfter` at admission.** Refuse any channel with `CancelAfter` set, or only one whose
   earlier deadline is inside the minimum delay plus a safety margin (SPEC step 10)? Is that a new
   `AdmissionRefusal` variant, or `DelayBelowMinimum` reused?
5. **Does the fake need a deletion shape?** Today no suite clause reads a finished channel (A.1),
   so under decision 2 it does not. It would need one if a clause is added for "a finished channel
   reports `ChannelNotFound` to the receiver".
6. **Any suite clause about a finished channel.** What `finish_withdrawal` returns when the
   receiver's `tfClose`, or a third party's touch, already ended the channel and refunded the
   payer outside `finish` (A.4, E).
7. **A watcher that seals early.** Should an XRPL receiver watcher seal on seeing `Expiration`, as
   Solana's `SealEarly` does? It refunds the payer at once and makes `tfRenew` moot (C.2).
8. **Landing on an expired channel succeeds and delivers nothing** (`PaymentChannelClaim.cpp:121-126`).
   Should XRPL `land` check the delivered amount as well as `tesSUCCESS`? This is a chain-only test.
