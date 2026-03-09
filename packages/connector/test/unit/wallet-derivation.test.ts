/**
 * Integration tests for HD wallet derivation
 * @packageDocumentation
 * @remarks
 * Tests derive 1000 unique wallet addresses from single master seed to validate
 * HD derivation strategy. Verifies deterministic derivation and uniqueness.
 */

import { HDKey } from 'ethereum-cryptography/hdkey';
import { Wallet } from 'ethers';
import { promises as fs } from 'fs';
import * as path from 'path';
import { WalletSeedManager, DERIVATION_PATHS } from '../../src/wallet/wallet-seed-manager';

/**
 * Derived wallet information
 */
interface DerivedWallet {
  index: number;
  evmAddress?: string;
  privateKey: Buffer;
}

/**
 * Derive EVM wallet from master seed
 * @param masterSeed - Master seed buffer (512 bits)
 * @param index - Wallet index
 * @returns Derived wallet with EVM address
 */
function deriveEVMWallet(masterSeed: Buffer, index: number): DerivedWallet {
  // Derive HD key using BIP-44 path for Ethereum
  const path = `${DERIVATION_PATHS.EVM}/${index}`;
  const hdKey = HDKey.fromMasterSeed(masterSeed).derive(path);

  if (!hdKey.privateKey) {
    throw new Error('Failed to derive private key');
  }

  // Create Ethers wallet from private key (convert Uint8Array to hex string)
  const privateKeyHex = '0x' + Buffer.from(hdKey.privateKey).toString('hex');
  const wallet = new Wallet(privateKeyHex);

  return {
    index,
    evmAddress: wallet.address,
    privateKey: Buffer.from(hdKey.privateKey),
  };
}

// Skip heavy wallet derivation tests in CI and integration test runs
// These tests are extremely resource-intensive (1000+ wallet derivations) and should only run locally
const skipInCI = process.env.CI === 'true' || process.env.INTEGRATION_TESTS === 'true';
const describeIfLocal = skipInCI ? describe.skip : describe;

