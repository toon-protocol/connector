/**
 * EVM Payment Channel Integration Tests (Consolidated)
 *
 * This file consolidates three previously separate test files:
 * - base-payment-channel.test.ts (SDK-level operations)
 * - base-payment-channel-bls.test.ts (BLS integration with programmatic connectors)
 * - base-payment-channel-e2e.test.ts (Full E2E with Docker Compose)
 *
 * Tests are organized into three describe blocks by integration level:
 * 1. EVM Payment Channel - SDK Level
 * 2. EVM Payment Channel - BLS Integration
 * 3. EVM Payment Channel - E2E with Docker Compose
 *
 * Prerequisites:
 * - Docker installed and daemon running
 * - Docker Compose 2.x installed
 * - E2E_TESTS=true environment variable for tests to run
 *
 * Setup:
 *   docker compose -f docker-compose-evm-test.yml up -d
 *
 * Teardown:
 *   docker compose -f docker-compose-evm-test.yml down -v
 */

/* eslint-disable no-console */

import { execSync } from 'child_process';
import path from 'path';
import pino from 'pino';
import { ethers } from 'ethers';
import express, { Express } from 'express';
import { PaymentChannelSDK } from '../../src/settlement/payment-channel-sdk';
import { KeyManager } from '../../src/security/key-manager';
import { createAdminRouter, AdminAPIConfig } from '../../src/http/admin-api';
import { ConnectorNode } from '../../src/core/connector-node';
import type { ConnectorConfig } from '../../src/config/types';
import type { BalanceProof } from '@crosstown/shared';
import { waitFor } from '../helpers/wait-for';

// ============================================================================
// Shared Configuration
// ============================================================================

const ANVIL_RPC_URL = 'http://localhost:8545';
const FAUCET_URL = 'http://localhost:3500';

// Deployed contracts (deterministic addresses from DeployLocal.s.sol)
const TOKEN_ADDRESS = '0x5FbDB2315678afecb367f032d93F642f64180aa3';
const REGISTRY_ADDRESS = '0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512';
const TOKEN_NETWORK_ADDRESS = '0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0';

// Test accounts (Anvil well-known Foundry accounts)
// Suite 1 (SDK Level) uses accounts 0 and 1
const ACCOUNT_0_PRIVATE_KEY = '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80';
const ACCOUNT_1_PRIVATE_KEY = '0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d';
const ACCOUNT_0_ADDRESS = '0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266';
const ACCOUNT_1_ADDRESS = '0x70997970C51812dc3A010C7d01b50e0d17dc79C8';
// Suite 2 (BLS Integration) uses accounts 2 and 3 to avoid faucet rate limits
const ACCOUNT_2_PRIVATE_KEY = '0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a';
const ACCOUNT_3_PRIVATE_KEY = '0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6';
const ACCOUNT_2_ADDRESS = '0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC';
const ACCOUNT_3_ADDRESS = '0x90F79bf6EB2c4f870365E785982E1f101E93b906';

// Test timeouts
jest.setTimeout(600000); // 10 minutes for full test suite

// ============================================================================
// Docker Helper Functions
// ============================================================================

function isDockerAvailable(): boolean {
  try {
    execSync('docker info', { stdio: 'ignore' });
    return true;
  } catch {
    return false;
  }
}

function isDockerComposeAvailable(): boolean {
  try {
    execSync('docker-compose --version', { stdio: 'ignore' });
    return true;
  } catch {
    return false;
  }
}

function getRepoRoot(): string {
  const cwd = process.cwd();
  if (cwd.endsWith('/packages/connector')) {
    return path.join(cwd, '../..');
  }
  return cwd;
}

function executeCommand(
  cmd: string,
  options: { cwd?: string; ignoreError?: boolean } = {}
): string {
  const cwd = options.cwd || getRepoRoot();

  try {
    const output = execSync(cmd, {
      cwd,
      encoding: 'utf-8',
      stdio: 'pipe',
    });
    return output;
  } catch (error: unknown) {
    if (options.ignoreError) {
      const execError = error as { stdout?: string };
      return execError.stdout || '';
    }
    throw error;
  }
}

function cleanupDockerCompose(composeFile: string): void {
  try {
    executeCommand(`docker-compose -f ${composeFile} down -v --remove-orphans`, {
      ignoreError: true,
    });
  } catch {
    // Ignore cleanup errors
  }
}

async function waitForHealthy(urlOrContainer: string, timeoutMs: number = 120000): Promise<void> {
  const startTime = Date.now();
  const isUrl = urlOrContainer.startsWith('http');

  await waitFor(
    async () => {
      if (isUrl) {
        const response = await globalThis.fetch(urlOrContainer);
        if (response.ok) {
          console.log(`✅ Service healthy: ${urlOrContainer} (took ${Date.now() - startTime}ms)`);
          return true;
        }
        return false;
      } else {
        const runningStatus = executeCommand(
          `docker inspect ${urlOrContainer} --format '{{.State.Running}}'`,
          { ignoreError: true }
        ).trim();

        if (runningStatus === 'true') {
          console.log(
            `✅ Container ${urlOrContainer} is running (took ${Date.now() - startTime}ms)`
          );
          return true;
        }
        return false;
      }
    },
    { timeout: timeoutMs, interval: 500, backoff: 1.5 }
  );
}

async function waitForAnvilReady(timeoutMs: number = 120000): Promise<void> {
  const startTime = Date.now();

  await waitFor(
    async () => {
      const provider = new ethers.JsonRpcProvider(ANVIL_RPC_URL);
      await provider.getBlockNumber();
      console.log(`✅ Anvil RPC is ready (took ${Date.now() - startTime}ms)`);
      return true;
    },
    { timeout: timeoutMs, interval: 500, backoff: 1.5 }
  );
}

