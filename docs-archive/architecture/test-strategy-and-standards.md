# Test Strategy and Standards

## Testing Philosophy

- **Approach:** Test-Driven Development (TDD) encouraged but not required
- **Coverage Goals:**
  - `packages/shared`: >90% (critical protocol logic)
  - `packages/connector`: >80% (core routing, BTP, settlement)
- **Test Pyramid:**
  - 70% Unit Tests (fast, isolated, comprehensive)
  - 20% Integration Tests (multi-component, Docker-based)
  - 10% E2E Tests (full system validation)

**Note:** Dashboard visualization deferred - see DASHBOARD-DEFERRED.md in root

## Test Types and Organization

### Unit Tests

- **Framework:** Jest 29.7.x with TypeScript support (`ts-jest`)
- **File Convention:** `<filename>.test.ts` co-located with source
- **Location:** Same directory as source file (e.g., `src/core/packet-handler.test.ts`)
- **Mocking Library:** Jest built-in mocking (`jest.fn()`, `jest.mock()`)
- **Coverage Requirement:** >80% line coverage for connector, >90% for shared

**AI Agent Requirements:**

- Generate tests for all public methods and exported functions
- Cover edge cases: empty inputs, null values, maximum values, expired timestamps
- Follow AAA pattern (Arrange, Act, Assert) with clear test descriptions
- Mock all external dependencies (WebSocket, Logger, BTPClient)
- Use descriptive test names: `should reject packet when expiry time has passed`

**Example Unit Test Structure:**

```typescript
describe('PacketHandler', () => {
  let handler: PacketHandler;
  let mockRoutingTable: jest.Mocked<RoutingTable>;
  let mockLogger: jest.Mocked<Logger>;

  beforeEach(() => {
    mockRoutingTable = createMockRoutingTable();
    mockLogger = createMockLogger();
    handler = new PacketHandler(mockRoutingTable, mockLogger);
  });

  it('should reject packet when expiry time has passed', async () => {
    // Arrange
    const expiredPacket = createExpiredPreparePacket();

    // Act
    const result = await handler.processPrepare(expiredPacket);

    // Assert
    expect(result.type).toBe(PacketType.REJECT);
    expect(result.code).toBe('T00'); // Transfer Timed Out
  });
});
```

### Integration Tests

- **Scope:** Multi-component interaction within connector package
- **Location:** `packages/connector/test/integration/`
- **Test Infrastructure:**
  - **WebSocket:** Use real ws library with localhost connections (not mocked)
  - **Routing Table:** Real RoutingTable instance with test data
  - **BTP:** Real BTPServer + BTPClient connecting locally
  - **Blockchain Nodes:** Use docker-compose-dev.yml local infrastructure (Anvil)

**Local Blockchain Infrastructure (Epic 7):**

All integration tests requiring blockchain interaction MUST use the local node infrastructure from `docker-compose-dev.yml`:

- **Anvil (Base L2 EVM)**: Local fork of Base Sepolia at `http://localhost:8545`
  - 10 pre-funded test accounts with deterministic addresses
  - Instant block mining for fast test execution
  - Managed by docker-compose, shared across test runs

**Starting Infrastructure:**

```bash
# Start blockchain nodes for integration tests
docker-compose -f docker-compose-dev.yml up -d anvil tigerbeetle

# Check health status
docker-compose -f docker-compose-dev.yml ps
```

**Test Configuration:**

- Tests should check infrastructure availability in `beforeAll()` hook
- Skip tests if docker-compose-dev not running (use `describe.skip`)
- Never start/stop blockchain nodes within tests (managed by docker-compose)
- CI environment: Set `INTEGRATION_TESTS=true` to run, or skip by default

**Example Integration Test:**

- Deploy 3 connector instances in-process
- Send ILP Prepare through Connector A
- Verify packet routed through B to C
- Validate telemetry events emitted at each hop

**Example Blockchain Integration Test:**

```typescript
describeIfInfra('Payment Channel Integration', () => {
  beforeAll(async () => {
    // Check infrastructure availability
    const infraHealthy = await checkDockerInfrastructure();
    if (!infraHealthy) {
      throw new Error(
        'Start docker-compose-dev: docker-compose -f docker-compose-dev.yml up -d anvil'
      );
    }
    // Connect to existing Anvil
    provider = new ethers.JsonRpcProvider('http://localhost:8545');
  });

  it('should open and close payment channel', async () => {
    // Test uses real blockchain transactions
  });
});
```

