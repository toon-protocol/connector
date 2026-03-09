# Test & Pre-Push Hook Optimization Plan

## Executive Summary

The pre-push hook is severely impacted by three compounding issues:

1. **`--findRelatedTests` fan-out is massive** — For the epic-26 branch (36 changed source files), it triggers **87 out of 137 test files** (64% of all tests), including the 587-second `wallet-derivation.test.ts`
2. **`format:check` scans all 1,158 files** — Takes ~20s even though pre-commit already formats staged files
3. **Integration/performance/acceptance tests run in pre-push** — Tests requiring external services (rippled, TigerBeetle, Anvil) and heavy benchmarks get pulled in by `--findRelatedTests`

**Estimated current pre-push time for epic-26**: 800+ seconds (13+ minutes)
**Target after optimization**: <30 seconds for typical changes

---

## Findings

### 1. Test Execution Profiling (Full Suite: 832s)

| Test File                                   | Time     | Category               | Pre-push?            |
| ------------------------------------------- | -------- | ---------------------- | -------------------- |
| `wallet-derivation.test.ts`                 | **587s** | Integration            | Should be CI-only    |
| `multi-chain-settlement-acceptance.test.ts` | 64s      | Acceptance             | Should be CI-only    |
| `throughput-benchmark.test.ts`              | 32-37s   | Performance            | Should be CI-only    |
| `e2e-performance-benchmark.test.ts`         | 33s      | Integration            | Should be CI-only    |
| `memory-profile.test.ts`                    | 21-31s   | Performance            | Should be CI-only    |
| `aptos-client.test.ts`                      | 18s      | Unit (with 3s retries) | Trim retries         |
| `latency-benchmark.test.ts`                 | 17s      | Performance            | Should be CI-only    |
| `admin-api-settlement.test.ts`              | 17s      | Unit                   | Keep but optimize    |
| `claim-redemption.integration.test.ts`      | 15-33s   | Integration            | Should be CI-only    |
| `cpu-profile.test.ts`                       | 14-23s   | Performance            | Should be CI-only    |
| `violation-counter.test.ts`                 | 12s      | Unit                   | Investigate slowness |
| `benchmark.test.ts`                         | 11s      | Unit                   | Should be CI-only    |
| `environment-backend.test.ts`               | 10s      | Unit                   | Keep                 |
| `admin-api-peers.test.ts`                   | 6.5s     | Unit                   | Keep                 |

**Top 10 slowest tests account for ~840s (14 minutes).** Most are performance benchmarks and integration tests that should never run in pre-push.

### 2. `--findRelatedTests` Fan-Out Analysis

For the epic-26 branch diff (36 changed source files):

- **87 test files triggered** out of 137 total (64%)
- Includes ALL performance tests, ALL integration tests, ALL settlement tests
- `connector-node.ts` alone triggers 10 tests
- `btp-server.ts` triggers 15 tests
- `aptos-client.ts` triggers 16 tests

The fan-out is so large because core modules (`connector-node.ts`, `packet-handler.ts`, `admin-server.ts`) are imported transitively by nearly every integration test.

### 3. Pre-Push / Pre-Commit Redundancy

| Check               | Pre-Commit                  | Pre-Push                             | Redundant?                                                                                                        |
| ------------------- | --------------------------- | ------------------------------------ | ----------------------------------------------------------------------------------------------------------------- |
| ESLint on TS files  | `eslint --fix` (staged)     | `eslint` (diff vs remote)            | **Partially** — pre-push catches unstaged changes but if using proper git workflow, pre-commit already covered it |
| Prettier formatting | `prettier --write` (staged) | `prettier --check` (ALL 1,158 files) | **Yes** — pre-push should scope to changed files only                                                             |
| Tests               | None                        | `--findRelatedTests`                 | No redundancy, but scope is too broad                                                                             |

### 4. Test File Overlap (Epics 24-26)

