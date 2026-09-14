# A verified payment is stated to the app; an unverified one is stated by nobody

**Status:** Accepted. **Supersedes [0036](0036-a-paid-deliverys-attribution-stays-on-the-connector.md)'s conclusion** and narrows [0020](0020-a-price-is-flat-and-attaches-to-a-handler.md). Narrowed in turn by [0065-price](0065-a-price-is-a-schedule-over-payload-length.md): `X-TOON-Amount` is the charge for that packet, which for a flat route is the flat price it always was. Live: `connector-runtime/src/attribution.rs`.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

A terminating connector tells the app who paid for a delivery, how much, and on what chain —
`X-TOON-Payer`, `X-TOON-Amount`, `X-TOON-Chain` — **and only when it verified that payment
itself**. The source is the client channel whose covering claim it admitted at its own edge, so on
any path where that is not what happened the headers are absent rather than guessed, and a
caller's own spelling of those names is removed from every delivery whether or not one is then
stated. This **supersedes
[ADR 0036](0036-a-paid-deliverys-attribution-stays-on-the-connector.md)'s conclusion** ("no
successor, and there should not be one") while keeping the whole of its reasoning: what ADR 0036
and [ADR 0017](0017-the-typescript-connector-is-a-prototype.md) found wrong was the _source_ of
the TypeScript prototype's values, and this decision does not reuse either source.

## Context

### What the decision to say nothing actually costs

Verified live on the store box, 2026-08-16 — every upload the store records behind the Rust
connector:

```
[store] kind:5094 id=ca89a557... payer=- amount=- chain=- -> txId=_fHMe4zSfnFS6dRLi__DxgXJEK2F5ZOfVEuabPUg06Q
```

Those dashes are the store printing "absent" for the three headers it reads
(`store/src/store-backend.ts:118-120`). The app that did the paid work cannot attribute a single
byte of it. ADR 0036 answered that with a join an operator performs by hand, off two files on the
connector's box: a `"packet"` span carrying `client_channel_id`, and
`state_dir/client-edge-claims.log`. That join is real and it stays (it is what makes this decision
implementable at all), but it is not reachable from where the question is actually asked. The store
attributes a write to a payer inside its own request handler, at the moment the write happens; an
operator grepping two connector logs afterwards is not a substitute for that, and no amount of
connector-side record-keeping becomes one.

### Why "no successor" followed from ADR 0017, and why it no longer does

ADR 0017's finding is about provenance, and it is exactly right about the prototype:

- `X-TOON-Payer` carried `LocalDeliveryRequest.sourcePeer` — **the immediate previous hop**. On any
  path longer than one hop it names the wrong party. It looked correct only because the deployed
  path was one hop.
- `X-TOON-Chain` was the second label of the destination address — **chosen by whoever addressed
  the packet**, and presented to the app as though the connector had asserted it.

Neither objection is "an app must not be told". Both are "these particular values do not mean what
their names say". ADR 0036 drew the stronger conclusion because, at the time it was written, the
Rust connector had nothing better to say: it inferred the payer from nowhere at all, which is why
the live record came back empty.

It has something better now, and ADR 0036 is what built it. `client_channel_id` — the
chain-namespaced client channel key (`evm:0x<64 hex>`, `solana:<base58>`) a covering claim was
admitted under — is threaded from the client edge's own claim admission, where the claim's
signature was checked against the counterparty this node records for that channel
(client-edge-spec.md §1.3 step 4: _"nothing falls back to the claim's own self-declared signer"_).
It is the key the client-edge claim journal's `InboundClaimAccepted` entries are written under, and
the key the connector's `[[client_channels]]`/chain-resolved record states a counterparty for. It
is not the previous hop, and it is not sender-supplied. ADR 0036 called this the one honestly
assertable form of "payer" and then declined to hand it over; this decision hands it over.

### One hop is not an assumption here — it is the emit condition

The reason ADR 0017's failure cannot recur is not care, and not deployment shape. It is that the
value has one source and that source only exists on the path where it is true. A packet that
arrived across the peer wire, a packet forwarded onward, a packet nobody paid a claim for: none of
them admitted a client claim at this connector, so none of them carry `client_channel_id`, so none
of them can state a payer. There is no fallback to interrogate the previous hop with, because the
previous hop was never an input. "Absent" is what a longer path produces, by construction.

## Decision

**A terminating connector states three request headers on the delivery to a route's
`handler_url`**, and only when a covering client claim it verified itself admitted the packet and
the route's price is non-zero:

| Header          | Value                                                                                                                                                                                                                                                                                 |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `X-TOON-Payer`  | the admitted client channel key — `evm:0x<64 lower-case hex>`, `solana:<base58>`                                                                                                                                                                                                      |
| `X-TOON-Amount` | what this connector charged for **this packet** — the route's price schedule at the packet's own payload length ([ADR 0065](0065-a-price-is-a-schedule-over-payload-length.md)), which for a flat route is the flat price of ADR 0020 — decimal, in the settlement asset's base units |
| `X-TOON-Chain`  | that channel key's own namespace — `evm`, `solana`                                                                                                                                                                                                                                    |

