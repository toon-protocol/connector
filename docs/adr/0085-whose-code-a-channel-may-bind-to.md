# Whose code a channel may bind to

**Status:** Accepted (owner decision, 2026-10-28, #1564; part of map #1563). Nothing is built: the binary settles on EVM and Solana only, and this record admits no new chain. It sets the bar each later chain is held to. _Amendment proposed by [0087](0087-xrpl-settles-over-the-ledgers-payment-channels.md) (#1609, not yet accepted): decision 3's table gains its XRPL rows._ It **amends** [0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision 1, which admitted only x402's own deployed code, and it **records** #1558's amendment of the same decision for Mina, which no record had written down. The facts it rests on were read from primary sources on the day of decision: [research note](../research/whose-code-a-channel-may-bind-to.md).

**Scope:** connector architecture — which on-chain code a node will bind a channel to, and how the binary pins it. A binding's wire (its voucher and claim-state challenge) is protocol law, and is decided in each chain's own record. See the [ADR index](README.md).

**Falsifier:** none — the record builds nothing and claims nothing absent from the tree; it is a rule for chains not yet bound, checked when each one's own record is written.

**A channel binds only to on-chain code that is x402's own, a chain's native primitive, or TOON's.
Third-party code is never bound as found. Whose spec a binding follows is not the test, and an
unmerged spec is built to at a pinned commit. What a voucher binds is a constant of the binary per
(chain, network). On a mainnet entry, no single key may move, freeze or replace the money's code.**

## Context

ADR 0075 decision 1 makes every channel an x402 channel and admits only x402's own deployed code:
`x402BatchSettlement` on EVM and solana-foundation's `payment-channels` on Solana. Both are
constants of the binary, because a voucher binds them.

The owner wants the connector to settle on every chain that has an x402 batch-settlement binding
(#1563). None of the four chain bindings open upstream is merged, and none is backed by code x402
owns ([research](../research/x402-pending-chain-bindings.md)):

| Chain   | Spec                                           | On-chain code                                            |
| ------- | ---------------------------------------------- | -------------------------------------------------------- |
| XRPL    | Dhali's draft, x402 #2952                      | none: the ledger's own `PayChannel`                      |
| Hedera  | a contributor's draft, x402 #3466              | that contributor's fork, testnet only, admin key set     |
| Cardano | a contributor's draft, x402 #3580, and a rival | Kompact.io's Subbit validator, alpha, no audit published |
| Bitcoin | a contributor's draft, x402 #3544              | a vendor's custodial co-signing service, not a channel   |

#1558 (owner, 2026-10-28) had already amended decision 1 for Mina: a chain x402 does not yet serve
may use a **TOON-owned** binding of x402's chain-neutral outline, proposed upstream. That covers a
binding TOON writes. It does not say whether TOON may build to someone else's unmerged draft, or
bind to code that is neither x402's nor TOON's. This record says.

## Decision

### 1. Three kinds of on-chain code, and no fourth

A channel binds only to on-chain code of one of these kinds:

1. **x402's own deployed code.** Today's EVM and Solana.
2. **The chain's own native primitive.** Nothing is deployed, so nobody owns it. XRPL's `PayChannel`.
3. **Code TOON owns.** TOON deployed or pinned it, reviewed it, and answers for it. Mina's channel
   zkApp under #1558.

Third-party code bound as found is never admitted: Hedera's fork under its author's deploy, and
Subbit as it stands. It can enter only as kind 3, under decision 4.

Whose spec a binding follows is **not** the test. The money is in the code, not the prose. Waiting
for an upstream merge is dropped as a gate.

### 2. An unmerged spec is pinned at a commit and followed on purpose

A binding whose spec is someone else's open PR is built to that PR **at one commit**, and the
chain's own record names it (XRPL #2952's head on 2026-10-28 is `683fb7a6`). The connector never
tracks the PR loosely, and does not fork the spec unless upstream abandons it. Following a later
upstream change is a deliberate, reviewed change. When it moves claim bytes it bumps the vectors'
`schema_version` ([0021](0021-vectors-are-normative-prose-is-not.md)), so `toon-client`, `rig` and
`swap` see it.

### 3. What a voucher binds is a constant of the binary per (chain, network)

0075's "a constant of the binary, never config" becomes a table in the binary, keyed by network id.
An entry may be empty. XRPL's is: a claim binds only its channel id. Hedera's would map chain ids
295 and 296 to addresses, because Hedera does not deploy at a CREATE2 address. Cardano's would be a
script hash plus a reference-script UTxO for each network. An operator still cannot configure an
address. 0074 decision 4's reason holds unchanged: a configurable address only lets a node be
pointed at claims it cannot redeem.

Boot refuses, by name, a network with no entry, and one where the entry's code is absent. For a
native primitive, boot checks that the RPC answers with the network id the entry names (XRPL:
`server_info`'s `network_id`). It checks a ledger feature only while that feature is still a live
amendment. XRPL's PayChan is a retired amendment: its code always runs, and testnet reports it
`enabled: false` while channels work there.

_Amended (proposed, #1609, [0087](0087-xrpl-settles-over-the-ledgers-payment-channels.md) decision 5):_ the table gains XRPL rows, each with an **empty** binding and checked against `server_info.network_id`: `xrpl:0` (mainnet), `xrpl:1` (testnet), `xrpl:2` (devnet), and one stand-alone network, `xrpl:1000`, for the test harness and `local/` (testnet-class, at most 1024 so transactions carry no `NetworkID`, and not 0, 1 or 2). The mainnet row passes decision 5 as a native primitive. The body above is otherwise unchanged.

### 4. Adopting third-party code makes it TOON's

Third-party code enters as kind 3 only when all of these hold:

- **TOON's own instance.** On Hedera that means TOON deploys the contract itself, never the
  author's testnet deploy. A permissionless script such as Subbit is pinned by its hash.
- **TOON's review, recorded.** The chain's record names the exact code, a commit plus a bytecode
  hash or script hash, and records TOON's review of it.
- **An audit gates mainnet.** Until an external audit is published, the decision 3 table holds that
  chain's testnet entries only, so a node pointed at its mainnet is refused at boot by name.
- **The key rule** of decision 5.

### 5. One key rule, for mainnet entries, whoever wrote the code

On a **mainnet** entry of the table:

- **A single key that can move, freeze or replace channel funds or code fails.** That covers
  Hedera's author's ECDSA admin key, which Hedera's own spec says can delete the escrow and send its
  balance anywhere, and it covers any single TOON key.
- **A multisig upgrade authority passes** when an organisation publicly claims it and the admitting
  record names its address and threshold. The members need not be identified, since nothing on
  chain can identify them. This is what 0075 accepted for `payment-channels`: solana-foundation's
  Squads v4 vault, 3 of 5, no time lock.
- **Code nobody can change passes outright.** x402's EVM contracts (ownerless and immutable), a
  native primitive, and a permissionless script.
- **A key dormant until a protocol-version change passes** when the admitting record names it and
  who holds it, and TOON commits to reinstalling the same circuit, recompiled, at each such change,
  with the diff published. This is Mina's case: a zkApp's `Impossible` verification-key permission
  falls back to `Signature` at any hard fork that raises the transaction version, and Mina has no
  multisig account. The mechanics belong to #1548.

A **testnet** entry names its upgrade authority, and any authority is admitted, a single key
included. Testnet money has no value, and `payment-channels` on Solana devnet is upgradeable by one
keypair that this repository's own nodes already settle under.

## Considered options

- **Build to each open PR as it stands, and track it.** Rejected as worded: tracking an open PR
  moves the wire under the vectors without a decision. Kept in pinned form, as decision 2.
- **Wait for each binding to merge.** Rejected as the gate. A merge reviews the prose. It does not
  make third-party code safer to hold money in, and x402 owns none of these chains' code.
- **Admit native primitives only.** Admits XRPL and nothing else, and refuses Mina, contradicting
  #1558. Kept as the default for third-party code, which decision 4 is the only way past.
- **One key rule on every network.** Refuses Solana devnet, which 0075 already admits and the fleet
  settles on.
- **Destroy the Mina zkApp's key to meet the single-key rule.** Nothing on chain can prove a key
  destroyed, and the channels are stranded at the next fork that raises the transaction version.

## Consequences

- Hedera and Cardano stay parked (#1563). Adopting either is its own owner decision against
  decisions 4 and 5, including which of Cardano's two dialects to follow and whether Hedera's lack
  of a disposable test chain is fatal under the tier-3 rule. Bitcoin is not a channel and stays
  closed.
- XRPL clears decision 1 as a native primitive. Its claim binds no network id. That question stays
  with #1563's D10, not here.
- Every new chain adds a table entry per network, and the refusal at boot names the network.
- When x402 deploys code of its own on a chain, kind 1 applies and decision 4 does not.

## Sources

- Research note, 2026-10-28: [`whose-code-a-channel-may-bind-to.md`](../research/whose-code-a-channel-may-bind-to.md)
  (each fact with its primary source), and [`x402-pending-chain-bindings.md`](../research/x402-pending-chain-bindings.md)
  (the four bindings, D1 and D2).
- #1558: TOON builds its own Mina binding, proposed upstream ([`mina-x402-batch-settlement.md`](../research/mina-x402-batch-settlement.md)).
- #1563 (the map) and #1564 (this decision).
