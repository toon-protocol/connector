/**
 * Per-Packet Claim Service Unit Tests
 *
 * Tests claim generation, nonce tracking, cumulative amounts,
 * startup recovery, and graceful degradation.
 */

import { PerPacketClaimService } from './per-packet-claim-service';
import { BTP_CLAIM_PROTOCOL } from '../btp/btp-claim-types';
import type { PaymentChannelSDK } from './payment-channel-sdk';
import type { ChannelManager } from './channel-manager';
import type { Database } from 'better-sqlite3';
import type { Logger } from 'pino';

// Mock logger
const createMockLogger = (): Logger =>
  ({
    child: jest.fn().mockReturnThis(),
    info: jest.fn(),
    warn: jest.fn(),
    error: jest.fn(),
    debug: jest.fn(),
    trace: jest.fn(),
    fatal: jest.fn(),
  }) as unknown as Logger;

// Mock PaymentChannelSDK
const createMockSDK = (): jest.Mocked<
  Pick<
    PaymentChannelSDK,
    'signBalanceProof' | 'getChainId' | 'getTokenNetworkAddress' | 'getSignerAddress'
  >
> => ({
  signBalanceProof: jest.fn().mockResolvedValue('0xmocksignature'),
  getChainId: jest.fn().mockResolvedValue(31337),
  getTokenNetworkAddress: jest.fn().mockResolvedValue('0xTokenNetworkAddress1234567890abcdef'),
  getSignerAddress: jest.fn().mockResolvedValue('0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb1'),
});

// Mock ChannelManager
const createMockChannelManager = (
  channelMap?: Record<string, { channelId: string; tokenAddress: string }>
): jest.Mocked<Pick<ChannelManager, 'getChannelForPeer'>> => ({
  getChannelForPeer: jest.fn().mockImplementation((peerId: string, tokenId: string) => {
    const key = `${peerId}:${tokenId}`;
    const channel = channelMap?.[key];
    if (!channel) return null;
    return {
      channelId: channel.channelId,
      tokenAddress: channel.tokenAddress,
      peerId,
      tokenId,
      chain: 'evm:anvil:31337',
      createdAt: new Date(),
      lastActivityAt: new Date(),
      status: 'opened',
    };
  }),
});

// Mock SQLite Database
const createMockDb = (
  existingClaims?: Array<{ claim_data: string }>
): jest.Mocked<Pick<Database, 'prepare'>> => {
  const mockRun = jest.fn();
  const mockAll = jest.fn().mockReturnValue(existingClaims ?? []);
  const mockStatement = { run: mockRun, all: mockAll };
  return {
    prepare: jest.fn().mockReturnValue(mockStatement),
  } as unknown as jest.Mocked<Pick<Database, 'prepare'>>;
};

