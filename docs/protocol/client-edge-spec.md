# Client edge specification

**Status:** **Live — this is the client edge specification** (wayfinder map #1049, issues #1065, #1073).
Its known corrections are applied: §2 and §3.4 no longer cite the spent ADR 0013, §3.2's
`GET /ilp/versions` is retired in favour of the node self-description (#1054), and §1.4's claim that no
settlement address is configured is corrected — they have been in the greeting since issue #617.

**Its authentication, identity and privacy surface is now recorded**, in
[ADR 0052](../adr/0052-permissionless-payment-is-guaranteed-and-a-claim-is-what-authorises.md): a
conforming connector accepts payment from a buyer it has never heard of; a **claim** authorises and an
identity does not; _unverifiable is never accepted, by configuration, flag or build profile_; and a
claim may be presented plaintext or NIP-59-wrapped at the **client's** choice, with both accepted.
Before that record this surface appeared in none of the first 51.
_Originally:_ Non-normative. [ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md) makes the
Rust implementation (`crates/connector-client-edge`) the definition of this wire, and the committed
vector set (`vectors/wire-vectors.json`, issue #527) — fixed literal fixtures pushed through the
real implementation and self-verified against the same functions the invariants listed in
[`docs/protocol/wire-vectors.md`](wire-vectors.md) hold open, not values literally emitted by a
property-test run — the cross-repo contract `toon-client`, `rig` and `swap` are actually held to.
This document remains prose describing that
wire for a human reader: useful as orientation, evidence of intent, and a map of what's shipped
versus what isn't, but it is not itself something to conform to, and a disagreement between this
text and the code is a bug in this text. Where this document and an ADR disagree, the ADR wins —
this document is reconciled to match, not the other way around. Version 1 below is organized by
section number so `crates/connector-client-edge`'s own doc comments can cite it; §3 sketches how a
future version would be introduced, per
[ADR 0003](../adr/0003-clean-room-peer-wire-versioned-client-edge.md).
**Consumers:** `toon-client` and any other app that pays this connector directly — installed on
machines this repository's operators do not control.
**Vocabulary:** [`CONTEXT.md`](../../CONTEXT.md).
**Where the claim a client pays here goes next**, and why it is never forwarded onward:
[`money-model-pre-868.md`](money-model-pre-868.md).

The **client edge** is the protocol a client speaks to the connector it attaches to
(`CONTEXT.md`). Unlike the peer semantics, it is versioned rather than redesigned: its far end is
software this repository does not ship and cannot flag-day, so an old version keeps working
after a new one exists ([ADR 0003](../adr/0003-clean-room-peer-wire-versioned-client-edge.md),
[ADR 0001](../adr/0001-rust-workspace-library-first.md) — `connector-client-edge` is exposed as
an HTTP router).

## Scoping note

The TypeScript connector's now-removed embedded node (gone as of v4.0.0, [issue
#465](https://github.com/toon-protocol/connector/issues/465)) accepted client traffic over two
transports: the duplex, session-stateful BTP WebSocket (RFC-0023) that also carries peer-to-peer
traffic, and the one-shot ILP-over-HTTP binding (RFC-0035) at `POST /ilp` — its own documentation
described this as the edge transport for one-shot, stateless purchases: a buyer, a NAT'd client, a
browser, or an agent that only consumes. That source no longer exists in this repository but is
recoverable from git history prior to #465. That BTP did double duty is exactly the conflation
[ADR 0003](../adr/0003-clean-room-peer-wire-versioned-client-edge.md) retires: the peer semantics
(`docs/protocol/peer-semantics-pre-868.md`) is redesigned freely because both its ends are
operator-controlled, which is never true of a client. This document therefore specifies the
client edge as **ILP-over-HTTP** — `POST /ilp` — since that is the transport whose far end is
genuinely uncontrolled and whose shape carries forward as "version 1" of the versioned scheme. A
client that reached the old embedded node over BTP was, for the purposes of this spec, using the
peer semantics's pre-rewrite transport as a transitional convenience, not the client edge; it is out of
scope here and is not preserved by the redesigned peer semantics.

`POST /admin/ilp/send` was a distinct, operator-surface-adjacent interface the same removed
embedded node exposed so an app behind this connector could ask its _own_ connector to originate a
packet outward — also recoverable from git history prior to #465, not present in this repository.
It was not the client edge either — the caller there is the
connector's own app, not an unaffiliated payer — and is out of scope for this document.

## 1. Version 1 (current)

### 1.1 Transport and framing

- **Method/path:** `POST /ilp`.
- **Request body:** an ILPv4 PREPARE packet, `Content-Type: application/octet-stream`. **ILPv4
  semantics, TOON encoding**
  ([ADR 0063](../adr/0063-the-ilp-packet-is-toons-dialect-not-rfc-0027s.md)): the three type bytes,
  the field order and meanings, `condition = sha256(fulfilment)` and the `F`/`T`/`R` taxonomy are
  RFC-0027's; the **bytes are not**. This packet is not byte-compatible with RFC 0027, has never
  been, and is not going to be — an off-the-shelf ILPv4 encoder does not produce one this edge
  accepts. It diverges in exactly three places:
  - no outer type-length wrapper: the type byte is followed by the fields inline, not by a
    VarOctetString;
  - `amount` is a VarUInt, not a fixed 8-byte `UInt64`;
  - `expiresAt` is a 19-byte GeneralizedTime, `YYYYMMDDHHMMSS.fffZ`, not the 17-byte Interledger
    Timestamp.

  **The bytes are pinned by `vectors/wire-vectors.json`**, whose `peer_carriage.prepare`
  fixture carries a complete encoded PREPARE as `http_body_hex` and `btp_message_hex`. That
  fixture is the cross-repo contract for this encoding
  ([ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md)), not the RFC;
  `vectors/README.md`'s "The ILP packet encoding" section walks it byte by byte.

- **Response:** `200 OK` with a FULFILL or REJECT body in that same encoding, `Content-Type:
application/octet-stream`. An ILP-level outcome — fulfilled or rejected — is always HTTP 200;
  a non-2xx status is reserved for a transport-level failure and never carries an OER body:

  | Status | Meaning                                                                         |
  | ------ | ------------------------------------------------------------------------------- |
  | `400`  | Malformed request: not a PREPARE, undecodable OER, oversized body.              |
  | `401`  | An `ILP-Peer-Id` was presented but authentication failed. Answered before       |
  |        | the route is looked up, so it never arrives as a `402` instead. See §1.2.       |
  | `402`  | Unpaid request to a route this connector terminates and prices: x402 v2         |
  |        | payment-required terms, JSON body (not OER). See §1.4.                          |
  | `403`  | A probe (`POST /ilp/probe`) from a sender not authorized to probe: no           |
  |        | payment channel this connector recognizes, or over its rate limit. See §1.6.    |
  | `413`  | Request body too large. There is no config field for this: the limit is         |
  |        | axum's own `DefaultBodyLimit` (2 MiB), which this router does not override.     |
  | `500`  | Reserved by this spec for transport failure only; an unexpected                 |
  |        | internal error during routing is surfaced as a `200` + `T00` REJECT, not a 500. |

### 1.2 Identity

`GET /ilp/identity` (§1.7) answers a different question — the connector's own key, not who is
asking. This section is who is asking, and ships today (issue #502): `POST /ilp` reads
`ILP-Peer-Id` and `Authorization`, resolves the sender, and refuses a presented identity that does
not authenticate with `401`. The identities a node recognises are the `[[client_identities]]`
section of its config file (`id` + `secret`); a node that configures none — the default — treats
every request that presents no `ILP-Peer-Id` as anonymous and refuses every one that presents an
`ILP-Peer-Id` at all.

A request identifies its sender in one of two ways:

- **Configured peer:** `ILP-Peer-Id: <id>` plus `Authorization: Bearer <secret>` (an empty
  bearer, i.e. `Authorization` absent with `ILP-Peer-Id` present, is accepted on a
  permissionless-configured identity — mirrors BTP's `secret: ''` auth frame). Failure to
  authenticate a presented `ILP-Peer-Id` is `401`.
- **Anonymous:** no `ILP-Peer-Id`. The connector derives an ephemeral peer id from the plaintext
  `ILP-Payment-Channel-Claim` header's signer (`http:<signerAddress-or-signerPublicKey>`), or
  `http:anon` if that header is absent — including when only the wrapped
  `ILP-Payment-Channel-Claim-Wrapped` header is present, since deriving an identity from it would
  require unwrapping before the identity used to authenticate the request is known. This is the
  path an unaffiliated buyer uses — no prior registration with the connector's operator is
  required to pay for a terminated route.

### 1.3 Payment claim

A request pays with a claim header. The claim is a JSON object, `version: '1.0'`, discriminated
by `blockchain: 'evm' | 'solana'` — the shape below is this document's own definition, not a
pointer to source; the peer semantics's predecessor (BTP protocol) carried the same shape, but that
code no longer exists in this repository. `blockchain: 'mina'` is a distinct, invalid value here:
see the note at the end of this section.

| Header                              | Content                                                      |
| ----------------------------------- | ------------------------------------------------------------ |
| `ILP-Payment-Channel-Claim`         | `base64(JSON.stringify(claim))`, plaintext.                  |
| `ILP-Payment-Channel-Claim-Wrapped` | `base64(NIP-59-wrapped claim)`, for a privacy-wrapped claim. |

A wrapped claim is sealed to the same public key `GET /ilp/identity` (§1.7) reports — the
connector's own signing key, the one §1.8's payload sealing already uses — because that endpoint is
the only surface publishing a receiver key a sender could wrap to
([issue #556](https://github.com/toon-protocol/connector/issues/556)). Unwrapping grants no
exemption: the claim inside runs every step below exactly as a plaintext one does. A wrap this
connector cannot open is refused under its own reason, distinguishable both from a malformed header
and from a claim naming an unknown channel.

Required fields on every claim, regardless of chain: `version` (`'1.0'`), `blockchain`,
`messageId` (idempotency), `timestamp` (ISO 8601), `senderId`. Chain-specific fields:

- **evm**: `channelId` (bytes32 hex), `nonce` (uint), `transferredAmount` (decimal string,
  cumulative), `lockedAmount`/`locksRoot` (present on the wire today for backward compatibility
  but always zero — see [ADR 0004](../adr/0004-value-moves-on-fulfilment.md) — and dropped
  entirely once a client edge version built against the rewritten balance proof ships),
  `signature` (EIP-712), `signerAddress`; optional `chainId`, `tokenNetworkAddress`,
  `tokenAddress`. `signerAddress` and the optional domain fields ride the wire but carry no
  authority — step 4 below reads both the signer and the signing domain from the connector's own
  per-channel record instead.
- **solana**: `programId`, `channelAccount` (both base58), `nonce`, `transferredAmount` (lamports,
  decimal string), `signature` (base64 Ed25519), `signerPublicKey` (base58); optional `cluster`.
  `programId` MUST be the **settlement program the `channelAccount` lives under** — the program
  that owns that account and that would redeem this claim on chain, which is byte-for-byte the
  program id the payer put into the balance proof `signature` covers
  ([ADR 0053](../adr/0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md) puts it at
  offset 16 of that message). It is not a free-form label and it is not the payer's own key: a
  claim naming any other program names a program no channel of the payer's lives under. The
  normative statement of this is `peer_carriage.claim_solana` in `vectors/wire-vectors.json`, whose
  declared `programId` and whose `signed_message_hex` carry the same 32 bytes
  ([ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md) — prose is not).
  `signerPublicKey`, like an EVM claim's `signerAddress`, rides the wire but carries no authority.

A present claim is validated by the same gate the peer role uses (the inbound claim validator)
before the PREPARE is routed, in this order — deliberately freshness-and-value before
cryptography, so a replay or an underpayment never pays the cost of a signature verification and
never reaches the terminating app:

1. **Structural validation** — required/optional fields per chain, formats (hex length, base58
   alphabet) as enumerated above; a structurally invalid claim is rejected. `blockchain: 'mina'`
   fails here unconditionally — see the note below.
2. **Freshness** — the claim's nonce MUST strictly advance this connector's last-verified
   watermark for the (peer, blockchain, channel) tuple; a non-advancing nonce is rejected without
   spending a cryptographic verification on it.

   The **channel** in that tuple is the channel, not the text the claim spelled it with ([issue
   #643](https://github.com/toon-protocol/connector/issues/643)). A connector MUST identify a
   channel's watermark by a canonical form of the id, applied before the watermark is written or
   read. For `evm` that form is exactly `0x` followed by the `channelId`'s 32 bytes as 64
   **lower-case** hex characters — one spelling, not a family of accepted ones, since a canonical
   form that admitted alternatives would be the same ambiguity again. For `solana` it is the
   `channelAccount` as it arrives: base58 of an exact 32-byte decode already has only one
   spelling, and base58 is case-_sensitive_, so normalising it would merge distinct accounts.
   Hex is case-insensitive and everything else about a claim already treats
   the spellings as one channel — the counterparty record is looked up by the decoded bytes, and
   the EIP-712 digest is computed over them — so a connector that keyed a watermark by the literal
   text would grant a fresh, empty watermark per spelling, and `None` accepts every nonce: one
   signed claim would buy a write once per casing it was retyped in.

3. **Value binding** (for a locally-terminated, priced route) — the claim's cumulative amount
   MUST advance by at least the route's configured flat price, so a minimal fresh claim cannot pay
   for an expensive route. This compares the claim's plaintext `transferredAmount` directly.
4. **Cryptographic verification** — the signature (EIP-712 for EVM, Ed25519 for Solana) MUST
   recover to **the counterparty recorded for the channel the claim names**
   ([issue #558](https://github.com/toon-protocol/connector/issues/558)). A claim's own
   `signerAddress`/`signerPublicKey` is not consulted, and neither is the EIP-712 domain
   (`chainId`/`tokenNetworkAddress`) it declares for itself: both come from the connector's
   per-channel record, so a claim has no say in what it is checked against. A forger who signs
   correctly with a key of their own and declares themselves the payer is refused here, because
   that key is not the channel's counterparty.

   A `solana` claim's self-declared **`cluster` is cross-checked** against the cluster the
   connector settles on, before its channel is resolved
   ([issue #975](https://github.com/toon-protocol/connector/issues/975)). Where a connector
   can tell which cluster it is on, it MUST refuse — under its own reason — a claim whose
   optional `cluster` names a different one. The field is not an authority the check reads
   _from_; it is an assertion the claim makes, and a claim naming a chain the connector is
   not on is **wrong, not merely unverifiable**. Silently accepting it would leave a
   settlement on one chain permanently labelled with another's name, invisible to both
   parties — the payer sees a FULFILL, the operator sees nothing — and the claim is the
   artifact each side keeps.

   `cluster` gets this treatment because it is the one field naming a chain that **no
   signature can ever bind**. Since [ADR 0053](../adr/0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md)
   a Solana balance proof signs the settlement program, and the verifier rebuilds that
   message from the channel's own program id rather than the claim's — so cross-cluster
   replay is closed by the bytes. A Solana program cannot learn which cluster it is running
   on, so it could never rebuild a message containing one; the cluster stayed out of the
   signed bytes for that reason, and this off-chain comparison is the only check it can get.
   A claim's declared `programId` is a different case again, and the two halves of it should
   not be confused. **What a payer must write there is pinned** — the settlement program the
   `channelAccount` lives under, as the field list above and
   `peer_carriage.claim_solana` now both say. **What a connector does with a disagreement is
   still only to report it.** A connector MUST report one (this connector logs it at `warn`,
   naming the channel and the declared value) and MUST NOT refuse on it, because the signature
   is checked against the channel's own program either way: a claim that reaches this point is
   one whose payer signed for this node's program whatever they wrote in that field, so it is
   cryptographically correct and fully redeemable, and refusing it would refuse money the node
   can actually collect.

   That asymmetry is deliberate and dated. Until
   [issue #1127](https://github.com/toon-protocol/connector/issues/1127) the field had no
   pinned meaning at all: this document called it decorative and the one cross-repo statement
   of it — `peer_carriage.claim_solana` — declared the **system program**, which is not a
   settlement program and which no channel lives under. A payer built against that contract is
   conforming to what it said. Refusing on the field is therefore gated on adoption, not on
   this text: it becomes permissible once the payers on a connector's client edge are known to
   emit the pinned value, and that is a fleet-by-fleet fact rather than a protocol one, since
   the client edge is where buyers a connector has never heard of arrive. The vector's
   `schema_version` bump to `2` is the signal that the reading changed.

   **The duty to report is this edge's alone.** `peer-carriage-spec.md` §4 carries this whole
   field list onto the peer edge unchanged — a peer claim is the same JSON object, and what a
   payer MUST write in `programId` is the same there — but §4.1 deliberately does **not** carry
   the reporting duty with it: a peer claim's declared `programId` is validated structurally and
   then discarded, and this connector's shared claim codec drops it before the claim is judged.
   Neither edge may refuse on the field. The difference is in what a report could find and who
   could act on it, not in peers being trusted more. Since
   [issue #1128](https://github.com/toon-protocol/connector/issues/1128) a Solana peering has
   exactly one program it can be judged under — `[settlement.solana] program_id` — and that one
   value both renders the field on an outbound peer claim and keys the channel an inbound one is
   verified against, so a peer that declares a program it did not sign under is a disagreement
   this connector cannot produce, and a peer that signs under a program this node does not settle
   with already fails verification outright and carries no traffic at all. Here neither holds: the
   payer is someone the operator has never heard of, running software the operator did not
   configure and cannot reach, so this log line is the only channel by which the mislabelling can
   become known to anyone — and it is the adoption signal a future refusal is gated on, for the
   only population whose adoption is an open question. **Read issue #1127's "wait until the
   warning stops firing" accordingly: it is a statement about client-edge payers, and the peer
   edge's silence is not a gap in it.** §4.1 of that document states this difference and argues
   it from the same two cases; the two texts are meant to be read as a pair, and neither should be
   changed alone.

   The cluster check is skipped where there is nothing to compare: a claim that omits
   `cluster` declares none, and a connector with no `[settlement.solana]` table — or one
   whose `rpc_url` names no cluster it recognises, a third-party RPC provider's say — knows
   of none. A connector MUST NOT guess a cluster from such a URL: a wrong guess refuses every
   genuine claim it ever receives.

   A claim naming a channel the connector has **no record of** is refused with its own reason,
   distinguishable from a bad signature and from an underpayment — there is nothing to verify it
   against, and unverifiable is never accepted. A node that can vouch for no channel therefore
   accepts no claim at all; that is the intended failure mode, since the only alternative is
   trusting what a claim says about itself.

   Where the record comes from is a deployment question rather than a wire one, and there are two
   sources ([issue #556](https://github.com/toon-protocol/connector/issues/556)). A node declares
   channels in its `[[client_channels]]` config section, whose entries carry the counterparty and
   the signing domain per channel; and a node with a `[settlement]` section **resolves any other
   channel from the chain that section already names**, reading the counterparty and the EIP-712
   domain off the deployed `TokenNetwork` itself. The second is what makes §1.2's anonymous path
   real: an unaffiliated buyer registers on chain, which the connector can read, rather than with
   the operator. A declared channel is authoritative and is never resolved, so a node with no
   settlement backend — or one whose chain endpoint is unreachable — still accepts claims on
   exactly the channels it wrote down.

   A resolution that **fails** — an unreachable endpoint rather than an absent channel — refuses
   the claim under a third, separate reason. It never degrades to accepting the claim, and it is
   never reported as "no such channel": an operator has to be able to tell an outage from a sender
   naming channels at random, and a legitimate payer has to be told to retry rather than told they
   do not exist. A resolution the connector **declined to perform**, because its budget for lookups
   that do not resolve is spent, is a fourth reason again — see "A lookup that resolves nothing must
   be bounded too" below.

5. **Collateral binding** — the claim's cumulative `transferredAmount` MUST NOT exceed the
   **on-chain deposit of the channel's counterparty**
   ([issue #646](https://github.com/toon-protocol/connector/issues/646)). This is not a credit
   policy a connector invents: both settlement contracts already refuse an over-deposit claim at
   redemption (`TokenNetwork.claimFromChannel` reverts `InsufficientChannelBalance`;
   `packages/solana-program`'s claim handler returns `TransferredAmountExceedsDeposit`), so a claim
   above the deposit is not value at risk — it is provably unredeemable, and serving it is work the
   operator can never be paid for. Evaluating it here makes the accept rule agree with the redeem
   rule. It is checked **after** cryptographic verification, so only a claim that is already fresh,
   value-covering and correctly signed can provoke the chain read it may need.

   The refusal is its own reason, distinguishable from an underpayment: this claim _does_ cover the
   route's price, and it consumes nothing — no watermark advances and nothing is recorded — so the
   remedy is the one both contracts already document: **deposit more and resubmit the same claim,
   at the same nonce**.

   Deposits are monotonically non-decreasing while a channel is open or closed on both chains
   (`setTotalDeposit` reverts on a decrease; the Solana `Deposit` handler only `checked_add`s), so a
   deposit a connector read earlier is a permanent _lower bound_. A connector MAY therefore cache it
   and compare against the cached value, provided a claim that breaches it triggers a fresh read
   before being refused — the bound can only ever produce a false refusal, never a false accept, and
   one re-read repairs it.

   **The exemption is deliberate.** A `[[client_channels]]` record declares a counterparty and a
   signing domain and never an amount, and a node with no settlement backend has no chain to ask, so
   a declared channel is not subject to this step. An operator hand-declaring a channel _is_ the
   credit decision, correctly located in config and theirs to make; an anonymous buyer resolved from
   chain (§1.2) never made any such deal, and gets the check.

   **The ceiling nets a channel's own outbound payout ledger too** (issue #700,
   `toon-meta#262` decision 9). A connector that has separately signed the channel's counterparty a
   payout claim — for example, crediting an agent for factory work it completed, per §1.9 step 6's
   TRANSFER — raises this ceiling by that amount: the bound this step checks a claim's cumulative
   `transferredAmount` against is `deposit + credited`, not `deposit` alone, where `credited` is the
   running total this connector has committed to pay the same channel's counterparty back
   (`ClientPayoutLedger::credited`), not merely what is still unacknowledged. This is what makes
   decision 9's promise literal — an agent that has earned enough spends against its own earnings
   directly, with no on-chain round trip and no settlement — and it is a bounded, deliberate
   extension of trust rather than a new on-chain fact: `credited` is this connector's own signed IOU,
   redeemable against this connector's own deposit on the same channel, never the counterparty's.
   `credited` is read once, before this step's own chain-refresh read, so a payout recorded while an
   admission is already in flight cannot retroactively rescue it — the same "false refusal only,
   never a false accept" property the cached deposit above already has, since a payout ledger's
   running total is monotonic for exactly the same reason a deposit is. A channel with no payout
   ledger configured nets `0`, exactly this step's behaviour before issue #700. Netting is
   per-channel and never crosses a chain: a channel's `credited` figure comes from its own recorded
   payout ledger entry, the same one issue #629 already keys by chain for the deposit side.

A claim that fails any check is a validation failure and the PREPARE is rejected before it
reaches the terminating app or advances any watermark.

**A resolved channel's mutable facts expire.** The counterparty and signing domain a resolution
reports are immutable on chain, but the same resolution also asserts two things that are not — that
the channel has not `Settled`, and that its token/mint is the one this node settles in
([issue #649](https://github.com/toon-protocol/connector/issues/649)). A connector that memoises a
resolution therefore MUST re-verify it periodically, or a channel resolved while open and settled
afterwards keeps buying writes for the life of the process, with the settled-channel refusal
(step 4) silently bypassed. Re-verification and the deposit re-read above are one mechanism: a
refresh reports liveness and deposit together.

A **declared** channel is outside this too, and for the same reason it is outside the cap: config,
not the chain, is its authority, and a node with no settlement backend has no chain to ask. The
consequence is worth stating plainly rather than leaving to be discovered — an operator who both
runs a settlement backend **and** hand-declares a channel in `[[client_channels]]` gets a channel
that is exempt from the deposit cap _and_ never re-verified, so it keeps being paid on after it
settles on chain. That is the same credit decision the exemption above describes, extended in time:
declaring a channel says "I vouch for this one", and a connector cannot both take that at its word
and second-guess it. An operator who wants the chain consulted should not declare the channel — a
node with a settlement backend resolves it anyway (§1.2), which is the path both this step and the
cap apply to.

**Re-verification must not become a per-packet read.** The expiry above is a bound on staleness, and
a connector that implements it naively converts it into a bound on nothing: if a re-verification
that _fails_ leaves the entry expired and refuses the claim, then every subsequent packet on that
channel retries the same failing read, so an unreachable or rate-limited endpoint turns one read per
interval into one read per packet — a load pattern that sustains its own failure, on the endpoint
already failing. A connector MUST therefore bound the work as well as the staleness: it SHOULD serve
the last successfully-read resolution while a re-verification is failing, up to a hard staleness
ceiling past which it refuses, and it MUST NOT allow one channel to provoke unbounded lookups —
neither by arrival rate (many packets, one aged-out entry) nor by concurrency (many packets at once)
nor by resubmission (one undercollateralized claim, re-presented, which by design consumes nothing).
Serving a stale resolution is a deliberate, logged degradation, and it is bounded: it is strictly
better than refusing a paying client because a third party's endpoint is down, and the ceiling keeps
it far inside the close-challenge-settle window the expiry defends against.

The bound on work MUST hold **past the staleness ceiling too**, and this is the part that is easy to
get wrong in the direction of good intentions. Past the ceiling there is nothing left to serve, so it
reads as the moment to try hardest — but reaching it means the chain has already been failing for the
entire stale window, and a connector that waives its own rate limit there reinstates exactly the
per-packet storm described above, merely later, against an endpoint that has by then been failing for
the whole window. The claim is refused either way once nothing can be served; the only question is
whether each refusal also costs a chain read, and it must not. A connector that has to refuse SHOULD
say which refusal it is — "the chain answered and said no" is an operator's problem to fix, "I am
backing off from asking" is the same operator's endpoint already being known-bad — since the two lead
to different actions.

The three durations this implies (when a reading stops being believed, how long past that it may
still be served, and how often one channel may provoke a lookup) are a **deployment** choice, not a
protocol constant: a node on a metered or rate-limited endpoint needs them longer, and a node that
wants a settled channel noticed sooner needs the first shorter. A connector SHOULD make them
configurable and SHOULD refuse, at load, values that read as strictness but behave as a per-packet
read — a zero re-verification interval, or a zero floor on lookups per channel.

**A lookup that resolves nothing must be bounded too, and none of the above bounds it.**
Every bound in the paragraphs above is keyed to a channel the connector has resolved at least once —
it is an interval on _that entry_, a stale window measured from _that reading_. A channel that never
resolves has no entry, so a sender naming nonexistent channel ids provokes one chain read per
request, indefinitely ([issue #613](https://github.com/toon-protocol/connector/issues/613)). The gap
is wider than "a fresh id each time escapes a per-channel interval": **even the same nonexistent id,
repeated, escapes it**, because the entry an interval would be recorded on is never created. Every
one of those claims is refused, nothing is paid and nothing is delivered — which is what makes it
worth doing: the sender spends a packet, and the connector spends a unit of its own metered
settlement-RPC budget, on an anonymous request's say-so. A connector MUST therefore bound how many
lookups that do not resolve it will perform.

**The bound MUST NOT be a negative cache, and it MUST NOT be a plain ceiling either.** Both are
worse than the problem, and the second is the subtler one.

Remembering "no such channel" for a while breaks the exact buyer §1.2's registration-free path exists
for — the one who opens a channel and writes a second later, whose own first attempt would then
poison the next N seconds of their own attempts. A connector MUST NOT memoise a negative answer; the
thing that is metered is the _asking_, not the answer.

Refusing outright once a ceiling of _C_ lookups per window is reached breaks the same buyer by a
different road, and breaks them harder. It hands any sender able to sustain _C_ requests per window a
switch that turns §1.2 off for **every** new buyer, for as long as they hold it down — needing no
keypair, no valid signature (this step precedes step 4), and no funds. Set the two failure modes side
by side: with no bound at all, a flooder costs the connector one chain read per request **and the
feature keeps working**; with a dropping bound, the same flooder costs the connector nothing and the
feature is entirely off. A connector's overflow behaviour SHOULD therefore be to **hold the lookup
for a slot** — a leaky bucket, drained at the configured rate — and to refuse only a lookup whose
slot is further out than a bounded wait it will hold one for. The chain sees the configured rate,
which is the only thing the bound was ever for; a legitimate buyer arriving during a flood is
delayed rather than denied, and a client that retries gets through.

Stated precisely, since it is the figure an operator sizes an endpoint against: the **sustained**
rate is the configured one, and any single window may see up to the burst _plus_ a window's drain —
roughly twice it — when a flood arrives at an idle connector. That is inherent to tolerating a burst
at all, and a connector SHOULD document it rather than quote the sustained figure alone.

Three further properties follow, and each is a way of keeping the intended user working:

- **A lookup that resolves the channel MUST NOT count against the bound.** Otherwise a connector
  onboarding real anonymous buyers throttles itself for doing the thing the path is for. Claiming a
  slot before the chain is read (which is necessary — the point is to prevent the read, not to notice
  it afterwards) and returning it on a resolution satisfies this.
- **A lookup that _fails_ MAY count**, since the request was spent either way and an endpoint that is
  down must not keep being paid to say so. But a connector MUST NOT then report the resulting
  refusals as rate-limiting: a failing endpoint saturates the drain within seconds, so a connector
  that reported the saturation would tell its operator they were being walked when in fact their RPC
  is dead. While the last lookup a connector actually completed came back a failure, that failure is
  what its refusals SHOULD report.
- **Exhaustion MUST be its own refusal**, distinct from both "no such channel" and "the lookup
  failed", and it SHOULD be **temporary** rather than final — nothing is wrong with the claim, and a
  sender told otherwise would stop rather than retry. So SHOULD a failed lookup be, for the same
  reason and with more force: an unreachable endpoint is the connector's problem and not the claim's.
  The three refusals lead an operator to three different actions (nothing; fix the endpoint; look at
  who is saturating the drain), so reporting any of them as another sends somebody to fix the wrong
  thing.

**What identity such a bound is keyed to is genuinely hard, and a connector SHOULD be honest about
what it buys.** A probe (§1.6) is budgeted per recognized channel; a lookup that does not resolve has
no recognized channel by definition. The transport source address is the obvious fallback and is
worth little: a connector deployed behind a reverse proxy sees the proxy's address, so every
anonymous buyer shares one bucket with the attacker, and the remedy — trusting a forwarded-for header
— is trusting attacker-supplied text. The claim's own declared signer is available before any lookup
and costs nothing to read, but it is **not a credential**: for EVM the EIP-712 digest needs the
channel's own domain, which is precisely what has not been resolved yet, so nothing about the
declared signer can be verified at this point without either trusting the claim's self-declared
domain (which proves only that the sender can run one `ecrecover`) or spending elliptic-curve work on
every anonymous request — trading an RPC-spend amplifier for a CPU-spend one.

A connector that shapes per declared signer therefore MUST NOT present it as a bound: a keypair is
free, so an adaptive sender declares a fresh one per request. What the per-signer axis buys is that a
sender must _become_ adaptive — a flooder rotating a handful of identities is held to the per-signer
rate on each, so saturating the node-wide drain at all takes `total / per_signer` distinct declared
signers, sustained, which is loud in a log and reachable by the per-address limiter below. A
connector MUST also keep a **node-wide** rate, which is the only part an adaptive sender cannot route
around.

One hazard follows from the identity being unverified, and a connector SHOULD design it out rather
than document it: because anyone may declare anyone's address, a per-signer bound enforced
unconditionally is a cheap targeted denial of service against a _known_ buyer. Consulting the
per-signer axis only once the node-wide drain is genuinely in arrears **prices** that attack at a
whole node-wide burst before the first aimed request bites, and means an idle connector never refuses
anyone for their declared identity. It does not _remove_ the aim, and a connector SHOULD say so
plainly rather than claiming otherwise: a sender who sustains the flood can still spend a named
buyer's share, at which point it is the flood, not the aim, that an operator is looking at.

**Neither axis is a durable answer, and the durable answers live outside this step.** Two are worth
naming, because a reader who has followed the paragraphs above should not conclude that a declared
signer is the best that can be done:

- **Per-address rate limiting at the reverse proxy** a connector is deployed behind. That is the only
  sybil-resistant axis available at this layer — an address costs something, a keypair does not — and
  it is the right place for it, since the proxy is the only component that sees the real peer.
- **A local channel index built from the settlement contract's own logs** (issue #661, shipped as
  `connector-settlement-evm`'s `EvmChannelIndex`/`EvmChannelIndexSyncer`, wired in by
  `connector-cli::runtime`'s `IndexedEvmChannelSource`). A connector backfills, then polls,
  `ChannelOpened`/`ChannelNewDeposit`/`ChannelSettled` from `TokenNetwork` (the close events change
  nothing about claimability — `claimFromChannel` accepts a `Closed` channel — so they are not indexed)
  once each log is a configured confirmation depth behind chain head, and answers "is this a channel
  I can be paid on, and for how much?" from a local map rather than an RPC round trip. Once the index
  has caught up to a channel, an unknown- or settled-channel lookup costs a hashmap probe instead of
  an `eth_call` and this step's rates and wait have nothing left to bound _for that channel_ — that is
  the fix that dissolves the problem rather than rationing it. It is not a replacement for the axes
  above: a channel the index has not caught up to yet (never opened, opened inside the confirmation
  window, or the index's own sync lagging or down) falls through to exactly the RPC-reading,
  budget-shaped resolution this section describes, unchanged — so a connector whose index has never
  once caught up behaves exactly as one with no index at all, and the rates and wait above remain the
  bound for every lookup this index cannot yet answer. Since issue #1151 one further answer falls
  through: an indexed, open channel whose counterparty **deposit reads zero**. The index reports zero
  both for a channel that holds nothing and for one whose `ChannelNewDeposit` is younger than the
  confirmation depth, and it can never separate the two — it is permanently that many blocks behind
  head — so "for how much?" is asked of the chain in that case rather than answered from the map. It
  is deliberately **not** answered with the declared-channel exemption (`DepositFloor::Unknown`,
  `depositTotal: null`): that exemption covers every claim, and a channel opened and never funded
  would become an unlimited line of credit no `claimFromChannel` could ever redeem. A resolved
  channel still always reports a number; the number is now the chain's.

The rates and the wait are a **deployment** choice for the same reason the three durations above are
— what a connector can afford to spend discovering channels that do not exist depends on the
settlement endpoint it pays for, and it should be derived from that endpoint's real capacity rather
than picked for tidiness. The arithmetic is worth doing rather than eyeballing: at a common metered
schedule of 26 compute units per `eth_call`, ten lookups a second sustained is 864,000 lookups and
about 22.5M CU a day — over a 300M/month allowance in a fortnight, on discovery traffic alone and
before the connector's own settlement work. **An operator on a metered endpoint should therefore set
this rate well below what a self-hosted one would carry**, and lowering it costs an honest buyer
nothing, since a lookup that resolves returns its slot.

A connector SHOULD make them configurable and SHOULD refuse, at load: a zero rate (which switches
§1.2's path off entirely, silently, under a number that reads as a tightening); a zero window (which
makes every rate infinite and the bound nothing); a zero wait (which converts the shaper back into
the dropper this section rejects); and a wait longer than the window — the wait is not a timeout but
the **size of the waiting room**, since a room drained at the configured rate and holding a lookup
for that long parks more than a whole window's worth of them, which is more memory than the bound is
worth and a delay no packet's own deadline would survive.

**A watermark outlives the process.** Freshness (step 2) is only a replay defence if the watermark
it compares against survives a restart: a connector that forgets a channel's watermark compares
against nothing, and `None` accepts every nonce, so every claim the client already spent becomes
free service again ([issue
#605](https://github.com/toon-protocol/connector/issues/605)). A connector therefore MUST record
each accepted claim durably before treating it as accepted, and MUST rebuild its watermarks from
that record before serving. Two consequences follow, and both are refusals rather than degradations:
a claim whose acceptance cannot be made durable is refused (as a **temporary** error — the claim
itself is fine), and a record that cannot be read back, or that carries an entry the connector
cannot decode, stops the connector starting rather than letting it start at no watermarks. Where
the record lives is a deployment question rather than a wire one: today it is the `state_dir`
config field, and a config that configures `[[client_channels]]` without one does not load.

**Mina is not a supported chain.** [ADR 0002](../adr/0002-drop-mina-from-the-rust-connector.md)
drops Mina from the Rust connector: a Mina claim's on-chain lifecycle (open, deposit, close,
settle) has no Rust implementation and none is planned, so a connector that accepted a Mina claim
would be accepting value it can never settle. `blockchain: 'mina'` is therefore refused as a
structural validation failure (step 1 above) rather than parsed or cryptographically checked — the
zkApp-specific fields the peer semantics's predecessor once carried for it (`zkAppAddress`, `tokenId`,
`balanceCommitment`, `proof`, `salt`, and the dual-party `balanceB`/`signatureB` extension) are not
part of this connector's claim shape and are not documented here. A Mina client's claim is rejected
clearly and immediately; it is not owed a code path, only an unambiguous refusal.

### 1.4 Answering an unpaid request: x402 v2 terms

An unpaid request — no claim header of either kind — addressing a route this connector both
serves and prices is answered `402` with that route's terms instead of being routed at all
([issue #526](https://github.com/toon-protocol/connector/issues/526), [ADR
0022](../adr/0022-a-connector-answers-it-does-not-announce.md)): the app behind a priced route is
never asked to do free work for an anonymous, unpaying caller. A present claim header (valid or
not) suppresses this response unconditionally — its validation is §1.3's job, not this section's
— and an unpaid request to an unpriced or unmatched destination falls through unchanged, exactly
as it always has, **unless the PREPARE itself declares `greeting`**.

**A claimless, greeting-flagged PREPARE is answered this way regardless of destination** ([issue
#807](https://github.com/toon-protocol/connector/issues/807)). `Prepare` carries no execution
condition at all since [ADR 0069](../adr/0069-the-execution-condition-leaves-the-wire.md) (issue
#1269) — until then, this section identified a bootstrap probe by a missing or all-zero condition,
which issue #417's `reject_ineligible` refused outright before any route was selected; that rule
and the field it checked are both gone, and the probe is now identified by its own explicit
`greeting` flag instead, whatever it addresses
(`packages/announcer/src/edge-client.ts`'s `fetchGreeting` builds exactly this shape: zero
amount, `greeting: true`). Answering it the same way lets a client whose genesis peer seed is
stale or missing learn this node's settlement facts by asking the edge directly, rather than
needing a `[[routes]]`-matching, priced destination to probe with — which is precisely what such
a client does not have. This is still an answer, not an announce: nothing is sent unless this
connector is asked, over the connection that asked it. A present claim header still suppresses the
response unconditionally, whatever `greeting` says — a claim-bearing, greeting-flagged PREPARE is
never distinguished from an ordinary one and is routed exactly like any other claimed request,
which is what keeps the flag from ever being usable to get a packet routed, priced or delivered
for free.

**"Serves" spans both kinds of configured route** ([ADR
0028](../adr/0028-a-forwarded-route-is-priced-at-the-client-edge.md), [issue
#620](https://github.com/toon-protocol/connector/issues/620)): one that terminates here, whose
`price` buys the app's work, and one that forwards over a peering, whose `price` buys the whole
path and out of which this hop retains its `fee`. The terms are byte-identical either way —
nothing in the shape below names a route kind, and a client cannot tell, and has no reason to
care, which one it is paying. Before ADR 0028 a forwarded destination was greeted with nothing,
required no claim and was carried for free; that was a free gateway, not a design.

Two rules attach to the forwarded case and to nothing else. A client-edge PREPARE to a priced
forwarded destination is refused `F03_INVALID_AMOUNT` when its declared `amount` exceeds that
`price` — this connector never puts more value on the peer semantics than it collected, and the
refusal is decided before the claim is ingested so a packet that will not be carried never spends
a watermark. And a _peer-role_ PREPARE is never answered with this greeting at all
(`peer-carriage-spec.md` §3.1): everything in this section is the client-facing direction.

This is **answering, not announcing** ([ADR 0022](../adr/0022-a-connector-answers-it-does-not-announce.md)):
a reply to the request that asked, changing no state and reaching nobody who did not ask. [ADR
0006](../adr/0006-the-connector-is-mechanism-not-policy.md) rules out the connector pushing facts
about itself into a network unprompted — a genuine greeting, sent before anyone asked — which is
not what this is; an earlier draft of this section described the same status code as exactly that
unprompted greeting, and _that_ stays removed. [ADR
0011](../adr/0011-rejects-accumulate-fees-and-probes-discover-cost.md)'s "neither is reinstated"
was written against that same earlier, unprompted shape.

The body is an x402 v2 `PaymentRequired` document — `Content-Type: application/json` — repeated
byte-for-byte, base64-encoded, in a `Payment-Required` response header:

```json
{
  "x402Version": 2,
  "resource": { "url": "g.example.app" },
  "request": { "protocol": "nip90", "kinds": [5096, 5098] },
  "accepts": [
    {
      "scheme": "toon-channel",
      "network": "g.example.app",
      "amount": "100",
      "payTo": "g.example.app",
      "maxTimeoutSeconds": 60,
      "httpEndpoint": "/ilp",
      "extra": {
        "ilpAddress": "g.example.app",
        "endpoint": "/ilp",
        "price": "100",
        "sessionLeaseTtlMs": 120000
      }
    }
  ]
}
```

`accepts` is a list — ADR 0022 notes terms are plural — but exactly one entry exists today, for
the one payment method this client edge's own claim gate (§1.3) actually understands: a TOON
payment channel claim, presented back over this same `POST /ilp`. There is no per-chain `exact`
scheme entry naming a settlement `asset`/`payTo` address, for EVM, Solana or any other chain,
because this claim gate understands one payment method and an `exact` scheme entry would describe a
second. **Not** because settlement facts are unavailable — they have been in the greeting's `extra`
since issue #617, and are per-chain since #632 (corrected, issue #1073). `extra` is limited to
what the code actually sets — `ilpAddress`, `endpoint`, `price` and `sessionLeaseTtlMs` on every
greeting, plus `pricePerKib` where the addressed route prices by size, plus whichever of
`ilpAddresses`/`btpEndpoint`/`settlement`/`settlements`/`requiredTransport` this node has
configured (below) — and carries nothing else.

**`request`** ([issue #1210](https://github.com/toon-protocol/connector/issues/1210), [ADR
0067](../adr/0067-a-route-declares-its-request-shape-and-the-connector-never-reads-it.md)) — a
top-level member, present exactly when the addressed route's `[[routes]] request` table is
configured, absent (not `null`) otherwise. It sits **beside** `resource`, not inside `accepts[]`:
it describes what a client should send to use the addressed route — the resource itself — not a
property of one particular payment method, and applies whichever entry in `accepts[]` a payer ends
up satisfying. The value is the operator's table converted to JSON verbatim; this connector never
reads a key out of it, never fetches it from anywhere, and never varies its behaviour on what it
contains. A reader that predates this field ignores it, the same as any other addition to this
forgiving-deserialization document.

**`extra.ilpAddresses`/`extra.btpEndpoint`** ([issue
#807](https://github.com/toon-protocol/connector/issues/807)): this node's own ILP address(es) and
BTP endpoint — the same facts a kind:10032 announce carries under the identical names — present
exactly when `[announce]` is configured (`connector_config::AnnounceConfig`, the section
`connector announce` already reads these from), absent — not empty/null — otherwise. Unlike
`extra.ilpAddress` above, which echoes back whatever `destination` the probing PREPARE named,
these are this node's own authoritative facts regardless of what was probed; a client that trusts
`ilpAddress` as a confirmation of a _guessed_ destination should prefer `ilpAddresses` when it is
present, since the guess is exactly what a stale-or-missing-genesis-seed client cannot rely on.

`amount` and `extra.price` are read from the same longest-prefix route lookup that §1.3's value
binding and §1.7's `GET /ilp/routes/price` charge and answer against, so this response never states
a price a real request wouldn't also be charged.

They are **equal for a flat route, and that is every route that predates
[ADR 0065](../adr/0065-a-price-is-a-schedule-over-payload-length.md)**. Where a route's price
carries a slope the two answer different questions and must not be conflated:

- **`amount`** is what the request being answered would cost — the route's schedule evaluated at
  that request's own payload length. This is x402's meaning of the field: what to pay for _this_.
- **`extra.price`** is the schedule's **base**, and **`extra.pricePerKib`** its slope, in the same
  decimal-string spelling. Together they let a client compute the cost of a packet it has not sent
  yet, which is what keeps [ADR 0011](../adr/0011-rejects-accumulate-fees-and-probes-discover-cost.md)'s
  cacheability true: one greeting answers every size, rather than one greeting per size.

`extra.pricePerKib` is **absent**, not `"0"`, on a flat route, so a flat route's greeting is
byte-identical to what it was before schedules existed and a parser written against that greeting
is unaffected.

**Transport policy** (issue #701, `toon-meta#262` decision 11): which transport(s) a terminated
route accepts is per-connector config, not a protocol constant — `both` by default, so no deployed
route changes behavior until an operator opts in, or restricted to `http` or `btp` alone. A request
over a transport its route does not accept is refused with this SAME `402` shape — before payment
is considered at all, and whether or not the request carries a valid claim, since paying over the
wrong transport does not make the route reachable that way — with one addition: `extra` also
carries `requiredTransport` (`"http"` or `"btp"`), naming the transport the route actually
requires. An ordinary unpaid-request greeting (above) never sets this field. The BTP carriage
answers the mirror case (a route restricted to HTTP, reached over the websocket session) the same
way; see §1.9 step 3.

**`extra.sessionLeaseTtlMs`** ([issue #722](https://github.com/toon-protocol/connector/issues/722),
`toon-meta#262` decision 12): unlike every other `extra` field above, always present, on every
greeting this connector answers — a settlement-less node still has a client session registry. The
value is
[`connector_client_edge::session_registry::SESSION_LEASE_BACKSTOP_TTL`](../../crates/connector-client-edge/src/session_registry.rs)
in milliseconds, the same constant §1.9's "Session registry: the socket is the lease" section
describes — never a second literal typed nearby, so this field cannot drift from what the registry
actually enforces. It exists because that constant is a Rust `pub const`: nothing outside this
crate, and nothing outside Rust at all, has any path to read it except off the wire. A consumer in
any language — `buzz#84`'s relay-side provider-freshness window among them — reads this field
instead of hardcoding a guessed millisecond count, satisfying the cross-plane invariant that
freshness must never exceed this connector's own lease. Wiring `buzz#84`'s
`providerAvailability.ts` to read this field is left to a follow-up in the `buzz` repository; this
connector's obligation is that the value is on the wire and provably tied to the enforced constant
(pinned by a same-crate test), not that every consumer has been updated yet.

### 1.5 Request-request binding — decided against

**Not implemented, and not going to be.** No `requireRequestBinding` config field,
`RouteTermination` type or RFC 9421 verification of a client's request exists anywhere in `crates/`,
and none is planned — the RFC 9421 verification `connector-operator` does carry is the operator
surface's write authentication ([ADR 0008](../adr/0008-operator-surface-splits-read-from-write.md)),
a different mechanism on a different surface. This section previously specified an intended design — an RFC 9421 HTTP Message Signature over the
inner envelope, an RFC 9530 `Content-Digest`, and a `TOON-Price` header compared byte-exact against
the route's price, verified by the terminating connector before proxying to the app.
[ADR 0035](../adr/0035-request-request-binding-ships-no-new-mechanism.md) decided against building
it: the threat it targeted — a captured claim replayed against different work or a cheaper route —
is already closed, partly by construction (a payload is sealed to the terminating connector,
[ADR 0018](../adr/0018-a-payload-is-sealed-to-the-terminating-connector.md); a packet's condition is
already bound to a secret only that connector can open,
[ADR 0019](../adr/0019-a-terminating-connector-derives-the-fulfilment.md)) and partly by §1.3's
existing claim gate, whose watermark (step 2) and value binding (step 3) refuse a replayed or
underpaying claim before the app is ever contacted. The party a binding mechanism would need to
verify it is the terminating connector itself — the one party ADR 0035 finds it structurally cannot
defend against, since that party also controls whether the check runs at all. See ADR 0035 for the
full analysis, including the parties who do remain and why binding would not have defended against
them either.

### 1.6 Probing for cost

Implemented as of [issue #548](https://github.com/toon-protocol/connector/issues/548). The
connector no longer "charges a percentage spread with no per-hop fee accumulation" — there is no
percentage anywhere ([ADR 0010](../adr/0010-flat-per-packet-fee-and-minimum-delivery.md)), and a
REJECT genuinely does accumulate cost: `connector_domain::Reject` carries an `accumulated_cost`
field that sums every hop's flat fee and adds a terminated route's price
(`docs/protocol/peer-semantics-pre-868.md` §5.2, issues #523/#545/#584). That field is **not** part of the
RFC-0027 OER encoding — it rides beside the packet — so this edge reports it in a header. Version 1
does not change to gain it: the request/response shape below is unchanged.

**The header.** A client MAY send an ordinary PREPARE it expects to be rejected (a probe,
`CONTEXT.md` "Probe") to learn a path's cost. RFC-0027's REJECT `data` is reserved for an
application-level reject's own diagnostic payload (an `F99`/`T99`/`R99` from the terminating app),
so `accumulatedCost` MUST NOT be packed into it; instead the connector returns it as a response
header, `TOON-Accumulated-Cost` (decimal string, `uint64`), alongside the unchanged OER REJECT body
— the client-edge equivalent of the peer semantics carrying the field at the frame level, beside the
packet, rather than inside it. The header is present on every REJECT response this edge answers
with, from `POST /ilp` and `POST /ilp/probe` alike, and is absent from a FULFILL. It is `0` when
nothing was traversed and nothing terminated — no route matched, or a claim was refused as
malformed, stale or unverifiable — and otherwise reports one figure: the flat fee of every hop the
packet actually reached, plus the price of the route it terminated at. Never a breakdown, and never
a fee-versus-price split; ADR 0011's "returning a sum leaks nothing" is a property of the sum
alone.

A claim refused for **underpayment** is the one refusal that reports a non-zero figure: the route's
price. That refusal's whole subject is a figure the sender did not cover, and before #548 the only
channel through which a price was ever disclosed was that reject's human-readable `message` — so a
client learned a price by underpaying first, which is precisely what cost discovery exists to
prevent.

**The probe ingress: `POST /ilp/probe`.** Same request body and same response framing as `POST
/ilp` (§1.1); what differs is the gate in front of it, and that nothing is charged. Because probing
traverses the network for free, a probe is accepted only from a sender identified by a payment
channel claim on a channel this connector recognizes, and only within a rate limit per that channel
(ADR 0011's two conditions). A sender with no such channel, or one over its probe rate limit, is
rejected at ingress with `403` (a status this subsection adds to §1.1's table, distinct from `401`:
the sender may be perfectly well authenticated and is simply not authorized to probe) without being
forwarded. A `403` carries no OER body, per §1.1's rule that a non-2xx status never does.

The claim on a probe **identifies rather than pays**: it is validated in full (§1.3's five steps)
against a price of `0`, so possession of the channel is proven and a replay is still refused, but
no value need advance — a sender probes by reissuing at the same cumulative amount with a fresh
nonce. A connector recognizes a channel once a claim on it has cleared §1.3's gate at this edge.
It necessarily already holds that channel's counterparty, whether declared or resolved from chain
— step 4 above verifies against it, so without one no claim on the channel could clear the gate at
all — but holding a counterparty says only
_whose signature is accepted here_, never that anyone has turned up and paid; no chain indexes
that, so a cleared claim is the only evidence a connector ever gets of it. This is what makes the
probe gate satisfiable by a deployed node: a sender able to pay is, by the same record, a sender
able to probe, and a gate no deployed node could pass would not be a gate.

A probe is never **delivered** to a route this connector terminates. Free traversal is the whole of
what ADR 0011 grants a probe; it does not also buy the work behind a priced route, which is what
delivering would hand over. A destination that terminates here is answered `F03` with that route's
price as `TOON-Accumulated-Cost` — the same figure a real request would be charged, and the whole
path cost, since no hop was traversed to reach it. A destination beyond this connector is routed
by the ordinary routing table, exactly as ADR 0011 requires: a probe is not a distinct packet type
and fee accumulation is not a special mode for it.

A probe reaching a **remote** termination learns that termination's price from the reject the
remote connector raises there — a terminating connector adds its route's price to the running total
([ADR 0020](../adr/0020-a-price-is-flat-and-attaches-to-a-handler.md) — a price accumulates into a
reject's running total; issues #545/#584) — with each hop on the way back adding its own fee, so
what arrives is one figure covering both. Note that this is the ordinary packet path: a probe is
gated at the client edge it enters, and the peer semantics carries no probe frame, so a remote
connector cannot tell a probe from any other packet and the "never delivered to a termination"
rule above applies only to the connector the probe was submitted to.

### 1.7 Answering: identity and route price

A sender must hold the terminating connector's public key before it can seal a packet to it (§1
above; [ADR 0018](../adr/0018-a-payload-is-sealed-to-the-terminating-connector.md)), and must know a
route's price before it can construct a claim that pays for it. Both are answered directly by the
connector that terminates the route, over the same client edge a payer already speaks to it on
([ADR 0022](../adr/0022-a-connector-answers-it-does-not-announce.md)) — **answering, not announcing**: each
of the following is a reply to a request that reached this connector's own client edge, changes no
state, and is never pushed into a network unprompted.

- **`GET /ilp/identity`** — unauthenticated, no request body. Returns the uncompressed secp256k1
  public key a sender must seal a packet's payload to, plus the key id identifying it:
  ```json
  { "keyId": "...", "publicKey": "0x04..." }
  ```
  Mounted under `/ilp` rather than at the bare `/identity` because the operator surface already
  serves its own bearer-gated `GET /identity` (issue #420) for a different audience — a different
  operator-authenticated caller asking a different question — and the two routers are merged onto
  one port whenever the operator surface is enabled.
- **`GET /ilp/routes/price?destination=<ILP address>`** — unauthenticated. Returns `200` with the
  price of the configured route `destination` would match — terminated or forwarded ([ADR
  0028](../adr/0028-a-forwarded-route-is-priced-at-the-client-edge.md)) — reading the same
  longest-prefix lookup the x402 terms (§1.4) and claim value binding (§1.3) charge against, so
  this never states a price a real request wouldn't also be charged:
  ```json
  { "destination": "g.example.app", "price": 100 }
  ```
  A route priced by payload length ([ADR
  0065](../adr/0065-a-price-is-a-schedule-over-payload-length.md)) answers with its slope beside
  its base, so one read still tells a caller what any packet will cost:
  ```json
  { "destination": "g.example.store", "price": 1000, "price_per_kib": 30 }
  ```
  `price_per_kib` is **omitted** on a flat route, so this answer is unchanged for every route
  that predates schedules.
  `404` when no route this connector serves matches `destination` — this endpoint never fabricates
  a price for a route it does not serve. It answered `404` for a forwarded destination before ADR
  0028, which was correct only while such a destination was also uncharged; answering it now is
  the same rule applied to a route that is charged for.

### 1.8 Sealing (issue #524)

`Prepare.data` is a gift wrap (`connector_signer::giftwrap`), not a plaintext envelope: a sender
seals a structured request envelope, plus a freshly generated shared secret, to the public key
`GET /ilp/identity` (§1.7) reports — only the connector holding the matching private key can open
it, so a forwarding hop sees opaque bytes rather than the method, target, headers or size of what
crossed it ([ADR 0018](../adr/0018-a-payload-is-sealed-to-the-terminating-connector.md)). The
terminating connector seals its answer back with that same shared secret — no second exchange — on
both `Fulfill.data` and a `Reject.data` raised at the termination; a reject raised short of the
termination (no route, expiry, a ceiling) shares no secret with the sender and stays plaintext with
empty `data`, which is how a sender tells the two apart. `accumulated_cost` is unaffected: it never
rode inside `data` to begin with (§1.6), so nothing here changes how it travels. The fulfilment a
terminating connector derives from that shared secret (ADR 0019) is likewise part of this wire.
`vectors/wire-vectors.json`'s `envelope`, `giftwrap` and `fulfilment` sections are the reproducible
bytes for all of the above — this paragraph is orientation, not the thing to conform to.

The envelope's `target` is resolved strictly _beneath_ the terminated route's own configured
handler path, never in place of it
([ADR 0025](../adr/0025-an-envelope-target-is-confined-beneath-the-handler-path.md), issue #596):
`""` and `"/"` both address the handler's own path, and any other value naming an absolute path, a
`..`/`.` segment, a scheme, an authority, or a percent-encoded equivalent of any of those is refused
(`F00`) before the app is ever called, rather than delivered. This is what keeps ADR 0020's "one
handler, one price" true in the presence of a sender-chosen `target` — a route's configured handler
is the one thing a sender's own envelope can never override.

**A terminating connector tells the app about the payment it verified itself, and about no other**
([ADR 0040](../adr/0040-a-verified-payment-is-stated-to-the-app.md), superseding
[ADR 0036](../adr/0036-a-paid-deliverys-attribution-stays-on-the-connector.md)'s conclusion). Beyond the
opened envelope's own `method`, `target`, `headers` and `body`, the request made to the route's
`handler_url` carries three connector-stated headers:

| Header          | Value                                                                                                         |
| --------------- | ------------------------------------------------------------------------------------------------------------- |
| `X-TOON-Payer`  | the client channel key a covering claim was admitted under — `evm:0x<64 lower-case hex>` or `solana:<base58>` |
| `X-TOON-Amount` | the route's flat `price` (ADR 0020), decimal, in the settlement asset's base units                            |
| `X-TOON-Chain`  | that channel key's own namespace — `evm` or `solana`                                                          |

They are stated **only** for a delivery this connector was paid for at its own client edge: a
packet no client claim admitted (a peer-role arrival, a forwarded packet, an unclaimed request) or
a route priced at zero carries none of the three, absent rather than empty. `X-TOON-Payer` is
therefore never the previous hop — on a longer path there is no client channel to name, and
nothing is named (ADR 0017's defect, made unreachable rather than merely avoided). `X-TOON-Chain`
comes from the verified claim, never from the destination address; `X-TOON-Amount` is the price
this connector charged, never the arriving packet's sender-declared `amount`.

A sender's own spelling of those three names, inside the sealed envelope, is **removed on every
delivery** — including the deliveries that then state nothing. An app reading `X-TOON-Payer` is
reading the connector or reading nothing, and MUST treat all three as optional rather than
inferring "unpaid" from their absence: whatever reaches a handler was paid at that handler's one
price (ADR 0020), whether or not this hop was the one that took the payment.

The connector's own records are unchanged and remain the after-the-fact answer: an operator joins
the `"packet"` log (ADR 0014, `client_channel_id`) to `state_dir/client-edge-claims.log`'s
`InboundClaimAccepted` entries under that same chain-namespaced channel key, with the payer's
identity from the channel's `[[client_channels]]`/chain-resolved record (ADR 0036).
(`GET /channels`/`GET /claims` do not carry this: both project the node's own peer channels,
which a payer-opened client channel is not.)

### 1.9 Client BTP websocket transport (issue #674 family)

A second carriage for exactly the pipeline §1.1–§1.6 specify over HTTP: one persistent,
**ordered** websocket session carrying BTP-framed ILP packets and claims, so that a client
streaming many paid writes advances its claim nonces on one socket in one order instead of racing
parallel HTTP requests. Nothing here changes what is validated or charged — the same claim gate
instance, watermarks, journal and refusal taxonomy serve both carriages, and a write that arrived
over BTP is indistinguishable downstream from one that arrived over HTTP.

**Peer sessions (ADR 0027).** This section previously stated that peers do not use this transport,
so every BTP session was a client session by construction (ADR 0026). ADR 0027 reverses that: the
raw-TCP transport is deleted and connectors peer over BTP on the same codec. A session is a **peer**
session only while a frame it carries presents a claim on a channel one of that peering's
`[[peer_channels]]` rows configures, whose signature verifies against the counterparty key that row
configures ([ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md),
issue #1157; `peer-carriage-spec.md` §1.2); anything else is a client session, with no fallthrough,
and everything below in this section describes client sessions exactly as before. **There is no
peering credential**, and nothing replaced it: opening this transport is permissionless, a session
that proves no peering is accepted and stays a client, and role attaches to a frame's own evidence
rather than to the session's greeting (see step 1 below for what a client's `auth` entry is and is
not). The peer sub-protocol entries — `claim-ack` beside the `payment-channel-claim` and
`toon-accumulated-cost` entries this section already defines — are specified for the peer
direction, not here. (`toon-minimum-delivery` was named here too; it is
retired with the field, [ADR 0057](../adr/0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md).)

> **Superseded** by [ADR 0027](../adr/0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md):
> the raw-TCP transport is deleted (issue #679) and peers ride this same carriage, or
> ILP-over-HTTP. A session is a _peer_ session if and only if it presented a configured peer
> credential **and** has a `[[peer_channels]]` entry — role by authentication, not by transport
> or port. (_The credential half was deleted by
> [ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md) (issue
> #1157) and not replaced: role is the `[[peer_channels]]` binding plus a verified claim on one of
> that peering's channels, decided per frame. ADR 0027's point here — role by authentication, not
> by transport or port — is unchanged._) "Every BTP session is a client session by construction"
> no longer holds, and the classification it replaces is code, which is why it is a named
> stop-ship regression test on both carriages. Everything else in this section — one gate, one
> journal, one refusal taxonomy, indistinguishable downstream — is what ADR 0027 extends to peers
> rather than changes. ADR 0026's carriage architecture stands; only its peer conclusion is superseded.

- **Method/path:** `GET /ilp/btp`, websocket upgrade. The `btp` subprotocol is selected when
  offered; an upgrade offering no subprotocol is accepted identically.
- **Frames:** binary websocket messages, one BTP frame per message. Text frames are ignored.

**BTP frame layout** (all integers big-endian; this is the `@toon-protocol/client`
`btp/protocol.ts` dialect, which is the deployed client wire, extended additively with RFC-23's
TRANSFER as of issue #697 — the ILP packet still rides beside the protocolData list, not inside
it, which is the one respect in which this remains not RFC-23's grammar verbatim):

```
frame        = type(u8) requestId(u32) body
body         = pdCount(u8) pd* ilpLen(u32) ilpPacket[ilpLen]    ; type MESSAGE(6) / RESPONSE(1)
transferBody = amount(u64) pdCount(u8) pd*                      ; type TRANSFER(7) -- no ilpPacket
pd           = nameLen(u8) name[nameLen] contentType(u16) dataLen(u32) data[dataLen]
errorBody    = codeLen(u8) code nameLen(u8) name taLen(u8) triggeredAt dataLen(u32) data
                                                                 ; type ERROR(2)
```

The ILP packets themselves are the same OER encodings `POST /ilp` carries (§1.1): a MESSAGE's
`ilpPacket` is a PREPARE, a RESPONSE's is a FULFILL or REJECT. `requestId` correlates a RESPONSE
or ERROR to the MESSAGE or TRANSFER it answers.

**Symmetric grammar (RFC-23, issue #697):** after auth, either side may originate a MESSAGE or a
TRANSFER — this connector's own outbound requestId allocator guarantees the RFC's uniqueness
property ("duplicate IDs are never in-flight at the same time") for whatever it originates, exactly
as the deployed client's own allocator does for its own ids; the two id spaces are independent, so
neither side needs to know what the other has chosen. Server origination is a foundation-only
capability as of #697 — the mechanics (allocate, send, correlate the answer) are implemented and
tested (`crates/connector-btp/src/session.rs`), but nothing in this connector originates a
request yet; that is the session registry and payout-ledger work `toon-meta#262` builds on top.
Today's deployed client never sends TRANSFER and never receives a server-originated MESSAGE, and
observes no change: steps 1–5 below (all client-originated) are preserved byte-for-byte, and an
unsolicited RESPONSE/ERROR — the shape a server-originated request would eventually provoke — is
silently dropped exactly as it was before TRANSFER existed.

**Session flow, in order of what a frame carries:**

1. **Auth**: a MESSAGE whose protocolData contains an `auth` entry (JSON `{peerId, secret}`) is
   answered with an empty RESPONSE (same requestId). The contents are not verified — §1.2's
   authentication is `POST /ilp`'s only, deliberately not extended to this carriage, where an
   empty `secret` is the documented permissionless mirror and a BTP session is admitted whatever
   it presents. Authorization to _write_ comes from the claim, never the session.

   **Update (issue #698):** a non-empty `peerId` also binds this session into the client session
   registry, keyed by that value — see "Session registry: the socket is the lease" below. Binding
   is best-effort and never blocks the ack: a session with no usable `peerId` is simply never
   registered.

   **Update (issue #790):** the same `auth` entry MAY carry three additional fields —
   `channelId`, `expires` (unix seconds) and `signature` — a channel-control proof binding this
   session's `peerId` to an EVM channel _before_ it has ever presented a claim. This exists
   because issue #787's channel association is learned only from a genuine inbound claim, which a
   client that only ever earns (opens a channel, serves paid work, sends no claim of its own)
   never sends — it would otherwise be structurally uncreditable. The signature is the identical
   domain-separated `ClaimStateChallenge` `POST /ilp/claim-state` (§1.10) already verifies for a
   read, reused rather than a claim's own balance-proof scheme, so a captured claim-state proof
   and a captured claim can never stand in for each other (issue #558's rule, applied to this new
   surface too). Verified against `ClientChannelRegistry`'s already-registered counterparty for
   `channelId`, exactly as `/ilp/claim-state` verifies it, before this session is taught the
   association — a bare declaration is never trusted. Best-effort, like the `peerId` bind itself:
   an expired, malformed, unresolvable or wrongly-signed proof leaves the session exactly as
   uncreditable as it was, and `record_accepted_claim`'s inbound-claim path (issue #787) is
   untouched and stays the fallback for a session that pays before it ever declares. EVM only,
   matching the payout ledger's own reach. `vectors/wire-vectors.json`'s
   `channel_control_declaration` section is the reproducible bytes — the exact `channelId`/
   `expires`/`signature` JSON, the EIP-712 digest they cover, and a wrong-key and an expired case
   alongside the valid one — for all of the above; this paragraph is orientation, not the thing to
   conform to (ADR 0021, issue #792).

2. **Prepare + claim**: a MESSAGE with a non-empty `ilpPacket` is decoded as a PREPARE. A
   protocolData entry named `payment-channel-claim` carries the claim as **raw UTF-8 JSON**
   (`JSON.stringify(claim)` — no base64 layer; the base64 in §1.3's table is an HTTP-header
   artifact). The claim runs the SAME §1.3 pipeline and the PREPARE is then routed identically to
   `POST /ilp`; the outcome returns as a RESPONSE whose `ilpPacket` is the FULFILL or REJECT. On
   a REJECT, `accumulated_cost` (§1.6) rides as a protocolData entry named `toon-accumulated-cost`
   (decimal-uint64 UTF-8 text) beside the OER body — the BTP analogue of the HTTP header. The
   privacy-wrapped carriage (§1.3's `-Wrapped` header) has no BTP protocolData equivalent yet;
   a wrapped claim is an HTTP-only feature today.
3. **Wrong transport** (issue #701, `toon-meta#262` decision 11): a PREPARE addressed to a route
   whose per-connector transport policy does not accept BTP is refused before payment is
   considered at all — checked ahead of step 4 below, and whether or not the frame carries a
   claim, since paying over the wrong transport does not make the route reachable that way. The
   RESPONSE carries an `F02` (Unreachable) REJECT — from this carriage's own point of view, there
   is no route to the destination over BTP, even though one may exist over HTTP — with the SAME
   x402-shaped terms JSON step 4 below uses, again as a `payment-required` protocolData entry, but
   self-diagnosing via an additional `extra.requiredTransport` field (`"http"` or `"btp"`) naming
   the transport the route actually requires. This reuses §1.4's greeting mechanism rather than
   inventing a second one; the HTTP carriage answers the mirror case (a route restricted to BTP,
   reached over `POST /ilp`) the same way, with `402` and the same field. A route with no
   transport restriction (the default) is unaffected, and its greetings never carry
   `requiredTransport`.
4. **Unpaid prepare to a priced route**: BTP cannot answer HTTP `402`, so the §1.4 greeting is a
   RESPONSE carrying an `F06` (Unexpected Payment) REJECT, message
   `No payment channel claim attached`, with the x402 v2 terms JSON — byte-identical to §1.4's
   body — as a protocolData entry named `payment-required` (again mirroring the HTTP header of
   the same name). A claimless PREPARE to an unpriced route passes through unchanged, as on HTTP —
   unless, per §1.4's issue #807 update, the PREPARE itself declares `greeting`, in which case this
   same `F06` greeting fires regardless of destination or price.
5. **Standalone claim**: a MESSAGE with an empty `ilpPacket` and a `payment-channel-claim` entry
   is a fire-and-forget claim registration: it is ingested against price `0` (full validation, a
   replay still refused, no value need advance — §1.6's identify-not-pay semantics) and answered
   with nothing, per the client contract (`sendClaimMessage` expects no RESPONSE).
6. **Anything else**: a MESSAGE with no auth, no claim and no `ilpPacket` is ignored. An
   undecodable frame is answered with an ERROR frame (`code F00`, `name NotAcceptedError`, the
   parse failure as UTF-8 `data`) when its requestId was readable, and ignored when not.
7. **TRANSFER** (issue #697): acknowledged with an empty RESPONSE under the same requestId — RFC-23
   requires a responder answer every request, satisfied at the protocol level. The settlement/
   netting accounting a TRANSFER's `amount` will eventually drive is out of scope here; that is
   `toon-meta#262`'s payout-ledger ticket, built on this foundation.

   **Update (issue #699):** the outbound half of that ledger now exists —
   `connector_client_edge::ClientPayoutLedger` signs a cumulative claim per client channel
   (mirroring `connector_runtime::ClaimBook`'s peer-side outbound direction), and
   `payout_claim_protocol_data` carries it as a payout TRANSFER's `payout-claim` protocolData
   entry, JSON like every other entry this dialect carries. This only _creates credit_ and has no
   production caller yet: deciding when a packet's fulfillment should trigger a payout, and to
   which channel, is job-dispatch work built on top of the session registry below (issue #698).

   **Update (issue #698):** the session registry now exists (see "Session registry: the socket is
   the lease" below) and `SessionRegistry::deliver` can originate a MESSAGE through it end to end,
   fenced against a stale generation — but nothing yet decides _when_ to call it for a payout or a
   job. That decision remains the next ticket's.

8. **A RESPONSE or ERROR whose requestId this connector itself originated** (issue #697): resolved
   against that outbound request rather than treated as inbound traffic. One this connector never
   originated — every RESPONSE/ERROR a deployed client sends today — is silently dropped, exactly
   as any non-MESSAGE frame was before TRANSFER and server-origination existed.

**Ordering** (issue #688): _claims_ on one session are judged strictly sequentially, in arrival
order — a frame's claim is fully admitted (or refused) before the next frame's claim is looked
at. This is the transport's reason to exist: claims sent in order on one socket can never race
each other into `F01 NonceNotAdvancing`, which parallel HTTP requests can (issue #544's ordering
promise, extended across packets). What is **not** serialized is a judged frame's remaining work
— the durable record of its claim, routing its packet, sending its RESPONSE — which proceeds for
up to a bounded number of frames concurrently (the connector's `btp_session_window`, default 16;
when the window is full the session stops reading, so the bound is also the backpressure).
RESPONSE/ERROR frames may therefore arrive in a different order than the MESSAGEs that provoked
them; `requestId` is the correlation, per this section's own frame grammar, and the deployed
client resolves responses through its pending-request map by exactly that id. A client MUST NOT
assume responses arrive in request order. Concurrent sessions writing on the same channel still
serialize at the gate's watermark lock, exactly as concurrent HTTP requests do.

**Session registry: the socket is the lease (issue #698, `toon-meta#262` decision 12).**
`connector_client_edge::SessionRegistry` answers, for a client-edge address, which BTP session is
live right now — bound at step 1's auth (keyed by the declared `peerId`) and cleared when that
same session's read loop ends. This is deliberately the only record of reachability: there is no
separate route entry with its own TTL, because a route record and a socket can disagree, and
during the disagreement this connector would route paid work into a hole.

- **Fencing generations.** Each bind for an address is assigned the next number from one
  monotonic counter shared by the whole registry. The highest generation for an address always
  wins; a rebind's cleanup can never remove a binding at a generation newer than the one it names.
  This is buzz's own fencing law (`buzz-relay-mesh/src/wire.rs`): "membership is a hint; the
  fenced generation is the arbiter. The mesh may say 'don't dial' — it may never say 'take over.'"
  A caller retrying a delivery across a reconnect passes the generation it last saw; if the
  address has since moved to a higher one, the attempt is discarded rather than raced against the
  session that superseded it.
- **T-class rejection, never R00.** Every failure path — no live session for the address at all,
  or one that died or timed out mid-delivery — answers `T01` (Peer Unreachable): the packet is
  fine, there is currently no way to reach this peer, and the sender should retry. `R00` (Transfer
  Timed Out) would wrongly imply the packet's own expiry passed.
- **Backstop TTL.** The primary liveness signal is the socket's own read loop ending, which
  unbinds a session immediately. `connector_client_edge::session_registry::SESSION_LEASE_BACKSTOP_TTL`
  (120s) exists only for a socket that still looks alive at the TCP layer but has stopped
  producing frames, checked lazily on lookup rather than by a background sweep.
  **Cross-plane invariant:** `buzz#84`'s relay-side provider-freshness window must never exceed
  this value, or a buyer pays for a job advertised as routable here after this connector has
  already given up on it. `buzz#84` is TypeScript and has no path to import a Rust `pub const`
  (issue #722) — it reads `extra.sessionLeaseTtlMs` off the §1.4 greeting instead (see §1.4), the
  same value this constant enforces, rather than duplicating a guess.

No production caller decides when to push a job to a client session yet — that is the next
ticket's job, the same posture #697/#699 shipped their own foundations under. What this ticket
lands is real in production today: every BTP session's auth, per-frame liveness and close already
run through `bind`/`touch`/`unbind`, so the registry and its fencing invariant are exercised by
every live session, not only by tests.

### 1.10 Owner-authenticated claim state: `POST /ilp/claim-state` (issue #693)

A bulk, read-only answer to "what is the off-chain claim state of every channel I control?" —
deposit total, cumulative claimed, available balance, nonce and last-claim time, for as many
channels as one request names, each independently authenticated by a signature over that
channel alone. Exists because the off-chain claim watermark is known only to a channel's own
counterparty and to this connector's claim gate: an on-chain read gives deposit and channel
existence for free, but not the watermark, and an agent whose channel has run dry cannot afford
a paid write to report its own state (a management surface polling many agents' runway needs
this to work precisely when an agent is broke, dead or offline).

**Request.**

```json
POST /ilp/claim-state
Content-Type: application/json

{
  "channels": [
    {
      "blockchain": "evm",
      "channelId": "0x<64-char hex>",
      "expires": 1735689600,
      "signature": "0x<65-byte r||s||v hex>"
    },
    {
      "blockchain": "solana",
      "channelAccount": "<base58>",
      "expires": 1735689600,
      "signature": "<base64 64-byte Ed25519>"
    }
  ]
}
```

Every entry is independent: a request MAY mix EVM and Solana channels, and a request naming
channels controlled by different keys is answered exactly as one naming channels controlled by
one key would be — nothing about this endpoint requires the caller to be a single identity, only
that it can produce a valid signature per channel it asks about.

**Auth: a signature per channel, not a signature over the request.** Each entry's `signature` is
that channel's counterparty key (the same key that signs a real claim, §1.3 step 4's "the
counterparty this connector has recorded for the channel") signing a **claim-state challenge** —
a message distinct from a real claim's balance-proof signature, so a captured challenge can never
be replayed as a payment or vice versa:

- **evm** — EIP-712, same domain as a real claim (`EIP712Domain(name: "TokenNetwork", version:
"1", chainId, verifyingContract)`, read from the channel's own recorded domain, never from the
  request), a distinct typed struct:
  ```text
  ClaimStateChallenge(bytes32 channelId,uint256 expires)
  ```
- **solana** — Ed25519 over a tagged message distinct in both content and length from a real
  claim's 96-byte balance-proof message (48 bytes before ADR 0053 bound the program id into it,
  issue #1082; the challenge tag was chosen to be neither the same length as nor a prefix or suffix
  of either layout):
  ```text
  message = "toon-claim-state-challenge-v1" || channelAccount(32 bytes) || expires(u64 LE)
  ```

`expires` (unix seconds) is required and is the whole of this endpoint's replay bound: a
signature verifies for any `now <= expires`, reusably — this is a read that changes no state and
advances no watermark, so there is nothing for a nonce to protect. A caller reissues a fresh
`expires` (and therefore a fresh signature) whenever it wants a signature that outlives one it no
longer wants trusted.

**Response.** `200`, one result per requested channel, same order as the request:

```json
{
  "channels": [
    {
      "blockchain": "evm",
      "channelId": "0x...",
      "ok": true,
      "depositTotal": "1000000",
      "cumulativeClaimed": "250000",
      "available": "750000",
      "nonce": 3,
      "lastClaimTime": 1735680000
    },
    {
      "blockchain": "solana",
      "channelAccount": "...",
      "ok": false,
      "error": "unverified"
    }
  ]
}
```

Money fields are decimal strings (matching §1.3's `transferredAmount` convention), never a bare
JSON number — a value a JS `Number` cannot represent exactly past 2^53 is a real amount this
endpoint reports, not a hypothetical one.

- `depositTotal` — the channel's on-chain deposit, or `null` for a channel this connector only
  has _declared_ (`[[client_channels]]`) — declaring a channel names a counterparty and never an
  amount (§1.3's collateral-binding exemption applies here identically), so no figure exists to
  report. A resolved (chain-backed) channel always reports a number.
- `cumulativeClaimed` — the channel's watermark, `"0"` if this connector has never accepted a
  claim on it.
- `available` — `depositTotal - cumulativeClaimed + credited` (issue #700's netting: `credited` is
  what this connector has separately committed to pay this channel's counterparty back, e.g. for
  factory work it earned, `"0"` for a channel nothing has been paid out on) — the same spendable
  headroom figure §1.3 step 5's collateral binding admits an inbound claim against, not a raw
  on-chain balance. `null` exactly when `depositTotal` is.
- `nonce` — the watermark's nonce, `0` if none yet.
- `lastClaimTime` — unix seconds this connector last accepted a claim on this channel (over
  **any** carrier — `POST /ilp`, `POST /ilp/probe`, or the BTP session), or `null` if it never
  has. **Best-effort and non-durable**, unlike every other field above: a connector restart resets
  it to `null` until the next accepted claim, deliberately — recording it durably would mean
  stamping a wall-clock read into the claim admission path's write-lock or group-commit journal
  (issues #686/#690), which this endpoint's own acceptance criteria forbids adding to. A consumer
  MUST treat a `null` here as "unknown", never as "never claimed" — the deposit/cumulative/
  available/nonce figures beside it remain exact across a restart regardless, since those still
  come from the durable watermark.

**Which book answers (issue #1102).** A connector keeps two inbound books — this edge's own, and the
peer semantics's `ClaimBook` — and the watermark reported above is the one belonging to the book that
actually judges claims on that channel. Which book that is is a property of the **channel**, never of
who is asking: a channel this node holds as a `[[peer_channels]]` row is judged by the peer book and
MUST be reported from it; every other channel is this edge's and is reported from here. The two sets
cannot overlap, because a channel named in both `[[peer_channels]]` and `[[client_channels]]` is a
load-time refusal. This matters because a `[[pay_channels]]` payer (ADR 0042 item 2) asks this endpoint
about a channel it holds with its next hop in both roles at once, and signs its next claim from the
answer: answered out of the wrong book, it re-signs one cumulative amount at a fresh nonce forever.
`lastClaimTime` is the one field that does not follow — the peer book records no timestamp for an
accepted claim, so a peer channel reports `null` there, within the best-effort licence above.

**What a failed entry reveals.** `ok: false` carries only `error`, one of:

- `"expired"` — `expires` is not in the future. A fact about the request, safe to report exactly.
- `"unverified"` — everything else: the channel does not exist, the signature does not verify, or
  this connector's resolution of the channel from chain failed. These are deliberately collapsed
  into one reason, unlike §1.3's claim-refusal taxonomy (which _does_ distinguish "no such
  channel" from "bad signature" for a paying sender's benefit) — this endpoint's own acceptance
  criteria requires that a caller learn nothing about a channel it does not control, and "channel
  exists but your signature is wrong" already discloses existence. A caller cannot distinguish a
  channel that has never existed from one it simply guessed the wrong key for.

**Not on the admission path.** This endpoint only reads: the watermark, the channel registry
(counterparty, deposit floor, EIP-712 domain) and the best-effort last-claim-time index above. A
channel lookup this connector has not already resolved goes through the same budgeted resolution
§1.3's "a lookup that resolves nothing must be bounded too" already governs for a claim, so a
flood of fabricated channel ids against this endpoint costs no more than the same flood would
against `POST /ilp`. Nothing here calls into claim ingestion, and no per-packet work was added to
`handle_prepare` to build it.

## 2. What version 1 does not do

Version 1 has no field or header identifying its own version. That is the gap §3 closes: version
1 is the version a client speaks when it addresses `POST /ilp` with none of the version-selection
mechanism below, and is preserved exactly as specified above for as long as any client depends on
it. **That promise no longer rests on ADR 0013** (issue #1073): the parallel fleet it described was
switched off by issue #872, so "the old fleet stays up until nothing addresses its prefix" refers to
nothing. Version 1's preservation rests instead on §3.1's own guarantee — the unversioned path is a
**permanent** alias for `v1`, and a client that never adopts versioning is never asked to change.

## 3. Introducing a new version

A new client edge version is additive, never a breaking change to an existing one — the
mechanism below exists specifically so `toon-client` (and any other installed client) keeps
working, unmigrated, indefinitely.

### 3.1 Version-qualified paths

Each supported version is served at its own path: `POST /ilp/v{N}`. The unversioned `POST /ilp`
path (§1) is kept forever as a permanent alias for `v1` — a client that never adopts versioning
is a `v1` client by definition and is never asked to change. Introducing version `N+1` means
adding a new `POST /ilp/v{N+1}` handler beside the existing ones; it MUST NOT alter the behavior
of any lower-numbered path.

### 3.2 Discovering what a connector supports

**Retired before it was ever built (issue #1054).** Version support is a fact about this node, and a
node's facts live in **one** document: its self-description, which a `GET` on this connector's own URL
returns ([ADR 0050](../adr/0050-a-connectors-url-resolves-to-its-self-description.md),
[`self-description-spec.md`](self-description-spec.md)). A separate versions endpoint would have been a
third surface describing the same node, after the greeting and the kind:10032 announce — which is the
mess ADR 0046 and ADR 0050 exist to end.

`supportedVersions` and `defaultVersion` are therefore fields on that document, carrying exactly what
this section described. Built with #1080; on a connector serving only version 1 they read:

```json
{ "supportedVersions": [1], "defaultVersion": 1 }
```

`defaultVersion` is the version `POST /ilp` (unversioned) currently serves — always `1`, per §3.1's
permanence guarantee; the field exists so a client can assert its assumption rather than infer it.
A client MAY read the document before deciding whether to address a version-qualified path, but is
never required to — addressing `/ilp` directly always works. Note that reading it is a `GET` on the
**same URL** the client already posts packets to, so there is no second address to discover, and the
answer arrives with every other fact about the node rather than on its own.

### 3.3 Agreement

A client and this connector agree on which version is in use by the path the client chooses to
address: `POST /ilp` (or `/ilp/v1`) is a version-1 exchange end to end; `POST /ilp/v2` is a
version-2 exchange end to end. There is no per-request negotiation or content-type haggling — the
path is the entire agreement, which keeps the client edge as small as the two-repository
implementation cost in [ADR 0003](../adr/0003-clean-room-peer-wire-versioned-client-edge.md)
demands (implemented once in Rust, once in TypeScript for `toon-client`, and complexity here is
paid twice on those grounds alone). A connector that does not implement a version a client
requests returns `404` on that version's path, distinguishable from every in-spec response
defined above.

### 3.4 Retirement

This spec defines only how a version is _introduced_ alongside an existing one. Retiring a
version — ceasing to serve a version-qualified path — is a separate operational decision outside
this document's scope, gated on nothing addressing that version's prefix. **The mirror this
previously pointed at is gone** (issue #1073): ADR 0013's parallel fleet was switched off by issue
#872. The gate itself stands on its own; it needs no precedent.

## 4. Consistency

This specification uses exactly the vocabulary of `CONTEXT.md` (connector, app, handler, packet,
route, route termination, client edge, payment channel, claim, nonce, watermark, fee, price,
probe) and implements [ADR 0001](../adr/0001-rust-workspace-library-first.md) and
[ADR 0003](../adr/0003-clean-room-peer-wire-versioned-client-edge.md). It does not use
"terminator", "BLS"/"Business Logic Server", or "agent runtime" (all deprecated); it uses "app"
and "handler" for the payment-oblivious service behind a terminated route.
