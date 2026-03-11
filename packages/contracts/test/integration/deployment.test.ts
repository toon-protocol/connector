/**
 * Integration tests for Foundry deployment setup
 *
 * These tests verify that the Foundry development environment is correctly
 * configured and can deploy contracts to local Anvil.
 *
 * Prerequisites:
 * - Anvil must be running at http://localhost:8545 (via docker-compose-dev.yml)
 * - Environment variables must be loaded from .env
 */

import { execSync } from 'child_process';
import { ethers } from 'ethers';

describe('Foundry Deployment Integration Tests', () => {
  const ANVIL_RPC_URL = process.env.BASE_RPC_URL || 'http://localhost:8545';
  const PRIVATE_KEY =
    process.env.PRIVATE_KEY || '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80';

  let provider: ethers.JsonRpcProvider;
  let deployedAddress: string;

  beforeAll(() => {
    // Initialize ethers provider
    provider = new ethers.JsonRpcProvider(ANVIL_RPC_URL);
  });

  test('should verify Anvil is running and accessible', async () => {
    // Test RPC connectivity
    const network = await provider.getNetwork();
    expect(network).toBeDefined();

    // Verify block number is available
    const blockNumber = await provider.getBlockNumber();
    expect(blockNumber).toBeGreaterThanOrEqual(0);
  });

  test('should verify Anvil Account #0 has pre-funded balance', async () => {
    // Anvil Account #0 should have 10000 ETH pre-funded
    const wallet = new ethers.Wallet(PRIVATE_KEY, provider);
    const balance = await provider.getBalance(wallet.address);

    // Balance should be > 0 (Anvil pre-funds with 10000 ETH)
    expect(balance).toBeGreaterThan(0);

    // Verify it's the expected Anvil Account #0 address
    expect(wallet.address).toBe('0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266');
  });

  test('should deploy contract to local Anvil', async () => {
    // Deploy contract using Foundry script
    const contractsDir = process.cwd().endsWith('packages/contracts')
      ? process.cwd()
      : `${process.cwd()}/packages/contracts`;

    const deployOutput = execSync(
      'forge script script/Deploy.s.sol --rpc-url $BASE_RPC_URL --broadcast',
      {
        cwd: contractsDir,
        encoding: 'utf-8',
      }
    );

    // Parse deployment output to extract contract address
    const addressMatch = deployOutput.match(
      /TokenNetworkRegistry deployed to: (0x[a-fA-F0-9]{40})/
    );
    expect(addressMatch).not.toBeNull();

    if (addressMatch) {
      deployedAddress = addressMatch[1];

      // Verify address is valid Ethereum address
      expect(ethers.isAddress(deployedAddress)).toBe(true);
    }
  }, 30000); // 30 second timeout for deployment

  test('should verify contract has bytecode on Anvil', async () => {
    // Skip if deployment failed
    if (!deployedAddress) {
      console.warn('Skipping test: No deployed address available');
      return;
    }

    // Get contract bytecode
    const code = await provider.getCode(deployedAddress);

    // Bytecode should be non-empty (> "0x")
    expect(code).not.toBe('0x');
    expect(code.length).toBeGreaterThan(2);
  });
});
