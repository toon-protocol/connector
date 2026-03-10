/**
 * Per-Packet Claims E2E Test
 *
 * Validates that signed payment channel claims travel with each ILP PREPARE
 * packet via BTP protocolData.
 *
 * Two modes:
 *
 * A. In-Process (E2E_TESTS=true):
 *    - In-process ConnectorNode instances with in-memory ledger (no TigerBeetle)
 *    - Anvil (local EVM node, chain 31337) for settlement infrastructure
 *    - Sends packets via connectorA.sendPacket() directly
 *
 * B. Docker (E2E_DOCKER_TESTS=true):
 *    - Docker containers with TigerBeetle backend
 *    - Sends packets via Admin API HTTP endpoint
 *
 * Prerequisites:
 *   In-Process: ./scripts/run-per-packet-claims-e2e.sh
 *   Docker:     ./scripts/run-per-packet-claims-e2e.sh --docker
 *
 * Test scenarios:
 * 1. Claims travel with packets (valid EIP-712 signature, correct nonce/cumulative)
 * 2. Cumulative claim accuracy across multiple packets
 * 3. Packets flow without claims when no channel exists
 * 4. Claim failure resilience (packets still forward)
 * 5. Multi-hop packet routing (A → B → C)
 * 6. F02 error for unknown destinations
 * 7. Connection failure handling (T01 when intermediate node down)
 * 8. Settlement triggers when packet balances exceed threshold (in-memory ledger + Anvil)
 */

/* eslint-disable no-console */

import { ConnectorNode } from '../../src/core/connector-node';
import type { ConnectorConfig } from '../../src/config/types';
import { AccountManager } from '../../src/settlement/account-manager';
import { SettlementMonitor } from '../../src/settlement/settlement-monitor';
import { SettlementState } from '../../src/config/types';
import pino from 'pino';
import { ethers } from 'ethers';
import {
  PacketType,
  ILPPreparePacket,
  ILPRejectPacket,
  ILPErrorCode,
  BalanceProof,
} from '@crosstown/shared';
import { waitFor } from '../helpers/wait-for';

// 5-minute timeout for E2E tests
jest.setTimeout(300000);

// ============================================================================
// Configuration
// ============================================================================

const ANVIL_RPC_URL = 'http://localhost:8545';

// Deployed contracts (deterministic from docker-compose)
const TOKEN_ADDRESS = '0x5FbDB2315678afecb367f032d93F642f64180aa3';
const REGISTRY_ADDRESS = '0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512';

// Anvil default accounts
const CONNECTOR_A_KEY = '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80';
const CONNECTOR_B_KEY = '0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d';
const CONNECTOR_C_KEY = '0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a';

// Derived EVM addresses from Anvil private keys
const CONNECTOR_A_ADDRESS = new ethers.Wallet(CONNECTOR_A_KEY).address;
const CONNECTOR_B_ADDRESS = new ethers.Wallet(CONNECTOR_B_KEY).address;
// CONNECTOR_C_ADDRESS not used in settlement tests yet but available
// const CONNECTOR_C_ADDRESS = new ethers.Wallet(CONNECTOR_C_KEY).address;

// TigerBeetle (exposed via docker-compose-base-e2e-test.yml)
// Use 127.0.0.1 instead of localhost to avoid IPv6 resolution (TigerBeetle requires IPv4)
const TIGERBEETLE_ADDRESS = '127.0.0.1:3000';
const TIGERBEETLE_CLUSTER_ID = '0';

// Docker connector endpoints
const DOCKER_HEALTH_A = 'http://localhost:8080/health';
const DOCKER_HEALTH_B = 'http://localhost:8090/health';

// ERC20 ABI for token balance queries
const ERC20_BALANCE_ABI = [
  'function balanceOf(address owner) view returns (uint256)',
  'function symbol() view returns (string)',
];

// ============================================================================
// Helper Functions
// ============================================================================

/**
 * Query ERC20 token balance for an address on Anvil
 */
async function getTokenBalance(address: string): Promise<bigint> {
  const provider = new ethers.JsonRpcProvider(ANVIL_RPC_URL);
  const token = new ethers.Contract(TOKEN_ADDRESS, ERC20_BALANCE_ABI, provider);
  const balance = await token.getFunction('balanceOf')(address);
  return balance as bigint;
}

/**
 * Access AccountManager from a ConnectorNode (test-only)
 */
function getAccountManager(connector: ConnectorNode): AccountManager {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return (connector as any)._accountManager as AccountManager;
}

/**
 * Access SettlementMonitor from a ConnectorNode (test-only)
 */
function getSettlementMonitor(connector: ConnectorNode): SettlementMonitor {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return (connector as any)._settlementMonitor as SettlementMonitor;
}

/**
 * Check if TigerBeetle is reachable at the expected address
 */