`X-TOON-Payer` names a **channel**, not a wallet, and that is the honest unit: it is what the
connector verified, and it is the join key into both the claim journal and the channel record from
which the counterparty address follows. An app that wants the address resolves the channel; it is
never handed a guess at one.

`X-TOON-Amount` is the **price this connector charged**, never the amount field of the arriving
packet. A sender declares the latter; ADR 0020 decides the former, and it is exactly what the
covering claim had to advance by. Since
[ADR 0065](0065-a-price-is-a-schedule-over-payload-length.md) that figure is the route's
schedule evaluated at this packet's own payload length rather than a constant per route, which
changes nothing about what the header **means** — it was always "what was charged here" and
never "what the route lists" — and does mean an app can no longer infer it from its own route
table alone. Restating it to an app whose route table already knows it is
mild redundancy, deliberately accepted: the three travel as a set because they are read as a set.

`X-TOON-Chain` is derived from the **claim**, not from the destination address. ADR 0036 recorded
that this field "never had an honest source"; the namespace of a chain-verified channel key is one.

**Nothing is stated where nothing was verified.** No client claim admitted the packet (a peer-wire
arrival, a forwarded packet, an unclaimed request), or the route's price is zero: all three headers
are absent. Not empty, not `unknown` — absent, exactly as ADR 0036 made the span field absent
rather than recorded empty.

**A caller's own spelling of these names never survives, on any delivery.** The strip runs before
the injection and runs unconditionally — including on the deliveries that then state nothing —
because a route with nothing to state is precisely where a forged header would otherwise sail
through. An app reading `X-TOON-Payer` is reading this connector or reading nothing.

**ADR 0036's connector-side record stands unchanged.** The `"packet"` span keeps
`client_channel_id`, the claim journal keeps its entries, and the join between them remains the way
an operator answers the question after the fact. This decision adds a second reader of the same
fact — the app, at the moment of the work — rather than replacing the first.

Implemented in `crates/connector-runtime/src/attribution.rs` (the header names, the strip, and the
one place a value is chosen), applied by `Connector::deliver_opened_envelope`
(`crates/connector-runtime/src/connector.rs`) above the `AppClient` port, so the port itself stays
the thin adapter issue #521 made it. Asserted by `connector::tests::payment_attribution::*`, which
read the headers a `FakeAppClient` actually recorded receiving, and — against a real chain, a real
claim and a real signature check — by
`price_charging_real_chain.rs::a_claim_backed_by_real_on_chain_funding_is_charged_the_routes_price`.

## Considered options

**Leave ADR 0036 standing and close the gap operator-side only.** Rejected: this is the status quo,
and issue #994 is what it looks like from the app. The join ADR 0036 built answers the question in a
terminal on the connector's box; the app needs it in the request it is handling, and cannot get
there from a log line it has no access to.

**Send the payer's wallet address rather than the channel key.** Rejected: the connector would have
to resolve the channel's counterparty on the packet path (a registry read, potentially a chain
lookup) to state a fact the channel key already identifies. The key is what was verified; anything
derived from it can be derived by the reader too, off records that are not on the hot path.

**Send the packet's own `amount` field, as the prototype did.** Rejected for the same reason
`X-TOON-Chain` was: it is sender-declared, and presenting it as connector-asserted is the exact
category of error ADR 0017 documents. The price is the connector's own number.

**Emit the headers on every terminating delivery, filling "unknown" where no claim was admitted.**
Rejected: a sentinel is a value, and an app that special-cases one is one refactor away from
trusting it. Absence is unambiguous and matches how the same fact is already recorded on the span.

**Strip every `X-TOON-*` prefixed header rather than the three named ones.** Rejected: it claims a
namespace this decision does not own, and would silently eat a header some app and its clients
legitimately agreed on. The three names are stripped from one list that is also the injection list,
so a name that can be stated is always a name that is removed first.

## Consequences

`docs/protocol/client-edge-spec.md` §1.8, `docs/architecture/coding-standards.md` and ADR 0020's
"the app is told nothing" paragraph are reconciled to this decision in the same change; ADR 0036 is
marked superseded-in-conclusion, with its records half intact.

The store needs no change at all: it still reads these three names
(`store/src/store-backend.ts:118-120`), so `payer=-` becomes a real channel key on the next deploy
of the store box's connector. **The relay does need one.** ADR 0036 filed
[toon-protocol/relay#122](https://github.com/toon-protocol/relay/issues/122) to delete its reader
on the strength of "no successor header is coming", and that landed — `write-handler.ts` now
states it is told _"nothing about that payment"_. A successor did come; restoring the relay's
per-write attribution record against this contract is filed in that repo rather than written here —
[toon-protocol/relay#133](https://github.com/toon-protocol/relay/issues/133).
An app must
still treat the headers as optional — that is now their meaning, not an oversight — and must not
infer "unpaid" from their absence: what a handler receives was paid at that handler's one price
(ADR 0020), whether or not this connector was the hop that took the payment.

A second implementation of this protocol MUST NOT state these headers from any other source. In
particular it must not fall back to the previous hop's identity when no client claim was admitted:
that is ADR 0017's defect, and reproducing it puts a wrong payer in an app's own permanent records,
which is worse than the empty field this decision replaced.
