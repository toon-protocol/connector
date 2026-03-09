/**
 * BTP WebSocket Client
 * Implements RFC-0023 Bilateral Transfer Protocol client for outbound peer connections
 */

import WebSocket from 'ws';
import { EventEmitter } from 'events';
import { Logger } from '../utils/logger';
import {
  BTPMessage,
  BTPMessageType,
  BTPData,
  BTPError,
  isBTPData,
  isBTPErrorData,
} from './btp-types';
import { parseBTPMessage, serializeBTPMessage } from './btp-message-parser';
import { ILPPreparePacket, ILPFulfillPacket, ILPRejectPacket, PacketType } from '@crosstown/shared';
import { serializePacket, deserializePacket } from '@crosstown/shared';
import type { PacketHandler } from '../core/packet-handler';

/**
 * Peer configuration for BTP connections
 */
export interface Peer {
  /** Unique peer identifier */
  id: string;
  /** WebSocket URL for BTP connection (e.g., "ws://connector-b:3000") */
  url: string;
  /** Shared secret for BTP authentication */
  authToken: string;
  /** Current connection state */
  connected: boolean;
  /** Timestamp of last successful communication */
  lastSeen: Date;
}

/**
 * Connection state type
 */
type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'error';

/**
 * BTP connection error
 */
export class BTPConnectionError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'BTPConnectionError';
    Error.captureStackTrace(this, BTPConnectionError);
  }
}

/**
 * BTP authentication error
 */
export class BTPAuthenticationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'BTPAuthenticationError';
    Error.captureStackTrace(this, BTPAuthenticationError);
  }
}

/**
 * Pending request tracking
 */
interface PendingRequest {
  resolve: (packet: ILPFulfillPacket | ILPRejectPacket) => void;
  reject: (error: Error) => void;
  timeoutId: NodeJS.Timeout;
}

/**
 * BTPClient - WebSocket client for BTP protocol
 * Initiates outbound BTP connections to peer connectors
 */
export class BTPClient extends EventEmitter {
  private readonly _peer: Peer;
  private readonly _logger: Logger;
  private _ws: WebSocket | null = null;
  private _connectionState: ConnectionState = 'disconnected';
  private _retryCount = 0;
  private _maxRetries = 5;
  private _requestIdCounter = 0;
  private _pendingRequests: Map<number, PendingRequest> = new Map();
  private _pingInterval: NodeJS.Timeout | null = null;
  private _pongTimeout: NodeJS.Timeout | null = null;
  private readonly _pingIntervalMs = 30000; // 30 seconds
  private readonly _pongTimeoutMs = 10000; // 10 seconds
  private _explicitDisconnect = false; // Track if disconnect was intentional
  private readonly _defaultPacketSendTimeoutMs = parseInt(
    process.env.BTP_SEND_TIMEOUT_MS ?? '30000',
    10
  ); // Fallback timeout when packet has no expiresAt

  private readonly _nodeId: string;
  private _packetHandler: PacketHandler | null = null;

  /**
   * Create BTPClient instance
   * @param peer - Peer configuration
   * @param nodeId - Local node identifier (sent in auth message)
   * @param logger - Pino logger instance
   * @param maxRetries - Maximum retry attempts (default: 5)
   */
  constructor(peer: Peer, nodeId: string, logger: Logger, maxRetries?: number) {
    super();
    this._peer = peer;
    this._nodeId = nodeId;
    this._logger = logger.child({ peerId: peer.id });
    if (maxRetries !== undefined) {
      this._maxRetries = maxRetries;
    }
  }

  /**
   * Get current connection state
   */
  get isConnected(): boolean {
    return this._connectionState === 'connected';
  }

  /**
   * Set PacketHandler reference (to handle incoming prepare packets from server)
   * @param packetHandler - PacketHandler instance for routing incoming packets
   */
  setPacketHandler(packetHandler: PacketHandler): void {
    this._packetHandler = packetHandler;
  }

