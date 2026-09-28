# A peering is established from a URL, and its identity is trust-on-first-use

**Status:** Accepted — **built** (#1160), and **corrected** (#1217): the written peering could accept a claim but not sign one until #1217, because nothing populated the outbound CLIENT hop `Connector::cover_forward` reads — see "Update (issue #1217)" below. `POST /peers { id, url, fee, max_packet_amount }` reads the counterparty's self-description, derives the channel, opens it if absent, registers both the PEER-role and the CLIENT-role halves of the channel binding, and writes a durable runtime peering; `build_peer_transport` adds and removes a carriage while the process serves. Built on [0050](0050-a-connectors-url-resolves-to-its-self-description.md) (#1080), [0059](0059-a-channel-is-derived-from-its-participants.md) (#1158) and [0060](0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md) (#1157). Completes [0034](0034-a-runtime-peer-route-table-never-shadows-the-config-file.md), whose precedence rules governed a table that could not hold a peering. Satisfies both falsifiers of [0049](0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md). Leaves [0043](0043-purchasable-peering-is-removed.md) and [0006](0006-the-connector-is-mechanism-not-policy.md) intact; **narrows** [0022](0022-a-connector-answers-it-does-not-announce.md), and says how below. **Amended by [0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)** (accepted 2026-09-27): `POST /peers` opens and funds only this node's outbound channel; the inbound one is admitted, not derived. See the Update at the foot.

**Scope:** connector architecture — internal to this codebase. The document it reads is protocol law ([0050](0050-a-connectors-url-resolves-to-its-self-description.md)); the request that reads it is not. See the [ADR index](README.md).

The falsifier this record carried while it was unbuilt — no file at
`crates/connector-runtime/src/peer_route_store.rs` matching `endpoint` — is **satisfied and
removed**. `RuntimePeering::endpoint` is that field, and it is precisely the one the marker said no
implementation of this record could avoid: a durable peering has to persist somewhere to reach its
peer.

**An operator adds a peering to a running node by naming the peer's URL.** The connector `GET`s that
URL's self-description, derives the payment channel from the two participants, opens it on chain if
it is absent, and writes a durable runtime peering — with no restart and no edit to the config file.
**The peer's identity is trust-on-first-use over TLS:** whatever that URL serves is who the peering
is with, and this record does not pin it.

## What is wrong today: a peering cannot be added to a running node

[ADR 0034](0034-a-runtime-peer-route-table-never-shadows-the-config-file.md) shipped a durable
runtime peer/route table with careful rules — config wins by refusing the write, a route's `peer_id`
must resolve on every mutation, a peer in use cannot be removed. Those rules are right. They govern a
row that cannot hold a peering:

```rust
pub struct UpsertPeerRequest { id: String }   // connector-operator/src/lib.rs:371-374
```

That is the whole body. `PeerView` is a one-field struct, and `peer_route_store.rs:81-86` persists an
id and nothing else. Meanwhile the dial carriage is built once, at boot, exclusively from
`config.peers()` (`connector-cli/src/peer_transport.rs:74-88`). **A runtime peer has no endpoint, no
carriage and no channel binding.** It is a name a route or a lease may legally reference, and nothing
more.

The config path is no better, because it cannot be walked without stopping the process. A peering
needs four tables to agree (`config.rs:436-548`): `[[peers]]`, a `[[peer_channels]]` row or boot
fails `PeerChannelUnbound`, a `[[pay_channels]]` row for anything forwarded to, and `[[routes]]`. The
channel id in the second of those comes from a chain operation — `POST /channels`, an operator write
that only exists **after** boot. So onboarding today is:

1. Boot with no peering at all, because a declared peer with no channel refuses to start.
2. `POST /channels` to open the channel and read back its id.
3. Stop the node, hand-edit four TOML tables, restart.

A chain operation is sandwiched between two config states, and the node that comes back is a
different process. "Add a peer" is not an operation this connector has.

## The decision

**One operator write establishes a peering.**

```
POST /peers { id, url, fee, max_packet_amount }
```

- **`url`** — the peer's connector URL. The node `GET`s its self-description
  ([0050](0050-a-connectors-url-resolves-to-its-self-description.md)) and takes from it the endpoint,
  the carriage that endpoint's scheme implies, the edge identity, and the per-chain settlement
  addresses and chain facts.
- **`id`** — the operator's own name for the peering.
- **`fee`** and **`max_packet_amount`** — the operator's policy about this peer, which no document can
  supply.

The channel is then derived from the two participants and opened if absent
([0059](0059-a-channel-is-derived-from-its-participants.md)). The route is a second, separate write —
`POST /routes/peers { prefix, peer_id, price }` — because a peering and a route are different
decisions and one may exist without the other.

**The dial transport becomes rebuildable.** `build_peer_transport` running once at boot is what makes
the runtime row hollow; it must be able to add and remove a carriage while the process serves.

**Every load-time cross-table rule gains a runtime twin**, enforced continuously, exactly as
[0034](0034-a-runtime-peer-route-table-never-shadows-the-config-file.md) did for `UnknownPeerId`: a
peering with no channel binding is refused at write time rather than discovered at the first arriving
frame, and a peer forwarded to with no pay-channel likewise — checked, as of #1217, against the
CLIENT-role hop this write registers (the thing that lets this node SIGN a covering claim), not
against the PEER-role channel binding (the thing that lets it ACCEPT one), which is non-empty for
every peering this write ever produces and so cannot by itself distinguish a payable peering from an
accept-only one.

## What comes from the document, and what cannot

| Field                                                                                                             | Source                                                                                                     |
| ----------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| endpoint                                                                                                          | the self-description                                                                                       |
| carriage (BTP or HTTP)                                                                                            | the endpoint's scheme (`peer.rs:64-72`)                                                                    |
| edge identity — the key a payload is sealed to ([0018](0018-a-payload-is-sealed-to-the-terminating-connector.md)) | the self-description                                                                                       |
| per-chain settlement address, chain id, token network, registry, token, decimals                                  | the self-description                                                                                       |
| the channel                                                                                                       | **derived** from the two settlement addresses ([0059](0059-a-channel-is-derived-from-its-participants.md)) |
| `id`                                                                                                              | **the operator.** A local label                                                                            |
| `fee`, `max_packet_amount`                                                                                        | **the operator.** Policy about them, not facts about them                                                  |

**A node has three identities, and they are not interchangeable.** The edge identity is a secp256k1
key from `[signer]`; the EVM settlement address is 20 bytes (`peer_channel.rs:88`); the Solana one is
a base58 Ed25519 public key (`peer_channel.rs:52`). An Ed25519 key and a secp256k1 key cannot be the
same value. The channel derives from the **settlement address of the chain in question**, never from
the edge identity — `TokenNetwork.sol:330-335` recovers a balance proof's signer and requires it to
**be** a channel participant, which is what forces the claim key and the on-chain participant to be
one address.

**`fee` and `max_packet_amount` are the operator's, and that is the whole reason they are in the
request.** [ADR 0049](0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md)
already requires this — _"The operator surface must be able to express a cap, and today it cannot"_ —
and declares two falsifiers naming `connector-operator` and `peer_route_store.rs`. This record is what
satisfies them.

## Trust-on-first-use, stated plainly so nobody assumes otherwise

**Whatever the URL serves is who the peering is with.** The connector does not verify the fetched
identity against anything the operator supplied. A party who controls that hostname's DNS, or a
certificate for it, chooses the counterparty — and under
[0059](0059-a-channel-is-derived-from-its-participants.md) that choice determines the channel address,
so it is a party you would fund.

**Pinning was considered and rejected.** The alternative was a required `settlement_address` in the
request, with the write refused if the document disagreed. It was rejected because the protection is
conditional on something the mechanism cannot enforce: an operator who copies the address out of the
same document they are pointing the node at has pinned nothing, and that is the path of least
resistance. A pin that is usually theatre invites the belief that a peering's identity is
cryptographically bound when it is not.

**So the belief is refused directly instead.** No record, spec or runbook may describe a peering's
identity as pinned, verified or attested. It is trust-on-first-use over TLS, and the operator's
vetting of the URL is the whole of the assurance. Anyone who later wants a stronger property is
adding a mechanism, not documenting an existing one.

**What this does not weaken.** Every value-bearing check downstream is unchanged and remains
cryptographic: a claim's signature is verified against the counterparty key recorded for the channel
and never against anything the claim declares about itself
(`connector-runtime/src/claim.rs:439-443`), and a payload is sealed to the edge identity
([0018](0018-a-payload-is-sealed-to-the-terminating-connector.md)). A wrong document produces a
peering that does not work; it does not produce one that silently misroutes value to a third party
while appearing to work.

## Why the id is the operator's

An **ILP address is a claim, not a grant** (`CONTEXT.md`, **ILP address**): it is self-asserted, and
nothing in the world allocates one. Deriving the local identifier from the peer's advertised address
would therefore let a stranger
choose what this node's route table is keyed on and what its logs say (ADR 0014 labels every packet
line with the peer id). Deriving it from the URL host has a milder form of the same problem and
breaks when they move hosts.