async function fundAccountFromFaucet(address: string): Promise<void> {
  try {
    const response = await fetch(`${FAUCET_URL}/api/request`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ address }),
    });

    if (!response.ok) {
      throw new Error(`Faucet request failed: ${response.status} ${response.statusText}`);
    }

    const result = (await response.json()) as {
      transactions: { eth: { hash: string }; token: { hash: string } };
    };
    console.log(
      `Funded ${address} with ETH and tokens (ETH: ${result.transactions.eth.hash.slice(0, 10)}..., Token: ${result.transactions.token.hash.slice(0, 10)}...)`
    );
  } catch (error) {
    console.error(`❌ Failed to fund account from faucet:`, error);
    throw error;
  }
}

// ============================================================================
// Test Suite 1: EVM Payment Channel - SDK Level
// ============================================================================

const dockerAvailable = isDockerAvailable();
const composeAvailable = isDockerComposeAvailable();
const e2eEnabled = process.env.E2E_TESTS === 'true';

const describeIfDockerCompose =
  dockerAvailable && composeAvailable && e2eEnabled ? describe : describe.skip;

describeIfDockerCompose('EVM Payment Channel - SDK Level', () => {
  const COMPOSE_FILE = 'docker-compose-evm-test.yml';

  const logger = pino({ level: process.env.TEST_LOG_LEVEL || 'silent' });

  let provider: ethers.JsonRpcProvider;
  let account0Signer: ethers.Wallet;
  let account1Signer: ethers.Wallet;
  let keyManager0: KeyManager;
  let keyManager1: KeyManager;
  let sdk0: PaymentChannelSDK;
  let sdk1: PaymentChannelSDK;

  // Setup: Verify Docker Compose stack is running (expected to be started externally)
  beforeAll(async () => {
    console.log('Verifying Docker Compose stack (SDK Level)...');

    // Ensure Docker Compose stack is running (start if not already up)
    try {
      await waitForAnvilReady(5000);
      console.log('Anvil already running');
    } catch {
      console.log('Starting Docker Compose stack...');
      cleanupDockerCompose(COMPOSE_FILE);
      executeCommand(`docker compose -f ${COMPOSE_FILE} up -d`);
      console.log('Waiting for Anvil to be ready...');
      await waitForAnvilReady(120000);
      console.log('Waiting for token faucet...');
      await waitForHealthy(FAUCET_URL, 60000);
    }

    // Setup ethers provider and signers
    provider = new ethers.JsonRpcProvider(ANVIL_RPC_URL);
    account0Signer = new ethers.Wallet(ACCOUNT_0_PRIVATE_KEY, provider);
    account1Signer = new ethers.Wallet(ACCOUNT_1_PRIVATE_KEY, provider);

    console.log(`\n👤 Account 0: ${account0Signer.address}`);
    console.log(`👤 Account 1: ${account1Signer.address}`);
    console.log(`🪙  Token: ${TOKEN_ADDRESS}`);
    console.log(`🏛️  Registry: ${REGISTRY_ADDRESS}\n`);

    // Setup KeyManager instances (env backend for testing)
    keyManager0 = new KeyManager(
      { backend: 'env', nodeId: 'account-0', evmPrivateKey: ACCOUNT_0_PRIVATE_KEY },
      logger
    );

    keyManager1 = new KeyManager(
      { backend: 'env', nodeId: 'account-1', evmPrivateKey: ACCOUNT_1_PRIVATE_KEY },
      logger
    );

    // Create SDK instances
    sdk0 = new PaymentChannelSDK(provider, keyManager0, 'evm-key', REGISTRY_ADDRESS, logger);
    sdk1 = new PaymentChannelSDK(provider, keyManager1, 'evm-key', REGISTRY_ADDRESS, logger);

    // Fund accounts with tokens from faucet
    console.log('💰 Funding test accounts from faucet...');
    await fundAccountFromFaucet(account0Signer.address);
    await fundAccountFromFaucet(account1Signer.address);

    // Verify token balances
    const tokenContract = new ethers.Contract(
      TOKEN_ADDRESS,
      ['function balanceOf(address) view returns (uint256)'],
      provider
    );

    const balance0 = await tokenContract.balanceOf!(account0Signer.address);
    const balance1 = await tokenContract.balanceOf!(account1Signer.address);

    console.log(`💵 Account 0 balance: ${ethers.formatEther(balance0)} tokens`);
    console.log(`💵 Account 1 balance: ${ethers.formatEther(balance1)} tokens\n`);

    expect(balance0).toBeGreaterThan(0n);
    expect(balance1).toBeGreaterThan(0n);
  });

  // Teardown: cleanup SDK resources (leave Docker running for subsequent suites)
  afterAll(() => {
    console.log('SDK Level cleanup complete (Docker left running for BLS suite)');
  });

  // Shared across Channel Lifecycle and Channel Dispute Resolution
  let channelId: string;

  describe('Channel Lifecycle', () => {
    const settlementTimeout = 3600; // 1 hour
    const depositAmount = ethers.parseEther('100'); // 100 tokens

    it('should open a payment channel between two parties', async () => {
      console.log('\n📖 Test: Opening payment channel...');

      const result = await sdk0.openChannel(
        account1Signer.address,
        TOKEN_ADDRESS,
        settlementTimeout,
        0n // No initial deposit
      );

      channelId = result.channelId;

      expect(channelId).toBeDefined();
      expect(result.txHash).toBeDefined();
      expect(channelId).toMatch(/^0x[0-9a-f]{64}$/);

      console.log(`✅ Channel opened: ${channelId}`);
    });

    it('should deposit tokens into the channel', async () => {
      console.log('\n📖 Test: Depositing tokens...');

      await sdk0.deposit(channelId, TOKEN_ADDRESS, depositAmount);

      // Verify channel state
      const state = await sdk0.getChannelState(channelId, TOKEN_ADDRESS);

      expect(state.status).toBe('opened');
      expect(state.myDeposit).toBe(depositAmount);

      console.log(`✅ Deposited ${ethers.formatEther(depositAmount)} tokens`);
    });

    it('should create and verify balance proofs', async () => {
      console.log('\n📖 Test: Creating balance proofs...');

      const nonce = 1;
      const transferredAmount = ethers.parseEther('10'); // 10 tokens

      // Account 0 signs a balance proof
      const signature = await sdk0.signBalanceProof(
        channelId,
        nonce,
        transferredAmount,
        0n,
        ethers.ZeroHash
      );

      expect(signature).toBeDefined();
      expect(signature).toMatch(/^0x[0-9a-f]+$/);

      // Account 1 verifies the balance proof
      // First, ensure sdk1 has the token network in its cache by querying channel state
      await sdk1.getChannelState(channelId, TOKEN_ADDRESS);

      const balanceProof: BalanceProof = {
        channelId,
        nonce,
        transferredAmount,
        lockedAmount: 0n,
        locksRoot: ethers.ZeroHash,
      };

      const isValid = await sdk1.verifyBalanceProof(
        balanceProof,
        signature,
        account0Signer.address
      );

      expect(isValid).toBe(true);

      console.log(
        `✅ Balance proof created and verified (${ethers.formatEther(transferredAmount)} tokens transferred)`
      );
    });

    it('should cooperatively settle a channel', async () => {
      console.log('\n📖 Test: Cooperative settlement...');

      // Create final balance proofs for both participants
      // Only account0 has deposited (100 tokens), so only account0 can transfer
      // account1 has no deposit, so its transferredAmount must be 0
      const nonce = 5;
      const account0TransferredAmount = ethers.parseEther('30'); // Account 0 sent 30 tokens
      const account1TransferredAmount = 0n; // Account 1 has no deposit, cannot transfer

      // Account 0 balance proof
      const proof0: BalanceProof = {
        channelId,
        nonce,
        transferredAmount: account0TransferredAmount,
        lockedAmount: 0n,
        locksRoot: ethers.ZeroHash,
      };
      const sig0 = await sdk0.signBalanceProof(
        channelId,
        nonce,
        account0TransferredAmount,
        0n,
        ethers.ZeroHash
      );

      // Ensure sdk1 has the token network cached
      await sdk1.getChannelState(channelId, TOKEN_ADDRESS);

      // Account 1 balance proof
      const proof1: BalanceProof = {
        channelId,
        nonce,
        transferredAmount: account1TransferredAmount,
        lockedAmount: 0n,
        locksRoot: ethers.ZeroHash,
      };
      const sig1 = await sdk1.signBalanceProof(
        channelId,
        nonce,
        account1TransferredAmount,
        0n,
        ethers.ZeroHash
      );

      // Cooperatively settle
      await sdk0.cooperativeSettle(channelId, TOKEN_ADDRESS, proof0, sig0, proof1, sig1);

      // Verify channel is settled
      const state = await sdk0.getChannelState(channelId, TOKEN_ADDRESS);
      expect(state.status).toBe('settled');

      console.log(`✅ Channel cooperatively settled`);
    });
  });

  describe('Channel Dispute Resolution', () => {
    let disputeChannelId: string;
    const settlementTimeout = 3600; // 1 hour (contract MIN_SETTLEMENT_TIMEOUT)
    const depositAmount = ethers.parseEther('100');

    // Fresh provider and signer to avoid nonce conflicts with SDK's KeyManagerSigner
    // (the faucet's token wallet and the SDK both use Account 0, causing stale nonce state)
    let disputeProvider: ethers.JsonRpcProvider;
    let disputeSigner: ethers.Wallet;
    let tokenNetworkContract: ethers.Contract;

    // Account 2 SDK for signing balance proofs as non-closing participant
    let keyManager2: KeyManager;
    let sdk2: PaymentChannelSDK;

    const TOKEN_NETWORK_ABI_SUBSET = [
      'function openChannel(address participant2, uint256 settlementTimeout) external returns (bytes32)',
      'function setTotalDeposit(bytes32 channelId, address participant, uint256 totalDeposit) external',
      'function closeChannel(bytes32 channelId, tuple(bytes32 channelId, uint256 nonce, uint256 transferredAmount, uint256 lockedAmount, bytes32 locksRoot) balanceProof, bytes signature) external',
      'function settleChannel(bytes32 channelId) external',
      'function channels(bytes32) external view returns (uint256 settlementTimeout, uint8 state, uint256 closedAt, uint256 openedAt, address participant1, address participant2)',
      'event ChannelOpened(bytes32 indexed channelId, address indexed participant1, address indexed participant2, uint256 settlementTimeout)',
    ];

    it('should open and deposit for dispute test', async () => {
      console.log('\n📖 Test: Opening channel for dispute scenario...');

      // Create fresh provider/signer with NonceManager to avoid nonce conflicts
      // (the faucet's token wallet and SDK's KeyManagerSigner both use Account 0,
      //  leaving stale nonce state that a plain Wallet can't recover from)
      disputeProvider = new ethers.JsonRpcProvider(ANVIL_RPC_URL);
      disputeSigner = new ethers.NonceManager(
        new ethers.Wallet(ACCOUNT_0_PRIVATE_KEY, disputeProvider)
      ) as unknown as ethers.Wallet;

      // Discover actual TokenNetwork address from registry (may differ from deterministic constant)
      const registry = new ethers.Contract(
        REGISTRY_ADDRESS,
        ['function getTokenNetwork(address token) external view returns (address)'],
        disputeProvider
      );
      const tokenNetworkAddr = await registry.getTokenNetwork!(TOKEN_ADDRESS);
      tokenNetworkContract = new ethers.Contract(
        tokenNetworkAddr,
        TOKEN_NETWORK_ABI_SUBSET,
        disputeSigner
      );

      // Create SDK for account 2 (needed for signing balance proofs)
      keyManager2 = new KeyManager(
        { backend: 'env', nodeId: 'account-2', evmPrivateKey: ACCOUNT_2_PRIVATE_KEY },
        logger
      );
      sdk2 = new PaymentChannelSDK(
        disputeProvider,
        keyManager2,
        'evm-key',
        REGISTRY_ADDRESS,
        logger
      );

      // Open channel directly via contract (avoids SDK nonce management)
      const openTx = await tokenNetworkContract.openChannel!(ACCOUNT_2_ADDRESS, settlementTimeout);
      const receipt = await openTx.wait();

      // Extract channelId from ChannelOpened event
      // In ethers v6, receipt.logs may contain EventLog objects with pre-parsed args
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const channelOpenedLog = receipt.logs.find(
        (log: any) => log.fragment?.name === 'ChannelOpened' || log.eventName === 'ChannelOpened'
      );
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      disputeChannelId = (channelOpenedLog as any).args[0] as string;

      // Approve and deposit tokens
      const tokenContract = new ethers.Contract(
        TOKEN_ADDRESS,
        [
          'function approve(address spender, uint256 amount) external returns (bool)',
          'function allowance(address owner, address spender) external view returns (uint256)',
        ],
        disputeSigner
      );
      const approveTx = await tokenContract.approve!(TOKEN_NETWORK_ADDRESS, ethers.MaxUint256);
      await approveTx.wait();

      const depositTx = await tokenNetworkContract.setTotalDeposit!(
        disputeChannelId,
        await disputeSigner.getAddress(),
        depositAmount
      );
      await depositTx.wait();

      console.log(`✅ Dispute test channel opened and funded: ${disputeChannelId}`);
    });

    it('should close channel with balance proof', async () => {
      console.log('\n📖 Test: Closing channel...');

      // The balance proof is signed by the non-closing participant (account 2).
      // transferredAmount = 0 because account 2 has no deposit to transfer.
      const nonce = 1;
      const transferredAmount = 0n;

      const balanceProof: BalanceProof = {
        channelId: disputeChannelId,
        nonce,
        transferredAmount,
        lockedAmount: 0n,
        locksRoot: ethers.ZeroHash,
      };

      // Populate sdk2's tokenNetworkCache so signBalanceProof can find the channel
      await sdk2.getChannelState(disputeChannelId, TOKEN_ADDRESS);

      // Account 2 (non-closing participant) signs the balance proof
      const signature = await sdk2.signBalanceProof(
        disputeChannelId,
        nonce,
        transferredAmount,
        0n,
        ethers.ZeroHash
      );

      // Account 0 (closer) calls closeChannel with account 2's signature
      await tokenNetworkContract.closeChannel!(disputeChannelId, balanceProof, signature);

      // Verify channel is closed on-chain
      const channelData = await tokenNetworkContract.channels!(disputeChannelId);
      // state enum: 0=NonExistent, 1=Opened, 2=Closed, 3=Settled
      expect(Number(channelData.state)).toBe(2);

      console.log(`✅ Channel closed`);
    });

    it('should wait for challenge period and settle', async () => {
      console.log('\n📖 Test: Waiting for challenge period...');

      // Fast-forward Anvil time past the settlement timeout
      console.log(`⏩ Fast-forwarding ${settlementTimeout + 10} seconds on Anvil...`);
      await disputeProvider.send('evm_increaseTime', [settlementTimeout + 10]);
      await disputeProvider.send('evm_mine', []);

      // Settle the channel on-chain
      const settleTx = await tokenNetworkContract.settleChannel!(disputeChannelId);
      await settleTx.wait();

      // Verify channel is settled on-chain
      const channelData = await tokenNetworkContract.channels!(disputeChannelId);
      // state enum: 0=NonExistent, 1=Opened, 2=Closed, 3=Settled
      expect(Number(channelData.state)).toBe(3);

      console.log(`✅ Channel settled after challenge period`);
    });
  });

  describe('SDK Functionality', () => {
    it('should get TokenNetwork address for a token', async () => {
      const tokenNetworkAddress = await sdk0.getTokenNetworkAddress(TOKEN_ADDRESS);

      expect(tokenNetworkAddress).toBeDefined();
      expect(tokenNetworkAddress).toMatch(/^0x[0-9a-fA-F]{40}$/);

      console.log(`✅ TokenNetwork address: ${tokenNetworkAddress}`);
    });

    it('should list channels for an account', async () => {
      const channels = await sdk0.getMyChannels(TOKEN_ADDRESS);

      expect(Array.isArray(channels)).toBe(true);
      expect(channels.length).toBeGreaterThan(0);

      console.log(`✅ Found ${channels.length} channel(s) for account`);
    });
  });
});

