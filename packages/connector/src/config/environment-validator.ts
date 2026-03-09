/**
 * Environment Validation Module
 *
 * Provides functionality to validate connector configuration based on
 * deployment environment (development/staging/production). Enforces
 * production safety rules to prevent accidental mainnet deployments
 * with development credentials or localhost RPC endpoints.
 *
 * @packageDocumentation
 */

import { ConnectorConfig, EVMChainConfig } from './types';
import { ConfigurationError } from './config-loader';
import { createLogger } from '../utils/logger';

// Create logger for validation warnings
const logger = createLogger('environment-validator');

/**
 * Known Development Private Keys
 *
 * List of private keys that are publicly known from development tools.
 * These keys MUST NOT be used in production environments as they are
 * included in documentation and GitHub repositories.
 *
 * Sources:
 * - Anvil (Foundry): Default pre-funded accounts with deterministic private keys
 */
const KNOWN_DEV_PRIVATE_KEYS = [
  // Anvil Account #0 (0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266)
  '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80',
  // Anvil Account #1 (0x70997970C51812dc3A010C7d01b50e0d17dc79C8)
  '0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d',
  // Anvil Account #2 (0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC)
  '0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a',
];

/**
 * Validate Environment Configuration
 *
 * Main validation function that enforces environment-specific rules.
 * - Production: Strict validation (errors on misconfiguration)
 * - Development: Warning logs only (allows flexible configuration)
 * - Staging: Moderate validation (warnings for common issues)
 *
 * Production validation rules:
 * - Rejects known development private keys
 * - Rejects localhost RPC URLs
 * - Requires mainnet chain IDs and networks
 * - Requires HTTPS for RPC endpoints
 *
 * Development warnings:
 * - Logs development mode banner
 * - Logs active blockchain endpoints for visibility
 *
 * @param config - Connector configuration to validate
 * @throws ConfigurationError if production validation fails
 *
 * @example
 * ```typescript
 * const config = ConfigLoader.loadConfig('./config.yaml');
 * validateEnvironment(config);  // Throws if production misconfigured
 * // Safe to proceed with validated config
 * ```
 */
export function validateEnvironment(config: ConnectorConfig): void {
  if (config.environment === 'production') {
    validateProductionEnvironment(config);
  } else if (config.environment === 'development') {
    logDevelopmentWarnings(config);
  }
  // Staging uses development-like warnings (no strict validation)
  else if (config.environment === 'staging') {
    logStagingWarnings(config);
  }

  // Validate deployment mode configuration (applies to all environments)
  validateDeploymentMode(config);

  // Validate IP allowlist configuration (applies to all environments)
  validateIPAllowlist(config);
}

/**
 * Validate an EVM chain configuration for production environment.
 *
 * Enforces strict validation rules:
 * - Chain ID must match expected mainnet chain ID
 * - RPC URL must not contain localhost or 127.0.0.1
 * - RPC URL must use HTTPS (not HTTP)
 * - Private key must not be a known development key
 *
 * @param chain - EVM chain configuration
 * @param chainName - Human-readable chain name for error messages (e.g., 'Base', 'Arbitrum')
 * @param expectedChainId - Expected mainnet chain ID
 * @throws ConfigurationError if any production validation rule fails
 * @private
 */
function validateEVMChainProduction(
  chain: EVMChainConfig,
  chainName: string,
  expectedChainId: number
): void {
  // Chain ID must match expected mainnet
  if (chain.chainId !== expectedChainId) {
    throw new ConfigurationError(
      `Production must use ${chainName} mainnet (chainId ${expectedChainId}), got chainId ${chain.chainId}`
    );
  }

  // RPC URL must not contain localhost
  if (chain.rpcUrl.includes('localhost') || chain.rpcUrl.includes('127.0.0.1')) {
    throw new ConfigurationError(
      `Cannot use localhost RPC for ${chainName} in production. Use public mainnet endpoint.`
    );
  }

  // RPC URL must use HTTPS
  if (!chain.rpcUrl.startsWith('https://') && !chain.rpcUrl.startsWith('wss://')) {
    throw new ConfigurationError(`Production ${chainName} RPC URL must use HTTPS for security`);
  }

  // Private key must not be a known development key
  if (chain.privateKey && KNOWN_DEV_PRIVATE_KEYS.includes(chain.privateKey)) {
    throw new ConfigurationError(
      `Cannot use development private key for ${chainName} in production. Use secure key from KMS/HSM.`
    );
  }
}

