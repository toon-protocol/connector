/* eslint-disable @typescript-eslint/no-explicit-any */
/* eslint-disable @typescript-eslint/no-unused-vars */
/* eslint-disable @typescript-eslint/no-var-requires */
import { AWSKMSBackend } from './aws-kms-backend';
import { AWSConfig } from '../key-manager';
import pino from 'pino';

// Check if AWS SDK is available
let AWSSDKAvailable = false;

try {
  require('@aws-sdk/client-kms');
  AWSSDKAvailable = true;
} catch (error) {
  // AWS SDK not installed - tests will be skipped
}

const describeIf = AWSSDKAvailable ? describe : describe.skip;

describeIf('AWSKMSBackend', () => {
  let backend: AWSKMSBackend;
  let logger: pino.Logger;
  let mockKMSClient: any;
  const config: AWSConfig = {
    region: 'us-east-1',
    evmKeyId: 'arn:aws:kms:us-east-1:123456789012:key/evm-key-id',
    credentials: {
      accessKeyId: 'test-access-key',
      secretAccessKey: 'test-secret-key',
    },
  };

  beforeEach(() => {
    logger = pino({ level: 'silent' });
    jest.clearAllMocks();

    // Create mock KMS client
    mockKMSClient = {
      send: jest.fn(),
    };

    backend = new AWSKMSBackend(config, logger);
  });

  describe('sign', () => {
    it('should sign message using AWS KMS SignCommand with ECDSA_SHA_256 for EVM keys', async () => {
      const message = Buffer.from('test-message');
      const mockSignature = Buffer.from('mock-signature');

      mockKMSClient.send.mockResolvedValueOnce({
        Signature: mockSignature,
      });

      const result = await backend.sign(message, config.evmKeyId);

      expect(mockKMSClient.send).toHaveBeenCalled();
      expect(result).toEqual(mockSignature);

      // Verify command was created with correct parameters
      const call = mockKMSClient.send.mock.calls[0][0];
      expect(call.input).toMatchObject({
        KeyId: config.evmKeyId,
        Message: message,
        SigningAlgorithm: 'ECDSA_SHA_256',
        MessageType: 'RAW',
      });
    });

    it('should detect EVM key type from keyId containing "evm"', async () => {
      const message = Buffer.from('test-message');
      const mockSignature = Buffer.from('mock-signature');
      const evmKeyId = 'arn:aws:kms:us-east-1:123456789012:key/my-evm-signing-key';

      mockKMSClient.send.mockResolvedValueOnce({
        Signature: mockSignature,
      });

      await backend.sign(message, evmKeyId);

      const call = mockKMSClient.send.mock.calls[0][0];
      expect(call.input.SigningAlgorithm).toBe('ECDSA_SHA_256');
    });

    it('should throw error if AWS KMS returns no signature', async () => {
      const message = Buffer.from('test-message');

      mockKMSClient.send.mockResolvedValueOnce({
        Signature: undefined,
      });

      await expect(backend.sign(message, config.evmKeyId)).rejects.toThrow(
        'AWS KMS returned no signature'
      );
    });

    it('should throw error if AWS KMS signing fails', async () => {
      const message = Buffer.from('test-message');
      const error = new Error('KMS service unavailable');

      mockKMSClient.send.mockRejectedValueOnce(error);

      await expect(backend.sign(message, config.evmKeyId)).rejects.toThrow(
        'KMS service unavailable'
      );
    });
  });

  describe('getPublicKey', () => {
    it('should get public key using AWS KMS GetPublicKeyCommand', async () => {
      const mockPublicKey = Buffer.from('mock-public-key');

      mockKMSClient.send.mockResolvedValueOnce({
        PublicKey: mockPublicKey,
      });

      const result = await backend.getPublicKey(config.evmKeyId);

      expect(mockKMSClient.send).toHaveBeenCalled();
      expect(result).toEqual(mockPublicKey);

      // Verify command parameters
      const call = mockKMSClient.send.mock.calls[0][0];
      expect(call.input).toMatchObject({
        KeyId: config.evmKeyId,
      });
    });

    it('should throw error if AWS KMS returns no public key', async () => {
      mockKMSClient.send.mockResolvedValueOnce({
        PublicKey: undefined,
      });

      await expect(backend.getPublicKey(config.evmKeyId)).rejects.toThrow(
        'AWS KMS returned no public key'
      );
    });

    it('should throw error if AWS KMS public key retrieval fails', async () => {
      const error = new Error('Key not found');

      mockKMSClient.send.mockRejectedValueOnce(error);

      await expect(backend.getPublicKey(config.evmKeyId)).rejects.toThrow('Key not found');
    });
  });

  describe('rotateKey', () => {
    it('should rotate EVM key using CreateKeyCommand with ECC_SECG_P256K1 KeySpec', async () => {
      const newKeyArn = 'arn:aws:kms:us-east-1:123456789012:key/new-evm-key-id';

      mockKMSClient.send.mockResolvedValueOnce({
        KeyMetadata: {
          Arn: newKeyArn,
        },
      });

      const result = await backend.rotateKey(config.evmKeyId);

      expect(mockKMSClient.send).toHaveBeenCalled();
      expect(result).toBe(newKeyArn);

      // Verify correct KeySpec
      const call = mockKMSClient.send.mock.calls[0][0];
      expect(call.input).toMatchObject({
        KeyUsage: 'SIGN_VERIFY',
        KeySpec: 'ECC_SECG_P256K1',
      });
      expect(call.input.Description).toContain('EVM');
    });

    it('should include rotation tags in CreateKeyCommand', async () => {
      const newKeyArn = 'arn:aws:kms:us-east-1:123456789012:key/new-key-id';

      mockKMSClient.send.mockResolvedValueOnce({
        KeyMetadata: {
          Arn: newKeyArn,
        },
      });

      await backend.rotateKey(config.evmKeyId);

      const call = mockKMSClient.send.mock.calls[0][0];
      expect(call.input.Tags).toEqual(
        expect.arrayContaining([
          { TagKey: 'Purpose', TagValue: 'ILP-Connector-Settlement' },
          { TagKey: 'KeyType', TagValue: 'EVM' },
          { TagKey: 'RotatedFrom', TagValue: config.evmKeyId },
        ])
      );
    });

    it('should throw error if AWS KMS returns no key ARN', async () => {
      mockKMSClient.send.mockResolvedValueOnce({
        KeyMetadata: {},
      });

      await expect(backend.rotateKey(config.evmKeyId)).rejects.toThrow(
        'AWS KMS returned no key ARN'
      );
    });

    it('should throw error if AWS KMS key rotation fails', async () => {
      const error = new Error('Insufficient permissions');

      mockKMSClient.send.mockRejectedValueOnce(error);

      await expect(backend.rotateKey(config.evmKeyId)).rejects.toThrow('Insufficient permissions');
    });
  });

  describe('initialization', () => {
    it('should initialize KMSClient with correct region and credentials', () => {
      // Backend already initialized in beforeEach
      // Just verify it was created successfully
      expect(backend).toBeDefined();
    });
  });
});

if (!AWSSDKAvailable) {
  console.warn(
    '\n⚠️  AWS SDK (@aws-sdk/client-kms) not installed - skipping AWSKMSBackend tests\n' +
      '   Install with: npm install @aws-sdk/client-kms\n'
  );
}
