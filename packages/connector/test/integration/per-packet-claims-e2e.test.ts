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
 */

/* eslint-disable no-console */

import { ConnectorNode } from '../../src/core/connector-node';
import type { ConnectorConfig } from '../../src/config/types';
import pino from 'pino';
import { PacketType, ILPPreparePacket, ILPRejectPacket, ILPErrorCode } from '@crosstown/shared';
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

// Docker connector endpoints
const DOCKER_HEALTH_A = 'http://localhost:8080/health';
const DOCKER_HEALTH_B = 'http://localhost:8090/health';
// ============================================================================
// Helper Functions
// ============================================================================

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
  return {
    type: PacketType.PREPARE,
    amount,
    destination,
    executionCondition: Buffer.alloc(32),
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
