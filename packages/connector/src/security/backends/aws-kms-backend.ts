import { Logger } from 'pino';
import type {
  KMSClient as KMSClientType,
  SigningAlgorithmSpec as SigningAlgorithmSpecType,
  KeySpec as KeySpecType,
} from '@aws-sdk/client-kms';
import { KeyManagerBackend, AWSConfig } from '../key-manager';
import { requireOptional } from '../../utils/optional-require';

/**
 * AWSKMSBackend implements KeyManagerBackend using AWS Key Management Service
 * Supports EVM (secp256k1) key type
 */
export class AWSKMSBackend implements KeyManagerBackend {
  private client: KMSClientType | null = null;
  private awsSdk: typeof import('@aws-sdk/client-kms') | null = null;
  private config: AWSConfig;
  private logger: Logger;

  constructor(config: AWSConfig, logger: Logger) {
    this.config = config;
    this.logger = logger.child({ component: 'AWSKMSBackend' });

    this.logger.info(
      { region: config.region, evmKeyId: config.evmKeyId },
      'AWSKMSBackend initialized'
    );
  }

  /**
   * Lazily loads the AWS SDK and initializes the KMS client
   */
  private async _getClient(): Promise<KMSClientType> {
    if (!this.client) {
      this.awsSdk = await requireOptional<typeof import('@aws-sdk/client-kms')>(
        '@aws-sdk/client-kms',
        'AWS KMS key management'
      );
      this.client = new this.awsSdk.KMSClient({
        region: this.config.region,
        credentials: this.config.credentials,
      });
    }
    return this.client;
  }

  /**
   * Lazily loads the AWS SDK module
   */
  private async _getSdk(): Promise<typeof import('@aws-sdk/client-kms')> {
    if (!this.awsSdk) {
      this.awsSdk = await requireOptional<typeof import('@aws-sdk/client-kms')>(
        '@aws-sdk/client-kms',
        'AWS KMS key management'
      );
    }
    return this.awsSdk;
  }

  /**
   * Detects key type based on keyId
   * @param keyId - Key identifier (ARN or alias)
   * @returns Key type (always 'evm' for EVM-only connector)
   */
  private _detectKeyType(_keyId: string): 'evm' {
    // EVM-only connector - always return 'evm'
    return 'evm';
  }

  /**
   * Gets the signing algorithm for EVM keys
   * @returns AWS KMS signing algorithm (ECDSA_SHA_256 for secp256k1)
   */
  private async _getSigningAlgorithm(): Promise<SigningAlgorithmSpecType> {
    const sdk = await this._getSdk();
    return sdk.SigningAlgorithmSpec.ECDSA_SHA_256;
  }

  /**
   * Gets the key spec for EVM key creation
   * @returns AWS KMS key spec (secp256k1)
   */
  private async _getKeySpec(): Promise<KeySpecType> {
    const sdk = await this._getSdk();
    return sdk.KeySpec.ECC_SECG_P256K1; // secp256k1 for EVM
  }

  /**
   * Signs a message using AWS KMS
   * @param message - Message to sign
   * @param keyId - AWS KMS key ID or ARN
   * @returns Signature buffer
   */
  async sign(message: Buffer, keyId: string): Promise<Buffer> {
    const keyType = this._detectKeyType(keyId);
    const signingAlgorithm = await this._getSigningAlgorithm();
    const sdk = await this._getSdk();
    const client = await this._getClient();

    this.logger.debug({ keyId, keyType, signingAlgorithm }, 'Signing with AWS KMS');

    try {
      const command = new sdk.SignCommand({
        KeyId: keyId,
        Message: message,
        SigningAlgorithm: signingAlgorithm,
        MessageType: 'RAW', // Sign raw message (not digest)
      });

      const response = await client.send(command);

      if (!response.Signature) {
        throw new Error('AWS KMS returned no signature');
      }

      const signature = Buffer.from(response.Signature);
      this.logger.info({ keyId, signatureLength: signature.length }, 'AWS KMS signature generated');

      return signature;
    } catch (error) {
      this.logger.error({ keyId, error }, 'AWS KMS signing failed');
      throw error;
    }
  }

  /**
   * Retrieves public key from AWS KMS
   * @param keyId - AWS KMS key ID or ARN
   * @returns Public key buffer
   */
  async getPublicKey(keyId: string): Promise<Buffer> {
    this.logger.debug({ keyId }, 'Retrieving public key from AWS KMS');
    const sdk = await this._getSdk();
    const client = await this._getClient();

    try {
      const command = new sdk.GetPublicKeyCommand({
        KeyId: keyId,
      });

      const response = await client.send(command);

      if (!response.PublicKey) {
        throw new Error('AWS KMS returned no public key');
      }

      const publicKey = Buffer.from(response.PublicKey);
      this.logger.info(
        { keyId, publicKeyLength: publicKey.length },
        'AWS KMS public key retrieved'
      );

      return publicKey;
    } catch (error) {
      this.logger.error({ keyId, error }, 'AWS KMS public key retrieval failed');
      throw error;
    }
  }

  /**
   * Creates a new AWS KMS key for rotation
   * @param keyId - Current key ID (used to determine key type)
   * @returns New key ID (ARN)
   */
  async rotateKey(keyId: string): Promise<string> {
    const keyType = this._detectKeyType(keyId);
    const keySpec = await this._getKeySpec();
    const sdk = await this._getSdk();
    const client = await this._getClient();

    this.logger.info(
      { oldKeyId: keyId, keyType, keySpec },
      'Creating new AWS KMS key for rotation'
    );

    try {
      const command = new sdk.CreateKeyCommand({
        KeyUsage: sdk.KeyUsageType.SIGN_VERIFY,
        KeySpec: keySpec,
        Description: `Rotated ${keyType.toUpperCase()} key from ${keyId}`,
        Tags: [
          {
            TagKey: 'Purpose',
            TagValue: 'ILP-Connector-Settlement',
          },
          {
            TagKey: 'KeyType',
            TagValue: keyType.toUpperCase(),
          },
          {
            TagKey: 'RotatedFrom',
            TagValue: keyId,
          },
        ],
      });

      const response = await client.send(command);

      if (!response.KeyMetadata?.Arn) {
        throw new Error('AWS KMS returned no key ARN');
      }

      const newKeyId = response.KeyMetadata.Arn;
      this.logger.info({ oldKeyId: keyId, newKeyId }, 'AWS KMS key rotation completed');

      return newKeyId;
    } catch (error) {
      this.logger.error({ keyId, error }, 'AWS KMS key rotation failed');
      throw error;
    }
  }
}
