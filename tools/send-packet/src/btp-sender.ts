/**
 * BTP Sender for Test Packet Transmission
 *
 * Simplified BTP client for sending test ILP packets to connector nodes.
 * Based on BTPClient pattern but streamlined for CLI tool usage.
 */

import WebSocket from 'ws';
import pino from 'pino';
import {
  ILPPreparePacket,
  ILPFulfillPacket,
  ILPRejectPacket,
  serializePacket,
  deserializePacket,
} from '@toon-protocol/shared';

// Copy essential BTP types locally since we don't export them from connector
enum BTPMessageType {
  RESPONSE = 1,
  ERROR = 2,
  MESSAGE = 6,
}

interface BTPProtocolData {
  protocolName: string;
  contentType: number;
  data: Buffer;
}

interface BTPData {
  protocolData: BTPProtocolData[];
  ilpPacket?: Buffer;
}

interface BTPErrorData {
  code: string;
  name: string;
  triggeredAt: string;
  data: Buffer;
}

interface BTPMessage {
  type: BTPMessageType;
  requestId: number;
  data: BTPData | BTPErrorData;
}

/**
 * Custom errors
 */
export class BTPConnectionError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'BTPConnectionError';
  }
}

export class BTPAuthenticationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'BTPAuthenticationError';
  }
}

export class PacketTimeoutError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'PacketTimeoutError';
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
 * Type guard for BTPErrorData
 */
function isBTPErrorData(message: BTPMessage): message is BTPMessage & { data: BTPErrorData } {
  return message.type === BTPMessageType.ERROR;
}

/**
 * Parse BTP message from buffer - simplified version
 */
function parseBTPMessage(buffer: Buffer): BTPMessage {
  if (buffer.length < 5) {
    throw new Error('BTP message too short');
  }

  let offset = 0;

  // Read message type (1 byte)
  const type = buffer.readUInt8(offset);
  offset += 1;

  // Read request ID (4 bytes, big-endian uint32)
  const requestId = buffer.readUInt32BE(offset);
  offset += 4;

  // Parse data based on type
  let data: BTPData | BTPErrorData;

  if (type === BTPMessageType.ERROR) {
    // Parse error data
    const codeLength = buffer.readUInt8(offset);
    offset += 1;
    const code = buffer.subarray(offset, offset + codeLength).toString('utf8');
    offset += codeLength;

    const nameLength = buffer.readUInt8(offset);
    offset += 1;
    const name = buffer.subarray(offset, offset + nameLength).toString('utf8');
    offset += nameLength;

    const triggeredAtLength = buffer.readUInt8(offset);
    offset += 1;
    const triggeredAt = buffer.subarray(offset, offset + triggeredAtLength).toString('utf8');
    offset += triggeredAtLength;

    const dataLength = buffer.readUInt32BE(offset);
    offset += 4;
    const errorData = buffer.subarray(offset, offset + dataLength);

    data = { code, name, triggeredAt, data: errorData };
  } else {
    // Parse message data
    const protocolDataCount = buffer.readUInt8(offset);
    offset += 1;

    const protocolData: BTPProtocolData[] = [];
    for (let i = 0; i < protocolDataCount; i++) {
      const protocolNameLength = buffer.readUInt8(offset);
      offset += 1;
      const protocolName = buffer.subarray(offset, offset + protocolNameLength).toString('utf8');
      offset += protocolNameLength;

      const contentType = buffer.readUInt16BE(offset);
      offset += 2;

      const dataLength = buffer.readUInt32BE(offset);
      offset += 4;
      const protoData = buffer.subarray(offset, offset + dataLength);
      offset += dataLength;

      protocolData.push({ protocolName, contentType, data: protoData });
    }

    const ilpPacketLength = buffer.readUInt32BE(offset);
    offset += 4;

    let ilpPacket: Buffer | undefined;
    if (ilpPacketLength > 0) {
      ilpPacket = buffer.subarray(offset, offset + ilpPacketLength);
    }

    data = { protocolData, ilpPacket };
  }

  return { type: type as BTPMessageType, requestId, data };
}

/**
 * Serialize BTP message to buffer
 */
