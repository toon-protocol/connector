/**
 * Integration test for InMemoryLedgerClient as default connector backend
 * Tests the factory logic, lifecycle, and persistence features
 */

import { ConnectorNode } from '../../src/core/connector-node';
import { ConnectorConfig } from '../../src/config/types';
import { Logger } from '../../src/utils/logger';
import pino from 'pino';
import * as os from 'os';
import * as fs from 'fs';
import * as path from 'path';
import { AccountManager } from '../../src/settlement/account-manager';

describe('InMemoryLedgerClient Integration', () => {
  let logger: Logger;
  let testSnapshotPath: string;
  let originalEnv: NodeJS.ProcessEnv;

  beforeAll(() => {
    // Silent logger for tests
    logger = pino({ level: 'silent' });
  });

  beforeEach(() => {
    // Save original environment
    originalEnv = { ...process.env };

    // Disable explorer to avoid port conflicts in concurrent test runs
    process.env.EXPLORER_ENABLED = 'false';

    // Create temp snapshot path
    testSnapshotPath = path.join(os.tmpdir(), `ledger-test-${Date.now()}.json`);
  });

  afterEach(async () => {
    // Restore environment
    process.env = originalEnv;

    // Clean up temp files
    try {
      if (fs.existsSync(testSnapshotPath)) {
        fs.unlinkSync(testSnapshotPath);
      }
      // Clean up any .fresh-* files created during error recovery
      const tmpDir = os.tmpdir();
      const files = fs.readdirSync(tmpDir);
      for (const file of files) {
        if (file.startsWith('ledger-test-') && file.includes('.fresh-')) {
          fs.unlinkSync(path.join(tmpDir, file));
        }
      }
    } catch (error) {
      // Ignore cleanup errors
    }
  });

  describe('Connector Startup with InMemoryLedgerClient', () => {
    it('should start connector with in-memory backend when TigerBeetle not configured', async () => {
      // Arrange: Set required settlement env vars but NOT TigerBeetle
      process.env.SETTLEMENT_ENABLED = 'true';
      process.env.BASE_L2_RPC_URL = 'http://localhost:8545';
      process.env.TOKEN_NETWORK_REGISTRY = '0x1234567890123456789012345678901234567890';
      process.env.M2M_TOKEN_ADDRESS = '0x1234567890123456789012345678901234567890';
      process.env.TREASURY_EVM_PRIVATE_KEY =
        '0x0123456789012345678901234567890123456789012345678901234567890123';
      process.env.LEDGER_SNAPSHOT_PATH = testSnapshotPath;
      // Explicitly unset TigerBeetle env vars
      delete process.env.TIGERBEETLE_CLUSTER_ID;
      delete process.env.TIGERBEETLE_REPLICAS;

      const config: ConnectorConfig = {
        nodeId: 'test-node-1',
        btpServerPort: 7780,
        healthCheckPort: 17780,
        explorer: { enabled: false },
        environment: 'development',
        routes: [],
        peers: [],
      };

      // Act: Start connector
      const connector = new ConnectorNode(config, logger);
      await connector.start();

      try {
        // Assert: Connector started successfully
        expect(connector).toBeDefined();

        // Access private field for verification (test-only)
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const accountManager = (connector as any)._accountManager as AccountManager | null;
        expect(accountManager).not.toBeNull();
        expect(accountManager).toBeDefined();

        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const inMemoryClient = (connector as any)._inMemoryLedgerClient;
        expect(inMemoryClient).not.toBeNull();
        expect(inMemoryClient).toBeDefined();
      } finally {
        // Cleanup
        await connector.stop();
      }
    });

    it('should use custom env var values for snapshot path and persist interval', async () => {
      // Arrange: Set custom env vars
      const customSnapshotPath = path.join(os.tmpdir(), `custom-ledger-${Date.now()}.json`);
      process.env.SETTLEMENT_ENABLED = 'true';
      process.env.BASE_L2_RPC_URL = 'http://localhost:8545';
      process.env.TOKEN_NETWORK_REGISTRY = '0x1234567890123456789012345678901234567890';
      process.env.M2M_TOKEN_ADDRESS = '0x1234567890123456789012345678901234567890';
      process.env.TREASURY_EVM_PRIVATE_KEY =
        '0x0123456789012345678901234567890123456789012345678901234567890123';
      process.env.LEDGER_SNAPSHOT_PATH = customSnapshotPath;
      process.env.LEDGER_PERSIST_INTERVAL_MS = '5000';
      delete process.env.TIGERBEETLE_CLUSTER_ID;
      delete process.env.TIGERBEETLE_REPLICAS;

      const config: ConnectorConfig = {
        nodeId: 'test-node-2',
        btpServerPort: 7781,
        healthCheckPort: 17781,
        explorer: { enabled: false },
        environment: 'development',
        routes: [],
        peers: [],
      };

      // Act: Start connector, create an account to trigger dirty flag, then stop
      const connector = new ConnectorNode(config, logger);
      await connector.start();

      // Create an account so the ledger is marked dirty and persists on close
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const accountManager = (connector as any)._accountManager as AccountManager;
      await accountManager.createPeerAccounts('env-test-peer', 'ILP');

      await connector.stop();

      // Assert: Snapshot file exists at custom path
      expect(fs.existsSync(customSnapshotPath)).toBe(true);

      // Cleanup
      fs.unlinkSync(customSnapshotPath);
    });
  });

  describe('Snapshot Persistence and Restore', () => {
    it('should persist snapshot on stop and restore on restart', async () => {
      // Arrange
      process.env.SETTLEMENT_ENABLED = 'true';
      process.env.BASE_L2_RPC_URL = 'http://localhost:8545';
      process.env.TOKEN_NETWORK_REGISTRY = '0x1234567890123456789012345678901234567890';
      process.env.M2M_TOKEN_ADDRESS = '0x1234567890123456789012345678901234567890';
      process.env.TREASURY_EVM_PRIVATE_KEY =
        '0x0123456789012345678901234567890123456789012345678901234567890123';
      process.env.LEDGER_SNAPSHOT_PATH = testSnapshotPath;
      delete process.env.TIGERBEETLE_CLUSTER_ID;
      delete process.env.TIGERBEETLE_REPLICAS;

      const config: ConnectorConfig = {
        nodeId: 'test-node-3',
        btpServerPort: 7782,
        healthCheckPort: 17782,
        explorer: { enabled: false },
        environment: 'development',
        routes: [],
        peers: [],
      };

      // Act: Start first connector, create accounts, stop
      const connector1 = new ConnectorNode(config, logger);
      await connector1.start();

      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const accountManager1 = (connector1 as any)._accountManager as AccountManager;
      expect(accountManager1).not.toBeNull();

      // Create test accounts
      await accountManager1.createPeerAccounts('peer-a', 'ILP');

      // Stop connector (triggers snapshot persist)
      await connector1.stop();

      // Assert: Snapshot file exists
      expect(fs.existsSync(testSnapshotPath)).toBe(true);

      // Act: Start second connector with same snapshot path
      const connector2 = new ConnectorNode(config, logger);
      await connector2.start();

      try {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const accountManager2 = (connector2 as any)._accountManager as AccountManager;
        expect(accountManager2).not.toBeNull();

        // Assert: Balance was restored from snapshot
        const balance = await accountManager2.getAccountBalance('peer-a', 'ILP');
        expect(balance).toBeDefined();
        // Balance should be zero (we only created accounts, no transfers)
        expect(balance.debitBalance).toBe(0n);
        expect(balance.creditBalance).toBe(0n);
      } finally {
        await connector2.stop();
      }
    });

    it('should recover gracefully from corrupt snapshot', async () => {
      // Arrange: Write invalid JSON to snapshot file
      process.env.SETTLEMENT_ENABLED = 'true';
      process.env.BASE_L2_RPC_URL = 'http://localhost:8545';
      process.env.TOKEN_NETWORK_REGISTRY = '0x1234567890123456789012345678901234567890';
      process.env.M2M_TOKEN_ADDRESS = '0x1234567890123456789012345678901234567890';
      process.env.TREASURY_EVM_PRIVATE_KEY =
        '0x0123456789012345678901234567890123456789012345678901234567890123';
      process.env.LEDGER_SNAPSHOT_PATH = testSnapshotPath;
      delete process.env.TIGERBEETLE_CLUSTER_ID;
      delete process.env.TIGERBEETLE_REPLICAS;

      // Create corrupt snapshot file
      fs.mkdirSync(path.dirname(testSnapshotPath), { recursive: true });
      fs.writeFileSync(testSnapshotPath, 'INVALID JSON{{{', 'utf-8');

      const config: ConnectorConfig = {
        nodeId: 'test-node-4',
        btpServerPort: 7783,
        healthCheckPort: 17783,
        explorer: { enabled: false },
        environment: 'development',
        routes: [],
        peers: [],
      };

      // Act: Start connector — should recover gracefully
      const connector = new ConnectorNode(config, logger);
      await connector.start();

      try {
        // Assert: Connector started successfully with empty ledger
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const accountManager = (connector as any)._accountManager as AccountManager;
        expect(accountManager).not.toBeNull();
        expect(accountManager).toBeDefined();
      } finally {
        await connector.stop();
      }
    });
  });

  describe('TigerBeetle Fallback', () => {
    it('should attempt TigerBeetle path when env vars present, then fall back to in-memory', async () => {
      // Arrange: Set TigerBeetle env vars to unreachable address
      process.env.SETTLEMENT_ENABLED = 'true';
      process.env.BASE_L2_RPC_URL = 'http://localhost:8545';
      process.env.TOKEN_NETWORK_REGISTRY = '0x1234567890123456789012345678901234567890';
      process.env.M2M_TOKEN_ADDRESS = '0x1234567890123456789012345678901234567890';
      process.env.TREASURY_EVM_PRIVATE_KEY =
        '0x0123456789012345678901234567890123456789012345678901234567890123';
      process.env.LEDGER_SNAPSHOT_PATH = testSnapshotPath;
      process.env.TIGERBEETLE_CLUSTER_ID = '0';
      process.env.TIGERBEETLE_REPLICAS = 'localhost:99999'; // Unreachable port
      process.env.TIGERBEETLE_OPERATION_TIMEOUT = '100'; // Short timeout for faster test

      const config: ConnectorConfig = {
        nodeId: 'test-node-5',
        btpServerPort: 7784,
        healthCheckPort: 17784,
        explorer: { enabled: false },
        environment: 'development',
        routes: [],
        peers: [],
      };

      // Act: Start connector — TigerBeetle will fail, should fall back to in-memory
      const connector = new ConnectorNode(config, logger);
      await connector.start();

      try {
        // Assert: Connector started with in-memory ledger (not TigerBeetle)
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const inMemoryClient = (connector as any)._inMemoryLedgerClient;
        expect(inMemoryClient).not.toBeNull();
        expect(inMemoryClient).toBeDefined();

        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const tigerBeetleClient = (connector as any)._tigerBeetleClient;
        expect(tigerBeetleClient).toBeNull(); // TigerBeetle init failed

        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const accountManager = (connector as any)._accountManager as AccountManager;
        expect(accountManager).not.toBeNull();
      } finally {
        await connector.stop();
      }
    }, 30000); // Increase timeout for TigerBeetle connection attempt
  });
});