/**
 * Validate Production Environment Configuration
 *
 * Enforces strict validation rules for production deployments.
 * All validations are HARD ERRORS (throw ConfigurationError).
 *
 * EVM chain validations (Base, Arbitrum):
 * - Chain ID must match expected mainnet chain ID
 * - RPC URL must not contain localhost or 127.0.0.1
 * - RPC URL must use HTTPS (not HTTP)
 * - Private key must not be a known development key
 *
 * @param config - Connector configuration
 * @throws ConfigurationError if any production validation rule fails
 * @private
 */
function validateProductionEnvironment(config: ConnectorConfig): void {
  // Validate admin API security: require API key OR IP allowlist in production
  if (config.adminApi?.enabled) {
    const hasApiKey = !!config.adminApi.apiKey;
    const hasIPAllowlist = !!config.adminApi.allowedIPs && config.adminApi.allowedIPs.length > 0;

    if (!hasApiKey && !hasIPAllowlist) {
      throw new ConfigurationError(
        'Admin API is enabled in production without authentication. ' +
          'Set ADMIN_API_KEY or ADMIN_API_ALLOWED_IPS to secure the admin API.'
      );
    }
  }

  // Validate Base blockchain if enabled
  if (config.blockchain?.base?.enabled) {
    validateEVMChainProduction(config.blockchain.base, 'Base', 8453);
  }

  // Validate Arbitrum blockchain if enabled
  if (config.blockchain?.arbitrum?.enabled) {
    validateEVMChainProduction(config.blockchain.arbitrum, 'Arbitrum', 42161);
  }
}

/**
 * Log Development Environment Warnings
 *
 * Emits warning logs to clearly indicate development mode.
 * Logs active blockchain endpoints for visibility during local development.
 *
 * All logs are WARNINGS (not errors), allowing startup to proceed.
 *
 * @param config - Connector configuration
 * @private
 */
function logDevelopmentWarnings(config: ConnectorConfig): void {
  logger.warn('⚠️  DEVELOPMENT MODE - Using local blockchain nodes');
  logger.warn('⚠️  This is NOT production configuration');

  // Log Base blockchain config if enabled
  if (config.blockchain?.base?.enabled) {
    logger.warn(`⚠️  Base RPC: ${config.blockchain.base.rpcUrl}`);
    logger.warn(`⚠️  Base Chain ID: ${config.blockchain.base.chainId}`);
  }

  // Log Arbitrum blockchain config if enabled
  if (config.blockchain?.arbitrum?.enabled) {
    logger.warn(`⚠️  Arbitrum RPC: ${config.blockchain.arbitrum.rpcUrl}`);
    logger.warn(`⚠️  Arbitrum Chain ID: ${config.blockchain.arbitrum.chainId}`);
  }
}

/**
 * Validate an EVM chain configuration for staging environment.
 *
 * Enforces moderate validation rules for public testnet deployments:
 * - Chain ID must match expected testnet chain ID
 * - Warns on localhost RPC URLs
 * - Warns on HTTP for public endpoints
 * - Rejects known Anvil development keys
 *
 * @param chain - EVM chain configuration
 * @param chainName - Human-readable chain name for log/error messages
 * @param expectedChainId - Expected testnet chain ID
 * @throws ConfigurationError if staging validation fails
 * @private
 */
