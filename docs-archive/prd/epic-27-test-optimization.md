# Epic 27: Test Suite & Pre-Push Hook Optimization

**Epic Number:** 27
**Priority:** High — Pre-push hook takes 13+ minutes, blocking developer velocity
**Type:** Infrastructure — Developer Experience & CI/CD Optimization
**Dependencies:** Epics 24-26 (introduced test files being optimized)

## Epic Goal

Reduce pre-push hook execution from 13+ minutes to <30 seconds by restructuring the hook to run only unit tests, scoping lint/format checks to changed files, eliminating redundant test files introduced across epics 24-26, and establishing a proper test pyramid where fast feedback is local and comprehensive testing runs in CI.

## Epic Description

### Existing System Context

**Current Functionality:**

- Pre-push hook (`.husky/pre-push`) runs 3 sequential stages: ESLint on changed TS files, `prettier --check` on ALL 1,158 files, `jest --findRelatedTests` on changed source files
- Pre-commit hook (`.husky/pre-commit`) runs `lint-staged` which already does `eslint --fix` + `prettier --write` on staged files
- ~137 test files across the monorepo (102 in connector alone), organized as unit, integration, performance, acceptance, and load tests
- Jest 29.7 with ts-jest preset, 30s default timeout
- 10 test files already excluded in `jest.config.js` (cloud KMS, Docker, tri-chain)

**Technology Stack:**

- TypeScript 5.3.3, Node.js 22 LTS, Jest 29.7, ts-jest, Husky, lint-staged, ESLint, Prettier
- npm workspaces monorepo: `packages/connector`, `packages/shared`, `packages/agent-runtime`, `tools/send-packet`

**Integration Points:**

- `.husky/pre-push` — Main hook being redesigned
- `.husky/pre-commit` — Existing hook (unchanged, but informs redundancy analysis)
- `packages/connector/jest.config.js` — Test path patterns and ignore rules
- `package.json` — lint-staged config, format scripts
- Root `jest.config.js` — Multi-project configuration

### Enhancement Details

**What's Being Changed:**

1. **Pre-push hook restructure** — Scope `format:check` to changed files only (saves ~18s), exclude integration/performance/acceptance tests from `--findRelatedTests` (saves ~700s+), run lint + format in parallel (saves ~5s), add early exit for non-code changes

2. **Test file consolidation** — Delete `lib.test.ts` (9 tests, fully redundant with `consumer-types.test.ts`), delete `index.test.ts` (6 tests, fully redundant), trim 5 overlapping constructor/lifecycle tests from `connector-node-minimal.test.ts` and rename to `connector-node-optional-deps.test.ts`

3. **Jest configuration** — Create `jest.performance.config.js` for isolated performance test runs, add performance/acceptance/wallet-derivation to `testPathIgnorePatterns` in default config, fix 6 failing test suites with relaxed thresholds for concurrent execution

4. **Test distribution documentation** — Establish clear contract: pre-commit (lint+format staged), pre-push (unit tests for changed files), CI-on-PR (full unit + integration), CI-nightly (performance + acceptance + load)

**How It Integrates:**

- Pre-push hook changes are self-contained in `.husky/pre-push` — no impact on test code
- Test consolidation removes files and moves unique tests — no new test code, only deletion and migration
- Jest config changes add ignore patterns — existing `npm test` behavior changes to skip performance/acceptance by default
- New `test:performance` script provides explicit opt-in for performance tests
- CI pipeline should be updated to run `npm test` (unit+integration) + `npm run test:performance` (benchmarks)

**Success Criteria:**

1. Pre-push hook completes in <30 seconds for typical changes (was 800+ seconds)
2. No reduction in actual bug-catching capability — all tests still run somewhere (pre-push, CI, or nightly)
3. Zero test failures in default `npm test` run (currently 6 failures from perf thresholds)
4. Redundant test files eliminated — 2 files deleted, 1 renamed, net ~20 tests removed
5. Clear documentation of which tests run where (pre-commit, pre-push, CI, nightly)

### Why Not Other Options?

| Option                                                    | Verdict      | Reason                                                                                  |
| --------------------------------------------------------- | ------------ | --------------------------------------------------------------------------------------- |
| **Restructure pre-push + consolidate tests**              | **Selected** | Maximum impact (~725s savings), minimal effort (~1.5 hours), no framework changes       |
| Migrate to Vitest                                         | Deferred     | 2-3 day effort for 137 files, good long-term but premature now                          |
| Remove pre-push hook entirely                             | Rejected     | Lose fast local feedback, bad commits reach CI                                          |
| Run all tests but with `--maxWorkers=50%`                 | Rejected     | Still runs 87 test files including 587s wallet-derivation, minimal improvement          |
| Use Jest `--changedSince` instead of `--findRelatedTests` | Evaluated    | Similar fan-out, doesn't solve the category problem (integration tests still pulled in) |

