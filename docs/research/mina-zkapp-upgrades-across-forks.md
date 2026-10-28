# Who upgrades a Mina channel zkApp across hard forks?

> **Research note, not a decision record.** It decides nothing. The question is #1559, part of
> #1548 (an x402 batch-settlement channel zkApp on Mina, living in `toon-protocol/zk_toon`, that
> the connector binds to by zkApp address and verification-key hash). The owner has accepted four
> answers (Q1–Q4, restated in the last section). This note checks their factual premises; it does
> not re-decide them. It builds on
> [`mina-x402-batch-settlement.md`](mina-x402-batch-settlement.md) (cited as **CHANNEL**, mostly
> §3.1, §3.4, D5, D7 and "Unconfirmed") and
> [`zk-note-verifier-spike.md`](zk-note-verifier-spike.md) (cited as **SPIKE**), and does not redo
> their ground. Primary sources only. Written 2026-10-28, and every source was read that day.

**Evidence tags.** CONFIRMED-CODE means read in source at a pinned commit. CONFIRMED-DOCS means
read in the maintainer's own documents, release notes or issues. CONFIRMED-HISTORY means read in
this repository's git history. UNCONFIRMED means none of those, and says why. A sentence marked
**Inference** is this note's reasoning from confirmed facts, not a fact.

**Sources pinned.**

