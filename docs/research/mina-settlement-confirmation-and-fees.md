# Mina settlement: confirmation, throughput and fees

> **Research note, not a decision record.** It decides nothing. It serves
> [#1560](https://github.com/toon-protocol/connector/issues/1560) (_What "confirmed" means on Mina,
> and whether Mina can carry settlement traffic_), part of #1548. It checks the six decisions the
> owner accepted in round 1 of that grilling against primary sources, gives each a verdict, and
> lists the facts round 2 still needs. It builds on
> [`mina-x402-batch-settlement.md`](mina-x402-batch-settlement.md) (#1549, cited as **PRIOR**),
> does not repeat it, and corrects it where the code or the chain disagrees. Written 2026-10-28.
> Every source was read that day, and every live query was made between 14:20 and 14:40 UTC.

**Evidence tags.** CONFIRMED-CODE means read in source at a pinned commit. CONFIRMED-DOCS means
read in the maintainer's own documents, MIPs or release notes. CONFIRMED-LIVE means read from
Mina mainnet over GraphQL that day. UNCONFIRMED means none of those, and says why. A sentence
marked **Inference** is this note's reasoning from confirmed facts, not a fact.

**Sources pinned.**

- **Mina daemon**: tag [`4.0.3`](https://github.com/MinaProtocol/mina/tree/4.0.3) (`73134bfd`,
  2026-10-22, the current Mesa release, the same tag PRIOR pins). Cited as **MINA** plus a path.
  `develop` and `compatible` were checked for the two values that matter, and they agree.
- **o1js**: [`a428f9b9`](https://github.com/o1-labs/o1js/tree/a428f9b97817177ef4b5868cf148c0ac67a1048d)
  (`main`, 2026-10-27, the same commit PRIOR pins). Cited as **O1JS** plus a path under `src/lib/mina/v1/`.
- **MIPs**: [`MinaProtocol/MIPs@5aa77c89`](https://github.com/MinaProtocol/MIPs/tree/5aa77c8911e7af39ff9c594175226b171ed8166d):
  MIP-6 (90 s slots) and MIP-9 (account-update limit).
- **Release notes**: [`3.5.0-mainnet-stop-slot`](https://github.com/MinaProtocol/mina/releases/tag/3.5.0-mainnet-stop-slot)
  and [`4.0.0-mainnet-mesa`](https://github.com/MinaProtocol/mina/releases/tag/4.0.0-mainnet-mesa) (published 2026-09-03).
- **Docs**: [Lifecycle of a payment](https://docs.minaprotocol.com/mina-protocol/lifecycle-of-a-payment).
- **Live mainnet**: the public daemon GraphQL at `https://api.minascan.io/node/mainnet/v1/graphql`
  and its Archive-Node-API at `https://api.minascan.io/archive/mainnet/v1/graphql`. Both are run by a
  third party, not by o1Labs or the Mina Foundation. The 290-block sample runs from height 566,668
  (2026-10-27 16:22 UTC) to 566,957 (2026-10-28 14:24 UTC).

MINA links below are `https://github.com/MinaProtocol/mina/blob/4.0.3/<path>#L…`. O1JS links are
`https://github.com/o1-labs/o1js/blob/a428f9b9…/src/lib/mina/v1/<file>#L…`.

---

## Verdicts

| #   | Round-1 decision                                         | Verdict                                                                                                                                                                                                                                                                                                                                              |
| --- | -------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | "Landed" at inclusion, "counted" at 15 blocks deep       | **Holds with caveat:** 15 blocks is the docs' number. It is about **1 hour** on today's mainnet, not 30 min. Depth is computed by the client; no endpoint reports it. `transactionStatus` stops knowing a command once it is 290 deep. An included command can be **included and failed** and still charge its fee                                      |
| 2   | No credit before collateral counts (~30 min bring-up)    | **Holds with caveat:** the wait is ~63 min expected at the observed slot fill (~36%), not 30 min. The docs' 30 min assumes 75% of slots fill                                                                                                                                                                                                       |
| 3   | Top-ups stay manual                                      | **Holds.** Several deposits into one zkApp in one block conflict only if each one also asserts the zkApp's current state. A command that loses that race is included as **failed**, and its fee is still charged                                                                                                                                 |
| 4   | Mina withdraw-delay floor 6 h (default 1 day)            | **Holds with caveat:** a zkApp can only measure slots, never seconds. One slot's length changed at Mesa (180 s → 90 s), and the change halved every slot-denominated delay. The Mesa upgrade itself stopped transactions for **8 hours**, longer than the 6-hour floor. o1js's `network.timestamp` is off by ~998 days on mainnet today (**Inference**) |
| 5   | Daily sweep instead of every 10 min                      | **Holds.** The cap is **12** zkApp commands per block, set by the block producer, not by consensus (consensus caps a block at 128). Realised capacity is ~2.9 per minute network-wide, not 8. Live demand today is 0.08 per block                                                                                                                  |
| 6   | Each node pays its own fees; no facilitator              | **Holds.** Any key can be the fee payer. The pool ranks commands by fee per account update. No fee-estimation API exists. Mainnet zkApp fees in the sample were 0.012 or 0.1 MINA. The 1 MINA account-creation fee is **burned**, and nothing found refunds it                                                                                     |

---

## 1. Two levels of "confirmed"

### 1.1 What the sources say about depth

- Docs: "The recommended wait time for a transaction to be confirmed is 15 blocks", which "provides a 99.9% confidence that the transaction will not be reversed". The table assumes "a slot duration of 90 seconds and … 90% honest stake". Its "Average time is calculated based on consensus constants that determine the number of slots filled per epoch. This is currently set to 75%." ([Lifecycle of a payment](https://docs.minaprotocol.com/mina-protocol/lifecycle-of-a-payment)). CONFIRMED-DOCS.
- The 75% is the VRF's `f`: "f determines the fraction of slots that will have blocks if c = 2^0", `let f = Bignum.(of_int 3 / of_int 4)` (MINA [`consensus/vrf/consensus_vrf.ml#L287-L288`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/consensus/vrf/consensus_vrf.ml#L287-L288)). The comment says it is a probability under full stake participation, not a guarantee. CONFIRMED-CODE.
- o1js uses the same number. `DEFAULT_FINALITY_THRESHOLD = 15` (O1JS [`fetch.ts#L661`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/fetch.ts#L661)), and `waitForFinality` defaults to 15 blocks, polling every 120 s up to 45 times. Its JSDoc says 60 s and 30 attempts; the code is what runs (O1JS [`transaction.ts#L380-L384`, `#L780-L800`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/transaction.ts#L780-L800)). CONFIRMED-CODE.
- **`k = 290`** (MINA [`node_config/profiled/mainnet.ml#L7`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/node_config/profiled/mainnet.ml#L7); live `daemonStatus.consensusConfiguration.k = 290`). The transition frontier is loaded with `max_length = global_max_length genesis_constants` (MINA [`transition_frontier/transition_frontier.ml#L389-L391`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transition_frontier/transition_frontier.ml#L389-L391)). A node keeps only the last k blocks and never switches to a chain that forks below its root. So k is the depth past which a node itself treats a block as permanent. CONFIRMED-CODE (frontier length); the "never reorgs below root" reading is **Inference** from the frontier design.
- The archive marks a block `canonical` only once it is **k blocks** below a newer block, and leaves everything above that `pending` (MINA [`app/archive/lib/processor.ml#L4421-L4460`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/app/archive/lib/processor.ml#L4421-L4460)). CONFIRMED-CODE. Live: the Archive-Node-API reports `pendingMaxBlockHeight − canonicalMaxBlockHeight = 290`. CONFIRMED-LIVE.
  **Inference:** a watcher that waits for archive `canonical` waits 290 blocks, about 20 hours at today's fill, not 15 blocks.
- **Reorg frequency at depth 1–15 on mainnet: no data found.** The daemon's GraphQL returns only the best chain, and no orphaned-block statistics from o1Labs or the Foundation were found. The docs' confidence column is the only primary figure, and it is a model, not a measurement. The Ouroboros Samasika paper was not read for this note. UNCONFIRMED.

### 1.2 How a node reports inclusion

| Surface                                          | What it says                                                                                                                                                                                                       | Source                                                                                                                                                                                     |
| ------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `transactionStatus`                              | `INCLUDED` if the command is in any block on the best-tip path **in the frontier**; `PENDING` if it is in a frontier block off that path, or in the pool; otherwise `UNKNOWN`. **No depth.** Once its block passes the root (k deep), it is `UNKNOWN` again | MINA [`transaction_inclusion_status/transaction_inclusion_status.ml#L27-L52`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_inclusion_status/transaction_inclusion_status.ml#L27-L52) |
| `bestChain(maxLength)`                           | Blocks from the frontier root to the best tip, each with `protocolState.consensusState.blockHeight` and `slotSinceGenesis`. Depth = tip height − inclusion height, computed by the caller. History before the root is gone (live: `block(height: 563957)` → "Could not find block in transition frontier") | MINA [`graphql/mina_graphql/mina_graphql.ml#L2157-L2170`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/graphql/mina_graphql/mina_graphql.ml#L2157-L2170); CONFIRMED-LIVE |
| Archive `chain_status`                           | `pending` / `canonical` / `orphaned`, with canonical at k deep                                                                                                                                                  | §1.1                                                                                                                                                                                       |
| o1js `getDepth` / `waitForFinality`              | Scans the last 20 `bestChain` blocks for the hash; returns `null` if absent **or** if `failureReason` is set; depth = tip − inclusion height                                                                     | O1JS [`fetch.ts#L687-L731`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/fetch.ts#L687-L731)                                               |

All CONFIRMED-CODE. **Inference:**

- "Counted" has to be computed by the client from `bestChain`: find the command's block and subtract heights.
- A reorg shows up only as the command disappearing from the best chain. Nothing signals it.
- A client that waits past 290 blocks before looking gets `UNKNOWN`. Without an archive, it cannot tell that apart from "never included".

### 1.3 Included is not applied

- A zkApp command whose account updates fail is still put in the block. Every update but the fee payer is `Cancelled`, the status is `Failed`, and the fee payer's fee and nonce increment stand ("all the account updates would be cancelled except the fee payer one", MINA [`transaction_logic/mina_transaction_logic.ml#L1808-L1835`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/mina_transaction_logic.ml#L1808-L1835)). CONFIRMED-CODE.
- The block producer puts in every command whose first pass returns `Ok`, `Failed` status included. It skips only an `Error` (MINA [`staged_ledger/application_state.ml#L38-L75`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/staged_ledger/application_state.ml#L38-L75)). CONFIRMED-CODE.
- **Inference:** "landed" must mean included **and** `failureReason` null. `transactionStatus = INCLUDED` alone does not say that.

### 1.4 A reorged-out command goes back to the pool

- On a best-tip change, the pool re-adds "any commands … that were included in the old one but not the new one, provided they are still valid against the ledger of the best tip. The goal is that transactions are carried from losing forks to winning ones" (MINA [`network_pool/transaction_pool.ml#L534-L545`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/network_pool/transaction_pool.ml#L534-L545)). Re-added commands are then revalidated by fee-payer nonce and balance (`Indexed_pool.revalidate`, same file `#L648-L676`). Every node does this to its own pool. CONFIRMED-CODE.
- Account and network preconditions are checked when the command is applied, not when it is re-added. A command whose zkApp-state precondition no longer holds on the winning fork re-lands as **Failed** and pays its fee (§1.3). One whose `validWhile` or `valid_until` has passed is dropped from the pool (§4.3). The proof does not need redoing: its statement is the account update's digest and its calls (MINA [`mina_base/zkapp_statement.ml#L8`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/mina_base/zkapp_statement.ml#L8)). **Inference** from the code cited.

**Verdict 1: holds with caveat.** The depth of 15 is the docs' and o1js's own number. Four caveats:

- in wall-clock time it is about an hour (§2);
- the node gives no depth, so the client must compute it;
- a landing must be checked for `failureReason`;
- the archive's "canonical" means 290 deep, not 15.

---

## 2. What 15 blocks costs in wall-clock time

| Quantity                                                  | Value                                         | Source                                                                                                                                 | Tag            |
| --------------------------------------------------------- | --------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- | -------------- |
| Slot duration                                             | 90,000 ms                                     | MINA `profiled/mainnet.ml#L43`; live `slotDuration: 90000`                                                                              | CODE, LIVE     |
| Slots per epoch                                           | 7,140 (~7.4 days at 90 s)                     | MINA `profiled/mainnet.ml#L11`; MIP-6 "Epoch Transition Frequency"                                                                      | CODE, DOCS     |
| `f` (design fill under full participation)               | 0.75                                          | §1.1                                                                                                                                   | CODE           |
| Observed fill, last 290 blocks                            | 290 blocks in 881 slots = **0.328**, 274 s/block | live `bestChain(maxLength: 290)`                                                                                                     | LIVE           |
| Observed fill since Mesa genesis                          | 18,810 blocks in 52,656 slots = **0.357**, 252 s/block | live `genesisBlock` (height 548,147, slot 958,440, 2026-09-03 18:00 UTC) vs tip (566,957, slot 1,011,096) | LIVE           |
| Longest empty gap in the 290-block sample                 | 13 slots (19.5 min)                           | live                                                                                                                                   | LIVE           |
| 15 blocks at f = 0.75 (the docs' figure)                  | 20 slots = 30 min                             | arithmetic                                                                                                                             | —              |
| **15 blocks at the observed 0.357**                       | **~42 slots ≈ 63 min** (expected value)       | arithmetic                                                                                                                             | **Inference**  |
| 290 blocks (k) at the observed 0.357                      | ~812 slots ≈ 20 h                             | arithmetic                                                                                                                             | **Inference**  |

- o1js knows this. Its default inclusion poll is "45 attempts \* 20s each = 15min", with the comment that "the block time … is currently longer than the average 3-4min" (O1JS [`mina.ts#L326-L332`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/mina.ts#L326-L332), a pre-Mesa comment). CONFIRMED-CODE.
- Why fill is about half of `f` was not established. Offline stake is the obvious reading, since `f` assumes all stake participates, but no source states it. UNCONFIRMED.
- What the docs' 99.92% becomes when only ~half the stake produces was not established. UNCONFIRMED.

**Verdict 2: holds with caveat.** Plan for about an hour, not 30 minutes. Measure the time to 15 blocks rather than assuming it, since fill is a property of live stake, not a constant.

---

## 3. Several deposits to one zkApp in one block

- Commands in a block are applied in order, each against the ledger the previous one left. A state precondition (`getAndRequireEquals`) checked against a field that an earlier command in the same block changed fails. The command is then included as Failed and still charged (§1.3). This is the mechanism PRIOR §3.3 describes, with the fee consequence added. CONFIRMED-CODE (mechanism). **Inference:** a deposit that only raises the zkApp's balance (a zkApp's `receive` is `none`, PRIOR §3.1) and asserts no state does not conflict. One that rewrites a per-channel root does conflict.
- From one fee payer: the pool accepts a queue of nonces (each new command's nonce ≤ the last queued + 1, MINA [`network_pool/indexed_pool.ml#L866-L885`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/network_pool/indexed_pool.ml#L866-L885), conditions 1a–1b). Once a block's zkApp quota is used up, the producer skips the rest of that fee payer's commands in that block too (`dependency_skipped`, `application_state.ml#L43-L46`). CONFIRMED-CODE.

**Verdict 3: holds.** Nothing Mina-specific blocks manual top-ups. A zkApp design with a shared root makes concurrent top-ups a fee-burning race, and that is a design question for the zkApp (PRIOR D4).

---

## 4. The withdraw-delay floor on Mina

### 4.1 How a zkApp measures time

- The protocol's network precondition has `snarked_ledger_hash`, `blockchain_length`, `min_window_density`, `total_currency`, `global_slot_since_genesis` and two epoch-data fields, and **no timestamp** (MINA [`mina_base/zkapp_precondition.ml#L933-L950`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/mina_base/zkapp_precondition.ml#L933-L950)). `validWhile` is also a global-slot range. CONFIRMED-CODE.
- o1js's `network.timestamp` is synthesised: `slot × slotTime + genesisTimestamp`, and `requireBetween` becomes a slot range (O1JS [`precondition.ts#L156-L191`, `#L414-L423`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/precondition.ts#L414-L423)). The constants come from the daemon's `genesisConstants.genesisTimestamp` and `slotDuration` (O1JS [`mina.ts#L635-L641`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/mina.ts#L635-L641)). CONFIRMED-CODE.
- Live, `genesisTimestamp` is the **Mesa** genesis (2026-09-03 18:00 UTC), but `slotSinceGenesis` did not restart: the Mesa genesis block is at slot 958,440. CONFIRMED-LIVE. **Inference:** o1js maps the current slot (1,011,096) to 2029-07-22 when it is 2026-10-28, which is 958,440 × 90 s ≈ 998 days ahead. A channel zkApp must not use `network.timestamp`, and should express its delay in slots. This was not run against o1js. Open o1js issue #1588 (timestamp wrong on devnet, PRIOR §3.2) is consistent with it.
- **A slot is not a fixed length.** Mesa (MIP-6) changed `block_window_duration_ms` from 180,000 to 90,000 at a hard fork. Its migration rewrote only "actively vesting" accounts' timing, "to preserve real-world vesting timelines" (MIP-6 Specification item 3, Appendix). CONFIRMED-DOCS. **Inference:** a delay stored in zkApp state as slots was halved in wall-clock time at Mesa, and a future slot change would do it again. The zkApp's delay is fixed in slots at deploy, so TOON's seconds-denominated floor has to be converted at the slot length then current. PRIOR §3.2 already covers `network.*` preconditions being checked against the parent block.

### 4.2 Worst-case inclusion latency

- **Hard forks stop transactions for hours.** For Mesa: "Stop Transaction Slot: Thursday September 3rd 10:00 UTC … Blocks produced … will be empty, and no transactions will be accepted", and block production stopped at 15:00 UTC ([3.5.0-mainnet-stop-slot](https://github.com/MinaProtocol/mina/releases/tag/3.5.0-mainnet-stop-slot)). CONFIRMED-DOCS. The new chain's genesis is 18:00 UTC (live). So **no transaction could land for 8 hours.** CONFIRMED-DOCS, LIVE. How slots count across such a halt, and so whether a pending withdrawal's slot timer advances during it, was not established. UNCONFIRMED.
- **Congestion.** Only 12 zkApp commands per block (§5) at ~343 blocks a day is ~4,100 zkApp slots a day. In a 6-hour window that is ~86 expected blocks and ~1,030 zkApp slots, shared network-wide. **Inference**, arithmetic on §2 and §5. No Mina source bounds inclusion time. The docs say only: "If a transaction is valid and the network is honest, then in all likelihood, a transaction will make it into a block." CONFIRMED-DOCS.
- **Today's load is light.** In the 290-block sample there were 22 zkApp commands (mean 0.08 per block). One block was full at 12 and one held 7; 18 of the 22 were 31-update commands paying 0.012 MINA. The pool held 0 zkApp commands at query time. CONFIRMED-LIVE.

### 4.3 Mempool limits, eviction, expiry and replacement

| Rule                         | Value / behaviour                                                                                                                                                                                                       | Source                                                                                                                                                                                                                |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Pool size                    | `pool_max_size = 3000` commands, "needs to be consistent across nodes to prevent spurious bans"                                                                                                                        | MINA `profiled/mainnet.ml#L25-L27`                                                                                                                                                                                    |
| Eviction                     | Over the cap, the pool drops the **lowest fee per weight unit**, with its dependents. When full, a new command must beat the current minimum fee/wu                                                                     | MINA [`transaction_pool.ml#L451-L473`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/network_pool/transaction_pool.ml#L451-L473); `indexed_pool.ml#L845-L855`                                                |
| Expiry                       | Only by the fee payer's optional `valid_until` global slot (none = never). There is no age-based expiry                                                                                                                | MINA [`mina_base/user_command.ml#L348-L357`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/mina_base/user_command.ml#L348-L357); `indexed_pool.ml#L286-L292`, `#L831-L843`                                     |
| Rebroadcast                  | The originating node rebroadcasts its own commands every 10 min for 50 min, then stops. Pools are in memory                                                                                                            | MINA [`network_pool/network_pool_base.ml#L240-L255`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/network_pool/network_pool_base.ml#L240-L255); docs "in-memory store"                                       |
| Same-nonce replacement       | Allowed. The new fee must be ≥ the replaced fee, and the increment must cover the fees of later queued commands it displaces that cannot be re-added, plus `replace_fee` = **1 nanomina**. Else `Insufficient_replace_fee` | MINA [`indexed_pool.ml#L7-L8`, `#L1050-L1130`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/network_pool/indexed_pool.ml#L1050-L1130)                                                                       |
| Re-proving on a fee bump     | Not needed: the proof's statement excludes the fee payer (§1.4). The fee payer's signature, and any `useFullCommitment` signature, must be redone                                                                     | **Inference** from `zkapp_statement.ml#L8`                                                                                                                                                                             |
| Docs on eviction             | "If the transaction pool hits its capacity … it evicts the transaction with the lowest fee … the sender needs to resend the transaction with a higher fee"                                                              | CONFIRMED-DOCS ([Lifecycle](https://docs.minaprotocol.com/mina-protocol/lifecycle-of-a-payment))                                                                                                                        |

All CONFIRMED-CODE unless marked. **Inference:** a command can be dropped silently in three ways: evicted, its `valid_until` passed, or the pools that held it restarted after the 50-minute rebroadcast window. It is resubmittable at the same nonce with a higher fee. A low-fee command blocks every later nonce from the same fee payer.

**Verdict 4: holds with caveat.** Six hours is ~240 slots, ~86 expected blocks and ~1,000 network-wide zkApp slots at today's numbers. Today's demand fills a small fraction of that. But:

- the one recorded hard fork blocked inclusion for **8 h**, which a 6-hour floor would not survive (the 1-day default would);
- the delay must be set in slots, and a slot-length change rescales it;
- `network.timestamp` must not be used.

Whether 6 h is "sane" turns on whether a hard-fork halt counts as a risk to cover. That is a decision, not a fact.

---

## 5. Throughput: the zkApp-commands-per-block cap

| What                                  | Value                                                                                                                                                                                                                                                                                   | Source                                                                                                                                                                                                                                   | Tag   |
| ------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----- |
| Soft limit (mainnet)                  | `zkapp_cmd_limit = Some 12`, set by `be09ffac` (2026-02-20): "temporarily set to 12 for mesa until the final memory limit fixes have been delivered after the hard fork"; the same on `develop` (#18513, "temporarily") and `compatible`                                                   | MINA [`profiled/mainnet.ml#L53`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/node_config/profiled/mainnet.ml#L53)                                                                                                             | CODE  |
| Soft limit before Mesa                | 24 per 180 s block (`[%%define zkapp_cmd_limit 24]`, 3.3.0 `src/config/mainnet.mlh`). So the per-minute ceiling did not change at Mesa                                                                                                                                                  | MINA tag 3.3.0                                                                                                                                                                                                                           | CODE  |
| MIP-6's plan vs what shipped          | MIP-6 said the soft limit "will be unset by default"; the release ships 12. The code wins                                                                                                                                                                                              | MIP-6 Specification item 4                                                                                                                                                                                                               | DOCS  |
| Who enforces 12                       | The **producer** only: `try_applying_txn` stops adding zkApp commands when its quota hits 0. It is a mutable ref, settable at runtime only through the ITN-only `zkAppCommandLimit` mutation; no CLI flag was found                                                                       | MINA `application_state.ml#L43`; [`mina_graphql.ml#L1642-L1650`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/graphql/mina_graphql/mina_graphql.ml#L1642-L1650) (inside `module Itn`, `#L1043`); `mina_cli_entrypoint.ml#L1495` | CODE  |
| Hard cap                              | `zkapp_cmd_limit_hardcap = 128`. A block over it is **rejected on application** (`ZkApps_exceed_limit`), so this one binds every node                                                                                                                                                     | MINA [`node_config_unconfigurable_constants.ml#L9`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/node_config/unconfigurable_constants/node_config_unconfigurable_constants.ml#L9); [`staged_ledger.ml#L1044-L1048`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/staged_ledger/staged_ledger.ml#L1044-L1048) | CODE  |
| Block capacity, all transactions      | `scan_state_transaction_capacity_log_2 = 7` (128)                                                                                                                                                                                                                                      | MINA `profiled/mainnet.ml#L19`                                                                                                                                                                                                            | CODE  |
| Account updates per command           | `max_zkapp_segment_per_transaction = 16` segments. A proof update is one segment and a pair of signed/none updates is one. So at most **15 proof updates** plus the fee payer, or **31 signed** plus the fee payer                                                                          | MINA `node_config_unconfigurable_constants.ml#L3`; MIP-9 Modifications and Appendix                                                                                                                                                       | CODE, DOCS |
| Observed                              | 31-update commands do land on mainnet (live sample)                                                                                                                                                                                                                                    | live                                                                                                                                                                                                                                     | LIVE  |
| Plan to raise 12                      | No PR, issue or release note naming a date or a value was found                                                                                                                                                                                                                         | `gh search` over MinaProtocol/mina                                                                                                                                                                                                        | UNCONFIRMED |

**Correction to PRIOR §3.4.** "About 8 zkApp commands a minute" assumes every slot holds a block. At the observed fill it is **~2.9 a minute, ~4,100 a day**, network-wide. **Inference**, arithmetic.

**Verdict 5: holds.** A 24-hour sweep spends a few commands a day against ~4,100 network-wide zkApp slots. A 10-minute sweep per node would be ~144 a day each, which is a visible share once there are tens of nodes. The cap is a producer setting that o1Labs calls temporary, so it could rise without a hard fork. 128 cannot rise without one.

---

## 6. Fees

- **Fee payer.** The fee payer must sign and increment its nonce (PRIOR §3.5). It may be any account; its body carries `fee`, `nonce` and optional `valid_until` (MINA `user_command.ml#L348-L357`). CONFIRMED-CODE.
- **Minimum fee.** `minimum_user_command_fee = 0.001` MINA, for zkApp commands as for payments. It is checked at well-formedness (`Insufficient_fee`, MINA `user_command.ml#L486-L489`) and at GraphQL submission (`mina_graphql.ml#L519-L529`). `default_transaction_fee = 0.25` is a client default, not a floor (`profiled/mainnet.ml#L31-L35`). CONFIRMED-CODE.
- **Ordering.** The pool indexes by **fee per weight unit** (`all_by_fee`, `applicable_by_fee`, `indexed_pool.ml#L34-L44`). A zkApp command's weight is 1 for the fee payer plus 1 per account update; the memo weighs 0 (MINA [`mina_base/zkapp_command.ml#L815-L834`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/mina_base/zkapp_command.ml#L815-L834); `user_command.ml#L421-L423`). A proof update weighs the same as a signed one. CONFIRMED-CODE.
- **No fee estimation API.** The daemon's GraphQL query list has none (`mina_graphql.ml#L2828-L2870`). `pooledZkappCommands` exposes the local pool, so a client can read competing fees itself. CONFIRMED-CODE.
- **Observed fees** (290 blocks, CONFIRMED-LIVE):

  | Kind          | n   | min       | median    | max       |
  | ------------- | --- | --------- | --------- | --------- |
  | zkApp command | 22  | 0.012 MINA | 0.012 MINA | 0.1 MINA  |
  | Payment etc.  | 495 | 0.001 MINA | 0.01 MINA  | 0.25 MINA |

  18 of the 19 commands at 0.012 had 31 updates (about 0.000375 MINA per weight unit); one had 4. The three at 0.1 had 3–5 updates. **Inference:** a channel command of 2–4 updates at 0.012 MINA outranks a 31-update batch at the same fee.
- **Account creation fee: 1 MINA** (`account_creation_fee_int = "1.0"`, `profiled/mainnet.ml#L29`; live `accountCreationFee: 1000000000`). CONFIRMED-CODE, LIVE. Who pays:
  - When a command creates an account, the fee comes from that new account's own balance change if its update sets `implicitAccountCreationFee`, which only works in MINA (`Cannot_pay_creation_fee_in_token`). Otherwise it comes from the command's excess, which some other update must fund by debiting itself (MINA [`zkapp_command_logic.ml#L1419-L1500`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/zkapp_command_logic.ml#L1419-L1500)). o1js's `AccountUpdate.fundNewAccount(feePayer, n)` is that debit: a signed update of −n × 1 MINA from whichever key is named (O1JS [`account-update.ts#L1100-L1107`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/account-update.ts#L1100-L1107)). CONFIRMED-CODE.
  - **Refundable: no.** The fee is subtracted from `supply_increase`, so it is burned, not paid to anyone (same `zkapp_command_logic.ml` range). No account-deletion operation was found, as PRIOR also found. CONFIRMED-CODE (burn); the absence of a deletion is UNCONFIRMED as a negative.
- **Many channels without new accounts.** State fields are **32 after Mesa**, not 8 (PRIOR §3.4). Beyond that, channels can live behind a Merkle root in state, in actions (up to 1,024 elements per command, PRIOR §3.4), or in `OffchainState` (Experimental). Each of these serialises writers on the root (§3, PRIOR §3.3). The daemon keeps actions only for the frontier (`accountActions`: "from the current best tip", `mina_graphql.ml#L2184-L2203`). Anything older needs an archive. CONFIRMED-CODE.
- **Observed archive hazard.** The third-party Archive-Node-API above reported `canonicalMaxBlockHeight = 547,897` and `pendingMaxBlockHeight = 548,187`. Both are at or below the Mesa genesis (548,147) plus 40, so it has not followed the chain since 2026-09-03. CONFIRMED-LIVE. **Inference:** a watcher that depends on a public archive needs a second source, or its own.

**Verdict 6: holds.** Nothing requires a facilitator: any key can be fee payer and pay the fee. The open questions are the fee level and the 1 MINA creation fee (§7).

---

## 7. Open questions for round 2 (facts only)

### (a) ADR 0073's bounded waits when counting takes ~an hour

- ADR 0073 decision 5: EVM `confirm()` has an overall deadline of **180 s**, and reports "not observed" after 30 s of "not found" answers. Solana's `give_up_after` is two minutes (PRIOR §1.2). CONFIRMED-DOCS.
- The Mina stages a submitter can observe:
  1. accepted by GraphQL `sendZkapp` (pool admission);
  2. `PENDING`;
  3. `INCLUDED` on the best tip, with `failureReason` null;
  4. 15 deep, computed by the client;
  5. 290 deep, the root.

  Stage 3 is the earliest sign that a command was applied. §1.2–1.3.
- Time to inclusion is at least one block, which averages **~250–275 s** (§2). That is already longer than 180 s even with an empty pool. o1js's own inclusion wait defaults to 15 min. **Inference** plus CONFIRMED-CODE.
- There is no blockhash-style expiry. The bounding tool is `valid_until` (a global slot) on the fee payer. Past it, the pool drops the command, so a deadline can be made definitive rather than "not known to be safe" (§4.3). CONFIRMED-CODE.
- Proving with o1js adds time before submission. No first-party Mesa-era figure exists (PRIOR "Unconfirmed"). UNCONFIRMED.

### (b) Pricing fees

- Floor 0.001 MINA. Ranking by fee per weight unit. Observed zkApp fees of 0.012–0.1 MINA. A replacement needs only +1 nanomina over the old fee. A bump needs a new fee-payer signature but no new proof (§4.3, §6). CONFIRMED-CODE, LIVE.
- A full pool evicts the lowest fee/wu. With an empty pool, any fee ≥ 0.001 is included by a producer that has quota left (§4.3, live pool size 0). CONFIRMED-CODE, LIVE.
- For the withdrawal-landing race, the competing command is the payer's own finish-withdrawal. Both rank purely on fee/wu, and a producer orders by that within its 12-command quota. **Inference.**
- Nonce queueing: one stuck low-fee command from a node's settlement key blocks every later command from that key until it is replaced (§4.3). CONFIRMED-CODE.

### (c) Who bears the 1 MINA account-creation fee

- It is charged once per new account (each channel zkApp account, each token account) and burned. Any party in the command can pay it via `fundNewAccount`, or the new account itself can pay from incoming MINA (§6). CONFIRMED-CODE.
- Comparison facts: on Solana, `payment-channels` rent is paid by `rent_payer` at `open` and returned by `reclaim` (PRIOR §2.2). Mina has no reclaim (§6). On EVM there is no per-channel account cost (PRIOR §2.2). CONFIRMED-CODE via PRIOR.
- A design that keeps every channel inside one zkApp pays 1 MINA once per zkApp rather than once per channel, at the price of root contention (§3, §6). **Inference.**

---

## Unconfirmed, and why

- **Reorg frequency at depth 1–15.** No primary measurement was found. The daemon exposes only the best chain, and the public archive is stale.
- **Why slot fill is ~0.36 rather than 0.75,** and what that does to the docs' confidence table. No source states either.
- **How global slots count across a hard-fork halt.** The Mesa genesis slot (958,440) does not match a wall-clock count, so the rule was not derived.
- **o1js `network.timestamp` on mainnet.** Inferred from code and live constants, not run.
- **A plan to lift the 12-command limit.** None was found.
- **No account deletion.** A negative, not searched exhaustively.