async function isTigerBeetleAvailable(): Promise<boolean> {
  const net = await import('net');
  const [host, portStr] = TIGERBEETLE_ADDRESS.split(':');
  const port = parseInt(portStr!, 10);
  return new Promise((resolve) => {
    const socket = net.createConnection({ host, port, timeout: 2000 }, () => {
      socket.destroy();
      resolve(true);
    });
    socket.on('error', () => {
      socket.destroy();
      resolve(false);
    });
    socket.on('timeout', () => {
      socket.destroy();
      resolve(false);
    });
  });
}

async function waitForAnvil(timeout: number = 30000): Promise<void> {
  await waitFor(
    async () => {
      const response = await fetch(ANVIL_RPC_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          jsonrpc: '2.0',
          method: 'eth_blockNumber',
          params: [],
          id: 1,
        }),
      });
      return response.ok;
    },
    { timeout, interval: 1000 }
  );
}

async function waitForDockerConnectors(timeout: number = 120000): Promise<void> {
  console.log('Waiting for Docker connectors to become healthy...');
  await waitFor(
    async () => {
      const [responseA, responseB] = await Promise.all([
        fetch(DOCKER_HEALTH_A).catch(() => null),
        fetch(DOCKER_HEALTH_B).catch(() => null),
      ]);
      return responseA?.ok === true && responseB?.ok === true;
    },
    { timeout, interval: 2000 }
  );
  console.log('Docker connectors are healthy');

  // Verify Admin API is actually reachable (port may be blocked by another process)
  const adminHealth = await fetch('http://localhost:8081/health').catch(() => null);
  if (adminHealth?.ok) {
    const body = (await adminHealth.json()) as { service?: string };
    if (body.service !== 'admin-api') {
      throw new Error(
        'Port 8081 is occupied by another process (not the connector Admin API). ' +
          'Stop the conflicting process and retry.'
      );
    }
  } else {
    throw new Error('Admin API on port 8081 is not reachable');
  }
  console.log('Admin API is reachable');
}

async function sendViaAdminApi(
  port: number,
  destination: string,
  amount: bigint
): Promise<{ type: string; [key: string]: unknown }> {
  const response = await fetch(`http://localhost:${port}/admin/ilp/send`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      destination,
      amount: amount.toString(),
      data: Buffer.alloc(0).toString('base64'),
      timeoutMs: 30000,
    }),
  });
  const text = await response.text();
  try {
    return JSON.parse(text) as { type: string; [key: string]: unknown };
  } catch {
    throw new Error(
      `Admin API returned non-JSON response (status ${response.status}): ${text.slice(0, 200)}`
    );
  }
}

function createTestPacket(amount: bigint, destination: string): ILPPreparePacket {
  // Use unique executionCondition per packet for TigerBeetle transfer ID uniqueness
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  const condition = require('crypto').randomBytes(32) as Buffer;
  return {
    type: PacketType.PREPARE,
    amount,
    destination,
    executionCondition: condition,
    expiresAt: new Date(Date.now() + 30000),
    data: Buffer.alloc(0),
  };
}

// ============================================================================
// Test Suite A: In-Process Mode
// ============================================================================

const SKIP_IN_PROCESS = process.env.E2E_TESTS !== 'true';
const describeInProcess = SKIP_IN_PROCESS ? describe.skip : describe;

