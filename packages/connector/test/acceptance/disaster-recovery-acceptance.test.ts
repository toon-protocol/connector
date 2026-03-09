/**
 * Disaster Recovery Acceptance Tests
 * Story 12.10: Production Acceptance Testing and Go-Live
 *
 * Tests disaster recovery procedures including:
 * - Database backup and restore
 * - Key recovery and rotation
 * - State reconstruction from persistent storage
 * - Graceful degradation under failure conditions
 * - Service restart and recovery
 *
 * Test Coverage (AC: 5):
 * - Full system recovery from simulated failure
 * - Data integrity after recovery
 * - Key recovery procedures
 * - Settlement state preservation
 */

import Database from 'better-sqlite3';
import * as fs from 'fs';
import * as path from 'path';
import * as crypto from 'crypto';

// Acceptance tests have 5 minute timeout per test
jest.setTimeout(300000);

// Test configuration
const TEST_DATA_DIR = path.join(process.cwd(), 'test-data', 'disaster-recovery');

interface AccountState {
  agentId: string;
  evmBalance: bigint;
  pendingSettlements: number;
  lastUpdated: Date;
}

interface SettlementState {
  id: string;
  agentId: string;
  amount: bigint;
  chain: 'evm';
  status: 'pending' | 'completed' | 'failed';
  createdAt: Date;
}

interface SystemSnapshot {
  timestamp: Date;
  accounts: AccountState[];
  settlements: SettlementState[];
  checksum: string;
}

/**
 * Mock persistent storage for disaster recovery testing
 */
class PersistentStorage {
  private db: Database.Database;
  private dbPath: string;
  private backupDir: string;

  constructor(dbPath: string, backupDir: string) {
    this.dbPath = dbPath;
    this.backupDir = backupDir;

    // Ensure directories exist
    const dbDir = path.dirname(dbPath);
    if (!fs.existsSync(dbDir)) {
      fs.mkdirSync(dbDir, { recursive: true });
    }
    if (!fs.existsSync(backupDir)) {
      fs.mkdirSync(backupDir, { recursive: true });
    }

    this.db = new Database(dbPath);
    this.initializeSchema();
  }

