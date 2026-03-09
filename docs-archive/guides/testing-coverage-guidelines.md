# Testing and Coverage Guidelines

## Coverage Philosophy

This project collects code coverage metrics to help identify untested code, but **coverage thresholds should not block development** unless there's a strong reason.

### Why We Don't Enforce Strict Coverage Thresholds

1. **Different code has different testability**: Frontend UI components naturally have lower test coverage than backend business logic
2. **Thresholds become stale**: As new features are added, thresholds need constant adjustment
3. **Thresholds don't guarantee quality**: 100% coverage doesn't mean bug-free code
4. **Prototyping and experimentation**: New features often start with lower coverage and improve over time

## Current Coverage Configuration

### Dashboard Package

- **Thresholds**: Disabled
- **Rationale**: Mix of frontend React components and backend server code. UI components have naturally lower coverage.
- **Coverage still collected**: Yes, visible in CI and coverage reports

### Connector Package

- **Thresholds**: branches 45%, functions 70%, lines/statements 65%
- **Note**: Lowered from initial values due to skipped integration tests requiring Docker/TigerBeetle

### Shared Package

- **Thresholds**: branches 82%, functions 100%, lines/statements 90%
- **Note**: Pure utility/business logic, easier to test comprehensively

## Best Practices

### When Adding New Features

1. **Write tests for critical paths**: Focus on business logic, error handling, and edge cases
2. **Don't obsess over coverage percentages**: Aim for meaningful tests, not just line coverage
3. **If CI fails on coverage**:
   - Option A: Add tests for the new code
   - Option B: Lower the threshold (especially for UI/frontend code)
   - Option C: Disable thresholds for that package (document why)

### When to Disable Coverage Thresholds

Disable coverage thresholds when:

- Package contains significant UI/frontend code
- Package is in rapid prototyping phase
- Package has integration tests that are skipped in CI
- Coverage failures block legitimate PRs repeatedly

### When to Keep Coverage Thresholds

Keep coverage thresholds when:

- Package contains pure business logic (like `shared`)
- Package is stable and tests are comprehensive
- Team agrees it adds value without blocking development

## Modifying Coverage Thresholds

If you need to adjust coverage thresholds:

1. **Check current coverage**: Run `npm test -- --coverage` in the package
2. **Set thresholds slightly below current coverage**: Leave 2-3% margin for new code
3. **Document the change**: Add a comment explaining why thresholds were adjusted
4. **Commit message**: Explain the rationale in the commit message

### Example

```javascript
// packages/dashboard/jest.config.cjs
coverageThreshold: {
  global: {
    branches: 40,  // Was 45%, lowered due to Story 8.10 adding payment channel UI
    functions: 48, // Was 50%, lowered for same reason
    lines: 60,
    statements: 60,
  },
}
```

## Viewing Coverage Reports

### Local Development

```bash
# Run tests with coverage for a specific package
npm test --workspace=packages/dashboard -- --coverage

# Run tests with coverage for all packages
npm test --workspaces --if-present -- --coverage
```

### CI/CD

- Coverage reports are automatically generated on every CI run
- View coverage in the GitHub Actions job output
- Coverage data is uploaded to Codecov (if configured)

## Coverage vs. Quality

Remember:

- **High coverage ≠ good tests**: You can have 100% coverage with meaningless tests
- **Low coverage ≠ bad code**: Well-designed, simple code may need fewer tests
- **Focus on test quality**: Write tests that catch real bugs and document expected behavior
- **Test the important parts**: Critical paths, error handling, edge cases, business logic

## When This Document Doesn't Help

If you're stuck on coverage issues:

1. Ask the team in PR comments
2. Check similar packages for patterns
3. Consider if the threshold is actually helping or just blocking work
4. When in doubt, disable the threshold and document why
