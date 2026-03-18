/**
 * Unit tests for LocalDeliveryClient
 * @packageDocumentation
 */

/* eslint-disable @typescript-eslint/no-explicit-any */
import * as crypto from 'crypto';
import { PacketType, ILPErrorCode, ILPPreparePacket } from '@toon-protocol/shared';
import { LocalDeliveryClient } from './local-delivery-client';
import type { PaymentResponse } from './payment-handler';

// ────────────────────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────────────────────

const createMockLogger = (): any => ({
  info: jest.fn(),
  warn: jest.fn(),
  error: jest.fn(),
  debug: jest.fn(),
  fatal: jest.fn(),
  trace: jest.fn(),
  silent: jest.fn(),
  level: 'info',
  child: jest.fn().mockReturnThis(),
});

/** Build a valid ILPPreparePacket for testing. */
const createTestPacket = (overrides?: Partial<ILPPreparePacket>): ILPPreparePacket => {
  const data = Buffer.from('test-payload');
  const fulfillment = crypto.createHash('sha256').update(data).digest();
  const condition = crypto.createHash('sha256').update(fulfillment).digest();

  return {
    type: PacketType.PREPARE,
    destination: 'g.peerA.alice',
    amount: 1000n,
    executionCondition: condition,
    expiresAt: new Date(Date.now() + 30000),
    data,
    ...overrides,
  };
};

/** Convenience: create the client with enabled config. */
const createClient = (handlerUrl = 'http://localhost:8080', timeout = 30000): LocalDeliveryClient =>
  new LocalDeliveryClient({ enabled: true, handlerUrl, timeout }, createMockLogger());

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