  /**
   * Connect to peer connector
   * Establishes WebSocket connection and performs authentication
   */
  async connect(): Promise<void> {
    if (this._connectionState === 'connecting' || this._connectionState === 'connected') {
      this._logger.debug(
        { event: 'btp_connect_skip', state: this._connectionState },
        'Connection already in progress or established'
      );
      return;
    }

    // Reset explicit disconnect flag on new connection attempt
    this._explicitDisconnect = false;

    this._connectionState = 'connecting';
    this._logger.info(
      { event: 'btp_connection_attempt', url: this._peer.url },
      'Connecting to peer'
    );

    return new Promise<void>((resolve, reject) => {
      try {
        this._ws = new WebSocket(this._peer.url);

        // Set up event handlers
        this._ws.on('open', async () => {
          try {
            await this._authenticate();
            this._connectionState = 'connected';
            this._retryCount = 0; // Reset retry count on success
            this._peer.connected = true;
            this._peer.lastSeen = new Date();
            this._startKeepAlive();

            this._logger.info({ event: 'btp_connected', url: this._peer.url }, 'Connected to peer');
            this.emit('connected');
            resolve();
          } catch (error) {
            const errorMessage = error instanceof Error ? error.message : String(error);
            this._logger.error(
              { event: 'btp_connection_error', error: errorMessage },
              'Authentication failed'
            );
            this._connectionState = 'error';
            this._ws?.close();
            reject(error);
          }
        });

        this._ws.on('message', (data: Buffer) => {
          this._handleMessage(data).catch((error) => {
            this._logger.error(
              {
                event: 'btp_message_error',
                error: error instanceof Error ? error.message : String(error),
              },
              'Error handling message'
            );
          });
        });

        this._ws.on('close', () => {
          this._handleClose();
        });

        this._ws.on('error', (error: Error) => {
          this._logger.error(
            { event: 'btp_connection_error', error: error.message },
            'WebSocket error'
          );
          this._connectionState = 'error';
          this.emit('error', error);
          reject(new BTPConnectionError(error.message));
        });

        this._ws.on('pong', () => {
          this._handlePong();
        });
      } catch (error) {
        const errorMessage = error instanceof Error ? error.message : String(error);
        this._logger.error(
          { event: 'btp_connection_error', error: errorMessage },
          'Failed to create WebSocket'
        );
        this._connectionState = 'error';
        reject(new BTPConnectionError(errorMessage));
      }
    });
  }

  /**
   * Disconnect from peer connector
   * Gracefully closes WebSocket connection
   */
  async disconnect(): Promise<void> {
    this._logger.info({ event: 'btp_disconnect_requested' }, 'Disconnecting from peer');

    // Set flag to prevent automatic retry
    this._explicitDisconnect = true;

    this._stopKeepAlive();

    if (this._ws) {
      this._ws.close();
      this._ws = null;
    }

    this._connectionState = 'disconnected';
    this._peer.connected = false;

    // Reject all pending requests
    for (const [requestId, pending] of this._pendingRequests.entries()) {
      clearTimeout(pending.timeoutId);
      pending.reject(new BTPConnectionError('Connection closed'));
      this._pendingRequests.delete(requestId);
    }
  }

