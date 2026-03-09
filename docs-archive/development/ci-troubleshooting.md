# CI Troubleshooting Guide

This guide provides comprehensive troubleshooting steps for GitHub Actions CI failures, diagnostic procedures, and best practices for maintaining a reliable CI/CD pipeline.

## Table of Contents

- [Overview](#overview)
- [Common CI Failure Scenarios](#common-ci-failure-scenarios)
- [Job-Specific Debugging Procedures](#job-specific-debugging-procedures)
- [CI Workflow Best Practices](#ci-workflow-best-practices)
- [Investigation Runbook](#investigation-runbook)
- [Monitoring CI Health](#monitoring-ci-health)
- [Continuous Improvement Process](#continuous-improvement-process)
- [When to Enhance CI Workflow](#when-to-enhance-ci-workflow)

## Overview

The M2M project uses GitHub Actions for continuous integration, running multiple jobs in parallel to validate code quality, tests, builds, and deployments. Understanding how to quickly diagnose and fix CI failures is critical for maintaining development velocity.

**CI Jobs Overview:**

| Job                         | Purpose                       | Duration | Failure Rate             |
| --------------------------- | ----------------------------- | -------- | ------------------------ |
| `lint-and-format`           | Code style validation         | ~2 min   | Low (caught by hooks)    |
| `test` (Node 20.11.0, 20.x) | Unit tests with coverage      | ~3-5 min | Medium (test quality)    |
| `build`                     | Package compilation           | ~2-3 min | Low (type errors)        |
| `type-check`                | TypeScript type validation    | ~2 min   | Low (IDE catches)        |
| `contracts-coverage`        | Solidity tests (Foundry)      | ~1-2 min | Low (contract stability) |
| `security`                  | npm audit vulnerabilities     | ~1 min   | Medium (dep updates)     |
| `rfc-links`                 | Documentation link validation | ~1 min   | Low (docs only)          |
| `e2e-test`                  | Full system Docker test       | ~5-8 min | Medium (environment)     |

## Common CI Failure Scenarios

### Scenario 1: Lint Failures

**Symptom:** "Run ESLint" step fails in `lint-and-format` job

**Example Error:**

```
/home/runner/work/m2m/m2m/packages/connector/src/core/connector-node.ts
  23:7  error  'unusedVar' is assigned a value but never used  @typescript-eslint/no-unused-vars
  45:15 error  Unexpected console statement                   no-console
```

**Diagnosis:**

1. Check ESLint output for specific rule violations
2. Note file path and line numbers of violations
3. Identify rule ID (e.g., `@typescript-eslint/no-unused-vars`)

**Resolution:**

```bash
# Run ESLint locally to see all violations
npm run lint

# Auto-fix violations where possible
npm run lint -- --fix

# For specific rules, add exception (last resort)
# eslint-disable-next-line no-console
```

**Prevention:**

- Enable ESLint in IDE with real-time feedback
- Use pre-commit hooks (Story 10.2) to catch violations before commit
- Run `npm run lint` before pushing

**Reference:** [.github/workflows/ci.yml lines 46-49]

---

### Scenario 2: Format Check Failures

**Symptom:** "Check Prettier formatting" step fails

**Example Error:**

```
Checking formatting...
packages/connector/src/core/packet-handler.ts
packages/shared/src/types/ilp.ts
Code style issues found in the above files. Forgot to run Prettier?
```

**Diagnosis:**

- Files not formatted according to Prettier rules (line length 100, single quotes)
- Usually caused by manual edits without running formatter

**Resolution:**

```bash
# Auto-format all files
npm run format

# Check which files would be formatted (dry run)
npm run format:check

# Format specific file
npx prettier --write packages/connector/src/core/packet-handler.ts
```

**Prevention:**

- Pre-commit hook automatically formats staged files (Story 10.2)
- Configure IDE to format on save (VS Code: "editor.formatOnSave": true)
- Run `npm run format` before committing if bypassing hooks with `--no-verify`

**Reference:** [.github/workflows/ci.yml lines 51-54]

---

### Scenario 3: Test Failures (Unit Tests)

**Symptom:** "Run tests with coverage" step fails

**Example Error:**

```
FAIL packages/connector/src/settlement/settlement-executor.test.ts
  SettlementExecutor
    ✓ should open channel when none exists (52 ms)
    ✕ should deposit additional funds when channel balance low (102 ms)

  ● SettlementExecutor › should deposit additional funds when channel balance low

    expect(jest.fn()).toHaveBeenCalled()

    Expected number of calls: >= 1
    Received number of calls:    0
```

**Diagnosis:**

1. Identify failing test file and test name
2. Check test failure type:
   - Assertion failures (`expect()` not met)
   - Timeout errors (async operations)
   - Mock state issues (incorrect setup)
   - Event listener cleanup failures
3. Review test failure stack trace for root cause

**Resolution:**

```bash
# Run specific test file locally
npm test -- settlement-executor.test.ts

# Run with verbose output for more details
npm test -- settlement-executor.test.ts --verbose

# Run single test by name
npm test -- settlement-executor.test.ts -t "should deposit additional funds"

# Check for flakiness with repeated runs
for i in {1..10}; do npm test -- settlement-executor.test.ts || echo "Run $i FAILED"; done
```

**Common Causes:**

| Cause                   | Symptoms                            | Fix                                                         |
| ----------------------- | ----------------------------------- | ----------------------------------------------------------- |
| Mock state issues       | Calls expected but not received     | Use `mockResolvedValueOnce()` for sequential calls          |
| Async timeouts          | Intermittent failures               | Add timeout after `emit()`, increase timeout values         |
| Event listener cleanup  | Memory leaks, "did not exit" errors | Store bound handler, use same reference in `.off()`         |
| Test order dependencies | Passes alone, fails in suite        | Fresh mocks in `beforeEach()`, proper `afterEach()` cleanup |

**Reference Test Anti-Patterns:** [docs/architecture/test-strategy-and-standards.md#common-test-anti-patterns-and-solutions]

**Prevention:**

- Write tests following anti-pattern guidelines
- Run stability tests (10x) after fixing flaky tests
- Use test isolation validation techniques (--runInBand, --randomize)
- Review root cause analyses in docs/qa/

**Reference:** [.github/workflows/ci.yml lines 106-113]

---

### Scenario 4: Build Failures

**Symptom:** "Build all packages" step fails

**Example Error:**

```
Building all packages (shared first, then others)...
> m2m@1.0.0 build
> npm run build --workspace=packages/shared && npm run build --workspaces --if-present

packages/connector/src/core/packet-handler.ts:45:12 - error TS2339: Property 'invalidProp' does not exist on type 'RoutingTable'.

45     const route = routingTable.invalidProp;
              ~~~~~~~~~~~
```

**Diagnosis:**

1. Check build output for TypeScript compilation errors
2. Note error code (e.g., TS2339 - Property does not exist)
3. Identify package and file causing failure

**Resolution:**

```bash
# Build all packages locally
npm run build

# Build specific package
npm run build --workspace=packages/connector

# Build with verbose TypeScript output
npx tsc -p packages/connector/tsconfig.json --noEmit --extendedDiagnostics
```

**Common Causes:**

| Error Code | Meaning                 | Fix                                          |
| ---------- | ----------------------- | -------------------------------------------- |
| TS2339     | Property does not exist | Check type definitions, import missing types |
| TS2345     | Argument type mismatch  | Fix function call with correct types         |
| TS2307     | Cannot find module      | Add missing import, check tsconfig paths     |
| TS18003    | No inputs found         | Check tsconfig.json `include` patterns       |
| TS6305     | Circular dependency     | Restructure imports to break cycle           |

**Prevention:**

- Enable TypeScript checking in IDE (VS Code: install TypeScript extension)
- Run `npm run build` locally before pushing
- Use strict mode TypeScript configuration (enabled in this project)
- Check type definitions after adding new dependencies

**Reference:** [.github/workflows/ci.yml lines 196-201]

---

### Scenario 5: Type Check Failures

**Symptom:** "Type check all packages" step fails

**Example Error:**

```
Checking connector...
packages/connector/src/core/connector-node.ts:67:5 - error TS2322: Type 'string | undefined' is not assignable to type 'string'.

67     const address: string = config.address;
       ~~~~~
```

**Diagnosis:**

- TypeScript errors in specific package
- Often related to strict null checks, any types, or missing type annotations
- May pass build but fail strict type check

**Resolution:**

```bash
# Type check all packages
npx tsc --noEmit -p packages/shared/tsconfig.json
npx tsc --noEmit -p packages/connector/tsconfig.json
npx tsc --noEmit -p packages/dashboard/tsconfig.json

# Type check with specific flags
npx tsc --noEmit --strict -p packages/connector/tsconfig.json
```

**Common Causes:**

- Missing type definitions for dependencies (`npm install --save-dev @types/node`)
- Incorrect type annotations (using `any` instead of specific types)
- Strict mode violations (null/undefined not handled)
- Shared package types not built before checking dependent packages

**Prevention:**

- Build shared package first (contains exported types)
- Enable strict mode in tsconfig.json (already enabled)
- Avoid `any` types (use `unknown` and type guards instead)
- Run type check before committing: `npx tsc --noEmit`

**Reference:** [.github/workflows/ci.yml lines 288-304]

---

### Scenario 6: Contract Test Failures

**Symptom:** "Run Foundry tests" step fails in `contracts-coverage` job

**Example Error:**

```
Failing tests:
Encountered 1 failing test in test/TokenNetwork.t.sol:TokenNetworkTest
[FAIL. Reason: revert: Insufficient deposit] testDeposit_RevertsWhenInsufficientFunds() (gas: 82345)
```

**Diagnosis:**

1. Check forge test output for specific test failures
2. Note revert reason (e.g., "Insufficient deposit")
3. Review Solidity test file and contract logic

**Resolution:**

```bash
# Run Foundry tests locally (in packages/contracts/)
cd packages/contracts
forge test

# Run with verbose output (-vvv shows stack traces)
forge test -vvv

# Run specific test
forge test --match-test testDeposit_RevertsWhenInsufficientFunds

# Run with gas reporting
forge test --gas-report
```

**Common Causes:**

| Cause                    | Symptoms                            | Fix                                                 |
| ------------------------ | ----------------------------------- | --------------------------------------------------- |
| Revert conditions        | Test expects success but tx reverts | Fix contract logic or test expectations             |
| Gas estimation issues    | Out of gas errors                   | Optimize contract code, increase gas limits in test |
| State setup incorrect    | Test fails on assertions            | Review test setup (balances, approvals, etc.)       |
| Foundry version mismatch | Tests pass locally, fail in CI      | Use same Foundry version (nightly)                  |

**Prevention:**

- Run `forge test` before committing contract changes
- Use descriptive revert messages for debugging
- Follow Solidity coding standards (docs/architecture/coding-standards.md)
- Test with realistic gas limits

**Reference:** [.github/workflows/ci.yml lines 148-150]

---

### Scenario 7: E2E Test Failures

**Symptom:** "Run E2E Full System Test" step fails

**Example Error:**

```
● E2E Full System › should forward packet through network and visualize in dashboard

  Timeout - Async callback was not invoked within the 5000ms timeout specified by jest.setTimeout.

  at packages/connector/test/integration/e2e-full-system.test.ts:42:5
```

**Diagnosis:**

1. Check Docker container logs (uploaded as artifact on failure)
2. Review test output for error messages
3. Identify timeout or connection failures

**Resolution:**

```bash
# Run E2E tests locally with Docker Compose
npm test --workspace=packages/connector -- e2e-full-system.test.ts

# Check Docker container status
docker ps -a

# View container logs
docker logs connector-a
docker logs connector-b
docker logs connector-c
docker logs dashboard

# Restart specific container
docker restart connector-a

# Clean up and retry
docker-compose down -v
docker-compose up -d
```

**Common Causes:**

| Cause                         | Symptoms                    | Fix                                                          |
| ----------------------------- | --------------------------- | ------------------------------------------------------------ |
| Container startup timeout     | "Container not healthy"     | Increase health check timeout, check logs for startup errors |
| Network configuration         | Connection refused errors   | Verify Docker network settings, port bindings                |
| Port conflicts                | "Address already in use"    | Stop conflicting services, use different ports               |
| Missing environment variables | Config errors               | Check .env file, docker-compose.yml env vars                 |
| Image build failures          | Container exits immediately | Review Dockerfile, check build logs                          |

**Debug Artifacts:**

- E2E container logs uploaded to GitHub Actions artifacts
- Download artifact: Actions tab → Artifacts section → `e2e-container-logs-{sha}`

**Prevention:**

- Test Docker setup locally before pushing
- Use health checks in docker-compose.yml
- Add retry logic for network-dependent operations
- Review E2E test timeout settings (current: 5 minutes)

**Reference:** [.github/workflows/ci.yml lines 369-386]

---

## Job-Specific Debugging Procedures

### Job: `lint-and-format`

**Purpose:** Validate code style with ESLint and Prettier

**Diagnostic Commands:**

```bash
# Run ESLint with verbose output
npm run lint --verbose

# Check Prettier formatting (no changes)
npm run format:check

# Auto-fix linting issues
npm run lint -- --fix

# Auto-format all files
npm run format
```

**Log Location:**

- GitHub Actions job output → "Run ESLint" step
- GitHub Actions job output → "Check Prettier formatting" step

**Common Fixes:**

- Run `npm run lint -- --fix` to auto-fix violations
- Run `npm run format` to format all files
- Check .eslintrc.json for rule configuration
- Verify .prettierrc.json for formatting rules

**Reference:** [.github/workflows/ci.yml lines 12-55]

---

### Job: `test` (Matrix: Node 20.11.0, 20.x)

**Purpose:** Run Jest unit tests across Node.js versions with coverage

**Diagnostic Commands:**

```bash
# Run all tests
npm test -- --coverage

# Run specific test file with verbose output
npm test -- settlement-executor.test.ts --verbose

# Run with coverage for specific package
npm test --workspace=packages/connector -- --coverage

# Check for resource leaks
npm test -- --detectOpenHandles my-test.test.ts

# Run tests sequentially (detect race conditions)
npm test -- --runInBand
```

**Log Location:**

- GitHub Actions job output → "Run tests with coverage" step
- Coverage artifact: `lcov.info` (uploaded to Codecov)

**Common Fixes:**

- Review test failure stack traces for root cause
- Check mock setup in `beforeEach()` (create fresh instances)
- Add timeouts after `emit()` calls for async handlers
- Verify event listener cleanup with `listenerCount()` assertions
- Use `mockResolvedValueOnce()` for sequential calls

**Node Version Issues:**
If tests pass on Node 20.11.0 but fail on Node 20.x:

- Check for Node.js version-specific behavior
- Verify dependencies compatible with both versions
- Review changelog for Node.js LTS updates

**Reference:** [.github/workflows/ci.yml lines 56-131]

---

### Job: `build`

**Purpose:** Build all packages (shared, connector, dashboard)

**Diagnostic Commands:**

```bash
# Build all packages
npm run build

# Build specific package
npm run build --workspace=packages/shared
npm run build --workspace=packages/connector
npm run build --workspace=packages/dashboard

# Clean and rebuild
rm -rf packages/*/dist
npm run build

# Build with TypeScript verbose output
npx tsc -p packages/connector/tsconfig.json --extendedDiagnostics
```

**Log Location:**

- GitHub Actions → "Build all packages" step
- "Verify dist artifacts" step (lists built files)

**Artifacts:**

- `build-artifacts-{sha}` uploaded on failure or success
- Download: Actions tab → Artifacts section

**Common Fixes:**

- Fix TypeScript compilation errors (see error code table in Scenario 4)
- Ensure shared package builds first (contains types for other packages)
- Check tsconfig.json configuration (paths, composite settings)
- Verify all source files included in tsconfig.json

**Reference:** [.github/workflows/ci.yml lines 165-241]

---

### Job: `type-check`

**Purpose:** TypeScript type validation for all packages

**Diagnostic Commands:**

```bash
# Type check all packages
npx tsc --noEmit -p packages/shared/tsconfig.json
npx tsc --noEmit -p packages/connector/tsconfig.json
npx tsc --noEmit -p packages/dashboard/tsconfig.json

# Type check with strict mode
npx tsc --noEmit --strict -p packages/connector/tsconfig.json

# Check specific file
npx tsc --noEmit packages/connector/src/core/connector-node.ts
```

**Log Location:**

- GitHub Actions → "Type check all packages" step
- Per-package output (shared, connector, dashboard)

**Common Fixes:**

- Build shared package first (provides type definitions)
- Add missing type imports: `import type { ILPPacket } from '@crosstown/shared'`
- Fix type annotations (replace `any` with specific types)
- Handle null/undefined cases (use optional chaining, nullish coalescing)

**Note:** Type check runs after build job, so shared types are available

**Reference:** [.github/workflows/ci.yml lines 261-304]

---

### Job: `contracts-coverage`

**Purpose:** Run Solidity tests with Foundry, generate coverage

**Diagnostic Commands:**

```bash
# Run Foundry tests (in packages/contracts/)
cd packages/contracts
forge test

# Run with verbose output (stack traces)
forge test -vvv

# Run specific contract tests
forge test --match-contract TokenNetworkTest

# Generate coverage report
forge coverage --ir-minimum --report summary

# Check gas usage
forge test --gas-report
```

**Log Location:**

- GitHub Actions → "Run Foundry tests" step
- Coverage report in "Generate coverage report" step

**Common Fixes:**

- Review Solidity revert messages for failure reasons
- Check test setup (balances, approvals, state)
- Verify contract logic matches test expectations
- Use forge debugger for complex failures: `forge test --debug <test-name>`

**Coverage Note:**
Coverage analysis with `--ir-minimum` can have gas differences. Manual review required for exact percentages. All tests pass in normal mode.

**Reference:** [.github/workflows/ci.yml lines 132-164]

---

### Job: `e2e-test`

**Purpose:** Full system Docker-based end-to-end test

**Diagnostic Commands:**

```bash
# Run E2E tests locally
npm test --workspace=packages/connector -- e2e-full-system.test.ts

# Check Docker container status
docker ps -a
docker-compose ps

# View container logs
docker logs connector-a
docker logs connector-b
docker logs connector-c
docker logs dashboard

# Restart containers
docker-compose restart

# Clean and rebuild
docker-compose down -v
docker-compose up -d --build

# Check network connectivity
docker network ls
docker network inspect m2m_default
```

**Log Location:**

- GitHub Actions → "Run E2E Full System Test" step
- Uploaded artifacts: `e2e-container-logs-{sha}` (on failure)

**Common Fixes:**

- Check container health status and startup logs
- Verify network connectivity between containers
- Ensure ports not conflicting (8080, 8081, 8082, 3000)
- Review environment variable configuration
- Increase test timeout if containers start slowly

**Timeout:** 5 minutes (configurable in workflow)

**Reference:** [.github/workflows/ci.yml lines 335-386]

---

## CI Workflow Best Practices

### Practice 1: Fail Fast with Clear Error Messages

**Why:** Reduces debugging time by surfacing errors immediately with context

**Implementation:**

```yaml
# Descriptive step names
- name: Run ESLint across all workspaces
  run: npm run lint --verbose

# Echo status messages before commands
- name: Build all packages
  run: |
    echo "Building shared package..."
    npm run build --workspace=packages/shared
    echo "✓ Shared package built successfully"

# Use continue-on-error: false for critical jobs
- name: Run tests with coverage
  run: npm test -- --coverage
  continue-on-error: false # Fail immediately on test failures
```

**Benefits:**

- Clear error context in logs
- Easier to identify which step failed
- Status messages confirm successful operations

**Reference:** [.github/workflows/ci.yml step naming patterns]

---

### Practice 2: Optimize CI Execution Time

**Why:** Faster feedback loop improves developer productivity

**Implementation:**

```yaml
# Use npm caching to speed up installation
- name: Setup Node.js
  uses: actions/setup-node@v4
  with:
    node-version: '20.11.0'
    cache: 'npm'  # Caches node_modules based on package-lock.json

# Run independent jobs in parallel
jobs:
  lint-and-format:  # Runs in parallel
  test:             # Runs in parallel
  build:            # Runs in parallel
  type-check:       # Runs in parallel
  contracts-coverage: # Runs in parallel

# Use matrix strategy for multi-version testing
strategy:
  matrix:
    node-version: ['20.11.0', '20.x']
  fail-fast: false  # Continue testing other versions

# Skip expensive jobs when appropriate
e2e-test:
  needs: [lint-and-format, test, build]  # Only runs if dependencies pass
```

**Benefits:**

- npm cache reduces installation time from 2-3 minutes to 30 seconds
- Parallel jobs reduce total CI time from 20 minutes to 8 minutes
- Matrix testing validates compatibility without duplicating workflow

**Current CI Time:** ~8-10 minutes (all jobs in parallel)

**Reference:** [.github/workflows/ci.yml caching and parallel jobs]

---

### Practice 3: Provide Debugging Artifacts

**Why:** Enables debugging CI-specific failures that don't reproduce locally

**Implementation:**

```yaml
# Upload build artifacts on failure or success
- name: Upload build artifacts
  uses: actions/upload-artifact@v4
  with:
    name: build-artifacts-${{ github.sha }}
    path: packages/*/dist
    retention-days: 7
  if: always() # Upload even if build fails

# Upload container logs for E2E failures
- name: Upload container logs on failure
  if: failure() # Only upload if job fails
  uses: actions/upload-artifact@v4
  with:
    name: e2e-container-logs-${{ github.sha }}
    path: |
      /tmp/*.log
      packages/connector/test/integration/logs/*.log
    retention-days: 7

# Upload coverage reports (sent to Codecov)
- name: Upload coverage to Codecov
  uses: codecov/codecov-action@v4
  with:
    files: ./coverage/lcov.info
  continue-on-error: true # Don't fail CI if Codecov upload fails
```

**Accessing Artifacts:**

1. Go to GitHub Actions → Failed workflow run
2. Scroll to "Artifacts" section
3. Download artifact ZIP file

**Reference:** [.github/workflows/ci.yml artifact uploads]

---

### Practice 4: Version Pinning and Retries

**Why:** Ensures reproducible builds and handles transient network failures

**Implementation:**

```yaml
# Pin Node.js version for consistency
- name: Setup Node.js
  uses: actions/setup-node@v4
  with:
    node-version: '20.11.0'  # Exact version, not '20.x'

# Use retry action for network-dependent steps
- name: Install dependencies with retry
  uses: nick-fields/retry@v3
  with:
    timeout_minutes: 10
    max_attempts: 3
    command: npm ci --verbose

# Pin GitHub Actions versions
uses: actions/checkout@v4      # Not @latest
uses: actions/setup-node@v4    # Not @latest
uses: actions/upload-artifact@v4
```

**Benefits:**

- Consistent Node.js version across all jobs and developers
- Retry action handles intermittent npm registry failures
- Pinned action versions prevent breaking changes

**Retry Strategy:** 3 attempts with 10-minute timeout per attempt

**Reference:** [.github/workflows/ci.yml retry strategy lines 81-86]

---

### Practice 5: Environment Verification

**Why:** Confirms environment setup before running commands, easier debugging

**Implementation:**

```yaml
# Verify Node and npm versions
- name: Verify Node and npm versions
  run: |
    echo "Node version: $(node --version)"
    echo "npm version: $(npm --version)"
    echo "PATH: $PATH"

# Check workspace setup after installation
- name: Verify workspace setup
  run: |
    echo "Checking workspace configuration..."
    npm query ".workspace" | head -20 || true
    ls -la node_modules/.bin/ | head -20 || true

# Validate build artifacts after build step
- name: Verify dist artifacts
  run: |
    test -d packages/shared/dist && echo "✓ shared built" || exit 1
    test -d packages/connector/dist && echo "✓ connector built" || exit 1
    test -d packages/dashboard/dist && echo "✓ dashboard built" || exit 1

# Use health checks before E2E tests (in docker-compose.yml)
healthcheck:
  test: ["CMD", "curl", "-f", "http://localhost:8080/health"]
  interval: 10s
  timeout: 5s
  retries: 5
```

**Benefits:**

- Easier to debug environment-specific issues
- Confirms dependencies installed correctly
- Validates outputs before subsequent steps depend on them

**Reference:** [.github/workflows/ci.yml verification steps]

---

## Investigation Runbook

### General Debugging Workflow

Follow this systematic approach when investigating CI failures:

#### Step 1: Identify Failed Job

1. Go to GitHub repository → Actions tab
2. Click on failed workflow run
3. Review CI status summary (red X indicates failure)
4. Note which job(s) failed (lint-and-format, test, build, etc.)

#### Step 2: Review Job Logs for Error Messages

1. Click on failed job name
2. Expand failed step (marked with red X)
3. Scroll to error messages (usually at end of step output)
4. Copy error message and stack trace

**Example:**

```
Error: Command failed: npm test -- settlement-executor.test.ts
  ● SettlementExecutor › should deposit additional funds
    expect(jest.fn()).toHaveBeenCalled()
```

#### Step 3: Reproduce Locally with Same Commands

1. Find exact command from CI logs (e.g., `npm test -- --coverage`)
2. Run same command locally in project directory
3. Compare local output to CI output

**Example:**

```bash
# From CI logs: npm test -- settlement-executor.test.ts
npm test -- settlement-executor.test.ts

# Add verbose flag for more details
npm test -- settlement-executor.test.ts --verbose
```

**If local run succeeds but CI fails:**

- Environment difference (Node.js version, OS, dependencies)
- Run with exact Node.js version: `nvm use 20.11.0`
- Clean install dependencies: `rm -rf node_modules && npm ci`

#### Step 4: Check Recent Commits for Related Changes

1. Go to GitHub repository → Commits
2. Review recent commit messages
3. Click on commit SHA to see diff
4. Look for changes in:
   - Source files related to failure
   - Test files
   - Configuration files (tsconfig.json, .eslintrc.json, package.json)

**Example:**

```bash
# View recent commits
git log --oneline -10

# Show changes in specific commit
git show <commit-sha>

# Check if specific file changed recently
git log --oneline -- packages/connector/src/core/connector-node.ts
```

#### Step 5: Review Uploaded Artifacts if Available

1. Scroll to bottom of workflow run page
2. Check "Artifacts" section
3. Download relevant artifact (e.g., `e2e-container-logs-abc123`)
4. Extract ZIP and review logs

**Available Artifacts:**

- `build-artifacts-{sha}`: Built package dist/ folders
- `e2e-container-logs-{sha}`: Docker container logs from E2E tests
- Coverage reports (sent to Codecov, not downloadable)

---

### Diagnostic Commands Reference

Run these commands locally to mirror CI workflow behavior:

```bash
# Install dependencies (mirrors CI clean install)
npm ci

# Lint checks
npm run lint                    # Run ESLint
npm run lint --verbose          # Verbose output
npm run lint -- --fix           # Auto-fix violations

# Format checks
npm run format:check            # Check formatting (no changes)
npm run format                  # Auto-format all files

# Tests
npm test -- --coverage          # Run tests with coverage
npm test -- my-test.test.ts     # Run specific test file
npm test -- --verbose           # Verbose test output
npm test -- --runInBand         # Sequential execution
npm test -- --detectOpenHandles # Detect resource leaks

# Build
npm run build                   # Build all packages
npm run build --workspace=packages/shared # Build specific package

# Type check
npx tsc --noEmit -p packages/connector/tsconfig.json

# Solidity tests (in packages/contracts/)
cd packages/contracts
forge test                      # Run Foundry tests
forge test -vvv                 # Verbose output
forge coverage --ir-minimum     # Coverage report

# E2E tests
npm test --workspace=packages/connector -- e2e-full-system.test.ts
```

**Note:** Always use `npm ci` (not `npm install`) to mirror CI's clean dependency installation.

---

### Log Locations and Artifacts

**GitHub Actions Job Output:**

1. Go to Actions tab → Click workflow run
2. Click job name (e.g., "Test (Node.js 20.11.0)")
3. Expand step to see output
4. Use search (Cmd/Ctrl+F) to find error messages

**Build Artifacts:**

- Location: Workflow run page → "Artifacts" section
- File: `build-artifacts-{sha}.zip`
- Contains: `packages/*/dist` folders
- Retention: 7 days

**E2E Container Logs:**

- Location: Workflow run page → "Artifacts" section (only on E2E failure)
- File: `e2e-container-logs-{sha}.zip`
- Contains: Docker container logs (`/tmp/*.log`, `test/integration/logs/*.log`)
- Retention: 7 days

**Coverage Reports:**

- Location: Codecov.io (link in PR checks)
- Uploaded from: `coverage/lcov.info`
- Retention: Permanent (stored in Codecov)

---

### When to Escalate

Escalate CI issues in these scenarios:

**1. CI Infrastructure Issues**

- GitHub Actions outage (check [GitHub Status](https://www.githubstatus.com/))
- Runner unavailable or stuck (timeouts after 6 hours)
- Artifact upload failures (GitHub storage issues)

**Action:** Wait for GitHub to resolve, or contact GitHub Support

**2. Persistent Failures Across Multiple PRs**

- Same test fails in multiple unrelated PRs
- Systematic issue affecting all contributors
- Indicates broken main branch or flaky test

**Action:**

- Document root cause in `docs/qa/root-cause-analysis-{story}.md`
- Create story to fix systematic issue (like Epic 10)
- Update test-strategy-and-standards.md with anti-patterns

**3. Failures Only in CI, Not Reproducible Locally**

- Tests pass locally but fail in CI consistently
- Indicates environment difference (OS, Node.js version, timing)

**Action:**

- Check Node.js version match (use `nvm use 20.11.0`)
- Review CI environment variables
- Check for timing-dependent behavior (increase timeouts)
- Consider adding debug logging to CI workflow

**4. Security Audit Failures**

- npm audit reports critical vulnerabilities
- Dependencies have known security issues

**Action:**

- Review npm audit output for affected packages
- Update dependencies: `npm audit fix`
- If major version upgrade needed, test thoroughly
- Consider alternative packages if unmaintained

**Contact:**
Check project CONTRIBUTING.md for escalation contacts and issue reporting guidelines.

---

## Monitoring CI Health

Track these metrics to proactively identify CI/CD pipeline issues:

### Metrics to Track

**1. CI Pass Rate**

- **Metric:** Percentage of workflow runs that pass all jobs
- **Target:** >95% pass rate on main branch, >85% on PRs
- **Tool:** GitHub Actions insights

**How to Check:**

1. Go to Actions tab
2. Left sidebar → "Workflow runs"
3. Review success/failure ratio

**2. Flaky Test Detection**

- **Metric:** Tests that fail intermittently (pass on retry)
- **Target:** Zero flaky tests
- **Tool:** Manual tracking, stability test scripts

**How to Check:**

```bash
# Run stability test for suspicious test file
for i in {1..10}; do
  npm test -- my-test.test.ts || echo "Run $i FAILED"
done
```

**Action if Flaky:**

- Document in docs/qa/root-cause-analysis-{story}.md
- Fix using test anti-pattern guidelines
- Validate fix with 10x stability test run

**3. Average CI Execution Time**

- **Metric:** Total time from commit push to CI completion
- **Target:** <10 minutes for full CI suite
- **Tool:** GitHub Actions insights

**How to Check:**

1. Go to Actions tab → Workflow run
2. Note total duration (top right of workflow page)

**Action if Slow (>15 minutes):**

- Identify slowest job (expand each job to see duration)
- Optimize slow tests (parallelize, reduce setup time)
- Consider caching strategies (npm cache, Docker layer cache)

**4. Most Common Failure Types**

- **Metric:** Distribution of failures across jobs
- **Target:** No single job consistently failing
- **Tool:** Manual review of failed workflow runs

**Common Failure Distribution:**

- Lint: 10% (should be caught by pre-commit hooks)
- Test: 60% (most common, test quality focus)
- Build: 15% (type errors, usually caught in IDE)
- Type-check: 10% (strict mode violations)
- E2E: 5% (environment issues)

**Action if Imbalanced:**

- High lint failures: Improve pre-commit hook adoption
- High test failures: Review test quality, fix flaky tests
- High E2E failures: Improve Docker setup, add retries

### Tools

**GitHub Actions Insights:**

1. Go to repository → Actions tab
2. Left sidebar → "Workflow runs"
3. Filter by status (success, failure, cancelled)
4. Review trends over time

**Codecov for Test Coverage Trends:**

1. Visit Codecov.io dashboard for repository
2. Review coverage over time graph
3. Check coverage impact for each PR

**Manual Tracking:**

- Weekly review of CI failures in team meetings
- Document recurring issues in docs/qa/
- Track resolution time for CI issues

---

## Continuous Improvement Process

Use this systematic process to improve CI/CD pipeline reliability:

### Process Steps

**1. Identify Recurring CI Failures (Weekly Review)**

**How:**

- Review failed workflow runs from past week
- Group failures by type (lint, test, build, etc.)
- Identify patterns (same test failing, same job type)

**Criteria for "Recurring":**

- Same test fails 3+ times in different PRs
- Same job fails >20% of runs
- Failure impacts multiple contributors

**Example:**

```
Week of Jan 1-7, 2026:
- settlement-executor.test.ts failed 5 times across 3 PRs
- All failures: "expect(jest.fn()).toHaveBeenCalled()" assertion
- Pattern: Mock state issue with sequential calls
```

**2. Document Root Cause**

**Action:**
Create root cause analysis document: `docs/qa/root-cause-analysis-{story}.md`

**Template:**

```markdown
# Root Cause Analysis: {Story ID} - {Title}

## Failure Summary

- **Symptom:** Description of CI failure
- **Frequency:** How often it occurred
- **Impact:** Contributors affected, PRs blocked

## Root Cause

- **Cause:** Technical reason for failure
- **Example:** Code snippet demonstrating issue

## Resolution

- **Fix:** How issue was resolved
- **Prevention:** How to prevent recurrence

## References

- Links to failed workflow runs
- Links to PRs with fixes
```

**Example:** `docs/qa/root-cause-analysis-10.1.md` (Settlement Executor Test Failures)

**3. Add Preventive Measures**

**Action:**
Update `docs/architecture/test-strategy-and-standards.md` with anti-patterns

**Include:**

- Anti-pattern description
- Bad example (code demonstrating issue)
- Good example (code showing fix)
- Test validation approach

**Example:**
See Anti-Pattern 1: Inline bind(this) in Event Listeners (test-strategy-and-standards.md)

**4. Enhance CI Workflow if Needed**

**When to Update `.github/workflows/ci.yml`:**

- Add new package to monorepo → Add to build/test jobs
- New test type introduced → Create new job (e.g., performance tests)
- CI execution time exceeds 15 minutes → Optimize or parallelize
- Recurring failures from same root cause → Add preventive check
- New deployment target → Add deployment job

**Example Enhancement:**

```yaml
# Add pre-commit hook verification job
pre-commit-check:
  name: Verify Pre-Commit Hooks Installed
  runs-on: ubuntu-latest
  steps:
    - name: Check for pre-commit hook
      run: |
        if [ ! -f .husky/pre-commit ]; then
          echo "Error: pre-commit hook not found"
          exit 1
        fi
```

**5. Share Learnings with Team**

**Actions:**

- Present findings in team meeting
- Update developer documentation (developer-guide.md)
- Add entry to CHANGELOG.md
- Create preventive epic if systematic issue (like Epic 10)

**Example:**
Epic 10: CI/CD Pipeline Reliability & Test Quality

- Story 10.1: Fix test failures (root cause)
- Story 10.2: Add pre-commit hooks (prevention)
- Story 10.3: Document standards (knowledge sharing)

---

## When to Enhance CI Workflow

### Scenarios Requiring CI Changes

**Scenario 1: New Package Added to Monorepo**

**Trigger:** Add `packages/new-package/` to repository

**Required Changes:**

```yaml
# Add to build job
- name: Build all packages
  run: npm run build # Already builds all workspaces

# Verify new package artifact
- name: Verify dist artifacts
  run: |
    test -d packages/new-package/dist && echo "✓ new-package built" || exit 1

# Add to type-check job
- name: Type check all packages
  run: |
    npx tsc --noEmit -p packages/new-package/tsconfig.json
```

**Scenario 2: New Test Type Introduced**

**Trigger:** Add performance tests, security tests, or new test category

**Required Changes:**

```yaml
# Create new job for performance tests
performance-test:
  name: Performance Tests
  runs-on: ubuntu-latest
  steps:
    - name: Run performance tests
      run: npm run test:perf
    - name: Upload performance results
      uses: actions/upload-artifact@v4
      with:
        name: performance-results
        path: perf-results/
```

**Scenario 3: CI Execution Time Exceeds 15 Minutes**

**Trigger:** Total CI time (longest job) >15 minutes

**Optimization Strategies:**

- **Parallelize:** Split long job into multiple parallel jobs
- **Cache:** Add caching for dependencies, build artifacts
- **Selective:** Run expensive tests only on main branch, not PRs
- **Incremental:** Test only changed packages in monorepo

**Example:**

```yaml
# Split test job into multiple parallel jobs
test-connector:
  name: Test Connector Package
  run: npm test --workspace=packages/connector

test-dashboard:
  name: Test Dashboard Package
  run: npm test --workspace=packages/dashboard
```

**Scenario 4: Recurring Failures from Same Root Cause**

**Trigger:** Same failure pattern across multiple PRs

**Add Preventive Check:**

```yaml
# Example: Prevent commits without pre-commit hooks
- name: Verify pre-commit hook installed
  run: |
    if [ ! -f .husky/pre-commit ]; then
      echo "Error: Install pre-commit hooks with 'npm install'"
      exit 1
    fi
```

**Scenario 5: New Deployment Target**

**Trigger:** Deploy to new environment (staging, production, Docker registry)

**Add Deployment Job:**

```yaml
deploy-staging:
  name: Deploy to Staging
  runs-on: ubuntu-latest
  needs: [lint-and-format, test, build]
  if: github.ref == 'refs/heads/main' # Only on main branch
  steps:
    - name: Deploy to staging environment
      run: ./scripts/deploy-staging.sh
```

---

### Best Practices for CI Workflow Changes

**Practice 1: Test Workflow Changes in Feature Branch First**

**How:**

1. Create feature branch with workflow changes
2. Push to branch to trigger CI
3. Verify changes work as expected in CI logs
4. Merge to main after validation

**Example:**

```yaml
# Add experimental job that only runs on feature branches
experimental-job:
  if: github.ref != 'refs/heads/main'
  runs-on: ubuntu-latest
  steps:
    - name: Run experimental check
      run: ./scripts/experimental.sh
```

**Practice 2: Use Conditional Execution for Experimental Jobs**

**How:**

```yaml
# Run experimental job only on PRs, not main branch
experimental-job:
  if: github.event_name == 'pull_request'
  steps:
    - name: Experimental check
      run: ./scripts/experimental.sh
      continue-on-error: true # Don't fail CI if experimental job fails
```

**Practice 3: Monitor CI Execution Time After Changes**

**How:**

1. Note CI execution time before changes
2. Deploy workflow changes
3. Monitor next 10 workflow runs
4. Compare average execution time
5. Rollback if time increases >20%

**Practice 4: Document Workflow Changes in CHANGELOG.md**

**How:**

```markdown
## [Unreleased]

### Changed

- CI: Added performance test job to workflow (runs on main branch only)
- CI: Parallelized test job into connector and dashboard tests for faster execution
- CI: Increased E2E test timeout from 5 to 8 minutes
```

**Practice 5: Review GitHub Actions Best Practices**

**Resources:**

- [GitHub Actions Documentation](https://docs.github.com/actions)
- [GitHub Actions Best Practices](https://docs.github.com/en/actions/guides/best-practices-for-workflows)
- [Security hardening for GitHub Actions](https://docs.github.com/en/actions/security-guides/security-hardening-for-github-actions)

**Key Guidelines:**

- Pin action versions (@v4, not @latest)
- Use least privilege for tokens
- Validate inputs for workflow_dispatch
- Use caching to reduce execution time
- Set timeout-minutes to prevent runaway jobs

---

## Summary

This CI troubleshooting guide provides:

- **7 common failure scenarios** with diagnosis and resolution steps
- **8 job-specific debugging procedures** with diagnostic commands
- **5 CI workflow best practices** for reliability and performance
- **Investigation runbook** with step-by-step debugging workflow
- **Monitoring guidelines** for tracking CI health metrics
- **Continuous improvement process** for systematic issue resolution
- **Enhancement guidelines** for when and how to update CI workflow

**Key Takeaways:**

1. Use pre-commit/pre-push hooks to catch issues before CI (Story 10.2)
2. Follow test anti-patterns guide to prevent flaky tests (test-strategy-and-standards.md)
3. Download artifacts for CI-specific failures (build artifacts, container logs)
4. Document root causes for recurring failures (docs/qa/)
5. Review CI insights weekly to identify trends and improvements

**Related Documentation:**

- [Developer Guide](developer-guide.md) - Epic branch workflow, pre-push checklist
- [Git Hooks](git-hooks.md) - Pre-commit/pre-push hook configuration
- [Test Strategy and Standards](../architecture/test-strategy-and-standards.md) - Test anti-patterns and best practices
- [Coding Standards](../architecture/coding-standards.md) - TypeScript and Solidity standards
- [Root Cause Analyses](../qa/) - Documented failure investigations

For additional help, refer to CONTRIBUTING.md for escalation contacts.