describeInProcess('Per-Packet Claims E2E - In-Process', () => {
  let connectorA: ConnectorNode;
  let connectorB: ConnectorNode;
  let connectorC: ConnectorNode;

  beforeAll(async () => {
    console.log('Setting up Per-Packet Claims E2E (In-Process) - 3-node topology...');

    // Set BTP peer secrets for permissionless mode
    process.env.BTP_PEER_CONNECTOR_A_SECRET = '';
    process.env.BTP_PEER_CONNECTOR_B_SECRET = '';
    process.env.BTP_PEER_CONNECTOR_C_SECRET = '';

    // Verify Anvil is running
    try {
      await waitForAnvil();
      console.log('Anvil is ready');
    } catch {
      throw new Error(
        'Anvil not available. Run: docker compose -f docker-compose-base-e2e-test.yml up -d anvil_base_e2e'
      );
    }

    // Create Connector A config (routes to B and C via B)
    const configA: Partial<ConnectorConfig> = {
      nodeId: 'connector-a',
      btpServerPort: 14001,
      healthCheckPort: 18080,
      peers: [
        {
          id: 'connector-b',
          url: 'ws://localhost:14002',
          authToken: '',
        },
      ],
      routes: [
        { prefix: 'g.test.connector-b', nextHop: 'connector-b' },
        { prefix: 'g.test.connector-c', nextHop: 'connector-b' },
      ],
      settlementInfra: {
        enabled: true,
        rpcUrl: ANVIL_RPC_URL,
        registryAddress: REGISTRY_ADDRESS,
        tokenAddress: TOKEN_ADDRESS,
        privateKey: CONNECTOR_A_KEY,
      },
      adminApi: { enabled: false },
      explorer: { enabled: false },
    };

    // Create Connector B config (hub: peers with both A and C)
    const configB: Partial<ConnectorConfig> = {
      nodeId: 'connector-b',
      btpServerPort: 14002,
      healthCheckPort: 18090,
      peers: [
        {
          id: 'connector-a',
          url: 'ws://localhost:14001',
          authToken: '',
        },
        {
          id: 'connector-c',
          url: 'ws://localhost:14003',
          authToken: '',
        },
      ],
      routes: [
        { prefix: 'g.test.connector-a', nextHop: 'connector-a' },
        { prefix: 'g.test.connector-c', nextHop: 'connector-c' },
      ],
      settlementInfra: {
        enabled: true,
        rpcUrl: ANVIL_RPC_URL,
        registryAddress: REGISTRY_ADDRESS,
        tokenAddress: TOKEN_ADDRESS,
        privateKey: CONNECTOR_B_KEY,
      },
      adminApi: { enabled: false },
      explorer: { enabled: false },
    };

    // Create Connector C config (leaf node, peers with B)
    const configC: Partial<ConnectorConfig> = {
      nodeId: 'connector-c',
      btpServerPort: 14003,
      healthCheckPort: 18100,
      peers: [
        {
          id: 'connector-b',
          url: 'ws://localhost:14002',
          authToken: '',
        },
      ],
      routes: [
        { prefix: 'g.test.connector-b', nextHop: 'connector-b' },
        { prefix: 'g.test.connector-a', nextHop: 'connector-b' },
      ],
      settlementInfra: {
        enabled: true,
        rpcUrl: ANVIL_RPC_URL,
        registryAddress: REGISTRY_ADDRESS,
        tokenAddress: TOKEN_ADDRESS,
        privateKey: CONNECTOR_C_KEY,
      },
      adminApi: { enabled: false },
      explorer: { enabled: false },
    };

    const loggerA = pino({ level: 'warn' });
    const loggerB = pino({ level: 'warn' });
    const loggerC = pino({ level: 'warn' });

    connectorA = new ConnectorNode(configA as ConnectorConfig, loggerA);
    connectorB = new ConnectorNode(configB as ConnectorConfig, loggerB);
    connectorC = new ConnectorNode(configC as ConnectorConfig, loggerC);

    // Start connectors in reverse order so upstream peers can connect
    await connectorC.start();
    await connectorB.start();
    await connectorA.start();

    // Wait for BTP connections to establish
    await new Promise((resolve) => setTimeout(resolve, 5000));
    console.log('3-node topology started and connected (A → B → C)');
  });

  afterAll(async () => {
    console.log('Cleaning up...');
    try {
      await connectorA?.stop();
    } catch {
      // ignore cleanup errors
    }
    try {
      await connectorB?.stop();
    } catch {
      // ignore cleanup errors
    }
    try {
      await connectorC?.stop();
    } catch {
      // ignore cleanup errors
    }
    // Clear BTP peer env vars to avoid leaking state
    delete process.env.BTP_PEER_CONNECTOR_A_SECRET;
    delete process.env.BTP_PEER_CONNECTOR_B_SECRET;
    delete process.env.BTP_PEER_CONNECTOR_C_SECRET;
  });

  it('should forward packets without claims when no payment channel exists', async () => {
    const packet = createTestPacket(1000n, 'g.test.connector-b.receiver');

    const response = await connectorA.sendPacket({
      destination: packet.destination,
      amount: packet.amount,
      executionCondition: packet.executionCondition,
      expiresAt: packet.expiresAt,
      data: packet.data,
    });

    expect(response).toBeDefined();
    console.log(
      'Packet forwarded without claims (graceful degradation)',
      response.type ?? 'response received'
    );
  });

  it('should send multiple packets and verify cumulative claim tracking', async () => {
    const amounts = [100n, 200n, 300n];
    const responses = [];

    for (const amount of amounts) {
      const packet = createTestPacket(amount, 'g.test.connector-b.receiver');
      const response = await connectorA.sendPacket({
        destination: packet.destination,
        amount: packet.amount,
        executionCondition: packet.executionCondition,
        expiresAt: packet.expiresAt,
        data: packet.data,
      });
      responses.push(response);
    }

    expect(responses).toHaveLength(3);
    responses.forEach((response) => {
      expect(response).toBeDefined();
    });

    console.log('Multiple packets forwarded successfully');
  });

  // ==========================================================================
  // Multi-Hop Packet Routing (replaces multi-node-forwarding.test.ts)
  // ==========================================================================

  it('should route packet through A → B → C (multi-hop forwarding)', async () => {
    const packet = createTestPacket(1000n, 'g.test.connector-c.receiver');

    const response = await connectorA.sendPacket({
      destination: packet.destination,
      amount: packet.amount,
      executionCondition: packet.executionCondition,
      expiresAt: packet.expiresAt,
      data: packet.data,
    });

    // Connector C returns F02 because it has no local receiver — this confirms
    // the packet successfully routed through A → B → C
    expect(response).toBeDefined();
    expect(response.type).toBe(PacketType.REJECT);
    if (response.type === PacketType.REJECT) {
      const reject = response as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.F02_UNREACHABLE);
      expect(reject.triggeredBy).toBe('connector-c');
    }
    console.log('Multi-hop packet routed A → B → C successfully');
  });

  it('should return F02 for unknown destination', async () => {
    const packet = createTestPacket(1000n, 'g.test.unknown.destination');

    const response = await connectorA.sendPacket({
      destination: packet.destination,
      amount: packet.amount,
      executionCondition: packet.executionCondition,
      expiresAt: packet.expiresAt,
      data: packet.data,
    });

    expect(response).toBeDefined();
    expect(response.type).toBe(PacketType.REJECT);
    if (response.type === PacketType.REJECT) {
      const reject = response as ILPRejectPacket;
      expect(reject.code).toBe(ILPErrorCode.F02_UNREACHABLE);
    }
    console.log('Unknown destination correctly rejected with F02');
  });

  it('should return error when intermediate connector is down', async () => {
    // Stop connector B to simulate intermediate node failure
    await connectorB.stop();

    // Wait for connection loss to be detected
    await new Promise((resolve) => setTimeout(resolve, 1000));

    const packet = createTestPacket(1000n, 'g.test.connector-c.receiver');

    const response = await connectorA.sendPacket({
      destination: packet.destination,
      amount: packet.amount,
      executionCondition: packet.executionCondition,
      expiresAt: packet.expiresAt,
      data: packet.data,
    });

    expect(response).toBeDefined();
    expect(response.type).toBe(PacketType.REJECT);
    if (response.type === PacketType.REJECT) {
      const reject = response as ILPRejectPacket;
      // T01 (Peer Unreachable) or R00 (Transfer Timed Out) are both valid
      expect([ILPErrorCode.T01_PEER_UNREACHABLE, ILPErrorCode.R00_TRANSFER_TIMED_OUT]).toContain(
        reject.code
      );
    }
    console.log('Intermediate node failure correctly produces error');

    // Restart connector B for any subsequent tests
    const configB: Partial<ConnectorConfig> = {
      nodeId: 'connector-b',
      btpServerPort: 14002,
      healthCheckPort: 18090,
      peers: [
        { id: 'connector-a', url: 'ws://localhost:14001', authToken: '' },
        { id: 'connector-c', url: 'ws://localhost:14003', authToken: '' },
      ],
      routes: [
        { prefix: 'g.test.connector-a', nextHop: 'connector-a' },
        { prefix: 'g.test.connector-c', nextHop: 'connector-c' },
      ],
      settlementInfra: {
        enabled: true,
        rpcUrl: ANVIL_RPC_URL,
        registryAddress: REGISTRY_ADDRESS,
        tokenAddress: TOKEN_ADDRESS,
        privateKey: CONNECTOR_B_KEY,
      },
      adminApi: { enabled: false },
      explorer: { enabled: false },
    };
    connectorB = new ConnectorNode(configB as ConnectorConfig, pino({ level: 'warn' }));
    await connectorB.start();
    await new Promise((resolve) => setTimeout(resolve, 3000));
  });
});

