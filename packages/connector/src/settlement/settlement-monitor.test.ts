/**
 * Unit tests for SettlementMonitor (event-driven)
 *
 * Tests event-driven threshold detection via ClaimReceiver events,
 * state management, duplicate prevention, configuration hierarchy,
 * and error handling.
 *
 * @module settlement/settlement-monitor.test
 */

import { SettlementMonitor, SettlementMonitorConfig } from './settlement-monitor';
import { ClaimReceiver, ClaimReceivedEvent } from './claim-receiver';
import { SettlementState } from '../config/types';
import { EventEmitter } from 'events';
import pino from 'pino';
import type { Logger } from 'pino';

/**
 * Create a mock ClaimReceiver that extends EventEmitter
 * (only the event emitter functionality is needed for testing)
 */
function createMockClaimReceiver(): ClaimReceiver {
  return new EventEmitter() as unknown as ClaimReceiver;
}

describe('SettlementMonitor Event-Driven Threshold Detection', () => {
  let settlementMonitor: SettlementMonitor;
  let mockClaimReceiver: ClaimReceiver;
  let mockLogger: Logger;

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

    // Create mock ClaimReceiver
    mockClaimReceiver = createMockClaimReceiver();

    // Reset all mocks
    jest.clearAllMocks();
  });

  afterEach(() => {
    // Stop monitor if running
    if (settlementMonitor) {
      settlementMonitor.stop();
    }
  });

  describe('Initialization', () => {
    it('should initialize with valid config', () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
        },
        peers: ['peer-a', 'peer-b'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);

      expect(settlementMonitor).toBeDefined();
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          defaultThreshold: '1000',
          peerCount: 2,
          tokenCount: 1,
        }),
        'Settlement monitor initialized (event-driven)'
      );
    });

    it('should initialize all peer-token pairs to IDLE state', () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
        },
        peers: ['peer-a', 'peer-b'],
        tokenIds: ['M2M', 'USDC'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);

      expect(settlementMonitor.getSettlementState('peer-a', 'M2M')).toBe(SettlementState.IDLE);
      expect(settlementMonitor.getSettlementState('peer-a', 'USDC')).toBe(SettlementState.IDLE);
      expect(settlementMonitor.getSettlementState('peer-b', 'M2M')).toBe(SettlementState.IDLE);
      expect(settlementMonitor.getSettlementState('peer-b', 'USDC')).toBe(SettlementState.IDLE);
    });
  });

  describe('Threshold Detection (Event-Driven)', () => {
    it('should emit SETTLEMENT_REQUIRED when claim cumulative amount exceeds threshold', () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
        },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);
      settlementMonitor.start();

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // Emit claim with cumulative amount exceeding threshold
      const claimEvent: ClaimReceivedEvent = {
        peerId: 'peer-a',
        channelId: '0x' + 'a'.repeat(64),
        cumulativeAmount: 1500n,
      };
      mockClaimReceiver.emit('CLAIM_RECEIVED', claimEvent);

      expect(eventListener).toHaveBeenCalledTimes(1);
      expect(eventListener).toHaveBeenCalledWith(
        expect.objectContaining({
          peerId: 'peer-a',
          tokenId: 'M2M',
          currentBalance: 1500n,
          threshold: 1000n,
          exceedsBy: 500n,
          timestamp: expect.any(Date),
        })
      );

      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          peerId: 'peer-a',
          tokenId: 'M2M',
          cumulativeAmount: '1500',
          threshold: '1000',
          exceedsBy: '500',
        }),
        'Settlement threshold exceeded — triggering claimFromChannel()'
      );
    });

    it('should NOT emit event when cumulative amount is below threshold', () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
        },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);
      settlementMonitor.start();

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // Emit claim below threshold
      mockClaimReceiver.emit('CLAIM_RECEIVED', {
        peerId: 'peer-a',
        channelId: '0x' + 'a'.repeat(64),
        cumulativeAmount: 900n,
      });

      expect(eventListener).not.toHaveBeenCalled();
      expect(settlementMonitor.getSettlementState('peer-a', 'M2M')).toBe(SettlementState.IDLE);
    });

    it('should NOT emit event when cumulative amount equals threshold exactly', () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
        },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);
      settlementMonitor.start();

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      mockClaimReceiver.emit('CLAIM_RECEIVED', {
        peerId: 'peer-a',
        channelId: '0x' + 'a'.repeat(64),
        cumulativeAmount: 1000n,
      });

      expect(eventListener).not.toHaveBeenCalled();
    });

    it('should skip threshold check when no threshold configured', () => {
      const config: SettlementMonitorConfig = {
        thresholds: undefined, // No thresholds configured
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);
      settlementMonitor.start();

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // Emit claim with high amount
      mockClaimReceiver.emit('CLAIM_RECEIVED', {
        peerId: 'peer-a',
        channelId: '0x' + 'a'.repeat(64),
        cumulativeAmount: 10000n,
      });

      expect(eventListener).not.toHaveBeenCalled();
    });

    it('should handle claims from unknown peers gracefully', () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
        },
        peers: ['peer-a'], // Only peer-a configured
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);
      settlementMonitor.start();

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // Emit claim from unknown peer — should still trigger if threshold config applies
      mockClaimReceiver.emit('CLAIM_RECEIVED', {
        peerId: 'peer-unknown',
        channelId: '0x' + 'b'.repeat(64),
        cumulativeAmount: 1500n,
      });

      // Default threshold applies to unknown peers too
      expect(eventListener).toHaveBeenCalledTimes(1);
    });
  });

  describe('Duplicate Prevention', () => {
    it('should NOT emit duplicate triggers when multiple claims exceed threshold', () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
        },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);
      settlementMonitor.start();

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // First claim - should trigger
      mockClaimReceiver.emit('CLAIM_RECEIVED', {
        peerId: 'peer-a',
        channelId: '0x' + 'a'.repeat(64),
        cumulativeAmount: 1500n,
      });
      expect(eventListener).toHaveBeenCalledTimes(1);
      expect(settlementMonitor.getSettlementState('peer-a', 'M2M')).toBe(
        SettlementState.SETTLEMENT_PENDING
      );

      // Second claim - should NOT trigger (state is SETTLEMENT_PENDING)
      mockClaimReceiver.emit('CLAIM_RECEIVED', {
        peerId: 'peer-a',
        channelId: '0x' + 'a'.repeat(64),
        cumulativeAmount: 2000n,
      });
      expect(eventListener).toHaveBeenCalledTimes(1); // Still only 1 call
    });

    it('should trigger again after settlement completes and state resets to IDLE', () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
        },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);
      settlementMonitor.start();

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // First claim triggers settlement
      mockClaimReceiver.emit('CLAIM_RECEIVED', {
        peerId: 'peer-a',
        channelId: '0x' + 'a'.repeat(64),
        cumulativeAmount: 1500n,
      });
      expect(eventListener).toHaveBeenCalledTimes(1);

      // Complete settlement (resets to IDLE)
      settlementMonitor.markSettlementCompleted('peer-a', 'M2M');
      expect(settlementMonitor.getSettlementState('peer-a', 'M2M')).toBe(SettlementState.IDLE);

      // Next claim triggers again
      mockClaimReceiver.emit('CLAIM_RECEIVED', {
        peerId: 'peer-a',
        channelId: '0x' + 'a'.repeat(64),
        cumulativeAmount: 3000n,
      });
      expect(eventListener).toHaveBeenCalledTimes(2);
    });
  });

  describe('Configuration Hierarchy', () => {
    it('should use per-peer threshold override instead of default', () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
          perPeerThresholds: new Map([['peer-a', 2000n]]),
        },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);
      settlementMonitor.start();

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // Amount 1500 exceeds default (1000) but below peer-specific (2000)
      mockClaimReceiver.emit('CLAIM_RECEIVED', {
        peerId: 'peer-a',
        channelId: '0x' + 'a'.repeat(64),
        cumulativeAmount: 1500n,
      });

      // Should NOT trigger because peer-specific threshold (2000) not exceeded
      expect(eventListener).not.toHaveBeenCalled();
    });

    it('should use token-specific threshold instead of per-peer threshold', () => {
      const config: SettlementMonitorConfig = {
        thresholds: {
          defaultThreshold: 1000n,
          perPeerThresholds: new Map([['peer-a', 1000n]]),
          perTokenThresholds: new Map([['peer-a', new Map([['USDC', 500n]])]]),
        },
        peers: ['peer-a'],
        tokenIds: ['USDC'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);
      settlementMonitor.start();

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // Amount 600 exceeds token-specific threshold (500)
      mockClaimReceiver.emit('CLAIM_RECEIVED', {
        peerId: 'peer-a',
        channelId: '0x' + 'a'.repeat(64),
        cumulativeAmount: 600n,
      });

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
    it('should transition IDLE → SETTLEMENT_PENDING on threshold crossing', () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);
      settlementMonitor.start();

      expect(settlementMonitor.getSettlementState('peer-a', 'M2M')).toBe(SettlementState.IDLE);

      mockClaimReceiver.emit('CLAIM_RECEIVED', {
        peerId: 'peer-a',
        channelId: '0x' + 'a'.repeat(64),
        cumulativeAmount: 1500n,
      });

      expect(settlementMonitor.getSettlementState('peer-a', 'M2M')).toBe(
        SettlementState.SETTLEMENT_PENDING
      );
    });

    it('should transition SETTLEMENT_PENDING → SETTLEMENT_IN_PROGRESS via markSettlementInProgress', () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);

      settlementMonitor.markSettlementInProgress('peer-a', 'M2M');

      expect(settlementMonitor.getSettlementState('peer-a', 'M2M')).toBe(
        SettlementState.SETTLEMENT_IN_PROGRESS
      );

      expect(mockLogger.info).toHaveBeenCalledWith(
        { peerId: 'peer-a', tokenId: 'M2M' },
        'Settlement marked in progress'
      );
    });

    it('should transition SETTLEMENT_IN_PROGRESS → IDLE via markSettlementCompleted', () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);

      // Manually set to IN_PROGRESS
      settlementMonitor.markSettlementInProgress('peer-a', 'M2M');
      expect(settlementMonitor.getSettlementState('peer-a', 'M2M')).toBe(
        SettlementState.SETTLEMENT_IN_PROGRESS
      );

      // Mark completed
      settlementMonitor.markSettlementCompleted('peer-a', 'M2M');

      expect(settlementMonitor.getSettlementState('peer-a', 'M2M')).toBe(SettlementState.IDLE);

      expect(mockLogger.info).toHaveBeenCalledWith(
        { peerId: 'peer-a', tokenId: 'M2M' },
        'Settlement completed, state reset to IDLE'
      );
    });

    it('should return all settlement states via getAllSettlementStates', () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a', 'peer-b'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);

      const states = settlementMonitor.getAllSettlementStates();

      expect(states.size).toBe(2);
      expect(states.get('peer-a:M2M')).toBe(SettlementState.IDLE);
      expect(states.get('peer-b:M2M')).toBe(SettlementState.IDLE);

      // Verify it's a copy (mutations don't affect internal state)
      states.set('peer-a:M2M', SettlementState.SETTLEMENT_PENDING);
      expect(settlementMonitor.getSettlementState('peer-a', 'M2M')).toBe(SettlementState.IDLE);
    });
  });

  describe('Error Handling', () => {
    it('should handle errors in claim processing gracefully', () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);
      settlementMonitor.start();

      // Emit valid claim - should work
      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      mockClaimReceiver.emit('CLAIM_RECEIVED', {
        peerId: 'peer-a',
        channelId: '0x' + 'a'.repeat(64),
        cumulativeAmount: 1500n,
      });

      expect(eventListener).toHaveBeenCalledTimes(1);
    });
  });

  describe('Start and Stop', () => {
    it('should start and stop correctly', () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);

      settlementMonitor.start();
      expect(mockLogger.info).toHaveBeenCalledWith(
        'Settlement monitor started (subscribed to ClaimReceiver events)'
      );

      settlementMonitor.stop();
      expect(mockLogger.info).toHaveBeenCalledWith('Settlement monitor stopped');
    });

    it('should throw error if started when already running', () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);

      settlementMonitor.start();

      expect(() => settlementMonitor.start()).toThrow('Settlement monitor already running');
    });

    it('should not process events after stop', () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);
      settlementMonitor.start();

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // Stop monitor
      settlementMonitor.stop();

      // Emit claim after stop - should NOT trigger
      mockClaimReceiver.emit('CLAIM_RECEIVED', {
        peerId: 'peer-a',
        channelId: '0x' + 'a'.repeat(64),
        cumulativeAmount: 1500n,
      });

      expect(eventListener).not.toHaveBeenCalled();
    });

    it('should warn when started without ClaimReceiver', () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      // NOT calling setClaimReceiver

      settlementMonitor.start();

      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.stringContaining('Settlement monitor started without ClaimReceiver')
      );
    });

    it('should process events immediately on claim arrival (no polling delay)', () => {
      const config: SettlementMonitorConfig = {
        thresholds: { defaultThreshold: 1000n },
        peers: ['peer-a'],
        tokenIds: ['M2M'],
      };

      settlementMonitor = new SettlementMonitor(config, mockLogger);
      settlementMonitor.setClaimReceiver(mockClaimReceiver);
      settlementMonitor.start();

      const eventListener = jest.fn();
      settlementMonitor.on('SETTLEMENT_REQUIRED', eventListener);

      // Emit claim - should process synchronously (event-driven, no timer)
      mockClaimReceiver.emit('CLAIM_RECEIVED', {
        peerId: 'peer-a',
        channelId: '0x' + 'a'.repeat(64),
        cumulativeAmount: 1500n,
      });

      // Immediately available (no setTimeout/setInterval)
      expect(eventListener).toHaveBeenCalledTimes(1);
    });
  });
});