The id is a label in this operator's namespace. The thing that must be globally unambiguous is the
channel, and [0059](0059-a-channel-is-derived-from-its-participants.md) makes that derivable and
unique without consulting a name at all.

The cost is real and accepted: two nodes peering with each other will use different ids for the same
relationship, so a support conversation needs a translation step.

## How this narrows ADR 0022, and how it does not

[ADR 0022](0022-a-connector-answers-it-does-not-announce.md) and `CONTEXT.md`'s **Controller** entry
say the connector _"never learns, announces, or discovers."_ A connector that fetches a URL and
populates its own peer table from the response is doing something that word covers, and pretending
otherwise would be the kind of quiet redefinition this folder exists to prevent.

**The line this record draws instead:** the connector learns nothing about **whether** to peer, or
**with whom**. The operator decided both, and named the URL. What the connector learns is **how to
reach a counterparty already chosen** — the same relationship it has to `handler_url`, which is also
an operator-supplied URL the connector dereferences without being said to discover anything.

**Announcing is still forbidden** and no mechanism for it is reintroduced
([0046](0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md)). **A peering still
cannot be bought, earned or announced into existence**
([0043](0043-purchasable-peering-is-removed.md)); it is created by an operator, and this record only
changes which surface the operator uses. **`POST /ilp` gains nothing**: the document is read with a
`GET`, and [0050](0050-a-connectors-url-resolves-to-its-self-description.md)'s prohibition on a
`POST` there is untouched.

