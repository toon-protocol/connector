/**
 * TelemetryEmitter - WebSocket client for sending telemetry to dashboard
 * @packageDocumentation
 * @remarks
 * Non-blocking telemetry emission to dashboard via WebSocket.
 * Implements automatic reconnection with exponential backoff.
 * All failures are logged but never throw to prevent impacting packet processing.
 */

import WebSocket from 'ws';
import { ILPPreparePacket, RoutingTableEntry, TelemetryEvent } from '@crosstown/shared';
import { Logger } from '../utils/logger';
import {
  TelemetryMessage,
  NodeStatusData,
  PacketReceivedData,
  PacketSentData,
  RouteLookupData,
  LogTelemetryData,
  PeerStatus,
} from './types';
import {
  TelemetryBuffer,
  TelemetryBufferConfig,
  TelemetryEvent as BufferEvent,
} from './telemetry-buffer';
import type { EventStore } from '../explorer/event-store';

/**
 * TelemetryEmitter class - Sends telemetry events to dashboard telemetry server
 * @remarks
 * Design principles:
 * - Non-blocking: All emit methods are void, never throw errors
 * - Resilient: Automatic reconnection with exponential backoff
 * - Optional: Connector functions identically with or without telemetry
 */
export class TelemetryEmitter {
  private _ws: WebSocket | null = null;
  private _connected: boolean = false;
  private readonly _dashboardUrl: string;
  private readonly _nodeId: string;
  private readonly _logger: Logger;
  private _reconnectDelay: number = 1000; // Start at 1 second
  private _reconnectTimeout: NodeJS.Timeout | null = null;
  private _intentionalDisconnect: boolean = false; // Track if disconnect was intentional
  private _buffer: TelemetryBuffer | null = null; // Optional buffer for batching (Story 12.5)
  private _eventStore: EventStore | null = null; // Optional EventStore for persistence (Story 14.1)
  private _eventCallbacks: Set<(event: TelemetryEvent) => void> = new Set(); // Callbacks for Explorer (Story 14.2)

  /**
   * Create a TelemetryEmitter instance
   * @param dashboardUrl - WebSocket URL of dashboard telemetry server (e.g., ws://dashboard:9000)
   * @param nodeId - Connector node ID for telemetry message identification
   * @param logger - Pino logger instance for logging telemetry events
   * @param eventStore - Optional EventStore for persisting telemetry events (Story 14.1)
   */
  constructor(dashboardUrl: string, nodeId: string, logger: Logger, eventStore?: EventStore) {
    this._dashboardUrl = dashboardUrl;
    this._nodeId = nodeId;
    this._logger = logger;
    this._eventStore = eventStore ?? null;
  }

  /**
   * Create a TelemetryEmitter with buffering enabled for high-throughput scenarios
   *
   * When buffering is enabled, telemetry events are accumulated and flushed in batches,
   * reducing WebSocket overhead and improving performance at high packet rates.
   *
   * @param dashboardUrl - WebSocket URL of dashboard telemetry server
   * @param nodeId - Connector node ID for telemetry message identification
   * @param logger - Pino logger instance
   * @param bufferConfig - Buffer configuration (size and flush interval)
   * @param eventStore - Optional EventStore for persisting telemetry events (Story 14.1)
   * @returns TelemetryEmitter with buffering enabled
   *
   * [Source: Epic 12 Story 12.5 Task 5.2 - TelemetryBuffer integration]
   */
  static withBuffer(
    dashboardUrl: string,
    nodeId: string,
    logger: Logger,
    bufferConfig: TelemetryBufferConfig = { bufferSize: 1000, flushIntervalMs: 100 },
    eventStore?: EventStore
  ): TelemetryEmitter {
    const emitter = new TelemetryEmitter(dashboardUrl, nodeId, logger, eventStore);

    // Create buffer with flush callback that sends batch via WebSocket
    emitter._buffer = new TelemetryBuffer(
      bufferConfig,
      (events: BufferEvent[]) => {
        emitter._flushBufferedEvents(events);
      },
      logger
    );

    logger.info(
      {
        bufferSize: bufferConfig.bufferSize,
        flushIntervalMs: bufferConfig.flushIntervalMs,
      },
      'TelemetryEmitter created with buffering enabled'
    );

    return emitter;
  }

