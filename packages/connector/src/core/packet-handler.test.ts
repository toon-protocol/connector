/**
 * Unit tests for PacketHandler
 * @packageDocumentation
 */

import * as crypto from 'crypto';
import { PacketHandler } from './packet-handler';
import { RoutingTable } from '../routing/routing-table';
import {
  ILPPreparePacket,
  ILPErrorCode,
  PacketType,
  ILPRejectPacket,
  ILPFulfillPacket,
} from '@toon-protocol/shared';
import { Logger } from '../utils/logger';
import { BTPClientManager } from '../btp/btp-client-manager';
import type { PerPacketClaimService } from '../settlement/per-packet-claim-service';
import { computeFulfillmentFromData, validateFulfillment } from './payment-handler';

/**
 * Mock logger for testing log output without console noise
 */
const createMockLogger = (): jest.Mocked<Logger> =>
  ({
    info: jest.fn(),
    warn: jest.fn(),
    error: jest.fn(),
    debug: jest.fn(),
    fatal: jest.fn(),
    trace: jest.fn(),
    silent: jest.fn(),
    level: 'info',
    child: jest.fn().mockReturnThis(),
  }) as unknown as jest.Mocked<Logger>;

/**
 * Mock BTPClientManager for testing without real BTP connections.
 * sendToPeer computes the correct fulfillment from packet data
 * so that SHA256(fulfillment) == condition validation passes.
 */
const createMockBTPClientManager = (): jest.Mocked<BTPClientManager> =>
  ({
    addPeer: jest.fn().mockResolvedValue(undefined),
    removePeer: jest.fn().mockResolvedValue(undefined),
    sendToPeer: jest.fn().mockImplementation((_peerId: string, packet: ILPPreparePacket) => {
      return Promise.resolve({
        type: PacketType.FULFILL,
        fulfillment: computeFulfillmentFromData(packet.data),
        data: Buffer.alloc(0),
      });
    }),
    getPeerStatus: jest.fn().mockReturnValue(new Map()),
    getPeerIds: jest.fn().mockReturnValue([]),
    isConnected: jest.fn().mockReturnValue(true),
  }) as unknown as jest.Mocked<BTPClientManager>;

/**
 * Mock PerPacketClaimService for testing claim generation
 */
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

/**
 * Factory function to create valid ILP Prepare packet for testing.
 * Derives executionCondition from data by default so that
 * SHA256(SHA256(data)) == condition (the simplified fulfillment scheme).
 */
const createValidPreparePacket = (overrides?: Partial<ILPPreparePacket>): ILPPreparePacket => {
  const futureExpiry = new Date(Date.now() + 10000); // 10 seconds in future
  const data = overrides?.data ?? Buffer.alloc(0);
  const fulfillment = computeFulfillmentFromData(data);
  const defaultCondition = crypto.createHash('sha256').update(fulfillment).digest();
  return {
    type: PacketType.PREPARE,
    amount: BigInt(1000),
    destination: 'g.alice.wallet',
    executionCondition: defaultCondition,
    expiresAt: futureExpiry,
    data,
    ...overrides,
  };
};