function validateEVMChainStaging(
  chain: EVMChainConfig,
  chainName: string,
  expectedChainId: number
): void {
  logger.warn(`⚠️  ${chainName} RPC: ${chain.rpcUrl}`);
  logger.warn(`⚠️  ${chainName} Chain ID: ${chain.chainId}`);

  // Chain ID must match expected testnet
  if (chain.chainId !== expectedChainId) {
    throw new ConfigurationError(
      `Staging must use ${chainName} testnet (chainId ${expectedChainId}), got chainId ${chain.chainId}. ` +
        `Use ENVIRONMENT=production for ${chainName} mainnet.`
    );
  }

  // RPC URL must not point to localhost (staging uses public testnets)
  if (chain.rpcUrl.includes('localhost') || chain.rpcUrl.includes('127.0.0.1')) {
    logger.warn(
      `⚠️  Staging ${chainName} RPC points to localhost. ` +
        `Use a public testnet endpoint, or ENVIRONMENT=development for local Anvil.`
    );
  }

  // Warn if using HTTP instead of HTTPS for public endpoints
  if (
    chain.rpcUrl.startsWith('http://') &&
    !chain.rpcUrl.includes('localhost') &&
    !chain.rpcUrl.includes('127.0.0.1') &&
    !chain.rpcUrl.includes('anvil')
  ) {
    logger.warn(
      `⚠️  Staging ${chainName} RPC uses HTTP - consider HTTPS for public testnet endpoints`
    );
  }

  // Reject known development keys in staging
  if (chain.privateKey && KNOWN_DEV_PRIVATE_KEYS.includes(chain.privateKey)) {
    throw new ConfigurationError(
      `Cannot use Anvil development private key for ${chainName} in staging. ` +
        'Generate a dedicated testnet wallet for staging deployment.'
    );
  }
}

/**
 * Log Staging Environment Warnings and Validate Testnet Configuration
 *
 * Emits warning logs to indicate staging/testnet mode.
 * Validates all enabled EVM chains for testnet correctness.
 *
 * @param config - Connector configuration
 * @throws ConfigurationError if staging validation fails
 * @private
 */
function logStagingWarnings(config: ConnectorConfig): void {
  logger.warn('⚠️  STAGING MODE - Using public testnets');
  logger.warn('⚠️  This is NOT production configuration');

  // Validate Base blockchain if enabled
  if (config.blockchain?.base?.enabled) {
    validateEVMChainStaging(config.blockchain.base, 'Base', 84532);
  }

  // Validate Arbitrum blockchain if enabled
  if (config.blockchain?.arbitrum?.enabled) {
    validateEVMChainStaging(config.blockchain.arbitrum, 'Arbitrum', 421614);
  }
}

/**
 * Validate Deployment Mode Configuration
 *
 * Validates deployment mode configuration for consistency and best practices.
 * Runs for all environments (development, staging, production).
 *
 * **Validation Rules**:
 *
 * When `deploymentMode` is explicitly set to `'embedded'`:
 * - **ERROR** if `localDelivery.enabled` is true
 *   → Embedded mode uses function handlers, not HTTP delivery
 * - **WARNING** if `adminApi.enabled` is true
 *   → Admin API is typically unnecessary for in-process integration
 *
 * When `deploymentMode` is explicitly set to `'standalone'`:
 * - **ERROR** if `localDelivery.enabled` is true but `handlerUrl` is missing
 *   → Standalone mode requires BLS endpoint for HTTP forwarding
 * - **WARNING** if `adminApi.enabled` is false
 *   → External BLS typically needs admin API to send packets
 * - **WARNING** if `localDelivery.enabled` is false
 *   → Standalone deployments typically use HTTP for incoming packets
 *
 * When `deploymentMode` is omitted:
 * - No validation (mode is inferred from flags, backward compatible)
 *
 * @param config - Connector configuration
 * @throws ConfigurationError if deployment mode validation fails
 * @private
 */
