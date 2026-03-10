import { ChannelManager, ChannelManagerConfig } from './channel-manager';
import { PaymentChannelSDK } from './payment-channel-sdk';
import { SettlementExecutor } from './settlement-executor';
import { TelemetryEmitter } from '../telemetry/telemetry-emitter';
import { EventEmitter } from 'events';
import pino from 'pino';
import { ChannelState } from '@crosstown/shared';

describe('ChannelManager', () => {
  let channelManager: ChannelManager;
  let mockPaymentChannelSDK: jest.Mocked<PaymentChannelSDK>;
  let mockSettlementExecutor: jest.Mocked<SettlementExecutor>;
  let mockLogger: pino.Logger;
  let mockTelemetryEmitter: jest.Mocked<Partial<TelemetryEmitter>>;
  let config: ChannelManagerConfig;

  beforeEach(() => {
    // Create mock instances
    mockPaymentChannelSDK = {
      openChannel: jest.fn(),
      getChannelState: jest.fn(),
      closeChannel: jest.fn(),
      signBalanceProof: jest.fn(),
      settleChannel: jest.fn(),
      getMyChannels: jest.fn(),
      deposit: jest.fn(),
    } as unknown as jest.Mocked<PaymentChannelSDK>;

    mockSettlementExecutor = new EventEmitter() as jest.Mocked<SettlementExecutor>;

    mockLogger = pino({ level: 'silent' });

    mockTelemetryEmitter = {
      emit: jest.fn(),
    } as jest.Mocked<Partial<TelemetryEmitter>>;

    // Default mock for getChannelState (can be overridden in individual tests)
    mockPaymentChannelSDK.getChannelState.mockResolvedValue({
      channelId: '0xChannelId123',
      participants: ['0xMyAddress', '0xPeerAddress'],
      myDeposit: BigInt(10000000000000000000),
      theirDeposit: BigInt(0),
      myNonce: 0,
      theirNonce: 0,
      myTransferred: BigInt(0),
      theirTransferred: BigInt(0),
      status: 'opened',
      settlementTimeout: 86400,
      openedAt: Date.now(),
    });

    // Create config
    config = {
      nodeId: 'test-node',
      defaultSettlementTimeout: 86400,
      initialDepositMultiplier: 10,
      idleChannelThreshold: 86400,
      minDepositThreshold: 0.5,
      idleCheckInterval: 3600,
      tokenAddressMap: new Map([['TEST_TOKEN', '0xTokenAddress']]),
      peerIdToAddressMap: new Map([['peer-a', '0xPeerAddress']]),
      registryAddress: '0xRegistryAddress',
      rpcUrl: 'http://localhost:8545',
      privateKey: '0xPrivateKey',
    };

    // Create ChannelManager instance
    channelManager = new ChannelManager(
      config,
      mockPaymentChannelSDK,
      mockSettlementExecutor,
      mockLogger,
      mockTelemetryEmitter as TelemetryEmitter
    );
  });

  afterEach(() => {
    // Stop channel manager to clear timers
    channelManager.stop();
  });

  describe('constructor', () => {
    it('should initialize all properties correctly', () => {
      expect(channelManager).toBeDefined();
      expect(channelManager.getAllChannels()).toEqual([]);
    });
  });

  describe('ensureChannelExists', () => {
    it('should create new channel when none exists', async () => {
      const mockChannelId = '0xChannelId123';
      mockPaymentChannelSDK.openChannel.mockResolvedValue({
        channelId: mockChannelId,
        txHash: '0xMockTxHash',
      });

      const channelId = await channelManager.ensureChannelExists('peer-a', 'TEST_TOKEN');

      expect(channelId).toBe(mockChannelId);
      expect(mockPaymentChannelSDK.openChannel).toHaveBeenCalledWith(
        '0xPeerAddress',
        '0xTokenAddress',
        86400,
        expect.any(BigInt)
      );

      const metadata = channelManager.getChannelById(mockChannelId);
      expect(metadata).toBeDefined();
      expect(metadata?.peerId).toBe('peer-a');
      expect(metadata?.tokenId).toBe('TEST_TOKEN');
      expect(metadata?.status).toBe('open');
    });

    it('should reuse existing channel', async () => {
      const mockChannelId = '0xChannelId123';
      mockPaymentChannelSDK.openChannel.mockResolvedValue({
        channelId: mockChannelId,
        txHash: '0xMockTxHash',
      });

      // First call creates channel
      await channelManager.ensureChannelExists('peer-a', 'TEST_TOKEN');

      // Second call reuses existing
      const channelId = await channelManager.ensureChannelExists('peer-a', 'TEST_TOKEN');

      expect(channelId).toBe(mockChannelId);
      expect(mockPaymentChannelSDK.openChannel).toHaveBeenCalledTimes(1);
    });
  });

  describe('getChannelById', () => {
    it('should return channel metadata when found', async () => {
      const mockChannelId = '0xChannelId123';
      mockPaymentChannelSDK.openChannel.mockResolvedValue({
        channelId: mockChannelId,
        txHash: '0xMockTxHash',
      });

      await channelManager.ensureChannelExists('peer-a', 'TEST_TOKEN');

      const metadata = channelManager.getChannelById(mockChannelId);
      expect(metadata).toBeDefined();
      expect(metadata?.channelId).toBe(mockChannelId);
    });

    it('should return null when channel not found', () => {
      const metadata = channelManager.getChannelById('0xNonExistent');
      expect(metadata).toBeNull();
    });
  });

  describe('getChannelForPeer', () => {
    it('should return channel metadata for peer and token', async () => {
      const mockChannelId = '0xChannelId123';
      mockPaymentChannelSDK.openChannel.mockResolvedValue({
        channelId: mockChannelId,
        txHash: '0xMockTxHash',
      });

      await channelManager.ensureChannelExists('peer-a', 'TEST_TOKEN');

      const metadata = channelManager.getChannelForPeer('peer-a', 'TEST_TOKEN');
      expect(metadata).toBeDefined();
      expect(metadata?.channelId).toBe(mockChannelId);
      expect(metadata?.peerId).toBe('peer-a');
      expect(metadata?.tokenId).toBe('TEST_TOKEN');
    });

    it('should return null when no channel exists for peer', () => {
      const metadata = channelManager.getChannelForPeer('peer-unknown', 'TEST_TOKEN');
      expect(metadata).toBeNull();
    });
  });

  describe('registerExternalChannel', () => {
    const externalChannelParams = {
      channelId: '0xExternalChannel123',
      peerId: 'peer-external',
      tokenAddress: '0xTokenAddress',
      tokenNetworkAddress: '0xTokenNetworkAddress',
      chainId: 31337,
      status: 'open' as const,
    };

    it('should register external channel in both channelMetadata and peerChannelIndex', () => {
      const metadata = channelManager.registerExternalChannel(externalChannelParams);

      expect(metadata.channelId).toBe(externalChannelParams.channelId);
      expect(metadata.peerId).toBe(externalChannelParams.peerId);
      expect(metadata.tokenAddress).toBe(externalChannelParams.tokenAddress);
      expect(metadata.chain).toBe('evm:31337');
      expect(metadata.status).toBe('open');
      expect(metadata.tokenId).toBe('TEST_TOKEN'); // reverse-lookup matched

      // Verify accessible via getChannelById
      const byId = channelManager.getChannelById(externalChannelParams.channelId);
      expect(byId).toBe(metadata);

      // Verify accessible via getChannelForPeer
      const byPeer = channelManager.getChannelForPeer('peer-external', 'TEST_TOKEN');
      expect(byPeer).toBe(metadata);
    });

    it('should be idempotent -- duplicate registration returns existing', () => {
      const first = channelManager.registerExternalChannel(externalChannelParams);
      const second = channelManager.registerExternalChannel(externalChannelParams);

      expect(second).toBe(first);
      expect(channelManager.getAllChannels()).toHaveLength(1);
    });

    it('should handle token address reverse-lookup fallback', () => {
      const unknownTokenParams = {
        ...externalChannelParams,
        tokenAddress: '0xUnknownToken',
      };

      const metadata = channelManager.registerExternalChannel(unknownTokenParams);

      // Falls back to raw token address as tokenId
      expect(metadata.tokenId).toBe('0xUnknownToken');
    });

    it('should emit EXTERNAL_CHANNEL_REGISTERED telemetry', () => {
      channelManager.registerExternalChannel(externalChannelParams);

      expect(mockTelemetryEmitter.emit).toHaveBeenCalledWith(
        expect.objectContaining({
          type: 'EXTERNAL_CHANNEL_REGISTERED',
          channelId: externalChannelParams.channelId,
          peerId: externalChannelParams.peerId,
          chainId: externalChannelParams.chainId,
          tokenNetworkAddress: externalChannelParams.tokenNetworkAddress,
          tokenAddress: externalChannelParams.tokenAddress,
        })
      );
    });
  });

  describe('markChannelActivity', () => {
    it('should update lastActivityAt timestamp', async () => {
      const mockChannelId = '0xChannelId123';
      mockPaymentChannelSDK.openChannel.mockResolvedValue({
        channelId: mockChannelId,
        txHash: '0xMockTxHash',
      });

      await channelManager.ensureChannelExists('peer-a', 'TEST_TOKEN');

      const metadata = channelManager.getChannelById(mockChannelId);
      const oldTimestamp = metadata?.lastActivityAt;

      // Wait 10ms
      await new Promise((resolve) => setTimeout(resolve, 10));

      channelManager.markChannelActivity(mockChannelId);

      const updatedMetadata = channelManager.getChannelById(mockChannelId);
      expect(updatedMetadata?.lastActivityAt.getTime()).toBeGreaterThan(
        oldTimestamp?.getTime() || 0
      );
    });
  });

  describe('isChannelIdle', () => {
    it('should detect idle channel', async () => {
      const mockChannelId = '0xChannelId123';
      mockPaymentChannelSDK.openChannel.mockResolvedValue({
        channelId: mockChannelId,
        txHash: '0xMockTxHash',
      });

      await channelManager.ensureChannelExists('peer-a', 'TEST_TOKEN');

      const metadata = channelManager.getChannelById(mockChannelId);
      if (!metadata) throw new Error('Metadata not found');

      // Set lastActivityAt to 25 hours ago
      const oldDate = new Date(Date.now() - 25 * 60 * 60 * 1000);
      metadata.lastActivityAt = oldDate;

      // Access private method via type assertion
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const isIdle = (channelManager as any).isChannelIdle(metadata);
      expect(isIdle).toBe(true);
    });

    it('should not detect active channel as idle', async () => {
      const mockChannelId = '0xChannelId123';
      mockPaymentChannelSDK.openChannel.mockResolvedValue({
        channelId: mockChannelId,
        txHash: '0xMockTxHash',
      });

      await channelManager.ensureChannelExists('peer-a', 'TEST_TOKEN');

      const metadata = channelManager.getChannelById(mockChannelId);
      if (!metadata) throw new Error('Metadata not found');

      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const isIdle = (channelManager as any).isChannelIdle(metadata);
      expect(isIdle).toBe(false);
    });
  });

  describe('start and stop', () => {
    it('should start and stop idle check timer', () => {
      channelManager.start();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      expect((channelManager as any).idleCheckTimer).toBeDefined();

      channelManager.stop();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      expect((channelManager as any).idleCheckTimer).toBeUndefined();
    });
  });

  describe('close idle channel', () => {
    it('should close idle channel and set status to closing', async () => {
      const mockChannelId = '0xChannelId123';
      mockPaymentChannelSDK.openChannel.mockResolvedValue({
        channelId: mockChannelId,
        txHash: '0xMockTxHash',
      });

      mockPaymentChannelSDK.closeChannel.mockResolvedValue();

      await channelManager.ensureChannelExists('peer-a', 'TEST_TOKEN');

      const metadata = channelManager.getChannelById(mockChannelId);
      if (!metadata) throw new Error('Metadata not found');

      // Set as idle
      metadata.lastActivityAt = new Date(Date.now() - 25 * 60 * 60 * 1000);

      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      await (channelManager as any).closeIdleChannel(mockChannelId);

      // Wait for async operations
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(mockPaymentChannelSDK.closeChannel).toHaveBeenCalledWith(
        mockChannelId,
        '0xTokenAddress'
      );
      expect(metadata.status).toBe('closing');
      expect(mockTelemetryEmitter.emit).toHaveBeenCalledWith(
        expect.objectContaining({
          type: 'CHANNEL_CLOSED',
          channelId: mockChannelId,
        })
      );
    });

    it('should revert status to open if closeChannel fails', async () => {
      const mockChannelId = '0xChannelId123';
      mockPaymentChannelSDK.openChannel.mockResolvedValue({
        channelId: mockChannelId,
        txHash: '0xMockTxHash',
      });

      mockPaymentChannelSDK.closeChannel.mockRejectedValue(new Error('Close channel failed'));

      await channelManager.ensureChannelExists('peer-a', 'TEST_TOKEN');

      const metadata = channelManager.getChannelById(mockChannelId);
      if (!metadata) throw new Error('Metadata not found');

      // Set as idle
      metadata.lastActivityAt = new Date(Date.now() - 25 * 60 * 60 * 1000);

      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      await expect((channelManager as any).closeIdleChannel(mockChannelId)).rejects.toThrow(
        'Close channel failed'
      );

      expect(metadata.status).toBe('open');
    });
  });

  describe('telemetry emission', () => {
    it('should emit legacy CHANNEL_OPENED telemetry', async () => {
      const mockChannelId = '0xChannelId123';
      mockPaymentChannelSDK.openChannel.mockResolvedValue({
        channelId: mockChannelId,
        txHash: '0xMockTxHash',
      });

      await channelManager.ensureChannelExists('peer-a', 'TEST_TOKEN');

      expect(mockTelemetryEmitter.emit).toHaveBeenCalledWith(
        expect.objectContaining({
          type: 'CHANNEL_OPENED',
          nodeId: 'test-node',
          channelId: mockChannelId,
          peerId: 'peer-a',
          tokenId: 'TEST_TOKEN',
        })
      );
    });

    it('should emit PAYMENT_CHANNEL_OPENED telemetry event (Story 8.10)', async () => {
      const mockChannelId = '0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef';
      const mockChannelState: ChannelState = {
        channelId: mockChannelId,
        participants: ['0xMyAddress', '0xPeerAddress'],
        myDeposit: BigInt(10000000000000000000),
        theirDeposit: BigInt(0),
        myNonce: 0,
        theirNonce: 0,
        myTransferred: BigInt(0),
        theirTransferred: BigInt(0),
        status: 'opened',
        settlementTimeout: 86400,
        openedAt: Date.now(),
      };

      mockPaymentChannelSDK.openChannel.mockResolvedValue({
        channelId: mockChannelId,
        txHash: '0xMockTxHash',
      });
      mockPaymentChannelSDK.getChannelState.mockResolvedValue(mockChannelState);

      await channelManager.ensureChannelExists('peer-a', 'TEST_TOKEN');

      // Verify PAYMENT_CHANNEL_OPENED event was emitted
      expect(mockTelemetryEmitter.emit).toHaveBeenCalledWith(
        expect.objectContaining({
          type: 'PAYMENT_CHANNEL_OPENED',
          nodeId: 'test-node',
          channelId: mockChannelId,
          participants: ['0xMyAddress', '0xPeerAddress'],
          peerId: 'peer-a',
          tokenAddress: '0xTokenAddress',
          tokenSymbol: 'TEST_TOKEN',
          settlementTimeout: 86400,
          initialDeposits: {
            '0xMyAddress': '10000000000000000000',
            '0xPeerAddress': '0',
          },
        })
      );
    });

    it('should emit PAYMENT_CHANNEL_BALANCE_UPDATE telemetry event when activity marked', async () => {
      const mockChannelId = '0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef';
      const mockChannelState: ChannelState = {
        channelId: mockChannelId,
        participants: ['0xMyAddress', '0xPeerAddress'],
        myDeposit: BigInt(10000000000000000000),
        theirDeposit: BigInt(0),
        myNonce: 5,
        theirNonce: 3,
        myTransferred: BigInt(5000000000000000000),
        theirTransferred: BigInt(2000000000000000000),
        status: 'opened',
        settlementTimeout: 86400,
        openedAt: Date.now(),
      };

      mockPaymentChannelSDK.openChannel.mockResolvedValue({
        channelId: mockChannelId,
        txHash: '0xMockTxHash',
      });
      mockPaymentChannelSDK.getChannelState.mockResolvedValue(mockChannelState);

      await channelManager.ensureChannelExists('peer-a', 'TEST_TOKEN');

      // Clear previous calls
      (mockTelemetryEmitter.emit as jest.Mock).mockClear();

      // Mark channel activity (this should trigger balance update telemetry)
      await channelManager.markChannelActivity(mockChannelId);

      // Verify PAYMENT_CHANNEL_BALANCE_UPDATE event was emitted
      expect(mockTelemetryEmitter.emit).toHaveBeenCalledWith(
        expect.objectContaining({
          type: 'PAYMENT_CHANNEL_BALANCE_UPDATE',
          nodeId: 'test-node',
          channelId: mockChannelId,
          myNonce: 5,
          theirNonce: 3,
          myTransferred: '5000000000000000000',
          theirTransferred: '2000000000000000000',
        })
      );
    });
  });

  describe('settlement activity tracking', () => {
    it('should update channel activity when settlement occurs', async () => {
      const mockChannelId = '0xChannelId123';
      mockPaymentChannelSDK.openChannel.mockResolvedValue({
        channelId: mockChannelId,
        txHash: '0xMockTxHash',
      });

      await channelManager.ensureChannelExists('peer-a', 'TEST_TOKEN');

      const metadata = channelManager.getChannelById(mockChannelId);
      const oldTimestamp = metadata?.lastActivityAt;

      // Wait 10ms
      await new Promise((resolve) => setTimeout(resolve, 10));

      // Simulate settlement activity event
      mockSettlementExecutor.emit('CHANNEL_ACTIVITY', { channelId: mockChannelId });

      const updatedMetadata = channelManager.getChannelById(mockChannelId);
      expect(updatedMetadata?.lastActivityAt.getTime()).toBeGreaterThan(
        oldTimestamp?.getTime() || 0
      );
    });
  });
});
