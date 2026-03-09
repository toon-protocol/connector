import {
  KeyManagerConfig,
  AWSConfig,
  GCPConfig,
  AzureConfig,
  HSMConfig,
} from '../security/key-manager';

/**
 * Configuration errors thrown during config loading or validation
 */
export class ConfigurationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ConfigurationError';
  }
}

/**
 * Load KeyManager configuration from environment variables
 *
 * Environment variables:
 * - KEY_BACKEND: Backend type (env | aws-kms | gcp-kms | azure-kv | hsm)
 * - NODE_ID: Node identifier for audit logging
 *
 * AWS KMS (when KEY_BACKEND=aws-kms):
 * - AWS_REGION: AWS region (e.g., 'us-east-1')
 * - AWS_KMS_EVM_KEY_ID: ARN or alias for EVM signing key
 * - AWS_ACCESS_KEY_ID: (Optional) AWS access key
 * - AWS_SECRET_ACCESS_KEY: (Optional) AWS secret key
 *
 * GCP KMS (when KEY_BACKEND=gcp-kms):
 * - GCP_PROJECT_ID: GCP project ID
 * - GCP_LOCATION_ID: KMS location (e.g., 'us-east1')
 * - GCP_KEY_RING_ID: KMS key ring name
 * - GCP_KMS_EVM_KEY_ID: Crypto key name for EVM signing
 *
 * Azure Key Vault (when KEY_BACKEND=azure-kv):
 * - AZURE_VAULT_URL: Key Vault URL (e.g., 'https://myvault.vault.azure.net/')
 * - AZURE_EVM_KEY_NAME: Key name for EVM signing
 * - AZURE_TENANT_ID: (Optional) Azure AD tenant ID
 * - AZURE_CLIENT_ID: (Optional) Service principal client ID
 * - AZURE_CLIENT_SECRET: (Optional) Service principal secret
 *
 * HSM (when KEY_BACKEND=hsm):
 * - HSM_PKCS11_LIBRARY_PATH: Path to PKCS#11 library (e.g., '/usr/lib/softhsm/libsofthsm2.so')
 * - HSM_SLOT_ID: HSM slot ID (0-based index)
 * - HSM_PIN: HSM PIN (from environment variable, never hardcoded)
 * - HSM_EVM_KEY_LABEL: Key pair label for EVM signing
 *
 * @returns KeyManagerConfig loaded from environment variables
 * @throws ConfigurationError if required variables are missing or invalid
 */
export function loadKeyManagerConfig(): KeyManagerConfig {
  const backend = (process.env.KEY_BACKEND || 'env') as KeyManagerConfig['backend'];
  const nodeId = process.env.NODE_ID || 'connector-node';

  // Validate backend type
  const validBackends = ['env', 'aws-kms', 'gcp-kms', 'azure-kv', 'hsm'];
  if (!validBackends.includes(backend)) {
    throw new ConfigurationError(
      `Invalid KEY_BACKEND: ${backend}. Must be one of: ${validBackends.join(', ')}`
    );
  }

  const config: KeyManagerConfig = {
    backend,
    nodeId,
  };

  // Load backend-specific configuration
  switch (backend) {
    case 'env':
      // No additional config required for environment backend
      break;

    case 'aws-kms':
      config.aws = loadAWSConfig();
      break;

    case 'gcp-kms':
      config.gcp = loadGCPConfig();
      break;

    case 'azure-kv':
      config.azure = loadAzureConfig();
      break;

    case 'hsm':
      config.hsm = loadHSMConfig();
      break;
  }

  // Validate configuration
  validateKeyManagerConfig(config);

  return config;
}

/**
 * Load AWS KMS configuration from environment variables
 */
