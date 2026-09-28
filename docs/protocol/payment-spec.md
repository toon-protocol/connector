# Payment

**Status:** **Normative for its numbered rules.** Absorbs `peer-semantics-pre-868.md` §5.2 and the
shape of `money-model-pre-868.md` — the joining-piece role, not its text. Both are frozen history
(issues #1056, #1065); this document is what describes what money does **now**.

**Coverage:** none of PM-01 – PM-22 is vectored except where noted — the EIP-712 claim digest and the
peer-carriage claim framing are, and are the only payment facts under the cross-repo contract today.
The rest enter [ADR 0045](../adr/0045-a-behavioural-rule-is-normative-prose-until-its-vector-lands.md)'s
debt ledger; issue #1084 owns the order.

**Vocabulary:** [`CONTEXT.md`](../../CONTEXT.md). MUST, MUST NOT, SHOULD, MAY per RFC 2119.

---

## 1. What money is here

A **payment channel** is a two-party agreement anchored on a chain that lets value move between the
parties many times while touching the chain only to open, top up and close. A **claim** is a signed
statement of that channel's cumulative state, handed from payer to payee.

**PM-01** `[any participant]` — Each claim supersedes the last. A lost claim costs nothing and a
replayed claim gains nothing.

**PM-02** `[connector]` — **Claims are the source of truth.** Balances are a **projection** — derived
by replaying the journal, never a second store that can disagree with it. Recovery is replay, not
reconciliation.
([ADR 0005](../adr/0005-claims-are-truth-balances-are-a-projection.md))

**PM-03** `[connector]` — The **journal** is the only money state a connector persists: what was
signed or is otherwise irreversible — claims sent, claims accepted, and the watermarks that came with
them.

**PM-04** `[connector]` — A connector MUST refuse to start on a corrupt journal. A damaged record of
what was signed is not recoverable by guessing.

---

## 2. What authorises a packet

**PM-05** `[connector]` — A **verified claim** authorises a packet. An identity does not, and cannot
substitute for one.
([ADR 0052](../adr/0052-permissionless-payment-is-guaranteed-and-a-claim-is-what-authorises.md))

**PM-06** `[connector]` — A claim's signature MUST be verified against **this connector's own record
of the channel** — a configured row, or a channel resolved from the chain its settlement backend
already names — and **never** against anything the claim declares about itself.

**PM-07** `[connector]` — **Unverifiable is never accepted, by configuration, flag or build profile.**
A registry with neither a record nor a source refuses; a source that cannot answer — an unreachable
RPC endpoint — refuses the claim it was asked about, distinguishably and never silently. There is no
degraded mode.

**PM-08** `[connector]` — A **nonce** orders claims within a channel, and a payee accepts a claim only
if its nonce advances its **watermark**. A byte-identical retransmit at an accepted nonce is
idempotent; a _different_ claim at an accepted nonce MUST be refused and MUST NOT move the watermark.

---

## 3. What a claim signs

**PM-09** `[any participant]` — An **EVM** claim signs the EIP-712 `BalanceProof` digest, covering
`channel_id`, `nonce`, `transferred_amount` — **and `chain_id` and `token_network_address` through the
domain separator.** Changing any one of them invalidates a prior signature.
([ADR 0024](../adr/0024-peer-wire-claims-sign-the-eip-712-balance-proof.md)) — **vectored.**

**PM-10** `[any participant]` — A **Solana** claim signs an Ed25519 message that MUST bind the
settlement program and the cluster alongside the channel account, nonce and transferred amount. A
claim's signature MUST NOT verify against a channel account alone.
([ADR 0053](../adr/0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md)) — **not yet
built** (#1082); today the message binds neither.

**PM-11** `[connector]` — A claim's declared `blockchain` is a **routing hint, never a security
boundary.** It selects which verifier runs; the signed bytes underneath must commit to the chain that
verifier is actually on.

> PM-10 and PM-11 together are why issue #975 asks for the wrong fix. The declared `cluster` is not
> signed over, so checking it catches an honest misconfiguration and nothing else — a forger declares
> whichever cluster the receiver expects and the signature still verifies. **A check on an unsigned
> field reads like protection and is not.**

---

## 4. A packet carries its claim

**PM-12** `[any participant]` — The claim that pays for a packet travels **with** it, not behind it.
Nothing is owed between packets, so there is no window in which a counterparty can walk away.
([ADR 0042](../adr/0042-a-packet-carries-its-claim.md))

**PM-13** `[connector]` — **One claim per packet, never batched.**
([ADR 0004](../adr/0004-value-moves-on-fulfilment.md), the half that survives)

**PM-14** `[connector]` — An arrival at a **priced termination** whose claim does not cover the
route's price MUST be refused `F03` before delivery.
([ADR 0029](../adr/0029-a-peer-wire-arrival-to-a-priced-termination-must-cover-its-price.md)) — **live.**

**PM-15** `[connector]` — An arrival with **no** covering claim at all MUST be refused `F06`, with the
greeting that states the terms.

> **State of PM-12, stated plainly rather than aspirationally.** The rule is **live at a priced
> termination**, and — since issue #1142 — judged at a **forwarded** arrival too, against the
> packet's own `amount`. That half ships behind the per-peer `forwarded_claim_enforcement` knob
> **defaulting to `"observe"`**, so an uncovered forwarded arrival is still admitted, forwarded and
> logged until an operator writes `"enforce"` on a peering: the rule exists and binds no deployed
> box. (This paragraph said "not yet built for a forwarded arrival" until issue #1146 corrected it.)
> **The other side of the same hop is not merely built but unavoidable** (issue #1145): a node
> covers every PREPARE it forwards, minted before the packet is sent and never recovered after a
> refusal (issue #881) — on **both** settlement chains since issue #1146, EVM-only before it, which
> is why a Solana peering could until then only be paid postpay. The `[[pay_channels]]` row that
> supplies the channel is now **required** of a peering a route forwards to, refused at load without
> one, and a forward that cannot be covered is refused rather than carried. Since
> [ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) (issue #1380)
> the covering claim is a voucher on this node's own outbound x402 channel toward the hop, which the
> `[[pay_channels]]` row names, signed by that chain's settlement key. So "a claim trails its
> fulfilment" is no longer true of anything: ADR 0004's model is deleted from the tree.
> [ADR 0042](../adr/0042-a-packet-carries-its-claim.md) is no longer a target record on this point;
> its issue #1145 Update is the authority on what landed.

**PM-16** `[connector]` — **Exposure and its ceiling are retired**, not restated. Nothing tracks value
delivered but unclaimed, and no configuration bounds it. A `ceiling` or `flush_interval_ms` key is
refused by name.
([ADR 0033](../adr/0033-the-exposure-machinery-is-retired-not-restated.md))

---

## 5. Fee, price and cost

**PM-17** `[connector]` — A **fee** buys carriage across one peering relation. A **price** buys the
work at the end of a terminated route. A fee is **flat per packet**. A price is a **schedule** over
the packet's payload length — `base + per_kib × ceil(len / 1024)` — and is flat exactly when its
slope is zero. Carriage work does not scale with a payload the way the work behind a termination
does, which is why only one of the two gained a slope.
([ADR 0010](../adr/0010-flat-per-packet-fee-and-minimum-delivery.md),
[ADR 0065](../adr/0065-a-price-is-a-schedule-over-payload-length.md))

**PM-17a** `[connector]` — The length a price is evaluated at is the packet's own `data` length: the
**sealed** payload, never anything inside it. That is a property of carriage — every hop can measure
it without opening the wrap — which is what lets a forwarded route be priced at the client edge and a
peer arrival be gated by the same rule the termination charges under.
([ADR 0065](../adr/0065-a-price-is-a-schedule-over-payload-length.md))

**PM-18** `[operator]` — Pricing granularity is **handler** granularity: one handler, one price. An
operator charges differently for different work by publishing a route per handler — never by letting
one route's price vary with what a packet **carries**. That is how a connector prices without ever
interpreting what it carries. Charging differently for the same work at different **sizes** is the
price's own slope, not a second handler.
([ADR 0020](../adr/0020-a-price-is-flat-and-attaches-to-a-handler.md),
[ADR 0065](../adr/0065-a-price-is-a-schedule-over-payload-length.md))

**PM-19** `[client]` — **Cost** is what a caller must send for a packet to be delivered: every hop's
fee plus what the terminating route charges **for that packet**. A reject states the cost of the path
it travelled — **the sum only**, never the per-hop breakdown and never the fee/price split. That is
what a **probe** discovers. Where a terminating price carries a slope the probe's figure is exact for
a packet its own size; what answers every size is the terminating node's **published schedule**
(CF-13c), so a sender still needs one read rather than one probe per size.
([ADR 0011](../adr/0011-rejects-accumulate-fees-and-probes-discover-cost.md),
[ADR 0065](../adr/0065-a-price-is-a-schedule-over-payload-length.md))

**PM-20** `[connector]` — A **forwarded** route is priced at the client edge, and carries no more than
it was paid. The invariant `price − fee >= next hop price` is what protects a client across a path.
No sender declares a floor of its own — minimum delivery is retired, and what bounds erosion is the
claim covering each crossing. Where prices carry a slope that invariant must hold at **every** payload
length, so each hop's base must clear the next hop's by its fee **and** each hop's slope must be at
least the next hop's (ADR 0065).
([ADR 0028](../adr/0028-a-forwarded-route-is-priced-at-the-client-edge.md),
[ADR 0057](../adr/0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md))

---

## 6. What the app is told

**PM-21** `[connector]` — A delivery whose covering client claim **this** connector verified itself
states `X-TOON-Payer`, `X-TOON-Amount` and `X-TOON-Chain` to the app. A delivery this connector did
not verify states **none of the three** — an unverified payment is stated by nobody.
([ADR 0040](../adr/0040-a-verified-payment-is-stated-to-the-app.md))

**PM-22** `[app]` — An app MUST NOT assume those headers are present. Their absence means _this
connector cannot vouch for the payer_, not that the work was unpaid: whatever arrives at a handler was
paid for, at that handler's one price.

---

## 7. Settlement

**Settlement** is making a claim's promised value real on chain — redeeming the latest claim, or a
cooperative close. Rare and deliberate, the opposite of claims, which are constant and automatic.

A connector holds a **signer** and settlement backends: a local key or a key-management backend, with
rotation, and nothing more. **No mnemonic recovery, no seed management, no human authentication** —
those belong to an end-user wallet, which a connector is not.
([ADR 0012](../adr/0012-a-signer-and-a-treasury-not-a-wallet.md), the half that survives)

### The lock, and its two deadlines

A covered forward **locks** value rather than transferring it, so value releases against a verified
fulfilment (#1031, and the record #1034 will produce). Its expiry semantics are decided:

```
lock timeout  ≥  packet expiry  +  margin
```

These are not competing deadlines. **Packet expiry** bounds how long a _hop_ waits for a fulfilment —
packet-plane, per-hop, strictly decreasing along the path. **Lock timeout** bounds how long _capital_
stays locked before the payer can reclaim it without cooperation — settlement-plane. The on-chain
figure is derived from and strictly bounds the ledger-layer one.

**A node that cannot satisfy the relationship refuses to forward** rather than locking with an unsafe
timeout — _refuse, not refund_. And each lock carries **its own** deadline: packet expiries vary per
packet, while a channel's settlement timeout is a single constant used for close.

---

## 8. Consistency

Uses exactly the vocabulary of [`CONTEXT.md`](../../CONTEXT.md) and implements
[ADR 0004](../adr/0004-value-moves-on-fulfilment.md),
[0005](../adr/0005-claims-are-truth-balances-are-a-projection.md),
[0011](../adr/0011-rejects-accumulate-fees-and-probes-discover-cost.md),
[0012](../adr/0012-a-signer-and-a-treasury-not-a-wallet.md),
[0020](../adr/0020-a-price-is-flat-and-attaches-to-a-handler.md),
[0024](../adr/0024-peer-wire-claims-sign-the-eip-712-balance-proof.md),
[0028](../adr/0028-a-forwarded-route-is-priced-at-the-client-edge.md),
[0029](../adr/0029-a-peer-wire-arrival-to-a-priced-termination-must-cover-its-price.md),
[0033](../adr/0033-the-exposure-machinery-is-retired-not-restated.md),
[0040](../adr/0040-a-verified-payment-is-stated-to-the-app.md),
[0042](../adr/0042-a-packet-carries-its-claim.md),
[0052](../adr/0052-permissionless-payment-is-guaranteed-and-a-claim-is-what-authorises.md),
[0053](../adr/0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md) and
[0057](../adr/0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md).

**Not yet built:** PM-10's Solana domain binding (#1082), PM-12's forwarded half (#1031), and the
lock (#1031, #1035–#1037). Each says so above rather than being written in the present tense.
