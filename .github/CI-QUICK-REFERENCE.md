# CI/CD Quick Reference Guide

Quick troubleshooting and command reference for the M2M CI/CD pipeline.

## 🚀 Quick Start

### Test Locally Before Pushing

```bash
# Run all checks that CI will run
npm ci --verbose          # Install dependencies
npm run lint              # Lint check
npm run format:check      # Format check
npm run build             # Build all packages
npm test -- --coverage    # Run tests with coverage
```

### View CI Status

- **GitHub Actions:** `https://github.com/{org}/{repo}/actions`
- **Status Summary:** Check the "CI Status Summary" job at the end of each run

## 🔍 Common Issues & Quick Fixes

### Exit Code 127: Command Not Found

**Symptom:** Tests fail with "command not found" error

**Quick Fix:**

```bash
# Verify dependencies locally
npm ci
npm list --depth=0

# If issue persists, regenerate lockfile
rm package-lock.json node_modules -rf
npm install
```

**In CI:** Check "Verify installation" step logs for Jest binary location

### Build Failures

**Symptom:** Build job fails with exit code 1

**Quick Fix:**

```bash
# Clean build locally
rm -rf packages/*/dist
npm run build

# Check for TypeScript errors
npx tsc --noEmit
```

**In CI:** Check "Verify dist artifacts" step for specific package failures

### Lint Failures

**Symptom:** ESLint or Prettier failures

**Quick Fix:**

```bash
# Auto-fix formatting
npm run format

# Check lint errors
npm run lint

# Fix specific workspace
npm run lint --workspace=packages/connector
```

### Test Failures

**Symptom:** Tests pass locally but fail in CI

**Common Causes:**

1. Environment variables missing
2. Timing issues in tests
3. File path differences (case sensitivity)

**Quick Fix:**

```bash
# Run tests in CI mode locally
CI=true npm test -- --ci --coverage

# Run specific workspace tests
npm test --workspace=packages/connector -- --ci
```

## 📊 CI Job Overview

| Job                    | Purpose               | Typical Duration | Can Fail?             |
| ---------------------- | --------------------- | ---------------- | --------------------- |
| **lint-and-format**    | Code quality checks   | 1-2 min          | ✅ Yes (blocks merge) |
| **test**               | Run Jest tests        | 3-5 min          | ✅ Yes (blocks merge) |
| **build**              | Build all packages    | 2-4 min          | ✅ Yes (blocks merge) |
| **type-check**         | TypeScript validation | 2-3 min          | ✅ Yes (blocks merge) |
| **contracts-coverage** | Solidity tests        | 1-2 min          | ⚠️ Monitored          |
| **security**           | npm audit             | 1 min            | ⚠️ Warning only       |
| **rfc-links**          | Validate docs         | 1-2 min          | ✅ Yes (blocks merge) |
| **e2e-test**           | Full system test      | 5-10 min         | ✅ Yes (blocks merge) |
| **ci-status**          | Summary               | <1 min           | Shows overall status  |

## 🛠️ Debugging CI Failures

### Step 1: Identify Which Job Failed

Check the Actions tab and look for ❌ red X marks.

### Step 2: Review Job Logs

Click on the failed job and expand each step to see detailed logs.

### Step 3: Look for Key Indicators

```bash
# Exit code 127 = Command not found
"Error: Process completed with exit code 127"
→ Check "Verify installation" step

# Exit code 1 = Command failed
"Error: Process completed with exit code 1"
→ Check command output above the error

# ELIFECYCLE = npm script failed
"npm ERR! code ELIFECYCLE"
→ Check the actual command that failed
```

### Step 4: Reproduce Locally

```bash
# Match CI environment
node --version  # Should be 20.11.0 or 20.x
npm ci          # Use clean install like CI

# Run the failing command
npm run [command-that-failed]
```

### Step 5: Check Recent Changes

```bash
# View recent commits
git log --oneline -10

# See what changed in package files
git diff HEAD~1 package.json
git diff HEAD~1 package-lock.json
```

## 🔄 Retry Failed Jobs

GitHub Actions allows manual retry:

1. Go to failed workflow run
2. Click "Re-run jobs" dropdown (top right)
3. Select "Re-run failed jobs" or "Re-run all jobs"

## 📦 Build Artifacts

### View Build Outputs

