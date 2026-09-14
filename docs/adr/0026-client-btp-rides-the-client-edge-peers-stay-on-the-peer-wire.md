# Client BTP rides the client edge; peers stay on the peer wire

**Status:** Partly superseded by [0027](0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md). Its **peer conclusion is dead** — "peers never use it: connector↔connector traffic stays on the raw-TCP peer wire". Its **architecture is Accepted** and is the precedent 0027 builds the peer side on: one gate, two carriages, factored so they cannot drift. Its "every BTP session is a client session by construction" argument is replaced by role-by-authentication. **Corrected by issue #1073:** this record points at vectors for the client BTP dialect. They are **in-crate Rust unit tests** (`connector-btp/src/frame.rs`), which `toon-client` and every other non-Rust consumer cannot replay — so the client BTP dialect is **not covered by the cross-repo contract** ([0021](0021-vectors-are-normative-prose-is-not.md)) in any usable form. A false claim of coverage is worse than an acknowledged gap, because it stops anyone looking. First on the coverage burn-down.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

> **Conclusion partly superseded, architecture reaffirmed (2026-08-03) by
> [ADR 0027](0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md).** Peers
> now use both carriages too, so the title's second clause and the "every BTP session is a client
> session by construction" argument below no longer hold: role is decided by authentication (a
> configured `[[peers]]` credential **and** a `[[peer_channels]]` binding), not by transport or
> carriage. Everything else stands — and this ADR's central architecture, **one gate and two
> carriages that are factored so they cannot drift**, is the explicit precedent ADR 0027 builds the
> peer side on, including the reason for it: when the carriages did drift, it caused a devnet
> incident. Note for anyone chasing a reference: an unmerged draft on branch `adr/btp-peer-transport`
> also carried the number 0026; it was renumbered to 0027. This file keeps 0026.

`crates/connector-client-edge` now serves a BTP websocket transport at `GET /ilp/btp`
(client-edge-spec.md §1.9), mounted on the same router — and therefore the same bind address, the
same `ClientClaimGate` instance, the same watermarks and journal — as `POST /ilp`. It is a second
carriage for the client edge's existing pipeline, not a second pipeline. Peers never use it:
connector↔connector traffic stays on the raw-TCP peer wire (`peer_wire.rs`,
`docs/protocol/peer-semantics-pre-868.md`), unchanged.

## Why this is a client-edge route, not a peer transport

The retired TypeScript connector (ADR 0017) had one BTP server for both audiences, and that
ambiguity is precisely what `docs/btp-client-ingress-findings.md` measured on devnet: an anonymous
client session was admitted as a quasi-peer (`btp_auth … success:true mode:"no-auth"`), its
prepares were routed with `prepare.data` untouched, and the termination — which expected the
modern sealed envelope it had no key for — answered
`F01 Invalid HTTP envelope: malformed request-line: "<ciphertext>"`. The client's claim, riding as
protocolData, had no header to land in and fell on the floor as
`F06 No payment channel claim attached`.

Putting the websocket endpoint inside `connector-client-edge` dissolves the audience question
instead of re-answering it per-session: every BTP session is a client session by construction,
because peers speak a different protocol on a different listener. No client middleware can leak
onto peer traffic; no peer trust can leak onto client sessions. And because the route is mounted
in `router_with_gate_and_terms` beside `handle_ilp`, sharing `ClientEdgeState`, the claim
pipeline is shared by construction too — one gate, two carriages, with `claim_rejection_reject`
and the x402 terms builder factored so neither carriage can drift from the other's refusal
taxonomy (the F06 greeting REJECT is the one BTP-only code: BTP cannot answer HTTP 402, so the
same terms JSON rides as `payment-required` protocolData on a REJECT instead).

## Why the session is processed strictly in order

The transport exists because parallel HTTP writes race their own claims: two requests carrying
nonces n and n+1 can arrive at the gate's watermark lock in either order, and the loser is refused
`NonceNotAdvancing` even though the client did nothing wrong. A websocket delivers frames in
order; the session loop preserves that order all the way through the gate by not reading frame
k+1 until frame k's claim has been judged and its packet routed. In-order traffic on one session
therefore cannot self-race by design, which is the property the huddle-over-ilp measurement
(toon-meta `proto/huddle-over-ilp`, Phase D) needs. Pipelining routing while keeping only the
gate serial is a compatible future optimization; ordering is the contract, full serialization is
today's implementation.