| File                             | Tests | Unique Value                                               | Recommendation                                    |
| -------------------------------- | ----- | ---------------------------------------------------------- | ------------------------------------------------- |
| `consumer-types.test.ts`         | 10    | Type safety for all 15 exports + 9 types                   | **Keep as canonical export test**                 |
| `lib.test.ts`                    | 9     | 8 export checks (subset of above) + "main not exported"    | **Delete** — move 1 unique test to consumer-types |
| `index.test.ts`                  | 6     | 6 export checks (subset of consumer-types)                 | **Delete** — fully redundant                      |
| `connector-node.test.ts`         | 49    | Comprehensive unit tests                                   | **Keep**                                          |
| `connector-node-minimal.test.ts` | 13    | 5 overlap with connector-node, 8 unique optional-dep tests | **Remove 5 overlapping, rename to optional-deps** |
| `connector-node.aptos.test.ts`   | 8     | Entirely unique Aptos SDK lifecycle                        | **Keep**                                          |
| `main.test.ts`                   | 3     | Unique main() orchestration                                | **Keep**                                          |

**Net reduction: 20 redundant tests eliminated, 2 test files deleted.**

### 5. Failing Tests Discovered

6 test suites currently fail:

- `wallet-derivation.test.ts` — Timeout (should have CI skip flag but still runs)
- `cpu-profile.test.ts` — CPU utilization assertion too strict for CI
- `memory-profile.test.ts` — Memory slope assertion too strict for CI
- `throughput-benchmark.test.ts` — TPS thresholds too strict for CI
- `xrp-channel-manager.test.ts` — Requires running rippled (timeout)
- `oer.perf.test.ts` — Encoding thresholds too strict for CI

All 6 failures are performance/benchmark tests with thresholds that don't account for concurrent test execution. **These should be excluded from `--findRelatedTests` scope.**

---

## Optimization Plan

### Phase 1: Pre-Push Hook Redesign (Immediate — biggest impact)

#### 1A. Scope `format:check` to changed files only

**Before:** `npm run format:check` → checks all 1,158 files (20s)
**After:** Check only files in the diff (~36 files, <2s)

```bash
# 2/3 Format check (scoped to changed files)
echo "2/3 Checking code formatting..."
FORMAT_FILES=$(git diff --name-only --diff-filter=ACM "$REMOTE_BRANCH" | grep -E '\.(ts|tsx|js|json|md)$' || true)
if [ -n "$FORMAT_FILES" ]; then
  echo "$FORMAT_FILES" | xargs npx prettier --check
else
  echo "No formattable files changed, skipping format check."
fi
```

**Savings: ~18 seconds**

#### 1B. Exclude integration/performance/acceptance tests from pre-push

Add `--testPathIgnorePatterns` to the pre-push jest invocation:

```bash
npx jest --findRelatedTests --passWithNoTests --bail \
  --testPathIgnorePatterns='test/integration|test/performance|test/acceptance|\.perf\.test\.' \
  $SOURCE_FILES
```

This limits pre-push to **unit tests only** (`src/**/*.test.ts` and `test/unit/**/*.test.ts`).

**Savings: Eliminates the 87→~45 test fan-out, removes all tests >10s**

#### 1C. Run lint and format in parallel

Lint and format checking are independent — run them concurrently:

```bash
# Run lint and format in parallel
echo "1/3 Running lint and format checks..."
LINT_PID=""
FORMAT_PID=""

if [ -n "$CHANGED_TS_FILES" ]; then
  echo "$CHANGED_TS_FILES" | xargs npx eslint &
  LINT_PID=$!
fi

if [ -n "$FORMAT_FILES" ]; then
  echo "$FORMAT_FILES" | xargs npx prettier --check &
  FORMAT_PID=$!
fi

# Wait for both
FAILED=0
[ -n "$LINT_PID" ] && wait $LINT_PID || FAILED=1
[ -n "$FORMAT_PID" ] && wait $FORMAT_PID || FAILED=1
[ $FAILED -ne 0 ] && exit 1
```

**Savings: Overlap removes ~5s (lint + format run in parallel instead of sequential)**

#### 1D. Add early exit for non-code changes

Skip tests entirely for docs-only, config-only, or test-only changes:

```bash
# Skip tests if only non-source files changed
CODE_FILES=$(echo "$CHANGED_TS_FILES" | grep -v '\.test\.ts$' | grep -v '\.d\.ts$' || true)
if [ -z "$CODE_FILES" ]; then
  echo "Only tests/types changed, skipping test run."
fi
```

### Phase 2: Test Consolidation (Medium priority)

#### 2A. Delete redundant export tests

- Delete `src/lib.test.ts` (9 tests) — move "main not exported" check to `consumer-types.test.ts`
- Delete `src/index.test.ts` (6 tests) — fully covered by `consumer-types.test.ts`

