/**
 * Mock Factory Functions
 *
 * Provides properly typed mock factories for common dependencies.
 * Using these factories instead of `{} as Type` ensures:
 * - All required methods are present
 * - TypeScript catches missing method implementations
 * - Tests don't fail with "X is not a function" errors
 *
 * Usage:
 * ```typescript
 * const mockDerivation = createMockWalletDerivation();
 * const mockTracker = createMockBalanceTracker({
 *   getBalance: jest.fn().mockResolvedValue(100n), // Override specific methods
 * });
 * ```
 */

import { ethers } from 'ethers';
import type { Logger } from 'pino';

// Agent wallet imports removed - Epic 16 infrastructure deferred
// import type { AgentWalletDerivation, AgentWallet } from '../wallet/agent-wallet-derivation';
// import type { AgentBalanceTracker, AgentBalance } from '../wallet/agent-balance-tracker';
// import type { AgentWalletLifecycle, WalletLifecycleRecord } from '../wallet/agent-wallet-lifecycle';
// import { WalletState } from '../wallet/agent-wallet-lifecycle';
// import type { AgentWalletFunder, FundingResult } from '../wallet/agent-wallet-funder';
import type { TreasuryWallet } from '../wallet/treasury-wallet';
import type { TelemetryEmitter } from '../telemetry/telemetry-emitter';
import type { FraudDetector, FraudCheckResult } from '../wallet/wallet-security';

// ============================================================================
// Test Data Constants
// ============================================================================

/** Valid EVM address for testing (40 hex chars after 0x) */
export const TEST_EVM_ADDRESS = '0x742d35Cc6634C0532925a3b844Bc454321f0bEb0';

/** Default test agent ID */
export const TEST_AGENT_ID = 'test-agent-001';

/** Default test password meeting security requirements */
export const TEST_PASSWORD = 'TestP@ssw0rd12345678';

// ============================================================================
// Wallet Derivation Mocks
// ============================================================================
// REMOVED: Epic 16 (AI Agent Infrastructure) was deferred and these modules were deleted
// The following agent wallet functionality is no longer available:
// - agent-wallet-derivation
// - agent-balance-tracker
// - agent-wallet-lifecycle
// - agent-wallet-funder
//
// These mock factories are commented out to prevent TypeScript compilation errors.
// If Epic 16 is resumed, these can be uncommented and the imports restored.

/*
export interface MockWalletDerivationOptions {
  agentId?: string;
  evmAddress?: string;
  derivationIndex?: number;
}

/**
 * Create a mock AgentWalletDerivation with all required methods
 *\/
export function createMockWalletDerivation(
  options: MockWalletDerivationOptions = {},
  overrides?: Partial<jest.Mocked<AgentWalletDerivation>>
): jest.Mocked<AgentWalletDerivation> {
  const {
    agentId = TEST_AGENT_ID,
    evmAddress = TEST_EVM_ADDRESS,
    derivationIndex = 0,
  } = options;

  const defaultWallet: AgentWallet = {
    agentId,
    evmAddress,
    derivationIndex,
    createdAt: Date.now(),
  };

  return {
    deriveAgentWallet: jest.fn().mockImplementation((id: string) =>
      Promise.resolve({
        ...defaultWallet,
        agentId: id,
      })
    ),
    getAgentWallet: jest.fn().mockImplementation((id: string) =>
      Promise.resolve({
        ...defaultWallet,
        agentId: id,
      })
    ),
    getAllWallets: jest.fn().mockReturnValue([defaultWallet]),
    getAgentSigner: jest.fn().mockResolvedValue({} as ethers.Wallet),
    close: jest.fn(),
    ...overrides,
  } as unknown as jest.Mocked<AgentWalletDerivation>;
}
*/

// ============================================================================
// Balance Tracker Mocks
// ============================================================================
// REMOVED: Epic 16 (AI Agent Infrastructure) was deferred

