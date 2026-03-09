/**
 * Integration tests for AccountManager with real TigerBeetle container
 *
 * These tests connect to a real TigerBeetle container and perform actual account operations.
 * Requires Docker and docker-compose to be running with TigerBeetle port exposed.
 *
 * To run these tests:
 * 1. Temporarily expose TigerBeetle port in docker-compose.yml:
 *    Add under tigerbeetle service:
 *      ports:
 *        - "3000:3000"
 * 2. Start TigerBeetle: docker-compose up -d tigerbeetle
 * 3. Run tests: npm test -- account-manager.test.ts
 */

import { AccountManager } from '../../src/settlement/account-manager';
import { TigerBeetleClient } from '../../src/settlement/tigerbeetle-client';
import pino from 'pino';
import { exec } from 'child_process';
import { promisify } from 'util';

const execAsync = promisify(exec);

// Integration test timeout - 2 minutes for TigerBeetle operations
jest.setTimeout(120000);

// Skip tests unless E2E_TESTS is enabled (requires TigerBeetle container)
const e2eEnabled = process.env.E2E_TESTS === 'true';
const describeIfE2E = e2eEnabled ? describe : describe.skip;

/* eslint-disable no-console */
describeIfE2E('AccountManager Integration Tests', () => {
  let accountManager: AccountManager;
  let tigerBeetleClient: TigerBeetleClient;
  let logger: pino.Logger;

  /**
   * Check if Docker is available
   */
  const isDockerAvailable = async (): Promise<boolean> => {
    try {
      await execAsync('docker --version');
      return true;
    } catch {
      return false;
    }
  };

  beforeAll(async () => {
    // Skip tests if Docker not available
    const dockerAvailable = await isDockerAvailable();
    if (!dockerAvailable) {
      console.log('Docker not available, skipping integration tests');
      return;
    }

    // Create logger for client (silent for tests)
    logger = pino({ level: 'silent' });

    // Try to initialize TigerBeetle client
    // Use 127.0.0.1:3000 since test runs outside Docker network
    try {
      tigerBeetleClient = new TigerBeetleClient(
        {
          clusterId: 0,
          replicaAddresses: ['127.0.0.1:3000'],
          connectionTimeout: 5000,
          operationTimeout: 5000,
        },
        logger
      );

      // Initialize with timeout
      await Promise.race([
        tigerBeetleClient.initialize(),
        new Promise((_, reject) => setTimeout(() => reject(new Error('Connection timeout')), 5000)),
      ]);

      // Initialize AccountManager with timestamp-based node ID for uniqueness across test runs
      const nodeId = `test-node-${Date.now()}`;
      accountManager = new AccountManager({ nodeId }, tigerBeetleClient, logger);

      console.log('TigerBeetle client and AccountManager initialized successfully');
    } catch (error) {
      console.log('TigerBeetle container not accessible. Integration tests will be skipped.');
      console.log(
        'To run integration tests, ensure TigerBeetle container is running with port 3000 exposed.'
      );
      // Reset to undefined so tests will skip
      tigerBeetleClient = undefined as unknown as TigerBeetleClient;
      accountManager = undefined as unknown as AccountManager;
    }
  });

  afterAll(async () => {
    // Close client connection
    if (tigerBeetleClient) {
      await tigerBeetleClient.close();
    }
  });

  describe('Account Creation', () => {
    it('should create accounts for 10 peers with different tokens', async () => {
      if (!accountManager) {
        console.log('Skipping test - TigerBeetle not available');
        return;
      }

      // Define 10 test peers
      const peers = Array.from({ length: 10 }, (_, i) => `peer-${i + 1}`);
      // Define 2 test tokens
      const tokens = ['USD', 'ETH'];

      const accountPairs: Array<{
        peerId: string;
        tokenId: string;
        debitAccountId: bigint;
        creditAccountId: bigint;
      }> = [];

      // Create accounts for all peer-token combinations (20 total pairs = 40 accounts)
      for (const peerId of peers) {
        for (const tokenId of tokens) {
          const accountPair = await accountManager.createPeerAccounts(peerId, tokenId);

          expect(accountPair).toBeDefined();
          expect(accountPair.peerId).toBe(peerId);
          expect(accountPair.tokenId).toBe(tokenId);
          expect(accountPair.debitAccountId).toBeGreaterThan(0n);
          expect(accountPair.creditAccountId).toBeGreaterThan(0n);

          accountPairs.push(accountPair);
        }
      }

      // Verify 20 account pairs created (40 total accounts)
      expect(accountPairs.length).toBe(20);

      // Verify all account IDs are unique
      const allAccountIds = accountPairs.flatMap((pair) => [
        pair.debitAccountId,
        pair.creditAccountId,
      ]);
      const uniqueAccountIds = new Set(allAccountIds);
      expect(uniqueAccountIds.size).toBe(40);
    });

    it('should verify all balances are initialized to zero', async () => {
      if (!accountManager) {
        console.log('Skipping test - TigerBeetle not available');
        return;
      }

      const peers = Array.from({ length: 10 }, (_, i) => `peer-${i + 1}`);
      const tokens = ['USD', 'ETH'];

      // Query balances for all peer-token combinations
      for (const peerId of peers) {
        for (const tokenId of tokens) {
          const balance = await accountManager.getAccountBalance(peerId, tokenId);

          expect(balance.debitBalance).toBe(0n);
          expect(balance.creditBalance).toBe(0n);
          expect(balance.netBalance).toBe(0n);
        }
      }
    });
  });

  describe('Idempotent Account Creation', () => {
    it('should handle idempotent account creation', async () => {
      if (!accountManager) {
        console.log('Skipping test - TigerBeetle not available');
        return;
      }

      const peerId = 'peer-idempotent';
      const tokenId = 'USD';

      // Create accounts for the first time
      const accountPair1 = await accountManager.createPeerAccounts(peerId, tokenId);

      expect(accountPair1).toBeDefined();
      expect(accountPair1.peerId).toBe(peerId);
      expect(accountPair1.tokenId).toBe(tokenId);

      // Create accounts again with same parameters (should be idempotent)
      const accountPair2 = await accountManager.createPeerAccounts(peerId, tokenId);

      // Should NOT throw error
      expect(accountPair2).toBeDefined();

      // Should return same account IDs (deterministic generation)
      expect(accountPair2.debitAccountId).toBe(accountPair1.debitAccountId);
      expect(accountPair2.creditAccountId).toBe(accountPair1.creditAccountId);
    });
  });

  describe('Cache Performance', () => {
    it('should use cache for repeated queries', async () => {
      if (!accountManager) {
        console.log('Skipping test - TigerBeetle not available');
        return;
      }

      const peerId = 'peer-cache-test';
      const tokenId = 'BTC';

      // Create accounts (populates cache)
      await accountManager.createPeerAccounts(peerId, tokenId);

      // Verify cache populated
      expect(accountManager.getCacheStats().size).toBeGreaterThan(0);

      // Get account pair 5 times
      const accountPairs = [];
      for (let i = 0; i < 5; i++) {
        const accountPair = accountManager.getPeerAccountPair(peerId, tokenId);
        accountPairs.push(accountPair);
      }

      // Verify all calls returned same account IDs (from cache)
      const firstPair = accountPairs[0];
      for (const pair of accountPairs) {
        expect(pair.debitAccountId).toBe(firstPair!.debitAccountId);
        expect(pair.creditAccountId).toBe(firstPair!.creditAccountId);
      }

      // Verify cache performance: should return same object reference
      expect(accountPairs[0]).toBe(accountPairs[1]);
      expect(accountPairs[1]).toBe(accountPairs[2]);
    });

    it('should regenerate account IDs after cache clear', async () => {
      if (!accountManager) {
        console.log('Skipping test - TigerBeetle not available');
        return;
      }

      const peerId = 'peer-cache-clear';
      const tokenId = 'EUR';

      // Create accounts
      const originalPair = await accountManager.createPeerAccounts(peerId, tokenId);

      // Clear cache
      accountManager.clearCache();
      expect(accountManager.getCacheStats().size).toBe(0);

      // Get account pair (should regenerate deterministically)
      const regeneratedPair = accountManager.getPeerAccountPair(peerId, tokenId);

      // Should be same IDs (deterministic)
      expect(regeneratedPair.debitAccountId).toBe(originalPair.debitAccountId);
      expect(regeneratedPair.creditAccountId).toBe(originalPair.creditAccountId);

      // But should be a new object reference
      expect(regeneratedPair).not.toBe(originalPair);
    });
  });

  describe('Multiple Token Support', () => {
    it('should create separate account pairs for different tokens', async () => {
      if (!accountManager) {
        console.log('Skipping test - TigerBeetle not available');
        return;
      }

      const peerId = 'peer-multi-token';
      const tokens = ['USD', 'ETH', 'BTC', 'EUR', 'JPY'];

      const accountPairs = [];
      for (const tokenId of tokens) {
        const accountPair = await accountManager.createPeerAccounts(peerId, tokenId);
        accountPairs.push(accountPair);
      }

      // Verify 5 separate account pairs created (10 total accounts)
      expect(accountPairs.length).toBe(5);

      // Verify all account IDs are unique
      const allAccountIds = accountPairs.flatMap((pair) => [
        pair.debitAccountId,
        pair.creditAccountId,
      ]);
      const uniqueAccountIds = new Set(allAccountIds);
      expect(uniqueAccountIds.size).toBe(10);

      // Verify balances are independent
      for (const tokenId of tokens) {
        const balance = await accountManager.getAccountBalance(peerId, tokenId);
        expect(balance.debitBalance).toBe(0n);
        expect(balance.creditBalance).toBe(0n);
        expect(balance.netBalance).toBe(0n);
      }
    });
  });

  describe('Real World Scenario', () => {
    it('should handle realistic peer network topology', async () => {
      if (!accountManager) {
        console.log('Skipping test - TigerBeetle not available');
        return;
      }

      // Simulate a 5-node connector network with 3 tokens
      const connectorPeers = [
        'connector-a',
        'connector-b',
        'connector-c',
        'connector-d',
        'connector-e',
      ];
      const supportedTokens = ['USD', 'EUR', 'BTC'];

      // Create accounts for all peer-token combinations
      const creationResults = [];
      for (const peerId of connectorPeers) {
        for (const tokenId of supportedTokens) {
          const accountPair = await accountManager.createPeerAccounts(peerId, tokenId);
          creationResults.push({ peerId, tokenId, accountPair });
        }
      }

      // Verify 15 account pairs created (5 peers × 3 tokens = 30 total accounts)
      expect(creationResults.length).toBe(15);

      // Verify cache contains all 15 entries
      expect(accountManager.getCacheStats().size).toBeGreaterThanOrEqual(15);

      // Verify we can query balances for any peer-token combination
      const randomPeer = connectorPeers[2]!; // connector-c
      const randomToken = supportedTokens[1]!; // EUR

      const balance = await accountManager.getAccountBalance(randomPeer, randomToken);
      expect(balance).toBeDefined();
      expect(balance.netBalance).toBe(0n);
    });
  });
});
