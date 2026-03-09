import {
  loadKeyManagerConfig,
  validateKeyManagerConfig,
  ConfigurationError,
} from './key-manager-config';

describe('loadKeyManagerConfig', () => {
  let originalEnv: NodeJS.ProcessEnv;

  beforeEach(() => {
    // Save original environment
    originalEnv = { ...process.env };

    // Clear relevant environment variables
    delete process.env.KEY_BACKEND;
    delete process.env.NODE_ID;
    delete process.env.AWS_REGION;
    delete process.env.AWS_KMS_EVM_KEY_ID;
    delete process.env.AWS_ACCESS_KEY_ID;
    delete process.env.AWS_SECRET_ACCESS_KEY;
    delete process.env.GCP_PROJECT_ID;
    delete process.env.GCP_LOCATION_ID;
    delete process.env.GCP_KEY_RING_ID;
    delete process.env.GCP_KMS_EVM_KEY_ID;
    delete process.env.AZURE_VAULT_URL;
    delete process.env.AZURE_EVM_KEY_NAME;
    delete process.env.AZURE_TENANT_ID;
    delete process.env.AZURE_CLIENT_ID;
    delete process.env.AZURE_CLIENT_SECRET;
    delete process.env.HSM_PKCS11_LIBRARY_PATH;
    delete process.env.HSM_SLOT_ID;
    delete process.env.HSM_PIN;
    delete process.env.HSM_EVM_KEY_LABEL;
  });

  afterEach(() => {
    // Restore original environment
    process.env = originalEnv;
  });

  describe('backend selection', () => {
    it('should default to env backend if KEY_BACKEND not set', () => {
      const config = loadKeyManagerConfig();
      expect(config.backend).toBe('env');
    });

    it('should load env backend configuration', () => {
      process.env.KEY_BACKEND = 'env';
      process.env.NODE_ID = 'test-node';

      const config = loadKeyManagerConfig();

      expect(config.backend).toBe('env');
      expect(config.nodeId).toBe('test-node');
      expect(config.aws).toBeUndefined();
      expect(config.gcp).toBeUndefined();
      expect(config.azure).toBeUndefined();
      expect(config.hsm).toBeUndefined();
    });

    it('should throw error for invalid backend type', () => {
      process.env.KEY_BACKEND = 'invalid-backend';

      expect(() => loadKeyManagerConfig()).toThrow(ConfigurationError);
      expect(() => loadKeyManagerConfig()).toThrow(/Invalid KEY_BACKEND/);
    });
  });

  describe('AWS KMS configuration', () => {
    it('should load AWS KMS configuration', () => {
      process.env.KEY_BACKEND = 'aws-kms';
      process.env.NODE_ID = 'test-node';
      process.env.AWS_REGION = 'us-east-1';
      process.env.AWS_KMS_EVM_KEY_ID = 'arn:aws:kms:us-east-1:123456789012:key/evm-key';

      const config = loadKeyManagerConfig();

      expect(config.backend).toBe('aws-kms');
      expect(config.aws).toBeDefined();
      expect(config.aws?.region).toBe('us-east-1');
      expect(config.aws?.evmKeyId).toBe('arn:aws:kms:us-east-1:123456789012:key/evm-key');
    });

    it('should include AWS credentials if provided', () => {
      process.env.KEY_BACKEND = 'aws-kms';
      process.env.AWS_REGION = 'us-east-1';
      process.env.AWS_KMS_EVM_KEY_ID = 'alias/evm-key';
      process.env.AWS_ACCESS_KEY_ID = 'AKIAIOSFODNN7EXAMPLE';
      process.env.AWS_SECRET_ACCESS_KEY = 'wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY';

      const config = loadKeyManagerConfig();

      expect(config.aws?.credentials).toBeDefined();
      expect(config.aws?.credentials?.accessKeyId).toBe('AKIAIOSFODNN7EXAMPLE');
      expect(config.aws?.credentials?.secretAccessKey).toBe(
        'wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY'
      );
    });

    it('should throw error if AWS_REGION missing', () => {
      process.env.KEY_BACKEND = 'aws-kms';
      process.env.AWS_KMS_EVM_KEY_ID = 'key-id';

      expect(() => loadKeyManagerConfig()).toThrow(ConfigurationError);
      expect(() => loadKeyManagerConfig()).toThrow(/AWS_REGION required/);
    });

    it('should throw error if AWS_KMS_EVM_KEY_ID missing', () => {
      process.env.KEY_BACKEND = 'aws-kms';
      process.env.AWS_REGION = 'us-east-1';

      expect(() => loadKeyManagerConfig()).toThrow(ConfigurationError);
      expect(() => loadKeyManagerConfig()).toThrow(/AWS_KMS_EVM_KEY_ID required/);
    });
  });

  describe('GCP KMS configuration', () => {
    it('should load GCP KMS configuration', () => {
      process.env.KEY_BACKEND = 'gcp-kms';
      process.env.GCP_PROJECT_ID = 'my-project';
      process.env.GCP_LOCATION_ID = 'us-east1';
      process.env.GCP_KEY_RING_ID = 'my-keyring';
      process.env.GCP_KMS_EVM_KEY_ID = 'evm-key';

      const config = loadKeyManagerConfig();

      expect(config.backend).toBe('gcp-kms');
      expect(config.gcp).toBeDefined();
      expect(config.gcp?.projectId).toBe('my-project');
      expect(config.gcp?.locationId).toBe('us-east1');
      expect(config.gcp?.keyRingId).toBe('my-keyring');
      expect(config.gcp?.evmKeyId).toBe('evm-key');
    });

    it('should throw error if GCP_PROJECT_ID missing', () => {
      process.env.KEY_BACKEND = 'gcp-kms';
      process.env.GCP_LOCATION_ID = 'us-east1';
      process.env.GCP_KEY_RING_ID = 'my-keyring';
      process.env.GCP_KMS_EVM_KEY_ID = 'evm-key';

      expect(() => loadKeyManagerConfig()).toThrow(ConfigurationError);
      expect(() => loadKeyManagerConfig()).toThrow(/GCP_PROJECT_ID required/);
    });
  });

  describe('Azure Key Vault configuration', () => {
    it('should load Azure Key Vault configuration', () => {
      process.env.KEY_BACKEND = 'azure-kv';
      process.env.AZURE_VAULT_URL = 'https://myvault.vault.azure.net/';
      process.env.AZURE_EVM_KEY_NAME = 'evm-key';

      const config = loadKeyManagerConfig();

      expect(config.backend).toBe('azure-kv');
      expect(config.azure).toBeDefined();
      expect(config.azure?.vaultUrl).toBe('https://myvault.vault.azure.net/');
      expect(config.azure?.evmKeyName).toBe('evm-key');
    });

    it('should include Azure credentials if provided', () => {
      process.env.KEY_BACKEND = 'azure-kv';
      process.env.AZURE_VAULT_URL = 'https://myvault.vault.azure.net/';
      process.env.AZURE_EVM_KEY_NAME = 'evm-key';
      process.env.AZURE_TENANT_ID = 'tenant-id';
      process.env.AZURE_CLIENT_ID = 'client-id';
      process.env.AZURE_CLIENT_SECRET = 'client-secret';

      const config = loadKeyManagerConfig();

      expect(config.azure?.credentials).toBeDefined();
      expect(config.azure?.credentials?.tenantId).toBe('tenant-id');
      expect(config.azure?.credentials?.clientId).toBe('client-id');
      expect(config.azure?.credentials?.clientSecret).toBe('client-secret');
    });

    it('should throw error if AZURE_VAULT_URL missing', () => {
      process.env.KEY_BACKEND = 'azure-kv';
      process.env.AZURE_EVM_KEY_NAME = 'evm-key';

      expect(() => loadKeyManagerConfig()).toThrow(ConfigurationError);
      expect(() => loadKeyManagerConfig()).toThrow(/AZURE_VAULT_URL required/);
    });
  });

  describe('HSM configuration', () => {
    it('should load HSM configuration', () => {
      process.env.KEY_BACKEND = 'hsm';
      process.env.HSM_PKCS11_LIBRARY_PATH = '/usr/lib/softhsm/libsofthsm2.so';
      process.env.HSM_SLOT_ID = '0';
      process.env.HSM_PIN = '1234';
      process.env.HSM_EVM_KEY_LABEL = 'evm-key';

      const config = loadKeyManagerConfig();

      expect(config.backend).toBe('hsm');
      expect(config.hsm).toBeDefined();
      expect(config.hsm?.pkcs11LibraryPath).toBe('/usr/lib/softhsm/libsofthsm2.so');
      expect(config.hsm?.slotId).toBe(0);
      expect(config.hsm?.pin).toBe('1234');
      expect(config.hsm?.evmKeyLabel).toBe('evm-key');
    });

    it('should throw error if HSM_SLOT_ID is invalid', () => {
      process.env.KEY_BACKEND = 'hsm';
      process.env.HSM_PKCS11_LIBRARY_PATH = '/usr/lib/softhsm/libsofthsm2.so';
      process.env.HSM_SLOT_ID = 'invalid';
      process.env.HSM_PIN = '1234';
      process.env.HSM_EVM_KEY_LABEL = 'evm-key';

      expect(() => loadKeyManagerConfig()).toThrow(ConfigurationError);
      expect(() => loadKeyManagerConfig()).toThrow(/Invalid HSM_SLOT_ID/);
    });

    it('should throw error if HSM_SLOT_ID is negative', () => {
      process.env.KEY_BACKEND = 'hsm';
      process.env.HSM_PKCS11_LIBRARY_PATH = '/usr/lib/softhsm/libsofthsm2.so';
      process.env.HSM_SLOT_ID = '-1';
      process.env.HSM_PIN = '1234';
      process.env.HSM_EVM_KEY_LABEL = 'evm-key';

      expect(() => loadKeyManagerConfig()).toThrow(ConfigurationError);
      expect(() => loadKeyManagerConfig()).toThrow(/Invalid HSM_SLOT_ID/);
    });
  });
});

