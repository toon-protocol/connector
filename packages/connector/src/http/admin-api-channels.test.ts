/**
 * Unit Tests for Admin API Payment Channel Endpoints (Story 21.1)
 *
 * Tests channel CRUD endpoints: request validation, chain routing,
 * duplicate detection, listing, detail retrieval, auth, and settlement disabled.
 *
 * @module http/admin-api-channels.test
 */

import request from 'supertest';
import express, { Express } from 'express';
import {
  createAdminRouter,
  AdminAPIConfig,
  validateOpenChannelRequest,
  validateDepositRequest,
} from './admin-api';
import type { PeerConfig as SettlementPeerConfig } from '../settlement/types';
import type { Logger } from 'pino';
import type { RoutingTable } from '../routing/routing-table';
import type { BTPClientManager } from '../btp/btp-client-manager';
import type { ChannelManager, ChannelMetadata } from '../settlement/channel-manager';
import type { PaymentChannelSDK } from '../settlement/payment-channel-sdk';
import type { AccountManager } from '../settlement/account-manager';
import type { SettlementMonitor } from '../settlement/settlement-monitor';
import type { ClaimReceiver } from '../settlement/claim-receiver';
import { SettlementState } from '../config/types';

describe('Admin API Channel Endpoints (Story 21.1)', () => {
  let app: Express;
  let mockRoutingTable: jest.Mocked<RoutingTable>;
  let mockBTPClientManager: jest.Mocked<BTPClientManager>;
  let mockLogger: jest.Mocked<Logger>;
  let mockChannelManager: jest.Mocked<ChannelManager>;
  let mockPaymentChannelSDK: jest.Mocked<PaymentChannelSDK>;
  let settlementPeers: Map<string, SettlementPeerConfig>;

  const validEvmRequest = {
    peerId: 'peer-b',
    chain: 'evm:base:8453',
    token: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
    tokenNetwork: '0x1234567890abcdef1234567890abcdef12345678',
    initialDeposit: '1000000',
    settlementTimeout: 86400,
    peerAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
  };

  beforeEach(async () => {
    settlementPeers = new Map();

    mockRoutingTable = {
      addRoute: jest.fn(),
      removeRoute: jest.fn(),
      getAllRoutes: jest.fn().mockReturnValue([]),
      lookup: jest.fn(),
      removeRoutesForPeer: jest.fn(),
    } as unknown as jest.Mocked<RoutingTable>;

    mockBTPClientManager = {
      addPeer: jest.fn().mockResolvedValue(undefined),
      removePeer: jest.fn().mockResolvedValue(undefined),
      getPeerIds: jest.fn().mockReturnValue(['peer-b', 'peer-c', 'peer-d']),
      getPeerStatus: jest.fn().mockReturnValue(new Map()),
      isConnected: jest.fn().mockReturnValue(false),
      getConnectedPeers: jest.fn().mockReturnValue([]),
      getClientForPeer: jest.fn(),
    } as unknown as jest.Mocked<BTPClientManager>;

    // eslint-disable-next-line @typescript-eslint/no-explicit-any
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

    mockChannelManager = {
      ensureChannelExists: jest.fn().mockResolvedValue('0xchannel123'),
      getAllChannels: jest.fn().mockReturnValue([]),
      getChannelById: jest.fn().mockImplementation((channelId: string) => {
        if (channelId === '0xchannel123') {
          return {
            channelId: '0xchannel123',
            peerId: 'peer-b',
            tokenId: 'AGENT',
            tokenAddress: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
            chain: 'evm:base:8453',
            createdAt: new Date(),
            lastActivityAt: new Date(),
            status: 'open',
          };
        }
        return null;
      }),
      getChannelForPeer: jest.fn().mockReturnValue(null),
      markChannelActivity: jest.fn(),
      start: jest.fn(),
      stop: jest.fn(),
      on: jest.fn(),
      emit: jest.fn(),
    } as unknown as jest.Mocked<ChannelManager>;

    mockPaymentChannelSDK = {
      openChannel: jest.fn(),
      getChannelState: jest.fn(),
      getMyChannels: jest.fn(),
      signBalanceProof: jest.fn().mockResolvedValue('0x' + 'ab'.repeat(65)),
      closeChannel: jest.fn().mockResolvedValue(undefined),
      cooperativeSettle: jest.fn().mockResolvedValue(undefined),
      settleChannel: jest.fn(),
      deposit: jest.fn().mockResolvedValue(undefined),
      removeAllListeners: jest.fn(),
    } as unknown as jest.Mocked<PaymentChannelSDK>;

    const config: AdminAPIConfig = {
      routingTable: mockRoutingTable,
      btpClientManager: mockBTPClientManager,
      logger: mockLogger,
      nodeId: 'test-node',
      settlementPeers,
      channelManager: mockChannelManager,
      paymentChannelSDK: mockPaymentChannelSDK,
    };

    app = express();
    app.use('/admin', await createAdminRouter(config));
  });

  afterEach(() => {
    jest.clearAllMocks();
  });

  // --- validateOpenChannelRequest unit tests ---

  describe('validateOpenChannelRequest', () => {
    it('should accept a valid EVM request', () => {
      const result = validateOpenChannelRequest(validEvmRequest);
      expect(result.valid).toBe(true);
    });

    it('should reject missing peerId', () => {
      const result = validateOpenChannelRequest({ ...validEvmRequest, peerId: undefined } as Record<
        string,
        unknown
      >);
      expect(result.valid).toBe(false);
      expect(result.error).toContain('peerId');
    });

    it('should reject missing chain', () => {
      const result = validateOpenChannelRequest({ ...validEvmRequest, chain: undefined } as Record<
        string,
        unknown
      >);
      expect(result.valid).toBe(false);
      expect(result.error).toContain('chain');
    });

    it('should reject invalid chain format (no colons)', () => {
      const result = validateOpenChannelRequest({ ...validEvmRequest, chain: 'evm' });
      expect(result.valid).toBe(false);
      expect(result.error).toContain('Invalid chain format');
    });

    it('should reject unsupported blockchain prefix', () => {
      const result = validateOpenChannelRequest({ ...validEvmRequest, chain: 'solana:mainnet:0' });
      expect(result.valid).toBe(false);
      expect(result.error).toContain('Invalid chain format');
    });

    it('should reject missing initialDeposit', () => {
      const result = validateOpenChannelRequest({
        ...validEvmRequest,
        initialDeposit: undefined,
      } as Record<string, unknown>);
      expect(result.valid).toBe(false);
      expect(result.error).toContain('initialDeposit');
    });

    it('should reject non-numeric initialDeposit', () => {
      const result = validateOpenChannelRequest({ ...validEvmRequest, initialDeposit: 'abc' });
      expect(result.valid).toBe(false);
      expect(result.error).toContain('non-negative integer');
    });

    it('should reject negative initialDeposit', () => {
      const result = validateOpenChannelRequest({ ...validEvmRequest, initialDeposit: '-100' });
      expect(result.valid).toBe(false);
      expect(result.error).toContain('non-negative integer');
    });

    it('should reject invalid token address format', () => {
      const result = validateOpenChannelRequest({ ...validEvmRequest, token: 'invalid' });
      expect(result.valid).toBe(false);
      expect(result.error).toContain('token address');
    });

    it('should reject invalid tokenNetwork address format', () => {
      const result = validateOpenChannelRequest({ ...validEvmRequest, tokenNetwork: 'bad' });
      expect(result.valid).toBe(false);
      expect(result.error).toContain('tokenNetwork');
    });

    it('should reject non-positive settlementTimeout', () => {
      const result = validateOpenChannelRequest({ ...validEvmRequest, settlementTimeout: 0 });
      expect(result.valid).toBe(false);
      expect(result.error).toContain('settlementTimeout');
    });

    it('should reject non-integer settlementTimeout', () => {
      const result = validateOpenChannelRequest({ ...validEvmRequest, settlementTimeout: 1.5 });
      expect(result.valid).toBe(false);
      expect(result.error).toContain('settlementTimeout');
    });
  });

  // --- POST /admin/channels ---

  describe('POST /admin/channels — Chain Routing (AC: 1, 2, 3)', () => {
    it('should open EVM channel and return 201', async () => {
      const res = await request(app).post('/admin/channels').send(validEvmRequest);

      expect(res.status).toBe(201);
      expect(res.body.channelId).toBe('0xchannel123');
      expect(res.body.chain).toBe('evm:base:8453');
      expect(res.body.status).toBe('open');
      expect(res.body.deposit).toBe('1000000');
      expect(mockChannelManager.ensureChannelExists).toHaveBeenCalledWith(
        'peer-b',
        '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
        expect.objectContaining({
          initialDeposit: BigInt('1000000'),
          settlementTimeout: 86400,
          chain: 'evm:base:8453',
        })
      );
    });

    it('should use AGENT as default tokenId when no token provided', async () => {
      const res = await request(app).post('/admin/channels').send({
        peerId: 'peer-b',
        chain: 'evm:base:8453',
        initialDeposit: '500',
        peerAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
      });

      expect(res.status).toBe(201);
      expect(mockChannelManager.ensureChannelExists).toHaveBeenCalledWith(
        'peer-b',
        'AGENT',
        expect.any(Object)
      );
    });
  });

  describe('POST /admin/channels — Request Validation (AC: 4, 8)', () => {
    it('should return 400 for missing peerId', async () => {
      const res = await request(app)
        .post('/admin/channels')
        .send({ chain: 'evm:base:8453', initialDeposit: '1000' });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('peerId');
    });

    it('should return 400 for missing chain', async () => {
      const res = await request(app)
        .post('/admin/channels')
        .send({ peerId: 'peer-b', initialDeposit: '1000' });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('chain');
    });

    it('should return 400 for invalid chain format', async () => {
      const res = await request(app)
        .post('/admin/channels')
        .send({ peerId: 'peer-b', chain: 'invalid', initialDeposit: '1000' });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('Invalid chain format');
    });

    it('should return 400 for unsupported blockchain prefix in chain', async () => {
      const res = await request(app)
        .post('/admin/channels')
        .send({ peerId: 'peer-b', chain: 'solana:mainnet:0', initialDeposit: '1000' });

      expect(res.status).toBe(400);
    });

    it('should return 400 for missing initialDeposit', async () => {
      const res = await request(app)
        .post('/admin/channels')
        .send({ peerId: 'peer-b', chain: 'evm:base:8453' });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('initialDeposit');
    });

    it('should return 400 for non-numeric initialDeposit', async () => {
      const res = await request(app)
        .post('/admin/channels')
        .send({ peerId: 'peer-b', chain: 'evm:base:8453', initialDeposit: 'abc' });

      expect(res.status).toBe(400);
    });

    it('should return 400 for negative initialDeposit', async () => {
      const res = await request(app)
        .post('/admin/channels')
        .send({ peerId: 'peer-b', chain: 'evm:base:8453', initialDeposit: '-100' });

      expect(res.status).toBe(400);
    });

    it('should return 400 for invalid token address format', async () => {
      const res = await request(app)
        .post('/admin/channels')
        .send({ ...validEvmRequest, token: 'bad-address' });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('token address');
    });
  });

  describe('POST /admin/channels — Duplicate Detection (AC: 9)', () => {
    it('should return 409 for existing channel with same peer+token', async () => {
      const existingChannel: ChannelMetadata = {
        channelId: '0xexisting',
        peerId: 'peer-b',
        tokenId: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
        tokenAddress: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
        chain: 'evm:base:8453',
        createdAt: new Date(),
        lastActivityAt: new Date(),
        status: 'open',
      };
      mockChannelManager.getChannelForPeer.mockReturnValue(existingChannel);

      const res = await request(app).post('/admin/channels').send(validEvmRequest);

      expect(res.status).toBe(409);
      expect(res.body.error).toBe('Conflict');
    });

    it('should allow channel open if existing channel is closed', async () => {
      const closedChannel: ChannelMetadata = {
        channelId: '0xclosed',
        peerId: 'peer-b',
        tokenId: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
        tokenAddress: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
        chain: 'evm:base:8453',
        createdAt: new Date(),
        lastActivityAt: new Date(),
        status: 'closed',
      };
      mockChannelManager.getChannelForPeer.mockReturnValue(closedChannel);

      const res = await request(app).post('/admin/channels').send(validEvmRequest);

      expect(res.status).toBe(201);
    });
  });

  describe('POST /admin/channels — Error Handling', () => {
    it('should return 500 with sanitized message on SDK error', async () => {
      mockChannelManager.ensureChannelExists.mockRejectedValue(
        new Error('Contract revert: insufficient funds')
      );

      const res = await request(app).post('/admin/channels').send(validEvmRequest);

      expect(res.status).toBe(500);
      expect(res.body.message).toBe('Channel open failed');
      expect(res.body.message).not.toContain('Contract revert');
    });
  });

  // --- GET /admin/channels ---

  describe('GET /admin/channels (AC: 5)', () => {
    it('should return all channels', async () => {
      const channels: ChannelMetadata[] = [
        {
          channelId: '0xchannel1',
          peerId: 'peer-a',
          tokenId: 'M2M',
          tokenAddress: '0xtoken',
          chain: 'evm:base:8453',
          createdAt: new Date('2026-01-01'),
          lastActivityAt: new Date('2026-01-02'),
          status: 'open',
        },
        {
          channelId: '0xchannel2',
          peerId: 'peer-b',
          tokenId: 'M2M',
          tokenAddress: '0xtoken',
          chain: 'evm:base:8453',
          createdAt: new Date('2026-01-01'),
          lastActivityAt: new Date('2026-01-03'),
          status: 'closing',
        },
      ];
      mockChannelManager.getAllChannels.mockReturnValue(channels);

      const res = await request(app).get('/admin/channels');

      expect(res.status).toBe(200);
      expect(res.body).toHaveLength(2);
      expect(res.body[0].channelId).toBe('0xchannel1');
      expect(res.body[1].channelId).toBe('0xchannel2');
    });

    it('should filter by peerId', async () => {
      const channels: ChannelMetadata[] = [
        {
          channelId: '0xch1',
          peerId: 'peer-a',
          tokenId: 'M2M',
          tokenAddress: '0xt',
          chain: 'evm:base:8453',
          createdAt: new Date(),
          lastActivityAt: new Date(),
          status: 'open',
        },
        {
          channelId: '0xch2',
          peerId: 'peer-b',
          tokenId: 'M2M',
          tokenAddress: '0xt',
          chain: 'evm:base:8453',
          createdAt: new Date(),
          lastActivityAt: new Date(),
          status: 'open',
        },
      ];
      mockChannelManager.getAllChannels.mockReturnValue(channels);

      const res = await request(app).get('/admin/channels?peerId=peer-a');

      expect(res.status).toBe(200);
      expect(res.body).toHaveLength(1);
      expect(res.body[0].peerId).toBe('peer-a');
    });

    it('should filter by status', async () => {
      const channels: ChannelMetadata[] = [
        {
          channelId: '0xch1',
          peerId: 'peer-a',
          tokenId: 'M2M',
          tokenAddress: '0xt',
          chain: 'evm:base:8453',
          createdAt: new Date(),
          lastActivityAt: new Date(),
          status: 'open',
        },
        {
          channelId: '0xch2',
          peerId: 'peer-b',
          tokenId: 'M2M',
          tokenAddress: '0xt',
          chain: 'evm:base:8453',
          createdAt: new Date(),
          lastActivityAt: new Date(),
          status: 'closing',
        },
      ];
      mockChannelManager.getAllChannels.mockReturnValue(channels);

      const res = await request(app).get('/admin/channels?status=closing');

      expect(res.status).toBe(200);
      expect(res.body).toHaveLength(1);
      expect(res.body[0].status).toBe('closing');
    });

    it('should return empty array when no channels', async () => {
      mockChannelManager.getAllChannels.mockReturnValue([]);

      const res = await request(app).get('/admin/channels');

      expect(res.status).toBe(200);
      expect(res.body).toEqual([]);
    });
  });

  // --- GET /admin/channels/:channelId ---

  describe('GET /admin/channels/:channelId (AC: 6, 7)', () => {
    it('should return full state for known EVM channel', async () => {
      const metadata: ChannelMetadata = {
        channelId: '0xabc',
        peerId: 'peer-b',
        tokenId: 'M2M',
        tokenAddress: '0xtoken',
        chain: 'evm:base:8453',
        createdAt: new Date(),
        lastActivityAt: new Date(),
        status: 'open',
      };
      mockChannelManager.getChannelById.mockReturnValue(metadata);

      mockPaymentChannelSDK.getChannelState.mockResolvedValue({
        channelId: '0xabc',
        participants: ['0xSender', '0xReceiver'] as [string, string],
        myDeposit: BigInt('1000000'),
        theirDeposit: BigInt('0'),
        myNonce: 5,
        theirNonce: 3,
        myTransferred: BigInt('500000'),
        theirTransferred: BigInt('200000'),
        status: 'opened',
        settlementTimeout: 86400,
        openedAt: 1000000,
      });

      const res = await request(app).get('/admin/channels/0xabc');

      expect(res.status).toBe(200);
      expect(res.body.channelId).toBe('0xabc');
      expect(res.body.deposit).toBe('1000000');
      expect(res.body.theirDeposit).toBe('0');
      expect(res.body.transferred).toBe('500000');
      expect(res.body.theirTransferred).toBe('200000');
      expect(res.body.status).toBe('open');
      expect(res.body.nonce).toBe(5);
    });

    it('should return 404 for unknown channelId', async () => {
      mockChannelManager.getChannelById.mockReturnValue(null);

      const res = await request(app).get('/admin/channels/0xunknown');

      expect(res.status).toBe(404);
      expect(res.body.error).toBe('Not found');
    });

    it('should serialize BigInt fields as strings', async () => {
      const metadata: ChannelMetadata = {
        channelId: '0xbig',
        peerId: 'peer-b',
        tokenId: 'M2M',
        tokenAddress: '0xtoken',
        chain: 'evm:base:8453',
        createdAt: new Date(),
        lastActivityAt: new Date(),
        status: 'open',
      };
      mockChannelManager.getChannelById.mockReturnValue(metadata);

      mockPaymentChannelSDK.getChannelState.mockResolvedValue({
        channelId: '0xbig',
        participants: ['0xA', '0xB'] as [string, string],
        myDeposit: BigInt('99999999999999999999'),
        theirDeposit: BigInt('88888888888888888888'),
        myNonce: 1,
        theirNonce: 0,
        myTransferred: BigInt('0'),
        theirTransferred: BigInt('0'),
        status: 'opened',
        settlementTimeout: 86400,
        openedAt: 100,
      });

      const res = await request(app).get('/admin/channels/0xbig');

      expect(res.status).toBe(200);
      expect(typeof res.body.deposit).toBe('string');
      expect(res.body.deposit).toBe('99999999999999999999');
      expect(typeof res.body.theirDeposit).toBe('string');
    });

    it('should return 500 with sanitized message on SDK error', async () => {
      const metadata: ChannelMetadata = {
        channelId: '0xerror',
        peerId: 'peer-b',
        tokenId: 'M2M',
        tokenAddress: '0xtoken',
        chain: 'evm:base:8453',
        createdAt: new Date(),
        lastActivityAt: new Date(),
        status: 'open',
      };
      mockChannelManager.getChannelById.mockReturnValue(metadata);
      mockPaymentChannelSDK.getChannelState.mockRejectedValue(new Error('RPC connection timeout'));

      const res = await request(app).get('/admin/channels/0xerror');

      expect(res.status).toBe(500);
      expect(res.body.message).toBe('Failed to query channel state');
      expect(res.body.message).not.toContain('RPC');
    });
  });

  // --- Settlement disabled ---

  describe('Settlement Disabled (AC: 10)', () => {
    let appNoSettlement: Express;

    beforeEach(async () => {
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        // No channelManager — settlement disabled
      };

      appNoSettlement = express();
      appNoSettlement.use('/admin', await createAdminRouter(config));
    });

    it('POST /admin/channels should return 503 when channelManager is undefined', async () => {
      const res = await request(appNoSettlement).post('/admin/channels').send(validEvmRequest);

      expect(res.status).toBe(503);
      expect(res.body.message).toContain('Settlement infrastructure not enabled');
    });

    it('GET /admin/channels should return 503 when channelManager is undefined', async () => {
      const res = await request(appNoSettlement).get('/admin/channels');

      expect(res.status).toBe(503);
    });

    it('GET /admin/channels/:channelId should return 503 when channelManager is undefined', async () => {
      const res = await request(appNoSettlement).get('/admin/channels/0xabc');

      expect(res.status).toBe(503);
    });

    it('existing /admin/peers should still work when channel endpoints added', async () => {
      mockBTPClientManager.getPeerIds.mockReturnValue([]);
      mockBTPClientManager.getPeerStatus.mockReturnValue(new Map());
      mockRoutingTable.getAllRoutes.mockReturnValue([]);

      const res = await request(appNoSettlement).get('/admin/peers');

      expect(res.status).toBe(200);
    });

    it('existing /admin/routes should still work when channel endpoints added', async () => {
      mockRoutingTable.getAllRoutes.mockReturnValue([]);

      const res = await request(appNoSettlement).get('/admin/routes');

      expect(res.status).toBe(200);
    });
  });

  // --- Auth tests (Story 21.1) ---

  describe('Auth Tests (Story 21.1 AC: 11)', () => {
    let appWithAuth: Express;

    beforeEach(async () => {
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        apiKey: 'test-secret-key',
        channelManager: mockChannelManager,
        paymentChannelSDK: mockPaymentChannelSDK,
      };

      appWithAuth = express();
      appWithAuth.use('/admin', await createAdminRouter(config));
    });

    it('should return 401 for POST /admin/channels without API key', async () => {
      const res = await request(appWithAuth).post('/admin/channels').send(validEvmRequest);

      expect(res.status).toBe(401);
    });

    it('should return 401 for GET /admin/channels without API key', async () => {
      const res = await request(appWithAuth).get('/admin/channels');

      expect(res.status).toBe(401);
    });

    it('should return 401 for GET /admin/channels/:id without API key', async () => {
      const res = await request(appWithAuth).get('/admin/channels/0xabc');

      expect(res.status).toBe(401);
    });

    it('should allow access with valid API key in header', async () => {
      mockChannelManager.getAllChannels.mockReturnValue([]);

      const res = await request(appWithAuth)
        .get('/admin/channels')
        .set('X-API-Key', 'test-secret-key');

      expect(res.status).toBe(200);
    });

    it.skip('should allow access with valid API key in query param', async () => {
      mockChannelManager.getAllChannels.mockReturnValue([]);

      const res = await request(appWithAuth).get('/admin/channels?apiKey=test-secret-key');

      expect(res.status).toBe(200);
    });
  });
});

