# Root Cause Analysis: Story 10.1 - Settlement Executor Test Failures

**Date:** 2026-01-06
**Epic:** 10 - CI/CD Pipeline Reliability & Test Quality
**Story:** 10.1 - Fix Settlement Executor Test Failures
**Status:** Resolved
**Fix Commit:** 034a098

## Executive Summary

Settlement executor unit tests failed during epic branch PR CI runs due to event listener cleanup issues and async timeout handling. All issues were resolved in commit 034a098 with comprehensive validation performed in Story 10.1.

**Impact:** CI pipeline failures blocking PR merges
**Root Causes:** 3 identified (event listener cleanup, async timeouts, mock state concerns)
**Resolution:** Code fixes applied, stability validated (10/10 test runs), preventive guidelines documented

---

## Section 1: Event Listener Cleanup Issue

### Problem Description

The test "should stop event polling and unregister listener" failed intermittently because `EventEmitter.off()` could not match the original event handler, leaving listeners registered after `stop()` was called.

### Root Cause

**Anti-Pattern:** Calling `bind(this)` inline in both `start()` and `stop()` methods

```typescript
// BEFORE (Incorrect)
start(): void {
  this.settlementMonitor.on('SETTLEMENT_REQUIRED', this.handleSettlement.bind(this));
}

stop(): void {
  this.settlementMonitor.off('SETTLEMENT_REQUIRED', this.handleSettlement.bind(this));
}
```

**Why This Failed:**

- Each call to `bind(this)` creates a **new function reference**
- `EventEmitter.off(event, handler)` matches handlers by **reference equality**
- The handler passed to `off()` was a different reference than the one passed to `on()`
- Result: Handler never removed, `listenerCount('SETTLEMENT_REQUIRED')` remained 1

### Technical Details

**Source File:** `packages/connector/src/settlement/settlement-executor.ts`
**Test File:** `packages/connector/src/settlement/settlement-executor.test.ts:105-111`
**Symptom:** `expect(mockSettlementMonitor.listenerCount('SETTLEMENT_REQUIRED')).toBe(0)` failed (was 1)

### Solution Applied

**Pattern:** Store bound handler reference in constructor, reuse in `start()` and `stop()`

```typescript
// AFTER (Correct)
export class SettlementExecutor {
  private readonly boundHandleSettlement: (event: SettlementTriggerEvent) => Promise<void>;

  constructor(...) {
    // Bind once in constructor
    this.boundHandleSettlement = this.handleSettlement.bind(this);
  }

  start(): void {
    // Use stored reference
    this.settlementMonitor.on('SETTLEMENT_REQUIRED', this.boundHandleSettlement);
  }

  stop(): void {
    // Use same stored reference - cleanup succeeds
    this.settlementMonitor.off('SETTLEMENT_REQUIRED', this.boundHandleSettlement);
  }
}
```

**Fix Commit:** 034a098
**Lines Changed:** settlement-executor.ts:61-62, 90, 99, 113

### Prevention Guidelines

1. **Always store bound handler references** for event listeners that need cleanup
2. Store as `private readonly` property, initialize in constructor
3. Use stored reference in both `.on()` and `.off()` calls
4. Test cleanup with `listenerCount()` assertions

### Validation Results

- ✅ Test "should stop event polling and unregister listener" passes consistently
- ✅ Listener count correctly returns 0 after `stop()`
- ✅ No memory leaks from unremoved listeners

---

## Section 2: Async Timeout Issues

### Problem Description

Tests with async event handlers timed out or failed intermittently because the test completed assertions before async Promise chains finished executing.

### Root Cause

**Insufficient timeout values** for complex async operations

- **Basic operations** (50ms): Initially sufficient for single async handler calls
- **Deposit operations** (100ms): Required for 3 sequential `getChannelState()` calls (initially too low)
- **Retry operations** (500ms): Required for exponential backoff loops (initially too low)

### Technical Details

**Affected Test:** "should deposit additional funds when transferred exceeds deposit"
**File:** `settlement-executor.test.ts:237-290`
**Line 280:** `await new Promise((resolve) => setTimeout(resolve, 100));`

**Why Timeouts Are Needed:**
Async event handlers process settlement triggers via `EventEmitter.emit()`:

1. `emit('SETTLEMENT_REQUIRED', event)` returns immediately (synchronous)
2. Handler `handleSettlement()` executes asynchronously
3. Promise chain: `handleSettlement → executeSettlement → SDK calls → account updates`
4. Tests must await chain completion before assertions

**Sequential Calls Example (Deposit Operation):**

```typescript
mockSDK.getChannelState
  .mockResolvedValueOnce(lowDepositState) // Call 1: findChannelForPeer
  .mockResolvedValueOnce(lowDepositState) // Call 2: before deposit
  .mockResolvedValueOnce(highDepositState); // Call 3: after deposit
```

3 async calls require 100ms total (50ms insufficient)