### End-to-End Tests

- **Framework:** Jest with Docker Compose integration
- **Scope:** Full system deployment with connector network, agent wallet, settlement
- **Environment:** Automated Docker Compose startup in test
- **Test Data:** Pre-configured 3-node linear topology

**Example E2E Test Flow:**

```typescript
describe('Full System E2E', () => {
  beforeAll(async () => {
    await execAsync('docker-compose up -d');
    await waitForHealthy(['connector-a', 'connector-b', 'connector-c', 'tigerbeetle']);
  });

  it('should forward packet through network and trigger settlement', async () => {
    // Send packet using agent wallet
    await agentWallet.sendPayment('g.connectorC.dest', 1000);

    // Wait for telemetry events to be logged
    const logs = await collectStructuredLogs(timeout: 5000);

    // Verify packet flow
    expect(logs).toContainEqual(
      expect.objectContaining({ type: 'PACKET_SENT', nodeId: 'connector-a' })
    );
    expect(logs).toContainEqual(
      expect.objectContaining({ type: 'PACKET_RECEIVED', nodeId: 'connector-c' })
    );

    // Verify settlement triggered
    expect(logs).toContainEqual(
      expect.objectContaining({ type: 'SETTLEMENT_COMPLETED' })
    );
  });

  afterAll(async () => {
    await execAsync('docker-compose down');
  });
});
```

## Test Data Management

- **Strategy:** Factory functions for test data generation
- **Fixtures:** JSON fixtures in `test/fixtures/` for complex scenarios
- **Factories:** `createTestPreparePacket(overrides)` functions in `test/helpers/`
- **Cleanup:** Jest `afterEach` hooks reset in-memory state, Docker tests clean up containers

## Continuous Testing

- **CI Integration:**
  - `npm test` runs all unit tests
  - `npm run test:integration` runs integration tests
  - E2E tests run on main branch only (slow)
- **Performance Tests:** Separate `npm run test:perf` script (Story 4.9)
- **Security Tests:** `npm audit` in CI pipeline, dependency scanning with Dependabot

## Common Test Anti-Patterns and Solutions

**Source:** Epic 10 Story 10.1 - Settlement Executor Test Failures Root Cause Analysis

This section documents common testing mistakes and their solutions, derived from real test failures in the project.

### Anti-Pattern 1: Inline bind(this) in Event Listeners

**Problem:**
Calling `bind(this)` inline when registering and unregistering event listeners prevents proper cleanup because each `bind()` call creates a new function reference.

**Bad Example:**

```typescript
class SettlementExecutor {
  start(): void {
    this.settlementMonitor.on('SETTLEMENT_REQUIRED', this.handleSettlement.bind(this));
  }

  stop(): void {
    // This FAILS - different function reference than start()
    this.settlementMonitor.off('SETTLEMENT_REQUIRED', this.handleSettlement.bind(this));
  }
}
```

**Why It Fails:**

- `EventEmitter.off(event, handler)` matches handlers by **reference equality**
- Each `bind(this)` creates a **new function reference**
- The handler passed to `off()` is different from the one passed to `on()`
- Result: Handler never removed, memory leak

**Solution:**
Store bound handler reference in constructor, reuse in `start()` and `stop()`

**Good Example:**

```typescript
class SettlementExecutor {
  private readonly boundHandleSettlement: (event: Event) => Promise<void>;

  constructor(...) {
    // Bind once in constructor
    this.boundHandleSettlement = this.handleSettlement.bind(this);
  }

  start(): void {
    // Use stored reference
    this.monitor.on('EVENT', this.boundHandleSettlement);
  }

  stop(): void {
    // Use same stored reference - cleanup succeeds
    this.monitor.off('EVENT', this.boundHandleSettlement);
  }
}
```

**Test Validation:**

```typescript
it('should unregister listener on stop', () => {
  executor.start();
  executor.stop();

  // Verify cleanup succeeded
  expect(mockMonitor.listenerCount('EVENT')).toBe(0);
});
```

**Reference:** settlement-executor.ts:61-62, 90, 99, 113 (commit 034a098)

---

### Anti-Pattern 2: Insufficient Async Timeouts

**Problem:**
Async event handlers process events asynchronously via `EventEmitter.emit()`. Tests must wait for Promise chains to complete before assertions, or tests fail intermittently.