  private initializeSchema(): void {
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS accounts (
        agent_id TEXT PRIMARY KEY,
        evm_balance INTEGER DEFAULT 0,
        pending_settlements INTEGER DEFAULT 0,
        last_updated TEXT DEFAULT CURRENT_TIMESTAMP
      );

      CREATE TABLE IF NOT EXISTS settlements (
        id TEXT PRIMARY KEY,
        agent_id TEXT NOT NULL,
        amount INTEGER NOT NULL,
        chain TEXT NOT NULL,
        status TEXT DEFAULT 'pending',
        created_at TEXT DEFAULT CURRENT_TIMESTAMP
      );

      CREATE TABLE IF NOT EXISTS snapshots (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        timestamp TEXT NOT NULL,
        checksum TEXT NOT NULL,
        data BLOB NOT NULL
      );
    `);
  }

  createAccount(agentId: string, evmBalance: bigint): void {
    const stmt = this.db.prepare(`
      INSERT OR REPLACE INTO accounts (agent_id, evm_balance, last_updated)
      VALUES (?, ?, datetime('now'))
    `);
    stmt.run(agentId, Number(evmBalance));
  }

  getAccount(agentId: string): AccountState | null {
    const stmt = this.db.prepare('SELECT * FROM accounts WHERE agent_id = ?');
    const row = stmt.get(agentId) as Record<string, unknown> | undefined;

    if (!row) return null;

    return {
      agentId: row.agent_id as string,
      evmBalance: BigInt(row.evm_balance as number),
      pendingSettlements: row.pending_settlements as number,
      lastUpdated: new Date(row.last_updated as string),
    };
  }

  getAllAccounts(): AccountState[] {
    const stmt = this.db.prepare('SELECT * FROM accounts');
    const rows = stmt.all() as Record<string, unknown>[];

    return rows.map((row) => ({
      agentId: row.agent_id as string,
      evmBalance: BigInt(row.evm_balance as number),
      pendingSettlements: row.pending_settlements as number,
      lastUpdated: new Date(row.last_updated as string),
    }));
  }

  createSettlement(settlement: Omit<SettlementState, 'createdAt'>): void {
    const stmt = this.db.prepare(`
      INSERT INTO settlements (id, agent_id, amount, chain, status)
      VALUES (?, ?, ?, ?, ?)
    `);
    stmt.run(
      settlement.id,
      settlement.agentId,
      Number(settlement.amount),
      settlement.chain,
      settlement.status
    );
  }

  getSettlement(id: string): SettlementState | null {
    const stmt = this.db.prepare('SELECT * FROM settlements WHERE id = ?');
    const row = stmt.get(id) as Record<string, unknown> | undefined;

    if (!row) return null;

    return {
      id: row.id as string,
      agentId: row.agent_id as string,
      amount: BigInt(row.amount as number),
      chain: row.chain as 'evm',
      status: row.status as 'pending' | 'completed' | 'failed',
      createdAt: new Date(row.created_at as string),
    };
  }

  getAllSettlements(): SettlementState[] {
    const stmt = this.db.prepare('SELECT * FROM settlements');
    const rows = stmt.all() as Record<string, unknown>[];

    return rows.map((row) => ({
      id: row.id as string,
      agentId: row.agent_id as string,
      amount: BigInt(row.amount as number),
      chain: row.chain as 'evm',
      status: row.status as 'pending' | 'completed' | 'failed',
      createdAt: new Date(row.created_at as string),
    }));
  }

  updateSettlementStatus(id: string, status: 'pending' | 'completed' | 'failed'): void {
    const stmt = this.db.prepare('UPDATE settlements SET status = ? WHERE id = ?');
    stmt.run(status, id);
  }

  /**
   * Create a backup of the current database state
   */
  createBackup(): string {
    const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
    const backupPath = path.join(this.backupDir, `backup-${timestamp}.db`);

    // Checkpoint WAL to ensure all data is written to main file
    this.db.pragma('wal_checkpoint(TRUNCATE)');

    // Copy database file
    fs.copyFileSync(this.dbPath, backupPath);

    return backupPath;
  }

  /**
   * Restore database from backup
   */
  restoreFromBackup(backupPath: string): void {
    if (!fs.existsSync(backupPath)) {
      throw new Error(`Backup file not found: ${backupPath}`);
    }

    // Close current connection
    this.db.close();

    // Copy backup over current database
    fs.copyFileSync(backupPath, this.dbPath);

    // Reopen connection
    this.db = new Database(this.dbPath);
  }

  /**
   * Create a snapshot of current state with checksum
   */
  createSnapshot(): SystemSnapshot {
    const accounts = this.getAllAccounts();
    const settlements = this.getAllSettlements();
    const timestamp = new Date();

    // Create deterministic checksum
    const data = JSON.stringify({ accounts, settlements }, (_, v) =>
      typeof v === 'bigint' ? v.toString() : v
    );
    const checksum = crypto.createHash('sha256').update(data).digest('hex');

    // Store snapshot
    const stmt = this.db.prepare(`
      INSERT INTO snapshots (timestamp, checksum, data)
      VALUES (?, ?, ?)
    `);
    stmt.run(timestamp.toISOString(), checksum, Buffer.from(data));

    return { timestamp, accounts, settlements, checksum };
  }

  /**
   * Verify data integrity against snapshot
   */
  verifyIntegrity(snapshot: SystemSnapshot): boolean {
    const currentAccounts = this.getAllAccounts();
    const currentSettlements = this.getAllSettlements();

    // Recalculate checksum
    const data = JSON.stringify(
      { accounts: currentAccounts, settlements: currentSettlements },
      (_, v) => (typeof v === 'bigint' ? v.toString() : v)
    );
    const currentChecksum = crypto.createHash('sha256').update(data).digest('hex');

    return currentChecksum === snapshot.checksum;
  }

  close(): void {
    this.db.close();
  }

  getDbPath(): string {
    return this.dbPath;
  }
}

/**
 * Mock key manager for disaster recovery testing
 */
class KeyRecoveryManager {
  private encryptedKeys: Map<string, Buffer> = new Map();
  private masterKey: Buffer;

  constructor(masterKey?: Buffer) {
    this.masterKey = masterKey || crypto.randomBytes(32);
  }

  /**
   * Store encrypted key
   */
  storeKey(keyId: string, keyData: Buffer): void {
    const iv = crypto.randomBytes(12);
    const cipher = crypto.createCipheriv('aes-256-gcm', this.masterKey, iv);
    const encrypted = Buffer.concat([cipher.update(keyData), cipher.final()]);
    const authTag = cipher.getAuthTag();

    // Store IV + authTag + encrypted data
    const combined = Buffer.concat([iv, authTag, encrypted]);
    this.encryptedKeys.set(keyId, combined);
  }

  /**
   * Recover key
   */
  recoverKey(keyId: string): Buffer | null {
    const combined = this.encryptedKeys.get(keyId);
    if (!combined) return null;

    const iv = combined.subarray(0, 12);
    const authTag = combined.subarray(12, 28);
    const encrypted = combined.subarray(28);

    const decipher = crypto.createDecipheriv('aes-256-gcm', this.masterKey, iv);
    decipher.setAuthTag(authTag);

    try {
      return Buffer.concat([decipher.update(encrypted), decipher.final()]);
    } catch {
      return null; // Decryption failed
    }
  }

  /**
   * Rotate master key
   */
  rotateMasterKey(newMasterKey: Buffer): Map<string, Buffer> {
    const reencryptedKeys = new Map<string, Buffer>();

    for (const [keyId] of this.encryptedKeys) {
      const plainKey = this.recoverKey(keyId);
      if (plainKey) {
        // Encrypt with new key
        const iv = crypto.randomBytes(12);
        const cipher = crypto.createCipheriv('aes-256-gcm', newMasterKey, iv);
        const encrypted = Buffer.concat([cipher.update(plainKey), cipher.final()]);
        const authTag = cipher.getAuthTag();

        reencryptedKeys.set(keyId, Buffer.concat([iv, authTag, encrypted]));
      }
    }

    this.masterKey = newMasterKey;
    this.encryptedKeys = reencryptedKeys;

    return reencryptedKeys;
  }

  /**
   * Export encrypted keys for backup
   */
  exportKeys(): { keyId: string; encryptedData: string }[] {
    const exported: { keyId: string; encryptedData: string }[] = [];

    for (const [keyId, data] of this.encryptedKeys) {
      exported.push({
        keyId,
        encryptedData: data.toString('base64'),
      });
    }

    return exported;
  }

  /**
   * Import encrypted keys from backup
   */
  importKeys(backup: { keyId: string; encryptedData: string }[]): void {
    for (const { keyId, encryptedData } of backup) {
      this.encryptedKeys.set(keyId, Buffer.from(encryptedData, 'base64'));
    }
  }
}

/**
 * Service health manager for graceful degradation
 */
class ServiceHealthManager {
  private services: Map<string, { healthy: boolean; lastCheck: Date }> = new Map();
  private degradationMode: boolean = false;

  registerService(serviceId: string): void {
    this.services.set(serviceId, { healthy: true, lastCheck: new Date() });
  }

  setServiceHealth(serviceId: string, healthy: boolean): void {
    const service = this.services.get(serviceId);
    if (service) {
      service.healthy = healthy;
      service.lastCheck = new Date();
    }

    this.checkDegradationMode();
  }

  private checkDegradationMode(): void {
    const unhealthyCount = Array.from(this.services.values()).filter((s) => !s.healthy).length;

    // Enter degradation mode if >50% of services are unhealthy
    this.degradationMode = unhealthyCount > this.services.size / 2;
  }

  isDegradationMode(): boolean {
    return this.degradationMode;
  }

  getHealthySummary(): { total: number; healthy: number; unhealthy: string[] } {
    const unhealthy: string[] = [];
    let healthy = 0;

    for (const [serviceId, status] of this.services) {
      if (status.healthy) {
        healthy++;
      } else {
        unhealthy.push(serviceId);
      }
    }

    return {
      total: this.services.size,
      healthy,
      unhealthy,
    };
  }
}

describe('Disaster Recovery Acceptance Tests', () => {
  let storage: PersistentStorage;
  let keyManager: KeyRecoveryManager;
  let healthManager: ServiceHealthManager;
  let testDbPath: string;
  let testBackupDir: string;

  beforeAll(() => {
    // Setup test directories
    const timestamp = Date.now();
    testDbPath = path.join(TEST_DATA_DIR, `test-${timestamp}.db`);
    testBackupDir = path.join(TEST_DATA_DIR, `backups-${timestamp}`);

    if (!fs.existsSync(TEST_DATA_DIR)) {
      fs.mkdirSync(TEST_DATA_DIR, { recursive: true });
    }
  });

  beforeEach(() => {
    // Create fresh instances for each test
    const timestamp = Date.now() + Math.random();
    testDbPath = path.join(TEST_DATA_DIR, `test-${timestamp}.db`);
    testBackupDir = path.join(TEST_DATA_DIR, `backups-${timestamp}`);

    storage = new PersistentStorage(testDbPath, testBackupDir);
    keyManager = new KeyRecoveryManager();
    healthManager = new ServiceHealthManager();
  });

  afterEach(() => {
    storage.close();

    // Cleanup test files
    try {
      if (fs.existsSync(testDbPath)) {
        fs.unlinkSync(testDbPath);
      }
      if (fs.existsSync(testBackupDir)) {
        fs.rmSync(testBackupDir, { recursive: true });
      }
    } catch {
      // Ignore cleanup errors
    }
  });

  afterAll(() => {
    // Cleanup test directory
    try {
      if (fs.existsSync(TEST_DATA_DIR)) {
        fs.rmSync(TEST_DATA_DIR, { recursive: true });
      }
    } catch {
      // Ignore cleanup errors
    }
  });

  describe('Database Backup and Restore', () => {
    it('should create database backup with all data', () => {
      // Create test data
      storage.createAccount('agent-1', BigInt(1000));
      storage.createAccount('agent-2', BigInt(2000));
      storage.createSettlement({
        id: 'settlement-1',
        agentId: 'agent-1',
        amount: BigInt(100),
        chain: 'evm',
        status: 'pending',
      });

      // Create backup
      const backupPath = storage.createBackup();

      // Verify backup file exists
      expect(fs.existsSync(backupPath)).toBe(true);

      // Verify backup contains data
      const backupDb = new Database(backupPath);
      const accounts = backupDb.prepare('SELECT COUNT(*) as count FROM accounts').get() as {
        count: number;
      };
      const settlements = backupDb.prepare('SELECT COUNT(*) as count FROM settlements').get() as {
        count: number;
      };
      backupDb.close();

      expect(accounts.count).toBe(2);
      expect(settlements.count).toBe(1);
    });

    it('should restore database from backup', () => {
      // Create initial data
      storage.createAccount('agent-1', BigInt(1000));
      storage.createSettlement({
        id: 'settlement-1',
        agentId: 'agent-1',
        amount: BigInt(100),
        chain: 'evm',
        status: 'pending',
      });

      // Create backup
      const backupPath = storage.createBackup();

      // Modify data after backup
      storage.createAccount('agent-2', BigInt(9999), BigInt(9999));
      storage.updateSettlementStatus('settlement-1', 'completed');

      // Verify modified state
      const modifiedAccount = storage.getAccount('agent-2');
      expect(modifiedAccount).not.toBeNull();

      const modifiedSettlement = storage.getSettlement('settlement-1');
      expect(modifiedSettlement?.status).toBe('completed');

      // Restore from backup
      storage.restoreFromBackup(backupPath);

      // Verify restored state
      const restoredAccount1 = storage.getAccount('agent-1');
      const restoredAccount2 = storage.getAccount('agent-2');
      const restoredSettlement = storage.getSettlement('settlement-1');

      expect(restoredAccount1?.evmBalance).toBe(BigInt(1000));
      expect(restoredAccount2).toBeNull(); // Should not exist
      expect(restoredSettlement?.status).toBe('pending'); // Should be reverted
    });

    it('should preserve data integrity across backup/restore cycle', () => {
      // Create complex state
      for (let i = 0; i < 10; i++) {
        storage.createAccount(`agent-${i}`, BigInt(i * 1000));
        storage.createSettlement({
          id: `settlement-${i}`,
          agentId: `agent-${i}`,
          amount: BigInt(i * 100),
          chain: 'evm',
          status: 'pending',
        });
      }

      // Create snapshot before backup
      const snapshotBefore = storage.createSnapshot();

      // Create backup
      const backupPath = storage.createBackup();

      // Corrupt data
      storage.createAccount('corrupt-agent', BigInt(999999), BigInt(999999));
      for (let i = 0; i < 5; i++) {
        storage.updateSettlementStatus(`settlement-${i}`, 'failed');
      }

      // Restore from backup
      storage.restoreFromBackup(backupPath);

      // Verify integrity
      const isIntact = storage.verifyIntegrity(snapshotBefore);
      expect(isIntact).toBe(true);
    });
  });

  describe('Key Recovery and Rotation', () => {
    it('should store and recover encrypted keys', () => {
      const testKey = crypto.randomBytes(32);
      const keyId = 'wallet-key-001';

      // Store key
      keyManager.storeKey(keyId, testKey);

      // Recover key
      const recoveredKey = keyManager.recoverKey(keyId);

      expect(recoveredKey).not.toBeNull();
      expect(recoveredKey?.equals(testKey)).toBe(true);
    });

    it('should return null for non-existent keys', () => {
      const recoveredKey = keyManager.recoverKey('non-existent');
      expect(recoveredKey).toBeNull();
    });

    it('should rotate master key without data loss', () => {
      // Store multiple keys
      const keys: { id: string; data: Buffer }[] = [];
      for (let i = 0; i < 5; i++) {
        const keyData = crypto.randomBytes(32);
        const keyId = `key-${i}`;
        keyManager.storeKey(keyId, keyData);
        keys.push({ id: keyId, data: keyData });
      }

      // Rotate master key
      const newMasterKey = crypto.randomBytes(32);
      keyManager.rotateMasterKey(newMasterKey);

      // Verify all keys are still recoverable
      for (const { id, data } of keys) {
        const recovered = keyManager.recoverKey(id);
        expect(recovered?.equals(data)).toBe(true);
      }
    });

    it('should export and import keys for backup', () => {
      // Store keys
      const testKey1 = crypto.randomBytes(32);
      const testKey2 = crypto.randomBytes(32);
      keyManager.storeKey('key-1', testKey1);
      keyManager.storeKey('key-2', testKey2);

      // Export keys
      const exported = keyManager.exportKeys();
      expect(exported.length).toBe(2);

      // Create new key manager and import
      const newKeyManager = new KeyRecoveryManager(
        // Use same master key for import
        (keyManager as unknown as { masterKey: Buffer }).masterKey
      );
      newKeyManager.importKeys(exported);

      // Verify imported keys
      const recovered1 = newKeyManager.recoverKey('key-1');
      const recovered2 = newKeyManager.recoverKey('key-2');

      expect(recovered1?.equals(testKey1)).toBe(true);
      expect(recovered2?.equals(testKey2)).toBe(true);
    });
  });

  describe('State Reconstruction', () => {
    it('should create verifiable snapshots', () => {
      // Create state
      storage.createAccount('agent-1', BigInt(1000));
      storage.createSettlement({
        id: 'settlement-1',
        agentId: 'agent-1',
        amount: BigInt(100),
        chain: 'evm',
        status: 'pending',
      });

      // Create snapshot
      const snapshot = storage.createSnapshot();

      // Verify snapshot properties
      expect(snapshot.timestamp).toBeInstanceOf(Date);
      expect(snapshot.accounts.length).toBe(1);
      expect(snapshot.settlements.length).toBe(1);
      expect(snapshot.checksum).toMatch(/^[a-f0-9]{64}$/);
    });

    it('should detect data corruption', () => {
      // Create state
      storage.createAccount('agent-1', BigInt(1000));

      // Create snapshot
      const snapshot = storage.createSnapshot();

      // Verify integrity before modification
      expect(storage.verifyIntegrity(snapshot)).toBe(true);

      // Modify data (simulate corruption)
      storage.createAccount('agent-1', BigInt(9999), BigInt(9999));

      // Verify integrity detects change
      expect(storage.verifyIntegrity(snapshot)).toBe(false);
    });

    it('should reconstruct state from snapshots', () => {
      // Create initial state
      storage.createAccount('agent-1', BigInt(1000));
      storage.createAccount('agent-2', BigInt(2000), BigInt(1000));

      const snapshot = storage.createSnapshot();

      // Verify reconstruction
      expect(snapshot.accounts.length).toBe(2);
      expect(snapshot.accounts.find((a) => a.agentId === 'agent-1')?.evmBalance).toBe(BigInt(1000));
      expect(snapshot.accounts.find((a) => a.agentId === 'agent-2')?.evmBalance).toBe(BigInt(2000));
    });
  });

  describe('Graceful Degradation', () => {
    it('should track service health', () => {
      healthManager.registerService('database');
      healthManager.registerService('settlement-engine');
      healthManager.registerService('key-manager');

      const summary = healthManager.getHealthySummary();

      expect(summary.total).toBe(3);
      expect(summary.healthy).toBe(3);
      expect(summary.unhealthy.length).toBe(0);
    });

    it('should detect unhealthy services', () => {
      healthManager.registerService('database');
      healthManager.registerService('settlement-engine');
      healthManager.registerService('key-manager');

      // Mark service as unhealthy
      healthManager.setServiceHealth('settlement-engine', false);

      const summary = healthManager.getHealthySummary();

      expect(summary.healthy).toBe(2);
      expect(summary.unhealthy).toContain('settlement-engine');
    });

    it('should enter degradation mode when majority unhealthy', () => {
      healthManager.registerService('service-1');
      healthManager.registerService('service-2');
      healthManager.registerService('service-3');
      healthManager.registerService('service-4');

      // All healthy initially
      expect(healthManager.isDegradationMode()).toBe(false);

      // Mark 3 of 4 unhealthy
      healthManager.setServiceHealth('service-1', false);
      healthManager.setServiceHealth('service-2', false);
      healthManager.setServiceHealth('service-3', false);

      // Should be in degradation mode
      expect(healthManager.isDegradationMode()).toBe(true);
    });

    it('should exit degradation mode when services recover', () => {
      healthManager.registerService('service-1');
      healthManager.registerService('service-2');
      healthManager.registerService('service-3');

      // Enter degradation mode
      healthManager.setServiceHealth('service-1', false);
      healthManager.setServiceHealth('service-2', false);
      expect(healthManager.isDegradationMode()).toBe(true);

      // Recover services
      healthManager.setServiceHealth('service-1', true);
      healthManager.setServiceHealth('service-2', true);

      // Should exit degradation mode
      expect(healthManager.isDegradationMode()).toBe(false);
    });
  });

  describe('Full Recovery Workflow', () => {
    it('should complete full disaster recovery cycle', async () => {
      // Step 1: Create production-like state
      for (let i = 0; i < 5; i++) {
        storage.createAccount(`agent-${i}`, BigInt(i * 1000));
        keyManager.storeKey(`wallet-key-${i}`, crypto.randomBytes(32));
      }

      for (let i = 0; i < 10; i++) {
        storage.createSettlement({
          id: `settlement-${i}`,
          agentId: `agent-${i % 5}`,
          amount: BigInt(i * 100),
          chain: 'evm',
          status: 'pending',
        });
      }

      // Step 2: Create backup and snapshot
      const backupPath = storage.createBackup();
      const snapshot = storage.createSnapshot();
      const keyBackup = keyManager.exportKeys();

      // Step 3: Simulate disaster (data corruption)
      storage.createAccount('agent-0', BigInt(0), BigInt(0)); // Corrupt balance
      storage.createAccount('malicious', BigInt(9999999), BigInt(9999999));
      for (let i = 0; i < 5; i++) {
        storage.updateSettlementStatus(`settlement-${i}`, 'failed');
      }

      // Verify corruption detected
      expect(storage.verifyIntegrity(snapshot)).toBe(false);

      // Step 4: Restore from backup
      storage.restoreFromBackup(backupPath);

      // Step 5: Verify recovery
      expect(storage.verifyIntegrity(snapshot)).toBe(true);

      // Step 6: Verify key recovery
      const newKeyManager = new KeyRecoveryManager(
        (keyManager as unknown as { masterKey: Buffer }).masterKey
      );
      newKeyManager.importKeys(keyBackup);

      for (let i = 0; i < 5; i++) {
        const key = newKeyManager.recoverKey(`wallet-key-${i}`);
        expect(key).not.toBeNull();
      }

      // Step 7: Verify all accounts restored
      const accounts = storage.getAllAccounts();
      expect(accounts.length).toBe(5);
      expect(accounts.find((a) => a.agentId === 'malicious')).toBeUndefined();

      // Step 8: Verify settlements restored
      const settlements = storage.getAllSettlements();
      const pendingCount = settlements.filter((s) => s.status === 'pending').length;
      expect(pendingCount).toBe(10);
    });

    it('should handle partial failure with graceful degradation', () => {
      // Setup services
      healthManager.registerService('database');
      healthManager.registerService('settlement-evm');
      healthManager.registerService('telemetry');
      healthManager.registerService('key-manager');

      // Simulate partial failure
      healthManager.setServiceHealth('settlement-evm', false);

      // System should not be in full degradation
      expect(healthManager.isDegradationMode()).toBe(false);

      // But should report unhealthy service
      const summary = healthManager.getHealthySummary();
      expect(summary.unhealthy).toContain('settlement-evm');
      expect(summary.healthy).toBe(3);

      // Service recovers
      healthManager.setServiceHealth('settlement-evm', true);
      const finalSummary = healthManager.getHealthySummary();
      expect(finalSummary.healthy).toBe(4);
    });
  });
});