describe('PerPacketClaimService', () => {
  let service: PerPacketClaimService;
  let mockSDK: ReturnType<typeof createMockSDK>;
  let mockChannelManager: ReturnType<typeof createMockChannelManager>;
  let mockDb: ReturnType<typeof createMockDb>;
  let mockLogger: Logger;

  const TEST_CHANNEL_ID = '0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef';
  const TEST_TOKEN_ADDRESS = '0xabcdefabcdefabcdefabcdefabcdefabcdefabcd';
  const TEST_PEER_ID = 'connector-b';
  const TEST_NODE_ID = 'connector-a';

  beforeEach(() => {
    mockSDK = createMockSDK();
    mockChannelManager = createMockChannelManager({
      [`${TEST_PEER_ID}:M2M`]: {
        channelId: TEST_CHANNEL_ID,
        tokenAddress: TEST_TOKEN_ADDRESS,
      },
    });
    mockDb = createMockDb();
    mockLogger = createMockLogger();

    service = new PerPacketClaimService(
      mockSDK as unknown as PaymentChannelSDK,
      mockChannelManager as unknown as ChannelManager,
      mockDb as unknown as Database,
      mockLogger,
      TEST_NODE_ID
    );
  });

  describe('generateClaimForPacket', () => {
    it('should generate a valid claim for a packet', async () => {
      const result = await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 1000n);

      expect(result).not.toBeNull();
      expect(result!.protocolData.protocolName).toBe(BTP_CLAIM_PROTOCOL.NAME);
      expect(result!.protocolData.contentType).toBe(BTP_CLAIM_PROTOCOL.CONTENT_TYPE);

      const claim = result!.claimMessage;
      expect(claim.version).toBe('1.0');
      expect(claim.blockchain).toBe('evm');
      expect(claim.channelId).toBe(TEST_CHANNEL_ID);
      expect(claim.nonce).toBe(1);
      expect(claim.transferredAmount).toBe('1000');
      expect(claim.lockedAmount).toBe('0');
      expect(claim.signature).toBe('0xmocksignature');
      expect(claim.senderId).toBe(TEST_NODE_ID);
      expect(claim.chainId).toBe(31337);
      expect(claim.tokenAddress).toBe(TEST_TOKEN_ADDRESS);
    });

    it('should increment nonce for sequential packets', async () => {
      const result1 = await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 100n);
      const result2 = await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 200n);
      const result3 = await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 300n);

      expect(result1!.claimMessage.nonce).toBe(1);
      expect(result2!.claimMessage.nonce).toBe(2);
      expect(result3!.claimMessage.nonce).toBe(3);
    });

    it('should accumulate cumulative transferred amounts', async () => {
      const result1 = await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 100n);
      const result2 = await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 200n);
      const result3 = await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 300n);

      expect(result1!.claimMessage.transferredAmount).toBe('100');
      expect(result2!.claimMessage.transferredAmount).toBe('300'); // 100 + 200
      expect(result3!.claimMessage.transferredAmount).toBe('600'); // 100 + 200 + 300
    });

    it('should return null when no channel exists for peer', async () => {
      const result = await service.generateClaimForPacket('unknown-peer', 'M2M', 1000n);
      expect(result).toBeNull();
    });

    it('should cache channel context after first lookup', async () => {
      await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 100n);
      await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 200n);

      // ChannelManager should only be called once due to caching
      expect(mockChannelManager.getChannelForPeer).toHaveBeenCalledTimes(1);
    });

    it('should call signBalanceProof with correct parameters', async () => {
      await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 500n);

      expect(mockSDK.signBalanceProof).toHaveBeenCalledWith(
        TEST_CHANNEL_ID,
        1,
        500n,
        0n,
        '0x0000000000000000000000000000000000000000000000000000000000000000'
      );
    });

    it('should persist claim to database', async () => {
      await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 1000n);

      // The DB prepare should have been called for INSERT
      expect(mockDb.prepare).toHaveBeenCalled();
    });

    it('should serialize claim as JSON in protocolData', async () => {
      const result = await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 1000n);

      const parsed = JSON.parse(result!.protocolData.data.toString('utf8'));
      expect(parsed.channelId).toBe(TEST_CHANNEL_ID);
      expect(parsed.nonce).toBe(1);
      expect(parsed.transferredAmount).toBe('1000');
    });
  });

  describe('getLatestClaim', () => {
    it('should return null when no claims generated', () => {
      expect(service.getLatestClaim(TEST_CHANNEL_ID)).toBeNull();
    });

    it('should return latest claim after generation', async () => {
      await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 100n);
      await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 200n);

      const latest = service.getLatestClaim(TEST_CHANNEL_ID);
      expect(latest).not.toBeNull();
      expect(latest!.nonce).toBe(2);
      expect(latest!.transferredAmount).toBe('300');
    });
  });

  describe('resetChannel', () => {
    it('should clear all tracking state for a channel', async () => {
      await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 1000n);
      expect(service.getLatestClaim(TEST_CHANNEL_ID)).not.toBeNull();

      service.resetChannel(TEST_CHANNEL_ID);

      expect(service.getLatestClaim(TEST_CHANNEL_ID)).toBeNull();
    });

    it('should restart nonce and cumulative after reset', async () => {
      await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 100n);
      await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 200n);

      service.resetChannel(TEST_CHANNEL_ID);

      // Need to clear cache so context is re-fetched
      const result = await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 50n);
      expect(result!.claimMessage.nonce).toBe(1);
      expect(result!.claimMessage.transferredAmount).toBe('50');
    });
  });

  describe('startup recovery', () => {
    it('should recover nonce and cumulative from database', () => {
      const existingClaims = [
        {
          claim_data: JSON.stringify({
            channelId: TEST_CHANNEL_ID,
            nonce: 5,
            transferredAmount: '5000',
            blockchain: 'evm',
          }),
        },
      ];

      const recoveryDb = createMockDb(existingClaims);

      const recoveredService = new PerPacketClaimService(
        mockSDK as unknown as PaymentChannelSDK,
        mockChannelManager as unknown as ChannelManager,
        recoveryDb as unknown as Database,
        mockLogger,
        TEST_NODE_ID
      );

      // Latest claim should be restored
      const latest = recoveredService.getLatestClaim(TEST_CHANNEL_ID);
      expect(latest).not.toBeNull();
      expect(latest!.nonce).toBe(5);
    });

    it('should continue from recovered nonce', async () => {
      const existingClaims = [
        {
          claim_data: JSON.stringify({
            channelId: TEST_CHANNEL_ID,
            nonce: 10,
            transferredAmount: '10000',
            blockchain: 'evm',
          }),
        },
      ];

      const recoveryDb = createMockDb(existingClaims);

      const recoveredService = new PerPacketClaimService(
        mockSDK as unknown as PaymentChannelSDK,
        mockChannelManager as unknown as ChannelManager,
        recoveryDb as unknown as Database,
        mockLogger,
        TEST_NODE_ID
      );

      const result = await recoveredService.generateClaimForPacket(TEST_PEER_ID, 'M2M', 500n);
      expect(result!.claimMessage.nonce).toBe(11); // continues from 10
      expect(result!.claimMessage.transferredAmount).toBe('10500'); // 10000 + 500
    });

    it('should handle malformed DB data gracefully', () => {
      const existingClaims = [{ claim_data: 'not-valid-json' }];
      const recoveryDb = createMockDb(existingClaims);

      // Should not throw
      expect(
        () =>
          new PerPacketClaimService(
            mockSDK as unknown as PaymentChannelSDK,
            mockChannelManager as unknown as ChannelManager,
            recoveryDb as unknown as Database,
            mockLogger,
            TEST_NODE_ID
          )
      ).not.toThrow();
    });

    it('should handle DB query failure gracefully', () => {
      const failingDb = {
        prepare: jest.fn().mockReturnValue({
          all: jest.fn().mockImplementation(() => {
            throw new Error('DB read error');
          }),
          run: jest.fn(),
        }),
      } as unknown as Database;

      expect(
        () =>
          new PerPacketClaimService(
            mockSDK as unknown as PaymentChannelSDK,
            mockChannelManager as unknown as ChannelManager,
            failingDb,
            mockLogger,
            TEST_NODE_ID
          )
      ).not.toThrow();
    });
  });

  describe('error handling', () => {
    it('should return null when buildChannelContext fails', async () => {
      mockSDK.getChainId.mockRejectedValueOnce(new Error('RPC failure'));

      // Channel exists but context building fails
      const result = await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 1000n);
      expect(result).toBeNull();
    });

    it('should propagate signBalanceProof errors', async () => {
      // First, build context successfully
      await service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 100n);

      // Then fail on sign
      mockSDK.signBalanceProof.mockRejectedValueOnce(new Error('Signing failed'));

      await expect(service.generateClaimForPacket(TEST_PEER_ID, 'M2M', 200n)).rejects.toThrow(
        'Signing failed'
      );
    });
  });
});