**Bad Example:**

```typescript
it('should handle settlement event', async () => {
  executor.start();

  mockMonitor.emit('SETTLEMENT_REQUIRED', event);
  // NO TIMEOUT - assertions run before async handler completes

  expect(mockSDK.signBalanceProof).toHaveBeenCalled(); // FAILS - handler still running
});
```

**Why It Fails:**

- `emit()` returns immediately (synchronous)
- Handler executes asynchronously: `handleSettlement → executeSettlement → SDK calls`
- Assertions run before Promise chain completes

**Solution:**
Use operation-specific timeout guidelines based on complexity

**Timeout Guidelines:**
| Operation Type | Timeout | Use Case |
|---|---|---|
| Basic operations | 50ms | Single async handler (channel open, balance proof) |
| Deposit operations | 100ms | 3 sequential SDK calls (`getChannelState`) |
| Retry operations | 500ms | Exponential backoff with multiple retries |

**Good Example:**

```typescript
it('should handle settlement event', async () => {
  executor.start();

  mockMonitor.emit('SETTLEMENT_REQUIRED', event);

  // Wait for async handler to complete
  await new Promise((resolve) => setTimeout(resolve, 50));

  expect(mockSDK.signBalanceProof).toHaveBeenCalled(); // PASS
});
```

**Documentation Pattern:**

```typescript
/**
 * Test Timeout Guidelines:
 * - Basic operations: 50ms (single async event handler processing)
 * - Deposit operations: 100ms (3 sequential getChannelState calls)
 * - Retry operations: 500ms (exponential backoff with multiple attempts)
 *
 * Why timeouts are needed:
 * Async event handlers process settlement triggers asynchronously via EventEmitter.
 * Tests must await Promise chain completion before assertions.
 */
describe('MyComponent', () => { ... });
```

**Reference:** settlement-executor.test.ts:19-30, 280, 313, 338

---

### Anti-Pattern 3: Mock State Leakage with mockResolvedValue

**Problem:**
Using `mockResolvedValue()` (without `Once`) for functions called multiple times in a test with different expected return values causes state leakage and unpredictable behavior.

**Bad Example:**

```typescript
it('should deposit additional funds', async () => {
  // BAD: All 3 calls return lowDepositState
  mockSDK.getChannelState.mockResolvedValue(lowDepositState);

  // After deposit, we expect highDepositState but get lowDepositState again
  mockSDK.deposit.mockResolvedValue(undefined);

  await executor.handleSettlement(event);

  expect(mockSDK.signBalanceProof).toHaveBeenCalled(); // FAILS - deposit logic broken
});
```

**Why It Fails:**

- `mockResolvedValue()` returns the same value for **every call**
- Test expects different return values for sequential calls (before/after deposit)
- Mock state persists, causing incorrect behavior

**Solution:**
Always use `mockResolvedValueOnce()` for sequential calls with different return values

**Good Example:**

```typescript
it('should deposit additional funds', async () => {
  // Chain mockResolvedValueOnce for sequential calls
  mockSDK.getChannelState
    .mockResolvedValueOnce(lowDepositState) // Call 1: findChannelForPeer
    .mockResolvedValueOnce(lowDepositState) // Call 2: before deposit
    .mockResolvedValueOnce(highDepositState); // Call 3: after deposit

  mockSDK.deposit.mockResolvedValue(undefined);

  await executor.handleSettlement(event);

  expect(mockSDK.deposit).toHaveBeenCalled();
  expect(mockSDK.signBalanceProof).toHaveBeenCalled(); // PASS
});
```

**Best Practices:**

1. **Use `mockResolvedValueOnce()`** for sequential calls with different return values
2. **Use `mockResolvedValue()`** only for default behaviors in `beforeEach()`
3. **Create fresh mock instances** in `beforeEach()` to prevent state leakage
4. **Test isolation:** Run with `--runInBand` to verify no order dependencies

**Good beforeEach() Pattern:**

```typescript
beforeEach(() => {
  // Create fresh mock instances
  mockSDK = {
    openChannel: jest.fn().mockResolvedValue('0xabc123'), // Default
    signBalanceProof: jest.fn().mockResolvedValue('0xsig'), // Default
    getChannelState: jest.fn().mockResolvedValue(defaultState), // Default
  } as any;

  // Create new executor instance (clears internal state)
  executor = new SettlementExecutor(config, mockSDK, ...);
});
```

