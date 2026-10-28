# Whose code a channel may bind to: the facts under #1564's decisions

> **Research note, not a decision record.** It decides nothing. The owner decided issue
> [#1564](https://github.com/toon-protocol/connector/issues/1564) (part of map
> [#1563](https://github.com/toon-protocol/connector/issues/1563)) in a grilling session on
> 2026-10-28. The question was: may the connector build to an unmerged x402 binding, and to on-chain
> code that neither x402 nor TOON owns? This note checks the facts those decisions rest on, against
> primary sources: chain RPC, mirror nodes, the x402 repository and its PRs, and each project's own
> source. It starts from [`x402-pending-chain-bindings.md`](x402-pending-chain-bindings.md) (cited as
> **PENDING**), [`mina-x402-batch-settlement.md`](mina-x402-batch-settlement.md) (**MINA-NOTE**),
> [ADR 0074](../adr/0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md) and
> [ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md), and re-reads
> their claims rather than trusting them. Written 2026-10-28; every source was read that day.

**Evidence tags.** CONFIRMED means read in source at a pinned commit, or read live from a chain.
CONTRADICTED means a primary source says otherwise. UNCONFIRMED means neither, and says why.
**Inference** marks this note's reasoning, not a fact.

**Sources pinned.**

- **x402**: [`x402-foundation/x402@db2c98ec`](https://github.com/x402-foundation/x402/tree/db2c98ec9490e4e4286c862de845e667b037f219) (`main`, 2026-10-28T13:26Z).
- **rippled**: [`XRPLF/rippled@05847a3c`](https://github.com/XRPLF/rippled/tree/05847a3c908a9b3eaf43ddbd9af3c81214e60268) (`develop`, 2026-10-27).
- **Hedera protobufs**: [`hashgraph/hedera-protobufs@8c27786c`](https://github.com/hashgraph/hedera-protobufs/tree/8c27786cec93abab974309074feaef9b48a695b7) (`main`).
- **Subbit**: [`kompact-io/subbit-xyz@c45f4869`](https://github.com/kompact-io/subbit-xyz/tree/c45f486958b888560387e1094cbe73cc1e081af9) (`main`) and branch `x402` at `946d3e89`.
- **o1js**: [`o1-labs/o1js@a428f9b9`](https://github.com/o1-labs/o1js/tree/a428f9b97817177ef4b5868cf148c0ac67a1048d) (`main`, 2026-10-27).
- **Mina daemon**: [`MinaProtocol/mina@f92f3199`](https://github.com/MinaProtocol/mina/tree/f92f31995c252052f932b4b7f25d46c9b1331943) (`compatible`).
- **Chains**: Base mainnet `mainnet.base.org` (block 53 204 012) and Base Sepolia `sepolia.base.org`
  (block 51 377 538); Solana `api.mainnet-beta.solana.com` (slot 462 508 335) and
  `api.devnet.solana.com` (slot 516 083 441); XRPL `s1.ripple.com`, `s2.ripple.com`, `xrplcluster.com`
  (validated ledger 107 960 039) and `s.altnet.rippletest.net` (ledger 21 890 343); Hedera
  `testnet.mirrornode.hedera.com`; Cardano Koios `preprod.koios.rest` and `api.koios.rest`; Mina
  `api.minascan.io` GraphQL.

---

## Decisions (owner, 2026-10-28, #1564)

- **Q1.** ADR 0075 decision 1, amended, admits a binding only when its on-chain code is (1) x402's own
  deployed code, (2) the chain's native primitive, with nothing deployed, or (3) TOON-owned code, which
  TOON deployed or pinned, reviewed, and answers for. Third-party code run as-is is never admitted.
  Whose spec it is no longer matters. Waiting for an upstream merge is no longer the gate.
- **Q2.** A spec that is someone else's open PR (XRPL #2952) is built to that PR pinned at a commit
  SHA. Following upstream drift is a deliberate, reviewed change that bumps the wire vectors'
  `schema_version` (ADR 0021). No loose tracking. No fork unless upstream abandons it.
- **Q3.** "A constant of the binary, never config" becomes a per-(chain, network) table keyed by
  network id. An entry may be empty (XRPL). Hedera maps chain ids 295/296 to addresses. Cardano holds a
  script hash and a reference-script UTxO per network. Boot refuses, by name, a network with no entry
  or with its code absent. For a native primitive, the check is that the ledger feature is enabled
  (XRPL: the PayChan amendment).
- **Q4.** Adopting third-party code under category 3 needs: no outside key over the money (refined by
  Q8); TOON's review recorded in an ADR naming the exact code (commit and bytecode or script hash); an
  external audit before mainnet, not before devnet. Until audited, the Q3 table holds testnet entries
  only, so a mainnet node is refused at boot by name.
- **Q5.** #1564 sets the bar only. Hedera and Cardano stay parked (#1563). Each adoption is a later
  owner decision.
- **Q6.** That XRPL claims bind no network id stays in #1563's fog, under D10.
- **Q7.** One new ADR (roughly "Whose code a channel may bind to") amends ADR 0075 decision 1 and
  absorbs #1558's unwritten Mina amendment.
- **Q8.** One key rule for all three categories. A single key that can move, freeze or replace channel
  code fails (Hedera's author's ECDSA admin key; any single TOON key). An upgrade authority passes only
  as a multisig of an accountable party, named with its threshold in the admitting ADR. Immutable or
  ownerless code passes outright. TOON-owned code meets the same rule: Mina's channel zkApp is not
  deployed with a single TOON key able to change its verification key.
- **Q9.** Each chain's pinned spec commit lives in that chain's own spec or ADR, not the general ADR.

Settled after the contradictions below were put to the owner (same day):

- **Q10 (resolves C1).** Q8's key rule, like Q4's audit, gates **mainnet** entries of the table only.
  A testnet entry still names its upgrade authority, but any authority is admitted, a single key
  included. Category 3 is unchanged: TOON still deploys its own Hedera instance.
- **Q11.** "A multisig of an accountable party" means an organisation publicly claims the authority.
  The ADR records that claim, the multisig address and its threshold. The individual members need
  not be identified.
- **Q12 (resolves C2).** For a native primitive, boot checks that the RPC answers with the network
  id the table entry names (XRPL: `server_info`'s `network_id`, or the genesis identity). It checks a
  feature only when that feature is still a live amendment. Q3's PayChan check is dropped.
- **Q13 (resolves C3).** Q8 admits a key that is dormant until a protocol-version change, as a Mina
  zkApp's key is. The ADR names that key and who holds it, and TOON commits to reinstalling the same
  circuit, recompiled, at each fork, with the diff published. The mechanics go to #1548.
- **Q14.** The two errors in `x402-pending-chain-bindings.md` (PayChan's date, and the attribution of
  "unaudited" to Subbit's README) are corrected in place.

---

## Facts each decision rests on

### F1. x402's EVM contracts are ownerless and immutable (Q1 category 1, Q8 "passes outright")

**CONFIRMED**, on chain and in source.

- **Source.** `contracts/evm/src/x402BatchSettlement.sol` (587 lines) at `db2c98ec`. It inherits only
  OpenZeppelin `EIP712`, `Multicall` and `ReentrancyGuardTransient`. Its constructor takes no argument
  (`EIP712("x402 Batch Settlement", "1")`). Its state is four mappings (`channels`, `refundNonce`,
  `pendingWithdrawals`, `receivers`) plus constants. Its external functions are `deposit`, `claim`,
  `claimWithSignature`, `settle`, `initiateWithdraw`, `finalizeWithdraw`, `refund`,
  `refundWithSignature` and four view helpers. There is no owner, no pause, no proxy and no admin
  function. The collectors (`ERC3009DepositCollector`, `Permit2DepositCollector`, base
  `DepositCollector`) hold only immutables (`x402BatchSettlement`, `PERMIT2`). The contract file has
  one commit, `84ffb641` (2026-05-15, #1950).
- **Deployment.** `contracts/evm/script/DeployBatchSettlement.s.sol` deploys through Arachnid's
  CREATE2 deployer `0x4e59b448…956C`, so the address is the same on every chain.
- **On chain, Base mainnet (8453) and Base Sepolia (84532).** For all three addresses
  (`0x4020074e…0003`, `0x40208060…0004`, `0x4020425F…0005`):
  - code is present (11 175, 1 150 and 2 467 bytes);
  - the EIP-1967 implementation, admin and beacon slots are all zero;
  - `owner()` and `paused()` revert.
- **The bytecode.** The two collectors are byte-identical on both chains (keccak `0x0c2cfe88…1f3b`
  and `0x11377863…4a5a`). The settlement contract differs between the chains only at bytes
  8004–8006 and 8017–8048: the chain id (`0x2105` against `0x14a34`) and the cached EIP-712 domain
  separator, both immutables. The runtime has no `SELFDESTRUCT` and no `CALLCODE`. It has one
  `DELEGATECALL`, which is OpenZeppelin `Multicall`'s call to itself.
- **Verified source.** Sourcify has a full match for the settlement contract on 8453 (runtime match,
  verified 2026-06-11) and for all three on 84532 (runtime and creation match). The two collectors are
  **not** Sourcify-verified on 8453. Their mainnet bytecode equals the verified Sepolia bytecode, so
  they are the same build. [Sourcify](https://sourcify.dev/server/v2/contract/8453/0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003).
- ADR 0074's "Sources" row ("`owner()` reverts, and the EIP-1967 slot is zero") still holds.

### F2. Solana `payment-channels`: an upgrade authority, a Squads 3-of-5 on mainnet, a single key on devnet (Q8)

**CONFIRMED** for mainnet-beta. **CONFIRMED** for devnet, and it bears on Q8 (see Contradictions, C1).

- **Program data** `CghQXkmw2F6p1exMETiZdNeUx9QGraWsNZ4eom1Cuiw1`, owned by
  `BPFLoaderUpgradeab1e11111111111111111111111` on both clusters (JSON-RPC `getAccountInfo`, `jsonParsed`).
- **Mainnet-beta**, read at slot 462 508 335:
  - upgrade authority `DXtFpbPjcn2hxPnw79x1Pfoj35vXh5AsWBkS37YnXMVv`; last deploy slot 431 447 053
    (2026-07-07). The newest transaction touching the program data is at slot 435 599 419
    (2026-07-27), the authority hand-over ADR 0075 records. Nothing has touched it since.
  - The authority is **off** the ed25519 curve, so it has no private key.
  - `solana find-program-derived-address SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf string:multisig
pubkey:4CxQs26DewQ1KaCHfyyjktkYjndNdUqCJvVdygtJFwcJ string:vault u8:0` returns `DXtFp…XMVv`. So the
    authority is vault 0 of that Squads v4 multisig.
  - The multisig account (396 bytes, owned by `SQDS4ep6…pCf`), decoded by the Squads v4 layout:
    threshold **3**, time lock **0**, config authority the default key (so only the multisig can change
    itself), no rent collector, transaction index 11, and **5** members, each with permission mask 7.
    The members are `9DeYwyFk…BGzN`, `9vL7jtVc…Vc4Q`, `EWcevF86…wu5m`, `FBR1BPM6…LEQP`, `HL87yvhf…ritk`.
  - So ADR 0075's "Squads v4 vault, 3 of 5, time lock 0" still holds. **Who** the five members are is
    not on chain (see Unconfirmed).
- **Devnet**, read at slot 516 083 441: upgrade authority `4zTeC5mVqWLruDexgU2mV66p9t5vCA9JyiZqdGDUspap`,
  last deploy slot 480 232 051. The authority is **on** the curve, a System-owned account with no data
  and 46.49 SOL. It is a single keypair, as ADR 0075 says.

### F3. XRPL #2952: the SHA a later spec would pin (Q2, Q9)

**CONFIRMED** (`gh pr view 2952`, the compare API).

- State **open**, not draft, `mergeable: MERGEABLE`, `reviewDecision: REVIEW_REQUIRED`. Labels
  `specs`, `xrpl`. Author `desimmons`, from the fork `Dhali-org/x402`, branch `xrpl-batch-settlement`.
- Head **`683fb7a6b08b5cc12568b36d6f560ba0d57dcf1e`**, one commit, authored 2026-07-25, committed
  2026-08-13T20:10Z. The PR was last updated 2026-08-13T20:10Z, so it has been idle for 11 weeks.
- One review: `whawk46` (association CONTRIBUTOR, not a maintainer), COMMENTED, 2026-08-05. The only
  comments are Vercel's bot and the author.
- The head is reachable from upstream as `refs/pull/2952/head`, so a pin survives the fork being
  deleted.
- It changes one file, `specs/schemes/batch-settlement/scheme_batch_settlement_xrpl.md`. Its merge
  base is `2d23a165` (2026-08-13), and `main` is now 315 commits ahead of it.
- The chain-neutral `scheme_batch_settlement.md` has not changed since `c763d05a` (2026-05-05), so
  #2952 is not stale against it. `main` has moved the EVM and SVM chain files since (for example
  #3725, EVM "managed voucher mode", v1.1, merged 2026-10-26).
- **The `upto` reshaping did not touch batch-settlement.** #3950 is an **issue**, not a PR. It asks to
  move XRPL `upto` to an escrow flow, and it states the channel-id rule
  `SHA512Half(0x0078, Account, Destination, Sequence)`. PR #4008 (CarsonRoscoe, open, head `a1f02ffb`,
  no reviews, updated 2026-10-27) changes only `specs/schemes/upto/scheme_upto_xrpl.md`.
  **Inference:** #2952 may be asked to align with #4008. That would be the drift Q2 handles.

### F4. XRPL's native primitive: the amendment, the claim message, the channel id (Q1 category 2, Q3, Q6)

- **PayChan on mainnet: CONFIRMED enabled, since 2017-03-31.** `feature PayChan` returns
  `enabled: true` (`08DE7D96…8647`). The Amendments ledger object lists it. Its `EnableAmendment`
  pseudo-transaction is `16135C0B…FD7E`, in ledger 28 685 313, Ripple time 544 256 840, which is
  **2017-03-31T06:27:20Z** (read from `s2.ripple.com` and `xrplcluster.com`). PENDING §2.7 says "since
  2020". That is wrong (see C3).
- **PayChan on testnet: the feature reads disabled, but channels work.** CONFIRMED. On
  `s.altnet.rippletest.net` (rippled 3.4.1, `network_id` 1), `feature PayChan` returns
  `enabled: false`, and the Amendments object (82 entries) does not list it. Yet testnet holds live
  `PayChannel` objects. For example, `0000660712CE…4355` (source `rUcj1ZSA…`, destination `rsEUWmJY…`,
  Amount 10 XRP, Balance 1 XRP) was created by `PaymentChannelCreate` `752B4609…C9D3` and claimed by
  `PaymentChannelClaim` `9390498C…3CE5`. The reason is in rippled: `features.macro` has
  `XRPL_RETIRE_FEATURE(PayChan)`, and `Feature.cpp` says "Retired features are in the ledger and have
  no code controlled by the feature." The code runs unconditionally. Only a network whose Amendments
  object predates retirement still lists it. This bears on Q3 (see C2).
- **`fixPayChanCancelAfter`** (`D3456A86…83E2`) is enabled on both mainnet and testnet.
- **The claim message carries no network id. CONFIRMED** in rippled.
  `include/xrpl/protocol/PayChan.h` `serializePayChanAuthorization` writes
  `add32(HashPrefix::PaymentChannelClaim)`, then `addBitString(channelId)`, then `add64(drops)`.
  `HashPrefix.h` gives `PaymentChannelClaim = makeHashPrefix('C','L','M')`, which is `CLM\0`. Nothing
  else is signed.
- **The channel id is derived from the source, the destination and the sequence. CONFIRMED** in code
  and on chain. `Indexes.cpp` `payChannel(src, dst, seq)` is
  `indexHash(LedgerNameSpace::XRPPaymentChannel, src, dst, seq)`. Recomputing
  `SHA512Half(0x0078 ‖ AccountID(src) ‖ AccountID(dst) ‖ u32 BE 17515774)` for the testnet channel
  above gives exactly `0000660712CE17BDCF1B5EE548912A940375FB038518CBD070FD2C092C6B4355`. The id
  binds no network either. **Inference:** the same key pair and sequence on two networks give the same
  id and the same valid claim. That is the replay Q6 leaves to #1563 D10.
- **Nothing to own. CONFIRMED** by construction. There is no contract, program id or deployer. The
  ledger's rules change only by validator-voted amendment.

### F5. Hedera #3466: an admin key over the escrow, and that Hedera can make a contract immutable (Q8)

- **The escrow's admin key: CONFIRMED** (mirror node, `/api/v1/contracts/…`).
  - Escrow `0.0.10463847` (`0x…009faa67`), memo "x402 x402BatchSettlementHedera", and collector
    `0.0.10463851` (`0x…009faa6b`), memo "x402 HederaAllowanceDepositCollector". Both were created
    2026-09-10, neither is deleted, and neither has an auto-renew account.
  - Both have admin key `ECDSA_SECP256K1 030ddffb574c3deed6bc50f159da0608128d043b465c7a4716db09fbea52cea400`.
  - That is the key of account `0.0.10463136` (EVM `0x456ddc2a…588d`), which created them.
- **Who holds it: CONFIRMED that it is the PR's deploying operator.** PR #3466 (head `8358eb89`,
  open, 0 reviews, last updated 2026-09-13) says so twice. Its deploy script documents
  `HEDERA_OPERATOR_ID … (pays fees, becomes admin key holder)` and calls `.setAdminKey(…)` with
  `adminKey: operatorKey`. Its SDK config says "Deployed 2026-09-10 … (operator 0.0.10463136)". That
  this operator is the PR's author, `oemerfurkan`, personally is an **inference** from the PR alone.
- **What that key can do: CONFIRMED** in HAPI (`services/contract_delete.proto`). `ContractDelete`
  needs the admin key. On success, the named `transferAccountID` or `transferContractID` "SHALL
  receive all HBAR and other tokens still owned by the contract". So the key can delete the escrow and
  sweep everything in it to any account. This answers PENDING's open question ("whether deleting
  Hedera's testnet escrow could move escrowed balances"): by the spec, yes. It was not tested.
- **Hedera can deploy immutable code: CONFIRMED** (`contract_create.proto`, `contract_update.proto`).
  - "A contract without an admin key SHALL be immutable, except for expiration and renewal."
  - An update to any field but `expirationTime` on such a contract fails `MODIFYING_IMMUTABLE_CONTRACT`.
  - An admin key can be removed later by setting it to an empty `KeyList`.
  - `ContractDelete` treats an empty `KeyList` as no admin key.
- So Q8's Hedera example holds. A TOON-deployed copy with no admin key would pass Q8 as immutable.
  The "except for expiration and renewal" clause is the one gap (see Unconfirmed).

### F6. Subbit (Cardano): permissionless, its script hash, its audit status, the reference script (Q3, Q4, Q8)

- **No admin or upgrade key: CONFIRMED** in source.
  - `aiken/validators/subbit.ak` is a parameter-free validator: `spend(datum, redeemer, own_oref, tx)`
    delegates to `subbit/spend.do`, and `else` fails.
  - `plutus.json` lists `subbit.subbit.spend` with no parameters.
  - The datum (`lib/subbit/types.ak`) is `(own_hash, (tag, currency, iou_key, consumer, provider,
close_period), stage)`. The only keys are the two parties'.
  - The redeemers are `Defer`, `Main(Steps)` and `Mutual`. A step is a `Cont` (`Add`, `Sub`,
    `Close`, `Settle`; `Sub` and `Settle` carry an amount and a signature) or an `Eol`.
  - A Cardano script's address is the hash of its code, so the code cannot change under an address.
    **Inference** from Cardano's design, not read in a ledger spec here.
- **Script hash: CONFIRMED.** `6d8774631fece31d0c768afb960c9a8a9957f8a85fa31deae1159094`, the same in
  `plutus.json` on `main` (`c45f4869`) and on `x402` (`946d3e89`). Plutus V3, Aiken v1.1.24+bacbeb3.
  It is the hash Cardano PR #3580 (head `75ad5e81`, open, 0 reviews) names.
- **Audit: CONFIRMED that no audit is published.** The README says "Status: alpha." No audit report
  is in the repository; "audit" occurs only in two design remarks. #3580 itself says "The validator is
  Subbit's, alpha and unaudited." PENDING says the README calls it unaudited. The README says only
  "alpha"; the "unaudited" is #3580's.
- **The reference-script model: CONFIRMED on preprod.** UTxO `ff61e3f4…458c#0` (the one #3580 names)
  is unspent. It carries a Plutus V3 reference script with hash `6d877463…`, 3 058 bytes, and holds
  14.21 ADA. It was created 2026-10-07. Koios preprod `script_info` gives creation tx `23994f00…a588`.
  Koios **mainnet** `script_info` knows no such hash. Its address (`addr_test1qzyvhu…`) has a key
  payment credential.
  **Inference:** that key's holder can spend the UTxO and remove the reference script. That is a
  liveness fact, not a money fact: the script can be republished or attached inline. A Q3 entry
  would be safer pointing at a reference UTxO TOON publishes at an always-fail script address.

### F7. Mina: no setting keeps every key away from the verification key across a hard fork (Q8, Q7)

- **The permission exists, but only for the current version: CONFIRMED** in o1js.
  `src/lib/mina/v1/account-update.ts` defines `Permissions.VerificationKey.impossibleDuringCurrentVersion()`
  and `proofDuringCurrentVersion()`. Its comment says: "Once the current transaction version is
  higher than the one on the permission, the permission is treated as `signature`, and the zkApp can be
  redeployed with a signature of the original account owner." Plain `impossible` and `proof` are not
  offered for `setVerificationKey`.
- **The daemon enforces it, `impossible` included: CONFIRMED** in Mina's code.
  - `src/lib/mina_base/permissions.ml`: `verification_key_perm_fallback_to_signature_with_older_version`
    maps `Impossible | Proof -> Signature`.
  - `src/lib/transaction_logic/zkapp_command_logic.ml` applies it when
    `Account.txn_version a |> Txn_version.older_than_current`.
  - After every VK update, `set_txn_version_to_current` runs.
  - So after any hard fork that raises the transaction version, the zkApp **account's own key** can
    set a new verification key. A new key can then do anything the circuit allows, including moving the
    escrowed funds.
- **That has just happened on mainnet: CONFIRMED** in part.
  - Mina released "Mainnet Stable 4.0.0 Mesa" on 2026-09-03.
  - Minascan's mainnet node (`networkID mina:mainnet`) reports daemon commit `73134bfd` (2026-10-22),
    whose `node_config_version.ml` has `protocol_version_transaction = 4`.
  - Minascan's devnet node reports `b8cdab00` (2025-02), which has `3`. o1js `main` has
    `protocolVersions = { txnVersion: 4 }`.
  - **Inference:** Mesa raised the transaction version from 3 to 4. Any zkApp deployed before it with
    `impossibleDuringCurrentVersion` is now changeable by its account key.
- **What Q8 needs from this.** A Mina account's key is one Schnorr key, and Mina has no on-chain
  multisig account type. So the fallback key cannot be "a multisig named with its threshold" visibly
  on chain. The choices are:
  - destroy the key after deploying, which no chain read can prove; channels are then stranded at the
    next version-raising fork unless drained first;
  - generate the key by an off-chain threshold scheme, which is also unverifiable on chain.

  MINA-NOTE §D7(b) offers "`impossible`, so channels are stranded", but the daemon turns `impossible`
  into `signature` too. Stranding is reached only by destroying the key. See C4.

---

## Contradictions

None of these overturns a decision. Each is a place where a decision, as worded, does not fit a
primary source. All were put to the owner and settled as Q10–Q14 above.

- **C1. Q8 versus Solana devnet.** Q8 fails "a single key able to … replace channel code" and has no
  devnet exemption. `payment-channels` on devnet is upgradeable by one keypair, `4zTeC5mV…spap`, on
  the curve (F2). This repository's own nodes settle there (ADR 0075). ADR 0075 also found devnet's
  binary is not the audited source. Read literally, Q8 refuses the Q3 table's Solana devnet entry,
  which ADR 0075 already admits. Q4 relaxes only the audit for testnets, not the key rule. The ADR
  needs to say either that Q8 binds per network (and Solana devnet is grandfathered or refused) or that
  Q8 is judged on mainnet code only.
- **C2. Q3's "the ledger feature is enabled" check would refuse XRPL testnet.** `PayChan` is a retired
  amendment (`XRPL_RETIRE_FEATURE(PayChan)`). On testnet, `feature PayChan` reports `enabled: false`
  and the Amendments object omits it, yet payment channels run there (F4). A boot check that asks
  "is PayChan enabled?" passes on mainnet and refuses testnet, the one network Q4 lets an unaudited
  binding use. The check has to treat retired-and-supported as present: read `supported` with the
  server's `feature` output, or probe `account_channels`, or skip the check for retired features. The
  only live XRPL channel amendment to gate on is `fixPayChanCancelAfter`, and it is enabled on both
  networks. Whether a standalone `xrpld --start` reports PayChan enabled was not checked.
- **C3. PENDING §2.7: "enabled since 2020" is wrong.** PayChan was enabled on mainnet
  2017-03-31T06:27:20Z, in ledger 28 685 313 (F4). This changes no decision.
- **C4. Q8's Mina clause cannot be met as worded, and MINA-NOTE D7(b) does not exist.** "Not deployed
  with a single TOON key able to change its verification key" holds only until the next hard fork that
  raises the transaction version. Then the zkApp's single account key regains that power, whatever
  permission was set, `impossible` included (F7). Mina just had such a fork (Mesa, 4.0.0). So a
  TOON-owned Mina zkApp passes Q8 only if:
  - (a) its key is destroyed, which cannot be shown on chain and strands channels at the next such
    fork unless they are drained first; or
  - (b) Q8 accepts a key whose power is confined to a protocol-version change.

  Q8 offers neither. The Mina ADR has to pick one.

- **Minor, in PENDING §5.1.** The Subbit README says "alpha", not "unaudited". The "unaudited" is
  #3580's statement (F6). That is still true as far as anything here shows: no audit is published.

---

## Unconfirmed, and why

- **Who the five Squads members are.** They are keys, not named parties, and nothing on chain names
  them. Q8 wants "an accountable party named with its threshold". Solana's ADR can name
  solana-foundation as the party only on the foundation's own word, which was not looked for here.
- **Whether Hedera enforces contract expiry.** HAPI says an admin-key-less contract is immutable
  "except for expiration and renewal". The escrow's `expiration_timestamp` is 2026-12-09T18:14Z, and
  it has no auto-renew account. HIP-16 (entity auto-renewal) is marked Final, but whether expiry
  actually removes contracts on testnet or mainnet today was not established. An ownerless escrow that
  can expire is a liveness risk Q8 does not cover.
- **Whether the Hedera admin key can sweep escrowed HTS tokens in practice.** The protobuf says a
  delete sends "all HBAR and other tokens" to the obtainer. It was not tested.
- **That Mesa itself raised the transaction version.** The deployed mainnet daemon's source says 4 and
  a 2025 commit says 3, but the Mesa release notes read here do not state the bump. The devnet node
  Minascan exposes reports a 2025 commit, which may be stale.
- **That a Cardano script hash pins its code** was taken from Cardano's design, not read in the ledger
  spec. It is standard and was not doubted, but it was not cited to a source here.
- **Subbit on mainnet.** Koios mainnet knows no script with hash `6d877463…`. Only preprod has a
  reference script. That matches Q4 (testnet entries only until audited) but means there is no
  mainnet entry to put in the table even if one were wanted.
- **Hedera's lack of a disposable test chain** (Q5) was not re-checked; PENDING §3.5 is the source.
