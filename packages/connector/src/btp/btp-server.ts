/**
 * BTP WebSocket Server
 * Implements RFC-0023 Bilateral Transfer Protocol server for accepting peer connections
 */

import WebSocket, { WebSocketServer } from 'ws';
import { Logger } from '../utils/logger';
import { PacketHandler } from '../core/packet-handler';
import { BTPMessage, BTPMessageType, BTPData, BTPError, isBTPData } from './btp-types';
import { parseBTPMessage, serializeBTPMessage } from './btp-message-parser';
import {
  deserializePacket,
  serializePacket,
  ILPPreparePacket,
  ILPFulfillPacket,
  ILPRejectPacket,
  PacketType,
} from '@toon-protocol/shared';
import type { InboundClaimValidatorFn } from './inbound-claim-validator';

/**
 * BTP peer connection metadata
 */
interface PeerConnection {
  peerId: string;
  ws: WebSocket;
  authenticated: boolean;
}

/**
 * Pending request for response tracking
 */
interface PendingRequest {
  resolve: (response: ILPFulfillPacket | ILPRejectPacket) => void;
  reject: (error: Error) => void;
  timeout: NodeJS.Timeout;
}

/**
 * BTPServer - WebSocket server for BTP protocol
 * Accepts incoming BTP connections from peer connectors
 */
export class BTPServer {
  private readonly logger: Logger;
  private readonly packetHandler: PacketHandler;
  private wss: WebSocketServer | null = null;
  private readonly peers: Map<string, PeerConnection> = new Map();
  private readonly pendingRequests: Map<number, PendingRequest> = new Map();
  private onConnectionCallback?: (peerId: string, connection: WebSocket) => void;
  private onMessageCallback?: (peerId: string, message: BTPMessage) => void;
  private inboundClaimValidator?: InboundClaimValidatorFn;

  /**
   * Create BTPServer instance
   * @param logger - Pino logger instance
   * @param packetHandler - PacketHandler for processing ILP packets
   */
  constructor(logger: Logger, packetHandler: PacketHandler) {
    this.logger = logger;
    this.packetHandler = packetHandler;
  }

  /**
   * Start BTP WebSocket server
   * @param port - Port number to listen on (default from BTP_SERVER_PORT env var or 3000)
   */
  async start(port?: number): Promise<void> {
    const serverPort = port ?? parseInt(process.env['BTP_SERVER_PORT'] ?? '3000', 10);

    return new Promise<void>((resolve, reject) => {
      try {
        this.wss = new WebSocketServer({ port: serverPort });

        this.wss.on('listening', () => {
          this.logger.info(
            {
              event: 'btp_server_started',
              port: serverPort,
            },
            `BTP server listening on port ${serverPort}`
          );
          resolve();
        });

        this.wss.on('error', (error) => {
          this.logger.error(
            {
              event: 'btp_server_error',
              error: error.message,
            },
            'BTP server error'
          );
          reject(error);
        });

        this.wss.on('connection', (ws: WebSocket, req) => {
          this.handleConnection(ws, req);
        });
      } catch (error) {
        this.logger.error(
          {
            event: 'btp_server_start_failed',
            error: error instanceof Error ? error.message : String(error),
          },
          'Failed to start BTP server'
        );
        reject(error);
      }
    });
  }

  /**
   * Stop BTP server and close all connections
   */
  async stop(): Promise<void> {
    if (!this.wss) {
      return;
    }

    const activeConnections = this.peers.size;

    // Close all peer connections
    for (const [peerId, peerConn] of this.peers.entries()) {
      try {
        peerConn.ws.close(1000, 'Server shutting down');
      } catch (error) {
        this.logger.warn(
          {
            event: 'btp_connection_close_failed',
            peerId,
            error: error instanceof Error ? error.message : String(error),
          },
          'Failed to close peer connection during shutdown'
        );
      }
    }

    this.peers.clear();

    // Close WebSocket server
    return new Promise<void>((resolve, reject) => {
      if (!this.wss) {
        resolve();
        return;
      }

      this.wss.close((error) => {
        if (error) {
          this.logger.error(
            {
              event: 'btp_server_shutdown_error',
              error: error.message,
            },
            'Error during BTP server shutdown'
          );
          reject(error);
        } else {
          this.logger.info(
            {
              event: 'btp_server_shutdown',
              activeConnections,
            },
            'BTP server shutdown complete'
          );
          this.wss = null;
          resolve();
        }
      });
    });
  }

