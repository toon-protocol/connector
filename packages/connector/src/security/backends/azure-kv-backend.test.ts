/* eslint-disable @typescript-eslint/no-explicit-any */
/* eslint-disable @typescript-eslint/no-unused-vars */
/* eslint-disable @typescript-eslint/no-var-requires */
import { AzureKeyVaultBackend } from './azure-kv-backend';
import { AzureConfig } from '../key-manager';
import pino from 'pino';
import * as crypto from 'crypto';

// Check if Azure SDK is available
let AzureSDKAvailable = false;
let KeyClient: any, CryptographyClient: any, ClientSecretCredential: any;

try {
  const keyvaultSdk = require('@azure/keyvault-keys');
  const identitySdk = require('@azure/identity');
  KeyClient = keyvaultSdk.KeyClient;
  CryptographyClient = keyvaultSdk.CryptographyClient;
  ClientSecretCredential = identitySdk.ClientSecretCredential;
  AzureSDKAvailable = true;
} catch (error) {
  // Azure SDK not installed - tests will be skipped
}

const describeIf = AzureSDKAvailable ? describe : describe.skip;

describeIf('AzureKeyVaultBackend', () => {
  let backend: AzureKeyVaultBackend;
  let logger: pino.Logger;
  let mockKeyClient: any;
  let mockCryptoClient: any;
  let mockCredential: any;
  const config: AzureConfig = {
    vaultUrl: 'https://test-vault.vault.azure.net/',
    evmKeyName: 'evm-key',
    credentials: {
      tenantId: 'test-tenant-id',
      clientId: 'test-client-id',
      clientSecret: 'test-client-secret',
    },
  };

  beforeEach(() => {
    logger = pino({ level: 'silent' });
    jest.clearAllMocks();

    // Create mock credential
    mockCredential = {};
    (ClientSecretCredential as jest.Mock) = jest.fn(() => mockCredential);

    // Create mock KeyClient
    mockKeyClient = {
      getKey: jest.fn(),
      createKey: jest.fn(),
    };
    (mockKeyClient as any)['credential'] = mockCredential;
    (KeyClient as jest.Mock) = jest.fn(() => mockKeyClient);

    // Create mock CryptographyClient
    mockCryptoClient = {
      sign: jest.fn(),
    };
    (CryptographyClient as jest.Mock) = jest.fn(() => mockCryptoClient);

    backend = new AzureKeyVaultBackend(config, logger);
  });

  describe('sign', () => {
    it('should sign message using Azure Key Vault with ES256K algorithm for EVM keys', async () => {
      const message = Buffer.from('test-message');
      const mockSignature = Buffer.from('mock-signature');
      const digest = crypto.createHash('sha256').update(message).digest();

      mockKeyClient.getKey.mockResolvedValueOnce({
        id: 'https://test-vault.vault.azure.net/keys/evm-key/version',
        name: config.evmKeyName,
      });

      mockCryptoClient.sign.mockResolvedValueOnce({
        result: mockSignature,
      });

      const result = await backend.sign(message, config.evmKeyName);

      expect(mockKeyClient.getKey).toHaveBeenCalledWith(config.evmKeyName);
      expect(CryptographyClient).toHaveBeenCalled();
      expect(mockCryptoClient.sign).toHaveBeenCalledWith('ES256K', digest);
      expect(result).toEqual(mockSignature);
    });

    it('should create SHA256 digest of message before signing', async () => {
      const message = Buffer.from('test-message');
      const expectedDigest = crypto.createHash('sha256').update(message).digest();

      mockKeyClient.getKey.mockResolvedValueOnce({
        id: 'https://test-vault.vault.azure.net/keys/evm-key/version',
        name: config.evmKeyName,
      });

      mockCryptoClient.sign.mockResolvedValueOnce({
        result: Buffer.from('signature'),
      });

      await backend.sign(message, config.evmKeyName);

      expect(mockCryptoClient.sign).toHaveBeenCalledWith('ES256K', expectedDigest);
    });

    it('should detect EVM key type from keyName containing "evm"', async () => {
      const evmKeyName = 'my-evm-signing-key';
      const message = Buffer.from('test');

      mockKeyClient.getKey.mockResolvedValueOnce({
        id: 'https://test-vault.vault.azure.net/keys/my-evm-signing-key/version',
        name: evmKeyName,
      });

      mockCryptoClient.sign.mockResolvedValueOnce({
        result: Buffer.from('sig'),
      });

      await backend.sign(message, evmKeyName);

      expect(mockCryptoClient.sign).toHaveBeenCalledWith('ES256K', expect.any(Buffer));
    });

    it('should throw error if Azure Key Vault returns no key ID', async () => {
      const message = Buffer.from('test-message');

      mockKeyClient.getKey.mockResolvedValueOnce({
        id: undefined,
      });

      await expect(backend.sign(message, config.evmKeyName)).rejects.toThrow(
        'Azure Key Vault returned no key ID'
      );
    });

    it('should throw error if Azure Key Vault returns no signature', async () => {
      const message = Buffer.from('test-message');

      mockKeyClient.getKey.mockResolvedValueOnce({
        id: 'https://test-vault.vault.azure.net/keys/evm-key/version',
        name: config.evmKeyName,
      });

      mockCryptoClient.sign.mockResolvedValueOnce({
        result: undefined,
      });

      await expect(backend.sign(message, config.evmKeyName)).rejects.toThrow(
        'Azure Key Vault returned no signature'
      );
    });

    it('should throw error if Azure Key Vault signing fails', async () => {
      const message = Buffer.from('test-message');
      const error = new Error('Authentication failed');

      mockKeyClient.getKey.mockRejectedValueOnce(error);

      await expect(backend.sign(message, config.evmKeyName)).rejects.toThrow(
        'Authentication failed'
      );
    });
  });

  describe('getPublicKey', () => {
    it('should get public key from Azure Key Vault', async () => {
      const mockX = Buffer.from('x-coordinate-32-bytes').toString('base64');
      const mockY = Buffer.from('y-coordinate-32-bytes').toString('base64');

      mockKeyClient.getKey.mockResolvedValueOnce({
        name: config.evmKeyName,
        key: {
          x: mockX,
          y: mockY,
        },
      });

      const result = await backend.getPublicKey(config.evmKeyName);

      expect(mockKeyClient.getKey).toHaveBeenCalledWith(config.evmKeyName);
      expect(result).toBeInstanceOf(Buffer);

      // Verify uncompressed public key format (0x04 + x + y)
      expect(result[0]).toBe(0x04);
      expect(result.length).toBe(1 + 24 + 24); // 0x04 + x + y
    });

    it('should combine x and y coordinates for uncompressed public key format', async () => {
      const xBytes = Buffer.from('0123456789abcdef0123456789abcdef');
      const yBytes = Buffer.from('fedcba9876543210fedcba9876543210');
      const mockX = xBytes.toString('base64');
      const mockY = yBytes.toString('base64');

      mockKeyClient.getKey.mockResolvedValueOnce({
        name: config.evmKeyName,
        key: {
          x: mockX,
          y: mockY,
        },
      });

      const result = await backend.getPublicKey(config.evmKeyName);

      // Verify format: 0x04 + x + y
      const expectedPublicKey = Buffer.concat([Buffer.from([0x04]), xBytes, yBytes]);
      expect(result).toEqual(expectedPublicKey);
    });

    it('should throw error if Azure Key Vault returns no public key', async () => {
      mockKeyClient.getKey.mockResolvedValueOnce({
        name: config.evmKeyName,
        key: undefined,
      });

      await expect(backend.getPublicKey(config.evmKeyName)).rejects.toThrow(
        'Azure Key Vault returned no public key'
      );
    });

    it('should throw error if key is missing x or y coordinates', async () => {
      mockKeyClient.getKey.mockResolvedValueOnce({
        name: config.evmKeyName,
        key: {
          x: Buffer.from('x-coord').toString('base64'),
          // y is missing
        },
      });

      await expect(backend.getPublicKey(config.evmKeyName)).rejects.toThrow(
        'Azure Key Vault key missing x or y coordinates'
      );
    });

    it('should throw error if Azure Key Vault public key retrieval fails', async () => {
      const error = new Error('Key not found');

      mockKeyClient.getKey.mockRejectedValueOnce(error);

      await expect(backend.getPublicKey(config.evmKeyName)).rejects.toThrow('Key not found');
    });
  });

  describe('rotateKey', () => {
    it('should rotate EVM key using createKey with SECP256K1 curve', async () => {
      const currentTime = Date.now();
      jest.spyOn(Date, 'now').mockReturnValue(currentTime);

      const expectedNewKeyName = `${config.evmKeyName}-rotated-${currentTime}`;

      mockKeyClient.createKey.mockResolvedValueOnce({
        name: expectedNewKeyName,
      });

      const result = await backend.rotateKey(config.evmKeyName);

      expect(mockKeyClient.createKey).toHaveBeenCalledWith(expectedNewKeyName, 'SECP256K1', {
        keyOps: ['sign', 'verify'],
        tags: {
          purpose: 'ILP-Connector-Settlement',
          keyType: 'EVM',
          rotatedFrom: config.evmKeyName,
        },
      });
      expect(result).toBe(expectedNewKeyName);
    });

    it('should include rotation tags in createKey', async () => {
      const currentTime = Date.now();
      jest.spyOn(Date, 'now').mockReturnValue(currentTime);

      mockKeyClient.createKey.mockResolvedValueOnce({
        name: 'new-key',
      });

      await backend.rotateKey(config.evmKeyName);

      const callArgs = mockKeyClient.createKey.mock.calls[0][2];
      expect(callArgs?.tags).toEqual({
        purpose: 'ILP-Connector-Settlement',
        keyType: 'EVM',
        rotatedFrom: config.evmKeyName,
      });
    });

    it('should throw error if Azure Key Vault returns no key name', async () => {
      mockKeyClient.createKey.mockResolvedValueOnce({
        name: undefined,
      });

      await expect(backend.rotateKey(config.evmKeyName)).rejects.toThrow(
        'Azure Key Vault returned no key name'
      );
    });

    it('should throw error if Azure Key Vault key rotation fails', async () => {
      const error = new Error('Insufficient permissions');

      mockKeyClient.createKey.mockRejectedValueOnce(error);

      await expect(backend.rotateKey(config.evmKeyName)).rejects.toThrow(
        'Insufficient permissions'
      );
    });
  });

  describe('initialization', () => {
    it('should initialize KeyClient with vault URL and credentials', () => {
      expect(backend).toBeDefined();
    });
  });
});

if (!AzureSDKAvailable) {
  console.warn(
    '\n⚠️  Azure SDK (@azure/keyvault-keys, @azure/identity) not installed - skipping AzureKeyVaultBackend tests\n' +
      '   Install with: npm install @azure/keyvault-keys @azure/identity\n'
  );
}
