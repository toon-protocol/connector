# XRPL settles over the ledger's payment channels

**Status:** Proposed (2026-10-29, #1609; spec #1596, part of map #1563) — for the owner to accept. It binds nothing until accepted. It builds on [0085](0085-whose-code-a-channel-may-bind-to.md) (XRPL clears its decision 1 as a native primitive) and [0086](0086-a-node-settles-on-more-than-one-evm-network.md) (CAIP-2 networks, CAIP-19 assets, one table per network), and it **amends** 0085 decision 3's constant table (three XRPL rows and a stand-alone id, noted in place there) and #1567's `<chain>:native` asset spelling (decision 7 below). No XRPL code exists in the tree; every later XRPL ticket in #1596 is held to this record.

**Scope:** protocol law — binds every implementation, because an XRPL claim, its claim-state challenge, an asset id and the vectors are specified here. See the [ADR index](README.md).

**Falsifier:** none — the record builds nothing and claims nothing absent from the tree; `xrpl:native` is already refused by name (`AssetIdError::LegacyNative`, since #1607), and what else it refuses (a second XRPL table on one account, an XRPL claim with no `network`) is refused by name once built, which is a presence the tests will hold rather than an absence a pattern can find.

**A node may settle on XRPL over the ledger's own `PayChannel`, built to x402's `xrpl-paychan-v1` binding at one pinned commit. A voucher is `CLM\0 ‖ channelId ‖ u64 drops`, and a node's account on one XRPL network may never be its account on another, because a claim binds no network. Every XRPL leg is an ADR 0071 dealing boundary at a declared rate, since a channel holds XRP only. A receiving node answers a payer's exit by landing its latest voucher with `tfClose`.**

## Context

[0085](0085-whose-code-a-channel-may-bind-to.md) admits a channel bound to a chain's native primitive and keeps the wire of each binding for that chain's own record. XRPL is the first such chain (#1563, #1570): a channel is the ledger's own `PayChannel` object, with nothing to deploy, own or audit. This is that record. The facts are in [`x402-pending-chain-bindings.md`](../research/x402-pending-chain-bindings.md) §2 and [`native-coin-rate-sources.md`](../research/native-coin-rate-sources.md), each with its primary source.

Four properties of the ledger shape every decision below:

- **A claim binds no network id.** One signed claim verifies on mainnet, testnet and devnet alike, if the same account holds a channel with the same id on each. Channel ids are `SHA512Half(0x0078 ‖ source ‖ destination ‖ sequence)`, so they collide across networks whenever an account and its sequence do.
- **A channel holds XRP only.** `PayChannel` and `PaymentChannelClaim` amounts are drops of XRP; the token-escrow amendment (XLS-85) extends Escrow and not PayChannel. RLUSD and USDC exist on XRPL and cannot enter a channel.
- **The payer's exit is a delay that runs on the ledger's close time.** The source's `tfClose` sets `Expiration = parent close time + SettleDelay`, and after it any transaction touching the channel deletes it and refunds the payer.
- **PayChan is a retired amendment.** Its code always runs. Testnet reports it `enabled: false` while channels work there, so a boot check on the amendment would refuse a working network.

## Decision

### 1. The binding is pinned: x402 PR #2952 at `683fb7a6`, `xrpl-paychan-v1`

The XRPL backend is built to the batch-settlement spec of [x402 #2952](https://github.com/x402-foundation/x402/pull/2952) at `Dhali-org/x402@683fb7a6` (the head on 2026-10-28), whose version string is **`xrpl-paychan-v1`**. 0085 decision 2 applies: the connector does not track the PR. Following a later upstream change is a deliberate, reviewed change to this record, and one that moves claim bytes bumps the vectors' `schema_version` ([0021](0021-vectors-are-normative-prose-is-not.md)).

Only the spec's identifiers (§4), channel and signing input (§6.1–6.2) and redemption transaction (§11) bind the connector. Its facilitator-side metering (reservations, `committedAmount < authorizedAmount`, `payment-identifier`) does not: a TOON voucher signs the exact cumulative charge, so a landing is always `Amount == Balance == cumulative`, which the ledger permits. No `facilitator` is published for XRPL, and [0076](0076-the-operator-names-the-facilitator-and-pays-its-gas.md) stays EVM-only.

`binding: "xrpl-paychan-v1"` is published in the greeting's `extra` (decision 9). It is **not** the 0085 decision 3 constant for a voucher's binding, which is empty (decision 4).

### 2. The claim wire

A voucher's signed message is

```
"CLM\0" (0x43 0x4C 0x4D 0x00)  ‖  channelId[32]  ‖  amount: u64 big-endian, drops
```

- The key is the channel's `PublicKey`, fixed at creation: 33 bytes, **secp256k1 compressed** or **Ed25519 prefixed `0xED`**. The prefix byte picks the algorithm. A key that is neither is not a valid channel key, and admission refuses it.
- **secp256k1** signs `SHA512Half(message)` and the signature is **DER**. **Ed25519** signs the **raw message**. This is what `xrpl-rust` does and what the ledger's `PaymentChannelClaim` verifies.
- The amount is **cumulative** with no nonce and no expiry; the amount-only watermark of 0075 (`validate_voucher`) applies unchanged.
- **The ceiling is 10¹⁷ drops**, XRP's total supply and a figure inside `u64`. A voucher above it is refused by the named `AmountExceedsChain { amount, max }`, never truncated. Amounts stay `u128` at the port.
- On a `ClientClaim`: `blockchain: "xrpl"`, `scheme: "batch-settlement"`, a required CAIP-2 `network` (`xrpl:<id>`), and the signature. A claim with no `network`, or one for a network the node holds no table for, is refused by name (0086 decision 1). A voucher signer is spelled as the 33-byte public key in hex.
- The journal keys an XRPL channel `xrpl:<channel id>`, as 0086 decision 9 keys the others: a channel id is unique across networks **only because decision 4 makes the account unique across networks**, which is why that rule is a precondition of this one.

### 3. The XRPL voucher claim-state challenge

Nothing upstream defines how a payer proves control of a channel in order to read a node's watermark ([0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision 5). This record does, as TOON's own:

```
"toon-xrpl-voucher-claim-state-challenge-v1"  ‖  len(network): u8  ‖  network (UTF-8, CAIP-2)  ‖  channelId[32]  ‖  expires: u64 big-endian
```

- It has **its own domain tag**, not Solana's. It binds the **CAIP-2 network**, the channel id and the expiry, so a challenge signed for a testnet channel proves nothing on mainnet even though a claim would.
- **Its bytes can never parse as a claim**: a claim starts `0x43 0x4C 0x4D 0x00` and the tag starts `0x74` (`t`). A signature on one is never a signature on the other, so a challenge cannot be replayed as a voucher.
- It is signed **under the channel's key exactly as a claim is**: SHA512Half and DER for secp256k1, the raw message for Ed25519.
- The same shape, with a different tag, serves a client's auth-channel challenge (0074). The vectors fix the exact bytes; where this prose and the committed vectors disagree, the vectors win (0021).

### 4. One account is one network

A claim binds no network, so an XRPL **account** is bound to one network. Therefore:

- **Boot refuses, by name, two `[settlement.xrpl.<id>]` tables that resolve to the same account.** The account is derived from the table's key, and the comparison is by classic address.
- **Operators use a different account on testnet than on mainnet**, and the operator guide says so plainly. A testnet voucher signed by a mainnet account is redeemable on mainnet by anyone holding a channel with the same id and sequence there.
- The 0085 decision 3 entry for XRPL is **empty**: nothing in a claim binds a contract or program, and the network is checked by `server_info` (decision 5), never by a claim field.

The rule does not reach across nodes: it holds the operator to it on the node it can check, and the guide covers the rest. It is the same reason merged `scheme_upto_xrpl.md` tells wallets to keep an account per network.

### 5. The networks, and ADR 0085's constant table

ADR 0085 decision 3 gains these rows (noted in place there; its body is otherwise unchanged). Each has an **empty binding**, and a mainnet row passes the key rule of 0085 decision 5 as a native primitive:

| Network     | `server_info.network_id` | Class                                                     |
| ----------- | ------------------------ | --------------------------------------------------------- |
| `xrpl:0`    | 0                        | mainnet                                                   |
| `xrpl:1`    | 1                        | testnet                                                   |
| `xrpl:2`    | 2                        | devnet                                                    |
| `xrpl:1000` | 1000                     | stand-alone: the test harness and `local/`, testnet-class |

**The stand-alone network id is 1000.** Its constraints, which any replacement must keep:

- **At most 1024**, so a transaction on it carries **no `NetworkID` field** (the ledger requires one only above 1024) and the same transaction bytes work on every row of the table.
- **Not 0, 1 or 2**, so a harness-signed claim can never be taken for a real network's.
- **Testnet-class**: no value, no mainnet key rule, and a node pointed at it is never taken for mainnet.

Boot checks that the table's `rpc_url` answers `server_info` with `network_id` equal to the table's key, and refuses a mismatch by name. It **never** checks the PayChan amendment. A network with no row is refused by name. A missing settlement account, or one holding less than the base reserve, is refused by name at boot.

### 6. Native-coin dealing: every XRPL leg is an ADR 0071 dealing boundary

#1567 resolved, on 2026-10-28, that these rules belong in "the first native-coin settlement ADR", and this is it. A channel holds XRP only, so:

1. **A native-coin-only chain is admitted as a peering leg** as ordinary [0071](0071-a-forward-crosses-a-denomination-at-a-declared-rate.md) dealing. A hop whose two sides hold different tokens converts at a declared rate; with no rate there is no forward (0071 decision 2). A path that holds XRP end to end forwards unconverted.
2. **There is no extra cap.** An XRP channel is real escrow, unlike [0080](0080-a-client-may-pay-from-a-zk-note.md)'s allowance claims. The numeraire stays a stablecoin.
3. **The rate is static**: a `[[rates]]` row plus a spread, as 0080 does for MINA. XRPL has no rate source. An AMM gives only a spot price, which 0071 rejects, and an XLS-47 oracle is not a pool, so using one would amend 0071 and is not done here.
4. **`[[tokens]]` may name a native coin when the node has a settlement backend for its chain.** Every peering must resolve to a declared token, so an XRPL peering forces the XRP asset into `[[tokens]]`. 0080's refusal of a `mina:` asset in `[[tokens]]` is untouched: Mina has no settlement backend.
5. **A forward between two XRPL networks is a forward between two assets** (0086 decision 13). XRP on testnet is never XRP on mainnet.
6. **Fees are paid in XRP, the asset being settled.** No party needs a second coin for gas. There is no fee sponsorship on XRPL (XLS-68 is not enabled on mainnet), so each party pays its own fees and its own reserve, and a peering costs each side one owner reserve (0.2 XRP on 2026-10-28) until its channel is deleted. This costs a payer with no spare XRP a refusal by name (the shortfall), never a `tecUNFUNDED`.

### 7. The asset spelling is CAIP-19, and `<chain>:native` is refused

**Amends #1567 decision 2** (owner, 2026-10-28). #1567 spelled a native coin `<chain>:native` inside the connector, with CAIP-19 only in the greeting. 0086 decision 2 had made every asset id CAIP-19 inside the connector, and the two conflicted. CAIP-19 wins:

- XRP is **`xrpl:<id>/slip44:144`** everywhere: config, `[[tokens]]`, `[[rates]]`, the journal and the greeting. There is one asset-id grammar.
- **`xrpl:native` is refused by name**, and so is every other `<chain>:native` spelling in a place that parses an asset id. The asset-id parser already refuses them by name (`AssetIdError::LegacyNative`, since #1607); this decision is the record of that refusal.
- This amends #1567's internal spelling only; its other resolutions stand (decision 6 carries them). [0080](0080-a-client-may-pay-from-a-zk-note.md) decision 7 already meets the same rule: its 2026-10-28 amendment (merge of #1607) spells MINA `mina:<net>/slip44:12586` and refuses `mina:native` by name.

### 8. The watcher lands with `tfClose` on a payer's exit

#1565 left open how the XRPL watcher answers a payer's exit. **It lands the node's latest held voucher with `tfClose`, in one transaction**, the moment it sees `Expiration` set on an inbound channel. That pays the node, refunds the payer and deletes the entry at once, like Solana's `settle_and_seal`, before the delay can run out on the node. With nothing worth landing it closes the channel with `tfClose` alone, so the payer's refund is not held and nobody's reserve stays locked.

**The accepted cost:** a payer that began an exit and then wanted to `tfRenew` loses the chance once its receiving node has seen it. The alternative, landing without `tfClose` and waiting, leaves the channel open with an `Expiration` the payer may still renew, so the node must race the delay on every exit. The node never lets the payer's exit decide whether it is paid.

The ordinary land (`POST /channels/:id/land`) is a destination `PaymentChannelClaim` with **no** `tfClose`, and leaves the channel open.

### 9. The greeting, and what a node refuses

- **One `batch-settlement` entry per XRPL table**: `network: xrpl:<id>`, `asset: xrpl:<id>/slip44:144`, `payTo` the node's classic r-address, and `extra: {binding: "xrpl-paychan-v1", minSettleDelay, redemptionSafetySeconds}`. The self-description's `voucherSigners` publishes the channel key.
- **Admission refuses**, each by a named `AdmissionRefusal`: a `Destination` that is not the node's settlement account; a `SettleDelay` below the table's minimum (above the port's 901 s floor); an `Expiration` already set; a `CancelAfter` of any value (a new variant, since it is an immutable deadline the node cannot see coming and could strip a held voucher); a key that is not a valid channel `PublicKey`. A `DestinationTag` is not an admission dimension. A node's own outbound channels never set `CancelAfter`, so its peers never refuse them.
- **A land that delivered nothing is a failure.** An expired channel answers `tesSUCCESS` with nothing delivered, so success is read from transaction metadata (the channel's `Balance` advanced), never from the result code alone.
- **Every wait is bounded by `LastLedgerSequence`**, and a transaction past it is known-not-landed (0073). A retried open resumes the journaled channel and creates no second one (0075 decision 8's outbound-channel journal entry).

### 10. The state a ledger entry has

An XRPL channel deletes itself on close, so the port reads it as follows (0075, "Update (issue #1565)"): `landed = Balance`; `collateral = Amount − Balance` while there is no `Expiration` and 0 once there is; status `Open` with none and `Closing` with one, and back to `Open` after `tfRenew`, since a payer may undo its exit. There is no `Withdrawing`. A deleted entry reads `ChannelNotFound` on the receiving half, with no copy kept; on the paying half `outbound_state` answers from the journal's cache and `finish_withdrawal` returns `NoWithdrawalPending`.

## Update (issue #1614): a node boots on XRPL and publishes it

`[settlement.xrpl.<id>]` is read, and a node with one boots a receiving backend over it, with the
boot refusals of decisions 4 and 5 each raised by name from `connector-cli` (`XrplNetworkMismatch`,
`XrplAccountMissing`, `XrplAccountUnderfunded`, `XrplAccountOnTwoNetworks`; an unknown network is
refused earlier, at config load). The refusals are checked against `server_info` and `account_info`
before the backend is built. Decision 9's greeting entry, `voucherSigners`, and the claim gate's
admission, journaling (`xrpl:<channel id>`, with the network the voucher named) and restore are built;
`GET /channels` and `GET /claims` list the channels. The reserve is read from `server_info`'s
`validated_ledger.reserve_base_xrp`. The paying half and the watcher (decision 8) are later tickets
of the spec: an XRPL table adds no payer to the outbound channels, and
`OutboundChannelRecord::Xrpl` (the signed `PaymentChannelCreate` and its `LastLedgerSequence`, so a
retried open resumes the journaled channel) exists for them to write.

## Considered options

- **Track PR #2952 loosely.** Rejected by 0085 decision 2.
- **Bind a network id into the claim.** Impossible: the claim is the ledger's, and `PaymentChannelClaim` verifies exactly the `CLM\0` bytes. The network is bound by account (decision 4) and, in the challenge, by the CAIP-2 string (decision 3).
- **Reuse Solana's challenge domain tag.** Rejected: one signature would be valid on two chains' channels if an Ed25519 key were shared, and Solana's message has a different shape.
- **Land without `tfClose` on a payer's exit.** Rejected for the watcher (decision 8): it leaves the delay to run against the node.
- **Check the PayChan amendment at boot.** Rejected (0085 decision 3): testnet reports it disabled while channels work.
- **A `<chain>:native` asset spelling.** Rejected (decision 7), by the same reason 0086 refused `evm:<chainId>:0x…`: a second grammar beside CAIP-19, which the old parser read as a different token.
- **A stand-alone id above 1024.** Rejected: transactions would carry `NetworkID`, and the harness would sign bytes mainnet transactions never carry.
- **An XLS-47 oracle as the XRP rate source.** Parked as fog on #1563. It needs an amendment to 0071, and the only confirmed publisher updates up to about 5 h 40 m apart.

## Consequences

- **XRP is a dealing denomination.** An operator who wants an XRPL peering declares `xrpl:<id>/slip44:144` in `[[tokens]]` and a `[[rates]]` row with a spread, and carries the price risk of holding XRP between its declared rate and the market.
- **`schema_version` moves once more** when XRPL entries land in the vectors (`claim_voucher`, `payout_voucher`, `voucher_claim_state_challenge`, `client_auth_channel_challenge`, and `claim_voucher.xrpl_above_ceiling`), shared with 0086's bump if both ship in one release.
- **A new `AdmissionRefusal` variant** (`CancelAfter`) and **`AmountExceedsChain`** enter the port; Solana's `Backend(String)` narrowing moves to the latter (#1565).
- **A payer on XRPL pays for its own channel**: deposit, owner reserve and fee, with no sponsor.
- **Tests need `xrpld`.** The harness spawns `rippleci/xrpld` pinned by digest, one chain per test (0084 default), fails CI and skips locally without Docker, and is the first container-backed `require_*`. Whether stand-alone `ledger_accept` advances close time, and whether stand-alone accepts a configured `network_id`, are **unconfirmed** and checked first by the harness ticket.
- **Out of scope:** issued currencies and MPTs; #2952's facilitator-side metering; XLS-68 sponsorship; an XLS-47 rate source; which XRPL networks the devnet fleet runs; `DestinationTag` and DepositAuth credentials as admission dimensions; Hedera, Cardano and Bitcoin.

## Sources

- [`x402-pending-chain-bindings.md`](../research/x402-pending-chain-bindings.md) §2 and §8: the spec, the ledger's behaviour, the port mapping.
- [`native-coin-rate-sources.md`](../research/native-coin-rate-sources.md): why XRPL's rate is static.
- [`whose-code-a-channel-may-bind-to.md`](../research/whose-code-a-channel-may-bind-to.md): the binding rule.
- #1596 (the spec), #1567 (native-coin dealing, amended by decision 7), #1565 (the port's lifecycle), #1564 (0085), #1566 (0086), #1568 (0084), #1563 (the map).
