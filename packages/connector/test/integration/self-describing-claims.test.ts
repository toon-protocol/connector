/**
 * Self-Describing Claims Integration Tests (Story 31.3)
 *
 * End-to-end integration tests validating the complete self-describing claims flow
 * against a local Anvil blockchain. Tests dynamic on-chain verification for unknown
 * channels, caching behavior, tampered field rejection, and backward compatibility
 * with pre-registered channels.
 *
 * Prerequisites:
 *   docker compose -f docker-compose-evm-test.yml up -d
 *
 * Run:
 *   EVM_INTEGRATION=true npm test -- --testPathPattern=self-describing-claims
 *
 * @see Epic 31 - Self-Describing BTP Claims & Dynamic Channel Verification
 * @see Story 31.3 - Integration Tests & Backward Compatibility Verification
 */

/* eslint-disable no-console */

import { BTPServer } from '../../src/btp/btp-server';
import { BTPClient, Peer } from '../../src/btp/btp-client';
import type { BTPClientManager } from '../../src/btp/btp-client-manager';
import { ClaimSender } from '../../src/settlement/claim-sender';
import { ClaimReceiver } from '../../src/settlement/claim-receiver';
import { PaymentChannelSDK } from '../../src/settlement/payment-channel-sdk';
import { ChannelManager } from '../../src/settlement/channel-manager';
import type { ChannelManagerConfig } from '../../src/settlement/channel-manager';
import { KeyManager } from '../../src/security/key-manager';
import { PacketHandler } from '../../src/core/packet-handler';
import { RoutingTable } from '../../src/routing/routing-table';
import { TelemetryEmitter } from '../../src/telemetry/telemetry-emitter';
import { initializeClaimReceiverSchema } from '../../src/settlement/claim-receiver-db-schema';
import {
  SENT_CLAIMS_TABLE_SCHEMA,
  SENT_CLAIMS_INDEXES,
} from '../../src/settlement/claim-sender-db-schema';
import { EventEmitter } from 'events';
import type { SettlementExecutor } from '../../src/settlement/settlement-executor';
import Database from 'better-sqlite3';
import { TelemetryEvent, ClaimReceivedEvent } from '@crosstown/shared';
import { ethers } from 'ethers';
import pino from 'pino';
import { waitFor } from '../helpers/wait-for';

// ============================================================================
// Environment Gating
// ============================================================================

const evmIntegration = process.env.EVM_INTEGRATION === 'true';
const describeIfEVM = evmIntegration ? describe : describe.skip;

// ============================================================================
// Configuration Constants
// ============================================================================

const ANVIL_RPC_URL = 'http://localhost:8545';
const FAUCET_URL = 'http://localhost:3500';

// Deployed contract addresses (deterministic from Anvil deploy script)
const TOKEN_ADDRESS = '0x5FbDB2315678afecb367f032d93F642f64180aa3';
const REGISTRY_ADDRESS = '0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512';

// Anvil well-known accounts
const CONNECTOR_A_PRIVATE_KEY =
  '0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a';
const CONNECTOR_B_PRIVATE_KEY =
  '0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6';
const CONNECTOR_A_ADDRESS = '0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC';
const CONNECTOR_B_ADDRESS = '0x90F79bf6EB2c4f870365E785982E1f101E93b906';

// ============================================================================
// Helpers
// ============================================================================

/** Creates a pino logger for tests (silent unless DEBUG is set) */
function createTestLogger(nodeId: string): pino.Logger {
  return pino({ level: process.env.DEBUG ? 'debug' : 'silent', name: nodeId });
}

/** Funds an Anvil account with ETH and tokens via the faucet service */
async function fundAccountFromFaucet(address: string): Promise<void> {
  const response = await fetch(`${FAUCET_URL}/api/request`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ address }),
  });
  if (!response.ok) {
    throw new Error(`Faucet request failed: ${response.status} ${response.statusText}`);
  }
  console.log(`  Funded ${address.slice(0, 10)}...`);
}

/**
 * Wrapper around shared waitFor utility that returns boolean.
 */