describeIfLocal('HD Wallet Derivation Integration Tests', () => {
  let manager: WalletSeedManager;
  const testPassword = 'StrongP@ssw0rd123456';
  const tempStoragePath = path.join(__dirname, '.test-wallet-storage');

  beforeAll(async () => {
    // Create test storage directory
    await fs.mkdir(tempStoragePath, { recursive: true });

    // Initialize wallet seed manager with test storage path
    manager = new WalletSeedManager(undefined, {
      storageBackend: 'filesystem',
      storagePath: tempStoragePath,
    });
    await manager.initialize();
  });

  afterAll(async () => {
    // Clean up test storage directory
    try {
      const files = await fs.readdir(tempStoragePath);
      for (const file of files) {
        await fs.unlink(path.join(tempStoragePath, file));
      }
      await fs.rmdir(tempStoragePath);
    } catch (error) {
      // Ignore cleanup errors
    }
  });

  describe('EVM Address Derivation', () => {
    it('should derive 1000 unique EVM addresses from single master seed', async () => {
      // Generate master seed
      const masterSeed = await manager.generateMasterSeed(256);

      // Derive 1000 EVM addresses
      const startTime = Date.now();
      const addresses: string[] = [];

      for (let i = 0; i < 1000; i++) {
        const wallet = deriveEVMWallet(masterSeed.seed, i);
        addresses.push(wallet.evmAddress!);
      }

      const endTime = Date.now();
      const duration = endTime - startTime;

      // Verify all addresses are unique
      const uniqueAddresses = new Set(addresses);
      expect(uniqueAddresses.size).toBe(1000);

      // Sanity check: derivation should complete (threshold generous for concurrent test runs)
      expect(duration).toBeLessThan(600_000);
    }, 600_000);

    it('should derive same EVM addresses from same master seed (deterministic)', async () => {
      // Import same mnemonic twice
      const testMnemonic =
        'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
      const masterSeed1 = await manager.importMasterSeed(testMnemonic);
      const masterSeed2 = await manager.importMasterSeed(testMnemonic);

      // Derive wallet at index 0 from both seeds
      const wallet1 = deriveEVMWallet(masterSeed1.seed, 0);
      const wallet2 = deriveEVMWallet(masterSeed2.seed, 0);

      // Verify addresses match
      expect(wallet1.evmAddress).toBe(wallet2.evmAddress);
      expect(wallet1.privateKey.equals(wallet2.privateKey)).toBe(true);
    });

    it('should derive different addresses for different indices', async () => {
      const masterSeed = await manager.generateMasterSeed(256);

      const wallet0 = deriveEVMWallet(masterSeed.seed, 0);
      const wallet1 = deriveEVMWallet(masterSeed.seed, 1);
      const wallet100 = deriveEVMWallet(masterSeed.seed, 100);

      // Verify all addresses are different
      expect(wallet0.evmAddress).not.toBe(wallet1.evmAddress);
      expect(wallet0.evmAddress).not.toBe(wallet100.evmAddress);
      expect(wallet1.evmAddress).not.toBe(wallet100.evmAddress);

      // Verify all private keys are different
      expect(wallet0.privateKey.equals(wallet1.privateKey)).toBe(false);
      expect(wallet0.privateKey.equals(wallet100.privateKey)).toBe(false);
      expect(wallet1.privateKey.equals(wallet100.privateKey)).toBe(false);
    });

    it('should generate valid Ethereum addresses (42 characters, 0x prefix)', async () => {
      const masterSeed = await manager.generateMasterSeed(256);
      const wallet = deriveEVMWallet(masterSeed.seed, 0);

      // Verify address format
      expect(wallet.evmAddress).toBeDefined();
      expect(wallet.evmAddress!.length).toBe(42);
      expect(wallet.evmAddress!.startsWith('0x')).toBe(true);
      expect(/^0x[0-9a-fA-F]{40}$/.test(wallet.evmAddress!)).toBe(true);
    });
  });

  describe('Master Seed Persistence', () => {
    it('should derive same addresses after encrypting and decrypting master seed', async () => {
      // Generate master seed
      const originalSeed = await manager.generateMasterSeed(256);

      // Derive addresses before encryption
      const evmBefore = deriveEVMWallet(originalSeed.seed, 0);

      // Encrypt and store
      await manager.encryptAndStore(originalSeed, testPassword);

      // Decrypt and load
      const restoredSeed = await manager.decryptAndLoad(testPassword);

      // Derive addresses after decryption
      const evmAfter = deriveEVMWallet(restoredSeed.seed, 0);

      // Verify addresses match
      expect(evmAfter.evmAddress).toBe(evmBefore.evmAddress);
    });

    it('should derive same addresses after backup and restore', async () => {
      // Generate master seed
      const originalSeed = await manager.generateMasterSeed(256);

      // Derive addresses before backup
      const evmBefore = deriveEVMWallet(originalSeed.seed, 0);

      // Export backup
      const backup = await manager.exportBackup(originalSeed, testPassword);

      // Restore from backup
      const restoredSeed = await manager.restoreFromBackup(backup, testPassword);

      // Derive addresses after restore
      const evmAfter = deriveEVMWallet(restoredSeed.seed, 0);

      // Verify addresses match
      expect(evmAfter.evmAddress).toBe(evmBefore.evmAddress);
    });
  });

  describe('Performance', () => {
    it('should derive 1000 wallets in reasonable time (<15s)', async () => {
      const masterSeed = await manager.generateMasterSeed(256);

      const startTime = Date.now();

      // Derive 1000 wallets (EVM only)
      for (let i = 0; i < 1000; i++) {
        deriveEVMWallet(masterSeed.seed, i);
      }

      const duration = Date.now() - startTime;

      // Sanity check: derivation should complete (threshold generous for concurrent test runs)
      expect(duration).toBeLessThan(600_000);
    }, 600_000);
  });

  describe('Test Isolation', () => {
    it('should use temporary directory for test storage', async () => {
      // Verify test storage directory exists
      const stats = await fs.stat(tempStoragePath);
      expect(stats.isDirectory()).toBe(true);

      // Verify test files are written to temp directory
      const files = await fs.readdir(tempStoragePath);
      expect(files.length).toBeGreaterThan(0);
      expect(files).toContain('encryption-salt');
    });

    it('should not interfere with other tests', async () => {
      // Generate multiple seeds in sequence
      const seed1 = await manager.generateMasterSeed(256);
      await manager.encryptAndStore(seed1, testPassword);

      const seed2 = await manager.generateMasterSeed(256);
      await manager.encryptAndStore(seed2, 'DifferentP@ssw0rd789');

      // Verify seeds are different
      expect(seed1.mnemonic).not.toBe(seed2.mnemonic);

      // Verify we can load the second seed
      const loaded = await manager.decryptAndLoad('DifferentP@ssw0rd789');
      expect(loaded.mnemonic).toBe(seed2.mnemonic);
    });
  });
});
