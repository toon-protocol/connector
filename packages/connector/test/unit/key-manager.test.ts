import pino from 'pino';
import { KeyManager, KeyManagerConfig, KeyManagerBackend } from '../../src/security/key-manager';

describe('KeyManager', () => {
  let mockLogger: pino.Logger;
  let mockBackend: jest.Mocked<KeyManagerBackend>;

  beforeEach(() => {
    // Reset all mocks
    jest.clearAllMocks();

    // Create mock logger
    mockLogger = {
      info: jest.fn(),
      warn: jest.fn(),
      error: jest.fn(),
      debug: jest.fn(),
      child: jest.fn().mockReturnThis(),
    } as unknown as pino.Logger;

    // Create mock backend
    mockBackend = {
      sign: jest.fn().mockResolvedValue(Buffer.from('test-signature')),
      getPublicKey: jest.fn().mockResolvedValue(Buffer.from('test-public-key')),
      rotateKey: jest.fn().mockResolvedValue('new-key-id'),
    };
  });

  describe('backend selection', () => {
    it('should select EnvironmentVariableBackend when backend="env"', () => {
      const config: KeyManagerConfig = {
        backend: 'env',
        nodeId: 'test-node',
      };

      const keyManager = new KeyManager(config, mockLogger);

      expect(keyManager).toBeDefined();
      expect(mockLogger.child).toHaveBeenCalledWith({ component: 'KeyManager' });
      expect(mockLogger.info).toHaveBeenCalledWith({ backend: 'env' }, 'KeyManager initialized');
    });

    it('should throw error for unsupported backend types', () => {
      for (const backend of ['aws-kms', 'gcp-kms', 'azure-kv', 'hsm']) {
        const config = {
          backend,
          nodeId: 'test-node',
        } as unknown as KeyManagerConfig;

        expect(() => new KeyManager(config, mockLogger)).toThrow('is not supported');
      }
    });

    it('should throw error for unknown backend type', () => {
      const config = {
        backend: 'unknown-backend',
        nodeId: 'test-node',
      } as unknown as KeyManagerConfig;

      expect(() => new KeyManager(config, mockLogger)).toThrow('Unknown backend type');
    });
  });

  describe('sign method', () => {
    let keyManager: KeyManager;

    beforeEach(() => {
      const config: KeyManagerConfig = {
        backend: 'env',
        nodeId: 'test-node',
      };
      keyManager = new KeyManager(config, mockLogger);

      // Replace backend with our mock
      (keyManager as unknown as { backend: KeyManagerBackend }).backend = mockBackend;
    });

    it('should delegate sign() to backend', async () => {
      const message = Buffer.from('test-message');
      const keyId = 'test-key';

      const signature = await keyManager.sign(message, keyId);

      expect(mockBackend.sign).toHaveBeenCalledWith(message, keyId);
      expect(signature).toEqual(Buffer.from('test-signature'));
    });

    it('should log debug message before signing', async () => {
      const message = Buffer.from('test-message');
      const keyId = 'test-key';

      await keyManager.sign(message, keyId);

      expect(mockLogger.debug).toHaveBeenCalledWith(
        { keyId: 'test-key', messageLength: 12 },
        'Signing message'
      );
    });

    it('should log info message after successful signing', async () => {
      const message = Buffer.from('test-message');
      const keyId = 'test-key';

      await keyManager.sign(message, keyId);

      expect(mockLogger.info).toHaveBeenCalledWith(
        { keyId: 'test-key', signatureLength: 14 },
        'Message signed successfully'
      );
    });

    it('should log error and throw if backend.sign() fails', async () => {
      const message = Buffer.from('test-message');
      const keyId = 'test-key';
      const error = new Error('Signing failed');
      mockBackend.sign.mockRejectedValue(error);

      await expect(keyManager.sign(message, keyId)).rejects.toThrow('Signing failed');

      expect(mockLogger.error).toHaveBeenCalledWith(
        { keyId: 'test-key', error },
        'Message signing failed'
      );
    });

    it('should return signature buffer from backend', async () => {
      const message = Buffer.from('test-message');
      const keyId = 'test-key';
      const expectedSignature = Buffer.from('expected-signature');
      mockBackend.sign.mockResolvedValue(expectedSignature);

      const signature = await keyManager.sign(message, keyId);

      expect(signature).toBe(expectedSignature);
    });
  });

  describe('getPublicKey method', () => {
    let keyManager: KeyManager;

    beforeEach(() => {
      const config: KeyManagerConfig = {
        backend: 'env',
        nodeId: 'test-node',
      };
      keyManager = new KeyManager(config, mockLogger);

      // Replace backend with our mock
      (keyManager as unknown as { backend: KeyManagerBackend }).backend = mockBackend;
    });

    it('should delegate getPublicKey() to backend', async () => {
      const keyId = 'test-key';

      const publicKey = await keyManager.getPublicKey(keyId);

      expect(mockBackend.getPublicKey).toHaveBeenCalledWith(keyId);
      expect(publicKey).toEqual(Buffer.from('test-public-key'));
    });

    it('should return public key buffer from backend', async () => {
      const keyId = 'test-key';
      const expectedPublicKey = Buffer.from('expected-public-key');
      mockBackend.getPublicKey.mockResolvedValue(expectedPublicKey);

      const publicKey = await keyManager.getPublicKey(keyId);

      expect(publicKey).toBe(expectedPublicKey);
    });

    it('should throw error if key not found', async () => {
      const keyId = 'non-existent-key';
      const error = new Error('Key not found');
      mockBackend.getPublicKey.mockRejectedValue(error);

      await expect(keyManager.getPublicKey(keyId)).rejects.toThrow('Key not found');

      expect(mockLogger.error).toHaveBeenCalledWith(
        { keyId: 'non-existent-key', error },
        'Public key retrieval failed'
      );
    });
  });

  describe('rotateKey method', () => {
    let keyManager: KeyManager;

    beforeEach(() => {
      const config: KeyManagerConfig = {
        backend: 'env',
        nodeId: 'test-node',
      };
      keyManager = new KeyManager(config, mockLogger);

      // Replace backend with our mock
      (keyManager as unknown as { backend: KeyManagerBackend }).backend = mockBackend;
    });

    it('should initiate key rotation via backend.rotateKey()', async () => {
      const keyId = 'old-key';

      const newKeyId = await keyManager.rotateKey(keyId);

      expect(mockBackend.rotateKey).toHaveBeenCalledWith(keyId);
      expect(newKeyId).toBe('new-key-id');
    });

    it('should log info message when starting key rotation', async () => {
      const keyId = 'old-key';

      await keyManager.rotateKey(keyId);

      expect(mockLogger.info).toHaveBeenCalledWith({ keyId: 'old-key' }, 'Starting key rotation');
    });

    it('should log info message when key rotation completes', async () => {
      const keyId = 'old-key';

      await keyManager.rotateKey(keyId);

      expect(mockLogger.info).toHaveBeenCalledWith(
        { oldKeyId: 'old-key', newKeyId: 'new-key-id' },
        'Key rotation completed'
      );
    });

    it('should return new key ID from backend', async () => {
      const keyId = 'old-key';
      const expectedNewKeyId = 'expected-new-key-id';
      mockBackend.rotateKey.mockResolvedValue(expectedNewKeyId);

      const newKeyId = await keyManager.rotateKey(keyId);

      expect(newKeyId).toBe(expectedNewKeyId);
    });

    it('should log error and throw if rotation fails', async () => {
      const keyId = 'old-key';
      const error = new Error('Rotation failed');
      mockBackend.rotateKey.mockRejectedValue(error);

      await expect(keyManager.rotateKey(keyId)).rejects.toThrow('Rotation failed');

      expect(mockLogger.error).toHaveBeenCalledWith(
        { keyId: 'old-key', error },
        'Key rotation failed'
      );
    });
  });

  describe('backend configuration validation', () => {
    it('should reject unsupported cloud/HSM backends', () => {
      for (const backend of ['aws-kms', 'gcp-kms', 'azure-kv', 'hsm'] as const) {
        const config = {
          backend,
          nodeId: 'test-node',
        } as unknown as KeyManagerConfig;

        expect(() => new KeyManager(config, mockLogger)).toThrow('is not supported');
      }
    });
  });
});