async function waitForCondition(
  conditionFn: () => boolean | Promise<boolean>,
  timeoutMs = 5000,
  pollIntervalMs = 100
): Promise<boolean> {
  try {
    await waitFor(conditionFn, { timeout: timeoutMs, interval: pollIntervalMs });
    return true;
  } catch {
    return false;
  }
}

/** Database row type for received_claims */
interface ReceivedClaimRow {
  message_id: string;
  peer_id: string;
  blockchain: string;
  channel_id: string;
  claim_data: string;
  verified: number;
  received_at: number;
  redeemed_at: number | null;
  redemption_tx_hash: string | null;
}

// ============================================================================
// Test Suite
// ============================================================================

describeIfEVM('Self-Describing Claims Integration', () => {
  jest.setTimeout(60000);

  // Shared Anvil infrastructure
  let provider: ethers.JsonRpcProvider;
  let sdkA: PaymentChannelSDK;
  let sdkB: PaymentChannelSDK;

  // BTP topology
  let btpServer: BTPServer;
  let btpClient: BTPClient;
  let claimSender: ClaimSender;
  let claimReceiver: ClaimReceiver;
  let channelManager: ChannelManager;
  let senderDb: Database.Database;
  let receiverDb: Database.Database;
  let telemetryEmitterA: TelemetryEmitter;
  let telemetryEmitterB: TelemetryEmitter;
  let telemetryEventsA: TelemetryEvent[];
  let telemetryEventsB: TelemetryEvent[];
  let serverPort: number;

  // Self-describing claim metadata (fetched once from chain)
  let chainId: number;
  let tokenNetworkAddress: string;

  // ============================================================================
  // Setup: Anvil infrastructure + two-connector BTP topology
  // ============================================================================

  beforeAll(async () => {
    console.log('\n--- Self-Describing Claims Integration Test ---');

    // 1. Verify Anvil is running
    console.log('Checking Anvil health...');
    provider = new ethers.JsonRpcProvider(ANVIL_RPC_URL);
    await waitFor(
      async () => {
        await provider.getBlockNumber();
        return true;
      },
      { timeout: 30000, interval: 500, backoff: 1.5 }
    );
    console.log('  Anvil is ready');

    // 2. Verify faucet is running
    console.log('Checking faucet health...');
    await waitFor(
      async () => {
        const resp = await fetch(FAUCET_URL);
        return resp.ok || resp.status === 404;
      },
      { timeout: 30000, interval: 500, backoff: 1.5 }
    );
    console.log('  Faucet is ready');

    // 3. Fund test accounts
    console.log('Funding test accounts...');
    await fundAccountFromFaucet(CONNECTOR_A_ADDRESS);
    await fundAccountFromFaucet(CONNECTOR_B_ADDRESS);

    // 4. Set up PaymentChannelSDK instances
    const loggerSdkA = createTestLogger('sdk-a');
    const loggerSdkB = createTestLogger('sdk-b');

    const keyManagerA = new KeyManager(
      { backend: 'env', nodeId: 'connector-a', evmPrivateKey: CONNECTOR_A_PRIVATE_KEY },
      loggerSdkA
    );
    sdkA = new PaymentChannelSDK(provider, keyManagerA, 'evm-key', REGISTRY_ADDRESS, loggerSdkA);

    const keyManagerB = new KeyManager(
      { backend: 'env', nodeId: 'connector-b', evmPrivateKey: CONNECTOR_B_PRIVATE_KEY },
      loggerSdkB
    );
    sdkB = new PaymentChannelSDK(provider, keyManagerB, 'evm-key', REGISTRY_ADDRESS, loggerSdkB);

    // 5. Fetch chain metadata once
    chainId = await sdkA.getChainId();
    tokenNetworkAddress = await sdkA.getTokenNetworkAddress(TOKEN_ADDRESS);
    console.log(`  Chain ID: ${chainId}, TokenNetwork: ${tokenNetworkAddress.slice(0, 10)}...`);

    // 6. Set up two-connector BTP topology
    console.log('Setting up BTP topology...');
    serverPort = 30000 + Math.floor(Math.random() * 10000);
    const loggerA = createTestLogger('connector-a');
    const loggerB = createTestLogger('connector-b');

    // -- Connector B (receiver) --
    process.env['BTP_PEER_CONNECTOR_A_SECRET'] = '';

    receiverDb = new Database(':memory:');
    initializeClaimReceiverSchema(receiverDb);

    telemetryEventsB = [];
    telemetryEmitterB = new TelemetryEmitter('ws://localhost:9999', 'connector-b', loggerB);
    telemetryEmitterB.onEvent((event: TelemetryEvent) => {
      telemetryEventsB.push(event);
    });

    const routingTableB = new RoutingTable(undefined, loggerB);
    const mockBtpClientManagerB = {
      getClientForPeer: jest.fn(),
    } as unknown as BTPClientManager;
    const packetHandlerB = new PacketHandler(
      routingTableB,
      mockBtpClientManagerB,
      'connector-b',
      loggerB
    );
    btpServer = new BTPServer(loggerB, packetHandlerB);
    await btpServer.start(serverPort);

    // ChannelManager for Connector B
    const mockSettlementExecutor = new EventEmitter() as unknown as SettlementExecutor;
    const channelManagerConfig: ChannelManagerConfig = {
      nodeId: 'connector-b',
      defaultSettlementTimeout: 86400,
      initialDepositMultiplier: 10,
      idleChannelThreshold: 86400,
      minDepositThreshold: 0.5,
      idleCheckInterval: 3600,
      tokenAddressMap: new Map<string, string>([['AGENT', TOKEN_ADDRESS]]),
      peerIdToAddressMap: new Map<string, string>([['connector-a', CONNECTOR_A_ADDRESS]]),
      registryAddress: REGISTRY_ADDRESS,
      rpcUrl: ANVIL_RPC_URL,
      privateKey: CONNECTOR_B_PRIVATE_KEY,
    };
    channelManager = new ChannelManager(
      channelManagerConfig,
      sdkB,
      mockSettlementExecutor,
      loggerB,
      telemetryEmitterB
    );

    // ClaimReceiver WITH channelManager for dynamic verification
    claimReceiver = new ClaimReceiver(
      receiverDb,
      sdkB,
      loggerB,
      telemetryEmitterB,
      'connector-b',
      channelManager
    );
    claimReceiver.registerWithBTPServer(btpServer);

    // -- Connector A (sender) --
    senderDb = new Database(':memory:');
    senderDb.exec(SENT_CLAIMS_TABLE_SCHEMA);
    SENT_CLAIMS_INDEXES.forEach((indexSQL) => senderDb.exec(indexSQL));

    telemetryEventsA = [];
    telemetryEmitterA = new TelemetryEmitter('ws://localhost:9999', 'connector-a', loggerA);
    telemetryEmitterA.onEvent((event: TelemetryEvent) => {
      telemetryEventsA.push(event);
    });

    claimSender = new ClaimSender(senderDb, loggerA, telemetryEmitterA, 'connector-a');

    process.env['BTP_PEER_CONNECTORB_SECRET'] = '';
    const peerB: Peer = {
      id: 'connector-b',
      url: `ws://localhost:${serverPort}`,
      authToken: '',
      connected: false,
      lastSeen: new Date(),
    };
    btpClient = new BTPClient(peerB, 'connector-a', loggerA);

    // Establish BTP connection with retry
    let connectionEstablished = false;
    for (let attempt = 1; attempt <= 3; attempt++) {
      try {
        await btpClient.connect();
        connectionEstablished = true;
        break;
      } catch (error) {
        loggerA.warn({ attempt, error }, 'BTP connection attempt failed');
        if (attempt < 3) {
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    }
    if (!connectionEstablished) {
      throw new Error('Failed to establish BTP connection after 3 attempts');
    }

    const connected = await waitForCondition(() => btpClient.isConnected, 5000);
    if (!connected) {
      throw new Error('BTP connection not established within timeout');
    }

    console.log('  BTP topology established');
    console.log('--- Setup complete ---\n');
  });

  // ============================================================================
  // Teardown
  // ============================================================================

  afterAll(async () => {
    console.log('\n--- Cleanup ---');
    try {
      await telemetryEmitterA?.disconnect();
    } catch {
      /* cleanup */
    }
    try {
      await telemetryEmitterB?.disconnect();
    } catch {
      /* cleanup */
    }
    try {
      await btpClient?.disconnect();
    } catch {
      /* cleanup */
    }
    try {
      btpServer?.stop();
    } catch {
      /* cleanup */
    }
    try {
      senderDb?.close();
    } catch {
      /* cleanup */
    }
    try {
      receiverDb?.close();
    } catch {
      /* cleanup */
    }
    delete process.env['BTP_PEER_CONNECTORB_SECRET'];
    delete process.env['BTP_PEER_CONNECTOR_A_SECRET'];
    if (sdkA) sdkA.removeAllListeners();
    if (sdkB) sdkB.removeAllListeners();
    channelManager?.stop();
    await new Promise((r) => setTimeout(r, 100));
    console.log('  Cleanup complete');
    console.log('--- Cleanup complete ---\n');
  });

  // ============================================================================
  // Scenarios 1 & 2: First Contact Flow + Caching
  // ============================================================================

  describe('First Contact & Caching', () => {
    const depositAmount = ethers.parseEther('100');
    let channelId: string;

    it('Scenario 1: first contact flow -- unknown peer sends self-describing claim', async () => {
      // Arrange: open channel and deposit on Anvil
      const result = await sdkA.openChannel(CONNECTOR_B_ADDRESS, TOKEN_ADDRESS, 3600, 0n);
      channelId = result.channelId;
      await sdkA.deposit(channelId, TOKEN_ADDRESS, depositAmount);
      console.log(`  Channel opened: ${channelId.slice(0, 18)}...`);

      // Sign a real EIP-712 balance proof
      const nonce = 1;
      const transferredAmount = ethers.parseEther('10');
      const signature = await sdkA.signBalanceProof(
        channelId,
        nonce,
        transferredAmount,
        0n,
        ethers.ZeroHash
      );

      // Clear telemetry
      telemetryEventsB.length = 0;

      // Act: send self-describing claim with all three fields
      const sendResult = await claimSender.sendEVMClaim(
        'connector-b',
        btpClient,
        channelId,
        nonce,
        transferredAmount.toString(),
        '0',
        ethers.ZeroHash,
        signature,
        CONNECTOR_A_ADDRESS,
        chainId,
        tokenNetworkAddress,
        TOKEN_ADDRESS
      );
      expect(sendResult.success).toBe(true);

      // Assert: wait for claim to be received and verified
      const claimReceived = await waitForCondition(() => {
        const row = receiverDb
          .prepare('SELECT verified FROM received_claims WHERE channel_id = ?')
          .get(channelId) as ReceivedClaimRow | undefined;
        return !!row;
      }, 10000);
      expect(claimReceived).toBe(true);

      const row = receiverDb
        .prepare('SELECT verified FROM received_claims WHERE channel_id = ?')
        .get(channelId) as ReceivedClaimRow;
      expect(row.verified).toBe(1);

      // Assert: channel is auto-registered in ChannelManager
      const registeredChannel = channelManager.getChannelById(channelId);
      expect(registeredChannel).not.toBeNull();
      expect(registeredChannel!.peerId).toBe('connector-a');

      // Assert: telemetry
      await waitForCondition(() => {
        return telemetryEventsB.some(
          (e) => e.type === 'CLAIM_RECEIVED' && (e as ClaimReceivedEvent).channelId === channelId
        );
      }, 5000);

      const claimEvent = telemetryEventsB.find(
        (e) => e.type === 'CLAIM_RECEIVED' && (e as ClaimReceivedEvent).channelId === channelId
      ) as ClaimReceivedEvent | undefined;
      expect(claimEvent).toBeDefined();
      expect(claimEvent!.verified).toBe(true);
      expect(claimEvent!.peerId).toBeDefined();
      expect(claimEvent!.blockchain).toBe('evm');

      console.log('  Scenario 1 passed: first contact flow verified');
    });

    it('Scenario 2: caching -- second claim skips RPC', async () => {
      // Arrange: spy on getChannelStateByNetwork
      const spy = jest.spyOn(sdkB, 'getChannelStateByNetwork');

      // Sign second balance proof with incremented nonce
      const nonce = 2;
      const transferredAmount = ethers.parseEther('20');
      const signature = await sdkA.signBalanceProof(
        channelId,
        nonce,
        transferredAmount,
        0n,
        ethers.ZeroHash
      );

      // Act: send second claim
      const sendResult = await claimSender.sendEVMClaim(
        'connector-b',
        btpClient,
        channelId,
        nonce,
        transferredAmount.toString(),
        '0',
        ethers.ZeroHash,
        signature,
        CONNECTOR_A_ADDRESS,
        chainId,
        tokenNetworkAddress,
        TOKEN_ADDRESS
      );
      expect(sendResult.success).toBe(true);

      // Wait for second claim to be received
      const claimReceived = await waitForCondition(() => {
        const rows = receiverDb
          .prepare(
            'SELECT verified FROM received_claims WHERE channel_id = ? ORDER BY received_at DESC LIMIT 1'
          )
          .get(channelId) as ReceivedClaimRow | undefined;
        return !!rows && rows.verified === 1;
      }, 10000);
      expect(claimReceived).toBe(true);

      // Assert: getChannelStateByNetwork was NOT called (channel already cached)
      expect(spy).not.toHaveBeenCalled();

      spy.mockRestore();
      console.log('  Scenario 2 passed: caching verified, no RPC on second claim');
    });
  });

  // ============================================================================
  // Scenario 3: Tampered Fields
  // ============================================================================

  describe('Tampered Fields', () => {
    let tamperedChannelId: string;
    let validSignature: string;

    beforeAll(async () => {
      // Open a fresh channel specifically for tampered-field tests
      const depositAmount = ethers.parseEther('50');
      const result = await sdkA.openChannel(CONNECTOR_B_ADDRESS, TOKEN_ADDRESS, 3600, 0n);
      tamperedChannelId = result.channelId;
      await sdkA.deposit(tamperedChannelId, TOKEN_ADDRESS, depositAmount);

      // Sign a valid balance proof
      validSignature = await sdkA.signBalanceProof(
        tamperedChannelId,
        1,
        ethers.parseEther('5'),
        0n,
        ethers.ZeroHash
      );

      console.log(`  Tampered test channel opened: ${tamperedChannelId.slice(0, 18)}...`);
    });

    it('Scenario 3a: tampered chainId fails EIP-712 signature verification', async () => {
      // Clear telemetry
      telemetryEventsB.length = 0;

      // Act: send claim with tampered chainId (99999 instead of real chain ID)
      const sendResult = await claimSender.sendEVMClaim(
        'connector-b',
        btpClient,
        tamperedChannelId,
        1,
        ethers.parseEther('5').toString(),
        '0',
        ethers.ZeroHash,
        validSignature,
        CONNECTOR_A_ADDRESS,
        99999, // tampered chainId
        tokenNetworkAddress,
        TOKEN_ADDRESS
      );
      expect(sendResult.success).toBe(true);

      // Wait for claim to be received (verified=0)
      const claimReceived = await waitForCondition(() => {
        const row = receiverDb
          .prepare(
            'SELECT verified FROM received_claims WHERE channel_id = ? ORDER BY received_at DESC LIMIT 1'
          )
          .get(tamperedChannelId) as ReceivedClaimRow | undefined;
        return !!row;
      }, 10000);
      expect(claimReceived).toBe(true);

      // Assert: claim stored with verified=0
      const row = receiverDb
        .prepare(
          'SELECT verified FROM received_claims WHERE channel_id = ? ORDER BY received_at DESC LIMIT 1'
        )
        .get(tamperedChannelId) as ReceivedClaimRow;
      expect(row.verified).toBe(0);

      // Assert: telemetry emitted with verified=false
      await waitForCondition(() => {
        return telemetryEventsB.some(
          (e) =>
            e.type === 'CLAIM_RECEIVED' &&
            (e as ClaimReceivedEvent).channelId === tamperedChannelId &&
            (e as ClaimReceivedEvent).verified === false
        );
      }, 5000);

      const claimEvent = telemetryEventsB.find(
        (e) =>
          e.type === 'CLAIM_RECEIVED' &&
          (e as ClaimReceivedEvent).channelId === tamperedChannelId &&
          (e as ClaimReceivedEvent).verified === false
      );
      expect(claimEvent).toBeDefined();

      console.log('  Scenario 3a passed: tampered chainId rejected');
    });

    it('Scenario 3b: tampered tokenNetworkAddress fails on-chain verification', async () => {
      // Clear telemetry
      telemetryEventsB.length = 0;

      // We need a fresh channel for this scenario since tamperedChannelId may now
      // be registered from the previous test (if verification went far enough).
      // Open yet another channel.
      const depositAmount = ethers.parseEther('50');
      const result = await sdkA.openChannel(CONNECTOR_B_ADDRESS, TOKEN_ADDRESS, 3600, 0n);
      const freshChannelId = result.channelId;
      await sdkA.deposit(freshChannelId, TOKEN_ADDRESS, depositAmount);

      const freshSignature = await sdkA.signBalanceProof(
        freshChannelId,
        1,
        ethers.parseEther('5'),
        0n,
        ethers.ZeroHash
      );

      // Act: send claim with tampered tokenNetworkAddress
      const sendResult = await claimSender.sendEVMClaim(
        'connector-b',
        btpClient,
        freshChannelId,
        1,
        ethers.parseEther('5').toString(),
        '0',
        ethers.ZeroHash,
        freshSignature,
        CONNECTOR_A_ADDRESS,
        chainId,
        '0x0000000000000000000000000000000000000001', // tampered tokenNetworkAddress
        TOKEN_ADDRESS
      );
      expect(sendResult.success).toBe(true);

      // Wait for claim to be received
      const claimReceived = await waitForCondition(() => {
        const row = receiverDb
          .prepare(
            'SELECT verified FROM received_claims WHERE channel_id = ? ORDER BY received_at DESC LIMIT 1'
          )
          .get(freshChannelId) as ReceivedClaimRow | undefined;
        return !!row;
      }, 10000);
      expect(claimReceived).toBe(true);

      // Assert: claim stored with verified=0
      const row = receiverDb
        .prepare(
          'SELECT verified FROM received_claims WHERE channel_id = ? ORDER BY received_at DESC LIMIT 1'
        )
        .get(freshChannelId) as ReceivedClaimRow;
      expect(row.verified).toBe(0);

      // Assert: telemetry emitted with verified=false
      await waitForCondition(() => {
        return telemetryEventsB.some(
          (e) =>
            e.type === 'CLAIM_RECEIVED' &&
            (e as ClaimReceivedEvent).channelId === freshChannelId &&
            (e as ClaimReceivedEvent).verified === false
        );
      }, 5000);

      const claimEvent = telemetryEventsB.find(
        (e) =>
          e.type === 'CLAIM_RECEIVED' &&
          (e as ClaimReceivedEvent).channelId === freshChannelId &&
          (e as ClaimReceivedEvent).verified === false
      );
      expect(claimEvent).toBeDefined();

      console.log('  Scenario 3b passed: tampered tokenNetworkAddress rejected');
    });
  });

  // ============================================================================
  // Scenarios 4 & 5: Backward Compatibility + Mixed Mode
  // ============================================================================

  describe('Backward Compatibility & Mixed Mode', () => {
    let preRegisteredChannelId: string;

    it('Scenario 4: pre-registered channel works without self-describing fields', async () => {
      // Arrange: open channel and pre-register in ChannelManager
      const depositAmount = ethers.parseEther('100');
      const result = await sdkA.openChannel(CONNECTOR_B_ADDRESS, TOKEN_ADDRESS, 3600, 0n);
      preRegisteredChannelId = result.channelId;
      await sdkA.deposit(preRegisteredChannelId, TOKEN_ADDRESS, depositAmount);

      // Pre-register channel via registerExternalChannel (simulating Admin API)
      channelManager.registerExternalChannel({
        channelId: preRegisteredChannelId,
        peerId: 'connector-a',
        tokenAddress: TOKEN_ADDRESS,
        tokenNetworkAddress: tokenNetworkAddress,
        chainId: chainId,
        status: 'open',
      });

      // Warm up sdkB's tokenNetworkCache for known-channel verifyBalanceProof path
      await sdkB.getChannelState(preRegisteredChannelId, TOKEN_ADDRESS);

      // Sign balance proof
      const nonce = 1;
      const transferredAmount = ethers.parseEther('10');
      const signature = await sdkA.signBalanceProof(
        preRegisteredChannelId,
        nonce,
        transferredAmount,
        0n,
        ethers.ZeroHash
      );

      // Spy on getChannelStateByNetwork to ensure it's NOT called
      const spy = jest.spyOn(sdkB, 'getChannelStateByNetwork');

      // Act: send claim WITHOUT self-describing fields
      const sendResult = await claimSender.sendEVMClaim(
        'connector-b',
        btpClient,
        preRegisteredChannelId,
        nonce,
        transferredAmount.toString(),
        '0',
        ethers.ZeroHash,
        signature,
        CONNECTOR_A_ADDRESS
        // no chainId, no tokenNetworkAddress, no tokenAddress
      );
      expect(sendResult.success).toBe(true);

      // Wait for claim to be received and verified
      const claimReceived = await waitForCondition(() => {
        const row = receiverDb
          .prepare(
            'SELECT verified FROM received_claims WHERE channel_id = ? ORDER BY received_at DESC LIMIT 1'
          )
          .get(preRegisteredChannelId) as ReceivedClaimRow | undefined;
        return !!row && row.verified === 1;
      }, 10000);
      expect(claimReceived).toBe(true);

      // Assert: verified=1
      const row = receiverDb
        .prepare(
          'SELECT verified FROM received_claims WHERE channel_id = ? ORDER BY received_at DESC LIMIT 1'
        )
        .get(preRegisteredChannelId) as ReceivedClaimRow;
      expect(row.verified).toBe(1);

      // Assert: getChannelStateByNetwork NOT called (channel already cached)
      expect(spy).not.toHaveBeenCalled();

      spy.mockRestore();
      console.log('  Scenario 4 passed: backward compatibility with pre-registered channel');
    });

    it('Scenario 5: pre-registered channel works with self-describing fields (mixed mode)', async () => {
      // Spy on getChannelStateByNetwork to ensure it's NOT called
      const spy = jest.spyOn(sdkB, 'getChannelStateByNetwork');

      // Sign balance proof with incremented nonce
      const nonce = 2;
      const transferredAmount = ethers.parseEther('20');
      const signature = await sdkA.signBalanceProof(
        preRegisteredChannelId,
        nonce,
        transferredAmount,
        0n,
        ethers.ZeroHash
      );

      // Act: send claim WITH self-describing fields (informational, channel already cached)
      const sendResult = await claimSender.sendEVMClaim(
        'connector-b',
        btpClient,
        preRegisteredChannelId,
        nonce,
        transferredAmount.toString(),
        '0',
        ethers.ZeroHash,
        signature,
        CONNECTOR_A_ADDRESS,
        chainId,
        tokenNetworkAddress,
        TOKEN_ADDRESS
      );
      expect(sendResult.success).toBe(true);

      // Wait for claim to be received and verified
      const claimReceived = await waitForCondition(() => {
        const rows = receiverDb
          .prepare('SELECT * FROM received_claims WHERE channel_id = ? ORDER BY received_at DESC')
          .all(preRegisteredChannelId) as ReceivedClaimRow[];
        // Must have at least 2 claims (from scenarios 4 and 5)
        return rows.length >= 2 && rows[0]!.verified === 1;
      }, 10000);
      expect(claimReceived).toBe(true);

      // Assert: getChannelStateByNetwork NOT called
      expect(spy).not.toHaveBeenCalled();

      spy.mockRestore();
      console.log('  Scenario 5 passed: mixed mode with pre-registered channel');
    });
  });
});