  /**
   * Perform BTP authentication handshake
   * @private
   */
  private async _authenticate(): Promise<void> {
    this._logger.info({ event: 'btp_auth_attempt' }, 'Attempting authentication');

    // Create AUTH message with nodeId and shared secret in JSON format
    // Server expects: { "peerId": "connector-b", "secret": "shared-secret" }
    const authData = {
      peerId: this._nodeId,
      secret: this._peer.authToken,
    };
    const authDataBuffer = Buffer.from(JSON.stringify(authData), 'utf8');
    const authMessage: BTPMessage = {
      type: BTPMessageType.MESSAGE,
      requestId: this._generateRequestId(),
      data: {
        protocolData: [
          {
            protocolName: 'auth',
            contentType: 0,
            data: authDataBuffer,
          },
        ],
        ilpPacket: Buffer.alloc(0),
      } as BTPData,
    };

    // Send AUTH message
    const authBuffer = serializeBTPMessage(authMessage);

    if (!this._ws) {
      throw new BTPAuthenticationError('WebSocket not connected');
    }

    return new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this._logger.error(
          { event: 'btp_auth_failed', reason: 'timeout' },
          'Authentication timeout'
        );
        reject(new BTPAuthenticationError('Authentication timeout'));
      }, 5000);

      // Set up one-time message handler for auth response
      const authHandler = (data: Buffer): void => {
        try {
          const message = parseBTPMessage(data);

          if (message.requestId === authMessage.requestId) {
            clearTimeout(timeout);
            this._ws?.removeListener('message', authHandler);

            if (message.type === BTPMessageType.ERROR) {
              const errorData = isBTPErrorData(message)
                ? message.data
                : { code: 'UNKNOWN', name: 'Unknown error' };
              this._logger.error(
                { event: 'btp_auth_failed', reason: errorData.code },
                'Authentication failed'
              );
              reject(new BTPAuthenticationError(`Authentication failed: ${errorData.code}`));
            } else if (message.type === BTPMessageType.RESPONSE) {
              this._logger.info({ event: 'btp_auth_success' }, 'Authentication successful');
              resolve();
            }
          }
        } catch (error) {
          clearTimeout(timeout);
          this._ws?.removeListener('message', authHandler);
          const errorMessage = error instanceof Error ? error.message : String(error);
          this._logger.error(
            { event: 'btp_auth_failed', reason: errorMessage },
            'Authentication error'
          );
          reject(new BTPAuthenticationError(errorMessage));
        }
      };

      this._ws?.on('message', authHandler);
      this._ws?.send(authBuffer);
    });
  }

  /**
   * Send ILP packet to peer
   * @param packet - ILP Prepare packet
   * @returns ILP Fulfill or Reject packet
   */
  async sendPacket(
    packet: ILPPreparePacket,
    protocolData?: Array<{ protocolName: string; contentType: number; data: Buffer }>
  ): Promise<ILPFulfillPacket | ILPRejectPacket> {
    if (!this.isConnected) {
      throw new BTPConnectionError('Not connected to peer');
    }

    if (!this._ws) {
      throw new BTPConnectionError('WebSocket not available');
    }

    // Serialize ILP packet
    const serializedPacket = serializePacket(packet);

    // Generate unique request ID
    const requestId = this._generateRequestId();

    // Create BTP MESSAGE frame
    const btpMessage: BTPMessage = {
      type: BTPMessageType.MESSAGE,
      requestId,
      data: {
        protocolData: protocolData ?? [],
        ilpPacket: serializedPacket,
      } as BTPData,
    };

    // Encode BTP MESSAGE
    const btpBuffer = serializeBTPMessage(btpMessage);

    this._logger.debug(
      {
        event: 'btp_message_sent',
        requestId,
        packetType: packet.type,
      },
      'Sending BTP message'
    );

    // Send via WebSocket
    try {
      this._ws.send(btpBuffer);
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      throw new BTPConnectionError(`Failed to send message: ${errorMessage}`);
    }

    // Derive timeout from the ILP packet's expiresAt — the protocol-level timeout.
    // This ensures BTP waits as long as the packet is valid, regardless of hop count.
    let timeoutMs: number;
    if (packet.expiresAt) {
      const remaining = packet.expiresAt.getTime() - Date.now();
      timeoutMs = Math.max(remaining - 500, 1000);
    } else {
      timeoutMs = this._defaultPacketSendTimeoutMs;
    }

    // Wait for response
    return new Promise<ILPFulfillPacket | ILPRejectPacket>((resolve, reject) => {
      const timeoutId = setTimeout(() => {
        this._pendingRequests.delete(requestId);
        reject(new BTPConnectionError(`Packet send timeout (${timeoutMs}ms)`));
      }, timeoutMs);

      this._pendingRequests.set(requestId, {
        resolve,
        reject,
        timeoutId,
      });
    });
  }

  /**
   * Send protocol data message without expecting a response
   * Used for one-way messages like claim notifications
   * @param protocolName - Protocol identifier (e.g., "payment-channel-claim")
   * @param contentType - Content type code (e.g., 1 for JSON)
   * @param data - Protocol-specific data as Buffer
   */
  async sendProtocolData(protocolName: string, contentType: number, data: Buffer): Promise<void> {
    if (!this.isConnected) {
      throw new BTPConnectionError('Not connected to peer');
    }

    if (!this._ws) {
      throw new BTPConnectionError('WebSocket not available');
    }

    // Generate unique request ID
    const requestId = this._generateRequestId();

    // Create BTP MESSAGE frame with protocolData
    const btpMessage: BTPMessage = {
      type: BTPMessageType.MESSAGE,
      requestId,
      data: {
        protocolData: [
          {
            protocolName,
            contentType,
            data,
          },
        ],
        ilpPacket: Buffer.alloc(0), // Empty for protocol-data-only messages
      } as BTPData,
    };

    // Encode BTP MESSAGE
    const btpBuffer = serializeBTPMessage(btpMessage);

    this._logger.debug(
      {
        event: 'btp_protocol_data_sent',
        requestId,
        protocolName,
        contentType,
      },
      'Sending BTP protocol data message'
    );

    // Send via WebSocket (fire-and-forget, no response expected)
    try {
      this._ws.send(btpBuffer);
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      throw new BTPConnectionError(`Failed to send protocol data: ${errorMessage}`);
    }
  }

  /**
   * Handle incoming BTP message
   * @private
   */
  private async _handleMessage(data: Buffer): Promise<void> {
    // Try to parse as JSON first (simplified format from agent-server)
    // This handles backward compatibility with agent-server's JSON responses
    try {
      const jsonStr = data.toString('utf8');
      if (jsonStr.startsWith('{')) {
        const json = JSON.parse(jsonStr);
        if (json.type === 'FULFILL' || json.type === 'REJECT') {
          this._logger.debug(
            { event: 'btp_json_response', type: json.type },
            'Received JSON response'
          );
          // Find pending request and resolve it
          // Since JSON responses don't include requestId, resolve the most recent pending request
          const pendingEntries = Array.from(this._pendingRequests.entries());
          const firstEntry = pendingEntries[0];
          if (firstEntry) {
            const [requestId, pending] = firstEntry;
            clearTimeout(pending.timeoutId);
            this._pendingRequests.delete(requestId);

            if (json.type === 'FULFILL') {
              const fulfillPacket: ILPFulfillPacket = {
                type: PacketType.FULFILL,
                fulfillment: json.fulfillment
                  ? Buffer.from(json.fulfillment, 'base64')
                  : Buffer.alloc(32),
                data: json.data ? Buffer.from(json.data, 'base64') : Buffer.alloc(0),
              };
              pending.resolve(fulfillPacket);
            } else {
              const rejectPacket: ILPRejectPacket = {
                type: PacketType.REJECT,
                code: json.code || 'F00',
                message: json.message || 'Unknown error',
                triggeredBy: json.triggeredBy || '',
                data: json.data ? Buffer.from(json.data, 'base64') : Buffer.alloc(0),
              };
              pending.resolve(rejectPacket);
            }
          }
          return;
        }
      }
    } catch {
      // Not JSON, continue to BTP parsing
    }

    try {
      const message = parseBTPMessage(data);

      // Handle RESPONSE messages (responses to our outbound requests)
      if (message.type === BTPMessageType.RESPONSE || message.type === BTPMessageType.ERROR) {
        const pending = this._pendingRequests.get(message.requestId);
        if (pending) {
          clearTimeout(pending.timeoutId);
          this._pendingRequests.delete(message.requestId);

          if (message.type === BTPMessageType.ERROR) {
            const errorData = isBTPErrorData(message)
              ? message.data
              : {
                  code: 'UNKNOWN',
                  name: 'Unknown error',
                  data: Buffer.alloc(0),
                  triggeredAt: new Date().toISOString(),
                };
            pending.reject(new BTPError(errorData.code, errorData.name, errorData.data));
          } else if (isBTPData(message) && message.data.ilpPacket) {
            // Decode ILP packet from response
            const ilpPacket = deserializePacket(message.data.ilpPacket);
            pending.resolve(ilpPacket as ILPFulfillPacket | ILPRejectPacket);
          }
        }
      }

      // Handle incoming MESSAGE packets (new prepare packets from server)
      if (message.type === BTPMessageType.MESSAGE && isBTPData(message)) {
        if (!this._packetHandler) {
          this._logger.warn(
            { event: 'btp_incoming_packet_no_handler', requestId: message.requestId },
            'Received incoming prepare packet but no PacketHandler configured'
          );
          return;
        }

        const ilpPacket = message.data.ilpPacket;
        if (!ilpPacket || ilpPacket.length === 0) {
          this._logger.warn(
            { event: 'btp_incoming_packet_no_ilp', requestId: message.requestId },
            'Received MESSAGE with no ILP packet'
          );
          return;
        }

        try {
          // Deserialize ILP prepare packet
          const preparePacket = deserializePacket(ilpPacket);

          // Only handle PREPARE packets (not FULFILL/REJECT)
          if (preparePacket.type !== PacketType.PREPARE) {
            this._logger.warn(
              { event: 'btp_incoming_packet_wrong_type', packetType: preparePacket.type },
              'Received non-PREPARE packet in MESSAGE'
            );
            return;
          }

          this._logger.debug(
            {
              event: 'btp_incoming_prepare',
              requestId: message.requestId,
              destination: preparePacket.destination,
            },
            'Received incoming prepare packet from server'
          );

          // Route packet through PacketHandler (pass peer ID for settlement tracking)
          const response = await this._packetHandler.handlePreparePacket(
            preparePacket as ILPPreparePacket,
            this._peer.id
          );

          // Send response back
          const responseIlpBuffer = serializePacket(response);
          const btpResponse: BTPMessage = {
            type: BTPMessageType.RESPONSE,
            requestId: message.requestId,
            data: {
              protocolData: [],
              ilpPacket: responseIlpBuffer,
            },
          };

          const btpResponseBuffer = serializeBTPMessage(btpResponse);
          this._ws?.send(btpResponseBuffer, (error) => {
            if (error) {
              this._logger.error(
                {
                  event: 'btp_response_send_failed',
                  error: error.message,
                  requestId: message.requestId,
                },
                'Failed to send response to incoming prepare'
              );
            } else {
              this._logger.debug(
                { event: 'btp_response_sent', requestId: message.requestId },
                'Sent response to incoming prepare'
              );
            }
          });
        } catch (error) {
          const errorMessage = error instanceof Error ? error.message : String(error);
          this._logger.error(
            { event: 'btp_incoming_packet_error', error: errorMessage },
            'Error handling incoming prepare packet'
          );
        }
      }

      // Update last seen timestamp
      this._peer.lastSeen = new Date();
    } catch (error) {
      this._logger.error(
        {
          event: 'btp_message_parse_error',
          error: error instanceof Error ? error.message : String(error),
        },
        'Failed to parse BTP message'
      );
    }
  }

  /**
   * Handle WebSocket close event
   * @private
   */
  private _handleClose(): void {
    this._logger.info(
      { event: 'btp_disconnected', reason: 'connection_closed' },
      'Connection closed'
    );

    this._stopKeepAlive();
    this._connectionState = 'disconnected';
    this._peer.connected = false;
    this.emit('disconnected');

    // Only attempt to reconnect if disconnect was not explicit
    if (!this._explicitDisconnect) {
      this._retry().catch((error) => {
        this._logger.error(
          {
            event: 'btp_retry_failed',
            error: error instanceof Error ? error.message : String(error),
          },
          'Retry failed'
        );
      });
    }
  }

  /**
   * Retry connection with exponential backoff
   * @private
   */
  private async _retry(): Promise<void> {
    if (this._retryCount >= this._maxRetries) {
      this._logger.error(
        { event: 'btp_max_retries', retryCount: this._retryCount },
        'Max retries exceeded'
      );
      throw new BTPConnectionError('Max retries exceeded');
    }

    this._retryCount++;
    const backoffMs = Math.min(1000 * Math.pow(2, this._retryCount - 1), 16000);

    this._logger.warn(
      { event: 'btp_retry', retryCount: this._retryCount, backoffMs },
      'Retrying connection'
    );

    await new Promise((resolve) => setTimeout(resolve, backoffMs));

    try {
      await this.connect();
    } catch (error) {
      // If connection fails, retry will be triggered by close handler
      const errorMessage = error instanceof Error ? error.message : String(error);
      this._logger.error(
        { event: 'btp_connection_error', error: errorMessage },
        'Connection retry failed'
      );
    }
  }

  /**
   * Start keep-alive ping/pong mechanism
   * @private
   */
  private _startKeepAlive(): void {
    this._stopKeepAlive();

    this._pingInterval = setInterval(() => {
      if (this._ws && this.isConnected) {
        this._logger.debug({ event: 'btp_ping_sent' }, 'Sending ping');
        this._ws.ping();

        // Set pong timeout
        this._pongTimeout = setTimeout(() => {
          this._logger.warn({ event: 'btp_pong_timeout' }, 'Pong timeout - reconnecting');
          this._ws?.close();
        }, this._pongTimeoutMs);
      }
    }, this._pingIntervalMs);
  }

  /**
   * Stop keep-alive ping/pong mechanism
   * @private
   */
  private _stopKeepAlive(): void {
    if (this._pingInterval) {
      clearInterval(this._pingInterval);
      this._pingInterval = null;
    }

    if (this._pongTimeout) {
      clearTimeout(this._pongTimeout);
      this._pongTimeout = null;
    }
  }

  /**
   * Handle pong response
   * @private
   */
  private _handlePong(): void {
    this._logger.debug({ event: 'btp_pong_received' }, 'Received pong');

    if (this._pongTimeout) {
      clearTimeout(this._pongTimeout);
      this._pongTimeout = null;
    }
  }

  /**
   * Generate unique request ID
   * @private
   */
  private _generateRequestId(): number {
    this._requestIdCounter = (this._requestIdCounter + 1) & 0xffffffff; // Keep within uint32 range
    return this._requestIdCounter;
  }
}
