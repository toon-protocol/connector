# The Mina binding of x402 batch-settlement: checking the decisions, and the next round

> **Research note, not a decision record.** It records decisions D1–D7, which the owner took on
> 2026-10-28 in the grilling of [#1552](https://github.com/toon-protocol/connector/issues/1552)
> (part of map [#1548](https://github.com/toon-protocol/connector/issues/1548)). It checks each one
> against primary sources and gives a verdict: **confirmed**, **needs adjustment** or
> **contradicted**. It then researches the next round's open questions, R1–R6, and recommends an
> answer to each. Those answers are **recommended, pending owner**. They are not decisions.
>
> It builds on [`mina-x402-batch-settlement.md`](mina-x402-batch-settlement.md) (cited as
> **MINA-NOTE**) and [`x402-pending-chain-bindings.md`](x402-pending-chain-bindings.md) (cited as
> **PENDING**), and on [`zk-note-verifier-spike.md`](zk-note-verifier-spike.md) (cited as
> **SPIKE**). It does not redo their ground. Primary sources only. Written 2026-10-28, and every
> source was read that day.

**Evidence tags.** CONFIRMED-CODE means read in source at a pinned commit. CONFIRMED-LIVE means read
from a running chain's GraphQL. CONFIRMED-DOCS means read in the maintainer's own documents, specs,
issues or PR threads. UNCONFIRMED means none of those, and says why. A sentence marked
**Inference** is this note's reasoning from confirmed facts, not a fact.

**Sources pinned.**

- **x402**: [`x402-foundation/x402@db2c98ec`](https://github.com/x402-foundation/x402/tree/db2c98ec9490e4e4286c862de845e667b037f219)
  (`main`, 2026-10-28T13:26Z). PENDING pinned `261d0c5e`. Nothing under
  `specs/schemes/batch-settlement/` or `contracts/evm/src/` that this note cites has changed since
  (`git log` on both paths). Cited as **X402**.
- **o1js**: [`o1-labs/o1js@a428f9b9`](https://github.com/o1-labs/o1js/tree/a428f9b97817177ef4b5868cf148c0ac67a1048d)
  (`main`, 2026-10-27), the commit MINA-NOTE pinned. Cited as **O1JS**. Its issues
  [#1588](https://github.com/o1-labs/o1js/issues/1588) and
  [#2043](https://github.com/o1-labs/o1js/issues/2043) were read as they stood on 2026-10-28.
- **Mina daemon**: tag [`4.0.3`](https://github.com/MinaProtocol/mina/tree/4.0.3) (2026-10-23, the
  current Mesa release). Cited as **MINA**.
- **proof-systems** (Rust `mina-hasher`): [`o1-labs/proof-systems@92274448`](https://github.com/o1-labs/proof-systems/tree/9227444890),
  the commit MINA-NOTE pinned. Cited as **PS**.
- **MIPs**: [`MinaProtocol/MIPs@5aa77c89`](https://github.com/MinaProtocol/MIPs/tree/5aa77c8911e7af39ff9c594175226b171ed8166d).
- **ChainAgnostic namespaces**: [`mina/caip2.md`](https://github.com/ChainAgnostic/namespaces/blob/5c515816a2828914bb77a3ed92f9445d2e29f785/mina/caip2.md)
  at `5c515816` (the last change under `mina/`, 2024-06-03). **SLIP-44**:
  [`satoshilabs/slips@570ed55b`](https://github.com/satoshilabs/slips/blob/570ed55b/slip-0044.md).
- **Live chain**: Mina mainnet GraphQL at `https://api.minascan.io/node/mainnet/v1/graphql`, read
  at 2026-10-28T14:26Z (best block 566,957). The same host's devnet node answered from
  `syncStatus: "BOOTSTRAP"` with pre-Mesa constants, so this note uses no devnet live figure.
- **This repository**: `HEAD` `9a039468` (branch `fleet-pin-drift-relay-only`), with ADR 0076 and
  ADR 0080 read from the **working tree**, which carries uncommitted amendments to both.

---

## Owner's decisions after this note (2026-10-28)

The owner took every recommendation below in the same grilling. This note's "recommended, pending
owner" labels are kept as they were written, so read them as settled:

- **D4 re-decided:** `withdrawDelay` stays in seconds in the channel config. The zkApp converts it
  to slots as `ceil(secs / 90)`, with the 90 s slot baked into the verification key, and times
  every check with `this.currentSlot.requireBetween`. It records `initiatedAt` at the window's
  upper bound. `network.timestamp` is never used. A change of slot time is an upgrade (#1559).
- **D2 adjusted:** each tag is hashed as Poseidon's prefix (`hashWithPrefix`). `salt` is one field.
  The channel id has its own tag (`x402-mina-channel***`). The network id is a TOON constant per
  network, never Mina's `chainId`.
- **D5 adjusted:** the Mina spec adds `withdrawDelay ≥ maxTimeoutSeconds`.
- **D6 adjusted:** the upstream spec defines an optional facilitator that proves and submits.
  TOON does not implement it.
- **D7 adjusted:** `AssetChain` gains a `Mina` arm, which amends ADR 0080 decision 7. Mina terms are
  told apart by their `network` prefix.
- **R1–R5 taken as recommended.** Claim and settle are fused into one `claim`, under an in-circuit
  `receiverAuthorizer` signature that anyone may prove and submit. The signed refund keeps its
  nonce. Parity is option (b), with `let_delay_pass` waiting real slots until #1554 finds a way to
  move the clock. Vectors are peer-side only, at `schema_version` 9. Upstream starts with a
  discussion, then a spec-only PR, opened only on the owner's go-ahead.
- **R6:** the `blockchain: "mina"` refusal narrows to the client edge, so a Mina `batch-settlement`
  voucher is accepted on a peer carriage. This amends ADR 0080 decision 1 and `CLAUDE.md`. The
  remaining gaps go to #1553 and zk_toon#26: payer-signed deposit and withdraw, an existing
  receiver account, channel storage, and top-up starvation.
- **Settlement key:** `[settlement.mina.key] key_file` (`settlement-mina.key`), a 32-byte Pallas
  scalar as raw bytes or 64 hex characters. On the node's own channels it is the payer, the
  `payerAuthorizer`, the receiver, the `receiverAuthorizer` and `payTo` (ADR 0075 decision 3).

The ADR amendments wait for #1548's single "Mina returns as a settlement chain" ADR.

---

## Verdict

**Six of the seven decisions stand, four of them with adjustments. One is contradicted: D4's
clock.**

- **D4 contradicted.** o1js's `network.timestamp` is not a protocol field. o1js turns it into a
  `globalSlotSinceGenesis` range using `genesisTimestamp + slot × slotTime`, with both constants
  fetched from whichever network is active and baked into the circuit. On mainnet today that
  formula puts the current block **in July 2029**: the daemon reports the Mesa fork's genesis time,
  but the slot count runs from the 2021 genesis (CONFIRMED-LIVE). The o1js issue reporting this,
  #1588, is open, and a maintainer wrote "this is still a bug that makes timestamp unusable". The
  precondition is also checked against the **parent** block, not the including one. The fallback
  works and keeps everything else: keep the delay **in seconds** in the channel config, convert it
  to slots inside the circuit at Mesa's fixed 90 s, and time everything with
  `this.currentSlot.requireBetween`.
- **D2 needs four precisions** before vectors can be cut:
  - The tag is hashed as Poseidon's **salt** (`hashWithPrefix`), not as a first input element. The
    two give different digests.
  - A salt is one field element, not 32 bytes.
  - The channel id wants its own tag.
  - The network id must not be Mina's `chainId`, which changes at every hard fork.
- **D7's internal spelling collides with two standing rules.** `AssetChain` is a closed enum whose
  test asserts that `mina:` is refused. ADR 0080's 2026-10-28 amendment says a `mina:` asset "in
  `[[tokens]]` or a settlement table is still refused by name".
- **R6 found a collision the list misses.** Every voucher on every carriage goes through
  `parse_client_claim`, and that function refuses `blockchain: "mina"` by name **before** it looks
  at `scheme`. It does this because the refusal is "wire behaviour owed to `toon-client`"
  (`CLAUDE.md`, ADR 0080 decision 1). As things stand, a Mina peer's voucher cannot reach a backend.

| #   | Decision (owner, 2026-10-28)                                                                                                                       | Verdict                                                                  |
| --- | -------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| D1  | EVM-shaped: authorizers, cumulative vouchers, claim then settle, two-step withdraw, signed refund                                                  | **Confirmed**, with three facts to carry into R1                         |
| D2  | Schnorr/Pallas over `Poseidon(tag, channelId, cumulative)`; channel id over config, zkApp and network; challenge tag; devnet domain; chunked nonce | **Needs adjustment** (four precisions)                                   |
| D3  | `u64` nanomina, refused above `u64::MAX`, one field                                                                                                | **Confirmed**                                                            |
| D4  | `network.timestamp.requireBetween`, delay in seconds, one-slot margins                                                                             | **Contradicted**; fallback specified                                     |
| D5  | x402 bounds 15 min–30 days; default one day; floor 900 s; `min_withdraw_delay_secs`                                                                | **Confirmed**, with a caution                                            |
| D6  | No facilitator; each party proves and pays                                                                                                         | **Confirmed for TOON**; the upstream spec still needs a facilitator role |
| D7  | Greeting `mina:mainnet`/`mina:devnet`, `slip44:12586`, `AssetId` `mina:native`, terms                                                              | **Needs adjustment** (two standing refusals, untagged terms)             |

---

## Part 1. The decisions, checked

### D1. Shape: the EVM binding's, without Solana's distribute

**Verdict: confirmed.** Every element D1 names is in `x402BatchSettlement` and the EVM spec.

- **Roles.** `ChannelConfig{payer, payerAuthorizer, receiver, receiverAuthorizer, token,
uint40 withdrawDelay, bytes32 salt}`
  ([X402 `x402BatchSettlement.sol` L36–44](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/contracts/evm/src/x402BatchSettlement.sol#L36-L44)).
  `receiverAuthorizer` "authorizes claim and refund operations via EIP-712 signatures … Must not be
  zero" ([X402 `scheme_batch_settlement_evm.md` L57–61](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/schemes/batch-settlement/scheme_batch_settlement_evm.md#L57-L61)).
  CONFIRMED-CODE, DOCS.
- **Cumulative vouchers.** `Voucher(bytes32 channelId,uint128 maxClaimableAmount)` (sol L96). "The
  cumulative model makes nonces unnecessary" (spec L834). CONFIRMED-CODE, DOCS.
- **Claim records; the channel survives.** `_processVoucherClaim` raises `ch.totalClaimed` and the
  receiver's `totalClaimed`, and moves no token. A row at or below the recorded total is a silent
  `return` (sol L516–546). CONFIRMED-CODE.
- **Settle pays the difference.** `settle(receiver, token)` transfers `rs.totalClaimed −
rs.totalSettled`, permissionless (sol L297–307). CONFIRMED-CODE.
- **Two-step withdraw.** `initiateWithdraw(config, amount)` is payer or `payerAuthorizer` only. It
  reverts if a withdrawal is already pending or `amount > balance − totalClaimed`, and records
  `initiatedAt = block.timestamp`. `finalizeWithdraw` runs after `withdrawDelay` and caps the
  transfer at the unclaimed escrow then left (sol L324–386). CONFIRMED-CODE.
- **Refund.** There are two paths. `refund` is called directly by `receiver` or
  `receiverAuthorizer`. `refundWithSignature` is called by anyone with `receiverAuthorizer`'s
  `Refund(channelId, nonce, amount)` signature. Both bump `refundNonce` first (sol L399–427,
  L554–586). CONFIRMED-CODE.
- **No distribute.** The bps split is `payment-channels`' own (MINA-NOTE §2.2). The EVM shape has
  nothing like it.

Three facts D1 does not state carry into R1:

1. **`settle` aggregates per `(receiver, token)`, across channels.** `receivers[receiver][token]`
   is one accumulator that every claim to that receiver writes (sol L122, L542). On EVM that is
   cheap storage. On Mina, one state that every claim writes serialises every claim through state
   preconditions (MINA-NOTE §3.3).
2. **A claim carries its own `totalClaimed`, at most the voucher's `maxClaimableAmount`** (sol
   L71–77, L522–526). x402 treats the voucher as a **ceiling**, and the receiver claims the charge
   it actually made ("Over-claiming is a trust violation, not a protocol violation", spec L834).
   That is why `claim` is restricted to the receiver side (sol L247–259). TOON's port lands exactly
   the voucher's amount ([`port.rs` L188–196](../../crates/connector-settlement/src/batch/port.rs)).
3. **The connector uses only `claim` and `settle`.** It never calls `claimWithSignature`,
   `refund` or `refundWithSignature`
   ([`batch_settlement.rs` L541](../../crates/connector-settlement-evm/src/batch_settlement.rs),
   [`batch_watch.rs` L264, L289](../../crates/connector-settlement-evm/src/batch_watch.rs)). The
   port has no refund operation. CONFIRMED-CODE.

### D2. The voucher, the channel id and the challenge

**Verdict: needs adjustment.** The construction is sound and Rust can reproduce it, but four details
must be pinned before vectors exist.

#### What is confirmed

- **A tag is one field element.** o1js `prefixToField` fills a string's bytes into one field,
  zero-padded to 32 bytes, and refuses one of 32 bytes or more
  ([O1JS `bindings/lib/binable.ts` L294–299](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/bindings/lib/binable.ts#L294-L299)).
  CONFIRMED-CODE.
- **The 20-byte rule is an interop rule, and for Rust a hard limit.** Mina's OCaml
  `Hash_prefixes.create` pads every prefix to `length_in_bytes = 20` with `*`
  ([MINA `hash_prefixes.ml` L1–22](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/hash_prefixes/hash_prefixes.ml#L1-L22)).
  Rust `mina-hasher` asserts `prefix.len() <= 20` and pads with `*`
  ([PS `hasher/src/lib.rs` L23, L179–187](https://github.com/o1-labs/proof-systems/blob/9227444890/hasher/src/lib.rs#L179-L187)).
  o1js pads a short prefix with **zero bytes**. So a tag shorter than 20 bytes hashes differently
  in o1js and in Rust, which SPIKE also found ("Convention 1"). CONFIRMED-CODE.
  - `x402-mina-voucher***` is 20 bytes, and so is `x402-mina-challenge*`. Counted.
- **Prefix-in-hash is how both sides use a tag.** o1js `hashWithPrefix(prefix, input)` first
  absorbs `[prefixToField(prefix)]` as a salt, then absorbs `input`
  ([O1JS `hash-generic.ts` L13–25](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/provable/crypto/hash-generic.ts#L13-L25)).
  Rust `Hasher::init(domain)` absorbs `domain_prefix_to_field` the same way
  ([PS `hasher/src/poseidon.rs` L86–95](https://github.com/o1-labs/proof-systems/blob/9227444890/hasher/src/poseidon.rs#L86-L95)).
  CONFIRMED-CODE.
- **A public key is two field elements.** `PublicKey` is `@prop x: Field; @prop isOdd: Bool`
  ([O1JS `signature.ts` L131–135](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/provable/crypto/signature.ts#L131-L135)).
  A Mina address (`B62…`) is the base58check of that same `{x, isOdd}`
  ([O1JS `mina-signer/src/curve-bigint.ts` L21](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/mina-signer/src/curve-bigint.ts#L21)).
  Its fields are `[x, isOdd ? 1 : 0]`. CONFIRMED-CODE.
- **The signature domain is always devnet's.** In-circuit `Signature.create` and `Signature.verify`
  both use `signaturePrefix('devnet')` = `CodaSignature*******`, and derive the nonce with
  `'devnet'` ([O1JS `signature.ts` L254–300](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/provable/crypto/signature.ts#L254-L300)).
  So does mina-signer `signFields` ([O1JS `mina-signer.ts` L152–176](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/mina-signer/mina-signer.ts#L152-L176)).
  The mainnet prefix `MinaSignatureMainnet` is never used by a zkApp's `Signature.verify`
  ([O1JS `bindings/crypto/constants.ts` L24–25](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/bindings/crypto/constants.ts#L24-L25)).
  CONFIRMED-CODE.
- **The nonce is deterministic and chunked.** `deriveNonce` appends `[x, y, d]` and the 8-bit
  network id (`0x00` for devnet) to the message, packs it with `packToFields` (the "chunked"
  style), and takes blake2b-256 with the top two bits dropped
  ([O1JS `mina-signer/src/signature.ts` L35–36, L128–150](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/mina-signer/src/signature.ts#L128-L150)).
  Because it is deterministic, vectors can pin `(r, s)`. SPIKE reproduced it byte for byte in Rust
  under `NonceMode::Chunked`. CONFIRMED-CODE, CONFIRMED (SPIKE, RAN).
- **No nonce and no expiry** is exactly the EVM rule (spec L834, L840). CONFIRMED-DOCS.

#### What needs pinning

1. **Salt the hash with the tag; do not hash the tag as an input.**
   - `hashWithPrefix(tag, [channelId, cumulative])` absorbs `[tag]` and permutes, then absorbs
     `[channelId, cumulative]`.
   - `Poseidon.hash([tag, channelId, cumulative])` absorbs `[tag, channelId]`, permutes, then
     absorbs `[cumulative, 0]`. The sponge zero-pads to its rate of 2
     ([O1JS `bindings/crypto/poseidon.ts` L80–97](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/bindings/crypto/poseidon.ts#L80-L97)).

   They are different digests. "Poseidon(tag, …)" in D2 must mean the first, which is Mina's own
   idiom and Rust `Hasher::init`'s native shape. CONFIRMED-CODE for the mechanics; the
   recommendation is **Inference**.

2. **The salt is a field element, not 32 bytes.** EVM's `salt` is `bytes32`. A Pallas field is
   about 2²⁵⁴, so an arbitrary 32-byte salt does not always fit. The config should carry
   `salt: Field` (or 31 bytes). **Inference** from the field size.
3. **Give the channel id its own tag**, for example `x402-mina-channel***` (20 bytes). Then a
   channel-id preimage can never hash like a voucher or challenge preimage of the same length. The
   preimage D2 lists is 14 field elements:
   - `payer`, `payerAuthorizer`, `receiver` and `receiverAuthorizer`, 2 each;
   - `tokenId`, 1 (native MINA is `TokenId.default = Field(1)`,
     [O1JS `account-update.ts` L636–641](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/account-update.ts#L636-L641));
   - `withdrawDelay`, 1;
   - `salt`, 1;
   - the zkApp address, 2;
   - the network id, 1.

   CONFIRMED-CODE for the field counts; the tag is **Inference**.

4. **The network id must be stable across hard forks. Do not use `chainId`.**
   - Mainnet GraphQL reports `chainId` `0718f61a…3886` today (CONFIRMED-LIVE). That is a hash that
     includes the genesis state, so it changed at Mesa. This is **Inference** from the field's
     definition; this note did not trace its derivation in source.
   - A channel bound to it would stop verifying across the next fork.
   - A small constant does not have this problem: `0` devnet and `1` mainnet, the values Mina's
     own nonce derivation uses (mina-signer `networkIdDevnet`/`networkIdMainnet`, L35–36), or the
     CAIP-2 reference string through `prefixToField`.
   - The zkApp has to learn its own network. Either the id is a circuit constant (one VK per
     network), or it is set once at deploy in an app-state field that no method rewrites.
     **Inference.**

#### Two further notes for the spec text

- **Hash the amount as a raw field, not packed.** `UInt64` is one `Field` (`@prop value`,
  [O1JS `int.ts` L30–31](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/provable/int.ts#L30-L31)),
  but its `toInput` packs 64 bits, which `hashPacked` would merge with neighbours. The voucher must
  hash `UInt64.toFields`, as D3 says. CONFIRMED-CODE; the rule is **Inference**.
- **The voucher signature shares devnet transaction signing's domain.** A voucher is
  `CodaSignature*******` Schnorr over a one-field message, signed by the same settlement key that
  signs the node's Mina transactions (ADR 0075 decision 3). On devnet, a devnet transaction
  signature is under the same prefix. What keeps them apart is that the message is a tagged
  Poseidon digest, and finding a transaction whose commitment equals it is a preimage problem.
  **Inference.** The commitment format of a zkApp signature was not traced in OCaml.

### D3. Amount: `u64` nanomina, one field

**Verdict: confirmed.**

- `UInt64` is one field element and "Cannot overflow" past `2^64 − 1` (O1JS `int.ts` L30–40).
  CONFIRMED-CODE.
- MINA balances are 64-bit nanomina (TOKENS §1, MINA-NOTE §1.1). The live
  `accountCreationFee` is `"1000000000"`, one MINA in nanomina (CONFIRMED-LIVE).
- The port's amounts are `u128` ([`port.rs` L188–196](../../crates/connector-settlement/src/batch/port.rs)).
  A Mina backend refuses above `u64::MAX` exactly as Solana does; schema 8 states the Solana rule
  ([`connector-vectors/src/lib.rs` L161–180](../../crates/connector-vectors/src/lib.rs)).
  CONFIRMED-CODE.

### D4. Time: `network.timestamp` with margins

**Verdict: contradicted.** `network.timestamp` cannot be used for this, on four independent grounds.

1. **It is not a protocol field.** o1js builds `network.timestamp` out of
   `network.globalSlotSinceGenesis`. `requireBetween(lower, upper)` becomes a slot range through
   `timestampToGlobalSlotRange`, which computes `(ts − genesisTimestamp) / slotTime`
   ([O1JS `precondition.ts` L156–191, L414–444](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/precondition.ts#L156-L191)).
   CONFIRMED-CODE.
2. **Its constants are the active network's, baked into the circuit.**
   - `genesisTimestamp` and `slotTime` come from `Mina.getNetworkConstants()`. Before any network
     is set, that defaults to genesis `0` and **180 s**
     ([O1JS `mina-instance.ts` L37–38](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/mina-instance.ts#L37-L38)).
     Once a network is set, they come from the daemon's `genesisConstants.genesisTimestamp` and
     `consensusConfiguration.slotDuration`
     ([O1JS `fetch.ts` L1043–1066](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/fetch.ts#L1043-L1066),
     [`mina.ts` L635–641](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/mina.ts#L635-L641)).
   - Issue #1588 (open, last touched 2024-10-14) is titled "Hardcode the genesis timestamp because
     it is used as a constant in constraints". The maintainer (mitschabaude) says the VK depends on
     which network was active at compile time, and that "this is still a bug that makes timestamp
     unusable". A reporter adds that "the Timestamps I see on devnet are almost two years ahead …
     calculated from the devnet hardfork timestamp, not from the devnet genesis timestamp".
     CONFIRMED-DOCS.
3. **On mainnet today it is wrong by about 2.7 years.** At 2026-10-28T14:26Z, mainnet's GraphQL
   reported:

   | Field                                 | Value                                |
   | ------------------------------------- | ------------------------------------ |
   | `genesisConstants.genesisTimestamp`   | `2026-09-03T18:00:00Z` (Mesa's fork) |
   | `consensusConfiguration.slotDuration` | `90000`                              |
   | best block `slotSinceGenesis`         | `1,011,096`                          |
   | best block epoch / slot in epoch      | `7` / `2676`                         |
   | best block `utcDate`                  | `2026-10-28T14:24:00Z`               |

   CONFIRMED-LIVE.
   - o1js's formula gives `genesis + 1,011,096 × 90 s` = **2029-07-22T23:24Z**.
   - The block's real time is `genesis + 52,656 × 90 s`, where `52,656 = 7 × 7140 + 2676` is the
     slot count **since the fork**. The two match exactly, so the fork sits at global slot
     **958,440**.
   - The arithmetic is **Inference** from the live figures.
   - `slotSinceGenesis` spans eras of 180 s slots and of 90 s slots. No single
     `genesis + slot × slotTime` is right for both.

4. **It checks the parent block, not the including one.** A `network.*` precondition is checked
   against `global_state.protocol_state`, and the producer passes the previous block's state view
   ([MINA `mina_transaction_logic.ml` L1583–1590](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/mina_transaction_logic.ml#L1583-L1590);
   [`block_producer.ml` L202–241](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/block_producer/block_producer.ml#L202-L241),
   `~current_state_view:previous_state_view`). `validWhile` is checked against
   `global_state.block_global_slot`, the slot of the block that includes the transaction, as a
   `Global_slot_since_genesis.t`
   ([MINA `zkapp_command_logic.ml` L969, L1324–1342](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/zkapp_command_logic.ml#L1324-L1342)).
   CONFIRMED-CODE.

**Issue #2043** (open) is a test-harness defect only: `LocalBlockchain`'s `currentSlot()` follows
`Date.now()` while its network state's slot only moves when set by hand. It still hard-codes 180 s
slots ([O1JS `local-blockchain.ts` L72](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/local-blockchain.ts#L72)).
It bears on tests, not on mainnet. CONFIRMED-DOCS, CODE.

#### The fallback, and what it requires

**Recommended, pending owner.** It keeps D4's intent (seconds, never an exact slot) and D5
unchanged.

- **The config carries `withdrawDelay` in seconds**, as x402 and the greeting do. The circuit turns
  it into slots once: `delaySlots = ceil(withdrawDelay / 90)`. The 90 is a **constant of the
  VK**, not a fetched network constant, so #1588's failure cannot occur.
  - 15 minutes is 10 slots; 30 days is 28,800 slots; one day is 960 slots.
  - Keeping seconds in the config, rather than slots, is also what lets the contract suite run
    unchanged (R3, item 2).
- **Every clock check uses `this.currentSlot.requireBetween`** (`validWhile`), which is checked on
  the including block ([O1JS `precondition.ts` L229–240](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/precondition.ts#L229-L240)).
  Every stored time is a global slot since genesis.
- **Only one bound is load-bearing, and the slack in it lands on the payer.** D4's "one-slot
  margin in each bound" can be dropped:
  - `initiateWithdraw` takes a witness window `[lo, hi]` with `currentSlot.requireBetween(lo, hi)`
    and records `initiatedAt = hi`. The real inclusion slot is at most `hi`, so the receiver always
    gets at least the full delay. The window's width only changes how likely the payer's
    transaction is to land, and how long the payer then waits. A wide window (say 10 slots) is
    therefore safe.
  - `finalizeWithdraw` requires `currentSlot.requireBetween(initiatedAt + delaySlots, UInt32.MAX)`.
    No upper bound, no margin.
  - **Inference** from the precondition semantics above. This is the reverse of the old zkApp's
    `globalSlot` mistake (MINA-NOTE §5), which pinned an exact slot.
- **What it costs.** A hard fork that changes slot time, as Mesa did (180 → 90 s), changes the
  real length of a delay counted in slots. Mesa also changed every VK (MINA-NOTE §3.4), so a
  redeploy happens at such a fork anyway, and the constant changes with it. This ties into the
  open VK-upgrade question (MINA-NOTE D7). **Inference.**
- **The alternative is a TOON-written timestamp.** It would be `network.globalSlotSinceGenesis`
  bounds computed from the anchor (slot 958,440 ↔ 2026-09-03T18:00Z, 90 s) as circuit constants.
  It carries the same fork-pinning, plus the parent-block semantics of item 4. Not recommended.
  **Inference.**
- **Devnet.** Mina published `4.0.0-devnet-mesa` on 2026-08-19 (release list), so devnet is
  presumably on 90 s slots too. No synced devnet endpoint answered this note's reads, so devnet's
  live slot time is **UNCONFIRMED**. A 90 s constant on a 180 s network would halve every delay.

### D5. Delay bounds and config

**Verdict: confirmed, with a caution.**

- x402's bounds are `MIN_WITHDRAW_DELAY = 15 minutes`, `MAX_WITHDRAW_DELAY = 30 days`, checked at
  deposit (sol L85–86, L210–212). The SVM spec states the same range in seconds, 900 through
  2,592,000
  ([X402 `scheme_batch_settlement_svm.md` L247](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/schemes/batch-settlement/scheme_batch_settlement_svm.md#L247)).
  CONFIRMED-CODE, DOCS.
- The connector's floor is `BATCH_SETTLEMENT_DELAY_FLOOR_SECS = 900`, its default
  `DEFAULT_BATCH_SETTLEMENT_MIN_DELAY_SECS = 86_400`, and its EVM cap is thirty days
  ([`connector-config/src/batch_settlement.rs` L30–44](../../crates/connector-config/src/batch_settlement.rs)).
  `min_withdraw_delay_secs` is already the EVM key's name (L55). CONFIRMED-CODE.
- **Caution.** 900 s is ten 90 s slots. That is less than Mina's recommended 15-block confirmation
  (~30 min), and the network carries 12 zkApp commands per block (MINA-NOTE §3.4–3.5). A
  node that publishes the floor is betting that one proved claim lands inside ten slots. The
  one-day default is ~960 slots. **Inference.**
- The SVM spec adds a rule worth copying into the Mina spec: `withdrawDelay` "MUST be `>=
maxTimeoutSeconds`" (svm L247). CONFIRMED-DOCS.

### D6. No facilitator; each party proves and pays

**Verdict: confirmed for TOON; needs adjustment for the upstream spec.**

- **Nothing on Mina needs a third party.**
  - Any account can be a zkApp command's fee payer, and the fee payer need not be a party to the
    channel (MINA-NOTE §3.5).
  - A proof is anyone's to make, because the chain checks it against the VK, not against whoever
    submitted it.
  - On a native-MINA channel the payer already holds the fee asset.
  - ADR 0076's working-tree note (2026-10-01, #1443) makes "a node operator holds gas" a
    requirement: "A node sends its own `claim`, `settle` and withdrawal as well, so a relayed
    deposit would not spare it the gas."

  CONFIRMED-CODE, DOCS.

- **"Each party proves" means each party's prover, outside the connector.** ADR 0080's falsifiers
  refuse `o1js` in any `Cargo.toml` or `package.json` here. The prover is the out-of-process
  builder zk_toon plans (#14) or TOON's own Rust prover if spike #1561 succeeds (#1548). CONFIRMED-DOCS.
- **The upstream spec cannot leave the facilitator out.**
  - x402's chain-family process requires a spec with "payload structure, verification logic and
    settlement logic", followed by a reference implementation that provides
    `SchemeNetworkClient`, `SchemeNetworkServer` **and** `SchemeNetworkFacilitator` (TS), or the
    Go/Python equivalents
    ([X402 `CONTRIBUTING.md` L227–250](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/CONTRIBUTING.md#L227-L250)).
  - The EVM binding defines `/verify` and `/settle` facilitator behaviour (spec L390–630).
  - The spec should say that a facilitator **may** prove and submit any transaction a party has
    authorized. A server that runs no facilitator, as TOON does, does both itself.

  CONFIRMED-DOCS; the wording is **Inference**.

### D7. The greeting entry

**Verdict: needs adjustment.** The external identifiers are right, but two standing refusals in this
repository and one serde detail stand in the way.

#### Confirmed

- **CAIP-2.** `mina:mainnet` and `mina:devnet` are the namespace's listed ids
  ([ChainAgnostic `mina/caip2.md` L26–49, L96–103](https://github.com/ChainAgnostic/namespaces/blob/5c515816a2828914bb77a3ed92f9445d2e29f785/mina/caip2.md)).
  The namespace has no CAIP-19 file (only `README.md` and `caip2.md`). CONFIRMED-DOCS.
- **SLIP-44 12586** is "MINA | Mina" (SLIP-44 L1358). `slip44:12586` as the asset is TOON's own
  composition, which ADR 0080's amended decision 7 already states. CONFIRMED-DOCS.
- **`receiverAuthorizer` must equal this node.** That is the EVM precedent: `receiver_authorizer`
  is "Always the same address as `pay_to`: decision 5 never delegates it, so a facilitator can never
  refund a voucher this connector has not yet claimed"
  ([`connector-domain/src/x402.rs` L293–297](../../crates/connector-domain/src/x402.rs)).
  CONFIRMED-CODE.
- **`withdrawDelay` in seconds** matches x402's `extra.withdrawDelay` on both chains (evm L119, svm
  L247). CONFIRMED-DOCS.

#### Adjustments

1. **`mina:native` is refused twice today.**
   - `AssetChain` is a closed two-variant enum (`Evm`, `Solana`), and `from_str` refuses any other
     chain ([`connector-domain/src/asset.rs` L77–108](../../crates/connector-domain/src/asset.rs)).
     The test `an_unknown_chain_is_refused_by_name` asserts exactly that `"mina:B62qFoo"` is
     `UnknownChain("mina")` (L314–319). CONFIRMED-CODE.
   - ADR 0080 decision 7 (working tree): "a `mina:` asset in `[[tokens]]` or a settlement table is
     still refused by name". A `[settlement.mina]` table contradicts that sentence. CONFIRMED-DOCS.
   - Both must change, and ADR 0080 decision 7 is amended again.
2. **The terms enum is `#[serde(untagged)]` and disambiguates structurally.** EVM terms require
   `receiverAuthorizer`, `name` and `version`; Solana terms never carry `receiverAuthorizer`
   (`x402.rs` L263–272, L469). A third, Mina variant would parse only because the others fail
   first. Dispatch on `network`'s namespace instead (`eip155:`, `solana:`, `mina:`). CONFIRMED-CODE;
   the fix is **Inference**.
3. **x402's own `asset` is "Token contract address or ISO 4217 currency code"**
   ([X402 `x402-specification-v2.md` L127](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/x402-specification-v2.md#L127)).
   Native MINA has no contract. `slip44:12586` is defensible, but the upstream spec might instead
   choose the base58 token id, or the field `1`. The spec has to choose one. CONFIRMED-DOCS.
4. **`zkappAddress` and `verificationKeyHash` are published, not trusted.**
   - ADR 0075 decision 1 makes the contract or program a constant of the binary. The SVM spec says
     a facilitator "MUST NOT trust or negotiate a `channelProgram` value" (PENDING §1).
   - So a receiver publishes its two constants, and a payer refuses a peer whose published pair is
     not the one its own binary holds.

   **Inference.**

---

## Part 2. The next round (recommended, pending owner)

### R1. Who lands a voucher, and should claim and settle be one method?

**Recommendation.** One zkApp method, `claim`, that records **and** pays the delta in the same
transaction. It is authorized by a `receiverAuthorizer` Schnorr signature verified in-circuit, so
anyone may prove and submit it. There is no separate `settle`, and no per-receiver accumulator.

**Why the EVM split exists, and why it does not carry over.**

- The EVM spec: "A single claim transaction can cover many channels at once and only updates
  onchain accounting; claimed funds are later transferred to the receiver via a separate settle
  operation that sweeps many claims into one token transfer" (spec L5). The split is gas
  economics over one contract's storage. CONFIRMED-DOCS.
- On Mina, `settle` needs an aggregate of `totalClaimed − totalSettled` per receiver (sol L122).
  Wherever it lives, every claim writes it:
  - If it is shared state, every claim to that receiver writes one value, and state preconditions
    let only one writer per value succeed (MINA-NOTE §3.3).
  - If it is kept per channel, `settle` is per channel and the split saves nothing.

  **Inference.**

- **Fused, a landing is one proof and one transaction; split, it is two of each.** Each proof is
  tens of seconds (MINA-NOTE §6), and each transaction competes for 12 zkApp commands per mainnet
  block (MINA-NOTE §3.4). **Inference.**
- The port already wants one transaction per landing: `land` "sends exactly one transaction"
  (MINA-NOTE §1.1). On EVM the connector's `land` is `claim` alone and the sweep settles later
  (`batch_settlement.rs` L541, `batch_watch.rs` L289). A fused Mina `land` pays out at once, and
  the Mina sweep has nothing to settle. CONFIRMED-CODE; the consequence is **Inference**.

**Account updates for a fused claim.** For a native-MINA channel whose escrow is the zkApp's (or
the channel account's) balance:

| Account update     | Authorization          | Effect                             |
| ------------------ | ---------------------- | ---------------------------------- |
| fee payer (anyone) | signature              | pays the fee, increments its nonce |
| channel zkApp      | **proof**              | raises `totalClaimed`; balance −Δ  |
| receiver           | none (`receive: none`) | balance +Δ                         |

- That is one proof update plus a pair of signed or no-auth updates: `np = 1, n2 = 1`, cost 2
  against MIP-9's `np + n2 + n1 ≤ 16`
  ([MIP-9 L41–42, L91](https://github.com/MinaProtocol/MIPs/blob/5aa77c8911e7af39ff9c594175226b171ed8166d/MIPS/mip-0009-increase-zkapp-account-update-limit.md)).
  The split costs `np = 1` plus a fee payer for `claim`, then the same three updates again for
  `settle`. CONFIRMED-DOCS for the rule, **Inference** for the counts.
- `receive: none` on an ordinary account is `Permissions.initial()` (MINA-NOTE §3.1).
  CONFIRMED-CODE.
- The receiver's account must already exist. Otherwise the 1 MINA creation fee falls on the
  payout (see R6).

**Why keep receiver authorization, and why by signature.**

- x402's vouchers are ceilings. A third party landing a voucher at its full `maxClaimableAmount`
  would overcharge a payer whose receiver charged less (D1, fact 2). TOON lands exact amounts, but
  the binding is meant to go upstream. CONFIRMED-DOCS; **Inference** for the consequence.
- Mina has no `msg.sender`. The two ways to bind the receiver are:
  - **(a)** `receiverAuthorizer`'s signature over `hashWithPrefix("x402-mina-claim*****",
[channelId, totalClaimed])`, verified in-circuit. This is the analogue of `claimWithSignature`
    (sol L270–292). It is relay-friendly: any prover can submit it, which matters when proving
    happens in a service outside the connector.
  - **(b)** A signed child account update from `receiverAuthorizer` in the same transaction, the
    pattern the old zkApp used to bind a depositor (`AccountUpdate.createSigned`, MINA-NOTE §5).
    It must pass the replay check (nonce increment or full commitment, MINA-NOTE §3.1), and the
    signer must sign the finished transaction. It is the analogue of `claim`.

  Recommend **(a) only**: one code path and one circuit. It covers both EVM paths, because the
  receiver submitting its own signed claim is just the relay path with itself as relay.
  **Inference.**

- Replay is harmless. A used claim signature carries a `totalClaimed` no longer above the
  channel's, so it is a no-op as on EVM (sol L522). It is still a bearer authorization in the
  window before it lands, but it pays only the receiver. **Inference.**
- **Batching across channels** is possible but not needed. Up to 15 proof updates fit in one
  command (MIP-9 L63), so a sweep could fuse several channels' claims into one transaction. Each
  channel is still its own proof update. CONFIRMED-DOCS; **Inference**.

### R2. Refund: keep the signed `Refund(channelId, nonce, amount)`?

**Recommendation.** Keep it, nonce included, in the zkApp and the spec, for x402 parity. Do not
build it into the connector now: the port has no refund and the connector never refunds on EVM
(D1, fact 3).

- **The nonce exists because a refund amount is a delta.** `_executeRefund` takes `amount`, caps
  it at `balance − totalClaimed`, and moves it. A refund signature without a nonce could be
  replayed until the escrow ran dry. `refundNonce` is incremented before the cap, even when nothing
  moves (sol L113–120, L554–586). Mina has the same problem, because it is about the message, not
  the chain. CONFIRMED-CODE; **Inference** for Mina.
- **Alternatives on Mina, and why not:**
  - A cumulative `totalRefunded` (monotone, like a voucher) also needs one state field. It changes
    x402's refund semantics: a refund would no longer reduce a pending withdrawal by its own
    amount (sol L571–580). Not worth the divergence.
  - Binding a refund to the channel's current state (a state precondition on `balance`) is not
    safe either. A deposit can return the balance to an earlier value (ABA), and then the old
    signature applies again.

  **Inference.**

- **The tag** would be `x402-mina-refund****` (20 bytes). On EVM, cross-function replay is
  prevented by "distinct EIP-712 type hashes" (spec L838); on Mina, distinct tags do the same job.
  CONFIRMED-DOCS for EVM.
- The direct `refund` path by receiver-side signed account update can be dropped for the same
  reason as R1's (b).

### R3. Parity with the contract suites (shared with #1565)

**Recommendation.** **Option (b).** Mina passes `assert_upholds_the_contract` and
`assert_upholds_the_paying_contract` **without changes to what they assert**. Exhaustive matches on
the chain enums gain a Mina arm, which is a mechanical change, not a semantic one. Mina-only
behaviour gets tests of its own. Reaching that needs three choices, in the zkApp or in the
fixture; each is marked below.

Read from [`batch/contract.rs`](../../crates/connector-settlement/src/batch/contract.rs):

| #   | What the suite does                                                                                                                                                        | On Mina                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| --- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 1   | `in_another_token`, `for_the_other_chain` and `record_for_the_other_chain` `match` on `ReceiverTerms` / `OutboundChannelRecord` with exactly two arms (L419–450, L948–973) | **A Mina arm in each, or it does not compile.** "The other chain" stops being unique: any non-Mina value serves for Mina, and EVM's and Solana's arms are unchanged. Mechanical.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| 2   | `minimum_delay_secs > 901`, then opens a channel at `minimum − 1` seconds and expects `DelayBelowMinimum { delay_secs: minimum − 1, … }` (L108–111, L152–163)              | **Passes only if the config keeps seconds.** If the chain stored slots, a channel one second short of the minimum could not exist exactly, and the reported `delay_secs` would be a multiple of 90. This is the second reason for D4's fallback (seconds in the config, `ceil(·/90)` in-circuit). **Choice: the zkApp.**                                                                                                                                                                                                                                                                                                                                                                   |
| 3   | Opens a channel `in_settled_token: false` and expects `TokenNotSettled` (L142–150)                                                                                         | **Breaks if the zkApp accepts only token id 1**, because the fixture could not open a foreign-token channel on chain, and EVM's order (recompute the id, then read the chain) reports `ChannelNotFound` first. Either the Mina presentation (like EVM's, which carries a config) is judged on its config fields **before** the chain is read, or the zkApp records any token id and pays only in MINA. Recommend the former. **Choice: Mina's admission order.**                                                                                                                                                                                                                           |
| 4   | `payer_balance()` exact deltas: `funded − 1_000`, `funded − 2_500`, "1,500 less 400" (L504–524, L555–561, L696–700)                                                        | **Breaks if fees come out of the same balance.** On EVM gas is ETH and the token USDC; on Solana fees are SOL. On native MINA the settlement key pays fees and possibly a 1 MINA creation fee from the very balance the suite measures. The fixture must report the deposit asset net of fees, for example by giving the payer node a separate fee-payer account in tests, or by adding back the fees it recorded. **Choice: the fixture**, which the suite already parameterises.                                                                                                                                                                                                         |
| 5   | `let_delay_pass()` must let the minimum delay pass (L399–404); the suite then expects `finish_withdrawal` to succeed (L688–700)                                            | **No clock warp is known.** The daemon reads `MINA_TIME_OFFSET` once at start and caches it ([MINA `block_time.ml` L48–97](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/block_time/block_time.ml#L48-L97)). Whether restarting a lightnet with an offset would jump its chain forward is **UNCONFIRMED**. Lightnet's slot is **20 s** ([`profiled/lightnet.ml` L43](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/node_config/profiled/lightnet.ml#L43)). With a fixed 90 s constant, the fixture's 902 s minimum is `ceil(902/90) = 11` slots, about **220 s** of real waiting. That is slow but finite. A lightnet-built VK using 20 s would need about 15 minutes. |
| 6   | Exit status `Withdrawing \| Closing`, collateral 0 after an exit, the held voucher still lands (L340–369, L631–686)                                                        | **Passes.** EVM-shaped: the exit is `Withdrawing`, and a fused claim still lands while withdrawing.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| 7   | Solana's `min_deposit <= 1_000` guard (L476–481)                                                                                                                           | Inside `if let ReceiverTerms::Solana`. **No Mina arm needed.**                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| 8   | `sign_claim_state_challenge(&ch, 1_800_000_000)`, and two expiries give two signatures (L740–758)                                                                          | **Passes.** `expires` is one field, and the signature is deterministic per message.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| 9   | `prepare_open` sends nothing; `open_prepared` is idempotent across two crashes, with no second deposit (L765–839)                                                          | **Passes in principle, but the record must make it so.** On Mina the deposit is one zkApp command. Either the record journals a fully built and signed command (its fee-payer nonce makes a resend fail if it already landed, as Solana's blockhash does), or `open_prepared` reads the chain for the channel before it proves anything. Proving inside `open_prepared` takes tens of seconds. See R6.                                                                                                                                                                                                                                                                                     |
| 10  | Amounts in the suite are at most 1,501                                                                                                                                     | Far below `u64::MAX`. **Passes.**                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |

- **Mina-only tests** (option b's "plus"):
  - a voucher above `u64::MAX` is refused (as Solana's);
  - the delay is converted to slots with `ceil`;
  - `initiatedAt` is recorded at the window's upper bound;
  - a top-up landing between proving and inclusion invalidates a pending claim and forces a
    re-prove (R6);
  - the 1 MINA account-creation path.
- **What the chain gives the tests.** Lightnet runs with `proof_level = "none"`
  (`lightnet.ml` L23), so the suites cannot catch a VK mismatch. MINA-NOTE §6 and D11 already say a
  proofs-on check is needed beside them. CONFIRMED-CODE.

### R4. Wire vectors

**Recommendation.** Add only the peer-facing sections now, and bump `schema_version` to **9**.

| Section                              | Mina entry?  | Why                                                                                                                                                                                                                                     |
| ------------------------------------ | ------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `peer_carriage.voucher_mina`         | **Yes, now** | Peers settle on Mina (#1548), and the paying node renders this JSON on BTP and HTTP. It mirrors `voucher_solana`: channel id, signer, secret, `max_claimable_amount` as a decimal string, signed digest, signature, JSON and BTP bytes. |
| `voucher_claim_state_challenge.mina` | **Yes, now** | It proves the peer role on zero-value packets and reads the watermark on restore (ADR 0075 decisions 5–6). Include the valid, wrong-key and voucher-is-not-a-challenge cases, as Solana's set does.                                     |
| a channel-id derivation case         | **Yes, now** | A Mina channel id is computed off chain from the config, as EVM's is (`claim_voucher.evm` carries `channel_config` → `channel_id_hex`). It pins D2's 14-field preimage, its tag and the network id.                                     |
| `claim_voucher.mina`                 | Later        | It is the client-edge voucher. Whether `toon-client` pays over Mina channels is still fog in #1548. A `mina_above_u64_max` refusal case belongs here when it comes.                                                                     |
| `payout_voucher.mina`                | Later        | A connector paying a **client** (ADR 0075 decision 7). Client-only.                                                                                                                                                                     |
| `client_auth_channel_challenge.mina` | Later        | A client's BTP auth. Client-only.                                                                                                                                                                                                       |
| `peer_carriage.zero_value_challenge` | Optional     | It is one case today. A Mina variant only matters if its bytes differ in shape beyond the challenge entry, which is already covered above.                                                                                              |

- **Schema history sets the precedent.** Version 6 was "additive, a new section rather than a
  changed one — but the bump still matters", because an SDK otherwise has no signal that a new
  claim shape rides a header it already parses (`connector-vectors/src/lib.rs` L126–144).
  `SCHEMA_VERSION` is `8` today (L181). CONFIRMED-CODE.
- **Mind ADR 0071's falsifier.** It refuses any string in `vectors/wire-vectors.json` matching
  `"([a-z_]*(asset|denomination)[a-z_]*|token|token_(address|mint|id|decimals))"`
  ([ADR 0071 L24](../adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)).
  - A Mina config key literally named `"token_id"` would trip it.
  - EVM's `"token_hex"` does not. A name like `"token_id_field"` would not either.
  - ADR 0080's working tree reaffirms the rule for the zk note: "The asset stays out of the
    vectors."

  CONFIRMED-CODE, DOCS.

- **What the vectors must pin**, beyond values: the 20-byte tags, salt-not-input hashing, the
  chunked nonce under the devnet domain with `(r, s)` (o1js `Signature.toBase58` as well), and the
  channel id's text form in `ChannelId(String)` (decimal field, as o1js `Field.toString` prints it,
  is the least surprising). SPIKE's vectors already use the same conventions. **Inference.**
- **Prerequisite.** The voucher's JSON says `"blockchain": "mina"`, which today's parser refuses
  (R6, item 1).

### R5. The upstream PR (#1562)

**Recommendation.** Open a discussion or issue first, then a **spec-only** PR adding
`specs/schemes/batch-settlement/scheme_batch_settlement_mina.md`. It contains the sections below.
Treat it as a new chain family. Expect a re-audit, since the mechanism differs from the reference
contract.

What the x402 repository requires, read at `db2c98ec`:

- **Discussion first.** "Before writing a spec, open a GitHub issue or discussion to propose the
  idea", with the problem, the high-level approach, and "Why existing schemes/transports don't
  suffice"
  ([X402 `specs/CONTRIBUTING.md` L71–77](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/CONTRIBUTING.md#L71-L77)).
  CONFIRMED-DOCS.
- **Template.** A chain binding uses `scheme_impl_template.md`: Summary, Payment payload,
  Verification, Settlement, Appendix
  ([X402 `specs/scheme_impl_template.md`](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/scheme_impl_template.md)).
  It must have security sections on "Replay attack prevention", "Authorization scope" and
  "Settlement atomicity" (specs/CONTRIBUTING L188–194). CONFIRMED-DOCS.
- **New chain family, PR 1.** "Open a PR with specs for one payment scheme implementation … Must
  include: payload structure, verification logic and settlement logic." It is followed by PR 2, a
  reference implementation in **one** SDK with client, server and facilitator interfaces, plus
  unit, integration and E2E tests. "If the scheme mechanism varies from the reference
  implementation, the x402 Foundation will re-audit the scheme for that chain before accepting"
  (root CONTRIBUTING L211–264). CONFIRMED-DOCS.
- **The neutral outline's seven MUSTs**: commitment format, verification rules, storage and
  identifier, double-spend prevention, expiry, redemption, trust model
  ([X402 `scheme_batch_settlement.md` L59–67](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/schemes/batch-settlement/scheme_batch_settlement.md#L59-L67)).
  CONFIRMED-DOCS.
- **PR mechanics.** Commits must be signed. Docs-only changes skip the changelog fragment
  ([X402 `.github/pull_request_template.md`](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/.github/pull_request_template.md)).
  CONFIRMED-DOCS.
- **Precedent.** Mina has no `exact` binding either. `specs/schemes/exact/` lists 17 networks,
  none of them Mina, so this would be Mina's first x402 spec of any scheme. XRPL, Hedera, Bitcoin
  and Cardano batch-settlement PRs sit open and unmerged (PENDING §2–5). CONFIRMED-DOCS.

**What the spec has to state, mapped to this note:**

- **Payload.** The voucher, the deposit authorization (the payer's signed account update), the
  claim, refund and withdraw authorizations. Each gets its own 20-byte tag (D2, R1, R2, R6).
- **Verification.** The off-chain voucher check is Schnorr against `payerAuthorizer`. Then the
  ceiling, the balance and the cumulative rule.
- **Settlement.** Claim fused with payout (R1), refund (R2), and the withdrawal clock in slots
  (D4).
- **The facilitator's role.** Optional: anyone may prove and submit what a party signed (D6).
- **Time.** No `network.timestamp`, and why (D4).
- **Identity of the code.** The zkApp address and VK hash, and how a VK change at a hard fork is
  handled (MINA-NOTE D7). This is not yet decided.
- **Network ids and asset spelling** (D7).
- **Security.** The devnet-domain signature (D2), the precondition race (R6), and the liveness gap
  EVM documents (sol L240–246).

### R6. What the list misses

1. **The `mina` refusal blocks every Mina voucher, on both edges.**
   - `parse_client_claim` returns `ClientClaimError::Mina` for `blockchain: "mina"` **before**
     reading `scheme`, "whatever it declares"
     ([`connector-domain/src/client_claim.rs` L634–655](../../crates/connector-domain/src/client_claim.rs)).
   - The same parser serves the peer carriage (L22–24). `connector-peer-btp` maps the error to
     `UnsupportedChain("mina")` (`claim_json.rs` L141), and the client edge maps it to
     `ClaimIngestRejection::Mina` (`claim_gate.rs` L917).
   - `CLAUDE.md` calls the refusal "wire behaviour owed to `toon-client`". ADR 0080 decision 1
     builds on it ("The decoder checks `blockchain == "mina"` first and refuses it by name").
   - No vector pins it (no `mina` key or value in `wire-vectors.json`).

   CONFIRMED-CODE, DOCS.
   **Recommendation.** Narrow the refusal to claims that are not `scheme: "batch-settlement"`.
   Keep refusing Mina at the **client** edge until #1548's fog about `toon-client` clears. On the
   peer carriage, admit a Mina voucher only on a bound channel. This amends ADR 0080 decision 1 and
   the `CLAUDE.md` sentence, and belongs in the "Mina returns as a settlement chain" ADR.

2. **How the payer's deposit is authorized.**
   - A signature authorizes only its own signer's account update (MINA-NOTE §3.1). So a deposit is
     a payer-signed child update that sends MINA to the zkApp
     (`AccountUpdate.createSigned(payer)` plus `send`), inside the zkApp's proved `deposit`
     method. That is how the old zkApp bound its depositor (MINA-NOTE §5).
   - The signed update must pass the replay check: increment its nonce or use the full commitment
     (MINA-NOTE §3.1).
   - Anyone may then be fee payer and prover. That is the Mina form of EVM's "`deposit` is
     callable by anyone, authorized by the payer's signature inside the collector" (MINA-NOTE
     §2.2).

   CONFIRMED-CODE for the mechanics; the design is **Inference**.
   The cost is one proof update plus the payer's signed update plus a fee payer.

3. **Replay of withdraw authorizations.** `initiateWithdraw` cuts collateral to zero, and the
   receiver stops accepting vouchers.
   - If the payer's authorization were an in-circuit signature over `(channelId, amount)` with no
     nonce, anyone could replay an old one after a `finalizeWithdraw` and freeze the channel
     again.
   - EVM avoids this with `msg.sender` (sol L325–327).
   - Recommend a payer-signed child update for `initiateWithdraw` and `finalizeWithdraw`. The
     protocol's nonce then gives replay protection. The payer is online for its own exit anyway.

   **Inference.**

4. **Account-creation fee.** `accountCreationFee` is 1 MINA on mainnet (CONFIRMED-LIVE).
   - A payout to a receiver whose account does not exist pays the fee out of the payout, or fails.
     Require the receiver's account to exist (peers are funded nodes).
   - If each channel is its own account (MINA-NOTE D4 (a)/(b)), the opener pays 1 MINA per channel
     with no known way to reclaim it (MINA-NOTE: UNCONFIRMED). Who pays, and whether channel
     storage avoids per-channel accounts, is still undecided.

   **Inference.**

5. **Where channel state lives is still open** (MINA-NOTE D4), and three things here depend on it:
   - whether `channelId` needs the zkApp address (D2);
   - whether the 1 MINA fee is per channel (item 4);
   - how contended a fused claim is (R1).

   D2's preimage, with zkApp address and network id, implies **many channels in one zkApp**.

6. **A top-up can starve a claim.**
   - A proof made against `getAndRequireEquals` of a state field fails once another transaction
     changes that field (MINA-NOTE §3.3).
   - If `deposit` and `claim` both precondition on the same `balance` field, a payer could top up
     by one nanomina each block. Every claim the receiver proved would then fail until the
     withdrawal finalised.
   - Mitigation: escrow held as the account's own MINA balance, read with a **range** account
     precondition (`account.balance.requireBetween(needed, MAX)`), which a top-up does not
     invalidate. Or keep a field `claim` never reads with equality.

   **Inference** from confirmed mechanics. Not tested.

7. **A later custom token** keeps `tokenId` in the config (D2). Every transfer then needs the token
   contract's proof in the same command, and its admin can pause withdrawals (MINA-NOTE §3.6). A
   token channel is therefore a different zkApp and VK, not a parameter of the MINA one.
   CONFIRMED-CODE (MINA-NOTE); **Inference** for the consequence.
8. **The connector's internal spellings.**
   - `ChannelPresentation`, `ReceiverTerms`, `VoucherSigner` and `OutboundChannelRecord` each gain
     a `Mina` variant, and every `chain()` returns `"mina"` (`port.rs` L56, L142, L170, L205, L213,
     L293, L334).
   - `VoucherSigner::Mina` is `{x, isOdd}`, or the `B62` string.
   - `OutboundChannelRecord`'s versioned encoding gains a tag.

   CONFIRMED-CODE.

---

## Unconfirmed, and why

- **Devnet's live slot time.** The only devnet endpoint that answered was in `BOOTSTRAP` and
  reported pre-Mesa constants. `4.0.0-devnet-mesa` (2026-08-19) suggests 90 s, but that was not
  read live.
- **That `chainId` changes at every hard fork.** Inferred from its being a hash over the genesis
  state; its derivation was not traced in OCaml.
- **Whether a lightnet restarted with `MINA_TIME_OFFSET` jumps forward and keeps producing.** The
  variable exists and is read once at start; nobody tried it.
- **The commitment format a zkApp signature signs** (D2's domain-sharing note). Not traced in
  OCaml.
- **Whether a zkApp method can constrain its own fee payer.** This note avoided relying on it; R1
  uses a signature instead.
- **That the top-up starvation in R6 item 6 happens on a real chain.** It is reasoned from the
  precondition rules, not observed.

---

## Decision summary

| #   | Item                                                                  | Status                                                                                                                                                                                                                                               |
| --- | --------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| D1  | EVM shape, no distribute                                              | **Confirmed.** Carry three facts to R1: `settle` aggregates per receiver, claims carry their own `totalClaimed`, and the connector uses only `claim` and `settle`                                                                                    |
| D2  | Voucher, channel id, challenge, signing                               | **Adjust.** `hashWithPrefix` (salt, not input); 20-byte tags (Rust's hard limit); public key = 2 fields; `salt` is a field; a channel-id tag; a fork-stable network id, not `chainId`; amounts by `toFields`                                         |
| D3  | `u64` nanomina, one field                                             | **Confirmed**                                                                                                                                                                                                                                        |
| D4  | `network.timestamp` with margins                                      | **Contradicted.** o1js #1588 open, ~2.7-year error on mainnet today, parent-block check. Fallback: seconds in config, `ceil(·/90)` in-circuit, `currentSlot.requireBetween`, record `initiatedAt` at the window's upper bound                        |
| D5  | 15 min–30 days; default 1 day; floor 900 s; `min_withdraw_delay_secs` | **Confirmed.** Caution: 900 s is ten slots. Add the SVM rule `withdrawDelay ≥ maxTimeoutSeconds`                                                                                                                                                     |
| D6  | No facilitator                                                        | **Confirmed for TOON.** The upstream spec defines an optional facilitator (prove and submit), because PR 2 needs `SchemeNetworkFacilitator`                                                                                                          |
| D7  | Greeting entry                                                        | **Adjust.** `AssetChain` and its refusal test, and ADR 0080 decision 7's "settlement table" refusal; dispatch terms on `network`, not untagged; `zkappAddress`/`verificationKeyHash` checked against binary constants                                |
| R1  | Who lands; fuse claim and settle                                      | **Recommended, pending owner:** one fused `claim` that pays the delta, authorized by an in-circuit `receiverAuthorizer` signature, provable and submittable by anyone; 1 proof, 3 account updates                                                    |
| R2  | Refund                                                                | **Recommended, pending owner:** keep the signed `Refund(channelId, nonce, amount)` with its nonce (the amount is a delta); in the zkApp and the spec, not in the connector yet                                                                       |
| R3  | Suite parity                                                          | **Recommended, pending owner:** option (b), suites unchanged in what they assert, plus a Mina arm in three helpers; needs seconds in the config, config-first admission, and a fee-netted `payer_balance`; `let_delay_pass` waits ~11 lightnet slots |
| R4  | Vectors                                                               | **Recommended, pending owner:** `peer_carriage.voucher_mina`, `voucher_claim_state_challenge.mina` and a channel-id case now; client sections later; `schema_version` 9; no `"token_id"` key                                                         |
| R5  | Upstream PR                                                           | **Recommended, pending owner:** discussion first, then a spec-only `scheme_batch_settlement_mina.md` per `scheme_impl_template.md`, with the three security sections and the seven MUSTs; signed commits; expect a re-audit                          |
| R6  | Missed decisions                                                      | **Recommended, pending owner:** narrow the `blockchain: "mina"` refusal (blocks every Mina voucher today); payer-signed deposit and withdraw updates; receiver account must exist; settle channel storage; guard against top-up starvation           |

---

## Sources

- x402 at `db2c98ec`:
  - [`contracts/evm/src/x402BatchSettlement.sol`](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/contracts/evm/src/x402BatchSettlement.sol)
  - [`specs/schemes/batch-settlement/scheme_batch_settlement_evm.md`](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/schemes/batch-settlement/scheme_batch_settlement_evm.md)
  - [`scheme_batch_settlement_svm.md`](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/schemes/batch-settlement/scheme_batch_settlement_svm.md)
  - [`scheme_batch_settlement.md`](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/schemes/batch-settlement/scheme_batch_settlement.md)
  - [`x402-specification-v2.md`](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/x402-specification-v2.md)
  - [`specs/CONTRIBUTING.md`](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/CONTRIBUTING.md)
  - [`specs/scheme_impl_template.md`](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/scheme_impl_template.md)
  - [`CONTRIBUTING.md`](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/CONTRIBUTING.md)
  - [`.github/pull_request_template.md`](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/.github/pull_request_template.md)
- o1js at `a428f9b9`:
  - [`src/lib/mina/v1/precondition.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/precondition.ts)
  - [`mina-instance.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/mina-instance.ts)
  - [`mina.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/mina.ts)
  - [`fetch.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/fetch.ts)
  - [`local-blockchain.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/local-blockchain.ts)
  - [`account-update.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/account-update.ts)
  - [`src/lib/provable/crypto/signature.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/provable/crypto/signature.ts)
  - [`hash-generic.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/provable/crypto/hash-generic.ts)
  - [`poseidon.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/provable/crypto/poseidon.ts)
  - [`src/lib/provable/int.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/provable/int.ts)
  - [`src/bindings/lib/binable.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/bindings/lib/binable.ts)
  - [`src/bindings/crypto/poseidon.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/bindings/crypto/poseidon.ts)
  - [`constants.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/bindings/crypto/constants.ts)
  - [`src/mina-signer/src/signature.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/mina-signer/src/signature.ts)
  - [`curve-bigint.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/mina-signer/src/curve-bigint.ts)
  - [`mina-signer.ts`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/mina-signer/mina-signer.ts)
- o1js issues: [#1588](https://github.com/o1-labs/o1js/issues/1588) (open) and
  [#2043](https://github.com/o1-labs/o1js/issues/2043) (open), read 2026-10-28.
- Mina at `4.0.3`:
  - [`hash_prefixes.ml`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/hash_prefixes/hash_prefixes.ml)
  - [`zkapp_command_logic.ml`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/zkapp_command_logic.ml)
  - [`mina_transaction_logic.ml`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/transaction_logic/mina_transaction_logic.ml)
  - [`block_producer.ml`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/block_producer/block_producer.ml)
  - [`block_time.ml`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/block_time/block_time.ml)
  - [`profiled/lightnet.ml`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/node_config/profiled/lightnet.ml)
  - [`profiled/mainnet.ml`](https://github.com/MinaProtocol/mina/blob/4.0.3/src/lib/node_config/profiled/mainnet.ml)
  - the [release list](https://github.com/MinaProtocol/mina/releases)
- Mina mainnet GraphQL, `api.minascan.io/node/mainnet/v1/graphql`, 2026-10-28T14:26Z:
  `genesisConstants`, `daemonStatus.consensusConfiguration`, `bestChain(maxLength: 1)`.
- proof-systems at `92274448`:
  [`hasher/src/lib.rs`](https://github.com/o1-labs/proof-systems/blob/9227444890/hasher/src/lib.rs) and
  [`hasher/src/poseidon.rs`](https://github.com/o1-labs/proof-systems/blob/9227444890/hasher/src/poseidon.rs).
- [MIP-9](https://github.com/MinaProtocol/MIPs/blob/5aa77c8911e7af39ff9c594175226b171ed8166d/MIPS/mip-0009-increase-zkapp-account-update-limit.md)
  at `5aa77c89`.
- [ChainAgnostic `mina/caip2.md`](https://github.com/ChainAgnostic/namespaces/blob/5c515816a2828914bb77a3ed92f9445d2e29f785/mina/caip2.md)
  at `5c515816`; [SLIP-44](https://github.com/satoshilabs/slips/blob/570ed55b/slip-0044.md) at `570ed55b`.
- This repository at `9a039468`, working tree for ADR 0076 and ADR 0080:
  - [`batch/contract.rs`](../../crates/connector-settlement/src/batch/contract.rs)
  - [`batch/port.rs`](../../crates/connector-settlement/src/batch/port.rs)
  - [`connector-settlement-evm/src/batch_settlement.rs`](../../crates/connector-settlement-evm/src/batch_settlement.rs)
  - [`batch_watch.rs`](../../crates/connector-settlement-evm/src/batch_watch.rs)
  - [`connector-domain/src/client_claim.rs`](../../crates/connector-domain/src/client_claim.rs)
  - [`x402.rs`](../../crates/connector-domain/src/x402.rs)
  - [`asset.rs`](../../crates/connector-domain/src/asset.rs)
  - [`connector-signer/src/claim_state_challenge.rs`](../../crates/connector-signer/src/claim_state_challenge.rs)
  - [`connector-vectors/src/lib.rs`](../../crates/connector-vectors/src/lib.rs)
  - [`vectors/wire-vectors.json`](../../vectors/wire-vectors.json)
  - [`connector-config/src/batch_settlement.rs`](../../crates/connector-config/src/batch_settlement.rs)
  - ADRs [0071](../adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md),
    [0074](../adr/0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md),
    [0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md),
    [0076](../adr/0076-the-operator-names-the-facilitator-and-pays-its-gas.md) and
    [0080](../adr/0080-a-client-may-pay-from-a-zk-note.md)
- Earlier notes: [`mina-x402-batch-settlement.md`](mina-x402-batch-settlement.md),
  [`x402-pending-chain-bindings.md`](x402-pending-chain-bindings.md),
  [`zk-note-verifier-spike.md`](zk-note-verifier-spike.md).