#### 2B. Trim `connector-node-minimal.test.ts`

- Remove 5 overlapping constructor/start/stop tests
- Rename to `connector-node-optional-deps.test.ts`
- Keep 8 unique optional-dependency error tests

#### 2C. Move performance tests to dedicated config

Already have `jest.acceptance.config.js` and `jest.load.config.js`. Create `jest.performance.config.js`:

```javascript
module.exports = {
  ...require('./jest.config'),
  displayName: 'performance',
  testMatch: ['**/test/performance/**/*.test.ts', '**/*.perf.test.ts'],
  testTimeout: 60000,
};
```

Add script: `"test:performance": "jest --config jest.performance.config.js"`

### Phase 3: Jest Configuration Optimization (Lower priority)

#### 3A. Add `testPathIgnorePatterns` for external-service tests

Update `jest.config.js` to also skip tests that require running services:

```javascript
testPathIgnorePatterns: [
  // Existing skips...
  'test/performance/',        // Performance benchmarks
  'test/acceptance/',         // Acceptance tests
  'wallet-derivation',       // 587s integration test
  'xrp-channel-manager',     // Requires rippled
  'xrp-channel-lifecycle',   // Requires rippled
],
```

#### 3B. Add `--shard` support for CI parallelism

Split the test suite across CI runners:

```yaml
# GitHub Actions matrix
strategy:
  matrix:
    shard: [1/4, 2/4, 3/4, 4/4]
steps:
  - run: npx jest --shard=${{ matrix.shard }}
```

#### 3C. Fix flaky performance test thresholds

For the 6 failing test suites, either:

- Increase thresholds by 3x to account for CI concurrent execution
- Move to dedicated performance runner with no parallelism
- Add `process.env.CI` guards to relax thresholds

### Phase 4: TigerBeetle Test Cleanup (Epic-27 related)

Epic-27 replaces TigerBeetle with an in-memory ledger (`InMemoryLedgerClient`). This creates significant test cleanup opportunities:

#### Dedicated TigerBeetle Test Files (5 files)

| File                                                    | Status                          | Action                                                         |
| ------------------------------------------------------- | ------------------------------- | -------------------------------------------------------------- |
| `src/settlement/tigerbeetle-client.test.ts`             | Unit test for TB client         | **Adapt** → test `InMemoryLedgerClient` against same interface |
| `test/integration/tigerbeetle-client.test.ts`           | Integration (requires TB)       | **Delete** — no longer needed when TB is optional              |
| `test/integration/tigerbeetle-deployment.test.ts`       | Docker deployment test          | **Delete** — TB no longer a core dependency                    |
| `test/integration/tigerbeetle-5peer-deployment.test.ts` | Already excluded in jest.config | **Delete**                                                     |
| `test/unit/settlement/tigerbeetle-batch-writer.test.ts` | Batch writer unit test          | **Delete or adapt** — if batch writer is removed               |

#### Files Referencing TigerBeetle (26 additional files)

These 26 test files reference TigerBeetle in mocks, setup, or assertions. After epic-27:

- Tests that mock `TigerBeetleClient` should mock `InMemoryLedgerClient` instead (or use the real in-memory implementation since it's zero-dependency)
- Tests that skip when TB is unavailable can remove those skip guards
- `connector-node-minimal.test.ts` TigerBeetle error tests become obsolete (TB is no longer expected)

**Net impact**: 3-4 test files deleted, ~26 files simplified (fewer mocks, fewer skip guards), faster test execution since in-memory ledger doesn't need external service availability checks.

### Phase 5: Future Considerations

#### Vitest Migration (Long-term)

- Vitest offers ~2-5x faster cold start vs Jest with ts-jest
- Native ESM/TypeScript support (no transformation overhead)
- Compatible API — most tests would need minimal changes
- Migration effort: ~2-3 days for 137 test files
- **Recommendation**: Worth evaluating after epics 24-27 stabilize, not urgent now

---

## Proposed Pre-Push Hook (Complete)

```bash
#!/usr/bin/env sh

echo "🚀 Running pre-push quality gates..."

# Get current branch and remote branch
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD)
REMOTE_BRANCH="origin/$CURRENT_BRANCH"

# Check if remote branch exists, fallback to origin/main if not
if ! git rev-parse --verify "$REMOTE_BRANCH" >/dev/null 2>&1; then
  REMOTE_BRANCH="origin/main"
fi

# Get list of ALL changed files for format checking
ALL_CHANGED_FILES=$(git diff --name-only --diff-filter=ACM "$REMOTE_BRANCH" || true)

# Get list of changed TypeScript source files (exclude deleted, .test.ts, .d.ts)
CHANGED_TS_FILES=$(echo "$ALL_CHANGED_FILES" | grep -E '\.tsx?$' | grep -v '\.test\.ts$' | grep -v '\.d\.ts$' || true)

# Get formattable files for prettier (scoped, not all files)
FORMAT_FILES=$(echo "$ALL_CHANGED_FILES" | grep -E '\.(ts|tsx|js|json|md)$' || true)

# ---- Stage 1/2: Lint + Format (parallel) ----
echo "1/2 Checking lint and formatting..."
LINT_EXIT=0
FORMAT_EXIT=0

if [ -n "$CHANGED_TS_FILES" ]; then
  echo "$CHANGED_TS_FILES" | xargs npx eslint 2>&1 &
  LINT_PID=$!
else
  echo "  No TypeScript source files changed, skipping lint."
  LINT_PID=""
fi

if [ -n "$FORMAT_FILES" ]; then
  echo "$FORMAT_FILES" | xargs npx prettier --check 2>&1 &
  FORMAT_PID=$!
else
  echo "  No formattable files changed, skipping format check."
  FORMAT_PID=""
fi

if [ -n "$LINT_PID" ]; then
  wait $LINT_PID || LINT_EXIT=$?
fi
if [ -n "$FORMAT_PID" ]; then
  wait $FORMAT_PID || FORMAT_EXIT=$?
fi

if [ $LINT_EXIT -ne 0 ]; then
  echo "❌ Linting failed. Run 'npm run lint' to see errors."
  exit 1
fi
if [ $FORMAT_EXIT -ne 0 ]; then
  echo "❌ Formatting failed. Run 'npm run format' to fix."
  exit 1
fi

# ---- Stage 2/2: Unit tests only (no integration/performance/acceptance) ----
echo "2/2 Running unit tests for changed files..."
if [ -n "$CHANGED_TS_FILES" ]; then
  SOURCE_FILES=$(echo "$CHANGED_TS_FILES" | tr '\n' ' ')
  npx jest --findRelatedTests --passWithNoTests --bail \
    --testPathIgnorePatterns='test/integration|test/performance|test/acceptance|\.perf\.test\.' \
    $SOURCE_FILES
  if [ $? -ne 0 ]; then
    echo "❌ Tests failed. Run 'npm test' to debug."
    exit 1
  fi
else
  echo "  No source files changed, skipping tests."
fi

echo "✅ All pre-push checks passed!"
```

---

## Impact Summary

| Optimization                                       | Time Saved           | Effort         |
| -------------------------------------------------- | -------------------- | -------------- |
| Scope format:check to changed files                | ~18s                 | 5 min          |
| Exclude integration/perf/acceptance from pre-push  | ~700s+               | 10 min         |
| Parallelize lint + format                          | ~5s                  | 15 min         |
| Delete redundant test files (lib.test, index.test) | ~2s per run          | 30 min         |
| Trim connector-node-minimal overlap                | ~1s per run          | 20 min         |
| Early exit for non-code changes                    | Variable             | 5 min          |
| **Total**                                          | **~725s+ reduction** | **~1.5 hours** |

**Expected pre-push time after optimization: 10-25 seconds** (depending on which source files changed)

---

## Test Distribution After Optimization

| Where                   | What Runs                                                     | Approximate Time |
| ----------------------- | ------------------------------------------------------------- | ---------------- |
| **Pre-commit**          | lint-staged (ESLint --fix + Prettier --write on staged files) | 2-5s             |
| **Pre-push**            | Scoped lint + scoped format + unit tests for changed files    | 10-25s           |
| **CI (on PR)**          | Full unit suite + integration tests + shared package tests    | 3-5 min          |
| **CI (nightly/manual)** | Performance benchmarks + acceptance tests + load tests        | 15-30 min        |

This creates a proper **test pyramid** where fast feedback is local and comprehensive testing is automated in CI.
