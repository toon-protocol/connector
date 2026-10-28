# An x402 batch-settlement channel on Mina: what it would take

> **Research note, not a decision record.** It decides nothing. The question comes from the owner:
> can connector-to-connector peerings settle in native MINA (and later a Mina token) on a
> batch-settlement channel, the way they settle today on EVM (`x402BatchSettlement`) and Solana
> (`payment-channels`) under [ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)?
> This note gathers the facts the design decisions need. It does not design the zkApp. It builds on
> [`mina-tokens-and-stablecoins.md`](mina-tokens-and-stablecoins.md) (cited as **TOKENS**),
> [`mina-stablecoin-options.md`](mina-stablecoin-options.md) and
> [`zk-note-verifier-spike.md`](zk-note-verifier-spike.md) (cited as **SPIKE**), and does not redo
> their ground. Primary sources only. Written 2026-10-28, and every source was read that day.

**Evidence tags.** CONFIRMED-CODE means read in source at a pinned commit. CONFIRMED-DOCS means
read in the maintainer's own documents, release notes or issues. CONFIRMED-HISTORY means read in
this repository's git history. UNCONFIRMED means none of those, and says why. A sentence marked
**Inference** is this note's reasoning from confirmed facts, not a fact.

**Sources pinned.**

- **x402**: [`x402-foundation/x402@712e20e1`](https://github.com/x402-foundation/x402/tree/712e20e1e036bfd28f40be35281f95f96df575dd)
  (`HEAD`, 2026-10-28). [ADR 0074](../adr/0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md)
  pins the older `0cb1a1f0`. `coinbase/x402` is now a fork of this repository, 700 commits behind.
  Cited as **X402** plus a path.
- **payment-channels**: [`solana-foundation/payment-channels@3ffa4d67`](https://github.com/solana-foundation/payment-channels/tree/3ffa4d6728ad88e4a9667a76ad9ccd68a302c696)
  (`HEAD`, the same commit ADR 0074 pins). Cited as **PC** plus a path under `program/payment_channels/src/`.
- **o1js**: [`o1-labs/o1js@a428f9b9`](https://github.com/o1-labs/o1js/tree/a428f9b97817177ef4b5868cf148c0ac67a1048d)
  (`main`, 2026-10-27; latest release `v3.1.1`, 2026-10-23). Cited as **O1JS**.
- **Mina daemon**: tag [`4.0.3`](https://github.com/MinaProtocol/mina/tree/4.0.3) (`73134bfd`,
  2026-10-23, the current Mesa release). Cited as **MINA**.
- **MIPs**: [`MinaProtocol/MIPs@5aa77c89`](https://github.com/MinaProtocol/MIPs/tree/5aa77c8911e7af39ff9c594175226b171ed8166d).
- **mina-fungible-token**: [`MinaFoundation/mina-fungible-token@8dbf75a4`](https://github.com/MinaFoundation/mina-fungible-token/tree/8dbf75a4a50d236d9a9ab137a7fd451bafddae41)
  (the commit TOKENS pinned). Cited as **MFT**.
- **mina-rust** (the Mina Rust node, formerly openmina): [`o1-labs/mina-rust@82480cd4`](https://github.com/o1-labs/mina-rust/tree/82480cd468)
  (`develop`, 2026-07-15, the last push). Cited as **MR**.
- **mina-multi-sig**: [`o1-labs/mina-multi-sig@e9b5ffc8`](https://github.com/o1-labs/mina-multi-sig/tree/e9b5ffc8118ab871e3b7e4d86f2bc6c7e48375c2)
  (2026-08-03), whose `mina-tx` crate is a Rust zkApp-command signer. Cited as **MMS**.
- **proof-systems**: [`o1-labs/proof-systems@92274448`](https://github.com/o1-labs/proof-systems/tree/9227444890) (`master`, 2026-09-24).
- **zk_toon**: [`toon-protocol/zk_toon@70ebc12f`](https://github.com/toon-protocol/zk_toon/tree/70ebc12f)
  (2026-10-16), and its issues #2, #6–#17 as they stood on 2026-10-28. Cited as **ZKT**.
- **This repository**: `HEAD` `9a039468` for the connector, and `dee8c99a` (the parent of
  `e13bd780`, the commit that deleted Mina) for TOON's own Mina payment-channel zkApp. Cited as
  **OLD** plus a path under `packages/mina-zkapp/src/`.

---

## Verdict

**There is a hard blocker, and it is the one ADR 0002 named, now sharper.** Every on-chain
operation of a Mina channel that changes the channel's state or moves its escrow needs a zkApp
**proof**. No Rust program can make that proof for an o1js-compiled contract today. The operation
that most needs a proof is the receiver's: **landing a voucher**. A voucher is a payer's signature,
and Mina checks a third party's signature only inside a circuit. Under ADR 0074 decision 5, landing
a voucher is the only thing that protects a receiver from a payer's exit. So a connector that
receives on a Mina channel cannot protect what it has been paid unless an o1js prover runs
somewhere on its behalf. That has to happen either inside this repository, which ADR 0002, ADR
0065-mina, ADR 0080's falsifiers and `CLAUDE.md` all forbid, or outside it, as zk_toon already
plans for its vault (#14).

**There is a second blocker in this repository's own records.** ADR 0075 decision 1 says a
connector settles only on **x402's own** deployed code, and that "a network x402 has not deployed
to is unsupported, loudly". x402 has no Mina binding. No Mina implementation exists or is proposed,
and Mina is not mentioned anywhere in x402's code, issues or PRs. A Mina channel would be TOON's own
on-chain code. That is the very cost ADR 0075 ("What two kinds of channel cost") retired TOON's EVM
and Solana channels to stop paying.

**Everything off-chain is easy.** Signing and verifying a Mina voucher (Schnorr over Poseidon) is
native Rust, byte-compatible with o1js (SPIKE). Signing and submitting a signature-authorized zkApp
command from Rust is possible, through an alpha-quality o1-labs crate. Reading account state is
plain GraphQL.

| #   | Question                                                       | Answer                                                                                                                                                                                                 | Tag                  |
| --- | -------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------- |
| 1   | Is there network-neutral x402 batch-settlement text to follow? | **Only a 76-line abstract lifecycle** and seven things a binding must define. Every concrete rule (voucher, claim, withdraw, refund) lives in the EVM and SVM bindings                                 | CONFIRMED-DOCS       |
| 1   | Any non-EVM/SVM implementation, or any Mina mention, in x402?  | **None merged.** Open PRs exist for XRPL, Hedera, Bitcoin and Cardano. **No Mina mention** in code, issues or PRs                                                                                      | CONFIRMED-CODE, DOCS |
| 2   | Can a channel operation be authorized by signature, not proof? | **Only the payer's own debit.** A signature authorizes only its signer's own account update. Any change to channel state or escrow, and any check of a voucher, needs the zkApp's proof                | CONFIRMED-CODE       |
| 2   | On-chain state after Mesa                                      | **32 fields** (was 8). Mesa has been live on mainnet since **2026-09-03**                                                                                                                              | CONFIRMED-CODE, DOCS |
| 2   | Throughput                                                     | **90 s slots**, with mainnet block producers capping zkApp commands at **12 per block** ("temporarily", pending memory fixes)                                                                          | CONFIRMED-CODE       |
| 2   | Confirmation                                                   | **15 blocks recommended (~30 min at 90 s slots)**; `k = 290` for full finality                                                                                                                         | CONFIRMED-DOCS, CODE |
| 3   | Can Rust build, sign and submit a zkApp command without o1js?  | **Signature-authorized: yes**, via MMS `mina-tx` (`1.0.0-alpha.0`). **Proof-authorized: no.** No Rust prover exists for o1js circuits                                                                  | CONFIRMED-CODE       |
| 3   | Is the Mina Rust node maintained?                              | **No. Paused by o1Labs since 2026-07-15.** "not actively developing the project, reviewing pull requests, or accepting external contributions"                                                         | CONFIRMED-DOCS       |
| 4   | Can a test spawn its own Mina chain, like `Anvil::spawn`?      | **Not cheaply.** Lightnet is a ~1 GB Docker image that takes 1–2 min to sync (single node). `LocalBlockchain` is in-process JavaScript                                                                 | CONFIRMED-DOCS, CODE |
| 5   | Native MINA vs a token                                         | **Native MINA needs no third-party approval.** A `FungibleToken` balance needs that token contract's **proof** in every transfer, and its admin can pause every transfer, channel withdrawals included | CONFIRMED-CODE       |

---

## 1. What the connector asks of a settlement chain today

A Mina channel would have to implement this port. These are the facts it would have to match.

### 1.1 The port and its suites

- `BatchSettlementBackend` (receiving half) and `BatchSettlementPayer` (paying half) in
  [`crates/connector-settlement/src/batch/port.rs`](../../crates/connector-settlement/src/batch/port.rs)
  are the only settlement port. TOON's two-sided `SettlementBackend` was deleted by ADR 0075 (#1385).
  ADR 0002's sentence "`SettlementBackend` is nonetheless designed so an out-of-process
  implementation is possible … it arrives as a sidecar crate" was written about that deleted trait.
  CONFIRMED-CODE.
- **Receiving half**: `admit(presentation)`, `restore(presentation)`, `channel_state(id)` (always
  read from the chain, never cached, because EVM collateral can fall) and `land(id, voucher)`.
  `land` works on every status but `Sealed`, refuses a stale voucher by name (`StaleVoucher`) and one
  above the deposit (`VoucherExceedsDeposit`), and sends exactly one transaction. CONFIRMED-CODE.
- **Paying half**: `prepare_open(terms, deposit)` (builds the channel and sends nothing, so the
  caller can journal it, ADR 0075 decision 8), `open_prepared(record)` (idempotent: a record opens
  exactly one channel with exactly one deposit, however many times it is called, even after a
  crash), `restore_outbound`, `top_up(increment)`, `sign_voucher(cumulative)`,
  `start_withdrawal`, and finishing the withdrawal. CONFIRMED-CODE.
- `OutboundChannelRecord` is an enum with one variant per chain, versioned and encoded opaquely for
  the journal. `ChannelPresentation`, `ReceiverTerms` and `VoucherSigner` are likewise per-chain
  enums (`Evm`, `Solana`; `VoucherSigner::Ed25519`). A third chain adds a variant to each.
  CONFIRMED-CODE.
- `BatchChannelStatus` is the union of both chains' lifecycles: `Open`, `Withdrawing` (EVM only),
  `Closing` and `Sealed` (Solana only). The contract suites
  ([`contract.rs`](../../crates/connector-settlement/src/batch/contract.rs),
  `assert_upholds_the_contract` and `assert_upholds_the_paying_contract`) assert "only what holds on
  **both** chains", parameterised by a `PayerExit` of withdrawal or close. ADR 0007 and ADR 0075
  decision 2 require every implementation to pass them unmodified. CONFIRMED-CODE.
- Amounts are `u128` in the port. A Solana voucher is `u64`. MINA amounts are `uint64` nanomina
  (TOKENS §1). CONFIRMED-CODE.

### 1.2 What surrounds the port

- **Watchers and sweeps** are callers of the port, not part of it. The EVM watcher reacts to
  `WithdrawInitiated` logs by claiming at once. Its sweep claims many channels in one `claim`, then
  `settle(receiver, token)`
  ([`batch_watch.rs`](../../crates/connector-settlement-evm/src/batch_watch.rs)). The Solana sweep
  rediscovers every sponsored channel each tick with `getProgramAccounts`, then settles, runs
  `settle_and_seal` on Closing, `distribute` on Sealed and `reclaim`
  ([`batch/sweep.rs`](../../crates/connector-settlement-solana/src/batch/sweep.rs)). Both rely on
  **finding** a payer's exit on chain promptly. CONFIRMED-CODE.
- **Confirmation** (ADR 0073 decision 5). EVM: a transaction no poll has observed for 30 s, or one
  observed but not mined within **180 s**, is reported as not observed, and the report says a retry
  is not known to be safe ([`send.rs`](../../crates/connector-settlement-evm/src/send.rs)). Solana:
  blockhash expiry (~60–90 s) makes the wait finite, `give_up_after` is two minutes
  ([`submit.rs`](../../crates/connector-settlement-solana/src/submit.rs)). Every settlement client
  is built from its table's one `RpcTransport`, which may ride the SOCKS circuit. CONFIRMED-CODE.
- **Keys** (ADR 0075 decision 3). Each chain's settlement key pays, signs every transaction and
  signs every voucher. `[signer]` (secp256k1) never gains spending authority. A Mina settlement key
  would be a Pallas key, a third key type. CONFIRMED-DOCS.
- **Boot** refuses a chain where the x402 contract or program is absent, and both addresses are
  constants of the binary (ADR 0075 decision 1). CONFIRMED-DOCS.
- **Delay policy.** A receiver publishes a minimum `withdrawDelay` / `grace_period`, defaulting to
  **one day**, never lower than x402's floor (EVM 15 min, SVM 900 s). The day is "the window in which
  a censored or delayed `claim` must still land" (ADR 0074 §5). CONFIRMED-DOCS.
- **Tier-3 tests** spawn their own chain per test: `Anvil::spawn` forks `anvil` on its own port.
  `SolanaValidator::spawn` forks `solana-test-validator` with `payment-channels` and p-token loaded
  into genesis from pinned fixtures. `require_anvil()` / `require_solana_test_validator()` panic
  when `CI` is set and the binary is missing
  ([`test_support.rs`](../../crates/connector-settlement-evm/src/test_support.rs),
  [`solana test_support.rs`](../../crates/connector-settlement-solana/src/test_support.rs)).
  CONFIRMED-CODE.

---

## 2. The x402 batch-settlement scheme, chain-neutrally

### 2.1 The network-neutral spec is a frame, not a protocol

- X402 `specs/schemes/batch-settlement/` holds four files at `712e20e1`:
  - [`scheme_batch_settlement.md`](https://github.com/x402-foundation/x402/blob/712e20e1e036bfd28f40be35281f95f96df575dd/specs/schemes/batch-settlement/scheme_batch_settlement.md): network-neutral, 76 lines, last changed in `c763d05a` (#2051, 2026-05-05);
  - `_evm.md`: v1.1, 866 lines, last changed in #3725 (2026-10-26);
  - `_svm.md`: 2096 lines, last changed in #3474 (2026-10-26);
  - `_cloudflare.md`: credit-backed, with no chain.

  CONFIRMED-DOCS.

- The neutral file says how a commitment "is formed, what backs it, and how it is eventually
  redeemed, is defined entirely by the network binding" (L9). It gives:
  - a lifecycle of **Commit → Accumulate → Redeem** (L49–53);
  - two trust models, capital-backed (escrow or a channel) and credit-backed (L27–33);
  - seven things every binding MUST specify (L59–67): commitment format; verification rules
    (signature scheme, balance check, replay prevention, expiry); storage and the commitment
    identifier; double-spend prevention; expiry; redemption (who, when, which rail); and the trust
    model.

  **It defines no voucher struct, no cumulative rule and no withdrawal delay.** CONFIRMED-DOCS.

- Adding a chain: X402 `specs/CONTRIBUTING.md` says "Before writing a spec, open a GitHub issue or
  discussion". A binding is `scheme_<name>_<chain>.md` written from `scheme_impl_template.md`
  (Summary, Payment payload, Verification, Settlement, Appendix). It must have security sections on
  replay prevention, authorization scope and settlement atomicity. CONFIRMED-DOCS.
- **The de facto neutral vocabulary is the EVM one.** The SVM spec §4.2 maps x402's names onto the
  program's fields:
  - `channelConfig`: `payer`, `payerAuthorizer`, `receiver`, `receiverAuthorizer`, `token`,
    `withdrawDelay`, `salt`;
  - `voucher`: `channelId`, `maxClaimableAmount`;
  - `channelState`: `balance`, `totalClaimed`, `withdrawRequestedAt`, `chargedCumulativeAmount`.

  The open Cardano PR (#3580) states its approach as "payloads and the server and facilitator roles
  follow the EVM binding." CONFIRMED-DOCS.

### 2.2 The semantics both bindings share

These are what a Mina binding would have to preserve. EVM line numbers are
[`contracts/evm/src/x402BatchSettlement.sol`](https://github.com/x402-foundation/x402/blob/712e20e1e036bfd28f40be35281f95f96df575dd/contracts/evm/src/x402BatchSettlement.sol).
CONFIRMED-CODE unless marked.

| Semantic           | EVM `x402BatchSettlement`                                                                                                                                                                                                                      | Solana `payment-channels`                                                                                                                                                                         |
| ------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Channel identity   | `channelId` = EIP-712 hash of `ChannelConfig{payer, payerAuthorizer, receiver, receiverAuthorizer, token, uint40 withdrawDelay, bytes32 salt}`, bound to chainId and contract (L33–41, L442–446). The config is **not stored**; only the id is | A PDA over `["channel", payer, payee, mint, authorized_signer, salt, open_slot]`, a 256-byte account (`state/channel.rs`). `open_slot` as a seed stops a voucher replaying on a later incarnation |
| Per-channel state  | `{uint128 balance, uint128 totalClaimed}`, `refundNonce`, `pendingWithdrawal{amount, initiatedAt}` (L109–118). Per receiver and token: `{totalClaimed, totalSettled}`                                                                          | `status`, `deposit`, `settled`, `payout_watermark`, `closure_started_at`, `grace_period`, `distribution_hash`, six addresses, `open_slot`                                                         |
| Voucher            | EIP-712 `Voucher(bytes32 channelId, uint128 maxClaimableAmount)`: **cumulative, no nonce, no expiry**. "The cumulative model makes nonces unnecessary" (spec L834)                                                                             | 50 bytes: `0x56 0x01 ‖ channel_id ‖ u64 cumulative ‖ i64 expires_at`, Ed25519 through the precompile. x402 requires `expires_at = 0`                                                              |
| Voucher signer     | `payerAuthorizer` by ECDSA when nonzero, otherwise `payer` through `SignatureChecker`                                                                                                                                                          | `authorized_signer`, fixed at open                                                                                                                                                                |
| Deposit / top-up   | `deposit(config, amount, collector, data)`, **callable by anyone**: authorization is the payer's ERC-3009 or Permit2 signature inside the collector. The first deposit creates the channel                                                     | `open(...)` signed by payer and `rent_payer`; `topUp` by payer; `deposit > 0`                                                                                                                     |
| Land a voucher     | `claim(VoucherClaim[])` by `receiver`/`receiverAuthorizer`, or `claimWithSignature` by anyone with the `receiverAuthorizer`'s `ClaimBatch` signature. It **records** only (no transfer); a replay at or below `totalClaimed` is a silent no-op | `settle`, by **anyone** (the voucher is the authority), Open only. `settleAndSeal` by `payee`, from Open or Closing                                                                               |
| Pay out            | `settle(receiver, token)` by anyone: one transfer of `totalClaimed − totalSettled` across every channel for that receiver                                                                                                                      | `distribute` by anyone: bps split of `settled − payout_watermark` (≤ 32 recipients); on Sealed it also refunds `deposit − settled` to the payer                                                   |
| Payer's exit       | `initiateWithdraw(config, amount)` then `finalizeWithdraw` after `withdrawDelay`, by `payer`/`payerAuthorizer`. The channel survives                                                                                                           | `requestClose` → Closing; `seal` by anyone after `grace_period`; `withdrawPayer`; `reclaim` rent after 1500 slots                                                                                 |
| Cooperative refund | `refund` by receiver, or `refundWithSignature` by anyone with a `Refund(channelId, nonce, amount)` signature                                                                                                                                   | none in the program; x402's `CloseAuthorization` (SHA-256 over a domain-separated string, spec L452)                                                                                              |
| Delay bounds       | 15 min ≤ `withdrawDelay` ≤ 30 days, checked at deposit                                                                                                                                                                                         | Program: `grace_period ≥ 1`. x402: 900 s to 2,592,000 s                                                                                                                                           |
| Known liveness gap | No reservation for signed-but-unclaimed vouchers: a `finalizeWithdraw` that lands first makes the receiver's later `claim` revert (implementer notes, CONFIRMED-DOCS)                                                                          | After `requestClose`, only the `payee` can land a voucher (ADR 0074 §5)                                                                                                                           |
| Upgradeability     | Ownerless and immutable; CREATE2 at one address on every chain                                                                                                                                                                                 | Upgradeable by solana-foundation's key                                                                                                                                                            |

**Inference.** The chain-neutral core a Mina binding would keep is:

- a channel bound to (payer, voucher signer, receiver, token, delay, salt);
- one cumulative voucher amount with a strict "greater than landed" rule and a ceiling at the deposit;
- landing that records, separate from (or batched with) paying out;
- a payer's exit only after a delay long enough for the receiver to land its latest voucher.

Everything else is binding detail.

### 2.3 Other chains, and Mina

- In-tree implementations are Go `evm/` and `svm/` and Python `evm/` only. **No other chain's
  batch-settlement has been merged.** Open: PR #2952 (XRPL native payment channels), PR #3466
  (Hedera, a forked contract routing signature checks through a system contract), PR #3544 (Bitcoin
  2-of-3 PSBT), PR #3580 (Cardano Subbit channels), PR #2612 (credit-backed). Issues: #3341
  (Stellar), #3645 (Kaspa). CONFIRMED-DOCS.
- Stellar's issue is instructive: a Soroban `require_auth` signature expires within 180 days, so it
  does not fit a voucher that never expires. **Inference.** This is the kind of chain-specific
  mismatch a Mina binding would need its own section for, such as how Mina's time preconditions
  bound a voucher (§3.2). CONFIRMED-DOCS for Stellar.
- **Mina.** A code search for "mina" in x402-foundation/x402 and coinbase/x402 matches only
  substrings ("terminal", "denominations"). Issue and PR searches return nothing. There is no `mina`,
  `o1js` or `zkapp` path. CONFIRMED-CODE. The CAIP-2 ids `mina:mainnet` and `mina:devnet` exist in
  ChainAgnostic's `mina` namespace (TOKENS §3).

---

## 3. Mina and o1js constraints on such a zkApp

### 3.1 Authorization: what a signature can and cannot do

- Permission fields: `editState`, `send`, `receive`, `setDelegate`, `setPermissions`,
  `setVerificationKey`, `setZkappUri`, `editActionState`, `setTokenSymbol`, `incrementNonce`,
  `setVotingFor`, `setTiming`, `access`. Auth kinds: `none`, `impossible`, `proof`, `signature`,
  `proofOrSignature`
  ([O1JS `account-update.ts#L290-L470`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/account-update.ts#L290-L470)).
  CONFIRMED-CODE.
- `Permissions.default()` (a zkApp): `editState: proof`, `send: proof`, `receive: none`,
  `editActionState: proof`, `access: none`, and everything else `signature`. The JSDoc says `send`
  defaults to `signature`; the code says `proof`, and the code is what runs. `Permissions.initial()`
  (an ordinary account) is `signature` everywhere except `receive` and `access`, which are `none`.
  CONFIRMED-CODE.
- **A signature authorizes only its own signer's account update.** The protocol checks each signed
  update against that update's own `body.public_key`
  ([MINA `verifier/common.ml#L100-L130`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/verifier/common.ml#L100-L130)),
  and each update carries exactly one of `Signature`, `Proof` or `None_given`
  ([`mina_transaction_logic.ml#L1379-L1391`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/mina_transaction_logic.ml#L1379-L1391)).
  CONFIRMED-CODE.
- Consequences. The first two are **Inference** from the code above:
  - If the channel zkApp set `editState` or `send` to `signature` or `proofOrSignature`, whoever
    holds **the zkApp account's own private key** could rewrite channel state or empty the escrow
    with no proof. That key holder is a custodian, which is not x402's trust model ("ownerless" on
    EVM; "the sponsor controls _when_ a channel closes, never _where the money goes_" on Solana, ADR
    0074 §5).
  - **A payer's or receiver's signature cannot authorize a change to the channel zkApp.** The
    only way a voucher (a payer's Schnorr signature) can gate a state change is
    `Signature.verify` **inside the zkApp method**, which makes it part of the proof.
  - What a signature **can** do: the payer's own debit into the channel (its account is
    `send: signature`, and a zkApp's `receive` is `none`), and a plain signed payment. That part is
    CONFIRMED-CODE.
- Preconditions (account, network and `validWhile`) are checked on **every** account update whatever
  its authorization. A signed non-fee-payer update must also pass a replay check: it must
  either increment and constrain its nonce or use the full commitment, or it fails
  `Zkapp_command_replay_check_failed`
  ([`zkapp_command_logic.ml#L1318-L1390`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/zkapp_command_logic.ml#L1318-L1390)).
  CONFIRMED-CODE.
- Mina accounts have **no hashlock or timelock to a different key**. The only time-based account
  feature is vesting `timing`. A "2-of-2 channel account with a unilateral timeout" therefore cannot
  be built from account permissions alone. MMS's FROST group keys could make a cooperative-only
  2-of-2 signer, but it has no unilateral exit. UNCONFIRMED as a negative: no source was found that
  states it, and the permission list above is the evidence.

### 3.2 Time: the precondition pitfall the old zkApp hit

- `validWhile` (o1js `this.currentSlot.requireBetween`) is checked against the slot of the block that
  **includes** the transaction. `network.*` preconditions (`globalSlotSinceGenesis`, `timestamp`) are
  checked against the **parent** block's protocol state: the producer passes
  `~current_state_view:previous_state_view`
  ([MINA `block_producer.ml#L202-L241`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/block_producer/block_producer.ml#L202-L241);
  [`mina_transaction_logic.ml#L1583-L1590`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/mina_transaction_logic.ml#L1583-L1590)).
  CONFIRMED-CODE.
- o1js 0.9.2's changelog: "`currentSlot` refers to the slot at which the transaction _will be
  included in a block_ … the only supported method is `currentSlot.assertBetween()` because
  `currentSlot.get()` is impossible to implement … and `currentSlot.assertEquals()` is error-prone"
  ([O1JS `CHANGELOG.md`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/CHANGELOG.md)).
  `CurrentSlot` now exposes only `requireBetween`. `network.globalSlotSinceGenesis` still offers
  `requireEquals`, which holds only if the parent's slot equals the value. No current docs page
  states the pitfall. CONFIRMED-CODE (changelog and `precondition.ts#L229-L242`).
- `network.timestamp` is computed as `genesisTimestamp + slot × slotTime`. Its `requireBetween`
  becomes a slot range (`precondition.ts#L156-L191`). Open o1js issues bear on it:
  - #1588 (open): timestamp wrong on devnet, with genesis baked into constraints;
  - #2043 (open): LocalBlockchain's `currentSlot()` and its network state disagree;
  - #2725 (closed 2026-01): current slot not readable from remote networks; `fetchCurrentSlot` was
    added in 2.13.0.

    3.1.1 makes a conflicting second range precondition throw instead of silently replacing the first
    (PR #2918). CONFIRMED-CODE, DOCS.

- `Mina.LocalBlockchain` hard-codes `slotTime = 3 * 60 * 1000`, the **pre-Mesa** 180 s
  ([`local-blockchain.ts#L70-L110`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/local-blockchain.ts#L70-L110)).
  CONFIRMED-CODE.

### 3.3 Concurrency and where channel state lives

- **State contention.** A `getAndRequireEquals` state precondition is checked at application. Once
  one transaction changes the field, every other transaction proved against the old value fails
  `Account_precondition`. **Inference:** one successful writer per state value, in order, so N
  channels in one zkApp's fields serialise all N channels' operations. The mechanics are
  CONFIRMED-CODE; no docs page phrases it this way.
- **Reducer.** "The reducer API in o1js is currently not safe to use in production applications. The
  `reduce()` method breaks if more than the hard-coded number (default: 32) of actions are pending"
  (O1JS `reducer.ts#L65-L67`). CONFIRMED-CODE.
- **BatchReducer** is still `Experimental.BatchReducer` (since o1js 1.5.0: "Handles arbitrary numbers
  of pending actions thanks to recursive validation"). It warns "Don't call `processBatch()` on two
  _different_ batches within the same method". zk_toon chose it for vault deposits: "Two payers
  acting in the same block therefore never conflict on a root precondition", and "A deposit becomes
  a spendable note only once it has been reduced" (ZKT #2). The reduce call costs fees and needs a
  proof, and "someone has to make them. The zk edge operator is the natural caller" (ZKT #2, Risks).
  CONFIRMED-CODE, DOCS.
- **OffchainState** is `Experimental`. "State is only available after it was settled via
  `settle()`", and settlement needs a proof from `createSettlementProof()`
  ([docs](https://docs.minaprotocol.com/zkapps/writing-a-zkapp/feature-overview/offchain-storage)).
  CONFIRMED-DOCS.
- **IndexedMerkleMap** left `Experimental` in o1js 2.7.0 and is public API. Its root now commits to
  the tree length, which fixed a vulnerability. CONFIRMED-CODE.
- **One account per channel** is possible in two shapes:
  - a separate zkApp account per channel, as TOON's old zkApp did (§5);
  - a zkApp deployed at (address, someone's token id). MFT's `TokenEscrow` example is
    `new TokenEscrow(addr, token.deriveTokenId())` with `token.approveAccountUpdate(escrow.self)`
    ([MFT `examples/escrow.eg.ts`](https://github.com/MinaFoundation/mina-fungible-token/blob/8dbf75a4a50d236d9a9ab137a7fd451bafddae41/examples/escrow.eg.ts)).

  Either way **every new account costs `ACCOUNT_CREATION_FEE` = 1 MINA** (O1JS `constants.ts`).
  `TokenContract.MAX_ACCOUNT_UPDATES = 9` children per `approveBase` (`token-contract.ts#L29`).
  CONFIRMED-CODE. Whether the creation fee is ever reclaimable (Solana's `reclaim` returns rent):
  Mina has no account-deletion operation that this note found. UNCONFIRMED.

- **Reading state back.** Account fields are a plain GraphQL read. Actions and events are not kept
  by the daemon's GraphQL. TOON's old deployment guide said "An archive node is required for event
  retrieval", and lightnet exposes an Archive-Node-API on 8282 (OLD `docs/mina-deployment.md`,
  CONFIRMED-HISTORY; §3.6). **Inference:** a watcher equivalent to EVM's `WithdrawInitiated` log
  scan, or a channel kept as actions or offchain state, needs an archive endpoint as well as a
  daemon endpoint.

### 3.4 Limits after Mesa

Mesa activated on mainnet on **2026-09-03**
([`4.0.0-mainnet-mesa`](https://github.com/MinaProtocol/mina/releases/tag/4.0.0-mainnet-mesa));
the current release is `4.0.3`. CONFIRMED-DOCS.

| What                                  | Before Mesa                          | After Mesa                                                                                                                                             | Source                                                                                                                                                                                 |
| ------------------------------------- | ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| zkApp state fields (MIP7)             | 8                                    | **32**                                                                                                                                                 | o1js 3.0.0 `MAX_ZKAPP_STATE_FIELDS`                                                                                                                                                    |
| Action / event elements per tx (MIP8) | 100 each, 16 per event               | **1024 each**, no per-event cap                                                                                                                        | [MINA `node_config_unconfigurable_constants.ml`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/node_config/unconfigurable_constants/node_config_unconfigurable_constants.ml) |
| Account updates per tx (MIP9)         | cost formula, ~5 proof or ~10 signed | `np + n2 + n1 ≤ 16`: up to 15 proof updates + fee payer                                                                                                | [MIP-9](https://github.com/MinaProtocol/MIPs/blob/5aa77c8911e7af39ff9c594175226b171ed8166d/MIPS/mip-0009-increase-zkapp-account-update-limit.md)                                       |
| Slot time (MIP6)                      | 180 s                                | **90 s**                                                                                                                                               | MINA `profiled/mainnet.ml` `block_window_duration = 90000`                                                                                                                             |
| zkApp commands per block              | 24 (default)                         | **12** on mainnet, "temporarily … until the final memory limit fixes" (commit `be09ffac`, 2026-02-20); hard cap 128; a producer setting, not consensus | MINA `profiled/mainnet.ml` `zkapp_cmd_limit = Some 12`                                                                                                                                 |

All CONFIRMED-CODE. **Inference:** at 12 zkApp commands per 90 s block, the whole network carries
about 8 zkApp commands a minute, shared with every other zkApp. Each channel operation that needs a
proof competes for that.

- o1js 3.0.0 (2026-08-18) targets Mesa, and "All verification keys will change". 3.1.0 fixed two
  account-update vulnerabilities and requires recompiling and redeploying. Unreleased `main` changes
  VKs again for nested-call `callee.self`. CONFIRMED-CODE (changelog).
- SPIKE found the same pinning on the verifier side: "The build that verifies pre-Mesa proofs
  refuses Mesa proofs, and the Mesa build refuses pre-Mesa ones." CONFIRMED (SPIKE, RAN).
- The permissions `impossibleDuringCurrentVersion` and `proofDuringCurrentVersion` **fall back to
  `signature` after a protocol version change** (O1JS `account-update.ts`). CONFIRMED-CODE.
  **Inference:** a zkApp that must survive a VK-changing hard fork needs someone holding its account
  key to re-set the VK. This is an upgrade authority, like solana-foundation's over
  `payment-channels`, and unlike x402's ownerless EVM contract. How existing mainnet zkApps were
  carried through Mesa's VK change was not established. UNCONFIRMED.

### 3.5 Fees and finality

- The fee payer must sign (`Fee_payer_must_be_signed`) and increment its nonce
  ([`zkapp_command_logic.ml#L1359-L1367`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/zkapp_command_logic.ml#L1359-L1367)).
  Mainnet profile: `minimum_user_command_fee = 0.001`, `default_transaction_fee = 0.25`,
  `account_creation_fee = 1.0` MINA. Any account can be fee payer; the fee payer need not be a party
  to the channel. CONFIRMED-CODE.
- "The recommended wait time for a transaction to be confirmed is 15 blocks"
  ([docs](https://docs.minaprotocol.com/mina-protocol/lifecycle-of-a-payment)). At 90 s slots:

  | Blocks | Time   | Confidence |
  | ------ | ------ | ---------- |
  | 8      | 16 min | 98.67%     |
  | 15     | 30 min | 99.9231%   |
  | 23     | 46 min | 99.9965%   |
  | 30     | 60 min | 99.9998%   |

  Pre-Mesa pages still say 15 blocks is about 60 minutes. Full finality is `k = 290`. CONFIRMED-DOCS,
  CODE.

- **Inference against §1.2.** ADR 0073's EVM bound ("observed but not mined within 180 s") is two
  Mesa slots. A Mina backend's confirm loop, and the meaning of "confirmed" in `land`'s return, would
  need their own numbers. A one-day `withdrawDelay` is ~960 slots, which leaves ample room for a
  receiver's landing. A 15-minute one is ten slots, less than the 15-block confirmation
  recommendation.

### 3.6 Native MINA vs a custom token

- **Native MINA** needs no token-owner approval. The channel zkApp's own `send`/`editState`
  permissions govern it. CONFIRMED-CODE.
- **A custom token**: every account update under a token id needs the token owner's approval,
  either as parent or through `approveBase` (TOKENS §2).
  - `FungibleToken` sets `access: Permissions.proof()`
    ([MFT `FungibleToken.ts#L76-L83`](https://github.com/MinaFoundation/mina-fungible-token/blob/8dbf75a4a50d236d9a9ab137a7fd451bafddae41/FungibleToken.ts#L76-L83)),
    so **every** transfer into or out of a channel's token account needs a `FungibleToken` proof in
    the same transaction, beside the channel's own proof.
  - `transfer` and `approveBase` both read `paused.getAndRequireEquals()`, a state precondition on
    the token contract in every transfer.
  - The pluggable admin (default: an admin signature) can **pause every transfer**, channel
    withdrawals included.

  CONFIRMED-CODE.

- MFT was last pushed 2025-01-03. Its latest release is v1.1.0 (2024-12-13), with
  `peerDependencies: "o1js": "^2.1.0"`. **No Mesa / o1js 3.x release was found.** CONFIRMED-CODE.
- TOON's old zkApp hit this directly. A channel proof cannot move a custom token
  (`Token_owner_not_caller`). The first fix made the channel "accounting-only" and had the SDK build
  the transfers, which "did NOT bind the token payout to the channel's committed balances". The
  second fix, `UsdcChannelToken`, subclassed `FungibleToken` so that **the token itself** enforced
  the channel's rules (OLD `usdc-channel-token.ts` header, `PaymentChannel.ts` `deposit`/`settle`
  comments). **Inference:** binding payouts to channel state for a third party's token needs either
  that token's cooperation or a token whose owner is the channel. CONFIRMED-HISTORY.
- No stablecoin is live on Mina mainnet (TOKENS §4), so "later a Mina token" names an asset that
  does not exist yet.

---

## 4. Submitting Mina transactions from Rust

### 4.1 What Rust can do today

- **Payments: yes.** mina-rust's `wallet send` builds a `SignedCommandPayload`, signs it with
  `mina_signer` (`create_legacy(network_id)`, `sign(&kp, &payload, true)`) and posts it to GraphQL.
  The source says: "Use packed=true for OCaml/TypeScript compatibility (required by Mina protocol)"
  ([MR `crates/cli/src/commands/wallet/send.rs#L274-L295`](https://github.com/o1-labs/mina-rust/blob/82480cd468/crates/cli/src/commands/wallet/send.rs#L274-L295)).
  CONFIRMED-CODE.
- **Signature-authorized zkApp commands: yes, through an alpha crate.** MMS `mina-tx`:
  - computes the call-forest commitment and the full commitment
    ([`commit.rs#L30-L49`](https://github.com/o1-labs/mina-multi-sig/blob/e9b5ffc8118ab871e3b7e4d86f2bc6c7e48375c2/mina-tx/src/transactions/zkapp_tx/commit.rs#L30-L49));
  - injects signatures into the fee payer and into full-commitment account updates (it skips
    `use_full_commitment = false` updates with a `PartialCommitmentSkipped` warning);
  - builds the `sendZkapp` mutation (`zkapp_graphql.rs`);
  - ships pre-Mesa and Mesa test vectors behind a `mesa-hardfork` feature.

  It is `1.0.0-alpha.0`, only partly audited, and shaped around FROST group keys. Its documented
  workflow has **o1js produce the unsigned transaction JSON** (`proofsEnabled: false`,
  `Permissions.signature()`); Rust then signs and submits (`O1JS-WORKFLOW.md`). It sets
  `proof: None`. CONFIRMED-CODE.

- mina-rust itself has `ZkAppCommand::commitment()`, but its zkApp builder returns
  `Signature::dummy()` ("TODO: Really sign the zkapp") and `prover: Option<()> // TODO: We don't
support that yet`
  ([MR `zkapp_command_builder.rs#L11-L43`](https://github.com/o1-labs/mina-rust/blob/82480cd468/crates/ledger/src/generators/zkapp_command_builder.rs#L11-L43)).
  Upstream work on Rust zkApp signing is open and stalled: MR #1748, #1759 (open since 2025-12-08,
  "to verify that we are actually signing zkApp correctly in the Rust codebase"), #1665 and #1409.
  proof-systems' `mina-tx-type` has no zkApp types; PR #3481 adding them has been open since
  2026-02-17. CONFIRMED-CODE, DOCS.
- **Vouchers: yes.** Poseidon, hash-to-group and Schnorr match o1js byte for byte in Rust, with two
  conventions: domain prefixes padded to 20 bytes, and the packed/`NonceMode::Chunked` nonce. o1js
  `Signature.verify` and mina-signer `signFields` **always use the devnet domain**
  (`CodaSignature*******`), even for mainnet (SPIKE §2). zk_toon adopted that: "Mina Schnorr under
  the devnet network ID with the o1js nonce, even on mainnet" (ZKT #2). CONFIRMED (SPIKE, RAN).
- **Verifying an o1js proof: yes**, ~0.1 s, through a patched mina-rust verifier pinned to one hard
  fork (SPIKE). It is not needed for a channel, because the chain verifies the zkApp's proof.

### 4.2 What Rust cannot do

- **No Rust prover for o1js circuits exists.** mina-rust's Pickles/kimchi prover covers Mina's own
  protocol circuits (block, transaction, merge, and the zkApp segments of the transaction SNARK). It
  loads "pre-compiled circuit data generated by the OCaml implementation" from
  `o1-labs/circuit-blobs`. `generate_zkapp_proof` produces snark work over a zkApp command; it
  verifies the user's side-loaded proof and does not create one (MR `provers.rs`,
  `circuit_blobs.rs`, `proofs/zkapp.rs#L1699`). CONFIRMED-CODE. **Inference:** proving an o1js
  `SmartContract` method in Rust would need that circuit's constraint system and a witness
  generator. o1js computes witnesses in JavaScript, so none of this machinery does it. No
  third-party Rust prover for o1js circuits was found. UNCONFIRMED as a negative.
- ADR 0002 already rejected "reimplementing the payment-channel circuit against Rust kimchi/pickles"
  because "the verification key would have to stay bit-identical to the deployed zkApp, and a
  mismatch is undetectable until a transaction is rejected on-chain". CONFIRMED-DOCS.
- **mina-rust is paused.** README, 2026-07-15: "Development of the Mina Rust Node is currently
  paused as o1Labs focuses engineering efforts on other priorities… o1Labs is not actively developing
  the project, reviewing pull requests, or accepting external contributions, and there is currently
  no committed timeline for future releases or mainnet deployment… Developers are welcome to fork
  the repository." Its last release `v0.19.0` predates Mesa; the GitHub API shows it published
  2026-01-20 (SPIKE says 2026-01-15, probably the tag date). No o1Labs blog or forum post announcing
  the pause was found. CONFIRMED-DOCS (README commit only).

### 4.3 Operation by operation: where o1js is needed

**Inference** from §3.1 and §4.1–4.2, for any design in which the channel's escrow and state are
held by a zkApp with `editState`/`send` at `proof` (anything weaker makes the zkApp's key a
custodian). "Rust" means this connector, unchanged in kind.

| Operation (port method)                          | On-chain effect on Mina                                                             | Authorization needed                                                                          | o1js prover needed? |
| ------------------------------------------------ | ----------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------- | ------------------- |
| `sign_voucher` (payer)                           | none: Schnorr over Poseidon fields                                                  | the payer's key, off-chain                                                                    | **No.** Rust        |
| verify a voucher before accepting it (receiver)  | none                                                                                | —                                                                                             | **No.** Rust        |
| `channel_state`, `admit`, `restore`              | read account state over GraphQL (plus the archive, if the channel lives in actions) | —                                                                                             | **No.** Rust        |
| `prepare_open` / `open_prepared` (payer)         | payer's debit, plus the zkApp recording the channel (state, action or new account)  | payer's signature on its debit; **zkApp proof** for the record                                | **Yes**             |
| `top_up` (payer)                                 | payer's debit, plus the zkApp raising the deposit                                   | payer's signature; **zkApp proof**                                                            | **Yes**             |
| `land` (receiver), the protection of ADR 0074 §5 | zkApp verifies the payer's signature in-circuit and raises `landed`                 | **zkApp proof** (anyone can submit it: the voucher is the authority, as in Solana's `settle`) | **Yes**             |
| pay out (EVM `settle`, Solana `distribute`)      | zkApp `send` from escrow to receiver (and refund to payer)                          | **zkApp proof**, with a time precondition for a refund                                        | **Yes**             |
| `start_withdrawal` (payer)                       | zkApp records the request and its time                                              | **zkApp proof** (with the payer's signature checked in-circuit or as a signed child update)   | **Yes**             |
| finish withdrawal (payer)                        | zkApp `send` of the unlanded remainder to the payer after the delay                 | **zkApp proof** plus `currentSlot`/timestamp range precondition                               | **Yes**             |
| reduce pending actions (if actions are used)     | zkApp folds actions into its roots                                                  | **zkApp proof**, and fees, paid by someone                                                    | **Yes**             |
| any transfer of a `FungibleToken`                | token account update                                                                | **the token contract's proof** too                                                            | **Yes**             |

**Every row that writes needs o1js.** The only operations Rust covers are the off-chain ones and the
reads. CONFIRMED in its parts. The table itself is Inference.

### 4.4 The pattern zk_toon already chose

- ZKT #2 (spec, open, ready-for-agent): "Submitting needs o1js. A Mina zkApp call needs a
  transaction proof that only o1js can make. So the port's real adapter reads through Mina GraphQL
  and submits through an o1js submitter process. That is outside the connector, so connector ADR
  0002's rule against a sidecar process doesn't apply." CONFIRMED-DOCS.
- ZKT #14, "o1js transaction builder for Rust callers" (open, needs-triage): "one interface that Rust
  callers (the wallet and the challenger) use to build and prove vault transactions … this is the
  one place Rust reaches o1js". Its acceptance criteria include documenting the interface's requests
  and answers (failures included), and "Note secrets cross the interface only from the caller, and
  are never logged or written to disk by the builder." Its AFK runs so far were blocked or failed.
  CONFIRMED-DOCS.
- ZKT #15, the challenger's Mina adapter: it reads through GraphQL and submits through that builder.
  "A dropped GraphQL connection must never end a wait on a submitted transaction." It allows an
  opt-in SOCKS proxy with no direct fallback, mirroring ADR 0073. CONFIRMED-DOCS.
- ZKT design doc (`docs/design/zkapi-payments.md`, "Draft design. Nothing here is built"): "Mina
  zkApps can only be written in o1js" (L53). The Mesa 32-field budget holds the note root, the
  nullifier root (`IndexedMerkleMap`) and a ring of recent roots (L156). It notes that a fallback "o1js
  helper process … would need a new ADR overturning ADR 0002" if it ran **in** the connector
  (L164–170). CONFIRMED-DOCS.
- ZKT #6 (vault deposits, PR #24 open): deposit sizes 10/100/1000 MINA, tree height 20, ring length 8,
  a 2-day challenge window. The deposit method requires the block timestamp to fall in
  `[depositTime, depositTime + one slot]`. Leaf indices are assigned at a permissionless reduce. The
  latest run reports "o1js passes 25/25, including the 6 vault tests". ZKT #16 publishes VKs
  reproducibly with a proofs-on CI set; #17 is a manual devnet deploy that records costs and
  inclusion times. CONFIRMED-DOCS.
- **Inference.** Every design question this note raises for a channel zkApp (state budget, actions
  vs per-account, time preconditions, a Rust-callable o1js builder, VK publication, who pays reduce
  fees) is already open in zk_toon for the vault, and none of it is built.

---

## 5. TOON's own Mina payment-channel zkApp, from history

`packages/mina-zkapp` existed from Story 34.4 (#131) until `e13bd780` (2026-08-27, #1205, ADR
0065-mina). Read at OLD = `dee8c99a`. CONFIRMED-HISTORY throughout.

- **Shape.** `PaymentChannel` was **one zkApp account per channel**, with "exactly 8 Field elements
  (the Mina protocol maximum)": `channelHash` (Poseidon of A.x, B.x and nonce), `balanceCommitment`,
  `nonceField`, `channelState` (0 uninitialized, 1 open, 2 closing, 3 settled), `depositTotal`,
  `closedAtSlot`, `settlementTimeout` and `tokenId_`. It was a **two-party balance-proof** channel
  (balances A and B), not a one-way cumulative voucher channel.
- **Five methods**, all `@method`, so all proof-authorized:
  - `initializeChannel`
  - `deposit(amount, depositor)`, with `AccountUpdate.createSigned(depositor)` binding the depositor
  - `initiateClose(balanceA, balanceB, salt, nonce, sigA, sigB, currentSlot)`
  - `settle(...)`
  - `claimFromChannel(...)`, which verified both participants' signatures in-circuit and updated
    only the commitment and nonce: the "privacy" path
- **The `globalSlot` bug.** `initiateClose` and `settle` originally pinned the exact slot with
  `getAndRequireEquals()`. That "fails with `Protocol_state_precondition_unsatisfied`" on a real
  chain, because "the global slot advances between proof generation and tx inclusion". It was masked
  in tests because "On LocalBlockchain the slot is frozen during a tx". The fix (#202, `ab81d269`)
  replaced it:
  - `initiateClose` takes a caller-supplied `currentSlot` witness bound by
    `globalSlotSinceGenesis.requireBetween(currentSlot, currentSlot + SLOT_WINDOW)`, with
    `SLOT_WINDOW = 7` ("≈ ~21 min on mainnet (≈3 min/slot)", i.e. ~10.5 min after Mesa);
  - `settle` takes `requireBetween(closedAt + timeout, UInt32.MAXINT())`.
- **ADR 0002 is wrong about that fix.** It says the bug was "fixed on a branch and never merged".
  `ab81d269` (2026-06-21) is an ancestor of `main`, merged as `08f93f54` ("Merge pull request #202"),
  a month before ADR 0002 was written (`69815282`, 2026-07-25). Note too that §3.2 shows a
  `network.*` precondition is checked against the parent block's slot. `validWhile`
  (`currentSlot`) is the precondition that targets the inclusion block.
- **Other lessons recorded in the source:**
  - `initiateClose` accepted `sigA`/`sigB` "as circuit witnesses but … not verified on-chain"
    ("TODO(34.4)"). The close was authorized by whoever could prove it, with participant binding
    left to "the SDK level".
  - Custom-token custody needed the token's cooperation (§3.6). The accounting-only variant made
    "the channel key sign the settle tx, authorizing the outflows from the channel's token account".
    That is the custodial pattern of §3.1.
  - `UsdcChannelToken` needed hand-built "lazy-none" account updates and manual cross-account state
    preconditions, because "the high-level `au.account.state` helper is a NO-OP for the state
    array". These pieces "can only be confirmed on a real Mina node with REAL proofs" (commit
    `4ec4da95`), so a nightly lightnet job ran them behind `MINA_INTEGRATION=true`.
- **Operational figures** (OLD `docs/mina-deployment.md`, pre-Mesa; the doc itself calls the
  proving numbers "estimates"):
  - compile 30–90 s and ~2 GB;
  - per-method proving 10–90 s and 1–1.5 GB;
  - 1 MINA account creation per zkApp, ~0.01 MINA per transaction;
  - "~15 confirmations for high confidence" (~45 min at 3 min);
  - a 30-block challenge-period minimum;
  - "Maximum 24 zkApp transactions per block";
  - an archive node required for events;
  - lightnet startup "1–3 minutes to reach SYNCED", needing 4–8 GB RAM.
- ADR 0065-mina's cost accounting: o1js made the faucet image 822 MB, forced a 4 GB plan for a
  99 MB-resident service, compiled circuits for about three minutes at boot, and needed the Node
  ≥ 22.12 pin and a glibc base. CONFIRMED-DOCS.

---

## 6. Testing: can a Mina chain be spawned per test?

- **`Mina.LocalBlockchain`** is an in-process JavaScript ledger: `LocalBlockchain({ proofsEnabled,
enforceTransactionLimits })`, 10 accounts × 1000 MINA, with `setGlobalSlot` and
  `incrementGlobalSlot`. It cannot be reached from a Rust test except through a JS process. It froze
  the slot in a way that hid TOON's #202 bug (§5), and its `slotTime` is the pre-Mesa 180 s (§3.2).
  CONFIRMED-CODE, HISTORY.
- **Lightnet** is the Docker image `o1labs/mina-local-network`. Tags `4.0.2-lightnet`,
  `master-latest-lightnet` and `compatible-latest-lightnet` were updated 2026-10-14, at ~0.9–1.0 GB.

  |                | Single node                                    | Multi node     |
  | -------------- | ---------------------------------------------- | -------------- |
  | Start and sync | "~1-2 minutes"                                 | "~4-6 minutes" |
  | RAM            | ~0.85–0.97 GB steady; docs say 4.5 GB to start | 5.5–6+ GB      |
  - Ports: 8080 (NGINX → GraphQL), 8181 (accounts manager: `GET /acquire-account`,
    `PUT /release-account`, built for "parallel automated tests execution" against one shared
    network), 8282 (Archive-Node-API), 5432 (Postgres).
  - Blocks "every ~20-40 seconds", "Transaction finality (k) in 30 blocks", over 1000 genesis accounts
    at 1550 MINA.
  - The lightnet profile is `proof_level = "none"` and `zkapp_cmd_limit = None`
    ([MINA `profiled/lightnet.ml`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/node_config/profiled/lightnet.ml)).
    `zkapp lightnet start` defaults to type `fast` and proof level `none`.

  CONFIRMED-DOCS, CODE.

- **Inference:** a default lightnet does not verify proofs as mainnet does. A test that should
  catch a VK mismatch or an unprovable precondition needs a lightnet configured for real proofs, or
  a separate proofs-on check like ZKT #16's. Lightnet's per-block limits differ from mainnet's 12.
- `zkapp-cli` runs **one fixed-name container on fixed host ports**, and polls `syncStatus ==
SYNCED` every 10 s. Its latest release is v0.22.9 (2025-06). CONFIRMED-CODE. Running several
  lightnets per host would need a raw `docker run` with remapped ports. UNCONFIRMED.
- The `wait-for-mina-network` GitHub Action exists for CI readiness. CONFIRMED-DOCS.
- **Compile and prove times**, from o1js's own perf baselines on GitHub runners
  ([O1JS `tests/perf-regression`](https://github.com/o1-labs/o1js/tree/a428f9b97817177ef4b5868cf148c0ac67a1048d/tests/perf-regression)),
  in seconds:
  - TokenContract compiles in 11.1 native / 37.0 WASM; Voting in 14.1 / 43.4;
  - a 53k-row program compiles in 57.8 / 172.8 and proves in 23.4 / 66.8;
  - a 13-row program proves in 4.4 / 13.9;
  - verification takes ~0.4 s.

  o1js 3.0's native Node prover claims "~2x proving performance". No first-party memory figures were
  found. CONFIRMED-CODE.

- **Against the repository's rules.** `anvil` starts in well under a second, and
  `solana-test-validator` in seconds. **Inference:** a per-test lightnet would add 1–2 minutes and
  ~1 GB per test. A per-run shared lightnet with the accounts manager matches lightnet's own design,
  but not `CLAUDE.md`'s "`cargo test` spawns its own disposable chain per test". Every write test
  also needs an o1js prover available in CI, and `require_*`'s "fail in CI when missing" rule would
  then cover Docker, the image and Node with o1js.

---

## 7. What in this repository a Mina channel collides with

- **[ADR 0002](../adr/0002-drop-mina-from-the-rust-connector.md):** Mina dropped because "its five
  zkApp methods … require proof generation through o1js"; "a Node sidecar beside the binary" refused.
  It leaves "an out-of-process implementation" open "as a sidecar crate". CONFIRMED-DOCS.
- **[ADR 0065-mina](../adr/0065-mina-leaves-the-repository.md):** falsifier `**/package.json`
  matching `"o1js"`. "If Mina work resumes it starts from `git show`". CONFIRMED-DOCS.
- **[ADR 0080](../adr/0080-a-client-may-pay-from-a-zk-note.md):** falsifiers `**/Cargo.toml`
  matching `o1js` and `**/package.json` matching `"o1js"`. "The connector **verifies** proofs that o1js
  produced elsewhere. It proves nothing, and it sends no Mina transaction." "There is still no Mina
  channel, no Mina settlement backend, and no sidecar." **A Mina channel contradicts all three
  sentences.** CONFIRMED-DOCS.
- **`CLAUDE.md`:** "Do not reintroduce an o1js dependency." CONFIRMED-DOCS.
- **[ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision
  1:** "A connector settles only on x402 `batch-settlement` channels"; the contract or program is "a
  constant of the binary"; "A network x402 has not deployed to is unsupported, loudly". Its "What two
  kinds of channel cost" names "Two sets of on-chain code to deploy, record, audit and keep in step
  with the binary" as the reason TOON's own contracts were retired. CONFIRMED-DOCS.
- **[ADR 0073](../adr/0073-settlement-rpc-may-ride-the-circuit-once-every-wait-on-it-is-bounded.md):**
  every client of a table's `rpc_url` is built from one `RpcTransport`, and every write confirms
  through a bounded loop. A Mina table would add a daemon GraphQL URL, probably an archive URL, and
  (if proving is out of process) a prover or submitter endpoint. CONFIRMED-DOCS; the URL list is
  Inference.
- **[ADR 0071](../adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md):** a MINA leg
  next to a USDC leg is a denomination boundary (9 vs 6 decimals, and a price) that a forwarding node
  crosses only at a declared rate. A peering in MINA is therefore also a rate source decision.
  CONFIRMED-DOCS.
- **[ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md):** a Mina voucher is a new claim
  shape and a new greeting entry, so it means new vectors and a `schema_version` bump. Inference
  from 0074/0075's precedent.

---

## Unconfirmed, and why

- **No Rust prover for o1js circuits anywhere.** This is a negative. mina-rust's prover was read and
  does not do it, and searches found no other. A fork of o1js's OCaml/Rust core could in principle be
  driven natively; nobody has published one.
- **Account creation fee is not reclaimable.** No account-deletion operation was found in the
  transaction logic read. It was not searched exhaustively.
- **How mainnet zkApps survived Mesa's VK change.** The `*DuringCurrentVersion` fallback to
  `signature` is confirmed. Whether any deployed zkApp was actually re-keyed that way was not checked.
- **Several lightnets per host.** Inferred from zkapp-cli's fixed container name and ports, not
  tried.
- **Proving memory on o1js 3.x.** No first-party figures. The old repository's 1–2 GB figures were
  self-described estimates, pre-Mesa.
- **The date of the mina-rust pause.** Only the README commit (2026-07-15) is primary. No blog or
  forum announcement was found.
- **Whether `editActionState: none` could let a payer dispatch channel actions without the zkApp's
  proof.** It was not tested. If it can, the reduce step must re-validate every action, so a proof is
  still needed, only later.

---

## Decisions this forces

Each item gives the options and the facts that bear on them. This note recommends none, beyond
flagging the blockers.

### D1. Is a non-x402 channel program acceptable at all? **(blocker in the records)**

ADR 0075 decision 1 admits only x402's own deployed code, fixed in the binary, and refuses a chain x402
has not deployed to. x402 has no Mina binding, no Mina implementation and no Mina mention (§2.3). Its
neutral spec is a frame, not a protocol (§2.1).

- **(a)** Write `scheme_batch_settlement_mina.md` upstream first. CONTRIBUTING requires an issue
  or discussion before a spec. Five other chains' bindings sit open, none merged. TOON would still
  write and deploy the zkApp.
- **(b)** A TOON-owned zkApp that speaks the batch-settlement **semantics** (§2.2) under TOON's own
  binding. This amends ADR 0075 decision 1 and reopens the "two sets of on-chain code" cost it
  retired.
- **(c)** Not now.

Either (a) or (b) means TOON deploys, audits and redeploys on-chain code again. Mesa (VKs changed)
and o1js 3.1.0 (recompile and redeploy) have each already forced that once in 2026 (§3.4).

### D2. Where does the zkApp's source live? **(blocker in this repository)**

ADR 0065-mina's and ADR 0080's falsifiers fail the build on `o1js` in any `package.json` or
`Cargo.toml` here, and `CLAUDE.md` says "Do not reintroduce an o1js dependency" (§7).

- **(a)** zk_toon, beside the vault: it already has the o1js package, Rust/o1js parity vectors and
  a planned Rust-callable builder (#14) and VK publication (#16).
- **(b)** A new repository.
- **(c)** This repository, which overturns 0065-mina and amends 0080 and `CLAUDE.md`.

ADR 0065-mina rejected moving the old zkApp to its own repository as "ceremony" only because
nothing consumed it. That reason would not hold for a zkApp the connector settles on.

### D3. How does the connector get proofs and submit? **(the hard blocker)**

Every write row in §4.3 needs an o1js proof, including `land`, the receiver's only protection
(ADR 0074 §5). No Rust prover exists (§4.2).

- **(a)** An o1js process beside the connector, run by the operator. This is ADR 0002's refused
  sidecar; it overturns 0002 and amends 0080 ("no sidecar").
- **(b)** An o1js submitter **outside** the connector, behind a port, as zk_toon #2 and #14 plan.
  zk_toon argues this is "outside the connector, so connector ADR 0002's rule against a sidecar
  process doesn't apply". Whether a process the operator must run for its node to be safe is a
  "sidecar" is exactly what an amendment to 0002 would decide.
- **(c)** A third party proves and submits. On Mina, as on Solana's `settle`, a voucher landing
  can be made by anyone (§4.3). ADR 0076's operator-named facilitator is the nearest precedent.
  The receiver's safety would then depend on that party's liveness inside the withdraw delay.
- **(d)** A Rust prover. None exists, mina-rust is paused (§4.2), and ADR 0002 rejected
  reimplementing the circuit.

Whichever is chosen, signature-authorized pieces (the payer's debit, the fee payer) can be signed in
Rust (§4.1), but MMS `mina-tx` is `1.0.0-alpha.0`. The port's idempotence (`open_prepared` safe to
repeat) and ADR 0073's bounded waits must hold across the process boundary.

### D4. Channel per account, or many channels in one zkApp

Facts:

- 32 fields after Mesa;
- state preconditions serialise writers;
- the basic Reducer breaks past 32 pending actions;
- BatchReducer and OffchainState are `Experimental`, and IndexedMerkleMap is stable;
- each new account costs 1 MINA, probably not reclaimable;
- 12 zkApp commands per block on mainnet (§3.3–3.4).

The options:

- **(a)** One zkApp account per channel, as TOON's old zkApp did. No contention between channels,
  but a deploy and 1 MINA per channel, and a deploy transaction per open.
- **(b)** One zkApp with per-channel accounts under its own token id (the `TokenEscrow` shape).
- **(c)** One shared zkApp with channels in an `IndexedMerkleMap`, updated through actions and a
  BatchReducer, as zk_toon's vault does. This needs someone to pay for and prove the reduce calls,
  and an archive endpoint to read state back.

How `channel_state`, the `WithdrawInitiated`-style watcher and the Solana-style rediscovery sweep
read the chain differs for each.

### D5. Authorization per operation

Facts: a signature authorizes only its own signer's update. Voucher checks happen in-circuit only.
`proofOrSignature` on the zkApp makes its key holder a custodian (§3.1).

- **(a)** `proof` for every channel write, with `setVerificationKey` at
  `proofDuringCurrentVersion` (§D7).
- **(b)** Allow `signature` for some writes, which reintroduces a custodial key. TOON's
  accounting-only variant did this and was then replaced (§5).

Also decide who may land a voucher. EVM restricts it to receiver/`receiverAuthorizer`; Solana's
`settle` is anyone's.

### D6. Time, delay and the slot pitfall

Facts:

- use `currentSlot.requireBetween` (inclusion block) rather than `network.*` (parent block);
- never require an exact slot;
- slots are 90 s; LocalBlockchain still assumes 180 s (§3.2).

Choose the delay's unit (slots or timestamp) and its minimum. Weigh the minimum against 15-block
(~30 min) confirmation and the 12-per-block zkApp cap. The connector's published minimum is one day.
Also decide whether the EVM liveness gap (a withdrawal finalising before a claim) is reproduced or
closed.

### D7. Hard forks and verification keys

Mesa changed every VK. o1js 3.1.0 and unreleased `main` change them again. The `*DuringCurrentVersion`
permissions fall back to `signature` after a version change (§3.4). The connector's verifier was
already found to be pinned to one fork (SPIKE).

- **(a)** An upgrade key, which is a trust statement like `payment-channels`' upgrade authority
  (ADR 0074 §5).
- **(b)** `impossible`, so channels are stranded at the next VK-changing fork unless drained first.
  The drain procedure for TOON's EVM and Solana channels is the precedent.

### D8. Asset: native MINA first, a token later

Native MINA needs no third-party proof. A `FungibleToken` needs its proof in every transfer, its
admin can pause withdrawals, and MFT has no o1js 3.x release (§3.6). No stablecoin is live on Mina
(TOKENS).

A MINA peering next to a USDC peering is an ADR 0071 denomination boundary with a rate source. A
later token channel is, per TOKENS §5.3's reasoning, a different zkApp and VK.

### D9. Voucher format and the wire

Decide:

- the signed fields, for example Poseidon over (domain tag, channel id, cumulative, maybe the
  zkApp address and network);
- the network-id domain: o1js's in-circuit `Signature.verify` uses the devnet domain even on
  mainnet (§4.1);
- the amount width: `u64` nanomina, against the port's `u128`;
- the greeting's `batchSettlements` entry for Mina (CAIP-2 `mina:mainnet` / `mina:devnet`);
- a new `VoucherSigner`, `ChannelPresentation`, `ReceiverTerms` and `OutboundChannelRecord`
  variant;
- the vectors and `schema_version` bump (ADR 0021);
- the Pallas settlement key, a third key type (ADR 0075 decision 3).

### D10. Confirmation semantics in the backend

ADR 0073's EVM bound (180 s to mine) is two Mesa slots. Mina's recommended 15 blocks is ~30 min
(§3.5).

Decide:

- what "confirmed" means for `land`, `open_prepared` and `top_up`;
- how a lost answer is resolved: Mina has no blockhash expiry, but zkApp commands carry a nonce,
  and `validWhile` can bound inclusion;
- how a proving step of tens of seconds fits inside the bounded waits.

### D11. How tier-3 tests get a Mina chain

Facts:

- lightnet is ~1 GB with 1–2 min sync, built for one shared network with an accounts manager, and
  defaults to `proof_level = none`;
- LocalBlockchain is in-process JS that hid the #202 bug;
- every write test also needs o1js to prove (§6).

The options:

- **(a)** One lightnet per test, faithful to `CLAUDE.md`'s rule but costly.
- **(b)** One lightnet per `cargo test` run, using the accounts manager. This needs a `CLAUDE.md`
  amendment.
- **(c)** A proofs-on lightnet or devnet job off the gate, like `onion`. `CLAUDE.md` keeps
  third-party-network checks off the gate deliberately.

Whichever is chosen, `require_*`'s fail-in-CI rule would have to cover Docker, the image and the
o1js prover, and a proofs-off lightnet must not be the only thing that checks the VK.

### D12. Who pays Mina fees

Facts: every zkApp command has a signing fee payer; a new account costs 1 MINA; reduce calls cost
fees and someone must make them (§3.5, ZKT #2).

On Solana the receiver sponsors rent and fees (ADR 0074 decision 9). On EVM each party pays its own
gas. Decide the Mina equivalent for open, land, reduce and payout, and whether a payer needs MINA
for fees as well as for the deposit. On a native-MINA channel it trivially has it.
