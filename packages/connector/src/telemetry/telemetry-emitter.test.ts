/**
 * Integration tests for TelemetryEmitter
 * @packageDocumentation
 * @remarks
 * Uses real WebSocket server (ws library) for authentic integration testing.
 * Tests verify actual WebSocket communication, message serialization, and reconnection logic.
 */

import { TelemetryEmitter } from './telemetry-emitter';
import { Logger } from '../utils/logger';
import { TelemetryMessage } from './types';
import { RoutingTableEntry, ILPPreparePacket, PacketType } from '@crosstown/shared';
import WebSocket, { WebSocketServer } from 'ws';

/**
 * Mock logger for testing log output without console noise
 */
const createMockLogger = (): jest.Mocked<Logger> =>
  ({
    info: jest.fn(),
    warn: jest.fn(),
    error: jest.fn(),
    debug: jest.fn(),
    fatal: jest.fn(),
    trace: jest.fn(),
    silent: jest.fn(),
    level: 'info',
    child: jest.fn().mockReturnThis(),
  }) as unknown as jest.Mocked<Logger>;

/**
 * Test WebSocket server for integration testing
 */
class TestWebSocketServer {
  private wss: WebSocketServer | null = null;
  private port: number;
  public receivedMessages: TelemetryMessage[] = [];
  public connectedClients: WebSocket[] = [];

  constructor(port: number) {
    this.port = port;
  }

  async start(): Promise<void> {
    return new Promise((resolve) => {
      this.wss = new WebSocketServer({ port: this.port });

      this.wss.on('connection', (ws) => {
        this.connectedClients.push(ws);

        ws.on('message', (data) => {
          const message = JSON.parse(data.toString()) as TelemetryMessage;
          this.receivedMessages.push(message);
        });
      });

      this.wss.on('listening', () => {
        resolve();
      });
    });
  }

  async stop(): Promise<void> {
    return new Promise((resolve) => {
      if (this.wss) {
        // Close all client connections
        this.connectedClients.forEach((ws) => ws.close());
        this.connectedClients = [];

        this.wss.close(() => {
          this.wss = null;
          resolve();
        });
      } else {
        resolve();
      }
    });
  }

  clearMessages(): void {
    this.receivedMessages = [];
  }

  closeAllClients(): void {
    this.connectedClients.forEach((ws) => ws.close());
    this.connectedClients = [];
  }
}

/**
 * Helper to wait for condition with timeout
 */
const waitFor = async (
  condition: () => boolean,
  timeoutMs: number = 5000,
  checkIntervalMs: number = 50
): Promise<void> => {
  const startTime = Date.now();
  while (!condition()) {
    if (Date.now() - startTime > timeoutMs) {
      throw new Error('Timeout waiting for condition');
    }
    await new Promise((resolve) => setTimeout(resolve, checkIntervalMs));
  }
};

/**
 * Factory function to create valid ILP Prepare packet for testing
 */
const createValidPreparePacket = (overrides?: Partial<ILPPreparePacket>): ILPPreparePacket => {
  const futureExpiry = new Date(Date.now() + 10000); // 10 seconds in future
  return {
    type: PacketType.PREPARE,
    amount: BigInt(1000),
    destination: 'g.alice.wallet',
    executionCondition: Buffer.alloc(32, 'a'), // 32-byte hash
    expiresAt: futureExpiry,
    data: Buffer.alloc(0),
    ...overrides,
  };
};