function validateDeploymentMode(config: ConnectorConfig): void {
  const mode = config.deploymentMode;

  // No validation if deploymentMode is not explicitly set (backward compatible)
  if (!mode) {
    return;
  }

  // Validate embedded mode configuration
  if (mode === 'embedded') {
    // ERROR: Embedded mode should not use HTTP local delivery
    if (config.localDelivery?.enabled) {
      throw new ConfigurationError(
        'deploymentMode is set to "embedded" but localDelivery.enabled is true. ' +
          'Embedded mode uses function handlers (setPacketHandler or setLocalDeliveryHandler) ' +
          'for in-process packet delivery, not HTTP forwarding. ' +
          'Either set deploymentMode to "standalone" or disable localDelivery.'
      );
    }

    // WARNING: Embedded mode typically doesn't need admin API
    if (config.adminApi?.enabled) {
      logger.warn(
        '⚠️  deploymentMode is "embedded" but adminApi.enabled is true. ' +
          'Embedded mode typically uses node.sendPacket() for outgoing packets, ' +
          'not the admin API HTTP endpoint. Admin API is primarily for standalone ' +
          'deployments where external processes need to send packets via HTTP.'
      );
    }
  }

  // Validate standalone mode configuration
  if (mode === 'standalone') {
    // ERROR: Standalone mode with localDelivery requires handlerUrl
    if (config.localDelivery?.enabled && !config.localDelivery.handlerUrl) {
      throw new ConfigurationError(
        'deploymentMode is set to "standalone" with localDelivery.enabled=true ' +
          'but localDelivery.handlerUrl is missing. Standalone mode requires a ' +
          'business logic server endpoint for HTTP packet forwarding. ' +
          'Set localDelivery.handlerUrl to the BLS /handle-packet endpoint ' +
          '(e.g., "http://business-logic:8080").'
      );
    }

    // WARNING: Standalone mode typically needs admin API
    if (!config.adminApi?.enabled) {
      logger.warn(
        '⚠️  deploymentMode is "standalone" but adminApi.enabled is false. ' +
          'Standalone deployments typically enable the admin API so the external ' +
          'business logic server can send packets via POST /admin/ilp/send. ' +
          'Without the admin API, the external BLS cannot initiate outgoing payments.'
      );
    }

    // WARNING: Standalone mode typically uses HTTP local delivery
    if (!config.localDelivery?.enabled) {
      logger.warn(
        '⚠️  deploymentMode is "standalone" but localDelivery.enabled is false. ' +
          'Standalone deployments typically forward incoming packets to an external ' +
          'business logic server via HTTP POST to /handle-packet. ' +
          'If you intend to use function handlers (setPacketHandler), consider ' +
          'setting deploymentMode to "embedded" instead.'
      );
    }
  }
}

/**
 * Validate IP Allowlist Configuration
 *
 * Validates admin API IP allowlist settings:
 * - IP addresses must be valid IPv4 or IPv6
 * - CIDR notation must be valid
 * - trustProxy should only be used behind a reverse proxy
 *
 * @param config - Connector configuration
 * @throws ConfigurationError if IP allowlist validation fails
 * @private
 */
function validateIPAllowlist(config: ConnectorConfig): void {
  const allowedIPs = config.adminApi?.allowedIPs;
  const trustProxy = config.adminApi?.trustProxy;

  // No validation if IP allowlist not configured
  if (!allowedIPs || allowedIPs.length === 0) {
    // Warn if trustProxy is enabled without IP allowlist (has no effect)
    if (trustProxy && config.adminApi?.enabled) {
      logger.warn(
        '⚠️  adminApi.trustProxy is enabled but allowedIPs is not set. ' +
          'trustProxy only affects IP allowlist middleware. It has no effect without allowedIPs.'
      );
    }
    return;
  }

  // Validate each IP/CIDR in the allowlist
  for (const entry of allowedIPs) {
    if (!entry || typeof entry !== 'string' || entry.trim() === '') {
      throw new ConfigurationError(
        `Invalid IP allowlist entry: "${entry}". Each entry must be a non-empty string.`
      );
    }

    const ip = entry.trim();

    // Check if it's CIDR notation
    if (ip.includes('/')) {
      try {
        // Use a simple dynamic import or require to validate CIDR
        // The Netmask library will throw if CIDR is invalid
        // We can't import it here since this is a validator, but we'll trust the middleware
        // to catch invalid CIDRs at runtime. Just do basic format check.
        const parts = ip.split('/');
        if (parts.length !== 2 || !parts[1]) {
          throw new Error('CIDR must have exactly one slash');
        }
        const prefix = parseInt(parts[1], 10);
        if (isNaN(prefix) || prefix < 0 || prefix > 128) {
          throw new Error('CIDR prefix must be 0-128');
        }
      } catch (err) {
        throw new ConfigurationError(
          `Invalid CIDR notation in allowedIPs: "${ip}". ` +
            `Error: ${err instanceof Error ? err.message : 'Unknown error'}`
        );
      }
    } else {
      // Individual IP - validate IPv4 or IPv6 format
      const ipv4Regex = /^(\d{1,3}\.){3}\d{1,3}$/;
      const ipv6Regex = /^([0-9a-fA-F]{0,4}:){2,7}[0-9a-fA-F]{0,4}$/;

      if (!ipv4Regex.test(ip) && !ipv6Regex.test(ip)) {
        throw new ConfigurationError(
          `Invalid IP address in allowedIPs: "${ip}". Must be valid IPv4 or IPv6 address.`
        );
      }

      // Additional IPv4 validation: each octet must be 0-255
      if (ipv4Regex.test(ip)) {
        const octets = ip.split('.').map(Number);
        if (octets.some((octet) => octet > 255)) {
          throw new ConfigurationError(
            `Invalid IPv4 address in allowedIPs: "${ip}". Each octet must be 0-255.`
          );
        }
      }
    }
  }

  // Warn if trustProxy is not set but allowedIPs contains common proxy headers
  if (!trustProxy && config.adminApi?.enabled) {
    logger.info(
      'ℹ️  adminApi.allowedIPs is configured with trustProxy=false. ' +
        'Client IPs will be taken from direct socket connections. ' +
        'If behind a reverse proxy (nginx, traefik, ALB), set trustProxy=true ' +
        'to use X-Forwarded-For header for accurate IP detection.'
    );
  }
}

