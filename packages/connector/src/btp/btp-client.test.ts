/**
 * Unit tests for BTPClient
 * Tests BTP WebSocket client functionality per RFC-0023
 */

import { BTPClient, Peer, BTPConnectionError, BTPAuthenticationError } from './btp-client';
import { Logger } from '../utils/logger';
import { BTPMessage, BTPMessageType, BTPData, BTPErrorData } from './btp-types';
import { serializeBTPMessage, parseBTPMessage } from './btp-message-parser';
import {
  ILPPreparePacket,
  ILPFulfillPacket,
  ILPRejectPacket,
  PacketType,
  ILPErrorCode,
  serializePacket,
} from '@toon-protocol/shared';
import WebSocket from 'ws';
import { EventEmitter } from 'events';

// Mock the 'ws' module
jest.mock('ws', () => {
  return jest.fn();
});

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
 * Create mock Peer configuration
 */
const createTestPeer = (id = 'connectorB', url = 'ws://localhost:3000'): Peer => ({
  id,
  url,
  authToken: 'shared-secret-123',
  connected: false,
  lastSeen: new Date(),
});

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
 * Create BTP auth response message
 */
const createAuthResponse = (requestId: number): BTPMessage => ({
  type: BTPMessageType.RESPONSE,
  requestId,
  data: {
    protocolData: [],
  } as BTPData,
});

/**
 * Create BTP error response message
 */
const createErrorResponse = (
  requestId: number,
  code = 'F00',
  errorMessage = 'Test error'
): BTPMessage => ({
  type: BTPMessageType.ERROR,
  requestId,
  data: {
    code,
    name: errorMessage,
    triggeredAt: new Date().toISOString(),
    data: Buffer.alloc(0),
  } as BTPErrorData,
});

/**
 * Create BTP MESSAGE response with ILP packet
 */
const createPacketResponse = (requestId: number, ilpPacket: Buffer): BTPMessage => ({
  type: BTPMessageType.RESPONSE,
  requestId,
  data: {
    protocolData: [],
    ilpPacket,
  } as BTPData,
});

/**
 * Mock WebSocket for testing
 */
class MockWebSocket extends EventEmitter {
  public readyState: number = WebSocket.CONNECTING;
  public sentMessages: Buffer[] = [];
  public url: string;
  private _openTimer: NodeJS.Immediate | null = null;

  constructor(url: string, autoOpen = false) {
    super();
    this.url = url;
    // Only auto-open if explicitly requested
    if (autoOpen) {
      this._openTimer = setImmediate(() => this.simulateOpen());
    }
  }

  // Helper to simulate connection opening
  simulateOpen(): void {
    this.readyState = WebSocket.OPEN;
    this.emit('open');
  }

  send(data: Buffer): void {
    this.sentMessages.push(data);
  }

  close(): void {
    if (this._openTimer) {
      clearImmediate(this._openTimer);
    }
    this.readyState = WebSocket.CLOSED;
    this.emit('close');
  }

  ping(): void {
    // Simulate ping
    this.emit('ping');
  }

  // Helper to simulate receiving a message
  simulateMessage(data: Buffer): void {
    this.emit('message', data);
  }

  // Helper to simulate pong
  simulatePong(): void {
    this.emit('pong');
  }
}