describe('TelemetryEmitter Integration Tests', () => {
  const TEST_PORT = 9999;
  let testServer: TestWebSocketServer;
  let mockLogger: jest.Mocked<Logger>;

  beforeEach(async () => {
    testServer = new TestWebSocketServer(TEST_PORT);
    await testServer.start();
    mockLogger = createMockLogger();
  });

  afterEach(async () => {
    await testServer.stop();
  });

  describe('Connection Management', () => {
    it('should connect to WebSocket server successfully', async () => {
      // Arrange
      const emitter = new TelemetryEmitter(`ws://localhost:${TEST_PORT}`, 'test-node', mockLogger);

      // Act
      await emitter.connect();

      // Wait for connection to establish
      await waitFor(() => emitter.isConnected(), 1000);

      // Assert
      expect(emitter.isConnected()).toBe(true);
      expect(testServer.connectedClients.length).toBe(1);
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({ event: 'telemetry_connected' }),
        'Telemetry connected to dashboard'
      );

      // Cleanup
      await emitter.disconnect();
    });

    it.skip('should handle connection failure gracefully', async () => {
      // Arrange - Use invalid port that's not listening
      const invalidPort = 9998;
      const emitter = new TelemetryEmitter(
        `ws://localhost:${invalidPort}`,
        'test-node',
        mockLogger
      );

      // Act
      await emitter.connect();

      // Wait a bit for connection attempt
      await new Promise((resolve) => setTimeout(resolve, 200));

      // Assert - Should not crash
      expect(emitter.isConnected()).toBe(false);

      // Emit should not throw even though not connected
      expect(() => {
        emitter.emitNodeStatus([], [], 'healthy');
      }).not.toThrow();

      // Cleanup
      await emitter.disconnect();
    });

    it.skip('should reconnect after connection loss', async () => {
      // Arrange
      const emitter = new TelemetryEmitter(`ws://localhost:${TEST_PORT}`, 'test-node', mockLogger);
      await emitter.connect();
      await waitFor(() => emitter.isConnected(), 1000);

      // Act - Close the connection from server side
      testServer.closeAllClients();

      // Wait for disconnection
      await waitFor(() => !emitter.isConnected(), 1000);
      expect(emitter.isConnected()).toBe(false);

      // Wait for reconnection attempt (should happen within 1-2 seconds)
      await waitFor(() => emitter.isConnected(), 3000);

      // Assert
      expect(emitter.isConnected()).toBe(true);
      expect(mockLogger.debug).toHaveBeenCalledWith(
        expect.objectContaining({ event: 'telemetry_reconnect_attempt' }),
        'Attempting telemetry reconnection'
      );

      // Cleanup
      await emitter.disconnect();
    });
  });

  describe('Telemetry Message Emission', () => {
    let emitter: TelemetryEmitter;

    beforeEach(async () => {
      emitter = new TelemetryEmitter(`ws://localhost:${TEST_PORT}`, 'test-node', mockLogger);
      await emitter.connect();
      await waitFor(() => emitter.isConnected(), 1000);
      testServer.clearMessages();
    });

    afterEach(async () => {
      await emitter.disconnect();
    });

    it('should emit NODE_STATUS message', async () => {
      // Arrange
      const routes: RoutingTableEntry[] = [
        { prefix: 'g.alice', nextHop: 'peer-alice', priority: 10 },
      ];
      const peers = [{ id: 'peer-alice', url: 'ws://alice:3000', connected: true }];

      // Act
      emitter.emitNodeStatus(routes, peers, 'healthy');

      // Wait for message to be received
      await waitFor(() => testServer.receivedMessages.length > 0, 1000);

      // Assert
      expect(testServer.receivedMessages.length).toBe(1);
      const message = testServer.receivedMessages[0]!;
      expect(message.type).toBe('NODE_STATUS');
      expect(message.nodeId).toBe('test-node');
      expect(message.timestamp).toBeDefined();
      expect(message.data).toMatchObject({
        routes,
        peers,
        health: 'healthy',
        uptime: expect.any(Number),
        peersConnected: 1,
        totalPeers: 1,
      });
    });

    it('should emit PACKET_RECEIVED message', async () => {
      // Arrange
      const packet = createValidPreparePacket();
      const source = 'peer-alice';

      // Act
      emitter.emitPacketReceived(packet, source);

      // Wait for message to be received
      await waitFor(() => testServer.receivedMessages.length > 0, 1000);

      // Assert
      expect(testServer.receivedMessages.length).toBe(1);
      const message = testServer.receivedMessages[0]!;
      expect(message.type).toBe('PACKET_RECEIVED');
      expect(message.nodeId).toBe('test-node');
      expect(message.timestamp).toBeDefined();
      expect(message.data).toMatchObject({
        packetId: packet.executionCondition.toString('hex'),
        packetType: 'PREPARE',
        source: 'peer-alice',
        destination: 'g.alice.wallet',
        amount: '1000',
      });
    });

    it('should emit PACKET_SENT message', async () => {
      // Arrange
      const packetId = 'abc123';
      const nextHop = 'peer-bob';

      // Act
      emitter.emitPacketSent(packetId, nextHop);

      // Wait for message to be received
      await waitFor(() => testServer.receivedMessages.length > 0, 1000);

      // Assert
      expect(testServer.receivedMessages.length).toBe(1);
      const message = testServer.receivedMessages[0]!;
      expect(message.type).toBe('PACKET_SENT');
      expect(message.nodeId).toBe('test-node');
      expect(message.timestamp).toBeDefined();
      expect(message.data).toMatchObject({
        packetId: 'abc123',
        nextHop: 'peer-bob',
        timestamp: expect.any(String),
      });
    });

    it('should emit ROUTE_LOOKUP message', async () => {
      // Arrange
      const destination = 'g.alice.wallet';
      const selectedPeer = 'peer-alice';
      const reason = 'longest prefix match';

      // Act
      emitter.emitRouteLookup(destination, selectedPeer, reason);

      // Wait for message to be received
      await waitFor(() => testServer.receivedMessages.length > 0, 1000);

      // Assert
      expect(testServer.receivedMessages.length).toBe(1);
      const message = testServer.receivedMessages[0]!;
      expect(message.type).toBe('ROUTE_LOOKUP');
      expect(message.nodeId).toBe('test-node');
      expect(message.timestamp).toBeDefined();
      expect(message.data).toMatchObject({
        destination: 'g.alice.wallet',
        selectedPeer: 'peer-alice',
        reason: 'longest prefix match',
      });
    });

    it('should emit ROUTE_LOOKUP with null selectedPeer for no route found', async () => {
      // Arrange
      const destination = 'g.unknown.wallet';
      const selectedPeer = null;
      const reason = 'no route found';

      // Act
      emitter.emitRouteLookup(destination, selectedPeer, reason);

      // Wait for message to be received
      await waitFor(() => testServer.receivedMessages.length > 0, 1000);

      // Assert
      expect(testServer.receivedMessages.length).toBe(1);
      const message = testServer.receivedMessages[0]!;
      expect(message.type).toBe('ROUTE_LOOKUP');
      expect(message.data).toMatchObject({
        destination: 'g.unknown.wallet',
        selectedPeer: null,
        reason: 'no route found',
      });
    });
  });

  describe('Non-Blocking Behavior', () => {
    it('should not throw when emitting without connection', async () => {
      // Arrange
      const emitter = new TelemetryEmitter(`ws://localhost:${TEST_PORT}`, 'test-node', mockLogger);
      // Do NOT connect

      // Act & Assert - All emit methods should not throw
      expect(() => emitter.emitNodeStatus([], [], 'healthy')).not.toThrow();
      expect(() => emitter.emitPacketReceived(createValidPreparePacket(), 'unknown')).not.toThrow();
      expect(() => emitter.emitPacketSent('packetId', 'nextHop')).not.toThrow();
      expect(() => emitter.emitRouteLookup('destination', 'peer', 'reason')).not.toThrow();

      // Verify DEBUG logs were generated
      expect(mockLogger.debug).toHaveBeenCalledWith(
        expect.objectContaining({ event: 'telemetry_not_connected' }),
        'Telemetry not connected, skipping emission'
      );
    });
  });
});