// ============================================================================
// Test Suite 2: EVM Payment Channel - BLS Integration
// ============================================================================

// Admin API Types
interface OpenChannelRequest {
  peerId: string;
  chain: string;
  token: string;
  tokenNetwork: string;
  initialDeposit: string;
  settlementTimeout: number;
  peerAddress: string;
}

interface OpenChannelResponse {
  channelId: string;
  status: string;
}

// Connector Management
interface ConnectorInstance {
  node: ConnectorNode;
  adminApp: Express;
  adminServer: ReturnType<Express['listen']>;
  port: number;
}

async function createConnectorInstance(
  nodeId: string,
  adminPort: number,
  privateKey: string,
  btpPort: number
): Promise<ConnectorInstance> {
  const logger = pino({ level: process.env.TEST_LOG_LEVEL || 'silent' });

  const config: ConnectorConfig = {
    nodeId,
    btpServerPort: btpPort,
    healthCheckPort: adminPort + 1000,
    deploymentMode: 'embedded',
    adminApi: {
      enabled: true,
      port: adminPort,
    },
    localDelivery: { enabled: false },
    settlementInfra: {
      enabled: true,
      rpcUrl: ANVIL_RPC_URL,
      registryAddress: REGISTRY_ADDRESS,
      tokenAddress: TOKEN_ADDRESS,
      privateKey,
    },
    peers: [],
    routes: [],
    environment: 'development',
  };

  const node = new ConnectorNode(config, logger);
  await node.start();

  // Create admin API server
  const adminApp = express();
  adminApp.use(express.json());

  const adminRouter = await createAdminRouter({
    routingTable: node.routingTable,
    btpClientManager: node.btpClientManager,
    logger,
    nodeId,
    settlementPeers: new Map(),
    channelManager: node.channelManager,
    paymentChannelSDK: node.paymentChannelSDK,
    accountManager: node.accountManager,
  } as AdminAPIConfig);

  adminApp.use('/admin', adminRouter);

  const adminServer = adminApp.listen(adminPort);

  console.log(`✅ Connector ${nodeId} started (admin: ${adminPort}, btp: ${btpPort})`);

  return { node, adminApp, adminServer, port: adminPort };
}