// ============================================================
// Story 21.2 — Channel Lifecycle Endpoints (Deposit and Close)
// ============================================================

describe('Admin API Channel Lifecycle Endpoints (Story 21.2)', () => {
  let app: Express;
  let mockRoutingTable: jest.Mocked<RoutingTable>;
  let mockBTPClientManager: jest.Mocked<BTPClientManager>;
  let mockLogger: jest.Mocked<Logger>;
  let mockChannelManager: jest.Mocked<ChannelManager>;
  let mockPaymentChannelSDK: jest.Mocked<PaymentChannelSDK>;
  let settlementPeers: Map<string, SettlementPeerConfig>;

  const activeEvmChannel: ChannelMetadata = {
    channelId: '0xevm123',
    peerId: 'peer-b',
    tokenId: 'AGENT',
    tokenAddress: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
    chain: 'evm:base:8453',
    createdAt: new Date('2026-02-01'),
    lastActivityAt: new Date('2026-02-07'),
    status: 'open',
  };

  const defaultChannelState = {
    channelId: '0xevm123',
    participants: ['0xSender', '0xReceiver'] as [string, string],
    myDeposit: BigInt('1000000'),
    theirDeposit: BigInt('0'),
    myNonce: 5,
    theirNonce: 3,
    myTransferred: BigInt('500000'),
    theirTransferred: BigInt('200000'),
    status: 'opened' as const,
    settlementTimeout: 86400,
    openedAt: 1000000,
  };

  beforeEach(async () => {
    settlementPeers = new Map();

    mockRoutingTable = {
      addRoute: jest.fn(),
      removeRoute: jest.fn(),
      getAllRoutes: jest.fn().mockReturnValue([]),
      lookup: jest.fn(),
      removeRoutesForPeer: jest.fn(),
    } as unknown as jest.Mocked<RoutingTable>;

    mockBTPClientManager = {
      addPeer: jest.fn().mockResolvedValue(undefined),
      removePeer: jest.fn().mockResolvedValue(undefined),
      getPeerIds: jest.fn().mockReturnValue([]),
      getPeerStatus: jest.fn().mockReturnValue(new Map()),
      isConnected: jest.fn().mockReturnValue(false),
      getConnectedPeers: jest.fn().mockReturnValue([]),
      getClientForPeer: jest.fn(),
    } as unknown as jest.Mocked<BTPClientManager>;

    // eslint-disable-next-line @typescript-eslint/no-explicit-any
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

    mockChannelManager = {
      ensureChannelExists: jest.fn().mockResolvedValue('0xchannel123'),
      getAllChannels: jest.fn().mockReturnValue([]),
      getChannelById: jest.fn().mockReturnValue(null),
      getChannelForPeer: jest.fn().mockReturnValue(null),
      markChannelActivity: jest.fn(),
      start: jest.fn(),
      stop: jest.fn(),
      on: jest.fn(),
      emit: jest.fn(),
    } as unknown as jest.Mocked<ChannelManager>;

    mockPaymentChannelSDK = {
      openChannel: jest.fn(),
      getChannelState: jest.fn().mockResolvedValue(defaultChannelState),
      getMyChannels: jest.fn(),
      signBalanceProof: jest.fn().mockResolvedValue('0x' + 'ab'.repeat(65)),
      closeChannel: jest.fn().mockResolvedValue(undefined),
      cooperativeSettle: jest.fn().mockResolvedValue(undefined),
      settleChannel: jest.fn(),
      deposit: jest.fn().mockResolvedValue(undefined),
      removeAllListeners: jest.fn(),
    } as unknown as jest.Mocked<PaymentChannelSDK>;

    const config: AdminAPIConfig = {
      routingTable: mockRoutingTable,
      btpClientManager: mockBTPClientManager,
      logger: mockLogger,
      nodeId: 'test-node',
      settlementPeers,
      channelManager: mockChannelManager,
      paymentChannelSDK: mockPaymentChannelSDK,
    };

    app = express();
    app.use('/admin', await createAdminRouter(config));
  });

  afterEach(() => {
    jest.clearAllMocks();
  });

  // --- validateDepositRequest unit tests ---

  describe('validateDepositRequest', () => {
    it('should accept a valid deposit request', () => {
      const result = validateDepositRequest({ amount: '500000' });
      expect(result.valid).toBe(true);
    });

    it('should reject missing amount', () => {
      const result = validateDepositRequest({});
      expect(result.valid).toBe(false);
      expect(result.error).toContain('Missing amount');
    });

    it('should reject non-string amount', () => {
      const result = validateDepositRequest({ amount: 500 });
      expect(result.valid).toBe(false);
      expect(result.error).toContain('must be a string');
    });

    it('should reject non-numeric amount', () => {
      const result = validateDepositRequest({ amount: 'abc' });
      expect(result.valid).toBe(false);
      expect(result.error).toContain('positive integer string');
    });

    it('should reject zero amount', () => {
      const result = validateDepositRequest({ amount: '0' });
      expect(result.valid).toBe(false);
      expect(result.error).toContain('greater than zero');
    });

    it('should reject negative amount', () => {
      const result = validateDepositRequest({ amount: '-100' });
      expect(result.valid).toBe(false);
      expect(result.error).toContain('positive integer string');
    });
  });

  // --- POST /admin/channels/:channelId/deposit ---

  describe('POST /admin/channels/:channelId/deposit — Deposit Validation (AC: 3, 4, 5)', () => {
    it('should return 400 for missing amount', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });

      const res = await request(app).post('/admin/channels/0xevm123/deposit').send({});

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('Missing amount');
    });

    it('should return 400 for non-numeric amount', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });

      const res = await request(app)
        .post('/admin/channels/0xevm123/deposit')
        .send({ amount: 'abc' });

      expect(res.status).toBe(400);
    });

    it('should return 400 for zero amount', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });

      const res = await request(app).post('/admin/channels/0xevm123/deposit').send({ amount: '0' });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('greater than zero');
    });

    it('should return 400 for negative amount', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });

      const res = await request(app)
        .post('/admin/channels/0xevm123/deposit')
        .send({ amount: '-100' });

      expect(res.status).toBe(400);
    });

    it('should return 404 for unknown channelId', async () => {
      mockChannelManager.getChannelById.mockReturnValue(null);

      const res = await request(app)
        .post('/admin/channels/0xunknown/deposit')
        .send({ amount: '500000' });

      expect(res.status).toBe(404);
      expect(res.body.error).toBe('Not found');
    });

    it('should return 400 for channel in opening state', async () => {
      mockChannelManager.getChannelById.mockReturnValue({
        ...activeEvmChannel,
        status: 'opening',
      });

      const res = await request(app)
        .post('/admin/channels/0xevm123/deposit')
        .send({ amount: '500000' });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('not in open state');
    });

    it('should return 400 for channel in closing state', async () => {
      mockChannelManager.getChannelById.mockReturnValue({
        ...activeEvmChannel,
        status: 'closing',
      });

      const res = await request(app)
        .post('/admin/channels/0xevm123/deposit')
        .send({ amount: '500000' });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('not in open state');
    });

    it('should return 400 for channel in closed state', async () => {
      mockChannelManager.getChannelById.mockReturnValue({
        ...activeEvmChannel,
        status: 'closed',
      });

      const res = await request(app)
        .post('/admin/channels/0xevm123/deposit')
        .send({ amount: '500000' });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('not in open state');
    });

    it('should return 503 when settlement disabled (no channelManager)', async () => {
      const appNoSettlement = express();
      appNoSettlement.use(
        '/admin',
        await createAdminRouter({
          routingTable: mockRoutingTable,
          btpClientManager: mockBTPClientManager,
          logger: mockLogger,
          nodeId: 'test-node',
        })
      );

      const res = await request(appNoSettlement)
        .post('/admin/channels/0xevm123/deposit')
        .send({ amount: '500000' });

      expect(res.status).toBe(503);
      expect(res.body.message).toContain('Settlement infrastructure not enabled');
    });
  });

  describe('POST /admin/channels/:channelId/deposit — EVM Routing (AC: 1)', () => {
    it('should call paymentChannelSDK.deposit() with correct args', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });
      mockPaymentChannelSDK.getChannelState.mockResolvedValue({
        ...defaultChannelState,
        myDeposit: BigInt('1500000'),
      });

      const res = await request(app)
        .post('/admin/channels/0xevm123/deposit')
        .send({ amount: '500000' });

      expect(res.status).toBe(200);
      expect(mockPaymentChannelSDK.deposit).toHaveBeenCalledWith(
        '0xevm123',
        '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
        BigInt('500000')
      );
    });

    it('should return newDeposit from getChannelState after deposit', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });
      mockPaymentChannelSDK.getChannelState.mockResolvedValue({
        ...defaultChannelState,
        myDeposit: BigInt('1500000'),
      });

      const res = await request(app)
        .post('/admin/channels/0xevm123/deposit')
        .send({ amount: '500000' });

      expect(res.status).toBe(200);
      expect(res.body.channelId).toBe('0xevm123');
      expect(res.body.newDeposit).toBe('1500000');
      expect(res.body.status).toBe('open');
    });

    it('should return 500 with sanitized message on SDK deposit error', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });
      mockPaymentChannelSDK.deposit.mockRejectedValue(
        new Error('Contract revert: insufficient allowance')
      );

      const res = await request(app)
        .post('/admin/channels/0xevm123/deposit')
        .send({ amount: '500000' });

      expect(res.status).toBe(500);
      expect(res.body.message).toBe('Deposit failed');
      expect(res.body.message).not.toContain('Contract revert');
    });
  });

  // --- POST /admin/channels/:channelId/close ---

  describe('POST /admin/channels/:channelId/close — Close Mode Selection (AC: 6, 7, 8, 9)', () => {
    it('should attempt cooperative close by default (no body)', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });

      const res = await request(app).post('/admin/channels/0xevm123/close').send({});

      expect(res.status).toBe(200);
      expect(mockPaymentChannelSDK.cooperativeSettle).toHaveBeenCalled();
      expect(res.body.status).toBe('settled');
    });

    it('should attempt cooperative close when cooperative: true', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });

      const res = await request(app)
        .post('/admin/channels/0xevm123/close')
        .send({ cooperative: true });

      expect(res.status).toBe(200);
      expect(mockPaymentChannelSDK.cooperativeSettle).toHaveBeenCalled();
      expect(res.body.status).toBe('settled');
    });

    it('should skip cooperative and use unilateral when cooperative: false', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });

      const res = await request(app)
        .post('/admin/channels/0xevm123/close')
        .send({ cooperative: false });

      expect(res.status).toBe(200);
      expect(mockPaymentChannelSDK.cooperativeSettle).not.toHaveBeenCalled();
      expect(mockPaymentChannelSDK.signBalanceProof).toHaveBeenCalled();
      expect(mockPaymentChannelSDK.closeChannel).toHaveBeenCalled();
      expect(res.body.status).toBe('closing');
    });

    it('should return status "settled" on cooperative close success', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });

      const res = await request(app).post('/admin/channels/0xevm123/close').send({});

      expect(res.status).toBe(200);
      expect(res.body.channelId).toBe('0xevm123');
      expect(res.body.status).toBe('settled');
    });

    it('should fall back to unilateral close when cooperative fails', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });
      mockPaymentChannelSDK.cooperativeSettle.mockRejectedValue(new Error('Invalid signatures'));

      const res = await request(app)
        .post('/admin/channels/0xevm123/close')
        .send({ cooperative: true });

      expect(res.status).toBe(200);
      expect(mockPaymentChannelSDK.cooperativeSettle).toHaveBeenCalled();
      expect(mockPaymentChannelSDK.signBalanceProof).toHaveBeenCalled();
      expect(mockPaymentChannelSDK.closeChannel).toHaveBeenCalled();
      expect(res.body.status).toBe('closing');
    });

    it('should call signBalanceProof then closeChannel for unilateral close', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });

      const res = await request(app)
        .post('/admin/channels/0xevm123/close')
        .send({ cooperative: false });

      expect(res.status).toBe(200);
      expect(mockPaymentChannelSDK.signBalanceProof).toHaveBeenCalledWith(
        '0xevm123',
        defaultChannelState.myNonce + 1,
        defaultChannelState.myTransferred,
        0n,
        '0x' + '0'.repeat(64)
      );
      expect(mockPaymentChannelSDK.closeChannel).toHaveBeenCalledWith(
        '0xevm123',
        '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
        expect.objectContaining({
          channelId: '0xevm123',
          nonce: defaultChannelState.myNonce + 1,
        }),
        '0x' + 'ab'.repeat(65)
      );
    });

    it('should return 404 for unknown channelId', async () => {
      mockChannelManager.getChannelById.mockReturnValue(null);

      const res = await request(app).post('/admin/channels/0xunknown/close').send({});

      expect(res.status).toBe(404);
      expect(res.body.error).toBe('Not found');
    });

    it('should return 400 for channel in closed state', async () => {
      mockChannelManager.getChannelById.mockReturnValue({
        ...activeEvmChannel,
        status: 'closed',
      });

      const res = await request(app).post('/admin/channels/0xevm123/close').send({});

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('not in a closeable state');
    });

    it('should return 400 for channel in closing state', async () => {
      mockChannelManager.getChannelById.mockReturnValue({
        ...activeEvmChannel,
        status: 'closing',
      });

      const res = await request(app).post('/admin/channels/0xevm123/close').send({});

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('not in a closeable state');
    });

    it('should return 400 for channel in settling state', async () => {
      mockChannelManager.getChannelById.mockReturnValue({
        ...activeEvmChannel,
        status: 'settling',
      });

      const res = await request(app).post('/admin/channels/0xevm123/close').send({});

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('not in a closeable state');
    });

    it('should allow close for channel in opening state', async () => {
      mockChannelManager.getChannelById.mockReturnValue({
        ...activeEvmChannel,
        status: 'opening',
      });

      const res = await request(app).post('/admin/channels/0xevm123/close').send({});

      expect(res.status).toBe(200);
    });

    it('should return 503 when settlement disabled', async () => {
      const appNoSettlement = express();
      appNoSettlement.use(
        '/admin',
        await createAdminRouter({
          routingTable: mockRoutingTable,
          btpClientManager: mockBTPClientManager,
          logger: mockLogger,
          nodeId: 'test-node',
        })
      );

      const res = await request(appNoSettlement).post('/admin/channels/0xevm123/close').send({});

      expect(res.status).toBe(503);
    });

    it('should return 500 with sanitized message on close error', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });
      mockPaymentChannelSDK.cooperativeSettle.mockRejectedValue(new Error('Coop fail'));
      mockPaymentChannelSDK.closeChannel.mockRejectedValue(new Error('RPC error: gas too low'));

      const res = await request(app).post('/admin/channels/0xevm123/close').send({});

      expect(res.status).toBe(500);
      expect(res.body.message).toBe('Channel close failed');
      expect(res.body.message).not.toContain('RPC');
    });
  });

  // --- Auth tests for Story 21.2 endpoints ---

  describe('Auth Tests (Story 21.2)', () => {
    let appWithAuth: Express;

    beforeEach(async () => {
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        apiKey: 'test-secret-key',
        channelManager: mockChannelManager,
        paymentChannelSDK: mockPaymentChannelSDK,
      };

      appWithAuth = express();
      appWithAuth.use('/admin', await createAdminRouter(config));
    });

    it('should return 401 for deposit endpoint without API key', async () => {
      const res = await request(appWithAuth)
        .post('/admin/channels/0xevm123/deposit')
        .send({ amount: '500000' });

      expect(res.status).toBe(401);
    });

    it('should return 401 for close endpoint without API key', async () => {
      const res = await request(appWithAuth).post('/admin/channels/0xevm123/close').send({});

      expect(res.status).toBe(401);
    });

    it('should allow deposit with valid API key', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });
      mockPaymentChannelSDK.getChannelState.mockResolvedValue({
        ...defaultChannelState,
        myDeposit: BigInt('1500000'),
      });

      const res = await request(appWithAuth)
        .post('/admin/channels/0xevm123/deposit')
        .set('X-API-Key', 'test-secret-key')
        .send({ amount: '500000' });

      expect(res.status).toBe(200);
    });

    it('should allow close with valid API key', async () => {
      mockChannelManager.getChannelById.mockReturnValue({ ...activeEvmChannel });

      const res = await request(appWithAuth)
        .post('/admin/channels/0xevm123/close')
        .set('X-API-Key', 'test-secret-key')
        .send({});

      expect(res.status).toBe(200);
    });
  });
});

