/**
 * Settlement Executor Unit Tests
 *
 * Tests for automated on-chain settlement via payment channels.
 * Uses mocked dependencies (PaymentChannelSDK, AccountManager, SettlementMonitor).
 *
 * **Test Coverage:**
 * 1. Event listener registration and cleanup
 * 2. New channel opening and settlement
 * 3. Settlement via existing channel
 * 4. Channel deposit management
 * 5. Retry logic with exponential backoff
 * 6. Error handling
 * 7. Settlement monitor state transitions
 *
 * Source: Epic 8 Story 8.8 - Settlement Engine Integration Tests
 */

import { SettlementExecutor, SettlementExecutorConfig } from './settlement-executor';
import { AccountManager } from './account-manager';
import { PaymentChannelSDK } from './payment-channel-sdk';
import { SettlementMonitor } from './settlement-monitor';
import { SettlementTriggerEvent, SettlementState } from '../config/types';
import { ChannelState } from '../../../shared/src/types/payment-channel';
import pino from 'pino';

// Mock dependencies
jest.mock('./account-manager');
jest.mock('./payment-channel-sdk');
jest.mock('./settlement-monitor');

describe('SettlementExecutor', () => {
  let executor: SettlementExecutor;
  let mockAccountManager: jest.Mocked<AccountManager>;
  let mockPaymentChannelSDK: jest.Mocked<PaymentChannelSDK>;
  let mockSettlementMonitor: jest.Mocked<SettlementMonitor>;
  let logger: pino.Logger;
  let config: SettlementExecutorConfig;

  // Test data
  const testPeerId = 'connector-a';
  const testTokenId = 'M2M';
  const testTokenAddress = '0x1234567890123456789012345678901234567890';
  const testPeerAddress = '0xabcdefabcdefabcdefabcdefabcdefabcdefabcd';
  const testChannelId = '0xaaaa111122223333444455556666777788889999aaaabbbbccccddddeeeeffff';
  const testCurrentBalance = 1200n;
  const testThreshold = 1000n;

  beforeEach(() => {
    // Create fresh mock instances
    /* eslint-disable @typescript-eslint/no-explicit-any */
    mockAccountManager = new AccountManager(
      {} as any,
      {} as any,
      {} as any
    ) as jest.Mocked<AccountManager>;
    mockPaymentChannelSDK = new PaymentChannelSDK(
      {} as any,
      {} as any,
      '0x1234',
      {} as any,
      {} as any
    ) as jest.Mocked<PaymentChannelSDK>;
    mockSettlementMonitor = new SettlementMonitor(
      {} as any,
      {} as any
    ) as jest.Mocked<SettlementMonitor>;
    /* eslint-enable @typescript-eslint/no-explicit-any */

    // Setup mock implementations
    mockAccountManager.recordSettlement = jest.fn().mockResolvedValue(undefined);
    mockPaymentChannelSDK.getMyChannels = jest.fn().mockResolvedValue([]);
    mockPaymentChannelSDK.getChannelState = jest.fn().mockResolvedValue({
      channelId: testChannelId,
      participants: [testPeerAddress.toLowerCase(), '0x9876543210987654321098765432109876543210'],
      myDeposit: 10000n,
      theirDeposit: 10000n,
      myNonce: 1,
      theirNonce: 1,
      myTransferred: 0n,
      theirTransferred: 0n,
      status: 'opened' as const,
      settlementTimeout: 86400,
      openedAt: Math.floor(Date.now() / 1000),
    } as ChannelState);
    mockPaymentChannelSDK.openChannel = jest
      .fn()
      .mockResolvedValue({ channelId: testChannelId, txHash: '0xMockTxHash' });
    mockPaymentChannelSDK.deposit = jest.fn().mockResolvedValue(undefined);
    mockPaymentChannelSDK.signBalanceProof = jest.fn().mockResolvedValue('0xsignature');
    mockPaymentChannelSDK.claimFromChannel = jest.fn().mockResolvedValue(undefined);
    mockSettlementMonitor.markSettlementInProgress = jest.fn();
    mockSettlementMonitor.markSettlementCompleted = jest.fn();
    mockSettlementMonitor.getSettlementState = jest.fn().mockReturnValue(SettlementState.IDLE);
    mockSettlementMonitor.on = jest.fn();
    mockSettlementMonitor.off = jest.fn();

    // Create logger
    logger = pino({ level: 'silent' }); // Silent logger for tests

    // Create config
    config = {
      nodeId: 'connector-b',
      defaultSettlementTimeout: 86400,
      initialDepositMultiplier: 10,
      minDepositThreshold: 0.5,
      maxRetries: 3,
      retryDelayMs: 5000,
      tokenAddressMap: new Map([[testTokenId, testTokenAddress]]),
      peerIdToAddressMap: new Map([[testPeerId, testPeerAddress]]),
      registryAddress: '0xregistry1234567890123456789012345678901234',
      rpcUrl: 'http://localhost:8545',
      privateKey: '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80',
    };

    // Create executor instance
    executor = new SettlementExecutor(
      config,
      mockAccountManager,
      mockPaymentChannelSDK,
      mockSettlementMonitor,
      logger
    );
  });

  afterEach(async () => {
    // Cleanup: Stop executor to remove listeners and drain in-flight settlements
    await executor.stop();
  });

  describe('Constructor', () => {
    it('should initialize all properties correctly', () => {
      expect(executor).toBeInstanceOf(SettlementExecutor);
      expect(executor.getSettlementState).toBeDefined();
    });
  });

  describe('Event Listener Registration', () => {
    it('should register listener on start() and unregister on stop()', () => {
      // Start executor
      executor.start();

      // Verify listener registered
      expect(mockSettlementMonitor.on).toHaveBeenCalledWith(
        'SETTLEMENT_REQUIRED',
        expect.any(Function)
      );

      // Stop executor
      executor.stop();

      // Verify listener unregistered
      expect(mockSettlementMonitor.off).toHaveBeenCalledWith(
        'SETTLEMENT_REQUIRED',
        expect.any(Function)
      );

      // Verify same handler reference used for both on() and off()
      const onHandler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      const offHandler = (mockSettlementMonitor.off as jest.Mock).mock.calls[0][1];
      expect(onHandler).toBe(offHandler);
    });
  });

  describe('Settlement via New Channel', () => {
    it('should open new channel and settle when no existing channel', async () => {
      // Mock: No existing channel
      mockPaymentChannelSDK.getMyChannels.mockResolvedValue([]);

      // Create settlement event
      const event: SettlementTriggerEvent = {
        peerId: testPeerId,
        tokenId: testTokenId,
        currentBalance: testCurrentBalance,
        threshold: testThreshold,
        exceedsBy: testCurrentBalance - testThreshold,
        timestamp: new Date(),
      };

      // Start executor
      executor.start();

      // Simulate settlement event — handler enqueues onto settlement chain
      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      handler(event);

      // Drain the settlement chain by stopping the executor
      await executor.stop();

      // Verify: openChannel called with correct parameters
      expect(mockPaymentChannelSDK.openChannel).toHaveBeenCalledWith(
        testPeerAddress,
        testTokenAddress,
        config.defaultSettlementTimeout,
        testCurrentBalance * BigInt(config.initialDepositMultiplier)
      );

      // Verify: recordSettlement called after channel open
      expect(mockAccountManager.recordSettlement).toHaveBeenCalledWith(
        testPeerId,
        testTokenId,
        testCurrentBalance
      );

      // Verify: markSettlementInProgress called
      expect(mockSettlementMonitor.markSettlementInProgress).toHaveBeenCalledWith(
        testPeerId,
        testTokenId
      );

      // Verify: markSettlementCompleted called
      expect(mockSettlementMonitor.markSettlementCompleted).toHaveBeenCalledWith(
        testPeerId,
        testTokenId
      );
    });
  });

  describe('Settlement via Existing Channel', () => {
    it('should use claimFromChannel when channel exists (channel stays open)', async () => {
      // Mock: Existing channel found
      mockPaymentChannelSDK.getMyChannels.mockResolvedValue([testChannelId]);

      // Create settlement event
      const event: SettlementTriggerEvent = {
        peerId: testPeerId,
        tokenId: testTokenId,
        currentBalance: testCurrentBalance,
        threshold: testThreshold,
        exceedsBy: testCurrentBalance - testThreshold,
        timestamp: new Date(),
      };

      // Start executor
      executor.start();

      // Simulate settlement event — handler enqueues onto settlement chain
      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      handler(event);

      // Drain the settlement chain by stopping the executor
      await executor.stop();

      // Verify: claimFromChannel called
      expect(mockPaymentChannelSDK.claimFromChannel).toHaveBeenCalledWith(
        testChannelId,
        testTokenAddress,
        expect.objectContaining({
          channelId: testChannelId,
        }),
        expect.any(String)
      );

      // Verify: markSettlementCompleted called
      expect(mockSettlementMonitor.markSettlementCompleted).toHaveBeenCalledWith(
        testPeerId,
        testTokenId
      );
    });
  });

  describe('Per-Packet Claim Integration', () => {
    it('should use latest per-packet claim for claimFromChannel when available', async () => {
      // Mock: Existing channel found
      mockPaymentChannelSDK.getMyChannels.mockResolvedValue([testChannelId]);

      // Create executor with per-packet claim service
      const mockPerPacketClaimService = {
        getLatestClaim: jest.fn().mockReturnValue({
          channelId: testChannelId,
          nonce: 5,
          transferredAmount: '5000',
          lockedAmount: '0',
          locksRoot: '0x' + '0'.repeat(64),
          signature: '0xperpacketsignature',
        }),
        resetChannel: jest.fn(),
        start: jest.fn(),
        stop: jest.fn(),
      };

      // Create executor and set perPacketClaimService
      const executorWithClaims = new SettlementExecutor(
        config,
        mockAccountManager,
        mockPaymentChannelSDK,
        mockSettlementMonitor,
        logger
      );
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      executorWithClaims.setPerPacketClaimService(mockPerPacketClaimService as any);

      // Create settlement event
      const event: SettlementTriggerEvent = {
        peerId: testPeerId,
        tokenId: testTokenId,
        currentBalance: testCurrentBalance,
        threshold: testThreshold,
        exceedsBy: testCurrentBalance - testThreshold,
        timestamp: new Date(),
      };

      // Start executor
      executorWithClaims.start();

      // Simulate settlement event — handler enqueues onto settlement chain
      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      handler(event);

      // Drain the settlement chain by stopping the executor
      await executorWithClaims.stop();

      // Verify: claimFromChannel used per-packet claim data
      expect(mockPaymentChannelSDK.claimFromChannel).toHaveBeenCalledWith(
        testChannelId,
        testTokenAddress,
        expect.objectContaining({
          channelId: testChannelId,
          nonce: 5,
          transferredAmount: 5000n,
        }),
        '0xperpacketsignature'
      );

      // Verify: signBalanceProof NOT called (used existing claim)
      expect(mockPaymentChannelSDK.signBalanceProof).not.toHaveBeenCalled();

      // Verify: per-packet claim tracking reset after successful claim
      expect(mockPerPacketClaimService.resetChannel).toHaveBeenCalledWith(testChannelId);
    });
  });

  describe('Retry Logic', () => {
    it('should retry on transient failures with exponential backoff', async () => {
      // Create custom config with fast retry delays for testing
      const fastRetryConfig = {
        ...config,
        retryDelayMs: 10, // Fast retries for test: 10ms, 20ms, 40ms
      };

      // Create executor with fast retry config
      const fastRetryExecutor = new SettlementExecutor(
        fastRetryConfig,
        mockAccountManager,
        mockPaymentChannelSDK,
        mockSettlementMonitor,
        logger
      );

      // Mock: First 2 calls fail with retryable error, 3rd succeeds
      mockPaymentChannelSDK.openChannel
        .mockRejectedValueOnce(new Error('Network timeout'))
        .mockRejectedValueOnce(new Error('Network timeout'))
        .mockResolvedValueOnce({ channelId: testChannelId, txHash: '0xMockTxHash' });

      // Mock: No existing channel
      mockPaymentChannelSDK.getMyChannels.mockResolvedValue([]);

      // Create settlement event
      const event: SettlementTriggerEvent = {
        peerId: testPeerId,
        tokenId: testTokenId,
        currentBalance: testCurrentBalance,
        threshold: testThreshold,
        exceedsBy: testCurrentBalance - testThreshold,
        timestamp: new Date(),
      };

      // Start executor
      fastRetryExecutor.start();

      // Simulate settlement event — handler enqueues onto settlement chain
      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      handler(event);

      // Drain the settlement chain (retries complete within the chain)
      await fastRetryExecutor.stop();

      // Verify: openChannel called 3 times (2 failures + 1 success)
      expect(mockPaymentChannelSDK.openChannel).toHaveBeenCalledTimes(3);

      // Verify: Settlement eventually succeeds
      expect(mockSettlementMonitor.markSettlementCompleted).toHaveBeenCalledWith(
        testPeerId,
        testTokenId
      );
    });
  });

  describe('Error Handling', () => {
    it('should NOT mark completed on permanent failure', async () => {
      // Mock: Permanent failure (insufficient funds)
      mockPaymentChannelSDK.openChannel.mockRejectedValue(new Error('Insufficient funds'));

      // Mock: No existing channel
      mockPaymentChannelSDK.getMyChannels.mockResolvedValue([]);

      // Create settlement event
      const event: SettlementTriggerEvent = {
        peerId: testPeerId,
        tokenId: testTokenId,
        currentBalance: testCurrentBalance,
        threshold: testThreshold,
        exceedsBy: testCurrentBalance - testThreshold,
        timestamp: new Date(),
      };

      // Start executor
      executor.start();

      // Simulate settlement event — handler enqueues onto settlement chain
      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      handler(event);

      // Drain the settlement chain
      await executor.stop();

      // Verify: markSettlementCompleted NOT called
      expect(mockSettlementMonitor.markSettlementCompleted).not.toHaveBeenCalled();
    });
  });

  describe('Settlement Monitor State Transitions', () => {
    it('should call markSettlementInProgress immediately and markSettlementCompleted after success', async () => {
      // Mock: No existing channel
      mockPaymentChannelSDK.getMyChannels.mockResolvedValue([]);

      // Create settlement event
      const event: SettlementTriggerEvent = {
        peerId: testPeerId,
        tokenId: testTokenId,
        currentBalance: testCurrentBalance,
        threshold: testThreshold,
        exceedsBy: testCurrentBalance - testThreshold,
        timestamp: new Date(),
      };

      // Start executor
      executor.start();

      // Simulate settlement event — handler enqueues onto settlement chain
      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      handler(event);

      // Drain the settlement chain
      await executor.stop();

      // Verify: markSettlementInProgress called first
      expect(mockSettlementMonitor.markSettlementInProgress).toHaveBeenCalledWith(
        testPeerId,
        testTokenId
      );

      // Verify: markSettlementCompleted called after success
      expect(mockSettlementMonitor.markSettlementCompleted).toHaveBeenCalledWith(
        testPeerId,
        testTokenId
      );

      // Verify: markSettlementInProgress called before markSettlementCompleted
      const inProgressCall =
        (mockSettlementMonitor.markSettlementInProgress as jest.Mock).mock.invocationCallOrder[0] ||
        0;
      const completedCall =
        (mockSettlementMonitor.markSettlementCompleted as jest.Mock).mock.invocationCallOrder[0] ||
        0;
      expect(inProgressCall).toBeLessThan(completedCall);
    });

    it('should NOT call markSettlementCompleted when error occurs', async () => {
      // Mock: Permanent failure
      mockPaymentChannelSDK.openChannel.mockRejectedValue(new Error('Insufficient funds'));
      mockPaymentChannelSDK.getMyChannels.mockResolvedValue([]);

      // Create settlement event
      const event: SettlementTriggerEvent = {
        peerId: testPeerId,
        tokenId: testTokenId,
        currentBalance: testCurrentBalance,
        threshold: testThreshold,
        exceedsBy: testCurrentBalance - testThreshold,
        timestamp: new Date(),
      };

      // Start executor
      executor.start();

      // Simulate settlement event — handler enqueues onto settlement chain
      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      handler(event);

      // Drain the settlement chain
      await executor.stop();

      // Verify: markSettlementInProgress called
      expect(mockSettlementMonitor.markSettlementInProgress).toHaveBeenCalledWith(
        testPeerId,
        testTokenId
      );

      // Verify: markSettlementCompleted NOT called
      expect(mockSettlementMonitor.markSettlementCompleted).not.toHaveBeenCalled();
    });
  });

  describe('Settlement Serialization', () => {
    it('should serialize concurrent settlement events to prevent nonce collisions', async () => {
      // Track execution order
      const executionOrder: string[] = [];

      // Mock: No existing channels
      mockPaymentChannelSDK.getMyChannels.mockResolvedValue([]);

      // Mock: openChannel records execution order with a delay
      mockPaymentChannelSDK.openChannel.mockImplementation(async (participant2: string) => {
        const peerId = participant2 === testPeerAddress ? 'peer-a' : 'peer-b';
        executionOrder.push(`start-${peerId}`);
        await new Promise((resolve) => setTimeout(resolve, 20));
        executionOrder.push(`end-${peerId}`);
        return { channelId: testChannelId, txHash: '0xMockTxHash' };
      });

      // Setup second peer
      const secondPeerAddress = '0x1111111111111111111111111111111111111111';
      config.peerIdToAddressMap.set('connector-b', secondPeerAddress);

      // Recreate executor with updated config
      const serialExecutor = new SettlementExecutor(
        config,
        mockAccountManager,
        mockPaymentChannelSDK,
        mockSettlementMonitor,
        logger
      );

      serialExecutor.start();

      // Fire two settlement events concurrently
      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      handler({
        peerId: testPeerId,
        tokenId: testTokenId,
        currentBalance: testCurrentBalance,
        threshold: testThreshold,
        exceedsBy: testCurrentBalance - testThreshold,
        timestamp: new Date(),
      });
      handler({
        peerId: 'connector-b',
        tokenId: testTokenId,
        currentBalance: testCurrentBalance,
        threshold: testThreshold,
        exceedsBy: testCurrentBalance - testThreshold,
        timestamp: new Date(),
      });

      // Drain all settlements
      await serialExecutor.stop();

      // Verify: settlements executed sequentially (no interleaving)
      expect(executionOrder[0]).toBe('start-peer-a');
      expect(executionOrder[1]).toBe('end-peer-a');
      expect(executionOrder[2]).toBe('start-peer-b');
      expect(executionOrder[3]).toBe('end-peer-b');
    });
  });

  describe('Graceful Shutdown', () => {
    it('should ignore new settlement events after stop() is called', async () => {
      // Start executor
      executor.start();

      // Stop executor first
      await executor.stop();

      // Try to fire a settlement event after stop
      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      handler({
        peerId: testPeerId,
        tokenId: testTokenId,
        currentBalance: testCurrentBalance,
        threshold: testThreshold,
        exceedsBy: testCurrentBalance - testThreshold,
        timestamp: new Date(),
      });

      // Give time for any async operations
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Verify: No settlement operations were attempted
      expect(mockSettlementMonitor.markSettlementInProgress).not.toHaveBeenCalled();
      expect(mockPaymentChannelSDK.openChannel).not.toHaveBeenCalled();
    });

    it('should await in-flight settlement before stop() resolves', async () => {
      let settlementResolved = false;

      // Mock: openChannel with a delay to simulate in-flight settlement
      mockPaymentChannelSDK.getMyChannels.mockResolvedValue([]);
      mockPaymentChannelSDK.openChannel.mockImplementation(async () => {
        await new Promise((resolve) => setTimeout(resolve, 50));
        settlementResolved = true;
        return { channelId: testChannelId, txHash: '0xMockTxHash' };
      });

      // Start executor
      executor.start();

      // Fire settlement event
      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      handler({
        peerId: testPeerId,
        tokenId: testTokenId,
        currentBalance: testCurrentBalance,
        threshold: testThreshold,
        exceedsBy: testCurrentBalance - testThreshold,
        timestamp: new Date(),
      });

      // Stop executor — should await the in-flight settlement
      await executor.stop();

      // Verify: settlement completed before stop() resolved
      expect(settlementResolved).toBe(true);
      expect(mockSettlementMonitor.markSettlementCompleted).toHaveBeenCalled();
    });
  });

  describe('getSettlementState', () => {
    it('should delegate to settlementMonitor.getSettlementState', () => {
      mockSettlementMonitor.getSettlementState.mockReturnValue(SettlementState.SETTLEMENT_PENDING);

      const state = executor.getSettlementState(testPeerId, testTokenId);

      expect(state).toBe(SettlementState.SETTLEMENT_PENDING);
      expect(mockSettlementMonitor.getSettlementState).toHaveBeenCalledWith(
        testPeerId,
        testTokenId
      );
    });
  });
});
