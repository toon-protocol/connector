# Developer Guide

## Getting Started

This guide covers essential development workflows for the M2M (Multi-node Interledger Connector) project.

---

## Development Environment Setup

### Node.js Version Management

This project requires **Node.js v22.11.0** to match the CI/CD environment.

**Using nvm (recommended):**

```bash
# Install and use the correct Node version
nvm install 22.11.0
nvm use 22.11.0

# Or simply use the .nvmrc file
nvm use
```

The project includes a `.nvmrc` file that specifies the required Node version. Many tools (nvm, IDEs) will automatically detect and use this version.

**Verify your Node version:**

```bash
node --version  # Should output: v22.11.0
npm --version   # Should be 10.x or higher
```

**Why this matters:**

- **Local/CI consistency:** Ensures your local environment matches CI (ubuntu-latest with Node 22.11.0)
- **Prevents version-specific bugs:** Avoids issues that only appear in CI
- **Modern features:** v22.x provides latest Node.js features and performance improvements

**Without nvm:**

Download Node.js v22.11.0 directly from [nodejs.org](https://nodejs.org/download/release/v22.11.0/)

---

## Local Quality Checks with Git Hooks

This project uses Git hooks to enforce quality standards before code reaches CI/CD. Hooks run automatically and catch issues early.

### Quick Reference

| Hook       | Trigger      | What It Checks                              | Bypass Command           |
| ---------- | ------------ | ------------------------------------------- | ------------------------ |
| pre-commit | `git commit` | Lint & format staged files                  | `git commit --no-verify` |
| pre-push   | `git push`   | Lint, format, related tests (changed files) | `git push --no-verify`   |

### Pre-Commit Hook

- **When:** Runs on `git commit`
- **Checks:** ESLint + Prettier on staged files only
- **Speed:** 2-5 seconds
- **Auto-fixes:** Yes (eslint --fix, prettier --write)

### Pre-Push Hook

- **When:** Runs on `git push`
- **Checks:** Lint, format, and related tests on changed files
- **Speed:** 10-30 seconds
- **Auto-fixes:** Lint/format only (tests must pass)

### Bypassing Hooks

Use `--no-verify` only for:

- WIP commits on feature branches
- Emergency hotfixes (document why)
- Temporary debugging commits

**Never bypass for:**

- PRs to main/epic branches
- Production-bound code

**Detailed documentation:** [Git Hooks Workflow](./git-hooks.md)

---

## Epic Branch Workflow

Epic branches consolidate multiple stories into larger features before merging to main. This workflow ensures all epic stories are tested together and prevents integration issues.

### Overview

**Branch Strategy:**

- Epic branches created from `main` (e.g., `epic-8`, `epic-10`)
- Story branches created from epic branch (e.g., `story-8.1`, `story-10.2`)
- Merge order: story → epic branch → main via PR

**Example:**

```
main
 └── epic-10
      ├── story-10.1 (merged)
      ├── story-10.2 (merged)
      └── story-10.3 (merged)
```

After all stories complete, create PR: `epic-10` → `main`

### Creating Epic Branch PRs

**When to Create:**
After all epic stories are completed and merged to the epic branch.

**Pre-PR Checklist (CRITICAL):**

Before creating an epic branch PR, **verify all of the following locally:**

- [ ] All story branches merged to epic branch
- [ ] Pre-commit hooks passed locally on final commit
- [ ] Pre-push hooks passed (lint, format, related tests)
- [ ] **Full test suite runs successfully:**
  ```bash
  npm test --workspaces --if-present
  ```
- [ ] **All packages build successfully:**
  ```bash
  npm run build
  ```
- [ ] **Type check passes for all packages:**
  ```bash
  npx tsc --noEmit -p packages/shared/tsconfig.json
  npx tsc --noEmit -p packages/connector/tsconfig.json
  npx tsc --noEmit -p packages/dashboard/tsconfig.json
  ```
- [ ] Integration tests run (if applicable, currently skipped in CI)
- [ ] **CHANGELOG.md updated** with epic changes
- [ ] Architecture documentation updated if epic changes system design

**PR Description:**
Use `.github/PULL_REQUEST_TEMPLATE.md` template to ensure quality checklist is followed.

### Epic Branch Quality Standards

Epic branch PRs must meet these standards:

**Zero Tolerance:**

- ✅ All tests pass (no failures allowed)
- ✅ No lint/format violations (pre-commit hooks enforce)
- ✅ No TypeScript compilation errors
- ✅ No regression in test coverage

**Coverage Requirements:**

- `packages/connector`: >80% line coverage
- `packages/shared`: >90% line coverage
- `packages/dashboard`: >70% line coverage

**Acceptance Criteria:**

- All epic story acceptance criteria met
- Definition of Done checklist completed
- QA gates passed for all stories

### Handling Epic Branch PR Failures

**If CI fails on epic branch PR:**

1. **Review Failed Job Logs**
   - See [CI Troubleshooting Guide](ci-troubleshooting.md) for job-specific debugging
   - Identify which job failed (lint, test, build, type-check, etc.)
   - Note specific error messages

2. **Reproduce Failure Locally**

   ```bash
   # Run exact command from CI logs
   npm test -- failing-test.test.ts

   # Add verbose output for details
   npm test -- failing-test.test.ts --verbose
   ```

3. **Create Hotfix Branch from Epic Branch**

   ```bash
   git checkout epic-10
   git pull origin epic-10
   git checkout -b epic-10-hotfix-fix-settlement-test
   ```

4. **Fix Issue and Test Locally**
   - Apply fix to source/test code
   - Run full test suite to verify fix
   - Validate with stability testing (run 10x):
     ```bash
     for i in {1..10}; do
       npm test -- fixed-test.test.ts || echo "Run $i FAILED"
     done
     ```

5. **Merge Hotfix to Epic Branch**

   ```bash
   git add .
   git commit -m "fix: resolve settlement test mock state issue"
   git push origin epic-10-hotfix-fix-settlement-test

   # Create PR: epic-10-hotfix → epic-10
   gh pr create --base epic-10 --head epic-10-hotfix-fix-settlement-test
   ```

6. **Re-run CI on Epic Branch PR**
   - CI automatically re-runs after hotfix merge
   - Verify all jobs pass

**If Failure is Systematic (Affects Multiple Epics):**

1. **Document Root Cause**
   - Create `docs/qa/root-cause-analysis-{story}.md`
   - Follow root cause analysis template
   - Include failure symptoms, cause, resolution, prevention

2. **Update Test Standards**
   - Add anti-pattern to `docs/architecture/test-strategy-and-standards.md`
   - Include bad example, good example, test validation

3. **Consider Preventive Epic**
   - If issue is widespread, create epic to address systematically
   - Example: Epic 10 created to address CI/CD test quality issues

**Reference:** Epic 10 Story 10.1 demonstrated this approach for settlement executor test failures.

---

## Pre-Push Quality Checklist

Use this checklist before every `git push` to ensure code quality. Pre-push hooks enforce most items automatically, but manual verification is recommended for complex changes.

### Checklist Items

#### Code Review

- [ ] **Staged changes reviewed and intentional**
  - Use `git diff --staged` to review changes before committing
  - Remove debug code, commented-out code, console.log statements

- [ ] **No console.log statements in production code**
  - Use Pino logger instead: `logger.info()`, `logger.error()`, `logger.debug()`
  - Exception: Test files can use console for debugging

#### Quality Gates (Automated by Hooks)

- [ ] **Pre-commit hooks passed**
  - Lint: ESLint with `--fix` applied to staged files
  - Format: Prettier auto-formats staged files

- [ ] **Related tests written and passing**
  - New features have corresponding unit tests
  - Bug fixes have regression tests
  - Test coverage >80% for new code

- [ ] **Pre-push hooks passed**
  - Lint and format checks on all changed files
  - Related tests run and pass (tests in same package as changes)

#### Type Safety

- [ ] **TypeScript strict mode compliance**
  - No `any` types (use `unknown` with type guards)
  - Handle null/undefined cases (use optional chaining `?.`, nullish coalescing `??`)
  - Explicit return types for public functions

- [ ] **Type check passes**
  ```bash
  npx tsc --noEmit -p packages/{package}/tsconfig.json
  ```

#### Test Coverage

- [ ] **New code has >80% test coverage**
  - Run tests with coverage:
    ```bash
    npm test -- --coverage my-feature.test.ts
    ```
  - Check coverage report for uncovered lines

- [ ] **Integration tests updated if needed**
  - Update integration tests if changing public APIs
  - Run integration tests manually (currently not in CI):
    ```bash
    npm test -- test/integration/
    ```

#### Documentation

- [ ] **Documentation updated**
  - README.md if changing public API or setup
  - Architecture docs if changing system design
  - CHANGELOG.md with user-facing changes
  - Inline code comments for complex logic

### Bypass Scenarios

**When to use `git push --no-verify`:**

- WIP commits on feature branch (not ready for CI)
- Emergency hotfix (document reason in commit message)
- Debugging CI issues (temporary bypass)

**Never bypass for:**

- Epic branch PRs
- PRs to main branch
- Production deployments

### Pre-Push Command Examples

```bash
# Standard push (runs pre-push hook automatically)
git push origin feature-branch

# Bypass hooks (use sparingly)
git push --no-verify origin feature-branch

# Force push (only on feature branches, never main/epic)
git push --force origin feature-branch
```

**Note:** Pre-commit and pre-push hooks are enforced by Husky (Story 10.2). Hooks run automatically on `git commit` and `git push`.

---

## Cross-References

- **CI Troubleshooting:** [ci-troubleshooting.md](ci-troubleshooting.md) - Debugging CI failures, investigation runbook
- **Git Hooks:** [git-hooks.md](git-hooks.md) - Detailed git hooks configuration and troubleshooting
- **Test Standards:** [../architecture/test-strategy-and-standards.md](../architecture/test-strategy-and-standards.md) - Test anti-patterns, best practices
- **Coding Standards:** [../architecture/coding-standards.md](../architecture/coding-standards.md) - TypeScript and Solidity guidelines

---

## Additional Topics

(To be expanded with setup instructions, architecture overview, etc.)
