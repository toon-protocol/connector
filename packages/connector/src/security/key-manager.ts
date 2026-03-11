import { Logger } from 'pino';
import { AuditLogger } from './audit-logger';

/**
 * KeyManagerBackend interface for key storage and signing backends
 * Supports multiple backend types: environment variables, AWS KMS, GCP KMS, Azure Key Vault, HSM
 */
export interface KeyManagerBackend {
  sign(message: Buffer, keyId: string): Promise<Buffer>;
  getPublicKey(keyId: string): Promise<Buffer>;
  rotateKey(keyId: string): Promise<string>;
}

/**
 * AWS KMS configuration
 */
export interface AWSConfig {
  region: string;
  evmKeyId: string;
  credentials?: {
    accessKeyId: string;
    secretAccessKey: string;
  };
}

/**
 * GCP KMS configuration
 */
export interface GCPConfig {
  projectId: string;
  locationId: string;
  keyRingId: string;
  evmKeyId: string;
}

/**
 * Azure Key Vault configuration
 */
export interface AzureConfig {
  vaultUrl: string;
  evmKeyName: string;
  credentials?: {
    tenantId: string;
    clientId: string;
    clientSecret: string;
  };
}

/**
 * Hardware Security Module configuration (PKCS#11)
 */
export interface HSMConfig {
  pkcs11LibraryPath: string;
  slotId: number;
  pin: string;
  evmKeyLabel: string;
}

/**
 * Key rotation policy configuration
 */
export interface KeyRotationConfig {
  enabled: boolean;
  intervalDays: number; // Default: 90 days
  overlapDays: number; // Both old and new key valid during transition (default: 7 days)
  notifyBeforeDays: number; // Warning notification before rotation (default: 14 days)
}

/**
 * KeyManager configuration for backend selection and initialization
 */
export interface KeyManagerConfig {
  backend: 'env' | 'aws-kms' | 'gcp-kms' | 'azure-kv' | 'hsm';
  nodeId: string;
  /** Optional EVM private key for direct injection. Bypasses process.env.EVM_PRIVATE_KEY.
   *  Used by config-driven settlement to avoid env var mutation. */
  evmPrivateKey?: string;
  aws?: AWSConfig;
  gcp?: GCPConfig;
  azure?: AzureConfig;
  hsm?: HSMConfig;
  rotation?: KeyRotationConfig;
}

/**
 * Audit log entry structure for key operations
 */
export interface AuditLogEntry {
  event:
    | 'SIGN_REQUEST'
    | 'SIGN_SUCCESS'
    | 'SIGN_FAILURE'
    | 'KEY_ROTATION_START'
    | 'KEY_ROTATION_COMPLETE'
    | 'KEY_ACCESS_DENIED';
  keyId: string;
  timestamp: number;
  nodeId: string;
  backend: string;
  details?: Record<string, unknown>;
}

/**
 * KeyManager class provides enterprise-grade key management
 * with support for multiple backends (env, AWS KMS, GCP KMS, Azure Key Vault, HSM)
 */
export class KeyManager {
  private backend: KeyManagerBackend;
  private logger: Logger;
  private auditLogger: AuditLogger;

  constructor(config: KeyManagerConfig, logger: Logger) {
    this.logger = logger.child({ component: 'KeyManager' });

    // Initialize audit logger
    this.auditLogger = new AuditLogger(logger, {
      nodeId: config.nodeId,
      backend: config.backend,
    });

    // Select backend based on configuration
    switch (config.backend) {
      case 'env': {
        // Lazy import to avoid loading unnecessary dependencies
        // eslint-disable-next-line @typescript-eslint/no-var-requires
        const { EnvironmentVariableBackend } = require('./backends/environment-backend');
        this.backend = new EnvironmentVariableBackend(this.logger, {
          evmPrivateKey: config.evmPrivateKey,
        });
        break;
      }
      case 'aws-kms':
      case 'gcp-kms':
      case 'azure-kv':
      case 'hsm': {
        throw new Error(
          `Backend type '${config.backend}' is not supported. Only 'env' backend is available.`
        );
      }
      default:
        throw new Error(`Unknown backend type: ${config.backend}`);
    }

    this.logger.info({ backend: config.backend }, 'KeyManager initialized');
  }

  /**
   * Signs a message using the backend-specific signing mechanism
   * @param message - Message to sign
   * @param keyId - Key identifier (backend-specific format)
   * @returns Signature buffer compatible with EVM (ECDSA) verification
   */
  async sign(message: Buffer, keyId: string): Promise<Buffer> {
    const messageHash = message.toString('hex');

    // Log audit event: SIGN_REQUEST
    this.auditLogger.logSignRequest(keyId, messageHash);
    this.logger.debug({ keyId, messageLength: message.length }, 'Signing message');

    try {
      const signature = await this.backend.sign(message, keyId);
      const signatureHash = signature.toString('hex');

      // Log audit event: SIGN_SUCCESS
      this.auditLogger.logSignSuccess(keyId, signatureHash);
      this.logger.info({ keyId, signatureLength: signature.length }, 'Message signed successfully');

      return signature;
    } catch (error) {
      // Log audit event: SIGN_FAILURE
      this.auditLogger.logSignFailure(keyId, error as Error);
      this.logger.error({ keyId, error }, 'Message signing failed');
      throw error;
    }
  }

  /**
   * Retrieves public key for signature verification
   * @param keyId - Key identifier
   * @returns Public key buffer in format compatible with blockchain (secp256k1 for EVM)
   */
  async getPublicKey(keyId: string): Promise<Buffer> {
    this.logger.debug({ keyId }, 'Retrieving public key');

    try {
      const publicKey = await this.backend.getPublicKey(keyId);
      this.logger.info({ keyId, publicKeyLength: publicKey.length }, 'Public key retrieved');
      return publicKey;
    } catch (error) {
      this.logger.error({ keyId, error }, 'Public key retrieval failed');
      throw error;
    }
  }

  /**
   * Initiates key rotation: creates new key, maintains overlap period
   * @param keyId - Key identifier to rotate
   * @returns New key ID to update configuration
   */
  async rotateKey(keyId: string): Promise<string> {
    // Log audit event: KEY_ROTATION_START
    this.auditLogger.logKeyRotation(keyId, '', 'START');
    this.logger.info({ keyId }, 'Starting key rotation');

    try {
      const newKeyId = await this.backend.rotateKey(keyId);

      // Log audit event: KEY_ROTATION_COMPLETE
      this.auditLogger.logKeyRotation(keyId, newKeyId, 'COMPLETE');
      this.logger.info({ oldKeyId: keyId, newKeyId }, 'Key rotation completed');

      return newKeyId;
    } catch (error) {
      this.logger.error({ keyId, error }, 'Key rotation failed');
      throw error;
    }
  }
}
