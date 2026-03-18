/**
 * Unit tests for packet-factory.ts
 */

import { createHash } from 'crypto';
import { PacketType, ILPErrorCode } from '@toon-protocol/shared';
import {
  createTestPreparePacket,
  createTestFulfillPacket,
  createTestRejectPacket,
} from '../../src/packet-factory';

describe('packet-factory', () => {
  describe('createTestPreparePacket', () => {
    it('should create valid Prepare packet structure', () => {
      const destination = 'g.test.dest';
      const amount = 1000n;
      const expirySeconds = 30;

      const { packet, preimage } = createTestPreparePacket(destination, amount, expirySeconds);

      expect(packet.type).toBe(PacketType.PREPARE);
      expect(packet.destination).toBe(destination);
      expect(packet.amount).toBe(amount);
      expect(packet.executionCondition).toBeInstanceOf(Buffer);
      expect(packet.executionCondition.length).toBe(32);
      expect(packet.expiresAt).toBeInstanceOf(Date);
      expect(packet.data).toBeInstanceOf(Buffer);
      expect(preimage).toBeInstanceOf(Buffer);
      expect(preimage.length).toBe(32);
    });

    it('should generate execution condition that matches SHA-256 hash of preimage', () => {
      const { packet, preimage } = createTestPreparePacket('g.test', 1000n, 30);

      const hash = createHash('sha256').update(preimage).digest();

      expect(hash.equals(packet.executionCondition)).toBe(true);
    });

    it('should throw error for invalid destination address', () => {
      expect(() => {
        createTestPreparePacket('..invalid', 1000n, 30);
      }).toThrow('Invalid ILP address');

      expect(() => {
        createTestPreparePacket('invalid..address', 1000n, 30);
      }).toThrow('Invalid ILP address');

      expect(() => {
        createTestPreparePacket('.leading.dot', 1000n, 30);
      }).toThrow('Invalid ILP address');
    });

    it('should calculate expiry timestamp correctly', () => {
      const expirySeconds = 60;
      const beforeTime = Date.now();

      const { packet } = createTestPreparePacket('g.test', 1000n, expirySeconds);

      const afterTime = Date.now();
      const expectedExpiry = beforeTime + expirySeconds * 1000;
      const actualExpiry = packet.expiresAt.getTime();

      // Allow 5 second tolerance for test execution time
      expect(actualExpiry).toBeGreaterThanOrEqual(expectedExpiry - 5000);
      expect(actualExpiry).toBeLessThanOrEqual(afterTime + expirySeconds * 1000 + 5000);
    });

    it('should include optional data payload', () => {
      const dataPayload = Buffer.from('test data', 'utf8');
      const { packet } = createTestPreparePacket('g.test', 1000n, 30, dataPayload);

      expect(packet.data.equals(dataPayload)).toBe(true);
    });

    it('should use empty buffer when no data payload provided', () => {
      const { packet } = createTestPreparePacket('g.test', 1000n, 30);

      expect(packet.data.length).toBe(0);
    });

    it('should generate unique preimages for each packet', () => {
      const { preimage: preimage1 } = createTestPreparePacket('g.test', 1000n, 30);
      const { preimage: preimage2 } = createTestPreparePacket('g.test', 1000n, 30);

      expect(preimage1.equals(preimage2)).toBe(false);
    });
  });

  describe('createTestFulfillPacket', () => {
    it('should create valid Fulfill packet', () => {
      const { preimage } = createTestPreparePacket('g.test', 1000n, 30);
      const fulfillPacket = createTestFulfillPacket(preimage);

      expect(fulfillPacket.type).toBe(PacketType.FULFILL);
      expect(fulfillPacket.fulfillment).toBeInstanceOf(Buffer);
      expect(fulfillPacket.fulfillment.equals(preimage)).toBe(true);
      expect(fulfillPacket.data).toBeInstanceOf(Buffer);
      expect(fulfillPacket.data.length).toBe(0);
    });

    it('should include optional data payload', () => {
      const { preimage } = createTestPreparePacket('g.test', 1000n, 30);
      const dataPayload = Buffer.from('return data', 'utf8');
      const fulfillPacket = createTestFulfillPacket(preimage, dataPayload);

      expect(fulfillPacket.data.equals(dataPayload)).toBe(true);
    });

    it('should throw error if preimage is not 32 bytes', () => {
      const tooShort = Buffer.alloc(16);
      const tooLong = Buffer.alloc(64);

      expect(() => {
        createTestFulfillPacket(tooShort);
      }).toThrow('Preimage must be 32 bytes');

      expect(() => {
        createTestFulfillPacket(tooLong);
      }).toThrow('Preimage must be 32 bytes');
    });
  });

  describe('createTestRejectPacket', () => {
    it('should create valid Reject packet', () => {
      const code = ILPErrorCode.F02_UNREACHABLE;
      const message = 'Destination unreachable';
      const triggeredBy = 'g.connector.test';

      const rejectPacket = createTestRejectPacket(code, message, triggeredBy);

      expect(rejectPacket.type).toBe(PacketType.REJECT);
      expect(rejectPacket.code).toBe(code);
      expect(rejectPacket.message).toBe(message);
      expect(rejectPacket.triggeredBy).toBe(triggeredBy);
      expect(rejectPacket.data).toBeInstanceOf(Buffer);
      expect(rejectPacket.data.length).toBe(0);
    });

    it('should include optional data payload', () => {
      const dataPayload = Buffer.from('error details', 'utf8');
      const rejectPacket = createTestRejectPacket(
        ILPErrorCode.T01_PEER_UNREACHABLE,
        'Peer timeout',
        'g.connector',
        dataPayload
      );

      expect(rejectPacket.data.equals(dataPayload)).toBe(true);
    });

    it('should throw error for invalid triggeredBy address', () => {
      expect(() => {
        createTestRejectPacket(ILPErrorCode.F00_BAD_REQUEST, 'Error', '..invalid');
      }).toThrow('Invalid ILP address');

      expect(() => {
        createTestRejectPacket(ILPErrorCode.F00_BAD_REQUEST, 'Error', 'invalid..address');
      }).toThrow('Invalid ILP address');
    });

    it('should support all ILP error code types', () => {
      // Final errors (F-prefix)
      const finalError = createTestRejectPacket(ILPErrorCode.F02_UNREACHABLE, 'Test', 'g.test');
      expect(finalError.code).toBe('F02');

      // Temporary errors (T-prefix)
      const tempError = createTestRejectPacket(ILPErrorCode.T01_PEER_UNREACHABLE, 'Test', 'g.test');
      expect(tempError.code).toBe('T01');

      // Relative errors (R-prefix)
      const relativeError = createTestRejectPacket(
        ILPErrorCode.R00_TRANSFER_TIMED_OUT,
        'Test',
        'g.test'
      );
      expect(relativeError.code).toBe('R00');
    });
  });
});