## Why the frame grammar is the deployed client's dialect, not RFC-23

The only BTP speakers this edge must interoperate with are `@toon-protocol/client`'s
`IsomorphicBtpClient` and its consumers — the deployed wire. That dialect (fixed-width
protocolData list, ILP packet as a trailing length-prefixed field beside it, MESSAGE/RESPONSE/
ERROR only, no TRANSFER) is simpler than RFC-23 and is what §1.9 specifies normatively. The unit
vectors pin it byte-for-byte against the TS serializer (they lived in
`crates/connector-client-edge/src/btp.rs` when this ADR was written; issue #713 moved them, and
the codec they pin, to `crates/connector-btp` so ADR 0027's peer carriage cannot fork them). Implementing RFC-23's full grammar would add frames no deployed client sends, to a
transport ADR 0003 already versions behind the client edge's own discipline.

## Update (issue #697): the grammar is now additively symmetric

toon-meta#262 (agents earning) needs the connector to pay a client, not just be paid by one —
which needs the other half of RFC-23: a server-originated MESSAGE and TRANSFER (type 7), the
frame RFC-23 specifies for carrying settlement value. "No deployed client sends it" stopped being
a reason not to add TRANSFER once a _future_ client (the paired `toon-client` ticket) needs to.
The paragraph above is now historical: as of #697, `btp.rs` decodes/encodes TRANSFER,
acknowledges an inbound one, and can originate a MESSAGE or TRANSFER of its own with a
session-scoped outbound requestId allocator satisfying RFC-23's uniqueness property. This is
additive, not a reopening of the decision above — the deployed client still speaks exactly the
dialect this ADR describes and observes no change, because it never sends TRANSFER and nothing
yet originates a request to it (that caller is the session registry `toon-meta#262`'s
payout-ledger ticket adds next). See `docs/protocol/client-edge-spec.md` §1.9 for the current,
normative frame grammar.

## Update (issue #699): the payout ledger exists, still no production caller

`crates/connector-client-edge/src/outbound_ledger.rs`'s `ClientPayoutLedger` is now the client
edge's mirror of `connector_runtime::ClaimBook`'s outbound (peer-paying) direction: it signs a
fresh cumulative claim per client channel against that channel's own recorded EIP-712 domain
(ADR 0024), degrading to no claim at all absent a signer or a domain, exactly as `ClaimBook`
already does. `btp.rs`'s `payout_claim_protocol_data` carries that claim as a TRANSFER's
protocolData, JSON like every other entry this dialect carries rather than `WireClaim::encode`'s
peer-wire binary shape. Both are proven end-to-end (`btp.rs`'s own test signs a claim, sends it
over a real `BtpSessionHandle::send_transfer`, and verifies the signature on the other end) but
still have no production caller: deciding when a job completes and which channel to credit is
`toon-meta#262`'s session registry (connector#698), held until this ticket merged so the two do
not collide in the same client-edge state. This ticket creates credit only — netting (whether
that credit raises spendable headroom) is connector#700.

## Update (issue #700): credit raises spendable headroom, the netting ticket

`ClientClaimGate::with_payout_ledger` (`crates/connector-client-edge/src/claim_gate.rs`) binds a
`ClientPayoutLedger` to the same channel ids the gate's own `ClientChannelRegistry` accepts an
inbound claim on. The collateral-binding check (client-edge-spec.md §1.3 step 5) and the
claim-state endpoint (§1.10) both net `ClientPayoutLedger::credited` against the on-chain deposit:
a channel's spendable headroom is `deposit - owed + credited`, not `deposit` alone — decision 9 of
`toon-meta#262`, "an inbound claim raises spendable headroom directly." `credited` is read once,
before the collateral check's own chain-refresh await, so a payout recorded mid-admission cannot
retroactively rescue a claim already past that snapshot (only ever a false refusal, never a false
accept — the same direction every other staleness bound in this gate already errs in). Still no
production caller decides _when_ to credit a channel; connector#698's session registry remains the
next ticket that closes that gap. Solana channels net nothing yet: `ClientPayoutLedger` wraps
`ClaimBook`, which only ever signs an EVM balance proof.

## Update (issue #698): the session registry exists — "the socket is the lease"

`crates/connector-client-edge/src/session_registry.rs`'s `SessionRegistry` answers the question
this ADR's own §"Update (issue #697)" deferred: which socket, right now, is the live session for a
client-edge address. One instance lives on `ClientEdgeState`, shared by every session
`btp::btp_session` serves — bound at auth (keyed by the declared `peerId`), cleared when that same
session's read loop ends. Deliberately the only record of reachability: no separate route entry
with its own TTL, because a route record and a socket can disagree, and during the disagreement
this connector would route paid work into a hole (`toon-meta#262` decision 12).

Each bind is assigned the next generation from one monotonic counter shared by the whole registry;
the highest generation for an address always wins, and a rebind's own later cleanup can never
evict a binding at a generation newer than the one it names — buzz's own fencing law
(`buzz-relay-mesh/src/wire.rs`: "the mesh may say 'don't dial' — it may never say 'take over'"),
applied here to a socket instead of a mesh peer. `SessionRegistry::deliver` originates a MESSAGE
through whichever session is current, fenced against a caller's stale remembered generation, and
answers every failure path — no live session, or one that died or timed out mid-delivery — with a
`T01` (Peer Unreachable) reject rather than `R00`, since the packet itself is fine and the sender
should simply retry. A 120s backstop TTL (`SESSION_LEASE_BACKSTOP_TTL`) covers only a socket that
looks alive at the TCP layer but has stopped producing frames; the primary liveness signal remains
the socket's own read loop ending. `buzz#84`'s relay-side provider-freshness window must never
exceed this constant's value.

`bind`/`touch`/`unbind` are live in production — every real BTP session runs through them today.
`deliver` still has no production caller: deciding when a packet's fulfillment should trigger a
payout or a job dispatch, and to which address, remains the next ticket's job, same posture
#697/#699 already shipped their own foundations under. Claim watermarks are unaffected by any of
this — they stay exactly where issue #699's update already confirmed they survive reconnect: per
channel in `ClientClaimGate`, never per session.

## Update (issue #722): the backstop TTL is now on the wire, not just in a Rust const

`buzz#84`'s relay-side provider-freshness window "must never exceed" `SESSION_LEASE_BACKSTOP_TTL`
was, until this update, an instruction no TypeScript consumer could actually follow — the constant
is a Rust `pub const` with no import path from another language. The §1.4 x402 greeting (reused by
both the HTTP and BTP carriages via `x402_terms_body`) now carries it as `extra.sessionLeaseTtlMs`,
always present, derived directly from `SESSION_LEASE_BACKSTOP_TTL` at the point the greeting is
built rather than typed a second time (`client-edge-spec.md` §1.4, §1.9). A same-crate test pins
the two together. Wiring `buzz#84`'s `providerAvailability.ts` to read this field is left as a
follow-up in the `buzz` repository.

## Update (issue #770): a client session's own fulfilment now has a production caller

`SessionRegistry::deliver` and `ClientPayoutLedger`/`payout_claim_protocol_data` (issues #698,
#699, #700) were each proven end to end by their own tests and never wired together: nothing
decided _when_ a fulfilled PREPARE should credit a channel, so `credited` — and therefore
`available` — stayed structurally `0` in every deployed process, the same "green tests, no
production caller" shape issue #736 already named once for the read side of session routing.

`crate::session_route::route_prepare` is where that decision now lives, alongside the routing
decision it already owns: once a client session's own answer is a genuine FULFILL — rides home
unchecked since [ADR 0069](0069-the-execution-condition-leaves-the-wire.md), previously checked
against `prepare`'s execution condition — it calls `ClientPayoutLedger::record_payout_once` — a new
method, not `record_payout` itself, because `record_payout`/`ClaimBook::record_fulfillment` advance
their nonce and cumulative amount unconditionally on every call and so cannot by themselves tell a
genuine second job from a retried first one (issue #770's AC3). `record_payout_once` dedupes on
`(channel_id, execution_condition)` at the time this was written; ADR 0069 keys it on `(channel_id,
fulfillment)` instead, taken from the FULFILL already in hand — the same identity, since RFC-0022's
relation was `condition = sha256(fulfillment)`: a packet's fulfilment is deterministic per job, so a
retransmitted fulfilment of the same job always carries the identical value, and a genuinely new job
never does.

The channel to credit needs no new lookup: `destination` — the address `SessionRegistry::resolve`
found this session bound at — _is_ the channel id, per this ADR's `outbound_ledger.rs` update
above ("a client-edge channel has no separate peer identity — the channel _is_ the identity").
Once credited, `SessionRegistry::deliver_transfer` (new, `deliver`'s TRANSFER-originating sibling)
carries the freshly signed claim to the same session as a payout TRANSFER, fenced against the same
generation the original delivery used. Both steps are best-effort past this point and never hold
up the packet's own already-decided answer: a session that has died by the time the TRANSFER would
go out loses only that delivery attempt, not the credit — `pending_claim` carries it to whatever
next reaches this channel.

`connector-cli/src/runtime.rs`'s `client_claim_gate` now also builds a `ClientPayoutLedger` from
every EVM `[[client_channels]]` entry (`client_payout_ledger`), signed by the same key
`with_identity_signer` already uses for this connector's identity and its peer-wire outbound
claims, and binds it with `with_payout_ledger` before the gate is returned — the configuration step
every update above noted was still missing. A node with only chain-resolved (undeclared) client
channels credited nothing at this point: signing a payout claim needs a domain, and only a declared
channel's config carried one. The next update closes that gap.

## Update (issue #780): a self-opened channel with no config row is now credited too

The previous update's last sentence was the bug: toon-meta#261's whole model is an agent deriving
its own key and opening or resuming its own channel at runtime (ADR 0005/0006), so its channel id
does not exist when the connector's config is written, and `client_payout_ledger`'s
`[[client_channels]]` walk could never see it. Every self-opened agent channel was credited `0`
in production regardless of how many jobs it fulfilled — confirmed live by re-running rig#59's
proof after issue #773 shipped: `available` stayed `100000 -> 100000`, byte-identical to the
pre-#773 run.

The inbound claim path had already solved this shape: an unknown channel is resolved on chain
through `ClientChannelRegistry::evm`, budgeted per issue #613 against `ClientClaim::signer_key` so
a flood of fabricated ids cannot force unbounded lookups. The fix reuses that same resolver on the
outbound side rather than inventing a second one.

`ClientClaimGate::credit_payout` (new, `crates/connector-client-edge/src/claim_gate.rs`) is now
`credit_session_earnings`'s call site instead of `ClientPayoutLedger::record_payout_once` directly:
if the ledger has no domain yet for the channel being paid (`ClientPayoutLedger::has_channel_domain`,
new), it asks the gate's own `ClientChannelRegistry::evm` — the same instance and the same budget
the inbound path already spends against — and, on a genuine on-chain answer, registers the domain
through `ClientPayoutLedger::ensure_channel_domain` (new, `outbound_ledger.rs`) before falling
through to `record_payout_once` exactly as before. A channel that fails to resolve (never opened,
or the chain endpoint is down) is left unresolved and `record_payout_once` then produces nothing,
same as it always has — resolution failure never becomes a free credit (issue #780's AC3).
`record_payout_once`'s own dedupe (issue #770's AC3 — `(channel_id, fulfillment)` since ADR 0069,
`(channel_id, execution_condition)` before it) is unaffected: `ensure_channel_domain` only ever
adds a domain, it never touches credited state.

`ensure_channel_domain` takes `&self` where `set_channel_domain` takes `&mut self`, because a
channel discovered at payout time arrives after this ledger is already shared as an `Arc` across
every in-flight session — `outbound_ledger.rs`'s `book` field is now an `RwLock<ClaimBook>` for
exactly this reason, held for a write only on the rare first-resolution path; every other method
here still only ever takes a read lock, since `ClaimBook`'s own mutating operations are already
safe under `&ClaimBook`. `[[client_channels]]` rows still work unchanged as a pre-seed — resolution
is only ever consulted for a channel the ledger does not already know.
