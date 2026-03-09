/**
 * Unit + Integration Tests for Admin API Settlement Extension (Story 20.3)
 *
 * Tests settlement validation, PeerConfig creation, GET response enhancement,
 * DELETE cleanup, and backward compatibility for the Admin API.
 *
 * @module http/admin-api-settlement.test
 */

import request from 'supertest';
import express, { Express } from 'express';
import { createAdminRouter, AdminAPIConfig } from './admin-api';
import {
  PeerConfig as SettlementPeerConfig,
  isValidEvmAddress,
  isValidNonNegativeIntegerString,
} from '../settlement/types';
import type { Logger } from 'pino';
import type { RoutingTable } from '../routing/routing-table';
import type { BTPClientManager } from '../btp/btp-client-manager';

describe('Admin API Settlement Extension', () => {
  let app: Express;
  let mockRoutingTable: jest.Mocked<RoutingTable>;
  let mockBTPClientManager: jest.Mocked<BTPClientManager>;
  let mockLogger: jest.Mocked<Logger>;
  let settlementPeers: Map<string, SettlementPeerConfig>;

  const validBaseRequest = {
    id: 'peer-a',
    url: 'ws://peer-a:3000',
    authToken: 'secret-token',
    routes: [{ prefix: 'g.peer-a', priority: 0 }],
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

    const config: AdminAPIConfig = {
      routingTable: mockRoutingTable,
      btpClientManager: mockBTPClientManager,
      logger: mockLogger,
      nodeId: 'test-node',
      settlementPeers,
    };

    app = express();
    app.use('/admin', await createAdminRouter(config));
  });

  afterEach(() => {
    jest.clearAllMocks();
  });

  describe('Address Validation Utilities', () => {
    describe('isValidEvmAddress', () => {
      it('should accept valid EVM address', () => {
        expect(isValidEvmAddress('0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28')).toBe(true);
      });

      it('should reject address without 0x prefix', () => {
        expect(isValidEvmAddress('742d35Cc6634C0532925a3b844Bc9e7595f2bD28')).toBe(false);
      });

      it('should reject address with wrong length', () => {
        expect(isValidEvmAddress('0x742d35Cc')).toBe(false);
      });

      it('should reject empty string', () => {
        expect(isValidEvmAddress('')).toBe(false);
      });

      it('should reject address with non-hex characters', () => {
        expect(isValidEvmAddress('0xZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ')).toBe(false);
      });
    });

    describe('isValidNonNegativeIntegerString', () => {
      it('should accept "0"', () => {
        expect(isValidNonNegativeIntegerString('0')).toBe(true);
      });

      it('should accept positive integer', () => {
        expect(isValidNonNegativeIntegerString('1000000')).toBe(true);
      });

      it('should reject negative number', () => {
        expect(isValidNonNegativeIntegerString('-1')).toBe(false);
      });

      it('should reject decimal', () => {
        expect(isValidNonNegativeIntegerString('1.5')).toBe(false);
      });

      it('should reject empty string', () => {
        expect(isValidNonNegativeIntegerString('')).toBe(false);
      });

      it('should reject non-numeric string', () => {
        expect(isValidNonNegativeIntegerString('abc')).toBe(false);
      });
    });
  });

  describe('POST /admin/peers — Settlement Validation (AC: 1, 2)', () => {
    it('should create peer with valid EVM settlement config', async () => {
      const res = await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
          },
        });

      expect(res.status).toBe(201);
      expect(settlementPeers.has('peer-a')).toBe(true);
      expect(settlementPeers.get('peer-a')?.settlementPreference).toBe('evm');
    });

    it('should reject invalid preference value', async () => {
      const res = await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'bitcoin',
          },
        });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('settlement.preference must be one of');
    });

    it('should reject EVM preference without evmAddress', async () => {
      const res = await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
          },
        });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('settlement.evmAddress required when preference is evm');
    });

    it('should reject invalid EVM address format (no 0x prefix)', async () => {
      const res = await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
            evmAddress: '742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
          },
        });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('settlement.evmAddress must be a valid');
    });

    it('should reject invalid EVM address format (wrong length)', async () => {
      const res = await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc',
          },
        });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('settlement.evmAddress must be a valid');
    });

    it('should reject negative chainId', async () => {
      const res = await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
            chainId: -1,
          },
        });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('settlement.chainId must be a positive integer');
    });

    it('should reject non-integer string for initialDeposit', async () => {
      const res = await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
            initialDeposit: '1.5',
          },
        });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain(
        'settlement.initialDeposit must be a non-negative integer string'
      );
    });

    it('should reject invalid tokenAddress format', async () => {
      const res = await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
            tokenAddress: 'invalid',
          },
        });

      expect(res.status).toBe(400);
      expect(res.body.message).toContain('settlement.tokenAddress must be a valid');
    });
  });

  describe('POST /admin/peers — PeerConfig Creation (AC: 3, 7)', () => {
    it('should correctly map settlement config fields to PeerConfig', async () => {
      await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
            tokenAddress: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
            tokenNetworkAddress: '0x1234567890abcdef1234567890abcdef12345678',
            chainId: 8453,
            channelId: '0xabcdef1234567890',
            initialDeposit: '1000000',
          },
        });

      const config = settlementPeers.get('peer-a');
      expect(config).toBeDefined();
      expect(config!.peerId).toBe('peer-a');
      expect(config!.address).toBe('g.peer-a');
      expect(config!.settlementPreference).toBe('evm');
      expect(config!.evmAddress).toBe('0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28');
      expect(config!.tokenAddress).toBe('0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48');
      expect(config!.tokenNetworkAddress).toBe('0x1234567890abcdef1234567890abcdef12345678');
      expect(config!.chainId).toBe(8453);
      expect(config!.channelId).toBe('0xabcdef1234567890');
      expect(config!.initialDeposit).toBe('1000000');
    });

    it('should store PeerConfig in settlement Map with correct peerId key', async () => {
      await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
          },
        });

      expect(settlementPeers.size).toBe(1);
      expect(settlementPeers.has('peer-a')).toBe(true);
    });

    it('should store channelId when provided', async () => {
      await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
            channelId: '0xchannelid123',
          },
        });

      expect(settlementPeers.get('peer-a')?.channelId).toBe('0xchannelid123');
    });

    it('should not have channelId when not provided', async () => {
      await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
          },
        });

      expect(settlementPeers.get('peer-a')?.channelId).toBeUndefined();
    });

    it('should derive ILP address from first route prefix', async () => {
      await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          routes: [
            { prefix: 'g.peer-a.main', priority: 0 },
            { prefix: 'g.peer-a.alt', priority: 1 },
          ],
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
          },
        });

      expect(settlementPeers.get('peer-a')?.address).toBe('g.peer-a.main');
    });

    it('should use empty string for address when no routes', async () => {
      await request(app)
        .post('/admin/peers')
        .send({
          id: 'peer-a',
          url: 'ws://peer-a:3000',
          authToken: 'secret-token',
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
          },
        });

      expect(settlementPeers.get('peer-a')?.address).toBe('');
    });

    it('should set settlementTokens from tokenAddress when provided', async () => {
      await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
            tokenAddress: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
          },
        });

      expect(settlementPeers.get('peer-a')?.settlementTokens).toEqual([
        '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
      ]);
    });

    it('should set settlementTokens to EVM when no tokenAddress', async () => {
      await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
          },
        });

      expect(settlementPeers.get('peer-a')?.settlementTokens).toEqual(['EVM']);
    });
  });

  describe('GET /admin/peers — Settlement in Response (AC: 4)', () => {
    it('should include settlement in response for peer with settlement config', async () => {
      // Register a peer with settlement
      mockBTPClientManager.getPeerIds.mockReturnValue(['peer-a']);
      mockBTPClientManager.getPeerStatus.mockReturnValue(new Map([['peer-a', true]]));
      mockRoutingTable.getAllRoutes.mockReturnValue([
        { prefix: 'g.peer-a', nextHop: 'peer-a', priority: 0 },
      ]);

      settlementPeers.set('peer-a', {
        peerId: 'peer-a',
        address: 'g.peer-a',
        settlementPreference: 'evm',
        settlementTokens: ['EVM'],
        evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
        chainId: 8453,
      });

      const res = await request(app).get('/admin/peers');

      expect(res.status).toBe(200);
      expect(res.body.peers[0].settlement).toBeDefined();
      expect(res.body.peers[0].settlement.preference).toBe('evm');
      expect(res.body.peers[0].settlement.evmAddress).toBe(
        '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28'
      );
      expect(res.body.peers[0].settlement.chainId).toBe(8453);
    });

    it('should not include settlement field for peer without settlement config', async () => {
      mockBTPClientManager.getPeerIds.mockReturnValue(['peer-b']);
      mockBTPClientManager.getPeerStatus.mockReturnValue(new Map([['peer-b', true]]));
      mockRoutingTable.getAllRoutes.mockReturnValue([]);

      const res = await request(app).get('/admin/peers');

      expect(res.status).toBe(200);
      expect(res.body.peers[0].settlement).toBeUndefined();
    });

    it('should handle multiple peers with mixed settlement configs', async () => {
      mockBTPClientManager.getPeerIds.mockReturnValue(['peer-a', 'peer-b']);
      mockBTPClientManager.getPeerStatus.mockReturnValue(
        new Map([
          ['peer-a', true],
          ['peer-b', false],
        ])
      );
      mockRoutingTable.getAllRoutes.mockReturnValue([]);

      settlementPeers.set('peer-a', {
        peerId: 'peer-a',
        address: 'g.peer-a',
        settlementPreference: 'evm',
        settlementTokens: ['EVM'],
        evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
      });

      const res = await request(app).get('/admin/peers');

      expect(res.status).toBe(200);
      expect(res.body.peers[0].settlement).toBeDefined();
      expect(res.body.peers[1].settlement).toBeUndefined();
    });
  });

  describe('DELETE /admin/peers — Settlement Cleanup (AC: 5)', () => {
    it('should remove PeerConfig when deleting peer with settlement', async () => {
      mockBTPClientManager.getPeerIds.mockReturnValue(['peer-a']);

      settlementPeers.set('peer-a', {
        peerId: 'peer-a',
        address: 'g.peer-a',
        settlementPreference: 'evm',
        settlementTokens: ['EVM'],
        evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
      });

      const res = await request(app).delete('/admin/peers/peer-a');

      expect(res.status).toBe(200);
      expect(settlementPeers.has('peer-a')).toBe(false);
    });

    it('should not error when deleting peer without settlement config', async () => {
      mockBTPClientManager.getPeerIds.mockReturnValue(['peer-b']);

      const res = await request(app).delete('/admin/peers/peer-b');

      expect(res.status).toBe(200);
      expect(res.body.success).toBe(true);
    });
  });

  describe('Backward Compatibility (AC: 6)', () => {
    it('should create peer without settlement field — no PeerConfig created', async () => {
      const res = await request(app).post('/admin/peers').send({
        id: 'peer-b',
        url: 'ws://peer-b:3000',
        authToken: 'secret-token',
      });

      expect(res.status).toBe(201);
      expect(settlementPeers.size).toBe(0);
    });

    it('should create peer with undefined settlement — no PeerConfig created', async () => {
      const res = await request(app).post('/admin/peers').send({
        id: 'peer-c',
        url: 'ws://peer-c:3000',
        authToken: 'secret-token',
        settlement: undefined,
      });

      expect(res.status).toBe(201);
      expect(settlementPeers.size).toBe(0);
    });

    it('should preserve existing GET behavior for non-settlement peers', async () => {
      mockBTPClientManager.getPeerIds.mockReturnValue(['peer-d']);
      mockBTPClientManager.getPeerStatus.mockReturnValue(new Map([['peer-d', false]]));
      mockRoutingTable.getAllRoutes.mockReturnValue([]);

      const res = await request(app).get('/admin/peers');

      expect(res.status).toBe(200);
      expect(res.body.peers[0].id).toBe('peer-d');
      expect(res.body.peers[0].settlement).toBeUndefined();
    });

    it('should preserve existing DELETE behavior for non-settlement peers', async () => {
      mockBTPClientManager.getPeerIds.mockReturnValue(['peer-e']);
      mockRoutingTable.getAllRoutes.mockReturnValue([]);

      const res = await request(app).delete('/admin/peers/peer-e');

      expect(res.status).toBe(200);
      expect(res.body.success).toBe(true);
    });
  });

  describe('Settlement without settlementPeers Map (settlement disabled)', () => {
    let appNoSettlement: Express;

    beforeEach(async () => {
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        // No settlementPeers — settlement disabled
      };

      appNoSettlement = express();
      appNoSettlement.use('/admin', await createAdminRouter(config));
    });

    it('should accept POST with settlement but not store PeerConfig', async () => {
      const res = await request(appNoSettlement)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
          },
        });

      expect(res.status).toBe(201);
      // No crash, silently skipped
    });

    it('should still validate settlement even without settlementPeers', async () => {
      const res = await request(appNoSettlement)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'invalid',
          },
        });

      expect(res.status).toBe(400);
    });

    it('should return peers without settlement in GET response', async () => {
      mockBTPClientManager.getPeerIds.mockReturnValue(['peer-a']);
      mockBTPClientManager.getPeerStatus.mockReturnValue(new Map([['peer-a', true]]));
      mockRoutingTable.getAllRoutes.mockReturnValue([]);

      const res = await request(appNoSettlement).get('/admin/peers');

      expect(res.status).toBe(200);
      expect(res.body.peers[0].settlement).toBeUndefined();
    });
  });

  describe('Integration Test — Full Lifecycle (AC: 9)', () => {
    it('should handle full lifecycle: POST → GET → verify → DELETE → verify removal', async () => {
      // Step 1: POST peer with settlement
      const postRes = await request(app)
        .post('/admin/peers')
        .send({
          ...validBaseRequest,
          settlement: {
            preference: 'evm',
            evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28',
            tokenAddress: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
            chainId: 8453,
            channelId: '0xchannelid',
            initialDeposit: '1000000',
          },
        });

      expect(postRes.status).toBe(201);
      expect(settlementPeers.has('peer-a')).toBe(true);

      // Step 2: GET peers — verify settlement in response
      mockBTPClientManager.getPeerIds.mockReturnValue(['peer-a']);
      mockBTPClientManager.getPeerStatus.mockReturnValue(new Map([['peer-a', false]]));
      mockRoutingTable.getAllRoutes.mockReturnValue([
        { prefix: 'g.peer-a', nextHop: 'peer-a', priority: 0 },
      ]);

      const getRes = await request(app).get('/admin/peers');

      expect(getRes.status).toBe(200);
      expect(getRes.body.peers[0].settlement).toBeDefined();
      expect(getRes.body.peers[0].settlement.preference).toBe('evm');
      expect(getRes.body.peers[0].settlement.evmAddress).toBe(
        '0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28'
      );
      expect(getRes.body.peers[0].settlement.chainId).toBe(8453);
      expect(getRes.body.peers[0].settlement.channelId).toBe('0xchannelid');

      // Step 3: Verify PeerConfig in Map
      const storedConfig = settlementPeers.get('peer-a');
      expect(storedConfig).toBeDefined();
      expect(storedConfig!.peerId).toBe('peer-a');
      expect(storedConfig!.tokenAddress).toBe('0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48');
      expect(storedConfig!.initialDeposit).toBe('1000000');

      // Step 4: DELETE peer
      const deleteRes = await request(app).delete('/admin/peers/peer-a');

      expect(deleteRes.status).toBe(200);
      expect(settlementPeers.has('peer-a')).toBe(false);

      // Step 5: GET peers — verify settlement removed
      mockBTPClientManager.getPeerIds.mockReturnValue([]);
      mockBTPClientManager.getPeerStatus.mockReturnValue(new Map());
      mockRoutingTable.getAllRoutes.mockReturnValue([]);

      const getRes2 = await request(app).get('/admin/peers');

      expect(getRes2.status).toBe(200);
      expect(getRes2.body.peers).toHaveLength(0);
    });
  });
});
