---
name: rfc-0038-settlement-engines
description: Expert knowledge of Interledger RFC 0038 - Settlement Engines. Use when users ask about settlement engines, ledger integration, settlement triggers, account balance management, or settlement system interfaces. NOTE: this connector does not implement RFC 0038 — the skill explains what TOON does instead. Triggers on 'settlement engine', 'ledger settlement', 'settlement integration', or settlement questions.
---

# RFC 0038: Settlement Engines

## This connector does not implement it

TOON settles in-process through its x402 settlement port (EVM and Solana, ADR 0075), not through a sidecar behind an HTTP API. There is no settlement-engine interface to implement.

RFC 0038 is therefore **not vendored** into `docs/rfcs/`
([ADR 0062](../../../docs/adr/0062-an-rfc-is-vendored-verbatim-and-profiled-never-forked.md)
D4: a copy would assert a relevance this one does not have). The ten RFCs this
connector *does* implement or profile are there, each with a TOON profile.

Say this plainly when asked. Somebody looking for RFC 0038 behaviour in this
codebase will not find it, and the useful answer is what replaced it — not a
search.

## If you need the RFC itself

It is upstream, and this repository holds no copy:
<https://github.com/interledger/rfcs/blob/main/0038-settlement-engines/0038-settlement-engines.md>

Read it there when the question is about Interledger generally rather than about
this connector. Do not vendor it in passing — that is ADR 0062 D4's decision,
not a gap.

## Where to look instead

- [`docs/rfcs/README.md`](../../../docs/rfcs/README.md) — the ten vendored RFCs.
- [`CONTEXT.md`](../../../CONTEXT.md) — this project's vocabulary, which differs
  from RFC 0019's in the places that matter here.
- [`docs/adr/README.md`](../../../docs/adr/README.md) — the decisions, grouped.
