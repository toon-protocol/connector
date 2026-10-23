# A reject code binds where a sender must act differently, and only there

**Status:** Accepted, **the table is stated here for the first time**. Extends [0011](0011-rejects-accumulate-fees-and-probes-discover-cost.md), whose probe cannot work without it. Corrects three documents that claim nothing emits `T04`, alongside [0049](0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md).

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

**The reject codes are RFC 0027's; what this protocol adds is which code answers which situation.**
That mapping binds a second implementation **only where a sender can do something different on
receiving the specific code than it could knowing only the class.** Everywhere else, the class binds
and the code is the implementation's own.

## The test

> Can a sender take a different next action on this code than it could on its class alone?

If yes, the code is law. If no, binding it would freeze one implementation's internal taxonomy for no
interoperability gain.

This is not a stylistic split. [ADR 0011](0011-rejects-accumulate-fees-and-probes-discover-cost.md)
makes a probe the way cost is discovered — and a probe is a packet sent expecting a reject. A sender
that cannot tell _no route_ from _you did not pay enough_ from _you attached no claim at all_ learns
nothing actionable from one, and the probe stops working. Class-only was rejected on exactly that.

## The table

**Binding — the specific code is law.** Each names a distinct, actionable next move.

| code  | situation                                                                                                                                                                 | what a sender does about it                     |
| ----- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------- |
| `F00` | the app refused the envelope's target — it named something this handler does not expose                                                                                   | fix the target                                  |
| `F02` | no route matches the destination                                                                                                                                          | this path is wrong; find another                |
| `F03` | the amount exceeds the route's price, or the claim does not cover a priced termination ([0029](0029-a-peer-wire-arrival-to-a-priced-termination-must-cover-its-price.md)) | pay the stated amount                           |
| `F06` | no covering claim was attached at all                                                                                                                                     | attach one                                      |
| `F99` | a fulfilment does not satisfy the condition the sender minted                                                                                                             | **stop trusting that counterparty** — see below |
| `R01` | amount minus this hop's fee falls below the declared minimum delivery ([0010](0010-flat-per-packet-fee-and-minimum-delivery.md))                                          | lower the floor, or take a cheaper path         |
| `T04` | over this peering's cap ([0049](0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md))                                                          | send smaller — and the message states the cap   |

**Class-only — `F`, `T` or `R` binds; the code does not.**

| code  | situation                                                                                                                                                                                           | why class is enough                                                         |
| ----- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| `F01` | undecodable envelope · zero or absent condition · malformed minimum-delivery header · a termination reject that could not be sealed                                                                 | the packet is malformed; there is one next move, and it is "fix the packet" |
| `R00` | the packet expired                                                                                                                                                                                  | relative by construction — the sender's own clock decided it                |
| `T00` | **this connector's own configuration error** — a destination matching both a live client session and a locally terminated route ([0032](0032-a-client-destination-is-never-a-route-termination.md)) | nothing about the packet is wrong; retry later                              |
| `T01` | the app or the next peer is unreachable                                                                                                                                                             | retry later                                                                 |
| `T05` | rate limited                                                                                                                                                                                        | retry later                                                                 |

`T00`'s choice is worth keeping even though it does not bind: an operator's misconfiguration is
reported as an **internal error**, never as a verdict on the packet — _"not a value judgement about
the packet"_. A second implementation should reach the same conclusion; it need not reach the same
code.

## `F99` binds against the test, deliberately

By the test above `F99` would be class-only: a sender cannot retry its way out of it. It binds anyway,
because **it is evidence about a counterparty rather than about a packet.** A fulfilment that does not
satisfy the condition the sender itself minted means the party that returned it is misbehaving or
lying — and a sender that can detect that should be able to, since the correct response is to stop
trusting a peer rather than to adjust a packet. Temporary-looking and permanent-meaning is exactly
when a specific code earns its place.

## What a reject carries besides its code

**The accumulated cost of the path it travelled** ([0011](0011-rejects-accumulate-fees-and-probes-discover-cost.md)) —
the sum only, never the per-hop breakdown and never the split between fees and price.

**`T04` states the current cap in its message** ([0049](0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md)).
That is the whole discovery mechanism for a cap, and a bare `T04` would satisfy every other rule while
leaving a sender unable to size anything.

**Sealed or not, and what that proves.** A reject raised _at_ a termination is sealed back to the
sender; one raised short of it cannot be. The converse does not hold — a termination that never
recovered the shared secret also answers in plaintext ([0018](0018-a-payload-is-sealed-to-the-terminating-connector.md),
as amended). **Sealed identifies the destination; unsealed identifies nobody.**

## Consequences

**An app's `404` is not a reject.** It arrives as `Answered` and rides home on a **FULFILL**: the app
answered, and the answer is what was paid for. Only an app that is unreachable (`T01`) or that refuses
the envelope's target (`F00`) produces a reject. This is the sharpest thing in the table and it was
written down nowhere.

