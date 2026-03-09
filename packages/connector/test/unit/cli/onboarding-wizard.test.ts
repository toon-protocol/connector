/**
 * OnboardingWizard Unit Tests
 *
 * Tests for the interactive onboarding wizard including:
 * - Address validation functions
 * - .env file generation
 * - File writing operations
 */

import {
  validateEthereumAddress,
  generateEnvFile,
  writeEnvFile,
} from '../../../src/cli/onboarding-wizard';
import type { OnboardingConfig } from '../../../src/cli/types';
import * as fs from 'fs/promises';
import * as path from 'path';
import * as os from 'os';

// Mock inquirer for wizard tests
jest.mock('inquirer', () => ({
  prompt: jest.fn(),
}));

describe('OnboardingWizard', () => {
  describe('validateEthereumAddress', () => {
    it('should accept valid 0x-prefixed 40-char hex address', () => {
      const validAddress = '0x742d35Cc6634C0532925a3b844Bc9e7595f12AB3';
      expect(validateEthereumAddress(validAddress)).toBe(true);
    });

    it('should accept lowercase ethereum address', () => {
      const validAddress = '0x742d35cc6634c0532925a3b844bc9e7595f12ab3';
      expect(validateEthereumAddress(validAddress)).toBe(true);
    });

    it('should accept uppercase ethereum address', () => {
      const validAddress = '0x742D35CC6634C0532925A3B844BC9E7595F12AB3';
      expect(validateEthereumAddress(validAddress)).toBe(true);
    });

    it('should reject address without 0x prefix', () => {
      const invalidAddress = '742d35Cc6634C0532925a3b844Bc9e7595f12AB3';
      expect(validateEthereumAddress(invalidAddress)).toBe(false);
    });

    it('should reject address with wrong length (too short)', () => {
      const invalidAddress = '0x742d35Cc6634C0532925a3b844Bc9e759';
      expect(validateEthereumAddress(invalidAddress)).toBe(false);
    });

    it('should reject address with wrong length (too long)', () => {
      const invalidAddress = '0x742d35Cc6634C0532925a3b844Bc9e7595f12AB3aa';
      expect(validateEthereumAddress(invalidAddress)).toBe(false);
    });

    it('should reject address with invalid characters', () => {
      const invalidAddress = '0x742d35Cc6634C0532925a3b844Bc9e7595f12ZZZ';
      expect(validateEthereumAddress(invalidAddress)).toBe(false);
    });

    it('should reject empty string', () => {
      expect(validateEthereumAddress('')).toBe(false);
    });

    it('should reject address with spaces', () => {
      const invalidAddress = '0x742d35Cc6634C0532925a3b844Bc 9e7595f12AB3';
      expect(validateEthereumAddress(invalidAddress)).toBe(false);
    });
  });

  describe('generateEnvFile', () => {
    it('should include all required variables for EVM-only config', () => {
      const config: OnboardingConfig = {
        nodeId: 'test-node',
        settlementPreference: 'evm',
        evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f12AB3',
        keyBackend: 'env',
        enableMonitoring: true,
        btpPort: 4000,
        healthCheckPort: 8080,
        logLevel: 'info',
      };

      const envContent = generateEnvFile(config);

      expect(envContent).toContain('NODE_ID=test-node');
      expect(envContent).toContain('SETTLEMENT_PREFERENCE=evm');
      expect(envContent).toContain('EVM_ADDRESS=0x742d35Cc6634C0532925a3b844Bc9e7595f12AB3');
      expect(envContent).toContain('BASE_RPC_URL=https://mainnet.base.org');
      expect(envContent).toContain('KEY_BACKEND=env');
      expect(envContent).toContain('BTP_PORT=4000');
      expect(envContent).toContain('HEALTH_CHECK_PORT=8080');
      expect(envContent).toContain('LOG_LEVEL=info');
      expect(envContent).toContain('PROMETHEUS_ENABLED=true');
    });

    it('should include GCP KMS config when gcp-kms backend selected', () => {
      const config: OnboardingConfig = {
        nodeId: 'gcp-node',
        settlementPreference: 'evm',
        evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f12AB3',
        keyBackend: 'gcp-kms',
        enableMonitoring: true,
        btpPort: 4000,
        healthCheckPort: 8080,
        logLevel: 'info',
      };

      const envContent = generateEnvFile(config);

      expect(envContent).toContain('KEY_BACKEND=gcp-kms');
      expect(envContent).toContain('GCP_LOCATION_ID=us-east1');
      expect(envContent).toContain('GCP_KEY_RING_ID=connector-keyring');
    });

    it('should include Azure Key Vault config when azure-kv backend selected', () => {
      const config: OnboardingConfig = {
        nodeId: 'azure-node',
        settlementPreference: 'evm',
        evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f12AB3',
        keyBackend: 'azure-kv',
        enableMonitoring: false,
        btpPort: 4000,
        healthCheckPort: 8080,
        logLevel: 'warn',
      };

      const envContent = generateEnvFile(config);

      expect(envContent).toContain('KEY_BACKEND=azure-kv');
      expect(envContent).toContain('AZURE_EVM_KEY_NAME=evm-signing-key');
    });

    it('should include TigerBeetle default configuration', () => {
      const config: OnboardingConfig = {
        nodeId: 'test-node',
        settlementPreference: 'evm',
        evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f12AB3',
        keyBackend: 'env',
        enableMonitoring: true,
        btpPort: 4000,
        healthCheckPort: 8080,
        logLevel: 'info',
      };

      const envContent = generateEnvFile(config);

      expect(envContent).toContain('TIGERBEETLE_CLUSTER_ID=0');
      expect(envContent).toContain('TIGERBEETLE_REPLICAS=tigerbeetle:3000');
    });

    it('should include peer discovery defaults', () => {
      const config: OnboardingConfig = {
        nodeId: 'test-node',
        settlementPreference: 'evm',
        evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f12AB3',
        keyBackend: 'env',
        enableMonitoring: true,
        btpPort: 4000,
        healthCheckPort: 8080,
        logLevel: 'info',
      };

      const envContent = generateEnvFile(config);

      expect(envContent).toContain('PEER_DISCOVERY_ENABLED=false');
    });

    it('should include warning for env key backend', () => {
      const config: OnboardingConfig = {
        nodeId: 'test-node',
        settlementPreference: 'evm',
        evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f12AB3',
        keyBackend: 'env',
        enableMonitoring: true,
        btpPort: 4000,
        healthCheckPort: 8080,
        logLevel: 'info',
      };

      const envContent = generateEnvFile(config);

      expect(envContent).toContain('WARNING');
      expect(envContent).toContain('development only');
    });
  });

  describe('writeEnvFile', () => {
    let tempDir: string;

    beforeEach(async () => {
      tempDir = await fs.mkdtemp(path.join(os.tmpdir(), 'onboarding-test-'));
    });

    afterEach(async () => {
      // Clean up temp directory
      try {
        await fs.rm(tempDir, { recursive: true });
      } catch {
        // Ignore cleanup errors
      }
    });

    it('should write file to specified path', async () => {
      const content = 'NODE_ID=test-node\nLOG_LEVEL=info';
      const filePath = path.join(tempDir, '.env');

      await writeEnvFile(content, filePath);

      const written = await fs.readFile(filePath, 'utf8');
      expect(written).toBe(content);
    });

    it('should create nested directories if needed', async () => {
      const content = 'NODE_ID=nested-node';
      const filePath = path.join(tempDir, 'nested', 'dir', '.env');

      await writeEnvFile(content, filePath);

      const written = await fs.readFile(filePath, 'utf8');
      expect(written).toBe(content);
    });

    it('should overwrite existing file', async () => {
      const filePath = path.join(tempDir, '.env');
      await fs.writeFile(filePath, 'OLD_CONTENT=true', 'utf8');

      const newContent = 'NEW_CONTENT=true';
      await writeEnvFile(newContent, filePath);

      const written = await fs.readFile(filePath, 'utf8');
      expect(written).toBe(newContent);
    });

    it('should throw error if path is not writable', async () => {
      // Try to write to a path that doesn't exist and can't be created
      const invalidPath = '/nonexistent/readonly/path/.env';

      await expect(writeEnvFile('content', invalidPath)).rejects.toThrow();
    });

    it('should handle relative paths', async () => {
      const content = 'NODE_ID=relative-test';
      const relativePath = path.relative(process.cwd(), path.join(tempDir, '.env'));

      await writeEnvFile(content, relativePath);

      const absolutePath = path.resolve(relativePath);
      const written = await fs.readFile(absolutePath, 'utf8');
      expect(written).toBe(content);
    });
  });
});