- **Mina daemon**: tag [`4.0.3`](https://github.com/MinaProtocol/mina/tree/4.0.3) (`73134bfd`,
  2026-10-22, the current Mesa release). Cited as **MINA** plus a path. Also tag
  [`3.4.0`](https://github.com/MinaProtocol/mina/tree/3.4.0) (last Berkeley release), tag
  [`3.5.0-mainnet-stop-slot`](https://github.com/MinaProtocol/mina/releases/tag/3.5.0-mainnet-stop-slot)
  (published 2026-08-21) and its config commit
  [`20735c4c`](https://github.com/MinaProtocol/mina/commit/20735c4c) (2026-08-13), and tag
  [`4.0.0-mainnet-mesa`](https://github.com/MinaProtocol/mina/releases/tag/4.0.0-mainnet-mesa)
  (published 2026-09-03T15:35Z).
- **Mina RFC 0052**, _Verification Key Permissions_:
  [MINA `rfcs/0052-verification-key-permissions.md`](https://github.com/MinaProtocol/mina/blob/4.0.3/rfcs/0052-verification-key-permissions.md).
  Cited as **RFC0052**.
- **o1js**: [`o1-labs/o1js@a428f9b9`](https://github.com/o1-labs/o1js/tree/a428f9b97817177ef4b5868cf148c0ac67a1048d)
  (`main`, 2026-10-27, the commit CHANNEL pins), plus npm publish times. Cited as **O1JS**.
- **o1Labs guide**: [_Upgrading zkApps to Mesa (o1js 3.0)_](https://docs.o1labs.org/o1js/zkapps/upgrade-mesa).
  Cited as **GUIDE**. **Mina docs**: [Mesa upgrade examples](https://docs.minaprotocol.com/network-upgrades/mesa/upgrade-steps/examples)
  and [post-upgrade](https://docs.minaprotocol.com/network-upgrades/mesa/upgrade-steps/post-upgrade).
  Cited as **MESA-DOCS**.
- **mina-multi-sig**: [`o1-labs/mina-multi-sig@e9b5ffc8`](https://github.com/o1-labs/mina-multi-sig/tree/e9b5ffc8118ab871e3b7e4d86f2bc6c7e48375c2)
  (the commit CHANNEL pins). `Raspberry-Devs/mina-multi-sig` is a GitHub fork of it, and the
  README's badges and the audit name Raspberry Devs as the developer. Cited as **MMS**.
- **mina-fungible-token**: [`MinaFoundation/mina-fungible-token@8dbf75a4`](https://github.com/MinaFoundation/mina-fungible-token/tree/8dbf75a4a50d236d9a9ab137a7fd451bafddae41).
  Cited as **MFT**.

---

## Verdict

**Q1's mechanism is real and lives in the protocol, not only in o1js.** After a hard fork bumps
the transaction version, a `setVerificationKey` permission of `Impossible` or `Proof` is read as
`Signature`. `setPermissions: impossible` stays impossible. **But Q1's permission list is not
enough to make the key dormant.** It has two gaps:

- **`access` must be `proof`.** Otherwise anyone can close the fallback window after a fork with
  a no-op account update, and the VK is then locked for good. Every account update bumps the
  account's `txn_version` to current, not only a VK change.
- **Every other permission must be `impossible`.** o1js's `Permissions.default()` leaves
  `setTiming`, `setDelegate`, `setZkappUri`, `setTokenSymbol`, `incrementNonce` and `setVotingFor`
  at `signature`. Those work **at any time**, not only after a fork. `setTiming` alone lets the key
  holder put a vesting lock on the escrow balance.

**The freeze is confirmed.** Mesa carried accounts over with their old VKs and permissions
unchanged. Official guidance says a zkApp "cannot process transactions on Mesa until its
verification key is updated", by a transaction its own key signs. The window is **one VK
install per fork**: the install itself closes it.

| #   | Question                                                     | Answer                                                                                                                                                                                                                                       | Tag                                  |
| --- | ------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------ |
| 1   | Where does `*DuringCurrentVersion` revert to `signature`?    | **In the transaction logic** (`zkapp_command_logic.ml`), the same functor the ledger and the transaction snark both run. "Version" is the **transaction protocol version** (`txn_version`): 3 for Berkeley, 4 for Mesa                       | CONFIRMED-CODE                       |
| 1   | Does anything else fall back?                                | **Only `access`, and only `Proof`→`Signature`.** `setPermissions`, `editState`, `send`, `editActionState` and the rest have no version and never fall back. `impossible` stays impossible everywhere except the VK permission                | CONFIRMED-CODE                       |
| 1   | Can the post-fork signature change other permissions?        | **No.** RFC 0052 proposed it ("even if `set_permission` disagrees"), but 4.0.3 does not implement it: the `set_permissions` check has no fallback                                                                                            | CONFIRMED-CODE                       |
| 1   | Is the key otherwise dormant?                                | **Only if every other permission is `impossible` (or `proof`).** With `Permissions.default()` it can set timing, delegate, URI, symbol, nonce and voting-for at any time. And `access: proof` is needed to keep the window closed to others  | CONFIRMED-CODE                       |
| 2   | Are proof updates against an old VK refused after a fork?    | **Yes**, until the VK is reset. "The contract cannot process transactions on Mesa until its verification key is updated"                                                                                                                     | CONFIRMED-DOCS, (SPIKE RAN)          |
| 2   | Does a fork migrate VKs or state?                            | **State, balance and permissions carry over unchanged; the VK carries over unchanged and stale.** `migrate_to_mesa` copies every field. Only vesting `timing` is rescaled for 90 s slots                                                     | CONFIRMED-CODE, DOCS                 |
| 2   | What happened to mainnet zkApps at Mesa?                     | **Owners re-keyed by signature after genesis**; that is the official procedure. No list of which zkApps did so was found                                                                                                                     | CONFIRMED-DOCS; examples UNCONFIRMED |
| 3   | Mesa's stop slots                                            | `slot_tx_end = 393800` (2026-09-03 10:00Z), `slot_chain_end = 393900` (15:00Z), `hard_fork_genesis_slot_delta = 60`, Mesa genesis 18:00Z. **Committed 21 days ahead, released 13 days ahead**                                                | CONFIRMED-CODE, DOCS                 |
| 3   | Halt length                                                  | **8 hours with no transactions** (10:00–18:00Z), plus however long each zkApp owner took to re-key                                                                                                                                           | CONFIRMED-DOCS; Inference            |
| 3   | Is the slot continuous?                                      | **Since-genesis: yes.** Mesa genesis = 564480 + 393900 + 60 = **958440**, which `validWhile` uses. Since-hard-fork: resets. Slot **duration** halved                                                                                         | CONFIRMED-CODE                       |
| 4   | Could the post-fork VK be known before the fork?             | **Yes for Mesa.** o1js 3.0.0 shipped 2026-08-18, 16 days before (prereleases since March). VK compilation is deterministic and CI-pinned. **But 3.1.0 changed VKs again 20 days after the fork, for a security fix**                         | CONFIRMED-CODE, DOCS                 |
| 5   | Can the key be a threshold key?                              | **Only as a single Schnorr key**, which a FROST group can produce. MMS does this: alpha, `frost-core` RC, audited only in part. Mina has no native multisig account, and a governing zkApp cannot act after a fork (its own VK is stale too) | CONFIRMED-CODE, DOCS                 |
| 6   | Can Rust read and match the VK hash, and sign the VK update? | **Read: yes**, `verificationKey { hash }` and `permissions { setVerificationKey { txnVersion } }`. **Match: yes** (SPIKE ran it). **Sign: yes in form**, via MMS `mina-tx`, given an o1js-compiled VK; one `mina-tx` build per era           | CONFIRMED-CODE; signing Inference    |
| 7   | Precedent                                                    | MFT uses exactly Q1's shape plus `access: proof`, with one admin keypair by default. No Mina zkApp publishing a timelock or notice period was found                                                                                          | CONFIRMED-CODE, DOCS                 |

---

## 1. The fallback, exactly

### 1.1 Where it lives

- The permission record carries a version on **one** field only:
  `set_verification_key : 'controller * 'txn_version`. Every other field is a bare controller
  ([MINA `mina_base/permissions.ml#L357-L372`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/mina_base/permissions.ml#L357-L372)).
  The protocol has no `impossibleDuringCurrentVersion` kind; o1js's name means
  `(Impossible, current txn_version)`
  ([O1JS `account-update.ts#L173-L183`, `#L230-L262`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/account-update.ts#L230-L262)).
  CONFIRMED-CODE.
- The fallbacks:

  ```ocaml
  let verification_key_perm_fallback_to_signature_with_older_version = function
    | Impossible | Proof -> Signature
    | t -> t

  let access_perm_fallback_to_signature_with_older_version = function
    | Proof -> Signature
    | t -> t
  ```

  ([MINA `permissions.ml#L69-L79`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/mina_base/permissions.ml#L69-L79);
  the in-circuit versions are at `#L304-L318`). CONFIRMED-CODE.

- They are applied when an account update is processed. `auth_with_fallback` reads the account's
  stored `txn_version` and, if it is older than current, runs the fallback. It is used for the
  `access` check and for the VK check, and **nowhere else**
  ([MINA `transaction_logic/zkapp_command_logic.ml#L1553-L1567`, `#L1623-L1647`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/zkapp_command_logic.ml#L1553-L1647)).
  That file is a functor run by both the ledger (`mina_transaction_logic.ml`) and the transaction
  snark (`transaction_snark.ml`), so the rule is consensus. CONFIRMED-CODE.
- "Older" is `t < current`, where
  `current = Protocol_version.(transaction current)`
  ([MINA `mina_numbers/txn_version.ml#L17-L27`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/mina_numbers/txn_version.ml#L17-L27)).
  `protocol_version_transaction` is **3** at
  [`3.4.0`](https://github.com/MinaProtocol/mina/blob/3.4.0/src/lib/node_config/version/node_config_version.ml)
  and **4** at
  [`4.0.0-mainnet-mesa`](https://github.com/MinaProtocol/mina/blob/4.0.0-mainnet-mesa/src/lib/node_config/version/node_config_version.ml)
  and `4.0.3`. o1js 3.x hard-codes `protocolVersions = { txnVersion: 4 }`
  (O1JS `src/bindings/crypto/constants.ts#L504`). RFC 0051 defines the version string as
  `<txn_version>.<net_version>.<impl_version>`, where only a transaction-system change bumps the
  first. CONFIRMED-CODE, DOCS.
- A transaction that **sets** a VK permission with any `txn_version` other than current is
  refused as `Incompatible_version` before it reaches a block
  ([MINA `mina_base/zkapp_command.ml#L1439-L1451`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/mina_base/zkapp_command.ml#L1439-L1451)).
  So nobody can pre-lock an account against a future version. CONFIRMED-CODE.

### 1.2 What stays impossible

- `setPermissions: impossible` stays impossible across every fork. The permissions step checks
  `Account.Permissions.set_permissions a` with **no** fallback
  ([`zkapp_command_logic.ml#L1798-L1814`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/zkapp_command_logic.ml#L1798-L1814)).
  The same holds for `editState`, `send`, `editActionState`, `setTiming`, `setDelegate`,
  `setZkappUri`, `setTokenSymbol`, `incrementNonce` and `setVotingFor`; each is checked by plain
  `Controller.check`. CONFIRMED-CODE.
- `access: impossible` stays impossible too: the unchecked fallback maps only `Proof`. GUIDE agrees:
  "`access: impossible`: No fallback exists. The account stays permanently locked by design."
  CONFIRMED-CODE, DOCS.
- **RFC 0052 promised more than the code does.** Its test plan lists, as a success case, updating
  "the `verification_key` **permission** using a `Signature` authorization … when the account's
  … `txn_version` is less than the current hard fork version (even if `set_permission`
  disagrees)". 4.0.3 does not do this, and the unit test for the fallback
  ([`transaction_snark/test/verification_key_permission/verification_key_permission.ml#L24-L100`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_snark/test/verification_key_permission/verification_key_permission.ml#L24-L100))
  only covers accounts whose `set_permissions` is `Signature`. CONFIRMED-CODE.
  **Inference:** with `setPermissions: impossible`, the post-fork signature can change the VK and
  nothing else in the permission record. That half of Q1 holds.

### 1.3 Every account update closes the window, not only the VK update

- `set_txn_version_to_current` runs in the VK step **unconditionally**. It runs whether or not the
  update sets a VK, so every account update applied to the account sets its `txn_version` to
  current
  ([`zkapp_command_logic.ml#L1644-L1646`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/zkapp_command_logic.ml#L1644-L1646);
  ledger implementation
  [`mina_transaction_logic.ml#L1272-L1285`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/mina_transaction_logic.ml#L1272-L1285);
  snark implementation
  [`transaction_snark.ml#L1064-L1080`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_snark/transaction_snark.ml#L1064-L1080)).
  CONFIRMED-CODE.
- RFC 0052 discussed requiring that "outdated permissions must be reset by the first account update"
  and decided against it. GUIDE describes only the VK update closing the window ("Setting the new VK
  bumps the account's `txn_version`"). Neither says that any other update closes it too.
  CONFIRMED-DOCS.
- **Inference (from the code, not run on a chain):** after a fork, the first **successful** account
  update to the zkApp account, from anyone, closes the fallback. If its `access` is `none` (the o1js
  default), any third party can send a no-op, unauthorised account update on the zkApp
  (`None_given`, no balance change). That update passes every check and sets `txn_version` to
  current. `setVerificationKey` is then `Impossible` at the current version, and the stale VK can
  never be replaced. The escrow is frozen until the **next** fork, or for good if a stale-VK account
  stays locked at each later version too.
- **`access: proof` closes this.** `access` is checked first, under the same fallback. Before the
  fork, every account update must carry a proof. After it, `access` reads as `signature`, so only
  the zkApp key can touch the account until it installs the new VK. Then `txn_version` becomes
  current and `access` is `proof` again, against the new VK. GUIDE: "`access: proof`: Falls back to
  `signature` during the upgrade window, then is restored." Signed **payments** (user commands,
  not zkApp commands) do not consult `access` at all: `access` appears in the ledger code only on
  the zkApp path (`mina_transaction_logic.ml`, lines 992–994 and 1104). So a plain MINA transfer
  into the account still works. CONFIRMED-CODE, DOCS for the parts; the conclusion is Inference.
- MFT's `FungibleToken.deploy` sets exactly `access: Permissions.proof()` beside
  `setPermissions: impossible`
  ([MFT `FungibleToken.ts#L76-L83`](https://github.com/MinaFoundation/mina-fungible-token/blob/8dbf75a4a50d236d9a9ab137a7fd451bafddae41/FungibleToken.ts#L76-L83)).
  CONFIRMED-CODE.

### 1.4 What the signature can do, outside a fork and inside one

- **Outside a fork**, a signature by the zkApp key can do whatever any permission at `signature`
  allows. `Permissions.default()` leaves at `signature`: `setDelegate`, `setPermissions`,
  `setZkappUri`, `setTokenSymbol`, `incrementNonce`, `setVotingFor` and `setTiming`
  ([O1JS `account-update.ts#L393-L407`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/account-update.ts#L393-L407)).
  Q1 overrides `setPermissions` but names none of the others. CONFIRMED-CODE.
- **`setTiming` is the one that touches money.** On an **untimed** account, a holder of
  `set_timing` may set any vesting schedule
  ([`zkapp_command_logic.ml#L1396-L1416`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/zkapp_command_logic.ml#L1396-L1416)).
  The minimum balance it imposes is enforced on every later debit (`Source_minimum_balance_violation`,
  `#L1530-L1546`). **Inference:** the key holder could, at any time, lock the whole escrow behind a
  far cliff, and every `send` (payout, refund, withdrawal) would then fail. It cannot redirect the
  money. `setDelegate` only moves the escrow's stake delegation. `incrementNonce` matters only if
  the circuit reads the account nonce. `setZkappUri`, `setTokenSymbol` and `setVotingFor` are
  cosmetic for a native-MINA channel.
  o1js has `Permissions.allImpossible()` (`#L441-L455`), with the VK at
  `impossibleDuringCurrentVersion`. CONFIRMED-CODE.
- **Inside the window**, the signature can:
  - pass `access`;
  - set the VK.

  It **cannot** edit state, `send` or dispatch actions: those stay `proof`, and the old VK cannot
  verify a new proof. **It can install any VK**, so it can install a circuit that sends the escrow
  anywhere. That is the whole trust in Q1's key. CONFIRMED-CODE; the last sentence is Inference.

- **One install per fork.** The VK update itself bumps `txn_version`, so a second VK change in the
  same era is refused. A wrong or buggy post-fork VK cannot be corrected until the following fork.
  Under Q1 that means a new deployment and a drain. **Inference** from §1.3.
- A signed update on a non-fee-payer account must increment its nonce or use the full commitment
  (CHANNEL §3.1). With `incrementNonce: impossible`, the VK update must use the full commitment.
  MMS `mina-tx` signs only full-commitment updates anyway (CHANNEL §4.1). CONFIRMED-CODE.

## 2. The freeze

- **Mesa migrated accounts by copying them.** `Account.Hardfork.migrate_to_mesa` copies
  `permissions` and `zkapp` (VK, app state, action state, `proved_state`) field for field
  ([MINA `mina_base/account.ml#L1103-L1131`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/mina_base/account.ml#L1103-L1131)).
  `zkapp_account.ml`'s `Hardfork` module widens app state to 32 fields (`#L492-L527`). The only
  semantic rewrite is to vesting `timing`, rescaled for the shorter slot
  (`Account.slot_reduction_update`,
  [`account.ml#L919-L924`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/mina_base/account.ml#L919-L924);
  `account_timing.ml#L335-L522`). CONFIRMED-CODE.
- **Old VKs are stale.** MESA-DOCS (zkApp developer example): state "carr[ies] over to the Mesa
  chain automatically", but "the verification key generated for Berkeley is no longer valid under
  the Mesa protocol", so "the contract cannot process transactions on Mesa until its verification
  key is updated". The post-upgrade page: "the protocol version bump invalidates Berkeley
  verification keys". GUIDE: "Proofs made against the old key stop verifying… No upgrade: The
  contract cannot process proof-authorized transactions, and on-chain state is frozen. Funds remain
  safe." o1js 3.0.0's changelog: "All verification keys will change". SPIKE observed the same
  directly: the Mesa verifier refuses pre-Mesa proofs. CONFIRMED-DOCS (SPIKE RAN).
- **The official procedure** is a signed account update from the zkApp key, after genesis, with no
  deadline:

  ```ts
  const update = AccountUpdate.createSigned(zkAppAddress);
  update.account.verificationKey.set(verificationKey);
  await tx.sign([senderKey, zkAppKey]).send();
  ```

  It is followed by one proof-authorized call to check the result (GUIDE; MESA-DOCS). GUIDE warns:
  "Lost zkApp key: You cannot sign the upgrade". CONFIRMED-DOCS.

- **"Funds remain safe"** is true only in that nobody else can take them. With a lost key, or a
  window closed by someone else (§1.3), they are stranded. **Inference.**
- **Which mainnet zkApps re-keyed, and when, was not found.** A press report (CryptoSlate,
  2026-09-04; secondary, not relied on) says "deployed zkApps now need new verification keys before
  they can resume proof-authorized activity". It names no app. UNCONFIRMED (see below).

## 3. The stop slot

- **The mechanism.** `slot_tx_end`: "the slot at which the daemon should stop accepting
  transactions, or producing or validating blocks with transactions or coinbase in them. The last
  block produced before this slot will become the hard fork block." `slot_chain_end`: "stop
  producing blocks entirely." `hard_fork_genesis_slot_delta`: "the difference between the
  [slot_chain_end] and the genesis slot of the hard fork chain"
  ([MINA `runtime_config/runtime_config.ml#L471-L487`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/runtime_config/runtime_config.ml#L471-L487)).
  CONFIRMED-CODE.
- **Mesa's values** were set in mainnet's genesis config by commit `20735c4c` (2026-08-13,
  "mainnet: set the hard fork stop-slot schedule for 2026-09-03"). They are `slot_tx_end: 393800`,
  `slot_chain_end: 393900` and `hard_fork_genesis_slot_delta: 60`, counted since the Berkeley fork
  (2024-06-05T00:00Z, 180 s slots). That gives 10:00Z, 15:00Z and genesis at 18:00Z on 2026-09-03.
  The release notes (published 2026-08-21) give the same wall-clock times:
  "Blocks produced by upgraded stakeholders will be empty, and no transactions will be accepted."
  CONFIRMED-CODE, DOCS.
- **Notice:** 21 days from the public commit, 13 days from the release. Earlier o1Labs posts said
  the date would follow the Mesa Trail testnet. No primary announcement earlier than 2026-08-13 was
  found. CONFIRMED-DOCS for the two dates; earlier notice UNCONFIRMED.
- **The halt** was eight hours: no transactions from 10:00Z, no blocks from 15:00Z, Mesa genesis at
  18:00Z (`genesis_state_timestamp: 2026-09-03T18:00:00Z` in
  [MINA `genesis_ledgers/mainnet.json`](https://github.com/MinaProtocol/mina/blob/4.0.3/genesis_ledgers/mainnet.json)).
  For a zkApp, proof-authorized activity stayed halted after 18:00Z until its owner re-keyed it
  (§2). CONFIRMED-CODE, DOCS; the last sentence is Inference.
- **The since-genesis slot is continuous.** Mesa's genesis config records the fork block at
  `global_slot_since_genesis: 958440`, which is Berkeley's own fork slot `564480`
  (`3.5.0-mainnet-stop-slot`'s `mainnet.json`) + `393900` + `60`. The GraphQL schema describes
  `slotSinceGenesis` as "Slot since genesis (across all hard-forks)". `validWhile`
  (`currentSlot.requireBetween`) is typed `Global_slot_since_genesis`
  ([MINA `mina_base/zkapp_precondition.ml#L1342-L1353`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/mina_base/zkapp_precondition.ml#L1342-L1353)).
  Slots since the hard fork restart at zero. CONFIRMED-CODE.
- **The slot length changed.** It was 180 s before Mesa and 90 s after (MIP6; CHANNEL §3.4). The
  protocol rescaled vesting timing for this (above), but **nothing rescales a slot count held in
  zkApp state or baked into a circuit**. **Inference:** a `minDelay` counted in slots halves in
  wall-clock time across a slot-reducing fork. A `requestedAt` slot recorded before the fork stays
  comparable after it, because the counter is continuous, but the delay left runs twice as fast.
- **The connector cannot read the stop slots from a node.** The 4.0.3 GraphQL schema has no
  `slot_tx_end` or `slot_chain_end` field. `genesisConstants` exposes only `accountCreationFee`,
  `coinbase` and `genesisTimestamp`. They live in the daemon's runtime config and the release.
  CONFIRMED-CODE (schema at
  [MINA `graphql_schema.json`](https://github.com/MinaProtocol/mina/blob/4.0.3/graphql_schema.json)).
- **No next fork is scheduled.** 4.0.3's `mainnet.json` sets no `slot_tx_end`. CONFIRMED-CODE.

## 4. Compiling the post-fork VK before the fork

- **o1js release dates (npm):**
  - `3.0.0-mesa.0`: 2026-03-30.
  - `3.0.0-mesa.698ca`: 2026-04-27, the build MESA-DOCS named for the preflight network.
  - `3.0.0-mesa.final`: 2026-06-02.
  - `3.0.0-mesa.rc2`: 2026-07-22.
  - **`3.0.0`: 2026-08-18**, 16 days before `slot_tx_end`.
  - **`3.1.0`: 2026-09-23**, 20 days after the fork.

  MESA-DOCS told developers to test on Mesa Trail "weeks before". CONFIRMED-DOCS.

- **Determinism.** A VK depends only on the circuit and the o1js/proof-systems version. o1js CI
  runs `tests/vk-regression/vk-regression.ts` against committed hashes in `vk-regression.json`, on
  both the wasm and the native backend (O1JS `run-ci-tests.sh` lines 39 and 44). SPIKE computed the
  same VK hash in Rust as o1js (`vk_hash_matches_o1js=true`) for both eras. zk_toon #16 plans
  reproducible VK publication (CHANNEL §4.4). CONFIRMED-CODE. **Inference:** for Mesa, a zkApp's
  post-fork VK hash could have been compiled and published at least 16 days before the stop slot,
  and earlier from the release candidates if they matched 3.0.0. Whether `mesa.rc2` and `3.0.0`
  produce identical VKs was not checked.
- **The VK did not hold still.** o1js 3.1.0, Security: two account-update vulnerabilities fixed;
  "**Verification keys change; contracts must be recompiled and redeployed to pick up the fix.**"
  Unreleased `main` changes VKs again for nested-call `callee.self`
  ([O1JS `CHANGELOG.md`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/CHANGELOG.md)).
  CONFIRMED-CODE. **Inference:** a VK pinned before Mesa from 3.0.0 would have been a
  known-vulnerable build three weeks later, if the circuit used the affected patterns. Under Q1 the
  only remedy is a fresh deployment and a drain, since the fork's one install would already be
  spent.

## 5. The key's form

- **The protocol accepts one Schnorr signature per account update**, checked against that update's
  `public_key` (CHANNEL §3.1). There is no native multisig or threshold account. A governing zkApp
  (MFT's `canChangeVerificationKey` pattern) works only through `proof`, and after a VK-changing
  fork its own VK is stale too. So the post-fork fallback can be exercised only by a **single
  Schnorr key**. CONFIRMED-CODE for the parts; the conclusion is Inference.
- **A FROST group key is a single Schnorr key.** MMS's `frost-bluepallas` implements FROST on Pallas
  with Poseidon and normalises the commitment's y-parity. Mina's Schnorr requires that
  (`frost-bluepallas/src/lib.rs#L238`, `#L267`). The README says it "allows generation of signatures
  that are compatible with Mina nodes". It names this very use: "During a fork zero knowledge
  verification keys may become obselete which causes all smart contracts to fallback to using
  signature-based updates. Multi-sig coordination provides an additional layer of safety when
  updating verification keys after a hard fork." Its `O1JS-WORKFLOW.md` ships an
  `update_verification_key.ts` generator and uses `impossibleDuringCurrentVersion`.
  CONFIRMED-CODE, DOCS.
- **Maturity:**
  - `frost-bluepallas` and `mina-tx` are `1.0.0-alpha.0` (GitHub release `v1.0.0-alpha.0`,
    2026-02-23, prerelease) and depend on `frost-core = "3.0.0-rc.0"`.
  - The trusted-dealer setup is "test-only". DKG needs the Zcash Foundation's `frostd` server.
  - The last push was 2026-08-03. The `o1-labs` repository has 6 stars; Raspberry-Devs' fork is the
    development home.

  CONFIRMED-CODE, DOCS.

- **Audit:** Veridise, 2026-06-15 to 06-22, on commit `1457cdd`
  (`audits/VAR_Nori_260615_multisig_V2.pdf`, V2 dated 2026-07-09). Scope: `frost-bluepallas`, plus
  `mina-tx`'s `bluepallas_compat` and `pallas_message`. "The rest of the repository (including the
  remainder of `mina-tx` and all of `mina-frost-client`) was not covered". Findings: 6 (1 high, 2
  medium, 3 low), all reported fixed. CONFIRMED-DOCS.
- **Mina Foundation's own tooling uses a single keypair.** MFT: "The default admin contract uses a
  single keypair. That is not ideal, as it introduces a single point of failure… it is recommended"
  to use multi-sig or governance. "We are planning to use a more refined access control (using
  multi-sig) in an upcoming version" (`documentation/deploy.md` lines 39–50 and 100–106).
  CONFIRMED-DOCS.
- No mainnet zkApp was found whose VK authority is a FROST key. UNCONFIRMED.

## 6. The Rust side

- **Reading.** GraphQL `account(publicKey:)` returns `verificationKey { verificationKey hash }`
  (type `AccountVerificationKeyWithHash`) and
  `permissions { setVerificationKey { auth txnVersion } access … }` (types `AccountPermissions`,
  `VerificationKeyPermission`) (MINA `graphql_schema.json`). The hash is computed by the node.
  CONFIRMED-CODE.
- **The current era is not readable directly.** No GraphQL field returns the node's
  `protocol_version_transaction`. `daemonStatus { chainId }`, `networkID` and `genesisBlock` exist.
  **Inference:** a new fork has a new genesis, so `chainId` should change. That was not checked.
  CONFIRMED-CODE for the schema.
- **Matching the published hash.** SPIKE computed the VK hash in Rust from o1js's artefact and got
  o1js's value in both eras (`vk_hash_matches_o1js=true`). CONFIRMED (SPIKE, RAN).
- **Signing a signature-authorized VK update in Rust.** MMS `mina-tx`'s `Update` carries
  `verification_key: Option<VerificationKeyData { data, hash }>` and `permissions`. Its packing
  commits to them (`zkapp_tx.rs#L208-L219`, `#L311-L314`; `zkapp_tx/packing.rs#L330-L340`). It signs
  full-commitment updates and submits `sendZkapp` (CHANNEL §4.1). The VK itself must come from an o1js
  `compile()`; Rust cannot produce it (CHANNEL §4.2). **Inference:** the key holder can sign the
  install in Rust (or through a FROST round) from an o1js-made VK artefact and unsigned JSON. It was
  not run. CONFIRMED-CODE for the parts.
- **`mina-tx` is built for one era.** `TXN_VERSION_CURRENT` is 3 without and 4 with the
  `mesa-hardfork` feature (`zkapp_tx/constants.rs#L16-L20`), as is the app-state length. SPIKE found
  the verifier pinned to one era the same way. CONFIRMED-CODE.

## 7. Precedent

- **EVM x402**: ownerless and immutable. **Solana payment-channels**: upgradeable by
  solana-foundation's key, whose form was not determined (ADR 0074 §5). CONFIRMED-DOCS
  (this repository).
- **MFT** is the nearest Mina precedent and matches Q1's shape:
  - `FungibleTokenAdmin`: `setVerificationKey: impossibleDuringCurrentVersion`,
    `setPermissions: impossible`. Its `updateVerificationKey` says it "will only be possible in case
    of a protocol update that requires an update" (`FungibleTokenAdmin.ts#L38-L58`).
  - `FungibleToken` adds `access: proof`. Its `allowUpdates` option makes the VK
    `proofDuringCurrentVersion`, gated by the admin contract's `canChangeVerificationKey`, so it can
    take o1js fixes between forks.
  - The docs call upgradeability "a double edged sword" and say v1.0.0 was fork-only
    (`documentation/deploy.md` lines 83–106).

  All three leave the rest of `Permissions.default()` (including `setTiming: signature`) in place.
  CONFIRMED-CODE, DOCS.

- No Mina zkApp was found that publishes a timelock or notice period on its upgrade key. Mina
  permissions offer no timelock (CHANNEL §3.1). A notice period would have to be a promise off chain,
  or a circuit rule in the **new** VK. UNCONFIRMED as a negative.

---

## Unconfirmed, and why

- **That a no-op update really closes the window on a live chain (§1.3).** This was read in the
  transaction logic and the snark, both unconditional, but not run. A lightnet test would settle it:
  fork with an account at `access: none`, then send a `None_given` update. RFC 0052's own test plan
  has no such case.
- **Which mainnet zkApps re-keyed after Mesa, and how fast.** No list was found in o1Labs or Mina
  Foundation material. The only report found is secondary and names none. Finding examples would
  mean scanning mainnet accounts for `txnVersion` 3 against 4 through GraphQL or an archive, which
  was not done.
- **Whether `3.0.0-mesa.rc2` and `3.0.0` give identical VKs.** This bears on how early a post-fork
  VK hash could have been published. Not compiled.
- **Whether `daemonStatus.chainId` changes at each fork.** Inferred from its being derived from
  genesis. Not read in the code.
- **Whether mempool transactions survive a fork.** Not checked. **Inference** from the
  "empty blocks" rule: anything not landed before `slot_tx_end` must be resubmitted after genesis,
  under the new VK.
- **Notice earlier than 2026-08-13.** Third-party posts gave 2026-09-03 earlier. No primary source
  before the config commit was found.
- **Any FROST-signed transaction on mainnet.** MMS documents devnet submission. No mainnet example
  was found.
- **Whether the mainnet o1Labs policy for future forks will again ship the post-fork o1js before
  the stop slot.** Mesa's pattern is the only evidence. No stated policy was found.

---

## What this means for the accepted decisions

**Q1: the zkApp key is kept; VK `impossibleDuringCurrentVersion`, `setPermissions: impossible`,
`proof` for `editState`, `send` and `editActionState`.**

- **Holds:**
  - the fallback is consensus code;
  - `setPermissions: impossible` survives every fork;
  - after a fork the signature can set the VK only;
  - between forks the VK cannot change, so a bug fix means a new deployment and a drain. o1js
    3.1.0 is a live example of that need, 20 days after Mesa.
- **Breaks as stated, in two places:**
  - **(a) "The key is dormant" is false** while `setTiming`, `setDelegate`, `setZkappUri`,
    `setTokenSymbol`, `incrementNonce` and `setVotingFor` stay at o1js's default `signature`.
    `setTiming` lets the key lock the escrow at any time (§1.4). Dormancy needs each of these at
    `impossible` (or `proof`).
  - **(b) The window can be closed by anyone** unless `access` is `proof`. The window is also
    **one install per fork**: a wrong post-fork VK is permanent until the next fork (§1.3–§1.4).

**Q2: the connector pins (address, network) → a set of accepted VK hashes, checks at boot and
before each Mina write, and fails closed for the Mina backend only.**

- **Holds:** the hash is readable over GraphQL and matchable in Rust, and a pre-fork release could
  have carried Mesa's VK (§4, §6).
- **Gap (a):** "On-chain VK ∈ set" is **not** the same as "usable". Between genesis and the install,
  the on-chain VK is the pre-fork one, still in the set, and no proof against it verifies.
  **Inference:** the check needs the VK paired with its era. Two candidates are the account's
  `permissions.setVerificationKey.txnVersion`, which with `access: proof` changes only at the
  install, and the chain's identity (`chainId`, unverified).
- **Gap (b):** the connector's own Mina dependencies are built for one era: SPIKE's verifier, and
  `mina-tx` should the payer's debit be signed in Rust. So a pre-fork release that accepts both VKs
  still cannot sign or verify across the fork unless it carries both builds.

**Q3: land every unlanded voucher before the announced stop slot; the post-fork circuit bakes a
`resumeSlot` and `finalizeWithdraw` needs `currentSlot ≥ resumeSlot + minDelay`.**

- **Holds:**
  - notice was 13–21 days;
  - the since-genesis slot is continuous;
  - the Mesa genesis slot (958440) was computable from the announced config, so `resumeSlot` could
    have been baked in before the fork.
- **Flags:**
  - **(a)** The receiver is protected only from when the **install** lands, not from genesis.
    If `resumeSlot` is the genesis slot, the delay runs while nothing can land. **Inference:** the
    margin in `minDelay` has to cover the key holder's install latency.
  - **(b)** A slot-length change rescales any slot-counted delay in wall-clock time; Mesa halved it.
  - **(c)** The stop slots are not on GraphQL, so the connector learns them from a release, a
    config value or its own constant.
  - **(d)** Landing before the stop slot competes for the 12-zkApp-commands-per-block cap
    (CHANNEL §3.4).

**Q4: the token channel is deferred; the trust is "two upgrade keys plus an admin's pause".**

- **Holds in substance.** The count is worth rechecking: at a fork, MFT has **three** account
  keys that can install a VK:
  - the `FungibleToken` account's key;
  - the `FungibleTokenAdmin` account's key;
  - the channel zkApp's own key.

  With `allowUpdates`, the admin contract can also change the token's VK **between** forks, by
  proof. The admin defaults to a single keypair (§5, §7).

### Facts the next grilling round needs

- **Who holds the key, and in what form.** The protocol accepts exactly one Schnorr key per
  account. The candidates are a single key (MFT's default), a FROST group key (MMS: alpha, partly
  audited, built for this use) or a key per channel account. A governing zkApp cannot act at a fork.
  Losing the key strands the escrow, and so does missing the window to a third party.
- **What binds the holder to a faithful port.** **Nothing on chain.** In the window the key can
  install any VK, including one that sends the escrow anywhere (§1.4). What exists off chain:
  - the connector's pinned VK set, which protects the **connector's** writes but not funds already
    in escrow;
  - reproducible VK publication (zk_toon #16), so anyone can check the installed hash against the
    published source;
  - a post-fork circuit whose own rules (`resumeSlot`, `minDelay`) honour open channels;
  - the holder's identity and reputation, as with solana-foundation's upgrade authority.
- **What Q1 requires of storage (#1553).** Each account whose escrow is behind `send: proof`
  carries its own VK, its own permission record and its own `txn_version`. **Inference:**
  - One zkApp account per channel (or per-channel token accounts under the zkApp) means one key and
    one post-fork install **per account**, each racing its own window, each needing `access: proof`
    and all-`impossible` permissions.
  - One shared account (actions plus a Merkle map) means one key and one install.
  - Mesa carried action state and app state over unchanged (§2). A shared design survives the fork
    as data and needs only the VK.