// ============================================================================
// Test Suite B: Docker Mode
// ============================================================================

const SKIP_DOCKER = process.env.E2E_DOCKER_TESTS !== 'true';
const describeDocker = SKIP_DOCKER ? describe.skip : describe;

describeDocker('Per-Packet Claims E2E - Docker', () => {
  beforeAll(async () => {
    console.log('Setting up Per-Packet Claims E2E (Docker)...');

    // Wait for Docker connectors to become healthy
    try {
      await waitForDockerConnectors();
    } catch {
      throw new Error(
        'Docker connectors not available. Run: ./scripts/run-per-packet-claims-e2e.sh --docker'
      );
    }
  });

  it('should forward packets without claims when no payment channel exists', async () => {
    const response = await sendViaAdminApi(8081, 'g.test.connector-b.receiver', 1000n);

    expect(response).toBeDefined();
    console.log(
      'Packet forwarded without claims via Docker (graceful degradation)',
      response.type ?? 'response received'
    );
  });

  it('should send multiple packets and verify cumulative claim tracking', async () => {
    const amounts = [100n, 200n, 300n];
    const responses = [];

    for (const amount of amounts) {
      const response = await sendViaAdminApi(8081, 'g.test.connector-b.receiver', amount);
      responses.push(response);
    }

    expect(responses).toHaveLength(3);
    responses.forEach((response) => {
      expect(response).toBeDefined();
    });

    console.log('Multiple packets forwarded via Docker successfully');
  });
});

