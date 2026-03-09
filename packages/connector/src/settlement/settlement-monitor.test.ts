/**
 * Unit tests for SettlementMonitor
 *
 * Tests threshold detection, state management, duplicate prevention,
 * configuration hierarchy, error handling, and telemetry integration.
 *
 * @module settlement/settlement-monitor.test
 */

import { SettlementMonitor, SettlementMonitorConfig } from './settlement-monitor';
import { AccountManager } from './account-manager';
import { SettlementState, SettlementTriggerEvent } from '../config/types';
import { TelemetryEmitter } from '../telemetry/telemetry-emitter';
import { Logger } from 'pino';
import pino from 'pino';

// Mock AccountManager
jest.mock('./account-manager');

/**
 * Helper to access private _checkBalances method for testing
 */
function checkBalances(monitor: SettlementMonitor): Promise<void> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return (monitor as any)._checkBalances();
}

/**
 * Helper to access private _isRunning field for testing
 */
function isRunning(monitor: SettlementMonitor): boolean {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return (monitor as any)._isRunning;
}

describe('SettlementMonitor Threshold Detection', () => {
  let settlementMonitor: SettlementMonitor;
  let mockAccountManager: jest.Mocked<AccountManager>;
  let mockLogger: Logger;
  let mockTelemetryEmitter: jest.Mocked<TelemetryEmitter>;

  beforeEach(() => {
    // Create mock logger using pino silent mode for tests
    mockLogger = pino({ level: 'silent' });

    // Spy on logger methods
    jest.spyOn(mockLogger, 'info');
    jest.spyOn(mockLogger, 'warn');
    jest.spyOn(mockLogger, 'error');
    jest.spyOn(mockLogger, 'debug');
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    jest.spyOn(mockLogger, 'child').mockReturnValue(mockLogger as any);

    // Create mock AccountManager
    mockAccountManager = {
      getAccountBalance: jest.fn(),
    } as unknown as jest.Mocked<AccountManager>;

    // Create mock TelemetryEmitter
    mockTelemetryEmitter = {
      connect: jest.fn().mockResolvedValue(undefined),
      disconnect: jest.fn().mockResolvedValue(undefined),
      isConnected: jest.fn().mockReturnValue(true),
      emitNodeStatus: jest.fn(),
      emitPacketReceived: jest.fn(),
      emitPacketSent: jest.fn(),
      emitRouteLookup: jest.fn(),
      emitLog: jest.fn(),
      emit: jest.fn(),
    } as unknown as jest.Mocked<TelemetryEmitter>;

    // Reset all mocks
    jest.clearAllMocks();
  });

  afterEach(async () => {
    // Stop monitor if running
    if (settlementMonitor) {
      await settlementMonitor.stop();
    }
  });

  describe('Initialization', () => {
    it('should initialize with valid config', () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
          pollingInterval: 30000,
        },
        peers: ['peer-a', 'peer-b'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      expect(settlementMonitor).toBeDefined();
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          pollingInterval: 30000,
          defaultThreshold: '1000',
          peerCount: 2,
          tokenCount: 1,
        }),
        'Settlement monitor initialized'
      );
    });

    it('should initialize all peer-token pairs to IDLE state', () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
        },
        peers: ['peer-a', 'peer-b'],
        tokenIds: ['ILP', 'USDC'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      expect(settlementMonitor.getSettlementState('peer-a', 'ILP')).toBe(SettlementState.IDLE);
      expect(settlementMonitor.getSettlementState('peer-a', 'USDC')).toBe(SettlementState.IDLE);
      expect(settlementMonitor.getSettlementState('peer-b', 'ILP')).toBe(SettlementState.IDLE);
      expect(settlementMonitor.getSettlementState('peer-b', 'USDC')).toBe(SettlementState.IDLE);
    });
  });

  describe('Threshold Detection', () => {
    it('should emit SETTLEMENT_REQUIRED when balance exceeds threshold', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
          pollingInterval: 100,
        },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      // Mock balance exceeding threshold
      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 1500n,
        debitBalance: 0n,
        netBalance: 1500n,
      });

      // Listen for settlement event
      const settlementEventPromise = new Promise<SettlementTriggerEvent>((resolve) => {
        settlementMonitor.once('SETTLEMENT_REQUIRED', resolve);
      });

      // Start monitor (runs initial check immediately)
      await settlementMonitor.start();

      // Wait for event
      const event = await settlementEventPromise;

      expect(event).toEqual({
        peerId: 'peer-a',
        tokenId: 'ILP',
        currentBalance: 1500n,
        threshold: 1000n,
        exceedsBy: 500n,
        timestamp: expect.any(Date),
      });

      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          peerId: 'peer-a',
          tokenId: 'ILP',
          balance: '1500',
          threshold: '1000',
          exceedsBy: '500',
        }),
        'Settlement threshold exceeded'
      );
    });

    it('should NOT emit event when balance is below threshold', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
        },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      // Mock balance below threshold
      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 900n,
        debitBalance: 0n,
        netBalance: 900n,
      });

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // Manually trigger check
      await checkBalances(settlementMonitor);

      expect(eventListener).not.toHaveBeenCalled();
      expect(settlementMonitor.getSettlementState('peer-a', 'ILP')).toBe(SettlementState.IDLE);
    });

    it('should NOT emit event when balance equals threshold exactly', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
        },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      // Mock balance exactly at threshold
      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 1000n,
        debitBalance: 0n,
        netBalance: 1000n,
      });

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // Manually trigger check
      await checkBalances(settlementMonitor);

      expect(eventListener).not.toHaveBeenCalled();
    });

    it('should skip threshold check when no threshold configured', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: undefined, // No thresholds configured
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      // Mock high balance
      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 10000n,
        debitBalance: 0n,
        netBalance: 10000n,
      });

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // Manually trigger check
      await checkBalances(settlementMonitor);

      expect(eventListener).not.toHaveBeenCalled();
      expect(mockAccountManager.getAccountBalance).not.toHaveBeenCalled();
    });
  });

  describe('Duplicate Prevention', () => {
    it('should NOT emit duplicate triggers when balance remains above threshold', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
        },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      // Mock balance exceeding threshold
      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 1500n,
        debitBalance: 0n,
        netBalance: 1500n,
      });

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // First check - should trigger
      await checkBalances(settlementMonitor);
      expect(eventListener).toHaveBeenCalledTimes(1);
      expect(settlementMonitor.getSettlementState('peer-a', 'ILP')).toBe(
        SettlementState.SETTLEMENT_PENDING
      );

      // Second check - should NOT trigger (state is SETTLEMENT_PENDING)
      await checkBalances(settlementMonitor);
      expect(eventListener).toHaveBeenCalledTimes(1); // Still only 1 call
    });

    it('should reset state to IDLE when balance drops below threshold', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
        },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      // First: balance exceeds threshold
      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 1500n,
        debitBalance: 0n,
        netBalance: 1500n,
      });

      await checkBalances(settlementMonitor);
      expect(settlementMonitor.getSettlementState('peer-a', 'ILP')).toBe(
        SettlementState.SETTLEMENT_PENDING
      );

      // Then: balance drops below threshold
      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 900n,
        debitBalance: 0n,
        netBalance: 900n,
      });

      await checkBalances(settlementMonitor);
      expect(settlementMonitor.getSettlementState('peer-a', 'ILP')).toBe(SettlementState.IDLE);

      expect(mockLogger.info).toHaveBeenCalledWith(
        { peerId: 'peer-a', tokenId: 'ILP' },
        'Balance returned below threshold, resetting to IDLE'
      );
    });
  });

  describe('Configuration Hierarchy', () => {
    it('should use per-peer threshold override instead of default', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
          perPeerThresholds: new Map([['peer-a', 2000n]]),
        },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      // Balance at 1500 - exceeds default (1000) but below peer-specific (2000)
      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 1500n,
        debitBalance: 0n,
        netBalance: 1500n,
      });

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      await checkBalances(settlementMonitor);

      // Should NOT trigger because peer-specific threshold (2000) not exceeded
      expect(eventListener).not.toHaveBeenCalled();
    });

    it('should use token-specific threshold instead of per-peer threshold', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
          perPeerThresholds: new Map([['peer-a', 1000n]]),
          perTokenThresholds: new Map([['peer-a', new Map([['USDC', 500n]])]]),
        },
        peers: ['peer-a'],
        tokenIds: ['USDC'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      // Balance at 600 - exceeds token-specific threshold (500)
      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 600n,
        debitBalance: 0n,
        netBalance: 600n,
      });

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      await checkBalances(settlementMonitor);

      // Should trigger because token-specific threshold (500) exceeded
      expect(eventListener).toHaveBeenCalledTimes(1);
      expect(eventListener).toHaveBeenCalledWith(
        expect.objectContaining({
          peerId: 'peer-a',
          tokenId: 'USDC',
          threshold: 500n,
        })
      );
    });
  });

  describe('State Management', () => {
    it('should transition IDLE → SETTLEMENT_PENDING on threshold crossing', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      expect(settlementMonitor.getSettlementState('peer-a', 'ILP')).toBe(SettlementState.IDLE);

      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 1500n,
        debitBalance: 0n,
        netBalance: 1500n,
      });

      await checkBalances(settlementMonitor);

      expect(settlementMonitor.getSettlementState('peer-a', 'ILP')).toBe(
        SettlementState.SETTLEMENT_PENDING
      );
    });

    it('should transition SETTLEMENT_PENDING → SETTLEMENT_IN_PROGRESS via markSettlementInProgress', () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      settlementMonitor.markSettlementInProgress('peer-a', 'ILP');

      expect(settlementMonitor.getSettlementState('peer-a', 'ILP')).toBe(
        SettlementState.SETTLEMENT_IN_PROGRESS
      );

      expect(mockLogger.info).toHaveBeenCalledWith(
        { peerId: 'peer-a', tokenId: 'ILP' },
        'Settlement marked in progress'
      );
    });

    it('should transition SETTLEMENT_IN_PROGRESS → IDLE via markSettlementCompleted', () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      // Manually set to IN_PROGRESS
      settlementMonitor.markSettlementInProgress('peer-a', 'ILP');
      expect(settlementMonitor.getSettlementState('peer-a', 'ILP')).toBe(
        SettlementState.SETTLEMENT_IN_PROGRESS
      );

      // Mark completed
      settlementMonitor.markSettlementCompleted('peer-a', 'ILP');

      expect(settlementMonitor.getSettlementState('peer-a', 'ILP')).toBe(SettlementState.IDLE);

      expect(mockLogger.info).toHaveBeenCalledWith(
        { peerId: 'peer-a', tokenId: 'ILP' },
        'Settlement completed, state reset to IDLE'
      );
    });

    it('should return all settlement states via getAllSettlementStates', () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a', 'peer-b'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      const states = settlementMonitor.getAllSettlementStates();

      expect(states.size).toBe(2);
      expect(states.get('peer-a:ILP')).toBe(SettlementState.IDLE);
      expect(states.get('peer-b:ILP')).toBe(SettlementState.IDLE);

      // Verify it's a copy (mutations don't affect internal state)
      states.set('peer-a:ILP', SettlementState.SETTLEMENT_PENDING);
      expect(settlementMonitor.getSettlementState('peer-a', 'ILP')).toBe(SettlementState.IDLE);
    });
  });

  describe('Error Handling', () => {
    it('should handle AccountManager errors gracefully', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      // Mock getAccountBalance to throw error
      mockAccountManager.getAccountBalance.mockRejectedValue(
        new Error('TigerBeetle connection failed')
      );

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // Should not throw
      await expect(checkBalances(settlementMonitor)).resolves.not.toThrow();

      expect(mockLogger.error).toHaveBeenCalledWith(
        { error: 'TigerBeetle connection failed' },
        'Settlement threshold check failed'
      );

      // No events should be emitted
      expect(eventListener).not.toHaveBeenCalled();
    });

    it('should continue monitoring after error', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      // First check fails
      mockAccountManager.getAccountBalance.mockRejectedValueOnce(new Error('Temporary error'));

      await checkBalances(settlementMonitor);

      // Second check succeeds
      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 1500n,
        debitBalance: 0n,
        netBalance: 1500n,
      });

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      await checkBalances(settlementMonitor);

      // Should emit event successfully
      expect(eventListener).toHaveBeenCalledTimes(1);
    });
  });

  describe('Telemetry Integration', () => {
    it('should emit telemetry event when threshold exceeded', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
        telemetryEmitter: mockTelemetryEmitter,
        nodeId: 'test-node',
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 1200n,
        debitBalance: 0n,
        netBalance: 1200n,
      });

      await checkBalances(settlementMonitor);

      expect(mockTelemetryEmitter.emit).toHaveBeenCalledWith({
        type: 'SETTLEMENT_TRIGGERED',
        nodeId: 'test-node',
        peerId: 'peer-a',
        tokenId: 'ILP',
        currentBalance: '1200',
        threshold: '1000',
        exceedsBy: '200',
        triggerReason: 'THRESHOLD_EXCEEDED',
        timestamp: expect.any(String),
      });

      expect(mockLogger.debug).toHaveBeenCalledWith(
        { peerId: 'peer-a', tokenId: 'ILP' },
        'Settlement trigger telemetry sent to dashboard'
      );
    });

    it('should NOT emit telemetry when no telemetry emitter configured', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
        // No telemetryEmitter
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 1200n,
        debitBalance: 0n,
        netBalance: 1200n,
      });

      await checkBalances(settlementMonitor);

      // Should NOT throw or log errors
      expect(mockLogger.error).not.toHaveBeenCalled();
    });

    it('should handle telemetry emission errors gracefully', async () => {
      mockTelemetryEmitter.emit.mockImplementation(() => {
        throw new Error('Telemetry server unreachable');
      });

      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
        telemetryEmitter: mockTelemetryEmitter,
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 1200n,
        debitBalance: 0n,
        netBalance: 1200n,
      });

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      await checkBalances(settlementMonitor);

      // Settlement event should still be emitted (non-blocking telemetry)
      expect(eventListener).toHaveBeenCalledTimes(1);

      expect(mockLogger.error).toHaveBeenCalledWith(
        { error: 'Telemetry server unreachable', peerId: 'peer-a', tokenId: 'ILP' },
        'Failed to emit settlement telemetry'
      );
    });
  });

  describe('Start and Stop', () => {
    it('should start and stop polling correctly', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n, pollingInterval: 100 },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 500n,
        debitBalance: 0n,
        netBalance: 500n,
      });

      await settlementMonitor.start();

      expect(mockLogger.info).toHaveBeenCalledWith('Settlement monitor started');
      expect(isRunning(settlementMonitor)).toBe(true);

      await settlementMonitor.stop();

      expect(mockLogger.info).toHaveBeenCalledWith('Settlement monitor stopped');
      expect(isRunning(settlementMonitor)).toBe(false);
    });

    it('should throw error if started when already running', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 500n,
        debitBalance: 0n,
        netBalance: 500n,
      });

      await settlementMonitor.start();

      await expect(settlementMonitor.start()).rejects.toThrow('Settlement monitor already running');
    });

    it('should run initial check immediately on start', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n, pollingInterval: 100000 }, // Very long interval
        peers: ['peer-a'],
        tokenIds: ['ILP'],
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 1500n,
        debitBalance: 0n,
        netBalance: 1500n,
      });

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      await settlementMonitor.start();

      // Event should fire immediately (before interval timer)
      expect(eventListener).toHaveBeenCalledTimes(1);
    });
  });

  describe('Time-Based Settlement Threshold', () => {
    it('should trigger settlement when time interval exceeded and balance > 0', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 999999n, // Very high amount threshold (won't trigger)
          pollingInterval: 30000,
          timeBasedIntervalMs: 100, // 100ms time interval for testing
        },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
        nodeId: 'test-node',
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      // Return balance below amount threshold but > 0
      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 500n, // Below 999999n threshold
        debitBalance: 0n,
        netBalance: 500n,
      });

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // First check: time threshold starts at 0, so interval is exceeded
      await checkBalances(settlementMonitor);

      expect(eventListener).toHaveBeenCalledTimes(1);
      expect(eventListener).toHaveBeenCalledWith(
        expect.objectContaining({
          peerId: 'peer-a',
          tokenId: 'ILP',
          currentBalance: 500n,
        })
      );
    });

    it('should NOT trigger time-based settlement when balance is 0', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 999999n,
          pollingInterval: 30000,
          timeBasedIntervalMs: 100,
        },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
        nodeId: 'test-node',
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 0n,
        debitBalance: 0n,
        netBalance: 0n,
      });

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      await checkBalances(settlementMonitor);

      expect(eventListener).not.toHaveBeenCalled();
    });

    it('should NOT trigger time-based settlement when state is not IDLE', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 999999n,
          pollingInterval: 30000,
          timeBasedIntervalMs: 100,
        },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
        nodeId: 'test-node',
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 500n,
        debitBalance: 0n,
        netBalance: 500n,
      });

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // First check triggers settlement (time exceeded)
      await checkBalances(settlementMonitor);
      expect(eventListener).toHaveBeenCalledTimes(1);

      // Second check: state is SETTLEMENT_PENDING, should NOT trigger again
      await checkBalances(settlementMonitor);
      expect(eventListener).toHaveBeenCalledTimes(1);
    });

    it('should record last settlement time on markSettlementCompleted', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 999999n,
          pollingInterval: 30000,
          timeBasedIntervalMs: 60000, // 60s interval
        },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
        nodeId: 'test-node',
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 500n,
        debitBalance: 0n,
        netBalance: 500n,
      });

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // First check triggers (time starts at 0)
      await checkBalances(settlementMonitor);
      expect(eventListener).toHaveBeenCalledTimes(1);

      // Complete settlement
      settlementMonitor.markSettlementCompleted('peer-a', 'ILP');

      // Immediately check again — 60s hasn't passed since completion
      await checkBalances(settlementMonitor);
      // Should NOT trigger because interval hasn't elapsed since last settlement
      expect(eventListener).toHaveBeenCalledTimes(1);
    });

    it('should NOT trigger time-based when timeBasedIntervalMs is not configured', async () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 999999n,
          pollingInterval: 30000,
          // No timeBasedIntervalMs
        },
        peers: ['peer-a'],
        tokenIds: ['ILP'],
        nodeId: 'test-node',
      };

      settlementMonitor = new SettlementMonitor(config, mockAccountManager, mockLogger);

      mockAccountManager.getAccountBalance.mockResolvedValue({
        creditBalance: 500n,
        debitBalance: 0n,
        netBalance: 500n,
      });

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      await checkBalances(settlementMonitor);

      // Only amount-based triggers; 500 < 999999 so no trigger
      expect(eventListener).not.toHaveBeenCalled();
    });
  });
});
