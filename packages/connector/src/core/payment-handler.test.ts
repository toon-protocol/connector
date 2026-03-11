/**
 * Unit tests for payment-handler adapter
 * @packageDocumentation
 */

import * as crypto from 'crypto';
import { Logger } from '../utils/logger';
import { LocalDeliveryRequest } from '../config/types';
import {
  createPaymentHandlerAdapter,
  PaymentHandler,
  PaymentRequest,
  REJECT_CODE_MAP,
  computeFulfillmentFromData,
  validateFulfillment,
  generatePaymentId,
  mapRejectCode,
  validateResponseData,
} from './payment-handler';

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
 * Create a valid LocalDeliveryRequest for testing.
 */
const createTestPacket = (overrides?: Partial<LocalDeliveryRequest>): LocalDeliveryRequest => {
  const data = Buffer.from('test-payload');
  return {
    destination: 'g.peerA.alice',
    amount: '1000',
    executionCondition: crypto
      .createHash('sha256')
      .update(crypto.createHash('sha256').update(data).digest())
      .digest()
      .toString('base64'),
    expiresAt: new Date(Date.now() + 30000).toISOString(),
    data: data.toString('base64'),
    sourcePeer: 'peerA',
    ...overrides,
  };
};

describe('computeFulfillmentFromData', () => {
  it('should return SHA256 of input data', () => {
    const data = Buffer.from('test-payload');
    const result = computeFulfillmentFromData(data);

    const expected = crypto.createHash('sha256').update(data).digest();
    expect(result).toEqual(expected);
    expect(result.length).toBe(32);
  });
});

describe('validateFulfillment', () => {
  it('should return true when SHA256(fulfillment) equals condition', () => {
    const data = Buffer.from('test-payload');
    const fulfillment = crypto.createHash('sha256').update(data).digest();
    const condition = crypto.createHash('sha256').update(fulfillment).digest();

    expect(validateFulfillment(fulfillment, condition)).toBe(true);
  });

  it('should return false when fulfillment does not match condition', () => {
    const fulfillment = Buffer.alloc(32, 0xaa);
    const wrongCondition = Buffer.alloc(32, 0xbb);

    expect(validateFulfillment(fulfillment, wrongCondition)).toBe(false);
  });

  it('should work with computeFulfillmentFromData output', () => {
    const data = Buffer.from('some-packet-data');
    const fulfillment = computeFulfillmentFromData(data);
    const condition = crypto.createHash('sha256').update(fulfillment).digest();

    expect(validateFulfillment(fulfillment, condition)).toBe(true);
  });

  it('should return false when fulfillment is all zeros', () => {
    const fulfillment = Buffer.alloc(32, 0);
    const condition = Buffer.alloc(32, 0);

    // SHA256(zeros) != zeros
    expect(validateFulfillment(fulfillment, condition)).toBe(false);
  });
});

describe('generatePaymentId', () => {
  it('should return a base64url string', () => {
    const id = generatePaymentId();
    expect(typeof id).toBe('string');
    expect(id.length).toBeGreaterThan(0);
    // base64url chars only
    expect(id).toMatch(/^[A-Za-z0-9_-]+$/);
  });

  it('should return unique values', () => {
    const ids = new Set(Array.from({ length: 100 }, () => generatePaymentId()));
    expect(ids.size).toBe(100);
  });
});

describe('mapRejectCode', () => {
  it('should map known business codes to ILP codes', () => {
    expect(mapRejectCode('insufficient_funds')).toBe('T04');
    expect(mapRejectCode('expired')).toBe('R00');
    expect(mapRejectCode('invalid_request')).toBe('F00');
    expect(mapRejectCode('invalid_amount')).toBe('F03');
    expect(mapRejectCode('unexpected_payment')).toBe('F06');
    expect(mapRejectCode('application_error')).toBe('F99');
    expect(mapRejectCode('internal_error')).toBe('T00');
    expect(mapRejectCode('timeout')).toBe('T00');
  });

  it('should return F99 for unknown codes', () => {
    expect(mapRejectCode('unknown_code')).toBe('F99');
    expect(mapRejectCode('')).toBe('F99');
  });
});

