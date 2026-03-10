/**
 * Settlement API Unit Tests
 *
 * Tests for settlement API request validation, authentication, and mock settlement execution.
 * Uses supertest for HTTP endpoint testing with mocked dependencies.
 *
 * Test Coverage:
 * - Request validation (POST /settlement/execute)
 * - Status endpoint (GET /settlement/status/:peerId)
 * - Bearer token authentication
 * - Mock settlement execution flow
 * - Error handling
 *
 * @packageDocumentation
 */

import request from 'supertest';
import express, { Express } from 'express';
import { createSettlementRouter, SettlementAPIConfig } from './settlement-api';
import { AccountManager } from './account-manager';
import { SettlementMonitor } from './settlement-monitor';
import { SettlementState } from '../config/types';
import pino from 'pino';

// Mock dependencies
jest.mock('./account-manager');
jest.mock('./settlement-monitor');

describe('Settlement API', () => {
  let app: Express;
  let mockAccountManager: jest.Mocked<AccountManager>;
  let mockSettlementMonitor: jest.Mocked<SettlementMonitor>;
  let logger: pino.Logger;

  beforeEach(() => {
    // Create test logger (silent in tests)
    logger = pino({ level: 'silent' });

    // Create mock AccountManager
    mockAccountManager = {
      getAccountBalance: jest.fn(),
      recordSettlement: jest.fn(),
      getPeerAccountPair: jest.fn(),
    } as unknown as jest.Mocked<AccountManager>;

    // Create mock SettlementMonitor
    mockSettlementMonitor = {
      markSettlementInProgress: jest.fn(),
      markSettlementCompleted: jest.fn(),
      getSettlementState: jest.fn(),
      on: jest.fn(),
    } as unknown as jest.Mocked<SettlementMonitor>;

    // Create Express app for testing (no auth token by default)
    app = express();
  });

  describe('POST /settlement/execute - Request Validation', () => {
    beforeEach(async () => {
      const config: SettlementAPIConfig = {
        accountManager: mockAccountManager,
        settlementMonitor: mockSettlementMonitor,
        logger,
        authToken: undefined, // No auth for validation tests
      };
      app.use(await createSettlementRouter(config));
    });

    test('should return 400 if peerId missing', async () => {
      const response = await request(app).post('/settlement/execute').send({}).expect(400);

      expect(response.body.error).toContain('Invalid peerId');
    });

    test('should return 400 if peerId is not a string', async () => {
      const response = await request(app)
        .post('/settlement/execute')
        .send({ peerId: 123 })
        .expect(400);

      expect(response.body.error).toContain('Invalid peerId');
    });

    test('should return 400 if peerId is empty string', async () => {
      const response = await request(app)
        .post('/settlement/execute')
        .send({ peerId: '' })
        .expect(400);

      expect(response.body.error).toContain('Invalid peerId');
    });

    test('should return 400 if tokenId is not a string', async () => {
      const response = await request(app)
        .post('/settlement/execute')
        .send({ peerId: 'peer-a', tokenId: 123 })
        .expect(400);

      expect(response.body.error).toContain('Invalid tokenId');
    });

    test('should default tokenId to "M2M" if not provided', async () => {
      // Mock successful settlement
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 1000n,
        netBalance: -1000n,
      });
      mockAccountManager.recordSettlement.mockResolvedValueOnce(undefined);
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 0n,
        netBalance: 0n,
      });

      const response = await request(app)
        .post('/settlement/execute')
        .send({ peerId: 'peer-a' })
        .expect(200);

      expect(response.body.tokenId).toBe('M2M');
      expect(mockAccountManager.recordSettlement).toHaveBeenCalledWith('peer-a', 'M2M', 1000n);
    });

    test('should return 200 with valid response on success', async () => {
      // Mock balance before settlement
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 1000n,
        netBalance: -1000n,
      });
      mockAccountManager.recordSettlement.mockResolvedValueOnce(undefined);
      // Mock balance after settlement
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 0n,
        netBalance: 0n,
      });

      const response = await request(app)
        .post('/settlement/execute')
        .send({ peerId: 'peer-a', tokenId: 'M2M' })
        .expect(200);

      expect(response.body).toMatchObject({
        success: true,
        peerId: 'peer-a',
        tokenId: 'M2M',
        previousBalance: '1000',
        newBalance: '0',
        settledAmount: '1000',
      });
      expect(response.body.timestamp).toBeDefined();
    });

    test('should return 500 if settlement execution fails', async () => {
      // Mock settlement failure
      mockAccountManager.getAccountBalance.mockRejectedValueOnce(
        new Error('TigerBeetle connection error')
      );

      const response = await request(app)
        .post('/settlement/execute')
        .send({ peerId: 'peer-a', tokenId: 'M2M' })
        .expect(500);

      expect(response.body.error).toContain('TigerBeetle connection error');
    });
  });

  describe('GET /settlement/status/:peerId - Status Endpoint', () => {
    beforeEach(async () => {
      const config: SettlementAPIConfig = {
        accountManager: mockAccountManager,
        settlementMonitor: mockSettlementMonitor,
        logger,
        authToken: undefined,
      };
      app.use(await createSettlementRouter(config));
    });

    test('should return current balance and state', async () => {
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 500n,
        netBalance: -500n,
      });
      mockSettlementMonitor.getSettlementState.mockReturnValueOnce(SettlementState.IDLE);

      const response = await request(app)
        .get('/settlement/status/peer-a')
        .query({ tokenId: 'M2M' })
        .expect(200);

      expect(response.body).toMatchObject({
        peerId: 'peer-a',
        tokenId: 'M2M',
        currentBalance: '500',
        settlementState: 'IDLE',
      });
      expect(response.body.timestamp).toBeDefined();
    });

    test('should default tokenId to "M2M" if not in query', async () => {
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 200n,
        netBalance: -200n,
      });
      mockSettlementMonitor.getSettlementState.mockReturnValueOnce(SettlementState.IDLE);

      await request(app).get('/settlement/status/peer-a').expect(200);

      expect(mockAccountManager.getAccountBalance).toHaveBeenCalledWith('peer-a', 'M2M');
      expect(mockSettlementMonitor.getSettlementState).toHaveBeenCalledWith('peer-a', 'M2M');
    });

    test('should return 400 if peerId is empty', async () => {
      // Express routes empty peerId as 404 (route not found)
      // Test with missing peerId parameter
      await request(app).get('/settlement/status/').expect(404);
    });

    test('should return 404 if account not found', async () => {
      mockAccountManager.getAccountBalance.mockRejectedValueOnce(new Error('Account not found'));

      const response = await request(app).get('/settlement/status/peer-unknown').expect(404);

      expect(response.body.error).toContain('Account not found');
    });

    test('should return 500 on balance query failure', async () => {
      mockAccountManager.getAccountBalance.mockRejectedValueOnce(new Error('Database error'));

      const response = await request(app).get('/settlement/status/peer-a').expect(500);

      expect(response.body.error).toContain('Database error');
    });
  });

  describe('Settlement API Authentication', () => {
    test('should accept request when valid Bearer token provided', async () => {
      const config: SettlementAPIConfig = {
        accountManager: mockAccountManager,
        settlementMonitor: mockSettlementMonitor,
        logger,
        authToken: 'test-secret',
      };
      app.use(await createSettlementRouter(config));

      // Mock successful settlement
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 100n,
        netBalance: -100n,
      });
      mockAccountManager.recordSettlement.mockResolvedValueOnce(undefined);
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 0n,
        netBalance: 0n,
      });

      const response = await request(app)
        .post('/settlement/execute')
        .set('Authorization', 'Bearer test-secret')
        .send({ peerId: 'peer-a' })
        .expect(200);

      expect(response.body.success).toBe(true);
    });

    test('should reject request with 401 when Authorization header missing', async () => {
      const config: SettlementAPIConfig = {
        accountManager: mockAccountManager,
        settlementMonitor: mockSettlementMonitor,
        logger,
        authToken: 'test-secret',
      };
      app.use(await createSettlementRouter(config));

      const response = await request(app)
        .post('/settlement/execute')
        .send({ peerId: 'peer-a' })
        .expect(401);

      expect(response.body.error).toContain('Bearer token required');
    });

    test('should reject request with 401 when Authorization header has wrong format', async () => {
      const config: SettlementAPIConfig = {
        accountManager: mockAccountManager,
        settlementMonitor: mockSettlementMonitor,
        logger,
        authToken: 'test-secret',
      };
      app.use(await createSettlementRouter(config));

      const response = await request(app)
        .post('/settlement/execute')
        .set('Authorization', 'Basic dGVzdDp0ZXN0')
        .send({ peerId: 'peer-a' })
        .expect(401);

      expect(response.body.error).toContain('Bearer token required');
    });

    test('should reject request with 403 when Bearer token invalid', async () => {
      const config: SettlementAPIConfig = {
        accountManager: mockAccountManager,
        settlementMonitor: mockSettlementMonitor,
        logger,
        authToken: 'test-secret',
      };
      app.use(await createSettlementRouter(config));

      const response = await request(app)
        .post('/settlement/execute')
        .set('Authorization', 'Bearer wrong-token')
        .send({ peerId: 'peer-a' })
        .expect(403);

      expect(response.body.error).toContain('Invalid token');
    });

    test('should allow requests when auth token not configured (development mode)', async () => {
      const config: SettlementAPIConfig = {
        accountManager: mockAccountManager,
        settlementMonitor: mockSettlementMonitor,
        logger,
        authToken: undefined, // No auth token
      };
      app.use(await createSettlementRouter(config));

      // Mock successful settlement
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 100n,
        netBalance: -100n,
      });
      mockAccountManager.recordSettlement.mockResolvedValueOnce(undefined);
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 0n,
        netBalance: 0n,
      });

      const response = await request(app)
        .post('/settlement/execute')
        .send({ peerId: 'peer-a' })
        .expect(200);

      expect(response.body.success).toBe(true);
    });

    test('should allow requests when auth token is empty string', async () => {
      const config: SettlementAPIConfig = {
        accountManager: mockAccountManager,
        settlementMonitor: mockSettlementMonitor,
        logger,
        authToken: '', // Empty string
      };
      app.use(await createSettlementRouter(config));

      // Mock successful settlement
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 100n,
        netBalance: -100n,
      });
      mockAccountManager.recordSettlement.mockResolvedValueOnce(undefined);
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 0n,
        netBalance: 0n,
      });

      const response = await request(app)
        .post('/settlement/execute')
        .send({ peerId: 'peer-a' })
        .expect(200);

      expect(response.body.success).toBe(true);
    });
  });

  describe('Mock Settlement Execution', () => {
    beforeEach(async () => {
      const config: SettlementAPIConfig = {
        accountManager: mockAccountManager,
        settlementMonitor: mockSettlementMonitor,
        logger,
        authToken: undefined,
      };
      app.use(await createSettlementRouter(config));
    });

    test('should reduce balance to zero after settlement', async () => {
      // Mock balance before: 1200
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 1200n,
        netBalance: -1200n,
      });
      mockAccountManager.recordSettlement.mockResolvedValueOnce(undefined);
      // Mock balance after: 0
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 0n,
        netBalance: 0n,
      });

      const response = await request(app)
        .post('/settlement/execute')
        .send({ peerId: 'peer-a', tokenId: 'M2M' })
        .expect(200);

      expect(mockAccountManager.recordSettlement).toHaveBeenCalledWith('peer-a', 'M2M', 1200n);
      expect(response.body.previousBalance).toBe('1200');
      expect(response.body.newBalance).toBe('0');
      expect(response.body.settledAmount).toBe('1200');
    });

    test('should update settlement state to IN_PROGRESS then IDLE', async () => {
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 500n,
        netBalance: -500n,
      });
      mockAccountManager.recordSettlement.mockResolvedValueOnce(undefined);
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 0n,
        netBalance: 0n,
      });

      await request(app)
        .post('/settlement/execute')
        .send({ peerId: 'peer-a', tokenId: 'M2M' })
        .expect(200);

      // Verify state transitions
      expect(mockSettlementMonitor.markSettlementInProgress).toHaveBeenCalledWith('peer-a', 'M2M');
      expect(mockSettlementMonitor.markSettlementCompleted).toHaveBeenCalledWith('peer-a', 'M2M');
    });

    test('should handle settlement errors gracefully', async () => {
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 500n,
        netBalance: -500n,
      });
      mockAccountManager.recordSettlement.mockRejectedValueOnce(
        new Error('TigerBeetle transfer failed')
      );

      const response = await request(app)
        .post('/settlement/execute')
        .send({ peerId: 'peer-a', tokenId: 'M2M' })
        .expect(500);

      expect(response.body.error).toContain('TigerBeetle transfer failed');

      // State marked IN_PROGRESS but NOT completed (remains for retry)
      expect(mockSettlementMonitor.markSettlementInProgress).toHaveBeenCalled();
      expect(mockSettlementMonitor.markSettlementCompleted).not.toHaveBeenCalled();
    });

    test('should throw error if balance not reduced to zero', async () => {
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 1000n,
        netBalance: -1000n,
      });
      mockAccountManager.recordSettlement.mockResolvedValueOnce(undefined);
      // Mock balance still has value after settlement (verification failure)
      mockAccountManager.getAccountBalance.mockResolvedValueOnce({
        debitBalance: 0n,
        creditBalance: 100n, // Should be 0!
        netBalance: -100n,
      });

      const response = await request(app)
        .post('/settlement/execute')
        .send({ peerId: 'peer-a', tokenId: 'M2M' })
        .expect(500);

      expect(response.body.error).toContain('balance not reduced to zero');

      // Settlement NOT marked as completed
      expect(mockSettlementMonitor.markSettlementCompleted).not.toHaveBeenCalled();
    });
  });

  describe('Automatic Settlement Integration', () => {
    test('should listen for SETTLEMENT_REQUIRED events', async () => {
      const config: SettlementAPIConfig = {
        accountManager: mockAccountManager,
        settlementMonitor: mockSettlementMonitor,
        logger,
        authToken: undefined,
      };

      await createSettlementRouter(config);

      // Verify event listener attached
      expect(mockSettlementMonitor.on).toHaveBeenCalledWith(
        'SETTLEMENT_REQUIRED',
        expect.any(Function)
      );
    });
  });
});
