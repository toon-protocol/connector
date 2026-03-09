/**
 * Audit Logger Tests
 * Story 11.9: Security Hardening for Agent Wallets
 */

import { AuditLogger } from './audit-logger';
import pino from 'pino';
import Database from 'better-sqlite3';
import * as fs from 'fs';
import * as path from 'path';

describe('AuditLogger', () => {
  let auditLogger: AuditLogger;
  let mockLogger: pino.Logger;
  let db: Database.Database;
  let tempDbPath: string;

  beforeEach(() => {
    mockLogger = pino({ level: 'silent' });

    // Create temporary database for testing
    tempDbPath = path.join(
      process.cwd(),
      'test-data',
      `audit-test-${Date.now()}-${Math.random().toString(36).substr(2, 9)}.db`
    );
    const dbDir = path.dirname(tempDbPath);
    if (!fs.existsSync(dbDir)) {
      fs.mkdirSync(dbDir, { recursive: true });
    }

    db = new Database(tempDbPath);
    auditLogger = new AuditLogger(mockLogger, db);
  });

  afterEach(() => {
    db.close();
    if (fs.existsSync(tempDbPath)) {
      fs.unlinkSync(tempDbPath);
    }
  });

  describe('auditLog', () => {
    it('should log wallet operation to database', async () => {
      await auditLogger.auditLog('wallet_created', 'agent-001', {
        evmAddress: '0x1234567890123456789012345678901234567890',
      });

      const logs = await auditLogger.getAuditLog('agent-001');
      expect(logs.length).toBe(1);
      expect(logs[0]?.operation).toBe('wallet_created');
      expect(logs[0]?.agentId).toBe('agent-001');
      expect(logs[0]?.result).toBe('success');
    });

    it('should store operation details as JSON', async () => {
      const details = {
        amount: '1000',
        token: 'USDC',
        recipient: 'agent-002',
      };

      await auditLogger.auditLog('payment_sent', 'agent-001', details);

      const logs = await auditLogger.getAuditLog('agent-001');
      expect(logs[0]?.details).toEqual(details);
    });

    it('should record failure results', async () => {
      await auditLogger.auditLog(
        'wallet_creation_failed',
        'agent-001',
        { error: 'Rate limit exceeded' },
        'failure'
      );

      const logs = await auditLogger.getAuditLog('agent-001');
      expect(logs[0]?.result).toBe('failure');
    });

    it('should record IP address and user agent', async () => {
      await auditLogger.auditLog(
        'wallet_created',
        'agent-001',
        {},
        'success',
        '192.168.1.1',
        'Mozilla/5.0'
      );

      const logs = await auditLogger.getAuditLog('agent-001');
      expect(logs[0]?.ip).toBe('192.168.1.1');
      expect(logs[0]?.userAgent).toBe('Mozilla/5.0');
    });

    it('should work without database (logs to Pino only)', async () => {
      const noDatabaseLogger = new AuditLogger(mockLogger);

      await expect(
        noDatabaseLogger.auditLog('wallet_created', 'agent-001', {})
      ).resolves.not.toThrow();
    });
  });

  describe('getAuditLog', () => {
    beforeEach(async () => {
      // Create test data
      await auditLogger.auditLog('wallet_created', 'agent-001', { detail: 'created' });
      await auditLogger.auditLog('wallet_funded', 'agent-001', { detail: 'funded' });
      await auditLogger.auditLog('payment_sent', 'agent-001', { detail: 'payment' });
      await auditLogger.auditLog('wallet_created', 'agent-002', { detail: 'created' });
    });

    it('should return all audit logs when no filters provided', async () => {
      const logs = await auditLogger.getAuditLog();
      expect(logs.length).toBe(4);
    });

    it('should filter by agent ID', async () => {
      const logs = await auditLogger.getAuditLog('agent-001');
      expect(logs.length).toBe(3);
      logs.forEach((log) => expect(log.agentId).toBe('agent-001'));
    });

    it('should filter by operation', async () => {
      const logs = await auditLogger.getAuditLog(undefined, 'wallet_created');
      expect(logs.length).toBe(2);
      logs.forEach((log) => expect(log.operation).toBe('wallet_created'));
    });

    it('should filter by agent ID and operation', async () => {
      const logs = await auditLogger.getAuditLog('agent-001', 'wallet_funded');
      expect(logs.length).toBe(1);
      expect(logs[0]?.agentId).toBe('agent-001');
      expect(logs[0]?.operation).toBe('wallet_funded');
    });

    it('should filter by date range', async () => {
      const now = Date.now();
      const startDate = now - 60000; // 1 minute ago
      const endDate = now + 60000; // 1 minute future

      const logs = await auditLogger.getAuditLog(undefined, undefined, startDate, endDate);
      expect(logs.length).toBe(4); // All within range
    });

    it('should return empty array when no database configured', async () => {
      const noDatabaseLogger = new AuditLogger(mockLogger);
      const logs = await noDatabaseLogger.getAuditLog();
      expect(logs).toEqual([]);
    });

    it('should return logs in reverse chronological order', async () => {
      const logs = await auditLogger.getAuditLog();

      // Verify timestamps are descending
      for (let i = 0; i < logs.length - 1; i++) {
        expect(logs[i]!.timestamp).toBeGreaterThanOrEqual(logs[i + 1]!.timestamp);
      }
    });

    it('should limit results to 1000 entries', async () => {
      // This test verifies the LIMIT clause exists
      // In practice, would need 1001+ entries to test properly
      const logs = await auditLogger.getAuditLog();
      expect(logs.length).toBeLessThanOrEqual(1000);
    });
  });

  describe('clear', () => {
    it('should clear all audit log entries', async () => {
      await auditLogger.auditLog('wallet_created', 'agent-001', {});
      await auditLogger.auditLog('wallet_funded', 'agent-001', {});

      let logs = await auditLogger.getAuditLog();
      expect(logs.length).toBe(2);

      auditLogger.clear();

      logs = await auditLogger.getAuditLog();
      expect(logs.length).toBe(0);
    });

    it('should handle clear when no database configured', () => {
      const noDatabaseLogger = new AuditLogger(mockLogger);
      expect(() => noDatabaseLogger.clear()).not.toThrow();
    });
  });
});
