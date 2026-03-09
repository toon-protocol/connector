import { Logger } from 'pino';
import type { KeyClient as KeyClientType } from '@azure/keyvault-keys';
import { KeyManagerBackend, AzureConfig } from '../key-manager';
import { requireOptional } from '../../utils/optional-require';

/**
 * AzureKeyVaultBackend implements KeyManagerBackend using Azure Key Vault
 * Supports EVM (secp256k1) key type
 */
export class AzureKeyVaultBackend implements KeyManagerBackend {
  private keyClient: KeyClientType | null = null;
  private kvSdk: typeof import('@azure/keyvault-keys') | null = null;
  private config: AzureConfig;
  private logger: Logger;

  constructor(config: AzureConfig, logger: Logger) {
    this.config = config;
    this.logger = logger.child({ component: 'AzureKeyVaultBackend' });

    this.logger.info(
      { vaultUrl: config.vaultUrl, evmKeyName: config.evmKeyName },
      'AzureKeyVaultBackend initialized'
    );
  }

  /**
   * Lazily loads the Azure SDKs and initializes the Key Vault client
   */
  private async _getKeyClient(): Promise<KeyClientType> {
    if (!this.keyClient) {
      const identitySdk = await requireOptional<typeof import('@azure/identity')>(
        '@azure/identity',
        'Azure Key Vault authentication'
      );
      this.kvSdk = await requireOptional<typeof import('@azure/keyvault-keys')>(
        '@azure/keyvault-keys',
        'Azure Key Vault key management'
      );

      let credential;
      if (this.config.credentials) {
        credential = new identitySdk.ClientSecretCredential(
          this.config.credentials.tenantId,
          this.config.credentials.clientId,
          this.config.credentials.clientSecret
        );
      } else {
        credential = new identitySdk.DefaultAzureCredential();
      }

      this.keyClient = new this.kvSdk.KeyClient(this.config.vaultUrl, credential);
    }
    return this.keyClient;
  }

  /**
   * Lazily loads the Azure Key Vault SDK module
   */
  private async _getKvSdk(): Promise<typeof import('@azure/keyvault-keys')> {
    if (!this.kvSdk) {
      this.kvSdk = await requireOptional<typeof import('@azure/keyvault-keys')>(
        '@azure/keyvault-keys',
        'Azure Key Vault key management'
      );
    }
    return this.kvSdk;
  }

  /**
   * Detects key type based on keyName
   * @param keyName - Key name in Azure Key Vault
   * @returns Key type (always 'evm' for EVM-only connector)
   */
  private _detectKeyType(_keyName: string): 'evm' {
    // EVM-only connector - always return 'evm'
    return 'evm';
  }

  /**
   * Gets the appropriate signing algorithm for Azure Key Vault
   * @param _keyType - Key type ('evm')
   * @returns Azure signing algorithm
   */
  private _getSignAlgorithm(_keyType: 'evm'): string {
    return 'ES256K'; // secp256k1 with SHA-256
  }

  /**
   * Signs a message using Azure Key Vault
   * @param message - Message to sign
   * @param keyName - Azure Key Vault key name
   * @returns Signature buffer
   */
  async sign(message: Buffer, keyName: string): Promise<Buffer> {
    const keyType = this._detectKeyType(keyName);
    const algorithm = this._getSignAlgorithm(keyType);

    this.logger.debug({ keyName, keyType, algorithm }, 'Signing with Azure Key Vault');

    try {
      const keyClient = await this._getKeyClient();
      const kvSdk = await this._getKvSdk();

      // Get the key to create a CryptographyClient
      const key = await keyClient.getKey(keyName);

      if (!key.id) {
        throw new Error('Azure Key Vault returned no key ID');
      }

      // Create cryptography client for signing
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const cryptoClient = new kvSdk.CryptographyClient(key, (keyClient as any)['credential']);

      // Azure Key Vault requires message digest (SHA256 for ES256K)
      // eslint-disable-next-line @typescript-eslint/no-var-requires
      const crypto = require('crypto');
      const digest = crypto.createHash('sha256').update(message).digest();

      // Sign the digest
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const result = await cryptoClient.sign(algorithm as any, digest);

      if (!result.result) {
        throw new Error('Azure Key Vault returned no signature');
      }

      const signature = Buffer.from(result.result);
      this.logger.info(
        { keyName, signatureLength: signature.length },
        'Azure Key Vault signature generated'
      );

      return signature;
    } catch (error) {
      this.logger.error({ keyName, error }, 'Azure Key Vault signing failed');
      throw error;
    }
  }

  /**
   * Retrieves public key from Azure Key Vault
   * @param keyName - Azure Key Vault key name
   * @returns Public key buffer
   */
  async getPublicKey(keyName: string): Promise<Buffer> {
    this.logger.debug({ keyName }, 'Retrieving public key from Azure Key Vault');

    try {
      const keyClient = await this._getKeyClient();
      const key = await keyClient.getKey(keyName);

      if (!key.key) {
        throw new Error('Azure Key Vault returned no public key');
      }

      // Extract public key from JWK format
      // For EC keys, we need to combine x and y coordinates
      if (key.key.x && key.key.y) {
        const xBuffer = Buffer.from(key.key.x);
        const yBuffer = Buffer.from(key.key.y);

        // Combine x and y for uncompressed public key format (0x04 + x + y)
        const publicKey = Buffer.concat([Buffer.from([0x04]), xBuffer, yBuffer]);

        this.logger.info(
          { keyName, publicKeyLength: publicKey.length },
          'Azure Key Vault public key retrieved'
        );

        return publicKey;
      } else {
        throw new Error('Azure Key Vault key missing x or y coordinates');
      }
    } catch (error) {
      this.logger.error({ keyName, error }, 'Azure Key Vault public key retrieval failed');
      throw error;
    }
  }

  /**
   * Creates a new Azure Key Vault key for rotation
   * @param keyName - Current key name
   * @returns New key name
   */
  async rotateKey(keyName: string): Promise<string> {
    const keyType = this._detectKeyType(keyName);

    this.logger.info(
      { oldKeyName: keyName, keyType },
      'Creating new Azure Key Vault key for rotation'
    );

    try {
      const keyClient = await this._getKeyClient();

      // Azure Key Vault supports key rotation via creating a new key version
      // For manual rotation, we create a new key with a suffix
      const newKeyName = `${keyName}-rotated-${Date.now()}`;

      // Determine key type and curve
      const curve = keyType === 'evm' ? 'SECP256K1' : 'Ed25519';

      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const newKey = await keyClient.createKey(newKeyName, curve as any, {
        keyOps: ['sign', 'verify'],
        tags: {
          purpose: 'ILP-Connector-Settlement',
          keyType: keyType.toUpperCase(),
          rotatedFrom: keyName,
        },
      });

      if (!newKey.name) {
        throw new Error('Azure Key Vault returned no key name');
      }

      this.logger.info(
        { oldKeyName: keyName, newKeyName: newKey.name },
        'Azure Key Vault key rotation completed'
      );

      return newKey.name;
    } catch (error) {
      this.logger.error({ keyName, error }, 'Azure Key Vault key rotation failed');
      throw error;
    }
  }
}