**Three documents still claim nothing emits `T04`** — `connector-domain/src/packet.rs`'s constructor
doc, [0033](0033-the-exposure-machinery-is-retired-not-restated.md)'s body, and
`peer-semantics-pre-868.md` §5.1–§5.3, which additionally omit the cap refusal. The last is frozen
history (issue #1065) and stays as it is; the other two are corrected.

**A second implementation may add codes this table does not list**, within the class its situation
demands. It may not reuse a binding code for a different situation.

## Update (issue #1143) — `R01` leaves the vocabulary, and `F01` loses a clause

> **Half of this update is wrong and is superseded by the corrected one below. `F01`'s half stands;
> `R01`'s does not.** Left in place because the trail is the point (see the [index](README.md)'s
> Conventions), not because any of it is still true.

[0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md) retires minimum delivery, and
issue #1143 deletes it. Two rows of the tables above change:

- **The `R01` row is deleted in full.** It fails this record's own test: with no floor to declare,
  _"lower the floor, or take a cheaper path"_ is no longer a move a sender has, and half of what
  remains is not a distinct action. No connector emits `R01`, and no second implementation should
  reuse the code for a different situation.
- **`F01` loses its "malformed minimum-delivery header" clause.** The other three situations —
  undecodable envelope, zero or absent condition, a termination reject that could not be sealed —
  stand, and `F01` stays class-only.

**`F03` gains the residual case the deletion left behind**, within the row it already has: a packet
that does not cover **this hop's own fee** is refused `F03` naming the fee and the amount. The
sender's move is the row's existing one — pay the stated amount — which is why this is that row and
not a new code. It sits beside the row's other two situations rather than replacing either.

One reject code fewer is one fewer sender behaviour to specify, which is the test this record
applies to every row.

## Update (issue #1143, corrected) — `R01` stays; its situation is RFC 0027's, not the floor's

The update above is **wrong about `R01` and is superseded by this one.** `F01`'s half of it stands
unchanged; so does every deletion issue #1143 made. Only the reject taxonomy moves.

[0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md)'s sweep claimed _"no sender action
remains behind `R01`"_ and this record took it. It does not hold. `R01` carried two situations and
the sweep saw one:

- the **minimum-delivery** one this record's original row named — _amount minus this hop's fee falls
  below the declared floor_ — which does die with the field; and
- **RFC 0027's own** — _"the amount received by a connector in the path was too little to forward
  (zero or less)"_ — which does not. `connector_domain::fee::amount_after_fee` still yields nothing
  when a hop's flat fee alone exceeds what arrived, and that packet still has to be answered.

This record's first line is that _"the reject codes are RFC 0027's"_. A field this protocol invented
cannot retire a code the base protocol defines. So **the `R01` row stays, with its situation and its
sender action rewritten** — replacing the row in the binding table above, which now reads:

| code  | situation                                                                                                                                             | what a sender does about it                      |
| ----- | ----------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------ |
| `R01` | this hop's own flat fee exceeds the amount, so nothing would be forwarded at all (RFC 0027, [0010](0010-flat-per-packet-fee-and-minimum-delivery.md)) | send a larger amount — the message names the fee |

**`F03` does not gain this case**, and the paragraph above that gave it one is withdrawn. `F03`'s
row is an amount wrong against a **price** — the route's, or a priced termination's — and its move is
to pay that stated figure. Here no price is in question and there is no figure to pay: the fee ate
everything. Folding the two together is this record's own prohibition — _"may not reuse a binding
code for a different situation"_ — and `F03`'s three-situation row goes back to two.

It passes the test twice over. The **situations** are distinct, above. And the **class letter is
itself a sender instruction**: RFC 0027 makes `F` final and `R` relative, where _"relative errors
indicate that the payment did not have enough of a margin in terms of money or time"_ and the sender
MAY retry with a larger one. `F03` says the packet is finally invalid; the truth here is "send more".
A sender that keys its retry policy off the class — which is the whole point of the class — takes the
opposite action on the wrong one.

Restoring it costs this record's other rule nothing: `R01` is still one situation, still one move,
and no second implementation may put anything else behind it.

## Update (issue #1532)

**The `F03` row now covers four situations, told apart by a binding reject reason.** The client edge
raises `F03` for an **underpayment**, an **undercollateralized** claim (#646), a forwarded packet
whose `amount` is above the forwarding charge, and a **probe** to a route this connector terminates.
The code alone left a sender parsing English to learn its move, so each reject this connector raises
for one of them carries a `reason` from a closed set, beside its code and cost (the `Toon-Reject-Reason`
header, the `toon-reject-reason` protocolData entry; `docs/protocol/client-edge-spec.md` §1.6):

| reason                  | sender's move                                        |
| ----------------------- | ---------------------------------------------------- |
| `underpayment`          | sign a voucher that pays the stated cost             |
| `undercollateralized`   | top the channel up, then resend **the same** voucher |
| `amount_over_charge`    | send `amount` equal to the stated cost               |
| `probe_terminates_here` | pay the stated cost; a probe is never delivered      |

Undercollateralized's move is **not** "pay the stated amount": its cost is `0`, the voucher already
covers the price, and nothing was consumed. A `T`-class code was considered for it and declined:
changing the code breaks every client that already reads `F03`, and the reason carries the
distinction without touching the code.

A reason is attached only to a reject this connector raised itself. A reject relayed back from a
peer reaches the client with no reason, and a connector never invents one; every other code carries
none. An absent reason means "not raised here", never "unknown", and a client that sees an absent or
unknown reason acts on the code and the cost alone. The reason is not a claim-ack (that is the peer
carriage's verdict on a claim) and never rides the REJECT's `data`.