**Reference:** settlement-executor.test.ts:272-275, 43-95

---

### Testing Checklist for Event-Driven Components

Use this checklist when testing components with event handlers:

- [ ] **Event Listener Cleanup:**
  - [ ] Store bound handler as `private readonly` property
  - [ ] Bind handler once in constructor
  - [ ] Use stored reference in `.on()` and `.off()` calls
  - [ ] Test cleanup with `listenerCount()` assertions

- [ ] **Async Timeout Coverage:**
  - [ ] Add timeout after `emit()` calls in tests
  - [ ] Scale timeout to operation complexity (50ms / 100ms / 500ms)
  - [ ] Document timeout rationale in test comments
  - [ ] Validate with repeated test runs (10x) to detect flakiness

- [ ] **Mock Isolation:**
  - [ ] Create fresh mock instances in `beforeEach()`
  - [ ] Use `mockResolvedValueOnce()` for sequential calls
  - [ ] Verify tests pass with `--runInBand` (sequential execution)
  - [ ] Check no state leakage between tests

---

### Stability Testing

For critical test suites, validate stability with repeated runs:

```bash
# Run tests 10 times to detect flakiness
for i in {1..10}; do
  npm test -- my-test-file.test.ts || echo "Run $i FAILED"
done
```

**Example Stability Script:** `packages/connector/test/stability/run-settlement-tests.sh`

**Success Criteria:**

- 10/10 runs pass (100% success rate)
- No intermittent failures
- Consistent execution time (variance <10%)

**Reference:** Epic 10 Story 10.1 - Settlement Executor Test Stability Validation

---

### Anti-Pattern 4: Hardcoded Timeouts in Production Code

**Problem:**
Tests depend on arbitrary delays hardcoded in application logic, making tests fragile, slow, and environment-dependent.

**Bad Example:**

```typescript
class ConnectionManager {
  async connectWithRetry(): Promise<void> {
    for (let i = 0; i < 3; i++) {
      try {
        await this.connect();
        return;
      } catch (error) {
        // Hardcoded 1-second delay
        await new Promise((resolve) => setTimeout(resolve, 1000));
      }
    }
    throw new Error('Connection failed');
  }
}
```

**Why It Fails:**

- Tests must wait full hardcoded duration (1000ms × 3 retries = 3 seconds)
- Slows down test suite significantly
- Cannot test retry behavior quickly
- Different environments may require different timeouts

**Solution:**
Use event-driven patterns or allow timeout configuration via constructor/config

**Good Example:**

```typescript
class ConnectionManager {
  constructor(private config: { retryDelayMs?: number } = {}) {
    this.retryDelayMs = config.retryDelayMs ?? 1000; // Default 1s, configurable
  }

  async connectWithRetry(): Promise<void> {
    for (let i = 0; i < 3; i++) {
      try {
        await this.connect();
        return;
      } catch (error) {
        // Configurable delay
        await new Promise((resolve) => setTimeout(resolve, this.retryDelayMs));
      }
    }
    throw new Error('Connection failed');
  }
}

// Test can use 1ms delay instead of 1000ms
it('should retry connection 3 times', async () => {
  const manager = new ConnectionManager({ retryDelayMs: 1 }); // Fast for tests
  await expect(manager.connectWithRetry()).rejects.toThrow('Connection failed');
});
```

**Alternative: Event-Driven Pattern**

```typescript
class ConnectionManager extends EventEmitter {
  async connectWithRetry(): Promise<void> {
    for (let i = 0; i < 3; i++) {
      try {
        await this.connect();
        this.emit('connected');
        return;
      } catch (error) {
        this.emit('retry', { attempt: i + 1 });
      }
    }
    throw new Error('Connection failed');
  }
}

// Test listens for events instead of waiting for timeouts
it('should emit retry events', async () => {
  const retryEvents: number[] = [];
  manager.on('retry', (data) => retryEvents.push(data.attempt));

  await expect(manager.connectWithRetry()).rejects.toThrow();
  expect(retryEvents).toEqual([1, 2, 3]);
});
```

**Reference:** Epic 10 general test quality practices

---

### Anti-Pattern 5: Incomplete Test Cleanup (Resources Not Released)

**Problem:**
Open file handles, network connections, timers, or event listeners persist after tests complete, causing resource leaks and test interference.