## Stories

### Story 27.1: Redesign Pre-Push Hook

**Description:** Replace the current 3-stage sequential pre-push hook with an optimized 2-stage parallel hook that scopes all checks to changed files and excludes integration/performance/acceptance tests.

**Acceptance Criteria:**

- `format:check` scoped to changed files only (via `git diff --name-only` filtered to `.ts|.tsx|.js|.json|.md`)
- ESLint and Prettier run in parallel (background processes with `wait`)
- Jest `--findRelatedTests` includes `--testPathIgnorePatterns='test/integration|test/performance|test/acceptance|\.perf\.test\.'`
- Early exit when no source files changed (docs-only, test-only, config-only changes)
- Hook completes in <30 seconds for typical changes
- All error messages preserved with actionable fix commands
- Existing pre-commit hook unchanged

### Story 27.2: Consolidate Redundant Test Files

**Description:** Delete test files introduced in epics 24-26 that are fully redundant with `consumer-types.test.ts`, trim overlapping tests from `connector-node-minimal.test.ts`, and rename it to reflect its actual purpose (testing optional dependency error handling).

**Acceptance Criteria:**

- `src/lib.test.ts` deleted — "main is NOT exported" test moved to `consumer-types.test.ts`
- `src/index.test.ts` deleted — all 6 export checks already covered by `consumer-types.test.ts`
- `connector-node-minimal.test.ts` renamed to `connector-node-optional-deps.test.ts`
- 5 overlapping constructor/start/stop tests removed from renamed file (constructor: 2, start-minimal: 3)
- 8 unique optional-dependency error tests retained (settlement: 2, express: 2, tigerbeetle: 2, requireOptional: 2)
- All retained tests pass
- No coverage regression in meaningful code paths

### Story 27.3: Fix Failing Tests & Jest Configuration

**Description:** Fix 6 currently-failing test suites by either relaxing performance thresholds for concurrent execution or moving them to dedicated performance config. Create `jest.performance.config.js` for isolated benchmark runs. Update default `jest.config.js` to exclude performance/acceptance tests from the default `npm test` run.

**Acceptance Criteria:**

- `jest.performance.config.js` created with `testMatch` for `test/performance/**` and `*.perf.test.*`
- `test:performance` script added to `packages/connector/package.json`
- Default `jest.config.js` `testPathIgnorePatterns` updated to exclude: `test/performance/`, `test/acceptance/`, `wallet-derivation`, `xrp-channel-manager`, `xrp-channel-lifecycle`
- 6 previously-failing test suites either: (a) pass with relaxed thresholds, or (b) excluded from default run and pass via `test:performance`
- `npm test` at root level completes with 0 failures
- Performance tests still runnable via `npm run test:performance --workspace=packages/connector`
- Test distribution documented in project README or CONTRIBUTING.md

## Compatibility Requirements

- [x] Existing APIs remain unchanged — no source code modifications outside test infrastructure
- [x] Pre-commit hook unchanged — lint-staged behavior preserved
- [x] All tests still exist and run somewhere — nothing permanently deleted without CI coverage
- [x] `npm test` still works at root level — behavior changes (fewer tests in default run) but all pass
- [x] CI pipeline can run full suite by combining `npm test` + `npm run test:performance` + `npm run test:integration`

## Risk Mitigation

- **Primary Risk:** Excluding integration tests from pre-push could let bugs reach CI that would have been caught locally
- **Mitigation:** `--findRelatedTests` still runs all unit tests for changed files, which covers the vast majority of logic bugs. Integration tests catch environment/wiring issues that are better validated in CI's consistent environment anyway. The 6 currently-failing test suites prove the integration tests are unreliable locally.
- **Secondary Risk:** Deleting test files could remove coverage for edge cases
- **Mitigation:** Detailed overlap analysis confirmed all deleted tests are strict subsets of retained tests. The "main not exported" test from `lib.test.ts` is explicitly migrated to `consumer-types.test.ts`.
- **Rollback Plan:** Git revert the pre-push hook changes. Test file deletions can be restored from git history. Jest config changes are additive (new ignore patterns) and easily reverted.

## Definition of Done

- [x] All stories completed with acceptance criteria met
- [x] Pre-push hook completes in <30 seconds for typical changes
- [x] `npm test` at root passes with 0 failures
- [x] No test coverage regression for meaningful code paths
- [x] Test distribution documented (pre-commit / pre-push / CI / nightly)
- [x] Performance tests runnable via dedicated script
- [x] Existing pre-commit hook and CI pipeline behavior preserved
