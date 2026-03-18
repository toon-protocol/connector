# Contributing to M2M

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

- **[Developer Guide](docs/development/developer-guide.md)** - Epic branch workflow, pre-push checklist, git hooks overview
- **[Git Hooks](docs/development/git-hooks.md)** - How pre-commit/pre-push hooks work and troubleshooting
- **[Test Strategy and Standards](docs/architecture/test-strategy-and-standards.md)** - Test quality anti-patterns, best practices, stability testing
- **[Coding Standards](docs/architecture/coding-standards.md)** - TypeScript strict mode guidelines, critical rules, naming conventions

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

- **Node.js** 20.11.0 or higher (LTS)
- **npm** 10.x or higher
- **Git** 2.x
- **Docker**:
  - **Linux/Windows:** Docker Desktop or Docker Engine
  - **macOS:** Native TigerBeetle installation required (no Docker) - See [macOS Setup Guide](docs/guides/local-development-macos.md)
    - ⚠️ TigerBeetle requires native installation on macOS
    - One-command setup: `npm run tigerbeetle:install`
- **Familiarity** with TypeScript and Interledger Protocol basics

### Initial Setup

1. Fork the repository on GitHub
2. Clone your fork locally:
   ```bash
   git clone https://github.com/YOUR_USERNAME/m2m.git
   cd m2m
   ```
3. Add upstream remote:
   ```bash
   git remote add upstream https://github.com/ORIGINAL_OWNER/m2m.git
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

- `connector` - Changes to @toon-protocol/connector package
- `shared` - Changes to @toon-protocol/shared package
- `explorer` - Changes to the built-in Explorer UI
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

- ✅ ESLint checks (no errors)
- ✅ Prettier formatting checks
- ✅ TypeScript compilation (all packages)
- ✅ Jest tests with coverage thresholds:
  - `@toon-protocol/shared`: ≥90% coverage
  - `@toon-protocol/connector`: ≥80% coverage

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

### Test Coverage Thresholds

- **@toon-protocol/shared**: Minimum 90% line coverage (critical protocol logic)
- **@toon-protocol/connector**: Minimum 80% line coverage

### Test Organization

- **Unit tests**: Co-located with source (`*.test.ts` next to `*.ts`)
- **Integration tests**: In `packages/*/test/integration/`
- **Mocks**: Shared mocks in `__mocks__/` directories

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

- **[CI Troubleshooting Guide](docs/development/ci-troubleshooting.md)** - Comprehensive guide for debugging CI failures
  - Common failure scenarios (lint, test, build, type-check, contracts, E2E)
  - Job-specific debugging procedures with diagnostic commands
  - Investigation runbook for systematic debugging

### Test Failures

- **[Test Anti-Patterns](docs/architecture/test-strategy-and-standards.md#common-test-anti-patterns-and-solutions)** - Common testing mistakes and fixes
  - Event listener cleanup failures
  - Async timeout issues
  - Mock state leakage
  - Testing implementation details instead of behavior
  - Incomplete test cleanup (resources not released)
  - Hardcoded timeouts in production code

### Root Cause Analyses

Past failures and their resolutions are documented in `docs/qa/`:

- **[RCA 10.1: Settlement Executor Test Failures](docs/qa/root-cause-analysis-10.1.md)** - Event listener cleanup anti-patterns

### Reporting Issues

If you discover a bug or systematic issue:

1. **Check Existing Issues**: Search [GitHub Issues](https://github.com/USERNAME/m2m/issues) for similar reports
2. **Provide Context**:
   - Clear description of the problem
   - Steps to reproduce
   - Expected vs actual behavior
   - Environment (Node.js version, OS, npm version)
   - Relevant logs or error messages
3. **Create Issue**: Use the appropriate issue template (bug report, feature request)
4. **Tag Appropriately**: Use labels like `bug`, `enhancement`, `documentation`, `test-quality`

### Getting Help

- **Documentation**: Start with [Developer Documentation Index](docs/development/README.md)
- **GitHub Discussions**: Ask questions in [Discussions](https://github.com/USERNAME/m2m/discussions)
- **CI Failures**: Use [CI Troubleshooting Guide](docs/development/ci-troubleshooting.md)
- **Epic Branch Issues**: See [Epic Branch Workflow](docs/development/developer-guide.md#epic-branch-workflow)

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

Thank you for contributing to M2M! Your efforts help make Interledger education and testing better for everyone.