describe('BTPClient', () => {
  let client: BTPClient;
  let mockLogger: jest.Mocked<Logger>;
  let mockPeer: Peer;
  let mockWs: MockWebSocket;

  /**
   * Helper function to simulate a successful connection and authentication
   */
  async function simulateSuccessfulConnection(): Promise<void> {
    const connectPromise = client.connect();

    // Wait a tick for WebSocket to be created
    await new Promise((resolve) => setImmediate(resolve));

    // Simulate WebSocket opening
    mockWs.simulateOpen();

    // Wait for async open handler to start and send auth message
    await new Promise((resolve) => setImmediate(resolve));

    // Send auth response
    const authMsg = parseBTPMessage(mockWs.sentMessages[0]!);
    const authResponse = createAuthResponse(authMsg.requestId);
    mockWs.simulateMessage(serializeBTPMessage(authResponse));

    await connectPromise;
    mockWs.sentMessages = []; // Clear auth messages
  }

  beforeEach(() => {
    mockLogger = createMockLogger();
    mockPeer = createTestPeer('connectorB', 'ws://localhost:3000');

    // Pre-create the mock WebSocket instance
    mockWs = new MockWebSocket('ws://localhost:3000');

    // Mock WebSocket constructor to return our pre-created instance
    (WebSocket as unknown as jest.Mock).mockImplementation((url: string) => {
      // Reset state and update URL on the existing instance
      mockWs.url = url;
      mockWs.sentMessages = [];
      mockWs.readyState = WebSocket.CONNECTING;
      mockWs.removeAllListeners();
      return mockWs;
    });

    client = new BTPClient(mockPeer, 'test-node', mockLogger);
    jest.clearAllMocks();
  });

  afterEach(async () => {
    try {
      if (client.isConnected) {
        await client.disconnect();
      }
    } catch (error) {
      // Ignore disconnect errors in cleanup
    }
  });

  describe('Connection Management', () => {
    it('should connect to peer and authenticate with shared secret', async () => {
      // Arrange
      const connectPromise = client.connect();

      // Wait for WebSocket to be created
      await new Promise((resolve) => setImmediate(resolve));

      // Simulate WebSocket opening
      mockWs.simulateOpen();

      // Wait for auth message to be sent
      await new Promise((resolve) => setImmediate(resolve));

      expect(mockWs.sentMessages.length).toBe(1);
      const authMsg = parseBTPMessage(mockWs.sentMessages[0]!);
      const authResponse = createAuthResponse(authMsg.requestId);
      mockWs.simulateMessage(serializeBTPMessage(authResponse));

      // Act
      await connectPromise;

      // Assert
      expect(client.isConnected).toBe(true);
      expect(mockPeer.connected).toBe(true);
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({ event: 'btp_connected' }),
        expect.any(String)
      );
    });

    it('should throw error on authentication failure', async () => {
      // Arrange
      const connectPromise = client.connect();

      // Wait for WebSocket to be created
      await new Promise((resolve) => setImmediate(resolve));

      // Simulate WebSocket opening
      mockWs.simulateOpen();

      // Wait for auth message to be sent
      await new Promise((resolve) => setImmediate(resolve));

      const authMsg = parseBTPMessage(mockWs.sentMessages[0]!);
      const errorResponse = createErrorResponse(authMsg.requestId, 'F00', 'Invalid auth');
      mockWs.simulateMessage(serializeBTPMessage(errorResponse));

      // Act & Assert
      await expect(connectPromise).rejects.toThrow(BTPAuthenticationError);
      expect(client.isConnected).toBe(false);
    });

    it('should throw error on authentication timeout', async () => {
      // Arrange
      jest.useFakeTimers();
      const connectPromise = client.connect();

      // Simulate WebSocket opening but no auth response
      mockWs.simulateOpen();

      // Act - advance time beyond timeout
      jest.advanceTimersByTime(6000);

      // Assert
      await expect(connectPromise).rejects.toThrow(BTPAuthenticationError);
      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({ event: 'btp_auth_failed', reason: 'timeout' }),
        expect.any(String)
      );

      jest.useRealTimers();
    });

    it('should support no-auth connection with empty auth token (RFC-0023)', async () => {
      // Arrange - Create peer with empty auth token
      const noAuthPeer: Peer = {
        id: 'connector-noauth',
        url: 'ws://localhost:3000',
        authToken: '', // Empty string per RFC-0023
        connected: false,
        lastSeen: new Date(),
      };

      const noAuthClient = new BTPClient(noAuthPeer, 'test-node', mockLogger);

      // Replace WebSocket constructor to use our mock
      (WebSocket as unknown as jest.Mock).mockImplementation((url: string) => {
        mockWs = new MockWebSocket(url);
        return mockWs;
      });

      const connectPromise = noAuthClient.connect();

      // Wait for WebSocket to be created
      await new Promise((resolve) => setImmediate(resolve));

      // Simulate WebSocket opening
      mockWs.simulateOpen();

      // Wait for auth message to be sent
      await new Promise((resolve) => setImmediate(resolve));

      // Verify auth message contains empty secret
      expect(mockWs.sentMessages.length).toBe(1);
      const authMsg = parseBTPMessage(mockWs.sentMessages[0]!);
      expect(authMsg.type).toBe(BTPMessageType.MESSAGE);

      // Decode auth data to verify empty secret
      const authData = JSON.parse((authMsg.data as BTPData).protocolData[0]!.data.toString('utf8'));
      expect(authData.peerId).toBe('test-node');
      expect(authData.secret).toBe(''); // Empty string

      // Simulate server accepting no-auth connection
      const authResponse = createAuthResponse(authMsg.requestId);
      mockWs.simulateMessage(serializeBTPMessage(authResponse));

      // Act
      await connectPromise;

      // Assert
      expect(noAuthClient.isConnected).toBe(true);
      expect(noAuthPeer.connected).toBe(true);

      // Cleanup
      await noAuthClient.disconnect();
    });

    it('should disconnect gracefully', async () => {
      // Arrange
      await simulateSuccessfulConnection();

      // Act
      await client.disconnect();

      // Assert
      expect(client.isConnected).toBe(false);
      expect(mockPeer.connected).toBe(false);
    });

    it('should emit connected event on successful connection', async () => {
      // Arrange
      const connectedHandler = jest.fn();
      client.on('connected', connectedHandler);

      // Act
      await simulateSuccessfulConnection();

      // Assert
      expect(connectedHandler).toHaveBeenCalled();
    });

    it('should emit disconnected event on connection close', async () => {
      // Arrange
      await simulateSuccessfulConnection();

      const disconnectedHandler = jest.fn();
      client.on('disconnected', disconnectedHandler);

      // Act
      mockWs.close();
      await new Promise((resolve) => setImmediate(resolve));

      // Assert
      expect(disconnectedHandler).toHaveBeenCalled();
    });

    it('should emit error event on connection error', async () => {
      // Arrange
      const errorHandler = jest.fn();
      client.on('error', errorHandler);

      const connectPromise = client.connect();

      // Simulate error
      setImmediate(() => {
        mockWs.emit('error', new Error('Connection failed'));
      });

      // Act & Assert
      await expect(connectPromise).rejects.toThrow();
      expect(errorHandler).toHaveBeenCalled();
    });
  });

  describe('Packet Sending', () => {
    beforeEach(async () => {
      await simulateSuccessfulConnection();
    });

    it('should send ILP Prepare packet wrapped in BTP MESSAGE', async () => {
      // Arrange
      const preparePacket = createValidPreparePacket();
      const fulfillPacket = createValidFulfillPacket();

      const sendPromise = client.sendPacket(preparePacket);

      // Simulate response
      setImmediate(() => {
        const btpMsg = parseBTPMessage(mockWs.sentMessages[0]!);
        const response = createPacketResponse(btpMsg.requestId, serializePacket(fulfillPacket));
        mockWs.simulateMessage(serializeBTPMessage(response));
      });

      // Act
      const result = await sendPromise;

      // Assert
      expect(mockWs.sentMessages.length).toBe(1);
      expect(result.type).toBe(PacketType.FULFILL);
      expect(mockLogger.debug).toHaveBeenCalledWith(
        expect.objectContaining({ event: 'btp_message_sent' }),
        expect.any(String)
      );
    });

    it('should handle ILP Reject response', async () => {
      // Arrange
      const preparePacket = createValidPreparePacket();
      const rejectPacket = createValidRejectPacket();

      const sendPromise = client.sendPacket(preparePacket);

      // Simulate reject response
      setImmediate(() => {
        const btpMsg = parseBTPMessage(mockWs.sentMessages[0]!);
        const response = createPacketResponse(btpMsg.requestId, serializePacket(rejectPacket));
        mockWs.simulateMessage(serializeBTPMessage(response));
      });

      // Act
      const result = await sendPromise;

      // Assert
      expect(result.type).toBe(PacketType.REJECT);
    });

    it('should throw error when not connected', async () => {
      // Arrange
      await client.disconnect();
      const preparePacket = createValidPreparePacket();

      // Act & Assert
      await expect(client.sendPacket(preparePacket)).rejects.toThrow(BTPConnectionError);
    });

    it('should timeout if no response received', async () => {
      // Arrange
      jest.useFakeTimers();
      const preparePacket = createValidPreparePacket();

      const sendPromise = client.sendPacket(preparePacket);

      // Act - advance time beyond timeout (10 seconds)
      jest.advanceTimersByTime(11000);

      // Assert
      await expect(sendPromise).rejects.toThrow('Packet send timeout');

      jest.useRealTimers();
    });

    it('should use unique request IDs for each packet', async () => {
      // Arrange
      const preparePacket1 = createValidPreparePacket();
      const preparePacket2 = createValidPreparePacket();
      const fulfillPacket = createValidFulfillPacket();

      // Act
      const send1 = client.sendPacket(preparePacket1);
      const send2 = client.sendPacket(preparePacket2);

      setImmediate(() => {
        const msg1 = parseBTPMessage(mockWs.sentMessages[0]!);
        const msg2 = parseBTPMessage(mockWs.sentMessages[1]!);

        mockWs.simulateMessage(
          serializeBTPMessage(createPacketResponse(msg1.requestId, serializePacket(fulfillPacket)))
        );
        mockWs.simulateMessage(
          serializeBTPMessage(createPacketResponse(msg2.requestId, serializePacket(fulfillPacket)))
        );
      });

      await Promise.all([send1, send2]);

      // Assert
      const msg1 = parseBTPMessage(mockWs.sentMessages[0]!);
      const msg2 = parseBTPMessage(mockWs.sentMessages[1]!);
      expect(msg1.requestId).not.toBe(msg2.requestId);
    });
  });

  describe.skip('Retry Logic', () => {
    it('should retry connection with exponential backoff after failure', async () => {
      // Arrange
      jest.useFakeTimers();
      let connectAttempts = 0;

      jest.spyOn(global, 'WebSocket' as never).mockImplementation((() => {
        connectAttempts++;
        if (connectAttempts < 3) {
          // Fail first 2 attempts
          const failWs = new EventEmitter() as MockWebSocket;
          setImmediate(() => failWs.emit('error', new Error('Connection refused')));
          return failWs;
        } else {
          // Succeed on 3rd attempt
          mockWs = new MockWebSocket('ws://localhost:3000');
          return mockWs;
        }
      }) as never);

      const newClient = new BTPClient(mockPeer, 'test-node', mockLogger);

      // Act - first attempt fails
      newClient.connect().catch(() => {
        // Ignore initial connection error
      });
      await jest.runAllTimersAsync();

      // Verify first retry (1s backoff)
      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({ event: 'btp_retry', retryCount: 1, backoffMs: 1000 }),
        expect.any(String)
      );

      // Advance through retries
      jest.advanceTimersByTime(2000); // Second retry (2s backoff)
      await jest.runAllTimersAsync();

      // Simulate successful auth on third attempt
      if (mockWs) {
        const authMsg = parseBTPMessage(mockWs.sentMessages[0]!);
        mockWs.simulateMessage(serializeBTPMessage(createAuthResponse(authMsg.requestId)));
      }

      await jest.runAllTimersAsync();

      // Assert
      expect(connectAttempts).toBeGreaterThanOrEqual(2);

      jest.useRealTimers();
      await newClient.disconnect();
    });

    it('should throw error after max retries exceeded', async () => {
      // Arrange
      jest.useFakeTimers();

      jest.spyOn(global, 'WebSocket' as never).mockImplementation((() => {
        const failWs = new EventEmitter() as MockWebSocket;
        setImmediate(() => failWs.emit('error', new Error('Connection refused')));
        return failWs;
      }) as never);

      const newClient = new BTPClient(mockPeer, 'test-node', mockLogger, 2); // Max 2 retries

      // Act
      newClient.connect().catch(() => {
        // Ignore initial error
      });

      // Advance through all retries
      for (let i = 0; i < 5; i++) {
        await jest.runAllTimersAsync();
        jest.advanceTimersByTime(20000); // Advance past all backoffs
      }

      // Assert
      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({ event: 'btp_max_retries' }),
        expect.any(String)
      );

      jest.useRealTimers();
      await newClient.disconnect();
    });

    it('should reset retry count on successful connection', async () => {
      // Arrange
      await simulateSuccessfulConnection();

      // Simulate disconnect and reconnect
      mockWs.close();
      await new Promise((resolve) => setImmediate(resolve));

      // Mock successful reconnect
      const newMockWs = new MockWebSocket('ws://localhost:3000');
      jest.spyOn(global, 'WebSocket' as never).mockImplementation(((url: string) => {
        newMockWs.url = url;
        return newMockWs;
      }) as never);

      // Trigger reconnect manually
      const reconnectPromise = client.connect();
      setImmediate(() => {
        newMockWs.simulateOpen();
        setImmediate(() => {
          const authMsg = parseBTPMessage(newMockWs.sentMessages[0]!);
          newMockWs.simulateMessage(serializeBTPMessage(createAuthResponse(authMsg.requestId)));
        });
      });
      await reconnectPromise;

      // Assert - should not have excessive retry count
      expect(client.isConnected).toBe(true);
    });
  });

  describe.skip('Keep-Alive (Ping/Pong)', () => {
    beforeEach(async () => {
      await simulateSuccessfulConnection();
    });

    it('should send ping every 30 seconds when connected', async () => {
      // Arrange
      jest.useFakeTimers();
      const pingSpy = jest.spyOn(mockWs, 'ping');

      // Act - advance time
      jest.advanceTimersByTime(30000);

      // Assert
      expect(pingSpy).toHaveBeenCalled();
      expect(mockLogger.debug).toHaveBeenCalledWith(
        expect.objectContaining({ event: 'btp_ping_sent' }),
        expect.any(String)
      );

      jest.useRealTimers();
    });

    it('should handle pong response', async () => {
      // Arrange
      jest.useFakeTimers();

      // Act - send ping and receive pong
      jest.advanceTimersByTime(30000);
      mockWs.simulatePong();

      // Assert
      expect(mockLogger.debug).toHaveBeenCalledWith(
        expect.objectContaining({ event: 'btp_pong_received' }),
        expect.any(String)
      );

      jest.useRealTimers();
    });

    it('should reconnect if pong timeout occurs', async () => {
      // Arrange
      jest.useFakeTimers();
      const closeSpy = jest.spyOn(mockWs, 'close');

      // Act - send ping but don't respond with pong
      jest.advanceTimersByTime(30000); // Send ping
      jest.advanceTimersByTime(10000); // Pong timeout

      // Assert
      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({ event: 'btp_pong_timeout' }),
        expect.any(String)
      );
      expect(closeSpy).toHaveBeenCalled();

      jest.useRealTimers();
    });

    it('should clear ping interval on disconnect', async () => {
      // Arrange
      jest.useFakeTimers();

      // Act
      await client.disconnect();
      jest.advanceTimersByTime(30000);

      // Assert - ping should not be sent after disconnect
      const pingSpy = jest.spyOn(mockWs, 'ping');
      expect(pingSpy).not.toHaveBeenCalled();

      jest.useRealTimers();
    });
  });

  describe.skip('Logging', () => {
    it('should log connection attempt with peer ID and URL', async () => {
      // Arrange & Act
      await simulateSuccessfulConnection();

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_connection_attempt',
          url: 'ws://localhost:3000',
        }),
        expect.any(String)
      );
    });

    it('should never log authToken value', async () => {
      // Arrange & Act
      await simulateSuccessfulConnection();

      // Assert - check all logger calls don't contain authToken
      const allLogCalls = [
        ...mockLogger.info.mock.calls,
        ...mockLogger.debug.mock.calls,
        ...mockLogger.warn.mock.calls,
        ...mockLogger.error.mock.calls,
      ];

      allLogCalls.forEach((call) => {
        const logData = JSON.stringify(call);
        expect(logData).not.toContain('shared-secret-123');
      });
    });

    it('should include peer ID in all log entries', async () => {
      // Assert - child logger created with peerId
      expect(mockLogger.child).toHaveBeenCalledWith({ peerId: 'connectorB' });
    });

    it('should log authentication events at appropriate levels', async () => {
      // Arrange & Act
      await simulateSuccessfulConnection();

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({ event: 'btp_auth_attempt' }),
        expect.any(String)
      );
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({ event: 'btp_auth_success' }),
        expect.any(String)
      );
    });

    it('should log packet send at DEBUG level with request ID', async () => {
      // Arrange
      await simulateSuccessfulConnection();

      const preparePacket = createValidPreparePacket();
      const sendPromise = client.sendPacket(preparePacket);
      setImmediate(() => {
        const btpMsg = parseBTPMessage(mockWs.sentMessages[mockWs.sentMessages.length - 1]!);
        mockWs.simulateMessage(
          serializeBTPMessage(
            createPacketResponse(btpMsg.requestId, serializePacket(createValidFulfillPacket()))
          )
        );
      });

      // Act
      await sendPromise;

      // Assert
      expect(mockLogger.debug).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_message_sent',
          requestId: expect.any(Number),
          packetType: PacketType.PREPARE,
        }),
        expect.any(String)
      );
    });
  });

  describe.skip('Connection State', () => {
    it('should maintain connection state correctly', async () => {
      // Initial state
      expect(client.isConnected).toBe(false);

      // Connecting
      const connectPromise = client.connect();
      expect(client.isConnected).toBe(false); // Still not connected until auth

      // Connected
      setImmediate(() => {
        mockWs.simulateOpen();
        setImmediate(() => {
          const authMsg = parseBTPMessage(mockWs.sentMessages[0]!);
          mockWs.simulateMessage(serializeBTPMessage(createAuthResponse(authMsg.requestId)));
        });
      });
      await connectPromise;
      expect(client.isConnected).toBe(true);

      // Disconnected
      await client.disconnect();
      expect(client.isConnected).toBe(false);
    });

    it('should update peer lastSeen on successful message', async () => {
      // Arrange
      await simulateSuccessfulConnection();

      const beforeTime = mockPeer.lastSeen;

      // Small delay
      await new Promise((resolve) => setTimeout(resolve, 10));

      // Act - send packet and receive response
      const preparePacket = createValidPreparePacket();
      const sendPromise = client.sendPacket(preparePacket);
      setImmediate(() => {
        const btpMsg = parseBTPMessage(mockWs.sentMessages[mockWs.sentMessages.length - 1]!);
        mockWs.simulateMessage(
          serializeBTPMessage(
            createPacketResponse(btpMsg.requestId, serializePacket(createValidFulfillPacket()))
          )
        );
      });
      await sendPromise;

      // Assert
      expect(mockPeer.lastSeen.getTime()).toBeGreaterThanOrEqual(beforeTime.getTime());
    });
  });

  describe.skip('Error Handling', () => {
    it('should handle malformed BTP messages gracefully', async () => {
      // Arrange
      await simulateSuccessfulConnection();

      // Act - send malformed message
      mockWs.simulateMessage(Buffer.from('invalid data'));

      // Assert - should log error but not crash
      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({ event: 'btp_message_parse_error' }),
        expect.any(String)
      );
    });

    it('should reject pending requests on disconnect', async () => {
      // Arrange
      await simulateSuccessfulConnection();

      const preparePacket = createValidPreparePacket();
      const sendPromise = client.sendPacket(preparePacket);

      // Act - disconnect before response
      await client.disconnect();

      // Assert
      await expect(sendPromise).rejects.toThrow('Connection closed');
    });

    it('should handle WebSocket send errors', async () => {
      // Arrange
      await simulateSuccessfulConnection();

      // Mock send to throw error
      jest.spyOn(mockWs, 'send').mockImplementation(() => {
        throw new Error('Send failed');
      });

      const preparePacket = createValidPreparePacket();

      // Act & Assert
      await expect(client.sendPacket(preparePacket)).rejects.toThrow(BTPConnectionError);
    });
  });
});