// ============================================================
// Story 21.3 — Balance and Settlement State Query Endpoints
// ============================================================

describe('Admin API Balance and Settlement State Endpoints (Story 21.3)', () => {
  let app: Express;
  let mockRoutingTable: jest.Mocked<RoutingTable>;
  let mockBTPClientManager: jest.Mocked<BTPClientManager>;
  let mockLogger: jest.Mocked<Logger>;
  let mockChannelManager: jest.Mocked<ChannelManager>;
  let mockAccountManager: jest.Mocked<AccountManager>;
  let mockSettlementMonitor: jest.Mocked<SettlementMonitor>;
  let mockClaimReceiver: jest.Mocked<ClaimReceiver>;

  beforeEach(async () => {
    mockRoutingTable = {
      addRoute: jest.fn(),
      removeRoute: jest.fn(),
      getAllRoutes: jest.fn().mockReturnValue([]),
      lookup: jest.fn(),
      removeRoutesForPeer: jest.fn(),
    } as unknown as jest.Mocked<RoutingTable>;

    mockBTPClientManager = {
      addPeer: jest.fn().mockResolvedValue(undefined),
      removePeer: jest.fn().mockResolvedValue(undefined),
      getPeerIds: jest.fn().mockReturnValue([]),
      getPeerStatus: jest.fn().mockReturnValue(new Map()),
      isConnected: jest.fn().mockReturnValue(false),
      getConnectedPeers: jest.fn().mockReturnValue([]),
      getClientForPeer: jest.fn(),
    } as unknown as jest.Mocked<BTPClientManager>;

    // eslint-disable-next-line @typescript-eslint/no-explicit-any
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

    mockChannelManager = {
      ensureChannelExists: jest.fn().mockResolvedValue('0xchannel123'),
      getAllChannels: jest.fn().mockReturnValue([]),
      getChannelById: jest.fn().mockReturnValue(null),
      getChannelForPeer: jest.fn().mockReturnValue(null),
      markChannelActivity: jest.fn(),
      start: jest.fn(),
      stop: jest.fn(),
      on: jest.fn(),
      emit: jest.fn(),
    } as unknown as jest.Mocked<ChannelManager>;

    mockAccountManager = {
      getAccountBalance: jest.fn().mockResolvedValue({
        debitBalance: 5000n,
        creditBalance: 3000n,
        netBalance: -2000n,
      }),
      checkCreditLimit: jest.fn(),
      wouldExceedCreditLimit: jest.fn(),
      createPeerAccounts: jest.fn(),
      recordSettlement: jest.fn(),
      recordPacketSettlement: jest.fn(),
      recordPacketTransfers: jest.fn(),
      setEventStore: jest.fn(),
      setEventBroadcaster: jest.fn(),
    } as unknown as jest.Mocked<AccountManager>;

    mockSettlementMonitor = {
      getAllSettlementStates: jest.fn().mockReturnValue(new Map()),
      getSettlementState: jest.fn().mockReturnValue(SettlementState.IDLE),
      start: jest.fn(),
      stop: jest.fn(),
      on: jest.fn(),
      emit: jest.fn(),
    } as unknown as jest.Mocked<SettlementMonitor>;

    mockClaimReceiver = {
      getLatestVerifiedClaim: jest.fn().mockResolvedValue(null),
    } as unknown as jest.Mocked<ClaimReceiver>;

    const config: AdminAPIConfig = {
      routingTable: mockRoutingTable,
      btpClientManager: mockBTPClientManager,
      logger: mockLogger,
      nodeId: 'test-node',
      channelManager: mockChannelManager,
      accountManager: mockAccountManager,
      settlementMonitor: mockSettlementMonitor,
      claimReceiver: mockClaimReceiver,
    };

    app = express();
    app.use('/admin', await createAdminRouter(config));
  });

  afterEach(() => {
    jest.clearAllMocks();
  });

  // --- GET /admin/balances/:peerId ---

  describe('GET /admin/balances/:peerId (AC: 1, 2, 3, 4)', () => {
    it('should return 200 with balance response', async () => {
      const res = await request(app).get('/admin/balances/peer-b');

      expect(res.status).toBe(200);
      expect(res.body.peerId).toBe('peer-b');
      expect(res.body.balances).toHaveLength(1);
      expect(res.body.balances[0].tokenId).toBe('ILP');
      expect(res.body.balances[0].debitBalance).toBe('5000');
      expect(res.body.balances[0].creditBalance).toBe('3000');
      expect(res.body.balances[0].netBalance).toBe('-2000');
    });

    it('should use specified tokenId from query parameter', async () => {
      const res = await request(app).get('/admin/balances/peer-b?tokenId=USDC');

      expect(res.status).toBe(200);
      expect(res.body.balances[0].tokenId).toBe('USDC');
      expect(mockAccountManager.getAccountBalance).toHaveBeenCalledWith('peer-b', 'USDC');
    });

    it('should default tokenId to ILP when not provided', async () => {
      await request(app).get('/admin/balances/peer-b');

      expect(mockAccountManager.getAccountBalance).toHaveBeenCalledWith('peer-b', 'ILP');
    });

    it('should return 200 with zero balances for unknown peer (TigerBeetle semantics)', async () => {
      mockAccountManager.getAccountBalance.mockResolvedValue({
        debitBalance: 0n,
        creditBalance: 0n,
        netBalance: 0n,
      });

      const res = await request(app).get('/admin/balances/unknown-peer');

      expect(res.status).toBe(200);
      expect(res.body.peerId).toBe('unknown-peer');
      expect(res.body.balances[0].debitBalance).toBe('0');
      expect(res.body.balances[0].creditBalance).toBe('0');
      expect(res.body.balances[0].netBalance).toBe('0');
    });

    it('should serialize BigInt values as strings', async () => {
      mockAccountManager.getAccountBalance.mockResolvedValue({
        debitBalance: 99999999999999999999n,
        creditBalance: 88888888888888888888n,
        netBalance: -11111111111111111111n,
      });

      const res = await request(app).get('/admin/balances/peer-big');

      expect(res.status).toBe(200);
      expect(typeof res.body.balances[0].debitBalance).toBe('string');
      expect(res.body.balances[0].debitBalance).toBe('99999999999999999999');
      expect(res.body.balances[0].creditBalance).toBe('88888888888888888888');
      expect(res.body.balances[0].netBalance).toBe('-11111111111111111111');
    });

    it('should return 503 when accountManager is unavailable', async () => {
      const appNoAccounts = express();
      appNoAccounts.use(
        '/admin',
        await createAdminRouter({
          routingTable: mockRoutingTable,
          btpClientManager: mockBTPClientManager,
          logger: mockLogger,
          nodeId: 'test-node',
        })
      );

      const res = await request(appNoAccounts).get('/admin/balances/peer-b');

      expect(res.status).toBe(503);
      expect(res.body.message).toContain('Account management not enabled');
    });

    it('should return 500 when accountManager throws error', async () => {
      mockAccountManager.getAccountBalance.mockRejectedValue(
        new Error('TigerBeetle connection timeout')
      );

      const res = await request(app).get('/admin/balances/peer-b');

      expect(res.status).toBe(500);
      expect(res.body.message).toBe('Balance query failed');
      expect(res.body.message).not.toContain('TigerBeetle');
    });
  });

  describe('GET /admin/balances/:peerId — Auth', () => {
    let appWithAuth: Express;

    beforeEach(async () => {
      appWithAuth = express();
      appWithAuth.use(
        '/admin',
        await createAdminRouter({
          routingTable: mockRoutingTable,
          btpClientManager: mockBTPClientManager,
          logger: mockLogger,
          nodeId: 'test-node',
          apiKey: 'test-secret-key',
          accountManager: mockAccountManager,
        })
      );
    });

    it('should return 401 without API key', async () => {
      const res = await request(appWithAuth).get('/admin/balances/peer-b');

      expect(res.status).toBe(401);
    });

    it('should allow access with valid API key', async () => {
      const res = await request(appWithAuth)
        .get('/admin/balances/peer-b')
        .set('X-API-Key', 'test-secret-key');

      expect(res.status).toBe(200);
    });
  });

  // --- GET /admin/settlement/states ---

  describe('GET /admin/settlement/states (AC: 5, 6)', () => {
    it('should return 200 with state array', async () => {
      const statesMap = new Map<string, SettlementState>();
      statesMap.set('peer-b:ILP', SettlementState.IDLE);
      statesMap.set('peer-c:ILP', SettlementState.SETTLEMENT_PENDING);
      mockSettlementMonitor.getAllSettlementStates.mockReturnValue(statesMap);

      const res = await request(app).get('/admin/settlement/states');

      expect(res.status).toBe(200);
      expect(res.body).toHaveLength(2);
      expect(res.body[0]).toEqual({ peerId: 'peer-b', tokenId: 'ILP', state: 'IDLE' });
      expect(res.body[1]).toEqual({
        peerId: 'peer-c',
        tokenId: 'ILP',
        state: 'SETTLEMENT_PENDING',
      });
    });

    it('should return 200 with empty array when no states', async () => {
      mockSettlementMonitor.getAllSettlementStates.mockReturnValue(new Map());

      const res = await request(app).get('/admin/settlement/states');

      expect(res.status).toBe(200);
      expect(res.body).toEqual([]);
    });

    it('should correctly map all settlement state values', async () => {
      const statesMap = new Map<string, SettlementState>();
      statesMap.set('peer-a:ILP', SettlementState.IDLE);
      statesMap.set('peer-b:ILP', SettlementState.SETTLEMENT_PENDING);
      statesMap.set('peer-c:ILP', SettlementState.SETTLEMENT_IN_PROGRESS);
      mockSettlementMonitor.getAllSettlementStates.mockReturnValue(statesMap);

      const res = await request(app).get('/admin/settlement/states');

      expect(res.status).toBe(200);
      expect(res.body).toHaveLength(3);
      expect(res.body[0].state).toBe('IDLE');
      expect(res.body[1].state).toBe('SETTLEMENT_PENDING');
      expect(res.body[2].state).toBe('SETTLEMENT_IN_PROGRESS');
    });

    it('should correctly parse state key to peerId and tokenId', async () => {
      const statesMap = new Map<string, SettlementState>();
      statesMap.set('peer-b:ILP', SettlementState.IDLE);
      mockSettlementMonitor.getAllSettlementStates.mockReturnValue(statesMap);

      const res = await request(app).get('/admin/settlement/states');

      expect(res.status).toBe(200);
      expect(res.body[0].peerId).toBe('peer-b');
      expect(res.body[0].tokenId).toBe('ILP');
    });

    it('should return 503 when settlementMonitor is unavailable', async () => {
      const appNoMonitor = express();
      appNoMonitor.use(
        '/admin',
        await createAdminRouter({
          routingTable: mockRoutingTable,
          btpClientManager: mockBTPClientManager,
          logger: mockLogger,
          nodeId: 'test-node',
        })
      );

      const res = await request(appNoMonitor).get('/admin/settlement/states');

      expect(res.status).toBe(503);
      expect(res.body.message).toContain('Settlement monitoring not enabled');
    });
  });

  describe('GET /admin/settlement/states — Auth', () => {
    let appWithAuth: Express;

    beforeEach(async () => {
      appWithAuth = express();
      appWithAuth.use(
        '/admin',
        await createAdminRouter({
          routingTable: mockRoutingTable,
          btpClientManager: mockBTPClientManager,
          logger: mockLogger,
          nodeId: 'test-node',
          apiKey: 'test-secret-key',
          settlementMonitor: mockSettlementMonitor,
        })
      );
    });

    it('should return 401 without API key', async () => {
      const res = await request(appWithAuth).get('/admin/settlement/states');

      expect(res.status).toBe(401);
    });

    it('should allow access with valid API key', async () => {
      mockSettlementMonitor.getAllSettlementStates.mockReturnValue(new Map());

      const res = await request(appWithAuth)
        .get('/admin/settlement/states')
        .set('X-API-Key', 'test-secret-key');

      expect(res.status).toBe(200);
    });
  });

  // --- GET /admin/channels/:channelId/claims ---

  describe('GET /admin/channels/:channelId/claims (AC: 7, 8)', () => {
    const evmChannel: ChannelMetadata = {
      channelId: '0xevm123',
      peerId: 'peer-b',
      tokenId: 'AGENT',
      tokenAddress: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
      chain: 'evm:base:8453',
      createdAt: new Date('2026-02-01'),
      lastActivityAt: new Date('2026-02-07'),
      status: 'open',
    };

    const evmClaim = {
      version: '1.0' as const,
      blockchain: 'evm' as const,
      messageId: 'claim-002',
      timestamp: '2026-02-08T12:00:00.000Z',
      senderId: 'peer-b',
      channelId: '0xevm123',
      nonce: 5,
      transferredAmount: '1000000',
      lockedAmount: '0',
      locksRoot: '0x' + '0'.repeat(64),
      signature: '0xabcdef',
      signerAddress: '0x742d35Cc',
    };

    it('should return 200 with EVM claim fields', async () => {
      mockChannelManager.getChannelById.mockReturnValue(evmChannel);
      mockClaimReceiver.getLatestVerifiedClaim.mockResolvedValue(evmClaim);

      const res = await request(app).get('/admin/channels/0xevm123/claims');

      expect(res.status).toBe(200);
      expect(res.body.blockchain).toBe('evm');
      expect(res.body.channelId).toBe('0xevm123');
      expect(res.body.nonce).toBe(5);
      expect(res.body.transferredAmount).toBe('1000000');
      expect(res.body.signerAddress).toBe('0x742d35Cc');
    });

    it('should call claimReceiver with correct blockchain type from channel chain', async () => {
      mockChannelManager.getChannelById.mockReturnValue(evmChannel);
      mockClaimReceiver.getLatestVerifiedClaim.mockResolvedValue(evmClaim);

      await request(app).get('/admin/channels/0xevm123/claims');

      expect(mockClaimReceiver.getLatestVerifiedClaim).toHaveBeenCalledWith(
        'peer-b',
        'evm',
        '0xevm123'
      );
    });

    it('should return 404 for unknown channelId', async () => {
      mockChannelManager.getChannelById.mockReturnValue(null);

      const res = await request(app).get('/admin/channels/0xunknown/claims');

      expect(res.status).toBe(404);
      expect(res.body.message).toBe('Channel not found');
    });

    it('should return 404 when no claims found (claimReceiver returns null)', async () => {
      mockChannelManager.getChannelById.mockReturnValue(evmChannel);
      mockClaimReceiver.getLatestVerifiedClaim.mockResolvedValue(null);

      const res = await request(app).get('/admin/channels/0xevm123/claims');

      expect(res.status).toBe(404);
      expect(res.body.message).toBe('No claims found for this channel');
    });

    it('should return 503 when channelManager is unavailable', async () => {
      const appNoChannel = express();
      appNoChannel.use(
        '/admin',
        await createAdminRouter({
          routingTable: mockRoutingTable,
          btpClientManager: mockBTPClientManager,
          logger: mockLogger,
          nodeId: 'test-node',
          claimReceiver: mockClaimReceiver,
        })
      );

      const res = await request(appNoChannel).get('/admin/channels/0xevm123/claims');

      expect(res.status).toBe(503);
      expect(res.body.message).toContain('Settlement infrastructure not enabled');
    });

    it('should return 503 when claimReceiver is unavailable', async () => {
      const appNoClaims = express();
      appNoClaims.use(
        '/admin',
        await createAdminRouter({
          routingTable: mockRoutingTable,
          btpClientManager: mockBTPClientManager,
          logger: mockLogger,
          nodeId: 'test-node',
          channelManager: mockChannelManager,
        })
      );
      mockChannelManager.getChannelById.mockReturnValue(evmChannel);

      const res = await request(appNoClaims).get('/admin/channels/0xevm123/claims');

      expect(res.status).toBe(503);
      expect(res.body.message).toContain('Claim receiver not enabled');
    });

    it('should return 500 when claimReceiver throws error', async () => {
      mockChannelManager.getChannelById.mockReturnValue(evmChannel);
      mockClaimReceiver.getLatestVerifiedClaim.mockRejectedValue(
        new Error('SQLite database locked')
      );

      const res = await request(app).get('/admin/channels/0xevm123/claims');

      expect(res.status).toBe(500);
      expect(res.body.message).toBe('Claim query failed');
      expect(res.body.message).not.toContain('SQLite');
    });
  });

  describe('GET /admin/channels/:channelId/claims — Auth', () => {
    let appWithAuth: Express;

    beforeEach(async () => {
      appWithAuth = express();
      appWithAuth.use(
        '/admin',
        await createAdminRouter({
          routingTable: mockRoutingTable,
          btpClientManager: mockBTPClientManager,
          logger: mockLogger,
          nodeId: 'test-node',
          apiKey: 'test-secret-key',
          channelManager: mockChannelManager,
          claimReceiver: mockClaimReceiver,
        })
      );
    });

    it('should return 401 without API key', async () => {
      const res = await request(appWithAuth).get('/admin/channels/0xevm123/claims');

      expect(res.status).toBe(401);
    });

    it('should allow access with valid API key', async () => {
      mockChannelManager.getChannelById.mockReturnValue(null);

      const res = await request(appWithAuth)
        .get('/admin/channels/0xevm123/claims')
        .set('X-API-Key', 'test-secret-key');

      expect(res.status).toBe(404);
    });
  });
});