  /**
   * Flush buffered events via WebSocket (internal)
   * @param events - Array of buffered telemetry events
   */
  private _flushBufferedEvents(events: BufferEvent[]): void {
    if (!this._connected || !this._ws || events.length === 0) {
      return;
    }

    try {
      // Convert buffered events back to TelemetryMessage format for sending
      const messages: TelemetryMessage[] = events.map((event) => ({
        type: event.eventType as TelemetryMessage['type'],
        nodeId: this._nodeId,
        timestamp: new Date(event.timestamp).toISOString(),
        data: event.data as unknown as TelemetryMessage['data'],
      }));

      // Send batch as single message array
      this._ws.send(JSON.stringify({ batch: messages }));

      this._logger.debug({ eventCount: events.length }, 'Flushed buffered telemetry events');
    } catch (error) {
      this._logger.warn(
        { error, eventCount: events.length },
        'Failed to flush buffered telemetry events'
      );
    }
  }

  /**
   * Check if buffering is enabled
   * @returns True if TelemetryBuffer is active
   */
  isBufferingEnabled(): boolean {
    return this._buffer !== null;
  }

  /**
   * Connect to dashboard telemetry server
   * @remarks
   * Establishes WebSocket connection with automatic reconnection on failure.
   * Connection failures are logged but do not throw errors.
   * Safe to call during connector startup - failures won't prevent startup.
   */
  async connect(): Promise<void> {
    return new Promise((resolve, reject) => {
      try {
        this._intentionalDisconnect = false; // Reset flag when connecting
        this._ws = new WebSocket(this._dashboardUrl);
        let settled = false; // Track if promise has been settled

        this._ws.on('open', () => {
          this._connected = true;
          this._reconnectDelay = 1000; // Reset backoff on successful connection
          this._logger.info(
            { event: 'telemetry_connected', dashboardUrl: this._dashboardUrl },
            'Telemetry connected to dashboard'
          );
          if (!settled) {
            settled = true;
            resolve(); // Resolve promise when connection is open
          }
        });

        this._ws.on('error', (error) => {
          this._logger.warn(
            { event: 'telemetry_error', error: error.message },
            'Telemetry connection error'
          );
          this._connected = false;
          // Only reject if we haven't successfully connected yet
          if (!settled) {
            settled = true;
            reject(error);
          }
        });

        this._ws.on('close', () => {
          this._connected = false;
          this._logger.info({ event: 'telemetry_disconnected' }, 'Telemetry disconnected');
          this._scheduleReconnect();
        });
      } catch (error) {
        this._logger.warn(
          { event: 'telemetry_connect_failed', error },
          'Failed to connect telemetry'
        );
        this._scheduleReconnect();
        reject(error);
      }
    });
  }

  /**
   * Disconnect from dashboard telemetry server
   * @remarks
   * Gracefully closes WebSocket connection.
   * Call during connector shutdown.
   */
  async disconnect(): Promise<void> {
    // Set flag to prevent auto-reconnect
    this._intentionalDisconnect = true;

    // Cancel any pending reconnection attempts
    if (this._reconnectTimeout) {
      clearTimeout(this._reconnectTimeout);
      this._reconnectTimeout = null;
    }

    // Shutdown buffer if active (flushes remaining events and stops timer)
    if (this._buffer) {
      this._buffer.shutdown();
    }

    if (this._ws) {
      // Only close if WebSocket is open; use terminate() if still connecting
      if (this._ws.readyState === WebSocket.OPEN) {
        this._ws.close();
      } else if (this._ws.readyState === WebSocket.CONNECTING) {
        // Suppress the expected "WebSocket was closed before the connection
        // was established" error that fires when terminating a connecting socket
        this._ws.removeAllListeners('error');
        this._ws.on('error', () => {});
        this._ws.terminate();
      }
      // If CLOSING or CLOSED, no action needed
      this._ws = null;
    }
    this._connected = false;
    this._logger.info({ event: 'telemetry_disconnect' }, 'Telemetry disconnected');
  }

  /**
   * Check if telemetry is currently connected
   * @returns true if WebSocket connection is active, false otherwise
   */
  isConnected(): boolean {
    return this._connected;
  }

  /**
   * Register callback for telemetry events (used by Explorer for live streaming)
   *
   * @param callback - Function to call when event is emitted
   * @returns Unsubscribe function
   *
   * @example
   * ```typescript
   * const unsubscribe = telemetryEmitter.onEvent((event) => {
   *   broadcaster.broadcast(event);
   * });
   *
   * // Later, to unsubscribe:
   * unsubscribe();
   * ```
   *
   * [Source: Story 14.2 Task 4 - Explorer callback registration]
   */
  onEvent(callback: (event: TelemetryEvent) => void): () => void {
    this._eventCallbacks.add(callback);
    return () => this._eventCallbacks.delete(callback);
  }

