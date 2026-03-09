import pino from 'pino';
import { KeyManager, KeyManagerConfig } from '../../src/security/key-manager';

// Check if AWS SDK and credentials are available
let awsKmsAvailable = false;
try {
  require('@aws-sdk/client-kms');
  // Check if AWS credentials are configured
  if (
    process.env.AWS_ACCESS_KEY_ID &&
    process.env.AWS_SECRET_ACCESS_KEY &&
    process.env.AWS_KMS_EVM_KEY_ID &&
    process.env.AWS_REGION
  ) {
    awsKmsAvailable = true;
  }
} catch {
  // AWS SDK not installed
}

const describeIf = (condition: boolean): jest.Describe => (condition ? describe : describe.skip);

describeIf(awsKmsAvailable)('AWS KMS Signing Integration Tests', () => {
  let logger: pino.Logger;
  let keyManager: KeyManager;

  beforeAll(() => {
    // Create logger
    logger = pino({ level: 'silent' }); // Silent for tests

    // Create KeyManager with AWS KMS backend
    const config: KeyManagerConfig = {
      backend: 'aws-kms',
      nodeId: 'test-node',
      aws: {
        region: process.env.AWS_REGION!,
        evmKeyId: process.env.AWS_KMS_EVM_KEY_ID!,
        credentials: {
          accessKeyId: process.env.AWS_ACCESS_KEY_ID!,
          secretAccessKey: process.env.AWS_SECRET_ACCESS_KEY!,
        },
      },
    };

    keyManager = new KeyManager(config, logger);
  });

  afterAll(() => {
    // Note: AWS KMS keys created during tests are not automatically deleted.
    // Clean up manually in AWS Console if key rotation tests created new keys.
  });

  describe('EVM signing with real AWS KMS key', () => {
    it('should sign EVM message with real AWS KMS key', async () => {
      const testMessage = Buffer.from('test-message-for-evm-signing');
      const evmKeyId = process.env.AWS_KMS_EVM_KEY_ID!;

      // Sign message using AWS KMS
      const signature = await keyManager.sign(testMessage, evmKeyId);

      // Verify signature is not empty
      expect(signature).toBeDefined();
      expect(signature.length).toBeGreaterThan(0);

      // Get public key
      const publicKey = await keyManager.getPublicKey(evmKeyId);

      expect(publicKey).toBeDefined();
      expect(publicKey.length).toBeGreaterThan(0);

      // Note: Full signature verification would require:
      // 1. Converting AWS KMS signature format (DER-encoded ECDSA) to Ethereum format (r,s,v)
      // 2. Recovering signer address from signature
      // 3. Deriving address from public key
      // This is beyond the scope of this test - the test verifies that signing works
    }, 30000); // 30 second timeout for AWS API calls
  });

  describe('Key rotation with AWS KMS', () => {
    it.skip('should rotate key and maintain overlap period', async () => {
      // IMPORTANT: This test is skipped by default because:
      // 1. Key rotation creates new KMS keys (costs money)
      // 2. KMS keys cannot be immediately deleted (scheduled deletion only)
      // 3. Manual cleanup required in AWS Console
      //
      // To run this test:
      // 1. Remove .skip from this test
      // 2. Ensure you have permissions to create KMS keys
      // 3. Be prepared to manually delete keys afterward
      //
      // Test implementation:
      const evmKeyId = process.env.AWS_KMS_EVM_KEY_ID!;

      // Rotate key
      const newKeyId = await keyManager.rotateKey(evmKeyId);

      // Verify new key ID returned
      expect(newKeyId).toBeDefined();
      expect(newKeyId).not.toBe(evmKeyId);

      // Sign with new key
      const testMessage = Buffer.from('test-message-with-new-key');
      const signatureWithNewKey = await keyManager.sign(testMessage, newKeyId);
      expect(signatureWithNewKey).toBeDefined();

      // Note: Testing old key would require KeyRotationManager integration
      // which maintains the overlap period. This test verifies that:
      // 1. New key is created
      // 2. New key can sign messages
      // Overlap period testing is covered in unit tests
      // MANUAL CLEANUP REQUIRED: Delete rotated key from AWS KMS Console
    }, 60000); // 60 second timeout for key creation
  });
});

// If AWS KMS not available, provide helpful message
if (!awsKmsAvailable) {
  describe.skip('AWS KMS Signing Integration Tests (skipped)', () => {
    it('requires AWS credentials', () => {
      // AWS KMS integration tests skipped. To run these tests, set:
      // - AWS_REGION (e.g., us-east-1)
      // - AWS_ACCESS_KEY_ID
      // - AWS_SECRET_ACCESS_KEY
      // - AWS_KMS_EVM_KEY_ID (ARN or alias for EVM signing key)
    });
  });
}
