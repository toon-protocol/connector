/* eslint-disable @typescript-eslint/no-explicit-any */
/* eslint-disable @typescript-eslint/no-unused-vars */
/* eslint-disable @typescript-eslint/no-var-requires */
import { GCPKMSBackend } from './gcp-kms-backend';
import { GCPConfig } from '../key-manager';
import pino from 'pino';
import * as crypto from 'crypto';

// Check if GCP SDK is available
let GCPSDKAvailable = false;
let KeyManagementServiceClient: any;

try {
  const gcpSdk = require('@google-cloud/kms');
  KeyManagementServiceClient = gcpSdk.KeyManagementServiceClient;
  GCPSDKAvailable = true;
} catch (error) {
  // GCP SDK not installed - tests will be skipped
}

const describeIf = GCPSDKAvailable ? describe : describe.skip;

describeIf('GCPKMSBackend', () => {
  let backend: GCPKMSBackend;
  let logger: pino.Logger;
  let mockKMSClient: any;
  const config: GCPConfig = {
    projectId: 'test-project',
    locationId: 'us-east1',
    keyRingId: 'test-keyring',
    evmKeyId: 'evm-key',
  };

  beforeEach(() => {
    logger = pino({ level: 'silent' });
    jest.clearAllMocks();

    // Create mock GCP KMS client
    mockKMSClient = {
      asymmetricSign: jest.fn(),
      getPublicKey: jest.fn(),
      createCryptoKeyVersion: jest.fn(),
    };

    (KeyManagementServiceClient as jest.Mock) = jest.fn(() => mockKMSClient);

    backend = new GCPKMSBackend(config, logger);
  });

  describe('sign', () => {
    it('should sign message using GCP KMS asymmetricSign API', async () => {
      const message = Buffer.from('test-message');
      const mockSignature = Buffer.from('mock-signature');
      const digest = crypto.createHash('sha256').update(message).digest();

      mockKMSClient.asymmetricSign.mockResolvedValueOnce([
        {
          signature: new Uint8Array(mockSignature),
        },
      ]);

      const result = await backend.sign(message, config.evmKeyId);

      expect(mockKMSClient.asymmetricSign).toHaveBeenCalled();
      expect(result).toEqual(mockSignature);

      // Verify asymmetricSign was called with correct parameters
      const callArgs = mockKMSClient.asymmetricSign.mock.calls[0][0];
      expect(callArgs.name).toBe(
        `projects/${config.projectId}/locations/${config.locationId}/keyRings/${config.keyRingId}/cryptoKeys/${config.evmKeyId}/cryptoKeyVersions/1`
      );
      expect(callArgs.digest?.sha256).toEqual(digest);
    });

    it('should create SHA256 digest of message before signing', async () => {
      const message = Buffer.from('test-message');
      const expectedDigest = crypto.createHash('sha256').update(message).digest();

      mockKMSClient.asymmetricSign.mockResolvedValueOnce([
        {
          signature: new Uint8Array(Buffer.from('signature')),
        },
      ]);

      await backend.sign(message, config.evmKeyId);

      const callArgs = mockKMSClient.asymmetricSign.mock.calls[0][0];
      expect(callArgs.digest?.sha256).toEqual(expectedDigest);
    });

    it('should detect EVM key type and use correct crypto key version name', async () => {
      const message = Buffer.from('test-message');

      mockKMSClient.asymmetricSign.mockResolvedValueOnce([
        {
          signature: new Uint8Array(Buffer.from('signature')),
        },
      ]);

      await backend.sign(message, config.evmKeyId);

      const callArgs = mockKMSClient.asymmetricSign.mock.calls[0][0];
      expect(callArgs.name).toContain(config.evmKeyId);
    });

    it('should throw error if GCP KMS returns no signature', async () => {
      const message = Buffer.from('test-message');

      mockKMSClient.asymmetricSign.mockResolvedValueOnce([
        {
          signature: undefined,
        },
      ]);

      await expect(backend.sign(message, config.evmKeyId)).rejects.toThrow(
        'GCP KMS returned no signature'
      );
    });

    it('should throw error if GCP KMS signing fails', async () => {
      const message = Buffer.from('test-message');
      const error = new Error('Permission denied');

      mockKMSClient.asymmetricSign.mockRejectedValueOnce(error);

      await expect(backend.sign(message, config.evmKeyId)).rejects.toThrow('Permission denied');
    });
  });

  describe('getPublicKey', () => {
    it('should get public key using GCP KMS getPublicKey API', async () => {
      const mockPublicKeyPem = `-----BEGIN PUBLIC KEY-----
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEMock+public+key+data+here
-----END PUBLIC KEY-----`;

      mockKMSClient.getPublicKey.mockResolvedValueOnce([
        {
          pem: mockPublicKeyPem,
        },
      ]);

      const result = await backend.getPublicKey(config.evmKeyId);

      expect(mockKMSClient.getPublicKey).toHaveBeenCalled();
      expect(result).toBeInstanceOf(Buffer);

      // Verify getPublicKey was called with correct crypto key version name
      const callArgs = mockKMSClient.getPublicKey.mock.calls[0][0];
      expect(callArgs.name).toBe(
        `projects/${config.projectId}/locations/${config.locationId}/keyRings/${config.keyRingId}/cryptoKeys/${config.evmKeyId}/cryptoKeyVersions/1`
      );
    });

    it('should convert PEM format to DER buffer', async () => {
      const mockPublicKeyPem = `-----BEGIN PUBLIC KEY-----
TW9ja1B1YmxpY0tleURhdGE=
-----END PUBLIC KEY-----`;

      mockKMSClient.getPublicKey.mockResolvedValueOnce([
        {
          pem: mockPublicKeyPem,
        },
      ]);

      const result = await backend.getPublicKey(config.evmKeyId);

      // The result should be a Buffer containing the base64-decoded data
      expect(Buffer.isBuffer(result)).toBe(true);
      // Verify the PEM headers/footers were stripped
      expect(result.toString('base64')).toBe('TW9ja1B1YmxpY0tleURhdGE=');
    });

    it('should throw error if GCP KMS returns no public key', async () => {
      mockKMSClient.getPublicKey.mockResolvedValueOnce([
        {
          pem: undefined,
        },
      ]);

      await expect(backend.getPublicKey(config.evmKeyId)).rejects.toThrow(
        'GCP KMS returned no public key'
      );
    });

    it('should throw error if GCP KMS public key retrieval fails', async () => {
      const error = new Error('Key not found');

      mockKMSClient.getPublicKey.mockRejectedValueOnce(error);

      await expect(backend.getPublicKey(config.evmKeyId)).rejects.toThrow('Key not found');
    });
  });

  describe('rotateKey', () => {
    it('should rotate key using createCryptoKeyVersion API', async () => {
      const mockNewVersionName = `projects/${config.projectId}/locations/${config.locationId}/keyRings/${config.keyRingId}/cryptoKeys/${config.evmKeyId}/cryptoKeyVersions/2`;

      mockKMSClient.createCryptoKeyVersion.mockResolvedValueOnce([
        {
          name: mockNewVersionName,
        },
      ]);

      const result = await backend.rotateKey(config.evmKeyId);

      expect(mockKMSClient.createCryptoKeyVersion).toHaveBeenCalled();
      // Should return the crypto key name (not version name)
      expect(result).toBe(config.evmKeyId);

      // Verify createCryptoKeyVersion was called with correct parent
      const callArgs = mockKMSClient.createCryptoKeyVersion.mock.calls[0][0];
      expect(callArgs.parent).toBe(
        `projects/${config.projectId}/locations/${config.locationId}/keyRings/${config.keyRingId}/cryptoKeys/${config.evmKeyId}`
      );
    });

    it('should throw error if GCP KMS returns no key version name', async () => {
      mockKMSClient.createCryptoKeyVersion.mockResolvedValueOnce([
        {
          name: undefined,
        },
      ]);

      await expect(backend.rotateKey(config.evmKeyId)).rejects.toThrow(
        'GCP KMS returned no key version name'
      );
    });

    it('should throw error if GCP KMS key rotation fails', async () => {
      const error = new Error('Insufficient permissions');

      mockKMSClient.createCryptoKeyVersion.mockRejectedValueOnce(error);

      await expect(backend.rotateKey(config.evmKeyId)).rejects.toThrow('Insufficient permissions');
    });
  });

  describe('initialization', () => {
    it('should initialize KeyManagementServiceClient', () => {
      expect(backend).toBeDefined();
    });
  });

  describe('key type detection', () => {
    it('should detect EVM key from keyId containing "evm"', async () => {
      const evmKeyId = 'my-evm-signing-key';
      const message = Buffer.from('test');

      mockKMSClient.asymmetricSign.mockResolvedValueOnce([
        {
          signature: new Uint8Array(Buffer.from('sig')),
        },
      ]);

      await backend.sign(message, evmKeyId);

      const callArgs = mockKMSClient.asymmetricSign.mock.calls[0][0];
      expect(callArgs.name).toContain(evmKeyId);
    });
  });
});

if (!GCPSDKAvailable) {
  console.warn(
    '\n⚠️  GCP SDK (@google-cloud/kms) not installed - skipping GCPKMSBackend tests\n' +
      '   Install with: npm install @google-cloud/kms\n'
  );
}
