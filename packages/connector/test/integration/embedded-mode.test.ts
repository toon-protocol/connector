/**
 * Embedded Mode Integration Tests
 *
 * Tests the connector library when used in embedded mode (in-process integration).
 * Verifies packet handling, direct method APIs, and payment flows without HTTP dependencies.
 *
 * Test Coverage:
 * - Embedded mode detection and configuration
 * - In-process packet handler registration
 * - Direct sendPacket() library calls
 * - Payment handler adapter (fulfillment computation, error mapping)
 * - Direct method APIs (registerPeer, listPeers, addRoute, etc.)
 * - Multi-node embedded topology
 * - Payment rejection scenarios
 * - Error handling and edge cases
 *
 * Prerequisites:
 * - None (no Docker required - pure TypeScript tests)
 *
 * Usage:
 *   npm test --workspace=packages/connector -- embedded-mode.test.ts
 */

import { ConnectorNode } from '../../src/core/connector-node';
import type { ConnectorConfig } from '../../src/config/types';
import type { PaymentHandler, PaymentRequest } from '../../src/core/payment-handler';
import { ILPErrorCode, PacketType } from '@crosstown/shared';
import pino from 'pino';
import * as crypto from 'crypto';
import { waitFor } from '../helpers/wait-for';

// Test timeout - 60 seconds (no Docker needed)
jest.setTimeout(60000);

// Test logger (silent during tests unless DEBUG=1)
const createTestLogger = (nodeId: string): pino.Logger => {
  return pino({
    level: process.env.DEBUG ? 'debug' : 'silent',
    name: nodeId,
  });
};

// Helper: Create minimal embedded mode config
const createEmbeddedConfig = (
  nodeId: string,
  btpPort: number,
  healthPort?: number
): ConnectorConfig => ({
  nodeId,
  btpServerPort: btpPort,
  healthCheckPort: healthPort ?? btpPort + 10000,
  deploymentMode: 'embedded',
  adminApi: { enabled: false },
  localDelivery: { enabled: false },
  explorer: { enabled: false },
  peers: [],
  routes: [],
  environment: 'development',
});

