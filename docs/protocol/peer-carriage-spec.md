# Peer carriage specification

**Status:** **Live — this is the peering specification** (wayfinder map #1049, issues #1065, #1073).
Its known stale citations are corrected: §5.3's `T04` claim (false since the cap landed — see
[ADR 0049](../adr/0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md)),
§5.3's and §6.4's citations of ADR 0031 (superseded in full by
[0042](../adr/0042-a-packet-carries-its-claim.md)), I7's "P1/P2 rule" (P1 has not decided role since
issue #868), and the semantics row in §0, which treated a now-frozen document as normative. Its
"Normative for the carriage mapping" scope survives: [ADR 0045](../adr/0045-a-behavioural-rule-is-normative-prose-until-its-vector-lands.md)
blesses exactly this narrower form, where prose binds a rule until a vector covers it and the vectors
win on any encoding disagreement. _Originally:_ Normative for the carriage mapping, in the same sense
[`peer-semantics-pre-868.md`](peer-semantics-pre-868.md) §3–§6 were said to be normative — this is an operator-to-operator
wire, and a third-party connector has nothing else to implement against. Subject to
[ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md) where bytes are concerned: **where
this prose and `vectors/wire-vectors.json` disagree about an encoding, the vectors are right and
this text is the bug.** §10 enumerates the vectors that must exist for that sentence to mean
anything.
**End-to-end money model**, of which the claim re-derivation here is one step:
[`money-model-pre-868.md`](money-model-pre-868.md).
**Implements:** [ADR 0027](../adr/0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md),
and §2.1's runtime half [ADR 0058](../adr/0058-a-peering-is-established-from-a-url.md).
This document carries ADR 0027's decisions through to the wire; it does not re-decide them. Where
it sharpens or resolves an ambiguity in that ADR it says so, in §12.
**Consumers:** issue #676 (the two carriage implementations behind the `PeerTransport` port),
issue #677 (the config schema), issue #678 (devnet bring-up), and any non-Rust connector that
wishes to peer with this fleet.
**Vocabulary:** [`CONTEXT.md`](../../CONTEXT.md). The key words MUST, MUST NOT, REQUIRED, SHALL,
SHOULD, SHOULD NOT and MAY are per RFC 2119.

---

## 0. What this document is, and its relationship to the surviving spec

ADR 0027 split one document into two layers.