What is genuinely new is that the connector now makes an **outbound** request to an
operator-supplied host from inside an authenticated write handler. That request must be bounded —
timeout, response size, redirect policy — and must never be made on the packet path.

## Rejected: a CLI verb that fetches and confirms

`connector peer add <url>` — fetching the document on the operator's workstation, printing it,
requiring confirmation, then issuing the signed write with explicit fields — was considered. It has a
real precedent (`connector send` already forms a packet and makes an RFC 9421-signed `POST /packets`
from outside the serving process) and it keeps the outbound fetch out of the connector entirely.

It was rejected as the **primary** mechanism because it makes the capability depend on having the
binary installed wherever the operator is, and the API would then need the explicit-field form
anyway — two surfaces for one operation. Nothing here forbids adding the verb later as a convenience
over the same endpoint.

## The sweep

**Does not survive:**

- **[0034](0034-a-runtime-peer-route-table-never-shadows-the-config-file.md)**'s implicit premise
  that a runtime row is an id. Its **decision is untouched and is strengthened** — config wins by
  refusing the write, the `UnknownPeerId` twin is enforced continuously, a peer in use cannot be
  removed, and the whole-table JSON snapshot is still how durability works. What changes is that the
  table now holds the thing those rules are about.
- **[0009](0009-one-typed-config-file-no-environment-layer.md)**'s reach, narrowly. Config remains
  one typed file, validated once, immutable for the process lifetime, with no environment layer —
  **none of that moves.** What moves is that the _peer transport_ is no longer derived once from that
  file and never again. A config-file peering is still immutable; a runtime peering is not a config
  peering, and 0034 already refuses any collision between the two.

