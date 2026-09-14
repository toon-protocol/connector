# The metrics surface is decided, not accreted, and logs correlate a packet by its condition

**Status:** Accepted, amended by [0033](0033-the-exposure-machinery-is-retired-not-restated.md) and [0069](0069-the-execution-condition-leaves-the-wire.md). Four of the five metrics stand. `toon_exposure` is kept at its decided name for scrape-config stability and is **permanently zero with no producer**, because the projection it was shaped for is retired. The log-correlation half is retired outright by 0069: the execution condition it correlated by no longer exists, and cross-hop correlation is not replaced.

**Scope:** connector architecture — internal to this codebase. See the [ADR index](README.md).

The Rust connector exposes exactly five decided metrics — `toon_packets_total`,
`toon_packets_rejected_total`, `toon_fees_earned_total`, `toon_exposure` and
`toon_settlement_total` — as `GET /metrics` on the operator surface, in Prometheus text
exposition format, gated by the same bearer token as every other read (ADR 0008). Every log
line emitted while a packet is being handled carries a `correlation_id` — originally the
packet's own execution condition, hex-encoded, requiring no new field and no wire change; see
[ADR 0069](0069-the-execution-condition-leaves-the-wire.md) for why that stopped being true and
what replaced it.

## Why

The PRD that started this rewrite (#409) said outright that observability "was not designed
in this session" and should be settled before the first slice was built rather than accreted
afterward. It wasn't — issue #429 is where the connector first has a binary worth scraping,
so it is also where this gets decided.

**Why these five names, and why now, even though three of them are always zero.**
`toon_packets_total{outcome}` and `toon_packets_rejected_total{code}` are populated today:
`Connector::handle_prepare` has exactly one choke point every return path passes through
(`Connector::finish`), so no outcome can be reported without also being counted.
`toon_fees_earned_total` is populated on fulfilment only, matching ADR 0010: a fee is earned
when a forwarded packet fulfills, not when it is merely attempted. `toon_exposure` and
`toon_settlement_total` were declared at their decided names and reported zero until the
claim/exposure projection (#423, #424) and channel lifecycle (#422) existed to populate them —
the same shape-first, populate-later precedent `connector_runtime::operator_view` already set
for `PeerView`/`ChannelView`/`ClaimView`/`ExposureView` in issue #420. A dashboard or alert
built against these names did not need to change when those tickets landed; it started
reporting non-zero.

> **`toon_exposure` never reached that populate-later step.** [ADR 0033](0033-the-exposure-machinery-is-retired-not-restated.md)
> (issue #882) retired the exposure projection this gauge was shaped for before it was ever wired
> up; `ExposureView` and the operator surface's `GET /exposure` are gone with it. The gauge itself
> is kept at its decided name for scrape-config stability, but it is now permanently zero with no
> producer, not "zero until" — the shape-first precedent held for `toon_settlement_total`, not for
> this one.

**Why metrics live behind the operator surface's bearer token rather than an unauthenticated
port.** ADR 0008 already lists metrics as exactly the kind of thing a read-only dashboard
needs, alongside peers and routes. Prometheus supports a bearer token per scrape target
natively, so gating `/metrics` the same way as every other read costs nothing operationally
and avoids introducing a second, differently-authenticated (or unauthenticated) HTTP surface
for what is, functionally, one more read. The consequence is direct: a node with no
`[operator]` section configured exposes no metrics at all, the same way it exposes no peer or
route inspection — configuring the operator surface is how an operator opts into any of it.

**Why the execution condition, not a new correlation-id field.** `Prepare.execution_condition`
is already invariant across every hop a packet passes through — forwarding only ever changes
`amount` (see `Connector::forward_to_peer`), everything else, including the condition, is
carried unchanged. That makes it a correlation id for free: two independent connectors, each
logging this same value for the same packet, produce structured logs that `jq
'select(.fields.correlation_id == "...")'` can join across the hop boundary with no addition
to the peer wire and no new packet field to keep in sync between implementations.

> **This is retired, not replaced.** [ADR 0069](0069-the-execution-condition-leaves-the-wire.md)
> (issue #1269) names the property this paragraph is proud of as the defect it actually was:
> invariant and distinctive per packet is exactly what makes a value a cross-hop join key, so
> "free correlation" was also free deanonymization for anyone forwarding traffic. The condition
> is gone from the wire entirely, and `correlation_id` is now a random id each hop mints for
> itself at packet entry and never transmits — logs still correlate perfectly within one node's
> own handling of one packet, and no longer join across a second one at all.

## Consequences

Logs are structured (one JSON object per line, via `tracing-subscriber`'s JSON formatter) so
they're greppable/joinable by tooling rather than parsed by regex. Every packet-handling log
line sits inside one `tracing` span (`"packet"`) carrying `correlation_id` and `destination`,
so nothing downstream of `Connector::handle_prepare` needs to thread either through by hand.

Verbosity is controlled by `RUST_LOG`, the standard `tracing-subscriber` environment filter —
this is an operational knob, not a behavioral one, so it does not conflict with ADR 0009's "no
environment-variable layer": nothing the connector _decides_ is read from the environment,
only how loudly it talks about what it already decided.

The metric and reject-code label cardinality is bounded by construction: `outcome` is
`fulfill`/`reject`, `code` is one of `connector_domain::RejectCode`'s fixed set of RFC-0027
codes, and there is no per-peer or per-destination label anywhere — nothing here can grow
unbounded as routes or peers are added.
