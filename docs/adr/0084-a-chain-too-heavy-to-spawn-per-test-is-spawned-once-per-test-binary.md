# A chain too heavy to spawn per test is spawned once per test binary

**Status:** Accepted (owner decision, 2026-10-28, #1568). **Amends** [0007](0007-testing-doctrine-fakes-yes-mocks-no.md): its local-disposable-chain section said a chain per _run_, where CLAUDE.md, CONTRIBUTING.md and the code have always meant a chain per _test_. The rule is now per test by default, with a shared tier for a chain that is too heavy for that. The shared tier is **not yet built**: no admitted chain needs it. Mina is the first that will, and [#1554](https://github.com/toon-protocol/connector/issues/1554) builds it.

**Scope:** connector architecture — internal to this codebase. See the [ADR index](README.md).

**Falsifier:** `crates/**/*.rs` matching `fn spawn_shared` — the name this record prescribes for the harness entry point that spawns a chain once per test binary. A match means the shared tier has been built and this Status line is stale. Like ADR 0048's falsifier, it is a name this record prescribes, so an implementer who chooses another name slips past it. (It was `toon\.test-chain\.owner` until #1610; see the Update at the end.)

**A chain-backed test spawns its own disposable chain, as before. A chain too heavy for that may
instead be spawned once per test binary, by the harness itself, with each test isolated by fresh
accounts and fresh channels. A chain is admitted to the gate only if the harness can spawn one that
runs the deployed program unmodified. A public testnet never counts toward the gate.**

## Context

The connector settles on EVM and Solana. Map [#1563](https://github.com/toon-protocol/connector/issues/1563)
asks it to settle on every chain with an x402 batch-settlement binding, and map
[#1548](https://github.com/toon-protocol/connector/issues/1548) asks it to settle on Mina. The tier-3
rule (CLAUDE.md, "Tier 3 does not use the Docker containers") is that every chain-backed test spawns
its own chain and drops it, and that a missing chain binary fails CI. Anvil meets that rule in about
0.03 s and Solana's validator in under 5 s on CI. Not every chain does
([`harness-spawned-test-chains.md`](../research/harness-spawned-test-chains.md), cited as **NOTE**):

- **XRPL** (`xrpld` stand-alone) is ready in 1–3 s, so it fits the rule as written.
- **Mina** (lightnet) takes 37–46 s to sync at about 0.7 GB, and about 26 s to fund an account. It
  was built to be one shared network with an accounts manager.
- **Cardano** (Dolos's emulator or Yaci DevKit) is unmeasured, and neither runs the Haskell node.
- **Hedera** (Solo) needs a Kubernetes cluster, at least 12 GB and 3–5 minutes warm. Nothing lighter
  runs its real system contracts, and its binding's own tests stub them.

Issue #1568 offered three answers: a shared chain per CI job, a public testnet, or not admitting the
chain. The first changes the doctrine, the second makes the gate depend on a third party (the reason
the `onion` topology is off the gate, ADR 0070), and the third forecloses Mina.

ADR 0007's own reasons against a shared chain are cross-run nonce contention on one funded key, dry
faucets and rate-limited RPC, a funded credential wherever the tests run, and inheriting a shared
network's outages. All four are about a chain shared **across** runs or **with** other people. A
chain the test binary spawns for itself and throws away has none of them.

## Decision

### 1. Per test is the default

A chain-backed test spawns its own chain and tears it down on drop, as `Anvil::spawn` and
`SolanaValidator::spawn` do today. A test that moves the chain's clock, or depends on chain-wide
state, always gets its own chain, whatever tier its chain is in.

### 2. A chain too heavy for that is spawned once per test binary

A chain is **too heavy** when one instance takes more than **10 s** to its first usable block, or
more than **1 GB** of memory, measured on the gate's runner at the version the gate pins. The 1 GB
figure comes from the widest test binary: today it spawns 15 chains at once on a 16 GB runner that
is also compiling the workspace. A chain's spec records its measurement against both figures.

Such a chain may use the **shared tier**: one chain per test binary, spawned lazily by the harness on
first use. Each test then gets fresh funded accounts and opens its own channels. The receiving
contract suite already works this way on one chain, with a fresh channel per case (NOTE §1.4).

The rule is not retroactive. Anvil, Solana's validator and XRPL stay per test.

### 3. Funding on a shared chain goes through one funder

On a shared chain every test funds itself from the same genesis account or pool. Concurrent funding
races that account's sequence numbers, which looks exactly like the nonce bugs ADR 0007 says this
tier exists to catch. So a shared-tier harness serialises funding through a single funder per chain.
Pre-splitting accounts at spawn was considered and not chosen, because it fixes in advance how many
tests the chain can serve.

### 4. A shared chain never outlives the next run

Rust never drops a `static`, so a chain held in a `OnceLock` outlives its test binary unless the
harness arranges otherwise. A shared-tier harness:

- starts every container with `--rm` and a `toon.test-chain.owner` label holding the PID of the test
  process that spawned it;
- before it spawns, removes any container carrying that label whose owner is no longer running;
- starts a native chain process with `PR_SET_PDEATHSIG`, so it dies with its parent.

A chain left behind by a crash or a Ctrl-C therefore lasts until the next run at most.

### 5. A Docker-only chain fails CI and skips locally, like a missing binary

A shared-tier chain may come only as a Docker image (Mina's does). For such a chain, "the binary is
present" means Docker is reachable and the image is present, **pinned by digest** the way Foundry and
the Solana CLI are pinned by version. Its `require_*` guard panics when `CI` is set and skips
otherwise, exactly as `require_anvil()` does.

### 6. The harness spawns one process or one container

A chain the harness spawns is a single process or a single container. A harness that drives a
cluster (Solo's kind deployment) does not count.

### 7. A chain is admitted only if its test chain runs the deployed program unmodified

What makes a test chain real is the program it runs, not the node that runs it. Anvil already
reimplements Ethereum and is the precedent. A test chain qualifies if it runs **the exact program
the connector settles against** (bytecode, validator, program or zkApp) with nothing swapped in. On
that test, Dolos's emulator qualifies for Cardano, and Hedera's `vm.etch` emulation of its system
contracts does not.

A test chain may run in a mode that switches off a check the real chain makes, such as Mina's
lightnet with proofs off. It is admissible only if that check is tested somewhere else on the gate.
How Mina meets that is #1554's to decide.

A chain is admitted to settlement only when such a test chain exists, under decision 6, and its
delay tests run on the gate in bounded time. How a chain without a clock warp meets the second
condition (a test-only floor or something else) is decided per chain, under map #1563's D7.

### 8. A public testnet is never the gate

A test against a public testnet never counts toward the gate and never replaces a tier-3 suite. A
chain may have such a check **off** the gate, run by hand or on a schedule as `onion` is, and only
**beside** a tier-3 suite that passes the gate.

## Considered options

**A shared chain per CI job**, as #1568 put it. Refused as written: a chain shared per job, and not
spawned by the harness, brings back the hand-started container that "Nothing under `crates/` dials
`localhost:8545`" exists to forbid. Decision 2 keeps what was right about it, a chain shared
between tests, and drops what was wrong.

**A public testnet as the gate.** Refused (decision 8). The gate's verdict would depend on someone
else's network, and a funded credential would have to live wherever the tests run. Both are ADR
0007's own reasons.

**Not admitting a chain that cannot be spawned per test.** Kept as the fallback for a chain that
cannot meet decisions 6 and 7, but not as the rule: under that rule Mina could not be admitted at
all.

**A threshold in seconds only, or memory only.** Each misses the other cost. The limit that bites is
how many instances the widest binary runs at once on a 16 GB runner, which takes both figures.

**Building the shared-tier harness now.** Not done. No admitted chain uses it, so it would be code
nothing calls and nothing tests. Mina's #1554 is its first user and builds it.

## Consequences

**Hedera stays out under this rule,** separately from its parking on map #1563: Solo is a cluster
(decision 6). It comes back only if something that is one process or one container runs its real
system contracts.

**Cardano can be admitted on Dolos's emulator** if its measurements and delay tests pass. Whether it
runs per test or shared is decided by those measurements.

**Mina is admitted on a shared lightnet,** provided #1554 tests the proof check somewhere else on the
gate (in-process verification of a real proof against the verification key, which spike #1512 showed
works in Rust, is one way) and its delay tests run in bounded time.

**The Rust gate will need Docker** once the first Docker-only chain is admitted, and it does not
before then. CONTRIBUTING.md says so.

**Nothing changes for EVM, Solana or XRPL.** They stay per test.

## Update (issue #1610)

The `toon.test-chain.owner` label is no longer evidence of a shared tier. The XRPL test chain
(`connector_settlement_xrpl::test_support::Xrpld`, the first container-backed chain) is spawned **per
test**, as decision 2 says XRPL stays, and it applies decision 4's label and orphan sweep to its
containers: a container left by a killed run is removed by the next spawn, by owner PID. Per-test
containers need that as much as shared ones, since `--rm` does nothing for a process that was killed.
So the falsifier now looks for the shared tier's own entry point, `fn spawn_shared`, which does not
exist yet. The "Rust gate will need Docker" consequence has come true for `connector-settlement-xrpl`
alone; CONTRIBUTING.md says so.