**Survives unchanged:**

- **[0043](0043-purchasable-peering-is-removed.md)** — an operator creates a peering and nothing
  else does. A URL is not a purchase.
- **[0006](0006-the-connector-is-mechanism-not-policy.md)** — `fee` and `max_packet_amount` are in
  the request precisely because they are policy the operator holds.
- **[0046](0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md)** — nothing here
  needs a relay, a Nostr event or a directory. Two operators and one URL are sufficient, which is
  the case 0046 said had to work.
- **[0018](0018-a-payload-is-sealed-to-the-terminating-connector.md)** and
  **[0024](0024-peer-wire-claims-sign-the-eip-712-balance-proof.md)** — what the edge identity and the
  counterparty key are _for_ is unchanged; only how they arrive is new.
- **[0008](0008-operator-surface-splits-read-from-write.md)** — `POST /peers` is a write and is
  RFC 9421-signed like every other. No bearer token reaches it.

## Consequences

**Onboarding becomes three calls, and two of them already exist.** `POST /peers`, then
`POST /routes/peers`, with `POST /channels` still available for an operator who wants to open a
channel explicitly rather than let the peering derive it.

**The endpoint can spend gas.** Deriving-and-opening a channel means `POST /peers` may submit a
transaction and wait for confirmations, so it can fail _after_ money has moved. The durable table row
must be written from a **confirmed** channel, and the endpoint must be safely retryable: a repeat of
the same request against a peering already established is a success, not a second channel. Under
[0059](0059-a-channel-is-derived-from-its-participants.md) the derivation makes that idempotence
structural rather than a matter of care.

**The response must say which branch it took** — whether the channel was found or opened — so an
unintended second channel is visible in the operator's own output rather than discovered later on a
block explorer.

**A peering added at runtime and one written in the config file become the same object.** They differ
only in where they are recorded and which wins a collision, which is what 0034 already decided.

**`CONTEXT.md`'s Peering entry changes.** _"a counterparty key, a carriage to reach it on, a fee, and
a cap"_ stays accurate in substance, but "created by an operator — in the config file or through the
operator surface" stops being aspirational for the second half.

## Update (issue #1160) — what the build settled that the decision left open

Three questions the record did not answer had to be answered to build it, and each is answered the
way this folder's existing rules already pointed.

