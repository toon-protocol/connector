# The execution condition leaves the wire

**Status:** Accepted — built (issue #1269). Amends [0014](0014-metrics-surface-and-packet-correlated-logs.md)
(cross-hop log correlation by execution condition is retired, not replaced), corrects
[0019](0019-a-terminating-connector-derives-the-fulfilment.md)'s `accept_if_fulfilled` rationale, and
corrects `condition.rs`'s residual assertion of [0004](0004-value-moves-on-fulfilment.md)'s retired
model (issue #417's own doc already citing [0042](0042-a-packet-carries-its-claim.md) correctly one
paragraph below it), and corrects [0026](0026-client-btp-rides-the-client-edge-peers-stay-on-the-peer-wire.md)'s
payout-dedupe narrative (`(channel_id, execution_condition)` becomes `(channel_id, fulfillment)`).
Variant **B** of the two measured in issue #1268; variant C (per-hop
re-randomization of `data`) and variant D (a validity proof) are not in scope and are not blocked on
this.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

**Falsifier:** `crates/**/*.rs` matching `execution_condition` — this record claims the field is gone from the workspace's Rust source entirely; a match outside a comment (comments are skipped, so citing the retired name by history is not itself a regression) means it has come back.

`Prepare` no longer carries an execution condition. No hop reads one, no hop verifies one, and there
is nothing invariant left on the wire for two hops to join their logs on. The sender's existing
end-to-end check — comparing a returned fulfilment against `derive_fulfillment` of its own sealed
secret — is now the only check, which is where it always belonged.

## Context

Every packet this connector forwards used to carry a 32-byte `execution_condition`, copied verbatim
at every hop. `Connector::correlation_id` hex-encoded it as a cross-hop log correlation id, and ADR
0014 documented why that worked: the value was _"invariant across every hop."_ That is exactly what
makes a value a perfect join key — invariant **and** distinctive per packet — so any two hops on a
path, or anyone reading two hops' logs, could trivially link the packet they each saw. An operator
forwarding traffic could offer their sender no better privacy than "the hop after me knows which
packet of yours this was."

The check that value bought was worth nothing to the hop paying for it:

- Under ADR 0042 a hop is paid on arrival; a fulfilment is _"a delivery receipt, not a payment
  trigger."_ Verifying a candidate fulfilment against the condition protected nothing the hop owns.
- Both `accept_if_fulfilled` implementations (`connector-runtime`'s and its client-edge twin in
  `session_route.rs`) answered a mismatch with `f99_application_error` carrying whatever price was on
  offer — **a hop that caught a forgery still charged for it.** The sender paid either way.
- At a route termination the check was a tautology: `deliver_opened_envelope` derived the fulfilment
  from the shared secret it had just opened, then checked that derivation against a condition minted
  from the same secret by the same sender. If the wrap opened, it was sealed to this connector; the
  condition added nothing on top.
- The sender **already** verifies end to end. `connector send` compares the returned fulfilment
  against `derive_fulfillment(&shared_secret)` and reports `FulfilledWithWrongFulfillment` — this
  predates the change and needed no new code to keep working once nothing else checked anything.

The value did not even need to be transmitted: it was `derive_condition(derive_fulfillment(
shared_secret))`, and the shared secret already travels inside the gift wrap (ADR 0018). The sender
minted it; a termination recomputes it. Every byte of it on the wire was redundant data whose only
working effect was to correlate.