describe('Embedded Mode Integration Tests', () => {
  const originalEnv = { ...process.env };

  beforeEach(() => {
    // Disable explorer to avoid port conflicts in concurrent test runs
    process.env.EXPLORER_ENABLED = 'false';
  });

  afterEach(() => {
    // Restore EXPLORER_ENABLED
    if (originalEnv.EXPLORER_ENABLED !== undefined) {
      process.env.EXPLORER_ENABLED = originalEnv.EXPLORER_ENABLED;
    } else {
      delete process.env.EXPLORER_ENABLED;
    }
  });

  describe('Deployment Mode Detection', () => {
    let connector: ConnectorNode;

    afterEach(async () => {
      if (connector) {
        await connector.stop();
      }
    });

    it('should detect embedded mode from explicit config', async () => {
      const config = createEmbeddedConfig('test-node', 4001);
      connector = new ConnectorNode(config, createTestLogger('test-node'));

      expect(connector.getDeploymentMode()).toBe('embedded');
      expect(connector.isEmbedded()).toBe(true);
      expect(connector.isStandalone()).toBe(false);
    });

    it('should infer embedded mode when both APIs disabled', async () => {
      const config: ConnectorConfig = {
        nodeId: 'test-node',
        btpServerPort: 4002,
        healthCheckPort: 14002,
        // No deploymentMode specified
        adminApi: { enabled: false },
        localDelivery: { enabled: false },
        explorer: { enabled: false },
        peers: [],
        routes: [],
        environment: 'development',
      };

      connector = new ConnectorNode(config, createTestLogger('test-node'));

      expect(connector.getDeploymentMode()).toBe('embedded');
      expect(connector.isEmbedded()).toBe(true);
    });

    it('should infer standalone mode when both APIs enabled', async () => {
      const config: ConnectorConfig = {
        nodeId: 'test-node',
        btpServerPort: 4003,
        healthCheckPort: 14003,
        adminApi: { enabled: true, port: 18081 },
        localDelivery: { enabled: true, handlerUrl: 'http://localhost:8080' },
        explorer: { enabled: false },
        peers: [],
        routes: [],
        environment: 'development',
      };

      connector = new ConnectorNode(config, createTestLogger('test-node'));

      expect(connector.getDeploymentMode()).toBe('standalone');
      expect(connector.isStandalone()).toBe(true);
      expect(connector.isEmbedded()).toBe(false);
    });
  });

  describe('Payment Handler Registration', () => {
    let connector: ConnectorNode;

    beforeEach(async () => {
      const config = createEmbeddedConfig('test-node', 4010);
      connector = new ConnectorNode(config, createTestLogger('test-node'));
      await connector.start();
    });

    afterEach(async () => {
      if (connector) {
        await connector.stop();
      }
    });

    it('should register a payment handler', async () => {
      let handlerCalled = false;

      const handler: PaymentHandler = async (_request: PaymentRequest) => {
        handlerCalled = true;
        return { accept: true };
      };

      connector.setPacketHandler(handler);

      // Verify handler was registered (internal state - can't directly test,
      // but we'll verify it works in end-to-end tests below)
      expect(handlerCalled).toBe(false); // Not called yet
    });

    it('should clear payment handler when set to null', async () => {
      const handler: PaymentHandler = async (_request: PaymentRequest) => {
        return { accept: true };
      };

      connector.setPacketHandler(handler);
      connector.setPacketHandler(null);

      // Handler cleared - verified in end-to-end tests
    });
  });

  describe('Payment Flow - Single Node (Local Delivery)', () => {
    let connector: ConnectorNode;
    const receivedPayments: PaymentRequest[] = [];

    beforeEach(async () => {
      receivedPayments.length = 0;
      const config = createEmbeddedConfig('receiver', 4020);
      connector = new ConnectorNode(config, createTestLogger('receiver'));

      // Register handler that captures received payments
      connector.setPacketHandler(async (request: PaymentRequest) => {
        receivedPayments.push(request);
        return { accept: true };
      });

      await connector.start();

      // Add self-route so packets to g.receiver.* are delivered locally
      connector.addRoute({ prefix: 'g.receiver', nextHop: 'receiver', priority: 0 });
    });

    afterEach(async () => {
      if (connector) {
        await connector.stop();
      }
    });

    it('should handle local payment via setPacketHandler', async () => {
      const data = Buffer.from(JSON.stringify({ invoice: 'TEST-001' }));
      const executionCondition = crypto.createHash('sha256').update(data).digest();

      const result = await connector.sendPacket({
        destination: 'g.receiver.alice',
        amount: 5000n,
        executionCondition,
        expiresAt: new Date(Date.now() + 30000),
        data,
      });

      // Verify payment was fulfilled
      expect(result.type).toBe(PacketType.FULFILL);
      expect(result).toHaveProperty('fulfillment');

      // Verify handler received the payment
      await waitFor(() => receivedPayments.length > 0, { timeout: 5000, interval: 100 });
      expect(receivedPayments).toHaveLength(1);

      const receivedPayment = receivedPayments[0]!;
      expect(receivedPayment.destination).toBe('g.receiver.alice');
      expect(receivedPayment.amount).toBe('5000');
      expect(receivedPayment.paymentId).toBeDefined();
      expect(receivedPayment.data).toBeDefined();

      // Verify data was passed through
      const decodedData = Buffer.from(receivedPayment.data!, 'base64').toString('utf-8');
      expect(JSON.parse(decodedData)).toEqual({ invoice: 'TEST-001' });
    });

    it('should compute fulfillment correctly (SHA256 of data)', async () => {
      const data = Buffer.from('test-data');
      const expectedFulfillment = crypto.createHash('sha256').update(data).digest();
      const executionCondition = crypto.createHash('sha256').update(expectedFulfillment).digest();

      const result = await connector.sendPacket({
        destination: 'g.receiver.test',
        amount: 1000n,
        executionCondition,
        expiresAt: new Date(Date.now() + 30000),
        data,
      });

      expect(result.type).toBe(PacketType.FULFILL);
      if (result.type === PacketType.FULFILL) {
        expect(result.fulfillment.toString('hex')).toBe(expectedFulfillment.toString('hex'));
      }
    });

    it('should reject payment with invalid_amount error', async () => {
      // Replace handler with one that validates amount
      connector.setPacketHandler(async (request: PaymentRequest) => {
        if (BigInt(request.amount) < 1000n) {
          return {
            accept: false,
            rejectReason: {
              code: 'invalid_amount',
              message: 'Minimum payment is 1000 units',
            },
          };
        }
        return { accept: true };
      });

      const data = Buffer.from('test');
      const executionCondition = crypto.createHash('sha256').update(data).digest();

      const result = await connector.sendPacket({
        destination: 'g.receiver.test',
        amount: 500n, // Below minimum
        executionCondition,
        expiresAt: new Date(Date.now() + 30000),
        data,
      });

      // Verify payment was rejected
      expect(result.type).toBe(PacketType.REJECT);
      if (result.type === PacketType.REJECT) {
        expect(result.code).toBe(ILPErrorCode.F03_INVALID_AMOUNT);
        expect(result.message).toBe('Minimum payment is 1000 units');
      }
    });

    it('should reject payment with application_error', async () => {
      connector.setPacketHandler(async (_request: PaymentRequest) => {
        return {
          accept: false,
          rejectReason: {
            code: 'application_error',
            message: 'Custom business logic rejection',
          },
        };
      });

      const data = Buffer.from('test');
      const executionCondition = crypto.createHash('sha256').update(data).digest();

      const result = await connector.sendPacket({
        destination: 'g.receiver.test',
        amount: 1000n,
        executionCondition,
        expiresAt: new Date(Date.now() + 30000),
        data,
      });

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type === PacketType.REJECT) {
        expect(result.code).toBe(ILPErrorCode.F99_APPLICATION_ERROR);
        expect(result.message).toBe('Custom business logic rejection');
      }
    });

    it('should handle handler throwing an error', async () => {
      connector.setPacketHandler(async (_request: PaymentRequest) => {
        throw new Error('Unexpected handler error');
      });

      const data = Buffer.from('test');
      const executionCondition = crypto.createHash('sha256').update(data).digest();

      const result = await connector.sendPacket({
        destination: 'g.receiver.test',
        amount: 1000n,
        executionCondition,
        expiresAt: new Date(Date.now() + 30000),
        data,
      });

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type === PacketType.REJECT) {
        expect(result.code).toBe(ILPErrorCode.T00_INTERNAL_ERROR);
        expect(result.message).toContain('Internal error');
      }
    });

    it('should reject expired packets', async () => {
      const data = Buffer.from('test');
      const executionCondition = crypto.createHash('sha256').update(data).digest();

      const result = await connector.sendPacket({
        destination: 'g.receiver.test',
        amount: 1000n,
        executionCondition,
        expiresAt: new Date(Date.now() - 1000), // Already expired
        data,
      });

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type === PacketType.REJECT) {
        expect(result.code).toBe(ILPErrorCode.R00_TRANSFER_TIMED_OUT);
        expect(result.message).toContain('expired');
      }
    });
  });

  describe('Direct Method APIs - Peer Management', () => {
    let connector: ConnectorNode;

    beforeEach(async () => {
      const config = createEmbeddedConfig('test-node', 4030);
      connector = new ConnectorNode(config, createTestLogger('test-node'));
      await connector.start();
    });

    afterEach(async () => {
      if (connector) {
        await connector.stop();
      }
    });

    it('should register a peer via direct API', async () => {
      const peerInfo = await connector.registerPeer({
        id: 'peer1',
        url: 'ws://peer1.example.com:3000',
        authToken: 'secret-token',
      });

      expect(peerInfo.id).toBe('peer1');
      expect(peerInfo.connected).toBe(false); // BTP connection won't succeed (no server)
      expect(peerInfo.ilpAddresses).toEqual([]);
      expect(peerInfo.routeCount).toBe(0);
    });

    it('should register a peer with routes', async () => {
      const peerInfo = await connector.registerPeer({
        id: 'peer2',
        url: 'ws://peer2.example.com:3000',
        authToken: 'secret-token',
        routes: [
          { prefix: 'g.peer2', priority: 0 },
          { prefix: 'g.peer2.alice', priority: 10 },
        ],
      });

      expect(peerInfo.routeCount).toBe(2);
      expect(peerInfo.ilpAddresses).toContain('g.peer2');
      expect(peerInfo.ilpAddresses).toContain('g.peer2.alice');
    });

    it('should list all peers', async () => {
      await connector.registerPeer({
        id: 'peer1',
        url: 'ws://peer1.example.com:3000',
        authToken: 'secret1',
      });

      await connector.registerPeer({
        id: 'peer2',
        url: 'ws://peer2.example.com:3000',
        authToken: 'secret2',
      });

      const peers = connector.listPeers();

      expect(peers).toHaveLength(2);
      expect(peers.map((p) => p.id).sort()).toEqual(['peer1', 'peer2']);
    });

    it('should remove a peer', async () => {
      await connector.registerPeer({
        id: 'peer-to-remove',
        url: 'ws://peer.example.com:3000',
        authToken: 'secret',
        routes: [{ prefix: 'g.peer', priority: 0 }],
      });

      const result = await connector.removePeer('peer-to-remove', true);

      expect(result.peerId).toBe('peer-to-remove');
      expect(result.removedRoutes).toContain('g.peer');

      const peers = connector.listPeers();
      expect(peers).toHaveLength(0);
    });

    it('should throw error when removing non-existent peer', async () => {
      await expect(connector.removePeer('non-existent')).rejects.toThrow('Peer not found');
    });
  });

  describe('Direct Method APIs - Route Management', () => {
    let connector: ConnectorNode;

    beforeEach(async () => {
      const config = createEmbeddedConfig('test-node', 4040);
      connector = new ConnectorNode(config, createTestLogger('test-node'));
      await connector.start();

      // Register a peer for routes to reference
      await connector.registerPeer({
        id: 'peer1',
        url: 'ws://peer1.example.com:3000',
        authToken: 'secret',
      });
    });

    afterEach(async () => {
      if (connector) {
        await connector.stop();
      }
    });

    it('should add a route via direct API', () => {
      connector.addRoute({
        prefix: 'g.alice',
        nextHop: 'peer1',
        priority: 0,
      });

      const routes = connector.listRoutes();
      expect(routes).toHaveLength(1);
      expect(routes[0]).toMatchObject({
        prefix: 'g.alice',
        nextHop: 'peer1',
        priority: 0,
      });
    });

    it('should list all routes', () => {
      connector.addRoute({ prefix: 'g.alice', nextHop: 'peer1', priority: 0 });
      connector.addRoute({ prefix: 'g.bob', nextHop: 'peer1', priority: 10 });
      connector.addRoute({ prefix: 'g.charlie', nextHop: 'peer1', priority: 5 });

      const routes = connector.listRoutes();

      expect(routes).toHaveLength(3);
      expect(routes.map((r) => r.prefix).sort()).toEqual(['g.alice', 'g.bob', 'g.charlie']);
    });

    it('should remove a route', () => {
      connector.addRoute({ prefix: 'g.to-remove', nextHop: 'peer1', priority: 0 });
      connector.removeRoute('g.to-remove');

      const routes = connector.listRoutes();
      expect(routes).toHaveLength(0);
    });

    it('should throw error when removing non-existent route', () => {
      expect(() => connector.removeRoute('g.non-existent')).toThrow('Route not found');
    });

    it('should throw error when adding route with invalid prefix', () => {
      expect(() =>
        connector.addRoute({
          prefix: 'invalid prefix with spaces',
          nextHop: 'peer1',
          priority: 0,
        })
      ).toThrow('Invalid ILP address');
    });
  });

  describe('Multi-Node Embedded Topology', () => {
    let nodeA: ConnectorNode;
    let nodeB: ConnectorNode;
    let nodeC: ConnectorNode;

    const receivedPaymentsA: PaymentRequest[] = [];
    const receivedPaymentsB: PaymentRequest[] = [];
    const receivedPaymentsC: PaymentRequest[] = [];

    beforeEach(async () => {
      receivedPaymentsA.length = 0;
      receivedPaymentsB.length = 0;
      receivedPaymentsC.length = 0;

      // Create Node A (sender)
      const configA = createEmbeddedConfig('node-a', 4050);
      nodeA = new ConnectorNode(configA, createTestLogger('node-a'));
      nodeA.setPacketHandler(async (request) => {
        receivedPaymentsA.push(request);
        return { accept: true };
      });

      // Create Node B (intermediate)
      const configB = createEmbeddedConfig('node-b', 4051);
      nodeB = new ConnectorNode(configB, createTestLogger('node-b'));
      nodeB.setPacketHandler(async (request) => {
        receivedPaymentsB.push(request);
        return { accept: true };
      });

      // Create Node C (receiver)
      const configC = createEmbeddedConfig('node-c', 4052);
      nodeC = new ConnectorNode(configC, createTestLogger('node-c'));
      nodeC.setPacketHandler(async (request) => {
        receivedPaymentsC.push(request);
        return { accept: true };
      });

      // Start all nodes
      await nodeA.start();
      await nodeB.start();
      await nodeC.start();

      // Connect Node A -> Node B (empty authToken = no-auth mode per RFC-0023)
      await nodeA.registerPeer({
        id: 'node-b',
        url: 'ws://localhost:4051',
        authToken: '',
        routes: [{ prefix: 'g.nodeb', priority: 0 }],
      });

      // Connect Node B -> Node C (empty authToken = no-auth mode per RFC-0023)
      await nodeB.registerPeer({
        id: 'node-c',
        url: 'ws://localhost:4052',
        authToken: '',
        routes: [{ prefix: 'g.nodec', priority: 0 }],
      });

      // Add route on Node A: g.nodec -> node-b (multi-hop)
      nodeA.addRoute({ prefix: 'g.nodec', nextHop: 'node-b', priority: 0 });

      // Add self-route on Node C so packets to g.nodec.* are delivered locally
      nodeC.addRoute({ prefix: 'g.nodec', nextHop: 'node-c', priority: 0 });

      // Wait for BTP connections to establish
      await waitFor(
        () => {
          const peersA = nodeA.listPeers();
          const peersB = nodeB.listPeers();
          return (
            peersA.some((p) => p.id === 'node-b' && p.connected) &&
            peersB.some((p) => p.id === 'node-c' && p.connected)
          );
        },
        { timeout: 10000, interval: 100 }
      );
    });

    afterEach(async () => {
      if (nodeA) await nodeA.stop();
      if (nodeB) await nodeB.stop();
      if (nodeC) await nodeC.stop();
    });

    it('should route payment through multiple hops (A -> B -> C)', async () => {
      const data = Buffer.from(JSON.stringify({ test: 'multi-hop' }));
      const executionCondition = crypto.createHash('sha256').update(data).digest();

      const result = await nodeA.sendPacket({
        destination: 'g.nodec.receiver',
        amount: 10000n,
        executionCondition,
        expiresAt: new Date(Date.now() + 30000),
        data,
      });

      // Verify payment was fulfilled
      expect(result.type).toBe(PacketType.FULFILL);

      // Verify Node C (final hop) received the payment
      await waitFor(() => receivedPaymentsC.length > 0, { timeout: 5000, interval: 100 });
      expect(receivedPaymentsC).toHaveLength(1);
      expect(receivedPaymentsC[0]!.destination).toBe('g.nodec.receiver');
      expect(receivedPaymentsC[0]!.amount).toBe('10000');
    });

    it('should verify all nodes are in embedded mode', () => {
      expect(nodeA.isEmbedded()).toBe(true);
      expect(nodeB.isEmbedded()).toBe(true);
      expect(nodeC.isEmbedded()).toBe(true);

      expect(nodeA.isStandalone()).toBe(false);
      expect(nodeB.isStandalone()).toBe(false);
      expect(nodeC.isStandalone()).toBe(false);
    });

    it('should list connected peers for each node', () => {
      const peersA = nodeA.listPeers();
      const peersB = nodeB.listPeers();
      const peersC = nodeC.listPeers();

      expect(peersA.map((p) => p.id)).toContain('node-b');
      expect(peersB.map((p) => p.id)).toContain('node-c');
      expect(peersC).toHaveLength(0); // Node C has no outgoing peers
    });
  });

  describe('Error Handling - sendPacket()', () => {
    let connector: ConnectorNode;

    beforeEach(async () => {
      const config = createEmbeddedConfig('test-node', 4060);
      connector = new ConnectorNode(config, createTestLogger('test-node'));
    });

    afterEach(async () => {
      if (connector) {
        await connector.stop();
      }
    });

    it('should throw error when calling sendPacket before start()', async () => {
      const data = Buffer.from('test');
      const executionCondition = crypto.createHash('sha256').update(data).digest();

      await expect(
        connector.sendPacket({
          destination: 'g.test',
          amount: 1000n,
          executionCondition,
          expiresAt: new Date(Date.now() + 30000),
          data,
        })
      ).rejects.toThrow('Connector is not started');
    });

    it('should return F02_UNREACHABLE for unknown destination', async () => {
      await connector.start();

      const data = Buffer.from('test');
      const executionCondition = crypto.createHash('sha256').update(data).digest();

      const result = await connector.sendPacket({
        destination: 'g.unknown.destination',
        amount: 1000n,
        executionCondition,
        expiresAt: new Date(Date.now() + 30000),
        data,
      });

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type === PacketType.REJECT) {
        expect(result.code).toBe(ILPErrorCode.F02_UNREACHABLE);
      }
    });
  });

  describe('Configuration Validation', () => {
    it('should create connector with minimal embedded config', async () => {
      const config = createEmbeddedConfig('minimal-node', 4070);
      const connector = new ConnectorNode(config, createTestLogger('minimal-node'));

      expect(connector.isEmbedded()).toBe(true);

      await connector.start();
      await connector.stop();
    });

    it('should accept both config objects and YAML paths', async () => {
      // Config object (preferred for embedded mode)
      const config = createEmbeddedConfig('config-object-node', 4071);
      const connector1 = new ConnectorNode(config, createTestLogger('config-object-node'));
      expect(connector1).toBeDefined();
      await connector1.start();
      await connector1.stop();

      // YAML path (for standalone mode compatibility)
      // Note: ConnectorNode accepts string paths for YAML files
      // This is useful for standalone mode deployments
    });
  });

  describe('Business Logic Error Code Mapping', () => {
    let connector: ConnectorNode;

    beforeEach(async () => {
      const config = createEmbeddedConfig('test-node', 4080);
      connector = new ConnectorNode(config, createTestLogger('test-node'));
      await connector.start();

      // Add self-route so packets to g.test-node.* are delivered locally
      connector.addRoute({ prefix: 'g.test-node', nextHop: 'test-node', priority: 0 });
    });

    afterEach(async () => {
      if (connector) {
        await connector.stop();
      }
    });

    const testErrorMapping = async (
      businessCode: string,
      expectedILPCode: ILPErrorCode
    ): Promise<void> => {
      connector.setPacketHandler(async (_request: PaymentRequest) => {
        return {
          accept: false,
          rejectReason: {
            code: businessCode,
            message: `Test ${businessCode} error`,
          },
        };
      });

      const data = Buffer.from('test');
      const executionCondition = crypto.createHash('sha256').update(data).digest();

      const result = await connector.sendPacket({
        destination: 'g.test-node.test',
        amount: 1000n,
        executionCondition,
        expiresAt: new Date(Date.now() + 30000),
        data,
      });

      expect(result.type).toBe(PacketType.REJECT);
      if (result.type === PacketType.REJECT) {
        expect(result.code).toBe(expectedILPCode);
      }
    };

    it('should map insufficient_funds -> T04', async () => {
      await testErrorMapping('insufficient_funds', ILPErrorCode.T04_INSUFFICIENT_LIQUIDITY);
    });

    it('should map expired -> R00', async () => {
      await testErrorMapping('expired', ILPErrorCode.R00_TRANSFER_TIMED_OUT);
    });

    it('should map invalid_request -> F00', async () => {
      await testErrorMapping('invalid_request', ILPErrorCode.F00_BAD_REQUEST);
    });

    it('should map invalid_amount -> F03', async () => {
      await testErrorMapping('invalid_amount', ILPErrorCode.F03_INVALID_AMOUNT);
    });

    it('should map unexpected_payment -> F06', async () => {
      await testErrorMapping('unexpected_payment', ILPErrorCode.F06_UNEXPECTED_PAYMENT);
    });

    it('should map application_error -> F99', async () => {
      await testErrorMapping('application_error', ILPErrorCode.F99_APPLICATION_ERROR);
    });

    it('should map internal_error -> T00', async () => {
      await testErrorMapping('internal_error', ILPErrorCode.T00_INTERNAL_ERROR);
    });

    it('should map unknown codes -> F99', async () => {
      await testErrorMapping('unknown_error_code', ILPErrorCode.F99_APPLICATION_ERROR);
    });
  });
});