/**
 * Validate Chain ID Against RPC Endpoint (Runtime Validation)
 *
 * Queries the RPC endpoint to get actual chain ID and compares it
 * with the configured chain ID. Logs a warning if mismatch detected.
 *
 * This validation is performed asynchronously and does NOT block
 * connector startup (logs warning only, no error thrown).
 *
 * @param config - Connector configuration
 * @returns Promise that resolves when validation complete
 *
 * @example
 * ```typescript
 * // Start validation asynchronously (don't await)
 * validateChainId(config).catch(err => {
 *   logger.warn(`Chain ID validation failed: ${err.message}`);
 * });
 * ```
 */
export async function validateChainId(config: ConnectorConfig): Promise<void> {
  const validations: Promise<void>[] = [];

  // Validate Base chain ID if enabled
  if (config.blockchain?.base?.enabled) {
    validations.push(validateEVMChainId(config.blockchain.base, 'Base', 'BASE_RPC_URL'));
  }

  // Validate Arbitrum chain ID if enabled
  if (config.blockchain?.arbitrum?.enabled) {
    validations.push(
      validateEVMChainId(config.blockchain.arbitrum, 'Arbitrum', 'ARBITRUM_RPC_URL')
    );
  }

  await Promise.all(validations);
}

/**
 * Validate a single EVM chain's configured chain ID against its RPC endpoint.
 *
 * @param chain - EVM chain configuration
 * @param chainName - Human-readable chain name for log messages
 * @param envVarHint - Environment variable name to mention in mismatch warnings
 * @private
 */
async function validateEVMChainId(
  chain: EVMChainConfig,
  chainName: string,
  envVarHint: string
): Promise<void> {
  try {
    const response = await fetch(chain.rpcUrl, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        jsonrpc: '2.0',
        method: 'eth_chainId',
        params: [],
        id: 1,
      }),
    });

    if (!response.ok) {
      logger.warn(
        `${chainName} chain ID validation failed: RPC endpoint returned HTTP ${response.status}`
      );
      return;
    }

    const data = (await response.json()) as { result?: string; error?: { message: string } };

    if (data.error) {
      logger.warn(`${chainName} chain ID validation failed: ${data.error.message}`);
      return;
    }

    if (!data.result) {
      logger.warn(`${chainName} chain ID validation failed: No result from RPC endpoint`);
      return;
    }

    const actualChainId = parseInt(data.result, 16);

    if (actualChainId !== chain.chainId) {
      logger.warn(
        `⚠️  ${chainName} chain ID mismatch: config expects ${chain.chainId}, RPC returned ${actualChainId}`
      );
      logger.warn(`⚠️  Verify ${envVarHint} points to correct network`);
    }
  } catch (error) {
    logger.warn(`${chainName} chain ID validation failed: ${(error as Error).message}`);
  }
}