**Bad Example:**

```typescript
describe('SettlementMonitor', () => {
  let monitor: SettlementMonitor;

  beforeEach(() => {
    monitor = new SettlementMonitor();
    monitor.startPolling(); // Starts setInterval polling
  });

  it('should detect settlement requirement', async () => {
    // Test logic...
  });

  // NO afterEach - polling continues after test, causing leaks
});
```

**Why It Fails:**

- `setInterval` timer continues running after test completes
- Subsequent tests inherit running timers from previous tests
- Resource leaks accumulate across test suite
- Tests may fail with "Jest did not exit one second after test run completed"

**Solution:**
Use `afterEach()` to release all resources

**Good Example:**

```typescript
describe('SettlementMonitor', () => {
  let monitor: SettlementMonitor;
  let server: HTTPServer;
  let client: WebSocketClient;

  beforeEach(() => {
    monitor = new SettlementMonitor();
    monitor.startPolling();
    server = createTestServer();
    client = new WebSocketClient('ws://localhost:8080');
  });

  afterEach(async () => {
    // Clear all timers
    monitor.stopPolling(); // Calls clearInterval internally

    // Close all connections
    await client.disconnect();
    await server.close();

    // Remove all event listeners
    monitor.removeAllListeners();
  });

  it('should detect settlement requirement', async () => {
    // Test logic...
  });
});
```

**Common Resources Requiring Cleanup:**

| Resource Type        | Cleanup Method                          | Example                        |
| -------------------- | --------------------------------------- | ------------------------------ |
| Timers               | `clearInterval(id)`, `clearTimeout(id)` | `monitor.stopPolling()`        |
| Network Connections  | `client.disconnect()`, `server.close()` | `await client.disconnect()`    |
| Event Listeners      | `.off()`, `.removeAllListeners()`       | `emitter.removeAllListeners()` |
| File Handles         | `fs.close()`, `stream.destroy()`        | `await fileHandle.close()`     |
| Database Connections | `connection.close()`                    | `await db.disconnect()`        |

**Detection:**

Run tests with `--detectOpenHandles` to find resource leaks:

```bash
npm test -- --detectOpenHandles my-test.test.ts
```

**Reference:** settlement-executor.test.ts cleanup patterns

---

### Anti-Pattern 6: Testing Implementation Details Instead of Behavior

**Problem:**
Tests break when refactoring internal logic, even though public behavior remains correct. Tests become brittle and discourage refactoring.

**Bad Example:**

```typescript
class PacketRouter {
  private routingTable: Map<string, Route>;

  route(packet: ILPPreparePacket): Route | null {
    // Internal implementation: uses Map.get()
    return this.routingTable.get(packet.destination) ?? null;
  }
}

// BAD: Tests internal implementation (private Map usage)
it('should store routes in internal routing table Map', () => {
  const router = new PacketRouter();
  router.addRoute('g.alice', route);

  // Testing private implementation detail
  expect(router['routingTable'].get('g.alice')).toEqual(route);
});
```

**Why It Fails:**

- Test directly accesses private `routingTable` property
- If implementation changes (Map → Array → Database), test breaks
- Test doesn't validate actual routing behavior
- Discourages refactoring internal logic

**Solution:**
Test public API behavior, not private methods or state

**Good Example:**

```typescript
// GOOD: Tests public behavior (routing logic)
it('should route packet to correct next hop', () => {
  const router = new PacketRouter();
  router.addRoute('g.alice', { nextHop: 'connector-b', fee: 100 });

  const packet = createPreparePacket({ destination: 'g.alice' });
  const route = router.route(packet);

  // Assert on public behavior, not internal state
  expect(route).toEqual({ nextHop: 'connector-b', fee: 100 });
});

it('should return null when no route found', () => {
  const router = new PacketRouter();
  const packet = createPreparePacket({ destination: 'g.unknown' });

  expect(router.route(packet)).toBeNull();
});
```

**Behavior vs Implementation Testing:**

| Test Type          | What It Tests             | Example                     | Brittleness               |
| ------------------ | ------------------------- | --------------------------- | ------------------------- |
| **Implementation** | How code works internally | `routingTable.get()` called | High - breaks on refactor |
| **Behavior**       | What code does externally | Packet routed correctly     | Low - refactor-safe       |

**When Implementation Testing Is Acceptable:**