describe('REJECT_CODE_MAP', () => {
  it('should contain all expected mappings', () => {
    expect(Object.keys(REJECT_CODE_MAP)).toEqual([
      'insufficient_funds',
      'expired',
      'invalid_request',
      'invalid_amount',
      'unexpected_payment',
      'application_error',
      'internal_error',
      'timeout',
    ]);
  });
});

describe('validateResponseData', () => {
  let logger: jest.Mocked<Logger>;

  beforeEach(() => {
    logger = createMockLogger();
  });

  it('should pass through valid base64 data', () => {
    const data = Buffer.from('hello').toString('base64');
    expect(validateResponseData(data, logger)).toBe(data);
  });

  it('should return undefined for falsy values', () => {
    expect(validateResponseData(undefined, logger)).toBeUndefined();
    expect(validateResponseData('', logger)).toBe('');
  });

  it('should reject non-base64 data', () => {
    const result = validateResponseData('not!valid!base64!!!', logger);
    expect(result).toBeUndefined();
    expect(logger.warn).toHaveBeenCalled();
  });

  it('should reject oversized data', () => {
    const largeData = Buffer.alloc(32769).toString('base64');
    const result = validateResponseData(largeData, logger);
    expect(result).toBeUndefined();
    expect(logger.warn).toHaveBeenCalledWith(
      expect.objectContaining({ size: 32769, limit: 32768 }),
      expect.any(String)
    );
  });
});