| Layer                                                                                                                                        | Where it is specified                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| -------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Semantics** — what a peer interaction _means_: claim exchange, claim acknowledgement, claim contents, fees, reject codes, accumulated cost | **the records**, not a prose spec. [`peer-semantics-pre-868.md`](peer-semantics-pre-868.md) is **frozen history** (issue #1065): it claimed normative status over §3.2's trailing claim, §3.3's flush, §5.3's ceiling and §5.4's greeting gate, all retired or superseded. Its three live sections — §3.1, §4, §5.2 — migrate to the payment and packet-flow specifications. Authority meanwhile: [ADR 0010](../adr/0010-flat-per-packet-fee-and-minimum-delivery.md), [0011](../adr/0011-rejects-accumulate-fees-and-probes-discover-cost.md), [0042](../adr/0042-a-packet-carries-its-claim.md), [0049](../adr/0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md), [0051](../adr/0051-a-reject-code-binds-where-a-sender-must-act-differently.md), [0057](../adr/0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md) |
| **Carriage** — _where the bytes ride_ for each of those concepts, on each of the two wires a connector already serves                        | **this document**                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **Framing** — the deleted raw-TCP stream and its six frame types                                                                             | gone: `peer-semantics-pre-868.md` §1–§2, superseded by ADR 0027, implementation removed by issue #679                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |

**This document sits beside `peer-semantics-pre-868.md` §3–§6. It supersedes nothing in them.** It does
not restate them and MUST NOT be read as replacing them: every existing citation of §3.2, §3.3,
§3.4, §3.5, §4, §5.1, §5.2 and §5.3 — in the code, in ADRs 0010/0011/0024, and in
`client-edge-spec.md` — continues to resolve there, and this document cites them the same way. A
reader implementing a peer connector needs both: §3–§6 for what to do, this document for what to
put on the wire while doing it.

Where §3–§6 say "frame", read "whatever the configured carriage frames it as". This document is
that mapping.

### 0.1 The two carriages

A connector peers over one of the two carriages it already serves clients on, per ADR 0027:

- **BTP** — RFC-0023 over `wss://`, the frame grammar `client-edge-spec.md` §1.9 defines, decoded
  by the `connector-btp` crate extracted in issue #713.
- **ILP-over-HTTP** — `POST` over `https://`, the request/response shape `client-edge-spec.md`
  §1.1 and §1.3 define.

Which of them a connector _exposes_, and which it _dials_ for a given peer, is operator policy
(§2). Neither is a protocol constant, and a connector MAY expose both.

**One pipeline, two carriages.** Downstream of the carriage there is exactly one peer pipeline:
one route lookup, one `ClaimBook`, one journal, one fee policy, one refusal taxonomy.
A peer PREPARE that arrived over HTTP MUST be indistinguishable, everywhere below the
`PeerTransport` port, from one that arrived over BTP. Any observable peer behaviour that exists on
one carriage and not the other is a defect, not a carriage property — except where this document
names it as one (§6.4, §7.2). §9 states the invariants that hold this, and §10 the vectors that
enforce them mechanically.

---

## 1. Role is decided by authentication

This is the security core of this document and the property ADR 0027 spent to get here: ADR 0026's
proof-by-construction — "peers speak a different protocol on a different listener, so no client
trust can leak onto a peer session and no peer trust onto a client one" — is gone, and what
replaces it is code, on two carriages. Everything in this section is a stop-ship invariant.

### 1.1 Definitions

An **interaction** is either a BTP session (from its websocket upgrade to its close) or a single
HTTP request. Every interaction has exactly one **role**: `peer` or `client`. There is no third
role, no `unknown`, and no unroled state.

### 1.2 The rule

> **Amended 2026-08-07 by [issue #868](https://github.com/toon-protocol/connector/issues/868)**, the
> owner decision that _every peer packet carries a covering claim, or gets the 402 greeting_. **P1,
> the `{peerId, secret}` bearer credential, no longer decides role.** The argument P1 rested on is
> not deleted: it is kept, dated and marked superseded, at the end of this section.
> [Issue #863](https://github.com/toon-protocol/connector/issues/863) was filed because that
> argument was **absent** from this document, and deleting it now would recreate the very gap that
> issue named.

An interaction has role `peer` **if and only if both** of the following hold:

- **P2 — a channel binding.** The interaction is bound to a peer id `p` that has at least one
  `[[peer_channels]]` entry.
- **P3 — a verified claim on one of that peer's channels.** The frame carries a claim naming a
  `channel_id` that one of `p`'s `[[peer_channels]]` rows configures, and that claim's signature
  verifies against **the counterparty key that row configures** — never against anything the claim
  declares about itself.

If either fails, for any reason, the interaction has role `client`. **There is no fallthrough**: no
degraded peer, no peer-for-routing-but-client-for-claims, no retry into peer role.

There is no third case left to decide. Under #868 a peer PREPARE carrying no covering claim is not
admitted at all — it is answered with the same 402 greeting the client edge already gives. Role and
payment are therefore read from the same bytes, on the same packet, every time.

**Why a verified claim proves more than a bearer token does.** A claim's signature is checked
against this connector's **own** record of the channel — `counterparties` for an EVM channel, a
`SolanaChannel`'s `counterparty_public_key` for a Solana one — populated from `[[peer_channels]]`,
"never the claim's own self-declared field" (`ClaimBook`, in
`crates/connector-runtime/src/claim.rs`). The check itself is `ClaimBook::verify_signature`, which
answers `UnknownChannel` for a channel it holds no record of and `SignatureInvalid` for a signature
that does not recover to the configured key. A bearer secret proved only possession of a string both
operators had written into their own config files, presented by the dialer out of its own
`[[peers]]` row on the session's first MESSAGE. A signature over ADR 0024's balance proof proves
control of the key the channel was actually opened against — strictly stronger, and present on every
packet rather than once per session.

> **Tenses and citations corrected 2026-08-26 by [ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md)
> (issue #1157).** The bearer-secret sentence stood in the present tense and pointed at the two
> dialer lines that built the credential and put it on the first MESSAGE. **Nothing builds one
> now**: `connector_peer_btp`'s dialer constructs a `PeerRelation` carrying an endpoint, its
> channels' EIP-712 domains and this document's two timeouts, and no credential of any kind. The
> comparison is kept rather than dropped — it is the argument for P3, and #863 was filed because
> that argument was absent — but it is stated as a comparison against a surface that no longer
> exists. The line numbers went with it, here and in the two bullets below: every one of them had
> rotted onto unrelated code, and a symbol a reader can grep outlives a line a refactor moves.

**P3 resolves to exactly one relation.** A `channel_id` may appear in at most one
`[[peer_channels]]` row — a second is `PeerChannelDuplicate` at load, refused precisely so that
"whichever row's counterparty key won" cannot depend on iteration order
(`crates/connector-config/src/peer_channel.rs:285-293`) — and a channel in `[[peer_channels]]` may
never also appear in `[[client_channels]]` (§1.8, `ChannelInBothNamespaces`). A verified claim
therefore names one channel, one row and one `peer_id`, with no ambiguity for a caller to resolve
and none for an attacker to manufacture. That config-enforced uniqueness is what makes deciding role
from a claim safe, and it is why §1.3's former prohibition on doing so is withdrawn.

**What is retired, and what is not.** P1 was retired **as a role requirement** by the amendment
above, and the credential surface itself is retired with it by
[ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md) (issue #1157).
`[[peers]].credential` does **not** load: it is parsed solely in order to be refused **by name**
(`ConfigError::PeerCredentialRemoved`, `crates/connector-config/src/peer.rs`), so a node whose
committed TOML still sets one stops at boot rather than peering without it. A dialer presents
nothing (§1.4); `peer_auth_refused` names which of P2/P3 failed rather than a mismatched secret
(§1.6); and §12(7)'s "both operators write the same string" is superseded there — `[[peers]].id` is
a **local label**, and what the two operators MUST agree on is the channel. There is no replacement
credential: not renamed, not demoted to a label, not kept as an optional discriminator. §1.9's five
regression cases all still classify `client`, because none of them carries a verifying claim on a
configured peer channel; three of them stopped being expressible when the credential was deleted,
and are restated there in terms of the claim that now decides. Whether the credential surface
should exist at all was [issue #867](https://github.com/toon-protocol/connector/issues/867)'s
question, and ADR 0060 answers it: it should not.

**Implementation status.** This section's rule for _role_ **is** the code, as of issue #1157
([ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md)).
`connector_peer_auth::decide_role` takes the presented claim and the peering's channel bindings, and
answers `peer` only on P2 and a verified P3. It said otherwise here for six months: the P1/P2 branch
table survived #868's amendment to this section, so the weaker check gated the stronger one in a
tree whose spec already said it did not. That gap is closed and this paragraph records that it
existed.
`Connector::handle_peer_prepare` itself is unchanged and still accepts a `None` claim
(`crates/connector-runtime/src/connector.rs:667-676`): issue #880 lands the _price-coverage_ half of
this section (a `Terminated` route's own `price`, §3.1 — since joined by ADR 0042's
forwarded-arrival rule in the same gate) one layer up, in the accept pipelines
(`connector-peer-http`'s `PeerHttpState::handle` and `connector-peer-btp`'s
`PeerSession::handle_message`) -- before `handle_peer_prepare` is ever called, using the claim each
carriage already judges inline. Both call one decision,
`connector_peer_btp::price_gate::payment_required`, so §0.1's one pipeline cannot admit over one
carriage what it refuses over the other; each carriage keeps only the shape its own wire gives the
refusal. §3.1's former "a connector MUST NOT answer a peer-role PREPARE with the x402 greeting" was
corrected by #880 to state the rule that now runs. Issue #881 is the send
side: covering an outbound peer PREPARE with a claim in the first place. It lands in
`Connector::forward_via_peer_route` (`crates/connector-runtime/src/connector.rs`): a next hop
configured via `Connector::with_outbound_client_hop` is covered proactively, from the outbound
client ledger (#873), for this node's own forwarded value -- before the first attempt is ever sent,
not merely recovered by #875's retry arm after a refusal teaches this node it must pay. **A hop with
no such config is refused, not carried** (issue #1145): `cover_forward` has no not-configured arm,
the packet answers `T00` naming the hop, and `Config::load` refuses a route to such a peering by
name before the node serves at all. Until #1145 that case fell through to the peer ledger's
`pending_claim` — ADR 0004's postpay convention, the claim covering crossing _n_ signed after it
fulfilled and riding crossing _n + 1_ — and nothing arms one any more. ADR 0042's item 3 has
since extended the same shared gate to a `Forwarded` arrival, judged against the PREPARE's own
`amount` and defaulting to observe rather than refuse — §3.1 below states both rules and the one
per-peer knob that is left to select between them (the terminated rule's own `claim_enforcement`
knob was deleted by ADR 0042 item 4, issue #1077).

**What "configured via `with_outbound_client_hop`" means in a config file**, since for a while it
meant nothing an operator could write and the covering therefore never ran on a deployed node: it is
a **`[[pay_channels]]`** row (ADR 0042's item 2). One row per peering this node pays — `peer_id`,
the `channel_id` it pays from, that channel's `chain_id`/`token_network` (its EIP-712 domain, the
same two facts its `[[peer_channels]]` row carries, because both roles sign against the very same
on-chain channel), and `client_edge_url`: that hop's own `POST /ilp` endpoint, asked over
`POST /ilp/claim-state` (#693) for where this node's claims stand, on every covered packet. The
signing key is `[settlement.evm]`'s and no second key exists (ADR 0030). **The table is required of
any peering a `[[routes]]` entry forwards to** (issue #1145): a route naming a peer with no row is
`ConfigError::PayChannelUnbound`, refused at load naming the peer and the route. A peering this node
only accepts on needs no row -- the requirement is keyed on the route, not on the peering. It was
additive when it shipped, and stopped being so when the postpay path it fell back to was deleted.

**The row has a Solana shape too** (issue #1146), a different shape rather than a different spelling
of the same one, exactly as `[[peer_channels]]` and `[[client_channels]]` do: `channel_account` in
place of `channel_id`, no `chain_id` and no `token_network` (Solana has neither a numeric chain id
nor a per-token verifying contract), the same `client_edge_url`, and the signing key
`[settlement.solana]`'s. Its ADR 0053 binding — the settlement program id — is **not** a field of the
row: it is read from `[settlement.solana] program_id`, the one program this node can redeem through,
under issue #1128's rule. Before #1146 there was no such shape, so a Solana peering could only be
paid postpay.

A Solana row additionally requires that the same peering bind that channel account as a Solana
`[[peer_channels]]` row, refused at load naming the peer if it does not. `programId` is a required
field of the claim wire (§4.2) where an EVM claim's domain fields are optional and ride absent, and
both carriages render it from the peer-channel row; without one there is nothing to write there.
Holding one channel in both roles with one hop is the shape this table is for, so a real config
already satisfies it.

The hop being asked MUST answer that ask out of the book that judges the channel — for a channel it
holds as a `[[peer_channels]]` row, its **peer** book, not its client edge's (`client-edge-spec.md`
§1.10, issue #1102). One channel in both roles with one hop is the shape this table is for, and a hop
that answers out of a book no peer claim reaches reports nonce 0 forever: the payer then re-signs one
cumulative amount at a fresh nonce on every packet, each claim is accepted and advances nothing, and
a priced termination refuses everything after the first crossing with `F06`, `advanced = 0`.

#### Peer role is not a prerequisite for paid carriage

Stated here because #863 was originally filed while standing up an `apex-relay` peering, and implied
the opposite — that because peer role needed a shared credential, one connector paying another for
carriage needed one too. **It did not**, and leaving the correction to be inferred would re-teach
the error.

**The premise is now gone from both halves of that sentence** ([ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md),
issue #1157): there is no shared credential for peer role either, so the confusion that produced
#863 can no longer even be stated. This subsection survives that intact, with its conclusion
unchanged and its argument strictly shorter, because the distinction it draws was never about the
credential. It is about **configuration at the accepting side**: peer role requires some, and paid
carriage requires none. Deleting the credential changed what that configuration has to say — a
channel and a key rather than a matching string — and changed nothing about which of the two needs
it.

- A `[[peers]]` row is the **sending** node's own outbound config. It is what lets that node dial,
  and dialling is all it does; by itself it grants the sender nothing at the far end. Peer **role**
  does still require configuration by the accepting side — but **what the two files must share is a
  channel, not a name.** The accepting side must hold the on-chain channel the claims are signed
  against as a `[[peer_channels]]` row, naming the counterparty key that channel was opened
  against, bound to a peering that side itself configures (a row naming no configured peering binds
  nothing, and `PeerChannelOrphaned` refuses it at load). What it calls that peering is its own
  business: `[[peers]].id` is a **local label**, and a connector MUST NOT require the counterparty's
  file to spell the relation as its own does (§12(7), as amended by ADR 0060). Paid **carriage**
  requires nothing of the sort: the counterparty needs no `[[peers]]` row, no `[[peer_channels]]`
  row, and no peering with the payer at all.
- A connector may simply pay another as an ordinary client. Its `auth` frame, if it sends one at
  all, is _acknowledged, not verified_ at the far client edge — "Authorization to write comes from
  the claim on each packet, never from the session"
  (`crates/connector-client-edge/src/btp.rs`) — and the claim JSON a peer would have sent _is_ a
  client-edge claim, judged by the same `ClaimBook`. Since ADR 0060 that is one mechanism rather
  than two that agree: `connector_peer_btp::role_gate::decide` is the single place in this workspace
  where §1.2's P2/P3 rule meets the claim book's verdict on a signature, and both peer carriages
  **and** the client edge's shared front door call it. A claim is judged the same way whichever door
  it arrives at; what differs is only whether it also resolves to a peering.
- [ADR 0028](../adr/0028-a-forwarded-route-is-priced-at-the-client-edge.md) prices a **forwarded**
  route at the client edge: `Connector::client_route` reports a peer route's own `price` under
  `ClientRouteKind::Forwarded`, so the 402 greeting covers carriage and not only termination —
  "one that terminates here, whose `price` buys
  the app's work, and one that forwards over a peering, whose `price` buys the whole path"
  (`client-edge-spec.md:457-464`). Before that ADR a forwarded destination "was greeted with
  nothing, required no claim and was carried for free; that was a free gateway, not a design".

**Historical evidence, this fleet, 2026-08-07 — a configuration that no longer exists.** This
paragraph is kept as a dated record, not as a description of anything runnable today: the apex it
observed was removed by issue #872, and the `[announce]` section it names was deleted outright by
[ADR 0046](../adr/0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md)
(issue #1074). Read it for what it established about §1.2's rule, and do not expect to reproduce
it. Its citations have been re-pointed at the code that carries the behaviour now (issue #1191);
the behaviour survived the deletion, the file that held it did not.

The store box paid box 1 **as a client, not as a peer**, even though an `apex-store` peering _was_
configured between them. Both halves of that peering were in the store box's config — a `[[peers]]`
row and a `[[peer_channels]]` row in `infra/linode-store/connector-rust.toml` — and the box
nonetheless paid through what was then `[announce] pay_channel`, which that same file describes in as
many words as "a funded EVM channel this box PAYS … as an ordinary client … deliberately NOT a
`[[client_channels]]` row". The peer channel had nothing to claim against: the committed row was
still the issue #822 placeholder, and the live box's row named a real channel (`0x0bfd0b88…`) whose
deposit was 0, so a claim on it is refused before it is ever signed — `InsufficientHeadroom`, because
"a claim above what has actually been deposited could never be redeemed on chain"
(`crates/connector-runtime/src/outbound_client.rs:258-269` for the error, `:640-650` for the check).
It fell back to a client channel, and that fallback is the point: on that path every packet is covered
by a claim, and an uncovered one is answered `402` with the x402 terms
(`crates/connector-client-edge/src/lib.rs:754-782`, and `btp.rs:684`/`:725` for the BTP half).
#868's rule was already what ran in production on the link that mattered, with no shared credential
anywhere in it.

**Still true after #872, and more so.** The apex is gone and neither surviving box carries a
`[[peers]]`/`[[peer_channels]]` table at all, so the store box buys relay writes over exactly that
client path with no peering to fall back from (issue #871). The keys that configured it —
`[announce] publish_to` and `[announce] pay_channel` — are **not live configuration**: under ADR 0046
the section is `[node]`, carrying only `addresses`, `http_endpoint` and `btp_endpoint`, and each
announce-only key is now parsed in order to be refused by name at boot (`AnnounceKeyRemoved`,
`crates/connector-config/src/node.rs:213-229`; `AnnounceSectionRenamed`, `config.rs:394`, for the
section itself). The peer-carriage rules this spec states still describe what a peering must do;
this fleet simply has none to demonstrate them on today.

#### Superseded 2026-08-07 by #868 — the credit-window rationale for P1

Kept rather than deleted. It is the answer #863 was filed to obtain, and it is the reason the rule
above could not have been written before the decision that removed its premise.

> While a peer PREPARE could legally carry **no claim at all**, a claim signature could not carry
> the role. The receive path takes `claim: Option<WireClaim>` and treats `None` as
> `ClaimAckOutcome::NotSent` rather than a refusal
> (`crates/connector-runtime/src/connector.rs:667-676`). The send path emits claimless PREPAREs by
> construction: it attaches `pending_claim` (`crates/connector-runtime/src/connector.rs:996`), which
> answers `None` once the previous claim was acknowledged
> (`crates/connector-runtime/src/claim.rs:956-964`, and `:966-975` for why an acknowledgement clears
> `pending`), and a fresh claim is armed only by `record_fulfillment`, after a fulfil
> (`crates/connector-runtime/src/connector.rs:1009-1010`). Value consumed without a covering claim
> was recorded as uncovered exposure (`crates/connector-runtime/src/claim.rs:839-849`), bounded by
> `ceiling` (`crates/connector-runtime/src/connector.rs:678-689`,
> `crates/connector-domain/src/projection.rs:169-174`) and settled later on `flush_interval_ms`
> (`crates/connector-config/src/peer.rs:458-462`).
>
> That was the asymmetry. A **client** presented a covering claim per frame, with no configuration,
> flag or build profile able to disable it (`crates/connector-client-edge/src/lib.rs:26-31`). A
> **peer** was extended a credit window. So on precisely the packets that made peering _peering_ —
> the ones arriving between flushes — there was no signature to check, and something other than a
> claim had to carry the role.
>
> `ceiling = 0` did not recover the property either: `ceiling` is `Option<u64>` where `None` means
> unbounded (`crates/connector-config/src/peer.rs:379-380`) and the predicate is
> `exposure > ceiling` (`crates/connector-domain/src/projection.rs:172-173`), so exposure is still
> `0` when the check runs and exactly one uncovered packet is admitted before `T04`.

#868 removes the premise rather than answering the question: with a covering claim on every peer
packet there is no claimless packet left for P1 to cover. The disposition of the exposure machinery
itself — `record_inbound_delivery`, `ceiling`, `flush_interval_ms` — was
[issue #882](https://github.com/toon-protocol/connector/issues/882)'s, not this document's: it landed
as removal, not restatement ([ADR 0033](../adr/0033-the-exposure-machinery-is-retired-not-restated.md)).
The three names above no longer exist in `crates/` — `ceiling`/`flush_interval_ms` are parsed only
as removed-field traps — and are described above only as the historical shape P1's justification
argued from.

### 1.3 What MUST NOT enter the decision

A connector MUST NOT infer, weight or override role from any of:

- the carriage (BTP vs HTTP), the listener, the port, or the bind address;
- the source address, the TLS SNI name, or the presence of a TLS client certificate;
- whether the `btp` websocket subprotocol was offered or selected;
- a hostname or endpoint appearing in `[[peers]]`;
- the shape of what the interaction sent — an inbound TRANSFER, or any carriage-layer entry;
- anything the interaction did earlier, or that another interaction from the same address did.

Role is decided by P2 and a verified claim, or it is `client`.

> **Withdrawn 2026-08-07 by #868.** The fifth bullet used to end "…or a claim naming a channel that
> happens to be in `[[peer_channels]]`". That prohibition is now the exact inverse of the rule:
> under §1.2 a claim naming a configured peer channel, **whose signature verifies against that
> row's counterparty key**, is what decides role. The word carrying the weight is _verifies_ —
> "happens to be in `[[peer_channels]]`" describes a claim taken at face value, and a claim is never
> taken at face value (`crates/connector-runtime/src/claim.rs:1055-1089`). Every other entry on this
> list is unchanged and still forbidden; one bullet moved, and it moved because the credit window it
> was written under is gone, not because face-value inference became acceptable.

### 1.4 Presentation, on each carriage

**There is nothing to present.** A peering carries no credential
([ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md), issue #1157).
The `{peerId, secret}` object, the BTP `auth` protocolData entry as a **peer** credential, and the
`Toon-Peer-Auth` request header are all deleted, on both carriages together — peer behaviour that
exists on one carriage and not the other is a defect rather than a property of the carriage (§0.1,
[ADR 0027](../adr/0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md)).

What a peer presents is what every peer frame already carried: **its covering claim**
([ADR 0042](../adr/0042-a-packet-carries-its-claim.md)), in the shapes `client-edge-spec.md` §1.9
already pins. Role is read from those same bytes, on the same packet, every time — §1.2's P2 and P3.

A connector MUST NOT refuse a request that still carries a `Toon-Peer-Auth` header or an `auth`
protocolData entry naming a peer. It ignores the value. That is what lets the two ends of a peering
be upgraded in either order, without the peering going dark mid-flight while one side still sends a
field the other has stopped reading.

`client-edge-spec.md` §1.9 step 1's `auth` entry is untouched **as a client-edge mechanism**, along
with its "the contents are not verified" and its permissionless empty-`secret` mirror. Nothing in
this section constrains the client role, and an interaction that proves no peering is a client
(§1.2).

Because HTTP has no session, a request is judged on its own claim. A request carrying none is a
client request, whatever the previous request from the same connection carried.

### 1.5 Binding, and the anti-escalation rules

> **Inverted 2026-08-07 by #868.** The last bullet of this section used to require role to be fixed
> _before_ a claim is decoded. **That ordering existed because claimless peer packets existed**; it
> falls with them. The claim moves from _after_ the decision to _inside_ it. What the bullet was
> actually protecting — that nothing downstream re-derives role, and that no money and no state move
> before role is known — is preserved below, unchanged in force. The session-binding bullet inverts
> with it, for the same reason: a per-session credential fixed a per-session role, and a per-packet
> claim fixes a per-packet one.

- **Role is decided from the claim, not before it.** A connector MUST decode and verify the frame's
  claim first; the verification result is what P3 reads. Role MUST still be fixed **before the
  packet is routed, before a fee is taken, before a ceiling is consulted, and before any watermark
  is advanced or anything is journaled.** That ordering already holds in the claim path as written:
  `accept_inbound_inner` verifies the signature and returns on failure before it so much as reads a
  watermark, so a claim that does not verify advances nothing
  (`crates/connector-runtime/src/claim.rs:1116-1131`). Nothing downstream of the `PeerTransport`
  port may ask which carriage or which credential produced the interaction; it is handed a role.
- **Role is a property of the frame, not of the session.** On BTP a session no longer becomes
  `peer` once and stay so: each frame stands on the claim it carries, and a frame carrying no claim
  that satisfies P2 and P3 is a client frame however many peer frames preceded it on that socket.
  This is strictly narrower than the rule it replaces — a session could previously present one
  credential and then send anything.
- **Frames not admitted as peer frames MUST NOT be retroactively reclassified.** A claim ingested
  as a client claim stays a client claim, and its effects on client watermarks stand. §1.8's
  namespace disjointness is what keeps that safe: the two namespaces can never describe the same
  channel, so a frame judged in one can never be re-judged in the other.
- **A second `auth` entry on a session MUST NOT be evaluated** — nor a first one. There is no
  session-bound role left for one to escalate (bullet 2 above), and no credential left in the entry
  to read ([ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md));
  §1.4 requires it be **ignored** rather than answered with an error, so that the two ends of a
  peering can be upgraded in either order.
- **Ambiguous claims are refused, not resolved.** More than one claim entry on a single BTP frame,
  or more than one claim header on a single HTTP request, MUST refuse the frame or request — BTP:
  an ERROR frame as above; HTTP: `400`, with no ILP body. The connector MUST NOT pick the first,
  the last, or a concatenation. This is the smuggling defence, and its absence is how "which claim
  did we verify?" becomes unanswerable.

> **Restated by [ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md)
> (issue #1157).** The ambiguity bullet used to protect the `auth` entry and the `Toon-Peer-Auth`
> header, and #868 had already demoted it from a role rule to hygiene on a surface it expected #867
> to dispose of. #867 never did; ADR 0060 deleted the surface instead. The rule survives because the
> defect it names — an unanswerable "which one did we check?" — is a property of duplicated
> authentication material, not of the credential in particular, and the claim is now that material.

### 1.6 An asserted role is not a proven one

A claim that names a channel a `[[peer_channels]]` row configures, but fails P2 or P3, is an
**assertion**. The connector:

- MUST treat the interaction as a client, per §1.2;
- MUST NOT refuse it for the assertion alone — refusing would make the check an oracle for which
  channels this connector has configured;
- MUST NOT record, log, meter or expose it as a peer interaction anywhere (`ADR 0014`'s metric and
  log surfaces included); and
- MUST emit a distinguishable, rate-limited operator-visible event — `peer_auth_refused`, carrying
  the peering's configured peer id and which of P2/P3 failed — because the most likely real cause
  is a genuine peer whose key was rotated on one side only, or a `[[peer_channels]]` row this node
  holds no chain record for, and a silent downgrade to client role would otherwise present to an
  operator as "peering configured, nothing peers, no error anywhere."

The two failures are deliberately **not** one bucket, because they have different fixes. **P2**
(`UnknownChannel`) means config names a channel the claim book has no record of — config and chain
disagree, which is this node's wiring rather than the caller's. **P3** (`SignatureInvalid`) means a
record exists and the signature did not recover to the counterparty key that row configures — a
rotated key, or somebody else's claim. A shared secret gave one message for both.

### 1.7 What each role grants

Stated as an enumeration because "peer trust" and "client trust" are otherwise undefined, and
undefined trust is what leaks.

**Peer role grants, and only these:**

- claims judged against `ClaimBook` and the `[[peer_channels]]` records, advancing peer watermarks
  and appended to the peer claim ledger;
- being a next hop: packets from this interaction may be forwarded per the routing table, and this
  peering relation may be a route's next hop;
- `accumulatedCost` relayed with this hop's own fee added (`peer-semantics-pre-868.md` §5.2);
- FLUSH accepted (§6).

**Peer role does NOT grant:** free carriage; a route the routing table does not have; any operator
or admin surface (ADR 0008); any exemption from sealing (§8); any say in this connector's fees or
a route's price; nor the ability to open the payload of a packet it forwards.

**Client role does NOT grant, and a connector MUST refuse these to a client interaction even when
it presents bytes that look like them:**

- advancing a `[[peer_channels]]` watermark or writing to the peer claim ledger;
- a `claim-ack` / `Toon-Claim-Ack` on a client response — a connector MUST NOT emit one on a
  client interaction;
- being treated as a peering relation for flush purposes (§6.4).

### 1.8 Namespace disjointness

Peer watermarks and client watermarks are **separate records**, keyed in separate namespaces, even
for the same on-chain channel id. A connector MUST NOT let a claim judged in one namespace advance
a watermark in the other.

To make that safe rather than merely separate — two namespaces over one channel would otherwise
let the same claim be counted as credit twice — **a channel id configured in `[[peer_channels]]`
MUST NOT also appear in `[[client_channels]]`, and a configuration containing both MUST fail at
load** (§11, `ChannelInBothNamespaces`). Disjointness is enforced in config, so the two namespaces
can never describe the same money.

### 1.9 The named regression

The invariant exists because the TypeScript fleet violated it. `toon-sandbox` admitted an
anonymous BTP session with `btp_auth … success:true mode:"no-auth"` and then treated it as a
quasi-peer (the ingress findings ADR 0027 cites).

**Both carriages MUST carry a stop-ship regression test named for it**, asserting that each of the
following is classified `client` and reaches no peer handling whatsoever:

1. an interaction carrying no claim at all;
2. an interaction carrying a claim on a channel no `[[peer_channels]]` row configures;
3. an interaction carrying a claim on a configured channel whose signature does not recover to the
   counterparty key that row configures (P3 failing);
4. an interaction carrying a claim on a channel a `[[peer_channels]]` row configures but this node
   holds no record of (P2 failing);
5. an interaction carrying a claim on a `[[client_channels]]` channel — the namespace-disjointness
   case of §1.8, which can never be read as a peering.

> **Rewritten by [ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md)
> (issue #1157).** The list previously enumerated credential shapes — no credential, an empty
> `secret`, a correct `peerId` with a wrong one, and a valid credential naming an unconfigured peer.
> All five still classify `client`; three of them stopped being **expressible** when the credential
> was deleted, so the cases are restated in terms of the claim that now decides. The invariant and
> the reason for it are unchanged, and so is the requirement that both carriages carry the test.

"Reaches no peer handling" is testable as: no peer watermark moved, nothing was appended to the
peer claim ledger, and no `claim-ack` was emitted. (Before ADR 0033 this list also named
peer-relation exposure, which no longer exists to change.)

### 1.10 The dedicated-listener fallback

ADR 0027 names one escape hatch, and it is bounded here so it is not invented under pressure. If
role-by-auth cannot be shown safe on a shared listener, a connector MAY expose a **dedicated peer
listener with mandatory authentication**. If it does:

- role is **still** decided by P2 and P3 on that listener. The listener is defence in depth and
  MUST NOT become the decider — §1.3 still holds in full;
- an interaction on that listener that fails P2 or P3 MUST be **refused outright** (BTP: ERROR then
  close; HTTP: `401`) rather than downgraded to client. This is the single place refusal replaces
  downgrade, and it is safe only because a dedicated peer listener serves no clients, so there is
  no client to downgrade to and no oracle to leak (the peer ids it protects are the ones already
  advertised to the peer that dials it);
- it MUST still be BTP or ILP-over-HTTP. Never a bespoke wire, never raw TCP.

---

## 2. Expose and dial are separate axes

### 2.1 The axes

- **`expose`** — which peer carriages this connector opens a listener for. A subset of
  `{btp, http}`, including the empty set. **The empty set is legal and meaningful**: a connector
  behind NAT exposes nothing and only dials.
- **`dial`** — per peering, which carriage this connector reaches _that peer_ on. Determined
  **solely by the scheme of that peering's `endpoint`**: `wss://` → BTP, `https://` → HTTP. Any
  other scheme MUST be an error where the peering is declared. A peering with **no** `endpoint` is
  accept-only from this connector's point of view: this connector never dials it, and it dials us.

  "Where the peering is declared" is load time for a `[[peers]]` row and **write time** for a
  peering established over the operator surface, whose endpoint is read from the counterparty's
  self-description ([ADR 0058](../adr/0058-a-peering-is-established-from-a-url.md)). One rule, two
  moments: a peering added while the process serves selects its carriage by this same sentence, and
  a connector MUST be able to add and remove a dial carriage without a restart. A runtime peering
  whose endpoint scheme selects no carriage this connector dials registers none, and packets routed
  to it get §2.2's `T01` with the peer named.

  **A host ending in `.onion` or `.anyone` permits the plaintext schemes** ([ADR 0070](../adr/0070-an-onion-address-is-a-host-not-a-carriage.md),
  amended by issue #1284): `ws://` → BTP and `http://` → HTTP when, and only when, the endpoint's
  host has one of those suffixes. This adds no carriage — an onion address is a **host**, and both
  carriages ride it unchanged — and it is independent of `peer_allow_plaintext_endpoints`, which
  keeps its own meaning and its own scope. The exemption is narrow because of what a v3 onion
  address is: the address _is_ the ed25519 public key the circuit is authenticated to, so such an
  endpoint authenticates itself and ADR 0004's requirement is satisfied by a different mechanism
  rather than waived — which is why the two spellings are one rule, since `anon` renamed the TLD it
  publishes without changing what an address is. At every other host, a plaintext scheme remains the
  error this sentence's second clause requires.

These are independent. Exposing BTP says nothing about how any peer is dialed; dialing a peer over
HTTP says nothing about what this connector listens on.

### 2.2 The intersection rule

**A peering establishes only if at least one side dials a carriage the other exposes.** Two
operators who each expose only the carriage the other cannot dial simply cannot peer, and no
amount of retrying changes that.

Where the failure is detectable from this connector's own configuration alone, it MUST be a
**named load-time error**, never a runtime mystery (§11):

- this connector exposes nothing **and** a configured peer has no `endpoint` — a declared peering
  that can never establish (`PeerUndialable`);
- a route names a peer as its next hop that this connector can never originate to (§2.4)
  (`PeerRouteUndeliverable`).

What is **not** locally detectable — whether the remote actually exposes what we dial — MUST
surface as an ordinary dial failure with the peer id and the attempted endpoint named, and packets
routed to that peer MUST reject `T01` (`peer-semantics-pre-868.md` §5.1), never `T00` and never a silent
drop.

### 2.3 Origination

**A connector can only originate a request to a peer it can dial, on HTTP.** On BTP a dialed
session is symmetric once established: after auth, either side may originate a MESSAGE or a
TRANSFER (`client-edge-spec.md` §1.9, "Symmetric grammar"). This is the whole of the difference
between the carriages, and everything in §6.4 and §7.2 follows from it.

| Configuration                                           | Who can originate                              |
| ------------------------------------------------------- | ---------------------------------------------- |
| A dials B over `wss://`                                 | both A and B, on the one session               |
| A dials B over `https://`, B does not dial A            | A only                                         |
| A and B each dial the other over `https://`             | both, on their own outbound connections        |
| A dials B over `wss://`, B also dials A over `https://` | both — and the peering has two paths; see §2.5 |

### 2.4 The NAT consequence, and the HTTP limit

An operator behind NAT exposes nothing and must dial out. It can hold an inbound-capable session
only over a persistent socket, so it must dial **BTP**. Therefore:

> **An HTTP-only peer can neither reach nor be reached by a NAT'd peer.** The NAT'd side can only
> dial, so it needs the counterparty to expose something; and it can only receive over a persistent
> session, so that something must be BTP. An operator who exposes HTTP only has chosen to peer
> exclusively with dialable counterparties.

This is a property of the HTTP carriage, not a defect scheduled for repair. It is why BTP is the
recommendation for anything resembling a fleet link (ADR 0027).

### 2.5 One peering relation, however many paths

The fee, the claim watermarks and the claim ledger are **per peering relation, not per carriage and
not per connection**. A peering that happens to have two paths (last row of §2.3) is still one
relation with one set of watermarks. A connector MUST NOT maintain per-carriage watermarks for one
peer; doing so is a double-spend surface, since the same claim would advance two independent
watermarks.

Where two paths exist, a connector SHOULD prefer the BTP path for claim-bearing traffic, because
it is the one on which claims cannot race (§7).

### 2.6 A held session is not the peering

A connector that keeps a dialed session between packets — which BTP invites, since the session is
the point — holds a cache, not the relation. The relation is §2.5's; the session is one path it
currently has open, and the counterparty may take that path away at any time by restarting.

> **A connector MUST NOT answer `T01` on a session it can already tell is dead.** §2.2's `T01` is
> the answer to a dial that was **attempted and failed**. A held session that has died is not a
> dial failure; it is a cache miss, and the packet is owed a fresh dial before any answer is
> formed.

A connector MAY additionally send a frame **once** more on a new session when the send on the held
one failed in a way that proves the frame was never written. It MUST NOT retry a frame that was
written and went unanswered: a timeout means the counterparty may be acting on it, and a second
copy is a second packet. A claim riding such a resend is §6.3's byte-identical retransmission and
carries the same nonce, which is exactly the case §6.3 requires a payee to answer `accepted`.

> **Recorded 2026-08-28 (issue #1240).** Observed twice on the devnet relay, the second time under
> deliberate reproduction: the store's connector restarted, and the relay's next packet to it was
> refused `T01 peer 'store' unreachable` with **no dial attempted**, the one after it crossing and
> fulfilling seconds later with nothing changed in between. The peering was registered and the
> endpoint was sound; what was stale was the held session.
>
> The cost is the refused packet, and — on the evidence — only that. The issue as filed also said
> the refusal burned a claim nonce, because the payer mints the covering claim before the send
> fails. The reproduction does not bear that out: both packets signed the same nonce over the same
> cumulative, and the second was fulfilled. A claim covering a packet that was never written is not
> banked at either end, so the payer retransmits it rather than advancing past it — which is §6.3's
> byte-identical retransmission arriving by a different road.

---

## 3. Frame carriage

The normative mapping. Each row is a concept from `peer-semantics-pre-868.md` §3–§6 and where its bytes
ride on each carriage.

| Concept (`peer-semantics-pre-868.md`) | BTP carriage (`wss://`)                                                                                                                    | ILP-over-HTTP carriage (`https://`)                                                                                 |
| ------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------- |
| PREPARE (§3.1)                        | **MESSAGE** (type 6), OER PREPARE in `ilpPacket`                                                                                           | **POST**, OER PREPARE as the request body                                                                           |
| FULFILL (§3.1)                        | **RESPONSE** (type 1) under the MESSAGE's `requestId`, OER FULFILL in `ilpPacket`                                                          | **200**, OER FULFILL as the response body                                                                           |
| REJECT (§5.1)                         | **RESPONSE** under the MESSAGE's `requestId`, OER REJECT in `ilpPacket`                                                                    | **200**, OER REJECT as the response body                                                                            |
| piggybacked claim (§3.2)              | `payment-channel-claim` protocolData entry, **raw UTF-8 JSON** (§4)                                                                        | `ILP-Payment-Channel-Claim` request header, `base64(JSON)` (§4)                                                     |
| **FLUSH** (§3.3)                      | **TRANSFER** (type 7): `amount` = the claim's new cumulative, claim in `payment-channel-claim`, no `ilpPacket`                             | **POST with an empty body** plus the claim header — the standalone-claim shape of `client-edge-spec.md` §1.9 step 5 |
| **CLAIM_ACK** (§3.4)                  | `claim-ack` protocolData entry on the RESPONSE that already answers the claim-bearing frame (§5)                                           | `Toon-Claim-Ack` response header on the response that already answers the claim-bearing request (§5)                |
| `accumulatedCost` (§5.2)              | `toon-accumulated-cost` entry on the REJECT's RESPONSE, decimal-uint64 UTF-8 — **already implemented on the client edge, reused verbatim** | `Toon-Accumulated-Cost` response header — **already implemented on the client edge, reused verbatim**               |
| flush prompt (§6.4)                   | _(none — the payee can originate on BTP)_                                                                                                  | `Toon-Flush-Requested` response header, optional (§6.4)                                                             |

Header names are matched case-insensitively per RFC 9110; the canonical lower-case forms are the
ones the vectors pin.

**A peer connector MUST NOT invent additional entries or headers.** A protocolData entry or header
this document does not name MUST be ignored on receipt (never refused, so the carriage stays
additively extensible) and MUST NOT be emitted.

### 3.1 What this table does not change

- **The ILP packets themselves are unchanged**, byte for byte, on both carriages: the same OER
  encodings `POST /ilp` already carries (`client-edge-spec.md` §1.1), the same as the deleted peer
  wire carried in its §2. `vectors/wire-vectors.json`'s existing envelope and fulfilment sections
  were never peer-specific and are not re-derived here.
- **ADR 0024's EIP-712 `BalanceProof` digest is untouched**, on both carriages. A peer claim signs
  exactly the digest `connector_signer::evm_balance_proof_digest` produces today, over exactly the
  fields the deployed `TokenNetwork.sol` typehash requires, `lockedAmount`/`locksRoot` included and
  hashed as zeros (`peer-semantics-pre-868.md` §3.5). Only carriage moves.
- **`peer-semantics-pre-868.md` §5.1's reject-code table is unchanged, but `F06_UNEXPECTED_PAYMENT` now has
  one peer use** (issue #880, correcting what this bullet said before it landed): a peer PREPARE
  addressed to one of this node's own **`Terminated`** routes, reached over either carriage, MUST
  carry a claim whose advance over that channel's watermark covers what that route charges **for
  that packet** -- its `price` evaluated at the packet's own `data` length, which for a flat price
  is the `price` itself ([ADR 0065](../adr/0065-a-price-is-a-schedule-over-payload-length.md)) --
  or it is
  refused `F06` with the x402 greeting of `client-edge-spec.md` §1.4 attached exactly as the client
  edge's own BTP carriage attaches it -- `payment-required` protocolData (BTP) or a `payment-required`
  response header, base64 (HTTP) -- built by the one shared emitter
  (`connector_domain::x402::terms_body`), never a second wire shape. Where the route prices by
  size that greeting quotes this packet's own figure as `amount` and publishes the schedule beside
  it (`extra.price` + `extra.pricePerKib`), so a peer refused once can price its next packet
  without being refused again. This is the same rule the
  pre-existing amount check right beside it in `Connector::handle_peer_prepare` already enforces
  against `prepare.amount`, extended to require that value be _proven_, not merely declared -- owner
  decision #868's "every packet is paid, or it gets the 402 greeting" applied to the one place a
  peer PREPARE is priced independently of the bilateral fee. A route explicitly priced at `0` is
  untouched, exactly like the amount check.

  **Which record "that channel's watermark" means**, since reading the wrong one is free service:
  the **durable** one, out of the book that judges the claim — `ClaimBook`'s own inbound watermark
  for that channel, keyed by channel alone as that book keys it, read before the claim is judged and
  so possibly advanced. Never a per-process record of what a carriage has observed. `AcceptedClaims`
  holds one of those for §6.3's byte-identical re-ack, which is correctly a per-relation, per-process
  question, and a restart empties it while the book replays its journal (ADR 0005). Coverage measured
  against the emptied record read zero, so the first priced peer PREPARE after a payee restart was
  credited with its claim's whole cumulative amount instead of its advance — one packet per restart
  per channel, at the gate standing between a priced termination and free service
  ([issue #1104](https://github.com/toon-protocol/connector/issues/1104)). The claim book's verdict
  does not catch this on its own: such a claim's nonce and amount both genuinely advance, so it is
  `accepted`; only the baseline was wrong.

  **A `Forwarded` arrival must cover its own `amount`** ([ADR
  0042](../adr/0042-a-packet-carries-its-claim.md) item 3, correcting what this bullet said while
  only the `Terminated` rule existed). A peer-role PREPARE this node will forward onward MUST carry
  a claim whose advance over that channel's watermark is at least the PREPARE's own `amount`, or it
  is refused `F06` with the same x402 greeting, quoting that amount. The figure is the packet's
  `amount` and nothing else:
  - **Not the route's `price`.** A `[[routes]]` entry naming a `peer_id` carries a `price`, and a
    **client-role** PREPARE to it is greeted, claim-gated and journaled exactly as one to a
    terminated route is (issue #620, [ADR
    0028](../adr/0028-a-forwarded-route-is-priced-at-the-client-edge.md)). That is the
    client-facing direction of the same node: the `price` is a fact about this node's client edge,
    and it is not what a peer owes.
  - **Not the `fee`, and not the post-fee amount this hop passes on.** The send half covers the next
    hop for `amount_after_fee(amount, fee)` (`Connector::forward_via_peer_route`), so a peer
    covering the amount that _arrives_ leaves this
    node exactly its bilaterally agreed flat fee. Peer fees stay bilateral configuration
    (`peer-semantics-pre-868.md` §4) and are not negotiated by this greeting; `requiredTransport`
    (issue #701) remains a client-edge route policy with no peer analogue.

  **This rule ships defaulting to observe.** Its per-peer knob is `forwarded_claim_enforcement`,
  and `"observe"` is its default: an uncovered forwarded arrival is admitted, forwarded, and logged
  exactly as a refusal would be logged, until an operator writes `"enforce"` on that peering. ADR
  0042 records why -- no box on this fleet covers its forwards yet, and each forwards to the other,
  so enforcing by default would stop forwarding fleet-wide.

  **The `Terminated` rule above has no such knob and never gets one.** It once had a mirror,
  `claim_enforcement`, whose `"observe"` was issue #883's canary step for the issue #880 rollout;
  it is **deleted** (ADR 0042 item 4, issue #1077) and the key is now parsed only to be rejected by
  name. An uncovered arrival to a priced termination is refused under every setting a peering can
  carry. Keeping the two knobs as separate fields rather than one is what let that deletion happen
  without taking the forwarded rule's opposite default with it.

  **A destination that resolves to no configured route is still gated by nothing**, and that
  includes a **leased** route: `Connector::client_route` excludes leases by construction (ADR 0028),
  so neither rule here reaches one. That is ADR 0028's own gap, unchanged by ADR 0042.

  **Neither rule reaches a peering established at runtime, because on the accepting side there is
  no peering** ([ADR 0058](../adr/0058-a-peering-is-established-from-a-url.md), whose dial half §2.1
  already carries). `POST /peers` is one operator's own write on one node: it derives the channel,
  binds it into that node's `ClaimBook` and registers the outbound client hop that pays over it
  (`Connector::bind_runtime_peer_channel`, `Connector::register_outbound_client_hop`), and it puts
  nothing into the counterparty's configuration -- ADR 0058 makes a peering establishable **from** a
  URL, not **on** somebody else's node. Role there is still §1.2's P2 and P3, and P2 reads
  `[[peer_channels]]`: `PeerAuthPolicy::from_config` is built once from the loaded config and no
  runtime write mutates it, so a claim naming a runtime-derived channel binds no peering and the
  arrival is a **client** arrival. On a node whose own config declares no `[[peers]]` row at all,
  no peer handling is even mounted (`PeerCarriages::from_config` answers `None`), whatever
  `peer_expose` says. This is §1.2's "Peer role is not a prerequisite for paid carriage" reached
  from the other direction: the payer is paying an ordinary client edge, which is the shape the
  runtime peering was built to work in.

  **So over a runtime peering the refusal is the client edge's, and it is not `F06`.** An arrival
  carrying no claim is greeted with the x402 terms of `client-edge-spec.md` §1.4 -- the same
  `connector_domain::x402::terms_body` emitter, carrying this node's identity and settlement facts
  where the peer greeting above quotes the figure alone. An arrival whose claim **under-covers** the
  route's price is `F03` (Invalid Amount) with that price in `accumulatedCost`, not `F06` with a
  greeting attached (`ClaimIngestRejection::Underpayment`, `client-edge-spec.md` §1.3). Both
  differences are the client edge's own taxonomy, and neither is a hole in §0.1's one pipeline or a
  drift under I7: the two peer carriages still answer this rule identically, and what changed is
  which **edge** the packet arrived at, not which wire it rode. An operator who wants §3.1's rule to
  govern what a counterparty sends writes `[[peers]]` and `[[peer_channels]]` on the **accepting**
  side; there is no runtime write on the payer's node that can put them there.

### 3.2 The `WireClaim` binary encoding is not used on either carriage

`connector_runtime::WireClaim::encode`'s length-prefixed binary form was the deleted peer semantics's ad
hoc encoding. Neither carriage uses it. Both carry the JSON of §4. `WireClaim` remains an in-process
type above the `PeerTransport` port; a carriage converts to and from it and MUST NOT put its
`encode()` bytes on a wire.

---

## 4. The claim on the wire

One claim shape, one JSON encoding, two transfer encodings.

A peer claim is the **same JSON object** `client-edge-spec.md` §1.3 defines for a client claim:
`version: "1.0"`, discriminated by `blockchain`, with the required fields (`version`, `blockchain`,
`messageId`, `timestamp`, `senderId`) and the chain-specific fields of that section. This is not a
convenience: it is why one claim codec, one structural validator and one signature verifier serve
both edges, and why a change to the claim shape cannot land on one and not the other.

- **BTP**: the `payment-channel-claim` protocolData entry carries `JSON.stringify(claim)` as **raw
  UTF-8** — no base64 layer. This is verbatim the client edge's existing convention
  (`client-edge-spec.md` §1.9 step 2).
- **HTTP**: the `ILP-Payment-Channel-Claim` request header carries `base64(JSON.stringify(claim))`.
  Base64 is a header artifact and nothing more.

**The privacy-wrapped carriage (`ILP-Payment-Channel-Claim-Wrapped`, NIP-59) is not part of the
peer carriage on either wire.** A peering relation is configured on both ends by operators who know
each other's channel identity, so the anonymity it buys has no peer use. A connector MUST ignore
that header on a peer-role request.

### 4.1 Validation

A peer claim is validated by the same gate, in the same order, that
`peer-semantics-pre-868.md` §3.2 and §3.4 and `client-edge-spec.md` §1.3 already describe — structure,
then freshness against the watermark, then value, then cryptography — with the peer-side
differences that were already true and are unchanged by carriage:

- the record it is checked against is the `[[peer_channels]]` entry for this peering relation
  (§1.7, §1.8), never a client channel record and never anything the claim says about itself;
- the channel-id canonical form of `client-edge-spec.md` §1.3 step 2 applies identically —
  `0x` + 64 lower-case hex for `evm`, the base58 `channelAccount` as it arrives for `solana` — and
  MUST be applied before a watermark is read or written. A connector that keyed a peer watermark by
  literal text would grant a fresh watermark per spelling, and one signed claim would buy carriage
  once per casing it was retyped in;
- the four refusal reasons are `peer-semantics-pre-868.md` §3.4's four, unchanged (§5.2);
- a Solana claim's declared `programId` is validated structurally and then **discarded**, and a
  disagreement with the program the channel lives under is **not reported**. This is the one place
  a peer claim is judged by a different rule from the client claim it is byte-for-byte the same
  object as, and the next heading says why.

#### The declared `programId` is not reported on this edge — and why that differs from the client edge

**Normatively.** A peer claim's `programId` MUST carry exactly what `client-edge-spec.md` §1.3 pins
— the settlement program its `channelAccount` lives under, byte-for-byte the program id the payer
put into the balance proof its `signature` covers (ADR 0053, offset 16). §4 imports that field list
verbatim and there is no peer spelling of it. A connector **MUST NOT refuse** a peer claim on a
disagreement, for the same reason it must not on the client edge: the signature is verified against
the channel's own program either way, so a claim that reaches the verifier is one whose payer signed
for this node's program whatever they wrote in the field, and refusing it would refuse money the
node can collect. Unlike the client edge, a connector is **not required to report** one. This
connector does not: `connector_peer_btp::claim_json::parse` drops the field before the claim becomes
a `WireClaim`, and both carriages share that one codec (I4).

So the two edges pin the same value and take different action on a disagreement. **That difference
is deliberate**, it is the only one of its kind in this document, and it turns on what the
comparison could find and who could act on it — not on peers being trusted more in the abstract.

Since [issue #1128](https://github.com/toon-protocol/connector/issues/1128) a Solana peering has
exactly one program it can be judged under, `[settlement.solana] program_id` (§11 — a
`[[peer_channels]]` row that restates it is a named load-time refusal). That single value both
renders the `programId` this connector puts on an **outbound** peer claim and keys the
`SolanaChannel` its `ClaimBook` verifies an **inbound** one against. A disagreement therefore means
one of exactly two things:

1. **The peer signed under the program it declared, and that is not this node's.** The signature
   then fails, the claim is refused `SignatureInvalid`, that verdict rides back in the claim ack
   (§6), and under §1's **P3** the PREPARE it covered is not admitted as peer traffic at all — it
   gets the 402 greeting. The peering moves nothing, from its first packet, visibly at both ends. A
   report would annotate a failure that is already total and already loud, in a relation where each
   operator configured the other deliberately and knows them by name. It would not surface anything
   hidden.
2. **The peer declared one program and signed under another.** This is the silent case, and it is
   the one the client edge's report exists to catch. It is also the one a peering cannot produce
   from this connector: a payer would have to read two different sources for one value, and this
   connector reads one.

A **client** claim differs on both counts, and that is the whole of the asymmetry. Its payer is by
construction someone the operator has never heard of
([ADR 0052](../adr/0052-permissionless-payment-is-guaranteed-and-a-claim-is-what-authorises.md)),
running software the operator did not configure and cannot reach, so the connector's own log is the
only channel by which a mislabelled artifact can become known to anybody — which is exactly
[issue #975](https://github.com/toon-protocol/connector/issues/975)'s complaint, restated for a
field a signature does bind. It is also the adoption signal
[issue #1127](https://github.com/toon-protocol/connector/issues/1127)'s step 4 is gated on, and
adoption is only an open question for payers an operator did not configure. **Issue #1127's
readiness condition on that warning is therefore a client-edge condition; the peer edge's silence is
not a hole in it.** A peering's conformance is settled by the peering agreement, and failing that by
case 1 above.

What this rule **rejects** is retaining the field so the peer edge can report a disagreement the way
the client edge does. The cost was the deciding half: it would put an unauthenticated, authority-free
value on `WireClaim`, the in-process type whose whole discipline is that what a claim is checked
against comes from this connector's own per-channel record and never from the claim — bought for a
log line that, per the two cases above, either restates an outage or reports a bug the far end of a
peering cannot have.

None of the above is a licence to promote the field to a refusal on the client edge either. That is
`client-edge-spec.md` §1.3 step 4's rule, it is dated, and it is gated on payers deployed against
the pre-#1133 contract, not on this document.

_Held to by:_ `crates/connector-peer-btp/tests/a_peer_claims_declared_program_is_not_consulted.rs`,
which states the policy by name (a peer claim declaring a foreign program is accepted on its
signature, silently) and holds both halves of the argument above — that a claim signed under a
program this node does not settle with is already refused, and that a claim this connector renders
declares the program its own signature is bound to. `a_solana_claim_flushed_from_a_loaded_config_declares_the_settlement_tables_program`,
on **both** carriages, closes the hop from `[settlement.solana] program_id` to the wire.

### 4.2 Recovery id

Unchanged from `peer-semantics-pre-868.md` §3.5: an `evm` signature is 65 bytes `r ‖ s ‖ v`, with `v` as
libsecp256k1 emits it (`{0, 1}`), never the wallet `{27, 28}` convention. The one place the
conversion happens is immediately before on-chain submission. Both carriages carry the byte
unchanged, and the vectors pin it (§10).

---

## 5. Carriage-layer fields

### 5.1 `minimumDelivery`

**Retired 2026-08-24 by [ADR 0057](../adr/0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md)
(issue #1143).** No packet declares a minimum delivery. The `toon-minimum-delivery` protocolData
entry and the `Toon-Minimum-Delivery` header are gone from both carriages — deleted together,
because a field on one carriage and not the other is the drift §9 exists to prevent — along with
absent-means-zero, malformed-is-`F01`, the propagate-unchanged rule, the client-role ignore rule and
the `R01` reject the inequality produced. **`R01` itself stays in the reject vocabulary**, narrowed to
RFC 0027's own meaning — _"the amount received by a connector in the path was too little to
forward"_ — which no floor was ever needed to state
([ADR 0051](../adr/0051-a-reject-code-binds-where-a-sender-must-act-differently.md) as corrected).

What bounds erosion instead is the claim covering each crossing: `cover_forward` mints for the
packet's own forwarded value, so every hop holds a claim for at least what it passes on and its fee
is the difference — which chains, without any hop being handed a figure and trusted to check it.
`connector_domain::fee::amount_after_fee(amount, fee)` takes no floor; a packet that does not cover
this hop's own fee is refused **`R01`** naming both numbers and the figure to clear.

### 5.2 `accumulatedCost`

Already implemented on the client edge on both carriages and **reused verbatim** — the
`toon-accumulated-cost` protocolData entry and the `Toon-Accumulated-Cost` response header, both
decimal uint64 text, both already constant-named in `connector-client-edge`. The peer carriage adds
no new encoding.

The semantics are entirely `peer-semantics-pre-868.md` §5.2's and are not restated here. Two carriage-level
requirements:

- The field rides **only** a REJECT's response. A connector MUST NOT emit it beside a FULFILL, and
  MUST ignore it if one arrives there.
- **Absent means zero on receipt**, and a relaying hop MUST still add its own fee to that zero
  before passing the REJECT upstream. A hop MUST always emit the field on a REJECT it sends, even
  when the value is `0`, so that "absent" never has to carry meaning in the direction that matters.

### 5.3 Ceiling

**Retired 2026-08-10 by [ADR 0033](../adr/0033-the-exposure-machinery-is-retired-not-restated.md)
(issue #882).** `peer-semantics-pre-868.md` §5.3 no longer describes live behaviour: exposure is not
tracked and `ceiling` is not live configuration. The accept-only HTTP peering's ceiling
configuration obligation (§6.4, §11) is retired with it — an accept-only peering now loads with no
ceiling-shaped config at all.

> **Corrected 2026-08-20 (issue #1073).** An earlier version of this section added "and no PREPARE is
> ever rejected `T04`". That was true between issue #424 and the cap landing, and is **false now**: a
> packet exceeding a peering's **cap** is refused `T04`, never carried and never split, and the
> reject's message states the cap
> ([ADR 0049](../adr/0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md)).
> The bound an accept-only peering carries is that cap, plus the covering-claim requirement
> ([ADR 0042](../adr/0042-a-packet-carries-its-claim.md)) — **live at a priced termination, and not
> yet built for a forwarded arrival**. Cited to 0042 rather than to ADR 0031, which 0042 supersedes
> in full.

---

## 6. Claim acknowledgement

### 6.1 Where it rides

A `claim-ack` is a field on the response the carriage **already requires** for the claim-bearing
frame — never a frame of its own:

- **BTP**: a `claim-ack` protocolData entry on the RESPONSE under the claim-bearing MESSAGE's or
  TRANSFER's `requestId`.
- **HTTP**: a `Toon-Claim-Ack` response header on the response to the claim-bearing request.

The body in both cases is the same JSON, raw UTF-8 on BTP and `base64(JSON)` in the HTTP header:

```json
{ "result": "accepted" }
{ "result": "rejected", "reason": "signature_invalid" }
```

`reason` is exactly one of `peer-semantics-pre-868.md` §3.4's four, unchanged and not extensible without a
spec change: `signature_invalid`, `nonce_not_advancing`, `amount_not_advancing`, `unknown_channel`.
These are the wire spellings of `connector_runtime::ClaimRejectReason`'s four variants; a fifth
variant added to that enum without a corresponding change here and to the vectors is a wire break.

### 6.2 Independence of the two verdicts

**Preserved exactly, on both carriages.** `peer-semantics-pre-868.md` §3.4 is explicit that a `rejected`
claim does not reject the PREPARE the claim rode on. On the wire:

- BTP: one RESPONSE carries **two independent answers** — `ilpPacket` answers the packet, the
  `claim-ack` entry answers the claim.
- HTTP: the response body answers the packet, the `Toon-Claim-Ack` header answers the claim, and
  the **status is `200` regardless of the claim verdict**.

Therefore, normatively:

- A rejected claim MUST NOT be expressed as a BTP ERROR frame. ERROR remains reserved for
  undecodable frames (`client-edge-spec.md` §1.9 step 6).
- A rejected claim MUST NOT be expressed as a non-`200` HTTP status. `4xx`/`5xx` remain reserved
  for a malformed request or a connector fault, i.e. cases where there is no ILP answer at all.
- A rejected claim MUST NOT change the packet's own outcome, its `accumulatedCost`, or its fee
  accounting.
- The **consequence** is policy above the carriage: the payee's watermark did not advance, so it
  holds no claim covering what that peer's packets asked of it, and it SHOULD stop forwarding to
  that peer until a valid claim restores the watermark (`peer-semantics-pre-868.md` §3.4). The exposure
  accounting that used to quantify this is retired
  ([ADR 0033](../adr/0033-the-exposure-machinery-is-retired-not-restated.md), issue #882, with
  `peer-semantics-pre-868.md` §5.3); the SHOULD above is unchanged by that and is now the whole of the
  consequence.

A `claim-ack` MUST NOT appear on a response answering a frame that carried no claim. If one
arrives there it MUST be ignored.

### 6.3 Absence, timeout, and retransmission

The one honest loss of moving CLAIM_ACK from a frame type to a field: as an entry or a header it is
**omissible**, where a distinct frame type made "the peer sent no ack" inexpressible. The
compensating rules are the sharpest new requirements in this document.

**Absence.** A response answering a claim-bearing request that carries **no** `claim-ack` /
`Toon-Claim-Ack` means **NOT ACKNOWLEDGED**. Never accepted, never rejected, never inferred from
the packet's verdict. The claim stays pending until the retransmission deadline below — there is
no flush timer to keep running, and no exposure accounting to disturb, since ADR 0033 (issue #882)
retired both. A malformed ack — undecodable JSON, an unknown `result`, an
unknown `reason`, a `rejected` with no `reason` — is likewise **not acknowledged**, and MUST NOT be
read as either verdict.

**Timeout.** The deleted wire had no timeout on an ack that never arrived, so a pending claim could
hang forever. Both carriages now bound it structurally, because RFC-0023 requires a responder to
answer every request and HTTP always answers. The deadline:

| What was sent                              | Ack deadline                                                                                             |
| ------------------------------------------ | -------------------------------------------------------------------------------------------------------- |
| a claim riding a PREPARE (either carriage) | the same deadline as the packet's own answer: the PREPARE's `expiresAt`, capped by `peerAnswerTimeoutMs` |
| a FLUSH (BTP TRANSFER)                     | `claimAckTimeoutMs`                                                                                      |
| a FLUSH (HTTP standalone claim POST)       | `claimAckTimeoutMs`, applied to the HTTP response                                                        |

`peerAnswerTimeoutMs` and `claimAckTimeoutMs` are per peering relation, both defaulting to
**30 000 ms** — the value `connector-btp`'s existing `OUTBOUND_ANSWER_TIMEOUT` already uses, adopted
rather than re-derived. `claimAckTimeoutMs` SHOULD be less than or equal to `flushIntervalMs`, so
that a timed-out flush is superseded by the next flush tick rather than overlapping it; a
configuration where it is greater MUST at least be a load-time warning.

On expiry: the claim is **not acknowledged** (as above). A connector MUST NOT tear down a peering
on a single ack timeout, MUST NOT retry by signing a _new_ claim at a higher nonce for the same
cumulative, and MUST continue to count the packet's value in its own owed projection.

**Retransmission and the idempotent re-ack.** A lost ack and a lost claim are indistinguishable at
the payer, so retransmission is required and must be safe:

- A payer whose claim was not acknowledged MUST retransmit the **latest pending claim** for that
  channel — byte-identical if nothing has changed, or the newer, higher-nonce, higher-cumulative
  claim if further fulfilments have occurred since (`peer-semantics-pre-868.md` §3.2 step 3, unchanged: a
  newer claim supersedes an older pending one, and acknowledging the newer one clears both).
- A payee that receives a claim whose `(channel, nonce, cumulative, signature)` is **byte-identical
  to the claim already at its current watermark** MUST answer `{"result":"accepted"}`, MUST NOT
  answer `nonce_not_advancing`, and MUST NOT advance or record anything (there is nothing to
  advance — the cumulative amount covered is identical).
- A claim at the **same nonce** but differing in any other field is a _different_ claim and MUST be
  refused `nonce_not_advancing`, exactly as §3.2's strictly-advancing rule requires.

This is a strict narrowing of the strictly-advancing rule that costs nothing and is the only thing
standing between a lost ack and a permanently wedged peering. It is derived from ADR 0027's
"missing ack means not acknowledged" rather than stated by it; see §12.

### 6.4 The HTTP asymmetry, stated exactly

ADR 0027 names this as the price of the HTTP carriage. Stated mechanically:

**On HTTP, only the dialing side can originate. Packets therefore flow only in the dialing
direction; debt flows in the direction packets flow (`peer-semantics-pre-868.md` §3.2 — the sender owes);
therefore on a one-way-dialed HTTP peering the dialing side is structurally the payer and the
accept-only side is structurally the payee.**

Three consequences, in the order an operator meets them — the third retired by
[ADR 0033](../adr/0033-the-exposure-machinery-is-retired-not-restated.md) (issue #882):

1. **The peering is unidirectional for packets.** The accept-only side can never forward a packet
   to that peer. A route naming it as next hop is undeliverable, MUST be a load-time error where
   detectable (§11, `PeerRouteUndeliverable`) and MUST reject `T01` at runtime otherwise. **This is
   the consequence that actually bites at configuration time**, and it is more likely to surprise
   an operator than the flush question below.
2. **The residual flush case.** Where an accept-only side nonetheless holds a pending claim for
   that peer — because it could dial earlier and can no longer, or its configured endpoint is
   unreachable — it **cannot send the FLUSH at all**. The claim stays pending until it can dial
   again. `flushIntervalMs` no longer exists as configuration (ADR 0033), and the ceiling that used
   to bound its counterparty during that window is retired with it — every peer PREPARE this
   connector admits still requires its own covering claim ([ADR 0042](../adr/0042-a-packet-carries-its-claim.md),
   which supersedes ADR 0031 in full) regardless of this case — live at a priced termination, and not
   yet built for a forwarded arrival.
3. ~~**The ceiling is the accept-only payee's only real bound, and MUST be explicit.**~~ **Retired**
   (ADR 0033, issue #882). An accept-only peering now loads with no ceiling-shaped config at all;
   `AcceptOnlyPeerWithoutCeiling` no longer exists. Kept here, struck through, only so a reader
   following §11's history is not left guessing what the removed error covered.

**`Toon-Flush-Requested` — a hint, and only a hint.** A payee that cannot originate MAY set this
response header on any response it sends to that peer:

```
Toon-Flush-Requested: 0x3f2a…    # the channel id, canonical form per §4.1
```

- It MAY appear more than once, one channel id per occurrence; a comma-separated list form MUST NOT
  be used. A payee SHOULD NOT name the same channel more than once in one response.
- A payer receiving it, and holding a pending claim for the named channel, SHOULD send that claim
  on its next request to that peer, or immediately as a standalone claim POST (§3).
- A payer with **no** pending claim for the named channel, or that does not recognise the channel,
  MUST ignore the header. It MUST NOT be answered, acknowledged, or error on.
- **It creates no obligation.** A payee MUST NOT refuse traffic, reject a packet, or change any
  accounting because a hint went unanswered — nothing does (ADR 0033 retired the ceiling that
  used to). A payer that ignores every hint is not in violation of this specification.
- A payee MUST NOT set it on a response to a **client** interaction, and a connector MUST ignore it
  on a client-role response.

BTP has no equivalent and needs none: on BTP the payee can originate a request of its own, and a
peering whose payee needs to prompt should be on BTP.

---

## 7. Ordering and concurrency

### 7.1 BTP

Identical to the client edge's, and for the same reason (`client-edge-spec.md` §1.9, "Ordering",
issue #688). **Claims on one peer session are judged strictly sequentially, in arrival order** — a
frame's claim is fully admitted or refused before the next frame's claim is looked at. Claims sent
in order on one socket therefore cannot race each other into `nonce_not_advancing`.

What is **not** serialized is a judged frame's remaining work — recording the claim, routing the
packet, sending the RESPONSE — which proceeds for up to a bounded number of frames concurrently
(the connector's `btp_session_window`, default 16; when the window is full the session stops
reading, so the bound is also the backpressure). A connector MUST reuse that mechanism rather than
re-deriving a peer-specific one.

Consequently RESPONSE frames may arrive in a different order than the MESSAGEs that provoked them.
`requestId` is the correlation. **A peer MUST NOT assume responses arrive in request order**, and
MUST NOT infer which claim an ack answers from position — the defect §12 records as fixed.

### 7.2 HTTP

The race `client-edge-spec.md` §1.9 exists to remove is present on an HTTP peering and absent on a
BTP one: parallel requests carrying nonces _n_ and _n+1_ reach the watermark lock in either order,
and the loser is refused `nonce_not_advancing` for nothing.

Normative mitigation, matching what the client edge already ships: **a connector dialing a peer
over HTTP MUST NOT have more than one claim-bearing request in flight to that peer per channel.**
Requests carrying no claim are unconstrained. A connector MAY instead accept the retry cost, but
only if it treats a `nonce_not_advancing` ack on a claim it knows to be fresh as a retryable
condition rather than an error — and it MUST NOT respond to it by minting a higher nonce for the
same cumulative (§6.3).

This is a documented property of the carriage an operator chose, not a defect. It is the second
reason (after §2.4) that BTP is the recommendation for a fleet link.

### 7.3 Correlation

- **BTP**: `requestId`, per `client-edge-spec.md` §1.9's grammar. A RESPONSE or ERROR whose
  `requestId` this connector originated resolves against that outbound request; one it never
  originated is dropped.
- **HTTP**: the request/response pairing itself.

Neither carriage has, or needs, the deleted wire's `correlationId` field on a claim ack.

---

## 8. Sealing and fulfilment on a peer hop

ADR 0018 (a payload is sealed to the terminating connector) and ADR 0019 (a terminating connector
derives the fulfilment) are unchanged by carriage. What this section fixes is the distinction
between a **peer hop** and a **termination**, because the two carriages make it easier to blur.

### 8.1 A peer hop is a forwarding hop

- A packet's `data` is a gift wrap sealed to the identity of the connector that **terminates** its
  route — not to the peer it is forwarded to. A forwarding connector holds no key that opens it.
- A forwarding connector MUST forward `data` **byte-for-byte unchanged** on whichever carriage the
  outbound hop uses. Crossing from BTP to HTTP or back MUST NOT re-encode, re-wrap, unwrap, pad or
  truncate it. Opacity is a property of carriage (ADR 0016) and neither carriage adds a layer.
- A forwarding connector MUST NOT derive a fulfilment. ADR 0019's derivation is a **termination-only**
  capability; issue #417's rule — a connector never produces a fulfilment itself — stands unchanged
  for every forwarding hop, on both carriages.
- **A forwarding connector relays a downstream FULFILL unchecked** (issue #1269,
  [ADR 0069](../adr/0069-the-execution-condition-leaves-the-wire.md)). The PREPARE carries no
  execution condition and there is no field left to verify a candidate fulfilment against: a hop
  is paid on arrival (ADR 0042) regardless of what the FULFILL it relays turns out to contain, so
  the check `peer-semantics-pre-868.md` §3.1 once required here protected nothing this hop owns.
  The sender's own end-to-end check — comparing the fulfilment that comes back against
  `derive_fulfillment` of its own sealed secret — is what a forged delivery actually meets.
  `peer-semantics-pre-868.md` §3.1's F01-on-missing-condition rule is retired the same way: there is
  no condition for a PREPARE to omit.

### 8.2 A termination reached over a peering

When the peer link's far end **is** the termination, ADR 0018 and ADR 0019 apply exactly as they do
at the client edge: the terminating connector opens the wrap with its own identity key, derives the
fulfilment from the sealed shared secret, seals its answer back under that same secret, and confines
the envelope's `target` beneath the route's handler path (ADR 0025). That the packet arrived from a
peer rather than from a client changes **nothing** about any of it — including the fact that the app
supplies no preimage and there is no `TOON-Fulfillment` response header.

The one thing the peer arrival _does_ change is accounting. Before any of the above happens, the
terminating connector checks that the PREPARE's own `amount` covers that route's `price`
(`peer-semantics-pre-868.md` §5.4, issue #752); an arrival that does not is refused `F03` with
`accumulatedCost = 0` and the wrap is never opened. An arrival that clears that check is delivered
exactly as described above, and if the termination itself then rejects it, the REJECT carries that
route's configured price as `accumulatedCost` (`peer-semantics-pre-868.md` §5.2); the peer hop that
forwarded to it adds its own fee on the way back.

### 8.3 The layering invariant

> **Carriage-layer fields are never sealed, and sealed payloads are never carriage-layer fields.**

The claim, the claim ack and `accumulatedCost` ride the
carriage — protocolData entries or headers — precisely so a hop can read and judge them without
opening a payload it has no key for. Nothing in this document ever asks a connector to look inside
`data`, and nothing in ADR 0018's wrap is ever promoted to a protocolData entry or a header.

Corollary on propagation: **a carriage-layer field is re-derived by each hop, not copied**, with no
exceptions. `accumulatedCost` is recomputed (`+ thisHopFee`); the claim is this hop's own claim on
its own channel; the claim ack answers this hop's own inbound
claim. The one field that used to propagate unchanged was `minimumDelivery`, and it is retired
(§5.1) — the rule is now universal rather than universal-with-an-exception.

> **Amended 2026-08-26 by [ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md)
> (issue #1157).** Both lists above also named the **peer credential** — as a carriage-layer field in
> the first, and as one whose value is "this hop's own" in the second. It is struck from both,
> because it is deleted outright rather than moved: a peering carries no credential on either wire
> (§1.4), so there is nothing left for either sentence to classify. Neither statement weakens. The
> claim is the carriage-layer authentication material now, it was always carried in the same two
> shapes the credential was (§3), and per-hop re-derivation is not merely still true of it but
> **required** by §1.2 — a claim is signed against one channel's counterparty key, so a claim
> copied onward proves a peering it was not signed for and verifies nowhere.

---

## 9. The invariants that keep two carriages from drifting

ADR 0026's factoring of `claim_rejection_reject` and the x402 terms builder exists because the two
_client_ carriages drifted and caused a devnet incident. The peer side inherits that discipline as a
requirement. Each invariant below names the structural enforcement, not a review commitment.

**I1 — One semantic value, two encodings.** For every row of §3's table, the value a connector
decodes from the BTP encoding and the value it decodes from the HTTP encoding are the same value.
_Enforced by:_ paired vectors generated from **one** fixture set (§10), plus a test that parses both
members of each pair and asserts equality of the decoded value — not of the bytes.

**I2 — One name table.** The BTP protocolData entry name and the HTTP header name for a given
concept are declared **once**, as a pair, in one shared module, and both carriages read them from
it. _Enforced by:_ a single table (the entry names already live in `connector-btp` after issue
#713; the pairing table belongs beside them or in `connector-domain`, which both carriages already
depend on). Adding a header without its protocolData twin must be impossible to express, not merely
noticed in review — a second `const CLAIM_PROTOCOL` declared in a peer module is exactly the fork
issue #713 was opened to prevent.

**I3 — One refusal taxonomy.** `ClaimRejectReason` → ack JSON is **one** function, called by both
carriages, exactly as `claim_rejection_reject` is on the client edge. A fifth reason cannot appear
on one carriage and not the other, and cannot appear on the wire without a vector.

**I4 — One claim codec, one validator, one verifier.** The claim JSON of §4 is the client edge's
claim JSON, parsed by the same structural validator and checked against the same
`connector_signer::claim_signature` digest (ADR 0024). There is no peer claim type on the wire.

**I5 — One pipeline below the port.** Route lookup, `ClaimBook`, journal and fee accounting are
reached only through `PeerTransport`, and none of them can observe which carriage delivered a
packet. _Enforced by:_ the port's existing contract suite being generic over how a peer is wired up
(kept deliberately so in issue #679), extended with one arm per carriage. A carriage that needs to
change anything above the port is a signal the seam is wrong.

**I6 — One relation, one set of watermarks.** §2.5: watermarks and the ledger are per peering
relation, never per carriage or per connection.

**I7 — One role decision.** §1: the same **P2/P3** rule — a channel binding _and_ a verified claim
on one of that peer's channels — the same downgrade behaviour, and the same named regression test on
both carriages, with the **claim** in the two encodings §3's table pins so a carriage cannot admit as
a peer a frame the other would downgrade. _Enforced by:_ one decision function taking a channel id
and a verification verdict and nothing else — a value that cannot carry the carriage, the port, the
source address or the session's history, so two frames differing only in those are literally the same
input (`crates/connector-peer-auth/src/decision.rs`, `decide_role` and its
`a_presented_claim_carries_nothing_a_carriage_could_weight`). That is §1.3 made structural rather
than reviewed.

> **Corrected 2026-08-20 (issue #1073).** This invariant said "the same P1/P2 rule". **P1 — the
> `{peerId, secret}` bearer credential — has not decided role since issue #868**, as §1's own banner
> records; role is P2 **and** P3. The credential still exists as a carriage artifact and still has to
> be framed identically on both wires, which is what the rest of this invariant is about.

> **Amended 2026-08-26 by [ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md)
> (issue #1157).** The clause corrected above ended "with the credential in one JSON shape (§1.4) so
> a carriage cannot accept a credential the other would refuse", and the 2026-08-20 note closed by
> saying the credential "still exists as a carriage artifact and still has to be framed identically
> on both wires". Neither is true now: the `{peerId, secret}` credential is deleted from both wires
> and from the config (§1.4, §11), and nothing replaced it.
>
> **The invariant still binds; it is not vacuous.** What it protects is that the two carriages cannot
> disagree about which frames they admit as peers, and that concern kept its subject when the
> credential lost its — the material role is decided from is now the claim, carried in exactly the
> two shapes the credential was (raw UTF-8 JSON in the BTP `payment-channel-claim` entry,
> `base64(JSON)` in the `ILP-Payment-Channel-Claim` header, §3). So the clause is **restated over the
> claim**, not deleted and not replaced by an invention.
>
> Its two halves now rest in different places, which is worth stating because the encoding half is no
> longer I7's to hold alone. That the two encodings decode to one value, are refused by one taxonomy
> and are verified by one verifier is I1, I3 and I4, each with its own structural enforcement. What
> remains **I7's own** is the step after: that a claim which verifies produces the same role, the same
> downgrade and the same silence-or-event on either carriage. Two carriages can share a codec and
> still differ there — one binding role per session and the other per frame would (§1.5) — so this
> invariant has work left to do that no other invariant does.

**Any peer behaviour that exists on one carriage and not the other, other than the two this
document names as carriage properties (§6.4's origination asymmetry and §7.2's claim race), is a
defect.** That is ADR 0027's revisit condition, restated as an acceptance criterion.

---

## 10. Vectors (ADR 0021)

**A new frame shape without vectors is how the dialect drifted the first time.** The deleted peer
wire had 102 lines of codec, prose describing it, and no vectors; the divergence issue #575 found —
`ClaimBook` signing a connector-internal SHA-256 tuple where the spec said EIP-712 — was invisible
for exactly as long as nothing pinned the bytes. This section exists so that cannot recur across two
carriages, where there is twice as much surface and a second copy to fall out of step with.

Per ADR 0021 the vectors are normative and this prose is not. Issue #676 MUST produce every vector
below, in `vectors/wire-vectors.json` under a new `peer_carriage` section, generated by
`crates/connector-vectors` from **fixed literal fixtures** (hardcoded keys, channel ids, nonces,
amounts and payloads — never values sampled per run), self-verified at generation time against the
same functions that judge them at runtime, and gated by `cargo test -p connector-vectors` exactly as
the existing sections are. `vectors/README.md` MUST document the new section's schema for a reader
in another repository importing no Rust from this one.

### 10.1 The pairing rule

**Vectors are generated in pairs from one fixture set.** For every concept, the BTP encoding and the
HTTP encoding are produced from the _same_ fixture struct in the same generator run, and a test
decodes both and asserts the decoded values are equal (I1). A change to one carriage that is not
made to the other fails CI rather than being caught in review. This pairing is the mechanical form
of ADR 0026's anti-drift discipline and is the reason the vector set is the enforcement point rather
than the documentation.

### 10.2 What must be pinned

Every item is required. An item marked _(pair)_ is one BTP vector and one HTTP vector over the same
fixture.

**Role**

There is nothing to pin. Role is decided by the claim below, so the vectors that pin the claim pin
role with it. The `peer_auth` _(pair)_ that stood here — the credential JSON, raw UTF-8 as the BTP
`auth` entry and `base64` as the `Toon-Peer-Auth` header value — is deleted from the corpus by
[ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md) (issue #1157),
and `schema_version` moves to **4** to announce it.

**Claim**

2. `peer_claim_evm` _(pair)_ — the claim JSON of §4 for `blockchain: "evm"`, raw and base64,
   including the 65-byte signature with `v ∈ {0,1}` (§4.2).
3. `peer_claim_digest` — the EIP-712 `BalanceProof` digest for that same claim's fields, pinned
   **unchanged** against the existing claim section of `wire-vectors.json`, demonstrating ADR 0024
   is untouched by carriage.
4. `peer_claim_solana` _(pair)_ — the `solana` claim JSON, marked aspirational exactly as
   `peer-semantics-pre-868.md` §3.5 marks that row, so the shape is pinned before an implementation exists.

**Claim-bearing PREPARE**

5. `peer_prepare` _(pair)_ — BTP: a complete MESSAGE frame's bytes (type, `requestId`, protocolData
   list containing `payment-channel-claim`, the OER PREPARE in `ilpPacket`). HTTP: method, path, the
   full header set and the OER body.
6. `peer_prepare_no_claim` _(pair)_ — the same PREPARE with no claim entry/header, so "claimless is
   legal" is pinned rather than assumed.

**Answers and claim-ack**

7. `peer_fulfill_ack_accepted` _(pair)_ — a FULFILL answer carrying `{"result":"accepted"}`.
8. `peer_fulfill_ack_rejected` _(pair)_ — **a rejected claim riding a fulfilled PREPARE**: a FULFILL
   body together with a `rejected` ack. This is §6.2's independence property, and it is the single
   most important vector in this set, because coupling the two verdicts is the failure mode that
   would silently destroy ADR 0024's semantics.
9. `peer_ack_rejected_<reason>` _(pair × 4)_ — one per §6.1 reason: `signature_invalid`,
   `nonce_not_advancing`, `amount_not_advancing`, `unknown_channel`.
10. `peer_reject_with_cost` _(pair)_ — a REJECT answer carrying `toon-accumulated-cost` /
    `Toon-Accumulated-Cost` **and** a `claim-ack`, both on the one response.
11. `peer_ack_absent` _(pair)_ — **a response answering a claim-bearing request with no ack at
    all**, pinned as the "not acknowledged" case (§6.3). Vectoring an _absence_ is unusual and
    deliberate: the encoding cannot express it, so only a pinned example makes the rule testable.
12. `peer_ack_malformed` _(pair)_ — an ack whose JSON is undecodable or whose `result` is unknown,
    pinned as also meaning not-acknowledged, not as an error.

**Flush**

13. `peer_flush` _(pair)_ — BTP: a complete TRANSFER frame with `amount` equal to the claim's new
    cumulative, the `payment-channel-claim` entry, and **no** `ilpPacket`. HTTP: a POST with an
    empty body and the claim header. The equality between the TRANSFER `amount` and the claim's
    `transferredAmount` MUST be asserted by the generator, not just by the reader.
14. `peer_flush_ack` _(pair)_ — the answer to a flush: BTP an empty RESPONSE carrying the `claim-ack`
    entry; HTTP a `200` with an empty body and the `Toon-Claim-Ack` header.
15. `peer_claim_retransmit` _(pair)_ — a byte-identical retransmission of an already-accepted claim
    and its idempotent `accepted` ack (§6.3), paired with
16. `peer_claim_same_nonce_different_bytes` _(pair)_ — the same nonce with any other field changed,
    and its `nonce_not_advancing` ack. Together these pin the boundary that keeps a lost ack from
    wedging a peering.
17. `peer_flush_requested` — HTTP only: a response carrying `Toon-Flush-Requested` with a canonical
    channel id (§6.4). No BTP counterpart exists, and the vector set MUST record that absence
    explicitly rather than leaving the pair incomplete.

**Minimum delivery**

18–19. ~~`peer_minimum_delivery_absent`, `peer_minimum_delivery_malformed`~~ **Retired 2026-08-24 by
[ADR 0057](../adr/0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md) (issue #1143)**, and
deleted from `vectors/wire-vectors.json`. The item numbers are not reused. This was a cross-repo
wire change ([ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md)): `toon-client`, `rig`
and `swap` replay this set.

**Sealing**

20. `peer_forwarded_data_unchanged` _(pair)_ — one sealed `data` payload from the existing giftwrap
    section, carried on both carriages, with the generator asserting the bytes are identical to the
    source. This pins §8.1's "byte-for-byte unchanged, including across a carriage change".

### 10.3 What is deliberately _not_ re-vectored

The OER packet encodings, the envelope, the gift wrap, the derived fulfilment and the EIP-712 claim
digest are already pinned in `wire-vectors.json` and were never peer-specific
(`docs/protocol/wire-vectors.md`). The peer carriage MUST reference them, not copy them; a second
copy of the claim digest would be a second thing to keep in step.

---

## 11. What this specification requires of the config (issue #677)

Naming below is normative for the **wire-visible** parts (carriage names `btp`/`http`, endpoint
schemes) and for the **error identities**; the exact TOML table and field spelling is #677's to
settle. `deny_unknown_fields` stays.

Required surface:

- `[peers].expose` — a set drawn from `{"btp", "http"}`; `[]` is legal and means dial-only (§2.1).
- Per peer: `id`; optional `endpoint` (a URL whose scheme is `wss://` or `https://`, with host and
  port, SNI-capable — omitted means accept-only); the per-peering-relation
  `fee` (`peer-semantics-pre-868.md` §4); and this document's `claim_ack_timeout_ms` and
  `peer_answer_timeout_ms` (§6.3, default 30 000 each). `ceiling`/`flush_interval_ms` were also
  required here before [ADR 0033](../adr/0033-the-exposure-machinery-is-retired-not-restated.md)
  (issue #882); both are retired and now parsed only as removed-field traps
  (`PeerCeilingRemoved`/`PeerFlushIntervalRemoved`, below).
- **`credential` is not a field of a peer**, and nothing takes its place.
  [ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md) (issue
  #1157) deleted the `{peerId, secret}` shared secret outright — not renamed, not demoted to a
  label, not kept as an optional discriminator. It was listed in the bullet above until then, and
  is called out separately rather than struck silently because this is the section a second
  implementation reads to learn what to parse. There is nothing to present (§1.4), nothing decides
  on one (§1.2), and a config that still writes one is a named load-time refusal
  (`PeerCredentialRemoved`, below) rather than a file that loads with the key dropped — the posture
  [ADR 0009](../adr/0009-one-typed-config-file-no-environment-layer.md) requires of every removed
  key, and the reason it is a **hard** error is the one the removed-field row below already gives:
  the devnet boxes run bind-mounted configs that lead the repo.
- Per peer: `max_packet_amount` — [ADR 0042](../adr/0042-a-packet-carries-its-claim.md)'s **cap**,
  the largest amount this connector will forward to that peering in **one packet**, in the
  settlement asset's base units. A packet needing more is refused with `T04`, never carried and
  never split. Those base units are the **outgoing** channel's — the peering the row is written
  on, never the peering the packet arrived over. The distinction is free while a whole path holds
  one token and is the whole of the reading once it does not: a hop that crosses a denomination
  applies its declared rate first and compares the **converted** figure against this number
  ([ADR 0071](../adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)), so a cap
  beside a peering holding an 18-decimals token is a count of 10⁻¹⁸ of that token and nothing
  else. There is also a ceiling below this one that no operator configures: an amount is a `u64`,
  so one packet on an 18-decimals leg cannot exceed `u64::MAX / 10¹⁸ ≈ 18.4` tokens whatever this
  row says, and a conversion landing past it is refused rather than wrapped. `max_packet_amount`
  is where an operator says something smaller and deliberate. Optional and defaulted (`connector_config::DEFAULT_MAX_PACKET_AMOUNT`, 1 000 000 =
  1 USDC), so a peering that writes nothing is still bounded; there is deliberately no spelling
  that disables it, and `0` is a named load error rather than "off". This bounds one packet, not
  an accumulation — it is not `ceiling` returning (ADR 0033, retired above).
- Per peer, **temporary** (ADR 0042 item 3): `forwarded_claim_enforcement`, one of `"observe"`
  (**default**) or `"enforce"`. Governs §3.1's forwarded-arrival rule only: omitted, an uncovered
  forwarded arrival is admitted and logged rather than refused, because no box on this fleet covers
  its forwards yet. An operator writes `"enforce"` per peering once that peering's counterparty is
  covering. Its sibling `claim_enforcement` — issue #883's canary knob for §3.1's **terminated**
  rule, one of `"enforce"` (default) or `"observe"` — is **removed** (ADR 0042 item 4, issue
  #1077); it is a removed-field trap below, and the terminated rule now enforces unconditionally.
  Two fields rather than one is what made that possible: the two migrations defaulted in opposite
  directions and ended on different days, so deleting `claim_enforcement`'s `"observe"` would
  otherwise have deleted this field's default with it.
- The accepting mirror, **inverted** by
  [ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md) (issue
  #1157): configured **channels** map to peer ids, not peer ids to channels. The accepting side
  reads the `channel_id` the frame's claim names, resolves it to the one `[[peer_channels]]` row
  that may configure it — one is exact, because a second row on the same channel is
  `PeerChannelDuplicate` at load — and takes the peer id off **that row**, never off the
  interaction (§1.2, §1.3). Where the credential mapped a presented name to a peering and thence to
  its channels, the claim names the channel and config names the peering, so no attacker-chosen
  string reaches the lookup at all. This is why nothing had to replace the credential: its one
  remaining structural job, saying which peering to evaluate against, is done by the claim at no
  cost.
- `[[peer_channels]]` — EVM shape: `peer_id`, `channel_id`, `counterparty_key`, `chain_id`,
  `token_network`. Solana shape (issue #759): `peer_id`, `channel_account`, `counterparty_key` —
  no `chain_id`/`token_network`, since a Solana channel has neither an EVM-style numeric chain id
  nor a per-token verifying contract, and (issue #1128) **no `program_id` either**. A Solana
  claim's `programId` is still a required field of §4's claim shape, but it is not a fact this
  row declares: it is read from `[settlement.solana] program_id`, and the row MUST NOT restate it.
  Since ADR 0053 binds the settlement program into a Solana claim's signed message, a row naming
  its own program could disagree with the table, and a node in that state accepts peer claims
  signed under one program while redeeming under another — carriage rendered for money it can
  never collect, silently and in the paying direction. So there is exactly one program a Solana
  peer channel can be judged under, and it is the one this node settles with; a row still writing
  `program_id` is a named load-time refusal, and a Solana row on a node with no
  `[settlement.solana]` is another. This is the same "no second declaration" rule
  `[[client_channels]]` took in #981/#1082.
  The EVM shape keeps its own `chain_id`/`token_network`, and does so deliberately: ADR 0024
  makes the EIP-712 domain a configured input per channel, and `[settlement.evm]` names a
  `TokenNetworkRegistry` rather than a `TokenNetwork`, so unlike the Solana program id there is
  no second copy in the file to read it from. It MUST NOT go unchecked, though (issue #1136): a
  node holds the declared pair against the `TokenNetwork` its own
  `TokenNetworkRegistry.getTokenNetwork(token_address)` resolves at connect, and **refuses to
  start** when they disagree — the same posture `[settlement.evm] decimals` has taken against the
  token's own `decimals()` since #564. The failure it closes is the EVM twin of the Solana one
  above, and just as silent: a row left stale after a redeploy verifies peer claims under one
  `TokenNetwork` while redeeming through another. A node with no `[settlement.evm]` table has no
  resolved contract to be held against, so nothing is compared there — that file does not load at
  all, under the rule immediately below.
  The EVM shape is the surface whose absence makes ADR
  0024 inert (#620 gap 3); it MUST actually wire `ClaimBook`'s signer, verification key and
  EIP-712 domain, with **no code-only setters left on the config path**. The Solana shape's
  program id reaches claim rendering the same way, and (issue #998) `channel_account`/
  `counterparty_key` reach `ClaimBook`'s Solana verification key and signer through the same
  no-code-only-setters rule -- `Connector::with_solana_channel`/`with_solana_signer`, wired from
  `[[peer_channels]]` and `[settlement.solana]` respectively, so a Solana row can both
  `accept_inbound` and sign an outbound claim on that channel.

### 11.1 A channel row requires the settlement table of its own chain (issue #1138)

**One rule, and it governs every channel table.** A `[[peer_channels]]`, `[[client_channels]]` or
`[[pay_channels]]` row on a chain for which this node declares no `[settlement.<chain>]` table is a
**named load-time refusal**. Never skipped, never accepted-and-inert. It is per chain and no wider:
a row needs the table for its own chain and no other, so a Solana-only node still loads its Solana
rows and an EVM-only node its EVM ones.

The reason is one reason, which is why the rule is one rule. `[settlement.<chain>]` is not merely
how a node _submits_ a redemption — it is where the node's **on-chain identity** on that chain comes
from. `[settlement.evm.key]` is this node's EVM address and `[settlement.solana.key]` its Solana
one; the connector holds a signer, not a wallet (ADR 0012), and "there is no second key to configure
and none is invented" (ADR 0030). A node with no table for a chain therefore has no address on it,
so it **cannot be a participant of any channel there** —
`TokenNetwork.claimFromChannel` reverts `InvalidParticipant` for a caller that is not one, and the
Solana program refuses a `claimer` account that is not one (`UnauthorizedSigner`). The row names a
channel this node is not in, and every claim it admits is carriage rendered for money it can never
collect: the same sentence #1128 refused for Solana peer channels, applied everywhere it is true.

**The client edge is not an exception, and this is the part worth stating explicitly.** A declared
`[[client_channels]]` row is deliberately usable by a node with no _chain connection_: it is
answered from memory, never resolved, never re-verified, and exempt from the deposit cap
(`DepositFloor::Unknown`, issue #646) — because how much unverified exposure to take on a channel is
a **policy**, and an operator hand-declaring a channel is making it. That latitude is over how much
may be spent on a channel this node **is** a participant of. It presupposes redeemability; it does
not confer it. Whether this node can redeem at all is a **fact** about the chain with exactly one
answer, the same category issue #1136 put the EIP-712 domain in and for the same reason — so the
declared-channel path's latitude does not reach it.

Two independent things already agreed before the refusal existed, which is corroboration rather
than the argument: a settlement-less node's x402 greeting carries no `settlement` or `settlements`
key at all, so no conforming client can discover the domain to sign under; and this connector's own
announce path already refuses to pay such a node by name — _"a node with no settlement backend
cannot be paid by channel claim"_.

**No difference between the tables survives.** Only the consequence each refusal states differs:

- `[[peer_channels]]` — the peering is bound on paper and unredeemable in fact. `PeerChannelUnbound`
  already requires every peering to carry a row, so this is the row that binds nothing, and the node
  also signs no covering claim outbound (ADR 0024's peer-claim key is that same settlement key).
- `[[client_channels]]` — a buyer's claim verifies, the write is served, the claim is worthless.
- `[[pay_channels]]` — there is no key to sign a covering claim with. Already refused before #1138
  for an EVM row; a Solana row (issue #1146) is refused the same way, and for the second reason as
  well: `[settlement.solana]` is also where the program id ADR 0053 signs into the claim comes from.

One consequence of stating it once is worth naming: because every peering must carry a channel row,
the only file that can now reach `PayChannelWithoutEvmSettlement` is one peering over a chain it
does settle on while paying over one it does not.

Named load-time errors this specification requires (spelling #677's, identity ours):

| Error                                           | Condition                                                                                                                                                                                                                                                                                                                   | Source             |
| ----------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------ |
| `PeerUndialable`                                | `expose` is empty **and** a configured peer has no `endpoint` — a peering that can never establish                                                                                                                                                                                                                          | §2.2               |
| `PeerEndpointScheme`                            | an `endpoint` whose scheme is neither `wss://` nor `https://` — nor, at a `.onion` or `.anyone` host, `ws://` or `http://` (ADR 0070)                                                                                                                                                                                       | §2.1               |
| `PeerCredentialRemoved`                         | a `[[peers]]` entry setting `credential` at all — the `{peerId, secret}` shared secret is deleted and nothing replaces it, so a file writing one is stopped **by name** rather than peered without it. Replaces `PeerCredentialMissing` ("no credential — it could never satisfy P1"), a requirement that no longer exists  | §1.2, ADR 0060     |
| `PeerChannelUnbound`                            | a `[[peers]]` entry with no `[[peer_channels]]` row — it could never satisfy P2                                                                                                                                                                                                                                             | §1.2               |
| `PeerChannelOrphaned`                           | a `[[peer_channels]]` row naming an unknown `peer_id`                                                                                                                                                                                                                                                                       | §1.2               |
| `PeerChannelDuplicate`                          | one `channel_id` in two `[[peer_channels]]` rows — "whichever row's counterparty key won" would depend on iteration order. Load-bearing since ADR 0060: P3 resolves a claim to a peering **through its channel**, so this is what makes the claim's `channel_id` a sufficient identifier                                    | §1.2, ADR 0060     |
| `ChannelInBothNamespaces`                       | a channel id present in both `[[peer_channels]]` and `[[client_channels]]`                                                                                                                                                                                                                                                  | §1.8               |
| `PeerChannelProgramIdRemoved`                   | a Solana `[[peer_channels]]` row still setting `program_id` — the key is removed; the program is read from `[settlement.solana]`, the only one this node can redeem under                                                                                                                                                   | #1128              |
| `PeerChannelWithoutSolanaSettlement`            | a Solana `[[peer_channels]]` row on a node with no `[settlement.solana]` table — nothing to read the program id from, and nothing to redeem a claim through                                                                                                                                                                 | #1128              |
| `PeerChannelWithoutEvmSettlement`               | an EVM `[[peer_channels]]` row on a node with no `[settlement.evm]` table — no address to be the channel's on-chain participant, so nothing to redeem an inbound claim at and no key to sign an outbound one with                                                                                                           | §11.1, #1138       |
| `ClientChannelWithoutEvmSettlement`             | an EVM `[[client_channels]]` row on a node with no `[settlement.evm]` table — the client-edge case of §11.1; the declared-channel exemption is a credit policy and does not reach redeemability                                                                                                                             | §11.1, #1138       |
| `ClientChannelWithoutSolanaSettlement`          | a Solana `[[client_channels]]` row on a node with no `[settlement.solana]` table — no program to judge the claim under and no address to collect it at. Was a warn-and-skip that left the row configured and every claim on it refused as unknown                                                                           | §11.1, #1138       |
| `ClientChannelSolanaSettlementProgramIdInvalid` | a Solana `[[client_channels]]` row whose `[settlement.solana] program_id` is not base58 of a 32-byte value — the twin of the peer row's, and it replaces a boot panic                                                                                                                                                       | §11.1, #1138       |
| `PeerChannelSolanaSettlementProgramIdInvalid`   | a Solana `[[peer_channels]]` row whose `[settlement.solana] program_id` is not base58 of a 32-byte value                                                                                                                                                                                                                    | #1128              |
| `PeerChannelInvalidSolanaAccount`               | a Solana `[[peer_channels]]` row's `channel_account`/`counterparty_key` is not base58 of a 32-byte value                                                                                                                                                                                                                    | #759               |
| `PayChannelUnbound`                             | a `[[routes]]` entry whose next hop is a peering with no `[[pay_channels]]` row — a connector covers every PREPARE it sends and the postpay fallback is deleted, so every packet on that route would be refused at packet time. Keyed on the **route**: a peering this node only accepts on needs no row                    | ADR 0042, #1145    |
| `PayChannelWithoutSolanaSettlement`             | a Solana `[[pay_channels]]` row on a node with no `[settlement.solana]` table — no ed25519 key to sign a covering claim with, and no program id for ADR 0053 to bind into it                                                                                                                                                | §11.1, #1146       |
| `PayChannelSolanaSettlementProgramIdInvalid`    | a Solana `[[pay_channels]]` row whose `[settlement.solana] program_id` is not base58 of a 32-byte value — the pay-book twin of the peer and client rows'                                                                                                                                                                    | #1146              |
| `PayChannelInvalidSolanaAccount`                | a Solana `[[pay_channels]]` row's `channel_account` is not base58 of a 32-byte value                                                                                                                                                                                                                                        | #1146              |
| `PayChannelProgramIdNotDeclared`                | a Solana `[[pay_channels]]` row setting `program_id` — this table never declared one; the program is `[settlement.solana]`'s, as it is for the other two books. Named rather than left to `#[serde(untagged)]`'s "matched no variant"                                                                                       | #1146              |
| `PayChannelSolanaWithoutPeerChannel`            | a Solana `[[pay_channels]]` row whose channel account is not also bound by a Solana `[[peer_channels]]` row for that peering — `programId` is a required claim-wire field and the carriage renders it from that row, so every claim this row minted would be unrenderable                                                   | #1146              |
| `PeerRouteUndeliverable`                        | a route naming as next hop a peer this connector can never originate to                                                                                                                                                                                                                                                     | §2.2, §6.4         |
| `DuplicatePeerId`                               | two `[[peers]]` entries with the same `id`                                                                                                                                                                                                                                                                                  | —                  |
| `InvalidForwardedClaimEnforcement`              | `forwarded_claim_enforcement` set to anything other than `"observe"` or `"enforce"` — here a typo meant as `"enforce"` falls through to the permissive default and carries forwards for free                                                                                                                                | ADR 0042           |
| `PeerMaxPacketAmountZero`                       | `max_packet_amount = 0` — a cap of zero refuses every packet the peering could carry, and there is no "disable the cap" spelling                                                                                                                                                                                            | ADR 0042           |
| `PeerClaimEnforcementRemoved`                   | `claim_enforcement` set at all — issue #883's canary knob is gone and the terminated rule enforces unconditionally, so `"observe"` names no mode and `"enforce"` names the only behaviour there is. The message also disambiguates the still-live `forwarded_claim_enforcement`, since the two spellings differ by one word | ADR 0042, #1077    |
| removed-field errors                            | `peer_wire_addr`, `addr` in its old `SocketAddr` shape, or `ceiling`/`flush_interval_ms` (ADR 0033, issue #882) — a **hard, named** error pointing at the bring-up doc, never a silent ignore, because the devnet boxes run bind-mounted configs that lead the repo                                                         | ADR 0027, ADR 0033 |

`AcceptOnlyPeerWithoutCeiling` and the `claim_ack_timeout_ms > flush_interval_ms` load-time warning
(§6.3) are retired along with `ceiling`/`flush_interval_ms` (ADR 0033, issue #882).

**No `transport` selector.** There is no field selecting between a peer semantics and a carriage: the
raw-TCP wire is deleted, and the carriage is selected by `expose` and by each endpoint's scheme.

**Discovery needs no schema change.** `kind:10032` already advertises a `wss://` `btpEndpoint` and
an HTTP endpoint and never carried a raw-TCP endpoint; what changes is values and ownership
(issue #678), not schema.

---

## 12. Where this document sharpens ADR 0027

Recorded explicitly so review can accept or overturn each, rather than discovering them later.

1. **The HTTP claim header is `ILP-Payment-Channel-Claim`, not `Payment-Channel-Claim`.** ADR 0027's
   table wrote the header as `Payment-Channel-Claim`, mirroring the BTP entry name. The deployed
   client edge's header is `ilp-payment-channel-claim`, and the ADR's own governing rule is that the
   claim carriage is "reused verbatim" with one codec. A new header name would require a second
   decoder on the HTTP path, which is the drift I2 exists to prevent. §3 pins the deployed name.
2. **The peer credential's HTTP presentation was named here** — _superseded by
   [ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md) (issue
   #1157); kept because it records why the spelling was what it was._ ADR 0027 required a credential
   and §1.4 had to say what it looked like on HTTP; `Toon-Peer-Auth: base64(JSON)` was chosen to
   mirror the BTP `auth` entry's existing JSON exactly, so the two carriages shared one credential
   struct. The credential is now deleted on both carriages, and the mirroring argument transferred
   intact to the claim, which was always carried in the same two shapes.
3. **§6.4 restates the HTTP asymmetry more precisely than the ADR does.** ADR 0027 says the
   non-dialing side "cannot flush, and `flushIntervalMs` does not bound its trailing exposure at
   all." That is exactly true only in the residual case §6.4(2). In the ordinary accept-only
   configuration the non-dialing side is structurally a _payee_ — debt flows with packets, packets
   flow only in the dialing direction — so it has no trailing exposure of its own to bound, and the
   real loss is **unidirectional packet flow** (§6.4(1)). The ADR's _conclusions_ as originally
   recorded here were that the ceiling was still the accept-only side's only real bound and had to
   be explicit; both are retired ([ADR 0033](../adr/0033-the-exposure-machinery-is-retired-not-restated.md),
   issue #882) along with the ceiling itself. The hint is still only a hint.
4. **The idempotent re-ack (§6.3) is derived, not stated.** ADR 0027 fixes "missing ack means not
   acknowledged" and requires a timeout, both of which imply retransmission; nothing in the ADR or
   in `peer-semantics-pre-868.md` §3.2 says what a payee does with a byte-identical retransmission. Without
   the rule in §6.3, a lost ack permanently wedges a peering, since the payer's only honest
   retransmission is refused `nonce_not_advancing`. The rule is a strict narrowing of §3.2 that
   changes no exposure.
5. **Client-role fields are ignored, not refused (§1.7).** ADR 0027 states role-by-auth but not what
   a client interaction's peer-shaped bytes do. Ignoring is chosen over refusing so a client SDK
   that sets an unrecognised header is not broken by a peer feature, and so no error message
   discloses the peer surface.
6. **`ChannelInBothNamespaces` (§1.8).** ADR 0027 requires separate roles; the double-counting risk
   of one channel in both namespaces is not addressed there. Enforcing disjointness in config is
   the cheapest safe answer.
7. **The credential's `peerId` named the peering _relation_, so both operators wrote the same
   string (§1.4, §1.2 P1)** — _superseded by
   [ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md) (issue
   #1157); kept because it records an ambiguity found the expensive way, and because refusing a
   second name for one relation is why nothing replaced the field._ §1.4's example showed one
   credential and did not say whose id was in it, and P1 was stated from the accepting side ("a
   peer id `p` that appears in `[[peers]]`") — which left "the dialing side's own id, as the
   accepting side configured it" and "the accepting side's id, as the dialing side configured it"
   both readable. Issue #678 found the ambiguity the expensive way: the first real dial presented
   the id it had configured for the _remote_, the remote had no such entry, and the interaction was
   admitted as an ordinary client — correctly, silently, and uselessly. The resolution cost no new
   configuration surface: `[[peers]].id` was the relation's name on both sides, presented by the
   dialer and looked up by the accepter, so a peering established only when the two files carried
   the same literal string. There was deliberately no separate "the id this peer knows me by"
   field; a second name for one relation is a second thing to keep in step, and this is a bilateral
   configuration either way (`peer-semantics-pre-868.md` §4).

   **Nothing puts a peering id on the wire now, and the two operators need not agree on one.**
   `[[peers]].id` is a **local label**: it names a peering to this node's own `[[routes]]`,
   `[[peer_channels]]` and `[[pay_channels]]` rows, and the accepting side reads the peer id off
   the `[[peer_channels]]` row the claim's channel resolves to, never off the interaction (§1.2).
   A connector MUST NOT require the counterparty's file to spell the relation as its own file
   does, and MUST NOT take an id from an interaction at all (§1.3). **What the two files MUST
   agree on is the channel**: the on-chain channel the peering's claims are signed against and the
   counterparty key it was opened against, as `[[peer_channels]]` configures them (§11). A
   disagreement there is no longer invisible — it is P2 or P3, and §1.6 requires it be reported as
   `peer_auth_refused`.

8. **One node-wide, default-false opt-in may widen which endpoint _schemes_ resolve (§2.1).**
   §2.1's "any other scheme MUST be a load-time error" is kept as the default and as the only
   production configuration. A connector MAY offer a single explicit switch — this implementation's
   `peer_allow_plaintext_endpoints` — under which `ws://` resolves onto the BTP carriage and
   `http://` onto the ILP-over-HTTP one, for loopback and tests. It widens which schemes resolve
   and **nothing else**: the carriage each selects, the role rule, the claim and every other
   requirement of this document are unchanged, and a connector that offers it MUST log a loud
   startup event naming every plaintext peering. Per-peer forms of the switch are forbidden — a
   per-peer field reads as an ordinary property of that peering and travels into production one
   line at a time. The reason to have it at all is that without it the end-to-end proof of this
   specification cannot run anywhere but a deployment: two connectors on one laptop cannot dial
   each other, and a specification whose acceptance test needs a TLS terminator is one nobody runs.

Nothing in this document reopens ADR 0027's decisions: not the two carriages, not FLUSH-as-TRANSFER,
not the claim ack as a field, not role-by-auth, not the deletion of the raw-TCP wire.

---

## 13. Consistency

This specification uses exactly the vocabulary of `CONTEXT.md` (connector, app, packet, route,
client edge, claim, nonce, watermark, exposure, ceiling, flush, in flight, projection, settlement,
fee, probe — of which _exposure_, _ceiling_ and _flush_ are retired terms per
[ADR 0033](../adr/0033-the-exposure-machinery-is-retired-not-restated.md), and _minimum delivery_
per [ADR 0057](../adr/0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md); all four appear
above only in clauses marked retired or historical), adding **carriage**, **expose**, **dial** and **peering
relation** as defined in §0.1 and §2 — the first three from ADR 0027, the fourth already implicit in
`peer-semantics-pre-868.md` §3.3's "per peering relation".

It implements [ADR 0027](../adr/0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md)
and carries, without restating,
[ADR 0004](../adr/0004-value-moves-on-fulfilment.md),
[ADR 0005](../adr/0005-claims-are-truth-balances-are-a-projection.md),
[ADR 0010](../adr/0010-flat-per-packet-fee-and-minimum-delivery.md),
[ADR 0011](../adr/0011-rejects-accumulate-fees-and-probes-discover-cost.md),
[ADR 0016](../adr/0016-payload-opacity-is-a-property-of-carriage.md),
[ADR 0018](../adr/0018-a-payload-is-sealed-to-the-terminating-connector.md),
[ADR 0019](../adr/0019-a-terminating-connector-derives-the-fulfilment.md),
[ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md),
[ADR 0023](../adr/0023-oer-length-determinants-are-canonical.md),
[ADR 0024](../adr/0024-peer-wire-claims-sign-the-eip-712-balance-proof.md),
[ADR 0025](../adr/0025-an-envelope-target-is-confined-beneath-the-handler-path.md) and
[ADR 0069](../adr/0069-the-execution-condition-leaves-the-wire.md).

It does not reintroduce raw-TCP framing, a `transport` selector, a peer-specific claim encoding, a
quoting protocol, `lockedAmount`/`locksRoot`, the derived-preimage condition path, or a
positional claim acknowledgement.