// ============================================================
// Story 21.4 — Channel Opening Integration Fixes
// ============================================================

describe('Admin API Channel Opening Integration Fixes (Story 21.4)', () => {
  let app: Express;
  let mockRoutingTable: jest.Mocked<RoutingTable>;
  let mockBTPClientManager: jest.Mocked<BTPClientManager>;
  let mockLogger: jest.Mocked<Logger>;
  let mockChannelManager: jest.Mocked<ChannelManager>;
  let mockPaymentChannelSDK: jest.Mocked<PaymentChannelSDK>;
  let settlementPeers: Map<string, SettlementPeerConfig>;

  const validEvmAddress = '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28';

  beforeEach(async () => {
    settlementPeers = new Map();

    mockRoutingTable = {
      addRoute: jest.fn(),
      removeRoute: jest.fn(),
      getAllRoutes: jest.fn().mockReturnValue([]),
      lookup: jest.fn(),
      removeRoutesForPeer: jest.fn(),
    } as unknown as jest.Mocked<RoutingTable>;

    mockBTPClientManager = {
      addPeer: jest.fn().mockResolvedValue(undefined),
      removePeer: jest.fn().mockResolvedValue(undefined),
      getPeerIds: jest.fn().mockReturnValue(['peer-b', 'peer-c']),
      getPeerStatus: jest.fn().mockReturnValue(new Map()),
      isConnected: jest.fn().mockReturnValue(false),
      getConnectedPeers: jest.fn().mockReturnValue([]),
      getClientForPeer: jest.fn(),
    } as unknown as jest.Mocked<BTPClientManager>;

    // eslint-disable-next-line @typescript-eslint/no-explicit-any
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

    mockChannelManager = {
      ensureChannelExists: jest.fn().mockResolvedValue('0xchannel123'),
      getAllChannels: jest.fn().mockReturnValue([]),
      getChannelById: jest.fn().mockImplementation((channelId: string) => {
        if (channelId === '0xchannel123') {
          return {
            channelId: '0xchannel123',
            peerId: 'peer-b',
            tokenId: 'AGENT',
            tokenAddress: '0xtoken',
            chain: 'evm:base:8453',
            createdAt: new Date(),
            lastActivityAt: new Date(),
            status: 'open',
          };
        }
        return null;
      }),
      getChannelForPeer: jest.fn().mockReturnValue(null),
      markChannelActivity: jest.fn(),
      start: jest.fn(),
      stop: jest.fn(),
      on: jest.fn(),
      emit: jest.fn(),
    } as unknown as jest.Mocked<ChannelManager>;

    mockPaymentChannelSDK = {
      openChannel: jest.fn(),
      getChannelState: jest.fn(),
      getMyChannels: jest.fn(),
      signBalanceProof: jest.fn().mockResolvedValue('0x' + 'ab'.repeat(65)),
      closeChannel: jest.fn().mockResolvedValue(undefined),
      cooperativeSettle: jest.fn().mockResolvedValue(undefined),
      settleChannel: jest.fn(),
      deposit: jest.fn().mockResolvedValue(undefined),
      removeAllListeners: jest.fn(),
    } as unknown as jest.Mocked<PaymentChannelSDK>;

    const config: AdminAPIConfig = {
      routingTable: mockRoutingTable,
      btpClientManager: mockBTPClientManager,
      logger: mockLogger,
      nodeId: 'test-node',
      settlementPeers,
      channelManager: mockChannelManager,
      paymentChannelSDK: mockPaymentChannelSDK,
    };

    app = express();
    app.use('/admin', await createAdminRouter(config));
  });

  afterEach(() => {
    jest.clearAllMocks();
  });

  // --- peerAddress propagation (AC: 10) ---

  describe('peerAddress propagation (AC: 1, 10)', () => {
    it('should pass peerAddress from request body through to ensureChannelExists options', async () => {
      const res = await request(app).post('/admin/channels').send({
        peerId: 'peer-b',
        chain: 'evm:base:8453',
        initialDeposit: '1000000',
        peerAddress: validEvmAddress,
      });

      expect(res.status).toBe(201);
      expect(mockChannelManager.ensureChannelExists).toHaveBeenCalledWith(
        'peer-b',
        'AGENT',
        expect.objectContaining({
          peerAddress: validEvmAddress,
        })
      );
    });

    it('should fall back to settlementPeers when peerAddress not in request', async () => {
      settlementPeers.set('peer-b', {
        peerId: 'peer-b',
        address: 'g.peer-b',
        settlementPreference: 'evm',
        settlementTokens: ['EVM'],
        evmAddress: '0xFallbackAddress1234567890abcdef12345678',
      });

      const res = await request(app).post('/admin/channels').send({
        peerId: 'peer-b',
        chain: 'evm:base:8453',
        initialDeposit: '1000000',
      });

      expect(res.status).toBe(201);
      expect(mockChannelManager.ensureChannelExists).toHaveBeenCalledWith(
        'peer-b',
        'AGENT',
        expect.objectContaining({
          peerAddress: '0xFallbackAddress1234567890abcdef12345678',
        })
      );
    });

    it('should return 400 when neither peerAddress nor settlementPeers has EVM address', async () => {
      const res = await request(app).post('/admin/channels').send({
        peerId: 'peer-b',
        chain: 'evm:base:8453',
        initialDeposit: '1000000',
      });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('Peer EVM address must be provided');
    });
  });

  // --- peer existence validation (AC: 12) ---

  describe('peer existence validation (AC: 4, 5, 12)', () => {
    it('should return 404 for unknown peerId', async () => {
      const res = await request(app).post('/admin/channels').send({
        peerId: 'unknown-peer',
        chain: 'evm:base:8453',
        initialDeposit: '1000000',
        peerAddress: validEvmAddress,
      });

      expect(res.status).toBe(404);
      expect(res.body.error).toBe('Not found');
      expect(res.body.message).toContain("Peer 'unknown-peer' must be registered");
    });

    it('should proceed to channel opening for known peerId', async () => {
      const res = await request(app).post('/admin/channels').send({
        peerId: 'peer-b',
        chain: 'evm:base:8453',
        initialDeposit: '1000000',
        peerAddress: validEvmAddress,
      });

      expect(res.status).toBe(201);
      expect(mockChannelManager.ensureChannelExists).toHaveBeenCalled();
    });
  });

  // --- response status accuracy (AC: 13) ---

  describe('response status accuracy (AC: 6, 7, 8, 13)', () => {
    it('should return normalized metadata.status for EVM', async () => {
      const res = await request(app).post('/admin/channels').send({
        peerId: 'peer-b',
        chain: 'evm:base:8453',
        initialDeposit: '1000000',
        peerAddress: validEvmAddress,
      });

      expect(res.status).toBe(201);
      expect(res.body.status).toBe('open');
    });

    it('should return 500 when EVM metadata unavailable after creation', async () => {
      mockChannelManager.getChannelById.mockReturnValue(null);

      const res = await request(app).post('/admin/channels').send({
        peerId: 'peer-b',
        chain: 'evm:base:8453',
        initialDeposit: '1000000',
        peerAddress: validEvmAddress,
      });

      expect(res.status).toBe(500);
      expect(res.body.message).toContain('metadata unavailable');
    });
  });

  // --- peerAddress format validation (AC: 9) ---

  describe('peerAddress format validation (AC: 9)', () => {
    it('should return 400 for invalid EVM address format (too short)', async () => {
      const res = await request(app).post('/admin/channels').send({
        peerId: 'peer-b',
        chain: 'evm:base:8453',
        initialDeposit: '1000000',
        peerAddress: '0x1234',
      });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('Invalid EVM address format');
    });

    it('should return 400 for invalid EVM address format (no 0x prefix)', async () => {
      const res = await request(app).post('/admin/channels').send({
        peerId: 'peer-b',
        chain: 'evm:base:8453',
        initialDeposit: '1000000',
        peerAddress: '742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
      });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('Invalid EVM address format');
    });

    it('should return 400 for invalid EVM address format (non-hex chars)', async () => {
      const res = await request(app).post('/admin/channels').send({
        peerId: 'peer-b',
        chain: 'evm:base:8453',
        initialDeposit: '1000000',
        peerAddress: '0xGGGd35Cc6634C0532925a3b844Bc9e7595f2bD28',
      });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('Invalid EVM address format');
    });

    it('should accept valid EVM address format', async () => {
      const res = await request(app).post('/admin/channels').send({
        peerId: 'peer-b',
        chain: 'evm:base:8453',
        initialDeposit: '1000000',
        peerAddress: validEvmAddress,
      });

      expect(res.status).toBe(201);
    });
  });
});

