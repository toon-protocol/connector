/* eslint-disable no-console */
/**
 * Onboarding Wizard
 *
 * Interactive CLI tool that guides new operators through connector configuration.
 * Generates a .env file with all required configuration.
 */

import inquirer from 'inquirer';
import * as fs from 'fs/promises';
import * as path from 'path';
import * as crypto from 'crypto';
import {
  OnboardingConfig,
  WizardAnswers,
  SettlementPreference,
  KeyBackend,
  LogLevel,
} from './types';

// Address validation patterns
// Ethereum address: 0x-prefixed, 40 hex characters (case-insensitive)
const ETH_ADDRESS_REGEX = /^0x[a-fA-F0-9]{40}$/;

/**
 * Validate Ethereum address format
 * @param address - The address to validate
 * @returns true if valid, false otherwise
 */
export function validateEthereumAddress(address: string): boolean {
  return ETH_ADDRESS_REGEX.test(address);
}

/**
 * Generate a unique node ID
 * @returns A unique node identifier
 */
function generateNodeId(): string {
  const randomSuffix = crypto.randomBytes(4).toString('hex');
  return `connector-${randomSuffix}`;
}

/**
 * Run the interactive onboarding wizard
 * @returns The completed onboarding configuration
 */
export async function runOnboardingWizard(): Promise<OnboardingConfig> {
  console.log('\n========================================');
  console.log('  M2M Connector Onboarding Wizard');
  console.log('========================================\n');
  console.log('This wizard will guide you through configuring your connector.');
  console.log('Press Ctrl+C at any time to cancel.\n');

  const defaultNodeId = generateNodeId();

  // Step 1: Basic configuration
  const basicAnswers = await inquirer.prompt<
    Pick<WizardAnswers, 'nodeId' | 'settlementPreference'>
  >([
    {
      type: 'input',
      name: 'nodeId',
      message: 'Enter a unique node ID for this connector:',
      default: defaultNodeId,
      validate: (input: string) => {
        if (!input || input.trim().length === 0) {
          return 'Node ID cannot be empty';
        }
        if (!/^[a-zA-Z0-9-_]+$/.test(input)) {
          return 'Node ID can only contain letters, numbers, hyphens, and underscores';
        }
        return true;
      },
    },
    {
      type: 'list',
      name: 'settlementPreference',
      message: 'Select your settlement preference:',
      choices: [{ name: 'EVM only (Base L2)', value: 'evm' }],
      default: 'evm',
    },
  ]);

  // Step 2: Blockchain addresses
  const evmAnswer = await inquirer.prompt<{ evmAddress: string }>([
    {
      type: 'input',
      name: 'evmAddress',
      message: 'Enter your Ethereum address (0x...):',
      validate: (input: string) => {
        if (!input || input.trim().length === 0) {
          return 'Ethereum address is required for EVM settlement';
        }
        if (!validateEthereumAddress(input)) {
          return 'Invalid Ethereum address format. Must be 0x followed by 40 hex characters.';
        }
        return true;
      },
    },
  ]);
  const addressAnswers = { evmAddress: evmAnswer.evmAddress };

  // Step 3: Key management configuration
  const keyAnswers = await inquirer.prompt<Pick<WizardAnswers, 'keyBackend'>>([
    {
      type: 'list',
      name: 'keyBackend',
      message: 'Select your key management backend:',
      choices: [
        { name: 'Environment variables (development only)', value: 'env' },
        { name: 'AWS KMS (production)', value: 'aws-kms' },
        { name: 'GCP KMS (production)', value: 'gcp-kms' },
        { name: 'Azure Key Vault (production)', value: 'azure-kv' },
      ],
      default: 'env',
    },
  ]);

  // Step 4: Monitoring and network configuration
  const advancedAnswers = await inquirer.prompt<
    Pick<WizardAnswers, 'enableMonitoring' | 'btpPort' | 'healthCheckPort' | 'logLevel'>
  >([
    {
      type: 'confirm',
      name: 'enableMonitoring',
      message: 'Enable Prometheus/Grafana monitoring?',
      default: true,
    },
    {
      type: 'number',
      name: 'btpPort',
      message: 'BTP server port:',
      default: 4000,
      validate: (input: number) => {
        if (!Number.isInteger(input) || input < 1 || input > 65535) {
          return 'Port must be a valid number between 1 and 65535';
        }
        return true;
      },
    },
    {
      type: 'number',
      name: 'healthCheckPort',
      message: 'Health check and metrics HTTP port:',
      default: 8080,
      validate: (input: number) => {
        if (!Number.isInteger(input) || input < 1 || input > 65535) {
          return 'Port must be a valid number between 1 and 65535';
        }
        return true;
      },
    },
    {
      type: 'list',
      name: 'logLevel',
      message: 'Select log level:',
      choices: [
        { name: 'debug - Verbose debugging information', value: 'debug' },
        { name: 'info - General operational information (recommended)', value: 'info' },
        { name: 'warn - Warning messages only', value: 'warn' },
        { name: 'error - Error messages only', value: 'error' },
      ],
      default: 'info',
    },
  ]);

  // Combine all answers into config
  const config: OnboardingConfig = {
    nodeId: basicAnswers.nodeId,
    settlementPreference: basicAnswers.settlementPreference as SettlementPreference,
    evmAddress: addressAnswers.evmAddress,
    keyBackend: keyAnswers.keyBackend as KeyBackend,
    enableMonitoring: advancedAnswers.enableMonitoring,
    btpPort: advancedAnswers.btpPort,
    healthCheckPort: advancedAnswers.healthCheckPort,
    logLevel: advancedAnswers.logLevel as LogLevel,
  };

  return config;
}