  /**
   * Register connection event handler
   * @param callback - Called when peer successfully authenticates
   */
  onConnection(callback: (peerId: string, connection: WebSocket) => void): void {
    this.onConnectionCallback = callback;
  }

  /**
   * Register message event handler
   * @param callback - Called when BTP message received from peer
   */
  onMessage(callback: (peerId: string, message: BTPMessage) => void): void {
    this.onMessageCallback = callback;
  }

  /**
   * Set inbound claim validator for ILP PREPARE packets.
   * When set, every ILP PREPARE arriving via BTP must pass claim validation
   * before being forwarded to the packet handler. This prevents unpaid writes.
   *
   * @param validator - Async function that returns null (valid) or ILPRejectPacket (reject)
   */
  setInboundClaimValidator(validator: InboundClaimValidatorFn): void {
    this.inboundClaimValidator = validator;
  }

  /**
   * Check if a peer is connected (authenticated incoming connection)
   * @param peerId - Peer identifier to check
   * @returns True if peer is connected and authenticated
   */
  hasPeer(peerId: string): boolean {
    const peer = this.peers.get(peerId);
    return peer !== undefined && peer.authenticated;
  }

  /**
   * Send ILP packet to an incoming authenticated peer
   * @param peerId - Target peer identifier
   * @param packet - ILP Prepare packet to send
   * @returns ILP response packet (Fulfill or Reject)
   * @throws Error if peer not found or not authenticated
   */
  async sendPacketToPeer(
    peerId: string,
    packet: ILPPreparePacket,
    protocolData?: Array<{ protocolName: string; contentType: number; data: Buffer }>
  ): Promise<ILPFulfillPacket | ILPRejectPacket> {
    this.logger.debug(
      { event: 'btp_server_send_to_peer', peerId, destination: packet.destination },
      'Sending packet to incoming peer'
    );

    // Look up authenticated peer connection
    const peerConn = this.peers.get(peerId);
    if (!peerConn || !peerConn.authenticated) {
      const errorMessage = `Incoming peer not found or not authenticated: ${peerId}`;
      this.logger.error({ event: 'btp_server_peer_not_found', peerId }, errorMessage);
      throw new Error(errorMessage);
    }

    // Check WebSocket state
    if (peerConn.ws.readyState !== WebSocket.OPEN) {
      const errorMessage = `WebSocket to incoming peer ${peerId} not in OPEN state`;
      this.logger.error({ event: 'btp_server_ws_not_open', peerId }, errorMessage);
      throw new Error(errorMessage);
    }

    // Generate unique request ID
    const requestId = Math.floor(Math.random() * 0xffffffff);

    // Serialize ILP packet
    const ilpPacketBuffer = serializePacket(packet);

    // Create BTP MESSAGE
    const btpMessage: BTPMessage = {
      type: BTPMessageType.MESSAGE,
      requestId,
      data: {
        protocolData: protocolData ?? [],
        ilpPacket: ilpPacketBuffer,
      },
    };

    // Serialize BTP message
    const btpBuffer = serializeBTPMessage(btpMessage);

    // Derive timeout from the ILP packet's expiresAt — the protocol-level timeout.
    // This ensures BTP waits as long as the packet is valid, regardless of hop count.
    // Fall back to 10s only if expiresAt is missing (shouldn't happen for valid packets).
    let timeoutMs: number;
    if (packet.expiresAt) {
      const remaining = packet.expiresAt.getTime() - Date.now();
      timeoutMs = Math.max(remaining - 500, 1000);
    } else {
      timeoutMs = 10000;
    }

    // Create promise for response
    return new Promise<ILPFulfillPacket | ILPRejectPacket>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pendingRequests.delete(requestId);
        reject(new Error(`Timeout waiting for response from peer ${peerId} (${timeoutMs}ms)`));
      }, timeoutMs);

      // Register pending request
      this.pendingRequests.set(requestId, { resolve, reject, timeout });

      // Send BTP message
      peerConn.ws.send(btpBuffer, (error) => {
        if (error) {
          clearTimeout(timeout);
          this.pendingRequests.delete(requestId);
          reject(error);
        }
      });
    });
  }

  /**
   * Handle new WebSocket connection
   */
  private handleConnection(
    ws: WebSocket,
    req: { socket: { remoteAddress?: string; remotePort?: number } }
  ): void {
    const remoteAddress = req.socket.remoteAddress ?? 'unknown';
    const connectionId = `${remoteAddress}:${req.socket.remotePort}`;

    this.logger.info(
      {
        event: 'btp_connection',
        connectionId,
        remoteAddress,
      },
      'BTP connection established (awaiting authentication)'
    );

    // Store temporary connection (not yet authenticated)
    const peerConn: PeerConnection = {
      peerId: connectionId, // Temporary ID until authenticated
      ws,
      authenticated: false,
    };

    ws.on('message', async (data: Buffer) => {
      try {
        await this.handleWebSocketMessage(peerConn, data);
      } catch (error) {
        this.logger.error(
          {
            event: 'btp_message_error',
            peerId: peerConn.peerId,
            error: error instanceof Error ? error.message : String(error),
          },
          'Error handling BTP message'
        );

        // Send BTP ERROR response if connection is still open
        if (ws.readyState === WebSocket.OPEN) {
          try {
            const btpError =
              error instanceof BTPError ? error : new BTPError('F00', 'Internal server error');

            const errorMessage: BTPMessage = {
              type: BTPMessageType.ERROR,
              requestId: 0,
              data: btpError.toBTPErrorData(),
            };

            ws.send(serializeBTPMessage(errorMessage));
          } catch (sendError) {
            this.logger.error(
              {
                event: 'btp_error_response_failed',
                peerId: peerConn.peerId,
              },
              'Failed to send BTP ERROR response'
            );
          }
        }
      }
    });

    ws.on('close', (code, reason) => {
      this.logger.info(
        {
          event: 'btp_disconnect',
          peerId: peerConn.peerId,
          code,
          reason: reason.toString(),
        },
        'BTP connection closed'
      );

      // Remove peer from active connections
      if (peerConn.authenticated) {
        this.peers.delete(peerConn.peerId);
      }
    });

    ws.on('error', (error) => {
      this.logger.error(
        {
          event: 'btp_connection_error',
          peerId: peerConn.peerId,
          error: error.message,
        },
        'BTP connection error'
      );
    });
  }

  /**
   * Handle incoming WebSocket message
   */
  private async handleWebSocketMessage(peerConn: PeerConnection, data: Buffer): Promise<void> {
    // Parse BTP message
    const message = parseBTPMessage(data);

    this.logger.debug(
      {
        event: 'btp_message_received',
        peerId: peerConn.peerId,
        messageType: BTPMessageType[message.type],
        requestId: message.requestId,
      },
      'BTP message received'
    );

    // If not authenticated, expect AUTH message
    if (!peerConn.authenticated) {
      await this.authenticatePeer(peerConn, message);
      return;
    }

    // Check if this is a response to an outbound request
    const pendingRequest = this.pendingRequests.get(message.requestId);
    if (
      pendingRequest &&
      (message.type === BTPMessageType.RESPONSE || message.type === BTPMessageType.ERROR)
    ) {
      // This is a response to an outbound packet we sent
      clearTimeout(pendingRequest.timeout);
      this.pendingRequests.delete(message.requestId);

      try {
        if (message.type === BTPMessageType.RESPONSE && isBTPData(message)) {
          const ilpPacket = message.data.ilpPacket;
          if (!ilpPacket) {
            pendingRequest.reject(new Error('Response missing ILP packet'));
            return;
          }
          const ilpResponse = deserializePacket(ilpPacket);

          if (ilpResponse.type === PacketType.FULFILL || ilpResponse.type === PacketType.REJECT) {
            this.logger.debug(
              {
                event: 'btp_server_packet_response',
                peerId: peerConn.peerId,
                responseType: PacketType[ilpResponse.type],
              },
              'Received ILP response from incoming peer'
            );
            pendingRequest.resolve(ilpResponse as ILPFulfillPacket | ILPRejectPacket);
          } else {
            pendingRequest.reject(new Error(`Unexpected ILP packet type: ${ilpResponse.type}`));
          }
        } else if (message.type === BTPMessageType.ERROR) {
          const errorData = message.data as { code: string; name?: string };
          pendingRequest.reject(
            new Error(
              `BTP Error from peer ${peerConn.peerId}: ${errorData.code} - ${errorData.name ?? 'Unknown'}`
            )
          );
        }
      } catch (error) {
        const errorMessage = error instanceof Error ? error.message : String(error);
        pendingRequest.reject(new Error(`Error parsing response: ${errorMessage}`));
      }
      return;
    }

    // Handle authenticated messages (new incoming packets)
    if (message.type === BTPMessageType.MESSAGE) {
      await this.handleMessage(peerConn, message);
    } else {
      this.logger.warn(
        {
          event: 'btp_unexpected_message_type',
          peerId: peerConn.peerId,
          messageType: BTPMessageType[message.type],
        },
        'Unexpected BTP message type (expected MESSAGE)'
      );
    }

    // Notify message callback
    if (this.onMessageCallback) {
      this.onMessageCallback(peerConn.peerId, message);
    }
  }

  /**
   * Authenticate BTP peer
   * @param peerConn - Peer connection to authenticate
   * @param authMessage - BTP AUTH message
   */
  private async authenticatePeer(peerConn: PeerConnection, authMessage: BTPMessage): Promise<void> {
    try {
      // Validate message type (expect MESSAGE with auth protocol data)
      if (authMessage.type !== BTPMessageType.MESSAGE) {
        throw new BTPError(
          'F00',
          `Expected MESSAGE for authentication, got ${BTPMessageType[authMessage.type]}`
        );
      }

      if (!isBTPData(authMessage)) {
        throw new BTPError('F00', 'Invalid authentication message format');
      }

      const messageData = authMessage.data as BTPData;

      // Find auth protocol data
      const authProtocolData = messageData.protocolData.find((pd) => pd.protocolName === 'auth');

      if (!authProtocolData) {
        throw new BTPError('F00', 'Missing auth protocol data');
      }

      // Extract shared secret and peer ID from auth data
      // Format: JSON { "peerId": "connector-b", "secret": "shared-secret-123" }
      const authData = JSON.parse(authProtocolData.data.toString('utf8'));
      const { peerId, secret } = authData;

      if (!peerId) {
        throw new BTPError('F00', 'Invalid auth data: missing peerId');
      }

      // RFC-0023: Allow empty string for secret when no authentication is needed
      // Check if secret is undefined (field missing) vs empty string (no auth)
      if (secret === undefined) {
        throw new BTPError('F00', 'Invalid auth data: secret field missing');
      }

      // Handle no-auth case (empty secret string) per RFC-0023
      // This is the default configuration for permissionless, ILP-gated networks
      // where access control happens at the ILP layer (routing policies, credit limits,
      // settlement requirements) rather than at the BTP transport layer.
      if (secret === '') {
        // Default to true (permissionless mode) unless explicitly disabled
        const allowNoAuth = process.env['BTP_ALLOW_NOAUTH'] !== 'false';

        if (!allowNoAuth) {
          this.logger.warn(
            {
              event: 'btp_auth',
              peerId,
              success: false,
              reason: 'no-auth disabled',
            },
            'BTP authentication failed: no-auth mode disabled (set BTP_ALLOW_NOAUTH=true to enable)'
          );
          throw new BTPError('F00', 'Authentication failed: no-auth mode disabled');
        }

        this.logger.info(
          {
            event: 'btp_auth',
            peerId,
            success: true,
            mode: 'no-auth',
          },
          `BTP peer authenticated (no-auth mode): ${peerId}`
        );

        // Skip secret validation for no-auth connections
        peerConn.peerId = peerId;
        peerConn.authenticated = true;
        this.peers.set(peerId, peerConn);

        // Send RESPONSE acknowledging authentication
        const responseMessage: BTPMessage = {
          type: BTPMessageType.RESPONSE,
          requestId: authMessage.requestId,
          data: {
            protocolData: [],
          },
        };

        peerConn.ws.send(serializeBTPMessage(responseMessage));

        // Notify connection callback
        if (this.onConnectionCallback) {
          this.onConnectionCallback(peerId, peerConn.ws);
        }
        return;
      }

      // Validate shared secret from environment variable (non-empty secret)
      const envVarKey = `BTP_PEER_${peerId.toUpperCase().replace(/-/g, '_')}_SECRET`;
      const expectedSecret = process.env[envVarKey];

      if (!expectedSecret) {
        this.logger.warn(
          {
            event: 'btp_auth',
            peerId,
            success: false,
            reason: 'no configured secret for peer',
          },
          'BTP authentication failed: peer not configured'
        );
        throw new BTPError('F00', 'Authentication failed: peer not configured');
      }

      if (secret !== expectedSecret) {
        this.logger.warn(
          {
            event: 'btp_auth',
            peerId,
            success: false,
            reason: 'invalid secret',
          },
          'BTP authentication failed: invalid secret'
        );
        throw new BTPError('F00', 'Authentication failed: invalid secret');
      }

      // Authentication successful
      peerConn.peerId = peerId;
      peerConn.authenticated = true;
      this.peers.set(peerId, peerConn);

      this.logger.info(
        {
          event: 'btp_auth',
          peerId,
          success: true,
        },
        `BTP peer authenticated: ${peerId}`
      );

      // Send RESPONSE acknowledging authentication
      const responseMessage: BTPMessage = {
        type: BTPMessageType.RESPONSE,
        requestId: authMessage.requestId,
        data: {
          protocolData: [],
        },
      };

      peerConn.ws.send(serializeBTPMessage(responseMessage));

      // Notify connection callback
      if (this.onConnectionCallback) {
        this.onConnectionCallback(peerId, peerConn.ws);
      }
    } catch (error) {
      // Authentication failed - close connection
      this.logger.error(
        {
          event: 'btp_auth_error',
          error: error instanceof Error ? error.message : String(error),
        },
        'BTP authentication error'
      );

      // Send ERROR response
      const btpError =
        error instanceof BTPError ? error : new BTPError('F00', 'Authentication failed');

      const errorMessage: BTPMessage = {
        type: BTPMessageType.ERROR,
        requestId: authMessage.requestId,
        data: btpError.toBTPErrorData(),
      };

      peerConn.ws.send(serializeBTPMessage(errorMessage));

      // Close connection after sending error
      setTimeout(() => {
        peerConn.ws.close(1008, 'Authentication failed');
      }, 100);
    }
  }

  /**
   * Handle BTP MESSAGE containing ILP packet
   * @param peerConn - Authenticated peer connection
   * @param message - BTP MESSAGE
   */
  private async handleMessage(peerConn: PeerConnection, message: BTPMessage): Promise<void> {
    const childLogger = this.logger.child({ peerId: peerConn.peerId });

    try {
      // Validate message type
      if (message.type !== BTPMessageType.MESSAGE) {
        throw new BTPError('F00', `Expected MESSAGE, got ${BTPMessageType[message.type]}`);
      }

      if (!isBTPData(message)) {
        throw new BTPError('F00', 'Invalid message format');
      }

      const messageData = message.data as BTPData;

      // Check if this is a protocol-data-only message (no ILP packet)
      const hasILPPacket = messageData.ilpPacket && messageData.ilpPacket.length > 0;

      if (!hasILPPacket) {
        // Protocol-data-only message (e.g., payment channel claims)
        // No ILP packet processing needed, just notify callback
        childLogger.debug(
          {
            event: 'btp_protocol_data_received',
            peerId: peerConn.peerId,
            requestId: message.requestId,
            protocols: messageData.protocolData.map((pd) => pd.protocolName),
          },
          'Processing BTP protocol-data-only message'
        );
        return; // onMessageCallback will be called after this function returns
      }

      childLogger.debug(
        {
          event: 'btp_message_received',
          peerId: peerConn.peerId,
          messageType: 'MESSAGE',
          requestId: message.requestId,
        },
        'Processing BTP MESSAGE with ILP packet'
      );

      // Decode ILP packet from buffer (we know it exists due to check above)
      const ilpPacket = deserializePacket(messageData.ilpPacket!);

      // Validate packet type (must be PREPARE)
      if (ilpPacket.type !== PacketType.PREPARE) {
        throw new BTPError('F00', `Expected ILP PREPARE packet, got type ${ilpPacket.type}`);
      }

      // Validate inbound claim before forwarding to packet handler.
      // This prevents unpaid writes: packets without valid signed claims
      // are rejected at the BTP transport layer and never reach the handler.
      if (this.inboundClaimValidator) {
        const rejection = await this.inboundClaimValidator(
          messageData.protocolData,
          ilpPacket as ILPPreparePacket,
          peerConn.peerId
        );
        if (rejection) {
          childLogger.warn(
            {
              event: 'btp_claim_validation_rejected',
              peerId: peerConn.peerId,
              destination: (ilpPacket as ILPPreparePacket).destination,
              errorCode: rejection.code,
              reason: rejection.message,
            },
            'ILP PREPARE rejected: claim validation failed'
          );

          const rejectBuffer = serializePacket(rejection);
          const btpRejectResponse: BTPMessage = {
            type: BTPMessageType.RESPONSE,
            requestId: message.requestId,
            data: {
              protocolData: [],
              ilpPacket: rejectBuffer,
            },
          };
          peerConn.ws.send(serializeBTPMessage(btpRejectResponse));
          return;
        }
      }

      // Process packet through PacketHandler (pass peer ID for settlement tracking)
      const response = await this.packetHandler.handlePreparePacket(
        ilpPacket as ILPPreparePacket,
        peerConn.peerId
      );

      // Serialize ILP response
      const responseBuffer = serializePacket(response);

      // Wrap in BTP RESPONSE
      const btpResponse: BTPMessage = {
        type: BTPMessageType.RESPONSE,
        requestId: message.requestId,
        data: {
          protocolData: [],
          ilpPacket: responseBuffer,
        },
      };

      // Send BTP RESPONSE back to peer
      peerConn.ws.send(serializeBTPMessage(btpResponse));

      const responseType = response.type === 13 ? 'FULFILL' : 'REJECT';

      childLogger.info(
        {
          event: 'btp_response_sent',
          peerId: peerConn.peerId,
          responseType,
          requestId: message.requestId,
        },
        `BTP RESPONSE sent (${responseType})`
      );
    } catch (error) {
      childLogger.error(
        {
          event: 'btp_message_processing_error',
          peerId: peerConn.peerId,
          requestId: message.requestId,
          error: error instanceof Error ? error.message : String(error),
        },
        'Error processing BTP MESSAGE'
      );

      // Send BTP ERROR response
      const btpError =
        error instanceof BTPError
          ? error
          : new BTPError('F00', 'Internal error processing message');

      const errorMessage: BTPMessage = {
        type: BTPMessageType.ERROR,
        requestId: message.requestId,
        data: btpError.toBTPErrorData(),
      };

      if (peerConn.ws.readyState === WebSocket.OPEN) {
        peerConn.ws.send(serializeBTPMessage(errorMessage));
      }

      throw error; // Re-throw to trigger outer error handler
    }
  }
}