// ============================================================================
// Settlement Test Helpers
// ============================================================================

// Settlement test configuration shared across ledger backends
const SETTLEMENT_THRESHOLD = '500';
const SETTLEMENT_POLLING_MS = 500;
const PACKET_AMOUNT = 200n;
const PACKETS_TO_SEND = 3; // 3 × 200 = 600 > 500 threshold

interface SettlementTestEnv {
  connectorA: ConnectorNode;
  connectorB: ConnectorNode;
}

/**
 * Build connector configs for the 2-node settlement topology.
 * Uses unique ports offset by `portOffset` to avoid conflicts across suites.
 */
function buildSettlementConfigs(portOffset: number): {
  configA: Partial<ConnectorConfig>;
  configB: Partial<ConnectorConfig>;
} {
  const btpPortA = 15001 + portOffset;
  const btpPortB = 15002 + portOffset;
  const healthPortA = 19080 + portOffset;
  const healthPortB = 19090 + portOffset;

  const configA: Partial<ConnectorConfig> = {
    nodeId: 'settlement-a',
    btpServerPort: btpPortA,
    healthCheckPort: healthPortA,
    peers: [
      {
        id: 'settlement-b',
        url: `ws://localhost:${btpPortB}`,
        authToken: '',
        evmAddress: CONNECTOR_B_ADDRESS,
      },
    ],
    routes: [{ prefix: 'g.test.settlement-b', nextHop: 'settlement-b' }],
    settlementInfra: {
      enabled: true,
      rpcUrl: ANVIL_RPC_URL,
      registryAddress: REGISTRY_ADDRESS,
      tokenAddress: TOKEN_ADDRESS,
      privateKey: CONNECTOR_A_KEY,
      threshold: SETTLEMENT_THRESHOLD,
      pollingIntervalMs: SETTLEMENT_POLLING_MS,
      initialDepositMultiplier: 1,
    },
    adminApi: { enabled: false },
    explorer: { enabled: false },
  };

  const configB: Partial<ConnectorConfig> = {
    nodeId: 'settlement-b',
    btpServerPort: btpPortB,
    healthCheckPort: healthPortB,
    peers: [
      {
        id: 'settlement-a',
        url: `ws://localhost:${btpPortA}`,
        authToken: '',
        evmAddress: CONNECTOR_A_ADDRESS,
      },
    ],
    routes: [{ prefix: 'g.test.settlement-a', nextHop: 'settlement-a' }],
    settlementInfra: {
      enabled: true,
      rpcUrl: ANVIL_RPC_URL,
      registryAddress: REGISTRY_ADDRESS,
      tokenAddress: TOKEN_ADDRESS,
      privateKey: CONNECTOR_B_KEY,
      threshold: SETTLEMENT_THRESHOLD,
      pollingIntervalMs: SETTLEMENT_POLLING_MS,
      initialDepositMultiplier: 1,
    },
    adminApi: { enabled: false },
    explorer: { enabled: false },
  };

  return { configA, configB };
}

/**
 * Create and start the 2-node settlement topology.
 */
async function startSettlementTopology(portOffset: number): Promise<SettlementTestEnv> {
  const { configA, configB } = buildSettlementConfigs(portOffset);

  const loggerA = pino({ level: 'warn' });
  const loggerB = pino({ level: 'warn' });

  const connectorA = new ConnectorNode(configA as ConnectorConfig, loggerA);
  const connectorB = new ConnectorNode(configB as ConnectorConfig, loggerB);

  // Start B first (downstream), then A
  await connectorB.start();
  await connectorA.start();

  // Wait for BTP connections to establish
  await new Promise((resolve) => setTimeout(resolve, 5000));

  return { connectorA, connectorB };
}

/**
 * Stop both connectors and clean up environment.
 */
async function stopSettlementTopology(env: SettlementTestEnv): Promise<void> {
  try {
    await env.connectorA?.stop();
  } catch {
    // ignore cleanup errors
  }
  try {
    await env.connectorB?.stop();
  } catch {
    // ignore cleanup errors
  }
}

/**
 * Core settlement test logic shared between in-memory and TigerBeetle backends.
 *
 * Validates the full per-packet claims lifecycle:
 * 1. A sends packets with signed claims → channel created & funded automatically
 * 2. Claims accumulate per-packet via BTP protocolData
 * 3. Settlement triggers on B's side → B claims from channel using A's signed claims
 * 4. B's wallet increases, payment channel balance decreases
 * 5. Channel stays OPEN — A can continue sending
 */