Separately, RFC-0027's `MUST NOT modify` on this field was added by a technical writer in
[interledger/rfcs#475](https://github.com/interledger/rfcs/pull/475) (2018-09-05), seven months after
ILPv4 was numbered, with no design discussion attached — this connector already departs from RFC-0027
encoding-wise (ADR 0063), and dropping a field the RFC never seriously defended is a smaller thing
than ADR 0063 already decided to do.

## Decision

**`execution_condition` leaves the `Prepare` packet entirely.** No hop reads it, no hop verifies it,
and there is nothing invariant left on the wire for two hops to join on:

```
Prepare ::= SEQUENCE {
    amount              UInt64,
    expiresAt           GeneralizedTime,
-   executionCondition  OCTET STRING (SIZE(32)),
+   greeting            BOOLEAN,
    destination         VarOctetString,
    data                VarOctetString
}
```

Net −31 bytes per packet per hop. `Fulfill.fulfillment` is untouched — the return direction's 32
bytes and their meaning are unchanged.

A greeting probe — until now identified by an all-zero condition — gets an explicit `greeting` flag
instead, so the bootstrap-probe discriminator survives the field that used to carry it. A boolean is
not a join key: ADR 0014's correlation property needed a value that was invariant **and** distinctive
per packet, and one bit partitions traffic into two enormous classes, distinguishing nothing.

### What is deleted

- `Connector::accept_if_fulfilled` and its client-edge twin in `session_route.rs`. Both become
  pass-through: a candidate FULFILL — from a peer, or from a bound client session — rides home as a
  FULFILL. The `f99_application_error` mismatch branch and its `accumulated_cost` argument go with
  them. `RejectCode::f99_application_error` itself stays, at its decided name, with no live producer
  today — the same shape ADR 0014 already accepted for `toon_exposure`.
- `Connector::correlation_id`'s condition-hashing form, and `reject_ineligible`'s condition arm (the
  `F01 prepare carries no execution condition` reject, issues #417 / #803).
- `condition_is_present` and `derive_condition` leave `connector-domain`'s public surface (the second
  stays as a private helper inside `condition.rs`, as `fulfillment_matches_condition`'s own
  arithmetic). `derive_fulfillment` (`connector-signer::giftwrap`) stays exported: the **sender**
  still uses it, and `connector send`'s own end-to-end check — comparing a returned fulfilment
  against `derive_fulfillment(&shared_secret)` directly — is where the one surviving check lives.
  `fulfillment_matches_condition` also stays exported, but has no production caller in this
  workspace today: a direct fulfilment-to-fulfilment comparison needs no intermediate condition,
  so it is kept as RFC-0022's relation stated plainly, not as something `connector send` calls.

### The payout dedupe key

`ClientPayoutLedger::record_payout_once` / `ClientClaimGate::credit_session_payout` used to dedupe
a client-session payout credit (issue #770 AC3) on `(channel_id, execution_condition)`. It becomes
`(channel_id, job_id)`, where `job_id` is a hash of the PREPARE
`crate::session_route::route_prepare` is about to hand the session, computed **before** the
session ever answers.

That order is load-bearing, not incidental, and is the one place in this record where "a
fulfilment rides home unchecked" cannot simply be extended by analogy. A candidate FULFILL from a
peer or a session is trusted for the _packet's own outcome_ because nothing downstream depends on
it being genuine — the sender's own end-to-end check catches a forgery, and no hop's money is at
risk either way (ADR 0042). But the payout dedupe is different: it is _this connector's own money_,
paid to the session, and its only defence against paying the same job twice is telling "a genuine
second job" apart from "a retry of the first one." Keying that on anything the session supplies —
its FULFILL's own `fulfillment`, say — would hand the decision to the party being paid: a dishonest
or buggy session could answer one retried job with different bytes each time and collect a fresh
credit on every retry, since nothing checks whether that value means anything. `job_id` keeps the
property the execution condition used to buy for free: the identity of "the same job" is fixed by
what this connector asked for, never by what came back. Determinism holds the same way it always
did: a genuine retransmission of one job re-sends byte-identical `data` (ADR 0018's sealed wrap
fixes its own ciphertext at encryption time), so it hashes to the same `job_id`; a different job's
freshly sealed `data` does not.

### The greeting discriminator

Both carriages — `connector-client-edge`'s HTTP `handle_ilp` and its BTP mirror in `btp.rs` — read
`prepare.greeting` where they read `condition_is_present` before. Behaviour is otherwise identical: a
greeting probe is never routed, never priced and never fulfilled; it is answered with `402` (HTTP) or
an `F06` REJECT carrying the same terms (BTP). The flag is consulted only when no claim header is
present — a present claim always suppresses the greeting exactly as it did before — so a sender
cannot use `greeting: true` to get a packet routed, priced or delivered for free; it can only ever
broaden when the _unclaimed_ case is greeted, never narrow when the _claimed_ case is charged.

`packages/announcer/src/oer.ts`'s `encodePrepare` and `edge-client.ts`'s `fetchGreeting` build the new
shape: a `greeting` boolean in place of the 32-byte `executionCondition` field.

### Log correlation after ADR 0014

Cross-hop correlation is **retired, not replaced** — it is the thing being removed. Each hop now
mints its own random per-packet id at packet entry (`connector::correlation_id`, 16 bytes from the
OS RNG, hex-encoded), held in the existing `"packet"` tracing span and never placed on the wire.
Within one node the logs correlate exactly as well as before; across two nodes they no longer join,
which is the point. A runbook that pipes `jq 'select(.fields.correlation_id == "...")'` across two
boxes' logs now fails loudly (no matching lines on the far side) rather than silently succeeding —
which is the intended failure mode for a mechanism this record retires, not an oversight left open.

### The cross-repo contract

`vectors/wire-vectors.json`'s `schema_version` moves 4 → 5. `PreparePacketFields.execution_condition_hex`
is replaced by `PreparePacketFields.greeting`; `FulfilmentCase.condition_hex` and `.matches` are
deleted, narrowing that section to `derive_fulfillment`'s own determinism, the one relation a
downstream implementer still needs. `toon-client`, `rig` and `swap` break at the contract until they
follow; that is intended and is why the version moves.

## Considered options

**Keep the condition, stop checking it.** Rejected: an unchecked-but-present field is still the same
join key ADR 0014 exploited, and removing the check while keeping the bytes buys none of the privacy
property and none of the byte savings — it would be worse than either doing nothing or doing this.

**Randomize the condition per hop (variant C, issue #1268).** Would remove the join key while keeping
RFC-0027's field, at the cost of per-hop re-encryption of `data` (since a condition minted from a
resealed secret must still equal what the termination derives) — the more invasive of the two
variants #1268 measured, and out of scope here. Nothing in this record forecloses it; it remains
available as a later, additive change.

**A validity proof (variant D, issue #1268).** Measured at 2.69 core-seconds per hop against a
one-second per-hop message window in the #1268 prototype — not close, and not a prerequisite for
this record's decision.

## Consequences

**This removes a correlator; it does not create an anonymity set.** An anonymity set is a function of
concurrent traffic volume. Shipping this makes the network's privacy no worse than its traffic and no
better than that. `data` is still copied byte for byte at every hop and is a better correlator than
the condition ever was — this record does not touch it and makes no unlinkability claim.

**Not a performance change, and not justified as one.** The #1268 prototype measured this variant at
roughly double the packet-handling throughput per core against keeping the condition (~0.001ms vs
~0.002ms per hop) — real, but ~1μs in absolute terms, noise beside network and settlement latency.
The −31 bytes per packet per hop is the same story: real for a length-priced route (ADR 0065), and not
the point.

**No config key changes.** This adds no required config, so it is not a breaking deploy in the sense
`[node]`/`[operator]` keys are. It does change the wire, which for the relay and store boxes means
their node repositories' own pins must move together (ADR 0068) — each repo's own reviewed change,
not a step here.

**`docs/rfcs/0027`'s TOON profile is amended, not the vendored body.** The profile above the marker
now records a fourth divergence — this connector's PREPARE carries no `executionCondition` at all —
alongside the three encoding ones ADR 0063 already recorded. `vendored_rfcs_are_unmodified.rs` keeps
passing because the body byte range is untouched.

**`CONTEXT.md`'s Condition and Fulfilment entries are corrected.** Condition no longer names a thing
this connector's packets carry; Fulfilment's entry — already accurate — is now the only one of the
pair describing a wire field.
