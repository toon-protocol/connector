/**
 * Unit tests for BTPServer
 * Tests BTP WebSocket server functionality per RFC-0023
 */

import { BTPServer } from './btp-server';
import { Logger } from '../utils/logger';
import { PacketHandler } from '../core/packet-handler';
import { BTPMessage, BTPMessageType } from './btp-types';
import { serializeBTPMessage } from './btp-message-parser';
import {
  ILPPreparePacket,
  ILPFulfillPacket,
  ILPRejectPacket,
  PacketType,
  ILPErrorCode,
  serializePacket,
} from '@toon-protocol/shared';
import WebSocket, { WebSocketServer } from 'ws';
import { EventEmitter } from 'events';

/**
 * Mock logger for testing
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
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    child: jest.fn(function (this: any) {
      return this;
    }),
  }) as unknown as jest.Mocked<Logger>;

/**
 * Mock PacketHandler
 */
const createMockPacketHandler = (): jest.Mocked<PacketHandler> => {
  const mockHandler = {
    handlePreparePacket: jest.fn(),
  } as unknown as jest.Mocked<PacketHandler>;
  return mockHandler;
};

/**
 * Create mock WebSocket instance
 */
class MockWebSocket extends EventEmitter {
  public readyState = WebSocket.OPEN;
  public sentMessages: Buffer[] = [];

  send(data: Buffer): void {
    this.sentMessages.push(data);
  }

  close(code?: number, reason?: string): void {
    this.readyState = WebSocket.CLOSED as typeof WebSocket.OPEN;
    this.emit('close', code, reason);
  }
}

/**
 * Create valid ILP Prepare packet for testing
 */
const createValidPreparePacket = (): ILPPreparePacket => {
  const futureExpiry = new Date(Date.now() + 10000);
  return {
    type: PacketType.PREPARE,
    amount: BigInt(1000),
    destination: 'g.alice.wallet',
    executionCondition: Buffer.alloc(32),
    expiresAt: futureExpiry,
    data: Buffer.alloc(0),
  };
};

/**
 * Create valid ILP Fulfill packet
 */
const createValidFulfillPacket = (): ILPFulfillPacket => ({
  type: PacketType.FULFILL,
  fulfillment: Buffer.alloc(32),
  data: Buffer.alloc(0),
});

/**
 * Create valid ILP Reject packet
 */
const createValidRejectPacket = (): ILPRejectPacket => ({
  type: PacketType.REJECT,
  code: ILPErrorCode.F02_UNREACHABLE,
  triggeredBy: 'g.connector',
  message: 'No route found',
  data: Buffer.alloc(0),
});

/**
 * Create BTP auth message
 */
const createAuthMessage = (peerId: string, secret: string, requestId = 1): BTPMessage => ({
  type: BTPMessageType.MESSAGE,
  requestId,
  data: {
    protocolData: [
      {
        protocolName: 'auth',
        contentType: 0,
        data: Buffer.from(JSON.stringify({ peerId, secret }), 'utf8'),
      },
    ],
  },
});

/**
 * Create BTP MESSAGE with ILP packet
 */
const createBTPMessage = (ilpPacket: Buffer, requestId = 2): BTPMessage => ({
  type: BTPMessageType.MESSAGE,
  requestId,
  data: {
    protocolData: [],
    ilpPacket,
  },
});

// Use port 0 to let the OS assign a free port, avoiding EADDRINUSE in parallel CI runs
const AUTO_PORT = 0;

