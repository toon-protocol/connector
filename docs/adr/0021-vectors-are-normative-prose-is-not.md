# Vectors are normative; prose is not

**Status:** Accepted, **amended by [0045](0045-a-behavioural-rule-is-normative-prose-until-its-vector-lands.md)**, and still the tiebreaker for every protocol record: `crates/connector-vectors` is the contract for a fact a vector covers. 0045 adds a bounded prose tier for behavioural rules a vector does **not** yet cover, and corrects this record's claim that vectors are generated from property tests — they are generated from fixed literal fixtures (`connector-vectors/src/lib.rs`; the crate has no `proptest` dependency). **Disturbed by [0074](0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md)**: its voucher cases take `schema_version` to 6 (#1347).

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

The Rust implementation is the definition of the wire. A committed set of vectors, generated from
property tests over the invariants, is the contract every client SDK is held to. Prose describing
the wire still exists and is marked non-normative.

## Context

The prototype's client edge was specified by `docs/protocol/client-edge-spec.md`: 283 lines of
RFC-2119 prose over 59 lines of implementation (`crates/connector-client-edge/src/lib.rs`, 410 lines
of which 350 are tests). The gap was not the only problem:

- It contradicted `CONTEXT.md` for as long as both documents existed. ADR 0016 on why nobody caught
  it: _"Nobody noticed, because the thing they disagree about had no name."_
- Its §1.4 still describes an x402 greeting that ADR 0011 removed, which ADR 0016 had to note as
  stale rather than fix.
- Its §1.2 through §1.6 were never implemented, and nothing reported that.

Prose that nothing executes drifts from the code in every direction available to it, and reports
none of it. That matters more now than it did: `toon-client`, `rig` and `swap` all have to speak
this wire, and none of them live in this repository.

## Decision

**The Rust implementation is the definition.** A committed set of vectors — wrapped packets,
envelopes, conditions and the fulfilments they derive — is the cross-repo contract. Every client SDK
replays them as its own suite, and reproducing the bytes is what conformance means.

Vectors are **generated from property tests over the invariants**, not captured from whatever the
implementation happened to emit. The properties are the specification; the vectors are its evidence.

Prose describing the wire continues to exist, and says on its face that it is not normative.

## Considered options

**A normative prose specification.** Human-readable, and the conventional answer. Rejected: it is
exactly what failed here, documented above, at a cost this project has already paid once.

**A machine-readable schema with encoders generated for both languages.** Drift becomes impossible
rather than merely detectable, which is the strongest option on offer. Rejected on tooling: OER
schema codegen across Rust and TypeScript is not a road worth starting down for this.

## Consequences

The Rust implementation becomes normative by construction. An encoder bug becomes the standard the
moment vectors are generated from it and three repositories pin to them. Generating from properties
rather than from observed output is the mitigation, and it is only a partial one — a property that
is wrong produces vectors that are wrong, consistently and confidently.

Cross-repo work gains a definition of done that is not "read the specification carefully."

A vector set is only as good as the invariants behind it. The properties worth stating — envelope
round-tripping, decode rejecting every malformed input, a derived fulfilment satisfying the
condition the sender minted, cost summing across a path — should be written before the encoder they
are meant to pin.