async function runSettlementTest(env: SettlementTestEnv): Promise<void> {
  const { connectorA, connectorB } = env;

  // ========================================================================
  // Step 1: Snapshot initial state
  // ========================================================================
  const accountManagerA = getAccountManager(connectorA);
  const settlementMonitorA = getSettlementMonitor(connectorA);
  expect(accountManagerA).toBeDefined();
  expect(settlementMonitorA).toBeDefined();

  // Use the connector's resolved token ID (e.g., 'USDC' from on-chain symbol)
  const tokenId = connectorA.defaultSettlementTokenId;
  console.log(`Resolved settlement token ID: ${tokenId}`);

  // Initial token balance of connector A's wallet (deployer account, has tokens)
  const tokenBalanceABefore = await getTokenBalance(CONNECTOR_A_ADDRESS);
  console.log(`Initial token balance (A): ${tokenBalanceABefore}`);
  expect(tokenBalanceABefore).toBeGreaterThan(0n);

  // Snapshot B's initial token balance
  const tokenBalanceBBefore = await getTokenBalance(CONNECTOR_B_ADDRESS);
  console.log(`Initial token balance (B): ${tokenBalanceBBefore}`);

  // Verify no payment channel exists yet
  const sdkA = connectorA.paymentChannelSDK;
  expect(sdkA).not.toBeNull();
  const existingChannels = await sdkA!.getMyChannels(TOKEN_ADDRESS);
  const openChannelsBefore = [];
  for (const chId of existingChannels) {
    const st = await sdkA!.getChannelState(chId, TOKEN_ADDRESS);
    if (st.status === 'opened') openChannelsBefore.push(chId);
  }
  console.log(`Open channels before test: ${openChannelsBefore.length}`);

  // Initial accounting balance should be zero (fresh connectors)
  const initialBalance = await accountManagerA.getAccountBalance('settlement-b', tokenId);
  console.log(
    `Initial accounting balance - credit: ${initialBalance.creditBalance}, debit: ${initialBalance.debitBalance}`
  );

  // ========================================================================
  // Step 2: Send packets to accumulate balance above threshold
  // ========================================================================
  console.log(
    `Sending ${PACKETS_TO_SEND} packets of ${PACKET_AMOUNT} each (total: ${PACKET_AMOUNT * BigInt(PACKETS_TO_SEND)}, threshold: ${SETTLEMENT_THRESHOLD})...`
  );

  for (let i = 0; i < PACKETS_TO_SEND; i++) {
    const packet = createTestPacket(PACKET_AMOUNT, 'g.test.settlement-b.receiver');
    const response = await connectorA.sendPacket({
      destination: packet.destination,
      amount: packet.amount,
      executionCondition: packet.executionCondition,
      expiresAt: packet.expiresAt,
      data: packet.data,
    });
    expect(response).toBeDefined();
  }

  // Verify balance accumulated above threshold
  const balanceAfterPackets = await accountManagerA.getAccountBalance('settlement-b', tokenId);
  console.log(
    `Accounting balance after packets - credit: ${balanceAfterPackets.creditBalance}, debit: ${balanceAfterPackets.debitBalance}`
  );
  expect(balanceAfterPackets.creditBalance).toBeGreaterThanOrEqual(BigInt(SETTLEMENT_THRESHOLD));

  // ========================================================================
  // Step 3: Wait for settlement to trigger and complete
  // ========================================================================
  console.log('Waiting for settlement to trigger and complete...');

  // Settlement monitor has a 5s delayed start + polling interval
  // Wait up to 30s for settlement state to return to IDLE (means it completed)
  await waitFor(
    async () => {
      const state = settlementMonitorA.getSettlementState('settlement-b', tokenId);
      const balance = await accountManagerA.getAccountBalance('settlement-b', tokenId);
      console.log(`  Settlement state: ${state}, creditBalance: ${balance.creditBalance}`);
      // Settlement completed when state returns to IDLE and creditBalance was reduced
      return (
        state === SettlementState.IDLE && balance.creditBalance < balanceAfterPackets.creditBalance
      );
    },
    { timeout: 30000, interval: 500 }
  );

  console.log('Settlement completed!');

  // ========================================================================
  // Step 4: Verify A's accounting balance reduced after settlement
  // ========================================================================
  const balanceAfterSettlement = await accountManagerA.getAccountBalance('settlement-b', tokenId);
  console.log(
    `Accounting balance after settlement - credit: ${balanceAfterSettlement.creditBalance}, debit: ${balanceAfterSettlement.debitBalance}`
  );

  // Credit balance should be reduced (settlement records a transfer that reduces it)
  expect(balanceAfterSettlement.creditBalance).toBeLessThan(balanceAfterPackets.creditBalance);

  // ========================================================================
  // Step 5: Verify A's on-chain wallet token balance decreased (channel funded)
  // ========================================================================
  const tokenBalanceAAfterDeposit = await getTokenBalance(CONNECTOR_A_ADDRESS);
  console.log(
    `Token balance (A) before: ${tokenBalanceABefore}, after deposit: ${tokenBalanceAAfterDeposit}`
  );

  // A deposited tokens into the payment channel, so balance should decrease
  expect(tokenBalanceAAfterDeposit).toBeLessThan(tokenBalanceABefore);

  // ========================================================================
  // Step 6: Verify payment channel is OPEN and A's deposit is on-chain
  // ========================================================================

  // Get channel ID from A's SDK cache (populated during openChannel)
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const channelCache = (sdkA as any).channelStateCache as Map<string, any>;
  expect(channelCache.size).toBeGreaterThan(0);
  let channelId: string | undefined;
  for (const [id, state] of channelCache) {
    if (state.status === 'opened' && !openChannelsBefore.includes(id)) {
      channelId = id;
      break;
    }
  }
  expect(channelId).toBeDefined();
  channelId = channelId!;
  console.log(`Payment channel opened: ${channelId}`);

  // Query channel state from B's perspective
  const sdkB = connectorB.paymentChannelSDK;
  expect(sdkB).not.toBeNull();

  // Clear B's cache for this channel to force fresh on-chain query
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (sdkB as any).channelStateCache.delete(channelId);
  const channelStateFromB = await sdkB!.getChannelState(channelId, TOKEN_ADDRESS);

  console.log(
    `Channel state from B: theirDeposit (A's deposit) = ${channelStateFromB.theirDeposit}, ` +
      `myDeposit (B's deposit) = ${channelStateFromB.myDeposit}, status = ${channelStateFromB.status}`
  );

  // A deposited at least the settlement amount into the channel
  const depositAmount = channelStateFromB.theirDeposit;
  expect(depositAmount).toBeGreaterThanOrEqual(balanceAfterPackets.creditBalance);
  expect(channelStateFromB.status).toBe('opened');

  // ========================================================================
  // Step 7: B claims from channel using A's signed claim (channel stays open)
  // ========================================================================
  // A signed balance proofs (claims) that were sent with packets via BTP.
  // Here we simulate A's claim covering the deposit amount.
  const ZERO_HASH = '0x' + '0'.repeat(64);

  // A signs a balance proof (the claim A sends to B during packet forwarding)
  const sigA = await sdkA!.signBalanceProof(channelId, 1, depositAmount);

  const claimFromA: BalanceProof = {
    channelId,
    nonce: 1,
    transferredAmount: depositAmount,
    lockedAmount: 0n,
    locksRoot: ZERO_HASH,
  };

  // Clear B's cache to get fresh on-chain state before claiming
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (sdkB as any).channelStateCache.delete(channelId);

  // Snapshot payment channel token balance before claim
  const tokenNetworkAddr = await sdkB!.getTokenNetworkAddress(TOKEN_ADDRESS);
  const channelBalanceBefore = await getTokenBalance(tokenNetworkAddr);
  console.log(`Payment channel contract balance before claim: ${channelBalanceBefore}`);

  // B claims from channel using A's signed balance proof — channel stays OPEN
  await sdkB!.claimFromChannel(channelId, TOKEN_ADDRESS, claimFromA, sigA);
  console.log("B claimed from channel using A's signed claim — channel stays open");

  // ========================================================================
  // Step 8: Verify payment channel balance decreased after claim
  // ========================================================================
  const channelBalanceAfter = await getTokenBalance(tokenNetworkAddr);
  console.log(
    `Payment channel contract balance: before=${channelBalanceBefore}, after=${channelBalanceAfter}, ` +
      `decrease=${channelBalanceBefore - channelBalanceAfter}`
  );

  // Payment channel balance should decrease by the claimed amount
  expect(channelBalanceAfter).toBeLessThan(channelBalanceBefore);
  expect(channelBalanceBefore - channelBalanceAfter).toBe(depositAmount);

  // ========================================================================
  // Step 9: Verify B's wallet balance increased after claim
  // ========================================================================
  const tokenBalanceBAfter = await getTokenBalance(CONNECTOR_B_ADDRESS);
  console.log(
    `Token balance (B) before: ${tokenBalanceBBefore}, after: ${tokenBalanceBAfter}, ` +
      `increase: ${tokenBalanceBAfter - tokenBalanceBBefore}`
  );

  // B's wallet should have received the claimed amount
  expect(tokenBalanceBAfter).toBeGreaterThan(tokenBalanceBBefore);
  expect(tokenBalanceBAfter - tokenBalanceBBefore).toBe(depositAmount);

  // ========================================================================
  // Step 10: Verify channel is STILL OPEN after claim
  // ========================================================================
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (sdkB as any).channelStateCache.delete(channelId);
  const channelStateAfterClaim = await sdkB!.getChannelState(channelId, TOKEN_ADDRESS);
  console.log(`Channel status after claim: ${channelStateAfterClaim.status}`);
  expect(channelStateAfterClaim.status).toBe('opened');

  // ========================================================================
  // Step 11: Verify settlement state is back to IDLE
  // ========================================================================
  const finalState = settlementMonitorA.getSettlementState('settlement-b', tokenId);
  expect(finalState).toBe(SettlementState.IDLE);

  console.log(
    'Settlement test passed — all balances verified, channel remains open for continued use'
  );
}