describe('createPaymentHandlerAdapter', () => {
  let logger: jest.Mocked<Logger>;
  let handler: jest.MockedFunction<PaymentHandler>;
  let capturedRequest: PaymentRequest | null;

  beforeEach(() => {
    logger = createMockLogger();
    capturedRequest = null;
    handler = jest.fn(async (req: PaymentRequest) => {
      capturedRequest = req;
      return { accept: true };
    });
  });

  describe('expiry check', () => {
    it('should reject expired packets with R00 without calling handler', async () => {
      const adapter = createPaymentHandlerAdapter(handler, logger);
      const packet = createTestPacket({
        expiresAt: new Date(Date.now() - 10000).toISOString(),
      });

      const result = await adapter(packet, 'peerA');

      expect(result.reject).toBeDefined();
      expect(result.reject!.code).toBe('R00');
      expect(result.reject!.message).toBe('Payment has expired');
      expect(handler).not.toHaveBeenCalled();
    });
  });

  describe('PaymentRequest transformation', () => {
    it('should create PaymentRequest with paymentId, no executionCondition, no sourcePeer', async () => {
      const adapter = createPaymentHandlerAdapter(handler, logger);
      const packet = createTestPacket();

      await adapter(packet, 'peerA');

      expect(capturedRequest).toBeDefined();
      expect(capturedRequest!.paymentId).toBeDefined();
      expect(typeof capturedRequest!.paymentId).toBe('string');
      expect(capturedRequest!.paymentId.length).toBeGreaterThan(0);
      expect(capturedRequest!.destination).toBe(packet.destination);
      expect(capturedRequest!.amount).toBe(packet.amount);
      expect(capturedRequest!.expiresAt).toBe(packet.expiresAt);
      expect(capturedRequest!.data).toBe(packet.data);
      // Should NOT have executionCondition or sourcePeer
      expect(capturedRequest).not.toHaveProperty('executionCondition');
      expect(capturedRequest).not.toHaveProperty('sourcePeer');
    });

    it('should set data to undefined when packet.data is empty', async () => {
      const adapter = createPaymentHandlerAdapter(handler, logger);
      const packet = createTestPacket({ data: '' });

      await adapter(packet, 'peerA');

      expect(capturedRequest!.data).toBeUndefined();
    });
  });

  describe('accept response', () => {
    it('should compute fulfillment as SHA256(data) and return fulfill', async () => {
      const adapter = createPaymentHandlerAdapter(handler, logger);
      const packet = createTestPacket();

      const result = await adapter(packet, 'peerA');

      expect(result.fulfill).toBeDefined();
      expect(result.reject).toBeUndefined();

      // Verify fulfillment = SHA256(decoded data)
      const expectedFulfillment = crypto
        .createHash('sha256')
        .update(Buffer.from(packet.data, 'base64'))
        .digest()
        .toString('base64');
      expect(result.fulfill!.fulfillment).toBe(expectedFulfillment);
    });

    it('should pass through response data on accept', async () => {
      const responseData = Buffer.from('response-payload').toString('base64');
      handler.mockResolvedValue({ accept: true, data: responseData });

      const adapter = createPaymentHandlerAdapter(handler, logger);
      const packet = createTestPacket();

      const result = await adapter(packet, 'peerA');

      expect(result.fulfill!.data).toBe(responseData);
    });
  });

  describe('reject response', () => {
    it('should map known reject code (insufficient_funds → T04)', async () => {
      handler.mockResolvedValue({
        accept: false,
        rejectReason: { code: 'insufficient_funds', message: 'Not enough funds' },
      });

      const adapter = createPaymentHandlerAdapter(handler, logger);
      const result = await adapter(createTestPacket(), 'peerA');

      expect(result.reject).toBeDefined();
      expect(result.reject!.code).toBe('T04');
      expect(result.reject!.message).toBe('Not enough funds');
    });

    it('should map unknown reject code to F99', async () => {
      handler.mockResolvedValue({
        accept: false,
        rejectReason: { code: 'something_weird', message: 'Weird' },
      });

      const adapter = createPaymentHandlerAdapter(handler, logger);
      const result = await adapter(createTestPacket(), 'peerA');

      expect(result.reject!.code).toBe('F99');
    });

    it('should use F99 and default message when rejectReason is not provided', async () => {
      handler.mockResolvedValue({ accept: false });

      const adapter = createPaymentHandlerAdapter(handler, logger);
      const result = await adapter(createTestPacket(), 'peerA');

      expect(result.reject!.code).toBe('F99');
      expect(result.reject!.message).toBe('Payment rejected');
    });

    it('should pass through response data on reject', async () => {
      const responseData = Buffer.from('error-details').toString('base64');
      handler.mockResolvedValue({
        accept: false,
        rejectReason: { code: 'invalid_amount', message: 'Too low' },
        data: responseData,
      });

      const adapter = createPaymentHandlerAdapter(handler, logger);
      const result = await adapter(createTestPacket(), 'peerA');

      expect(result.reject!.data).toBe(responseData);
    });
  });

  describe('handler error', () => {
    it('should return T00 reject when handler throws', async () => {
      handler.mockRejectedValue(new Error('Handler crashed'));

      const adapter = createPaymentHandlerAdapter(handler, logger);
      const result = await adapter(createTestPacket(), 'peerA');

      expect(result.reject).toBeDefined();
      expect(result.reject!.code).toBe('T00');
      expect(result.reject!.message).toBe('Internal error processing payment');
      expect(logger.error).toHaveBeenCalledWith(
        expect.objectContaining({ error: 'Handler crashed' }),
        expect.any(String)
      );
    });

    it('should handle non-Error throws', async () => {
      handler.mockRejectedValue('string error');

      const adapter = createPaymentHandlerAdapter(handler, logger);
      const result = await adapter(createTestPacket(), 'peerA');

      expect(result.reject!.code).toBe('T00');
    });
  });

  describe('invalid response data', () => {
    it('should strip invalid base64 response data on accept', async () => {
      handler.mockResolvedValue({ accept: true, data: 'not!valid!base64!!!' });

      const adapter = createPaymentHandlerAdapter(handler, logger);
      const result = await adapter(createTestPacket(), 'peerA');

      expect(result.fulfill).toBeDefined();
      expect(result.fulfill!.data).toBeUndefined();
    });

    it('should strip oversized response data on reject', async () => {
      const largeData = Buffer.alloc(32769).toString('base64');
      handler.mockResolvedValue({
        accept: false,
        rejectReason: { code: 'invalid_amount', message: 'Bad' },
        data: largeData,
      });

      const adapter = createPaymentHandlerAdapter(handler, logger);
      const result = await adapter(createTestPacket(), 'peerA');

      expect(result.reject!.data).toBeUndefined();
    });
  });
});