describe('validateKeyManagerConfig', () => {
  it('should validate env backend config', () => {
    const config = {
      backend: 'env' as const,
      nodeId: 'test-node',
    };

    expect(() => validateKeyManagerConfig(config)).not.toThrow();
  });

  it('should throw error if nodeId is empty', () => {
    const config = {
      backend: 'env' as const,
      nodeId: '',
    };

    expect(() => validateKeyManagerConfig(config)).toThrow(ConfigurationError);
    expect(() => validateKeyManagerConfig(config)).toThrow(/nodeId cannot be empty/);
  });

  it('should validate AWS KMS config', () => {
    const config = {
      backend: 'aws-kms' as const,
      nodeId: 'test-node',
      aws: {
        region: 'us-east-1',
        evmKeyId: 'arn:aws:kms:us-east-1:123456789012:key/evm-key',
      },
    };

    expect(() => validateKeyManagerConfig(config)).not.toThrow();
  });

  it('should throw error for invalid AWS ARN format', () => {
    const config = {
      backend: 'aws-kms' as const,
      nodeId: 'test-node',
      aws: {
        region: 'us-east-1',
        evmKeyId: 'invalid-key-id',
      },
    };

    expect(() => validateKeyManagerConfig(config)).toThrow(ConfigurationError);
    expect(() => validateKeyManagerConfig(config)).toThrow(/Invalid AWS_KMS_EVM_KEY_ID format/);
  });

  it('should accept AWS alias format', () => {
    const config = {
      backend: 'aws-kms' as const,
      nodeId: 'test-node',
      aws: {
        region: 'us-east-1',
        evmKeyId: 'alias/evm-key',
      },
    };

    expect(() => validateKeyManagerConfig(config)).not.toThrow();
  });

  it('should accept AWS key ID (UUID) format', () => {
    const config = {
      backend: 'aws-kms' as const,
      nodeId: 'test-node',
      aws: {
        region: 'us-east-1',
        evmKeyId: '12345678-1234-1234-1234-123456789012',
      },
    };

    expect(() => validateKeyManagerConfig(config)).not.toThrow();
  });

  it('should validate GCP KMS config', () => {
    const config = {
      backend: 'gcp-kms' as const,
      nodeId: 'test-node',
      gcp: {
        projectId: 'my-project',
        locationId: 'us-east1',
        keyRingId: 'my-keyring',
        evmKeyId: 'evm-key',
      },
    };

    expect(() => validateKeyManagerConfig(config)).not.toThrow();
  });

  it('should throw error if GCP key ID is empty', () => {
    const config = {
      backend: 'gcp-kms' as const,
      nodeId: 'test-node',
      gcp: {
        projectId: 'my-project',
        locationId: 'us-east1',
        keyRingId: 'my-keyring',
        evmKeyId: '',
      },
    };

    expect(() => validateKeyManagerConfig(config)).toThrow(ConfigurationError);
    expect(() => validateKeyManagerConfig(config)).toThrow(/GCP_KMS_EVM_KEY_ID cannot be empty/);
  });

  it('should validate Azure Key Vault config', () => {
    const config = {
      backend: 'azure-kv' as const,
      nodeId: 'test-node',
      azure: {
        vaultUrl: 'https://myvault.vault.azure.net/',
        evmKeyName: 'evm-key',
      },
    };

    expect(() => validateKeyManagerConfig(config)).not.toThrow();
  });

  it('should throw error for invalid Azure vault URL', () => {
    const config = {
      backend: 'azure-kv' as const,
      nodeId: 'test-node',
      azure: {
        vaultUrl: 'http://invalid-vault.com',
        evmKeyName: 'evm-key',
      },
    };

    expect(() => validateKeyManagerConfig(config)).toThrow(ConfigurationError);
    expect(() => validateKeyManagerConfig(config)).toThrow(/Invalid AZURE_VAULT_URL format/);
  });

  it('should validate HSM config', () => {
    const config = {
      backend: 'hsm' as const,
      nodeId: 'test-node',
      hsm: {
        pkcs11LibraryPath: '/usr/lib/softhsm/libsofthsm2.so',
        slotId: 0,
        pin: '1234',
        evmKeyLabel: 'evm-key',
      },
    };

    expect(() => validateKeyManagerConfig(config)).not.toThrow();
  });

  it('should throw error if HSM slot ID is negative', () => {
    const config = {
      backend: 'hsm' as const,
      nodeId: 'test-node',
      hsm: {
        pkcs11LibraryPath: '/usr/lib/softhsm/libsofthsm2.so',
        slotId: -1,
        pin: '1234',
        evmKeyLabel: 'evm-key',
      },
    };

    expect(() => validateKeyManagerConfig(config)).toThrow(ConfigurationError);
    expect(() => validateKeyManagerConfig(config)).toThrow(/HSM_SLOT_ID must be non-negative/);
  });

  it('should throw error if HSM PIN is empty', () => {
    const config = {
      backend: 'hsm' as const,
      nodeId: 'test-node',
      hsm: {
        pkcs11LibraryPath: '/usr/lib/softhsm/libsofthsm2.so',
        slotId: 0,
        pin: '',
        evmKeyLabel: 'evm-key',
      },
    };

    expect(() => validateKeyManagerConfig(config)).toThrow(ConfigurationError);
    expect(() => validateKeyManagerConfig(config)).toThrow(/HSM_PIN cannot be empty/);
  });
});