/*
export interface MockBalanceTrackerOptions {
  defaultBalance?: bigint;
  balances?: AgentBalance[];
}

/**
 * Create a mock AgentBalanceTracker with all required methods
 *\/
export function createMockBalanceTracker(
  options: MockBalanceTrackerOptions = {},
  overrides?: Partial<jest.Mocked<AgentBalanceTracker>>
): jest.Mocked<AgentBalanceTracker> {
  const { defaultBalance = BigInt('1000000000000000000'), balances } = options;

  const defaultBalances: AgentBalance[] = balances || [
    {
      agentId: TEST_AGENT_ID,
      chain: 'evm',
      token: 'ETH',
      balance: defaultBalance,
      lastUpdated: Date.now(),
    },
    {
      agentId: TEST_AGENT_ID,
      chain: 'evm',
      token: 'USDC',
      balance: BigInt('1000000000'),
      lastUpdated: Date.now(),
    },
  ];

  return {
    getBalance: jest.fn().mockResolvedValue(defaultBalance),
    getAllBalances: jest.fn().mockImplementation((agentId: string) =>
      Promise.resolve(
        defaultBalances.map((b) => ({
          ...b,
          agentId,
        }))
      )
    ),
    refreshBalance: jest.fn().mockResolvedValue(undefined),
    refreshAllBalances: jest.fn().mockResolvedValue(undefined),
    startPolling: jest.fn(),
    stopPolling: jest.fn(),
    ...overrides,
  } as unknown as jest.Mocked<AgentBalanceTracker>;
}
*/

// ============================================================================
// Wallet Lifecycle Mocks
// ============================================================================
// REMOVED: Epic 16 (AI Agent Infrastructure) was deferred

/*
export interface MockWalletLifecycleOptions {
  defaultState?: WalletState;
}

/**
 * Create a mock AgentWalletLifecycle with all required methods
 *\/
export function createMockWalletLifecycle(
  options: MockWalletLifecycleOptions = {},
  overrides?: Partial<jest.Mocked<AgentWalletLifecycle>>
): jest.Mocked<AgentWalletLifecycle> {
  const { defaultState = WalletState.ACTIVE } = options;

  const createRecord = (agentId: string): WalletLifecycleRecord => ({
    agentId,
    state: defaultState,
    createdAt: Date.now(),
    activatedAt: defaultState === WalletState.ACTIVE ? Date.now() : undefined,
    lastActivity: Date.now(),
    totalTransactions: 0,
    totalVolume: {},
  });

  return {
    createAgentWallet: jest
      .fn()
      .mockImplementation((agentId: string) => Promise.resolve(createRecord(agentId))),
    getLifecycleRecord: jest
      .fn()
      .mockImplementation((agentId: string) => Promise.resolve(createRecord(agentId))),
    suspendWallet: jest.fn().mockResolvedValue(undefined),
    reactivateWallet: jest.fn().mockResolvedValue(undefined),
    archiveWallet: jest.fn().mockResolvedValue(undefined),
    recordActivity: jest.fn().mockResolvedValue(undefined),
    getTransactionCount: jest.fn().mockReturnValue(0),
    close: jest.fn(),
    ...overrides,
  } as unknown as jest.Mocked<AgentWalletLifecycle>;
}
*/

// ============================================================================
// Wallet Funder Mocks
// ============================================================================
// REMOVED: Epic 16 (AI Agent Infrastructure) was deferred

/*
/**
 * Create a mock AgentWalletFunder with all required methods
 *\/
export function createMockWalletFunder(
  overrides?: Partial<jest.Mocked<AgentWalletFunder>>
): jest.Mocked<AgentWalletFunder> {
  const defaultFundingResult: FundingResult = {
    agentId: TEST_AGENT_ID,
    transactions: [],
    timestamp: Date.now(),
  };

  return {
    fundAgentWallet: jest.fn().mockResolvedValue(defaultFundingResult),
    checkFundingStatus: jest.fn().mockResolvedValue({ funded: true }),
    ...overrides,
  } as unknown as jest.Mocked<AgentWalletFunder>;
}
*/

// ============================================================================
// Treasury Wallet Mocks
// ============================================================================

/**
 * Create a mock TreasuryWallet with all required methods
 */
export function createMockTreasuryWallet(
  overrides?: Partial<jest.Mocked<TreasuryWallet>>
): jest.Mocked<TreasuryWallet> {
  return {
    sendETH: jest.fn().mockResolvedValue({ hash: '0x' + '1'.repeat(64), to: TEST_EVM_ADDRESS }),
    sendERC20: jest.fn().mockResolvedValue({ hash: '0x' + '1'.repeat(64), to: TEST_EVM_ADDRESS }),
    getBalance: jest.fn().mockResolvedValue(BigInt('10000000000000000000')),
    evmAddress: TEST_EVM_ADDRESS,
    ...overrides,
  } as unknown as jest.Mocked<TreasuryWallet>;
}

// ============================================================================
// Telemetry Mocks
// ============================================================================

/**
 * Create a mock TelemetryEmitter with all required methods
 */
export function createMockTelemetryEmitter(
  overrides?: Partial<jest.Mocked<TelemetryEmitter>>
): jest.Mocked<TelemetryEmitter> {
  return {
    emit: jest.fn(),
    flush: jest.fn().mockResolvedValue(undefined),
    ...overrides,
  } as unknown as jest.Mocked<TelemetryEmitter>;
}

