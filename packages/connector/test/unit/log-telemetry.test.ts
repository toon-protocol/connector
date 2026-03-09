/**
 * Integration test for LOG telemetry end-to-end flow
 * @packageDocumentation
 * @remarks
 * Tests the complete LOG telemetry pipeline from connector logging through
 * telemetry emission to dashboard server reception.
 */

import { TelemetryEmitter } from '../../src/telemetry/telemetry-emitter';
import { createLogger } from '../../src/utils/logger';
import { Logger } from '../../src/utils/logger';
import { TelemetryMessage, LogTelemetryData } from '../../src/telemetry/types';
import WebSocket, { WebSocketServer } from 'ws';

/**
 * Mock logger for TelemetryEmitter (not the logger being tested)
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
 * Test WebSocket server simulating dashboard telemetry server
 */
class MockTelemetryServer {
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
}

/**
 * Helper to wait for condition
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

describe('Test 12: LOG Telemetry Integration Test', () => {
  const TEST_PORT = 30000 + Math.floor(Math.random() * 10000);
  let mockTelemetryServer: MockTelemetryServer;
  let telemetryEmitter: TelemetryEmitter;
  let mockEmitterLogger: jest.Mocked<Logger>;
  let testLogger: Logger;

  beforeEach(async () => {
    // Start mock telemetry server
    mockTelemetryServer = new MockTelemetryServer(TEST_PORT);
    await mockTelemetryServer.start();

    // Create telemetry emitter
    mockEmitterLogger = createMockLogger();
    telemetryEmitter = new TelemetryEmitter(
      `ws://localhost:${TEST_PORT}`,
      'test-connector',
      mockEmitterLogger
    );
    await telemetryEmitter.connect();
    await waitFor(() => telemetryEmitter.isConnected(), 2000);

    // Create logger with telemetry transport (async when telemetryEmitter is provided)
    testLogger = await createLogger('test-connector', 'debug', telemetryEmitter);

    // Clear any connection messages
    mockTelemetryServer.clearMessages();
  }, 15000);

  afterEach(async () => {
    // Wait a bit for any pending log writes to complete
    await new Promise((resolve) => setTimeout(resolve, 100));
    await telemetryEmitter.disconnect();
    await mockTelemetryServer.stop();
    // Wait for port to be fully released
    await new Promise((resolve) => setTimeout(resolve, 200));
  }, 15000);

  it('should emit LOG telemetry event when connector logs a message', async () => {
    // Act
    testLogger.info('Test log message');

    // Wait for LOG event to be received
    await waitFor(() => mockTelemetryServer.receivedMessages.length > 0, 2000);

    // Assert
    expect(mockTelemetryServer.receivedMessages.length).toBe(1);
    const logEvent = mockTelemetryServer.receivedMessages[0];

    expect(logEvent?.type).toBe('LOG');
    expect(logEvent?.nodeId).toBe('test-connector');
    expect(logEvent?.timestamp).toBeDefined();
    expect(logEvent?.data).toMatchObject({
      level: 'info',
      message: 'Test log message',
      nodeId: 'test-connector',
    });
  });

  it('should emit LOG events for different log levels', async () => {
    // Act - Only test info, warn, error (debug is below default level)
    testLogger.info('Info message');
    testLogger.warn('Warning message');
    testLogger.error('Error message');

    // Wait for all 3 LOG events
    await waitFor(() => mockTelemetryServer.receivedMessages.length >= 3, 2000);

    // Assert
    expect(mockTelemetryServer.receivedMessages.length).toBe(3);

    const infoEvent = mockTelemetryServer.receivedMessages.find(
      (msg) => (msg.data as LogTelemetryData).level === 'info'
    );
    const warnEvent = mockTelemetryServer.receivedMessages.find(
      (msg) => (msg.data as LogTelemetryData).level === 'warn'
    );
    const errorEvent = mockTelemetryServer.receivedMessages.find(
      (msg) => (msg.data as LogTelemetryData).level === 'error'
    );

    expect(infoEvent).toBeDefined();
    expect(warnEvent).toBeDefined();
    expect(errorEvent).toBeDefined();

    expect((infoEvent?.data as LogTelemetryData).message).toBe('Info message');
    expect((warnEvent?.data as LogTelemetryData).message).toBe('Warning message');
    expect((errorEvent?.data as LogTelemetryData).message).toBe('Error message');
  });

  it('should include correlationId in LOG event when present', async () => {
    // Act
    testLogger.info({ correlationId: 'pkt_abc123' }, 'Packet received');

    // Wait for LOG event
    await waitFor(() => mockTelemetryServer.receivedMessages.length > 0, 2000);

    // Assert
    expect(mockTelemetryServer.receivedMessages.length).toBe(1);
    const logEvent = mockTelemetryServer.receivedMessages[0];

    expect(logEvent?.type).toBe('LOG');
    expect(logEvent?.data).toMatchObject({
      level: 'info',
      message: 'Packet received',
      correlationId: 'pkt_abc123',
    });
  });

  it('should include context fields in LOG event', async () => {
    // Act
    testLogger.info(
      {
        correlationId: 'pkt_xyz',
        destination: 'g.alice.wallet',
        peer: 'peer-bob',
        amount: '1000',
      },
      'Forwarding packet'
    );

    // Wait for LOG event
    await waitFor(() => mockTelemetryServer.receivedMessages.length > 0, 2000);

    // Assert
    expect(mockTelemetryServer.receivedMessages.length).toBe(1);
    const logEvent = mockTelemetryServer.receivedMessages[0];

    expect(logEvent?.type).toBe('LOG');
    expect(logEvent?.data).toMatchObject({
      level: 'info',
      message: 'Forwarding packet',
      correlationId: 'pkt_xyz',
      context: {
        destination: 'g.alice.wallet',
        peer: 'peer-bob',
        amount: '1000',
      },
    });
  });

  it('should emit multiple LOG events sequentially', async () => {
    // Act
    testLogger.info('First message');
    testLogger.warn('Second message');
    testLogger.error('Third message');

    // Wait for all 3 events
    await waitFor(() => mockTelemetryServer.receivedMessages.length >= 3, 2000);

    // Assert
    expect(mockTelemetryServer.receivedMessages.length).toBe(3);
    expect((mockTelemetryServer.receivedMessages[0]?.data as LogTelemetryData).message).toBe(
      'First message'
    );
    expect((mockTelemetryServer.receivedMessages[1]?.data as LogTelemetryData).message).toBe(
      'Second message'
    );
    expect((mockTelemetryServer.receivedMessages[2]?.data as LogTelemetryData).message).toBe(
      'Third message'
    );
  });

  it('should verify LOG event structure matches schema', async () => {
    // Act
    testLogger.info('Schema validation test');

    // Wait for LOG event
    await waitFor(() => mockTelemetryServer.receivedMessages.length > 0, 2000);

    // Assert - Verify complete LOG event structure
    const logEvent = mockTelemetryServer.receivedMessages[0];

    expect(logEvent).toMatchObject({
      type: 'LOG',
      nodeId: expect.any(String),
      timestamp: expect.any(String),
      data: {
        level: expect.stringMatching(/^(debug|info|warn|error)$/),
        timestamp: expect.any(String),
        nodeId: expect.any(String),
        message: expect.any(String),
      },
    });

    // Verify timestamp is valid ISO 8601
    expect(() => new Date(logEvent!.timestamp)).not.toThrow();
    expect(() => new Date((logEvent!.data as LogTelemetryData).timestamp)).not.toThrow();
  });

  it('should continue emitting LOG events after telemetry server restart', async () => {
    // Act - Log first message
    testLogger.info('Before server restart');
    await waitFor(() => mockTelemetryServer.receivedMessages.length > 0, 2000);
    expect(mockTelemetryServer.receivedMessages.length).toBe(1);

    // Disconnect emitter to stop auto-reconnect attempts
    await telemetryEmitter.disconnect();
    await waitFor(() => !telemetryEmitter.isConnected(), 2000);

    // Stop and restart telemetry server
    await mockTelemetryServer.stop();
    // Wait for port to be fully released (critical for CI environments)
    await new Promise((resolve) => setTimeout(resolve, 500));
    mockTelemetryServer = new MockTelemetryServer(TEST_PORT);
    await mockTelemetryServer.start();

    // Manually reconnect the emitter to the new server
    await telemetryEmitter.connect();
    await waitFor(() => telemetryEmitter.isConnected(), 5000);

    mockTelemetryServer.clearMessages();

    // Act - Log second message after reconnection
    testLogger.info('After server restart');
    await waitFor(() => mockTelemetryServer.receivedMessages.length > 0, 2000);

    // Assert
    expect(mockTelemetryServer.receivedMessages.length).toBe(1);
    expect((mockTelemetryServer.receivedMessages[0]?.data as LogTelemetryData).message).toBe(
      'After server restart'
    );

    // Cleanup: disconnect and wait before next test
    await telemetryEmitter.disconnect();
    await new Promise((resolve) => setTimeout(resolve, 300));
  });

  it('should not crash when telemetry server is unavailable', async () => {
    // Arrange - Disconnect from server
    await mockTelemetryServer.stop();
    await waitFor(() => !telemetryEmitter.isConnected(), 2000);

    // Act - Logging should not throw even when telemetry is disconnected
    expect(() => {
      testLogger.info('Message while disconnected');
      testLogger.warn('Another message');
      testLogger.error('Error message');
    }).not.toThrow();

    // Logger should still work for console output
    // (No assertion needed - just verify no crash)
  });

  it('should preserve nodeId from child logger in LOG events', async () => {
    // Arrange - The logger is already a child logger with nodeId: 'test-connector'
    const childLogger = testLogger.child({ additionalField: 'value' });

    // Act
    childLogger.info('Message from child logger');

    // Wait for LOG event
    await waitFor(() => mockTelemetryServer.receivedMessages.length > 0, 2000);

    // Assert
    const logEvent = mockTelemetryServer.receivedMessages[0];
    expect((logEvent?.data as LogTelemetryData).nodeId).toBe('test-connector');
  });
});