/**
 * Generate .env file content from configuration
 * @param config - The onboarding configuration
 * @returns The .env file content as a string
 */
export function generateEnvFile(config: OnboardingConfig): string {
  const lines: string[] = [
    '# =============================================================================',
    '# M2M Connector Configuration',
    '# Generated by onboarding wizard',
    '# =============================================================================',
    '',
    '# Core Configuration',
    `NODE_ID=${config.nodeId}`,
    `SETTLEMENT_PREFERENCE=${config.settlementPreference}`,
    '',
  ];

  // Blockchain configuration
  lines.push('# Blockchain Configuration');
  lines.push('BASE_RPC_URL=https://mainnet.base.org');
  lines.push(`EVM_ADDRESS=${config.evmAddress || ''}`);
  lines.push('');

  // Key management
  lines.push('# Key Management');
  lines.push(`KEY_BACKEND=${config.keyBackend}`);

  if (config.keyBackend === 'env') {
    lines.push('# WARNING: env backend is for development only!');
    lines.push('# Uncomment and set your private key:');
    lines.push('# EVM_PRIVATE_KEY=0x...');
  } else if (config.keyBackend === 'aws-kms') {
    lines.push('AWS_REGION=us-east-1');
    lines.push('# AWS_KMS_EVM_KEY_ID=arn:aws:kms:...');
  } else if (config.keyBackend === 'gcp-kms') {
    lines.push('# GCP_PROJECT_ID=my-project');
    lines.push('GCP_LOCATION_ID=us-east1');
    lines.push('GCP_KEY_RING_ID=connector-keyring');
    lines.push('# GCP_KMS_EVM_KEY_ID=evm-signing-key');
  } else if (config.keyBackend === 'azure-kv') {
    lines.push('# AZURE_VAULT_URL=https://my-vault.vault.azure.net');
    lines.push('AZURE_EVM_KEY_NAME=evm-signing-key');
  }

  lines.push('');

  // Network configuration
  lines.push('# Network Configuration');
  lines.push(`BTP_PORT=${config.btpPort}`);
  lines.push(`HEALTH_CHECK_PORT=${config.healthCheckPort}`);
  lines.push('');

  // Monitoring
  lines.push('# Monitoring');
  lines.push(`PROMETHEUS_ENABLED=${config.enableMonitoring}`);
  if (config.enableMonitoring) {
    lines.push('GRAFANA_PASSWORD=admin  # Change this in production!');
  }
  lines.push('');

  // Logging
  lines.push('# Logging');
  lines.push(`LOG_LEVEL=${config.logLevel}`);
  lines.push('');

  // TigerBeetle (defaults)
  lines.push('# TigerBeetle');
  lines.push('TIGERBEETLE_CLUSTER_ID=0');
  lines.push('TIGERBEETLE_REPLICAS=tigerbeetle:3000');
  lines.push('');

  // Peer discovery (disabled by default)
  lines.push('# Peer Discovery (optional)');
  lines.push('PEER_DISCOVERY_ENABLED=false');
  lines.push('# PEER_DISCOVERY_ENDPOINTS=http://discovery.example.com:9999');
  lines.push('');

  return lines.join('\n');
}

/**
 * Write .env file to disk
 * @param content - The .env file content
 * @param filePath - The path to write the file to
 * @throws Error if the file cannot be written
 */
export async function writeEnvFile(content: string, filePath: string): Promise<void> {
  const absolutePath = path.resolve(filePath);
  const directory = path.dirname(absolutePath);

  // Ensure directory exists
  await fs.mkdir(directory, { recursive: true });

  // Write file
  await fs.writeFile(absolutePath, content, 'utf8');
}

/**
 * Run the complete onboarding process
 * @param outputPath - Optional path for the .env file (defaults to .env in cwd)
 */
export async function runOnboarding(outputPath?: string): Promise<void> {
  try {
    const config = await runOnboardingWizard();
    const envContent = generateEnvFile(config);
    const targetPath = outputPath || path.join(process.cwd(), '.env');

    // Check if file exists
    try {
      await fs.access(targetPath);
      const { overwrite } = await inquirer.prompt<{ overwrite: boolean }>([
        {
          type: 'confirm',
          name: 'overwrite',
          message: `.env file already exists at ${targetPath}. Overwrite?`,
          default: false,
        },
      ]);
      if (!overwrite) {
        console.log('\nOnboarding cancelled. Existing .env file preserved.');
        return;
      }
    } catch {
      // File doesn't exist, continue
    }

    await writeEnvFile(envContent, targetPath);

    console.log('\n========================================');
    console.log('  Configuration Complete!');
    console.log('========================================\n');
    console.log(`Configuration saved to: ${targetPath}\n`);
    console.log('Next steps:');
    console.log('1. Review and edit the .env file as needed');
    if (config.keyBackend === 'env') {
      console.log('2. Add your private keys to the .env file');
      console.log('   WARNING: Use KMS in production!');
    } else {
      console.log(`2. Configure your ${config.keyBackend} credentials`);
    }
    console.log('3. Initialize TigerBeetle (one-time):');
    console.log('   docker run --rm -v tigerbeetle-data:/data tigerbeetle/tigerbeetle \\');
    console.log('     format --cluster=0 --replica=0 --replica-count=1 /data/0_0.tigerbeetle');
    console.log('4. Start the production stack:');
    console.log('   docker-compose -f docker-compose-production.yml up -d');
    console.log('');
  } catch (error) {
    if ((error as Error).message?.includes('User force closed')) {
      console.log('\n\nOnboarding cancelled by user.');
      return;
    }
    throw error;
  }
}
