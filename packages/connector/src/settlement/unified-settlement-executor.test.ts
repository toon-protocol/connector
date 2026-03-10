/**
 * Unit Tests for UnifiedSettlementExecutor
 *
 * Tests EVM settlement routing logic for payment channels.
 * Verifies settlement method selection based on peer configuration and token type.
 *
 * Source: Epic 9 Story 9.5 - EVM Settlement Support
 * Extended: Epic 17 Story 17.4 - ClaimSender Integration for Off-Chain Claim Exchange
 *
 * @module settlement/unified-settlement-executor.test
 */

import { UnifiedSettlementExecutor, SettlementDisabledError } from './unified-settlement-executor';
import type { PaymentChannelSDK } from './payment-channel-sdk';
import type { SettlementMonitor } from './settlement-monitor';
import type { AccountManager } from './account-manager';
import type { Logger } from 'pino';
import type { UnifiedSettlementExecutorConfig, SettlementRequiredEvent } from './types';
import type { ClaimSender, ClaimSendResult } from './claim-sender';
import type { BTPClientManager } from '../btp/btp-client-manager';
import type { BTPClient } from '../btp/btp-client';

describe('UnifiedSettlementExecutor', () => {
  let executor: UnifiedSettlementExecutor;
  let mockEVMChannelSDK: jest.Mocked<PaymentChannelSDK>;
  let mockClaimSender: jest.Mocked<ClaimSender>;
  let mockBTPClientManager: jest.Mocked<BTPClientManager>;
  let mockBTPClient: jest.Mocked<BTPClient>;
  let mockSettlementMonitor: jest.Mocked<SettlementMonitor>;
  let mockAccountManager: jest.Mocked<AccountManager>;
  let mockLogger: jest.Mocked<Logger>;

  beforeEach(() => {
    // Create fresh mock instances (Anti-Pattern 3 solution)
    mockEVMChannelSDK = {
      openChannel: jest.fn().mockResolvedValue({ channelId: '0xabc123', txHash: '0xMockTxHash' }),
      signBalanceProof: jest.fn().mockResolvedValue('0xsignature'),
      getSignerAddress: jest.fn().mockResolvedValue('0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb'),
      getChainId: jest.fn().mockResolvedValue(8453), // Epic 31
      getTokenNetworkAddress: jest
        .fn()
        .mockResolvedValue('0x1234567890123456789012345678901234567890'), // Epic 31
      getChannelState: jest.fn(),
      closeChannel: jest.fn(),
      deposit: jest.fn(),
      getMyChannels: jest.fn(),
      settleChannel: jest.fn(),
      on: jest.fn(),
      off: jest.fn(),
      removeAllListeners: jest.fn(),
    } as unknown as jest.Mocked<PaymentChannelSDK>;

    mockSettlementMonitor = {
      on: jest.fn(),
      off: jest.fn(),
      emit: jest.fn(),
      listenerCount: jest.fn().mockReturnValue(0),
      removeAllListeners: jest.fn(),
    } as unknown as jest.Mocked<SettlementMonitor>;

    mockAccountManager = {
      recordSettlement: jest.fn().mockResolvedValue(undefined),
      getAccountBalance: jest.fn(),
      getPeerAccountPair: jest.fn(),
      recordPacketForward: jest.fn(),
      recordPacketReceive: jest.fn(),
    } as unknown as jest.Mocked<AccountManager>;

    mockLogger = {
      info: jest.fn(),
      error: jest.fn(),
      warn: jest.fn(),
      debug: jest.fn(),
      child: jest.fn().mockReturnThis(),
      fatal: jest.fn(),
      trace: jest.fn(),
      level: 'info',
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    // Mock BTPClient (Epic 17)
    mockBTPClient = {
      sendProtocolData: jest.fn().mockResolvedValue(undefined),
      isConnected: true,
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    // Mock BTPClientManager (Epic 17)
    mockBTPClientManager = {
      getClientForPeer: jest.fn().mockReturnValue(mockBTPClient),
      isConnected: jest.fn().mockReturnValue(true),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    // Mock ClaimSender (Epic 17)
    const successResult: ClaimSendResult = {
      success: true,
      messageId: 'evm-test-msg-456',
      timestamp: new Date().toISOString(),
    };

    mockClaimSender = {
      sendEVMClaim: jest.fn().mockResolvedValue(successResult),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    const config: UnifiedSettlementExecutorConfig = {
      peers: new Map([
        [
          'peer-alice',
          {
            peerId: 'peer-alice',
            address: 'g.alice',
            settlementPreference: 'evm',
            settlementTokens: ['USDC', 'DAI'],
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb',
          },
        ],
        [
          'peer-bob',
          {
            peerId: 'peer-bob',
            address: 'g.bob',
            settlementPreference: 'evm',
            settlementTokens: ['USDC'],
            evmAddress: '0x8ba1f109551bD432803012645Ac136ddd64DBA72',
          },
        ],
      ]),
      defaultPreference: 'evm',
      enabled: true,
    };

    executor = new UnifiedSettlementExecutor(
      config,
      mockEVMChannelSDK,
      mockClaimSender,
      mockBTPClientManager,
      mockSettlementMonitor,
      mockAccountManager,
      mockLogger
    );
  });

  afterEach(() => {
    // Ensure cleanup on test failure (Anti-Pattern 5 solution)
    executor.stop();
  });

  describe('Event Listener Cleanup', () => {
    it('should register listener on start', () => {
      executor.start();
      expect(mockSettlementMonitor.on).toHaveBeenCalledWith(
        'SETTLEMENT_REQUIRED',
        expect.any(Function)
      );
    });

    it('should unregister listener on stop', () => {
      executor.start();
      executor.stop();
      expect(mockSettlementMonitor.off).toHaveBeenCalledWith(
        'SETTLEMENT_REQUIRED',
        expect.any(Function)
      );
    });

    it('should log startup and shutdown messages', () => {
      executor.start();
      expect(mockLogger.info).toHaveBeenCalledWith('Starting UnifiedSettlementExecutor...');
      expect(mockLogger.info).toHaveBeenCalledWith('UnifiedSettlementExecutor started');

      executor.stop();
      expect(mockLogger.info).toHaveBeenCalledWith('Stopping UnifiedSettlementExecutor...');
      expect(mockLogger.info).toHaveBeenCalledWith('UnifiedSettlementExecutor stopped');
    });
  });

  describe('EVM Settlement Routing', () => {
    it('should route USDC settlement to EVM for peer with evm preference', async () => {
      executor.start();

      const event: SettlementRequiredEvent = {
        peerId: 'peer-alice',
        balance: '1000000000', // 1000 USDC
        tokenId: '0xUSDCAddress',
        timestamp: Date.now(),
      };

      // Manually invoke handler to simulate event emission
      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      await handler(event);

      expect(mockEVMChannelSDK.openChannel).toHaveBeenCalledWith(
        '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb',
        '0xUSDCAddress',
        86400,
        BigInt('1000000000')
      );
      expect(mockAccountManager.recordSettlement).toHaveBeenCalledWith(
        'peer-alice',
        '0xUSDCAddress',
        BigInt('1000000000')
      );
    });

    it('should route DAI settlement to EVM for peer-alice', async () => {
      executor.start();

      const event: SettlementRequiredEvent = {
        peerId: 'peer-alice',
        balance: '5000000000', // 5000 DAI
        tokenId: '0xDAIAddress',
        timestamp: Date.now(),
      };

      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      await handler(event);

      expect(mockEVMChannelSDK.openChannel).toHaveBeenCalled();
      expect(mockAccountManager.recordSettlement).toHaveBeenCalledWith(
        'peer-alice',
        '0xDAIAddress',
        BigInt('5000000000')
      );
    });

    it('should route USDC settlement to EVM for peer-bob', async () => {
      executor.start();

      const event: SettlementRequiredEvent = {
        peerId: 'peer-bob',
        balance: '1000000000',
        tokenId: '0xUSDCAddress',
        timestamp: Date.now(),
      };

      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      await handler(event);

      expect(mockEVMChannelSDK.openChannel).toHaveBeenCalled();
      expect(mockAccountManager.recordSettlement).toHaveBeenCalledWith(
        'peer-bob',
        '0xUSDCAddress',
        BigInt('1000000000')
      );
    });
  });

  describe('Error Handling', () => {
    it('should throw error for missing peer configuration', async () => {
      executor.start();

      const event: SettlementRequiredEvent = {
        peerId: 'unknown-peer',
        balance: '1000000000',
        tokenId: 'USDC',
        timestamp: Date.now(),
      };

      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];

      await expect(handler(event)).rejects.toThrow('Peer configuration not found');

      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({ peerId: 'unknown-peer' }),
        'Peer configuration not found'
      );
    });

    it('should throw error for missing evmAddress on EVM settlement', async () => {
      // Create config with peer missing evmAddress
      const configWithMissingAddress: UnifiedSettlementExecutorConfig = {
        peers: new Map([
          [
            'peer-incomplete',
            {
              peerId: 'peer-incomplete',
              address: 'g.incomplete',
              settlementPreference: 'evm',
              settlementTokens: ['USDC'],
              // evmAddress missing
            },
          ],
        ]),
        defaultPreference: 'evm',
        enabled: true,
      };

      const executorIncomplete = new UnifiedSettlementExecutor(
        configWithMissingAddress,
        mockEVMChannelSDK,
        mockClaimSender,
        mockBTPClientManager,
        mockSettlementMonitor,
        mockAccountManager,
        mockLogger
      );

      executorIncomplete.start();

      const event: SettlementRequiredEvent = {
        peerId: 'peer-incomplete',
        balance: '1000000000',
        tokenId: '0xUSDCAddress',
        timestamp: Date.now(),
      };

      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];

      await expect(handler(event)).rejects.toThrow('missing evmAddress');

      executorIncomplete.stop();
    });

    it('should throw error when settlement is disabled', async () => {
      const configDisabled: UnifiedSettlementExecutorConfig = {
        peers: new Map([
          [
            'peer-alice',
            {
              peerId: 'peer-alice',
              address: 'g.alice',
              settlementPreference: 'evm',
              settlementTokens: ['USDC'],
              evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb',
            },
          ],
        ]),
        defaultPreference: 'evm',
        enabled: false,
      };

      const executorDisabled = new UnifiedSettlementExecutor(
        configDisabled,
        mockEVMChannelSDK,
        mockClaimSender,
        mockBTPClientManager,
        mockSettlementMonitor,
        mockAccountManager,
        mockLogger
      );

      executorDisabled.start();

      const event: SettlementRequiredEvent = {
        peerId: 'peer-alice',
        balance: '1000000000',
        tokenId: '0xUSDCAddress',
        timestamp: Date.now(),
      };

      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];

      await expect(handler(event)).rejects.toThrow(SettlementDisabledError);

      executorDisabled.stop();
    });
  });

  describe('TigerBeetle Integration', () => {
    it('should update TigerBeetle accounts after successful EVM settlement', async () => {
      executor.start();

      const event: SettlementRequiredEvent = {
        peerId: 'peer-alice',
        balance: '1000000000',
        tokenId: '0xUSDCAddress',
        timestamp: Date.now(),
      };

      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      await handler(event);

      expect(mockAccountManager.recordSettlement).toHaveBeenCalledWith(
        'peer-alice',
        '0xUSDCAddress',
        BigInt('1000000000')
      );
    });

    it('should not update TigerBeetle accounts if settlement fails', async () => {
      // Mock EVM channel SDK to fail
      mockEVMChannelSDK.openChannel.mockRejectedValueOnce(new Error('Blockchain error'));

      executor.start();

      const event: SettlementRequiredEvent = {
        peerId: 'peer-alice',
        balance: '1000000000',
        tokenId: '0xUSDCAddress',
        timestamp: Date.now(),
      };

      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];

      await expect(handler(event)).rejects.toThrow('Blockchain error');

      // recordSettlement should NOT be called
      expect(mockAccountManager.recordSettlement).not.toHaveBeenCalled();
    });
  });

  describe('Logging', () => {
    it('should log settlement request details', async () => {
      executor.start();

      const event: SettlementRequiredEvent = {
        peerId: 'peer-alice',
        balance: '1000000000',
        tokenId: '0xUSDCAddress',
        timestamp: Date.now(),
      };

      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      await handler(event);

      expect(mockLogger.info).toHaveBeenCalledWith(
        { peerId: 'peer-alice', balance: '1000000000', tokenId: '0xUSDCAddress' },
        'Handling settlement request...'
      );
    });

    it('should log settlement completion', async () => {
      executor.start();

      const event: SettlementRequiredEvent = {
        peerId: 'peer-alice',
        balance: '1000000000',
        tokenId: '0xUSDCAddress',
        timestamp: Date.now(),
      };

      const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
      await handler(event);

      expect(mockLogger.info).toHaveBeenCalledWith(
        { peerId: 'peer-alice', balance: '1000000000', tokenId: '0xUSDCAddress' },
        'Settlement completed successfully'
      );
    });
  });

  describe('Epic 17: Claim Sender Integration', () => {
    describe('EVM Claim Sending', () => {
      it('should send EVM claim via ClaimSender when settling via EVM', async () => {
        executor.start();

        const event: SettlementRequiredEvent = {
          peerId: 'peer-alice',
          balance: '1000000000',
          tokenId: '0xUSDCAddress',
          timestamp: Date.now(),
        };

        const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];
        await handler(event);

        // Verify BTPClient retrieved
        expect(mockBTPClientManager.getClientForPeer).toHaveBeenCalledWith('peer-alice');
        expect(mockBTPClientManager.isConnected).toHaveBeenCalledWith('peer-alice');

        // Verify ClaimSender.sendEVMClaim called with correct parameters (Epic 31: includes self-describing fields)
        expect(mockClaimSender.sendEVMClaim).toHaveBeenCalledWith(
          'peer-alice',
          mockBTPClient,
          '0xabc123', // channelId from mockEVMChannelSDK
          1, // nonce
          '1000000000', // transferredAmount
          '0', // lockedAmount
          '0x0000000000000000000000000000000000000000000000000000000000000000', // locksRoot
          '0xsignature', // signature from mockEVMChannelSDK
          '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb', // signerAddress
          8453, // chainId (Epic 31)
          '0x1234567890123456789012345678901234567890', // tokenNetworkAddress (Epic 31)
          '0xUSDCAddress' // tokenAddress (Epic 31)
        );

        // Verify success logged
        expect(mockLogger.info).toHaveBeenCalledWith(
          expect.objectContaining({
            peerId: 'peer-alice',
            messageId: 'evm-test-msg-456',
          }),
          'EVM claim sent to peer successfully'
        );
      });

      it('should throw error when EVM claim send fails', async () => {
        executor.start();

        // Mock claim send failure
        mockClaimSender.sendEVMClaim.mockResolvedValue({
          success: false,
          messageId: 'evm-fail-456',
          timestamp: new Date().toISOString(),
          error: 'Timeout',
        });

        const event: SettlementRequiredEvent = {
          peerId: 'peer-alice',
          balance: '1000000000',
          tokenId: '0xUSDCAddress',
          timestamp: Date.now(),
        };

        const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];

        await expect(handler(event)).rejects.toThrow('Failed to send EVM claim to peer: Timeout');
      });

      it('should throw error when peer not connected for EVM settlement', async () => {
        executor.start();

        // Mock peer not connected
        mockBTPClientManager.getClientForPeer.mockReturnValue(undefined);

        const event: SettlementRequiredEvent = {
          peerId: 'peer-alice',
          balance: '1000000000',
          tokenId: '0xUSDCAddress',
          timestamp: Date.now(),
        };

        const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];

        await expect(handler(event)).rejects.toThrow('No BTP connection to peer peer-alice');
      });
    });

    describe('BTP Connection State Validation', () => {
      it('should throw error when BTP connection is not active', async () => {
        executor.start();

        // Mock connection inactive
        mockBTPClientManager.isConnected.mockReturnValue(false);

        const event: SettlementRequiredEvent = {
          peerId: 'peer-alice',
          balance: '1000000',
          tokenId: '0xUSDCAddress',
          timestamp: Date.now(),
        };

        const handler = (mockSettlementMonitor.on as jest.Mock).mock.calls[0][1];

        await expect(handler(event)).rejects.toThrow(
          'BTP connection to peer peer-alice is not active'
        );

        // Verify error logged
        expect(mockLogger.error).toHaveBeenCalledWith(
          expect.objectContaining({ peerId: 'peer-alice' }),
          'BTP connection to peer peer-alice is not active'
        );
      });
    });
  });
});