async function stopConnectorInstance(instance: ConnectorInstance): Promise<void> {
  instance.adminServer.close();
  await instance.node.stop();
}

// Admin API Helpers
async function openChannel(
  adminPort: number,
  request: OpenChannelRequest
): Promise<OpenChannelResponse> {
  const response = await fetch(`http://localhost:${adminPort}/admin/channels`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(request),
  });

  if (!response.ok) {
    const error = await response.text();
    throw new Error(`Failed to open channel: ${response.status} ${error}`);
  }

  return (await response.json()) as OpenChannelResponse;
}

async function getChannel(adminPort: number, channelId: string): Promise<Record<string, unknown>> {
  const response = await fetch(`http://localhost:${adminPort}/admin/channels/${channelId}`);

  if (!response.ok) {
    const error = await response.text();
    throw new Error(`Failed to get channel: ${response.status} ${error}`);
  }

  return await response.json();
}

async function depositToChannel(
  adminPort: number,
  channelId: string,
  amount: string
): Promise<Record<string, unknown>> {
  const response = await fetch(
    `http://localhost:${adminPort}/admin/channels/${channelId}/deposit`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ amount }),
    }
  );

  if (!response.ok) {
    const error = await response.text();
    throw new Error(`Failed to deposit: ${response.status} ${error}`);
  }

  return await response.json();
}

