# The node keeps a bounded packet history, for watching and for nothing else

**Status:** Accepted — **built** (#1477). Amends [0066](0066-the-operator-dashboard-is-a-page-the-surface-serves-and-signs-in-the-browser.md): its "the node keeps no time series" stays true of rates and totals, which remain the tab's and Prometheus's, and a bounded history of recent packets is now the one thing a node does keep. Amends [0014](0014-metrics-surface-and-packet-correlated-logs.md): the five decided metrics stand and gain no label; this adds a read beside them, not a metric. Consistent with [0069](0069-the-execution-condition-leaves-the-wire.md) and [0015](0015-read-mostly-state-is-a-swapped-snapshot.md) as argued below.

**Scope:** connector architecture — internal to this codebase. See the [ADR index](README.md).

**Falsifier:** `crates/connector-runtime/src/packet_history.rs` matching `std::fs|tokio::fs|File::|tracing::` — the history has grown a way to write a row to disk or to the log, which is the thing this record refuses. (The other half — any read of the history from the packet path — is a call to `packet_history(` or `history.view(` from `handle_prepare_traced`, `forward_via_peer_route` or anything they call, and is checked in review, not by a grep.)

**A connector keeps the most recent packets it handled, in memory, for its operator to watch.**
`[operator] packet_history = n` sets how many (absent or `0` keeps none, so the key is not a
breaking deploy), and `GET /packets` on the operator surface, behind the bearer token like every
other read, answers them newest first with `enabled`, `capacity` and `dropped`. A row is exactly
a packet `toon_packets_total` counts: it is made where every outcome already passes,
`Connector::finish`. A refusal made above the `Connector` (unpaid at the client edge, an
under-covered peer claim, a malformed body) is not a row, and a probe is not a row.

**The history is never a record and never an input to a decision.** It is bounded, lossy and gone
on restart. Nothing that routes, prices, forwards or rejects a packet reads it. Nothing is
written to disk or to the log for a row, and no log level changes. Recording does not slow a
packet (0015): the packet path hands a row to a collector task with `try_send` and goes on, and a
row that cannot be handed over at once is dropped and counted in `dropped`. A row ageing out of
the ring is not a drop.

**Units are neither converted nor named.** `amount` is in the arriving leg's unit (for `sent`, the
outgoing peering's) and `fee` is the figure `toon_fees_earned_total` adds, in the outgoing
peering's. On a dealing node the two differ; a reader resolves each against the peering or channel.

## Considered options

- **0066's "no time series" read as forbidding this.** 0066 meant rates and totals: a node that
  recorded a series of its own figures would be doing Prometheus's job badly. A list of recent
  packets is not a series of a figure, and a tab cannot see a packet to sample it, so the tab's own
  sampling was refused as an alternative. 0066 stands for everything it was about.
- **A log line per packet.** Refused: #690 moved them to `debug` because per-packet lines at
  huddle rates are per-event disk I/O, and a row for the operator is not worth reversing that.
- **A per-peer or per-destination metric label.** Refused by 0014's cardinality rule: a destination
  is unbounded, and a peer label would still say nothing about one packet.
- **A durable, complete record.** Refused: it would be accounting, which a lossy ring cannot be and
  which no decision here wants.

## On 0069

0069 removed a value that was invariant on the wire and visible to every hop, because it let
colluding hops link a payment. A history puts nothing on the wire, holds one hop's view of its own
packets, sits behind the bearer token and is off unless configured, so its rows are not coarsened:
they carry the destination whole and the exact amount, as the operator's own node already knew them.

## Consequences

`correlation_id` is not in a row, and neither is the amount that left on a forward: a row is one
hop's account of one packet, not a trace.