  /**
   * Emit NODE_STATUS telemetry to dashboard
   * @param routes - Current routing table entries
   * @param peers - Peer status information
   * @param health - Connector health status
   * @remarks
   * Non-blocking: Errors are logged but never thrown.
   * Called on connector startup and periodically for health monitoring.
   */
  emitNodeStatus(
    routes: RoutingTableEntry[],
    peers: PeerStatus[],
    health: 'healthy' | 'unhealthy' | 'starting'
  ): void {
    const startTime = process.uptime();
    const peersConnected = peers.filter((p) => p.connected).length;

    const data: NodeStatusData = {
      routes,
      peers,
      health,
      uptime: Math.floor(startTime),
      peersConnected,
      totalPeers: peers.length,
    };

    const message: TelemetryMessage = {
      type: 'NODE_STATUS',
      nodeId: this._nodeId,
      timestamp: new Date().toISOString(),
      data,
    };

    this._sendTelemetry(message);
  }

  /**
   * Emit PACKET_RECEIVED telemetry to dashboard
   * @param packet - ILP Prepare packet that was received
   * @param source - Source peer identifier (may be "unknown")
   * @remarks
   * Non-blocking: Errors are logged but never thrown.
   * Called when ILP packet arrives at connector via BTP.
   */
  emitPacketReceived(packet: ILPPreparePacket, source: string): void {
    const packetId = packet.executionCondition.toString('hex');

    const data: PacketReceivedData = {
      packetId,
      packetType: 'PREPARE',
      source,
      destination: packet.destination,
      amount: packet.amount.toString(),
    };

    const message: TelemetryMessage = {
      type: 'PACKET_RECEIVED',
      nodeId: this._nodeId,
      timestamp: new Date().toISOString(),
      data,
    };

    this._sendTelemetry(message);
  }

  /**
   * Emit PACKET_SENT telemetry to dashboard
   * @param packetId - Unique packet identifier (executionCondition hex string)
   * @param nextHop - Next hop peer identifier
   * @remarks
   * Non-blocking: Errors are logged but never thrown.
   * Called after connector forwards packet to next hop via BTP.
   */
  emitPacketSent(packetId: string, nextHop: string): void {
    const data: PacketSentData = {
      packetId,
      nextHop,
      timestamp: new Date().toISOString(),
    };

    const message: TelemetryMessage = {
      type: 'PACKET_SENT',
      nodeId: this._nodeId,
      timestamp: new Date().toISOString(),
      data,
    };

    this._sendTelemetry(message);
  }

  /**
   * Emit ROUTE_LOOKUP telemetry to dashboard
   * @param destination - ILP destination address
   * @param selectedPeer - Selected next hop peer (null if no route found)
   * @param reason - Routing decision reason (e.g., "longest prefix match", "no route found")
   * @remarks
   * Non-blocking: Errors are logged but never thrown.
   * Called after routing table lookup during packet processing.
   */
  emitRouteLookup(destination: string, selectedPeer: string | null, reason: string): void {
    const data: RouteLookupData = {
      destination,
      selectedPeer,
      reason,
    };

    const message: TelemetryMessage = {
      type: 'ROUTE_LOOKUP',
      nodeId: this._nodeId,
      timestamp: new Date().toISOString(),
      data,
    };

    this._sendTelemetry(message);
  }

  /**
   * Emit LOG telemetry to dashboard
   * @param logEntry - Log entry data from Pino transport
   * @remarks
   * Non-blocking: Errors are logged but never thrown.
   * Called by Pino transport for every log entry generated by connector.
   * This method is the bridge between Pino logging and telemetry emission.
   */
  emitLog(logEntry: LogTelemetryData): void {
    const message: TelemetryMessage = {
      type: 'LOG',
      nodeId: this._nodeId,
      timestamp: new Date().toISOString(),
      data: logEntry,
    };

    this._sendTelemetry(message);
  }