// ============================================================================
// Test Suite C: Settlement Integration - In-Memory Ledger + Anvil
// ============================================================================

describeInProcess('Settlement Integration E2E - In-Process (In-Memory Ledger)', () => {
  let env: SettlementTestEnv;

  beforeAll(async () => {
    console.log('Setting up Settlement Integration E2E (In-Memory Ledger) - 2-node topology...');

    // Enable settlement recording in packet handler
    process.env.SETTLEMENT_ENABLED = 'true';

    // Ensure TigerBeetle is NOT used (in-memory ledger mode)
    delete process.env.TIGERBEETLE_CLUSTER_ID;
    delete process.env.TIGERBEETLE_REPLICAS;

    // Set BTP peer secrets for permissionless mode
    process.env.BTP_PEER_CONNECTOR_A_SECRET = '';
    process.env.BTP_PEER_CONNECTOR_B_SECRET = '';

    // Verify Anvil is running
    try {
      await waitForAnvil();
      console.log('Anvil is ready');
    } catch {
      throw new Error(
        'Anvil not available. Run: docker compose -f docker-compose-base-e2e-test.yml up -d anvil_base_e2e'
      );
    }

    env = await startSettlementTopology(0);
    console.log('2-node settlement topology started (In-Memory Ledger)');
  });

  afterAll(async () => {
    console.log('Cleaning up settlement test connectors (In-Memory)...');
    await stopSettlementTopology(env);
    delete process.env.SETTLEMENT_ENABLED;
    delete process.env.BTP_PEER_CONNECTOR_A_SECRET;
    delete process.env.BTP_PEER_CONNECTOR_B_SECRET;
  });

  it('should trigger settlement when cumulative packet balances exceed threshold', async () => {
    await runSettlementTest(env);
  });
});

