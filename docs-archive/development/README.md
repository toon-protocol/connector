# Developer Documentation Index

Welcome to the M2M (Multi-node Interledger Connector) project developer documentation. This index organizes all documentation by category for easy discovery.

## Getting Started

Essential documentation for new contributors:

- **[Developer Guide](developer-guide.md)** - Epic branch workflow, pre-push checklist, git hooks quick reference
- **[Git Hooks Workflow](git-hooks.md)** - Pre-commit and pre-push hook configuration, troubleshooting
- **Contributing Guidelines** - `../../CONTRIBUTING.md` (project root) - How to contribute, code review process

## Development Workflow

Workflows and processes for daily development:

- **[Epic Branch Workflow](developer-guide.md#epic-branch-workflow)** - Creating epic branch PRs, handling failures, quality standards
- **[Pre-Push Quality Checklist](developer-guide.md#pre-push-quality-checklist)** - Checklist before every push
- **[Git Hooks](git-hooks.md)** - Automated quality gates (pre-commit, pre-push)

## Quality Standards

Standards and best practices for writing high-quality code:

- **[Test Strategy and Standards](../architecture/test-strategy-and-standards.md)** - Test anti-patterns, stability testing, isolation validation
- **[Coding Standards](../architecture/coding-standards.md)** - TypeScript strict mode, Solidity patterns, naming conventions
- **[Tech Stack](../architecture/tech-stack.md)** - Technology choices, versions, rationale

## CI/CD

Continuous integration and deployment workflows:

- **[CI Troubleshooting Guide](ci-troubleshooting.md)** - Debugging CI failures, investigation runbook, best practices
- **[GitHub Actions Workflow](../../.github/workflows/ci.yml)** - CI/CD pipeline configuration
- **[Pull Request Template](../../.github/PULL_REQUEST_TEMPLATE.md)** - Quality checklist for PRs

## Troubleshooting

Resources for debugging issues:

- **[CI Troubleshooting](ci-troubleshooting.md)** - CI failure scenarios, diagnostic commands, job-specific debugging
- **[Root Cause Analyses](../qa/)** - Documented investigations of past failures
  - [RCA 10.1: Settlement Executor Test Failures](../qa/root-cause-analysis-10.1.md)
- **[Common Test Anti-Patterns](../architecture/test-strategy-and-standards.md#common-test-anti-patterns-and-solutions)** - Flaky tests, mock issues, cleanup failures

## Architecture

System design and technical specifications:

- **[Architecture Overview](../architecture/)** - System design documents
- **[Source Tree](../architecture/source-tree.md)** - Project structure, directory organization
- **[Tech Stack](../architecture/tech-stack.md)** - Technology decisions and versions

## Quick Reference

### Common Commands

```bash
# Quality Checks
npm run lint                    # Run ESLint
npm run lint -- --fix           # Auto-fix linting issues
npm run format                  # Auto-format with Prettier
npm run format:check            # Check formatting (no changes)

# Testing
npm test                        # Run all tests
npm test -- --coverage          # Run with coverage
npm test -- my-test.test.ts     # Run specific test file
npm test -- --runInBand         # Run sequentially (detect race conditions)

# Building
npm run build                   # Build all packages
npm run build --workspace=packages/shared # Build specific package

# Type Checking
npx tsc --noEmit -p packages/connector/tsconfig.json

# Git Hooks
git commit                      # Triggers pre-commit hook
git push                        # Triggers pre-push hook
git commit --no-verify          # Bypass pre-commit (use sparingly)
git push --no-verify            # Bypass pre-push (use sparingly)
```

### Pre-Push Checklist (Quick Version)

Before every `git push`:

- [ ] Staged changes reviewed (`git diff --staged`)
- [ ] No console.log in production code
- [ ] Pre-commit hooks passed
- [ ] Related tests written and passing
- [ ] Pre-push hooks passed
- [ ] No TypeScript `any` types
- [ ] Type check passes
- [ ] Test coverage >80%
- [ ] Documentation updated (README, CHANGELOG, architecture docs)

See [full checklist](developer-guide.md#pre-push-quality-checklist) for details.

### Epic Branch PR Checklist (Quick Version)

Before creating epic branch PR:

- [ ] All story branches merged
- [ ] Full test suite passes (`npm test --workspaces --if-present`)
- [ ] All packages build (`npm run build`)
- [ ] Type check passes (all packages)
- [ ] CHANGELOG.md updated
- [ ] Architecture docs updated if needed

See [full workflow](developer-guide.md#epic-branch-workflow) for details.

## Contributing

Ready to contribute? Follow this path:

1. **Read:** [Developer Guide](developer-guide.md) - Understand workflows and quality standards
2. **Read:** [Git Hooks](git-hooks.md) - Understand automated quality gates
3. **Read:** [Test Standards](../architecture/test-strategy-and-standards.md) - Learn test anti-patterns
4. **Read:** [Coding Standards](../architecture/coding-standards.md) - Follow TypeScript/Solidity guidelines
5. **Setup:** Install pre-commit hooks (`npm install` installs Husky automatically)
6. **Code:** Follow [Pre-Push Quality Checklist](developer-guide.md#pre-push-quality-checklist)
7. **Debug:** Use [CI Troubleshooting Guide](ci-troubleshooting.md) if CI fails

## Additional Resources

- **[Product Requirements](../prd/)** - Epic and feature specifications
- **[QA Documentation](../qa/)** - Quality gates, root cause analyses
- **[Interledger RFCs](../rfcs/)** - Protocol specifications
- **[CHANGELOG](../../CHANGELOG.md)** - Version history and changes

---

**Last Updated:** 2026-01-06 (Epic 10 Story 10.3)