  /**
   * Emit settlement telemetry event (Story 6.8)
   * @param event - Settlement telemetry event (ACCOUNT_BALANCE, SETTLEMENT_TRIGGERED, SETTLEMENT_COMPLETED)
   * @remarks
   * Non-blocking: Errors are logged but never thrown.
   * Supports new shared telemetry event types from @crosstown/shared package.
   * All bigint fields must be pre-converted to strings by caller.
   * Called by AccountManager, SettlementMonitor, and SettlementAPI.
   *
   * @example
   * ```typescript
   * telemetryEmitter.emit({
   *   type: 'ACCOUNT_BALANCE',
   *   nodeId: 'connector-a',
   *   peerId: 'peer-b',
   *   tokenId: 'M2M',
   *   debitBalance: '0',
   *   creditBalance: '1000',
   *   netBalance: '-1000',
   *   creditLimit: '10000',
   *   settlementThreshold: '5000',
   *   settlementState: SettlementState.IDLE,
   *   timestamp: new Date().toISOString()
   * });
   * ```
   */
  emit(event: TelemetryEvent): void {
    // Persist to EventStore if configured (Story 14.1)
    // Non-blocking: failures logged but never thrown
    if (this._eventStore) {
      this._eventStore.storeEvent(event).catch((error) => {
        this._logger.warn({ error, eventType: event.type }, 'Failed to store telemetry event');
      });
    }

    // Invoke registered callbacks (Story 14.2 - Explorer live streaming)
    // Non-blocking: callback failures logged but never thrown
    for (const callback of this._eventCallbacks) {
      try {
        callback(event);
      } catch (error) {
        this._logger.warn({ error }, 'Event callback failed');
      }
    }

    // Emit via WebSocket
    if (!this._connected || !this._ws) {
      this._logger.debug(
        { event: 'telemetry_not_connected', eventType: event.type },
        'Telemetry not connected, skipping settlement event emission'
      );
      return;
    }

    try {
      const json = JSON.stringify(event);
      this._ws.send(json);
      this._logger.debug(
        { event: 'settlement_telemetry_sent', eventType: event.type },
        'Settlement telemetry event sent'
      );
    } catch (error) {
      this._logger.warn(
        { event: 'settlement_telemetry_send_failed', eventType: event.type, error },
        'Failed to send settlement telemetry event'
      );
      // Do NOT throw - this is non-blocking (Story 6.8 requirement)
    }
  }

  /**
   * Send telemetry message to dashboard via WebSocket
   * @private
   * @param message - Telemetry message to send
   * @remarks
   * Non-blocking: All errors are caught and logged.
   * If not connected, message is silently dropped (logged at DEBUG level).
   * Send failures are logged at WARN level but never throw.
   */
  private _sendTelemetry(message: TelemetryMessage): void {
    // If buffering is enabled, add to buffer instead of sending directly
    if (this._buffer) {
      const bufferEvent: BufferEvent = {
        eventType: message.type,
        timestamp: Date.now(),
        data: message.data as unknown as Record<string, unknown>,
        metadata: { nodeId: message.nodeId },
      };
      this._buffer.addEvent(bufferEvent);
      return;
    }

    // Direct send (no buffering)
    if (!this._connected || !this._ws) {
      this._logger.debug(
        { event: 'telemetry_not_connected', messageType: message.type },
        'Telemetry not connected, skipping emission'
      );
      return;
    }

    try {
      const json = JSON.stringify(message);
      this._ws.send(json);
      this._logger.debug(
        { event: 'telemetry_sent', messageType: message.type },
        'Telemetry message sent'
      );
    } catch (error) {
      this._logger.warn(
        { event: 'telemetry_send_failed', messageType: message.type, error },
        'Failed to send telemetry'
      );
      // Do NOT throw - this is non-blocking
    }
  }

  /**
   * Schedule reconnection attempt with exponential backoff
   * @private
   * @remarks
   * Backoff schedule: 1s, 2s, 4s, 8s, 16s (max)
   * Reconnection attempts continue indefinitely (dashboard may restart)
   */
  private _scheduleReconnect(): void {
    // Don't reconnect if disconnect was intentional
    if (this._intentionalDisconnect) {
      return;
    }

    // Cancel any existing reconnection timeout
    if (this._reconnectTimeout) {
      clearTimeout(this._reconnectTimeout);
    }

    this._reconnectTimeout = setTimeout(() => {
      this._logger.debug(
        { event: 'telemetry_reconnect_attempt', delay: this._reconnectDelay },
        'Attempting telemetry reconnection'
      );
      void this.connect();
    }, this._reconnectDelay);

    // Exponential backoff with max 16 seconds
    this._reconnectDelay = Math.min(this._reconnectDelay * 2, 16000);
  }
}