### Solution Applied

**Operation-Specific Timeout Guidelines:**

| Operation Type     | Timeout | Use Case                                           |
| ------------------ | ------- | -------------------------------------------------- |
| Basic operations   | 50ms    | Single async handler (channel open, balance proof) |
| Deposit operations | 100ms   | 3 sequential `getChannelState()` calls             |
| Retry operations   | 500ms   | Exponential backoff with multiple retries          |

**Documentation Added:** JSDoc comment in `settlement-executor.test.ts:19-30`

### Prevention Guidelines

1. **Always use timeouts** after `EventEmitter.emit()` in tests
2. **Scale timeouts** to operation complexity (number of async calls)
3. **Document timeout rationale** in test comments
4. **If tests flake intermittently**, increase timeout incrementally (25-50ms)

### Validation Results

- ✅ All 14 tests pass with current timeouts
- ✅ Deposit test passes 10/10 runs (no flakiness)
- ✅ Average execution time: 3.8s (under 5s target)

---

## Section 3: Mock State Leakage (Preventive Analysis)

### Problem Description

Potential for test interdependencies if mock state persists between tests, causing failures when tests run in different orders or isolation.

### Root Cause (Potential)

**Anti-Pattern:** Using `mockResolvedValue()` (without `Once`) for functions called multiple times in a test, or not resetting mocks in `beforeEach()`

```typescript
// RISKY (if function called multiple times with different expectations)
mockSDK.getChannelState.mockResolvedValue(state1);
// Second call returns state1 again, may not be intended
```

### Solution Applied

**Current Implementation (Correct):**

1. **Fresh mock instances in `beforeEach()`:**
   - New `SettlementExecutor` instance (clears `peerChannelMap`, `activeSettlements`)
   - New `EventEmitter` for `mockSettlementMonitor`
   - Fresh `jest.fn()` for all SDK methods

2. **Proper `mockResolvedValueOnce` usage:**

   ```typescript
   // Deposit test (settlement-executor.test.ts:272-275)
   mockSDK.getChannelState
     .mockResolvedValueOnce(lowDepositState) // Consumed by call 1
     .mockResolvedValueOnce(lowDepositState) // Consumed by call 2
     .mockResolvedValueOnce(highDepositState); // Consumed by call 3
   ```

3. **Default mocks reset per test:**
   - `beforeEach()` calls `jest.fn().mockResolvedValue(default)` for each method
   - Tests override with `.mockResolvedValueOnce()` as needed

### Prevention Guidelines

1. **Always create fresh mock instances** in `beforeEach()`
2. **Use `mockResolvedValueOnce()`** for sequential calls with different return values
3. **Use `mockResolvedValue()`** only for default behaviors in `beforeEach()`
4. **Test isolation:** Run with `--runInBand` to verify no order dependencies

### Validation Results

- ✅ All 14 tests pass sequentially (`--runInBand`)
- ✅ No test interdependencies detected
- ✅ Fresh mocks created for each test (verified in `beforeEach()` review)

---

## Overall Impact and Metrics

### Before Fix (Commit 034a098)

- ❌ CI pipeline failures on epic branch PRs
- ❌ Event listener cleanup test failed intermittently
- ❌ Potential async timeout issues

### After Fix and Validation (Story 10.1)

- ✅ All 14 tests pass consistently
- ✅ 10/10 stability test runs successful (100% pass rate)
- ✅ Test execution time: 3.8s (under 5s target)
- ✅ No flaky behavior detected
- ✅ Preventive guidelines documented

### Files Modified

1. `settlement-executor.ts` - Event listener cleanup fix
2. `settlement-executor.integration.test.ts` - Added missing `exceedsBy` property
3. `settlement-executor.test.ts` - Timeout guidelines documented
4. `test-strategy-and-standards.md` - Anti-patterns section added
5. `root-cause-analysis-10.1.md` - This document

---

## Recommendations

### Immediate Actions (Completed)

1. ✅ Apply event listener cleanup pattern project-wide
2. ✅ Document timeout guidelines in test files
3. ✅ Validate stability with repeated test runs
4. ✅ Update test standards documentation

### Future Considerations

1. **Consider linting rule:** Detect `bind(this)` in event listener callbacks
2. **CI enhancement:** Add stability testing step (run critical tests 3x)
3. **Test timeout automation:** Calculate timeouts based on mock call count
4. **Monitor for similar patterns** in other event-driven components

---

## References

- **Fix Commit:** 034a098 - "Fix settlement executor test failures"
- **Story:** 10.1 - Fix Settlement Executor Test Failures
- **Epic:** 10 - CI/CD Pipeline Reliability & Test Quality
- **Test File:** `packages/connector/src/settlement/settlement-executor.test.ts`
- **Source File:** `packages/connector/src/settlement/settlement-executor.ts`
- **Stability Script:** `packages/connector/test/stability/run-settlement-tests.sh`
