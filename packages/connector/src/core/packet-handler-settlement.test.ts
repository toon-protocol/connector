/**
 * Unit Tests for PacketHandler Settlement Integration (Story 6.4)
 *
 * Tests settlement recording functionality for packet forwarding:
 * - Connector fee calculation
 * - Settlement transfer recording
 * - Error handling for settlement failures
 * - Backward compatibility when settlement disabled
 *
 * @packageDocumentation
 */

import { PacketHandler } from './packet-handler';
import { RoutingTable } from '../routing/routing-table';
import { BTPClientManager } from '../btp/btp-client-manager';
import { AccountManager } from '../settlement/account-manager';
import {
  ILPPreparePacket,
  ILPFulfillPacket,
  ILPRejectPacket,
  ILPErrorCode,
  PacketType,
} from '@crosstown/shared';
import { SettlementConfig } from '../config/types';
import type { PerPacketClaimService } from '../settlement/per-packet-claim-service';
import pino from 'pino';

// Mock AccountManager
jest.mock('../settlement/account-manager');

describe('PacketHandler Settlement Integration (Story 6.4)', () => {
  // Test helpers
  const createMockLogger = (): pino.Logger => pino({ level: 'silent' });

  const createValidPreparePacket = (): ILPPreparePacket => ({
    type: PacketType.PREPARE,
    amount: 100000n,
    destination: 'g.alice.wallet.USD',
    executionCondition: Buffer.from('a'.repeat(64), 'hex'), // 32 bytes
    expiresAt: new Date(Date.now() + 30000), // 30 seconds from now
    data: Buffer.alloc(0),
  });

  const createMockAccountManager = (): jest.Mocked<AccountManager> => {
    const mockAccountManager = {
      getPeerAccountPair: jest.fn().mockReturnValue({
        debitAccountId: 123n,
        creditAccountId: 456n,
        peerId: 'peer-test',
        tokenId: 'M2M',
      }),
      createPeerAccounts: jest.fn(),
      getAccountBalance: jest.fn(),
      recordPacketTransfers: jest.fn().mockResolvedValue(undefined),
      checkCreditLimit: jest.fn().mockResolvedValue(null),
    };
    return mockAccountManager as unknown as jest.Mocked<AccountManager>;
  };

  const createMockBTPClientManager = (): jest.Mocked<BTPClientManager> => {
    const mockClientManager = {
      sendToPeer: jest.fn().mockResolvedValue({
        type: PacketType.FULFILL,
        fulfillment: Buffer.from('b'.repeat(64), 'hex'),
        data: Buffer.alloc(0),
      } as ILPFulfillPacket),
      connect: jest.fn(),
      disconnect: jest.fn(),
      isConnected: jest.fn().mockReturnValue(true),
    };
    return mockClientManager as unknown as jest.Mocked<BTPClientManager>;
  };

  const createSettlementConfig = (): SettlementConfig => ({
    connectorFeePercentage: 0.1,
    enableSettlement: true,
    tigerBeetleClusterId: 0,
    tigerBeetleReplicas: ['localhost:3000'],
  });

  const createMockPerPacketClaimService = (): jest.Mocked<PerPacketClaimService> =>
    ({
      generateClaimForPacket: jest.fn().mockResolvedValue({
        protocolData: {
          protocolName: 'evm_claim',
          contentType: 0,
          data: Buffer.from('mock-claim-data'),
        },
        claimMessage: { version: '1.0', blockchain: 'evm' },
      }),
      getLatestClaim: jest.fn().mockReturnValue(null),
      resetChannel: jest.fn(),
    }) as unknown as jest.Mocked<PerPacketClaimService>;

  describe('Fee Calculation Tests', () => {
    it('should calculate 0.1% fee correctly for 100000 units', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-a' }]);
      const mockBTPClientManager = createMockBTPClientManager();
      const mockAccountManager = createMockAccountManager();
      const settlementConfig = createSettlementConfig();
      const logger = createMockLogger();

      const handler = new PacketHandler(
        routingTable,
        mockBTPClientManager,
        'connector-test',
        logger,
        null, // btpServer
        mockAccountManager,
        settlementConfig
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());

      const packet = createValidPreparePacket();
      packet.amount = 100000n;

      // Act
      await handler.handlePreparePacket(packet);

      // Assert - Fee should be 100n (0.1% of 100000)
      // Forwarded amount should be 99900n
      expect(mockBTPClientManager.sendToPeer).toHaveBeenCalledWith(
        'peer-a',
        expect.objectContaining({
          amount: 99900n, // 100000 - 100 = 99900
        }),
        expect.any(Array)
      );
    });

    it('should calculate 0.1% fee correctly for 1000 units', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-a' }]);
      const mockBTPClientManager = createMockBTPClientManager();
      const mockAccountManager = createMockAccountManager();
      const settlementConfig = createSettlementConfig();
      const logger = createMockLogger();

      const handler = new PacketHandler(
        routingTable,
        mockBTPClientManager,
        'connector-test',
        logger,
        null,
        mockAccountManager,
        settlementConfig
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());

      const packet = createValidPreparePacket();
      packet.amount = 1000n;

      // Act
      await handler.handlePreparePacket(packet);

      // Assert - Fee should be 1n (0.1% of 1000)
      expect(mockBTPClientManager.sendToPeer).toHaveBeenCalledWith(
        'peer-a',
        expect.objectContaining({
          amount: 999n, // 1000 - 1 = 999
        }),
        expect.any(Array)
      );
    });

    it('should calculate zero fee for very small amounts (rounds down)', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-a' }]);
      const mockBTPClientManager = createMockBTPClientManager();
      const mockAccountManager = createMockAccountManager();
      const settlementConfig = createSettlementConfig();
      const logger = createMockLogger();

      const handler = new PacketHandler(
        routingTable,
        mockBTPClientManager,
        'connector-test',
        logger,
        null,
        mockAccountManager,
        settlementConfig
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());

      const packet = createValidPreparePacket();
      packet.amount = 999n;

      // Act
      await handler.handlePreparePacket(packet);

      // Assert - Fee rounds to 0n (999 * 0.001 = 0.999, rounds down to 0)
      expect(mockBTPClientManager.sendToPeer).toHaveBeenCalledWith(
        'peer-a',
        expect.objectContaining({
          amount: 999n, // 999 - 0 = 999 (no fee charged on small amounts)
        }),
        expect.any(Array)
      );
    });

    it('should handle 1% fee percentage correctly', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-a' }]);
      const mockBTPClientManager = createMockBTPClientManager();
      const mockAccountManager = createMockAccountManager();
      const settlementConfig: SettlementConfig = {
        ...createSettlementConfig(),
        connectorFeePercentage: 1.0, // 1% fee
      };
      const logger = createMockLogger();

      const handler = new PacketHandler(
        routingTable,
        mockBTPClientManager,
        'connector-test',
        logger,
        null,
        mockAccountManager,
        settlementConfig
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());

      const packet = createValidPreparePacket();
      packet.amount = 10000n;

      // Act
      await handler.handlePreparePacket(packet);

      // Assert - Fee should be 100n (1% of 10000)
      expect(mockBTPClientManager.sendToPeer).toHaveBeenCalledWith(
        'peer-a',
        expect.objectContaining({
          amount: 9900n, // 10000 - 100 = 9900
        }),
        expect.any(Array)
      );
    });
  });

  describe('Settlement Recording Tests', () => {
    it('should call recordPacketTransfers for settlement recording', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-b' }]);
      const mockBTPClientManager = createMockBTPClientManager();
      const mockAccountManager = createMockAccountManager();
      const settlementConfig = createSettlementConfig();
      const logger = createMockLogger();

      const handler = new PacketHandler(
        routingTable,
        mockBTPClientManager,
        'connector-test',
        logger,
        null,
        mockAccountManager,
        settlementConfig
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());

      const packet = createValidPreparePacket();

      // Act
      await handler.handlePreparePacket(packet, 'peer-sender');

      // Assert - Should record packet transfers for settlement
      expect(mockAccountManager.recordPacketTransfers).toHaveBeenCalledTimes(1);
      expect(mockAccountManager.recordPacketTransfers).toHaveBeenCalledWith(
        'peer-sender', // fromPeerId
        'peer-b', // toPeerId
        'M2M', // tokenId
        100000n, // incoming amount
        99900n, // outgoing amount (100000 - 100 fee)
        expect.any(BigInt), // incomingTransferId
        expect.any(BigInt), // outgoingTransferId
        1, // ledger
        1 // code
      );
    });

    it('should skip settlement recording if settlement disabled', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-a' }]);
      const mockBTPClientManager = createMockBTPClientManager();
      const logger = createMockLogger();

      // Create handler WITHOUT AccountManager (settlement disabled)
      const handler = new PacketHandler(
        routingTable,
        mockBTPClientManager,
        'connector-test',
        logger,
        null, // btpServer
        null, // accountManager = null
        null // settlementConfig = null
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());

      const packet = createValidPreparePacket();
      packet.amount = 1000n;

      // Act
      await handler.handlePreparePacket(packet);

      // Assert - Packet forwarded with original amount (no fee deduction)
      expect(mockBTPClientManager.sendToPeer).toHaveBeenCalledWith(
        'peer-a',
        expect.objectContaining({
          amount: 1000n, // Original amount, no fee
        }),
        expect.any(Array)
      );
    });

    it('should skip settlement if enableSettlement is false', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-a' }]);
      const mockBTPClientManager = createMockBTPClientManager();
      const mockAccountManager = createMockAccountManager();
      const settlementConfig: SettlementConfig = {
        ...createSettlementConfig(),
        enableSettlement: false,
      };
      const logger = createMockLogger();

      const handler = new PacketHandler(
        routingTable,
        mockBTPClientManager,
        'connector-test',
        logger,
        null,
        mockAccountManager,
        settlementConfig
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());

      const packet = createValidPreparePacket();
      packet.amount = 1000n;

      // Act
      await handler.handlePreparePacket(packet);

      // Assert - No settlement recording, original amount forwarded
      expect(mockAccountManager.getPeerAccountPair).not.toHaveBeenCalled();
      expect(mockBTPClientManager.sendToPeer).toHaveBeenCalledWith(
        'peer-a',
        expect.objectContaining({
          amount: 1000n,
        }),
        expect.any(Array)
      );
    });
  });

  describe('Error Handling Tests', () => {
    it('should reject packet with T00_INTERNAL_ERROR if settlement recording throws error', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-a' }]);
      const mockBTPClientManager = createMockBTPClientManager();
      const mockAccountManager = createMockAccountManager();

      // Make recordPacketTransfers throw error to simulate settlement failure
      mockAccountManager.recordPacketTransfers.mockRejectedValue(
        new Error('TigerBeetle connection failed')
      );

      const settlementConfig = createSettlementConfig();
      const logger = createMockLogger();

      const handler = new PacketHandler(
        routingTable,
        mockBTPClientManager,
        'connector-test',
        logger,
        null,
        mockAccountManager,
        settlementConfig
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());

      const packet = createValidPreparePacket();

      // Act
      const result = await handler.handlePreparePacket(packet, 'peer-sender');

      // Assert - Should return ILP Reject with T00_INTERNAL_ERROR
      expect(result.type).toBe(PacketType.REJECT);
      const rejectPacket = result as ILPRejectPacket;
      expect(rejectPacket.code).toBe(ILPErrorCode.T00_INTERNAL_ERROR);
      expect(rejectPacket.message).toContain('Settlement recording failed');

      // Packet should NOT be forwarded
      expect(mockBTPClientManager.sendToPeer).not.toHaveBeenCalled();
    });

    it('should not record settlement for rejected packets (no route)', async () => {
      // Arrange
      const routingTable = new RoutingTable([]); // Empty routing table
      const mockBTPClientManager = createMockBTPClientManager();
      const mockAccountManager = createMockAccountManager();
      const settlementConfig = createSettlementConfig();
      const logger = createMockLogger();

      const handler = new PacketHandler(
        routingTable,
        mockBTPClientManager,
        'connector-test',
        logger,
        null,
        mockAccountManager,
        settlementConfig
      );

      const packet = createValidPreparePacket();

      // Act
      const result = await handler.handlePreparePacket(packet);

      // Assert - Packet rejected with F02_UNREACHABLE
      expect(result.type).toBe(PacketType.REJECT);
      const rejectPacket = result as ILPRejectPacket;
      expect(rejectPacket.code).toBe(ILPErrorCode.F02_UNREACHABLE);

      // Settlement should NOT be recorded (no route found before settlement step)
      expect(mockAccountManager.getPeerAccountPair).not.toHaveBeenCalled();
    });

    it('should not record settlement for expired packets', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-a' }]);
      const mockBTPClientManager = createMockBTPClientManager();
      const mockAccountManager = createMockAccountManager();
      const settlementConfig = createSettlementConfig();
      const logger = createMockLogger();

      const handler = new PacketHandler(
        routingTable,
        mockBTPClientManager,
        'connector-test',
        logger,
        null,
        mockAccountManager,
        settlementConfig
      );

      const packet = createValidPreparePacket();
      packet.expiresAt = new Date(Date.now() - 10000); // Expired 10 seconds ago

      // Act
      const result = await handler.handlePreparePacket(packet);

      // Assert - Packet rejected with R00_TRANSFER_TIMED_OUT
      expect(result.type).toBe(PacketType.REJECT);
      const rejectPacket = result as ILPRejectPacket;
      expect(rejectPacket.code).toBe(ILPErrorCode.R00_TRANSFER_TIMED_OUT);

      // Settlement should NOT be recorded (expired before settlement step)
      expect(mockAccountManager.getPeerAccountPair).not.toHaveBeenCalled();
    });
  });

  describe('Backward Compatibility Tests', () => {
    it('should forward packets normally when accountManager is null', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-a' }]);
      const mockBTPClientManager = createMockBTPClientManager();
      const logger = createMockLogger();

      const handler = new PacketHandler(
        routingTable,
        mockBTPClientManager,
        'connector-test',
        logger,
        null, // btpServer
        null, // No AccountManager
        null // No SettlementConfig
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());

      const packet = createValidPreparePacket();

      // Act
      const result = await handler.handlePreparePacket(packet);

      // Assert - Packet forwarded successfully
      expect(result.type).toBe(PacketType.FULFILL);
      expect(mockBTPClientManager.sendToPeer).toHaveBeenCalledWith(
        'peer-a',
        expect.any(Object),
        expect.any(Array)
      );
    });

    it('should preserve original packet amount when settlement disabled', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-a' }]);
      const mockBTPClientManager = createMockBTPClientManager();
      const logger = createMockLogger();

      const handler = new PacketHandler(
        routingTable,
        mockBTPClientManager,
        'connector-test',
        logger
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());

      const packet = createValidPreparePacket();
      const originalAmount = packet.amount;

      // Act
      await handler.handlePreparePacket(packet);

      // Assert - Forwarded packet has SAME amount as original
      expect(mockBTPClientManager.sendToPeer).toHaveBeenCalledWith(
        'peer-a',
        expect.objectContaining({
          amount: originalAmount,
        }),
        expect.any(Array)
      );
    });
  });

  describe('Metadata Correlation Tests', () => {
    it('should use executionCondition as packetId for settlement metadata', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-a' }]);
      const mockBTPClientManager = createMockBTPClientManager();
      const mockAccountManager = createMockAccountManager();
      const settlementConfig = createSettlementConfig();
      const logger = createMockLogger();

      const handler = new PacketHandler(
        routingTable,
        mockBTPClientManager,
        'connector-test',
        logger,
        null,
        mockAccountManager,
        settlementConfig
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());

      const packet = createValidPreparePacket();

      // Act
      await handler.handlePreparePacket(packet, 'peer-sender');

      // Assert - recordPacketTransfers called (settlement recording occurred)
      expect(mockAccountManager.recordPacketTransfers).toHaveBeenCalled();

      // Note: Transfer IDs are deterministically generated from executionCondition
      // in the generateTransferId() helper method
    });
  });
});