describe('Admin API Channel Status Normalization (Story 21.5)', () => {
  let app: Express;
  let mockRoutingTable: jest.Mocked<RoutingTable>;
  let mockBTPClientManager: jest.Mocked<BTPClientManager>;
  let mockLogger: jest.Mocked<Logger>;
  let mockChannelManager: jest.Mocked<ChannelManager>;
  let mockPaymentChannelSDK: jest.Mocked<PaymentChannelSDK>;
  let settlementPeers: Map<string, SettlementPeerConfig>;

  beforeEach(async () => {
    settlementPeers = new Map();

    mockRoutingTable = {
      addRoute: jest.fn(),
      removeRoute: jest.fn(),
      getAllRoutes: jest.fn().mockReturnValue([]),
      lookup: jest.fn(),
      removeRoutesForPeer: jest.fn(),
    } as unknown as jest.Mocked<RoutingTable>;

    mockBTPClientManager = {
      addPeer: jest.fn().mockResolvedValue(undefined),
      removePeer: jest.fn().mockResolvedValue(undefined),
      getPeerIds: jest.fn().mockReturnValue(['peer-b']),
      getPeerStatus: jest.fn().mockReturnValue(new Map()),
      isConnected: jest.fn().mockReturnValue(false),
      getConnectedPeers: jest.fn().mockReturnValue([]),
      getClientForPeer: jest.fn(),
    } as unknown as jest.Mocked<BTPClientManager>;

    // eslint-disable-next-line @typescript-eslint/no-explicit-any
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

    mockChannelManager = {
      ensureChannelExists: jest.fn().mockResolvedValue('0xchannel123'),
      getAllChannels: jest.fn().mockReturnValue([]),
      getChannelById: jest.fn().mockReturnValue(null),
      getChannelForPeer: jest.fn().mockReturnValue(null),
      markChannelActivity: jest.fn(),
      start: jest.fn(),
      stop: jest.fn(),
      on: jest.fn(),
      emit: jest.fn(),
    } as unknown as jest.Mocked<ChannelManager>;

    mockPaymentChannelSDK = {
      openChannel: jest.fn(),
      getChannelState: jest.fn(),
      getMyChannels: jest.fn(),
      signBalanceProof: jest.fn().mockResolvedValue('0x' + 'ab'.repeat(65)),
      closeChannel: jest.fn().mockResolvedValue(undefined),
      cooperativeSettle: jest.fn().mockResolvedValue(undefined),
      settleChannel: jest.fn(),
      deposit: jest.fn().mockResolvedValue(undefined),
      removeAllListeners: jest.fn(),
    } as unknown as jest.Mocked<PaymentChannelSDK>;

    const config: AdminAPIConfig = {
      routingTable: mockRoutingTable,
      btpClientManager: mockBTPClientManager,
      logger: mockLogger,
      nodeId: 'test-node',
      settlementPeers,
      channelManager: mockChannelManager,
      paymentChannelSDK: mockPaymentChannelSDK,
    };

    app = express();
    app.use('/admin', await createAdminRouter(config));
  });

  afterEach(() => {
    jest.clearAllMocks();
  });

  describe('GET /admin/channels — status normalization in list response (AC: 7, 14)', () => {
    it('should normalize internal "active" status to "open" in list response', async () => {
      // Simulate legacy ChannelMetadata with 'active' status (untyped mock)
      mockChannelManager.getAllChannels.mockReturnValue([
        {
          channelId: '0xlegacy',
          peerId: 'peer-b',
          tokenId: 'AGENT',
          tokenAddress: '0xtoken',
          chain: 'evm:base:8453',
          createdAt: new Date(),
          lastActivityAt: new Date(),
          status: 'active',
        },
      ] as unknown as ChannelMetadata[]);

      const res = await request(app).get('/admin/channels');

      expect(res.status).toBe(200);
      expect(res.body).toHaveLength(1);
      expect(res.body[0].status).toBe('open');
    });

    it('should match ?status=open filter against channels with internal "active" status', async () => {
      mockChannelManager.getAllChannels.mockReturnValue([
        {
          channelId: '0xlegacy',
          peerId: 'peer-b',
          tokenId: 'AGENT',
          tokenAddress: '0xtoken',
          chain: 'evm:base:8453',
          createdAt: new Date(),
          lastActivityAt: new Date(),
          status: 'active',
        },
      ] as unknown as ChannelMetadata[]);

      const res = await request(app).get('/admin/channels?status=open');

      expect(res.status).toBe(200);
      expect(res.body).toHaveLength(1);
      expect(res.body[0].status).toBe('open');
    });

    it('should match ?status=active filter (alias) against channels with "open" status', async () => {
      mockChannelManager.getAllChannels.mockReturnValue([
        {
          channelId: '0xcanonical',
          peerId: 'peer-b',
          tokenId: 'AGENT',
          tokenAddress: '0xtoken',
          chain: 'evm:base:8453',
          createdAt: new Date(),
          lastActivityAt: new Date(),
          status: 'open',
        },
      ]);

      const res = await request(app).get('/admin/channels?status=active');

      expect(res.status).toBe(200);
      expect(res.body).toHaveLength(1);
      expect(res.body[0].status).toBe('open');
    });
  });

  describe('GET /admin/channels/:channelId — on-chain SDK normalization (AC: 6, 14)', () => {
    it('should normalize on-chain SDK "opened" status to "open" in detail response', async () => {
      mockChannelManager.getChannelById.mockReturnValue({
        channelId: '0xonchain',
        peerId: 'peer-b',
        tokenId: 'AGENT',
        tokenAddress: '0xtoken',
        chain: 'evm:base:8453',
        createdAt: new Date(),
        lastActivityAt: new Date(),
        status: 'open',
      });

      mockPaymentChannelSDK.getChannelState.mockResolvedValue({
        channelId: '0xonchain',
        participants: ['0xA', '0xB'] as [string, string],
        myDeposit: BigInt('1000000'),
        theirDeposit: BigInt('0'),
        myNonce: 1,
        theirNonce: 0,
        myTransferred: BigInt('0'),
        theirTransferred: BigInt('0'),
        status: 'opened',
        settlementTimeout: 86400,
        openedAt: 100,
      });

      const res = await request(app).get('/admin/channels/0xonchain');

      expect(res.status).toBe(200);
      expect(res.body.status).toBe('open');
    });

    it('should pass through canonical "closed" status from on-chain SDK', async () => {
      mockChannelManager.getChannelById.mockReturnValue({
        channelId: '0xclosed',
        peerId: 'peer-b',
        tokenId: 'AGENT',
        tokenAddress: '0xtoken',
        chain: 'evm:base:8453',
        createdAt: new Date(),
        lastActivityAt: new Date(),
        status: 'closed',
      });

      mockPaymentChannelSDK.getChannelState.mockResolvedValue({
        channelId: '0xclosed',
        participants: ['0xA', '0xB'] as [string, string],
        myDeposit: BigInt('0'),
        theirDeposit: BigInt('0'),
        myNonce: 0,
        theirNonce: 0,
        myTransferred: BigInt('0'),
        theirTransferred: BigInt('0'),
        status: 'closed',
        settlementTimeout: 86400,
        openedAt: 100,
      });

      const res = await request(app).get('/admin/channels/0xclosed');

      expect(res.status).toBe(200);
      expect(res.body.status).toBe('closed');
    });
  });

  describe('POST /admin/channels/:channelId/deposit — normalized status in response (AC: 9)', () => {
    it('should return normalized status from metadata instead of hardcoded "open"', async () => {
      mockChannelManager.getChannelById.mockReturnValue({
        channelId: '0xdeposit',
        peerId: 'peer-b',
        tokenId: 'AGENT',
        tokenAddress: '0xtoken',
        chain: 'evm:base:8453',
        createdAt: new Date(),
        lastActivityAt: new Date(),
        status: 'open',
      });

      mockPaymentChannelSDK.getChannelState.mockResolvedValue({
        channelId: '0xdeposit',
        participants: ['0xA', '0xB'] as [string, string],
        myDeposit: BigInt('2000000'),
        theirDeposit: BigInt('0'),
        myNonce: 1,
        theirNonce: 0,
        myTransferred: BigInt('0'),
        theirTransferred: BigInt('0'),
        status: 'opened',
        settlementTimeout: 86400,
        openedAt: 100,
      });

      const res = await request(app)
        .post('/admin/channels/0xdeposit/deposit')
        .send({ amount: '500000' });

      expect(res.status).toBe(200);
      expect(res.body.status).toBe('open');
    });
  });
});