async function listChannels(adminPort: number): Promise<Record<string, unknown>[]> {
  const response = await fetch(`http://localhost:${adminPort}/admin/channels`);

  if (!response.ok) {
    const error = await response.text();
    throw new Error(`Failed to list channels: ${response.status} ${error}`);
  }

  return (await response.json()) as Record<string, unknown>[];
}

const describeIfE2E = e2eEnabled ? describe : describe.skip;

describeIfE2E('EVM Payment Channel - BLS Integration (Fast - No Docker Build)', () => {
  const CONNECTOR_A_ADMIN_PORT = 8081;
  const CONNECTOR_B_ADMIN_PORT = 8082;

  let connectorA: ConnectorInstance;
  let connectorB: ConnectorInstance;

  beforeAll(async () => {
    console.log('\n🚀 Starting EVM Payment Channel BLS Integration Test');
    console.log('===================================================\n');

    // Check infrastructure (use RPC call, not HTTP GET - Anvil returns 400 for plain GET)
    console.log('⏳ Checking infrastructure...');
    await waitForAnvilReady(60000);
    console.log('');

    // Fund accounts (use accounts 2/3 to avoid faucet rate limits from Suite 1)
    console.log('💰 Funding test accounts...');
    await fundAccountFromFaucet(ACCOUNT_2_ADDRESS);
    await fundAccountFromFaucet(ACCOUNT_3_ADDRESS);
    console.log('');

    // Start connectors directly (no Docker!)
    console.log('🔧 Starting connectors from source...');
    connectorA = await createConnectorInstance(
      'connector-a',
      CONNECTOR_A_ADMIN_PORT,
      ACCOUNT_2_PRIVATE_KEY,
      4001
    );

    connectorB = await createConnectorInstance(
      'connector-b',
      CONNECTOR_B_ADMIN_PORT,
      ACCOUNT_3_PRIVATE_KEY,
      4002
    );

    console.log('');
    console.log('========================================');
    console.log('✅ All services ready!');
    console.log('========================================');
    console.log(`Connector A: http://localhost:${CONNECTOR_A_ADMIN_PORT}/admin`);
    console.log(`Connector B: http://localhost:${CONNECTOR_B_ADMIN_PORT}/admin`);
    console.log(`Token:       ${TOKEN_ADDRESS}`);
    console.log(`Registry:    ${REGISTRY_ADDRESS}`);
    console.log('========================================\n');
  });

  afterAll(async () => {
    console.log('\n🧹 Stopping connectors...');
    if (connectorA) await stopConnectorInstance(connectorA);
    if (connectorB) await stopConnectorInstance(connectorB);
    console.log('✅ Cleanup complete\n');
  });

  describe('BLS → Admin API Flow', () => {
    let channelId: string;

    it('should open a payment channel via Admin API', async () => {
      console.log('\n📖 Test: Opening channel via HTTP POST...');

      const request: OpenChannelRequest = {
        peerId: 'connector-b',
        chain: 'evm:base:8453',
        token: TOKEN_ADDRESS,
        tokenNetwork: REGISTRY_ADDRESS,
        initialDeposit: '0',
        settlementTimeout: 3600,
        peerAddress: ACCOUNT_3_ADDRESS,
      };

      const response = await openChannel(CONNECTOR_A_ADMIN_PORT, request);

      channelId = response.channelId;

      expect(response.channelId).toBeDefined();
      expect(response.channelId).toMatch(/^0x[0-9a-f]{64}$/);
      expect(response.status).toBe('open');

      console.log(`✅ Channel opened: ${channelId.slice(0, 10)}...`);
    });

    it('should retrieve channel details via GET', async () => {
      console.log('\n📖 Test: Retrieving channel details via HTTP GET...');

      const details = await getChannel(CONNECTOR_A_ADMIN_PORT, channelId);

      expect(details.channelId).toBe(channelId);
      expect(details.status).toBe('open');

      console.log(`✅ Channel retrieved: status=${details.status}`);
    });

    it('should list all channels via GET', async () => {
      console.log('\n📖 Test: Listing channels via HTTP GET...');

      const channels = await listChannels(CONNECTOR_A_ADMIN_PORT);

      expect(Array.isArray(channels)).toBe(true);
      expect(channels.length).toBeGreaterThan(0);

      const ourChannel = channels.find((ch) => ch.channelId === channelId);
      expect(ourChannel).toBeDefined();

      console.log(`✅ Found ${channels.length} channel(s)`);
    });

    it('should deposit tokens via POST', async () => {
      console.log('\n📖 Test: Depositing tokens via HTTP POST...');

      const depositAmount = '100000000000000000000'; // 100 tokens

      const response = await depositToChannel(CONNECTOR_A_ADMIN_PORT, channelId, depositAmount);

      expect(response.channelId).toBe(channelId);
      expect(response.deposit).toBeDefined();

      console.log(`✅ Deposited 100 tokens`);
    });

    it('should verify deposit in channel state', async () => {
      console.log('\n📖 Test: Verifying deposit via HTTP GET...');

      const details = await getChannel(CONNECTOR_A_ADMIN_PORT, channelId);

      expect(details.deposit).toBeDefined();
      expect(BigInt(details.deposit)).toBeGreaterThan(0n);

      console.log(`✅ Deposit verified: ${details.deposit} wei`);
    });
  });

  describe('Error Handling', () => {
    it('should reject invalid chain format', async () => {
      console.log('\n📖 Test: Testing invalid chain format...');

      const request: OpenChannelRequest = {
        peerId: 'connector-b',
        chain: 'invalid-chain',
        token: TOKEN_ADDRESS,
        tokenNetwork: REGISTRY_ADDRESS,
        initialDeposit: '0',
        settlementTimeout: 3600,
        peerAddress: ACCOUNT_3_ADDRESS,
      };

      await expect(openChannel(CONNECTOR_A_ADMIN_PORT, request)).rejects.toThrow();

      console.log(`✅ Invalid chain format rejected`);
    });

    it('should reject invalid token address', async () => {
      console.log('\n📖 Test: Testing invalid token address...');

      const request: OpenChannelRequest = {
        peerId: 'connector-b',
        chain: 'evm:base:8453',
        token: 'not-an-address',
        tokenNetwork: REGISTRY_ADDRESS,
        initialDeposit: '0',
        settlementTimeout: 3600,
        peerAddress: ACCOUNT_3_ADDRESS,
      };

      await expect(openChannel(CONNECTOR_A_ADMIN_PORT, request)).rejects.toThrow();

      console.log(`✅ Invalid token address rejected`);
    });
  });
});