**Which chain, when two nodes settle on more than one.** The write takes an optional `chain`, and
refuses by name when several are shared and none was named — the same posture `POST /channels`
already takes for the identical ambiguity (issue #630). Picking one silently would be picking which
asset a peering settles in, which is the operator's decision and not this connector's
([0006](0006-the-connector-is-mechanism-not-policy.md)).

**Which endpoint, when a node publishes both.** BTP where both are published. A dialed BTP session
is symmetric once established, so either side may originate on it (`peer-carriage-spec.md` §2.3),
where an ILP-over-HTTP peering can only ever be originated on by the dialer (§6.4). Preferring the
carriage that leaves both directions open forecloses least, and an operator who wants the other
writes the peering in the config file.

**How long the opened channel's settlement window is.** A fixed day, and not a field on this
request. An operator who wants a different one opens the channel with `POST /channels` first, and
this write then **finds** it — which is the derive-or-open branch working as designed rather than a
special case, and is why that endpoint stays available.

**The bounds on the outbound fetch, stated as numbers.** Ten seconds for the whole exchange, a 64
KiB body cap enforced as the body streams rather than after it is buffered, and **no redirect
followed at all** — a `3xx` is refused by name, because following one would let the named host hand
the peering to a different host, and under [0059](0059-a-channel-is-derived-from-its-participants.md)
that choice determines the channel address.

**Trust-on-first-use is unchanged and unstrengthened.** No `settlement_address` pin, no fingerprint,
no confirmation step was added, and no doc comment, log line or error message describes a peering's
identity as pinned, verified or attested. The one thing the build does check is the _shape_ of a
published settlement address — 20 bytes on EVM, 32 base58 bytes on Solana — and that is not a check
against anything the operator supplied. It is the refusal to coerce bytes into an address that names
a participant no chain holds.

## Update (issue #1217) — the runtime twin of the pay-channel rule was never wired

`connector_config::pay_channel`'s own header calls a channel binding "the deployed shape": one
channel, bound in **both** roles at once — the PEER role for what arrives, the CLIENT role for what
this node sends. #1160's build bound only the first half. `establish_peering` called
`bind_runtime_peer_channel` (peer role) and wrote a durable row whose `channels` field was therefore
always non-empty, and `upsert_runtime_peer_route`'s guard tested exactly that field — so it accepted
every peering `POST /peers` ever produced, never catching the one it exists to catch. The result: a
peering answered `200` to `POST /peers`, `200` to `POST /routes/peers`, and `T00` to every packet
originated over it, naming a `[[pay_channels]]` row the operator had been told they no longer needed
to write (ADR 0042 item 2).

**The fix adds the missing half**, not a new mechanism. `establish_peering` now also calls
`register_outbound_client_hop` — the CLIENT-role counterpart of `bind_runtime_peer_channel`, and the
runtime twin of what `wire_outbound_client_hops` does for a `[[pay_channels]]` row at boot — using the
same `document.httpEndpoint` the durable row already carried as `client_edge_url` (`POST
/ilp/claim-state` is always asked over plain HTTP, whichever carriage the packet itself rides). A
document with no `httpEndpoint` is now refused at write time
(`EstablishPeeringError::NoDialableClientEdge`) rather than silently producing an accept-only peering:
such a peering could never be paid, so the write is not worth a channel being opened for. Boot
rehydration (`with_runtime_peer_route_store`) re-arms both roles the same way establishing did, so a
restart does not turn a payable peering back into an accept-only one. And the outbound client ledger
(`OutboundClientLedger`) now opens for every node with a `state_dir`, not only one with
`[[pay_channels]]` rows — a runtime peering can need one that no config table predicted.

**`upsert_runtime_peer_route`'s guard now tests the CLIENT-role hop** (`outbound_client_hops`), not
`RuntimePeering::channels` — the fix for the gap above, and the reason the guard could never have
fired before it. `PeerHasNoPayChannel` still exists and still means the same thing operationally
("this peering cannot be paid"); what changed is which fact it is computed from.

None of this changes what the write asks an operator for, what it returns, or the idempotence and
gas-spending properties above — a repeat of the same `POST /peers` still finds the same channel, and
both roles are re-bound on every call the same way the channel-derivation logic already was.

## Update (ADR 0075, issue #1371) — accepted 2026-09-27, implementation pending

[0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision 4 amends this
write. `POST /peers` still reads the self-description, still checks a shared chain and token, and
still writes a durable runtime peering, but it **opens and funds only this node's outbound x402
channel**. "Is there a live channel with this peer?" becomes a lookup of this node's own outbound
channels, not a derivation ([0059](0059-a-channel-is-derived-from-its-participants.md) is retired),
so idempotence rests on the outbound channel's journal entry being written before its opening
transaction rather than on derivation. The inbound half is admitted when the peer's first voucher
(EVM) or sponsored `open` (Solana) arrives, and bound by the voucher signer the peer's document
publishes. Trust-on-first-use is unchanged.

**Built on EVM by #1378.** `POST /peers` takes a `deposit`, opens and funds this node's outbound
`x402BatchSettlement` channel (or finds it on a repeat), binds the peer's `voucherSigners` key, and
writes an `evm-voucher` row; no `TokenNetwork` channel is derived or opened, and a runtime EVM row
naming one is refused at boot by name. Solana peerings still follow this record's derivation until
#1379.