- Testing critical internal algorithms (e.g., OER encoding logic)
- Verifying security-critical state transitions
- Debugging complex internal behavior (temporary, removed after fix)

**Best Practice:**

Ask: "If I refactor this code, should the test still pass?"

- **Yes:** Good behavior test
- **No:** Probably testing implementation details

**Reference:** test-strategy-and-standards.md philosophy, Epic 10 Story 10.3

---

## Stability Testing Best Practices

**When to Run Stability Tests:**

- After fixing flaky tests (validate fix eliminates flakiness)
- Before production releases (ensure no intermittent failures)
- After refactoring test infrastructure (verify no regressions)
- When adding new async/event-driven components (detect race conditions)

**How to Create Stability Test Scripts:**

Create shell scripts that run tests multiple times and report failures.

**Example: Settlement Executor Stability Script**

```bash
#!/bin/bash
# File: packages/connector/test/stability/run-settlement-tests.sh

TEST_FILE="settlement-executor.test.ts"
RUNS=10
FAILURES=0

echo "Running stability test: $TEST_FILE ($RUNS runs)"

for i in $(seq 1 $RUNS); do
  echo "=== Run $i/$RUNS ==="
  if npm test -- "$TEST_FILE" --silent > /dev/null 2>&1; then
    echo "✓ Pass"
  else
    echo "✗ FAIL"
    ((FAILURES++))
  fi
done

echo ""
echo "Results: $((RUNS - FAILURES))/$RUNS passed"

if [ $FAILURES -eq 0 ]; then
  echo "✓ Stability test PASSED (100% success rate)"
  exit 0
else
  echo "✗ Stability test FAILED ($FAILURES failures)"
  exit 1
fi
```

**Success Criteria:**

| Test Type         | Required Pass Rate  | Number of Runs | Rationale                                                  |
| ----------------- | ------------------- | -------------- | ---------------------------------------------------------- |
| Unit Tests        | 100% (10/10 passes) | 10             | Fast execution, should never be flaky                      |
| Integration Tests | 100% (3/3 passes)   | 3              | Slower execution, network variability acceptable if stable |
| E2E Tests         | 100% (3/3 passes)   | 3              | Very slow, minimal runs, but must be stable                |

**Reference:** Epic 10 Story 10.1 stability testing approach, `packages/connector/test/stability/run-settlement-tests.sh`

---

## Test Isolation Validation Techniques

Proper test isolation ensures tests do not depend on execution order or shared state.

**Technique 1: Run Tests Sequentially with `--runInBand`**

Detects race conditions and timing dependencies that only appear in parallel execution.

```bash
# Run tests in serial (one at a time)
npm test -- --runInBand my-test.test.ts
```

**What It Detects:**

- Tests that fail only when run in parallel
- Shared state mutations across tests
- Race conditions in test setup/teardown

**Technique 2: Run Tests in Random Order with `--randomize` (Jest 28+)**

Detects tests that depend on specific execution order.

```bash
# Randomize test execution order
npm test -- --randomize my-test.test.ts
```

**What It Detects:**

- Test A setup pollutes Test B expectations
- Tests that assume specific beforeEach() execution order
- Global state dependencies

**Technique 3: Run Single Test File in Isolation**

Verifies test file has no dependencies on other workspace packages or test files.

```bash
# Run single test file
npm test -- my-test.test.ts

# Verify no imports from other test files
grep -r "from.*test" my-test.test.ts && echo "WARNING: Test imports from other tests"
```

**What It Detects:**

- Test file imports from other test files (anti-pattern)
- Missing mock setup (relies on global mocks from other tests)
- Workspace-level dependencies not declared in package.json

**Technique 4: Check for Test Interdependencies**

Run tests with `--bail` to stop on first failure, then analyze cascade failures.

```bash
# Stop on first failure
npm test -- --bail my-test.test.ts

# Check if Test B fails when Test A fails
npm test -- --testNamePattern="Test A" && npm test -- --testNamePattern="Test B"
```

**What It Detects:**

- Test B failure caused by Test A failure (interdependency)
- Shared state not cleaned up between tests
- `beforeEach()` not resetting state properly

**Test Isolation Checklist:**

- [ ] Tests pass with `--runInBand` (sequential execution)
- [ ] Tests pass with `--randomize` (random order)
- [ ] Single test file runs independently
- [ ] No test file imports from other test files
- [ ] `beforeEach()` creates fresh instances (no shared state)
- [ ] `afterEach()` releases all resources (timers, connections, listeners)
- [ ] Test A failure does not cause Test B to fail

