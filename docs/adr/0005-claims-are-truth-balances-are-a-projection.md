# Claims are the source of truth; balances are a projection

**Status:** Accepted, amended by [0033](0033-the-exposure-machinery-is-retired-not-restated.md). Claims-as-truth and the replayed journal stand. The exposure and ceiling arithmetic named under "Consequences" is retired — nothing projects exposure any more. The crate it names, `connector-core`, shipped as `connector-domain`. **Amended by [0074](0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md)** in one clause: the journal holds a batch-settlement voucher exactly as it holds a claim, but the voucher's watermark compares amounts, not nonces. **Amended again by [0074](0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md)** (decision 3, 2026-09-25): the journal also holds, as `BatchChannelAdmitted`, the EVM `ChannelConfig` a batch-settlement channel was admitted under — neither signed nor irreversible, but nowhere else after a restart, and needed to claim the vouchers held on it. **Amended by [0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)** (accepted 2026-09-27): the nonce watermark goes and the voucher watermark becomes the only freshness rule; the journal also holds each outbound channel's config; a journal holding `toon-channel` entries is refused at boot by name. See the Update at the foot.

**Scope:** connector architecture — internal to this codebase. See the [ADR index](README.md).

The connector durably persists only what is signed or otherwise irreversible — claims sent,
claims received with their watermarks, and fulfilments not yet covered by a claim. Per-peer
balances and credit-limit positions are an in-memory projection rebuilt from that journal on
start. There is no ledger abstraction and no TigerBeetle.

## Why

Under ADR 0004 the claim is the thing of value: signed, cumulative, superseding. A balance is
just an arithmetic consequence of the claims exchanged and the fulfilments since the last one.
Storing it as independent authoritative state creates a second thing that can disagree with
the first, and the reconciliation between them is work with no upside.

TigerBeetle is being dropped because it was never real. It appears nowhere in
`docker-compose*.yml`, `deploy/`, `infra/`, `config/connector.prod.yaml` or the `Makefile` —
only in `src/` and tests. Every deployed node has always fallen back to
`InMemoryLedgerClient`. What we actually carried was a `LedgerClient` port, two
implementations, a batch writer, an error-mapping layer, a dual-mode `AccountManager` and an
optional peer dependency, all to keep alive an option nobody exercised.

An official Rust client exists on the 0.16 line, so this is reversible if throughput ever
demands it. Re-adding a backend to a system with one concrete implementation is a bounded
task; carrying a port for a hypothetical second one is a permanent tax.

## Consequences

Recovery is replay, not reconciliation. On start the connector reads its journal and
recomputes balances. Correctness therefore depends on the journal being written before value
is considered moved, which is a much easier property to test than agreement between two
stores.

The projection must be reconstructible by pure code. That puts it in `connector-core` — no
async, no I/O — so the arithmetic that decides whether a peer is over its ceiling is
property-testable without a database, a chain, or a network.

Losing double-entry means losing its built-in `sum(debits) == sum(credits)` check. The
replacement invariant is that every peer's projected balance equals the delta between the
cumulative in its latest sent claim and its latest received claim, plus uncovered
fulfilments. That is checkable on every projection rebuild, and should be.

## Update (ADR 0075, issue #1371) — accepted 2026-09-27, implementation pending

[0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision 8 amends this
record three ways. **The nonce watermark is deleted**: `Watermark { nonce, … }`, `validate_claim`
and `advance_watermark` go, and 0074's amount-only voucher watermark becomes the only freshness
rule, on both books. **The journal holds one more thing that is neither signed nor irreversible**:
an outbound channel's config (on EVM the whole `ChannelConfig`, including `salt`), written before
the opening transaction is sent, for the reason 0074 gave for `BatchChannelAdmitted`. **A journal
holding `toon-channel` entries is refused at boot, by name**, with the drain procedure in the
message — never skipped. Claims-as-truth and recovery-by-replay are untouched.

Until #1371's implementing steps land, the binary behaves as this record describes above; this
Update states what changes when they do.
