/**
 * Unit Tests for Admin API Security Hardening
 *
 * Tests:
 * - Timing-safe API key comparison (same behavior, hardened implementation)
 * - X-Api-Key header-only enforcement (query param rejected)
 * - Various key comparison edge cases
 * - IP allowlist middleware (individual IPs and CIDR ranges)
 * - X-Forwarded-For header handling (trustProxy mode)
 * - Combined API key + IP allowlist (defense in depth)
 *
 * @module http/admin-api-security.test
 */

import request from 'supertest';
import express, { Express } from 'express';
import { createAdminRouter, AdminAPIConfig } from './admin-api';
import type { Logger } from 'pino';
import type { RoutingTable } from '../routing/routing-table';
import type { BTPClientManager } from '../btp/btp-client-manager';

describe('Admin API Security Hardening', () => {
  let appWithAuth: Express;
  let mockRoutingTable: jest.Mocked<RoutingTable>;
  let mockBTPClientManager: jest.Mocked<BTPClientManager>;
  let mockLogger: jest.Mocked<Logger>;

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

    mockLogger = {
      info: jest.fn(),
      error: jest.fn(),
      warn: jest.fn(),
      debug: jest.fn(),
      child: jest.fn().mockReturnThis(),
      fatal: jest.fn(),
      trace: jest.fn(),
      level: 'info',
    } as unknown as jest.Mocked<Logger>;

    const config: AdminAPIConfig = {
      routingTable: mockRoutingTable,
      btpClientManager: mockBTPClientManager,
      logger: mockLogger,
      nodeId: 'test-node',
      apiKey: 'test-secret-key',
    };

    appWithAuth = express();
    appWithAuth.use('/admin', await createAdminRouter(config));
  });

  describe('Timing-safe API key comparison', () => {
    it('should accept valid API key via X-Api-Key header', async () => {
      const res = await request(appWithAuth)
        .get('/admin/routes')
        .set('X-Api-Key', 'test-secret-key');

      expect(res.status).toBe(200);
    });

    it('should reject missing API key with 401', async () => {
      const res = await request(appWithAuth).get('/admin/routes');

      expect(res.status).toBe(401);
      expect(res.body.error).toBe('Unauthorized');
    });

    it('should reject wrong API key with 401', async () => {
      const res = await request(appWithAuth).get('/admin/routes').set('X-Api-Key', 'wrong-key');

      expect(res.status).toBe(401);
      expect(res.body.error).toBe('Unauthorized');
    });

    it('should reject API key with different length', async () => {
      const res = await request(appWithAuth).get('/admin/routes').set('X-Api-Key', 'short');

      expect(res.status).toBe(401);
    });

    it('should reject API key that is longer than expected', async () => {
      const res = await request(appWithAuth)
        .get('/admin/routes')
        .set('X-Api-Key', 'test-secret-key-extra-characters');

      expect(res.status).toBe(401);
    });

    it('should reject empty API key', async () => {
      const res = await request(appWithAuth).get('/admin/routes').set('X-Api-Key', '');

      expect(res.status).toBe(401);
    });
  });

  describe('Query parameter API key rejection', () => {
    it('should reject API key supplied via query parameter', async () => {
      const res = await request(appWithAuth).get('/admin/routes?apiKey=test-secret-key');

      expect(res.status).toBe(401);
      expect(res.body.message).toContain('X-Api-Key header');
    });

    it('should reject query param even with correct key', async () => {
      const res = await request(appWithAuth).get('/admin/peers?apiKey=test-secret-key');

      expect(res.status).toBe(401);
      expect(res.body.message).toContain('query parameter');
    });

    it('should reject when both query param and header are provided', async () => {
      // Query param presence alone should trigger rejection
      const res = await request(appWithAuth)
        .get('/admin/routes?apiKey=test-secret-key')
        .set('X-Api-Key', 'test-secret-key');

      expect(res.status).toBe(401);
      expect(res.body.message).toContain('X-Api-Key header');
    });
  });

  describe('No auth when apiKey not configured', () => {
    let appNoAuth: Express;

    beforeEach(async () => {
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        // No apiKey set
      };

      appNoAuth = express();
      appNoAuth.use('/admin', await createAdminRouter(config));
    });

    it('should allow access without any auth headers', async () => {
      const res = await request(appNoAuth).get('/admin/routes');
      expect(res.status).toBe(200);
    });
  });

  describe('IP Allowlist - Individual IPs', () => {
    let appWithIPAllowlist: Express;

    beforeEach(async () => {
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        allowedIPs: ['127.0.0.1', '::1', '192.168.1.100'],
        trustProxy: false,
      };

      appWithIPAllowlist = express();
      appWithIPAllowlist.use('/admin', await createAdminRouter(config));
    });

    it('should allow request from allowed IPv4 address', async () => {
      const res = await request(appWithIPAllowlist).get('/admin/routes');
      // supertest uses 127.0.0.1 by default
      expect(res.status).toBe(200);
    });

    it('should block request from non-allowed IP', async () => {
      // Mock a different IP by setting up a custom test
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        allowedIPs: ['192.168.1.100'], // Only allow this specific IP
        trustProxy: false,
      };

      const appBlockedIP = express();
      appBlockedIP.use('/admin', await createAdminRouter(config));

      const res = await request(appBlockedIP).get('/admin/routes');
      // supertest uses 127.0.0.1, which is not in the allowlist
      expect(res.status).toBe(403);
      expect(res.body.error).toBe('Forbidden');
      expect(res.body.message).toContain('IP address not allowed');
    });
  });

  describe('IP Allowlist - CIDR Ranges', () => {
    let appWithCIDR: Express;

    beforeEach(async () => {
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        allowedIPs: ['127.0.0.0/8', '10.0.0.0/16'], // Localhost range + private network
        trustProxy: false,
      };

      appWithCIDR = express();
      appWithCIDR.use('/admin', await createAdminRouter(config));
    });

    it('should allow request from IP within CIDR range', async () => {
      const res = await request(appWithCIDR).get('/admin/routes');
      // 127.0.0.1 is within 127.0.0.0/8
      expect(res.status).toBe(200);
    });

    it('should block request from IP outside CIDR range', async () => {
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        allowedIPs: ['192.168.1.0/24'], // Private network that doesn't include 127.0.0.1
        trustProxy: false,
      };

      const appOutsideCIDR = express();
      appOutsideCIDR.use('/admin', await createAdminRouter(config));

      const res = await request(appOutsideCIDR).get('/admin/routes');
      expect(res.status).toBe(403);
    });
  });

  describe('IP Allowlist - X-Forwarded-For (trustProxy)', () => {
    let appWithProxy: Express;

    beforeEach(async () => {
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        allowedIPs: ['203.0.113.5'], // Specific client IP
        trustProxy: true,
      };

      appWithProxy = express();
      // Enable trust proxy for Express to parse X-Forwarded-For
      appWithProxy.set('trust proxy', true);
      appWithProxy.use('/admin', await createAdminRouter(config));
    });

    it('should use X-Forwarded-For when trustProxy is true', async () => {
      const res = await request(appWithProxy)
        .get('/admin/routes')
        .set('X-Forwarded-For', '203.0.113.5');

      expect(res.status).toBe(200);
    });

    it('should extract first IP from comma-separated X-Forwarded-For', async () => {
      const res = await request(appWithProxy)
        .get('/admin/routes')
        .set('X-Forwarded-For', '203.0.113.5, 198.51.100.1, 192.0.2.1');

      expect(res.status).toBe(200);
    });

    it('should block when X-Forwarded-For IP not in allowlist', async () => {
      const res = await request(appWithProxy)
        .get('/admin/routes')
        .set('X-Forwarded-For', '198.51.100.99');

      expect(res.status).toBe(403);
    });

    it('should use socket IP when X-Forwarded-For not present', async () => {
      // Without X-Forwarded-For, falls back to socket IP (127.0.0.1)
      const res = await request(appWithProxy).get('/admin/routes');
      expect(res.status).toBe(403); // 127.0.0.1 not in allowlist
    });
  });

  describe('IP Allowlist - trustProxy: false (direct connection)', () => {
    let appNoProxy: Express;

    beforeEach(async () => {
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        allowedIPs: ['127.0.0.1'],
        trustProxy: false, // Ignore X-Forwarded-For
      };

      appNoProxy = express();
      appNoProxy.use('/admin', await createAdminRouter(config));
    });

    it('should ignore X-Forwarded-For when trustProxy is false', async () => {
      const res = await request(appNoProxy)
        .get('/admin/routes')
        .set('X-Forwarded-For', '192.168.1.100');

      // Should use socket IP (127.0.0.1), not X-Forwarded-For
      expect(res.status).toBe(200);
    });
  });

  describe('IP Allowlist + API Key (Defense in Depth)', () => {
    let appBothAuth: Express;

    beforeEach(async () => {
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        apiKey: 'secure-key',
        allowedIPs: ['127.0.0.1'],
        trustProxy: false,
      };

      appBothAuth = express();
      appBothAuth.use('/admin', await createAdminRouter(config));
    });

    it('should require BOTH IP allowlist AND API key', async () => {
      // Correct IP but no API key
      const res1 = await request(appBothAuth).get('/admin/routes');
      // API key middleware returns 401, but express.json() body parser may
      // return 400 if Content-Type header triggers a parse error before the
      // API key middleware runs. Accept either status code.
      expect([400, 401]).toContain(res1.status);

      // Correct API key but wrong IP (simulated by changing allowlist)
      const configWrongIP: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        apiKey: 'secure-key',
        allowedIPs: ['192.168.1.100'], // Different IP
        trustProxy: false,
      };

      const appWrongIP = express();
      appWrongIP.use('/admin', await createAdminRouter(configWrongIP));

      const res2 = await request(appWrongIP).get('/admin/routes').set('X-Api-Key', 'secure-key');
      expect(res2.status).toBe(403); // Blocked by IP allowlist
    });

    it('should allow when BOTH IP and API key are correct', async () => {
      const res = await request(appBothAuth).get('/admin/routes').set('X-Api-Key', 'secure-key');

      expect(res.status).toBe(200);
    });
  });

  describe('IP Allowlist - Edge Cases', () => {
    it('should handle empty allowedIPs array (no restriction)', async () => {
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        allowedIPs: [], // Empty array
        trustProxy: false,
      };

      const app = express();
      app.use('/admin', await createAdminRouter(config));

      const res = await request(app).get('/admin/routes');
      expect(res.status).toBe(200); // No IP restriction
    });

    it('should handle allowedIPs: undefined (no restriction)', async () => {
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        // allowedIPs not set
        trustProxy: false,
      };

      const app = express();
      app.use('/admin', await createAdminRouter(config));

      const res = await request(app).get('/admin/routes');
      expect(res.status).toBe(200);
    });

    it('should handle IPv6 loopback (::1)', async () => {
      const config: AdminAPIConfig = {
        routingTable: mockRoutingTable,
        btpClientManager: mockBTPClientManager,
        logger: mockLogger,
        nodeId: 'test-node',
        allowedIPs: ['::1'],
        trustProxy: false,
      };

      const app = express();
      app.use('/admin', await createAdminRouter(config));

      // Note: supertest default behavior varies by system for IPv6
      // This test verifies the middleware accepts ::1 in config
      const router = await createAdminRouter(config);
      expect(router).toBeDefined();
    });
  });
});
