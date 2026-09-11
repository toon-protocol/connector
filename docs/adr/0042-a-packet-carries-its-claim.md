# A packet carries its claim

**Status:** Accepted — **built** (issue #1145). **Supersedes [0031](0031-a-peer-prepare-arrives-with-its-covering-claim-or-it-is-greeted.md)**, retires [0004](0004-value-moves-on-fulfilment.md)'s headline, and amends [0010](0010-flat-per-packet-fee-and-minimum-delivery.md) and [0011](0011-rejects-accumulate-fees-and-probes-discover-cost.md). Built: the cap (`max_packet_amount`, with a default), the send half on **both chains** (`[[pay_channels]]` populating `outbound_client_hops` — issue #881 for EVM, issue #1146 for Solana), the rule that a **forwarded** arrival must carry a covering claim (issue #1142, per-peer `forwarded_claim_enforcement`, still defaulting to observe), and — issue #1145 — the **deletion of the postpay path** that made all of the above optional. `[[pay_channels]]` is now required of a peering this node forwards to, refused at load by name. (`ClaimEnforcement::Observe` was another item once listed here; it is resolved — **deleted**, decided in issue #1062 and deleted from the tree in issue #1077 — and it never governed forwarded arrivals in the first place, since `payment_required` filtered to `ClientRouteKind::Terminated`.) ~~Until an operator writes `forwarded_claim_enforcement = "enforce"`, forwarding still runs [0004](0004-value-moves-on-fulfilment.md)'s model end to end.~~ **That sentence is retired: 0004's model is deleted from the tree, and forwarding is covered whether or not a peering enforces on arrival.** See the issue #1145 Update at the foot of this record.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

**Falsifier:** `crates/**/*.rs` matching `\bClaimEnforcement\b` — the type the `## Update (issue #1062)` section below said was deleted while it was still in the tree, and that issue #1077 actually deleted. The live `ForwardedClaimEnforcement` is a different identifier and the word boundary is what keeps them apart.

**Falsifier:** `infra/**/*.toml` matching `^\s*\[\[peers\]\]` — item 3's deploy caveat reasons from the fleet holding no peering (issue #872 removed both). The `## Update (issue #1145)` section below calls that "a fact about today's tree, not a property"; this is the line that makes a box config regaining a peering announce itself.

**Falsifier:** `deploy/**/*.toml` matching `^\s*\[\[peers\]\]` — the same fact for the two committed deploy templates. A commented-out example does not count and is skipped, which is deliberate: a commented `[[peers]]` block breaks nobody, an uncommented one puts the ordering constraint back.

Every packet carries the claim that pays for it. A PREPARE arrives at a connector with a covering
claim or it is refused, whether that connector will forward it or terminate it; and a connector
covers every PREPARE it sends. A **fulfilment proves that the intended receiver got the packet**
and nothing else — it is a delivery receipt, not a payment trigger. Value in flight is therefore
at risk, and bounding that risk is the **sender's** business rather than the protocol's: small
packets, and larger ones only on a path that has earned them.

## What this supersedes

**[ADR 0004](0004-value-moves-on-fulfilment.md)'s headline is retired.** "Value moves on fulfilment
and only on fulfilment" conflated two questions — _when is value owed_ and _what proves delivery_ —
and answered both with the fulfilment. This record keeps only the second. What survives from 0004 is
untouched: **one claim per packet, never batched**, and `lockedAmount`/`locksRoot` stay dead.

**[ADR 0031](0031-a-peer-prepare-arrives-with-its-covering-claim-or-it-is-greeted.md) is superseded
entirely**, not amended. It stated this rule for the peer role alone, and every clause of its
Decision was false of the shipped binary: an uncovered arrival was refused only at a _priced
termination_ (a forwarded destination prices at `0` and is admitted free); no connector has ever
covered a PREPARE it sends, because issue #881's proactive covering was never wired to config;
`ClaimEnforcement::Observe` is an escape hatch it claimed did not exist; and the credit window it
declared "gone as an operating mode" is the only mode that runs. Four false clauses is past the
point where amendment banners help a reader.

**[ADR 0011](0011-rejects-accumulate-fees-and-probes-discover-cost.md) loses one inherited
property.** Its "understating a fee is unprofitable — honesty needs no enforcement" was reasoned
from ADR 0004's postpay: a hop advertising a low fee and then rejecting earned nothing. Under this
record it banks the claim instead, so fee honesty becomes bounded rather than self-enforcing — by
packet size and by the cap below. ADR 0011 carries the amendment; nothing else in it changes, probe
economics included.

## The trade this makes, and does not hide

ADR 0004 rejected prepay on a specific argument: prepayment makes the execution condition
economically inert, because "a hostile next hop takes the claim and rejects the packet… Trustless
forwarding and prepayment cannot both be true."

**That argument is correct, and this record overrides it deliberately.** ADR 0031 asserted the
argument "remains sound and is not disturbed here" while doing precisely the thing it forbids; the
inversion was real and went unstated. Stating it: under this record the condition stops being an
economic guarantee and becomes a delivery proof. A hop can take a claim and refuse to carry.

What answers the objection is not a protocol guarantee but two bounds, and Interledger has always
worked this way:

- **A per-peer cap** on the amount carried in one packet (below), which is the most a single theft
  can take.
- **The sender's own packet sizing.** RFC 0018: smaller payments carry proportionally less risk —
  "in ILPv4, this is the default." RFC 0027 designs for "large volumes of low-value packets."

RFC 0027 does not say when ledger value moves relative to the packet — "payment channels are only
used for funding and rebalancing… not directly part of the ILPv4 packet flow" — so both this record
and ADR 0004 were always protocol-legal. This is a local choice, made because a hop that is paid
before it carries can always decline to carry, and no amount of protocol prevents that — only a
bound on what one packet is worth does.

An earlier draft of this record justified the cap by saying a peering "may have been bought", so a
counterparty that selected itself is owed no credit.
[ADR 0043](0043-purchasable-peering-is-removed.md) removed that premise outright: a peering cannot
be bought, so every peer is operator-chosen. **The cap is unaffected** — it is the dial law 03's "trust grows" turns for
_every_ peer, not a guard against self-admitted ones.

## The cap

**Every peering carries a maximum amount this connector will forward to it in one packet.** A packet
needing more is refused with `T04` — never carried, never split. The cap is how far a connector
trusts a peer, expressed as the most it is willing to lose at once: a new peering starts at the
floor; a path that keeps fulfilling earns a larger one.

It has a default, so an operator who never configures one is still bounded. A bound only an
attentive operator gets is not a bound.

This is not [ADR 0033](0033-the-exposure-machinery-is-retired-not-restated.md)'s ceiling returning.
That bounded an _accumulation_, and under this record no accumulation exists — a packet carries its
own claim, so nothing is ever owed between packets. The cap bounds **one packet**. ADR 0033 stands
as written, and its premise (that a covering claim bounds every peering) becomes true for the first
time once the work below lands.

## What must be true for this record to be true

None of this ships today. Listed in the order it must be built, and the order matters for one
reason only — the third item is the one that can break a running fleet:

1. **The cap, refused with `T04`, with a default.** Independent of the rest, and safe to land on its
   own: it only ever refuses a packet this connector would otherwise have carried.
2. **Wire issue #881 — the send half.** ~~Proactive covering exists in the runtime and is exercised by
   tests, but no production path populates `outbound_client_hops`.~~ **Built.** `[[pay_channels]]`
   populates `outbound_client_hops` from `connector-cli`'s own build chain, one row per peering this
   node pays: the channel, its EIP-712 domain, and that hop's client edge as the watermark authority
   (asked over `POST /ilp/claim-state` on every covered packet, never remembered). Additive as
   promised: a peering with no row behaves exactly as it did — `cover_forward` answers
   `NotConfigured`, the postpay `pending_claim` path runs.

   **`[[pay_channels]]` is no longer the only populator of `outbound_client_hops` (issue #1217).**
   `Connector::establish_peering` — [ADR
   0058](0058-a-peering-is-established-from-a-url.md)'s `POST /peers`, built after this record — now
   also registers a client-role hop, live, from the counterparty's own self-description; boot
   rehydration re-arms one for every durable runtime peering the same way. A peering with neither a
   `[[pay_channels]]` row nor a runtime hop is untouched by this — it is as uncovered as it was, and
   since #1145 retired the postpay fall-through named above, a forward to it is refused rather than
   carried free. What no longer holds is "no outbound client ledger file is opened at all": the ledger
   now opens for every node with a `state_dir`, `[[pay_channels]]` or not, because a
   runtime-established hop needs one too and nothing rebuilds a running node's wiring when
   `POST /peers` adds one.

3. **Require a covering claim on forwarded arrivals.** ~~The price gate filters on
   `ClientRouteKind::Terminated`, so a packet this connector forwards onward is carried for free.~~
   **Built, and defaulting to observing.** `price_gate::payment_required` now judges a
   `ClientRouteKind::Forwarded` arrival too, against **the packet's own `amount`** — not a price and
   not the fee. That is the symmetric figure: the send half covers the next hop for
   `amount_after_fee(amount, fee, minimum_delivery)`, so an upstream peer covering the amount that
   arrives here leaves this connector exactly its flat fee ([ADR
   0010](0010-flat-per-packet-fee-and-minimum-delivery.md)). The advance is measured against the channel's prior
   watermark by the same `validate_price` the terminated rule uses, and an unaccepted claim advances
   nothing however much it declares.

   **This one is breaking, so it is off by default.** It ships behind a **second** per-peer knob,
   `forwarded_claim_enforcement` (`connector_config::ForwardedClaimEnforcement`), separate from
   `claim_enforcement` and defaulting the other way: **`"observe"`** — the arrival is admitted and
   forwarded, and logged exactly as a refusal would be logged. An operator flips one peering to
   `"enforce"` once that peering's counterparty is covering its forwards. Two settings rather than
   one restructured setting, because the two migrations default in opposite directions and end on
   different days: the terminated migration's escape hatch is dated for deletion and has since been
   deleted (the Update below, issue #1062), and folding the two together would have deleted this
   item's default along with it. The terminated rule ([ADR
   0029](0029-a-peer-wire-arrival-to-a-priced-termination-must-cover-its-price.md)) is unchanged
   under every combination of the two.

   **Not covered by this item:** a destination reached over a **leased** route. `Connector::client_route`
   excludes leases by construction (ADR 0028, and ADR 0029's "leased routes are unaffected"), so a
   leased arrival is neither priced nor gated here — it is the same hole ADR 0028 already names, not
   the `Terminated` filter this item was about, and closing it is separate work.

4. ~~**Resolve `ClaimEnforcement::Observe`** — honour its 2026-11-01 sunset, or record that the escape
   hatch is permanent.~~ **Resolved: deleted** (issue #1077), and the sunset honoured **early** —
   2026-08-24, not 2026-11-01. The date was a deadline, not a wait: its own two preconditions (no
   `[[peers]]` row anywhere reading `Observe`, and the runbook confirming it) were already met, and
   there is no `[[peers]]` row on the fleet at all. `claim_enforcement` is now a
   parsed-and-rejected key.

Until (2) lands the connector runs ADR 0004's model end to end for forwarding, which is coherent and
RFC-shaped; it is simply not this record. **Three documents already assert otherwise** — ADR 0031's
Decision, ADR 0033's premise, and `docs/protocol/money-model-pre-868.md`'s "current behaviour" banner. This
record does not become a fourth: it says plainly that it describes the target.

(2) has since landed, and what it changed is narrower than "the fleet now covers": a peering covers
its forwards **once an operator writes `[[pay_channels]]` for it**, and no committed config on this
fleet writes one yet. So the sentence above still describes every deployed box, and stops describing
one the moment its config names a channel to pay from.

(3) has since landed too, and changes nothing about a deployed box until an operator writes
`forwarded_claim_enforcement = "enforce"` on a peering. Until then a forwarded arrival is admitted
and logged, which is what makes the order above survivable: the receive half can roll out fleet-wide
while the send halves are still being configured, and each peering closes when its own counterparty
is ready.

## Consequences

**The app is never a hop.** Claims ride client-to-connector and connector-to-connector links only.
The app holds no channel, settles nothing, and is handed ordinary HTTP that was already paid for.

**Trust buys packet size, never deferred payment.** A well-trodden path earns a larger cap. It never
earns the right to owe — batching and credit windows stay retired, because that is what would put an
accumulation back.

**A signature sits on the hot path of every packet at every hop**, as ADR 0004 already noted. That
cost is now permanent rather than provisional, and is what this record buys: no peering is ever owed
anything between packets, so no peering can be left holding value a counterparty declines to cover.

## Update (issue #1062) — `ClaimEnforcement::Observe` is to be deleted, and it was never the forwarded half

This record's Status line listed two unbuilt items together: requiring a covering claim on
**forwarded** arrivals, and the resolution of `ClaimEnforcement::Observe`. **They are independent, and
listing them together was misleading.** `connector_peer_btp::price_gate::payment_required` filters to
`ClientRouteKind::Terminated`, so `Observe` only ever governed the ADR 0029 price gate at a terminated
route. It never touched forwarding.

### `Observe` is deleted

(**Corrected by the Update below, issue #1077.** This section recorded the decision, and was written
in a tense that read as though the code change had already landed. It had not; it landed in issue
#1077 on 2026-08-24. Everything the section argues is unchanged.)

It was a migration ramp and said so — _"Migration-only (issue #883): the packet is admitted exactly as
it was before issue #880, but logged so an operator can confirm real admissions before flipping this
peering to enforce."_ Its own type documentation carried a dated sunset: delete once every `[[peers]]`
row reads `Enforce` and the rollout runbook confirms it, no later than 2026-11-01.

**Both preconditions were already met.** No committed configuration sets it — the single occurrence is
a commented-out example in `deploy/connector-rust/connector.toml`. And
`docs/operators/claim-policy-rollout.md` states outright that the two peerings the ramp existed for
were destroyed with the apex box (issue #872): _"there is nothing left to flip from `observe` to
`enforce` on either leg, and no live peering traffic on this fleet at all."_

### The reason that makes it more than housekeeping

**`Observe` is observable, so it cannot be excused as local policy.** A peer can tell: it receives
service without a covering claim. Under
[0047](0047-the-configuration-schema-is-implementation-detail-capabilities-are-law.md), an observable
fact is protocol law — so had this survived, the specification would have had to document the
protocol's own bypass. A protocol that ships a documented not-enforcing mode has specified how to
ignore it.

### What is given up, and why it is affordable

The canary step for a future peering bring-up, which `claim-policy-rollout.md` says it is still there
for. Smaller than it looks: **an enforce-mode refusal is already logged just as loudly** — the same
line, at the same level, by deliberate design (_"Refusing is logged rather than silent"_). An operator
bringing up a new peering can watch shortfalls today without admitting unpaid packets. What `Observe`
uniquely bought was not breaking **live** traffic while watching, and there is none to break until
somebody deliberately creates a peering — at which point re-adding a canary with a record explaining it
beats carrying one for years against a hypothetical.

`claim_enforcement` becomes a parsed-and-rejected key, the `ceiling` / `flush_interval_ms` /
`[peer_sale]` convention.

## Update — [ADR 0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md) completes an amendment this record declined to make

This record amended [0011](0011-rejects-accumulate-fees-and-probes-discover-cost.md) because banking
the claim made fee honesty bounded rather than self-enforcing, and left
[0010](0010-flat-per-packet-fee-and-minimum-delivery.md)'s minimum delivery listed as "unchanged".
**The same reasoning retires the floor**, and [0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md)
applies it: once the claim is banked before the check, rejecting on a declared floor returns nobody
their value and only moves where the packet dies.

0057 is **blocked on item 3 above** and says so. Until a forwarded arrival must carry a covering
claim, the floor is the only bound on erosion on a forwarded path, and it stays.

Item 3 has since been built (issue #1142), which is what 0057 was waiting on — but note what the
observe default means for the sentence above: the mechanism that bounds erosion exists on every
peering, and **binds** only on a peering an operator has written
`forwarded_claim_enforcement = "enforce"` on. Retiring the floor is therefore no longer blocked on
code, only on the rollout that flips the peerings that carry traffic.

**0057 is now built too (issue #1143).** The floor is deleted — field, both carriage bindings, both
vectors and the `R01` reject. Item 3's own text above still spells the send-side figure as
`amount_after_fee(amount, fee, minimum_delivery)`; the signature is now
`amount_after_fee(amount, fee)`, and the figure it names — the amount this hop forwards, which is
what a peer must cover on arrival — is unchanged. Erosion is bounded by the covering claim alone,
and on a peering still defaulting to observe it is bounded by the send half's own coverage until an
operator flips it.

**One correction to the paragraph above** (issue #1143, corrected): the floor is deleted, but the
`R01` reject is not — only its minimum-delivery meaning is. RFC 0027's own `R01`, _"the amount
received by a connector in the path was too little to forward"_, is what a hop answers when its fee
alone exceeds the arriving amount, and it is emitted at exactly the `amount_after_fee(amount, fee)`
call site this paragraph names. See [0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md)'s
corrected Update. Nothing else here changes.

## Update (issue #1077) — the deletion actually happened, and the record had run ahead of it

The `## Update (issue #1062)` section above is written throughout in the past tense — "`Observe` is
deleted", "`claim_enforcement` becomes a parsed-and-rejected key". **When it was written, neither was
true of the tree.** Issue #1062 was the decision; issue #1077 was the code change, and until #1077
landed this record described a binary that did not exist — the precise failure
[`README.md`](README.md)'s conventions exist to prevent. Nothing in #1062's reasoning is retracted:
only its tense was wrong, and the correction is recorded here rather than rewritten into it.

**What landed (issue #1077, 2026-08-24).** `ClaimEnforcement` and its `Observe` variant are gone,
with `PeerConfig::claim_enforcement`, `ConfigError::InvalidClaimEnforcement`, and the
`if enforcement == Observe` branch of `connector_peer_btp::price_gate::payment_required`. An
uncovered arrival to a priced termination is refused unconditionally; ADR 0029's refusal path — the
`F06` plus the x402 greeting — is byte-for-byte what it always was. `claim_enforcement` is parsed
solely so it can be **refused by name** (`ConfigError::PeerClaimEnforcementRemoved`), the
`ceiling` / `flush_interval_ms` / `[peer_sale]` convention, and the commented-out example in
`deploy/connector-rust/connector.toml` is deleted rather than left commented, because a commented
example of a rejected key is a trap for whoever uncomments it.

**The sunset was honoured early, not missed.** The date this record named was 2026-11-01; the
deletion is dated 2026-08-24. The date was always the _outer_ bound — "no later than the two-node
fleet epic closing, **or** 2026-11-01, whichever is first" — and the substantive precondition was
already met and had been since issue #872 destroyed the two peerings the ramp existed for. No
committed config sets the key: `infra/linode-relay/`, `infra/linode-store/` and every `local/`
topology carry no `[[peers]]` row that writes it, and the sole occurrence anywhere was the
commented-out example now deleted. Deleting it in November rather than August would have changed
nothing except how long the escape hatch sat in the tree.

**`forwarded_claim_enforcement` is untouched, which is the whole reason it was a second field.** ADR
0042 item 3's knob still defaults to `"observe"`, still admits-and-logs an uncovered _forwarded_
arrival, and is still the migration path for a rule no deployed box enforces. Item 3's own text says
folding the two settings together would have tied this default to the terminated hatch's dated
deletion; that deletion has now happened, and the default survived it exactly as intended. The two
error variants sit next to each other and the removed-key message says so, because the two spellings
differ by one word.

## Update (issue #1146): item 2's "Built." was true only on EVM

**Item 2 above records the send half as flatly "Built."** It was built for **EVM only**, and had been
since #881. A node could not cover a forward to a **Solana** peering at all, so such a peering could
only ever be paid **postpay** — [ADR 0004](0004-value-moves-on-fulfilment.md)'s model, the one this
record exists to retire, running on a leg this record claimed it had left behind.

Four pieces were missing, and each was EVM-shaped rather than merely absent:

- `[[pay_channels]]` had no `#[serde(untagged)]` Solana twin. `crates/connector-config/src/pay_channel.rs`
  said so outright and gave a reason — an outbound client claim is an EIP-712 balance proof and the
  outbound client ledger signs nothing else — so a Solana row was refused as an unknown field.
- `OutboundClientLedger::next_claim` took an `EvmDomain` and a secp256k1 `&dyn Signer`.
- `HttpClaimState` could sign only an EIP-712 claim-state challenge, so a covering payer had no way to
  ask a Solana peer where its claims stood — even though the **receiving** half,
  `verify_solana_claim_state_challenge`, already existed and was already wired into
  `POST /ilp/claim-state`.
- `cover_forward` minted `ClaimSignature::Evm` and nothing else.

**Since issue #1146 the send half is built on both chains.** A Solana `[[pay_channels]]` row names a
`channel_account` and a `client_edge_url`; its settlement program is
`[settlement.solana] program_id`, never declared by the row, so the program
[ADR 0053](0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md) signs into every
covering claim is by construction the one this node would redeem it through (issue #1128's rule).
`next_claim` takes an `OutboundClaimBinding` carrying a domain and its signer together, so a
secp256k1 key can never be paired with a Solana program id. The claim-state ask is the Solana
challenge the existing verifier accepts — base58 `channelAccount`, base64 ed25519 signature — rather
than a second design.

**One thing a Solana row must have that an EVM row need not**, refused at load naming the peer
(`ConfigError::PayChannelSolanaWithoutPeerChannel`): the same peering must also bind that channel as a
Solana `[[peer_channels]]` row. `programId` is a **required** field of the Solana claim wire, where an
EVM claim's EIP-712 domain fields are optional and simply ride absent; both peer carriages render it
from that peer-channel row. Without one, every covering claim the row minted would reach
`claim_json::encode` with nothing to write there — a caller bug it panics on, on the packet path, with
the money already committed. Holding one channel in both roles with one hop is the deployed shape this
record's own item 2 describes, so the requirement costs a real config nothing.

**What is still true of every deployed box.** Nothing above changes a running node: a peering covers
its forwards only once an operator writes `[[pay_channels]]` for it, and no committed config on this
fleet or in `local/` writes a Solana one yet. `local/mixed-chain`'s b-c leg remains postpay, and
converting it belongs with deleting the postpay path
([issue #1145](https://github.com/toon-protocol/connector/issues/1145)), which this unblocks.

## Update (issue #1145): the postpay path is deleted, and item 3's deploy caveat was stale

**The three items above are now all built, and the model they were replacing is gone from the
tree.** Until this landed, "a connector covers every PREPARE it sends" was a target with a
fallback underneath it: `Connector::cover_forward` answered `NotConfigured` for a peering with no
`[[pay_channels]]` row, `forward_via_peer_route` then rode `ClaimBook::pending_claim` — armed by a
_previous_ fulfilment via `ClaimBook::record_fulfillment` — and the peering was paid under
[ADR 0004](0004-value-moves-on-fulfilment.md)'s model, the one this record exists to retire. Item
2's own text describes that fallback as the thing that makes the send half "additive". It is not
additive any more.

What went:

- **The `NotConfigured` fallback.** `cover_forward` returns a claim or a reason it could not mint
  one, and a forward it cannot cover is refused with `T00` naming the hop. There is no arm that
  reaches the wire uncovered.
- **The peer role's postpay arming.** No fulfilment signs a peer claim; nothing reads
  `pending_claim` on the peer side; `Connector::with_peer_claim_channel` is deleted, so
  `[[peer_channels]]` is an **inbound** binding only — whose signature this node accepts on a claim
  naming that channel, and nothing else.
- **The FLUSH sweep.** `Connector::sweep_flush` and `ClaimBook::due_for_flush` existed to deliver a
  claim armed by a fulfilment that had no packet to ride. Nothing arms one. (The `flush` carriage
  itself is untouched: a peer may still send a standalone claim, and this node still accepts one.)

`ClaimBook::record_fulfillment` survives, and deliberately: `ClientPayoutLedger` wraps it for this
connector paying a **client** back (ADR 0026, issues #699/#770/#779), which is a different edge and
a live feature. Only the peer-role caller is gone.

### `[[pay_channels]]` is required, and that makes it a breaking deploy

A peering a `[[routes]]` entry forwards to must carry a `[[pay_channels]]` row, refused at load by
name (`ConfigError::PayChannelUnbound`) the way `PeerChannelUnbound` already refuses a peering with
no `[[peer_channels]]` row. A peering this node only ever _receives_ from needs nothing — that is
why the check is keyed on routes rather than on peerings.

Load-time rather than packet-time because the alternative is precisely what
[ADR 0009](0009-one-typed-config-file-no-environment-layer.md) exists to prevent: the node would
boot cleanly and then refuse every packet on that route. And a newly required key is a **breaking
deploy** by 0009's own definition — the binary and the box's bind-mounted TOML are a matched pair in
both directions, so the config lands first and the tag moves second. Nothing deployed is affected
today (see below), but the ordering rule does not depend on that.

### The deploy caveat this record carried was stale, and is now resolved

Item 3 said to enforce only once every box's send half was live, "because the other end is not
covering yet" — written when both devnet boxes held BTP peerings to the apex. **Issue #872 removed
them.** Re-checked at this change: `infra/linode-relay/connector-rust.toml`,
`infra/linode-store/connector-rust.toml`, `deploy/connector-rust/connector.toml` and
`connector.production.toml` contain no uncommented `[[peers]]`, `[[peer_channels]]` or
`[[pay_channels]]` table between them — the only uncommented array-of-tables in any of the four is
`[[routes]]`. Neither box forwards a peer packet, so deleting the postpay path cannot take the fleet
dark, and requiring a new key cannot refuse a fleet config's boot.

That is a fact about today's tree, not a property. **A fleet config that gains a peering brings the
ordering constraint straight back**, and this caveat went stale exactly once already by nobody
re-reading it.

### Item 3 is enforced somewhere for the first time

`local/mixed-chain`'s `a-b` row reads `forwarded_claim_enforcement = "enforce"`. ~~It is the only
peering in the repository where a node forwards a packet that arrived from a peer, and therefore the
only place the enforcing path can be exercised against a running image at all.~~ **The first clause
of that sentence was overtaken by `local/dealing` (issue #1299); what it was claiming is still true,
on a different fact.** See the issue #1303 Update below. It could not be
turned on before this: A had no `[[pay_channels]]` row, so crossing 1 arrived uncovered by
construction and enforcing would have deadlocked the peering rather than charging for it. A has one
now, and covers `amount_after_fee(1200, 100)` — exactly the 1100 that arrives at B.

`local/mixed-chain`'s `b-c` leg is prepay too, on Solana, which the Update above (issue #1146) made
possible. **No topology in this repository runs ADR 0004's model any longer**, and ADR 0004 says so.

## Update (issue #1303): a second topology forwards a peer arrival, and the conclusion rests on the setting

**The section above derived its conclusion from the shape of the repository's topologies, and the
shape moved.** It reasoned that `local/mixed-chain`'s B was the only node here forwarding a packet
that arrived from a peer, and concluded from that alone that its `a-b` row was the only place item
3's enforcing path could run against a running image. `local/dealing` (issue #1299, built for
[ADR 0071](0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)) is a second such node, and
forwarding a peer arrival is the whole of its subject: 6-decimal mock USDC in from A over EVM,
9-decimal mock SPL out to C over Solana, converted at B's declared rate as it crosses. The premise
is false as of that topology landing.

**The conclusion is not, because `local/dealing` leaves the setting alone on purpose.** Its `a-b`
row holds `forwarded_claim_enforcement` at its default `observe`, and
`local/dealing/connector-b.toml` says why in the row itself: that topology's subject is the
denomination boundary, turning the setting on there would give one fact two homes to drift between
without proving anything new, and what rules out a crossing carried for free there is the rehearsal
reading C's journal for the **converted** figure — a stronger check than admission, since a claim
can be present and still be for the wrong number. So `local/mixed-chain`'s `a-b` remains the only
row in this repository where `forwarded_claim_enforcement` does anything, and therefore still the
only place item 3's enforcing path is exercised against a running image.

**What carries the claim is now the setting rather than the forwarding shape**, which is the sturdier
of the two: a third topology that forwards a peer arrival costs this record nothing, while a second
row writing `"enforce"` costs it the sentence.
[`docs/operators/claim-policy-rollout.md`](../operators/claim-policy-rollout.md) was re-read at this
change and needed no amendment — it already reasoned from the setting, calling `a-b` "the worked
example, and the only place in the repository this setting does anything".
`local/mixed-chain/connector-b.toml` was corrected in #1299 and names `local/dealing` itself;
`connector-a.toml` claims only that the setting is turned on nowhere else, which was true as
written. The two doc comments in `crates/connector-bin/tests/local_topologies_load.rs` that had
copied the old premise are corrected with this record.

**No falsifier is written for this sentence, and the reason is worth recording.** The claim to check
is "no row outside `local/mixed-chain/connector-b.toml` turns this setting on", and the marker's
grammar is one path glob plus one regex, with no way to exempt a path and no way for a regex to see
which file matched it. The natural pattern — the key, under `local/**/*.toml` — fires on the very
row this record is describing, and a gate that cries wolf gets deleted. Enumerating the other
topology directories one glob per line would pass today and silently cover no topology added
tomorrow, which is a check that looks complete and is not — the same failure one level up. So this
one stays a human's job, alongside `docs/adr/README.md`'s index rows, which
`records_state_their_own_falsifier.rs` declines to cover for the same kind of reason.
