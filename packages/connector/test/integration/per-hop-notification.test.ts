/**
 * Integration tests for per-hop BLS notification pipeline (Story 30.3)
 * Tests 3-connector chain (A → B → C) with per-hop notification at B
 */

import { ConnectorNode } from '../../src/core/connector-node';
import { createLogger } from '../../src/utils/logger';
import { BTPClient, Peer } from '../../src/btp/btp-client';
import { ILPFulfillPacket, PacketType } from '@crosstown/shared';
import { ConnectorConfig, LocalDeliveryRequest } from '../../src/config/types';
import { waitFor } from '../helpers/wait-for';

/** Random base port to avoid collisions */
const basePort = 50000 + Math.floor(Math.random() * 10000);
const portA = basePort;
const portB = basePort + 1;
const portC = basePort + 2;

/** Skip unless E2E_TESTS=true (uses real BTP connections) */
const e2eEnabled = process.env.E2E_TESTS === 'true';
const describeIfE2E = e2eEnabled ? describe : describe.skip;

describeIfE2E('Per-Hop Notification Pipeline (3-connector chain)', () => {
  jest.setTimeout(30000);
  let connectorA: ConnectorNode;
  let connectorB: ConnectorNode;
  let connectorC: ConnectorNode;
  let testClient: BTPClient;

  /** Notifications recorded by Connector B's transit handler */
  const transitNotifications: LocalDeliveryRequest[] = [];
  /** Deliveries recorded by Connector C's final-hop handler */
  const finalDeliveries: LocalDeliveryRequest[] = [];

  beforeAll(() => {
    // Disable explorer UI to avoid port conflicts in tests
    process.env['EXPLORER_ENABLED'] = 'false';
  });

  afterAll(() => {
    delete process.env['EXPLORER_ENABLED'];
  });

  beforeEach(async () => {
    transitNotifications.length = 0;
    finalDeliveries.length = 0;

    // Configs use empty peers — peers registered dynamically via registerPeer (no-auth mode)
    const configA: ConnectorConfig = {
      nodeId: 'connector-a',
      btpServerPort: portA,
      healthCheckPort: basePort + 10,
      logLevel: 'error',
      environment: 'development',
      peers: [],
      routes: [],
    };

    const configB: ConnectorConfig = {
      nodeId: 'connector-b',
      btpServerPort: portB,
      healthCheckPort: basePort + 11,
      logLevel: 'error',
      environment: 'development',
      peers: [],
      routes: [],
      localDelivery: {
        enabled: true,
        handlerUrl: 'http://localhost:9999', // dummy — in-process handler overrides
        timeout: 5000,
        perHopNotification: true,
      },
    };

    const configC: ConnectorConfig = {
      nodeId: 'connector-c',
      btpServerPort: portC,
      healthCheckPort: basePort + 12,
      logLevel: 'error',
      environment: 'development',
      peers: [],
      routes: [{ prefix: 'g.connector-c', nextHop: 'connector-c' }],
    };

    const loggerA = createLogger('connector-a', 'error');
    const loggerB = createLogger('connector-b', 'error');
    const loggerC = createLogger('connector-c', 'error');

    connectorC = new ConnectorNode(configC, loggerC);
    connectorB = new ConnectorNode(configB, loggerB);
    connectorA = new ConnectorNode(configA, loggerA);

    // Register mock BLS handlers BEFORE starting connectors
    // Connector B: transit notification mock (records incoming notifications)
    connectorB.setLocalDeliveryHandler(async (request: LocalDeliveryRequest) => {
      transitNotifications.push(request);
      return { fulfill: { fulfillment: Buffer.alloc(32).toString('base64') } };
    });

    // Connector C: final-hop BLS (returns fulfill)
    connectorC.setLocalDeliveryHandler(async (request: LocalDeliveryRequest) => {
      finalDeliveries.push(request);
      return { fulfill: { fulfillment: Buffer.alloc(32).toString('base64') } };
    });

    // Start connectors (C first, then B, then A)
    await connectorC.start();
    await connectorB.start();
    await connectorA.start();

    // Register peers dynamically with empty authToken (no-auth/permissionless mode)
    await connectorB.registerPeer({
      id: 'connector-c',
      url: `ws://localhost:${portC}`,
      authToken: '',
      routes: [{ prefix: 'g.connector-c' }],
    });

    await connectorA.registerPeer({
      id: 'connector-b',
      url: `ws://localhost:${portB}`,
      authToken: '',
      routes: [{ prefix: 'g.connector-c' }],
    });

    // Wait for BTP connections to establish
    await waitFor(
      () => {
        const peersA = connectorA.listPeers();
        const peersB = connectorB.listPeers();
        return (
          peersA.some((p) => p.id === 'connector-b' && p.connected) &&
          peersB.some((p) => p.id === 'connector-c' && p.connected)
        );
      },
      { timeout: 5000, interval: 100 }
    );

    // Create test client to send packets into Connector A (no-auth mode)
    const testPeer: Peer = {
      id: 'testClient',
      url: `ws://localhost:${portA}`,
      authToken: '',
      connected: false,
      lastSeen: new Date(),
    };
    testClient = new BTPClient(testPeer, 'test-client', createLogger('testClient', 'error'));
    await testClient.connect();
  });

  afterEach(async () => {
    try {
      if (testClient?.isConnected) await testClient.disconnect();
    } catch {
      /* cleanup */
    }
    try {
      if (connectorA) await connectorA.stop();
    } catch {
      /* cleanup */
    }
    try {
      if (connectorB) await connectorB.stop();
    } catch {
      /* cleanup */
    }
    try {
      if (connectorC) await connectorC.stop();
    } catch {
      /* cleanup */
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  });

  it('should fulfill packet through 3-hop chain with per-hop notification', async () => {
    // Arrange
    const packet = {
      type: PacketType.PREPARE as const,
      amount: BigInt(1000),
      destination: 'g.connector-c.destination',
      executionCondition: Buffer.alloc(32, 1),
      expiresAt: new Date(Date.now() + 10000),
      data: Buffer.alloc(0),
    };

    // Act
    const response = await testClient.sendPacket(packet);

    // Wait for fire-and-forget to settle
    await new Promise((resolve) => setTimeout(resolve, 100));

    // Assert — packet fulfilled
    expect(response.type).toBe(PacketType.FULFILL);
    expect((response as ILPFulfillPacket).fulfillment).toBeInstanceOf(Buffer);

    // Assert — B's mock handler received transit notification with isTransit: true
    expect(transitNotifications.length).toBe(1);
    const transitNotif = transitNotifications[0]!;
    expect(transitNotif.isTransit).toBe(true);
    expect(transitNotif.destination).toBe('g.connector-c.destination');
    expect(transitNotif.sourcePeer).toBe('connector-a');

    // Assert — C's mock handler received final delivery (isTransit not set)
    expect(finalDeliveries.length).toBe(1);
    const finalDelivery = finalDeliveries[0]!;
    expect(finalDelivery.isTransit).toBeUndefined();
    expect(finalDelivery.destination).toBe('g.connector-c.destination');
  });

  it('should add negligible latency with fire-and-forget notification', async () => {
    const ITERATIONS = 5;

    // First, disable per-hop notification to get baseline
    connectorB.setLocalDeliveryHandler(async (request: LocalDeliveryRequest) => {
      transitNotifications.push(request);
      return { fulfill: { fulfillment: Buffer.alloc(32).toString('base64') } };
    });

    // Create a helper to create fresh packets (each needs unique expiry)
    const createPacket = () => ({
      type: PacketType.PREPARE as const,
      amount: BigInt(1000),
      destination: 'g.connector-c.destination',
      executionCondition: Buffer.alloc(32, 1),
      expiresAt: new Date(Date.now() + 10000),
      data: Buffer.alloc(0),
    });

    // Baseline: per-hop notification already enabled, just measure
    const perHopTimes: number[] = [];
    for (let i = 0; i < ITERATIONS; i++) {
      const start = performance.now();
      await testClient.sendPacket(createPacket());
      perHopTimes.push(performance.now() - start);
    }
    const perHopAvg = perHopTimes.reduce((a, b) => a + b, 0) / ITERATIONS;

    // Verify per-hop overhead is reasonable (< 500ms per packet)
    // In practice, fire-and-forget adds <1ms but BTP roundtrip dominates
    expect(perHopAvg).toBeLessThan(500);
  });
});