**Reference:** test-strategy-and-standards.md line 329, Epic 10 Story 10.3

---

## Code Examples from Project Tests

This section provides real examples from the project demonstrating good and bad testing patterns.

**Good Example: Event Listener Cleanup (settlement-executor.test.ts:43-95)**

```typescript
describe('SettlementExecutor - Event Listener Cleanup', () => {
  let executor: SettlementExecutor;
  let mockMonitor: MockSettlementMonitor;

  beforeEach(() => {
    // Create fresh mock instances
    mockMonitor = {
      on: jest.fn(),
      off: jest.fn(),
      listenerCount: jest.fn().mockReturnValue(0),
    } as any;

    mockSDK = {
      openChannel: jest.fn().mockResolvedValue('0xabc123'),
      signBalanceProof: jest.fn().mockResolvedValue('0xsig'),
    } as any;

    // Create executor (binds handlers in constructor)
    executor = new SettlementExecutor(config, mockSDK, mockMonitor, ...);
  });

  afterEach(() => {
    // Ensure cleanup on test failure
    executor.stop();
  });

  it('should register listener on start', () => {
    executor.start();
    expect(mockMonitor.on).toHaveBeenCalledWith(
      'SETTLEMENT_REQUIRED',
      expect.any(Function)
    );
  });

  it('should unregister listener on stop', () => {
    executor.start();
    executor.stop();

    // Verify cleanup succeeded
    expect(mockMonitor.off).toHaveBeenCalledWith(
      'SETTLEMENT_REQUIRED',
      expect.any(Function)
    );
  });

  it('should verify zero listeners after stop', () => {
    mockMonitor.listenerCount.mockReturnValue(1); // Simulate active listener
    executor.start();

    mockMonitor.listenerCount.mockReturnValue(0); // Simulate cleanup
    executor.stop();

    expect(mockMonitor.listenerCount('SETTLEMENT_REQUIRED')).toBe(0);
  });
});
```

**Good Example: Mock Isolation in `beforeEach()` (settlement-executor.test.ts:43-95)**

```typescript
beforeEach(() => {
  // GOOD: Create fresh mock instances every test
  mockSDK = {
    openChannel: jest.fn().mockResolvedValue('0xabc123'), // Default behavior
    deposit: jest.fn().mockResolvedValue(undefined),
    signBalanceProof: jest.fn().mockResolvedValue('0xsig'),
    getChannelState: jest.fn().mockResolvedValue(defaultChannelState),
  } as any;

  mockLogger = {
    info: jest.fn(),
    error: jest.fn(),
    warn: jest.fn(),
  } as any;

  // GOOD: Create new executor instance (clears internal state)
  executor = new SettlementExecutor(config, mockSDK, mockMonitor, mockLogger);
});
```

**Bad Example: Inline bind(this) Anti-Pattern (documented in RCA)**

From docs/qa/root-cause-analysis-10.1.md:

```typescript
// BAD: settlement-executor.ts before fix (inline bind)
class SettlementExecutor {
  start(): void {
    // Each bind() creates new function reference
    this.settlementMonitor.on('SETTLEMENT_REQUIRED', this.handleSettlement.bind(this));
  }

  stop(): void {
    // Different reference - cleanup FAILS
    this.settlementMonitor.off('SETTLEMENT_REQUIRED', this.handleSettlement.bind(this));
  }
}
```

**Fix:** Store bound handler in constructor (settlement-executor.ts:61-62, 90, 99, 113):

```typescript
// GOOD: settlement-executor.ts after fix
class SettlementExecutor {
  private readonly boundHandleSettlement: (event: SettlementEvent) => Promise<void>;

  constructor(...) {
    // Bind once in constructor
    this.boundHandleSettlement = this.handleSettlement.bind(this);
  }

  start(): void {
    this.monitor.on('SETTLEMENT_REQUIRED', this.boundHandleSettlement);
  }

  stop(): void {
    this.monitor.off('SETTLEMENT_REQUIRED', this.boundHandleSettlement); // Same reference
  }
}
```

**Reference:** Epic 10 Story 10.1 implementation examples, settlement-executor.test.ts, docs/qa/root-cause-analysis-10.1.md