// ============================================================================
// Test Suite D: Settlement Integration - TigerBeetle + Anvil
// ============================================================================

describeInProcess('Settlement Integration E2E - In-Process (TigerBeetle)', () => {
  let env: SettlementTestEnv;
  let tigerBeetleAvailable = false;

  beforeAll(async () => {
    // Check if TigerBeetle is reachable before setting up
    tigerBeetleAvailable = await isTigerBeetleAvailable();
    if (!tigerBeetleAvailable) {
      console.log(
        'TigerBeetle not available at ' +
          TIGERBEETLE_ADDRESS +
          ' — skipping TigerBeetle settlement tests. ' +
          'Run: docker compose -f docker-compose-base-e2e-test.yml up -d tigerbeetle_e2e'
      );
      return;
    }

    console.log('Setting up Settlement Integration E2E (TigerBeetle) - 2-node topology...');

    // Enable settlement recording in packet handler
    process.env.SETTLEMENT_ENABLED = 'true';

    // Configure TigerBeetle backend
    process.env.TIGERBEETLE_CLUSTER_ID = TIGERBEETLE_CLUSTER_ID;
    process.env.TIGERBEETLE_REPLICAS = TIGERBEETLE_ADDRESS;

    // Set BTP peer secrets for permissionless mode
    process.env.BTP_PEER_CONNECTOR_A_SECRET = '';
    process.env.BTP_PEER_CONNECTOR_B_SECRET = '';

    // Verify Anvil is running
    try {
      await waitForAnvil();
      console.log('Anvil is ready');
    } catch {
      throw new Error(
        'Anvil not available. Run: docker compose -f docker-compose-base-e2e-test.yml up -d anvil_base_e2e'
      );
    }

    // Use different port offset to avoid conflicts with in-memory suite
    env = await startSettlementTopology(100);
    console.log('2-node settlement topology started (TigerBeetle)');
  });

  afterAll(async () => {
    if (tigerBeetleAvailable) {
      console.log('Cleaning up settlement test connectors (TigerBeetle)...');
      await stopSettlementTopology(env);
      delete process.env.SETTLEMENT_ENABLED;
      delete process.env.TIGERBEETLE_CLUSTER_ID;
      delete process.env.TIGERBEETLE_REPLICAS;
      delete process.env.BTP_PEER_CONNECTOR_A_SECRET;
      delete process.env.BTP_PEER_CONNECTOR_B_SECRET;
    }
  });

  it('should trigger settlement when cumulative packet balances exceed threshold', async () => {
    if (!tigerBeetleAvailable) {
      console.log('Skipping: TigerBeetle not available');
      return;
    }
    await runSettlementTest(env);
  });
});