describe('LocalDeliveryClient', () => {
  const originalFetch = global.fetch;

  afterEach(() => {
    global.fetch = originalFetch;
    jest.restoreAllMocks();
  });

  // ── Constructor ────────────────────────────────────────────────────────

  describe('constructor', () => {
    it('should throw when enabled without handlerUrl', () => {
      expect(() => new LocalDeliveryClient({ enabled: true }, createMockLogger())).toThrow(
        'LOCAL_DELIVERY_URL is required'
      );
    });

    it('should not throw when disabled without handlerUrl', () => {
      expect(() => new LocalDeliveryClient({ enabled: false }, createMockLogger())).not.toThrow();
    });
  });

  // ── isEnabled ──────────────────────────────────────────────────────────

  describe('isEnabled', () => {
    it('should return true when enabled', () => {
      expect(createClient().isEnabled()).toBe(true);
    });

    it('should return false when disabled', () => {
      const client = new LocalDeliveryClient({ enabled: false }, createMockLogger());
      expect(client.isEnabled()).toBe(false);
    });
  });

  // ── deliver() — endpoint and request format ────────────────────────────

  describe('deliver — request format', () => {
    it('should POST to /handle-packet (not /ilp/packets)', async () => {
      let capturedUrl = '';
      global.fetch = jest.fn(async (url: any) => {
        capturedUrl = url;
        return new Response(JSON.stringify({ accept: true }), { status: 200 });
      }) as any;

      const client = createClient('http://localhost:8080');
      await client.deliver(createTestPacket(), 'peerA');

      expect(capturedUrl).toBe('http://localhost:8080/handle-packet');
    });

    it('should send PaymentRequest with paymentId, no executionCondition, no sourcePeer', async () => {
      let capturedBody: any = null;
      global.fetch = jest.fn(async (_url: any, init: any) => {
        capturedBody = JSON.parse(init.body);
        return new Response(JSON.stringify({ accept: true }), { status: 200 });
      }) as any;

      const packet = createTestPacket();
      const client = createClient();
      await client.deliver(packet, 'peerA');

      expect(capturedBody).toBeDefined();
      expect(capturedBody.paymentId).toBeDefined();
      expect(typeof capturedBody.paymentId).toBe('string');
      expect(capturedBody.paymentId.length).toBeGreaterThan(0);
      expect(capturedBody.destination).toBe('g.peerA.alice');
      expect(capturedBody.amount).toBe('1000');
      expect(capturedBody.expiresAt).toBeDefined();
      expect(capturedBody.data).toBe(packet.data.toString('base64'));
      // Must NOT contain ILP internals
      expect(capturedBody).not.toHaveProperty('executionCondition');
      expect(capturedBody).not.toHaveProperty('sourcePeer');
    });

    it('should omit data field when packet data is empty', async () => {
      let capturedBody: any = null;
      global.fetch = jest.fn(async (_url: any, init: any) => {
        capturedBody = JSON.parse(init.body);
        return new Response(JSON.stringify({ accept: true }), { status: 200 });
      }) as any;

      const packet = createTestPacket({ data: Buffer.alloc(0) });
      const client = createClient();
      await client.deliver(packet, 'peerA');

      expect(capturedBody.data).toBeUndefined();
    });
  });

  // ── deliver() — accept ─────────────────────────────────────────────────

  describe('deliver — accept', () => {
    it('should return fulfillment as SHA256(data)', async () => {
      global.fetch = jest.fn(
        async () => new Response(JSON.stringify({ accept: true }), { status: 200 })
      ) as any;

      const packet = createTestPacket();
      const client = createClient();
      const result = await client.deliver(packet, 'peerA');

      expect(result.type).toBe(PacketType.FULFILL);
      if (result.type !== PacketType.FULFILL) throw new Error('Expected FULFILL');

      const expectedFulfillment = crypto.createHash('sha256').update(packet.data).digest();
      expect(result.fulfillment).toEqual(expectedFulfillment);
    });

    it('should pass through valid response data on accept', async () => {
      const responseData = Buffer.from('response-payload').toString('base64');
      global.fetch = jest.fn(
        async () =>
          new Response(JSON.stringify({ accept: true, data: responseData }), { status: 200 })
      ) as any;

      const client = createClient();
      const result = await client.deliver(createTestPacket(), 'peerA');

      expect(result.type).toBe(PacketType.FULFILL);
      if (result.type !== PacketType.FULFILL) throw new Error('Expected FULFILL');
      expect(result.data).toEqual(Buffer.from(responseData, 'base64'));
    });
  });

  // ── deliver() — reject ─────────────────────────────────────────────────

  describe('deliver — reject', () => {
    it('should map known reject code (insufficient_funds → T04)', async () => {
      const body: PaymentResponse = {
        accept: false,
        rejectReason: { code: 'insufficient_funds', message: 'Not enough funds' },
      };
      global.fetch = jest.fn(
        async () => new Response(JSON.stringify(body), { status: 200 })
      ) as any;

      const client = createClient();
      const result = await client.deliver(createTestPacket(), 'peerA');

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type !== PacketType.REJECT) throw new Error('Expected REJECT');
      expect(result.code).toBe('T04');
      expect(result.message).toBe('Not enough funds');
    });

    it('should map unknown reject code to F99', async () => {
      const body: PaymentResponse = {
        accept: false,
        rejectReason: { code: 'something_unknown', message: 'Hmm' },
      };
      global.fetch = jest.fn(
        async () => new Response(JSON.stringify(body), { status: 200 })
      ) as any;

      const client = createClient();
      const result = await client.deliver(createTestPacket(), 'peerA');

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type !== PacketType.REJECT) throw new Error('Expected REJECT');
      expect(result.code).toBe('F99');
    });

    it('should use F99 and default message when rejectReason is absent', async () => {
      global.fetch = jest.fn(
        async () => new Response(JSON.stringify({ accept: false }), { status: 200 })
      ) as any;

      const client = createClient();
      const result = await client.deliver(createTestPacket(), 'peerA');

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type !== PacketType.REJECT) throw new Error('Expected REJECT');
      expect(result.code).toBe('F99');
      expect(result.message).toBe('Payment rejected');
    });

    it('should pass through valid response data on reject', async () => {
      const responseData = Buffer.from('error-details').toString('base64');
      const body: PaymentResponse = {
        accept: false,
        rejectReason: { code: 'invalid_amount', message: 'Too low' },
        data: responseData,
      };
      global.fetch = jest.fn(
        async () => new Response(JSON.stringify(body), { status: 200 })
      ) as any;

      const client = createClient();
      const result = await client.deliver(createTestPacket(), 'peerA');

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type !== PacketType.REJECT) throw new Error('Expected REJECT');
      expect(result.data).toEqual(Buffer.from(responseData, 'base64'));
    });
  });

  // ── deliver() — expiry ─────────────────────────────────────────────────

  describe('deliver — expired packet', () => {
    it('should return R00 without making a fetch call', async () => {
      const fetchMock = jest.fn();
      global.fetch = fetchMock as any;

      const packet = createTestPacket({
        expiresAt: new Date(Date.now() - 10000),
      });
      const client = createClient();
      const result = await client.deliver(packet, 'peerA');

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type !== PacketType.REJECT) throw new Error('Expected REJECT');
      expect(result.code).toBe(ILPErrorCode.R00_TRANSFER_TIMED_OUT);
      expect(result.message).toBe('Payment has expired');
      expect(fetchMock).not.toHaveBeenCalled();
    });
  });

  // ── deliver() — HTTP errors ────────────────────────────────────────────

  describe('deliver — HTTP errors', () => {
    it('should return T00 on non-ok HTTP status', async () => {
      global.fetch = jest.fn(
        async () => new Response('Internal Server Error', { status: 500 })
      ) as any;

      const client = createClient();
      const result = await client.deliver(createTestPacket(), 'peerA');

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type !== PacketType.REJECT) throw new Error('Expected REJECT');
      expect(result.code).toBe(ILPErrorCode.T00_INTERNAL_ERROR);
      expect(result.message).toContain('500');
    });

    it('should return T00 on network error', async () => {
      global.fetch = jest.fn(async () => {
        throw new Error('ECONNREFUSED');
      }) as any;

      const client = createClient();
      const result = await client.deliver(createTestPacket(), 'peerA');

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type !== PacketType.REJECT) throw new Error('Expected REJECT');
      expect(result.code).toBe(ILPErrorCode.T00_INTERNAL_ERROR);
      expect(result.message).toBe('ECONNREFUSED');
    });
  });

  // ── deliver() — timeout ────────────────────────────────────────────────

  describe('deliver — timeout', () => {
    it('should return R00 on AbortError', async () => {
      global.fetch = jest.fn(async () => {
        const err = new Error('The operation was aborted');
        err.name = 'AbortError';
        throw err;
      }) as any;

      const client = createClient();
      const result = await client.deliver(createTestPacket(), 'peerA');

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type !== PacketType.REJECT) throw new Error('Expected REJECT');
      expect(result.code).toBe(ILPErrorCode.R00_TRANSFER_TIMED_OUT);
      expect(result.message).toContain('timed out');
    });
  });

  // ── deliver() — invalid / oversized response data ──────────────────────

  describe('deliver — invalid response data', () => {
    it('should strip invalid base64 response data on accept', async () => {
      global.fetch = jest.fn(
        async () =>
          new Response(JSON.stringify({ accept: true, data: 'not!valid!base64!!!' }), {
            status: 200,
          })
      ) as any;

      const client = createClient();
      const result = await client.deliver(createTestPacket(), 'peerA');

      expect(result.type).toBe(PacketType.FULFILL);
      if (result.type !== PacketType.FULFILL) throw new Error('Expected FULFILL');
      // Invalid data stripped → empty buffer
      expect(result.data).toEqual(Buffer.alloc(0));
    });

    it('should strip oversized response data on reject', async () => {
      const largeData = Buffer.alloc(32769).toString('base64');
      const body: PaymentResponse = {
        accept: false,
        rejectReason: { code: 'invalid_amount', message: 'Bad' },
        data: largeData,
      };
      global.fetch = jest.fn(
        async () => new Response(JSON.stringify(body), { status: 200 })
      ) as any;

      const client = createClient();
      const result = await client.deliver(createTestPacket(), 'peerA');

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type !== PacketType.REJECT) throw new Error('Expected REJECT');
      // Oversized data stripped → empty buffer
      expect(result.data).toEqual(Buffer.alloc(0));
    });
  });

  // ── deliver() — malformed response ─────────────────────────────────────

  describe('deliver — malformed response', () => {
    it('should return T00 when response is missing accept field', async () => {
      global.fetch = jest.fn(
        async () => new Response(JSON.stringify({ data: 'something' }), { status: 200 })
      ) as any;

      const client = createClient();
      const result = await client.deliver(createTestPacket(), 'peerA');

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type !== PacketType.REJECT) throw new Error('Expected REJECT');
      expect(result.code).toBe(ILPErrorCode.T00_INTERNAL_ERROR);
      expect(result.message).toContain('Malformed');
    });
  });

  // ── healthCheck ────────────────────────────────────────────────────────

  describe('healthCheck', () => {
    it('should hit /health endpoint and return true on 200', async () => {
      let capturedUrl = '';
      global.fetch = jest.fn(async (url: any) => {
        capturedUrl = url;
        return new Response('OK', { status: 200 });
      }) as any;

      const client = createClient('http://localhost:8080');
      const result = await client.healthCheck();

      expect(result).toBe(true);
      expect(capturedUrl).toBe('http://localhost:8080/health');
    });

    it('should return false on non-ok status', async () => {
      global.fetch = jest.fn(async () => new Response('Not Found', { status: 404 })) as any;

      const result = await createClient().healthCheck();
      expect(result).toBe(false);
    });

    it('should return false on network error', async () => {
      global.fetch = jest.fn(async () => {
        throw new Error('ECONNREFUSED');
      }) as any;

      const result = await createClient().healthCheck();
      expect(result).toBe(false);
    });

    it('should return true without calling fetch when disabled', async () => {
      const fetchMock = jest.fn();
      global.fetch = fetchMock as any;

      const client = new LocalDeliveryClient({ enabled: false }, createMockLogger());
      const result = await client.healthCheck();

      expect(result).toBe(true);
      expect(fetchMock).not.toHaveBeenCalled();
    });
  });
});
