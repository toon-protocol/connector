# A stablecoin for zk notes: fork zkUSD, build one, or neither?

> **Research note, not a decision record.** It decides nothing.
> [ADR 0080](../adr/0080-a-client-may-pay-from-a-zk-note.md) (_A client may pay from a zk note_) is
> the record this note serves. The question: can zk-note payments be denominated in a stable asset
> instead of native MINA, and if so, should TOON fork zkUSD (`zkUSD-Protocol/core`) or build its
> own? It builds on [`mina-tokens-and-stablecoins.md`](mina-tokens-and-stablecoins.md) (cited
> below as **TOKENS**), which established that no stablecoin is live on Mina mainnet, that zkUSD
> reached devnet only, and that Nori bridges ETH only. That ground is not redone here. Primary
> sources only. Written 2026-10-27, and every source was read that day.

**Evidence tags.** CONFIRMED-CODE means read in source at a pinned commit. CONFIRMED-DOCS means
read in the maintainer's, issuer's or legislator's own documents. CONFIRMED-LIVE means observed
from a live endpoint on 2026-10-27. UNCONFIRMED means none of those, and says why. Sections
marked **Judgement** are this note's opinion, not facts.

**Sources pinned.**

- **zkUSD core, `main`**: [`zkUSD-Protocol/core@2142a531`](https://github.com/zkUSD-Protocol/core/tree/2142a531c3e9bdc659bf8495efbb377f32fd8847)
  (2024-12-19). This is the commit TOKENS read, and still `main`'s head. Cited as **CORE-MAIN**.
- **zkUSD core, `develop`**: [`zkUSD-Protocol/core@2ea2df04`](https://github.com/zkUSD-Protocol/core/tree/2ea2df04f6e328333765f91a108b3e2903eaf316)
  (2025-05-05). It is 292 commits ahead of `main` and 0 behind (`gh api …/compare/main...develop`),
  so **`main` is not the latest code**. Cited as **CORE-DEV**.
- **Fizk rollup**: [`zkUSD-Protocol/rollup@9ed45ce1`](https://github.com/zkUSD-Protocol/rollup/tree/9ed45ce192c1e09f2cea5be5a8f4a52269aa2096)
  (2025-08-04). Cited as **ROLLUP**.
- **Fizk whitepaper**: [`zkUSD-Protocol/whitepaper@2629ba11`](https://github.com/zkUSD-Protocol/whitepaper/tree/2629ba115a8291eaf17cf681dffd86c9bfa18148),
  file `The Fizk Protocol (zkRollup) - Whitepaper.pdf` (dated June 2025, committed 2025-08-08).
  Cited as **WP**.
- **This repository**: [`toon-protocol/connector@605f6efe`](https://github.com/toon-protocol/connector/tree/605f6efe58a18267af8d8934c35181fdc1d936a0)
  (`origin/main`), for the `connector-zk-note` crate #1521 built. Cited as **CONNECTOR**.
- **Fungible-token standard**: [`MinaFoundation/mina-fungible-token@8dbf75a4`](https://github.com/MinaFoundation/mina-fungible-token/tree/8dbf75a4a50d236d9a9ab137a7fd451bafddae41)
  (the commit TOKENS pinned; the repository is archived). Cited as **MFT**.
- **o1js**: [`o1-labs/o1js@a428f9b9`](https://github.com/o1-labs/o1js/tree/a428f9b97817177ef4b5868cf148c0ac67a1048d), as TOKENS.
- **Zeko**: [`zeko-labs/zeko@49fb253d`](https://github.com/zeko-labs/zeko/tree/49fb253de86ce9188bcd289a8d5bb09e04164084) and
  [`zeko-labs/ethereum-settlement@053bbb4d`](https://github.com/zeko-labs/ethereum-settlement/tree/053bbb4ded6013ada3759f8577f59664ba3b2cc5).
- **Nori**: [`Nori-zk/nori-bridge-sdk@2a73bdf8`](https://github.com/Nori-zk/nori-bridge-sdk/tree/2a73bdf88f9326478dfab02235afc2491b2441a8)
  (still head; same as TOKENS) and [`Nori-zk/nori-tempo-sdk@ef244472`](https://github.com/Nori-zk/nori-tempo-sdk/tree/ef244472e0298df79ffb02e9307094a183285793).
- **GENIUS Act**: [Public Law 119-27](https://www.govinfo.gov/link/plaw/119/public/27?link-type=html), read on govinfo.gov.

---

## Verdict

**Don't fork zkUSD, don't build a stablecoin, and keep the vault in native MINA for now.** Price
routes in USD terms at the zk edge through ADR 0071's declared rate, and say plainly in ADR 0080
that the payer carries MINA's price risk on its note and the zk edge carries it on its open budget
until collection. Pin "one vault, one asset" now, so a later stable-asset vault is a new vault and
not a migration. Revisit when a stablecoin someone else issues and redeems is live on Mina L1.

The reasons, in short:

- **zkUSD is abandoned, unaudited, and not the code TOKENS read.** The real line of work is the
  `develop` branch (May 2025), which redesigned the engine, oracle and governance. The team then
  pivoted to "Fizk", a Sui-sequenced, AWS-Nitro-enclave rollup for bridged USDC/USDT with a
  governance audit key and a blacklist. That pivot stopped in August 2025. `fizk.xyz` is now listed
  for sale, and the oracle hosts don't resolve. No audit was ever announced.
- **What it would hand TOON is a business, not a library.** A CDP stablecoin needs an oracle
  network, liquidators, a governance key, hard-fork upgrade ceremonies, an audit, peg defence and
  legal advice. zkUSD's own author stopped at legal advice. Routing fees (CLAUDE.md, "How anyone
  earns") don't pay for any of it, and every failure lands on payers' notes.
- **Reading the `develop` code found a likely price-forgery path** (§1.6): the devnet oracle
  whitelist fills four of its eight slots with a dummy key whose **private** key is committed, and
  the aggregation proof trusts a prover-supplied `isDummy` flag. If that reading is right, anyone
  could prove an arbitrary MINA price. The fact that this survived 292 commits is the strongest
  argument against forking.
- **No alternative is live either.** Circle and Tether don't issue on Mina (TOKENS §4). Nori is
  ETH-only on Mina devnet. Its newer Tempo and Solana bridges mirror ERC-20s, but its Mina SDK has no
  ERC-20 leg. Zeko now settles on Ethereum, not Mina, and its ERC-20 bridge is a permissioned
  testnet PoC.
- **The zk note design survives any later choice.** A token vault is a token-holder zkApp under
  the token owner's approval. That approval doesn't touch the off-chain allowance path, so it
  doesn't break unlinkability. It does give the issuer a pause switch over the whole vault (§3).
  The note commitment from #1521 carries no asset, and it needs none if each vault holds one asset.

| #   | Question                                | Answer                                                                                                                                                                | Tag                          |
| --- | --------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------- |
| 1   | Latest zkUSD code                       | `develop@2ea2df04` (2025-05-05), not `main`. Then the Fizk `rollup` (last 2025-08-04). Nothing since                                                                  | CONFIRMED-CODE               |
| 1   | Licence                                 | `core` and `cli`: Apache-2.0, LICENSE file with no copyright line, no NOTICE. `rollup`: `package.json` says Apache-2.0 but there's no LICENSE file. Other repos: none | CONFIRMED-CODE               |
| 1   | Audit                                   | None found. Planned as a next step in the Nov 2024 feasibility study, and outside every grant's scope                                                                 | CONFIRMED-DOCS / UNCONFIRMED |
| 1   | o1js                                    | `^2.*` (`main`), `^2.2.0` (`develop`). Pre-Mesa, so every verification key changes on port (spike #1512)                                                              | CONFIRMED-CODE               |
| 1   | "Private"                               | **Nothing private in `core`**: vault collateral, debt and owner are public zkApp state and events. Privacy appears only in the Fizk rollup, with an audit key         | CONFIRMED-CODE               |
| 1   | Live?                                   | No. `fizk.xyz` redirects to a GoDaddy for-sale page; `oracle1.fizk.xyz` and `zkusd.oracle.nori.global` don't answer                                                   | CONFIRMED-LIVE               |
| 2   | Any stable asset on Mina L1 today       | None (TOKENS). No bridged one is in reach either                                                                                                                      | CONFIRMED-DOCS               |
| 3   | Can a zk-note vault hold a custom token | Yes, as a token-holder zkApp on `(vault, tokenId)`. Every move needs the token owner's `approveBase`, which a pause turns off for the whole vault                     | CONFIRMED-CODE               |
| 3   | Does #1521's commitment bind an asset   | **No.** One value generator, no token ID in the commitment, leaf, nullifier or context. Only the signed state names the `vault`                                       | CONFIRMED-CODE               |
| 4   | Recommendation                          | Neither fork nor build. Native MINA with declared-rate pricing; one vault, one asset                                                                                  | Judgement                    |

---

## 1. zkUSD, from its code

### 1.1 The organisation and its repositories

The `zkUSD-Protocol` org (display name "Fizk Protocol", created 2024-11-26) has nine repositories.
None is archived. Read with `gh api orgs/zkUSD-Protocol/repos`:

| Repo                    | Last push  | Licence (GitHub) | What it is                                                                                                 |
| ----------------------- | ---------- | ---------------- | ---------------------------------------------------------------------------------------------------------- |
| `core`                  | 2025-06-17 | Apache-2.0       | The contracts. `main` 2024-12-19; `develop` 2025-05-05; `feat/private-research-contract-vaults` 2025-06-17 |
| `rollup`                | 2025-08-04 | none             | The Fizk confidential rollup: notes, nullifiers, Sui bridging, enclaves, audit key                         |
| `whitepaper`            | 2025-08-08 | none             | Three PDFs: Fizk v0.1, v0.3 and the current "(zkRollup)" paper                                             |
| `ui`                    | 2025-08-08 | none             | Next.js UI (`main` last commit 2025-02-23)                                                                 |
| `cli`                   | 2025-05-13 | Apache-2.0       | "CLI tool for interaction with the Fizk Protocol"                                                          |
| `oracle-price-provider` | 2025-04-18 | none             | Off-chain price fetcher, Docker                                                                            |
| `services`              | 2025-04-17 | none             | Proof aggregation and vault-tracker services on AWS                                                        |
| `worker`                | 2025-03-24 | none             | "Serverless worker for external proving"                                                                   |
| `tools`                 | 2025-03-14 | none             | A Python risk-model simulator                                                                              |

Nothing in the org was pushed after 2025-08-08. The npm package `@zkusd/core` stopped at `0.0.6`,
published 2025-04-17 (`npm view @zkusd/core time`). CONFIRMED-CODE.

### 1.2 Two designs, not one

TOKENS read `main`. `main` is the December 2024 MVP, and it is **not** what ran on devnet:

- **`main` (Dec 2024).** An engine `TokenContract` keeps per-user vault contracts on its own token
  accounts. It is the `FungibleToken` admin for zkUSD, and it keeps the price on chain in
  even/odd-block slots fed by whitelisted oracles
  ([CORE-MAIN `README.md`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/README.md),
  [`src/zkusd-engine.ts`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-engine.ts)).
- **`develop` (May 2025).** It reflects two redesigns the author reported on the forum. First,
  "a zkProgram that will collect and aggregate oracle price submissions and generate a proof that
  users can use in their transactions", because the old feed used "around 10 transactions … each
  block … just under half of all the network capacity for zkApps". Second, absorbing vault logic into the engine,
  because a Mina hard fork resets verification-key permissions: user-owned vault zkApps would need
  each user to re-sign before they could be liquidated
  ([forum t/6756, updates of 2025-01-24 and 2025-02-12](https://forums.minaprotocol.com/t/zkusd-deployment-readiness/6756)).
  It adds a governing council
  ([CORE-DEV `src/contracts/zkusd-governing-council.ts`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/src/contracts/zkusd-governing-council.ts)).
  CONFIRMED-CODE / CONFIRMED-DOCS.
- **Then Fizk (Jun–Aug 2025).** The whitepaper is no longer a MINA-backed CDP. It is "a
  confidential rollup that brings privacy to stablecoin payments": users deposit USDC/USDT through
  Wormhole into a Coordinator contract **on Sui**, receive zkUSDC/zkUSDT notes, intents are
  sequenced on Sui, blobs go to Walrus, the operators run in AWS Nitro enclaves, and proofs are
  settled to Mina ([WP](<https://github.com/zkUSD-Protocol/whitepaper/blob/2629ba115a8291eaf17cf681dffd86c9bfa18148/The%20Fizk%20Protocol%20(zkRollup)%20-%20Whitepaper.pdf>),
  pp. 4, 7–10, 15–17). The rollup code agrees: a `CollateralType` of `MINA` or `SUI`
  ([ROLLUP `vault-collateral-type.ts#L3-L7`](https://github.com/zkUSD-Protocol/rollup/blob/9ed45ce192c1e09f2cea5be5a8f4a52269aa2096/src/domain/vault/vault-collateral-type.ts#L3-L7)),
  and a `RegulatoryState` of `auditPublicKey` plus `blackListMerkleRoot`
  ([ROLLUP `regulatory-state.ts#L6-L8`](https://github.com/zkUSD-Protocol/rollup/blob/9ed45ce192c1e09f2cea5be5a8f4a52269aa2096/src/domain/regulatory/regulatory-state.ts#L6-L8)).
  CONFIRMED-CODE / CONFIRMED-DOCS.

"Fork zkUSD" therefore has three possible meanings, and none of them is finished.

### 1.3 Vault, collateral and liquidation

- **Collateral ratio 150%, MINA only.** `main` hardcodes `COLLATERAL_RATIO = Field.from(150)`
  ([CORE-MAIN `zkusd-vault.ts#L51`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-vault.ts#L51)).
  `develop` makes it a governed parameter, deployed at `collateralRatio: 150`,
  `liquidationBonusRatio: 110`, and a vault debt ceiling of 200,000 USD
  ([CORE-DEV `deployment.ts#L253-L255`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/src/deployment/deployment.ts#L253-L255)).
  CONFIRMED-CODE.
- **Liquidation is whole-vault, by anyone.** In `main`, a vault at health factor ≤ 100 is
  zeroed. The liquidator burns the whole debt and receives **all** the collateral
  ([CORE-MAIN `zkusd-vault.ts#L243-L273`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-vault.ts#L243-L273),
  [`zkusd-engine.ts#L636`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-engine.ts#L636)).
  `develop` caps the liquidator at 110% of the debt's MINA value and returns the rest to the owner
  ([CORE-DEV `system/vault.ts#L586-L598`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/src/system/vault.ts#L586-L598)).
  CONFIRMED-CODE.
- **No bad-debt path.** In both versions the liquidator must burn the **whole** debt. If the
  collateral is worth less than the debt, liquidating loses money, so nobody does it. There is no
  surplus buffer, stability pool or debt auction to absorb it. CONFIRMED-CODE, by absence in the
  engine and vault at both commits.
- **Fees.** There is no stability fee or interest. The only on-chain fee is a flat oracle fee paid
  from a MINA balance the protocol funds (`depositOracleFunds`, `submitPrice`,
  [CORE-MAIN `zkusd-engine.ts#L894`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-engine.ts#L894)).
  The README's "Any deposited MINA is delegated, and the vault earns those rewards" has **no code**
  behind it: there is no delegation call in the engine or vault at either commit. The feasibility
  study presents delegation as the economic model ("negative interest loans")
  ([`docs/zkUSD Feasibility Study.pdf`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/docs/zkUSD%20Feasibility%20Study.pdf),
  "Economics"). Fizk replaces this with a flat transfer fee shared with FIZK stakers
  (WP pp. 18, 27). CONFIRMED-CODE / CONFIRMED-DOCS.

### 1.4 Admin keys, emergency powers, upgradeability

- **`main`: one admin key does everything.** `ensureAdminSignature()` requires a signature from
  `protocolData.admin`
  ([CORE-MAIN `zkusd-engine.ts#L704`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-engine.ts#L704)).
  It gates `stopTheProtocol`/`resumeTheProtocol` (every vault action then fails in `getPrice`),
  the oracle whitelist, the oracle fee, `updateAdmin`, `updateFallbackPrice`, and the zkUSD token's
  `canPause`/`canResume`/`canChangeAdmin`
  ([`#L715`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-engine.ts#L715),
  [`#L818`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-engine.ts#L818),
  [`#L869`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-engine.ts#L869),
  [`#L1168`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-engine.ts#L1168)).
  `updateVerificationKey` is admin-gated too
  ([`#L807`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-engine.ts#L807)),
  but the engine is deployed with `setVerificationKey: impossibleDuringCurrentVersion()`
  ([`#L154`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-engine.ts#L154)),
  so that method can never succeed. CONFIRMED-CODE.
- **`develop`: council plus admin, and a key that can replace the code.** Parameter changes,
  emergency stop and engine VK upgrades go through `runGovUpdateCommon`, which asks the governing
  council's `canExecuteGovResolution`
  ([CORE-DEV `zkusd-engine.ts#L606`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/src/contracts/zkusd-engine.ts#L606),
  [`#L786`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/src/contracts/zkusd-engine.ts#L786)).
  The admin signature still gates `updateAdmin` and the token's pause/resume/admin/VK hooks. The
  code says "this will be a multisig"
  ([`#L1169`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/src/contracts/zkusd-engine.ts#L1169)).
  The engine is deployed with `setVerificationKey: Permissions.VerificationKey.proofOrSignature()`,
  justified in a comment: Mina resets VK permissions to signature at a hard fork anyway
  ([`#L141-L153`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/src/contracts/zkusd-engine.ts#L141-L153)).
  So **whoever holds the engine account's private key can swap the engine's code**, and the engine
  holds every vault's MINA. CONFIRMED-CODE.
- **Fizk: council-held audit key, a blacklist, and enclave key rotation.** Every note is encrypted
  twice, "once with the recipient's viewing key and once with an Audit Key controlled by
  governance". The Audit Key's private half is Shamir-split among the council, and the council
  "can launch an Audit Report to trace flows and, if desired, blacklist addresses". The council can
  recycle any enclave's keys, and a compromised observer quorum can do "extensive damage" (WP
  pp. 19–20, 30–31). CONFIRMED-DOCS.

### 1.5 Oracle trust model

- **`main`.** Up to 8 whitelisted oracles write signed prices into the even/odd tracker accounts.
  `settlePriceUpdate` takes the median of the slots, **padding missing or stale slots with the
  admin-set fallback price**
  ([CORE-MAIN `zkusd-price-tracker.ts#L59-L91`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-price-tracker.ts#L59-L91)).
  With 8 slots, the median is the mean of the 4th and 5th values. Five or more fallback copies
  always cover both, so **with three or fewer fresh oracle prices, the price is exactly the
  admin's**. A settled price has no expiry: `getPrice` checks only the emergency flag
  ([CORE-MAIN `zkusd-engine.ts#L1030`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-engine.ts#L1030)).
  CONFIRMED-CODE; the median arithmetic is this note's.
- **`develop`.** The user attaches a ZkProgram proof aggregating signed submissions. The engine
  checks the proof's VK hash, whitelist hash and block height, that the chain is within
  `validPriceBlockCount` blocks of it, and that at least `MINIMUM_VALID_ORACLE_SUBMISSIONS = 3`
  submissions were valid
  ([CORE-DEV `zkusd-engine.ts#L107`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/src/contracts/zkusd-engine.ts#L107),
  [`#L991-L1021`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/src/contracts/zkusd-engine.ts#L991-L1021);
  [`proofs/oracle-price-aggregation/verify.ts`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/src/proofs/oracle-price-aggregation/verify.ts)).
  The devnet set is **four** real oracles: three `oracleN.fizk.xyz` hosts the team ran, and one at
  `zkusd.oracle.nori.global`. The other four slots are a dummy key, and the config throws on any
  network other than devnet
  ([CORE-DEV `config/oracles.ts#L20-L66`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/src/config/oracles.ts#L20-L66)).
  The author wrote: "Currently we are running all the oracles"
  ([forum t/6786, 2025-04-07](https://forums.minaprotocol.com/t/zkusd-beta-readiness-completing-the-protocol-for-full-operational-testing/6786)).
  CONFIRMED-CODE / CONFIRMED-DOCS.

### 1.6 Known issues, including one found here

No issue tracker records anything: `core` has one issue in total. These come from reading the
code:

1. **A likely price forgery on `develop` (devnet config).** UNCONFIRMED as an exploit, because it
   was not run. Each fact below is CONFIRMED-CODE:
   - The dummy oracle's **private key** is committed: `DUMMY_ORACLE_KEY = PrivateKey.fromBase58('EKF9wz…')`
     ([`config/oracles.ts#L20-L22`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/src/config/oracles.ts#L20-L22)).
   - The whitelist fills slots 5–8 with its public key
     ([`#L55-L57`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/src/config/oracles.ts#L55-L57)).
   - The aggregation program counts a submission as valid when its signature verifies, its key
     matches that slot, its block is current, its price is positive, and `isDummy.not()`. Here
     `isDummy` is a field of the **prover-supplied private input**
     ([`proofs/oracle-price-aggregation/prove.ts#L22-L28`, `#L86-L104`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/src/proofs/oracle-price-aggregation/prove.ts#L22-L104)).
   - The engine verifies nothing more about which keys signed (§1.5).

   So whoever builds the proof can sign four submissions at any price with the committed key, mark
   them `isDummy = false`, and pass the threshold of 3 without a single real oracle. Then they
   can mint zkUSD against almost no MINA. It is devnet config, but it is the only oracle config
   the repository has.

2. **The admin is the oracle on `main`** whenever fewer than four oracles report (§1.5).
3. **No bad-debt handling** (§1.3). The design's own risk is MINA collapsing faster than anyone
   liquidates, and nothing absorbs that loss.
4. **Dead and contradictory code on `main`**: `updateVerificationKey` can never run (§1.4); the
   README advertises delegation that doesn't exist (§1.3); comments say `@param secret` on methods
   that take a `PublicKey owner`
   ([CORE-MAIN `zkusd-vault.ts#L73-L82`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/zkusd-vault.ts#L73-L82)).
5. **Pre-Mesa.** `o1js` is `^2.*` on `main` and `^2.2.0` on `develop`
   ([CORE-MAIN `package.json`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/package.json),
   [CORE-DEV `package.json`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/package.json)).
   Mesa changed every verification key ([spike #1512](zk-note-verifier-spike.md)), so nothing
   deployed or tested by zkUSD carries over without recompiling and retesting. The token comes from
   `@minatokens/token` (`^0.5.5` on `main`, `^0.16.16` on `develop`), zkcloudworker's package
   (`npm view @minatokens/token`: last modified 2025-02-05), not the archived MinaFoundation
   repository. CONFIRMED-CODE.

### 1.7 Tests, audit, licence, "private"

- **Tests.** `main` has 17 unit test files on o1js `LocalBlockchain`. `develop` has about 40 (unit,
  governance, upgradability) plus lightnet integration and concurrency suites. A Python risk model
  lives in `tools`. No coverage figure is published. CI runs on push
  ([CORE-DEV `.github/workflows/ci.yml`](https://github.com/zkUSD-Protocol/core/blob/2ea2df04f6e328333765f91a108b3e2903eaf316/.github/workflows/ci.yml)),
  and the API lists no recent runs. CONFIRMED-CODE.
- **Audit: none.** The feasibility study lists "Smart Contract Audit" and "Seek legal & regulatory
  advice" under next steps. The beta-readiness proposal puts the audit after its own scope. The
  last post says "We have engaged a lawyer and are currently working together to understand at what
  stage we will be able to launch on mainnet"
  ([forum t/6756, 2025-04-07](https://forums.minaprotocol.com/t/zkusd-deployment-readiness/6756)).
  No audit report exists in any org repository. CONFIRMED-DOCS for the plan. **That no audit was
  done: UNCONFIRMED**, since absence can't be proven. Nothing suggests one.
- **Licence: forkable.** `core`'s LICENSE is the unmodified Apache License 2.0 text with no
  copyright line, and there is no NOTICE file
  ([CORE-MAIN `LICENSE`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/LICENSE);
  `develop` is identical). Apache-2.0 §2 grants a "perpetual, worldwide, non-exclusive, no-charge,
  royalty-free, irrevocable copyright license to reproduce, prepare Derivative Works of …", and §4
  requires keeping the licence, marking changed files, and keeping any NOTICE. `cli` is Apache-2.0
  too. **`rollup` has no LICENSE file.** Its `package.json` says `"license": "Apache-2.0"`, but a
  manifest field isn't a grant, so forking `rollup` would mean asking first. `services`, `worker`,
  `oracle-price-provider`, `ui`, `tools` and `whitepaper` carry no licence, so all rights are
  reserved by default. CONFIRMED-CODE. Whether a `package.json` field is enough of a grant is a
  legal question, so UNCONFIRMED.
- **"Private" means nothing in `core`.** `VaultState` (collateral, debt, owner) is zkApp app
  state, and every action emits events with the amounts and the vault address
  ([CORE-MAIN `src/events.ts`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/src/events.ts)).
  zkUSD is a standard `FungibleToken`, whose `approveBase` emits a `BalanceChange` event with
  address and amount for each update
  ([MFT `FungibleToken.ts#L222-L244`](https://github.com/MinaFoundation/mina-fungible-token/blob/8dbf75a4a50d236d9a9ab137a7fd451bafddae41/FungibleToken.ts#L222-L244)).
  Privacy exists only in the Fizk rollup's notes. There, a governance audit key can decrypt every
  note, and notes carry no asset field
  ([ROLLUP `zkusd-note.ts#L44-L50`](https://github.com/zkUSD-Protocol/rollup/blob/9ed45ce192c1e09f2cea5be5a8f4a52269aa2096/src/domain/zkusd/zkusd-note.ts#L44-L50)).
  CONFIRMED-CODE.

### 1.8 Status

- `fizk.xyz` redirects to `/lander`, which redirects to `forsale.godaddy.com/forsale/fizk.xyz`.
  `oracle1.fizk.xyz` and `zkusd.oracle.nori.global` don't answer (`curl`, 2026-10-27).
  CONFIRMED-LIVE.
- The forum's last zkUSD post is 2025-04-07 (topics 6584, 6756 and 6786), and o1Labs' 2026 updates
  don't mention it (TOKENS §4). CONFIRMED-DOCS.
- **The project is abandoned.** Judgement, from the domain sale, the dead oracle hosts and 14
  months without a push.

---

## 2. Ways to get a stable asset on Mina, compared

### (a) A CDP against MINA (zkUSD-style)

- **Reflexivity.** The only collateral is MINA, so the stablecoin is weakest exactly when MINA
  falls. Liquidators are paid in MINA (§1.3) and have to sell it to recover their zkUSD, which
  pushes the price down further. The author conceded there was nowhere to defend a peg:
  "As there are no money markets on Mina … there is no market where the price of zkUSD could
  diverge from its intended peg … zkUSD would be effectively useless in its current state"
  ([forum t/6584, post 4](https://forums.minaprotocol.com/t/zkusd-algo-stablecoin/6584)).
  TOKENS records Solis as the first and only DEX live on mainnet (Q1 2026). CONFIRMED-DOCS.
- **Concurrency.** zkUSD's architecture analysis lists the constraints it designed around: "Only
  one user interaction per block can be processed", a block time of "approximately 3 minutes", and
  "a limit of 9 account updates per transaction"
  ([CORE-MAIN `docs/architectural-analysis.md`](https://github.com/zkUSD-Protocol/core/blob/2142a531c3e9bdc659bf8495efbb377f32fd8847/docs/architectural-analysis.md)).
  `FungibleToken.approveBase` documents the same limit of 9
  ([MFT `#L218-L220`](https://github.com/MinaFoundation/mina-fungible-token/blob/8dbf75a4a50d236d9a9ab137a7fd451bafddae41/FungibleToken.ts#L218-L220)).
  Liquidations compete for the same block space as the panic that causes them. CONFIRMED-DOCS as
  zkUSD stated it in Dec 2024. **Whether Mesa changed the block time or the account-update limit:
  UNCONFIRMED**, not checked against Mesa's release notes.
- **Hard forks.** Every hard fork resets VK permissions, so the engine's key holder has to act
  before vaults can move again (§1.4, and the forum post in §1.2). A peg that depends on liquidations
  can't afford that gap.
- **Fit for TOON:** poor. See §4.

### (b) A bridged stablecoin

- **Nori (Ethereum → Mina): ETH only, devnet staging** (TOKENS §4). Head is unchanged since
  TOKENS read it (`2a73bdf8`, 2026-09-29), and its tree has no ERC-20 or USDC path
  (`git/trees` search for `erc20|usdc|mirror`: no match). CONFIRMED-CODE.
- **Nori's newer bridges show it can mirror ERC-20s, on other chains.** `nori-tempo-sdk` (Oct 2026)
  locks ERC-20s on Ethereum and mints a TIP-20 mirror on Tempo, with `test-fork/` tests "against
  real mainnet USDC". It says the mirror is "backed 1:1 by what it holds", and that the ERC-20's
  pause follows it to the mirror. Its operator is an OpenZeppelin `TimelockController` "fronted by
  a Safe multisig"
  ([`nori-tempo-sdk` README](https://github.com/Nori-zk/nori-tempo-sdk/blob/ef244472e0298df79ffb02e9307094a183285793/README.md)).
  `nori-solana-sdk` locks ETH only
  ([README](https://github.com/Nori-zk/nori-solana-sdk/blob/ee4cc78d96054d8e0238d2c44f4495a021ab7ffb/README.md)).
  CONFIRMED-CODE. **A USDC mirror on Mina: UNCONFIRMED and not announced.** If one shipped, its
  trust model would be Nori's SP1 light-client proof plus a timelocked multisig over the Ethereum
  escrow, and the asset would be "bridged USDC", not Circle's liability.
- **Circle CCTP is burn-and-mint and only reaches chains Circle supports.** Mina isn't one of them
  (TOKENS §4). So native USDC can't be "bridged" to Mina. Only a wrapped claim on locked USDC can.
- **Sotatek wUSDC**: cancelled in April 2025 (TOKENS §4).

### (c) A custodial, issuer-backed token on Mina

Technically this is the easy option: a `FungibleToken` with an admin contract that mints against
reserves. The hard parts are legal and operational:

- **United States.** The GENIUS Act (Public Law 119-27, 2025-07-18), §3(a): "It shall be unlawful
  for any person other than a permitted payment stablecoin issuer to issue a payment stablecoin in
  the United States." §4(a)(1)(A) requires "identifiable reserves … on an at least 1 to 1 basis".
  §2(16) defines a "lawful order" as one that "requires a person to seize, freeze, burn, or prevent
  the transfer of payment stablecoins issued by the person"
  ([govinfo](https://www.govinfo.gov/link/plaw/119/public/27?link-type=html)). CONFIRMED-DOCS.
- **European Union.** MiCA (Regulation (EU) 2023/1114) Art. 48(1) lets an e-money token be offered
  only by its issuer, authorised "as a credit institution or as an electronic money institution"
  ([EUR-Lex CELEX:32023R1114](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:32023R1114)).
  EUR-Lex returned no text to this machine. The wording was read in a reading-aid copy
  ([springlex](https://www.springlex.eu/en/packages/mica/mica-regulation/article-48/)).
  CONFIRMED-DOCS at one remove.
- **Freeze versus a shielded vault.** A token's admin can freeze transfers in a `FungibleToken`
  only at token level (`pause`), or per account in a custom admin. A zk-note vault is one token
  account holding every payer's notes. An issuer under a lawful order against one payer therefore
  has two choices: freeze **the whole vault**, or demand a view into the notes. Fizk chose the view
  (§1.4). Judgement, from the code in §3 and the Act's definition.
- **Fit for TOON:** TOON as issuer is a regulated money business: licence, reserves, attestations,
  redemptions, sanctions screening. That is a different company from one that earns routing fees.
  A third-party issuer (Circle, Tether, a smaller licensed issuer) would be fine to use, but none
  has issued on Mina.

### (d) Zeko or another Mina-adjacent chain

- **Zeko no longer settles on Mina.** Its README says "Zeko is a zero-knowledge rollup settling on
  Ethereum … The Zeko rollup can settle natively to Ethereum as well as Mina". It also says that
  Mina-derived types "do not imply that the current rollup settles on Mina"
  ([`zeko-labs/zeko` README](https://github.com/zeko-labs/zeko/blob/49fb253de86ce9188bcd289a8d5bb09e04164084/README.md)).
  Its Ethereum settlement repository says "Native balances and fees on Zeko are in ETH", and it
  bridges "registered ERC-20" tokens through a `ZekoAssetRegistry`
  ([`ethereum-settlement` README](https://github.com/zeko-labs/ethereum-settlement/blob/053bbb4ded6013ada3759f8577f59664ba3b2cc5/README.md),
  [`contracts/src/ZekoAssetRegistry.sol`](https://github.com/zeko-labs/ethereum-settlement/blob/053bbb4ded6013ada3759f8577f59664ba3b2cc5/contracts/src/ZekoAssetRegistry.sol)).
  Its own security model calls it "an experimental, permissioned testnet PoC". `ADMIN_ROLE` can
  "emergency-withdraw any native custody, and emergency-withdraw unencumbered ERC-20 custody",
  which it calls "a strong trust assumption"
  ([`docs/content/reference/security-model.md`](https://github.com/zeko-labs/ethereum-settlement/blob/053bbb4ded6013ada3759f8577f59664ba3b2cc5/docs/content/reference/security-model.md)).
  It "has not yet been promoted to a persistent live Sepolia deployment"
  ([`docs/content/status.md`](https://github.com/zeko-labs/ethereum-settlement/blob/053bbb4ded6013ada3759f8577f59664ba3b2cc5/docs/content/status.md)).
  CONFIRMED-CODE / CONFIRMED-DOCS.
- **`zeko-labs/x402-zeko`** offers "Ethereum and Base USDC exact settlement" plus a "Zeko rail"
  of zkApp settlement helpers
  ([README](https://github.com/zeko-labs/x402-zeko/blob/928ce96ea9b8bec3737209b769a42b693c892a95/README.md)).
  Its USDC lives on EVM, not on Zeko. CONFIRMED-CODE.
- **What this means for TOON.** A ZK-note vault on Zeko would be a vault on an Ethereum L2 whose
  verification-key and settlement assumptions differ from Mina L1's. ADR 0080 pins a Mina vault,
  a Mina network and Mina GraphQL reads. Moving to Zeko is a different record, not a token choice.
  The Fizk rollup (Sui sequencing, Nitro enclaves) is abandoned (§1.8). Judgement.

### (e) No stablecoin: native MINA vault, USD-priced routes

ADR 0080 decision 7 already says how: the zk edge prices allowance-paid packets in the vault's
asset, and "its forward to the next hop crosses a denomination at a declared rate (0071)". The
pieces exist:

- **A rate.** 0071's rate is a rational over base units, rounded in the connector's favour, with
  `spread`, `ttl` and `max_move` guards. A pair that can't self-source runs "a **static rate row**
  the operator tends by hand"
  ([ADR 0071](../adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md), decisions 3–5).
  MINA has no TWAP source the `RateSource` port reads, so MINA→USDC is a static row (TOKENS §5.2,
  point 3). CONFIRMED-DOCS (this repository).
- **Who carries volatility, over what window** (Judgement, from ADR 0080's flow):

  | Party               | What they hold                                  | Window                                                                                            | Bounded by                                                                          |
  | ------------------- | ----------------------------------------------- | ------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
  | **Payer**           | Its note balance, in MINA                       | From deposit until it escapes or spends to zero: days to months                                   | Nothing. It's the payer's MINA, just as an ETH balance would be                     |
  | **Payer**           | The allowance's purchasing power                | One allowance: the lifetime the zk edge publishes                                                 | The edge's MINA prices are fixed at open. A move inside the window favours one side |
  | **Zk edge**         | Δ owed in MINA, already paid downstream in USDC | From each forward until the collector withdraws Δ: allowance lifetime, plus close, plus a Mina tx | The **open budget cap** (decision 5) times the MINA/USD move over that lag          |
  | **Zk edge**         | Collected MINA, if not sold at once             | The operator's choice                                                                             | Its own treasury policy                                                             |
  | **Downstream hops** | Nothing new                                     | —                                                                                                 | They're paid in their own unit (0071 decision 1)                                    |

  The zk edge's `spread` pays for its exposure, as 0071 intends for any dealing hop. A static
  row's staleness is the operator's own risk, and the run-or-fail-loudly rule says a rate it no
  longer trusts should fail loudly rather than deal quietly (0071, "Staleness is an outage").

- **What this gives up.** A payer can't hold a dollar balance in its note. A payer who wants one
  must deposit, spend quickly, and re-deposit. That costs privacy, because each deposit is the one
  step tied to an address (ADR 0080; #1511 step 1). Judgement.

### Comparison

| Option                   | Exists on Mina L1 today          | Who TOON trusts                                 | What TOON operates                                                        | Regulatory weight on TOON     | Fit with zk notes                        |
| ------------------------ | -------------------------------- | ----------------------------------------------- | ------------------------------------------------------------------------- | ----------------------------- | ---------------------------------------- |
| (a) Fork zkUSD / own CDP | No (abandoned devnet)            | Its own oracles, admin/council key, liquidators | Oracles, liquidation keepers, governance, audits, hard-fork upgrades, peg | Uncertain; zkUSD stalled here | Possible; issuer = TOON can pause vault  |
| (b) Bridged USDC         | No (Nori ETH-only on Mina)       | Bridge proofs plus bridge multisig, then Circle | Nothing, once someone else ships it                                       | Low (TOON is a user)          | Possible; bridge or issuer pause applies |
| (c) Issuer-backed token  | No (no issuer on Mina)           | The issuer's reserves and freeze policy         | Nothing if third-party; a licensed money business if TOON                 | Very high if TOON issues      | Possible; issuer pause applies           |
| (d) Zeko / Mina-adjacent | Not on Mina; Zeko is on Ethereum | Zeko PoC roles; Ethereum                        | A different chain adapter and a different ADR                             | Low                           | Out of ADR 0080's scope                  |
| (e) Native MINA + rate   | **Yes**                          | The operator's own declared rate                | A static rate row; collector sells MINA                                   | None new                      | **As designed**                          |

---

## 3. Fit with zk notes

### 3.1 Can the vault hold a custom token?

- **Yes, as a token-holder zkApp.** A token balance lives in the account `(vault address, tokenId)`
  (TOKENS §2), and a zkApp can be deployed on that token account. o1js's DEX example does exactly
  this: `new DexTokenHolder(this.address, tokenX.deriveTokenId())`, called "inside a token
  X-approved callback"
  ([O1JS `src/examples/zkapps/dex/dex.ts#L142-L145`, `#L226`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/examples/zkapps/dex/dex.ts#L142-L226)).
  zkUSD's own vaults are zkApps on the engine's token accounts (§1.2). CONFIRMED-CODE.
- **The standard token allows it.** `FungibleToken.approveBase` refuses a permissions update only
  if `access` or `receive` is set to anything but `none`. A vault needing `send: proof` passes
  ([MFT `FungibleToken.ts#L203-L215`](https://github.com/MinaFoundation/mina-fungible-token/blob/8dbf75a4a50d236d9a9ab137a7fd451bafddae41/FungibleToken.ts#L203-L215)).
  CONFIRMED-CODE. That a full vault proof program deploys and runs under it: UNCONFIRMED, not
  built.

### 3.2 Does token-owner approval break unlinkability or composability?

- **Unlinkability: no.** An allowance is opened by an off-chain proof sent to the zk edge, and
  paid down by session-key claims (ADR 0080 decisions 2–3). Neither touches the chain, so the token
  owner never sees them. What the owner approves are the on-chain moves: deposit, the collector's
  withdrawal of Δ, and escape. Those are public on Mina anyway, token or not. The extra
  `BalanceChange` event (§1.7) repeats what the account updates already show. The anonymity set is
  the vault's active notes, as before. Judgement, from the cited code.
- **Composability: costs, not blockers.**
  - Every vault transaction carries the token owner's account update, inside the 9-update forest
    limit (§2a).
  - `approveBase` reads `paused` as a precondition. It's read-only, so it doesn't contend with the
    vault's own root update. The vault's root update still serialises deposits per block, just as
    it does for a native vault.
  - The token owner's verification key is the issuer's to re-sign after each hard fork (§1.2).
    Until the issuer does, the vault can't move that token, **on top of** the connector's own
    hard-fork repin (spike #1512).

  CONFIRMED-CODE for the precondition, judgement for the rest.

- **The issuer gets a switch over the whole vault.** `FungibleToken.approveBase` starts with
  `this.paused.getAndRequireEquals().assertFalse(…)`
  ([MFT `#L223`](https://github.com/MinaFoundation/mina-fungible-token/blob/8dbf75a4a50d236d9a9ab137a7fd451bafddae41/FungibleToken.ts#L223)),
  and pause is the admin contract's to grant (`canPause`). zkUSD's engine grants it on an admin
  signature (§1.4). While paused, **no deposit, collection or escape moves**. Collection stalls
  while the zk edge has already paid downstream: open budget becomes loss. Escapes stall, so payers
  can't exit. A per-address blacklist in a custom admin could hit the vault's single token account
  too. CONFIRMED-CODE for the mechanism. The consequences are judgement.

### 3.3 Does #1521's note commitment carry an asset?

**No, and it doesn't need to for a single-asset vault.** CONFIRMED-CODE at CONNECTOR:

- `note_commitment = Poseidon(TAG_COMMIT, secret, anchor, C.x, C.y)`, `leaf = Poseidon(TAG_LEAF, commitment)`,
  `nullifier = Poseidon(TAG_NULLIFIER, secret, anchor)`, and
  `context = Poseidon(TAG_CTX, edge identity, session key, b, expiry)`. **None takes a token ID**
  ([`crates/connector-zk-note/src/derive.rs#L13-L46`](https://github.com/toon-protocol/connector/blob/605f6efe58a18267af8d8934c35181fdc1d936a0/crates/connector-zk-note/src/derive.rs#L13-L46)).
- The balance commitment is `B·G_v + r·G` with **one** value generator `G_v`, from the seed
  `toon-zk-note-gen-value`
  ([`commitment.rs#L14-L29`](https://github.com/toon-protocol/connector/blob/605f6efe58a18267af8d8934c35181fdc1d936a0/crates/connector-zk-note/src/commitment.rs#L14-L29),
  [`tags.rs#L94-L96`](https://github.com/toon-protocol/connector/blob/605f6efe58a18267af8d8934c35181fdc1d936a0/crates/connector-zk-note/src/tags.rs#L94-L96)).
- The signed state message is `(vault, C.x, C.y, anchor)`, so **the vault is bound**, and with one
  asset per vault, so is the asset
  ([`state.rs#L23-L45`](https://github.com/toon-protocol/connector/blob/605f6efe58a18267af8d8934c35181fdc1d936a0/crates/connector-zk-note/src/state.rs#L23-L45)).
  A tier is "in the vault asset's smallest unit"
  ([`tiers.rs#L11-L15`](https://github.com/toon-protocol/connector/blob/605f6efe58a18267af8d8934c35181fdc1d936a0/crates/connector-zk-note/src/tiers.rs#L11-L15)).

**What a multi-asset vault would need.** Zcash's shielded-assets design is the reference: "the
value of different Asset Identifiers in a given transaction will be committed using a different
value base point", and "An OrchardZSA note differs from an Orchard note by additionally including
the Asset Base" ([ZIP 226](https://zips.z.cash/zip-0226)). CONFIRMED-DOCS. For TOON that means:

- a per-asset generator `G_v(asset)` in place of the single `G_v`;
- the asset inside `note_commitment`, and probably inside `context`, so an allowance binds its
  asset;
- a new tag or seed scheme.

Every one of these is wire law in `tags.rs`, so it is breaking for every note and proof ever made.
A **single-asset vault needs no change**: a USD-token vault would be a second vault with its own
address, VK, asset string and tiers.

**Recommendation for ADR 0080 or #1522:** state "a vault holds exactly one asset, and the zk-note
table names it" as a rule. Then the commitment layout is final, and a stable-asset vault later is
additive. TOKENS §5.3 asked this question. This is the answer that keeps #1521 as built.

---

## 4. Recommendation

### 4.1 Facts this rests on (all cited above)

- No stable asset exists on Mina L1, and none is close: no issuer, Nori ETH-only, Zeko moved to
  Ethereum.
- zkUSD's latest code is `develop` (May 2025). It is pre-Mesa and unaudited. Its devnet oracle
  config appears forgeable, and its admin or engine key can stop the protocol, set the fallback
  price (on `main`), pause the token, and replace the engine's code (on `develop`).
- The team pivoted to a Sui- and enclave-based rollup with a governance audit key, then stopped.
  The domain is for sale.
- `core` is Apache-2.0 and legally forkable. `rollup` has no LICENSE file.
- A zk-note vault can hold a token, but the issuer then controls a pause over all of it.
- #1521's commitment is single-asset by construction, and the state message binds the vault.

### 4.2 Judgement

**Don't fork zkUSD.** What a fork buys: about 1,200 lines of pre-Mesa engine code, a council
contract, and an oracle-proof program. What TOON would take on:

| Duty                    | What it means in practice                                                                                                                    |
| ----------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Port and re-audit       | Port to o1js 3.x (Mesa), fix §1.6, and pay for a full audit. zkUSD never got that far                                                        |
| Run an oracle network   | At least 3 independent signers per block, an incentive scheme zkUSD never defined, and a fee budget paid in MINA                             |
| Run liquidation keepers | Bots that watch every vault and act within blocks during a MINA crash, on a chain with no deep MINA/stable market                            |
| Hold the keys           | The admin, council or multisig over pause, parameters and engine VK, plus a hard-fork re-signing ceremony each time                          |
| Absorb bad debt         | No mechanism exists. TOON's treasury or the holders eat it                                                                                   |
| Defend the peg          | No market exists to defend it in. zkUSD's author said so                                                                                     |
| Legal                   | Get an opinion on whether a crypto-collateralised token with redemption is a regulated stablecoin anywhere TOON operates. zkUSD stopped here |

None of this earns a routing fee, and every failure lands on payers' zk notes, which ADR 0080
exists to protect. CLAUDE.md's "How anyone earns" says a change that takes value off a path without
a fee saying so "is a change to that model, not a refactor". Running a stablecoin is a second
business model.

**Don't build our own** for the same reasons. A from-scratch CDP has every row above and none of
zkUSD's head start. A custodial token makes TOON an issuer under §2(c)'s law.

**Do this instead:**

1. **Keep the vault in native MINA** (option e). In ADR 0080 or #1522, write down:
   - the volatility allocation in §2(e);
   - that the MINA→next-hop rate is a 0071 static row the operator tends;
   - that the open-budget cap is also the cap on MINA price exposure.
2. **Rule "one vault, one asset"** (§3.3), and keep the asset string and decimals as config values
   (TOKENS §5.3). Then a later stable vault is a new vault, a new table row and a new verification
   key, with no change to #1521's primitives.
3. **Write down the conditions for revisiting**, so the decision isn't reopened by drift. Any one
   of these, **live on Mina L1 mainnet and audited**, reopens it:
   - a stablecoin issued and redeemed by a licensed third party;
   - Nori, or another proof-based bridge, mirroring USDC with a published trust model.

   Even then, use it; don't issue it. Before adopting it, read the issuer's pause and blacklist
   powers as a property of the vault (§3.2).

### 4.3 What would change this judgement

- Evidence that a licensed issuer intends to issue on Mina. That would make option (c)-as-user
  cheap.
- A user requirement that payers hold **dollar** balances privately for long periods. Option (e)
  can't meet it. Even so, the answer would be to wait for (b) or (c), not to run a CDP.
- The `develop` oracle reading in §1.6 being wrong. That would remove one argument against forking,
  but not the operational ones.

---

## Unconfirmed, and why

- **The `develop` price forgery** (§1.6) was reasoned from code and not run. Exploitability also
  depends on the on-chain whitelist matching `config/oracles.ts`, and the devnet deployment is
  presumably gone.
- **That zkUSD was never audited.** No report exists in the org and none was announced, but an
  absence can't be proven.
- **Whether `rollup`'s `package.json` licence field is a grant.** It's a legal question, and the
  repository has no LICENSE file.
- **Mesa's block time and account-update limit.** zkUSD's figures are from December 2024. Mesa's
  release notes weren't read.
- **A Nori USDC leg for Mina.** It isn't in the SDK, and no announcement was found.
- **MiCA Art. 48 wording** was read in a reading-aid copy, because EUR-Lex returned no content to
  this machine.
- **That a full token-holding vault proof program deploys and runs under `FungibleToken`.** The
  permission check permits it (§3.1), but nothing was built.
