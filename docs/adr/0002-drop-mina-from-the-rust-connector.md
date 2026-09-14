# The Rust connector settles on EVM and Solana only; Mina is dropped

**Status:** Accepted — extended by [0065-mina](0065-mina-leaves-the-repository.md). It holds in the tree: the Cargo workspace has no Mina crate. The one thing this record deliberately left standing, `packages/mina-zkapp` (the separately deployed zkApp), was deleted by 0065-mina along with the rest of the Mina surface.

**Scope:** connector architecture — internal to this codebase. See the [ADR index](README.md).

Mina is the only chain with no Rust path for the work that matters: its five zkApp methods
(`initializeChannel`, `deposit`, `initiateClose`, `settle`, `claimFromChannel`) require proof
generation through o1js, which exists only in JavaScript. Supporting Mina would mean shipping
a Node sidecar beside the binary forever, so we are dropping it rather than carrying a
JavaScript runtime into a rewrite whose point is to leave one behind.

## Considered options

Reimplementing the payment-channel circuit against Rust kimchi/pickles was rejected: the
verification key would have to stay bit-identical to the deployed zkApp, and a mismatch is
undetectable until a transaction is rejected on-chain.

## Consequences

The per-packet Mina work was never the hard part — inbound claim verification uses only
Poseidon hashing and Schnorr signature verification, both native in Rust. What we are giving
up is specifically the on-chain channel lifecycle, and with it Mina's privacy story.

Dropping Mina also retires the pile of accidental complexity attached to it: the o1js dual
CJS/ESM instance bug, the `Field is not a function` failure, the parallel `dist-esm/` build,
and the Node ≥ 22.12 pin that exists solely so `require()` of an ES module works on the Mina
claim path. It also retires an open correctness bug — `initiateClose`/`settle` use exact
`globalSlot` preconditions that are broken against real Mina, fixed on a branch and never
merged — instead of porting it.

`SettlementBackend` is nonetheless designed so an out-of-process implementation is possible.
That variant is not built. If Mina returns it arrives as a sidecar crate, not as surgery on
the trait.