function serializeBTPMessage(message: BTPMessage): Buffer {
  const buffers: Buffer[] = [];

  // Write message type (1 byte)
  const typeBuffer = Buffer.allocUnsafe(1);
  typeBuffer.writeUInt8(message.type, 0);
  buffers.push(typeBuffer);

  // Write request ID (4 bytes, big-endian uint32)
  const requestIdBuffer = Buffer.allocUnsafe(4);
  requestIdBuffer.writeUInt32BE(message.requestId, 0);
  buffers.push(requestIdBuffer);

  // Serialize data
  if (isBTPErrorData(message)) {
    // Serialize error data
    const { code, name, triggeredAt, data } = message.data;

    // Code length + code
    buffers.push(Buffer.from([code.length]));
    buffers.push(Buffer.from(code, 'utf8'));

    // Name length + name
    buffers.push(Buffer.from([name.length]));
    buffers.push(Buffer.from(name, 'utf8'));

    // TriggeredAt length + triggeredAt
    buffers.push(Buffer.from([triggeredAt.length]));
    buffers.push(Buffer.from(triggeredAt, 'utf8'));

    // Data length (4 bytes) + data
    const dataLengthBuffer = Buffer.allocUnsafe(4);
    dataLengthBuffer.writeUInt32BE(data.length, 0);
    buffers.push(dataLengthBuffer);
    buffers.push(data);
  } else {
    // Serialize message data
    const { protocolData, ilpPacket } = message.data as BTPData;

    // Protocol data count
    buffers.push(Buffer.from([protocolData.length]));

    // Serialize each protocol data entry
    for (const proto of protocolData) {
      // Protocol name length + name
      buffers.push(Buffer.from([proto.protocolName.length]));
      buffers.push(Buffer.from(proto.protocolName, 'utf8'));

      // Content type (2 bytes)
      const contentTypeBuffer = Buffer.allocUnsafe(2);
      contentTypeBuffer.writeUInt16BE(proto.contentType, 0);
      buffers.push(contentTypeBuffer);

      // Data length (4 bytes) + data
      const dataLengthBuffer = Buffer.allocUnsafe(4);
      dataLengthBuffer.writeUInt32BE(proto.data.length, 0);
      buffers.push(dataLengthBuffer);
      buffers.push(proto.data);
    }

    // ILP packet length (4 bytes) + packet
    const ilpPacketBuffer = ilpPacket ?? Buffer.alloc(0);
    const ilpLengthBuffer = Buffer.allocUnsafe(4);
    ilpLengthBuffer.writeUInt32BE(ilpPacketBuffer.length, 0);
    buffers.push(ilpLengthBuffer);
    if (ilpPacketBuffer.length > 0) {
      buffers.push(ilpPacketBuffer);
    }
  }

  return Buffer.concat(buffers);
}

/**
 * BTPSender - Simplified BTP client for sending test packets
 */
export class BTPSender {
  private readonly _connectorUrl: string;
  private readonly _authToken: string;
  private readonly _logger: pino.Logger;
  private _ws: WebSocket | null = null;
  private _requestIdCounter = 0;
  private _pendingRequests: Map<number, PendingRequest> = new Map();

  constructor(connectorUrl: string, authToken: string, logger: pino.Logger) {
    this._connectorUrl = connectorUrl;
    this._authToken = authToken;
    this._logger = logger;
  }

  /**
   * Connect to connector and authenticate
   */
  async connect(): Promise<void> {
    this._logger.info({ connectorUrl: this._connectorUrl }, 'Connecting to connector');

    return new Promise<void>((resolve, reject) => {
      this._ws = new WebSocket(this._connectorUrl);

      this._ws.on('open', async () => {
        try {
          await this._authenticate();
          this._logger.info('Connected and authenticated');

          // Set up message handler
          this._ws?.on('message', (data: Buffer) => {
            this._handleMessage(data);
          });

          resolve();
        } catch (error) {
          reject(error);
        }
      });

      this._ws.on('error', (error) => {
        this._logger.error({ error: error.message }, 'WebSocket error');
        reject(new BTPConnectionError(`WebSocket error: ${error.message}`));
      });

      this._ws.on('close', () => {
        this._logger.info('WebSocket closed');
        this._rejectAllPendingRequests(new BTPConnectionError('Connection closed'));
      });
    });
  }

  /**
   * Send ILP packet to connector
   */
  async sendPacket(packet: ILPPreparePacket): Promise<ILPFulfillPacket | ILPRejectPacket> {
    if (!this._ws || this._ws.readyState !== WebSocket.OPEN) {
      throw new BTPConnectionError('Not connected to connector');
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
        protocolData: [],
        ilpPacket: serializedPacket,
      },
    };