describe('BTPServer', () => {
  let server: BTPServer;
  let mockLogger: jest.Mocked<Logger>;
  let mockPacketHandler: jest.Mocked<PacketHandler>;
  let originalEnv: NodeJS.ProcessEnv;

  beforeEach(() => {
    mockLogger = createMockLogger();
    mockPacketHandler = createMockPacketHandler();
    server = new BTPServer(mockLogger, mockPacketHandler);
    originalEnv = { ...process.env };
    jest.clearAllMocks();
  });

  afterEach(async () => {
    await server.stop();
    process.env = originalEnv;
  });

  describe('Constructor', () => {
    it('should create BTPServer with logger and packet handler', () => {
      // Arrange & Act
      const btpServer = new BTPServer(mockLogger, mockPacketHandler);

      // Assert
      expect(btpServer).toBeDefined();
      expect(btpServer).toBeInstanceOf(BTPServer);
    });
  });

  describe('start()', () => {
    it('should start WebSocket server on specified port', async () => {
      // Act
      await server.start(AUTO_PORT);

      // Assert
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const actualPort = ((server as any).wss as WebSocketServer).address() as { port: number };
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_server_started',
          port: AUTO_PORT,
        }),
        expect.stringContaining(String(AUTO_PORT))
      );
      expect(actualPort.port).toBeGreaterThan(0);
    });

    it('should use BTP_SERVER_PORT environment variable when port not specified', async () => {
      // Arrange — use port 0 via env var to avoid collisions
      process.env['BTP_SERVER_PORT'] = '0';

      // Act
      await server.start();

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_server_started',
          port: 0,
        }),
        expect.any(String)
      );
    });

    it('should default to port from env var when no port specified', async () => {
      // Arrange — use port 0 via env var to avoid collisions
      process.env['BTP_SERVER_PORT'] = '0';

      // Act
      await server.start();

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_server_started',
          port: 0,
        }),
        expect.any(String)
      );
    });
  });

  describe('stop()', () => {
    it('should close all active connections and WebSocket server', async () => {
      // Arrange
      await server.start(AUTO_PORT);

      // Act
      await server.stop();

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_server_shutdown',
        }),
        expect.stringContaining('shutdown')
      );
    });

    it('should handle stop() when server not started', async () => {
      // Act & Assert - should not throw
      await expect(server.stop()).resolves.toBeUndefined();
    });
  });

  describe('Authentication', () => {
    it('should authenticate peer with valid shared secret', async () => {
      // Arrange
      const peerId = 'connector-b';
      const secret = 'shared-secret-123';
      process.env[`BTP_PEER_${peerId.toUpperCase().replace(/-/g, '_')}_SECRET`] = secret;

      await server.start(AUTO_PORT);

      // Simulate connection
      const mockWs = new MockWebSocket();
      const authMessage = createAuthMessage(peerId, secret);
      const authBuffer = serializeBTPMessage(authMessage);

      // Get the WebSocket server instance via reflection
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      // Act - simulate connection and auth message
      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });
      mockWs.emit('message', authBuffer);

      // Wait for async processing
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_auth',
          peerId,
          success: true,
        }),
        expect.stringContaining('authenticated')
      );

      // Verify RESPONSE sent
      expect(mockWs.sentMessages.length).toBeGreaterThan(0);
      const response = deserializeBTPMessage(mockWs.sentMessages[0]!);
      expect(response.type).toBe(BTPMessageType.RESPONSE);
    });

    it('should reject authentication with invalid secret', async () => {
      // Arrange
      const peerId = 'connector-b';
      process.env['BTP_PEER_CONNECTOR_B_SECRET'] = 'correct-secret';

      await server.start(AUTO_PORT);

      const mockWs = new MockWebSocket();
      const authMessage = createAuthMessage(peerId, 'wrong-secret');
      const authBuffer = serializeBTPMessage(authMessage);

      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      // Act
      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });
      mockWs.emit('message', authBuffer);

      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_auth',
          peerId,
          success: false,
        }),
        expect.stringContaining('failed')
      );
    });

    it('should reject authentication for unconfigured peer', async () => {
      // Arrange
      await server.start(AUTO_PORT);

      const mockWs = new MockWebSocket();
      const authMessage = createAuthMessage('unknown-peer', 'some-secret');
      const authBuffer = serializeBTPMessage(authMessage);

      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      // Act
      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });
      mockWs.emit('message', authBuffer);

      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_auth',
          success: false,
          reason: 'no configured secret for peer',
        }),
        expect.any(String)
      );
    });

    it('should accept no-auth connection by default (permissionless network)', async () => {
      // Arrange
      const peerId = 'connector-noauth';
      // Do not set BTP_ALLOW_NOAUTH - defaults to true (permissionless)

      await server.start(AUTO_PORT);

      const mockWs = new MockWebSocket();
      const authMessage = createAuthMessage(peerId, ''); // Empty secret per RFC-0023
      const authBuffer = serializeBTPMessage(authMessage);

      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      // Act
      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });
      mockWs.emit('message', authBuffer);

      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_auth',
          peerId,
          success: true,
          mode: 'no-auth',
        }),
        expect.stringContaining('no-auth mode')
      );

      // Verify RESPONSE sent
      expect(mockWs.sentMessages.length).toBeGreaterThan(0);
      const response = deserializeBTPMessage(mockWs.sentMessages[0]!);
      expect(response.type).toBe(BTPMessageType.RESPONSE);

      // Verify peer is authenticated
      expect(server.hasPeer(peerId)).toBe(true);
    });

    it('should reject no-auth connection when BTP_ALLOW_NOAUTH=false (private network mode)', async () => {
      // Arrange
      const peerId = 'connector-noauth-rejected';
      process.env['BTP_ALLOW_NOAUTH'] = 'false'; // Explicitly disable for private networks

      await server.start(AUTO_PORT);

      const mockWs = new MockWebSocket();
      const authMessage = createAuthMessage(peerId, ''); // Empty secret
      const authBuffer = serializeBTPMessage(authMessage);

      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      // Act
      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });
      mockWs.emit('message', authBuffer);

      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_auth',
          peerId,
          success: false,
          reason: 'no-auth disabled',
        }),
        expect.stringContaining('no-auth mode disabled')
      );

      // Verify peer is NOT authenticated
      expect(server.hasPeer(peerId)).toBe(false);
    });
  });

  describe('Message Handling', () => {
    it('should process BTP MESSAGE with ILP packet and return FULFILL', async () => {
      // Arrange
      const peerId = 'connector-c';
      const secret = 'test-secret';
      process.env['BTP_PEER_CONNECTOR_C_SECRET'] = secret;

      await server.start(AUTO_PORT);

      const mockWs = new MockWebSocket();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      // Authenticate first
      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });
      const authBuffer = serializeBTPMessage(createAuthMessage(peerId, secret));
      mockWs.emit('message', authBuffer);
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Setup PacketHandler to return FULFILL
      const fulfillPacket = createValidFulfillPacket();
      mockPacketHandler.handlePreparePacket.mockResolvedValue(fulfillPacket);

      // Create BTP MESSAGE with ILP packet
      const preparePacket = createValidPreparePacket();
      const ilpBuffer = serializePacket(preparePacket);
      const btpMessage = createBTPMessage(ilpBuffer, 2);
      const messageBuffer = serializeBTPMessage(btpMessage);

      // Clear previous messages
      mockWs.sentMessages = [];

      // Act
      mockWs.emit('message', messageBuffer);
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockPacketHandler.handlePreparePacket).toHaveBeenCalled();
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_response_sent',
          peerId,
          responseType: 'FULFILL',
          requestId: 2,
        }),
        expect.any(String)
      );

      // Verify RESPONSE contains FULFILL packet
      const lastMessage = mockWs.sentMessages[mockWs.sentMessages.length - 1];
      expect(lastMessage).toBeDefined();
      const response = deserializeBTPMessage(lastMessage!);
      expect(response.type).toBe(BTPMessageType.RESPONSE);
      expect(response.requestId).toBe(2);
    });

    it('should process BTP MESSAGE and return REJECT on packet error', async () => {
      // Arrange
      const peerId = 'connector-d';
      const secret = 'test-secret-2';
      process.env['BTP_PEER_CONNECTOR_D_SECRET'] = secret;

      await server.start(AUTO_PORT);

      const mockWs = new MockWebSocket();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      // Authenticate
      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });
      mockWs.emit('message', serializeBTPMessage(createAuthMessage(peerId, secret)));
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Setup PacketHandler to return REJECT
      const rejectPacket = createValidRejectPacket();
      mockPacketHandler.handlePreparePacket.mockResolvedValue(rejectPacket);

      // Send BTP MESSAGE
      const preparePacket = createValidPreparePacket();
      const ilpBuffer = serializePacket(preparePacket);
      const btpMessage = createBTPMessage(ilpBuffer, 3);

      mockWs.sentMessages = [];

      // Act
      mockWs.emit('message', serializeBTPMessage(btpMessage));
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_response_sent',
          responseType: 'REJECT',
          requestId: 3,
        }),
        expect.any(String)
      );
    });

    it('should handle BTP MESSAGE without ILP packet as protocol-data message', async () => {
      // Arrange
      const peerId = 'connector-e';
      const secret = 'test-secret-3';
      process.env['BTP_PEER_CONNECTOR_E_SECRET'] = secret;

      await server.start(AUTO_PORT);

      const mockWs = new MockWebSocket();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      // Authenticate
      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });
      mockWs.emit('message', serializeBTPMessage(createAuthMessage(peerId, secret)));
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Send MESSAGE without ILP packet (protocol-data only, like payment channel claims)
      const protocolDataMessage: BTPMessage = {
        type: BTPMessageType.MESSAGE,
        requestId: 4,
        data: {
          protocolData: [],
          // No ilpPacket - valid for protocol-data-only messages
        },
      };

      mockWs.sentMessages = [];

      // Act
      mockWs.emit('message', serializeBTPMessage(protocolDataMessage));
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert - should log debug message, not error (protocol-data messages are valid)
      expect(mockLogger.debug).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_protocol_data_received',
          peerId,
        }),
        expect.any(String)
      );
    });
  });

  describe('Event Handlers', () => {
    it.skip('should call onConnection callback when peer authenticates', async () => {
      // Arrange
      const peerId = 'connector-f';
      const secret = 'callback-secret';
      process.env['BTP_PEER_CONNECTOR_F_SECRET'] = secret;

      await server.start(AUTO_PORT);

      const connectionCallback = jest.fn();
      server.onConnection(connectionCallback);

      const mockWs = new MockWebSocket();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      // Act
      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });
      mockWs.emit('message', serializeBTPMessage(createAuthMessage(peerId, secret)));
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(connectionCallback).toHaveBeenCalledWith(peerId, expect.any(Object));
    });

    it.skip('should call onMessage callback when BTP message received', async () => {
      // Arrange
      const peerId = 'connector-g';
      const secret = 'message-callback-secret';
      process.env['BTP_PEER_CONNECTOR_G_SECRET'] = secret;

      await server.start(AUTO_PORT);

      const messageCallback = jest.fn();
      server.onMessage(messageCallback);

      const mockWs = new MockWebSocket();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      // Authenticate
      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });
      mockWs.emit('message', serializeBTPMessage(createAuthMessage(peerId, secret)));
      await new Promise((resolve) => setTimeout(resolve, 50));

      mockPacketHandler.handlePreparePacket.mockResolvedValue(createValidFulfillPacket());

      // Send MESSAGE
      const preparePacket = createValidPreparePacket();
      const btpMessage = createBTPMessage(serializePacket(preparePacket), 5);

      // Act
      mockWs.emit('message', serializeBTPMessage(btpMessage));
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(messageCallback).toHaveBeenCalledWith(
        peerId,
        expect.objectContaining({
          type: BTPMessageType.MESSAGE,
          requestId: 5,
        })
      );
    });
  });

  describe('Connection Lifecycle', () => {
    it('should log connection events with remote address', async () => {
      // Arrange
      await server.start(AUTO_PORT);

      const mockWs = new MockWebSocket();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      // Act
      wss.emit('connection', mockWs, {
        socket: { remoteAddress: '192.168.1.100', remotePort: 54321 },
      });

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_connection',
          remoteAddress: '192.168.1.100',
        }),
        expect.stringContaining('connection established')
      );
    });

    it('should log disconnect events when connection closes', async () => {
      // Arrange
      await server.start(AUTO_PORT);

      const mockWs = new MockWebSocket();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });

      // Act
      mockWs.close(1000, 'Normal closure');
      await new Promise((resolve) => setTimeout(resolve, 10));

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_disconnect',
        }),
        expect.stringContaining('closed')
      );
    });
  });

  describe('Error Handling', () => {
    it.skip('should send BTP ERROR response on malformed message', async () => {
      // Arrange
      await server.start(AUTO_PORT);

      const mockWs = new MockWebSocket();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });

      // Send malformed BTP message
      const malformedBuffer = Buffer.from([1, 2, 3]); // Too short

      // Act
      mockWs.emit('message', malformedBuffer);
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_message_error',
        }),
        expect.stringContaining('Error handling BTP message')
      );

      // Verify ERROR response sent
      expect(mockWs.sentMessages.length).toBeGreaterThan(0);
    });

    it.skip('should handle WebSocket connection error event', async () => {
      // Arrange
      const peerId = 'connector-error';
      const secret = 'error-secret';
      process.env['BTP_PEER_CONNECTOR_ERROR_SECRET'] = secret;

      await server.start(AUTO_PORT);

      const mockWs = new MockWebSocket();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      // Authenticate peer first
      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });
      mockWs.emit('message', serializeBTPMessage(createAuthMessage(peerId, secret)));
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Act - emit error event on WebSocket
      const wsError = new Error('WebSocket connection failed');
      mockWs.emit('error', wsError);
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert
      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_connection_error',
          peerId,
          error: 'WebSocket connection failed',
        }),
        expect.stringContaining('BTP connection error')
      );
    });

    it.skip('should handle error when closing peer connection during shutdown', async () => {
      // Arrange
      const peerId = 'connector-shutdown-error';
      const secret = 'shutdown-error-secret';
      process.env['BTP_PEER_CONNECTOR_SHUTDOWN_ERROR_SECRET'] = secret;

      await server.start(AUTO_PORT);

      // Create mock WebSocket that throws error on close
      class ErrorWebSocket extends MockWebSocket {
        close(): void {
          throw new Error('Connection close failed');
        }
      }

      const mockWs = new ErrorWebSocket();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      // Authenticate peer
      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });
      mockWs.emit('message', serializeBTPMessage(createAuthMessage(peerId, secret)));
      await new Promise((resolve) => setTimeout(resolve, 50));

      jest.clearAllMocks();

      // Act - stop server which should attempt to close connections
      await server.stop();

      // Assert - should log warning about failed connection close
      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_connection_close_failed',
          peerId,
          error: 'Connection close failed',
        }),
        expect.stringContaining('Failed to close peer connection')
      );
    });

    it('should handle error when sending BTP ERROR response fails', async () => {
      // Arrange
      await server.start(AUTO_PORT);

      // Create mock WebSocket that throws error on send
      class ErrorSendWebSocket extends MockWebSocket {
        send(): void {
          throw new Error('Send failed');
        }
      }

      const mockWs = new ErrorSendWebSocket();
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const wss = (server as any).wss as WebSocketServer;

      wss.emit('connection', mockWs, { socket: { remoteAddress: '127.0.0.1', remotePort: 12345 } });

      // Send malformed message
      const malformedBuffer = Buffer.from([1, 2, 3]);

      // Act
      mockWs.emit('message', malformedBuffer);
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Assert - should log error about failed ERROR response
      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_error_response_failed',
        }),
        expect.stringContaining('Failed to send BTP ERROR response')
      );
    });
  });
});

/**
 * Helper to deserialize BTP message from buffer
 */
function deserializeBTPMessage(buffer: Buffer): BTPMessage {
  // Simple deserialization for testing
  const type = buffer.readUInt8(0);
  const requestId = buffer.readUInt32BE(1);
  return {
    type: type as BTPMessageType,
    requestId,
    data: { protocolData: [] },
  };
}
