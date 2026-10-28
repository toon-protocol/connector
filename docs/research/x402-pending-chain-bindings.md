# x402 batch-settlement bindings still pending upstream: what each chain would take

> **Research note, not a decision record.** It decides nothing. The question comes from the owner:
> a spec for the connector to settle on **every chain with an open x402 batch-settlement binding PR**,
> alongside EVM and Solana (built, [ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md))
> and Mina (planned, map [#1548](https://github.com/toon-protocol/connector/issues/1548)). This note
> gathers the facts that spec needs, binding by binding, and the decisions they force. It builds on
> [`mina-x402-batch-settlement.md`](mina-x402-batch-settlement.md) (cited as **MINA-NOTE**) and does
> not redo its ground: the port (MINA-NOTE §1), the EVM/SVM semantics table (MINA-NOTE §2.2) and Mina
> itself are there. Primary sources only. Written 2026-10-28, and every source was read that day.

**Evidence tags.** CONFIRMED-CODE means read in source at a pinned commit, or read live from a
chain's RPC. CONFIRMED-DOCS means read in the maintainer's own documents, specs, release notes, issues
or PR threads. UNCONFIRMED means none of those, and says why. A sentence marked **Inference** is this
note's reasoning from confirmed facts, not a fact.

**Sources pinned.**

- **x402**: [`x402-foundation/x402@261d0c5e`](https://github.com/x402-foundation/x402/tree/261d0c5e1b3d65c7d41839923399de39cd02cec1)
  (`main`, 2026-10-28T12:14Z). MINA-NOTE pinned `712e20e1`; nothing under
  `specs/schemes/batch-settlement/` that it cited has changed except as §1 says. Cited as **X402**.
- **The PR heads**, each cited in its own section:
  XRPL [#2952](https://github.com/x402-foundation/x402/pull/2952) at `Dhali-org/x402@683fb7a6`;
  Hedera [#3466](https://github.com/x402-foundation/x402/pull/3466) at `8358eb89`;
  Bitcoin [#3544](https://github.com/x402-foundation/x402/pull/3544) at `davidshekunts-bit/x402-gobtc-pay@2c753c20`;
  Cardano [#3580](https://github.com/x402-foundation/x402/pull/3580) at `75ad5e81`;
  credit network [#2612](https://github.com/x402-foundation/x402/pull/2612) (draft).
- **Subbit** (Cardano's channel validator): [`kompact-io/subbit-xyz@c45f4869`](https://github.com/kompact-io/subbit-xyz/tree/c45f4869).
- **xrpl-rust**: [`XRPLF/xrpl-rust@2a5191b1`](https://github.com/XRPLF/xrpl-rust/tree/2a5191b1) (crate 1.3.0).
- **This repository**: `HEAD` `9a039468` (branch `adr-0077-zk-note`), and issue
  [#1558](https://github.com/toon-protocol/connector/issues/1558) (closed 2026-10-28, owner's
  resolution: option 2) for the amendment to ADR 0075 decision 1. That amendment is **decided but not
  yet written into ADR 0075** at this `HEAD`; decision 1 still reads "A network x402 has not deployed
  to is unsupported, loudly". CONFIRMED-DOCS.
- **The survey of other chains** is a full listing of all 3,795 x402-foundation issues and PRs (REST,
  not search) taken 2026-10-28, plus `coinbase/x402`'s open PRs.

---

## Verdict

**Four chain bindings are open upstream, and none is merged, reviewed by an x402 maintainer, or
backed by on-chain code x402 owns.** They are four different kinds of thing:

- **XRPL (#2952)** is a spec over a **native ledger primitive** (`PayChannel`). There is no contract
  for anyone to own, and the lifecycle maps onto the port's Solana shape. Its hard limit is that
  **channels hold XRP only**: RLUSD and native USDC exist on XRPL but cannot enter a channel.
- **Hedera (#3466)** is the **EVM contract forked**, signature checks routed through Hedera's Account
  Service (`0x16a`). It is the only binding with code. Its contract is deployed on **testnet only, by
  the PR's author, who holds an admin key**, and **no Hedera chain can be spawned per test**.
- **Cardano (#3580)** is a spec over a **third-party Aiken validator** (Subbit, Kompact.io), alpha and
  unaudited, whose own author is writing a **second, incompatible** Cardano x402 dialect. The
  mechanism fits the port's Solana shape; eUTXO, min-UTxO and the indexer it needs do not fit cleanly.
- **Bitcoin (#3544)** is **not a channel**. It is a custodial 2-of-3 co-signing service (GoBTC Rails)
  in which the receiver cannot land anything without the vendor's key and the connector would have to
  call the vendor's facilitator. It contradicts ADR 0074 decision 5 and ADR 0076's falsifier as written.

**Two "bindings" are not chains.** Cloudflare's merged `_cloudflare.md` and the credit network draft
(#2612) are credit-backed: nothing is escrowed and nothing can be landed, so neither can sit behind
`BatchSettlementBackend`.

**The cheapest new networks are not on the list at all.** `x402BatchSettlement` is deployed at its
canonical CREATE2 address on a dozen EVM chains, and the existing EVM backend already boots on any of
them. The limit is **one EVM chain per node** (§1, §8).

**Every new chain widens every per-chain enum** (`ChannelPresentation`, `ReceiverTerms`,
`OutboundChannelRecord`, `VoucherSigner`, `ClientClaim`, `SettlementChain`, `AssetChain`, the greeting
types) and adds a key to each per-chain wire-vector group (§8). The contract suites assert "what holds
on both chains"; each pending chain adds at least one lifecycle fact the suites do not yet express (§8,
D3).

| Chain                 | PR                                                                                                                                | Status (2026-10-28)                                                                                        | Spec / code                                       | On-chain code exists, and owner                                                                    | Voucher signature                                                                         | Native asset / stablecoin in a channel                                                      | Rust                                                        | Per-test chain                                                                                   | Finality                                            | Biggest blocker                                                                    |
| --------------------- | --------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------- | -------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- | ----------------------------------------------------------- | ------------------------------------------------------------------------------------------------ | --------------------------------------------------- | ---------------------------------------------------------------------------------- |
| **XRPL**              | [#2952](https://github.com/x402-foundation/x402/pull/2952)                                                                        | Open since 2026-07-25, idle since 2026-08-13; one non-maintainer review                                    | Spec only (888 lines)                             | Native `PayChannel` ledger object: protocol-owned, **nothing to deploy**                           | `CLM\0 ‖ channelId ‖ u64` cumulative; secp256k1 or Ed25519                                | XRP (u64 drops, 6 dp). **Channels are XRP-only**; RLUSD and USDC exist but cannot enter one | `xrpl-rust` 1.3.0 (XRPLF), native                           | `xrpld -a --start` + `ledger_accept`; apt (Ubuntu/RHEL) or Docker; **no clock warp**             | Validated ledger ~3–5 s, no reorgs                  | XRP-only: every XRPL leg is an ADR 0071 rate boundary; spec unreviewed             |
| **Hedera**            | [#3466](https://github.com/x402-foundation/x402/pull/3466)                                                                        | Open since 2026-09-13, one commit, 0 reviews, merge conflicts, CI not run                                  | Spec + forked Solidity + TS SDK + tests (+18,485) | **Testnet only**, deployed by the PR author, admin key set; no mainnet; not at the CREATE2 address | Same EIP-712 voucher digest, checked by HAS `isAuthorizedRaw` (`0x16a`): ECDSA or Ed25519 | HBAR (8 dp); **native Circle USDC** HTS `0.0.456858` (6 dp); HTS amounts `int64`            | `alloy` via the JSON-RPC relay, or `hiero-sdk` 0.45         | **None**: anvil lacks HAS/HTS; Local Node deprecated (16 GB, fixed ports); Solo needs Kubernetes | aBFT, deterministic, "within seconds"               | Unreviewed fork deployed only by its author; no disposable test chain              |
| **Cardano**           | [#3580](https://github.com/x402-foundation/x402/pull/3580) (+ issue [#3579](https://github.com/x402-foundation/x402/issues/3579)) | Open since 2026-09-25, last push 2026-10-12; 0 reviews; Cardano Foundation engineers positive in the issue | Spec only (v0.11); TS reference out of tree       | Subbit Aiken validator `6d877463…`, permissionless, Kompact.io, **alpha, unaudited**, preprod only | Ed25519 over CBOR `[tag, cumulative]`, no nonce, no expiry                                | ADA (6 dp, u64); USDCx (Circle xReserve, 6 dp); USDM (6 dp); no native USDC                 | pallas 1.4, `uplc` 1.1.24; **needs an indexer**             | Yaci DevKit or Dolos devnet, or cardano-node + indexer; no single binary like anvil              | ~20 s blocks, long-tailed; probabilistic (k = 2160) | Third-party unaudited validator, two incompatible dialects, eUTXO contention       |
| **Bitcoin**           | [#3544](https://github.com/x402-foundation/x402/pull/3544)                                                                        | Open since 2026-09-21, one commit, no human comment                                                        | Spec only (423 lines), mainnet only               | No contract: a 2-of-3 P2WSH wallet, K2 held by GoBTC Rails, K3 by a "recovery provider"            | K1-signed PSBT per payment, built by the facilitator (ECDSA secp256k1)                    | BTC only (8 dp, u64)                                                                        | `bitcoin` 0.32, `miniscript`, `bdk_wallet`, `corepc-client` | `bitcoind -regtest` via the `bitcoind` crate; the vendor service cannot be spawned               | ~10 min blocks, 6 conf by convention                | **Custodial**: receiver cannot land; connector would call the vendor's facilitator |
| Mina (for comparison) | none; TOON to propose (#1558 → #1562)                                                                                             | Planned, map #1548                                                                                         | TOON-owned binding, code in zk_toon               | None yet                                                                                           | Schnorr/Poseidon (Pallas)                                                                 | MINA (9 dp)                                                                                 | Off-chain yes; **no Rust prover**                           | Lightnet (~1 GB image, 1–2 min)                                                                  | ~30 min recommended                                 | Every channel transaction needs an o1js proof (MINA-NOTE)                          |

---

## 1. The chain-neutral outline and the merged bindings

Pinned at [`x402-foundation/x402@261d0c5e`](https://github.com/x402-foundation/x402/tree/261d0c5e1b3d65c7d41839923399de39cd02cec1) (`main`, 2026-10-28). Cited as **X402**.

- **The outline is unchanged** since the `712e20e1` that [`mina-x402-batch-settlement.md`](mina-x402-batch-settlement.md) §2.1 cites.
  [`scheme_batch_settlement.md`](https://github.com/x402-foundation/x402/blob/261d0c5e1b3d65c7d41839923399de39cd02cec1/specs/schemes/batch-settlement/scheme_batch_settlement.md)
  is still 76 lines, last touched by `c763d05a` (#2051, 2026-05-05). CONFIRMED-DOCS.
  - The lifecycle is **Commit → Accumulate → Redeem** (L49–53). There are two trust models,
    **capital-backed** and **credit-backed** (L27–33).
  - A binding MUST specify seven things (L59–67): commitment format; verification rules; storage
    behaviour and the commitment identifier; double-spend prevention; commitment expiry; redemption
    (who, when, which rail); and the trust model.
  - It defines no voucher struct, no cumulative rule, no withdrawal delay and no exit. The outline
    allows **"a channel close"** as the redemption (L53, the "payment channel streaming" use case).
    So a binding that settles only by closing (Bitcoin-, Cardano-, Kaspa-shaped) conforms to the
    outline. It would not conform to the EVM/SVM semantics the connector's suites assert (see the
    last section). **Inference.**
- **`_evm.md`**: 866 lines, now **v1.1** (Version History L866: "v1.1 | 2026-10-06 |
  Facilitator-managed vouchers mode"). Last changed in `be54b7d3` (#3725, 2026-10-26).
  CONFIRMED-DOCS.
  - **Facilitator-managed mode** (`extra.voucherManager: "facilitator"`, L79–92) makes the resource
    server a pass-through. The **facilitator** persists the vouchers, `chargedCumulativeAmount`
    and a `chargeCount`, and runs the claim/settle schedule. It is always the
    `receiverAuthorizer`. An optional server-owned `extra.refundAuthorizer` is packed into
    `salt` (`bytes12(entropy) ‖ bytes20(refundAuthorizer)`, L138), so it changes `channelId`
    derivation **for managed channels only**. Self-managed channels never pack (L138).
    CONFIRMED-DOCS.
  - For TOON: the mode is opt-in through the 402. The connector publishes self-managed terms
    (`receiverAuthorizer` = its own settlement key, ADR 0075 decision 3), so its channels are
    unaffected. Adopting managed mode would hand custody of the vouchers, the thing ADR 0074
    decision 5 says protects value, to a third party. That reverses ADR 0076 decision 1 ("publishes
    the facilitator and never calls it"). **Inference.**
- **`_svm.md`**: 2096 lines, last changed in `72dde233` (#3474, 2026-10-26, transaction v1).
  `payment-channels` is bound by a **constant** program id, `CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX`
  ("MUST NOT trust or negotiate a `channelProgram` value", L82–89). CONFIRMED-DOCS. The id holds an
  executable account on **both mainnet-beta and devnet** (`getAccountInfo`, 2026-10-28).
  CONFIRMED-CODE (RPC).
- **Every EVM chain with the canonical deployment is already a cheap add.**
  - [`contracts/evm/README.md`](https://github.com/x402-foundation/x402/blob/261d0c5e1b3d65c7d41839923399de39cd02cec1/contracts/evm/README.md)
    L28–64 lists the CREATE2 vanity addresses: `x402BatchSettlement` `0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003`,
    `ERC3009DepositCollector` `0x4020806089470a89826cB9fB1f4059150b550004`, and
    `Permit2DepositCollector` `0x4020425FAf3B746C082C2f942b4E5159887B0005`.
  - It lists deployments on **Base, Arbitrum, World Chain, World Chain Sepolia, Polygon, Optimism,
    Avalanche C-Chain, Celo, Linea, Unichain, Monad and Monad Testnet** (Monad Testnet added by
    #3563, merged 2026-10-22). CONFIRMED-DOCS.
  - **Base Sepolia is not in the table, but the address holds code there**: `eth_getCode` on
    `sepolia.base.org` returned bytecode, 2026-10-28. CONFIRMED-CODE (RPC). The README's table is
    incomplete, not authoritative.
  - "Anyone can deploy … to their canonical addresses on any EVM chain", through Arachnid's CREATE2
    deployer, provided Permit2 is present (README L80–117). CONFIRMED-DOCS.
  - The connector's boot check is only "code is non-empty at the constant address"
    ([`batch_settlement.rs` L162–170](../../crates/connector-settlement-evm/src/batch_settlement.rs)),
    and the CAIP-2 id is `eip155:<chain_id>` read from the chain
    ([`connector-cli/src/runtime.rs` L896](../../crates/connector-cli/src/runtime.rs)).
    CONFIRMED-CODE.
  - So any of those EVM chains works **today, with no code change**. The limit is **one EVM chain
    per node**: `[settlement.evm]` is a single TOML table, and `SettlementChain` has one `Evm`
    variant ([`settlement.rs` L146–149](../../crates/connector-config/src/settlement.rs)).
    CONFIRMED-CODE. Two EVM chains on one node is a config and port change, not a new backend.
    **Inference.**

---

## 2. XRPL (#2952)

**Pins.** x402 [`261d0c5e`](https://github.com/x402-foundation/x402/tree/261d0c5e1b3d65c7d41839923399de39cd02cec1)
(`main`, 2026-10-28). PR [#2952](https://github.com/x402-foundation/x402/pull/2952) at head
[`Dhali-org/x402@683fb7a6`](https://github.com/Dhali-org/x402/blob/683fb7a6b08b5cc12568b36d6f560ba0d57dcf1e/specs/schemes/batch-settlement/scheme_batch_settlement_xrpl.md)
(2026-08-13). xrpl-rust [`XRPLF/xrpl-rust@2a5191b1`](https://github.com/XRPLF/xrpl-rust/tree/2a5191b1)
(2026-10-26, `1.3.0`). rippled/xrpld `3.4.1` (2026-10-10), the version XRPL mainnet reported to
`server_info` on 2026-10-28. xrpl.org pages were read on 2026-10-28.

### 2.1 PR status and provenance

- **Who.** Opened 2026-07-25 by `desimmons` from the `Dhali-org` fork. Dhali (dhali.io) sells
  "off-chain high-frequency API payments" over payment channels. The same author's earlier PR
  [#1302](https://github.com/x402-foundation/x402/pull/1302) ("scheme deferred Dhali", 2026-02-22,
  82 lines) is still open and unreviewed. It proposed a Dhali-specific scheme across Ethereum, XRPL
  and Xahau. CONFIRMED-DOCS.
- **What it is.** **Spec only**: one new file, `scheme_batch_settlement_xrpl.md`, +888 lines,
  version string `xrpl-paychan-v1`, "Status: Draft". The PR body says: "This is a
  specification-only change. It does not modify the TypeScript, Python, or Go implementations."
  CONFIRMED-DOCS.
- **Review.**
  - Labels: `specs`, `xrpl`. `reviewDecision = REVIEW_REQUIRED`.
  - The only review is one COMMENTED review from `whawk46`, an outside contributor who discloses
    that the review was AI-assisted. It recommends "landing it after minor clarifications". The
    author answered all three points in the single force-pushed commit `683fb7a6`.
  - **No maintainer has commented.** The last activity was 2026-08-13: 2.5 months idle.
  - CONFIRMED-DOCS.
- **Likelihood signals** (**Inference**).
  - Maintainers are active on XRPL elsewhere in x402:
    - `exact` on XRPL is merged: spec #2547 (2026-07-03) and the TypeScript implementation #2801 (2026-07-14), as `typescript/packages/mechanisms/xrpl`.
    - `upto` on XRPL is merged as #3023 (2026-10-22).
    - CarsonRoscoe (a maintainer) agreed in [#3950](https://github.com/x402-foundation/x402/issues/3950) and opened [#4008](https://github.com/x402-foundation/x402/pull/4008) (2026-10-27) to move XRPL `upto` to an `escrow` flow.
  - So XRPL as a network is welcome upstream, but #2952 itself has had no maintainer attention.
- **No code exists anywhere upstream for XRPL batch-settlement**: no SDK, no facilitator and no
  vectors. CONFIRMED-CODE: the tree at `261d0c5e` has XRPL files only for `exact` and `upto`.
- **On-chain code: none to deploy.** XRPL payment channels are a native ledger object
  (`PayChannel`) driven by native transactions (`PaymentChannelCreate`, `PaymentChannelFund`,
  `PaymentChannelClaim`). There is no contract, no program id and no deployer. The "owner" is the
  XRPL protocol, changed only by validator-voted amendments. CONFIRMED-DOCS
  ([PayChannel](https://xrpl.org/docs/references/protocol/ledger-data/ledger-entry-types/paychannel)).
  - **Inference:** this suits ADR 0075 decision 1 better than any other pending binding. There is
    no third-party or TOON-owned money code at all, so "x402's own deployed code" collapses to "the
    ledger". What stays unmerged is only the spec.

### 2.2 Mechanism

- **Deposit.**
  - The payer submits `PaymentChannelCreate{Destination = payTo, Amount, SettleDelay ≥ extra.minSettleDelay, PublicKey, DestinationTag?}`.
  - It tops up with `PaymentChannelFund`, which only the source can submit and which can also
    extend `Expiration`.
  - Creation must be seen in a **validated** ledger before use (spec §6.1).
  - Channel id = `SHA512Half(0x0078 ‖ source ‖ destination ‖ Sequence-or-TicketSequence)`.
  - CONFIRMED-DOCS.
- **Voucher (a "claim").**
  - The signed message is `0x434C4D00 ("CLM\0") ‖ channelId[32] ‖ u64 BE drops` (spec §6.2).
  - The key is the channel's `PublicKey`, fixed at create. It is secp256k1 (33-byte compressed) or
    Ed25519 (`0xED`-prefixed, 33 bytes).
  - The amount is **cumulative**, with no nonce and no expiry. The format is the same shape as EVM's
    `Voucher(channelId, maxClaimableAmount)`.
  - xrpl-rust signs secp256k1 over `SHA512Half(message)` with a DER signature, and Ed25519 over the
    raw message (`src/core/keypairs/algorithms.rs` L63, L269–277, L415). It has a helper,
    `encode_for_signing_claim` (`src/core/binarycodec/mod.rs` L113–123).
  - The claim binds **no network id**: one signed claim verifies on mainnet, testnet and devnet
    alike. Merged `scheme_upto_xrpl.md` warns about this and tells wallets to use separate accounts
    per network.
  - CONFIRMED-CODE, DOCS.
- **The PR's metering model is not TOON's.**
  - In #2952 each request's claim signs `committedAmount + amount`, where `amount` is the
    _maximum_ price. The facilitator then records a lower actual charge off-ledger in
    `committedAmount`.
  - Redemption therefore submits `Amount = authorizedAmount` and `Balance = committedAmount`, with
    `Balance < Amount` allowed.
  - That needs a single serialized state authority per channel (§7), the `payment-identifier`
    extension, and a corrective 402 (§14.2).
  - TOON's vouchers sign the exact cumulative charge, so a TOON backend would always land
    `Amount == Balance == cumulative`. XRPL permits that.
  - **Inference:** the PR's off-ledger state machine (reservations, payout routing,
    `xrpl-paychan:` commitment ids) is facilitator-side and **irrelevant to the connector**. Only §4
    (identifiers), §6.1–6.2 (channel and signing input) and §11 (the redemption transaction) bind it.
- **Land.**
  - The destination submits `PaymentChannelClaim{Channel, Amount, Balance, Signature, PublicKey}`
    with **no `tfClose`**. That delivers `Balance − PayChannel.Balance` and leaves the channel open.
  - "At any time, the recipient can redeem a Claim."
  - This is EVM `claim` and Solana `settle`, and it maps 1:1 to `land`.
  - CONFIRMED-DOCS ([PaymentChannelClaim](https://xrpl.org/docs/references/protocol/transactions/types/paymentchannelclaim)).
- **Payer exit.**
  - The source submits `PaymentChannelClaim` with `tfClose`.
  - If XRP remains, the channel is not closed. Instead `Expiration = parent close time + SettleDelay`
    "unless the channel already has an earlier `Expiration`".
  - After `Expiration`, or after the immutable `CancelAfter`, _any_ transaction touching the channel
    closes it and refunds `Amount − Balance` to the source. The `PayChannel` entry is then
    **deleted** from the ledger.
  - CONFIRMED-DOCS.
- **Receiver close.** The destination's `tfClose` closes the channel immediately after processing
  the claim. This is Solana's `settle_and_seal`. CONFIRMED-DOCS.
- **No partial withdrawal.** The payer cannot reclaim part of the escrow and keep the channel. The
  exit is all-or-nothing, so it is **`PayerExit::Close`, Solana-shaped**. CONFIRMED-DOCS.
- **Port mapping** (**Inference**, from the facts above and `crates/connector-settlement/src/batch/port.rs`):

  | Port item                        | XRPL                                                                                                                                                                                                                                |
  | -------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
  | `admit` / `restore`              | `ledger_entry` / `account_channels` on a validated ledger. Presentation = channel id only (like Solana): every field is readable on chain                                                                                           |
  | `channel_state`                  | `landed = PayChannel.Balance`; `collateral = Amount − Balance` while no `Expiration`, else 0                                                                                                                                        |
  | `BatchChannelStatus`             | `Open` (no `Expiration`); `Closing` (`Expiration` set, not passed); `Sealed` = entry **gone** or deadline passed. No `Withdrawing`                                                                                                  |
  | `land`                           | destination `PaymentChannelClaim`, no flag; on `Closing`, may add `tfClose` (seal)                                                                                                                                                  |
  | `prepare_open` / `open_prepared` | sign `PaymentChannelCreate` with a fixed `Sequence` and `LastLedgerSequence`, derive the id offline, journal the blob, resubmit the **same blob**: lands at most once (sequence). Past `LastLedgerSequence` unlanded = `OpenLapsed` |
  | `top_up`                         | `PaymentChannelFund`                                                                                                                                                                                                                |
  | `sign_voucher`                   | `CLM\0` claim, u64                                                                                                                                                                                                                  |
  | `start_withdrawal`               | source `PaymentChannelClaim{tfClose}`                                                                                                                                                                                               |
  | `finish_withdrawal`              | any tx touching the channel after `Expiration` (source `PaymentChannelFund` or `PaymentChannelClaim`), which deletes it                                                                                                             |
  | `sign_claim_state_challenge`     | needs a TOON domain-separated message under the channel key; nothing upstream defines one                                                                                                                                           |

- **Misfits** (**Inference** unless tagged).
  1. **Sealed is a deleted entry.** A closed channel is _removed_. "Not found" must be told apart
     from "sealed" using the journal (an admitted or restored id that is now absent = `Sealed`).
     Today `ChannelNotFound` on `restore` is the suite's answer for an unknown channel.
  2. **`Closing` is reversible.** The source can `tfRenew` (clear `Expiration`), and
     `PaymentChannelFund` can change `Expiration`. Solana's `Closing` never returns to `Open`. A
     suite assertion that status is monotone would need checking.
  3. **`CancelAfter` is a second, immutable deadline** that neither EVM nor Solana has. Admission
     must refuse a channel whose `CancelAfter` (or `Expiration`) leaves less than the minimum delay.
     The PR's verify step 10 does the same. A new `AdmissionRefusal` variant is needed.
  4. **Amounts are u64 drops.** This fits u128 ports, as Solana does. A voucher above `u64::MAX`
     is refused structurally.
  5. **No time warp in tests.** `SettleDelay` and `Expiration` are compared with ledger close time
     (wall clock, rounded to close-time resolution). EVM tests warp anvil's clock. XRPL tests must
     use a tiny `SettleDelay` and real waiting. UNCONFIRMED: whether stand-alone `ledger_accept`
     can advance close time artificially. No such parameter is documented.
  6. **Destination prerequisites.** The destination account must exist and be funded to its
     reserve. A `DestinationTag` and the destination's DepositAuth (`CredentialIDs`) are extra
     admission dimensions. CONFIRMED-DOCS for the fields.
  7. **No fee sponsorship.** XRPL charges the fee to `Account` and has no co-signed fee payer. The
     `Sponsor` amendment (XLS-68) "is not enabled on Mainnet" (merged `scheme_upto_xrpl.md`). So a
     payer always pays its own create and fund fees **and reserve**, unlike Solana's sponsor seat,
     and there is no facilitator relay as on EVM (ADR 0076). CONFIRMED-DOCS.
- **Parity with the EVM and SVM bindings.** Cumulative voucher, receiver lands at any time,
  delay-protected payer exit, receiver-side seal: **all present natively**. Missing:
  - per-channel receiver authorizer separate from destination (only the destination account claims);
  - partial withdrawal;
  - a rent or sponsor payer.
    **Inference:** the XRPL lifecycle is closer to Solana `payment-channels` than to EVM, and a
    `PayerExit::Close` run of `batch::contract` plausibly passes after misfits 1–3 are handled.

### 2.3 Assets

- **The binding is XRP only.** #2952 §4.2: "Issued currencies, MPTs, LP tokens, NFTs … are
  outside this version". The asset is `xrpl:<id>/slip44:144`, and amounts are drops strings that
  fit u64 (1 XRP = 10⁶ drops). CONFIRMED-DOCS.
- **This is a ledger limit, not a spec choice.**
  - `PayChannel` and `PaymentChannelClaim` amounts are "XRP, in drops".
  - The merged `upto` spec says "the ledger rejects an issued-currency amount with `temBAD_AMOUNT`".
  - The token amendment, **`TokenEscrow` (XLS-85, Final; replaces XLS-34 "paychan-escrow-for-tokens")**,
    extends **Escrow only**. [XLS-85](https://xls.xrpl.org/xls/XLS-0085-token-escrow.html) never
    mentions PayChannel. Press coverage reports it active on mainnet since 2026-02-12, which is
    UNCONFIRMED against `feature` RPC.
  - CONFIRMED-DOCS.
  - **There is no stablecoin payment channel on XRPL today.**
- **Stablecoins that exist on XRPL**, usable only through `exact`, `upto`-escrow or Escrow, not
  through a channel:
  - **RLUSD** (Ripple): IOU currency `524C555344…` (hex "RLUSD"). Mainnet issuer
    `rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De`, testnet issuer `rQhWct2fv4Vc4KRjRgMrxa8xPN9Zx9iLKV`.
    x402 lists it with `decimals: 15` (x402 `typescript/packages/mechanisms/xrpl/src/defaultAssets.ts`).
    CONFIRMED-CODE. **Inference:** "15" is the IOU's significant-digit precision. IOU amounts are
    decimal floats (54-bit mantissa, exponent), not fixed-point integers, which matters to an
    `AssetId` and u128 base-unit model if tokens ever arrive.
  - **USDC**: Circle reports native USDC live on XRPL mainnet since 2025-06-12
    ([Circle](https://www.circle.com/blog/now-available-usdc-on-the-xrpl)). CONFIRMED-DOCS. Its
    issuer address was not checked against Circle's docs: UNCONFIRMED.
- **Inference:** a TOON XRPL peering is **XRP-denominated**. Combined with USDC peerings it needs an
  ADR 0071 `[[tokens]]` dealing boundary at a declared XRP/USD rate, with real FX risk for the
  dealer. It is not a dollar leg.

### 2.4 Rust

- **`xrpl-rust`** ([crates.io](https://crates.io/crates/xrpl-rust)):
  - `1.3.0`, released 2026-08-18; ~98k downloads, ~37k recent.
  - Now under the **XRPLF** org (XRP Ledger Foundation), ISC licence. Pushed 2026-10-27, 45 stars.
    Top contributors are LimpidCrypto (223 commits) and sephynox (105).
  - `no_std`-capable, with tokio JSON-RPC and WebSocket clients.
  - Has models for `PaymentChannelCreate`, `PaymentChannelFund`, `PaymentChannelClaim` and
    `channel_authorize`, the claim encoder, and secp256k1 (`secp256k1 0.30`) and Ed25519
    (`ed25519-dalek 2.1`) key derivation and signing.
  - Its own CI runs **`xrpld --standalone` in Docker** for integration tests.
  - CONFIRMED-CODE.
- **Alternatives.** `xrpl_types`, `xrpl_binary_codec` (`0.16.7`) and `xrpl_sdk_jsonrpc` (gmosx) were
  last released 2023–2024. `xrpl` (`0.1.2`) was last released 2023. `ripple-keypairs` (`0.1.0`) was
  last released 2021. `xrpl-mithril` (`0.5.3`, 2026-03) has 249 downloads. CONFIRMED-DOCS (crates.io API).
- **Inference:** everything the backend needs is native Rust: claim signing and verification
  (~40 lines over `k256`/`ed25519-dalek` even without the crate), transaction binary encoding and
  signing, submit, and `ledger_entry` reads. **No non-Rust runtime and no sidecar.** One question
  is open: whether xrpl-rust's `secp256k1` (libsecp256k1 C bindings) is acceptable beside the
  workspace's existing secp256k1 stack. That was not checked.

### 2.5 Testing

- **A stand-alone genesis ledger:** `xrpld -a --start --conf=…`.
  - The genesis account is `rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh` with the public secret
    `snoPBrXtMeMyMHUVTgbuqAfg1SUTb`, holding 100 billion XRP. Funding is a plain Payment from it.
  - Ledgers **do not close by themselves**: the test calls `ledger_accept`.
  - `--start` enables every amendment the build supports.
  - CONFIRMED-DOCS ([stand-alone mode](https://xrpl.org/docs/infrastructure/testing-and-auditing/start-a-new-genesis-ledger-in-stand-alone-mode)).
- **Inference:** this is anvil-like. An `Xrpld::spawn` would fork `xrpld` on its own ports with a
  temp config and DB dir, close ledgers explicitly, and get deterministic confirmation.
- **Install for CI.** Prebuilt `xrpld` exists only for **Ubuntu/Debian and RHEL**, with no macOS
  package (build from source). Docker images `rippleci/xrpld:<tag>` are what xrpl-rust's CI uses.
  The binary was renamed from `rippled` to `xrpld`, and the repository is still `XRPLF/rippled`.
  CONFIRMED-DOCS, CODE. **Inference:** a GitHub Ubuntu runner can `apt install` it. A developer on
  macOS would need Docker, or the test would have to spawn a container. That would be the first
  container-backed `require_*` in this repository.
- **Startup time: UNCONFIRMED.** Neither binary is installed locally. xrpl-rust's CI polls
  `server_info` every 2 s for up to 120 s. **Inference:** stand-alone start is seconds, not
  minutes, but it was not measured.
- **The no-time-warp problem** (misfit 5) makes delay tests real-time.

### 2.6 Finality and cost

- **Finality.** Ledgers close every ~3–5 s. x402's XRPL mechanism uses 5 s
  (`DEFAULT_LEDGER_CLOSE_SECONDS`). A result is final once in a **validated** ledger: there are no
  reorgs and no confirmation depth. The spec requires a `tesSUCCESS` in a validated ledger (§11).
  - A bounded wait (ADR 0073 decision 5) comes natively from `LastLedgerSequence`: once the
    validated ledger passes it, the transaction can never land.
  - CONFIRMED-DOCS, CODE. **Inference:** this is cleaner than EVM's 180 s heuristic and matches
    Solana's blockhash expiry.
- **Cost** (mainnet `server_info`, 2026-10-28):
  - base fee **10 drops** (0.00001 XRP);
  - base reserve **1 XRP**;
  - owner reserve **0.2 XRP** per object. Each open `PayChannel` locks 0.2 XRP of the _source's_
    reserve until it is deleted.
  - CONFIRMED-CODE (live RPC).
- **Inference:** a peering's two channels cost each side one 0.2 XRP reserve plus fractions of a
  cent per transaction. This is the cheapest of all the chains here.

### 2.7 Fit for TOON

- **CAIP-2.**
  - `xrpl:<network_id>`, with `network_id` read from `server_info`: `xrpl:0` mainnet, `xrpl:1`
    testnet, `xrpl:2` devnet.
  - The [CAIP-2 profile](https://namespaces.chainagnostic.org/xrpl/caip2) is a Draft (2023-02-23),
    and x402 constants use the same values.
  - Identifiers must be canonical (`xrpl:1`, not `xrpl:01`).
  - Stand-alone mode reports some `network_id`, probably 0, which collides with mainnet: UNCONFIRMED.
  - CONFIRMED-DOCS, CODE.
- **Asset.** CAIP-19 `xrpl:0/slip44:144` for XRP. IOUs are `token:<currency>.<issuer>` and MPTs
  `xls33:<hex>`. CONFIRMED-DOCS. A TOON `AssetId` would be `xrpl:native` (by analogy with ADR
  0080's `mina:native`) and a future `xrpl:<currency>.<issuer>`. **Inference.**
- **`AssetChain`.** A new `Xrpl` variant beside `Evm` and `Solana`
  (`crates/connector-domain/src/asset.rs` L77). New per-chain variants for `ChannelPresentation`,
  `ReceiverTerms`, `OutboundChannelRecord` and `VoucherSigner::Xrpl([u8; 33])`. **Inference.**
- **Greeting entry.** `{scheme: "batch-settlement", network: "xrpl:0", asset: "xrpl:0/slip44:144",
payTo: <classic r-address>, extra: {binding: "xrpl-paychan-v1", minSettleDelay,
redemptionSafetySeconds, destinationTag?}}`. **Inference:** TOON would publish the PR's `extra`
  verbatim, mapping `minSettleDelay` onto the existing min-delay policy (default one day, ADR
  0074 §5).
- **Config sketch** (**Inference**):

  ```toml
  [settlement.xrpl]
  rpc_url = "wss://…"         # or JSON-RPC
  rpc_via_socks_proxy = false # ADR 0073 applies unchanged
  network_id = 0              # boot asserts server_info.network_id
  [settlement.xrpl.key]
  key_file = "settlement-xrpl.key"  # secp256k1 or ed25519 seed; pays, signs txs and claims
  [settlement.xrpl.batch_settlement]
  min_settle_delay_secs = 86400
  redemption_safety_secs = 60
  destination_tag = …         # optional
  ```

  There is no contract address or program id to pin: the binding is the protocol. Boot would
  instead assert that the `PayChan` amendment is enabled (it has been since 2020).

- **Wire vectors** (ADR 0021). Add `claim_voucher.xrpl`:
  - channel id derivation;
  - the `CLM\0` message;
  - one secp256k1 and one Ed25519 signature;
  - a `u64::MAX`-boundary refusal (mirroring `claim_voucher_evm_above_u64_max`);
  - an XRPL `voucher_claim_state_challenge`.

  `toon-client`, `rig` and `swap` would all need an XRPL signer. **Inference.**

- **Blockers.**
  1. **XRP only.** No stablecoin channel exists on the ledger, so every XRPL leg is an FX-exposed
     dealing boundary (ADR 0071). This is the chain's one hard limit. CONFIRMED-DOCS.
  2. **The spec is unmerged**, with no maintainer review in 2.5 months. Under #1558 TOON may build to
     it anyway. The parts TOON depends on (§4, §6.2, §11) are XRPL protocol facts, not spec choices,
     so the risk of building to an unmerged draft is unusually low. **Inference.**
  3. **The port needs XRPL semantics:** `Sealed` = deleted entry, reversible `Closing`, and a
     `CancelAfter` admission rule. These are suite review items, not a redesign. **Inference.**
  4. **The test chain is Linux-packaged or Docker only, with no clock warp.** Delay tests take real
     time. **Inference.**
  5. **Payers pay their own reserve and fees.** There is no sponsor or facilitator equivalent until
     XLS-68. CONFIRMED-DOCS.

---

## 3. Hedera (#3466)

**Sources pinned.** x402 `main` at [`261d0c5e`](https://github.com/x402-foundation/x402/tree/261d0c5e1b3d65c7d41839923399de39cd02cec1)
(2026-10-28). PR [#3466](https://github.com/x402-foundation/x402/pull/3466) head
[`8358eb89`](https://github.com/x402-foundation/x402/tree/8358eb89e3ba1c62e1e08f4360de9bc1e5f0a372),
base `04c750e3`. Cited as **H** plus a path at that head. HIPs from
[`hiero-ledger/hiero-improvement-proposals`](https://github.com/hiero-ledger/hiero-improvement-proposals)
(`main`, 2026-10-28). The HAS address is from [`hiero-ledger/hiero-contracts@5ade6c8c`](https://github.com/hiero-ledger/hiero-contracts/blob/5ade6c8c/contracts/account-service/HederaAccountService.sol)
(2026-09-09). On-chain facts come from the public mirror nodes, read 2026-10-28.

### 3.1 PR status

- **Author.** Ömer Furkan Yürük ([@oemerfurkan](https://github.com/oemerfurkan), profile company "ITU
  Blockchain"), an outside contributor working from a personal fork. He is not a Hedera/Hashgraph or
  x402 maintainer as far as the profile shows. Opened 2026-09-13. **One commit**, last touched
  2026-09-13. CONFIRMED-DOCS.
- **Review.** No reviews, no review requests and no maintainer comments. The only comment is Vercel's
  bot. `reviewDecision: REVIEW_REQUIRED`. `mergeable: CONFLICTING` (`mergeStateStatus: DIRTY`)
  against current `main`. Labels: `specs`, `typescript`, `sdk`, `examples`, `hedera`. CONFIRMED-DOCS.
- **CI.** Only `labeler` and `check-verified-commits` ran, and both passed. The Vercel status
  failed because it needs team authorization. No test or build workflow has run on the PR, since
  workflows on an outside fork wait for a maintainer to approve them. CONFIRMED-DOCS.
- **Scope: code and spec.** +18 485 / −42 across 109 files:
  - the spec `specs/schemes/batch-settlement/scheme_batch_settlement_hedera.md` (273 lines, v1.0 dated 2026-09-10);
  - two Solidity contracts under `contracts/evm/src/hedera/`;
  - Foundry tests in which the system contracts are mocked by `vm.etch`;
  - a full TypeScript mechanism (`@x402/hedera/batch-settlement/{client,server,facilitator}`);
  - three examples.

  No Go, Python or Rust code. CONFIRMED-CODE.

- **Context that raises the odds.** `@x402/hedera` already exists on `main` for the `exact` scheme
  ([`scheme_exact_hedera.md`](https://github.com/x402-foundation/x402/blob/261d0c5e1b3d65c7d41839923399de39cd02cec1/specs/schemes/exact/scheme_exact_hedera.md)).
  Its history shows maintainer work: `ae26f5e1` (2026-02-06), `d00e3881` by phdargen (2026-07-03,
  "Hardened Hedera facilitator verify") and `8228bd6d` (2026-09-24). So Hedera is a network x402
  already serves. CONFIRMED-CODE.
- **Deployed code.** The deployments run on **testnet only**:
  - escrow `x402BatchSettlementHedera` at `0.0.10463847` (`0x…009faa67`);
  - collector `0.0.10463851` (`0x…009faa6b`).

  Both are Sourcify-verified. The escrow was created **2026-09-10** by account `0.0.10463136`, and
  the escrow carries an **ECDSA admin key**. All of the last 100 contract results come from that one
  account, the last on 2026-09-24. Mainnet is "not deployed". So the code is **owned and deployed by
  the PR author**, not by x402 or Hedera. Unlike the EVM `0x4020…` contracts it is **not CREATE2**,
  so its address differs on every network. CONFIRMED-CODE
  ([mirror node](https://testnet.mirrornode.hedera.com/api/v1/contracts/0.0.10463847)).

- **Likelihood signals.** **Inference.** An x402 maintainer would have to own a forked, unaudited
  money contract and deploy it to mainnet. Nobody has engaged in the six weeks since the PR opened.
  Against that, there is precedent: Hedera `exact` was merged, and it was hardened by maintainers
  afterwards.

### 3.2 Mechanism

- **It is the EVM contract, forked.** A diff of `x402BatchSettlementHedera.sol` (617 lines) against
  upstream `x402BatchSettlement.sol` (587 lines at the PR's base) changes only the following:
  - **Four signature checks** (the claim-batch authorizer, the refund authorizer, and both voucher
    paths) now call `HederaSystemContracts.isAuthorizedRaw(signer, digest, sig)`. Upstream calls
    OpenZeppelin `ECDSA.recoverCalldata` / `SignatureChecker`. The EIP-1271 path is gone.
  - It adds a permissionless `associateToken(token)`, which calls HTS `associateToken` at `0x167`.
    HTS response codes 22 and 194 count as success.
  - Everything else is identical:
    - the structs (`uint128 balance/totalClaimed`, `pendingWithdrawal`, `refundNonce`);
    - `MIN_WITHDRAW_DELAY = 15 minutes` and `MAX = 30 days`;
    - `EIP712("x402 Batch Settlement", "1")`;
    - the `Voucher(bytes32 channelId,uint128 maxClaimableAmount)` typehash;
    - `deposit`, `claim`, `claimWithSignature`, `settle`, `initiateWithdraw`, `finalizeWithdraw`,
      `refund` and `refundWithSignature`;
    - every event, `WithdrawInitiated` included.

  CONFIRMED-CODE (H `contracts/evm/src/hedera/x402BatchSettlementHedera.sol`).

- **The system contract is HAS at `0x16a`** (HIP-632, `isAuthorizedRaw(address,bytes,bytes)`,
  selector `0xb2a31da4`). It is **not** `0x169`. Hedera's own "ECDSA vs ED25519" docs page says
  `0x167`, which is wrong: `0x167` is HTS, and `hiero-contracts` pins
  `HASPrecompileAddress = address(0x16a)`. HIP-632 is **Final** (release v0.56.7). The wrapper does a
  low-level call and maps any revert to `false`, because HAS reverts on a signature whose length does
  not match the key type. CONFIRMED-CODE.
- **HIP-632 semantics** ([HIP-632](https://github.com/hiero-ledger/hiero-improvement-proposals/blob/main/HIP/hip-632.md)).
  - A 65-byte signature is `ECRECOVER(digest)`. It is compared to the address when the address is
    an EVM-address alias. When the address is a long-zero account-num alias, it is compared to the
    alias, or to the address derived from the account's single ECDSA key.
  - A 64-byte signature is Ed25519 over the 32-byte digest, checked against the account's single
    Ed25519 key.
  - Threshold and key-list accounts fail.
  - The check reads the account's **current** key, so rotating a key invalidates its outstanding
    vouchers.

  CONFIRMED-DOCS.

- **Deposit** goes through a new `HederaAllowanceDepositCollector`, not ERC-3009 and not Permit2.
  The spec notes Permit2 is not deployed on Hedera mainnet.
  1. The payer grants an HTS allowance to the collector once (`AccountAllowanceApproveTransaction`).
  2. For each deposit, the payer signs a raw digest
     `keccak256(abi.encode(HederaAllowanceDeposit typehash, channelId, token, amount, nonce, deadline, collector, chainId))`.
  3. The collector checks that signature through HAS, marks the nonce used, and pulls the funds with
     HTS `transferFrom` at `0x167`.

  The spec attributes that pull to "HIP-906". HIP-906 is the _HBAR_ allowance proxy. HTS
  `transferFrom` is HIP-376 (Final, v0.26.0). The spec has the wrong citation. CONFIRMED-CODE, DOCS.

- **Voucher, claim, settle, refund, withdraw: as on EVM.**
  - The voucher is cumulative, with no nonce and no expiry.
  - `claimWithSignature` batches across channels and `settle(receiver, token)` is permissionless.
  - The cooperative refund uses `refundNonce`.
  - The payer's escape hatch is `initiateWithdraw` → `finalizeWithdraw` after `withdrawDelay`.
  - Signature digests are EIP-712 under the same domain name and version. Only `chainId` (295
    mainnet, 296 testnet, 298 for a local node) and `verifyingContract` differ.

  CONFIRMED-CODE.

- **Transport in the reference SDK.** The facilitator submits writes as HAPI
  `ContractExecuteTransaction` through `@hiero-ledger/sdk` 2.85.0. Reads go through the Mirror Node
  `contracts/call`, and viem is used only for ABI encoding and typed-data hashing. The contract does
  not care how it is called. HIP-410 (Final) also lets an ECDSA account call it with an Ethereum
  transaction through the JSON-RPC relay. CONFIRMED-CODE, DOCS.
- **Parity with the connector's port.** **Inference** from the diff and the port in
  `crates/connector-settlement/src/batch/port.rs`:
  - **What maps 1:1.** - `channel_state`: `balance`, `totalClaimed` and `pendingWithdrawal`, giving `landed =
totalClaimed` and `collateral = balance − totalClaimed − pending`. - Statuses `Open` and `Withdrawing`, never `Closing` or `Sealed`. This is the EVM half of the
    `batch::contract` suites with `PayerExit::Withdrawal`. - `land` is `claim`. `top_up` is another `deposit`. - `start_withdrawal` and `finish_withdrawal` are `initiateWithdraw` and `finalizeWithdraw`. - `sign_voucher` is the same EIP-712 digest, and `sign_claim_state_challenge` is the same domain. - Amounts are `u128` on chain.
  - **What does not map.**
    - **Deposit.** `batch_payer.rs` deposits only through `ERC3009DepositCollector` or
      `Permit2DepositCollector`, whose addresses are constants, and checks those through
      `get_code`. A Hedera payer needs a third route: a one-time HTS allowance, then a signed
      `HederaAllowanceDeposit` authorization with a payer nonce. The crash-safe "same authorization
      re-sent lands once" property (ADR 0075 decision 8) carries over, because the collector
      consumes the nonce.
    - **Contract address.** `connector_signer::X402_BATCH_SETTLEMENT_ADDRESS` is one constant, and
      the EVM backend refuses to boot without code there (`batch_settlement.rs:162`). The Hedera
      escrow lives elsewhere, per network, and its admin key could delete it.
    - **Voucher signer.** `VoucherSigner::Evm([u8;20])` is checked off-chain by ecrecover. That is
      exact for an ECDSA **EVM-alias** account, where HIP-632 step 2 is plain ecrecover. A long-zero
      ECDSA account, or any Ed25519 account, needs the account's key from the Mirror Node. Ed25519
      would need a new `VoucherSigner` arm (64-byte signature over the EIP-712 digest).
    - **HTS amounts.** HTS amounts are `int64`, so the spec refuses a deposit above `2^63−1`. That
      is a tighter cap than `u128`.
  - **Can `connector-settlement-evm` be reused?** Largely. Only the contract address, the deposit
    route and the off-chain signer rule differ, provided TOON's own settlement keys are ECDSA alias
    accounts and every voucher signer it admits is restricted to an alias account.

### 3.3 Assets

- **HBAR** is native, at **8 decimals** (tinybar). Through the JSON-RPC relay, `msg.value` and
  `gasPrice` are 18-decimal **weibars** (1 tinybar = 10¹⁰ weibar), but contract arguments are
  8-decimal ([docs](https://docs.hedera.com/hedera/core-concepts/smart-contracts/understanding-hederas-evm-differences-and-compatibility/for-evm-developers-migrating-to-hedera/decimal-handling-8-vs.-18-decimals)).
  The binding escrows **HTS tokens only**. It has no native-HBAR deposit path: `token` must be an
  HTS long-zero address. CONFIRMED-DOCS, CODE.
- **USDC is native, issued by Circle as an HTS token.**
  - Mainnet `0.0.456858`: "USD Coin", **6 decimals**, treasury `0.0.439909`, with a **freeze key**
    and a supply key, and no pause, wipe or KYC key. Total supply 450 010 110.000000 on 2026-10-28.
  - Testnet `0.0.429274`, the token the PR's examples use.

  CONFIRMED-CODE ([mirror](https://mainnet.mirrornode.hedera.com/api/v1/tokens/0.0.456858)).

- **Association.** An HTS token must be associated before an account can hold it, unless the
  account has free auto-association slots. The escrow, the receiver's `payTo` and a refunded payer
  must all be associated. `TokenAssociate` costs $0.05. HTS tokens have an ERC-20 facade (HIP-218,
  HIP-376) and facade associate (HIP-719), all Final. CONFIRMED-DOCS.
- RLUSD, USDM and other stablecoins on Hedera were not checked. UNCONFIRMED, out of the time budget.

### 3.4 Rust

- **The Hiero Rust SDK** is [`hiero-sdk`](https://crates.io/crates/hiero-sdk) **0.45.0** (2026-04-30;
  `main` is at 0.46.0, last push 2026-09-14). It was formerly the `hedera` crate, last released as
  0.43.0 on 2026-01-08. It is maintained by LaunchBadge, Swirlds Labs and the Hiero community, and
  is still pre-1.0. It does HAPI `ContractExecuteTransaction`, `AccountAllowanceApproveTransaction`
  and `TokenAssociateTransaction` natively, with Ed25519 and ECDSA keys. CONFIRMED-DOCS.
- **Alternatively, alloy through the JSON-RPC relay** (hashio, or the relay a local node runs).
  This works for an ECDSA alias account sending Ethereum transactions (HIP-410).
  [Relay docs](https://github.com/hiero-ledger/hiero-json-rpc-relay/blob/main/docs/rpc-api.md)
  report:
  - `eth_getLogs` is "subject to Mirror Node query limits";
  - `txpool_*` needs explicit flags;
  - `eth_sendRawTransaction` "polls Mirror Node for confirmation";
  - `eth_getProof` and account methods are unsupported;
  - `stateRoot` is empty.

  CONFIRMED-DOCS. **Inference.** The EVM backend's log watcher (`batch_watch.rs`, which watches for
  `WithdrawInitiated`) would run on mirror-backed logs that lag consensus by seconds. HTS
  association and allowance can also be done in EVM through the facade, so a pure alloy backend is
  plausible. UNCONFIRMED end to end.

- **No non-Rust runtime is needed.** Signing is EIP-712 plus secp256k1 or Ed25519. CONFIRMED-CODE.

### 3.5 Testing: a chain per test?

- **anvil cannot stand in.** It has no HAS at `0x16a` and no HTS at `0x167`, and the PR's own
  Foundry tests mock both with `vm.etch`. Such mocks are stubs, which ADR 0007 refuses as a test
  subject. [`hashgraph/hedera-forking`](https://github.com/hashgraph/hedera-forking) (last push
  2026-05-18) emulates **HTS only** at `0x167`, and only on an anvil or Hardhat **fork of a live
  network** (chain ids 295, 296 or 297). Its README does not cover HAS or `isAuthorizedRaw`. So it
  is neither hermetic nor complete. CONFIRMED-DOCS.
- **Hiero Local Node** (`@hashgraph/hedera-local`, a Docker Compose stack of a consensus node, a
  mirror node and the relay; chain id 298):
  - its README says it is **deprecated, to be archived September 2026**;
  - its last release was v2.40.2 (2026-05-12);
  - it needs at least 16 GB RAM, with Docker set to 6 CPUs and 8 GB;
  - it binds **fixed ports** (50211, 5551, 7546), so only **one instance per host** can run, not
    one per test.

  CONFIRMED-DOCS ([README](https://github.com/hiero-ledger/hiero-local-node)). Startup time was not
  measured. UNCONFIRMED.

- **The successor is [Solo](https://github.com/hiero-ledger/solo)** (v0.95.0, 2026-10-27, with
  monthly releases and LTS lines). It needs Kubernetes (kind) ≥ 1.32, Node ≥ 22, **≥ 12 GB memory
  and ≥ 6 cores**. CONFIRMED-DOCS. **Inference.** Nothing spawnable like `Anvil::spawn` exists.
  Tier-3 tests would share one long-lived Solo network per CI job, using fresh accounts per test.
  That would break CLAUDE.md's "own disposable chain per test" rule unless it is amended, and it
  would need a heavyweight CI runner.

### 3.6 Finality and cost

- **Finality.** Hashgraph is aBFT, and consensus is final "within seconds", deterministically, with
  no reorgs ([docs](https://docs.hedera.com/hedera/core-concepts/hashgraph-consensus-algorithms)).
  The commonly quoted figure of 3–5 s does not appear on that page. UNCONFIRMED for the exact
  number. A receipt or record is available once consensus is reached, and Mirror Node visibility
  lags by a few seconds more. CONFIRMED-DOCS.
- **Cost.** Fees are **fixed in USD** and charged in HBAR at the live rate
  ([fees](https://docs.hedera.com/hedera/networks/mainnet/fees)):
  - gas $0.0000000852 per unit;
  - `CryptoApproveAllowance` $0.05;
  - `TokenAssociate` $0.05;
  - `CryptoCreate` $0.05.

  The `ContractExecute` base fee is not on that page. UNCONFIRMED. CONFIRMED-DOCS.

### 3.7 Fit for TOON

- **CAIP-2.** The PR uses `hedera:mainnet` and `hedera:testnet`, matching `exact` on `main`, not
  `eip155:295`. Assets and `payTo` are entity ids (`0.0.x`) on the wire, while `ChannelConfig`
  carries EVM addresses (an alias, or long-zero for tokens and `receiver`). CONFIRMED-CODE.
- **Config and AssetChain.** **Inference.**
  - The greeting entry would be `{scheme: "batch-settlement", network: "hedera:mainnet", asset:
"0.0.456858", payTo: "0.0.x", extra: {assetTransferMethod: "hts-allowance", receiverAuthorizer,
withdrawDelay, minDeposit}}`.
  - `AssetId` for EVM is `evm:0x…`, with no chain id (`crates/connector-domain/src/asset.rs`), and
    there is **one** `[settlement.evm]` table. A Hedera leg alongside Base cannot share it. It needs
    `[settlement.hedera]` and `AssetChain::Hedera`. The `AssetId` would be spelled `hedera:0.0.456858`
    or by the long-zero address, a choice the spec must make.
  - The config needs `rpc_url` (relay) and/or a mirror-node URL, the network, the escrow and
    collector **addresses, unless they become per-network constants**, and the token.
- **Keys.** **Inference.** A secp256k1 `settlement.key` maps to an ECDSA alias account (HIP-583),
  which must exist and be funded with HBAR before first use. That is a new bring-up step.
- **Wire vectors.** The voucher bytes and EIP-712 digest are the EVM ones, with a different
  `chainId` and `verifyingContract`. That needs new `claim_voucher` and challenge vectors per
  network. Ed25519 Hedera vouchers would need a new vector family. **Inference.**
- **Blockers.**
  1. **The upstream binding is unmerged and unreviewed.** It is single-commit and has merge
     conflicts. Its contract is an unaudited fork, deployed only on testnet, under the author's
     admin key. Under ADR 0075 decision 1 as amended by #1558, TOON would have to bind to this
     draft, or deploy its own copy (a TOON-owned contract again).
  2. **No per-test chain.** anvil lacks HAS and HTS, the local node is deprecated and runs one
     instance per host, and Solo needs Kubernetes and 12 GB.
  3. **Collateral can be frozen.** USDC's issuer holds a **freeze key** that can freeze the
     escrow's HTS balance, as on EVM, where USDC blacklisting exists too.
  4. **New deposit route, and an `int64` amount cap.**

---

## 4. Bitcoin (#3544)

**Sources pinned.** x402 at [`261d0c5e`](https://github.com/x402-foundation/x402/tree/261d0c5e1b3d65c7d41839923399de39cd02cec1) (`main`, 2026-10-28). PR head [`davidshekunts-bit/x402-gobtc-pay@2c753c20`](https://github.com/davidshekunts-bit/x402-gobtc-pay/blob/2c753c2040dbfbcff554ca6a4d4bcdf0e6839f05/specs/schemes/batch-settlement/scheme_batch_settlement_bitcoin.md), cited as **BTC §n**. Merged Lightning spec [`specs/schemes/exact/scheme_exact_lnbtc.md@261d0c5e`](https://github.com/x402-foundation/x402/blob/261d0c5e1b3d65c7d41839923399de39cd02cec1/specs/schemes/exact/scheme_exact_lnbtc.md), cited as **LN**. Every source was read on 2026-10-28.

### 4.1 PR status

- **Shape.** The PR adds one file, `specs/schemes/batch-settlement/scheme_batch_settlement_bitcoin.md` (+423/−0), in one commit, `2c753c20`, dated 2026-09-21. It is labelled `specs`. It is spec only: no SDK code, no test vectors, and testnet and SDK implementations are explicitly out of scope (BTC §11). CONFIRMED-CODE.
- **Review.** The PR is not a draft, but `reviewDecision` is `REVIEW_REQUIRED` and `mergeable_state` is `blocked`. No reviewer or team has been requested and no review has been submitted. The only comment is Vercel's bot asking a Coinbase team member to authorise a preview. The timeline has no activity after the bot label on 2026-09-21. CONFIRMED-DOCS ([`gh api …/pulls/3544`](https://github.com/x402-foundation/x402/pull/3544), read 2026-10-28).
- **Author.** The account is `davidshekunts-bit` ("David Shekunts"). It was created 2026-08-11, has no company or bio, and its one public repository is the fork the PR comes from. The PR body says "Significant AI assistance was used to draft and revise this specification". Its Description and Tests sections are left as the empty template. CONFIRMED-DOCS.
- **No tracking issue.** None was found among x402's 3,795 issues and PRs. x402's `specs/CONTRIBUTING.md` asks for one before a spec is written. CONFIRMED-DOCS.
- **Who GoBTC Rails is.** The spec never says who operates it beyond "GoBTC Rails custody-assisted wallets" (BTC Summary). GoMining's **GoBTC Pay** has the same shape:
  - a 2-of-3 wallet with "Key 1 held by you … Key 2 held by GoMining, as co-signer … Key 3 held by an independent regulated custodian" ([GoMining Telegram](https://t.me/s/gomining/4448); the custodian is unnamed);
  - an "Approved" answer to the merchant within seconds, then batched L1 settlement through GoMining's own mining pool ([Analytics Insight](https://www.analyticsinsight.net/bitcoin/gobtc-pay-review));
  - a 0.2% merchant fee and an SDK/API launched 2026-06-19 with up to 10 initial merchants ([ValueTheMarkets](https://www.valuethemarkets.com/cryptocurrency/news/gomining-launches-gobtc-pay-sdk-and-api-for-seamless-bitcoin-transactions)).

  **Inference:** GoBTC Rails is GoMining's GoBTC Pay, or its x402 adapter. UNCONFIRMED: the PR never names GoMining, and no GoMining source mentions x402 or "Rails".

- **Deployed code.** No x402 code exists for this binding, at `261d0c5e` or in the PR. No on-chain program exists or is needed: the "channel" is an ordinary multisig wallet whose rules live in GoBTC's service. CONFIRMED-CODE.
- **Likelihood signals.**
  - Against: no maintainer engagement in 5 weeks.
  - Against: the spec covers one vendor's wallets only ("does not define a generic PSBT flow for arbitrary Bitcoin wallets or facilitators", BTC Summary) and says verification "is not facilitator-portable" (BTC §11).
  - For: x402 has merged single-vendor-shaped specs before, such as `_cloudflare.md`.

  **Inference:** the odds of merge are low and, either way, what would merge is a vendor integration.

### 4.2 Mechanism

- **Keys** (BTC §1). The wallet is 2-of-3 P2WSH:
  - K1 is held by the client;
  - K2 is held by "the GoBTC Rails co-signer";
  - K3 is held by a "recovery provider".

  A normal spend needs K1+K2, and a recovery exit uses K1+K3. The facilitator "stores the wallet descriptor, selects UTXOs, constructs PSBTs, serializes wallet operations, and retains the latest accepted commitment". CONFIRMED-CODE (spec text).

- **Signature scheme.** The spec says only BIP-174 PSBT and P2WSH. **Inference:** P2WSH is segwit v0, so the K1/K2 signatures are ECDSA over secp256k1 with BIP-143 sighashes. It is not Taproot or Schnorr: there are no Taproot, Schnorr or MuSig2 terms anywhere in the spec (grep). CONFIRMED-CODE for "P2WSH"; the curve is inference from the script type.
- **Deposit / open.** Nothing is opened. The payer funds its GoBTC 2-of-3 wallet address, and top-up is the same as funding. No channel id, receiver, delay or salt is fixed on chain. CONFIRMED-CODE (BTC §1, §4).
- **Voucher.** One payment is one K1-signed PSBT, built by the facilitator. The client "MUST NOT supply arbitrary inputs, outputs, fee policy, or change addresses" (BTC §4). Flow per payment:
  1. the client asks the facilitator for a preparation;
  2. the facilitator returns `{preparationId, unsignedPsbt, expiresAt}` (BTC §3.2);
  3. the client signs with K1;
  4. the client returns `{type:"payment", preparationId, signedPsbt}` (BTC §3.3).

  Signing a payment therefore always needs an online round trip to the facilitator first. CONFIRMED-CODE.

- **Cumulative semantics, partial.** A child PSBT "increase[s] the aggregate value assigned to `payTo` by exactly `amount` … relative to the accepted parent" and must preserve every unredeemed obligation (BTC §4, §7). The figure is cumulative only **within one unbroadcast batch**. Once a batch is broadcast, the next one starts from new UTXOs. The amount is fixed per request, and "Dynamic undercharging is not supported" (BTC §6). There is no signed cumulative figure that can be read back from chain, unlike EVM `maxClaimableAmount` or SVM `cumulative`. CONFIRMED-CODE.
- **Claim / settle.** A background worker adds K2 and broadcasts within `maxBatchDelaySeconds` of the oldest commitment (BTC §7). Only the K2 holder can do this. **The receiver has no way to redeem a commitment on its own.** If the facilitator does not broadcast, the commitment is "resolved under the facilitator's merchant guarantee" (BTC §7), which is an off-chain promise. CONFIRMED-CODE.
- **Withdraw / close / refund.** There are no timelocks: no CSV, CLTV, nLockTime, grace period or withdraw delay appears in the spec (grep). Normal withdrawal is K1+K2. The recovery exit is K1+K3, and the spec's only protection for the receiver against it is that the facilitator and recovery provider "MUST reconcile all accepted commitments", a promise between two services (BTC §8, §9 "Facilitator and Recovery Trust"). There is no refund operation. CONFIRMED-CODE.
- **Replay.** The `preparationId` is single-use, `/settle` is idempotent on identical bytes, and UTXOs are the final replay guard (BTC §8). CONFIRMED-CODE.
- **Parity with x402's EVM/SVM bindings.** It shares only the Commit → Accumulate → Redeem outline of `scheme_batch_settlement.md`. Missing:
  - escrow a contract enforces;
  - a receiver-unilateral claim;
  - a withdraw delay;
  - a `channelId`;
  - a `voucher{channelId, maxClaimableAmount}`;
  - `payerAuthorizer`.

  The trust model is declared "capital-backed" (BTC §1). **Inference:** the payee's security actually rests on the facilitator, as with the credit-backed model. CONFIRMED-CODE for the fields; the characterisation is inference.

- **Fit with the connector's port, method by method** (`crates/connector-settlement/src/batch/port.rs`):

  | Port item                                                                           | Fit                                                                                                                                                                |
  | ----------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
  | `admit` / `restore` (find a channel, read its fixed terms)                          | **No channel to find.** The "channel" is a wallet descriptor GoBTC keeps; `payTo` must be a registered GoBTC merchant                                              |
  | `channel_state.landed` (on-chain cumulative in receiver's favour)                   | **No such figure.** The nearest thing is the sum of confirmed outputs to `payTo` from the wallet, unrelated to any voucher's signed amount                         |
  | `channel_state.collateral`                                                          | Wallet UTXOs not reserved by GoBTC. Only the facilitator knows its reservations, so this cannot be read from the chain                                             |
  | `land(voucher)`: receiver sends one transaction, works on every status but `Sealed` | **Impossible without K2.** The receiver cannot finalise a K1-only PSBT. This is the decisive mismatch with ADR 0074 decision 5                                     |
  | `prepare_open` / `open_prepared` (idempotent, journaled)                            | Becomes "fund a GoBTC wallet"; there is no record to journal except the descriptor                                                                                 |
  | `top_up(increment)`                                                                 | Fits: a deposit to the wallet address                                                                                                                              |
  | `sign_voucher(cumulative)`, local and offline                                       | **Does not fit.** Each payment needs a facilitator-built PSBT, and signing is per payment delta, not per cumulative amount                                         |
  | `start_withdrawal` / `finish_withdrawal`                                            | K1+K2 (needs GoBTC) or K1+K3 (needs the recovery provider). No delay, so there is no `Withdrawing`/`Closing` window to land in                                     |
  | `sign_claim_state_challenge`                                                        | No counterpart; a K1 message signature could be defined by TOON                                                                                                    |
  | `BatchChannelStatus`                                                                | Only `Open` is meaningful; there is no `Withdrawing`, `Closing` or `Sealed` on chain                                                                               |
  | `batch::contract` suites, `PayerExit::{Withdrawal, Close}`                          | The suites assume the receiver can land inside the exit's delay. Neither exit here has a delay or lets the receiver land, so the suites **cannot pass unmodified** |

  **Inference** from the port text and BTC §1–§8.

- **Is any trust-minimised Bitcoin channel possible?** **Inference:** yes, and none of it is in #3544. A Spilman-style unidirectional channel needs only two keys:
  - a 2-of-2 funding output plus a CSV/CLTV refund path for the payer;
  - cumulative payment transactions the payer signs and the payee can finalise and broadcast alone.

  That matches x402's EVM model closely: cumulative amount, a receiver-unilateral claim, and a delay before the payer's exit. Gaps: top-up means a new funding output, and claiming closes the channel (Bitcoin cannot land part of a channel and keep it open without a new funding output). Such a binding would be TOON-owned under #1558's amendment. No such x402 proposal exists. UNCONFIRMED as an x402 artefact; the construction is standard Bitcoin practice.

### 4.3 Assets

- **BTC only.** The asset is `bip122:000000000019d6689c085ae165831e93/slip44:0` and amounts are satoshis (BTC §3.1). Bitcoin amounts are `i64` satoshis in Core and `u64` in rust-bitcoin's `Amount`. One BTC is 10⁸ sats, so BTC has 8 decimals and fits the port's `u128`. CONFIRMED-CODE (spec); the sat width is CONFIRMED-DOCS ([rust-bitcoin `Amount`](https://docs.rs/bitcoin/0.32/bitcoin/struct.Amount.html)).
- **No stablecoin in the binding.** BTC §11 says "Only Bitcoin mainnet and native BTC are defined". USDT on Bitcoin exists through Taproot Assets, announced by Tether and Lightning Labs on 2025-01-30 ([The Block](https://www.theblock.co/post/338109/tethers-usdt-stablecoin-returns-to-bitcoin-with-lightning-labs-partnership)). Secondary sources report it live on Lightning with Taproot Assets v0.7–v0.8 in 2026 ([Spark glossary](https://www.spark.money/glossary/taproot-assets)). UNCONFIRMED: there is no primary Tether or Lightning Labs confirmation of current mainnet volume. In any case, a Taproot Asset in a P2WSH multisig is outside this binding. **Inference:** a BTC peering crosses into USDC only at an ADR 0071 denomination boundary, with a declared rate. That is a volatile-asset rate, unlike every committed peering today.

### 4.4 Rust

All figures are from crates.io on 2026-10-28. CONFIRMED-DOCS.

- `bitcoin` 0.32.102 (2026-07-22, 18.4M downloads) handles transactions, PSBT (BIP-174), sighash and addresses.
- `secp256k1` 0.33.1 provides ECDSA and Schnorr. The `[signer]` key is already secp256k1, but ADR 0075 decision 3 forbids reusing it for value. A Bitcoin settlement key would be a separate secp256k1 file.
- `miniscript` 13.2.0 (2026-10-27) handles descriptors and the 2-of-3 or Spilman policy and PSBT finalisation.
- `bdk_wallet` 3.2.0 (2026-09-20) handles UTXO selection and wallet sync.
- RPC clients:
  - `corepc-client` 0.18.0 (2026-10-20) is the successor to `bitcoincore-rpc` 0.19.0, whose GitHub repo is **archived** (last push 2025-11-25);
  - `electrum-client` 0.25.2;
  - `esplora-client` 0.13.0.
- `corepc` has moved from GitHub to [git.rust-bitcoin.org](https://git.rust-bitcoin.org/rust-bitcoin/corepc); the GitHub copy says "no longer maintained here".
- Everything is native Rust; no non-Rust runtime is needed. CONFIRMED-DOCS. **But** #3544's flow requires an HTTP client of GoBTC's preparation and `/settle` API, a service and not a library. UNCONFIRMED: GoBTC's API is not public in the PR.

### 4.5 Testing: a chain per test

- The **`bitcoind` crate** 0.43.2 (corepc; 2026-10-21) is the direct counterpart of `Anvil::spawn`. Per its README (corepc [`bitcoind/README.md`](https://github.com/rust-bitcoin/corepc/blob/8d82c88a/bitcoind/README.md)), it:
  - runs `bitcoind -regtest` on OS-assigned ports in a temp datadir;
  - waits until RPC is ready;
  - kills the process on drop;
  - "every `#[test]` could easily run isolated with its own environment".

  It takes the binary from `BITCOIND_EXE`/`PATH`, or from a `download` feature that fetches and hash-checks a pinned Core release at build time. CONFIRMED-DOCS.

- **Inference:** a `require_bitcoind()` guard panicking under `CI` fits CLAUDE.md's rule. The `download` feature is a build-time network fetch, which the repo should probably refuse in favour of a CI install step (Core tarball or package).
- Blocks are mined on demand (`generatetoaddress`), so confirmation tests take no wall-clock time.
- **Startup time:** UNCONFIRMED. `bitcoind` is not installed on this machine, so it was not measured. **Inference:** regtest starts in roughly a second.
- **The binding's counterpart cannot be spawned.** #3544 defines mainnet only (BTC §11), and K2/K3 live in GoBTC's and the recovery provider's services. A tier-3 test could exercise only Bitcoin, never the binding. A TOON-owned Spilman binding would be fully testable on regtest. **Inference.**

### 4.6 Confirmation, finality, cost

- The block target is ~10 minutes. Finality is probabilistic, with 6 confirmations (~1 h) by convention ([developer.bitcoin.org](https://developer.bitcoin.org/devguide/block_chain.html)). #3544 leaves the threshold to "the facilitator's configured Bitcoin confirmation threshold" (BTC §7). CONFIRMED-DOCS.
- GoBTC Pay targets ~12 h average L1 settlement. UNCONFIRMED (secondary press, [ValueTheMarkets](https://www.valuethemarkets.com/cryptocurrency/news/gomining-launches-gobtc-pay-sdk-and-api-for-seamless-bitcoin-transactions)).
- Fees:
  - the network fee is payer-funded and comes from change only (BTC §1, §9);
  - fee rates are market-driven, typically a few hundred to a few thousand sats for a 2-of-3 P2WSH spend (UNCONFIRMED, varies with mempool);
  - GoBTC Pay adds a 0.2% merchant fee (UNCONFIRMED, press).

  **Inference:** a percentage merchant fee taken by a third party is value taken off a path without a peering `fee` saying so. CLAUDE.md calls that a change to TOON's business model.

- **ADR 0073 bounded waits.** A broadcast transaction has no expiry like Solana's blockhash, and RBF can replace it. **Inference:** the backend needs an explicit give-up rule and a by-txid reconciliation, as BTC §7 itself requires of the facilitator.

### 4.7 Fit for TOON

- **Greeting entry.** The CAIP-2 network is `bip122:000000000019d6689c085ae165831e93` and the asset `bip122:000000000019d6689c085ae165831e93/slip44:0` (BTC §3.1). Regtest's own reference would be the first 32 hex characters of its genesis hash, `0f9188f13cb7b2c71f2a335e3a4fc328`, which #3544 does not define. The CAIP-2 rule is CONFIRMED-DOCS; regtest support in the binding is absent.
- **AssetId.** `crates/connector-domain/src/asset.rs` today refuses `"bitcoin:…"` by name in a test (`a_deserialized_asset_is_refused_in_this_modules_own_words`). A Bitcoin backend would add `AssetChain::Bitcoin` and choose a spelling such as `bitcoin:native` (by analogy with ADR 0080's `mina:native`). CONFIRMED-CODE.
- **Config.** A `[settlement.bitcoin]` table would hold `rpc_url` (Core RPC or Esplora), `rpc_via_socks_proxy`, `key_file` (secp256k1), `decimals = 8`, and a confirmation depth. #3544 would add a facilitator URL and a GoBTC wallet/merchant id, an **account with a vendor**. **Inference.**
- **Wire vectors (ADR 0021).** `claim_voucher` would need a Bitcoin variant. Under #3544 a "voucher" is a whole PSBT built by a third party, so a deterministic vector would have to fix UTXOs, fee rate and descriptor. A Spilman voucher would be one signed transaction per cumulative amount, a fixed shape. **Inference.**
- **Blockers:**
  1. **A trusted third party in every peering.** Both nodes would need GoBTC accounts: the payer a GoBTC wallet, the payee a registered GoBTC merchant (BTC §1, §11). The receiver cannot redeem without GoBTC's K2, and the payer can leave through K1+K3. That contradicts ADR 0074 decision 5, that landing protects accepted value. It also contradicts the doctrine that an operator runs its own node with its own keys. CONFIRMED-CODE (spec) and DOCS (ADR).
  2. **The connector would call a facilitator.** Verification "MUST delegate … to the facilitator that created the preparation" (BTC §5), and every payment needs a preparation call. ADR 0076's falsifier forbids the connector calling `/verify` or `/settle`. CONFIRMED-CODE.
  3. **The port's suites cannot pass unmodified.** There is no `landed`, no receiver `land`, no exit delay, and no local voucher signing (§2 above). **Inference.**
  4. **No stablecoin.** BTC only, so every BTC peering is an ADR 0071 boundary at a volatile rate. CONFIRMED-CODE (BTC §11).
  5. **Mainnet only, unreviewed, single-vendor.** It cannot be tier-3 tested and it is unlikely to merge as written. CONFIRMED-DOCS.
- **What would fit instead.** **Inference.**
  - A **TOON-owned Spilman-style binding** of x402's outline, under #1558's amendment. It needs two keys, a CSV refund path, cumulative payee-finalisable transactions and regtest tests. It matches the EVM semantics except that a claim closes the channel and a top-up re-funds it.
  - **Lightning** is the other Bitcoin path x402 already serves. `exact` on `lnbtc` merged on 2026-09-23 ([#2861](https://github.com/x402-foundation/x402/pull/2861), LN). It is BOLT11 invoice plus preimage, upfront, per request, and its namespace is `lnbtc:000000000019d6689c085ae165831e93`. Related PRs are open: [#2262](https://github.com/x402-foundation/x402/pull/2262) (TS `bip122/exact` Lightning), [#3688](https://github.com/x402-foundation/x402/pull/3688) (`@x402/lnbtc`) and [#3727](https://github.com/x402-foundation/x402/pull/3727) (shared vectors).
  - Lightning channels are themselves trust-minimised bidirectional channels with HTLCs, the same primitive as ILP's conditions. They do not fit the batch-settlement port (no cumulative voucher; a node must run LDK or an LN daemon). `lightning` (LDK) 0.2.7 is native Rust, but running a Lightning node is a much larger surface than a settlement backend. A separate decision, not a Bitcoin batch-settlement backend.

---

## 5. Cardano (#3580)

Sources pinned: x402 [`261d0c5e`](https://github.com/x402-foundation/x402/tree/261d0c5e1b3d65c7d41839923399de39cd02cec1) (`main`, 2026-10-28); PR head [`75ad5e81`](https://github.com/x402-foundation/x402/blob/75ad5e81f44521ffd1cb8c294a6268183be3c811/specs/schemes/batch-settlement/scheme_batch_settlement_cardano.md) (spec v0.11, 2026-10-13); Subbit [`kompact-io/subbit-xyz@c45f4869`](https://github.com/kompact-io/subbit-xyz/tree/c45f4869) (`main`, 2026-10-07) and its `x402` branch at [`946d3e89`](https://github.com/kompact-io/subbit-xyz/commit/946d3e89) (2026-10-12); reference implementation [`loveaihq/subbit-x402@30b89d63`](https://github.com/loveaihq/subbit-x402/tree/30b89d63) (2026-10-12). Mainnet parameters read from Koios on 2026-10-28 (epoch 664, protocol major 11). Cited as **CSPEC** (the PR's spec), **SUBBIT**, **REF**.

### 5.1 PR status

- **#3580** "docs(specs): add batch-settlement binding for Cardano", by `loveaihq`. Opened 2026-09-25, last pushed 2026-10-12. It adds one file, +700 lines, **spec only**. Labels `specs`, `cardano`. **No reviews.** `reviewDecision` is REVIEW_REQUIRED. All 7 commits are signed now. A bot flagged an earlier unsigned commit (`a7a089e`) and warned the PR would be closed "after 1 week of inactivity"; it was not closed. CONFIRMED-DOCS.
- The author's account was created 2026-09-16, with 8 public repos. The PR body says "Most of the spec was drafted with an AI assistant (Claude)". CONFIRMED-DOCS.
- **The discussion is in issue [#3579](https://github.com/x402-foundation/x402/issues/3579)**, not on the PR. Thomas Kammerlocher (`Kammerlo`, Cardano Foundation) engaged in substance. He proposed the "no watcher" rules (verify-past-TTL plus idle claims), the author adopted them as spec v0.9, and Kammerlo replied: "Yes, from my point of view that solves it great". `waalge`, Subbit's author (Kompact.io), also took part. **No x402-foundation maintainer has commented** on either thread. CONFIRMED-DOCS.
- **Who maintains Cardano in x402.** The Cardano Foundation does: Fabian Bormann (`fabianbormann`) wrote the merged `exact` spec (#1093, merged 2026-04-23, `4840c8df`) and the TypeScript `@x402/cardano` mechanism (#2537, 2026-09-09, `42ee42bd`). `typescript/packages/mechanisms/cardano` exists at `261d0c5e`. The issue cc's both CF engineers. CONFIRMED-CODE.
- **On-chain code exists, but nobody in x402 owns it.** The channel is Subbit's Aiken validator `subbit.subbit.spend`. Its owner is Kompact.io (`@waalge`), and its funding is Project Catalyst Fund 13 ([milestones 1300174](https://milestones.projectcatalyst.io/projects/1300174); README: M1–M5 done). CONFIRMED-DOCS.
  - The validator is Apache-2.0 and Plutus V3, built with Aiken v1.1.24.
  - Script hash at `c45f4869` and on the `x402` branch: `6d8774631fece31d0c768afb960c9a8a9957f8a85fa31deae1159094`, which is the hash CSPEC names. The older build `62ce4309…` has a liveness defect, fixed in subbit-xyz#10.
  - Both CSPEC and SUBBIT's README say it is **alpha and unaudited**. CONFIRMED-CODE.
- **Deployment.** The validator is parameter-free, so "deploying" means publishing a reference-script UTxO. Anyone can do that, and nobody needs to own it. REF published one on **preprod** (deploy tx `544752f6…`; CSPEC's example `referenceScript` is `ff61e3f4…#0`). **No mainnet reference script or mainnet channel was found** in CSPEC or REF's `RESULTS.md`. UNCONFIRMED: nothing indexes channels by script hash, and only preprod was searched.
- **Two Cardano x402 dialects already diverge.** Subbit's own Rust stack has an x402 dialect on its `x402` branch (ADR `docs/adrs/x402.md`, 2026-10-07, status accepted). It says REF "isn't load-bearing", and it "diverges from the reference where it disagrees (notably, `Keytag`'s derivation)". It also has **no facilitator** ("erodes the trustlessness guarantee"). CSPEC requires a channel id derived from a spent input and defines a facilitator. CONFIRMED-CODE.
- **Likelihood signals.**
  - For: CF engineers engaged positively, and preprod evidence is thorough. REF's `RESULTS.md` records 19 steps and reconciles every run to the lovelace.
  - Against: no maintainer review in 5 weeks; a third-party, unaudited validator; and the upstream validator author ships a competing dialect.
  - **Inference**: merging is plausible but not near.

### 5.2 Mechanism

The channel is one UTxO at the validator, with an inline datum `[ownHash, Constants, Stage]`:

- `Constants = [tag, currency, iouKey (Ed25519 32 B), consumer keyhash, provider keyhash, closePeriod ms]`
- `Stage = Opened(subbed) | Closed(subbed, elapseAt) | Settled`

Source: SUBBIT `aiken/lib/subbit/types.ak`. CONFIRMED-CODE.

| Step                | Signer              | Effect                                                                    | x402 name                   | TOON port                                   |
| ------------------- | ------------------- | ------------------------------------------------------------------------- | --------------------------- | ------------------------------------------- |
| open (no script)    | anyone              | creates the UTxO at `Opened(0)`                                           | `deposit`                   | `prepare_open` / `open_prepared`            |
| `Add`               | consumer            | more currency, stage unchanged                                            | `deposit` with `channelRef` | `top_up`                                    |
| `Sub(owed, sig)`    | provider            | takes `new subbed − old subbed`, with `subbed ≤ owed`; channel stays open | `claim`                     | `land` while Open                           |
| `Close`             | consumer            | `Opened → Closed`; `elapseAt ≥ validity upper bound + closePeriod`        | (none)                      | `start_withdrawal`                          |
| `Settle(owed, sig)` | provider            | `Closed → Settled`, takes `≤ owed − subbed`                               | `claim`                     | `land` while Closing (terminal)             |
| `End`               | consumer            | `Settled →` gone                                                          | (none)                      | `finish_withdrawal`                         |
| `Elapse`            | consumer            | `Closed →` gone once `elapseAt` has passed                                | (none)                      | `finish_withdrawal` (receiver never landed) |
| `Mutual`            | consumer + provider | anything                                                                  | `refund` (the channel ends) | none                                        |

CONFIRMED-CODE (SUBBIT types) and CONFIRMED-DOCS (CSPEC "Steps").

- **Voucher (IOU).** An Ed25519 signature by `iouKey` over Plutus `serialiseData([tag, amount])`, which is the CBOR `0x9f ‖ bytes(tag) ‖ uint(amount) ‖ 0xff`. `amount` is **cumulative**. There is **no expiry and no nonce**, as on EVM. CSPEC carries a test vector (seed `07…07`, tag `11…11`, amount 203000). Ed25519 over a 40-odd-byte message is the same primitive as Solana's voucher. CONFIRMED-DOCS.
- **Channel id.**
  - x402 `channelId` is Subbit's 32-byte `tag`. CSPEC requires `tag = blake2b-256(CBOR of an input the opening spends)`, which makes it unique, and requires a facilitator to refuse anything else.
  - **The validator does not enforce uniqueness of tags**: "Subbit allows duplicates". The rule is the binding's own, and Subbit's own dialect derives the tag differently.
  - The id is stable, but the **position** (`channelRef = txHash#index`) moves with every `Add` and `Sub`. It has to be found by following spends.

  CONFIRMED-DOCS.

- **Batching.** One transaction may spend many channels. The first channel input carries `Main([step…])` and the others carry `Defer`, under one provider signature. A 10-channel claim cost 0.571 tADA on preprod. CSPEC estimates about 45 ADA channels per transaction before the 16 KiB size cap; that figure is extrapolated, not measured. CONFIRMED-DOCS.
- **`receiver` is not on chain.** The datum has no receiver, and `Sub` and `Settle` may pay anywhere: the `provider` key is the authority. That is why CSPEC's "facilitator as provider" mode is custody. CONFIRMED-DOCS.
- **Min-UTxO reserve.** Every output must hold `coins_per_utxo_size` (4,310 lovelace per byte) × its size.
  - Reserve: 1.73 tADA for an ADA channel, 2.13 tADA for a token channel.
  - Capacity: `subbed` plus what the channel can still pay out. In an ADA channel that is the lovelace minus the reserve.
  - A token payout output needs its own ADA (about 1.18 tADA on preprod), which the provider puts up.

  CONFIRMED-DOCS, with the protocol parameter CONFIRMED from Koios.

**Parity with EVM, SVM and the port.**

- **Fits, and maps closer to Solana than to EVM.**
  - **Status.** `Opened` maps to `Open`, `Closed` to `Closing` (no new vouchers; only the provider can still land) and `Settled` to `Sealed`. There is no `Withdrawing`: a payer cannot partially withdraw while the channel stays open. So the suites' `PayerExit::Close` (the Solana path) applies unchanged.
  - **Landing.** `land` sends `Sub` while Open and `Settle` while Closing, which is exactly Solana's `settle` and `settle_and_seal`.
  - **Figures.** `landed = subbed`, and `collateral = payable-out` while Opened and 0 otherwise.
  - **Finishing.** `finish_withdrawal` is `End` or `Elapse`, matching Solana's `distribute`.
  - **Voucher signer.** An Ed25519 32-byte key, the same shape as `VoucherSigner::Solana`.

  **Inference.**

- **Fits: the open is idempotent by construction.** The opening transaction runs no script and spends ordinary UTxOs. A signed opening journaled by `prepare_open` and resent by `open_prepared` can land **at most once**: its inputs can be spent only once. "Lapsed" means another transaction spent one of its inputs. This mirrors the Solana signed-transaction record. In TOON the payer can submit it itself, with no sponsor. **Inference** from the eUTXO rules and CSPEC "Pending openings".
- **Does not fit as-is: eUTXO contention on one UTxO.** Every `Sub`, `Add`, `Close` and `Settle` spends **the same channel UTxO**. A receiver's `land` therefore races the payer's `top_up` or `Close`, and the loser's transaction is invalid because its input is already spent. It costs no fee: phase-1 failure takes no collateral. A backend must re-read the channel and rebuild, and on a lost race against `Close` it must rebuild as `Settle`. CSPEC says so ("A claim that loses a race with the consumer's `Close` reads the channel again and settles it"). The port's "exactly one transaction" for `land` holds only per successful attempt. Neither EVM nor Solana has this, since both use account state. CONFIRMED-DOCS plus **Inference**.
- **Does not fit as-is: a moving position.** `ChannelPresentation` needs a Cardano variant carrying `channel` (the tag) **and** a last-known `channelRef`, and the backend must follow spends forward from it. `channel_state` cannot be a single keyed read the way an EVM `channels(id)` or a Solana account fetch is. It needs an index: an address query filtered by datum tag, or a spend lookup. **Inference**.
- **Does not fit as-is: admission must judge an unenforced rule.** Tag uniqueness exists only by the binding's derivation rule. Admission must check that the opening transaction spent an input that hashes to the tag, which means reading the **opening transaction**, not only the current UTxO. Otherwise two channels could share a tag under the same `iouKey`, and a voucher would replay across them; CSPEC says this risk falls on the consumer, never on the provider. **Inference**.
- **Phase-2 collateral risk.** Every script spend needs collateral: ADA-only UTxOs worth 150% of the fee (`collateral_percent` 150), forfeited if the script fails at phase 2. A backend must evaluate execution units locally before it submits. CONFIRMED (protocol parameter) plus **Inference**.
- **Delay floor.** CSPEC allows `withdrawDelay` (the close period) of 900 s to 30 days. That matches the SVM 900 s floor, and TOON's one-day default fits inside it. CONFIRMED-DOCS.
- **No on-chain event log.** A Close shows only as the UTxO being re-spent. The watcher in CSPEC is optional ("idle claims" plus `/verify` past `TTL`). TOON's sweeps would follow the validator address or poll each channel. CSPEC's measured settle came 30–62 s after the close with a 15 s poll. CONFIRMED-DOCS.
- **Refund.** x402's `refund` is a co-signed `Mutual` that ends the channel. The TOON port has no refund operation, and none is needed. CONFIRMED-DOCS.

### 5.3 Assets

- **ADA.** 6 decimals (lovelace). The total supply cap is 45×10^15 lovelace, so amounts fit in u64. Ledger `Coin` and multi-asset quantities are u64, while Aiken `Int` is unbounded, so the port's `u128` is wider than needed. Asset spelling is `lovelace`. CONFIRMED (Koios genesis `maxlovelacesupply`).
- **USDCx** is Circle's, through xReserve, not native USDC.
  - Live on mainnet since **2026-02-27** ([cardano.org](https://cardano.org/news/2026-02-27-USDCx-goes-live-on-mainnet/), [Circle](https://www.circle.com/blog/usdcx-on-cardano-now-available-via-circle-xreserve)).
  - Policy `1f3aec8bfe7ea4fe14c5f121e2a92e301afe414147860d557cac7e34`, asset name `5553444378` ("USDCx"). **6 decimals** per the token registry. Supply about 42.63 M. First minted 2026-02-18.
  - The minting policy is a 405-byte Plutus V3 script, and holders are ordinary key addresses.
  - A Cardano native token's policy governs only mint and burn, so a holder can lock it at any script, a Subbit channel included. CONFIRMED-CODE (Koios `policy_asset_info`, `script_info`, `asset_addresses`).
  - **Inference**: nothing in the token stops it being a channel currency, but no channel has been run in USDCx.
- **USDM** (Moneta). Policy `c48cbb3d5e57ed56e276bc45f99ab39abe94e6cd7ac39fb402da47ad`, asset name `0014df105553444d` (the CIP-68 333 label plus "USDM"). 6 decimals, supply about 11.52 M. On preprod it is `e675b46e…` / `0014df10745553444d` (tUSDM). This is the token x402's Cardano `exact` example uses. CSPEC ran a full channel in preprod tUSDM (REF step 10). CONFIRMED-CODE (Koios) and CONFIRMED-DOCS (`scheme_exact_cardano.md` L142).
- iUSD and DJED were not checked. UNCONFIRMED: out of time, and no x402 text mentions them.
- **Asset spelling for TOON.** Use `<policyId>.<assetNameHex>`, as x402 Cardano `exact` and CSPEC do. CONFIRMED-DOCS.

### 5.4 Rust

- **pallas** (txpipe, Apache-2.0): `pallas-primitives`, `-crypto`, `-network`, `-txbuilder`, `-traverse`, plus a `validate` module with phase-1 and phase-2 (`phase2` feature). Version **1.4.0** (2026-09-17). pallas-crypto has about 751 k downloads, and it underlies Dolos and Oura. CONFIRMED-CODE (crates.io, README).
- **`uplc`** (aiken-lang) **1.1.24** (2026-09-26) evaluates Plutus in Rust, for execution units and dry-running scripts. **`amaru-uplc`** 0.4.0 is the alternative, and Subbit's own stack uses it. CONFIRMED-CODE.
- **cardano-serialization-lib** (Emurgo, Rust) is at `18.0.0-beta.1` (2026-09-25) and is mainly consumed through wasm. CONFIRMED-CODE.
- **Subbit's own Rust stack** (`subbit-core`, `-tx`, `-server`, `-client`, `-index`, `-l1-dto`; MPL-2.0; edition 2024, `rust-version 1.99`) builds Subbit transactions natively in Rust. It depends on **git-branch** crates from `cardano-lightning/konduit` (`cardano-sdk`, `cardano-connector`, `cardano-wallet`), which are **unpublished** and pinned to `branch = "main"`. Its `x402` dialect is a WIP ("massive refactor"). CONFIRMED-CODE (`Cargo.toml` at `c45f4869`, `x402` branch).
- **Reading the chain.**
  - `cardano-node` serves no HTTP. Its node-to-client protocol, which answers UTxO-by-address queries, runs over a **local unix socket**. Node-to-node over TCP (chain-sync, tx-submission) has no UTxO query.
  - A remote `rpc_url` therefore means a **Blockfrost-compatible REST API**: hosted Blockfrost or Koios, or self-hosted **Dolos** (txpipe, Rust, v1.6.2 2026-10-21, "MiniBF (Blockfrost-compatible)", UTxO RPC gRPC). Ogmios and Kupo (Haskell) are the other choice.
  - REF uses Blockfrost or Koios. CONFIRMED-DOCS (Dolos README) plus **Inference** on n2c.
- **Non-Rust runtime: none required** for the connector. Signing, CBOR, transaction building and script evaluation are all available in Rust. The only external service is the chain index (Blockfrost, Koios or Dolos), which is an RPC endpoint like an EVM `rpc_url`, not a sidecar. **Inference**.
- **Keys.** A Cardano payment key is Ed25519, the same curve as Solana's, so one settlement key can be the `consumer`/`provider` key **and** the `iouKey`, which ADR 0075 decision 3 needs. CSPEC's HKDF derivation of IOU keys is "the client's own business". The provider and consumer identities are **blake2b-224 key hashes** of the key, not the key itself. CONFIRMED-DOCS.

### 5.5 Testing: a chain per test

- **Yaci DevKit** (bloxbean, MIT, v0.12.0, 2026-10-14) creates a devnet "in seconds".
  - It comes as Docker, a non-Docker `yaci-cli` zip (linux-x64, macos-arm64) and an npm package ("CI/CD Ready", with a [sample CI project](https://github.com/bloxbean/devkit-npm-ci-test)).
  - Default blocks are 1 s, and it supports **sub-second** blocks (`--block-time 0.2 --slot-length 0.2`).
  - It bundles Yaci Store (Blockfrost-compatible, :8080), Ogmios (:1337) and Kupo (:1442), and offers `topup <addr>` funding. The Yaci CLI is Java.
  - Startup time is UNCONFIRMED: the vendor says "seconds" and nothing was measured. CONFIRMED-DOCS (README).
- **Dolos** has a "Local devnet mode — ephemeral single-node network … builtin in-memory stores for ephemeral nodes and tests", in one Rust binary that serves MiniBF and UTxO RPC. **This is the closest analogue to `anvil`.** Whether it produces blocks and evaluates Plutus the way a real node does was not verified. UNCONFIRMED.
- **cardano-node** 11.1.3 (2026-09-29) ships static tarballs for linux amd64 and arm64. A single-node private network from a custom genesis is possible, but it still needs an index (Dolos, Yaci Store or Kupo) to answer UTxO queries over HTTP. CONFIRMED-CODE (release assets).
- **Subbit's own e2e tests use no chain.** They spawn `subbit-server` against a temporary database. REF's 119 tests are "chain-free", and its chain runs were on public preprod. CONFIRMED-CODE.
- **Inference.**
  - Per-test spawning is feasible: Dolos devnet, or Yaci's non-Docker CLI with sub-second blocks. Nothing yet shows a `require_*()`-style binary that boots in under 2 s with Plutus V3 evaluation.
  - Loading the validator is cheap: it is parameter-free, so a test publishes it from the pinned `plutus.json` (Aiken output) as a reference script or inline script.

### 5.6 Finality and cost

- **Blocks.** Slot length 1 s and `activeSlotCoeff` 0.05, so a block every **~20 s** on average. `securityparam` k = **2160**. CONFIRMED (Koios genesis).
  - Ouroboros Praos is probabilistic. A rollback deeper than k is excluded, and k blocks is about 12 h.
  - In practice x402 Cardano `exact` defaults to `l1Confirmations: 1` (range −1 to 20). REF keeps a channel anchor **3 blocks deep** to survive rollbacks.
  - On preprod one block gap was 111 s, and one deposit confirmed 2.5 min after submission.

  CONFIRMED-DOCS.

- **Peras (fast settlement) is not live.** It is CIP-140, "Proposed", targeting about 2 min settlement, with activation planned for **2027** as part of Dijkstra. UNCONFIRMED in primary sources: [CIP-140](https://cips.cardano.org/cip/CIP-140) and the [cardano.org Dijkstra glossary](https://cardano.org/glossary/dijkstra/) were read through search summaries only.
- **Fee formula.** `44 × bytes + 155,381` lovelace, plus execution units (0.0577 per mem unit, 0.0000721 per step), plus 15 lovelace per reference-script byte (scaled). Transactions are capped at 16,384 bytes. CONFIRMED (Koios epoch 664 parameters).
- **Measured on preprod, in tADA** (REF `RESULTS.md`):

  | Transaction                           | Cost       |
  | ------------------------------------- | ---------- |
  | open                                  | 0.175      |
  | top-up                                | about 0.26 |
  | claim (`Sub`)                         | 0.257      |
  | settle                                | 0.257      |
  | close                                 | 0.250      |
  | end                                   | 0.231      |
  | `Mutual`                              | 0.233      |
  | each extra channel in a batched claim | 0.035      |

  A unilateral exit (close, settle, end) costs 0.738 tADA. CONFIRMED-DOCS.

- **Reserves.** 1.73 to 2.13 ADA is locked per channel as min-UTxO and comes back at exit, plus about 1 ADA of min-UTxO on each payout output. CONFIRMED-DOCS.
- **Bounded waits (ADR 0073).** A transaction can carry a validity upper bound (`invalid_hereafter`), which gives a hard expiry like Solana's blockhash. Index lag (Blockfrost trailing by about 20 s) means "not seen" is not "not landed"; CSPEC requires answering `settlement_pending` and never re-broadcasting a different transaction. CONFIRMED-DOCS.

### 5.7 Fit for TOON

- **Greeting `network`.** `cardano:mainnet` / `cardano:preprod` / `cardano:preview`, with the CIP-34 forms (`cip34:1-764824073`, `cip34:0-1`, `cip34:0-2`) accepted as aliases. **Neither `cardano` nor `cip34` is a registered CASA namespace.** `ChainAgnostic/namespaces@463bae56` has no Cardano directory, and x402's `exact` spec says "no strictly 'canonical CAIP-2' form exists". This is the first chain where TOON's CAIP-2 id would follow x402's convention rather than a registry. CONFIRMED-CODE.
- **`AssetChain::Cardano`** with the spelling `"cardano"`. The `AssetId` is `cardano:lovelace` or `cardano:<policyId>.<assetNameHex>`. The 28-byte policy plus up to 32 bytes of name is longer than an EVM address. ADR 0071's `AssetId` parser needs that form. **Inference**.
- **`[settlement.cardano]`** would hold:
  - `rpc_url`, a Blockfrost-compatible endpoint, with `rpc_via_socks_proxy`;
  - `key` (an Ed25519 key file);
  - `token` (`lovelace` or `policy.name`) and `decimals`;
  - `network`.

  Under `batch_settlement` it would hold `min_close_period_secs` (≥ 900) and a reserve or min-deposit figure. The validator hash would be a **binary constant**, as ADR 0075 makes x402's addresses. But CSPEC makes `scriptHash` server-advertised and lets clients trust several, and Subbit's ADR says the same. **Inference**.

- **Port changes.** It needs new variants: `ChannelPresentation::Cardano { channel, channel_ref }`, `ReceiverTerms::Cardano { provider_key_hash, pay_to, currency, min_close_period, reference_script }`, `OutboundChannelRecord::Cardano` (the signed opening transaction's bytes plus the tag), and a `VoucherSigner` Ed25519 variant (or the Solana one generalised). `land` needs a documented retry on a spent input. The status union and `PayerExit::Close` need no change. **Inference**.
- **Wire vectors (ADR 0021).** A Cardano `claim_voucher` vector: an Ed25519 signature over CBOR `[tag, amount]`, from CSPEC's test vector. A `voucher_claim_state_challenge` under a TOON domain separator. Network and asset spelling vectors. **Inference**.
- **Blockers.**
  1. **The on-chain code is third-party, alpha and unaudited** (Subbit, Kompact.io). Binding to it is closer to binding to solana-foundation `payment-channels` than to x402's own contract, but nobody has audited it.
  2. **The binding is unmerged and contested.** It has no maintainer review, and Subbit's own x402 dialect derives the channel id differently. TOON would be building to one of two drafts. (#1558's amendment covers a TOON-owned binding, not a third party's unmerged one.)
  3. **eUTXO single-UTxO contention** forces retry semantics into `land` and `top_up` that the `batch::contract` suites do not yet express.
  4. **Channel lookup needs a chain index.** A raw node is not enough, so the RPC model becomes a Blockfrost-compatible API.
  5. **No mainnet deployment or USDCx channel has been run.**
  6. **The per-test chain is not proven.** Dolos devnet and Yaci are candidates, and neither has been shown with Plutus V3 evaluation and a boot time under a few seconds.

---

## 6. Not chains: Cloudflare and the credit network binding (#2612)

- **`_cloudflare.md`** (395 lines, network `cloudflare:402`, last touched by `8fb28ceb`, #2438,
  2026-05-26). It came in with the scheme itself in
  [#1145](https://github.com/x402-foundation/x402/pull/1145) (CameronWhiteside; opened 2026-02-10,
  merged 2026-04-15 by CarsonRoscoe). CONFIRMED-DOCS.
  - It is **credit-backed** (L9): the client signs the HTTP request with **RFC 9421 HTTP Message
    Signatures** (Web Bot Auth, keys at `.well-known/http-message-signatures-directory`), and the
    signature agent is registered with Cloudflare against a billing identity.
  - `asset: "USD"` (ISO 4217, amounts in cents, L108–110).
  - Settlement: "The network (Cloudflare) acts as Merchant of Record … distributing revenue to
    content owners on a periodic basis through traditional off-chain financial rails" (L231–233).
  - Settlement periods, failure handling and disputes are "established during registration … not
    part of the x402 protocol" (L294–320). Registration is a human process (L271–276).
    CONFIRMED-DOCS.
  - **Not a chain.** There is no escrow, nothing for a receiver to land, and no key the connector
    could hold to redeem. The commitment is not a value instrument: it is an authenticated request
    plus a billing contract with Cloudflare. A connector cannot bind to it under ADR 0074 decision 5
    (value is protected only by landing on chain) or ADR 0075 decision 1. **Inference.**
- **#2612, the credit network binding**
  ([PR](https://github.com/x402-foundation/x402/pull/2612); orgtom78, **draft**; opened
  2026-06-12, last activity 2026-08-04; `REVIEW_REQUIRED`; one file, +360). CONFIRMED-DOCS.
  - It proposes `scheme_batch_settlement_credit.md`. A **bundle authority** sells fiat credit
    bundles through a PSP (Stripe, Adyen) and issues a short-lived **JWS attestation** per request.
    Its keys are at `{extra.authority}/.well-known/jwks.json`.
  - The authority **atomically decrements** the balance at `/authorize`. Settlement to the server is
    off-chain on a schedule.
  - The network id is `<authority>:credit`; the author asks whether it should be
    `credit:<authority>` instead.
  - `/settle` is idempotent on `jti`, and `holdExp` bounds a reservation.
  - The trust anchor is "the bundle authority, in the same trust position as a PSP".
  - Comments: the signed-commit bot flagged unverified commit `d81c85e`, and threatened closure
    after a week of inactivity. Two third parties: javimosch (the `peage` fiat rail) and sunsetpatrol
    (idempotency layers). There is **no maintainer comment**. The author notes the shipped
    `@delegare/x402` "implements the on-chain `exact` scheme … not this binding's attestation flow
    yet".
  - **How it differs from the capital-backed bindings:** there is no collateral, no channel and no
    on-chain redemption. The seller's guarantee is a contract with the authority, and a voucher is
    a bearer attestation an intermediary can revoke. **It does not belong in TOON's settlement
    port.** Nothing can be landed, so ADR 0074 decision 5's protection is undefined. ADR 0008 says no
    shared secret moves value, and here a third party's ledger moves it. TOON's "no protocol fee,
    flat per-peering fee" model also assumes on-chain money. **Inference.**
- Adjacent, one line each (CONFIRMED-DOCS):
  - [#3620](https://github.com/x402-foundation/x402/issues/3620) (gazoy, 2026-09-29): a
    **pooled-escrow** binding (one operator funds a crew of agents), plus a spending-policy
    extension. Only third-party comments; no binding PR.
  - [#3435](https://github.com/x402-foundation/x402/issues/3435) (Nicoiakl / Nyx5, 2026-09-09): x402
    over a **non-blockchain double-entry ledger**. A discussion, no binding; zero comments.
  - [#3447](https://github.com/x402-foundation/x402/issues/3447) (goun7, 2026-09-10): a draft
    "external-ledger-binding" field block, for hash-chained **work-receipt** ledgers. It is
    receipts, not settlement.

---

## 7. Other chains: interest without a binding PR

None of these has an open `batch-settlement` binding PR in `x402-foundation/x402`. All
CONFIRMED-DOCS unless marked.

- **TRON: the binding exists outside x402, not in an upstream PR.**
  - Tracking issue [#2075](https://github.com/x402-foundation/x402/issues/2075) (boboliu-1010 /
    BankofAI, open since 2026-04-20) lists "TRON batch-settlement specification merged" and
    "implementation merged" as unticked milestones.
  - On 2026-08-28 the author said #2076 had been rebuilt to include `batch-settlement`. As of
    2026-10-28, [#2076](https://github.com/x402-foundation/x402/pull/2076) touches **only**
    `scheme_exact.md` and `scheme_exact_tron.md`. The 2026-10-08 comment: "#2076 … covers `exact`
    only. Other schemes … will be proposed separately." No maintainer has commented.
  - The binding lives in the fork:
    [`BofAI/x402` `scheme_batch_settlement_tron.md`](https://github.com/BofAI/x402/blob/main/specs/schemes/batch-settlement/scheme_batch_settlement_tron.md),
    166 lines, last commit `b74e829e` (2026-09-02). It is a **port of the EVM contract**: TIP-712
    domain `{ name: "x402 Batch Settlement", version: "1", chainId, verifyingContract }`, the same
    `ChannelConfig`, `withdrawDelay` 900–2,592,000 s, and ERC-3009 and Permit2 collectors.
  - Its contracts are **deployed and owned by BankofAI**, at non-canonical per-network addresses:
    mainnet `tron:728126428` channel `TW9yNhTySkEHYfjnGQU2u4NAsdb1tW4fbm`, Nile `tron:3448148188`
    and Shasta `tron:2494104990`. The author says they "return deployed bytecode". UNCONFIRMED by
    this note.
  - **Inference.** This is the closest non-EVM/SVM binding to the existing EVM backend:
    secp256k1, TIP-712 ≈ EIP-712, and the same contract semantics. But it is not x402's code, and
    there is no upstream PR.
- **Stellar** ([#3341](https://github.com/x402-foundation/x402/issues/3341), Eras256, 2026-09-01;
  zero maintainer comments).
  - A voucher authorised the way Stellar `upto` does it (`require_auth_for_args`, PR
    [#3098](https://github.com/x402-foundation/x402/pull/3098), the same author, open,
    `REVIEW_REQUIRED`) is **host-capped**.
  - The cap: `signatureExpirationLedger ≤ current + max_entry_ttl − 1`, and testnet's
    `max_entry_ttl` is 3,110,400 ledgers, about **180 days**. So an open channel's voucher could
    expire. The issue cites `rs-soroban-env` `auth.rs` `verify_and_consume_nonce`.
  - The proposed fix is a Soroban contract that verifies a raw Ed25519 voucher with
    `env.crypto().ed25519_verify`, with no expiry, exactly as SVM does.
  - The precedent is [`stellar-experimental/one-way-channel`](https://github.com/stellar-experimental/one-way-channel)
    (the contract behind `stellar-mpp-sdk`'s Channel mode). Last commit `25dea1b3`, 2026-07-29;
    4 stars; "experimental".
  - Its design: XDR `Commitment{domain, network, channel, amount}` signed by a dedicated Ed25519
    `commitment_key`, and `close_start` → `refund_waiting_period` → `refund`. **Its contract is
    deployed per channel** (the key is "set at channel deployment"). The issue says this, and the
    note did not re-read the contract: UNCONFIRMED.
  - No binding spec or deployment exists in x402. The author: "isn't in this project's own roadmap
    yet".
- **Kaspa** ([#3645](https://github.com/x402-foundation/x402/issues/3645), elldeeone, 2026-10-01;
  zero comments).
  - It proposes `exact` and a `batch-settlement` built on a **covenant UTXO channel** (KIP-10
    introspection, KIP-20 covenant ids): cumulative vouchers, **partial claims**, jointly authorised
    top-ups and timed refunds.
  - There is a TypeScript reference in [`elldeeone/kaspa-x402`](https://github.com/elldeeone/kaspa-x402),
    with Testnet-10 evidence: "does not establish mainnet readiness".
  - The CAIP-2 namespace is pending at ChainAgnostic/namespaces#193.
  - The author plans a separate batch-settlement spec PR, which is **not yet opened**.
- **Older, not batch-settlement bindings.**
  - [#1302](https://github.com/x402-foundation/x402/pull/1302) "scheme deferred Dhali" (desimmons,
    the XRPL #2952 author; open since 2026-02-22, last activity 2026-05-03; bot comments only). A
    separate `dhali` scheme over Dhali's channels (XRPL, Xahau, Ethereum; USDC, RLUSD, XRP). It is
    superseded in practice by #2952. **Inference.**
  - [#1338](https://github.com/x402-foundation/x402/pull/1338) and
    [#1909](https://github.com/x402-foundation/x402/issues/1909) (pr0toshi): a separate
    `statechannel` scheme (hub and direct profiles). Open and stale since 2026-04 and 2026-05, with
    no maintainer review.
- **Aptos, Sui, TON, NEAR, Algorand, Starknet, Cosmos, Polkadot, Stacks, Concordium.** A grep of
  every x402 issue and PR title and body (3,795 items, fetched 2026-10-28) finds **`exact`-scheme
  work only** for each, and no batch-settlement or channel binding. Examples: Sui #3081/#3082,
  TON #1455/#1583 (merged), Starknet #2849 (merged), Cosmos #3788, NEAR #2948/#3370, Algorand
  (AVM) fixes. CONFIRMED-DOCS.
- **`coinbase/x402`** (now a fork of x402-foundation, pushed 2026-10-27) has **no open PR**
  mentioning batch, channel, deferred or voucher. CONFIRMED-DOCS.

---

## 8. What a third chain touches in this repository

At `HEAD` `9a039468`. CONFIRMED-CODE.

- **The port's per-chain enums each gain a variant**
  ([`port.rs`](../../crates/connector-settlement/src/batch/port.rs)): `VoucherSigner` (`Evm([u8;20])`,
  `Solana([u8;32])`), `ChannelPresentation`, `ReceiverTerms` (plus a new `*ReceiverTerms` struct),
  and `OutboundChannelRecord` (its versioned `encode`/`decode` is 21 matches in `port.rs`).
  `BatchChannelStatus` may need a state, and `chain()` returns the `&'static str` `"evm"` or
  `"solana"`. Exhaustive matches on these, by file:
  - `ChannelPresentation::`: `in_memory.rs` 10, `port.rs` 7, `outbound_voucher.rs` 6,
    `connector-cli/src/batch_settlement.rs` 6, `connector-vectors` 4, `connector-runtime/connector.rs` 4,
    `client-edge/btp.rs` 4.
  - `ReceiverTerms::`: `in_memory.rs` 7, `runtime/batch_channels.rs` 6, `contract.rs` 5, `port.rs` 4,
    `peering.rs` 2, `client-edge/outbound_ledger.rs` 2.
  - `VoucherSigner::`: `runtime/voucher_binding.rs` 8, `runtime/peering.rs` 8,
    `runtime/batch_channels.rs` 6, `client-edge/btp.rs` 6, `client-edge/claim_gate.rs` 5,
    `client-edge/peer.rs` 4, `runtime/connector.rs` 3, `cli/batch_settlement.rs` 3.
- **The claim wire**
  ([`connector-domain/src/client_claim.rs`](../../crates/connector-domain/src/client_claim.rs)):
  - `ClientClaim` is `EvmVoucher | SolanaVoucher` (L112–115). `parse_client_claim` dispatches on
    `blockchain` `"evm"` or `"solana"`, and anything else is `unsupported blockchain` (L651–656).
  - **`blockchain: "mina"` is refused by name** before anything else (L646–648), with a message
    pointing at ADR 0002 and "the TypeScript fleet" (L291–295). A returning Mina must remove or
    rewrite it.
  - The namespace constants are `EVM_NAMESPACE` and `SOLANA_NAMESPACE` (L189, L193). The journal's
    channel keys are namespaced `"<chain>:<id>"` (`canonical_channel_key`).
- **Signature checks** ([`connector-signer/src/voucher_signature.rs`](../../crates/connector-signer/src/voucher_signature.rs)):
  `verify_evm_voucher` (L252) and `verify_solana_voucher` (L283). A third curve or message format is
  a third function, and possibly a new key type in `connector-signer` (secp256k1 and Ed25519 are
  what it holds today).
- **Config** ([`connector-config/src/settlement.rs`](../../crates/connector-config/src/settlement.rs),
  [`batch_settlement.rs`](../../crates/connector-config/src/batch_settlement.rs)): `SettlementChain`
  `Evm | Solana`, with string mapping at L159–160 and L200–201. Each chain has its own
  `*SettlementConfig` (`deny_unknown_fields`) and `*BatchSettlementConfig` (EVM
  `min_withdraw_delay_secs` and the EIP-712 name and version; Solana `min_grace_period_secs` and
  `min_sponsored_deposit`).
- **Asset id** ([`connector-domain/src/asset.rs`](../../crates/connector-domain/src/asset.rs) L77–92):
  `AssetChain` `Evm | Solana`, which ADR 0071 spells `evm:<addr>` / `solana:<mint>`. It is matched in
  `rate_poller.rs` (6), `config/settlement.rs` (4), `config/denomination.rs` (4) and
  `cli/runtime.rs` (3). The rate source is EVM-only (`connector-rate-source-evm`), so a third chain
  either has no rate source or brings one.
- **Greeting and self-description**
  ([`connector-domain/src/x402.rs`](../../crates/connector-domain/src/x402.rs)): the per-chain enums
  are `X402BatchSettlementTerms` (L269), `…EvmTerms`/`…SolanaTerms` (L283, L385),
  `X402BatchSettlementExtra` (L470) and `…EvmExtra`/`…SolanaExtra` (L485, L519). The network is
  CAIP-2: `eip155:<chainId>` comes from the chain (`cli/runtime.rs` L896), and Solana's is
  `solana:<genesis-hash[..32]>` (`connector-settlement-solana/src/lib.rs` L66–82).
- **Other surfaces.**
  - `connector-chain-rpc` has `evm.rs` and `solana.rs`. ADR 0073 needs a third chain's every client
    built from the one `RpcTransport`, with bounded waits.
  - The handler header `X-TOON-Chain` carries `"evm"`/`"solana"` (`runtime/attribution.rs` L57).
  - The boot "x402 not deployed" error is per table (`RuntimeError::X402NotDeployed`).
  - The removed-key error text names both constants (`connector-config/src/error.rs` L730–737).
- **Vectors** (`vectors/wire-vectors.json`): `claim_voucher` is keyed per chain (`evm`,
  `evm_above_u64_max`, `solana`, plus chain-neutral `amount_only_watermark` and `invalid`).
  `payout_voucher` and `voucher_claim_state_challenge` are `{evm, solana}`, and
  `client_auth_channel_challenge` is `{now, max_lifetime_secs, evm, solana}`. A third chain adds a key
  to each, and ADR 0021 makes those normative for `toon-client`, `rig` and `swap`.
- **The suites**
  ([`contract.rs`](../../crates/connector-settlement/src/batch/contract.rs)). There is **no
  `PayerExit` in the suite itself**: `PayerExit::{Withdrawal, Close}` parameterises only the
  in-memory fake (`in_memory.rs` L17–25). The suite takes fixture closures (`open`, `deposit`,
  `begin_exit`, `sign`, `let_delay_pass`, `restart`) and asserts only what holds on both chains.
  - **Universal, and binding on a third chain:**
    - `minimum_delay_secs > 901`, and a channel 1 s short is refused by name.
    - Two channels from one payer are both admitted.
    - **Partial landing while the channel stays `Open`**: land 300 of 1,000, leaving collateral 700
      and status `Open`.
    - Stale replays (300, 200) refused as `StaleVoucher`.
    - 1,001 refused as `VoucherExceedsDeposit`, then **landing after a top-up of the same channel**.
    - Collateral read from the chain with no notification.
    - An exit makes `collateral = 0` with status `Withdrawing | Closing`, and **the held voucher still
      lands during the exit**.
    - The paying half: idempotent `open_prepared` across two simulated crashes, a watermark that
      refuses non-advancing and unbacked vouchers, `WithdrawalNotDue` inside the delay, the payer
      refunded exactly `deposit − landed` (1,500 − 400 = 1,100), and `finish_withdrawal` once only.
  - **Chain-specific, by necessity:** the status names in the exit assertions, and the
    `in_another_token` / `for_the_other_chain` / `record_for_the_other_chain` helpers, which match
    on the two variants.
  - **What other shapes need. Inference.**
    - A chain whose receiver claims **partially without closing**, with a payer-started timed exit,
      fits the suite as written. XRPL `PaymentChannelClaim` and the Kaspa proposal's "partial
      claims" are this shape. It needs new variants, not new assertions.
    - A **close-only** chain fails "land 300 and stay `Open` with collateral 700", unless the
      backend defers landing to the exit. A 2-of-3 PSBT or any channel whose only redemption is a
      settling close is this shape.
    - Cardano #3580 has a partial land (`Sub`) and fits this step (§5.2). Bitcoin #3544 has no receiver land at all (§4.2).
    - If close-only, the suite's contract (and ADR 0074 decision 5's "landing protects value") must
      be amended, or such a chain must keep a voucher's protection off-chain until close.

---

## Unconfirmed, and why

The chain sections carry their own UNCONFIRMED tags; these are the ones a spec would most likely lean on.

- **Start-up time of every candidate test chain.** None of `xrpld`, `bitcoind`, Yaci DevKit, Dolos or
  Solo is installed on this machine, so none was measured. The per-test-chain judgements in §2.5,
  §3.5, §4.5 and §5.5 rest on the projects' own documents.
- **A standalone `xrpld`'s `network_id`.** Probably 0, which would collide with livenet's CAIP-2 id
  (§2.7). Not checked.
- **Whether `xrpld` standalone can move its clock.** No time-setting parameter on `ledger_accept` was
  found (§2.5). If none exists, close-path tests wait out a real `SettleDelay`.
- **Whether deleting Hedera's testnet escrow (its admin key is set) could move escrowed balances.**
  Not tested (§3.1).
- **Whether Circle can burn or freeze USDCx on Cardano.** The minting policy's source was not read
  (§5.3).
- **Who GoBTC Rails is.** Very probably GoMining's GoBTC Pay, by name and design; the PR never says
  (§4.1).
- **Whether any x402 maintainer intends to review any of the four.** No maintainer has commented on
  any of them; that is a fact about the threads, not about intent.

---

## What a multi-chain spec must decide

Each item gives the options and the facts that bear on them. This note recommends none.

### D1. Building to unmerged bindings, and to whose code **(blocker in the records)**

ADR 0075 decision 1 admits only x402's own deployed code. #1558 (owner, 2026-10-28) amends it so that
"a chain x402 doesn't yet serve may use a **TOON-owned** binding of x402's outline, proposed upstream".
That amendment was written for Mina, where TOON writes the spec and the code. It does not obviously
cover the pending bindings, because each is **someone else's** draft, and three of them put the money
code in a third place:

| Chain   | Whose spec                                                          | Whose on-chain code                                           | Covered by decision 1 as amended?                                              |
| ------- | ------------------------------------------------------------------- | ------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| XRPL    | Dhali's draft                                                       | None: the ledger's own primitive                              | **Inference:** closest; the only open question is building to an unmerged spec |
| Hedera  | One contributor's draft                                             | That contributor's fork, deployed only by them, admin key set | No: neither x402's nor TOON's                                                  |
| Cardano | One contributor's draft; a second dialect by the validator's author | Kompact.io's Subbit, alpha, unaudited                         | No: neither x402's nor TOON's                                                  |
| Bitcoin | One contributor's draft                                             | GoBTC Rails' service                                          | No, and the trust model fails ADR 0074 decision 5 regardless                   |

- **(a)** Build to the open PR as it stands, and track it. Field names may move: XRPL `upto`'s wire was
  reshaped by maintainers this week (#3950, #4008), and #2952 would plausibly be asked to align (§2.1).
- **(b)** Treat each as a TOON-owned binding under #1558: TOON adopts (or forks) the draft, pins the
  on-chain code itself (Hedera: TOON deploys the PR's contract; Cardano: pin Subbit's script hash),
  and owns its review.
- **(c)** Wait for a merge per chain.
- **(d)** Extend decision 1 to "x402-proposed bindings over protocol-native primitives" only, which
  admits XRPL and nothing else today.

### D2. Which on-chain identity becomes a constant of the binary

Decision 1 makes the EVM address and the Solana program id constants, never config, because a voucher
binds them. Per chain: XRPL has nothing to pin (the claim binds only `channelId`, and **no network
id**, §2.2); Hedera's address is per network (HAPI-created, not CREATE2), so the constant becomes a
table keyed by chain id 295/296 (§3.2); Cardano's would be Subbit's script hash, plus a reference-script
UTxO per network (§5.7); Bitcoin has nothing to pin.

### D3. Port and contract-suite changes **(the engineering crux)**

The suites assert only what holds on **both** chains today (§8). Each candidate adds something:

| Fact                               | XRPL                                          | Hedera                                                           | Cardano                                 | Bitcoin #3544     |
| ---------------------------------- | --------------------------------------------- | ---------------------------------------------------------------- | --------------------------------------- | ----------------- |
| Partial land, channel stays `Open` | Yes (`PaymentChannelClaim` without `tfClose`) | Yes (`claim`)                                                    | Yes (`Sub`), but the channel UTxO moves | **No**            |
| Payer exit shape                   | Close (`tfClose` → `SettleDelay`)             | Withdrawal (as EVM)                                              | Close (`Close` → `closePeriod`)         | None with a delay |
| Exit reversible                    | **Yes** (`tfRenew`)                           | No                                                               | No                                      | —                 |
| Terminal state                     | **Entry deleted**                             | None (as EVM)                                                    | **UTxO gone** after End/Elapse/Mutual   | —                 |
| Second hard deadline               | **`CancelAfter`** (immutable)                 | No                                                               | No                                      | No                |
| Concurrency                        | Account sequence                              | Nonce (as EVM)                                                   | **One UTxO: top-up races land**         | UTXOs             |
| Amount width                       | u64 drops                                     | u128, HTS `int64`                                                | u64                                     | u64 sats          |
| New `VoucherSigner`                | `Xrpl([u8;33])`                               | `Evm` for alias accounts; Ed25519 needs a mirror-node key lookup | Ed25519, new message                    | —                 |

Options:

- **(a)** Grow `BatchChannelStatus` and the suites to the union (a reversible `Closing`, a "gone"
  terminal distinguished from `ChannelNotFound`, a rebuild-on-spent-input rule). ADR 0007 and ADR 0075
  decision 2 require every implementation to pass the suites unmodified, so this is a suite change
  applied to all chains at once.
- **(b)** Keep the suites as the intersection and add chain-only tests beside them.
- **(c)** Admit only chains whose lifecycle already fits (partial land, exit with a delay), refusing
  the rest structurally. Bitcoin #3544 fails this test; a TOON-owned Spilman-style Bitcoin binding
  would fit except that landing closes (§4.2).

`PayerExit` is a property of the in-memory fake, not of the suite (§8); a third exit shape would
start there.

### D4. More than one EVM-family network per node

`[settlement.evm]` and `SettlementChain::Evm` are single, and `AssetId` `evm:0x…` carries no chain id
(ADR 0071). That blocks two things at once: a node on Base **and** another canonical-deployment chain
(§1), and Hedera sharing the EVM backend (§3.7).

- **(a)** A generalised multi-EVM table keyed by chain id, with `AssetId` gaining the chain id.
- **(b)** A separate `[settlement.hedera]` table and `AssetChain::Hedera`, leaving `[settlement.evm]`
  single.
- Either is a config-breaking change under ADR 0009, and a wire change if `AssetId` moves.

### D5. Assets and denomination

Every peering committed today is USDC on both legs, so no forward crosses a rate except under
`dealing` (ADR 0071). Of the four: Hedera has **native Circle USDC**; Cardano has USDCx (Circle-backed
through xReserve, not native) and USDM; **XRPL channels and Bitcoin #3544 are native-coin only**, so
every forward that crosses one is a dealing boundary at a volatile declared rate. The spec must decide
whether a native-coin-only chain is acceptable as a peering leg at all, and what `AssetId` spelling a
native coin takes (`xrpl:native` following ADR 0080's `mina:native`, or CAIP-19 `slip44:144`).

### D6. Per-test chains **(blocker under CLAUDE.md for Hedera, cost for Cardano)**

CLAUDE.md: tests spawn their own disposable chain per test, and a missing binary fails CI.

- XRPL: `xrpld` standalone fits the pattern (apt package or Docker image).
- Bitcoin: `bitcoind -regtest` fits it directly (the `bitcoind` crate mirrors `Anvil::spawn`).
- Cardano: a node **plus** an indexer, in Java (Yaci) or Rust (Dolos); feasible, heavier, unmeasured.
- Hedera: **nothing** spawnable per test runs the real system contracts. The PR's own tests stub them
  with `vm.etch`, which ADR 0007 refuses as a test subject.

The spec decides whether a chain that cannot meet the rule is admitted with a shared per-job chain
(a doctrine change), with tests only against a public testnet (CI then depends on a third party, the
opposite of the rule that keeps `onion` off the gate), or not at all.

### D7. Clocks in tests

The suites' delay steps assume a movable clock (anvil) or a skippable delay (Solana's seal), and the
floor is 901 s. XRPL standalone has no clock warp found (UNCONFIRMED); Cardano's devnets run real
slots. Options: a shorter test-only floor per chain, real waiting, or chain-specific exit tests.

### D8. Confirmation and bounded waits (ADR 0073 decision 5)

Each backend needs a finite wait and a "retry not known safe" report. XRPL's `LastLedgerSequence` and
a signed `PaymentChannelCreate` at an explicit sequence give it cleanly (§2.2, §2.6). Hedera reads lag
by the mirror node, and `eth_sendRawTransaction` polls it (§3.4). Cardano has long-tailed blocks (gaps
over 100 s seen on preprod), rollbacks to survive, and probabilistic finality until Peras (2027
target) (§5.6). Bitcoin has no expiry at all.

### D9. Extra endpoints per table

ADR 0073 builds every client of a table's `rpc_url` from one `RpcTransport`. Cardano needs an indexer
API (Blockfrost-compatible, Kupo/Ogmios) as well as, or instead of, a node (§5.4); Hedera wants a
mirror-node URL beside the relay (§3.7). Each extra endpoint must ride the same transport, and
`rpc_via_socks_proxy`, or be refused.

### D10. Keys

ADR 0075 decision 3: one settlement key per chain pays, signs transactions and signs vouchers. New key
files: XRPL secp256k1 or Ed25519 (a family-seed format is a choice), Cardano Ed25519 (raw seed vs
`cardano-cli` TextEnvelope), Hedera ECDSA **alias** account, which must already exist on chain and
hold HBAR. XRPL claims bind no network id, so the same key on testnet and mainnet produces replayable
claims across them; x402's own XRPL `upto` spec says to use separate accounts per network (§2.2).

### D11. Network ids, greeting entries and wire vectors (ADR 0021)

- CAIP-2: XRPL `xrpl:0`/`xrpl:1` (registered namespace); Hedera `hedera:mainnet`/`hedera:testnet`;
  Cardano `cardano:mainnet` / `cip34:1-764824073`, **neither registered** at ChainAgnostic; Bitcoin
  `bip122:…`. Local test networks have no x402 name on any of them; Solana derives its id from the
  genesis hash, and each new backend needs an equivalent rule.
- Vectors: `claim_voucher`, `payout_voucher`, `voucher_claim_state_challenge` and
  `client_auth_channel_challenge` gain a key per chain, with a `schema_version` bump, and `toon-client`,
  `rig` and `swap` must follow. The claim-state challenge message is TOON-defined on every non-EVM
  chain.
- The client claim parser refuses `"mina"` by name today (`ClientClaimError::Mina`); Mina's return has
  to retire that, and any new chain is `unsupported blockchain` until added (§8).

### D12. The facilitator (ADR 0076)

ADR 0076 is EVM-specific: the payer deposits through a facilitator the operator names. None of the
pending bindings has that role in the same form: XRPL has no fee sponsorship (XLS-68 not enabled), so
each party pays its own fees; Cardano's facilitator only broadcasts; Hedera's is the HAPI fee payer;
Bitcoin #3544's facilitator holds a key. The greeting's `facilitator` field would be EVM-only, or
generalised. Separately, EVM v1.1's facilitator-managed voucher mode (§1) is opt-in and does not
affect TOON's self-managed channels.

### D13. Ordering criteria

Facts a prioritisation can be made from, per chain: whether on-chain code exists and who owns it
(D1, D2); whether the lifecycle fits the suites unmodified (D3); whether a stablecoin rides the
channel (D5); whether a disposable per-test chain exists (D6); upstream momentum (review state,
adjacent merged schemes); and Rust tooling maturity. Mina is ordered separately by #1548 and is gated
on its prover (MINA-NOTE).

---

## Readiness, chain by chain

Ranked by how few of D1–D12 each one leaves open. Facts, not a recommendation.

0. **More EVM chains with the canonical deployment** (not a pending binding). Base, Arbitrum,
   Optimism, Polygon, Avalanche, Celo, Linea, Unichain, World Chain, Monad (§1). x402's own code at
   its constant address; the existing backend and suites apply unchanged. **Open:** D4 only (one EVM
   chain per node; `AssetId` carries no chain id).
1. **XRPL (#2952).** No on-chain code to own, audit or deploy; native Rust tooling under the XRPL
   Foundation; a per-test chain that fits the CLAUDE.md pattern; deterministic finality; partial land
   and a close-with-delay exit, the port's Solana shape. **Open:** XRP-only channels (D5); an
   unreviewed, stalled spec whose sibling `upto` wire is being reshaped (D1); a reversible close,
   deletion as terminal and `CancelAfter` (D3); no clock warp in tests (D7); network-unbound claims
   (D10).
2. **Hedera (#3466).** The closest mechanism to the built EVM backend (same contract semantics, same
   voucher digest), and native Circle USDC. **Open:** the contract is unreviewed and exists only as
   its author's testnet deploy with an admin key (D1, D2); no disposable test chain runs HAS/HTS (D6);
   per-network addresses, a third deposit route and the one-EVM-table model (D2, D4); Ed25519 signers
   (D3).
3. **Cardano (#3580).** A Solana-shaped lifecycle, Ed25519 cumulative IOUs, idempotent pre-signed
   opens, real stablecoins (USDCx, USDM) and native Rust tooling. **Open:** a third-party, alpha,
   unaudited validator and two incompatible x402 dialects (D1, D2); eUTXO contention and channels that
   vanish (D3); an indexer as a second endpoint (D9); a heavier per-test chain (D6); ~2 ADA locked per
   channel and slow probabilistic finality (D8).
4. **Bitcoin (#3544).** Not implementable as written: the receiver cannot land without a vendor's key,
   the payer can exit through a recovery key without the receiver, every voucher needs the vendor's
   facilitator (ADR 0076's falsifier), and both peers would need vendor accounts. Bitcoin itself tests
   easily on regtest; a TOON-owned Spilman-style binding would be a separate proposal under #1558
   (§4.2).

**Not ranked, no open binding PR:** TRON (an EVM-contract port in BankofAI's fork, deployed at TRON
addresses, no upstream PR), Stellar (issue #3341 only; a Soroban channel contract would be needed),
Kaspa (issue #3645 only), and Aptos, Sui, TON, NEAR, Algorand, Starknet, Cosmos, Polkadot,
Concordium (`exact` only) (§7). **Not chains:** Cloudflare and the credit network binding (§6).