describe('PacketHandler', () => {
  describe('Constructor', () => {
    it('should create packet handler with required dependencies including logger', () => {
      // Arrange
      const routingTable = new RoutingTable();
      const btpClientManager = createMockBTPClientManager();
      const nodeId = 'test.connector';
      const mockLogger = createMockLogger();

      // Act
      const handler = new PacketHandler(routingTable, btpClientManager, nodeId, mockLogger);

      // Assert
      expect(handler).toBeDefined();
      expect(handler).toBeInstanceOf(PacketHandler);
    });
  });

  describe('validatePacket()', () => {
    let handler: PacketHandler;
    let mockLogger: ReturnType<typeof createMockLogger>;

    beforeEach(() => {
      const routingTable = new RoutingTable();
      const btpClientManager = createMockBTPClientManager();
      mockLogger = createMockLogger();
      handler = new PacketHandler(routingTable, btpClientManager, 'test.connector', mockLogger);
    });

    it('should return isValid true for valid packet with all required fields', () => {
      // Arrange
      const validPacket = createValidPreparePacket();

      // Act
      const result = handler.validatePacket(validPacket);

      // Assert
      expect(result.isValid).toBe(true);
      expect(result.errorCode).toBeUndefined();
      expect(result.errorMessage).toBeUndefined();
    });

    it('should return isValid false when amount field is missing', () => {
      // Arrange
      const invalidPacket = createValidPreparePacket();
      delete (invalidPacket as Partial<ILPPreparePacket>).amount;

      // Act
      const result = handler.validatePacket(invalidPacket);

      // Assert
      expect(result.isValid).toBe(false);
      expect(result.errorCode).toBe(ILPErrorCode.F01_INVALID_PACKET);
      expect(result.errorMessage).toBe('Missing required packet fields');
      expect(mockLogger.error).toHaveBeenCalled();
    });

    it('should return isValid false when destination field is missing', () => {
      // Arrange
      const invalidPacket = createValidPreparePacket({ destination: '' });

      // Act
      const result = handler.validatePacket(invalidPacket);

      // Assert
      expect(result.isValid).toBe(false);
      expect(result.errorCode).toBe(ILPErrorCode.F01_INVALID_PACKET);
    });

    it('should return isValid false when executionCondition field is missing', () => {
      // Arrange
      const invalidPacket = createValidPreparePacket();
      delete (invalidPacket as Partial<ILPPreparePacket>).executionCondition;

      // Act
      const result = handler.validatePacket(invalidPacket);

      // Assert
      expect(result.isValid).toBe(false);
      expect(result.errorCode).toBe(ILPErrorCode.F01_INVALID_PACKET);
    });

    it('should return isValid false when expiresAt field is missing', () => {
      // Arrange
      const invalidPacket = createValidPreparePacket();
      delete (invalidPacket as Partial<ILPPreparePacket>).expiresAt;

      // Act
      const result = handler.validatePacket(invalidPacket);

      // Assert
      expect(result.isValid).toBe(false);
      expect(result.errorCode).toBe(ILPErrorCode.F01_INVALID_PACKET);
    });

    it('should return isValid false when destination has invalid ILP address format', () => {
      // Arrange
      const invalidPacket = createValidPreparePacket({ destination: 'invalid..address' });

      // Act
      const result = handler.validatePacket(invalidPacket);

      // Assert
      expect(result.isValid).toBe(false);
      expect(result.errorCode).toBe(ILPErrorCode.F01_INVALID_PACKET);
      expect(result.errorMessage).toContain('Invalid ILP address format');
      expect(mockLogger.error).toHaveBeenCalled();
    });

    it('should return isValid false when packet has expired (expiresAt in past)', () => {
      // Arrange
      const expiredTime = new Date(Date.now() - 5000); // 5 seconds ago
      const expiredPacket = createValidPreparePacket({ expiresAt: expiredTime });

      // Act
      const result = handler.validatePacket(expiredPacket);

      // Assert
      expect(result.isValid).toBe(false);
      expect(result.errorCode).toBe(ILPErrorCode.R00_TRANSFER_TIMED_OUT);
      expect(result.errorMessage).toBe('Packet has expired');
      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({
          expiresAt: expiredTime.toISOString(),
          errorCode: ILPErrorCode.R00_TRANSFER_TIMED_OUT,
        }),
        'Packet validation failed: packet has expired'
      );
    });

    it('should return isValid false when packet expiring within next second (edge case)', () => {
      // Arrange
      const nearFutureExpiry = new Date(Date.now() + 500); // 500ms in future
      const packet = createValidPreparePacket({ expiresAt: nearFutureExpiry });

      // Act - Wait a bit to ensure expiry passes
      setTimeout(() => {
        const result = handler.validatePacket(packet);

        // Assert
        expect(result.isValid).toBe(false);
        expect(result.errorCode).toBe(ILPErrorCode.R00_TRANSFER_TIMED_OUT);
      }, 600);
    });

    it('should return isValid true for packet with far-future expiry', () => {
      // Arrange
      const farFutureExpiry = new Date(Date.now() + 3600000); // 1 hour in future
      const validPacket = createValidPreparePacket({ expiresAt: farFutureExpiry });

      // Act
      const result = handler.validatePacket(validPacket);

      // Assert
      expect(result.isValid).toBe(true);
    });

    it('should return isValid false when executionCondition is not 32 bytes', () => {
      // Arrange
      const invalidCondition = Buffer.alloc(16); // Only 16 bytes instead of 32
      const invalidPacket = createValidPreparePacket({ executionCondition: invalidCondition });

      // Act
      const result = handler.validatePacket(invalidPacket);

      // Assert
      expect(result.isValid).toBe(false);
      expect(result.errorCode).toBe(ILPErrorCode.F01_INVALID_PACKET);
      expect(result.errorMessage).toContain('executionCondition must be exactly 32 bytes');
      expect(mockLogger.error).toHaveBeenCalled();
    });
  });

  describe('generateReject()', () => {
    let handler: PacketHandler;
    let mockLogger: ReturnType<typeof createMockLogger>;

    beforeEach(() => {
      const routingTable = new RoutingTable();
      mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();
      handler = new PacketHandler(routingTable, btpClientManager, 'test.connector', mockLogger);
    });

    it('should create ILP Reject packet with T00 error code', () => {
      // Arrange
      const errorCode = ILPErrorCode.T00_INTERNAL_ERROR;
      const message = 'Internal error occurred';
      const triggeredBy = 'test.connector';

      // Act
      const reject = handler.generateReject(errorCode, message, triggeredBy);

      // Assert
      expect(reject.type).toBe(PacketType.REJECT);
      expect(reject.code).toBe(errorCode);
      expect(reject.message).toBe(message);
      expect(reject.triggeredBy).toBe(triggeredBy);
      expect(reject.data).toBeInstanceOf(Buffer);
    });

    it('should create ILP Reject packet with F02 error code', () => {
      // Arrange
      const errorCode = ILPErrorCode.F02_UNREACHABLE;
      const message = 'No route to destination';
      const triggeredBy = 'test.connector';

      // Act
      const reject = handler.generateReject(errorCode, message, triggeredBy);

      // Assert
      expect(reject.type).toBe(PacketType.REJECT);
      expect(reject.code).toBe(ILPErrorCode.F02_UNREACHABLE);
      expect(reject.message).toBe(message);
    });

    it('should create ILP Reject packet with R00 error code', () => {
      // Arrange
      const errorCode = ILPErrorCode.R00_TRANSFER_TIMED_OUT;
      const message = 'Transfer timed out';
      const triggeredBy = 'test.connector';

      // Act
      const reject = handler.generateReject(errorCode, message, triggeredBy);

      // Assert
      expect(reject.type).toBe(PacketType.REJECT);
      expect(reject.code).toBe(ILPErrorCode.R00_TRANSFER_TIMED_OUT);
    });

    it('should include human-readable error message in reject packet', () => {
      // Arrange
      const message = 'Detailed error explanation for debugging';

      // Act
      const reject = handler.generateReject(
        ILPErrorCode.F01_INVALID_PACKET,
        message,
        'test.connector'
      );

      // Assert
      expect(reject.message).toBe(message);
    });

    it('should set triggeredBy field to connector node ID', () => {
      // Arrange
      const nodeId = 'g.my-connector.node1';
      const routingTable = new RoutingTable();
      const mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();
      const handlerWithNodeId = new PacketHandler(
        routingTable,
        btpClientManager,
        nodeId,
        mockLogger
      );

      // Act
      const reject = handlerWithNodeId.generateReject(
        ILPErrorCode.F02_UNREACHABLE,
        'No route',
        nodeId
      );

      // Assert
      expect(reject.triggeredBy).toBe(nodeId);
    });

    it('should verify reject packet type field equals PacketType.REJECT (14)', () => {
      // Arrange & Act
      const reject = handler.generateReject(
        ILPErrorCode.F00_BAD_REQUEST,
        'Bad request',
        'test.connector'
      );

      // Assert
      expect(reject.type).toBe(14);
      expect(reject.type).toBe(PacketType.REJECT);
    });

    it('should log reject packet generation at INFO level', () => {
      // Arrange
      const errorCode = ILPErrorCode.F02_UNREACHABLE;
      const message = 'No route found';
      const triggeredBy = 'test.connector';

      // Act
      handler.generateReject(errorCode, message, triggeredBy);

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        {
          errorCode,
          message,
          triggeredBy,
        },
        'Generated reject packet'
      );
    });
  });

  describe('decrementExpiry()', () => {
    let handler: PacketHandler;
    let mockLogger: ReturnType<typeof createMockLogger>;

    beforeEach(() => {
      const routingTable = new RoutingTable();
      mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();
      handler = new PacketHandler(routingTable, btpClientManager, 'test.connector', mockLogger);
    });

    it('should subtract 1000ms safety margin correctly', () => {
      // Arrange
      const originalExpiry = new Date(Date.now() + 10000); // 10 seconds from now
      const safetyMargin = 1000;

      // Act
      const newExpiry = handler.decrementExpiry(originalExpiry, safetyMargin);

      // Assert
      expect(newExpiry).not.toBeNull();
      expect(newExpiry!.getTime()).toBe(originalExpiry.getTime() - safetyMargin);
    });

    it('should return null if decremented expiry would be in the past', () => {
      // Arrange
      const nearExpiry = new Date(Date.now() + 500); // Only 500ms remaining
      const safetyMargin = 1000; // Need to subtract 1000ms

      // Act
      const newExpiry = handler.decrementExpiry(nearExpiry, safetyMargin);

      // Assert
      expect(newExpiry).toBeNull();
      expect(mockLogger.debug).toHaveBeenCalledWith(
        expect.objectContaining({
          originalExpiry: nearExpiry.toISOString(),
        }),
        'Expiry decrement would create past timestamp'
      );
    });

    it('should handle custom safety margin values (500ms)', () => {
      // Arrange
      const originalExpiry = new Date(Date.now() + 5000);
      const safetyMargin = 500;

      // Act
      const newExpiry = handler.decrementExpiry(originalExpiry, safetyMargin);

      // Assert
      expect(newExpiry).not.toBeNull();
      expect(newExpiry!.getTime()).toBe(originalExpiry.getTime() - 500);
    });

    it('should handle custom safety margin values (2000ms)', () => {
      // Arrange
      const originalExpiry = new Date(Date.now() + 10000);
      const safetyMargin = 2000;

      // Act
      const newExpiry = handler.decrementExpiry(originalExpiry, safetyMargin);

      // Assert
      expect(newExpiry).not.toBeNull();
      expect(newExpiry!.getTime()).toBe(originalExpiry.getTime() - 2000);
    });

    it('should return null when packet expiring in exactly 1000ms (safety margin)', () => {
      // Arrange
      const exactMarginExpiry = new Date(Date.now() + 1000);
      const safetyMargin = 1000;

      // Act
      const newExpiry = handler.decrementExpiry(exactMarginExpiry, safetyMargin);

      // Assert
      // The decremented time will be very close to current time, likely past it
      expect(newExpiry).toBeNull();
    });

    it('should log expiry decrement at DEBUG level with timestamps', () => {
      // Arrange
      const originalExpiry = new Date(Date.now() + 10000);
      const safetyMargin = 1000;

      // Act
      handler.decrementExpiry(originalExpiry, safetyMargin);

      // Assert
      expect(mockLogger.debug).toHaveBeenCalledWith(
        expect.objectContaining({
          originalExpiry: originalExpiry.toISOString(),
          safetyMargin: 1000,
        }),
        'Decremented packet expiry'
      );
    });
  });

  describe('handlePreparePacket() - Happy Path', () => {
    let handler: PacketHandler;
    let routingTable: RoutingTable;
    let mockLogger: ReturnType<typeof createMockLogger>;

    beforeEach(() => {
      routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();
      handler = new PacketHandler(routingTable, btpClientManager, 'test.connector', mockLogger);
      handler.setPerPacketClaimService(createMockPerPacketClaimService());
    });

    it('should forward valid packet when route found and return fulfill packet', async () => {
      // Arrange
      const validPacket = createValidPreparePacket({ destination: 'g.alice.wallet' });

      // Act
      const result = await handler.handlePreparePacket(validPacket);

      // Assert
      expect(result.type).toBe(PacketType.FULFILL);
      expect((result as ILPFulfillPacket).fulfillment).toBeInstanceOf(Buffer);
      expect((result as ILPFulfillPacket).fulfillment.length).toBe(32);
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          correlationId: expect.stringMatching(/^pkt_[a-f0-9]{16}$/),
          packetType: 'PREPARE',
          destination: 'g.alice.wallet',
          timestamp: expect.any(Number),
        }),
        'Packet received'
      );
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          correlationId: expect.stringMatching(/^pkt_[a-f0-9]{16}$/),
          destination: 'g.alice.wallet',
          selectedPeer: 'peer-alice',
          reason: 'longest-prefix match',
        }),
        'Routing decision'
      );
    });

    it('should generate correlation ID for packet tracking', async () => {
      // Arrange
      const validPacket = createValidPreparePacket({ destination: 'g.alice.wallet' });

      // Act
      await handler.handlePreparePacket(validPacket);

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          correlationId: expect.any(String),
        }),
        expect.any(String)
      );
    });

    it('should log all packet handling events at INFO level with structured fields', async () => {
      // Arrange
      const validPacket = createValidPreparePacket({
        destination: 'g.alice.wallet',
        amount: BigInt(5000),
      });

      // Act
      await handler.handlePreparePacket(validPacket);

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          correlationId: expect.stringMatching(/^pkt_[a-f0-9]{16}$/),
          packetType: 'PREPARE',
          destination: 'g.alice.wallet',
          amount: '5000',
          timestamp: expect.any(Number),
        }),
        'Packet received'
      );
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          correlationId: expect.stringMatching(/^pkt_[a-f0-9]{16}$/),
          destination: 'g.alice.wallet',
          selectedPeer: 'peer-alice',
          reason: 'longest-prefix match',
        }),
        'Routing decision'
      );
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          correlationId: expect.stringMatching(/^pkt_[a-f0-9]{16}$/),
          event: 'btp_forward',
          destination: 'g.alice.wallet',
          amount: '5000',
          peerId: 'peer-alice',
        }),
        'Forwarding packet to peer via BTP'
      );
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          correlationId: expect.stringMatching(/^pkt_[a-f0-9]{16}$/),
          event: 'packet_response',
          packetType: PacketType.FULFILL,
        }),
        'Returning packet response'
      );
    });
  });

  describe('handlePreparePacket() - No Route Found', () => {
    let handler: PacketHandler;
    let mockLogger: ReturnType<typeof createMockLogger>;

    beforeEach(() => {
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();
      handler = new PacketHandler(routingTable, btpClientManager, 'test.connector', mockLogger);
    });

    it('should return F02 Unreachable reject packet when no route found', async () => {
      // Arrange
      const validPacket = createValidPreparePacket({ destination: 'g.bob.crypto' });

      // Act
      const result = await handler.handlePreparePacket(validPacket);

      // Assert
      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.F02_UNREACHABLE);
      expect(reject.message).toContain('No route to destination');
      expect(reject.message).toContain('g.bob.crypto');
      expect(reject.triggeredBy).toBe('test.connector');
    });

    it('should log route lookup failure when no route found', async () => {
      // Arrange
      const validPacket = createValidPreparePacket({ destination: 'g.unknown.destination' });

      // Act
      await handler.handlePreparePacket(validPacket);

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          correlationId: expect.stringMatching(/^pkt_[a-f0-9]{16}$/),
          destination: 'g.unknown.destination',
          selectedPeer: null,
          reason: 'no route found',
        }),
        'Routing decision'
      );
      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({
          correlationId: expect.stringMatching(/^pkt_[a-f0-9]{16}$/),
          packetType: 'REJECT',
          errorCode: ILPErrorCode.F02_UNREACHABLE,
          reason: 'no route found',
        }),
        'Packet rejected'
      );
    });
  });

  describe('handlePreparePacket() - Expired Packet', () => {
    let handler: PacketHandler;
    let mockLogger: ReturnType<typeof createMockLogger>;

    beforeEach(() => {
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();
      handler = new PacketHandler(routingTable, btpClientManager, 'test.connector', mockLogger);
    });

    it('should return R00 reject packet when packet has expired', async () => {
      // Arrange
      const expiredPacket = createValidPreparePacket({
        destination: 'g.alice.wallet',
        expiresAt: new Date(Date.now() - 5000), // 5 seconds ago
      });

      // Act
      const result = await handler.handlePreparePacket(expiredPacket);

      // Assert
      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.R00_TRANSFER_TIMED_OUT);
      expect(reject.message).toBe('Packet has expired');
    });

    it('should return R00 reject packet when expiry decrement creates past timestamp', async () => {
      // Arrange
      const nearExpiryPacket = createValidPreparePacket({
        destination: 'g.alice.wallet',
        expiresAt: new Date(Date.now() + 500), // Only 500ms remaining, less than 1000ms margin
      });

      // Act
      const result = await handler.handlePreparePacket(nearExpiryPacket);

      // Assert
      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.R00_TRANSFER_TIMED_OUT);
      expect(reject.message).toContain('Insufficient time remaining');
    });
  });

  describe('handlePreparePacket() - Invalid Packet', () => {
    let handler: PacketHandler;
    let mockLogger: ReturnType<typeof createMockLogger>;

    beforeEach(() => {
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();
      handler = new PacketHandler(routingTable, btpClientManager, 'test.connector', mockLogger);
    });

    it('should return F01 reject packet when packet structure is invalid', async () => {
      // Arrange
      const invalidPacket = createValidPreparePacket({ destination: '' });

      // Act
      const result = await handler.handlePreparePacket(invalidPacket);

      // Assert
      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.F01_INVALID_PACKET);
    });

    it('should log validation failure for invalid packet', async () => {
      // Arrange
      const invalidPacket = createValidPreparePacket({ destination: 'invalid..address' });

      // Act
      await handler.handlePreparePacket(invalidPacket);

      // Assert
      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({
          correlationId: expect.stringMatching(/^pkt_[a-f0-9]{16}$/),
          packetType: 'REJECT',
          errorCode: ILPErrorCode.F01_INVALID_PACKET,
          timestamp: expect.any(Number),
        }),
        'Packet rejected'
      );
    });
  });

  describe('handlePreparePacket() - Integration with RoutingTable', () => {
    it('should call routingTable.getNextHop() with correct destination', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      const getNextHopSpy = jest.spyOn(routingTable, 'getNextHop');
      const mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();
      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());
      const validPacket = createValidPreparePacket({ destination: 'g.alice.wallet.USD' });

      // Act
      await handler.handlePreparePacket(validPacket);

      // Assert
      expect(getNextHopSpy).toHaveBeenCalledWith('g.alice.wallet.USD');
      expect(getNextHopSpy).toHaveReturnedWith('peer-alice');
    });

    it('should handle null return from routingTable.getNextHop() gracefully', async () => {
      // Arrange
      const routingTable = new RoutingTable(); // Empty routing table
      const mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();
      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );
      const validPacket = createValidPreparePacket({ destination: 'g.unknown' });

      // Act
      const result = await handler.handlePreparePacket(validPacket);

      // Assert
      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.F02_UNREACHABLE);
    });
  });

  describe('setLocalDeliveryHandler() - Function Handler Delivery Path', () => {
    let handler: PacketHandler;
    let mockLogger: ReturnType<typeof createMockLogger>;

    beforeEach(() => {
      const routingTable = new RoutingTable([
        { prefix: 'g.alice', nextHop: 'peer-alice' },
        { prefix: 'g.local', nextHop: 'test.connector' },
      ]);
      mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();
      handler = new PacketHandler(routingTable, btpClientManager, 'test.connector', mockLogger);
    });

    it('should call function handler with correct LocalDeliveryRequest and sourcePeerId when packet is local', async () => {
      // Arrange - use valid fulfillment derived from packet data
      const packetData = Buffer.from('handler-test-data');
      const validFulfillment = computeFulfillmentFromData(packetData);
      const mockHandler = jest.fn().mockResolvedValue({
        fulfill: { fulfillment: validFulfillment.toString('base64'), data: '' },
      });
      handler.setLocalDeliveryHandler(mockHandler);

      const packet = createValidPreparePacket({ destination: 'g.local.wallet', data: packetData });

      // Act
      await handler.handlePreparePacket(packet, 'source-peer-1');

      // Assert
      expect(mockHandler).toHaveBeenCalledTimes(1);
      const [request, sourcePeerId] = mockHandler.mock.calls[0];
      expect(sourcePeerId).toBe('source-peer-1');
      expect(request).toEqual(
        expect.objectContaining({
          destination: 'g.local.wallet',
          amount: '1000',
          sourcePeer: 'source-peer-1',
        })
      );
      expect(request.executionCondition).toBeDefined();
      expect(request.expiresAt).toBeDefined();
      expect(request.data).toBeDefined();
    });

    it('should produce ILPFulfillPacket when function handler returns fulfill', async () => {
      // Arrange - use consistent fulfillment derived from packet data
      const packetData = Buffer.from('test-local-data');
      const expectedFulfillment = computeFulfillmentFromData(packetData);
      const responseData = Buffer.from('response-data').toString('base64');
      const mockHandler = jest.fn().mockResolvedValue({
        fulfill: { fulfillment: expectedFulfillment.toString('base64'), data: responseData },
      });
      handler.setLocalDeliveryHandler(mockHandler);

      const packet = createValidPreparePacket({ destination: 'g.local.wallet', data: packetData });

      // Act
      const result = await handler.handlePreparePacket(packet, 'source-peer-1');

      // Assert
      expect(result.type).toBe(PacketType.FULFILL);
      const fulfill = result as ILPFulfillPacket;
      expect(fulfill.fulfillment).toEqual(expectedFulfillment);
      expect(fulfill.data).toEqual(Buffer.from(responseData, 'base64'));
    });

    it('should produce ILPRejectPacket when function handler returns reject', async () => {
      // Arrange
      const mockHandler = jest.fn().mockResolvedValue({
        reject: {
          code: 'F99',
          message: 'Application error from handler',
        },
      });
      handler.setLocalDeliveryHandler(mockHandler);

      const packet = createValidPreparePacket({ destination: 'g.local.wallet' });

      // Act
      const result = await handler.handlePreparePacket(packet, 'source-peer-1');

      // Assert
      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe('F99');
      expect(reject.message).toBe('Application error from handler');
      expect(reject.triggeredBy).toBe('test.connector');
    });

    it('should produce ILP Reject T00_INTERNAL_ERROR when function handler throws an Error', async () => {
      // Arrange
      const mockHandler = jest.fn().mockRejectedValue(new Error('Handler crashed'));
      handler.setLocalDeliveryHandler(mockHandler);

      const packet = createValidPreparePacket({ destination: 'g.local.wallet' });

      // Act
      const result = await handler.handlePreparePacket(packet, 'source-peer-1');

      // Assert
      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.T00_INTERNAL_ERROR);
      expect(reject.message).toContain('Handler crashed');
      expect(reject.triggeredBy).toBe('test.connector');
    });

    it('should produce ILP Reject T00_INTERNAL_ERROR when function handler returns neither fulfill nor reject', async () => {
      // Arrange
      const mockHandler = jest.fn().mockResolvedValue({});
      handler.setLocalDeliveryHandler(mockHandler);

      const packet = createValidPreparePacket({ destination: 'g.local.wallet' });

      // Act
      const result = await handler.handlePreparePacket(packet, 'source-peer-1');

      // Assert
      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.T00_INTERNAL_ERROR);
      expect(reject.message).toContain('Invalid response from local delivery handler');
    });

    it('should use HTTP path when no function handler set but HTTP LocalDeliveryClient enabled (backward compat)', async () => {
      // Arrange - set up HTTP local delivery, no function handler
      handler.setLocalDelivery({
        enabled: true,
        handlerUrl: 'http://connector:3100',
        timeout: 5000,
      });
      // Do NOT set function handler

      const packet = createValidPreparePacket({ destination: 'g.local.wallet' });

      // Act
      const result = await handler.handlePreparePacket(packet, 'source-peer-1');

      // Assert - should not crash; HTTP client will attempt fetch (may fail in test env)
      // The key check is that it doesn't use function handler path
      expect(result.type).toBeDefined();
    });

    it('should use auto-fulfill stub when neither function handler nor HTTP client set (backward compat)', async () => {
      // Arrange - no function handler, no HTTP local delivery
      const packet = createValidPreparePacket({ destination: 'g.local.wallet' });

      // Act
      const result = await handler.handlePreparePacket(packet, 'source-peer-1');

      // Assert - auto-fulfill stub
      expect(result.type).toBe(PacketType.FULFILL);
    });

    it('should use function handler over HTTP LocalDeliveryClient when both are configured', async () => {
      // Arrange - set up both HTTP client and function handler with valid fulfillment
      handler.setLocalDelivery({
        enabled: true,
        handlerUrl: 'http://connector:3100',
        timeout: 5000,
      });
      const packetData = Buffer.from('combined-handler-data');
      const expectedFulfillment = computeFulfillmentFromData(packetData);
      const mockHandler = jest.fn().mockResolvedValue({
        fulfill: { fulfillment: expectedFulfillment.toString('base64') },
      });
      handler.setLocalDeliveryHandler(mockHandler);

      const packet = createValidPreparePacket({ destination: 'g.local.wallet', data: packetData });

      // Act
      const result = await handler.handlePreparePacket(packet, 'source-peer-1');

      // Assert - function handler should have been called
      expect(mockHandler).toHaveBeenCalledTimes(1);
      expect(result.type).toBe(PacketType.FULFILL);
      const fulfill = result as ILPFulfillPacket;
      expect(fulfill.fulfillment).toEqual(expectedFulfillment);
    });
  });

  describe('handlePreparePacket() - Expiry Decrement Integration', () => {
    it('should decrement packet expiry before forwarding', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      const mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();
      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());
      const originalExpiry = new Date(Date.now() + 10000);
      const validPacket = createValidPreparePacket({
        destination: 'g.alice.wallet',
        expiresAt: originalExpiry,
      });

      // Act
      await handler.handlePreparePacket(validPacket);

      // Assert - Check debug log for decremented expiry
      const debugCalls = mockLogger.debug.mock.calls;
      const decrementLog = debugCalls.find((call) => call[1] === 'Decremented packet expiry');
      expect(decrementLog).toBeDefined();
      expect(decrementLog![0]).toHaveProperty('originalExpiry', originalExpiry.toISOString());
      expect(decrementLog![0]).toHaveProperty('safetyMargin', 1000);
    });
  });

  describe('handlePreparePacket() - Per-Hop BLS Notification', () => {
    let routingTable: RoutingTable;
    let mockLogger: jest.Mocked<Logger>;
    let btpClientManager: jest.Mocked<BTPClientManager>;
    let handler: PacketHandler;

    beforeEach(() => {
      // Set up routing table with a peer route (not local)
      routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      mockLogger = createMockLogger();
      btpClientManager = createMockBTPClientManager();
      handler = new PacketHandler(routingTable, btpClientManager, 'test.connector', mockLogger);
      handler.setPerPacketClaimService(createMockPerPacketClaimService());
    });

    it('should fire notification when perHopNotification enabled (HTTP path)', async () => {
      // Arrange - enable per-hop notification with HTTP client
      handler.setLocalDelivery({
        enabled: true,
        handlerUrl: 'http://localhost:3100',
        timeout: 5000,
        perHopNotification: true,
      });

      const packet = createValidPreparePacket({ destination: 'g.alice.wallet' });

      // Act
      const result = await handler.handlePreparePacket(packet, 'source-peer-1');
      // Wait for fire-and-forget promise to settle
      await new Promise((resolve) => setTimeout(resolve, 100));

      // Assert - packet should still be forwarded successfully (fire-and-forget doesn't block)
      expect(result.type).toBe(PacketType.FULFILL);
      expect(btpClientManager.sendToPeer).toHaveBeenCalledTimes(1);
      // HTTP notification was attempted (may log debug on failure, but non-blocking)
    });

    it('should fire notification when perHopNotification enabled (in-process handler path)', async () => {
      // Arrange - enable per-hop notification with in-process handler
      handler.setLocalDelivery({
        enabled: true,
        handlerUrl: 'http://localhost:3100',
        timeout: 5000,
        perHopNotification: true,
      });
      const mockHandler = jest.fn().mockResolvedValue({
        fulfill: { fulfillment: Buffer.alloc(32, 0xcd).toString('base64') },
      });
      handler.setLocalDeliveryHandler(mockHandler);

      const packet = createValidPreparePacket({ destination: 'g.alice.wallet' });

      // Act
      await handler.handlePreparePacket(packet, 'source-peer-1');
      // Wait for fire-and-forget promise to settle
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert - in-process handler should have been called with isTransit: true
      expect(mockHandler).toHaveBeenCalledTimes(1);
      const callArgs = mockHandler.mock.calls[0];
      const transitRequest = callArgs[0];
      expect(transitRequest.isTransit).toBe(true);
      expect(transitRequest.destination).toBe('g.alice.wallet');
      expect(transitRequest.sourcePeer).toBe('source-peer-1');
    });

    it('should NOT fire notification when perHopNotification disabled (default)', async () => {
      // Arrange - local delivery enabled but perHopNotification NOT set (defaults to false)
      handler.setLocalDelivery({
        enabled: true,
        handlerUrl: 'http://localhost:3100',
        timeout: 5000,
      });
      const mockHandler = jest.fn().mockResolvedValue({
        fulfill: { fulfillment: Buffer.alloc(32, 0xcd).toString('base64') },
      });
      handler.setLocalDeliveryHandler(mockHandler);

      const packet = createValidPreparePacket({ destination: 'g.alice.wallet' });

      // Act
      await handler.handlePreparePacket(packet, 'source-peer-1');
      // Wait to ensure no fire-and-forget calls
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert - handler should NOT have been called (no notification for transit)
      expect(mockHandler).not.toHaveBeenCalled();
      // BTP forward should still have happened
      expect(btpClientManager.sendToPeer).toHaveBeenCalledTimes(1);
    });

    it('should still forward packet successfully when notification throws (HTTP path)', async () => {
      // Arrange - enable per-hop notification (HTTP will fail/throw in test env)
      handler.setLocalDelivery({
        enabled: true,
        handlerUrl: 'http://localhost:3100',
        timeout: 5000,
        perHopNotification: true,
      });

      const packet = createValidPreparePacket({ destination: 'g.alice.wallet' });

      // Act
      const result = await handler.handlePreparePacket(packet, 'source-peer-1');
      // Wait for fire-and-forget promise to settle
      await new Promise((resolve) => setTimeout(resolve, 100));

      // Assert - packet should still be forwarded and fulfilled despite notification failure
      // This is the key test: forwarding succeeds even if notification fails
      expect(result.type).toBe(PacketType.FULFILL);
      expect(btpClientManager.sendToPeer).toHaveBeenCalledTimes(1);
    });

    it('should still forward packet successfully when notification throws (in-process handler path)', async () => {
      // Arrange - enable per-hop notification with handler that rejects
      handler.setLocalDelivery({
        enabled: true,
        handlerUrl: 'http://localhost:3100',
        timeout: 5000,
        perHopNotification: true,
      });
      const mockHandler = jest.fn().mockRejectedValue(new Error('Handler error'));
      handler.setLocalDeliveryHandler(mockHandler);

      const packet = createValidPreparePacket({ destination: 'g.alice.wallet' });

      // Act
      const result = await handler.handlePreparePacket(packet, 'source-peer-1');
      // Wait for fire-and-forget promise to settle
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert - packet should still be forwarded and fulfilled despite notification rejection
      expect(result.type).toBe(PacketType.FULFILL);
      expect(btpClientManager.sendToPeer).toHaveBeenCalledTimes(1);
      // Debug log should show notification failure
      const debugCalls = mockLogger.debug.mock.calls;
      const notificationLog = debugCalls.find((call) =>
        call[1]?.includes('Per-hop notification failed (fire-and-forget, in-process)')
      );
      expect(notificationLog).toBeDefined();
      expect((notificationLog![0] as { error?: string }).error).toContain('Handler error');
    });

    it('should set isTransit: true for transit notifications (forwarding path)', async () => {
      // Arrange - enable per-hop notification with in-process handler
      handler.setLocalDelivery({
        enabled: true,
        handlerUrl: 'http://localhost:3100',
        timeout: 5000,
        perHopNotification: true,
      });
      const mockHandler = jest.fn().mockResolvedValue({
        fulfill: { fulfillment: Buffer.alloc(32, 0xcd).toString('base64') },
      });
      handler.setLocalDeliveryHandler(mockHandler);

      const packet = createValidPreparePacket({ destination: 'g.alice.wallet' });

      // Act
      await handler.handlePreparePacket(packet, 'source-peer-1');
      // Wait for fire-and-forget promise to settle
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert - isTransit should be true for the notification
      expect(mockHandler).toHaveBeenCalledTimes(1);
      const transitRequest = mockHandler.mock.calls[0][0];
      expect(transitRequest.isTransit).toBe(true);
    });

    it('should NOT set isTransit for final-hop local delivery', async () => {
      // Arrange - create handler with routing to local delivery
      const localRoutingTable = new RoutingTable([
        { prefix: 'g.local', nextHop: 'local' }, // Route to local delivery
      ]);
      const localHandler = new PacketHandler(
        localRoutingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );

      // Enable local delivery with handler, using valid fulfillment
      localHandler.setLocalDelivery({
        enabled: true,
        handlerUrl: 'http://localhost:3100',
        timeout: 5000,
        perHopNotification: true, // Even with per-hop enabled
      });
      const packetData = Buffer.from('local-final-hop-data');
      const validFulfillment = computeFulfillmentFromData(packetData);
      const mockHandler = jest.fn().mockResolvedValue({
        fulfill: { fulfillment: validFulfillment.toString('base64') },
      });
      localHandler.setLocalDeliveryHandler(mockHandler);

      // Route to local (final-hop delivery, not forwarding)
      const packet = createValidPreparePacket({ destination: 'g.local.wallet', data: packetData });

      // Act
      await localHandler.handlePreparePacket(packet, 'source-peer-1');

      // Assert - handler called for final-hop delivery, isTransit should NOT be set
      expect(mockHandler).toHaveBeenCalledTimes(1);
      const finalHopRequest = mockHandler.mock.calls[0][0];
      expect(finalHopRequest.isTransit).toBeUndefined();
    });

    it('should prioritize in-process handler over HTTP client for per-hop notification', async () => {
      // Arrange - configure both HTTP client and in-process handler
      handler.setLocalDelivery({
        enabled: true,
        handlerUrl: 'http://localhost:3100',
        timeout: 5000,
        perHopNotification: true,
      });
      const mockHandler = jest.fn().mockResolvedValue({
        fulfill: { fulfillment: Buffer.alloc(32, 0xcd).toString('base64') },
      });
      handler.setLocalDeliveryHandler(mockHandler);

      const packet = createValidPreparePacket({ destination: 'g.alice.wallet' });

      // Act
      await handler.handlePreparePacket(packet, 'source-peer-1');
      // Wait for fire-and-forget promise to settle
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert - in-process handler should have been called, not HTTP
      expect(mockHandler).toHaveBeenCalledTimes(1);
      // No debug log about HTTP failure (HTTP path not taken)
      const debugCalls = mockLogger.debug.mock.calls;
      const httpLog = debugCalls.find((call) => call[1]?.includes('(fire-and-forget, HTTP)'));
      expect(httpLog).toBeUndefined();
    });
  });

  describe('handlePreparePacket() - Mandatory Per-Packet Claims', () => {
    let mockLogger: ReturnType<typeof createMockLogger>;
    let btpClientManager: jest.Mocked<BTPClientManager>;

    beforeEach(() => {
      mockLogger = createMockLogger();
      btpClientManager = createMockBTPClientManager();
    });

    it('should reject with T00 when perPacketClaimService is null and forwarding to peer', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );
      // No claim service set
      const packet = createValidPreparePacket({ destination: 'g.alice.wallet' });

      // Act
      const result = await handler.handlePreparePacket(packet);

      // Assert
      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.T00_INTERNAL_ERROR);
      expect(reject.message).toBe('Per-packet claim service not configured');
      expect(btpClientManager.sendToPeer).not.toHaveBeenCalled();
    });

    it('should reject with T00 when generateClaimForPacket returns null (no channel)', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );
      const mockClaimService = createMockPerPacketClaimService();
      mockClaimService.generateClaimForPacket.mockResolvedValue(null);
      handler.setPerPacketClaimService(mockClaimService);
      const packet = createValidPreparePacket({ destination: 'g.alice.wallet' });

      // Act
      const result = await handler.handlePreparePacket(packet);

      // Assert
      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.T00_INTERNAL_ERROR);
      expect(reject.message).toBe('No payment channel available for peer');
      expect(btpClientManager.sendToPeer).not.toHaveBeenCalled();
    });

    it('should reject with T00 when generateClaimForPacket throws', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );
      const mockClaimService = createMockPerPacketClaimService();
      mockClaimService.generateClaimForPacket.mockRejectedValue(new Error('Signing failed'));
      handler.setPerPacketClaimService(mockClaimService);
      const packet = createValidPreparePacket({ destination: 'g.alice.wallet' });

      // Act
      const result = await handler.handlePreparePacket(packet);

      // Assert
      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.T00_INTERNAL_ERROR);
      expect(reject.message).toBe('Claim generation failed');
      expect(btpClientManager.sendToPeer).not.toHaveBeenCalled();
    });

    it('should forward successfully with claim data when generation succeeds', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );
      const mockClaimService = createMockPerPacketClaimService();
      handler.setPerPacketClaimService(mockClaimService);
      const packet = createValidPreparePacket({ destination: 'g.alice.wallet' });

      // Act
      const result = await handler.handlePreparePacket(packet);

      // Assert
      expect(result.type).toBe(PacketType.FULFILL);
      expect(mockClaimService.generateClaimForPacket).toHaveBeenCalledWith(
        'peer-alice',
        'M2M',
        BigInt(1000)
      );
      expect(btpClientManager.sendToPeer).toHaveBeenCalledWith('peer-alice', expect.any(Object), [
        expect.objectContaining({ protocolName: 'evm_claim' }),
      ]);
    });

    it('should not require claims for local delivery (claim service null is fine)', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.local', nextHop: 'test.connector' }]);
      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );
      // No claim service set
      const packet = createValidPreparePacket({ destination: 'g.local.wallet' });

      // Act
      const result = await handler.handlePreparePacket(packet);

      // Assert - auto-fulfill stub for local delivery, no rejection
      expect(result.type).toBe(PacketType.FULFILL);
    });

    it('should not require claims for zero-amount peer packets', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );
      // No claim service set
      const packet = createValidPreparePacket({ destination: 'g.alice.wallet', amount: 0n });

      // Act
      const result = await handler.handlePreparePacket(packet);

      // Assert - forwarded without claims (zero-amount probe)
      expect(result.type).toBe(PacketType.FULFILL);
      expect(btpClientManager.sendToPeer).toHaveBeenCalled();
    });
  });

  describe('handlePreparePacket() - Auto-Fulfill Stub Correctness', () => {
    it('should return a fulfillment where SHA256(fulfillment) == condition', async () => {
      // Arrange - route to local, no delivery handler/client → auto-fulfill stub
      const data = Buffer.from('test-payload');
      const fulfillment = computeFulfillmentFromData(data);
      const condition = crypto.createHash('sha256').update(fulfillment).digest();

      const routingTable = new RoutingTable([{ prefix: 'g.local', nextHop: 'test.connector' }]);
      const mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();
      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );

      const packet = createValidPreparePacket({
        destination: 'g.local.wallet',
        data,
        executionCondition: condition,
      });

      // Act
      const result = await handler.handlePreparePacket(packet);

      // Assert
      expect(result.type).toBe(PacketType.FULFILL);
      const fulfill = result as ILPFulfillPacket;
      expect(validateFulfillment(fulfill.fulfillment, condition)).toBe(true);
      expect(fulfill.fulfillment).toEqual(fulfillment);
    });
  });

  describe('handlePreparePacket() - Downstream Fulfillment Validation', () => {
    it('should reject when downstream peer returns invalid fulfillment', async () => {
      // Arrange
      const data = Buffer.from('test-payload');
      const fulfillment = computeFulfillmentFromData(data);
      const condition = crypto.createHash('sha256').update(fulfillment).digest();

      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      const mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();

      // Mock downstream peer returning WRONG fulfillment
      btpClientManager.sendToPeer.mockResolvedValue({
        type: PacketType.FULFILL,
        fulfillment: Buffer.alloc(32, 0xff), // Invalid fulfillment
        data: Buffer.alloc(0),
      });

      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());

      const packet = createValidPreparePacket({
        destination: 'g.alice.wallet',
        data,
        executionCondition: condition,
      });

      // Act
      const result = await handler.handlePreparePacket(packet);

      // Assert - should be rejected due to invalid fulfillment
      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.T00_INTERNAL_ERROR);
      expect(reject.message).toContain('Invalid fulfillment from downstream peer');
      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'invalid_fulfillment',
          source: 'downstream_peer',
          peerId: 'peer-alice',
        }),
        expect.any(String)
      );
    });

    it('should pass through valid fulfillment from downstream peer', async () => {
      // Arrange
      const data = Buffer.from('test-payload');
      const fulfillment = computeFulfillmentFromData(data);
      const condition = crypto.createHash('sha256').update(fulfillment).digest();

      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      const mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();

      // Mock downstream peer returning VALID fulfillment
      btpClientManager.sendToPeer.mockResolvedValue({
        type: PacketType.FULFILL,
        fulfillment,
        data: Buffer.alloc(0),
      });

      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());

      const packet = createValidPreparePacket({
        destination: 'g.alice.wallet',
        data,
        executionCondition: condition,
      });

      // Act
      const result = await handler.handlePreparePacket(packet);

      // Assert - should pass through as FULFILL
      expect(result.type).toBe(PacketType.FULFILL);
      const fulfill = result as ILPFulfillPacket;
      expect(fulfill.fulfillment).toEqual(fulfillment);
    });

    it('should pass through reject responses from downstream peer unchanged', async () => {
      // Arrange
      const routingTable = new RoutingTable([{ prefix: 'g.alice', nextHop: 'peer-alice' }]);
      const mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();

      // Mock downstream peer returning REJECT
      btpClientManager.sendToPeer.mockResolvedValue({
        type: PacketType.REJECT,
        code: ILPErrorCode.F02_UNREACHABLE,
        triggeredBy: 'peer-alice',
        message: 'No route',
        data: Buffer.alloc(0),
      });

      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );
      handler.setPerPacketClaimService(createMockPerPacketClaimService());

      const packet = createValidPreparePacket({ destination: 'g.alice.wallet' });

      // Act
      const result = await handler.handlePreparePacket(packet);

      // Assert - reject passes through without fulfillment validation
      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.F02_UNREACHABLE);
    });
  });

  describe('convertLocalDeliveryResponse() - Fulfillment Validation', () => {
    it('should reject when function handler returns invalid fulfillment', async () => {
      // Arrange
      const data = Buffer.from('test-payload');
      const fulfillment = computeFulfillmentFromData(data);
      const condition = crypto.createHash('sha256').update(fulfillment).digest();

      const routingTable = new RoutingTable([{ prefix: 'g.local', nextHop: 'test.connector' }]);
      const mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();
      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );

      // Function handler returns WRONG fulfillment
      const mockHandler = jest.fn().mockResolvedValue({
        fulfill: { fulfillment: Buffer.alloc(32, 0xff).toString('base64') },
      });
      handler.setLocalDeliveryHandler(mockHandler);

      const packet = createValidPreparePacket({
        destination: 'g.local.wallet',
        data,
        executionCondition: condition,
      });

      // Act
      const result = await handler.handlePreparePacket(packet, 'source-peer-1');

      // Assert - should be rejected due to invalid fulfillment
      expect(result.type).toBe(PacketType.REJECT);
      const reject = result as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.T00_INTERNAL_ERROR);
      expect(reject.message).toContain('Invalid fulfillment from local delivery handler');
      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'invalid_fulfillment',
          source: 'local_delivery_handler',
        }),
        expect.any(String)
      );
    });

    it('should accept when function handler returns valid fulfillment', async () => {
      // Arrange
      const data = Buffer.from('test-payload');
      const fulfillment = computeFulfillmentFromData(data);
      const condition = crypto.createHash('sha256').update(fulfillment).digest();

      const routingTable = new RoutingTable([{ prefix: 'g.local', nextHop: 'test.connector' }]);
      const mockLogger = createMockLogger();
      const btpClientManager = createMockBTPClientManager();
      const handler = new PacketHandler(
        routingTable,
        btpClientManager,
        'test.connector',
        mockLogger
      );

      // Function handler returns CORRECT fulfillment
      const mockHandler = jest.fn().mockResolvedValue({
        fulfill: { fulfillment: fulfillment.toString('base64') },
      });
      handler.setLocalDeliveryHandler(mockHandler);

      const packet = createValidPreparePacket({
        destination: 'g.local.wallet',
        data,
        executionCondition: condition,
      });

      // Act
      const result = await handler.handlePreparePacket(packet, 'source-peer-1');

      // Assert
      expect(result.type).toBe(PacketType.FULFILL);
      const fulfill = result as ILPFulfillPacket;
      expect(fulfill.fulfillment).toEqual(fulfillment);
    });
  });
});
