# A client may pay over an x402 batch-settlement channel, and only a client

**Status:** Accepted (owner decision, 2026-09-25, issue #1329) — **built** (epic #1349, tickets #1340–#1347). All three prerequisites were settled before acceptance, the last by a live Base Sepolia deposit on 2026-09-25, and the choices this record makes beyond #1329's seven were accepted with it. It **amends** [0059](0059-a-channel-is-derived-from-its-participants.md) (a client-edge exception to one-live-channel-per-pair), [0005](0005-claims-are-truth-balances-are-a-projection.md) (a second freshness rule for the journal to hold), `client-edge-spec.md` §1.3 (its freshness step, and step 5's rule that a cached deposit is a lower bound, which is false for a channel a payer can withdraw from) and `CONTEXT.md`'s **Claim**, **Nonce** and **Watermark**, plus a new **Voucher** entry (all applied). It **extends** [0024](0024-peer-wire-claims-sign-the-eip-712-balance-proof.md) and [0053](0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md) with a second claim scheme per chain, and it disturbs [0021](0021-vectors-are-normative-prose-is-not.md): `schema_version` goes to **6** when #1347 lands. It leaves [0022](0022-a-connector-answers-it-does-not-announce.md)'s deferral of paying over HTTP exactly where it is.

**Amended 2026-09-25 (#1349 review):** EVM admission now requires a nonzero `payerAuthorizer` whatever the payer is, where it had refused only a contract-wallet payer without one (decisions 2 and 4, and the last of the choices beyond #1329's seven). An EOA can gain code later by an EIP-7702 delegation, after which the contract checks a voucher by ERC-1271 rather than ECDSA and the vouchers already accepted no longer verify the way they did. Decision 3 also records the journal's second amendment to 0005 (the `BatchChannelAdmitted` entry, which holds an EVM channel's `ChannelConfig` so held vouchers stay claimable after a restart), decision 8 records the greeting's wire names (the EVM asset's EIP-712 `name`/`version`, Solana's `withdrawDelay` for the minimum `grace_period`, and the new `minDeposit`), and decision 3's retransmission rule now says what the code always did: a byte-identical voucher buys nothing, so against a nonzero charge it is refused as an underpayment, and the vectors pin that case too.

**Scope:** protocol law. It binds every implementation, because it adds a claim scheme to the wire and an offer to the greeting. The watchers and sweeps in decision 5 and the port shape in decision 9 are connector architecture. See the [ADR index](README.md).

**A client may pay a connector over an x402 `batch-settlement` channel, on Base and on Solana: the
audited contract and program x402 already deploys, with no TOON contract involved.** It is a second
way to pay at the **client edge only**. Value moves one way, client to connector. Peering, client
payout and every channel a connector opens itself stay on `TokenNetwork` and the TOON Solana program.
A **voucher** is a claim in this scheme. It is still a claim: journaled, one per packet, cumulative,
superseding, carried inside ILP. It names its own channel, and that channel is verified on chain
rather than derived. Its freshness is an amount-only watermark, because it has no nonce. In return,
a client can onboard and pay with no native gas: on EVM a stock x402 facilitator relays the deposit,
and on Solana the receiving connector's operator sponsors the open.

## Sources

Every upstream claim below is cited at a pinned commit. Nothing here is from recall.

- **x402** at [`0cb1a1f0`](https://github.com/x402-foundation/x402/tree/0cb1a1f0f4c2163357e255c824d319674e1db43f).
  Cited as **X402** plus a path. The contract is `contracts/evm/src/x402BatchSettlement.sol`, and
  the specs are `specs/schemes/batch-settlement/scheme_batch_settlement_{evm,svm}.md`. **A bare
  `#Lnn` cites `x402BatchSettlement.sol`**; every other file is named.
- **payment-channels** at [`3ffa4d67`](https://github.com/solana-foundation/payment-channels/tree/3ffa4d6728ad88e4a9667a76ad9ccd68a302c696).
  Cited as **PC** plus a path under `program/payment_channels/src/`.
- **Deployments**, read live on 2026-09-24:

  | Chain                          | What                                                                                                                                                                                                               | Checked                                                                                                                                                                                                                             |
  | ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
  | Base Sepolia and Base mainnet  | `x402BatchSettlement` `0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003`, `ERC3009DepositCollector` `0x4020806089470a89826cB9fB1f4059150b550004`, `Permit2DepositCollector` `0x4020425FAf3B746C082C2f942b4E5159887B0005` | Code is present on both chains. `eip712Domain()` returns `("x402 Batch Settlement","1")`, and chain 84532 / 8453 respectively. `VOUCHER_TYPEHASH()` is `0x1e1bd6ff…9a69` on both. `owner()` reverts, and the EIP-1967 slot is zero. |
  | Solana devnet and mainnet-beta | payment-channels `CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX`                                                                                                                                                    | Owned by the upgradeable loader. The upgrade authority is `DXtFpbPj…XMVv` on mainnet (last deploy slot 431447053) and `4zTeC5mV…spap` on devnet.                                                                                    |

- **Audits.** The EVM contract has Cantina's May 2026 report (`contracts/evm/audits/cantina_x402_may2026.pdf`), which reviewed commit `ca60063c`, not the pin. It found 12 issues: 1 medium and 3 low, of which 9 were fixed. The Solana program has Cantina's July 2026 report (PC `audits/`), which reviewed `0c07d575`. `git diff 0c07d575 3ffa4d67 -- program/` is empty. That report found 9 issues, all informational and all acknowledged.
- **Research this record rests on.** toon-protocol/infra `docs/research/x402-batch-settlement-as-toon-channel.md`, and this repository's [`docs/research/x402-and-channel-funding.md`](../research/x402-and-channel-funding.md). How the two are reconciled is covered at the foot of this record.

## Why the client edge, and nowhere else

We tried x402 channels as TOON's settlement layer in general, and four rules rule that out. At the
client edge, two of the four stop biting:

| TOON rule                                                                                                                                                                    | x402                                                                                                                                                                                                                                                                      | At the client edge                                                      |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------- |
| A peering pays and is paid on one channel ([0042](0042-a-packet-carries-its-claim.md), #1146)                                                                                | Value moves one way. The EVM contract has exactly three outbound transfers: `settle` to `receiver`, and `finalizeWithdraw` and refund to `payer` (X402 `x402BatchSettlement.sol#L304`, `#L384`, `#L585`). The Solana program has no instruction that debits the receiver. | **No conflict.** Client to connector is TOON's one purely one-way flow. |
| [0059](0059-a-channel-is-derived-from-its-participants.md): one live channel per pair, derived                                                                               | EVM: `channelId` is an EIP-712 digest of a `ChannelConfig` that includes a free `salt` (`#L36-L44`, `#L442-L446`). Solana: the PDA seeds include `salt` and `open_slot` (PC `state/channel.rs#L238-L258`). Many channels per pair are legal on both.                      | **Reconcilable** (decision 2).                                          |
| A claim's nonce may advance at an unchanged amount (`connector-domain/src/claim.rs`, 0005)                                                                                   | A voucher has no nonce (decision 4).                                                                                                                                                                                                                                      | **Conflict, resolved** (decision 3).                                    |
| Client payout nets against the client's deposit: headroom = `deposit − owed + credited` ([0026](0026-client-btp-rides-the-client-edge-peers-stay-on-the-peer-wire.md), #700) | There is no reverse leg.                                                                                                                                                                                                                                                  | **The loss is accepted.** An x402 client is payer-only.                 |

## Decision

### 1. Scope: the client edge, on both chains, payer-only

- **Where a voucher is accepted.** A connector may accept a voucher at the client edge, on the
  inbound claim gate that `client-edge-spec.md` §1.3 already defines. It rides where a claim rides
  today: the claim header on `POST /ilp`, and `payment-channel-claim` protocolData on BTP.
- **Where it is refused.** A voucher is **never** a peer claim. A peer carriage refuses it
  structurally, and it can never decide the role `peer`.
- **A channel's direction.** A connector never opens, funds or signs on a batch-settlement channel.
  It only receives on one.
- **Payouts.** A client that expects a payout must hold a `TokenNetwork` or TOON-program channel.
  `ClientPayoutLedger` credits nothing to a batch-settlement channel.
- **Opt-in.** Accepting these channels is per chain and **off unless configured**. A connector that
  has not opted in offers no `batch-settlement` entry and refuses a voucher by name.

### 2. Channel identity: named by the voucher, verified on chain, not unique per pair

[0059](0059-a-channel-is-derived-from-its-participants.md)'s derivation rule exists for **peering
symmetry**, the argument it takes from [0058](0058-a-peering-is-established-from-a-url.md): _"B must be able to add A the same way A added B and land on the same channel, without
either telling the other an id."_ The client edge has no such symmetry.

- The client opened the channel.
- Every voucher names its channel. On EVM that is the `channelId` it signs. On Solana it is the
  channel account at bytes 2..34 of the 50-byte message.
- The client edge has always taken the channel id from the claim and resolved it on chain
  ([0052](0052-permissionless-payment-is-guaranteed-and-a-claim-is-what-authorises.md)).

So this is a **client-edge exception to 0059's uniqueness**, not to its reasoning:

- **A second live channel from the same client is not refused.** Refusing it would need an identity
  to refuse by, and 0052 says an identity authorises nothing. Each channel carries its own
  collateral and its own watermark, so a second channel adds no risk that the first did not. 0059
  still binds every channel a connector opens itself.
- **EVM: the client presents its `ChannelConfig`, and the connector recomputes the id.** A voucher
  signs only `channelId`. The config is not readable from the chain, because the contract stores
  channels by id. So the first voucher on a channel the connector has not seen carries the full
  config. The connector computes `getChannelId(config)` off chain (an EIP-712 digest, no RPC) and
  refuses a mismatch. It then reads `channels(channelId)` and `pendingWithdrawals(channelId)` for
  collateral.
- **EVM: the connector fixes three of the seven config fields, and requires a fourth.** It admits
  a channel only if:
  - `receiver` **and** `receiverAuthorizer` are both this connector's EVM settlement address;
  - `token` is a token it settles in;
  - `withdrawDelay` is at least its published minimum (decision 5);
  - `payerAuthorizer` is nonzero, whatever `payer` is (amended 2026-09-25). With a zero one, the
    contract checks each voucher against `payer` through `SignatureChecker`, which asks ERC-1271 of
    any payer with code. An EOA payer can gain code at any time by an EIP-7702 delegation, so
    whether `payer` has code at admission says nothing about whether the vouchers accepted since
    will still verify by ECDSA when they are claimed. The connector does not ask; it refuses.

  `payer`, `payerAuthorizer` and `salt` are the client's: `payerAuthorizer` must exist, but which
  key it names is not fixed. **`salt` is not fixed.** Fixing it would buy derivability, and nothing
  at this edge needs that.

- **Solana: the connector reads the channel account and checks it.** It re-derives the PDA from the
  account's own seed fields before trusting it (X402 SVM spec `#L1379-L1385`). It then admits the
  channel only if:
  - `status` is Open;
  - `payee` **and** `rent_payer` are this connector's sponsor key (decision 5);
  - `mint` is a mint it settles in;
  - `distribution_hash` commits to exactly one recipient, this connector's receiving account, at
    10000 bps;
  - `grace_period` is at least its published minimum.

  `authorized_signer`, `salt` and `open_slot` are the client's.

### 3. Freshness without a nonce: an amount-only watermark

A voucher is accepted only if its cumulative amount **strictly advances** this connector's watermark
for that channel. The value-binding step (`client-edge-spec.md` §1.3 step 3) then requires that
advance to be at least the route's charge. A voucher that does not advance is refused as
`amount_not_advancing` — which for a voucher means _not strictly greater_, where today's
`AmountNotAdvancing` means _less than_; the vectors pin the difference (decision 7). It is refused before any signature check, which keeps §1.3's "freshness
and value before cryptography".

- **What a zero-value packet carries.** Nothing. An unpaid request to an explicitly free route
  already reaches the app (`connector-client-edge/src/lib.rs`,
  `an_unpaid_request_to_a_free_route_still_reaches_the_app`). So a packet to a free route carries
  **no voucher**. The nonce's one job that an amount cannot do was to let a zero-value packet carry
  a fresh claim, and at the client edge that job was never needed. A client that attaches a
  voucher to a free route must still advance the amount, because the rule has no exception.
- **Replay.** A replayed or reordered voucher fails to advance and costs nothing. This is
  `CONTEXT.md`'s **Claim** — "a replayed claim gains nothing" — reached without a counter.
- **Retransmission.** A voucher byte-identical to the one that set the watermark is a
  retransmission, not a new claim. It is answered exactly as the same edge answers a
  `toon-channel` claim retransmitted at its watermark today (the `peer_claim_retransmit` vector's
  rule): it buys nothing new, and it is not an error. Byte identity is the test, because an
  equal amount under a different signature is not the same voucher. **Because it buys nothing, it
  covers no charge** (amended 2026-09-25): against a route whose charge is zero it is accepted
  again, and against a nonzero charge it advances by `0` and is refused as an underpayment, by the
  value-binding step like any other voucher that advances too little. The vectors pin both
  (decision 7).
- **The width of an amount.** The EVM voucher's amount is `uint128`. A voucher above what the
  connector's amount type holds (`u64` today) is refused, not truncated.
- **Solana's `expiresAt` must be zero.** x402 already requires this, and servers reject a nonzero
  value (X402 SVM spec `#L313`, `#L1218-L1223`). The program would refuse an expired voucher at
  `settle` with no state change (PC `voucher.rs#L79-L82`). So a nonzero `expiresAt` is value that
  can lapse before the connector lands it. The connector refuses it structurally. It never signs a
  voucher itself, so it has no use for expiry.
- **The journal.** [0005](0005-claims-are-truth-balances-are-a-projection.md)'s journal holds a
  voucher exactly as it holds a claim: the signed bytes, and the watermark they set. The
  watermark's _key_ is §1.3's (peer, blockchain, channel) tuple, with the channel in its canonical
  form. Only its _comparison_
  differs by scheme. This is the first clause of 0005 that is amended.
- **What the journal also holds** (a second amendment to 0005, recorded 2026-09-25). With a
  channel's first accepted voucher, the journal records the channel itself, as
  `JournalEntry::BatchChannelAdmitted`: its canonical key and, on EVM, the `ChannelConfig` it was
  admitted under (on Solana, nothing more, since the channel account holds every field). 0005
  persists only what is signed or irreversible, and a config is neither. It is journaled anyway
  because the chain stores an EVM channel by id alone and a voucher signs only that id, so after a
  restart the config exists nowhere else, and `claim` cannot be sent without it. Without the entry,
  every voucher accepted before a restart would be unclaimable while the payer withdrew (decision
  5). The entry is written in the same batch as the voucher it arrives with, and folds into no
  balance.

### 4. Two claim schemes: the `toon-channel` claim and the batch-settlement voucher

A client-edge claim gains a **scheme** discriminator. When it is absent, the claim is `toon-channel`
and means exactly what it means today. When it is `batch-settlement`, the claim is a voucher:

| Chain  | Signed message                                                                                                                                                                          | Signer                                                                                                                                                                                                                                                                                                                                                    |
| ------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| EVM    | The EIP-712 digest of `Voucher(bytes32 channelId,uint128 maxClaimableAmount)` (X402 `#L96`, `#L454-L456`), under the domain `("x402 Batch Settlement", "1", chainId, 0x4020074e…0003)`. | `payerAuthorizer` if it is nonzero, otherwise `payer` (`#L530-L538`). **ECDSA only.** A zero `payerAuthorizer` leaves the check to `payer`, which needs an ERC-1271 `eth_call` whenever `payer` has code, as an EOA does after an EIP-7702 delegation; so a zero one is refused at admission (decision 2, amended). The client names a `payerAuthorizer`. |
| Solana | 50 bytes: `0x56 0x01 ‖ channel_id(32) ‖ u64 cumulative LE ‖ i64 expires_at LE` (PC `instructions/mod.rs#L27-L93`; X402 SVM spec `#L316-L323`). Ed25519.                                 | The channel's `authorized_signer`, which is fixed at open (PC `state/channel.rs#L135-L138`).                                                                                                                                                                                                                                                              |

- **Each voucher's domain is bound as 0024 and 0053 require.** The EVM digest binds the chain and
  the contract through its domain separator. The Solana message binds the channel account, and the
  account is the program's own PDA. The connector takes the program id from its own build (one
  constant, the same on every cluster; amended by the #1349 review, which removed the config key),
  never from the claim, and x402 forbids negotiating it on the wire anyway (X402 SVM spec `#L78-L86`).
- **The signer is taken from the chain, never from the claim.** It comes from the verified
  `ChannelConfig` on EVM and from `authorized_signer` on Solana. This is §1.3 step 4's rule — _"the
  counterparty recorded for the channel"_ — unchanged.
- **What `CONTEXT.md`'s "_Avoid_: voucher" becomes.** **Claim** stays the umbrella term. **Voucher**
  now names one thing: a claim under the `batch-settlement` scheme. It is never a synonym for a
  `toon-channel` claim, and a `toon-channel` claim is never called a voucher.

The exact field names on the wire are fixed by the vectors (decision 7), not by this prose (0021).

### 5. Safety

**EVM.**

- **Claim before a withdrawal lands.** Only `totalClaimed` recorded on chain protects the receiver.
  A signed but unclaimed voucher does not (`#L320-L323`, `#L352-L353`). The connector watches
  `WithdrawInitiated(bytes32 indexed channelId, uint128 amount, uint40 finalizeAfter)` (`#L147`)
  for every channel it holds a voucher on. On one, it `claim`s its latest voucher at once.
- **What a pending withdrawal leaves as collateral.** A voucher is accepted only up to
  `balance − totalClaimed − pendingWithdrawal`. That figure can **fall**, which §1.3 step 5 says a
  deposit never does: its licence to cache a deposit as a permanent lower bound does not extend to
  a batch-settlement channel on EVM. The cache is dropped on `WithdrawInitiated`, and re-read.
  On Solana it holds while the channel is Open, since only `top_up` moves `deposit` there, and no
  voucher is accepted once a channel is Closing.
- **The minimum `withdrawDelay`.** The contract admits 15 minutes to 30 days (`#L85-L86`). A
  connector publishes its own minimum. That minimum may be no lower than the contract's, and it
  defaults to **one day**. The day is the window in which a censored or delayed `claim` must still
  land. Cantina's finding 3.3.1 is exactly this race, and it was resolved in documentation only.
- **`receiverAuthorizer` is never delegated.** It can refund to the payer any value the receiver
  has earned but not yet claimed (`#L399-L429`). Delegating it hands a facilitator the power to
  grief the connector. x402.org's facilitator advertises no `receiverAuthorizer` for this scheme on
  Base Sepolia, which is consistent with this rule.

**Solana: the sponsor is the receiving operator, as a rule.** In x402's SVM scheme, the address that
opens a channel (`extra.feePayer`) takes three seats at once: it is fee payer, `rent_payer` and a
**zero-share `payee`**. The receiver is a separate 10000 bps distribution entry (X402 SVM spec
`#L36-L44`, `#L109-L116`, `#L193-L206`). Read from the program, here is every power those seats hold.
This settles the third prerequisite:

- **`payee`.**
  - It is the only signer of `settle_and_seal` (PC `settle_and_seal.rs#L83-L85`).
  - From Open, it may seal **at any time**, without a voucher (`#L153`).
  - From Closing, it may seal only while `now < closure_started_at + grace_period` (`#L93-L107`).
  - It is also a PDA seed, and the implicit-remainder beneficiary, which gets 0 under x402.
- **`rent_payer`.** It signs `open` and funds the rent (PC `open.rs#L339-L371`). After that it holds
  no signing power at all. It is only the bound recipient of rent at `distribute` and `reclaim`.
- **What nobody can change.** `payee`, `authorized_signer`, `distribution_hash`, `mint`,
  `rent_payer`, `payer` and `grace_period` are written only at open (PC `state/channel.rs#L311-L317`).
  So no seat can redirect funds, change the signer or create a nonzero claim.

Two facts make a third-party sponsor a loss to the connector, not merely a risk:

1. **It can seal before the connector settles.** `settled` then freezes, and the difference goes
   back to the payer (PC `distribute.rs#L317-L318`).
2. **Once the payer calls `request_close`, the payee is the only party that can land a voucher.**
   `settle` works only while Open (PC `settle.rs#L51-L53`), and afterwards only the payee's
   `settle_and_seal` accepts one. A connector that is not the payee has no way to collect anything
   once a client starts to close.

So **`payee` and `rent_payer` must be this connector's sponsor key**, which is its Solana settlement
key. That is the only configuration in which the connector can always land its latest voucher. It
does not make the connector custodial: the escrow is a program-owned account, payouts go only to the
addresses fixed at open, and the sponsor controls _when_ a channel closes, never _where the money
goes_.

- **The minimum `grace_period`.** The program's only bound is `>= 1` second (PC
  `open.rs#L267-L269`). A connector publishes its own minimum. That minimum may be no lower than
  x402's 900 seconds (X402 SVM spec `#L177`), and it defaults to **one day**, for the same reason as
  EVM's `withdrawDelay`.
- **What the connector does on a closing channel.** It settles promptly while a channel is Open. It
  watches for Closing, and then lands its latest voucher with `settle_and_seal` before the grace
  period ends. It accepts no new voucher on a channel that is Closing.
- **Rediscovering sponsored channels.** A sponsor rediscovers its channels with `getProgramAccounts`,
  filtering on `dataSize` 256 and `memcmp` against `rent_payer` at offset 216 (PC
  `state/channel.rs#L81-L156`; X402 SVM spec `#L1347-L1385`). A live mainnet query returned the
  channels of one `rent_payer`. That query is what finds channels for `distribute` and `reclaim`, so
  the rent float comes home. The journal is not the index for this.
- **Rent float.** Sponsoring costs 4,711,920 lamports per channel until reclaim (Cantina 3.1.9). The
  program's `deposit > 0` rule makes an attacker lock its own capital for at least `grace_period`
  for every channel it makes a connector sponsor. The connector also refuses to sponsor below a
  published minimum deposit.
- **An unusable token account forfeits to the program's treasury** (PC `distribute.rs#L157-L170`;
  Cantina 3.1.4). The connector refuses to sponsor unless its own receiving account and the payer's
  canonical ATA both already exist and are usable.

**Both chains: the trust statement.**

- **EVM.** The contracts are ownerless and immutable. Nothing here can change them, and nothing can
  rescue a stuck escrow either: a USDC-blacklisted payer or receiver traps it for good (Cantina
  3.2.1, acknowledged).
- **Solana.** payment-channels **can be upgraded by solana-foundation's key**. Whether that
  authority is a single keypair or a multisig vault was not determined. Whether the deployed binary
  equals the audited source was not verified on either cluster. The audit's scope list omits
  `settle.rs`, `top_up.rs` and `request_close.rs`.
- **What the connector risks.** At most what it has accepted in vouchers but not yet landed, on the
  channels it holds. The client's exposure is its own deposit.
- **Who takes that trust on.** An operator who opts in to either chain (decision 1) accepts this.
  The fleet does not opt in by default.

### 6. Session keys: a `payerAuthorizer` is admitted

A signer distinct from the funding key is admitted: `payerAuthorizer` on EVM, and on Solana an
`authorized_signer` that is not the payer. This does not reopen
[0060](0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md):

- 0060 makes a claim the proof of a **peering**. A voucher never decides the peer role (decision 1).
  At the client edge a claim proves only that value moved, and 0052 already says identity
  authorises nothing.
- Which key signs is a fact fixed on chain at open. The connector reads it (decision 4).
- A session key is the ordinary case here. The owner's key funds the channel once, and a hot key
  signs every packet.
- On EVM it is more than admitted: since the amendment to decision 2 (2026-09-25) a channel must
  name one. The key may be any the client holds, the payer's own included.

### 7. Vectors: two voucher cases, and `schema_version` 6

`vectors/wire-vectors.json` gains `claim_voucher_evm` and `claim_voucher_solana`, generated from
fixed literals as 0045 corrects 0021 to say. Each vector carries:

- the claim JSON with its `scheme` discriminator;
- the signed digest or message, in hex;
- the signature;
- for EVM, the `ChannelConfig` and the `channelId` it hashes to.

It also gains:

- an **amount-only watermark** case: equal amount refused, higher amount accepted;
- a **nonzero `expiresAt`** case: refused.

`schema_version` goes from 5 to **6**. `toon-client`, `rig` and `swap` replay it. Each case must
also be checked against the deployed contract's own `getVoucherDigest`, so the vector cannot drift
from the chain it names.

### 8. The greeting offers a second option

`accepts[]` keeps its `toon-channel` entry and gains one `batch-settlement` entry per chain the
connector has opted in to. Unlike today's entry, that entry is **x402-valid**:

- `network` is CAIP-2;
- `asset` is the token address or the mint;
- `payTo` is the receiver: the EVM settlement address, or on Solana the owner of the receiving
  account.

Its `extra` carries what a client needs to open a channel this connector will admit. The wire
names, recorded 2026-09-25, are x402's own wherever x402 has one:

- **EVM:** `receiverAuthorizer`, the minimum `withdrawDelay`, and `name` and `version`: the
  EIP-712 domain of the deposit's **asset**, which a client signs its ERC-3009 or Permit2
  authorization under. x402's EVM scheme requires both, and an ERC-20 need not expose either, so
  the connector does not read them off the chain: they come from the required config keys
  `asset_eip712_name` and `asset_eip712_version`.
- **Solana:** `feePayer`, which is the sponsor key; `withdrawDelay`, which carries the minimum
  `grace_period` under x402's SVM field name (x402 calls the program's `grace_period`
  `withdrawDelay` on both chains, and a stock client reads that name); and `minDeposit`, the
  published minimum deposit decision 5 has the sponsor refuse below, as a decimal string of the
  mint's base units. `minDeposit` is this connector's own addition: x402 has no field for it.

The self-description publishes the same facts. Vouchers still travel inside ILP. Only the one-time
deposit or open leaves the packet path, which is why [0022](0022-a-connector-answers-it-does-not-announce.md)'s
deferral of paying over plain HTTP is **untouched**. That deferral names "the x402 onramp" in so many words, but what it defers is a
different thing: a plain HTTP request carrying a one-shot payment that settles per request. Here
nothing settles per request. A voucher is a claim, and the chain sees only the deposit and the
sweep, so 0004 and 0005's "claims are constant, settlement is rare" holds.

### 9. The settlement port: a separate, receive-only port

`SettlementBackend` assumes two sides and open, fund, close and settle
(`connector-settlement/src/port.rs`). A batch-settlement channel has no `open` the connector calls,
no side of its own, and a different lifecycle on each chain. On EVM that is claim then sweep. On
Solana it is `settle`, `settle_and_seal`, `distribute` and `reclaim`.

Bending `SettlementBackend` to fit would leave `own_deposited` and `fund` meaning nothing. So the
backend implements a **new receive-only port** instead. Its contract suite is written once and run
against both chains' implementations (ADR 0007). The implementations live in
`connector-settlement-evm` and `connector-settlement-solana` as modules, reusing their RPC clients
and keys; they are not new crates.

**The Solana sponsor endpoint is public.** The channel does not exist yet, so the endpoint cannot be
paid for and cannot be an operator write. It must be callable by a buyer the connector has never
heard of (0052). The endpoint takes a client-built `open` transaction. It checks every field decision
2 and decision 5 fix, the minimum deposit, and that both token accounts exist. Only then does it
co-sign as fee payer and `rent_payer`.

## Prerequisites, as found

1. **Is a zero-value voucher accepted on a fresh EVM channel's deposit? Refuted as a design
   assumption — by reading the contract, the spec and all three reference facilitators, and then
   by a run.** The finding is that it depends on the facilitator, so no run against one facilitator
   could have confirmed it for all. The TypeScript half has since been run: the infra sandbox's
   own facilitator (toon-protocol/infra#23, the published `@x402/evm` 2.27.0 over the Base Sepolia
   bytecode placed at its production addresses) refuses a zero voucher on a fresh deposit with
   `invalid_batch_settlement_evm_cumulative_below_claimed` and settles a one-unit one, and
   `make smoke-x402` there pins that answer.
   - **The contract.** It takes no voucher at deposit
     (`deposit(config, amount, collector, collectorData)`, `#L200`). A zero-amount `claim` is a silent no-op (`#L522`). So the chain is
     indifferent.
   - **The spec.** It contradicts itself. Rule 10 (`scheme_batch_settlement_evm.md#L505`) requires
     `>` on-chain `totalClaimed`, and the error table (`#L605`) allows `>=` at deposit.
   - **The reference facilitators.** They split. TypeScript and Python reject a zero voucher on a
     fresh channel (`typescript/.../facilitator/deposit.ts#L300-L305`,
     `python/.../facilitator/deposit.py#L741`). Go accepts it (`go/.../facilitator/deposit.go#L196-L200`).
   - **So:** a client must sign its deposit's voucher for at least one base unit. It may use its
     first packet's charge and then present the same voucher with that packet. The connector's
     watermark starts at zero whatever the facilitator saw. The facilitator gains no claimable power
     from holding that voucher, because `receiverAuthorizer` is the connector's (decision 5).
2. **Does a hosted facilitator accept `batch-settlement` deposits on Base Sepolia for an arbitrary
   `payTo`? Confirmed, by a live deposit.**
   - `GET https://x402.org/facilitator/supported` lists `{"scheme":"batch-settlement","network":"eip155:84532"}`
     (read live, 2026-09-24). It advertises no `receiverAuthorizer`, and it does not list Base
     mainnet.
   - CDP's docs list the scheme on Base and Base Sepolia. Its `/supported` endpoint needs
     authentication.
   - The protocol requires only `channel.receiver == payTo` (`scheme_batch_settlement_evm.md#L498`),
     and neither facilitator documents an allowlist.
   - **Run on 2026-09-25.** A deposit went through x402.org's facilitator on Base Sepolia:
     - **payer:** a fresh wallet;
     - **`payTo` and `receiverAuthorizer`:** a second fresh address the facilitator had never seen;
     - **token:** the devnet's own mock USDC, `0x49beE1Bc…a9Ce`.

     The result:
     - `/verify` and `/settle` both succeeded.
     - Transaction `0x54e792b8…d7a5` in block 47284845 was sent **from x402.org's signer**
       (`0xd407e409…f1bf`) to `x402BatchSettlement`, so the facilitator paid the deposit's gas.
     - Channel `0x25712fc6…a8ec` holds 1,000,000 base units on chain.

     No allowlist, screening or registration stood in the way of the arbitrary `payTo` or the
     unfamiliar token.

   - **One caveat, found on the way.** The mock USDC has no ERC-3009 and no EIP-2612, so a
     deposit takes the Permit2 path, which needs a one-time `approve` from the payer. x402.org
     advertises `erc20ApprovalGasSponsoring`: fund the payer's gas, then broadcast its signed
     approval. On the first attempt it broadcast the approval **without funding it**, and `/settle`
     failed with `invalid_batch_settlement_evm_deposit_transaction_failed` (insufficient funds).
     The deposit above succeeded only after the payer approved Permit2 itself, for 0.00000028 ETH
     of Base Sepolia gas.

     So on x402.org, a deposit is fully gasless only for an ERC-3009 token. The devnet runs its
     own facilitator anyway (toon-protocol/infra#23); that one must fund the approval itself if it
     is to offer gasless deposits of the devnet's mock USDC.

3. **Every power of the Solana `payee` / sponsor seat. Confirmed from the program.** The full list
   is under decision 5. `payee` signs only `settle_and_seal`. `rent_payer` has no signing power
   after open. Neither can move money anywhere but to addresses fixed at open.

EVM gas is a business arrangement outside the protocol. `x402BatchSettlement` has no fee field, and
CDP bills off chain, charging $0.001 per on-chain transaction after 1,000 a month. On Solana, the
operator-sponsor pays the fees as a cost of selling and gets its rent back.

## Reconciling the research note

[`docs/research/x402-and-channel-funding.md`](../research/x402-and-channel-funding.md) says _"a
facilitator cannot open or fund a payment channel."_ **That is true of what it examined:** x402's
`exact` scheme, and TOON's own `TokenNetwork` and program. `exact` settles by a bare
`transferWithAuthorization` to `payTo`, and neither TOON contract accepts a third party's deposit on
a client's behalf.

It is not true of `batch-settlement`. That scheme's contract is itself the `to` of the
authorization. `deposit` is callable by anyone, and it pulls from `config.payer` through a collector.
The ERC-3009 nonce is `keccak256(channelId, salt)` (`ERC3009DepositCollector.sol#L34-L51`), and the
Permit2 witness is `DepositWitness(channelId)` (`Permit2DepositCollector.sol#L29-L33`). So the payer's
one signature binds the deposit to its channel, and the first deposit creates the channel.

That is the note's own **Option C**, the `receiveWithAuthorization`-style deposit wrapper, which the
note costs as "a new audited contract". It already exists, it has been audited, and it is deployed
at one address on Base Sepolia and Base mainnet. The note's verdict stands for `TokenNetwork`, and
nothing here changes it. This record takes the other road, using a channel that is not TOON's.

## Considered options

- **x402 channels as TOON's settlement layer everywhere.** Rejected. See the table above: bidirectional
  peering and client payout have no x402 shape.
- **Derive x402 channels too: a fixed `salt`, and one channel per pair.** Rejected. The Solana PDA
  includes `open_slot`, which no one can predict, so derivation is impossible there. On EVM it would
  buy symmetry nobody at this edge needs, and it would cost the client the ability to rotate a
  channel.
- **A synthetic nonce carried beside the voucher.** Rejected. A field no signature covers is a field
  anyone can rewrite. The one job it could do — a fresh claim on a zero-value packet — is not needed
  (decision 3).
- **Let a third party sponsor Solana channels.** Rejected. It is a loss, not a risk: after
  `request_close`, only the payee can land a voucher (decision 5).
- **Model the backend as a mode of `SettlementBackend`.** Rejected (decision 9).

## Consequences

- **A client with USDC and no native gas can pay a connector on either chain, with no TOON-specific
  onboarding.** On EVM a stock x402 facilitator relays the deposit, and on Solana the connector
  sponsors the open.
- **The greeting becomes x402-valid for the first time**, but only in its new entry. The
  `toon-channel` entry keeps its x402-shaped-but-bespoke fields, for the reason `client-edge-spec.md`
  §1.4 gives.
- **A connector that opts in takes on third-party contract and program risk.** The trust statement
  under decision 5 is the price of the onboarding.
- **Two freshness rules now exist**, one per scheme. `validate_claim`'s nonce rule is unchanged for
  `toon-channel`. The voucher rule sits beside it and never replaces it.

## Choices this record makes beyond #1329's seven

#1329 asked for decisions 1–7. These go further. The owner accepted each of them with the record,
on 2026-09-25:

- **Decisions 8 and 9 whole**: the greeting's shape, a separate receive-only port, and modules
  rather than crates.
- **The sponsor endpoint is public.** #1329 left it as "an operator-surface job or a small
  endpoint". It is public here because a buyer the connector has never heard of must be able to
  reach it (0052). That is a new unauthenticated surface that makes the node spend lamports.
- **A minimum sponsored deposit**, as the bound on that surface.
- **One day** as the default for both the minimum `withdrawDelay` and the minimum `grace_period`.
- **Refusing a channel with no `payerAuthorizer`**, so that no packet waits on an `eth_call` and
  no accepted voucher is stranded by a later EIP-7702 delegation. As accepted, this refused only a
  payer with code; the #1349 review amended it to every payer (2026-09-25).

## Glossary

Applied to `CONTEXT.md` on acceptance (2026-09-25). Its three entries now read:

> **Claim**: A signed statement of a payment channel's cumulative state, handed from payer to
> payee. Each claim supersedes the last, so a lost claim costs nothing and a replayed claim gains
> nothing. A claim has a **scheme**: `toon-channel`, or — at the client edge only —
> `batch-settlement`, whose claims are **vouchers**.
> _Avoid_: receipt, payment, balance proof; "voucher" for a `toon-channel` claim
>
> **Nonce**: The counter that orders `toon-channel` claims within a channel. A payee accepts such
> a claim only if its nonce advances. A voucher has none; its amount orders it.
>
> **Watermark**: The highest nonce a payee has accepted on a channel — for a voucher, the highest
> cumulative amount, which the next voucher must strictly exceed.

It also gained a **Voucher** entry: _A claim under x402's `batch-settlement` scheme (ADR 0074)._

## Implementation tickets

Filed on acceptance, each `ready-for-agent`, in the order #1329 lists them:

1. **#1340 Port.** The receive-only settlement port, and its contract suite.
2. **#1342 EVM backend.** Config admission (decision 2) and `claim`, against `test_support::Anvil` with
   the x402 contracts deployed.
3. **#1343 Solana backend.** Account admission (decision 2) and `settle`, against `SolanaValidator` with
   `CHNLx…` loaded into genesis.
4. **#1341 Claim schemes and the watermark.** The `scheme` discriminator; both voucher verifications in
   `connector-signer`; and the amount-only watermark and retransmission rule in
   `connector-domain`, with property tests.
5. **#1345 Greeting and self-description.** The `batch-settlement` `accepts[]` entries and published
   minimums (decision 8).
6. **#1344 Watchers and sweeps.**
   - EVM: the `WithdrawInitiated` watcher, dropping the collateral cache, and batched `claim` then
     `settle`.
   - Solana: the Closing watcher with `settle_and_seal`, then `distribute`, and `reclaim` through
     `getProgramAccounts` rediscovery.
7. **#1346 Solana sponsor endpoint** (decision 9).
8. **#1347 Vectors.** `schema_version` 6, the four cases of decision 7, and a cross-check against the
   deployed `getVoucherDigest`.
