# The exposure machinery is retired, not restated

**Status:** Accepted. **Retires the exposure machinery**: `record_inbound_delivery`, `is_over_ceiling`, `PeerConfig::ceiling` and `flush_interval_ms`. Verified in the tree — `ceiling` and `flush_interval_ms` survive only as config keys parsed to be rejected by name, and `ExposureView` / `GET /exposure` are gone. Amends [0005](0005-claims-are-truth-balances-are-a-projection.md), [0014](0014-metrics-surface-and-packet-correlated-logs.md), [0027](0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md) and [0029](0029-a-peer-wire-arrival-to-a-priced-termination-must-cover-its-price.md). _Note, 2026-10-16:_ [0080](0080-a-client-may-pay-from-a-zk-note.md) (conditional, not built) lets the risk this record retired come back at a zk edge only, bounded by a cap and named **open budget**. The word exposure and this record's falsifier both stand.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

**Falsifier:** `crates/**/*.rs` matching `\b(record_inbound_delivery|is_over_ceiling|ExposureView)\b` — the three symbols this record retired and whose absence its Status line asserts it verified in the tree. Any of them back in code means the exposure projection was restated rather than retired.

ADR 0031 (issue #868/B1) retired the credit window as the peer path's operating mode — every peer
PREPARE now carries its own covering claim, or is greeted — but explicitly left one question open:
"the exposure machinery's remaining purpose is undecided here... `record_inbound_delivery`, `ceiling`
and `flush_interval_ms` either go away or become a residual safety bound with a restated purpose."
This is that decision (issue #882, child B5): **the machinery is removed, not restated.**

## Context

Before this issue, three things still ran on every accepted peer PREPARE even though B1--B3 (ADR
0031, issues #880, #881) had already made a covering claim mandatory on both ends of the peer path:

- `ClaimBook::record_inbound_delivery` journalled a `JournalEntry::InboundFulfillmentRecorded` entry
  on every peer fulfilment, folded by `Projection::exposure` into "value delivered but not yet
  covered by an accepted claim."
- `Connector::handle_peer_prepare` checked `ClaimBook::is_over_ceiling` ahead of every forward and
  rejected `T04_INSUFFICIENT_LIQUIDITY` once a channel's exposure exceeded its configured `ceiling`.
- `PeerConfig::ceiling`/`flush_interval_ms`, and the accept-only load-time requirement
  (`ConfigError::AcceptOnlyPeerWithoutCeiling`) that a peering this connector cannot dial must carry
  an explicit `ceiling`, since it cannot originate a FLUSH.

Two independent defects were found in this machinery while it sat undecided (issue #882's own
thread, verified against `568b9e4f`):

- **`PeerConfig::ceiling`'s doc comment claimed `None` means "the runtime's own default."** There
  was no such default: `connector-cli/src/runtime.rs`'s `wire_peer_channels` called
  `Connector::with_channel_ceiling` only when a peering configured a figure, so an unset ceiling made
  `ClaimBook::is_over_ceiling` answer `false` without ever reading the projection. `None` was
  unbounded in effect, not "defaulted."
- **The off-by-one.** `Projection::is_over_ceiling` was `exposure > ceiling`, and exposure is still
  `0` when a PREPARE's ceiling check runs (it is only incremented afterward, on fulfilment) — so a
  configured `ceiling = 0` still admitted one uncovered packet before `T04` ever fired.

Neither defect is what settles this ADR by itself: fixing them would have been straightforward, and
"the ceiling has bugs" is not the same claim as "the ceiling has no job." What settles it is that,
under B1--B3's rule, the ceiling's job — bounding trailing exposure between a fulfilment and the
claim that eventually covers it — describes a window that no longer opens in normal operation.
`ClaimBook::record_inbound_delivery` still ran on every fulfilment and `is_over_ceiling` still read
its accumulation, but the covering-claim requirement had already made the quantity they tracked
structurally small: a claim now arrives with (or ahead of) the packet it covers, not after it.

**The throughput cost of keeping it anyway is now measured**, not assumed (issue #879, PR #895,
`crates/connector-runtime/examples/peer_claim_journal_bench.rs`; kernel-counted `fdatasync` per
forwarded packet, 500 packets, reproduced across two runs, buzz-huddles rate 49fps):

| path                             | syncs/pkt | p50 @ 49fps | p99 @ 49fps |
| -------------------------------- | --------- | ----------- | ----------- |
| baseline (no claim, no exposure) | 1.00      | 1.80 ms     | 2.97 ms     |
| credit window (pre-#868)         | 2.00      | 3.18 ms     | 7.53 ms     |
| covering claim, exposure kept    | 3.00      | 4.81 ms     | 9.36 ms     |
| covering claim, exposure retired | 2.00      | 3.40 ms     | 7.74 ms     |

Keeping the exposure machinery alongside a covering-claim requirement costs a **third** `fdatasync`
per packet — roughly +1.8 ms at p99 at the huddles rate — because `record_inbound_delivery` is a
second durable journal write on top of the claim's own. Retiring it costs nothing measurable versus
what already ships: `covering claim, exposure retired` and `credit window` both sit at 2.00 syncs/pkt,
and the retired path's own p99 (7.74 ms) is within run-to-run noise of the credit window's (7.53 ms).
"Remove it" and "keep it as a residual bound" were not equally priced, and the price is what tips
this decision: a residual bound whose window structurally does not open in normal operation, kept at
a real and measured throughput cost, is exactly the kind of undocumented, unjustified machinery
issue #863 was filed about.

## Decision

**The exposure/ceiling/flush machinery is removed, not kept as a residual bound.** Concretely:

- `ClaimBook::record_inbound_delivery`, `ClaimBook::exposure`/`is_over_ceiling`,
  `ClaimBook::exposure_views`, `Projection::exposure`/`is_over_ceiling`, `Connector::exposure`,
  `Connector::with_channel_ceiling`, the `ExposureView` operator-surface type and its `GET /exposure`
  endpoint, and the `toon_exposure`-feeding code path are all deleted from the runtime.
- `PeerConfig::ceiling`/`flush_interval_ms` are removed as **live** config: `RawPeer` keeps both
  field names as parsed-and-rejected traps (`ConfigError::PeerCeilingRemoved`/
  `PeerFlushIntervalRemoved`), the same convention `addr`/`peer_wire_addr` established for ADR 0027 —
  a devnet box's bind-mounted TOML that still names either key gets a named, actionable error at
  boot, not a silent `deny_unknown_fields` drop or an unexplained refusal to start. `ceiling`'s
  accept-only load-time requirement (`AcceptOnlyPeerWithoutCeiling`) is removed along with the field
  it required: an accept-only peering's only real bound was that ceiling, and the covering-claim
  requirement already bounds every peering, accept-only or not.
- `Connector::handle_peer_prepare` drops its now-unused `channel_id: Option<String>` parameter —
  the ceiling check and `record_inbound_delivery` call were its only readers — which in turn retires
  the carriage-side bookkeeping that existed only to supply it: `connector-peer-btp`'s
  `AcceptedClaims::known_channel`/`note_channel`/`channel_for`, and `InProcessPeerTransport`'s
  `known_channel_id` tracking and `set_peer_channel`.
- `JournalEntry::InboundFulfillmentRecorded` **stays** in the domain journal alphabet, but is now a
  historical entry kind: nothing produces it any more, and `Projection::apply` folds it into nothing.
  This is a durability decision, not an oversight — a devnet box's on-disk journal from before this
  change may still contain these entries, and the journal format must keep decoding them rather than
  fail a replay or need a migration tool. The same reasoning keeps `RejectCode::t04_insufficient_liquidity`
  (`T04`, a standard ILPv4 code, RFC-0027) in `connector-domain` for wire interop, even though nothing
  in this codebase emits it any more.
- `toon_exposure`, the always-zero Prometheus gauge ADR 0014 declared, keeps its name for
  scrape-config stability but its help text now says plainly that the thing it named is gone and it
  will never have a producer — unlike `toon_settlement_total`, which is still a legitimate
  placeholder for pending work (issue #425).
- **Issue #424's projection-divergence report goes with the accounting it checked.**
  `ProjectionDivergence`, `Projection::divergences` and `Projection::known_channels` are deleted,
  and `ClaimBook::set_journal`/`Connector::with_journal` no longer return a divergence list for
  `connector-cli` to log. The one invariant that report checked was "an accepted claim's cumulative
  never exceeds what this connector's own journal recorded fulfilling on that channel" — computable
  only from `inbound_fulfilled`, which nothing writes any more, so keeping it would report every
  accepted claim as a divergence. #424's criterion is retired with its subject, not silently
  dropped; nothing about journal replay itself is weakened, and an unreplayable journal is still a
  named `RuntimeError::JournalUnreplayable` refusal to start.
- **`connector-peer-btp::warn_if_claim_ack_outlives_flush`/`warn_if_claim_ack_outlives` are deleted**
  with `flush_interval_ms`, and with them `peer-carriage-spec.md` §6.3's
  `claim_ack_timeout_ms <= flush_interval_ms` SHOULD — a coherence check between two figures of
  which only one still exists. See "Considered options" below for why the surviving figure does not
  inherit the warning.

## Considered options

**Keep the ceiling as a residual safety bound, fixing both defects.** This was ADR 0031's own
suggested "restated purpose" branch: `ceiling = 0` would mean zero, and the bound would cover
"in-flight exposure between a claim and its ack." Rejected on the measured cost above — a third
`fdatasync` per packet for a window that does not open in normal operation is not a bound worth
paying for, and B1--B3 already established that a claimless PREPARE is refused outright rather than
tracked and bounded. There is no longer an uncovered-but-tolerated state for a ceiling to describe.

**Keep `flush_interval_ms` alone, dropping only `ceiling`/exposure.** Rejected: `flush_interval_ms`
has no producer today either. `connector-peer-btp::warn_if_claim_ack_outlives_flush`, the one
function that read it, had zero callers anywhere in the codebase (confirmed by exhaustive grep) —
there was never a scheduled flush task reading this value, only a load-time coherence warning
nothing invoked. Keeping a config key whose only consumer was already dead code is the same
undocumented-machinery problem this ADR closes for `ceiling`.

**Delete `JournalEntry::InboundFulfillmentRecorded` outright**, since nothing produces it. Rejected:
`JournalEntry` is a durable on-disk format (ADR 0005), not an in-memory type. A devnet box that ran a
pre-#882 build may hold a journal file containing these entries; deleting the enum variant would make
`FileJournal::read_all` fail to decode that file rather than degrade gracefully. Keeping the variant
as an intentionally-ignored historical entry costs one match arm and is the same shape ADR 0027 chose
for `addr`/`peer_wire_addr` at the config layer.

## Consequences

**Migration.** An operator with `ceiling`/`flush_interval_ms` set in a config gets a named,
actionable error at load (`PeerCeilingRemoved`/`PeerFlushIntervalRemoved`), not a silent drop or a
generic `deny_unknown_fields` message — delete the key, no replacement is needed, since the
covering-claim requirement (ADR 0031) already provides what these approximated. This repo's own
devnet configs (`infra/linode-{relay,store,node}/connector-rust.toml`) are updated in the same
change that lands this ADR, so no live box is left holding a key the binary now refuses.

**The operator surface loses `GET /exposure`.** Nothing populated it with real data even before this
ADR reached the accept pipelines that gate on a covering claim (B2/B3 already ran ahead of
`handle_peer_prepare`'s ceiling check), so its removal is not a loss of an operationally-relied-on
number, and keeping it would mean an endpoint that always answers `[]`.

**`handle_peer_prepare`'s signature shrinks**, and with it the carriage-layer channel-tracking that
existed only to fill its now-removed parameter. This is a larger diff than the config/runtime
surfaces alone, but leaving an unused parameter (and the bookkeeping built to supply it) in place
would be exactly the inert-but-undocumented state issue #863, and this issue, exist to close.

**Nothing about claim exchange, watermarks, or on-chain redemption changes.** `ClaimBook`'s
`inbound_claimed`/`inbound_claim_nonce`/`inbound_claim_signature` state, `latest_inbound_claim`
(issue #425's redemption source), and the outbound ledger are untouched — this ADR removes only the
fulfilled-but-uncovered accounting the credit window needed and the covering-claim rule made moot,
never the claims themselves.