// ============================================================================
// Fraud Detector Mocks
// ============================================================================

export interface MockFraudDetectorOptions {
  shouldDetectFraud?: boolean;
  fraudScore?: number;
  fraudReason?: string;
}

/**
 * Create a mock FraudDetector with all required methods
 */
export function createMockFraudDetector(
  options: MockFraudDetectorOptions = {},
  overrides?: Partial<jest.Mocked<FraudDetector>>
): jest.Mocked<FraudDetector> {
  const { shouldDetectFraud = false, fraudScore = 0, fraudReason = 'Test fraud' } = options;

  const defaultResult: FraudCheckResult = {
    detected: shouldDetectFraud,
    score: fraudScore,
    reason: shouldDetectFraud ? fraudReason : undefined,
  };

  return {
    analyzeTransaction: jest.fn().mockResolvedValue(defaultResult),
    ...overrides,
  } as jest.Mocked<FraudDetector>;
}

// ============================================================================
// External Provider Mocks
// ============================================================================

export interface MockEvmProviderOptions {
  balance?: bigint;
  chainId?: bigint;
  networkName?: string;
}

/**
 * Create a mock ethers.Provider with commonly used methods
 */
export function createMockEvmProvider(
  options: MockEvmProviderOptions = {},
  overrides?: Partial<jest.Mocked<ethers.Provider>>
): jest.Mocked<ethers.Provider> {
  const {
    balance = BigInt('1000000000000000000'),
    chainId = 8453n,
    networkName = 'base',
  } = options;

  return {
    getBalance: jest.fn().mockResolvedValue(balance),
    getNetwork: jest.fn().mockResolvedValue({ chainId, name: networkName }),
    getTransactionReceipt: jest.fn().mockResolvedValue({ status: 1 }),
    waitForTransaction: jest.fn().mockResolvedValue({ status: 1 }),
    ...overrides,
  } as unknown as jest.Mocked<ethers.Provider>;
}

// ============================================================================
// Logger Mocks
// ============================================================================

/**
 * Create a mock Pino logger that captures log calls
 */
export function createMockLogger(): jest.Mocked<Logger> {
  const mockLogger: Record<string, jest.Mock | string> = {
    trace: jest.fn().mockReturnThis(),
    debug: jest.fn().mockReturnThis(),
    info: jest.fn().mockReturnThis(),
    warn: jest.fn().mockReturnThis(),
    error: jest.fn().mockReturnThis(),
    fatal: jest.fn().mockReturnThis(),
    child: jest.fn(),
    level: 'silent',
  };

  // child() should return the same mock
  (mockLogger.child as jest.Mock).mockReturnValue(mockLogger);

  return mockLogger as unknown as jest.Mocked<Logger>;
}

// ============================================================================
// Composite Mock Helpers
// ============================================================================
// REMOVED: Epic 16 (AI Agent Infrastructure) was deferred
// The createWalletMocks composite helper has been removed because it depends on
// the agent wallet infrastructure that was deleted.

/*
/**
 * Create a complete set of wallet-related mocks
 * Useful for tests that need the full wallet infrastructure
 *\/
export function createWalletMocks(options?: {
  derivation?: MockWalletDerivationOptions;
  balanceTracker?: MockBalanceTrackerOptions;
  lifecycle?: MockWalletLifecycleOptions;
  fraudDetector?: MockFraudDetectorOptions;
  evmProvider?: MockEvmProviderOptions;
}): {
  walletDerivation: jest.Mocked<AgentWalletDerivation>;
  balanceTracker: jest.Mocked<AgentBalanceTracker>;
  lifecycle: jest.Mocked<AgentWalletLifecycle>;
  walletFunder: jest.Mocked<AgentWalletFunder>;
  treasuryWallet: jest.Mocked<TreasuryWallet>;
  telemetryEmitter: jest.Mocked<TelemetryEmitter>;
  fraudDetector: jest.Mocked<FraudDetector>;
  evmProvider: jest.Mocked<ethers.Provider>;
  logger: jest.Mocked<Logger>;
} {
  return {
    walletDerivation: createMockWalletDerivation(options?.derivation),
    balanceTracker: createMockBalanceTracker(options?.balanceTracker),
    lifecycle: createMockWalletLifecycle(options?.lifecycle),
    walletFunder: createMockWalletFunder(),
    treasuryWallet: createMockTreasuryWallet(),
    telemetryEmitter: createMockTelemetryEmitter(),
    fraudDetector: createMockFraudDetector(options?.fraudDetector),
    evmProvider: createMockEvmProvider(options?.evmProvider),
    logger: createMockLogger(),
  };
}
*/