1. Go to completed workflow run
2. Scroll to "Artifacts" section (bottom)
3. Download `build-artifacts-{sha}` or `e2e-container-logs-{sha}`

### Artifact Contents

```
build-artifacts-{sha}/
├── packages/
│   ├── connector/dist/
│   └── shared/dist/
```

### Retention

- Build artifacts: 7 days
- E2E logs: 7 days (only on failure)

## 🚨 Emergency Procedures

### CI Completely Broken

```bash
# Option 1: Revert recent changes
git revert HEAD
git push

# Option 2: Rollback CI config
git checkout HEAD~1 -- .github/workflows/ci.yml
git commit -m "chore: rollback CI config"
git push
```

### Bypass CI (Emergency Only)

```bash
# Skip CI for emergency hotfix (use sparingly!)
git commit -m "hotfix: critical fix [skip ci]"
```

**⚠️ Warning:** Only use `[skip ci]` for documentation changes or true emergencies.

### All Tests Timing Out

Check if GitHub Actions is experiencing issues:

- https://www.githubstatus.com/

## 📈 Performance Monitoring

### Check Job Duration Trends

```bash
# View recent workflow runs
gh run list --limit 20

# View specific run timing
gh run view {run-id} --log
```

### Optimize Slow Jobs

**If test job is slow:**

- Check for database seeding in tests
- Look for network calls without mocks
- Review timeout configurations

**If build job is slow:**

- Check for unnecessary file processing
- Review TypeScript project references
- Consider incremental builds

## 🔐 Security Checks

### Review Security Audit

```bash
# Run locally
npm audit

# Fix automatically fixable issues
npm audit fix

# Review details
npm audit --json
```

### Update Dependencies

```bash
# Check for outdated packages
npm outdated

# Update minor/patch versions
npm update

# Update major versions (carefully!)
npm install package@latest
```

## 💡 Pro Tips

### Speed Up Local Testing

```bash
# Run only changed tests
npm test -- --onlyChanged

# Run tests in watch mode
npm run test:watch

# Run specific test file
npm test -- path/to/test.test.ts
```

### Verbose Debugging

```bash
# Maximum verbosity for npm
npm ci --loglevel=verbose

# Debug Jest issues
npm test -- --debug --verbose

# TypeScript compiler details
npx tsc --noEmit --listFiles
```

### Workspace-Specific Commands

```bash
# Target specific package
npm run build --workspace=packages/shared
npm run test --workspace=packages/connector
npm run lint --workspace=packages/connector

# List all workspaces
npm query ".workspace"
```

## 📞 Getting Help

### Check These First

1. ✅ This quick reference guide
2. ✅ Full CI improvements documentation (`.github/CI-IMPROVEMENTS.md`)
3. ✅ GitHub Actions logs
4. ✅ Recent commits and PRs

### Escalation Path

1. **Local Issue:** Check your local environment setup
2. **CI-Specific Issue:** Review workflow configuration
3. **Persistent Failures:** Contact DevOps team
4. **Emergency:** Use emergency procedures above

## 🔗 Useful Links

- **GitHub Actions Docs:** https://docs.github.com/en/actions
- **npm Workspaces:** https://docs.npmjs.com/cli/v7/using-npm/workspaces
- **Jest Docs:** https://jestjs.io/docs/getting-started
- **TypeScript Docs:** https://www.typescriptlang.org/docs/

## 📝 Common Commands Cheat Sheet

```bash
# Dependency Management
npm ci                                    # Clean install (CI mode)
npm install                               # Regular install
npm list --depth=0                        # List installed packages
npm outdated                              # Check for updates

# Testing
npm test                                  # Run all tests
npm test -- --coverage                    # With coverage
npm test -- --watch                       # Watch mode
npm test -- path/to/file.test.ts         # Specific file

# Building
npm run build                             # Build all packages
npm run build --workspace=packages/shared # Build specific package

# Linting & Formatting
npm run lint                              # Lint all packages
npm run format                            # Auto-fix formatting
npm run format:check                      # Check formatting only

# Workspace Operations
npm run test --workspaces --if-present    # Run in all workspaces
npm run build --workspaces                # Build all workspaces
npm install --workspace=packages/shared   # Install to specific workspace
```

---

**Last Updated:** January 6, 2026
**Quick Access:** `.github/CI-QUICK-REFERENCE.md`