    // Encode BTP MESSAGE
    const btpBuffer = serializeBTPMessage(btpMessage);

    this._logger.info(
      {
        requestId,
        destination: packet.destination,
        amount: packet.amount.toString(),
      },
      'Packet sent'
    );

    // Send via WebSocket
    this._ws.send(btpBuffer);

    // Wait for response with timeout
    return new Promise<ILPFulfillPacket | ILPRejectPacket>((resolve, reject) => {
      const timeoutId = setTimeout(() => {
        this._pendingRequests.delete(requestId);
        reject(new PacketTimeoutError('Packet send timeout (10 seconds)'));
      }, 10000);

      this._pendingRequests.set(requestId, {
        resolve,
        reject,
        timeoutId,
      });
    });
  }

  /**
   * Disconnect from connector
   */
  async disconnect(): Promise<void> {
    if (this._ws) {
      this._logger.info('Disconnecting from connector');
      this._ws.close();
      this._ws = null;
    }

    this._rejectAllPendingRequests(new BTPConnectionError('Disconnected'));
  }

  /**
   * Perform BTP authentication handshake
   */
  private async _authenticate(): Promise<void> {
    this._logger.info('Authenticating');

    // Send auth data in JSON format: { "peerId": "send-packet-client", "secret": "token" }
    const authData = {
      peerId: 'send-packet-client',
      secret: this._authToken,
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
      },
    };

    const authBuffer = serializeBTPMessage(authMessage);

    if (!this._ws) {
      throw new BTPAuthenticationError('WebSocket not connected');
    }

    return new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this._logger.error('Authentication timeout');
        reject(new BTPAuthenticationError('Authentication timeout'));
      }, 5000);

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
              this._logger.error({ errorCode: errorData.code }, 'Authentication failed');
              reject(new BTPAuthenticationError(`Authentication failed: ${errorData.code}`));
            } else if (message.type === BTPMessageType.RESPONSE) {
              this._logger.info('Authentication successful');
              resolve();
            }
          }
        } catch (error) {
          clearTimeout(timeout);
          this._ws?.removeListener('message', authHandler);
          const errorMessage = error instanceof Error ? error.message : String(error);
          this._logger.error({ error: errorMessage }, 'Authentication error');
          reject(new BTPAuthenticationError(errorMessage));
        }
      };

      this._ws?.on('message', authHandler);
      this._ws?.send(authBuffer);
    });
  }

  /**
   * Handle incoming BTP message
   */
  private _handleMessage(data: Buffer): void {
    try {
      const message = parseBTPMessage(data);

      // Handle RESPONSE or ERROR messages
      if (message.type === BTPMessageType.RESPONSE || message.type === BTPMessageType.ERROR) {
        const pending = this._pendingRequests.get(message.requestId);
        if (pending) {
          clearTimeout(pending.timeoutId);
          this._pendingRequests.delete(message.requestId);

          if (message.type === BTPMessageType.ERROR) {
            const errorData = isBTPErrorData(message)
              ? message.data
              : { code: 'UNKNOWN', name: 'Unknown error', data: Buffer.alloc(0) };
            this._logger.error(
              { errorCode: errorData.code, errorName: errorData.name },
              'Received ERROR response'
            );
            pending.reject(new Error(`BTP Error: ${errorData.code} - ${errorData.name}`));
          } else {
            // Decode ILP packet from response
            const ilpPacket = (message.data as BTPData).ilpPacket;
            if (ilpPacket) {
              const responsePacket = deserializePacket(ilpPacket);
              this._logger.info({ packetType: responsePacket.type }, 'Received packet response');
              pending.resolve(responsePacket as ILPFulfillPacket | ILPRejectPacket);
            } else {
              pending.reject(new Error('No ILP packet in BTP RESPONSE'));
            }
          }
        }
      }
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      this._logger.error({ error: errorMessage }, 'Failed to handle BTP message');
    }
  }

  /**
   * Generate unique request ID
   */
  private _generateRequestId(): number {
    this._requestIdCounter = (this._requestIdCounter + 1) & 0xffffffff; // Keep within uint32 range
    return this._requestIdCounter;
  }

  /**
   * Reject all pending requests
   */
  private _rejectAllPendingRequests(error: Error): void {
    for (const [requestId, pending] of this._pendingRequests.entries()) {
      clearTimeout(pending.timeoutId);
      pending.reject(error);
      this._pendingRequests.delete(requestId);
    }
  }
}
