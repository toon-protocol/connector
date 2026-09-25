# Contributing to Connector

Thank you for your interest in contributing to the Multi-node Interledger Connector project! This document provides guidelines for contributing code, documentation, and bug reports.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Workflow](#development-workflow)
- [Commit Message Convention](#commit-message-convention)
- [Pull Request Process](#pull-request-process)
- [Code Review Guidelines](#code-review-guidelines)
- [Testing Requirements](#testing-requirements)
- [Coding Standards](#coding-standards)

## Code of Conduct

This project adheres to a professional and respectful environment. Please be kind, constructive, and collaborative in all interactions.

## Before You Start

Before contributing, please read the following documentation to understand project workflows and quality standards:

### Required Reading

- **[CONTEXT.md](CONTEXT.md)** - The vocabulary. Read before writing docs or naming anything; several terms here mean something narrower than they do elsewhere.
- **[ADR 0007](docs/adr/0007-testing-doctrine-fakes-yes-mocks-no.md)** - The testing doctrine: property tests over a pure core, contract suites per port, fakes yes and mocks no.
- **[docs/adr/README.md](docs/adr/README.md)** - The decisions, grouped. Where an ADR and a spec disagree, the ADR wins.
- **[docs/architecture/source-tree.md](docs/architecture/source-tree.md)** - What every crate does, and what in this repository is deliberately not the connector.
- **[Coding Standards](docs/architecture/coding-standards.md)** - Naming and structure conventions.

### Key Concepts

- **Epic Branch Workflow**: Multi-story features are developed on epic branches before merging to main
- **Quality Gates**: Pre-commit and pre-push hooks catch issues before CI
- **Test Anti-Patterns**: Avoid common testing mistakes (event listener cleanup, async timeouts, mock state leakage)
- **CI/CD Pipeline**: GitHub Actions validates all changes with lint, test, build, and type-check jobs

### Quick Setup

After reading the documentation above:

1. Fork and clone the repository
2. Install dependencies: `npm install` (this also installs git hooks automatically)
3. Verify setup: `npm run build && npm test && npm run lint`
4. Pre-commit hooks are now active and will run on every commit

## Getting Started

### Prerequisites

- **Rust** stable — the connector itself is Rust (ADR 0017), and CI pins nothing tighter than `dtolnay/rust-toolchain@stable`
- **Chain binaries** — `anvil`, `forge` and `solana-test-validator`, for the tests that need a real chain. See [Chain-backed tests](#chain-backed-tests) for which tests need which, and how to install each
- **Node.js** >= 22.11.0
- **npm** 10.x or higher
- **Git** 2.x
- **Docker** - Docker Desktop or Docker Engine. Needed for `local/` (the shipped image against real containerised chains) and for the `docker-compose.yml` chain profiles. The Rust test gate does **not** need it: every chain-backed test spawns its own `anvil` or `solana-test-validator` and throws it away.
- **Familiarity** with TypeScript and Interledger Protocol basics

### Initial Setup

1. Fork the repository on GitHub
2. Clone your fork locally, **with submodules**:
   ```bash
   git clone --recurse-submodules https://github.com/YOUR_USERNAME/connector.git
   cd connector
   ```
   `packages/contracts` vendors OpenZeppelin and forge-std as git submodules, and
   `connector-settlement-evm`'s `abi_provenance` test shells out to a real `forge build` of
   them — without the submodules that build fails on unresolved imports and the Rust gate
   reports a failure that has nothing to do with the Rust code. An existing clone catches up
   with `git submodule update --init --recursive`.
3. Add upstream remote:
   ```bash
   git remote add upstream https://github.com/toon-protocol/connector.git
   ```
4. Install dependencies:
   ```bash
   npm install
   ```
5. Verify setup:
   ```bash
   npm run build
   npm test
   npm run lint
   ```

## Development Workflow

### 1. Create a Feature Branch

Always create a new branch for your work. Use descriptive branch names following this pattern:

```bash
# Feature branches
git checkout -b feat/add-routing-table

# Bug fix branches
git checkout -b fix/btp-connection-timeout

# Documentation branches
git checkout -b docs/update-architecture

# Refactoring branches
git checkout -b refactor/simplify-packet-handler

# Test branches
git checkout -b test/add-oer-encoding-tests
```

### 2. Make Changes

- Follow the [Coding Standards](#coding-standards) documented in `docs/architecture/coding-standards.md`
- Write tests for all new functionality (see [Testing Requirements](#testing-requirements))
- Keep commits atomic and focused on a single change
- Run linting and formatting before committing:
  ```bash
  npm run lint
  npm run format
  ```

### 3. Commit Your Changes

Use the [Conventional Commits](#commit-message-convention) format for all commit messages.

### 4. Push to Your Fork

```bash
git push origin feat/your-feature-name
```

### 5. Open a Pull Request

- Go to the original repository on GitHub
- Click "New Pull Request"
- Select your fork and branch
- Fill out the pull request template with:
  - Description of changes
  - Related issue number (if applicable)
  - Testing performed
  - Screenshots (for UI changes)

## Commit Message Convention

This project uses **Conventional Commits** for clear and structured commit history. All commit messages MUST follow this format:

```
<type>(<scope>): <description>

[optional body]

[optional footer(s)]
```

### Commit Types

| Type       | Description                                    | Example                                                    |
| ---------- | ---------------------------------------------- | ---------------------------------------------------------- |
| `feat`     | New feature or functionality                   | `feat(connector): add BTP client reconnection logic`       |
| `fix`      | Bug fix                                        | `fix(routing): prevent null pointer in route lookup`       |
| `docs`     | Documentation changes                          | `docs(readme): update quick start instructions`            |
| `test`     | Adding or updating tests                       | `test(oer): add encoding edge case tests`                  |
| `refactor` | Code refactoring without behavior change       | `refactor(btp): extract message parser to separate module` |
| `perf`     | Performance improvements                       | `perf(routing): optimize route matching algorithm`         |
| `chore`    | Maintenance tasks (dependencies, build config) | `chore(deps): update TypeScript to 5.3.3`                  |
| `style`    | Code style changes (formatting, whitespace)    | `style(connector): apply Prettier formatting`              |
| `ci`       | CI/CD pipeline changes                         | `ci(github): add Docker build workflow`                    |
| `revert`   | Reverting a previous commit                    | `revert: revert "feat(connector): add rate limiting"`      |

### Commit Scope

The scope specifies which package or component is affected:

- `connector` - Changes to the Rust connector crates (`crates/*`)
- `contracts` - Changes to the EVM payment channel contracts
- `faucet` - Changes to the devnet faucet
- `monorepo` - Changes affecting the entire monorepo
- `btp` - BTP protocol implementation
- `routing` - Routing logic
- `oer` - OER encoding/decoding
- `telemetry` - Telemetry emission
- `config` - Configuration loading
- `deps` - Dependency updates

### Commit Description

- Use imperative mood: "add feature" not "added feature" or "adds feature"
- Start with lowercase (except for proper nouns)
- No period at the end
- Maximum 72 characters
- Be specific and descriptive

### Examples of Valid Commits

```bash
# Feature with scope
git commit -m "feat(connector): implement BTP client manager"

# Bug fix with body
git commit -m "fix(routing): handle invalid ILP address format

Adds validation for ILP addresses before route lookup to prevent
crashes when receiving malformed packets from peers."

# Documentation update
git commit -m "docs(architecture): add BTP protocol flow diagrams"

# Test addition
git commit -m "test(shared): add OER encoding test vectors from RFC-0030"

# Refactoring
git commit -m "refactor(connector): extract packet validation logic"

# Breaking change with footer
git commit -m "feat(routing)!: change routing table API to async

BREAKING CHANGE: RouteTable.lookup() now returns Promise<Route | null>
instead of synchronous Route | null. All callers must be updated to use
await or .then() for route lookups."
```

### Examples of Invalid Commits

```bash
# ❌ Too vague
git commit -m "fix stuff"

# ❌ Missing type
git commit -m "add routing feature"

# ❌ Wrong mood (past tense)
git commit -m "feat(connector): added BTP support"

# ❌ Capitalized description
git commit -m "feat(connector): Add BTP support"

# ❌ Period at end
git commit -m "fix(routing): prevent crash."

# ❌ No scope when specific package affected
git commit -m "feat: add BTP client"
```

## Pull Request Process

### Before Submitting

1. **Sync with upstream main:**

   ```bash
   git fetch upstream
   git rebase upstream/main
   ```

2. **Run all checks locally:**

   ```bash
   npm run build    # Must succeed
   npm test         # All tests must pass
   npm run lint     # No linting errors
   ```

3. **Review your changes:**
   ```bash
   git diff upstream/main
   ```

### PR Title

Use the same conventional commit format for PR titles:

```
feat(connector): add BTP reconnection with exponential backoff
```

### PR Description Template

```markdown
## Description

Brief summary of changes and motivation.

## Related Issue

Closes #123

## Type of Change

- [ ] Bug fix (non-breaking change which fixes an issue)
- [ ] New feature (non-breaking change which adds functionality)
- [ ] Breaking change (fix or feature that would cause existing functionality to change)
- [ ] Documentation update

## Testing Performed

- [ ] Unit tests added/updated
- [ ] Integration tests added/updated
- [ ] Manual testing in Docker environment

## Checklist

- [ ] Code follows project coding standards
- [ ] Tests pass locally (`npm test`)
- [ ] Linting passes (`npm run lint`)
- [ ] Documentation updated (if applicable)
- [ ] Commit messages follow conventional commits format
```

### CI Requirements

All pull requests must pass:

- ✅ `cargo fmt --all -- --check`
- ✅ `cargo build --workspace`
- ✅ `cargo test --workspace --exclude payment-channel`
- ✅ `cargo clippy --workspace --exclude payment-channel --all-targets -- -D warnings`
- ✅ ESLint + Prettier over the remaining npm workspaces (devnet tooling)

That is the order CI runs them in, and it is worth running locally in the same order — a
formatting failure is cheaper to find than a clippy one. `make rust-build` and `make rust-test`
are shorthands for the middle two; the `fmt` and `clippy` checks have no make target and are
typed out.

These run on a PR against **any** base branch, not only `main` (issue #1152).
That was not always true: `ci.yml` used to filter on `branches: [main]`, so a
PR stacked on another PR's branch ran none of the Rust gate and still showed
green checks from the workflows that had no branch filter. If you are looking
at a PR with no `Rust Workspace Gate` check at all, that is a trigger bug, not
a passing build — say so rather than merging it.

Beside the gate, `.github/workflows/codeql.yml` runs CodeQL's default query suite
over the Actions workflows, the JavaScript/TypeScript packages, the Python
scripts and the Rust workspace, filtered by `.github/codeql/codeql-config.yml`,
which excludes exactly one query (a literal claim `nonce` in a test is a
counter, not a key — see the comment there). A new alert of a shape that config
already excludes is a question about the config, to be answered in a PR that
also updates `fleet_release_gate.rs`, never a dismissal by hand.

### Merging, and Stacked PRs

Prefer basing a PR on `main`. When you do stack one PR on another's branch,
know the two hazards:

1. **Merging the base PR with `--delete-branch` auto-closes everything stacked
   on it**, and a PR whose base branch no longer exists **cannot be reopened**.
   `gh pr merge --squash --delete-branch` on the base is enough to lose the
   child outright — that is how #1149 was lost and had to be recreated by hand
   as #1150. Retarget the child at `main` _first_
   (`gh pr edit <child> --base main`), then merge and delete the base.

2. **Rebase the child after the base lands.** A squash-merged base leaves the
   child carrying the base's commits as duplicates until it is rebased onto
   `main`, which makes the diff — and every review of it — wrong.

## Code Review Guidelines

### For Authors

- Keep PRs focused and reasonably sized (<500 lines when possible)
- Respond to feedback promptly and professionally
- Mark conversations as resolved after addressing feedback
- Request re-review after making changes

### For Reviewers

- Review within 48 hours when possible
- Provide constructive, specific feedback
- Distinguish between required changes and suggestions
- Approve when code meets standards, even if minor improvements possible

## Testing Requirements

### Test Organization

The connector's tests are Rust: unit tests in-module (`#[cfg(test)]`), integration
tests in each crate's `tests/`. See ADR 0007 for the testing doctrine (fakes yes,
mocks no) — chain-touching tests run against a real `anvil` /
`solana-test-validator` and hard-fail rather than skip under `CI`.

### Chain-backed tests

Some integration tests need a real chain, and they **skip locally when the binary is absent but
panic when `CI` is set** — so the gate can never go green without one. A guard that returns early
and reports `passed` in `0.00s` is worse than a missing test, which is what issue #471 closed:
**never add a skip-when-unavailable branch that can go green in CI.**

| Needs                   | Get it with                                                      | Tests                                                                                                       |
| ----------------------- | ---------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `anvil` (Foundry)       | `curl -L https://foundry.paradigm.xyz \| bash`                   | `connector-settlement-evm`, `connector-operator`, `connector-cli`, `connector-client-edge`, `connector-bin` |
| `forge`                 | same                                                             | `connector-settlement-evm`'s `abi_provenance`, which rebuilds the contracts and diffs the committed ABI     |
| `solana-test-validator` | `sh -c "$(curl -sSfL https://release.anza.xyz/v2.1.21/install)"` | `connector-settlement-solana`, `connector-cli`, `connector-bin`                                             |

**That Solana version is not `stable` and not arbitrary.** This repository installs exactly two
Solana CLIs, for opposite reasons, and `crates/connector-settlement-solana/tests/solana_cli_pins.rs`
records both with the evidence behind them and fails the build if either literal drifts: **v2.1.21
wherever the program is run**, because v3's `solana-test-validator` hard-requires io_uring and
because the workspace pins the Solana crates to `=2.1.0`; **v3.1.12 wherever a deployed artifact is
built**. The row above is the run side, so it is the same CLI `ci.yml`'s `rust-gate` installs — a
local gate on a different one is not the gate.

**`cargo test` spawns its own chain.** This is the thing most often gotten wrong here. Every
chain-backed test forks a **disposable** node of its own on its own port and tears it down on drop
— `connector_settlement_evm::test_support::Anvil::spawn` for `anvil`, and
`connector_settlement_solana::test_support::SolanaValidator::spawn` for `solana-test-validator`,
which also loads `payment_channel.so` into genesis at a fixed program id, and the committed
`payment-channels` binary (`crates/connector-settlement-solana/fixtures/`, ADR 0074) at its
canonical one. Nothing under `crates/`
dials `localhost:8545` or `localhost:8899`, so running `make anvil-up` or `make solana-up` before
`cargo test` changes nothing. The Docker chain profiles exist for running a node by hand, and for
`local/` — not for the test gate.

### What the workspace gate does not cover

`cargo test --workspace --exclude payment-channel` is the connector's gate and nothing else's.
Four things sit outside it:

- **`packages/solana-program`** — the on-chain `payment-channel` crate, a Cargo workspace member
  and the thing `--exclude payment-channel` excludes. It has its own `cargo test-sbf` job in CI,
  and `make solana-test` locally.
- **`packages/contracts`** — a separate Foundry job (`forge test`, `.github/workflows/contracts.yml`).
  No make target runs it.
- **`npm test`** (and `make test`) — the surviving npm workspaces, which are devnet tooling only:
  the faucet and the announcer sidecar. It does **not** test the connector.
- **[`local/`](local/README.md)** — the shipped **image**, as uid 10001, on a mounted config,
  against real containerised chains. A separate gate answering a question `cargo test`
  structurally cannot: run one with
  `make local-verify LOCAL_TOPOLOGY=<solo|two-hop|mixed-chain|dealing>`. `dealing` is where a hop
  crosses a real denomination boundary at a declared rate
  ([ADR 0071](docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)). A fifth,
  `onion`, runs a real onion daemon per node and is deliberately not on the CI gate
  ([ADR 0070](docs/adr/0070-an-onion-address-is-a-host-not-a-carriage.md)) — run it by hand.

### Test Writing Guidelines

- Use AAA pattern (Arrange, Act, Assert)
- Descriptive test names: `should [expected behavior] when [condition]`
- Test edge cases: null inputs, empty arrays, maximum values
- Mock external dependencies (network calls, file I/O)
- Use `describe` blocks to group related tests

### Example Test

```typescript
describe('PacketHandler', () => {
  describe('validatePacket', () => {
    it('should return true when packet has valid ILP address', () => {
      // Arrange
      const packet = createMockILPPacket({ destination: 'g.us.alice' });

      // Act
      const result = validatePacket(packet);

      // Assert
      expect(result).toBe(true);
    });

    it('should throw InvalidPacketError when destination is empty', () => {
      // Arrange
      const packet = createMockILPPacket({ destination: '' });

      // Act & Assert
      expect(() => validatePacket(packet)).toThrow(InvalidPacketError);
    });
  });
});
```

## Test Distribution

Tests are organized into different categories based on execution time and purpose. This ensures fast feedback during development while maintaining comprehensive coverage in CI.

### Test Categories

| Category              | Location                                                        | Execution Time | When to Run                  |
| --------------------- | --------------------------------------------------------------- | -------------- | ---------------------------- |
| **Unit Tests**        | `src/**/*.test.ts` (co-located)                                 | <30s           | Every commit (pre-push hook) |
| **Integration Tests** | `test/integration/*.test.ts`                                    | 1-3 min        | CI pipeline                  |
| **Performance Tests** | `test/performance/*.test.ts`, `test/unit/performance/*.test.ts` | 2-5 min        | Nightly CI, on-demand        |
| **Acceptance Tests**  | `test/acceptance/*.test.ts`                                     | 5-30 min       | Nightly CI, release testing  |

### Running Tests by Category

```bash
# Unit tests only (fastest - used in pre-push hook)
npm run test:unit

# Default test run (unit + integration, excludes performance/acceptance)
npm test

# Performance tests (isolated configuration)
npm run test:performance

# Integration tests (requires docker-compose-dev.yml services)
npm run test:integration

# Full test suite (all categories)
npm test && npm run test:performance && npm run test:acceptance
```

### Test Stage Summary

| Stage            | Command                    | Scope                             | Typical Duration |
| ---------------- | -------------------------- | --------------------------------- | ---------------- |
| **Pre-commit**   | `lint-staged`              | Staged files only (lint + format) | <5s              |
| **Pre-push**     | `.husky/pre-push`          | Unit tests for changed files      | <30s             |
| **CI (PR)**      | `npm test`                 | Unit + Integration tests          | 3-5 min          |
| **CI (Nightly)** | `npm run test:performance` | Performance benchmarks            | 5-10 min         |

### Excluded Tests

The following tests are excluded from the default `npm test` run and must be executed explicitly:

- **Performance benchmarks** (`test/performance/`): Timing-sensitive tests requiring isolated execution
- **Unit performance tests** (`test/unit/performance/`): Profiler and metrics tests with strict thresholds
- **Acceptance tests** (`test/acceptance/`): Long-running end-to-end scenarios
- **Wallet derivation** (`wallet-derivation.test.ts`): 587s runtime, 1000+ wallet derivations
- **XRP channel tests** (`xrp-channel-*.test.ts`): Requires rippled node, unstable in CI

## When Things Go Wrong

If you encounter issues during development or CI failures, use these resources:

### CI Troubleshooting

Reproduce the gate locally before reading logs — it is the same four commands CI
runs, in the same order (see [CI Requirements](#ci-requirements)). If they pass
locally and fail in CI, the usual causes are a missing chain binary (see
[Chain-backed tests](#chain-backed-tests) — those panic under `CI` rather than
skipping) or a Solana CLI version other than the pinned one.

`.github/workflows/ci.yml` is the authority on what runs. `gh run view <id>
--log-failed` gets you the failing step without downloading the whole log.

### Test Failures

- **[ADR 0007](docs/adr/0007-testing-doctrine-fakes-yes-mocks-no.md)** - The doctrine, and the anti-pattern it exists to prevent: a stub that asserts a sequence of calls is not a test subject. A fake that upholds a port's contract suite is.
- A test that needs a chain gets one of its own. Nothing under `crates/` dials `localhost:8545` or `localhost:8899`, so a failure there is not a missing container.
- Never add a skip-when-unavailable branch that can go green in CI. A guard that returns early and reports `passed` in `0.00s` is worse than a missing test.

### Past failures

The reasoning behind a rule that looks arbitrary is usually in the record that
set it. [`docs/adr/`](docs/adr/README.md) is grouped by area, and a record's
`**Status:**` line — not the index — says whether it is still live.

### Reporting Issues

If you discover a bug or systematic issue:

1. **Check Existing Issues**: Search [GitHub Issues](https://github.com/toon-protocol/connector/issues) for similar reports
2. **Provide Context**:
   - Clear description of the problem
   - Steps to reproduce
   - Expected vs actual behavior
   - Environment (Node.js version, OS, npm version)
   - Relevant logs or error messages
3. **Create Issue**: Use the appropriate issue template (bug report, feature request)
4. **Tag Appropriately**: Use labels like `bug`, `enhancement`, `documentation`, `test-quality`

### Getting Help

- **What is where**: [docs/architecture/source-tree.md](docs/architecture/source-tree.md)
- **Why it is that way**: [docs/adr/README.md](docs/adr/README.md)
- **What a word means here**: [CONTEXT.md](CONTEXT.md)
- **Running a node rather than changing one**: [README.md](README.md)
- **GitHub Discussions**: [Discussions](https://github.com/toon-protocol/connector/discussions)

## Coding Standards

### Critical Rules

See `docs/architecture/coding-standards.md` for complete standards. Key rules:

- **TypeScript strict mode enabled** - No `any` types (except in test mocks)
- **No console.log** - Use Pino logger (`logger.info()`, `logger.error()`)
- **kebab-case filenames** - `packet-handler.ts` not `PacketHandler.ts`
- **PascalCase classes** - `class PacketHandler {}`
- **camelCase functions** - `function validatePacket() {}`
- **UPPER_SNAKE_CASE constants** - `const DEFAULT_BTP_PORT = 3000;`
- **Async/await preferred** - No callback-based code
- **Error handling required** - All async functions must handle errors

### File Naming Examples

```
✅ Good:
- packet-handler.ts
- btp-client-manager.ts
- oer-encoding.ts

❌ Bad:
- PacketHandler.ts
- btpClientManager.ts
- OEREncoding.ts
```

## Questions or Help?

- Open a GitHub issue with the `question` label
- Check existing documentation in `docs/`
- Review Interledger RFCs in `docs/rfcs/`

---

Thank you for contributing to Connector! Your efforts help make Interledger education and testing better for everyone.