function loadAWSConfig(): AWSConfig {
  const region = process.env.AWS_REGION;
  const evmKeyId = process.env.AWS_KMS_EVM_KEY_ID;

  if (!region) {
    throw new ConfigurationError('AWS_REGION required for aws-kms backend');
  }
  if (!evmKeyId) {
    throw new ConfigurationError('AWS_KMS_EVM_KEY_ID required for aws-kms backend');
  }

  const config: AWSConfig = {
    region,
    evmKeyId,
  };

  // Optional credentials (uses IAM role if not provided)
  const accessKeyId = process.env.AWS_ACCESS_KEY_ID;
  const secretAccessKey = process.env.AWS_SECRET_ACCESS_KEY;
  if (accessKeyId && secretAccessKey) {
    config.credentials = {
      accessKeyId,
      secretAccessKey,
    };
  }

  return config;
}

/**
 * Load GCP KMS configuration from environment variables
 */
function loadGCPConfig(): GCPConfig {
  const projectId = process.env.GCP_PROJECT_ID;
  const locationId = process.env.GCP_LOCATION_ID;
  const keyRingId = process.env.GCP_KEY_RING_ID;
  const evmKeyId = process.env.GCP_KMS_EVM_KEY_ID;

  if (!projectId) {
    throw new ConfigurationError('GCP_PROJECT_ID required for gcp-kms backend');
  }
  if (!locationId) {
    throw new ConfigurationError('GCP_LOCATION_ID required for gcp-kms backend');
  }
  if (!keyRingId) {
    throw new ConfigurationError('GCP_KEY_RING_ID required for gcp-kms backend');
  }
  if (!evmKeyId) {
    throw new ConfigurationError('GCP_KMS_EVM_KEY_ID required for gcp-kms backend');
  }

  return {
    projectId,
    locationId,
    keyRingId,
    evmKeyId,
  };
}

/**
 * Load Azure Key Vault configuration from environment variables
 */
function loadAzureConfig(): AzureConfig {
  const vaultUrl = process.env.AZURE_VAULT_URL;
  const evmKeyName = process.env.AZURE_EVM_KEY_NAME;

  if (!vaultUrl) {
    throw new ConfigurationError('AZURE_VAULT_URL required for azure-kv backend');
  }
  if (!evmKeyName) {
    throw new ConfigurationError('AZURE_EVM_KEY_NAME required for azure-kv backend');
  }

  const config: AzureConfig = {
    vaultUrl,
    evmKeyName,
  };

  // Optional credentials (uses DefaultAzureCredential if not provided)
  const tenantId = process.env.AZURE_TENANT_ID;
  const clientId = process.env.AZURE_CLIENT_ID;
  const clientSecret = process.env.AZURE_CLIENT_SECRET;
  if (tenantId && clientId && clientSecret) {
    config.credentials = {
      tenantId,
      clientId,
      clientSecret,
    };
  }

  return config;
}

/**
 * Load HSM configuration from environment variables
 */
function loadHSMConfig(): HSMConfig {
  const pkcs11LibraryPath = process.env.HSM_PKCS11_LIBRARY_PATH;
  const slotIdStr = process.env.HSM_SLOT_ID;
  const pin = process.env.HSM_PIN;
  const evmKeyLabel = process.env.HSM_EVM_KEY_LABEL;

  if (!pkcs11LibraryPath) {
    throw new ConfigurationError('HSM_PKCS11_LIBRARY_PATH required for hsm backend');
  }
  if (!slotIdStr) {
    throw new ConfigurationError('HSM_SLOT_ID required for hsm backend');
  }
  if (!pin) {
    throw new ConfigurationError('HSM_PIN required for hsm backend');
  }
  if (!evmKeyLabel) {
    throw new ConfigurationError('HSM_EVM_KEY_LABEL required for hsm backend');
  }

  const slotId = parseInt(slotIdStr, 10);
  if (isNaN(slotId) || slotId < 0) {
    throw new ConfigurationError(
      `Invalid HSM_SLOT_ID: ${slotIdStr}. Must be non-negative integer.`
    );
  }

  return {
    pkcs11LibraryPath,
    slotId,
    pin,
    evmKeyLabel,
  };
}