// ============================================================================
// Test Suite 3: EVM Payment Channel - E2E with Docker Compose
// ============================================================================

const describeIfDockerE2E =
  dockerAvailable && composeAvailable && e2eEnabled ? describe : describe.skip;

describeIfDockerE2E('EVM Payment Channel - E2E with Docker Compose', () => {
  const COMPOSE_FILE = 'docker-compose-base-e2e-test.yml';
  const CONNECTOR_A_ADMIN_URL = 'http://localhost:8081';
  const CONNECTOR_A_HEALTH_URL = 'http://localhost:8080/health';
  const CONNECTOR_B_ADMIN_URL = 'http://localhost:8082';
  const CONNECTOR_B_HEALTH_URL = 'http://localhost:8090/health';

  interface ChannelDetailResponse {
    channelId: string;
    status: string;
    chain: string;
    deposit?: string;
  }

  interface DepositRequest {
    amount: string;
  }

  interface DepositResponse {
    channelId: string;
    deposit: string;
  }

  interface CloseChannelRequest {
    cooperative?: boolean;
  }

  interface CloseChannelResponse {
    channelId: string;
    status: string;
    txHash?: string;
  }

  async function openChannelE2E(
    adminUrl: string,
    request: OpenChannelRequest
  ): Promise<OpenChannelResponse> {
    const response = await globalThis.fetch(`${adminUrl}/admin/channels`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(request),
    });

    if (!response.ok) {
      const error = await response.text();
      throw new Error(`Failed to open channel: ${response.status} ${error}`);
    }

    return (await response.json()) as OpenChannelResponse;
  }

  async function getChannelE2E(
    adminUrl: string,
    channelId: string
  ): Promise<ChannelDetailResponse> {
    const response = await globalThis.fetch(`${adminUrl}/admin/channels/${channelId}`);

    if (!response.ok) {
      const error = await response.text();
      throw new Error(`Failed to get channel: ${response.status} ${error}`);
    }

    return (await response.json()) as ChannelDetailResponse;
  }

  async function listChannelsE2E(adminUrl: string): Promise<ChannelDetailResponse[]> {
    const response = await globalThis.fetch(`${adminUrl}/admin/channels`);

    if (!response.ok) {
      const error = await response.text();
      throw new Error(`Failed to list channels: ${response.status} ${error}`);
    }

    return (await response.json()) as ChannelDetailResponse[];
  }

  async function depositToChannelE2E(
    adminUrl: string,
    channelId: string,
    request: DepositRequest
  ): Promise<DepositResponse> {
    const response = await globalThis.fetch(`${adminUrl}/admin/channels/${channelId}/deposit`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(request),
    });

    if (!response.ok) {
      const error = await response.text();
      throw new Error(`Failed to deposit: ${response.status} ${error}`);
    }

    return (await response.json()) as DepositResponse;
  }

  async function closeChannel(
    adminUrl: string,
    channelId: string,
    request: CloseChannelRequest
  ): Promise<CloseChannelResponse> {
    const response = await globalThis.fetch(`${adminUrl}/admin/channels/${channelId}/close`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(request),
    });

    if (!response.ok) {
      const error = await response.text();
      throw new Error(`Failed to close channel: ${response.status} ${error}`);
    }

    return (await response.json()) as CloseChannelResponse;
  }

  // Setup: Start Docker Compose stack
  beforeAll(async () => {
    console.log('🚀 Starting Docker Compose E2E stack...');
    console.log('   This may take 2-3 minutes on first run (downloading fork)...');
    console.log('');

    // Cleanup any previous state
    cleanupDockerCompose(COMPOSE_FILE);

    // Start all services
    console.log('📦 Starting services...');
    executeCommand(`docker-compose -f ${COMPOSE_FILE} up -d`);

    // Wait for Anvil
    console.log('⏳ Waiting for Anvil...');
    await waitForHealthy(ANVIL_RPC_URL, 120000);

    // Wait for contract deployment (check deployer logs)
    console.log('⏳ Waiting for contract deployment...');
    await new Promise((resolve) => setTimeout(resolve, 15000));

    // Wait for faucet
    console.log('⏳ Waiting for token faucet...');
    await new Promise((resolve) => setTimeout(resolve, 10000));

    // Fund connector accounts
    console.log('💰 Funding connector accounts...');
    await fundAccountFromFaucet(ACCOUNT_0_ADDRESS);
    await fundAccountFromFaucet(ACCOUNT_1_ADDRESS);

    // Wait for TigerBeetle
    console.log('⏳ Waiting for TigerBeetle...');
    await new Promise((resolve) => setTimeout(resolve, 10000));

    // Wait for connectors to be healthy
    console.log('⏳ Waiting for Connector A...');
    await waitForHealthy(CONNECTOR_A_HEALTH_URL, 120000);

    console.log('⏳ Waiting for Connector B...');
    await waitForHealthy(CONNECTOR_B_HEALTH_URL, 120000);

    console.log('');
    console.log('========================================');
    console.log('✅ All services ready!');
    console.log('========================================');
    console.log(`Connector A Admin API: ${CONNECTOR_A_ADMIN_URL}`);
    console.log(`Connector B Admin API: ${CONNECTOR_B_ADMIN_URL}`);
    console.log(`Anvil RPC:             ${ANVIL_RPC_URL}`);
    console.log(`Token Address:         ${TOKEN_ADDRESS}`);
    console.log(`Registry Address:      ${REGISTRY_ADDRESS}`);
    console.log(`TokenNetwork Address:  ${TOKEN_NETWORK_ADDRESS}`);
    console.log('========================================');
    console.log('');
  });

  // Teardown: Stop Docker Compose stack
  afterAll(() => {
    console.log('');
    if (isDockerAvailable() && isDockerComposeAvailable()) {
      cleanupDockerCompose(COMPOSE_FILE);
    }
  });

  describe('BLS → Admin API → Payment Channel Flow', () => {
    let channelIdA: string;
    let channelIdB: string;

    it('should open a payment channel from Connector A via Admin API', async () => {
      console.log('\n📖 Test: Opening channel from Connector A...');

      const request: OpenChannelRequest = {
        peerId: 'connector-b',
        chain: 'evm:base:8453',
        token: TOKEN_ADDRESS,
        tokenNetwork: TOKEN_NETWORK_ADDRESS,
        initialDeposit: '0', // Start with no deposit
        settlementTimeout: 3600, // 1 hour
        peerAddress: ACCOUNT_1_ADDRESS,
      };

      const response = await openChannelE2E(CONNECTOR_A_ADMIN_URL, request);

      channelIdA = response.channelId;

      expect(response.channelId).toBeDefined();
      expect(response.channelId).toMatch(/^0x[0-9a-f]{64}$/);
      expect(response.status).toBe('open');

      console.log(`✅ Channel opened: ${channelIdA}`);
      console.log(`   Status: ${response.status}`);
    });

    it('should retrieve channel details via Admin API', async () => {
      console.log('\n📖 Test: Retrieving channel details...');

      const details = await getChannelE2E(CONNECTOR_A_ADMIN_URL, channelIdA);

      expect(details.channelId).toBe(channelIdA);
      expect(details.status).toBe('open');
      expect(details.chain).toBe('evm:base:8453');

      console.log(`✅ Channel details retrieved`);
      console.log(`   Channel ID: ${details.channelId}`);
      console.log(`   Status: ${details.status}`);
      console.log(`   Chain: ${details.chain}`);
    });

    it('should list all channels via Admin API', async () => {
      console.log('\n📖 Test: Listing all channels...');

      const channels = await listChannelsE2E(CONNECTOR_A_ADMIN_URL);

      expect(Array.isArray(channels)).toBe(true);
      expect(channels.length).toBeGreaterThan(0);

      const ourChannel = channels.find((ch) => ch.channelId === channelIdA);
      expect(ourChannel).toBeDefined();
      expect(ourChannel?.status).toBe('open');

      console.log(`✅ Listed ${channels.length} channel(s)`);
      console.log(`   Found our channel: ${channelIdA}`);
    });

    it('should deposit tokens to the channel via Admin API', async () => {
      console.log('\n📖 Test: Depositing tokens...');

      const depositAmount = '100000000000000000000'; // 100 tokens (18 decimals)

      const request: DepositRequest = {
        amount: depositAmount,
      };

      const response = await depositToChannelE2E(CONNECTOR_A_ADMIN_URL, channelIdA, request);

      expect(response.channelId).toBe(channelIdA);
      expect(response.deposit).toBeDefined();
      expect(BigInt(response.deposit)).toBeGreaterThan(0n);

      console.log(`✅ Deposited ${depositAmount} wei (100 tokens)`);
      console.log(`   Channel ID: ${response.channelId}`);
      console.log(`   Total deposit: ${response.deposit}`);
    });

    it('should verify deposit reflects in channel state', async () => {
      console.log('\n📖 Test: Verifying deposit in channel state...');

      const details = await getChannelE2E(CONNECTOR_A_ADMIN_URL, channelIdA);

      expect(details.deposit).toBeDefined();
      expect(BigInt(details.deposit!)).toBeGreaterThan(0n);

      console.log(`✅ Deposit verified in channel state`);
      console.log(`   Total deposit: ${details.deposit}`);
    });

    it('should open a second channel from Connector B with initial deposit', async () => {
      console.log('\n📖 Test: Opening channel from Connector B...');

      const request: OpenChannelRequest = {
        peerId: 'connector-a',
        chain: 'evm:base:8453',
        token: TOKEN_ADDRESS,
        tokenNetwork: TOKEN_NETWORK_ADDRESS,
        initialDeposit: '50000000000000000000', // 50 tokens initial deposit
        settlementTimeout: 3600,
        peerAddress: ACCOUNT_0_ADDRESS,
      };

      const response = await openChannelE2E(CONNECTOR_B_ADMIN_URL, request);

      channelIdB = response.channelId;

      expect(response.channelId).toBeDefined();
      expect(response.status).toBe('open');

      console.log(`✅ Channel opened with initial deposit: ${channelIdB}`);
    });

    it('should cooperatively close a channel via Admin API', async () => {
      console.log('\n📖 Test: Cooperatively closing channel...');

      const request: CloseChannelRequest = {
        cooperative: true,
      };

      const response = await closeChannel(CONNECTOR_A_ADMIN_URL, channelIdA, request);

      expect(response.channelId).toBe(channelIdA);
      expect(['closing', 'closed', 'settled']).toContain(response.status);
      expect(response.txHash).toBeDefined();

      console.log(`✅ Channel close initiated`);
      console.log(`   Channel ID: ${response.channelId}`);
      console.log(`   Status: ${response.status}`);
      console.log(`   Tx Hash: ${response.txHash}`);
    });

    it('should verify channel status after close', async () => {
      console.log('\n📖 Test: Verifying channel status after close...');

      // Wait a bit for on-chain state to update
      await new Promise((resolve) => setTimeout(resolve, 3000));

      const details = await getChannelE2E(CONNECTOR_A_ADMIN_URL, channelIdA);

      expect(['closing', 'closed', 'settled']).toContain(details.status);

      console.log(`✅ Channel status verified`);
      console.log(`   Status: ${details.status}`);
    });

    it('should handle duplicate channel open request gracefully', async () => {
      console.log('\n📖 Test: Testing duplicate channel detection...');

      const request: OpenChannelRequest = {
        peerId: 'connector-b',
        chain: 'evm:base:8453',
        token: TOKEN_ADDRESS,
        tokenNetwork: TOKEN_NETWORK_ADDRESS,
        initialDeposit: '0',
        settlementTimeout: 3600,
        peerAddress: ACCOUNT_1_ADDRESS,
      };

      // Should either return existing channel or create new one
      const response = await openChannelE2E(CONNECTOR_A_ADMIN_URL, request);

      expect(response.channelId).toBeDefined();
      expect(response.status).toBeDefined();

      console.log(`✅ Duplicate handling works`);
      console.log(`   Returned channel: ${response.channelId}`);
      console.log(`   Status: ${response.status}`);
    });
  });

  describe('Admin API Error Handling', () => {
    it('should reject invalid chain format', async () => {
      console.log('\n📖 Test: Testing invalid chain format...');

      const request: OpenChannelRequest = {
        peerId: 'connector-b',
        chain: 'invalid-chain-format', // Invalid
        token: TOKEN_ADDRESS,
        tokenNetwork: TOKEN_NETWORK_ADDRESS,
        initialDeposit: '0',
        settlementTimeout: 3600,
        peerAddress: ACCOUNT_1_ADDRESS,
      };

      await expect(openChannelE2E(CONNECTOR_A_ADMIN_URL, request)).rejects.toThrow();

      console.log(`✅ Invalid chain format rejected`);
    });

    it('should reject invalid token address', async () => {
      console.log('\n📖 Test: Testing invalid token address...');

      const request: OpenChannelRequest = {
        peerId: 'connector-b',
        chain: 'evm:base:8453',
        token: 'not-a-valid-address', // Invalid
        tokenNetwork: TOKEN_NETWORK_ADDRESS,
        initialDeposit: '0',
        settlementTimeout: 3600,
        peerAddress: ACCOUNT_1_ADDRESS,
      };

      await expect(openChannelE2E(CONNECTOR_A_ADMIN_URL, request)).rejects.toThrow();

      console.log(`✅ Invalid token address rejected`);
    });

    it('should reject invalid deposit amount', async () => {
      console.log('\n📖 Test: Testing invalid deposit amount...');

      const request: DepositRequest = {
        amount: 'not-a-number', // Invalid
      };

      // Create a valid channel first (reuse from previous tests)
      const openRequest: OpenChannelRequest = {
        peerId: 'connector-b',
        chain: 'evm:base:8453',
        token: TOKEN_ADDRESS,
        tokenNetwork: TOKEN_NETWORK_ADDRESS,
        initialDeposit: '0',
        settlementTimeout: 3600,
        peerAddress: ACCOUNT_1_ADDRESS,
      };

      const { channelId } = await openChannelE2E(CONNECTOR_A_ADMIN_URL, openRequest);

      await expect(
        depositToChannelE2E(CONNECTOR_A_ADMIN_URL, channelId, request)
      ).rejects.toThrow();

      console.log(`✅ Invalid deposit amount rejected`);
    });

    it('should handle non-existent channel gracefully', async () => {
      console.log('\n📖 Test: Testing non-existent channel...');

      const fakeChannelId = '0x' + '0'.repeat(64);

      await expect(getChannelE2E(CONNECTOR_A_ADMIN_URL, fakeChannelId)).rejects.toThrow();

      console.log(`✅ Non-existent channel handled gracefully`);
    });
  });
});
