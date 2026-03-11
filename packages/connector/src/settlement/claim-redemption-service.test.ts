/**
 * Unit tests for ClaimRedemptionService
 *
 * Tests automatic on-chain claim redemption functionality including:
 * - Service lifecycle (start/stop)
 * - Claim polling and processing
 * - Profitability checks
 * - EVM redemption success
 * - Retry logic with exponential backoff
 * - Gas estimation
 * - Database updates
 */

/* eslint-disable @typescript-eslint/no-explicit-any */

import { ClaimRedemptionService } from './claim-redemption-service';
import type { Database } from 'better-sqlite3';
import type { Logger } from 'pino';
import type { ethers } from 'ethers';
import type { PaymentChannelSDK } from './payment-channel-sdk';

/** Helper to create a DB row as returned by the SELECT query */
function makeDbRow(claim: Record<string, any>) {
  return {
    message_id: claim.messageId,
    peer_id: claim.senderId,
    blockchain: claim.blockchain,
    channel_id: claim.channelId,
    claim_data: JSON.stringify(claim),
  };
}

describe('ClaimRedemptionService', () => {
  let service: ClaimRedemptionService;
  let mockDb: jest.Mocked<Database>;
  let mockEVMChannelSDK: jest.Mocked<PaymentChannelSDK>;
  let mockEvmProvider: jest.Mocked<ethers.Provider>;
  let mockLogger: jest.Mocked<Logger>;

  beforeEach(() => {
    // Mock Database
    mockDb = {
      prepare: jest.fn(),
    } as unknown as jest.Mocked<Database>;

    // Mock PaymentChannelSDK
    mockEVMChannelSDK = {
      claimFromChannel: jest.fn().mockResolvedValue(undefined),
    } as unknown as jest.Mocked<PaymentChannelSDK>;

    // Mock ethers.Provider
    mockEvmProvider = {
      getFeeData: jest.fn().mockResolvedValue({
        gasPrice: 10n, // 10 wei – keeps gas cost (10 * 150000 = 1.5M) below claim amounts
      }),
    } as unknown as jest.Mocked<ethers.Provider>;

    // Mock Logger
    mockLogger = {
      info: jest.fn(),
      error: jest.fn(),
      warn: jest.fn(),
      debug: jest.fn(),
      child: jest.fn().mockReturnThis(),
    } as unknown as jest.Mocked<Logger>;

    // Create service instance
    service = new ClaimRedemptionService(
      mockDb,
      mockEVMChannelSDK,
      mockEvmProvider,
      {
        minProfitThreshold: 1000n,
        pollingInterval: 60000,
        maxConcurrentRedemptions: 5,
        evmTokenAddress: '0x1234567890abcdef1234567890abcdef12345678',
      },
      mockLogger
    );
  });

  afterEach(() => {
    jest.clearAllMocks();
    jest.useRealTimers();
    service.stop();
  });

  /** Helper to set up DB mocks for a claim row */
  function setupDbMocks(claimRows: any[]) {
    const mockSelectStmt = {
      all: jest.fn().mockReturnValue(claimRows),
    };

    const mockUpdateStmt = {
      run: jest.fn().mockReturnValue({ changes: 1 }),
    };

    mockDb.prepare.mockImplementation((sql: string) => {
      if (sql.includes('SELECT')) {
        return mockSelectStmt as any;
      }
      if (sql.includes('UPDATE')) {
        return mockUpdateStmt as any;
      }
      return { all: jest.fn().mockReturnValue([]) } as any;
    });

    return { mockSelectStmt, mockUpdateStmt };
  }

  describe('start()', () => {
    it('should start polling and set isRunning to true', () => {
      jest.useFakeTimers();

      service.start();

      expect(service.isRunning).toBe(true);
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          pollingInterval: 60000,
          minProfitThreshold: '1000',
        }),
        'Starting claim redemption service'
      );

      jest.useRealTimers();
    });

    it('should warn if already running', () => {
      jest.useFakeTimers();

      service.start();
      service.start(); // Second start

      expect(mockLogger.warn).toHaveBeenCalledWith('Claim redemption service already running');

      jest.useRealTimers();
    });

    it('should call processRedemptions immediately on start', async () => {
      // Mock empty database result
      const mockStmt = {
        all: jest.fn().mockReturnValue([]),
      };
      mockDb.prepare.mockReturnValue(mockStmt as any);

      service.start();

      // Wait for async processRedemptions to complete
      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(mockDb.prepare).toHaveBeenCalledWith(expect.stringContaining('SELECT message_id'));
      expect(mockStmt.all).toHaveBeenCalledWith(5);

      service.stop();
    });
  });

  describe('stop()', () => {
    it('should stop polling and set isRunning to false', () => {
      jest.useFakeTimers();

      service.start();
      expect(service.isRunning).toBe(true);

      service.stop();

      expect(service.isRunning).toBe(false);
      expect(mockLogger.info).toHaveBeenCalledWith('Claim redemption service stopped');

      jest.useRealTimers();
    });
  });

  describe('processRedemptions() - no claims', () => {
    it('should return early when no claims are found', async () => {
      const mockStmt = {
        all: jest.fn().mockReturnValue([]),
      };
      mockDb.prepare.mockReturnValue(mockStmt as any);

      service.start();

      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(mockStmt.all).toHaveBeenCalledWith(5);
      expect(mockEVMChannelSDK.claimFromChannel).not.toHaveBeenCalled();

      service.stop();
    });
  });

  describe('EVM claim redemption', () => {
    it('should successfully redeem EVM claim', async () => {
      const evmClaim = {
        blockchain: 'evm',
        messageId: 'msg_evm_123',
        senderId: 'peer-alice',
        channelId: '0xABC123',
        nonce: 1,
        transferredAmount: '5000000',
        lockedAmount: '0',
        locksRoot: '0x0000000000000000000000000000000000000000000000000000000000000000',
        signature: 'sig_evm',
        signerAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb',
      };

      const { mockUpdateStmt } = setupDbMocks([makeDbRow(evmClaim)]);
      mockEVMChannelSDK.claimFromChannel.mockResolvedValue(undefined);

      service.start();
      await new Promise((resolve) => setTimeout(resolve, 100));

      // claimFromChannel called with (channelId, tokenAddress, balanceProof, signature)
      expect(mockEVMChannelSDK.claimFromChannel).toHaveBeenCalledWith(
        '0xABC123',
        '0x1234567890abcdef1234567890abcdef12345678',
        expect.objectContaining({
          channelId: '0xABC123',
          nonce: 1,
          transferredAmount: 5000000n,
          lockedAmount: 0n,
        }),
        'sig_evm'
      );
      // DB update: stmt.run(Date.now(), txHash, messageId)
      expect(mockUpdateStmt.run).toHaveBeenCalledWith(
        expect.any(Number),
        'msg_evm_123',
        'msg_evm_123'
      );

      service.stop();
    });

    it('should retry EVM claim redemption on failure', async () => {
      const evmClaim = {
        blockchain: 'evm',
        messageId: 'msg_evm_retry',
        senderId: 'peer-bob',
        channelId: '0xDEF456',
        nonce: 2,
        transferredAmount: '3000000',
        lockedAmount: '0',
        locksRoot: '0x0000000000000000000000000000000000000000000000000000000000000000',
        signature: 'sig_evm_retry',
        signerAddress: '0x8ba1f109551bD432803012645Ac136ddd64DBA72',
      };

      const { mockUpdateStmt } = setupDbMocks([makeDbRow(evmClaim)]);

      // First attempt fails, second succeeds
      mockEVMChannelSDK.claimFromChannel
        .mockRejectedValueOnce(new Error('Network error'))
        .mockResolvedValueOnce(undefined);

      service.start();

      // Wait for retry (1s backoff + buffer)
      await new Promise((resolve) => setTimeout(resolve, 1500));

      expect(mockEVMChannelSDK.claimFromChannel).toHaveBeenCalledTimes(2);
      expect(mockUpdateStmt.run).toHaveBeenCalledWith(
        expect.any(Number),
        'msg_evm_retry',
        'msg_evm_retry'
      );

      service.stop();
    });

    it('should fail after 3 retry attempts', async () => {
      const evmClaim = {
        blockchain: 'evm',
        messageId: 'msg_evm_fail',
        senderId: 'peer-charlie',
        channelId: '0xGHI789',
        nonce: 3,
        transferredAmount: '5000000',
        lockedAmount: '0',
        locksRoot: '0x0000000000000000000000000000000000000000000000000000000000000000',
        signature: 'sig_evm_fail',
        signerAddress: '0x9cA1f109551bD432803012645Ac136ddd64DBA73',
      };

      setupDbMocks([makeDbRow(evmClaim)]);

      // All attempts fail
      mockEVMChannelSDK.claimFromChannel.mockRejectedValue(new Error('Persistent network error'));

      service.start();

      // Wait for all retries (1s + 2s + 4s backoff + buffer)
      await new Promise((resolve) => setTimeout(resolve, 8000));

      expect(mockEVMChannelSDK.claimFromChannel).toHaveBeenCalledTimes(3);

      service.stop();
    }, 10000);
  });

  describe('Profitability check', () => {
    it('should redeem profitable claims', async () => {
      const evmClaim = {
        blockchain: 'evm',
        messageId: 'msg_profitable',
        senderId: 'peer-alice',
        channelId: '0xPROFIT',
        nonce: 1,
        transferredAmount: '10000000', // High amount
        lockedAmount: '0',
        locksRoot: '0x0000000000000000000000000000000000000000000000000000000000000000',
        signature: 'sig_evm',
        signerAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb',
      };

      const { mockUpdateStmt } = setupDbMocks([makeDbRow(evmClaim)]);

      mockEvmProvider.getFeeData.mockResolvedValue({
        gasPrice: 1n, // 1 wei – gas cost = 150000 wei << 10M claim
      } as any);

      mockEVMChannelSDK.claimFromChannel.mockResolvedValue(undefined);

      service.start();
      await new Promise((resolve) => setTimeout(resolve, 100));

      expect(mockEVMChannelSDK.claimFromChannel).toHaveBeenCalled();
      expect(mockUpdateStmt.run).toHaveBeenCalledWith(
        expect.any(Number),
        'msg_profitable',
        'msg_profitable'
      );

      service.stop();
    });

    it('should skip unprofitable claims', async () => {
      const evmClaim = {
        blockchain: 'evm',
        messageId: 'msg_unprofitable',
        senderId: 'peer-bob',
        channelId: '0xUNPROFIT',
        nonce: 1,
        transferredAmount: '500', // Very small amount
        lockedAmount: '0',
        locksRoot: '0x0000000000000000000000000000000000000000000000000000000000000000',
        signature: 'sig_evm',
        signerAddress: '0x8ba1f109551bD432803012645Ac136ddd64DBA72',
      };

      setupDbMocks([makeDbRow(evmClaim)]);

      mockEvmProvider.getFeeData.mockResolvedValue({
        gasPrice: 1000000000n, // 1 gwei
      } as any);

      service.start();
      await new Promise((resolve) => setTimeout(resolve, 100));

      // Should NOT attempt redemption
      expect(mockEVMChannelSDK.claimFromChannel).not.toHaveBeenCalled();
      expect(mockLogger.debug).toHaveBeenCalledWith(
        expect.objectContaining({
          messageId: 'msg_unprofitable',
        }),
        'Skipping unprofitable claim'
      );

      service.stop();
    });

    it('should skip high-gas EVM claims', async () => {
      const evmClaim = {
        blockchain: 'evm',
        messageId: 'msg_high_gas',
        senderId: 'peer-charlie',
        channelId: '0xHIGHGAS',
        nonce: 1,
        transferredAmount: '2000000', // Moderate amount
        lockedAmount: '0',
        locksRoot: '0x0000000000000000000000000000000000000000000000000000000000000000',
        signature: 'sig_evm',
        signerAddress: '0x9cA1f109551bD432803012645Ac136ddd64DBA73',
      };

      setupDbMocks([makeDbRow(evmClaim)]);

      mockEvmProvider.getFeeData.mockResolvedValue({
        gasPrice: 100000000000n, // 100 gwei (very high gas)
      } as any);

      service.start();
      await new Promise((resolve) => setTimeout(resolve, 100));

      // Should NOT attempt redemption due to high gas
      expect(mockEVMChannelSDK.claimFromChannel).not.toHaveBeenCalled();

      service.stop();
    });
  });

  describe('Gas estimation', () => {
    it('should estimate EVM gas using provider.getFeeData()', async () => {
      const evmClaim = {
        blockchain: 'evm',
        messageId: 'msg_gas_test',
        senderId: 'peer-alice',
        channelId: '0xGASEST',
        nonce: 1,
        transferredAmount: '5000000',
        lockedAmount: '0',
        locksRoot: '0x0000000000000000000000000000000000000000000000000000000000000000',
        signature: 'sig_evm',
        signerAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb',
      };

      setupDbMocks([makeDbRow(evmClaim)]);

      mockEvmProvider.getFeeData.mockResolvedValue({
        gasPrice: 2000000000n, // 2 gwei
      } as any);

      mockEVMChannelSDK.claimFromChannel.mockResolvedValue(undefined);

      service.start();
      await new Promise((resolve) => setTimeout(resolve, 100));

      expect(mockEvmProvider.getFeeData).toHaveBeenCalled();

      service.stop();
    });

    it('should return 0 when EVM gas estimation fails', async () => {
      const evmClaim = {
        blockchain: 'evm',
        messageId: 'msg_gas_fail',
        senderId: 'peer-bob',
        channelId: '0xGASFAIL',
        nonce: 1,
        transferredAmount: '3000000',
        lockedAmount: '0',
        locksRoot: '0x0000000000000000000000000000000000000000000000000000000000000000',
        signature: 'sig_evm',
        signerAddress: '0x8ba1f109551bD432803012645Ac136ddd64DBA72',
      };

      setupDbMocks([makeDbRow(evmClaim)]);

      mockEvmProvider.getFeeData.mockRejectedValue(new Error('RPC error'));
      mockEVMChannelSDK.claimFromChannel.mockResolvedValue(undefined);

      service.start();
      await new Promise((resolve) => setTimeout(resolve, 100));

      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({
          blockchain: 'evm',
        }),
        'Error estimating redemption cost'
      );

      // Should still attempt redemption with gas cost = 0
      expect(mockEVMChannelSDK.claimFromChannel).toHaveBeenCalled();

      service.stop();
    });
  });

  describe('processRedemptions error handling', () => {
    it('should log error when database query fails', async () => {
      mockDb.prepare.mockImplementation(() => {
        throw new Error('Database connection lost');
      });

      service.start();

      await new Promise((resolve) => setTimeout(resolve, 50));

      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({
          error: expect.any(Error),
        }),
        'Error in processRedemptions'
      );

      service.stop();
    });

    it('should handle malformed claim data gracefully', async () => {
      // Return a row with invalid JSON in claim_data
      const malformedRow = {
        message_id: 'msg_malformed',
        peer_id: 'peer-alice',
        blockchain: 'evm',
        channel_id: '0xMALFORMED',
        claim_data: 'not-valid-json{{{',
      };

      const mockSelectStmt = {
        all: jest.fn().mockReturnValue([malformedRow]),
      };

      mockDb.prepare.mockImplementation((sql: string) => {
        if (sql.includes('SELECT')) {
          return mockSelectStmt as any;
        }
        return { run: jest.fn().mockReturnValue({ changes: 1 }) } as any;
      });

      service.start();
      await new Promise((resolve) => setTimeout(resolve, 100));

      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({
          messageId: 'msg_malformed',
        }),
        'Error processing claim redemption'
      );

      service.stop();
    });
  });
});