/**
 * Validate KeyManager configuration
 *
 * Checks:
 * - Backend-specific config present
 * - Key ID formats valid (ARN for AWS, resource name for GCP, URL for Azure)
 * - Required fields not empty
 *
 * @param config KeyManagerConfig to validate
 * @throws ConfigurationError if validation fails
 */
export function validateKeyManagerConfig(config: KeyManagerConfig): void {
  if (!config.nodeId || config.nodeId.trim() === '') {
    throw new ConfigurationError('nodeId cannot be empty');
  }

  switch (config.backend) {
    case 'env':
      // No validation required for environment backend
      break;

    case 'aws-kms':
      if (!config.aws) {
        throw new ConfigurationError('AWS configuration required for aws-kms backend');
      }
      validateAWSKeyIds(config.aws);
      break;

    case 'gcp-kms':
      if (!config.gcp) {
        throw new ConfigurationError('GCP configuration required for gcp-kms backend');
      }
      validateGCPKeyIds(config.gcp);
      break;

    case 'azure-kv':
      if (!config.azure) {
        throw new ConfigurationError('Azure configuration required for azure-kv backend');
      }
      validateAzureKeyIds(config.azure);
      break;

    case 'hsm':
      if (!config.hsm) {
        throw new ConfigurationError('HSM configuration required for hsm backend');
      }
      validateHSMConfig(config.hsm);
      break;
  }
}

/**
 * Validate AWS KMS key IDs (ARN or alias format)
 */
function validateAWSKeyIds(config: AWSConfig): void {
  // AWS key IDs can be ARN, alias, or key ID
  // ARN format: arn:aws:kms:region:account-id:key/key-id
  // Alias format: alias/key-alias
  // Key ID format: UUID
  const validArnPattern = /^(arn:aws:kms:[a-z0-9-]+:\d{12}:key\/|alias\/|[a-f0-9-]{36}$)/;

  if (!validArnPattern.test(config.evmKeyId)) {
    throw new ConfigurationError(
      `Invalid AWS_KMS_EVM_KEY_ID format: ${config.evmKeyId}. Must be ARN, alias, or key ID.`
    );
  }
}

/**
 * Validate GCP KMS key names (must not be empty)
 */
function validateGCPKeyIds(config: GCPConfig): void {
  // GCP key names are alphanumeric + hyphens, underscores
  if (!config.evmKeyId || config.evmKeyId.trim() === '') {
    throw new ConfigurationError('GCP_KMS_EVM_KEY_ID cannot be empty');
  }
}

/**
 * Validate Azure Key Vault key names and vault URL
 */
function validateAzureKeyIds(config: AzureConfig): void {
  // Validate vault URL format
  if (!config.vaultUrl.startsWith('https://') || !config.vaultUrl.includes('.vault.azure.net')) {
    throw new ConfigurationError(
      `Invalid AZURE_VAULT_URL format: ${config.vaultUrl}. Must be https://<vault-name>.vault.azure.net/`
    );
  }

  // Key names must not be empty
  if (!config.evmKeyName || config.evmKeyName.trim() === '') {
    throw new ConfigurationError('AZURE_EVM_KEY_NAME cannot be empty');
  }
}

/**
 * Validate HSM configuration
 */
function validateHSMConfig(config: HSMConfig): void {
  // Validate library path is not empty
  if (!config.pkcs11LibraryPath || config.pkcs11LibraryPath.trim() === '') {
    throw new ConfigurationError('HSM_PKCS11_LIBRARY_PATH cannot be empty');
  }

  // Validate slot ID is non-negative
  if (config.slotId < 0) {
    throw new ConfigurationError(`HSM_SLOT_ID must be non-negative: ${config.slotId}`);
  }

  // Validate PIN is not empty
  if (!config.pin || config.pin.trim() === '') {
    throw new ConfigurationError('HSM_PIN cannot be empty');
  }

  // Validate key labels are not empty
  if (!config.evmKeyLabel || config.evmKeyLabel.trim() === '') {
    throw new ConfigurationError('HSM_EVM_KEY_LABEL cannot be empty');
  }
}
