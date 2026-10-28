# Property tests over a pure core; fakes are allowed, mocks are not

**Status:** Accepted, amended by [0084](0084-a-chain-too-heavy-to-spawn-per-test-is-spawned-once-per-test-binary.md) (a chain per test by default, and a shared tier, one chain per test binary, for a chain too heavy for that). The crate it names, `connector-core`, shipped as `connector-domain`; the local-disposable-chain section is an in-place amendment, not a separate record.

**Scope:** connector architecture — internal to this codebase. See the [ADR index](README.md).

Testing runs in three tiers: property tests over `connector-core`, which has no I/O at all;
contract tests defined once per port and run against every implementation of it; and
integration tests against real chains, only where a chain is genuinely involved. An
implementation that upholds a port's contract and passes its contract suite is a legitimate
test subject. A stub that asserts a sequence of calls is not, and stays banned.

## Why this replaces "never use mocks"

The existing rule is emphatic and was right for the code it governed: when every path needs a
chain, a socket and a database to execute, mocks are the only alternative to slow tests, and
mocks in money code are worse than slow tests. But it treated the symptom. Nothing could be
tested cheaply because nothing was pure, and the result is 72,433 lines of test code — more
than the source — with a Docker chain in the loop for changes to arithmetic.

Extracting `connector-core` removes the premise. Route selection, claim validation, nonce and
watermark rules, the balance projection, ceiling arithmetic and expiry are all decidable from
values alone. They are tested exhaustively in milliseconds by not needing I/O, rather than by
pretending to have it.

The rule also failed to distinguish two very different things. `InMemoryLedgerClient` was
never a mock: it is a working implementation of a contract, and it shipped to production as
the only ledger anyone actually ran. The line that matters is not real-versus-fake, it is
whether the thing under test upholds a contract or merely replays a script.

## Consequences

Every port owes a contract suite before it owes a second implementation. The suite is the
definition of the port; an implementation that has not passed it is not an implementation.
This is also what makes an in-memory backend safe to trust — it is held to the same statements
as the real one.

Integration tests shrink to the cases where chain behaviour is the thing under test: gas
estimation, nonce conflicts, reorgs, confirmation semantics. Everything that merely _involves_
a chain incidentally moves down a tier.

## The "real chain" in that tier is a local, disposable one

Added after #459, which first exercised this tier. "Real chain" means a real node, not a shared
one. Tests spawn their own throwaway chain per run — `anvil` for EVM, `solana-test-validator`
for Solana — deploy into it, and discard it. They do not run against devnet, testnet or any
long-lived shared network.

This is not merely the cheaper option. A shared chain reintroduces the coupling this doctrine
exists to remove:

- A single funded key across concurrent CI runs produces cross-run nonce contention, which is
  indistinguishable from the nonce bugs these tests exist to catch. The tier's own subject
  matter becomes its flakiness.
- Faucets run dry and public RPC rate-limits, so the gate's verdict starts depending on
  someone else's availability rather than on the commit.
- It requires a funded credential wherever the tests run, including inside agent sandboxes.
  A local chain needs none, which keeps chain secrets out of CI containers entirely.
- The devnet Solana validator has halted with a full ledger while `/health` still returned ok.
  That is the failure class you inherit by gating on infrastructure you share.

Two obligations follow, both learned the hard way in #471:

**Pin the chain tooling.** The gate installs a fixed version (Foundry `v1.7.1` today), so it
tests against what was actually verified rather than whatever upstream shipped that morning.

**A missing chain binary must fail CI, never skip it.** A guard that returns early when
`anvil` is absent reports `passed` in `0.00s` having asserted nothing — a green tick that
means less than a missing test, because it claims success. Guards therefore panic when `CI` is
set and skip only for local runs, where not every contributor should need Foundry installed to
run `cargo test`. `crates/connector-settlement-evm/tests/support/mod.rs` is the reference.

Deployments to shared testnets remain worthwhile for a different job — giving the running
devnet nodes something to settle against (`packages/contracts/deployments/base-sepolia.md`).
That is operating the network, not testing the code, and the two should not be conflated.

## Update (issue #1568)

"A throwaway chain per run" above was always read as a chain per **test**: that is what CLAUDE.md,
CONTRIBUTING.md and `Anvil::spawn` / `SolanaValidator::spawn` do.
[ADR 0084](0084-a-chain-too-heavy-to-spawn-per-test-is-spawned-once-per-test-binary.md) says so, and
adds a second tier. A chain too heavy to spawn per test (more than 10 s to its first usable block, or
more than 1 GB) may be spawned once per test binary, by the harness itself, with each test isolated
by fresh accounts and channels.

The four risks listed above are why that is acceptable rather than a retreat from this section. Each
one is about a chain shared across runs or with other people, and a chain the test binary spawns for
itself and discards has none of them. The two obligations above still bind the shared tier: its chain
is pinned (a Docker image by digest), and a missing one fails CI. Deployments to shared testnets are
still not testing the code (0084 decision 8).
