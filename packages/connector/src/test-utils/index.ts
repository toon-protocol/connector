/**
 * Test Utilities
 *
 * This module provides utilities for writing isolated, reliable tests:
 *
 * - IsolatedTestEnv: Automatic temp directory management and cleanup
 * - Mock Factories: Properly typed mock objects for common dependencies
 *
 * Usage:
 * ```typescript
 * import {
 *   IsolatedTestEnv,
 *   createMockWalletDerivation,
 *   createMockBalanceTracker,
 *   createWalletMocks,
 * } from '../test-utils';
 * ```
 */

// Isolated test environment
export {
  IsolatedTestEnv,
  createIsolatedTestHelper,
  type CleanupFunction,
  type ComponentWithClose,
} from './isolated-test-env';

// Mock factories
export {
  // Test data constants
  TEST_EVM_ADDRESS,
  TEST_AGENT_ID,
  TEST_PASSWORD,
  // Individual mock factories
  // REMOVED: Epic 16 (AI Agent Infrastructure) was deferred - agent wallet mocks commented out
  // createMockWalletDerivation,
  // createMockBalanceTracker,
  // createMockWalletLifecycle,
  // createMockWalletFunder,
  createMockTreasuryWallet,
  createMockFraudDetector,
  createMockEvmProvider,
  createMockLogger,
  // Composite helper
  // REMOVED: Epic 16 (AI Agent Infrastructure) was deferred
  // createWalletMocks,
  // Option types
  // REMOVED: Epic 16 (AI Agent Infrastructure) was deferred - agent wallet option types commented out
  // type MockWalletDerivationOptions,
  // type MockBalanceTrackerOptions,
  // type MockWalletLifecycleOptions,
  type MockFraudDetectorOptions,
  type MockEvmProviderOptions,
} from './mock-factories';
