/**
 * Integration tests for TigerBeetleClient with real TigerBeetle container
 *
 * These tests connect to a real TigerBeetle container and perform actual operations.
 * Requires Docker and docker-compose to be running.
 */

import { TigerBeetleClient } from '../../src/settlement/tigerbeetle-client';
import {
  TigerBeetleAccountError,
  TigerBeetleTransferError,
} from '../../src/settlement/tigerbeetle-errors';
import pino from 'pino';
import { exec } from 'child_process';
import { promisify } from 'util';

const execAsync = promisify(exec);

// Integration test timeout - 1 minute for TigerBeetle operations
jest.setTimeout(60000);

// Skip tests unless E2E_TESTS is enabled (requires TigerBeetle container)
const e2eEnabled = process.env.E2E_TESTS === 'true';
const describeIfE2E = e2eEnabled ? describe : describe.skip;

/* eslint-disable no-console */
describeIfE2E('TigerBeetleClient Integration Tests', () => {
  let client: TigerBeetleClient;
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

    // Create logger for client
    logger = pino({ level: 'silent' }); // Silent for tests

    // Note: TigerBeetle container should be started separately via docker-compose
    // Port 3000 is not exposed to host by default (security measure)
    //
    // To run these tests:
    // 1. Temporarily expose TigerBeetle port in docker-compose.yml:
    //    Add under tigerbeetle service:
    //      ports:
    //        - "3000:3000"
    // 2. Start TigerBeetle: docker-compose up -d tigerbeetle
    // 3. Run tests: npm test -- tigerbeetle-client.test.ts
    //
    // For CI/CD: Tests will skip if TigerBeetle container not accessible

    // Try to initialize TigerBeetle client
    // Use localhost:3000 since test runs outside Docker network
    try {
      client = new TigerBeetleClient(
        {
          clusterId: 0,
          replicaAddresses: ['127.0.0.1:3000'],
          connectionTimeout: 5000,
          operationTimeout: 5000,
        },
        logger
      );

      // Initialize with Promise.race to ensure it times out quickly
      await Promise.race([
        client.initialize(),
        new Promise((_, reject) => setTimeout(() => reject(new Error('Connection timeout')), 5000)),
      ]);
      console.log('TigerBeetle client initialized successfully');
    } catch (error) {
      console.log('TigerBeetle container not accessible. Integration tests will be skipped.');
      console.log(
        'To run integration tests, ensure TigerBeetle container is running with port 3000 exposed.'
      );
      // Reset client to undefined so tests will skip
      client = undefined as unknown as TigerBeetleClient;
    }
  });

  afterAll(async () => {
    // Close client connection
    if (client) {
      await client.close();
    }
  });

  it('should connect to TigerBeetle container', async () => {
    // Skip if client not initialized (TigerBeetle not accessible)
    if (!client) {
      console.log('Skipping test: TigerBeetle container not accessible');
      return;
    }

    // Client should be initialized successfully in beforeAll
    expect(client).toBeDefined();
  });

  it('should create account in TigerBeetle', async () => {
    if (!client) {
      console.log('Skipping test: TigerBeetle container not accessible');
      return;
    }

    // Generate unique account ID using timestamp to avoid conflicts
    const accountId = BigInt(Date.now()) * 1000n + 1n;

    await client.createAccount(accountId, 1, 100);

    // Verify account exists by querying balance
    const balance = await client.getAccountBalance(accountId);
    expect(balance.debits).toBe(0n);
    expect(balance.credits).toBe(0n);
    expect(balance.balance).toBe(0n);
  });

  it('should create transfer between accounts', async () => {
    if (!client) {
      console.log('Skipping test: TigerBeetle container not accessible');
      return;
    }

    // Generate unique account IDs
    const timestamp = BigInt(Date.now());
    const debitAccountId = timestamp * 1000n + 2n;
    const creditAccountId = timestamp * 1000n + 3n;
    const transferId = timestamp * 1000n + 4n;

    // Create both accounts
    await client.createAccount(debitAccountId, 1, 100);
    await client.createAccount(creditAccountId, 1, 100);

    // Create transfer: debit account -> credit account, amount 1000
    await client.createTransfer(transferId, debitAccountId, creditAccountId, 1000n, 1, 100);

    // Query balances after transfer
    const debitBalance = await client.getAccountBalance(debitAccountId);
    const creditBalance = await client.getAccountBalance(creditAccountId);

    // Debit account should have negative balance (debits > credits)
    expect(debitBalance.debits).toBe(1000n);
    expect(debitBalance.credits).toBe(0n);
    expect(debitBalance.balance).toBe(-1000n);

    // Credit account should have positive balance (credits > debits)
    expect(creditBalance.debits).toBe(0n);
    expect(creditBalance.credits).toBe(1000n);
    expect(creditBalance.balance).toBe(1000n);
  });

  it('should query account balances', async () => {
    if (!client) {
      console.log('Skipping test: TigerBeetle container not accessible');
      return;
    }

    // Generate unique account IDs
    const timestamp = BigInt(Date.now());
    const accountId = timestamp * 1000n + 5n;
    const counterpartyId = timestamp * 1000n + 6n;
    const transferId = timestamp * 1000n + 7n;

    // Create account and counterparty
    await client.createAccount(accountId, 1, 100);
    await client.createAccount(counterpartyId, 1, 100);

    // Post transfer to account
    await client.createTransfer(transferId, counterpartyId, accountId, 5000n, 1, 100);

    // Query balance
    const balance = await client.getAccountBalance(accountId);
    expect(balance.credits).toBe(5000n);
    expect(balance.debits).toBe(0n);
    expect(balance.balance).toBe(5000n);
  });

  it('should handle duplicate account creation gracefully', async () => {
    if (!client) {
      console.log('Skipping test: TigerBeetle container not accessible');
      return;
    }

    const accountId = BigInt(Date.now()) * 1000n + 8n;

    // Create account first time
    await client.createAccount(accountId, 1, 100);

    // Attempt to create same account again
    await expect(client.createAccount(accountId, 1, 100)).rejects.toThrow(TigerBeetleAccountError);
  });

  it('should handle transfer with insufficient balance', async () => {
    if (!client) {
      console.log('Skipping test: TigerBeetle container not accessible');
      return;
    }

    // Generate unique account IDs
    const timestamp = BigInt(Date.now());

    // Note: TigerBeetle allows negative balances by default
    // We need to test with debits_must_not_exceed_credits flag

    // Attempt transfer with insufficient balance (account has 0 balance)
    // Note: TigerBeetle allows negative balances by default unless
    // debits_must_not_exceed_credits flag is set
    // For this test, we'll create account with the flag
    const restrictedAccountId = timestamp * 1000n + 12n;
    const restrictedCreditAccountId = timestamp * 1000n + 13n;
    const restrictedTransferId = timestamp * 1000n + 14n;

    // Import AccountFlags
    // eslint-disable-next-line @typescript-eslint/no-var-requires
    const { AccountFlags } = require('tigerbeetle-node');

    // Create account with credit limit enforcement
    await client.createAccount(
      restrictedAccountId,
      1,
      100,
      AccountFlags.debits_must_not_exceed_credits
    );
    await client.createAccount(restrictedCreditAccountId, 1, 100);

    // This should fail because account has 0 credits and cannot debit
    await expect(
      client.createTransfer(
        restrictedTransferId,
        restrictedAccountId,
        restrictedCreditAccountId,
        1000n,
        1,
        100
      )
    ).rejects.toThrow(TigerBeetleTransferError);
  });

  it('should query multiple accounts in batch', async () => {
    if (!client) {
      console.log('Skipping test: TigerBeetle container not accessible');
      return;
    }

    // Generate unique account IDs
    const timestamp = BigInt(Date.now());
    const account1Id = timestamp * 1000n + 15n;
    const account2Id = timestamp * 1000n + 16n;
    const transferId = timestamp * 1000n + 17n;

    // Create accounts
    await client.createAccount(account1Id, 1, 100);
    await client.createAccount(account2Id, 1, 100);

    // Create transfer between accounts
    await client.createTransfer(transferId, account1Id, account2Id, 3000n, 1, 100);

    // Query both accounts in batch
    const balances = await client.getAccountsBatch([account1Id, account2Id]);

    expect(balances.size).toBe(2);
    expect(balances.get(account1Id)?.balance).toBe(-3000n);
    expect(balances.get(account2Id)?.balance).toBe(3000n);
  });

  it('should handle account not found in balance query', async () => {
    if (!client) {
      console.log('Skipping test: TigerBeetle container not accessible');
      return;
    }

    // Query non-existent account
    const nonExistentId = BigInt(Date.now()) * 1000n + 999999n;

    await expect(client.getAccountBalance(nonExistentId)).rejects.toThrow(TigerBeetleAccountError);
  });
});
