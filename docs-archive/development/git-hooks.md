# Git Hooks Workflow

## Overview

This project uses [Husky](https://typicode.github.io/husky/) to manage Git hooks that enforce local quality checks before code reaches CI/CD. These hooks catch issues early, reduce CI failures, and maintain code quality.

**Why Git Hooks?**

- Catch errors before they reach CI (faster feedback)
- Auto-fix issues when possible (eslint --fix, prettier --write)
- Prevent broken code from being pushed to shared branches
- Maintain consistent code quality across the team

**What They Check:**

- **Pre-commit:** Lint and format on staged files only
- **Pre-push:** Lint, format, and related tests on changed files

---

## Pre-Commit Hook

**Trigger:** Runs automatically on `git commit`

**What It Does:**

1. Runs ESLint with auto-fix on staged TypeScript files
2. Runs Prettier with auto-formatting on staged files
3. Only checks files you're committing (fast, targeted)

**Implementation:**

- Uses [lint-staged](https://github.com/lint-staged/lint-staged) to run linters on staged files
- Configuration in `package.json`:

```json
{
  "lint-staged": {
    "*.{ts,tsx}": ["eslint --fix", "prettier --write"],
    "*.{js,json,md}": ["prettier --write"]
  }
}
```

**Typical Execution Time:** 2-5 seconds for 1-5 files

**Example Output (Success):**

```
🔍 Running pre-commit quality checks...
✔ Running tasks for staged files...
✔ eslint --fix
✔ prettier --write
✅ Pre-commit checks passed!
```

**Example Output (Failure):**

```
🔍 Running pre-commit quality checks...
✖ eslint --fix:

/path/to/file.ts
  10:7  error  'unusedVar' is assigned a value but never used  @typescript-eslint/no-unused-vars
  15:3  error  Unexpected console statement                    no-console

✖ 2 problems (2 errors, 0 warnings)
```

**Fixing Errors:**

- Most errors auto-fixed by ESLint and Prettier
- Manual fixes required for unused variables, console statements, etc.
- Run `npx eslint --fix <file>` to attempt auto-fix
- Run `npm run format` to fix all formatting issues

---

## Pre-Push Hook

**Trigger:** Runs automatically on `git push`

**What It Does:**

1. **1/3 Linting:** Runs ESLint on changed TypeScript files (compared to remote branch)
2. **2/3 Formatting:** Checks all files for formatting issues
3. **3/3 Testing:** Runs Jest with `--findRelatedTests` for changed source files

**Implementation:**

- Compares current branch to `origin/<branch>` (or `origin/main` if branch doesn't exist)
- Uses `git diff --name-only --diff-filter=ACM` to find changed files
- Filters out deleted files, test files, and type definition files
- Runs `jest --findRelatedTests --passWithNoTests --bail` for fast feedback

**Typical Execution Time:** 10-30 seconds depending on changes

**Example Output (Success):**

```
🚀 Running pre-push quality gates...
1/3 Linting changed TypeScript files...
2/3 Checking code formatting...
3/3 Running tests for changed files...
✅ All pre-push checks passed! Ready to push.
```

**Example Output (Failure):**

```
🚀 Running pre-push quality gates...
1/3 Linting changed TypeScript files...
❌ Linting failed. Run 'npm run lint' to see all errors, or 'npx eslint --fix <file>' to auto-fix.
```

**Fixing Errors:**

- **Linting:** Run `npm run lint` to see all errors, `npx eslint --fix <file>` to auto-fix
- **Formatting:** Run `npm run format` to fix all files
- **Tests:** Run `npm test -- <file>` to debug failing tests

---

## Bypassing Hooks

**⚠️ Use Sparingly:** Bypassing hooks increases the risk of CI failures. Only bypass when necessary and document why.

### When to Bypass

**Acceptable:**

- Emergency hotfixes (document in commit message why)
- Work-in-progress commits to feature branch (not main/epic branches)
- Temporary debugging commits (to be reverted)

**NOT Acceptable:**

- Pull requests to main or epic branches
- Commits that will be merged without review
- Production-bound code

### How to Bypass

**Pre-commit:**

```bash
git commit --no-verify -m "WIP: debugging issue"
```

**Pre-push:**

```bash
git push --no-verify
```

**Important:** Always document why you bypassed hooks in:

- Commit message (for commits)
- PR description (for pushes)
- PR template has a section for this justification

---

## Troubleshooting Common Issues

### Issue 1: "Hook execution too slow"

**Cause:** Large number of staged files or changed files

**Solutions:**

- Commit smaller batches (fewer files = faster hooks)
- Use `--no-verify` for WIP commits on feature branches
- Review if you need to commit all staged files at once

---

### Issue 2: "Tests failing in hook but passing locally"

**Cause:** Different Node.js environment or dependencies

**Solutions:**

- Run `npm ci` to sync dependencies
- Check Node.js version matches requirement (20.11.0 LTS)
- Verify no stale node_modules (delete and `npm install`)
- Check for uncommitted changes to test files

---

### Issue 3: "Linting errors I don't understand"

**Cause:** ESLint rules or Prettier formatting

**Solutions:**

- Run `npx eslint --fix <file>` to auto-fix most issues
- Run `npm run format` to fix formatting
- Check ESLint configuration in `.eslintrc.json`
- Read error message carefully (usually includes fix suggestion)

**Common Errors:**

- `@typescript-eslint/no-unused-vars`: Remove unused variables/imports
- `no-console`: Use Pino logger instead (`logger.info()`, etc.)
- `@typescript-eslint/explicit-function-return-type`: Add return type to function

---

### Issue 4: "Hook not running at all"

**Cause:** Husky not initialized or .husky/ directory missing

**Solutions:**

- Run `npm install` (triggers prepare script which runs `husky`)
- Check if `.husky/` directory exists
- Verify hooks are executable: `chmod +x .husky/pre-commit .husky/pre-push`
- Run `npx husky install` manually if needed

---

### Issue 5: "Format check fails but I already ran prettier"

**Cause:** Prettier version mismatch or config differences

**Solutions:**

- Run `npm run format` (uses exact same Prettier version as hook)
- Check if `.prettierrc.json` was modified
- Verify no IDE-specific Prettier plugin conflicts
- Check for `.prettierignore` excluding wrong files

---

## Hook Workflow Quick Reference

| Hook       | Trigger      | What It Checks                              | Bypass Command           |
| ---------- | ------------ | ------------------------------------------- | ------------------------ |
| pre-commit | `git commit` | Lint & format staged files                  | `git commit --no-verify` |
| pre-push   | `git push`   | Lint, format, related tests (changed files) | `git push --no-verify`   |

---

## Related Documentation

- [Test Strategy and Standards](../architecture/test-strategy-and-standards.md) - Testing patterns and anti-patterns
- [Coding Standards](../architecture/coding-standards.md) - ESLint and Prettier rules
- [PR Template](../../.github/PULL_REQUEST_TEMPLATE.md) - Quality checklist for pull requests
