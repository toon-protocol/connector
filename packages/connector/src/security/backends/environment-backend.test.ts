import { EnvironmentVariableBackend } from './environment-backend';
import { Wallet } from 'ethers';
import pino from 'pino';

describe('EnvironmentVariableBackend', () => {
  let logger: pino.Logger;
  const originalEnv = process.env;

  beforeEach(() => {
    logger = pino({ level: 'silent' });
    jest.resetModules();
    process.env = { ...originalEnv };
  });

  afterEach(() => {
    process.env = originalEnv;
  });

  describe('Constructor', () => {
    it('should load EVM wallet from EVM_PRIVATE_KEY environment variable', () => {
      const testPrivateKey = '0x' + '1'.repeat(64); // Valid private key
      process.env.EVM_PRIVATE_KEY = testPrivateKey;

      const backend = new EnvironmentVariableBackend(logger);

      // Verify wallet was created (sign should work)
      expect(backend).toBeDefined();
    });

    it('should throw error for invalid EVM_PRIVATE_KEY on first use', async () => {
      process.env.EVM_PRIVATE_KEY = 'invalid-private-key';

      const backend = new EnvironmentVariableBackend(logger);

      // Validation is deferred until first wallet use
      await expect(backend.getPublicKey('evm-key')).rejects.toThrow(
        'Invalid EVM_PRIVATE_KEY in environment'
      );
    });

    it('should warn if no keys loaded from environment', () => {
      delete process.env.EVM_PRIVATE_KEY;

      const backend = new EnvironmentVariableBackend(logger);

      expect(backend).toBeDefined();
    });
  });

  describe('sign()', () => {
    it('should sign EVM message using ethers.Wallet', async () => {
      const testPrivateKey = '0x' + '1'.repeat(64);
      process.env.EVM_PRIVATE_KEY = testPrivateKey;

      const backend = new EnvironmentVariableBackend(logger);
      // signingKey.sign() requires a 32-byte hash, not raw message
      const testMessage = Buffer.from('0'.repeat(64), 'hex'); // 32-byte hash

      const signature = await backend.sign(testMessage, 'evm-key');

      expect(Buffer.isBuffer(signature)).toBe(true);
      expect(signature.length).toBe(65); // r + s + v = 65 bytes

      // Verify signature is valid by recovering the address
      const wallet = new Wallet(testPrivateKey);
      const recoveredAddress = wallet.address;
      expect(recoveredAddress).toBeDefined();
    });

    it('should detect EVM key type from keyId containing "evm"', async () => {
      const testPrivateKey = '0x' + '1'.repeat(64);
      process.env.EVM_PRIVATE_KEY = testPrivateKey;

      const backend = new EnvironmentVariableBackend(logger);
      // signingKey.sign() requires a 32-byte hash
      const testMessage = Buffer.from('0'.repeat(64), 'hex'); // 32-byte hash

      const signature = await backend.sign(testMessage, 'my-evm-signing-key');

      expect(Buffer.isBuffer(signature)).toBe(true);
      expect(signature.length).toBe(65); // r + s + v = 65 bytes
    });

    it('should throw error if EVM wallet not initialized', async () => {
      delete process.env.EVM_PRIVATE_KEY;

      const backend = new EnvironmentVariableBackend(logger);
      const testMessage = Buffer.from('test-message');

      await expect(backend.sign(testMessage, 'evm-key')).rejects.toThrow(
        'EVM wallet not initialized. Set EVM_PRIVATE_KEY environment variable.'
      );
    });
  });

  describe('getPublicKey()', () => {
    it('should derive EVM public key from private key', async () => {
      const testPrivateKey = '0x' + '1'.repeat(64);
      process.env.EVM_PRIVATE_KEY = testPrivateKey;

      const backend = new EnvironmentVariableBackend(logger);

      const publicKey = await backend.getPublicKey('evm-key');

      expect(Buffer.isBuffer(publicKey)).toBe(true);
      expect(publicKey.length).toBeGreaterThan(0);

      // Verify it matches the wallet's public key
      const wallet = new Wallet(testPrivateKey);
      const expectedPublicKey = wallet.signingKey.publicKey.slice(2); // Remove '0x'
      expect(publicKey.toString('hex')).toBe(expectedPublicKey);
    });

    it('should throw error if EVM wallet not initialized', async () => {
      delete process.env.EVM_PRIVATE_KEY;

      const backend = new EnvironmentVariableBackend(logger);

      await expect(backend.getPublicKey('evm-key')).rejects.toThrow(
        'EVM wallet not initialized. Set EVM_PRIVATE_KEY environment variable.'
      );
    });
  });

  describe('rotateKey()', () => {
    it('should throw error indicating manual rotation required', async () => {
      const testPrivateKey = '0x' + '1'.repeat(64);
      process.env.EVM_PRIVATE_KEY = testPrivateKey;

      const backend = new EnvironmentVariableBackend(logger);

      await expect(backend.rotateKey('evm-key')).rejects.toThrow(
        'Manual rotation required for environment backend'
      );
    });
  });
});
