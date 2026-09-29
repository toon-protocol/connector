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
  `ILP-Payment-Channel-Claim` header's self-declared sender (`http:<senderId>` — a voucher
  declares no signer, issue #1384; before it, a `toon-channel` claim's `signerAddress` or
  `signerPublicKey`), or
  `http:anon` if that header is absent — including when only the wrapped
  `ILP-Payment-Channel-Claim-Wrapped` header is present, since deriving an identity from it would
  require unwrapping before the identity used to authenticate the request is known. This is the
  path an unaffiliated buyer uses — no prior registration with the connector's operator is
  required to pay for a terminated route.

### 1.3 Payment claim

A request pays with a claim header. **Every claim is an x402 `batch-settlement` voucher**
([ADR 0074](../adr/0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md),
[ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision 8,
issue #1384): a JSON object, `version: '1.0'`, `scheme: 'batch-settlement'`, discriminated by
`blockchain: 'evm' | 'solana'`, paying on an x402 channel -- EVM `x402BatchSettlement`, Solana
`payment-channels` -- that the payer opened toward this connector. The normative bytes are
`claim_voucher` in `vectors/wire-vectors.json` ([ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md)
-- prose is not).

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

Required fields on every claim, regardless of chain: `version` (`'1.0'`), `scheme`
(`'batch-settlement'`), `blockchain`, `messageId` (idempotency), `timestamp` (ISO 8601),
`senderId`. `senderId` is self-declared and carries no authority: it labels the sender for §1.2's
anonymous identity and for the lookup budget below, and nothing is verified against it.
Chain-specific fields, named as x402's own voucher payloads name them:

- **evm**: `channelId` (bytes32 hex), `maxClaimableAmount` (decimal string, cumulative, a `uint128`
  that must also fit a `u64` -- a larger one is refused, never truncated), `signature` (`0x` + 130
  hex, `r ‖ s ‖ v`, EIP-712 `Voucher(bytes32 channelId,uint128 maxClaimableAmount)` under the
  `x402BatchSettlement` domain), and `channelConfig` -- the seven `ChannelConfig` fields `getChannelId`
  hashes -- on a channel's first voucher, optional after it.
- **solana**: `channelId` (the channel account, base58), `maxClaimableAmount` (decimal string,
  cumulative), `expiresAt` (MUST be `0`: a voucher that can expire is value that can lapse before
  it is landed, and is refused structurally), `signature` (base58 Ed25519 over the 50-byte voucher
  message).

**`scheme` is required, and a `toon-channel` claim is refused by name**
([ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision 8,
issue #1384). A claim with no `scheme` is the retired `toon-channel` claim -- the nonce-ordered
EIP-712 or `TOON-BALPROOF-V2` balance proof on a `TokenNetwork` or TOON-program channel
([ADR 0024](../adr/0024-peer-wire-claims-sign-the-eip-712-balance-proof.md),
[ADR 0053](../adr/0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md), both retired)
-- and so is one declaring `scheme: 'toon-channel'`. Either is refused as its own reason, naming the
retirement, before any scheme-specific field is read, the way `blockchain: 'mina'` is (see the note
at the end of this section) -- never reported as merely malformed, so a straggling client learns to
upgrade rather than wondering why payment failed. `vectors/wire-vectors.json`'s
`toon_channel_refused` pins the refusal. A `scheme` naming anything else is malformed.

> **Superseded by issue #1384.** Until #1384 a claim with no `scheme` was a `toon-channel` claim
> (`channelId`/`channelAccount`, `nonce`, `transferredAmount`, `lockedAmount`/`locksRoot`,
> `signerAddress`/`signerPublicKey`, `programId`, optional `chainId`/`tokenNetworkAddress`/
> `tokenAddress`/`cluster`), verified against a counterparty this connector recorded per channel --
> declared in `[[client_channels]]` or resolved from the `TokenNetwork`/TOON program -- under a nonce
> watermark, with a Solana `cluster` cross-check (issue #975) and a declared-`programId` report
> (issue #1127). All of it is deleted with the scheme; this section's history is in git.

A present claim is validated by the same gate a peer's voucher is judged by, before the PREPARE is
routed, in this order — deliberately freshness-and-value before cryptography, so a replay or an
underpayment never pays the cost of a channel lookup or a signature verification and never reaches
the terminating app:

1. **Structural validation** — required fields per chain, formats (hex length, base58 alphabet) as
   enumerated above; a structurally invalid claim is rejected. `blockchain: 'mina'` and a
   `toon-channel` claim fail here, each by name.
2. **Freshness** — a voucher has no nonce: its `maxClaimableAmount` MUST **strictly** exceed this
   connector's watermark for the (blockchain, channel) tuple, the highest cumulative amount it has
   accepted on that channel. A voucher byte-identical to the one that set the watermark -- same
   amount, same signature bytes -- is a **retransmission**: it buys nothing, so it is accepted again
   where the charge is zero, advancing and recording nothing, and refused as an underpayment
   (step 3) where it is not. An equal amount under a different signature is not the same voucher
   and is refused as not advancing. Nothing here spends a lookup.

   The **channel** in that tuple is the channel, not the text the claim spelled it with ([issue
   #643](https://github.com/toon-protocol/connector/issues/643)). A connector MUST identify a
   channel's watermark by a canonical form of the id, applied before the watermark is written or
   read. For `evm` that form is exactly `0x` followed by the `channelId`'s 32 bytes as 64
   **lower-case** hex characters — one spelling, not a family of accepted ones. For `solana` it is
   the channel account as it arrives: base58 of an exact 32-byte decode already has only one
   spelling, and base58 is case-_sensitive_, so normalising it would merge distinct accounts. Hex
   is case-insensitive and everything else about a voucher already treats the spellings as one
   channel — the channel is resolved, and the EIP-712 digest computed, over the decoded bytes — so
   a connector that keyed a watermark by the literal text would grant a fresh, empty watermark per
   spelling, and one signed voucher would buy a write once per casing it was retyped in.

3. **Value binding** (for a priced route) — the voucher's cumulative amount MUST advance past the
   watermark by at least the route's charge for this packet, so a minimal fresh voucher cannot pay
   for an expensive route.
4. **Cryptographic verification** — the voucher's channel is resolved from the chain by the
   settlement backend of its `blockchain`, and the signature MUST recover to **the channel's
   voucher signer as the chain records it** ([issue #558](https://github.com/toon-protocol/connector/issues/558)'s
   rule): on EVM `payerAuthorizer` from the verified `ChannelConfig` (a nonzero one is required
   at admission, ADR 0074 decision 2), on Solana `authorized_signer`. Nothing the voucher says
   about its own signer is consulted. On EVM the `channelConfig` -- presented, or this connector's
   journaled record of the channel -- MUST hash to `channelId` (`getChannelId`) before the backend
   is asked, and the backend's own config is re-hashed too, since the signer is read from it; a
   mismatch is refused under its own reason. A voucher on a chain this connector settles on no
   `batch-settlement` channel for -- no `[settlement.<chain>.batch_settlement]` table -- is refused
   by name before anything about it is judged.

   A voucher naming a channel the backend does **not admit** (it does not exist, is not toward this
   connector, or fails ADR 0074 decision 2's admission rules) is refused with its own reason,
   distinguishable from a bad signature and from an underpayment — there is nothing to verify it
   against, and unverifiable is never accepted. A channel the backend knows to be **done** (sealed,
   or withdrawn) is refused as such, a stronger and more actionable fact than "no record".

   A resolution that **fails** — an unreachable endpoint rather than an absent channel — refuses
   the claim under a third, separate reason. It never degrades to accepting the claim, and it is
   never reported as "no such channel": an operator has to be able to tell an outage from a sender
   naming channels at random, and a legitimate payer has to be told to retry rather than told they
   do not exist. A resolution the connector **declined to perform**, because its budget for lookups
   that do not resolve is spent, is a fourth reason again — see "A lookup that resolves nothing must
   be bounded" below.

5. **Collateral binding** — the voucher's cumulative amount MUST NOT exceed what its channel can
   pay, as the backend reads it now: the amount already landed plus what still backs a voucher above
   it -- EVM `balance − pendingWithdrawal`, Solana `deposit` while the channel is Open (ADR 0074
   decision 5) ([issue #646](https://github.com/toon-protocol/connector/issues/646)). A voucher
   above that is provably unredeemable, and serving it is work the operator can never be paid for.
   It is checked **after** cryptographic verification, against the backend's current reading rather
   than a cached floor: on EVM the figure can fall, so there is no lower bound to cache. The refusal
   is its own reason, distinguishable from an underpayment: this voucher _does_ cover the route's
   price, and it consumes nothing — no watermark advances and nothing is recorded — so the remedy
   is to top the channel up and resubmit the same voucher.

   **A payout nets against nothing** ([ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)
   decision 7, issue #1381). A payout rides a connector→client channel of its own (§1.9 step 7), so
   value paid out does not raise what the client may spend, and value the client pays in does not
   fund its payouts.

A claim that fails any check is a validation failure and the PREPARE is rejected before it
reaches the terminating app or advances any watermark. A channel's first accepted voucher also
records the channel itself -- on EVM its `ChannelConfig`, the only way to land any voucher on it
after a restart -- durably, with the voucher.

**A lookup that resolves nothing must be bounded.** A channel this connector has accepted a voucher
on is known, and its lookup is not a discovery. A channel it has no record of is, and a sender naming
nonexistent channel ids provokes one chain read per request, indefinitely
([issue #613](https://github.com/toon-protocol/connector/issues/613)) -- **even the same
nonexistent id, repeated**, since a lookup that finds nothing leaves no record behind. Every
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
— is trusting attacker-supplied text. The voucher's own declared sender (`senderId`, the "declared
signer" below) is available before any lookup and costs nothing to read, but it is **not a
credential**: a voucher's signer is the channel's, which is precisely what has not been resolved
yet, so nothing about the declared sender can be verified at this point without spending the very
lookup being budgeted.

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

**Neither axis is a durable answer, and the durable answer lives outside this step.** It is worth
naming, because a reader who has followed the paragraphs above should not conclude that a declared
signer is the best that can be done:

- **Per-address rate limiting at the reverse proxy** a connector is deployed behind. That is the only
  sybil-resistant axis available at this layer — an address costs something, a keypair does not — and
  it is the right place for it, since the proxy is the only component that sees the real peer.

The rates and the wait are a **deployment** choice — what a connector can afford to spend
discovering channels that do not exist depends on the settlement endpoint it pays for, and it should be derived from that endpoint's real capacity rather
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
against nothing, and an empty watermark admits any voucher naming more than zero, so every voucher
the client already spent becomes free service again ([issue
#605](https://github.com/toon-protocol/connector/issues/605)). A connector therefore MUST record
each accepted voucher durably before treating it as accepted, and MUST rebuild its watermarks from
that record before serving. Two consequences follow, and both are refusals rather than degradations:
a voucher whose acceptance cannot be made durable is refused (as a **temporary** error — the voucher
itself is fine), and a record that cannot be read back, or that carries an entry the connector
cannot decode, stops the connector starting rather than letting it start at no watermarks. Where
the record lives is a deployment question rather than a wire one: today it is the `state_dir`
config field, and a config with a settlement table and no `state_dir` does not load.

**A watermark belongs to one channel, and an x402 channel is never reopened at its own id.** An
EVM channel's id hashes its whole `ChannelConfig`, `salt` included, and a Solana channel's account
is derived with its `open_slot`, so a payer's next channel is a new id with a new, empty watermark,
and nothing carries a finished channel's watermark forward. (The `toon-channel` sweep that retired
the watermark of a settled `TokenNetwork` or TOON-program channel, whose ids a reopen could reuse --
issues #977, #1283 -- is deleted with that scheme, #1384.)

**Mina is not a supported chain.** [ADR 0002](../adr/0002-drop-mina-from-the-rust-connector.md)
drops Mina from the Rust connector: a Mina claim's on-chain lifecycle (open, deposit, close,
settle) has no Rust implementation and none is planned, so a connector that accepted a Mina claim
would be accepting value it can never settle. `blockchain: 'mina'` is therefore refused as a
structural validation failure (step 1 above) rather than parsed or cryptographically checked — the
zkApp-specific fields the peer semantics's predecessor once carried for it (`zkAppAddress`, `tokenId`,
`balanceCommitment`, `proof`, `salt`, and the dual-party `balanceB`/`signatureB` extension) are not
part of this connector's claim shape and are not documented here. A Mina client's claim is rejected
clearly and immediately; it is not owed a code path, only an unambiguous refusal. Mina is checked
before `scheme`, so a Mina claim is refused as Mina whatever scheme it declares.

> **History.** [ADR 0074](../adr/0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md)
> (epic #1349) added the `batch-settlement` voucher beside the `toon-channel` claim, with its own
> freshness, signer and collateral rules; its vectors landed as `claim_voucher` at `schema_version` 6. [ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) (issue
> #1384, `schema_version` 7) made the voucher the only claim, and the steps above are its rules.

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
byte-for-byte, base64-encoded, in a `Payment-Required` response header. Since issue #1384
([ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision 10)
its `accepts[]` holds **only x402-valid `batch-settlement` entries**, one per chain this node
settles on, and TOON's own terms for the request ride in x402 v2's `extensions` slot, under `toon`:

```json
{
  "x402Version": 2,
  "resource": { "url": "g.example.app" },
  "request": { "protocol": "nip90", "kinds": [5096, 5098] },
  "accepts": [
    {
      "scheme": "batch-settlement",
      "network": "eip155:84532",
      "amount": "100",
      "asset": "<token address>",
      "payTo": "<settlement address>",
      "maxTimeoutSeconds": 60,
      "extra": {
        "receiverAuthorizer": "<settlement address>",
        "withdrawDelay": 86400,
        "name": "USDC",
        "version": "2",
        "assetTransferMethod": "eip3009",
        "facilitator": "https://facilitator.example/x402"
      }
    }
  ],
  "extensions": {
    "toon": {
      "info": {
        "ilpAddress": "g.example.app",
        "amount": "100",
        "endpoint": "/ilp",
        "price": "100",
        "sessionLeaseTtlMs": 120000
      },
      "schema": { "type": "object", "required": ["amount"], "...": "..." }
    }
  }
}
```

`accepts` is a list — ADR 0022 notes terms are plural — of the payment methods this connector's
claim gate (§1.3) understands: an x402 `batch-settlement` voucher on each chain whose
`[settlement.<chain>.batch_settlement]` table is written. It is **empty** on a node that settles
on no such chain, which can be paid by nobody. The `toon-channel` entry that led the list until
#1384, and its `extra.settlement`/`extra.settlements` channel-opening terms (issues #617, #632), are
deleted with that claim scheme.

**`extensions.toon`** carries what that entry also carried that is not a payment option, unchanged
in name and meaning: `info` holds `ilpAddress`, `amount`, `endpoint` (`/ilp`), `price` and
`sessionLeaseTtlMs` on every greeting, plus `pricePerKib` where the addressed route prices by size,
plus whichever of `ilpAddresses`/`btpEndpoint`/`requiredTransport` applies (below), and nothing
else; `schema` is a JSON Schema describing `info`, as x402 v2 asks of an extension, and is
informational. It is on **every** greeting, including one with no `accepts[]` entry, so the amount
is always quoted. A reader MUST treat a greeting with no `extensions.toon`, or with an `amount` that
is not a decimal uint64, as unreadable terms, never as "nothing to pay". In the rest of this section
`toon.<field>` names `extensions.toon.info.<field>`.

> **The `batch-settlement` entry** ([ADR 0074](../adr/0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md)
> decision 8, issue #1345) names a real scheme this connector's settlement backends actually
> redeem, x402's own audited contracts, with no TOON contract involved. Its
> top-level `network`/`asset`/`payTo` and its `extra` carry everything x402's `batch-settlement`
> scheme spec requires, so a stock client can build a deposit from the greeting alone: `network` is
> CAIP-2 (`eip155:<chainId>` or `solana:<genesis-hash-prefix>`), `asset` is the token address or
> mint, and `payTo` is this node's own settlement address (Solana: the owner of its receiving
> account). `extra`'s wire names are recorded by ADR 0074 decision 8:
>
> - **EVM:** `receiverAuthorizer`, the minimum `withdrawDelay`, and `name`/`version` — the EIP-712
>   domain of the **asset**, which a client signs its deposit's ERC-3009 or Permit2 authorization
>   under. x402 requires both and an ERC-20 need not expose either, so they are not read off the
>   chain: they are the required config keys `asset_eip712_name`/`asset_eip712_version`.
>   `assetTransferMethod` is x402's own EVM field naming how that deposit moves the token:
>   `"eip3009"` (ERC-3009 `receiveWithAuthorization`; the token must implement ERC-3009, as USDC
>   does) or `"permit2"` (a Permit2 witness transfer, for any ERC-20). It is **always** written,
>   even at x402's own default `eip3009` (`[settlement.evm] asset_transfer_method`, toon-client#695):
>   x402 reads an absent value as `eip3009`, so the explicit default means the same to a stock
>   client, and a reader of this node's greeting never has to know x402's default to know how to
>   deposit. A reader MUST still read an absent `assetTransferMethod` as `eip3009`, as x402 does —
>   a node that predates the field writes none.
>   `facilitator` is this connector's own addition to x402's EVM `extra`, as `sponsorEndpoint` is to
>   its SVM one: the absolute `http(s)` URL of the x402 facilitator this operator relays deposits
>   through and pays the gas of (`[settlement.evm] facilitator_url`). A stock x402 seller calls its
>   facilitator itself; here the deposit precedes the channel and leaves the packet path, so the
>   payer calls it, and the seller names it. It is **absent** when the operator names none, and
>   this connector never calls it. With `permit2`, a payer's one-time Permit2 approval of a token
>   without ERC-3009 is gasless only when that facilitator offers x402's `eip2612GasSponsoring`
>   (a token with EIP-2612 `permit`) or `erc20ApprovalGasSponsoring` (a plain ERC-20); otherwise
>   the payer pays that approval once, from its own ETH.
> - **Solana:** `feePayer` (the sponsor key); `withdrawDelay`, x402's SVM field name, carrying the
>   minimum `grace_period` (`[settlement.solana.batch_settlement] min_grace_period_secs`);
>   `tokenProgram`, x402's required SVM field naming the program that owns `asset` — always SPL
>   Token (`TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`), the one program the backend boots
>   against and the sponsor co-signs under (a Token-2022 `open` is `token_program_unsupported`,
>   §1.11); and two of this connector's own additions to x402's SVM `extra`: `minDeposit`, the
>   smallest opening deposit, in the mint's base units and as a decimal string like every amount
>   here, that the sponsor endpoint (§1.11) co-signs an `open` for — the **published** minimum ADR
>   0074 decision 5 has the sponsor refuse below (`min_sponsored_deposit`) — and `sponsorEndpoint`,
>   the path on this node's HTTP endpoint where the payer-signed `open` is posted,
>   `/ilp/batch-settlement/solana/open` (§1.11; issue #1357).
>
>   ```json
>   {
>     "scheme": "batch-settlement",
>     "network": "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1",
>     "amount": "100",
>     "asset": "<mint>",
>     "payTo": "<settlement pubkey>",
>     "maxTimeoutSeconds": 60,
>     "extra": {
>       "feePayer": "<settlement pubkey>",
>       "withdrawDelay": 86400,
>       "tokenProgram": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
>       "minDeposit": "1000000",
>       "sponsorEndpoint": "/ilp/batch-settlement/solana/open"
>     }
>   }
>   ```
>
>   That is everything a stock x402 SVM client needs to build the `open` (x402 SVM spec
>   `#L193-L201`, `#L294-L305`) and to know where to post it. It posts x402's own `deposit` object,
>   `{"amount", "transaction"}` (`#L378-L379`), to `sponsorEndpoint` rather than handing a whole
>   `PaymentPayload` to the server with a paid request: the `open` leaves the packet path, and the
>   vouchers that follow ride inside ILP (ADR 0074 decision 8). The rest comes from the client's own
>   keys and the chain: its salt and `openSlot`, and a recent blockhash. x402's optional
>   `recentBlockhash` and `recentSlot` hints are **not** carried (ADR 0074 decision 8, amended by
>   #1357). A blockhash lapses in about a minute, so putting one in this answer would add a chain
>   read to every unpaid request and still hand out stale values. x402 lets a client ignore the
>   hints and requires it to refresh them when stale, and the client already has to reach the chain
>   to check `tokenProgram` against the mint's owner.
>
> The self-description publishes the same facts under `batchSettlements` (ND-11); this is a projection of that value, never a second
> assembly of it (`connector_domain::x402::batch_settlement_accept`).
>
> **What `extra` cannot say: this connector requires a `payerAuthorizer`.** x402's EVM scheme lets a
> client leave `ChannelConfig.payerAuthorizer` zero and sign vouchers with `payer`; this connector
> admits only a channel whose `payerAuthorizer` is nonzero (ADR 0074 decision 2, amended
> 2026-09-25), because a zero one hands every voucher check to `payer`, which the contract asks
> ERC-1271 of as soon as it has code — as an EOA does after an EIP-7702 delegation. x402's `extra`
> has no field for the requirement, so it is stated here. A channel opened without one is refused on
> its first voucher as a channel this node does not admit.

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

**`toon.ilpAddresses`/`toon.btpEndpoint`** ([issue
#807](https://github.com/toon-protocol/connector/issues/807)): this node's own ILP address(es) and
BTP endpoint — the same facts a kind:10032 announce carries under the identical names — present
exactly when `[node]` configures them, absent — not empty/null — otherwise. Unlike
`toon.ilpAddress` above, which echoes back whatever `destination` the probing PREPARE named,
these are this node's own authoritative facts regardless of what was probed; a client that trusts
`toon.ilpAddress` as a confirmation of a _guessed_ destination should prefer `ilpAddresses` when it is
present, since the guess is exactly what a stale-or-missing-genesis-seed client cannot rely on.

`amount` (on `extensions.toon` and on every `accepts[]` entry alike) and `toon.price` are read from the same longest-prefix route lookup that §1.3's value
binding and §1.7's `GET /ilp/routes/price` charge and answer against, so this response never states
a price a real request wouldn't also be charged.

They are **equal for a flat route, and that is every route that predates
[ADR 0065](../adr/0065-a-price-is-a-schedule-over-payload-length.md)**. Where a route's price
carries a slope the two answer different questions and must not be conflated:

- **`amount`** is what the request being answered would cost — the route's schedule evaluated at
  that request's own payload length. This is x402's meaning of the field: what to pay for _this_.
- **`toon.price`** is the schedule's **base**, and **`toon.pricePerKib`** its slope, in the same
  decimal-string spelling. Together they let a client compute the cost of a packet it has not sent
  yet, which is what keeps [ADR 0011](../adr/0011-rejects-accumulate-fees-and-probes-discover-cost.md)'s
  cacheability true: one greeting answers every size, rather than one greeting per size.

`toon.pricePerKib` is **absent**, not `"0"`, on a flat route.

**Transport policy** (issue #701, `toon-meta#262` decision 11): which transport(s) a terminated
route accepts is per-connector config, not a protocol constant — `both` by default, so no deployed
route changes behavior until an operator opts in, or restricted to `http` or `btp` alone. A request
over a transport its route does not accept is refused with this SAME `402` shape — before payment
is considered at all, and whether or not the request carries a valid claim, since paying over the
wrong transport does not make the route reachable that way — with one addition: `extensions.toon.info` also
carries `requiredTransport` (`"http"` or `"btp"`), naming the transport the route actually
requires. An ordinary unpaid-request greeting (above) never sets this field.

**The refusal is the backstop, not the discovery mechanism.** Since
[ADR 0072](../adr/0072-a-carriage-pin-is-published-on-the-route-that-enforces-it.md) the pin is
published on the route's own entry in the node self-description, so a client that reads `GET /ilp`
dials the right carriage on its first attempt and never sees this `402` at all. This shape stays
exactly as it is for a client that did not read it. The BTP carriage
answers the mirror case (a route restricted to HTTP, reached over the websocket session) the same
way; see §1.9 step 3.

**`toon.sessionLeaseTtlMs`** ([issue #722](https://github.com/toon-protocol/connector/issues/722),
`toon-meta#262` decision 12): always present, on every
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
no value need advance — a sender probes by resending its latest voucher byte for byte, a
retransmission (§1.3 step 2) that advances and records nothing. A connector recognizes a channel
once a claim on it has cleared §1.3's gate at this edge. It necessarily already holds that
channel's voucher signer, read from the chain — step 4 above verifies against it — but holding a
signer says only _whose signature is accepted here_, never that anyone has turned up and paid; no chain indexes
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
- **`GET /ilp/routes/price?destination=<ILP address>[&size=<bytes>]`** — unauthenticated. Returns
  `200` with the price of the configured route `destination` would match — terminated or forwarded
  ([ADR 0028](../adr/0028-a-forwarded-route-is-priced-at-the-client-edge.md)) — reading the same
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

  An optional `size` names the length in bytes of a packet's **sealed** payload (§1.8's gift wrap,
  never the plaintext inside it) and adds a `charge` field: what a packet of exactly that size
  would be charged, evaluated once by the same `Price::charge` every gate on the value path
  charges under (issue #1267) — never a second implementation of the ADR 0065 formula at this
  layer. This matches the semantics the x402 greeting's `accepts[0].amount` already publishes for
  a request it actually received (§1.4); `size` answers the same question for a size a caller only
  asks about, before it seals anything:

  ```json
  { "destination": "g.example.store", "price": 1000, "price_per_kib": 30, "charge": 1030 }
  ```

  A destination whose route **pins a client carriage** (§1.4's transport policy) answers with
  `requiredTransport` beside its price, `"http"` or `"btp"`, off that same lookup — a caller told
  what a destination costs and not what it takes to reach it can pay in full and still be refused
  ([ADR 0072](../adr/0072-a-carriage-pin-is-published-on-the-route-that-enforces-it.md),
  TOON_Network#111):

  ```json
  { "destination": "g.toon.relay", "price": 1, "requiredTransport": "btp" }
  ```

  It is **omitted** — never `"both"` — on a destination that accepts either, so an unpinned
  route's answer is unchanged.

  `charge` is **absent**, not `null`, when the request names no `size`, so the answer above is
  unchanged for a caller that does not ask. A `size` that is not a non-negative integer a `u64` can
  hold — negative, non-integer, or too many digits — is a `400`, never a silent fall-back to the
  sizeless answer, since that would hand the caller a number it would mistake for a charge; a
  destination this connector serves no route for is still a `404` whether or not `size` is present,
  since the route lookup fails before `size` is ever consulted.

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
identity from the channel's chain-resolved record (ADR 0036).
(`GET /channels`/`GET /claims` do not carry this: both project the node's own peer channels,
which a payer-opened client channel is not.)

### 1.9 Client BTP websocket transport (issue #674 family)

A second carriage for exactly the pipeline §1.1–§1.6 specify over HTTP: one persistent,
**ordered** websocket session carrying BTP-framed ILP packets and claims, so that a client
streaming many paid writes advances its vouchers on one socket in one order instead of racing
parallel HTTP requests. Nothing here changes what is validated or charged — the same claim gate
instance, watermarks, journal and refusal taxonomy serve both carriages, and a write that arrived
over BTP is indistinguishable downstream from one that arrived over HTTP.

**Peer sessions (ADR 0027).** This section previously stated that peers do not use this transport,
so every BTP session was a client session by construction (ADR 0026). ADR 0027 reverses that: the
raw-TCP transport is deleted and connectors peer over BTP on the same codec. A frame is a **peer**
frame only while it presents a voucher — or, on a packet that moves no value, a claim-state
challenge — on a channel whose voucher signer is bound to that peering, by a `[[peer_channels]]` row
or by `POST /peers` ([ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md),
issue #1157, as [ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)
decision 5 amends it, issue #1380; `peer-carriage-spec.md` §1.2). A `toon-channel` claim never
makes one: since #1384 it is refused by name on this session like any other (§1.3). Anything else is a client frame, with no fallthrough,
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
> #1157) and not replaced: role was the `[[peer_channels]]` binding plus a verified claim on one of
> that peering's channels, decided per frame — and since ADR 0075 (issue #1380) a verified voucher
> or challenge from a bound voucher signer. ADR 0027's point here — role by authentication, not by
> transport or port — is unchanged._) "Every BTP session is a client session by construction"
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

   **Declaring a channel at auth (issue #790, as issue #1384 replaces it).** The same `auth` entry
   MAY carry a `channelChallenge` field: the **voucher claim-state challenge** object — exactly what
   a `POST /ilp/claim-state` entry (§1.10) and a peer's `peer-role-challenge`
   (`peer-carriage-spec.md` §1.4) carry, `scheme: "batch-settlement"` required:

   ```json
   {"peerId": "g.example.agent", "secret": "",
    "channelChallenge": {"blockchain": "evm", "scheme": "batch-settlement",
                         "channelId": "0x…", "expires": 1800000000, "signature": "0x…",
                         "channelConfig": {…}}}
   ```

   It proves, _before_ this session has ever paid, that the session holds the voucher signer of an
   x402 channel toward this connector. That exists because a client that only ever earns (opens a
   channel, serves paid work, pays nothing) would otherwise never teach this connector where to pay
   it (step 7). It is verified exactly as a peer's challenge is: the channel it names is resolved by
   the settlement backend of its chain, the **voucher signer is read from the chain** (EVM
   `payerAuthorizer`, Solana `authorized_signer`), and the signature — over
   `ClaimStateChallenge(bytes32 channelId,uint256 expires)` under `x402BatchSettlement`'s EIP-712
   domain on EVM, Ed25519 over `"toon-voucher-claim-state-challenge-v1" ‖ channelAccount ‖ expires`
   on Solana (`vectors/wire-vectors.json`'s `voucher_claim_state_challenge`) — must recover to it.
   Its `expires` must be ahead of this connector's clock and no more than **300 seconds** ahead
   (the peer challenge's own bound): within it a challenge is a bearer proof. A verified challenge
   teaches the session its _payee_ — that voucher signer (step 7). Best-effort, like the `peerId`
   bind itself: an expired, too-distant, malformed, unresolvable or wrongly signed challenge leaves
   the session exactly where it was, and an accepted voucher (§1.3) teaches the same payee anyway.

   **The retired `auth_channel_proof` is refused by name.** Until #1384 the declaration was three
   flat fields on the `auth` entry — `channelId`, `expires`, `signature` — signing the same struct
   under a `TokenNetwork` channel's domain and verified against a `[[client_channels]]` or
   chain-resolved `toon-channel` counterparty. An `auth` entry carrying any of those three
   top-level fields is answered with an ERROR frame (`code F00`, `name NotAcceptedError`, data
   naming the retirement) instead of the empty RESPONSE, and **the session is not bound**; nothing
   in it is read. Its vector section, `channel_control_declaration`, is gone from
   `vectors/wire-vectors.json` at `schema_version` 7.

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
   self-diagnosing via an additional `extensions.toon.info.requiredTransport` field (`"http"` or `"btp"`) naming
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

   **Update (issue #1381, [ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)
   decision 7): a payout is a voucher on a connector→client channel.** The paragraphs above describe
   the `toon-channel` payout claim that preceded it; nothing signs one any more.
   - **The channel.** A client is paid on an x402 `batch-settlement` channel this connector opens
     toward it with the paying half of its settlement port — one of its ordinary outbound channels,
     opened and funded by the operator's signed `POST /channels` on the terms the client publishes
     (the same `batchSettlements` entry shape a node's self-description carries: on EVM `payTo`, both
     receiving seats the client's; on Solana the client's own `feePayer` and `sponsorEndpoint`, so the
     client holds the `payee` and `rent_payer` seats, ADR 0075 decision 3). The channel is journaled
     before its opening transaction is sent and every voucher on it before the voucher is handed out,
     so the payout watermark survives a restart. This connector never opens or tops up a payout
     channel on its own: funding stays an operator write (ADR 0075 decision 11).
   - **Which client a channel pays.** A session is paid at its **payee key**, learned only from a
     signature the claim gate has verified by that key: the voucher signer of the channel the session
     pays this connector on (EVM `payerAuthorizer`, Solana `authorized_signer`, as the chain records
     it), taught when a voucher on it is accepted on the session; or the voucher signer a
     `channelChallenge` at auth (step 1) proved. There is no client-declared payout address, and
     nothing a client merely asserts teaches a payee.
   - **Whose payee it is.** The payee belongs to the **authenticated session** that proved it — the
     session generation its bind issued (§1.9 step 1's "the socket is the lease") — never to the
     `peerId` it bound, which the client merely asserts and another socket can assert too (issue
     #1396). A new session starts with **no** payee, whatever an earlier session that declared the
     same `peerId` proved, and what it proves changes no other session's payee; the payee is cleared
     when the session unbinds, a superseded session's unbind clearing only its own. A fulfilled
     delivery is paid at the payee of **the session that fulfilled it**, and a session that has
     proved none is paid nothing for it. A payout is signed on this connector's open outbound channel
     whose receiver is that key; a session with no payee, or a payee with no open channel toward it,
     is paid nothing and the packet still answers as it would. A voucher is landable only by its
     channel's receiver, so a payout delivered to the wrong socket pays nobody else.
   - **Reconnecting.** A payout voucher still unacknowledged when its session dropped stays owed to
     its payee **key**, not to an address. A client reconnects on a new session and proves the same
     key again — a `channelChallenge` on its `auth` (step 1), or a voucher (§1.3) on the new session
     — and at that moment every voucher still pending toward that key is resent over the new session
     (after the challenge is recorded, never before). A session that proves a different key is
     resent nothing, and where the old key's vouchers are owed does not change. A client that
     reconnects without proving a key is paid nothing until it does.
   - **The signer.** Each chain's settlement key signs the voucher; `[signer]` signs none.
   - **The wire.** The TRANSFER's `payout-claim` protocolData entry is the voucher as JSON, spelled
     as a client spells its own voucher claim (§1.3) less the envelope fields: `blockchain`,
     `scheme: "batch-settlement"`, `channelId`, `maxClaimableAmount` (decimal string) and `signature`
     (`0x` + 130 hex on EVM, base58 of 64 bytes on Solana). An EVM payout always carries the
     `channelConfig` `claim` needs; a Solana one carries `expiresAt: 0`. The TRANSFER's `amount` is
     the voucher's cumulative amount. Everything the client needs to land the voucher itself is in the
     entry. `vectors/wire-vectors.json`'s `payout_voucher` section (`schema_version` 7, issue #1384)
     pins this entry and its TRANSFER; the rest of this dialect stays uncovered (ADR 0026's #1073
     correction).
   - **Delivery and dedupe are unchanged**: one voucher per fulfilled job, deduped on the job this
     connector asked for (issue #770), resent on the next delivery or reconnect until the client
     answers the TRANSFER with a RESPONSE (issue #779). A voucher is cumulative, so the latest one
     carries forward anything an earlier delivery failed to hand over.
   - **No netting.** A payout raises nothing the client may spend (§1.3 step 5, §1.10).

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
  (issue #722) — it reads `extensions.toon.info.sessionLeaseTtlMs` off the §1.4 greeting instead (see §1.4), the
  same value this constant enforces, rather than duplicating a guess.

No production caller decides when to push a job to a client session yet — that is the next
ticket's job, the same posture #697/#699 shipped their own foundations under. What this ticket
lands is real in production today: every BTP session's auth, per-frame liveness and close already
run through `bind`/`touch`/`unbind`, so the registry and its fencing invariant are exercised by
every live session, not only by tests.

### 1.10 Owner-authenticated claim state: `POST /ilp/claim-state` (issue #693)

A bulk, read-only answer to "where does every channel I pay on stand?" — the amount watermark, the
ceiling the next voucher is admitted against, and the last-claim time, for as many channels as one
request names, each independently authenticated by a signature over that channel alone. It exists
because the off-chain watermark is known only to the payer and to this connector's claim gate: a
voucher has no nonce, so this is the only way a client that lost its channel store learns the
amount its next voucher must strictly exceed — the chain's `totalClaimed` (EVM) or `settled`
(Solana) is only a floor, trailing the watermark until the connector lands its latest voucher — and
an agent whose channel has run dry cannot afford a paid write to report its own state.

Since issue #1384 ([ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)
decision 8) every entry asks about an x402 `batch-settlement` channel this connector receives
vouchers on, and names `scheme: "batch-settlement"`.

**Request.**

```json
POST /ilp/claim-state
Content-Type: application/json

{
  "channels": [
    {
      "blockchain": "evm",
      "scheme": "batch-settlement",
      "channelId": "0x<64-char hex>",
      "expires": 1735689600,
      "signature": "0x<65-byte r||s||v hex>",
      "channelConfig": {
        "payer": "0x…",
        "payerAuthorizer": "0x…",
        "receiver": "0x…",
        "receiverAuthorizer": "0x…",
        "token": "0x…",
        "withdrawDelay": 86400,
        "salt": "0x…"
      }
    },
    {
      "blockchain": "solana",
      "scheme": "batch-settlement",
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

**`scheme` is required.** An entry with no `scheme`, or with `scheme: "toon-channel"`, asks about a
retired `toon-channel` channel and is answered `"toon-channel-refused"` by name, with nothing looked
up for it (issue #1384); any other `scheme` is `"unverified"`. Until #1384 an absent `scheme` asked
about a `TokenNetwork` or TOON-program channel, proved over the same struct under that channel's
`TokenNetwork` domain or the `"toon-claim-state-challenge-v1"` Solana message, and answered with a
`nonce` and a `depositTotal`; all of that is deleted with the scheme.

**Auth: a signature per channel, not a signature over the request.** Each entry's `signature` is
by the channel's **voucher signer**, taken from the chain and never from the request — on EVM the
verified `ChannelConfig`'s `payerAuthorizer`, on Solana the channel account's `authorized_signer`
— over a **claim-state challenge**, kept apart from that key's vouchers so a captured challenge can
never be replayed as a payment or vice versa:

- **evm** — EIP-712 `ClaimStateChallenge(bytes32 channelId,uint256 expires)` under
  **`x402BatchSettlement`'s** domain (`("x402 Batch Settlement", "1", chainId, 0x4020074e…0003)`,
  this connector's, never the request's): a different typehash from `Voucher`.
- **solana** — Ed25519 over
  `"toon-voucher-claim-state-challenge-v1" || channelAccount(32 bytes) || expires(u64 LE)`.

`vectors/wire-vectors.json`'s `voucher_claim_state_challenge` pins both. The same message proves
the peer role for a packet that moves no value (`peer-carriage-spec.md` §1.4) and declares a
client's channel at BTP auth (§1.9 step 1).

**`channelConfig` (EVM, optional).** The contract stores a channel by id alone, so the connector
finds one by its config: its own journaled record for every channel it has accepted a voucher on,
else this field, in a voucher's own spelling. Either must hash to `channelId`. A client that lost
its store therefore needs only the channel id and its signing key for any channel it has paid on;
for one it has not, it presents the config it opened the channel with.

`expires` (unix seconds) is required and is the whole of this endpoint's replay bound: a signature
verifies for any `now < expires`, reusably — this is a read that changes no state and advances no
watermark. A caller reissues a fresh `expires` (and therefore a fresh signature) whenever it wants
a signature that outlives one it no longer wants trusted.

**Response.** `200`, one result per requested channel, same order as the request:

```json
{
  "channels": [
    {
      "blockchain": "evm",
      "channelId": "0x...",
      "ok": true,
      "scheme": "batch-settlement",
      "cumulativeClaimed": "250000",
      "maxCumulative": "1000000",
      "available": "750000",
      "lastClaimTime": 1735680000
    },
    {
      "blockchain": "solana",
      "channelId": "...",
      "ok": false,
      "error": "unverified"
    }
  ]
}
```

Money fields are decimal strings, never a bare JSON number — a value a JS `Number` cannot represent
exactly past 2^53 is a real amount this endpoint reports, not a hypothetical one.

- `cumulativeClaimed` — the amount watermark: the highest cumulative amount the connector has
  accepted a voucher for on this channel, `"0"` for none. The next voucher must strictly exceed it
  (§1.3 step 2). It is this edge's book's, which judges a peer's vouchers too: since
  [ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision 5 a
  voucher proves the peer role on a channel whose voucher signer is bound to a peering, it is
  judged against the channel's one watermark whichever role it arrives under, and decision 6 makes
  this endpoint the watermark a paying peer restores from, so a peer-bound channel is answered
  exactly as a client's is and never below where the channel stands.
- `maxCumulative` — the highest cumulative amount a voucher may name and be accepted, as §1.3 step
  5 reads it now: the amount landed on chain plus what still backs a voucher above it, which is
  `balance − pendingWithdrawal` on EVM and `deposit` on an Open Solana channel (ADR 0074 decision 5).
  It **can fall** on EVM, when the payer initiates a withdrawal.
- `available` — `maxCumulative − cumulativeClaimed`, at least `"0"`: what the next voucher may add.
  A payout this connector owes the client rides a channel of its own and nets against nothing
  (ADR 0075 decision 7, retiring issue #700's netting).
- `lastClaimTime` — unix seconds this connector last accepted a voucher on this channel (over
  **any** carrier — `POST /ilp`, `POST /ilp/probe`, or the BTP session), or `null` if it never
  has. **Best-effort and non-durable**, unlike every other field above: a connector restart resets
  it to `null` until the next accepted voucher, deliberately — recording it durably would mean
  stamping a wall-clock read into the claim admission path's write-lock or group-commit journal
  (issues #686/#690). A consumer MUST treat a `null` here as "unknown", never as "never claimed" —
  the figures beside it remain exact across a restart regardless, since those come from the
  durable watermark.

**One book answers** (amended by
[ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md), issue #1380).
Every channel this endpoint reports is judged by this edge's own claim gate, and is reported from
it, so a `[[pay_channels]]` payer restoring its outbound watermark from here is told exactly where
its channel stands. (Until #1380 a `toon-channel` channel held as a `[[peer_channels]]` row was
reported from the peer semantics's `ClaimBook`, issue #1102; no peering pays on one now.)

**What a failed entry reveals.** `ok: false` carries only `error`, one of:

- `"expired"` — `expires` is not in the future. A fact about the request, safe to report exactly.
- `"toon-channel-refused"` — the entry names no `scheme`, or `"toon-channel"` (issue #1384). A fact
  about the request too: nothing about any channel was looked up.
- `"unverified"` — everything else: a chain this connector does not settle vouchers on, a channel
  with no record and no `channelConfig`, a config that hashes to another channel, a channel that is
  not admitted or no longer accepts vouchers, a resolution from chain that failed, or a signature
  by any other key. These are deliberately collapsed into one reason, unlike §1.3's claim-refusal
  taxonomy (which _does_ distinguish "no such channel" from "bad signature" for a paying sender's
  benefit) — a caller learns nothing about a channel it does not control, and "channel exists but
  your signature is wrong" already discloses existence.

**Not on the admission path.** This endpoint only reads: the watermark, the journaled channel
records and the best-effort last-claim-time index above. A channel lookup this connector has no
record of goes through the same metered resolution §1.3's "a lookup that resolves nothing must be
bounded" governs for a voucher, so a flood of fabricated channel ids against this endpoint costs no
more than the same flood would against `POST /ilp`. Nothing here calls into claim ingestion, and no
per-packet work was added to `handle_prepare` to build it.

### 1.11 Sponsoring a Solana batch-settlement open: `POST /ilp/batch-settlement/solana/open` (issue #1346)

[ADR 0074](../adr/0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md) decision 9. A client
that holds the mint and no SOL opens an x402 `batch-settlement` channel on `payment-channels` with this
node as fee payer and `rent_payer`: it builds and signs the `open` from the greeting's `batch-settlement`
entry (`payTo`, `asset`, `extra.feePayer`, the minimum `withdrawDelay`, `extra.tokenProgram`, a
deposit of at least `extra.minDeposit`, §1.4), and posts it to `extra.sponsorEndpoint`, which is
this path. The
node co-signs, **submits**, waits for the outcome, and admits the channel it made.

**Public, not an operator write.** The channel does not exist yet, so the call cannot be paid for, and a
buyer this node has never heard of must be able to make it (ADR 0052). It carries no RFC 9421 signature
and no identity; [ADR 0008](../adr/0008-operator-surface-splits-read-from-write.md)'s write keys are
the operator's, not a buyer's. What bounds it is what it will sign, below, and the rate at which a
stranger can make the node spend: a body no larger than a transaction needs; at most 8 sponsorships in
flight (`sponsor_busy`), and at most one per payer (`payer_open_in_flight`); and a **failure budget** —
once 8 co-signed opens have been sent and failed within an hour, every request is refused
(`sponsor_paused`) until the oldest ages out. A client can make its `open` fail after the simulation
passed, by moving its tokens away first, and the node pays that transaction's fee; the fee caps below
bound one such fee, and the budget bounds how many.

**Off unless configured.** A node without `[settlement.solana.batch_settlement]` answers `404`
`batch_settlement_not_offered`.

**Request.** Base64 of the transaction's wire bytes, legacy or version 0, signed by the payer alone with
the fee payer's signature slot left empty — exactly what a stock x402 client's
`buildOpenPaymentChannelTransaction` returns, and what x402 carries as `deposit.transaction`. Other
members are ignored, so x402's `deposit` object (`{"amount", "transaction"}`) is accepted as it stands.

```json
POST /ilp/batch-settlement/solana/open
Content-Type: application/json

{ "transaction": "<base64>" }
```

**Response.** `200` once the `open` has confirmed and the channel has been re-read and admitted:

```json
{
  "channelId": "<base58 channel PDA>",
  "transaction": "<base58 signature>",
  "payer": "<base58>",
  "deposit": "1000000"
}
```

**Submitted, not returned.** x402's facilitator validates, co-signs and broadcasts a client's `open`
(X402 SVM spec `#L379`, `#L1008`), and a stock client expects exactly that of the `feePayer`. Returning
the co-signed bytes would instead let the client choose when, and whether, this node's rent is spent,
and leave the node unable to re-read the channel it paid for, which x402 requires before reporting
success (`#L1143-L1146`).

**What it signs.** x402's acceptance policy for a client-supplied `open` (X402 SVM spec
`#L1017-L1150`), checked on the compiled message before any signature, and stricter where the spec
allows:

- the top level is an optional Compute Budget prefix — at most one `SetComputeUnitLimit` (≤ 400,000),
  then at most one `SetComputeUnitPrice` (≤ 100,000 microlamports, a fiftieth of x402's cap, because the priority fee is the node's even when an `open` a client has sabotaged fails on chain) — exactly
  one `open` of the configured `program_id`, then at most one account-less, UTF-8 Memo of ≤ 256 bytes.
  Nothing else: no other program, no Lighthouse assertion, no address lookup table;
- the required signers are exactly the fee payer and the `open`'s `payer`, both writable, and the
  payer's signature already verifies;
- the sponsor key is the fee payer, the `open`'s `rent_payer` and its `payee`, and appears nowhere else —
  not as `payer`, not as `authorized_signer`, not in any other slot or instruction, never as a program;
- no account is writable but the five an `open` writes;
- every field decision 2 fixes, and every account the canonical `open` names for those fields — the
  channel PDA, both canonical ATAs, the token, system and ATA programs, the rent sysvar, the event
  authority and the program itself.

Then, from the chain: this node's receiving account (its ATA for the mint) and the payer's canonical ATA
exist, are SPL Token accounts of the mint owned by the right key, and are not frozen — an unusable one
forfeits its payout to the program's treasury (Cantina 3.1.4) — and the payer's holds the deposit. Where
the cluster's rent `exemption_threshold` is not 1, the channel already holds the cluster's real
rent-exempt minimum (see "No rent prefund" below). Last, the exact co-signed transaction is simulated,
and only a clean simulation is sent. The account reads and the simulation are at `processed`, the
freshest state there is; the Rent sysvar is read once per process.

**Refusals.** `{"error": "<name>", "detail": "<text>"}`. `400` for a request that is not a
transaction; `422` for one the node will not sign, with nothing signed or sent; `503` when the chain
could not be read, with nothing sent; `502` when the co-signed `open` was sent and did not produce a
channel this node admits.

| Name                                                                                            | Status | When                                                                                                                                |
| ----------------------------------------------------------------------------------------------- | ------ | ----------------------------------------------------------------------------------------------------------------------------------- |
| `batch_settlement_not_offered`                                                                  | 404    | the Solana `batch_settlement` table is not configured                                                                               |
| `request_malformed`, `transaction_not_base64`, `transaction_too_large`, `transaction_malformed` | 400    | not `{"transaction": base64}` of one Solana transaction of at most 1,232 bytes, decoded exactly                                     |
| `address_lookup_tables_refused`                                                                 | 422    | the message uses an address lookup table                                                                                            |
| `fee_payer_not_sponsor`                                                                         | 422    | the fee payer is not this node's sponsor key                                                                                        |
| `unexpected_instruction`                                                                        | 422    | any instruction outside the layout above, or no `open`                                                                              |
| `compute_budget_refused`                                                                        | 422    | a Compute Budget instruction outside the bounds above                                                                               |
| `memo_refused`                                                                                  | 422    | a second Memo, or one with accounts, over 256 bytes, or not UTF-8                                                                   |
| `open_malformed`                                                                                | 422    | the `open`'s data or account list is not exactly what its fields imply                                                              |
| `sponsor_misused`                                                                               | 422    | the sponsor key appears anywhere but its three seats                                                                                |
| `unexpected_signers`                                                                            | 422    | the signers are not exactly the sponsor and the payer, both writable                                                                |
| `payer_signature_invalid`                                                                       | 422    | the payer's signature is missing or does not verify                                                                                 |
| `payee_not_sponsor`, `rent_payer_not_sponsor`                                                   | 422    | that seat is not this node's sponsor key (decision 5)                                                                               |
| `mint_not_settled`                                                                              | 422    | the mint is not `[settlement.solana] token_address`                                                                                 |
| `distribution_not_sole_receiver`                                                                | 422    | the distribution is not exactly this node's receiver at 10000 bps                                                                   |
| `grace_period_below_minimum`                                                                    | 422    | `grace_period` is below `min_grace_period_secs`                                                                                     |
| `deposit_below_minimum`                                                                         | 422    | the deposit is below `min_sponsored_deposit`, published as `extra.minDeposit`; it bounds the rent float (Cantina 3.1.9)             |
| `token_program_unsupported`                                                                     | 422    | the token program is not SPL Token; Token-2022's account extensions can fail a payout                                               |
| `open_account_mismatch`                                                                         | 422    | an account is not the one the canonical `open` names for that role                                                                  |
| `unexpected_writable_account`                                                                   | 422    | an account the `open` does not write is writable                                                                                    |
| `receiving_account_unusable`                                                                    | 422    | this node's receiving account is missing, frozen, or not the mint's for this node                                                   |
| `payer_token_account_unusable`                                                                  | 422    | the payer's canonical ATA is missing, frozen, not the mint's for the payer, or short of the deposit                                 |
| `cluster_rent_threshold_unsupported`                                                            | 422    | the cluster's rent `exemption_threshold` is not 1 and the channel holds less than the cluster's real rent-exempt minimum (below)    |
| `simulation_failed`                                                                             | 422    | the co-signed transaction fails simulation: an expired blockhash, an `open_slot` out of the program's window, a channel that exists |
| `sponsor_busy`                                                                                  | 503    | eight sponsorships are already in flight                                                                                            |
| `payer_open_in_flight`                                                                          | 409    | this payer already has a sponsorship in flight                                                                                      |
| `sponsor_paused`                                                                                | 503    | the failure budget is spent                                                                                                         |
| `chain_unavailable`                                                                             | 503    | the settlement RPC endpoint could not be read                                                                                       |
| `submission_failed`                                                                             | 502    | sent, and did not land                                                                                                              |
| `not_admitted`                                                                                  | 502    | landed, and the channel is not one this node admits                                                                                 |

**No rent prefund.** `payment-channels` computes a channel's rent without the cluster's
`exemption_threshold`, which is correct everywhere SIMD-0194 has set it to 1 and half the real figure on
an older cluster. **Sponsored opens are supported on clusters whose threshold is 1: mainnet-beta, devnet
and v3+ validators.** Elsewhere — any validator before v3 — the node reads the cluster's Rent sysvar
before signing and refuses an `open` whose channel holds less than the cluster's real rent-exempt
minimum as `cluster_rent_threshold_unsupported`, naming the threshold, the channel's balance and the
minimum, with nothing signed or sent. A channel that already holds that minimum opens there too, since
the program tops up only a shortfall. The node does not top the channel up itself, either way. In a
transaction of its own first, that would not be atomic with the client's `open`, so a client could have
the node prefund an address and then make its `open` fail, stranding the lamports where nobody can sign
for them. Inside the client's transaction, it would void the payer's signature, which covers the whole
message before the node sees it, and would put the sponsor key in a second instruction, which
`sponsor_misused` exists to refuse.

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
route, route termination, client edge, payment channel, claim, voucher, watermark, fee, price,
probe) and implements [ADR 0001](../adr/0001-rust-workspace-library-first.md) and
[ADR 0003](../adr/0003-clean-room-peer-wire-versioned-client-edge.md). It does not use
"terminator", "BLS"/"Business Logic Server", or "agent runtime" (all deprecated); it uses "app"
and "handler" for the payment-oblivious service behind a terminated route.
