/**
 * ILP Packet Handler - Core forwarding logic for ILPv4 packets
 * @packageDocumentation
 * @see {@link https://github.com/interledger/rfcs/blob/master/0027-interledger-protocol-4/0027-interledger-protocol-4.md|RFC-0027: Interledger Protocol v4}
 */

import {
  ILPPreparePacket,
  ILPFulfillPacket,
  ILPRejectPacket,
  ILPErrorCode,
  PacketType,
  isValidILPAddress,
} from '@crosstown/shared';
import { RoutingTable } from '../routing/routing-table';
import { Logger, generateCorrelationId } from '../utils/logger';
import { BTPClientManager } from '../btp/btp-client-manager';
import { BTPServer } from '../btp/btp-server';
import { BTPConnectionError, BTPAuthenticationError } from '../btp/btp-client';
import { TelemetryEmitter } from '../telemetry/telemetry-emitter';
import { AccountManager } from '../settlement/account-manager';
import {
  SettlementConfig,
  LocalDeliveryConfig,
  LocalDeliveryHandler,
  LocalDeliveryRequest,
  LocalDeliveryResponse,
} from '../config/types';
import { AccountLedgerCodes } from '../settlement/types';
import { EventStore } from '../explorer/event-store';
import { EventBroadcaster } from '../explorer/event-broadcaster';
import { LocalDeliveryClient } from './local-delivery-client';
import type { PerPacketClaimService } from '../settlement/per-packet-claim-service';

/**
 * Packet validation result
 */
interface ValidationResult {
  /** Whether packet passed validation */
  isValid: boolean;
  /** Error code if validation failed */
  errorCode?: ILPErrorCode;
  /** Human-readable error message if validation failed */
  errorMessage?: string;
}

/**
 * Expiry safety margin in milliseconds
 * @remarks
 * Per RFC-0027, connectors must decrement packet expiry to prevent timeout during forwarding.
 * Default safety margin of 1000ms (1 second) provides buffer for network latency.
 */
const EXPIRY_SAFETY_MARGIN_MS = 1000;

/**
 * PacketHandler - Implements ILPv4 packet forwarding logic
 * @remarks
 * Handles ILP Prepare packets by:
 * 1. Validating packet structure and expiration time per RFC-0027
 * 2. Looking up next-hop peer using routing table
 * 3. Decrementing packet expiry by safety margin
 * 4. Forwarding to next-hop peer (integration point for Epic 2)
 * 5. Generating ILP Reject packets for errors
 *
 * @see {@link https://github.com/interledger/rfcs/blob/master/0027-interledger-protocol-4/0027-interledger-protocol-4.md|RFC-0027: Interledger Protocol v4}
 */
export class PacketHandler {
  /**
   * Routing table for next-hop lookups
   */
  private readonly routingTable: RoutingTable;

  /**
   * BTP client manager for packet forwarding to outbound peers
   */
  private readonly btpClientManager: BTPClientManager;

  /**
   * BTP server for packet forwarding to incoming authenticated peers
   */
  private btpServer: BTPServer | null;

  /**
   * Logger instance for structured logging
   * @remarks
   * Pino logger for structured JSON logging with correlation IDs
   */
  private readonly logger: Logger;

  /**
   * Connector node ID for triggeredBy field in reject packets
   */
  private readonly nodeId: string;

  /**
   * Telemetry emitter for sending telemetry to dashboard (optional)
   */
  private readonly telemetryEmitter: TelemetryEmitter | null;

  /**
   * Event store for direct event emission in standalone mode (optional)
   */
  private eventStore: EventStore | null = null;

  /**
   * Event broadcaster for real-time WebSocket event streaming (optional)
   */
  private eventBroadcaster: EventBroadcaster | null = null;

  /**
   * Per-packet claim service for attaching signed claims to outgoing packets (optional)
   */
  private perPacketClaimService: PerPacketClaimService | null = null;

  /**
   * Account manager for settlement recording (optional)
   * @remarks
   * When provided, enables settlement recording for packet forwarding.
   * Null if settlement is disabled (backward compatibility).
   * Not readonly to support late initialization via setSettlement().
   */
  private accountManager: AccountManager | null;

  /**
   * Settlement configuration (optional)
   * @remarks
   * Contains connector fee percentage and TigerBeetle connection settings.
   * Null if settlement is disabled.
   * Not readonly to support late initialization via setSettlement().
   */
  private settlementConfig: SettlementConfig | null;

  /**
   * Local delivery client for forwarding to agent runtime via HTTP (optional)
   * @remarks
   * When enabled, packets destined for local addresses are forwarded
   * via HTTP to an external agent runtime instead of auto-fulfilling.
   */
  private localDeliveryClient: LocalDeliveryClient | null = null;

  /**
   * Function handler for in-process local delivery (optional)
   * @remarks
   * When set, takes priority over HTTP LocalDeliveryClient. Allows
   * direct in-process packet delivery without HTTP round-trip.
   */
  private localDeliveryHandler: LocalDeliveryHandler | null = null;

  /**
   * Creates a new PacketHandler instance
   * @param routingTable - Routing table for next-hop lookups
   * @param btpClientManager - BTP client manager for forwarding packets to outbound peers
   * @param nodeId - Connector node ID for reject packet triggeredBy field
   * @param logger - Pino logger instance for structured logging
   * @param telemetryEmitter - Optional telemetry emitter for dashboard reporting
   * @param btpServer - Optional BTP server for forwarding to incoming authenticated peers
   * @param accountManager - Optional account manager for settlement recording (Story 6.4)
   * @param settlementConfig - Optional settlement configuration for fee calculation and TigerBeetle
   */
  constructor(
    routingTable: RoutingTable,
    btpClientManager: BTPClientManager,
    nodeId: string,
    logger: Logger,
    telemetryEmitter: TelemetryEmitter | null = null,
    btpServer: BTPServer | null = null,
    accountManager: AccountManager | null = null,
    settlementConfig: SettlementConfig | null = null
  ) {
    this.routingTable = routingTable;
    this.btpClientManager = btpClientManager;
    this.btpServer = btpServer;
    this.nodeId = nodeId;
    this.logger = logger;
    this.telemetryEmitter = telemetryEmitter;
    this.accountManager = accountManager;
    this.settlementConfig = settlementConfig;

    // Log settlement enabled/disabled state
    if (this.isSettlementEnabled()) {
      this.logger.info(
        {
          connectorFeePercentage: settlementConfig?.connectorFeePercentage,
          tigerBeetleClusterId: settlementConfig?.tigerBeetleClusterId,
        },
        'Settlement recording enabled'
      );
    } else {
      this.logger.info('Settlement recording disabled');
    }
  }

  /**
   * Set BTPServer reference (to resolve circular dependency during initialization)
   * @param btpServer - BTP server instance for incoming peer forwarding
   */
  setBTPServer(btpServer: BTPServer): void {
    this.btpServer = btpServer;
  }

  /**
   * Set EventStore reference for direct event emission in standalone mode
   * @param eventStore - EventStore instance for storing packet events
   * @remarks
   * Called by ConnectorNode when running in standalone mode (telemetryEmitter is null).
   * Allows PacketHandler to emit events directly to EventStore instead of via telemetry.
   */
  setEventStore(eventStore: EventStore | null): void {
    this.eventStore = eventStore;
    this.logger.info(
      { hasEventStore: eventStore !== null },
      'EventStore reference set for standalone event emission'
    );
  }

  /**
   * Set EventBroadcaster reference for real-time WebSocket event streaming
   * @param broadcaster - EventBroadcaster instance for live event streaming
   * @remarks
   * Called by ConnectorNode in standalone mode to enable real-time event broadcasting.
   * When set, PacketHandler will broadcast events to WebSocket clients in addition to storing them.
   */
  setEventBroadcaster(broadcaster: EventBroadcaster | null): void {
    this.eventBroadcaster = broadcaster;
    this.logger.info(
      { hasBroadcaster: broadcaster !== null },
      'EventBroadcaster reference set for live event streaming'
    );
  }

  /**
   * Set AccountManager and SettlementConfig for late initialization
   * @param accountManager - AccountManager instance for settlement recording
   * @param settlementConfig - Settlement configuration with fee and TigerBeetle settings
   * @remarks
   * Called after TigerBeetle initialization completes. Allows PacketHandler
   * to be created in constructor while settlement is initialized asynchronously.
   */
  setSettlement(accountManager: AccountManager, settlementConfig: SettlementConfig): void {
    this.accountManager = accountManager;
    this.settlementConfig = settlementConfig;

    if (this.isSettlementEnabled()) {
      this.logger.info(
        {
          event: 'settlement_enabled',
          connectorFeePercentage: settlementConfig.connectorFeePercentage,
          tigerBeetleClusterId: settlementConfig.tigerBeetleClusterId,
        },
        'Settlement recording enabled via late initialization'
      );
    }
  }

  /**
   * Set PerPacketClaimService for attaching signed claims to outgoing packets
   * @param service - PerPacketClaimService instance
   */
  setPerPacketClaimService(service: PerPacketClaimService): void {
    this.perPacketClaimService = service;
    this.logger.info('Per-packet claim service enabled');
  }

  /**
   * Set LocalDeliveryClient for forwarding local packets to agent runtime
   * @param config - Local delivery configuration
   * @remarks
   * When enabled, packets destined for local addresses (nextHop === nodeId || 'local')
   * are forwarded via HTTP to an external agent runtime instead of auto-fulfilling.
   * This allows custom business logic to handle payments.
   */
  setLocalDelivery(config: LocalDeliveryConfig): void {
    if (config.enabled) {
      this.localDeliveryClient = new LocalDeliveryClient(config, this.logger);
      this.logger.info(
        {
          event: 'local_delivery_enabled',
          handlerUrl: config.handlerUrl,
          timeout: config.timeout,
        },
        'Local delivery forwarding enabled'
      );
    } else {
      this.localDeliveryClient = null;
      this.logger.info('Local delivery forwarding disabled (using auto-fulfill stub)');
    }
  }

  /**
   * Set or clear the in-process local delivery function handler.
   * When set, takes priority over HTTP LocalDeliveryClient.
   * @param handler - Function handler or null to clear
   */
  setLocalDeliveryHandler(handler: LocalDeliveryHandler | null): void {
    this.localDeliveryHandler = handler;
    this.logger.info(
      { event: 'local_delivery_handler_set', hasHandler: handler !== null },
      'Local delivery function handler updated'
    );
  }

  /**
   * Check if local delivery forwarding is enabled
   * @returns True if local delivery client is configured and enabled
   */
  private isLocalDeliveryEnabled(): boolean {
    return this.localDeliveryClient !== null && this.localDeliveryClient.isEnabled();
  }

  /**
   * Check if settlement recording is enabled
   * @returns True if settlement recording is enabled, false otherwise
   * @remarks
   * Settlement is enabled when BOTH conditions are met:
   * 1. AccountManager is provided (not null)
   * 2. SettlementConfig.enableSettlement is true
   *
   * This method is used throughout packet handling to determine if
   * settlement transfers should be recorded in TigerBeetle.
   */
  private isSettlementEnabled(): boolean {
    return this.accountManager !== null && this.settlementConfig?.enableSettlement === true;
  }

  /**
   * Generate deterministic transfer ID from packet execution condition and direction
   *
   * TigerBeetle requires unique 128-bit transfer IDs. We derive them from the packet's
   * execution condition (32 bytes) combined with a direction indicator to ensure:
   * 1. Uniqueness: execution condition is cryptographically unique (SHA-256 hash)
   * 2. Determinism: same packet+direction always generates same transfer ID
   * 3. Idempotency: safe to retry transfer creation
   *
   * @param executionCondition - Packet's 32-byte execution condition
   * @param direction - 'incoming' or 'outgoing' to differentiate the two transfers
   * @returns 128-bit transfer ID as bigint
   * @private
   */
  private generateTransferId(
    executionCondition: Buffer,
    direction: 'incoming' | 'outgoing'
  ): bigint {
    // Generate unique transfer IDs per connector by incorporating nodeId
    // This ensures each connector in a multi-hop chain has unique transfer IDs
    const directionByte = direction === 'incoming' ? 0x01 : 0x02;

    // Hash nodeId to get a consistent numeric value
    const nodeIdHash = Buffer.alloc(8);
    let hash = 0;
    for (let i = 0; i < this.nodeId.length; i++) {
      hash = ((hash << 5) - hash + this.nodeId.charCodeAt(i)) | 0;
    }
    nodeIdHash.writeBigUInt64BE((BigInt(hash >>> 0) << 32n) | BigInt(hash >>> 0), 0);

    // Read first 16 bytes of execution condition as two 64-bit values
    const high = executionCondition.readBigUInt64BE(0);
    const low = executionCondition.readBigUInt64BE(8);

    // XOR with nodeId hash to make unique per connector
    const nodeIdValue = nodeIdHash.readBigUInt64BE(0);

    // Combine into 128-bit value with nodeId and direction differentiation
    const transferId = ((high ^ nodeIdValue) << 64n) | low;
    return transferId ^ BigInt(directionByte);
  }

  /**
   * Calculate connector fee for a packet amount
   * @param amount - Packet amount in smallest currency units (bigint)
   * @param feePercentage - Fee percentage (e.g., 0.1 = 0.1%)
   * @returns Fee amount in smallest currency units (bigint)
   * @remarks
   * Uses integer arithmetic to avoid floating-point precision issues.
   *
   * Fee calculation uses basis points conversion:
   * - 0.1% = 10 basis points = 10/10000
   * - Formula: fee = (amount × (feePercentage × 100)) / 10000
   *
   * Examples:
   * - amount=1000n, feePercentage=0.1 → fee=1n (0.1% of 1000)
   * - amount=100000n, feePercentage=0.1 → fee=100n (0.1% of 100000)
   * - amount=999n, feePercentage=0.1 → fee=0n (rounds down)
   *
   * Integer division rounds DOWN (floor division), which is acceptable:
   * connectors don't charge fees on very small packets (benefits micropayments).
   *
   * @throws {Error} if amount is negative or feePercentage is negative
   */
  private calculateConnectorFee(amount: bigint, feePercentage: number): bigint {
    // Input validation
    if (amount < 0n) {
      throw new Error(`Invalid amount: ${amount} (must be >= 0)`);
    }
    if (feePercentage < 0) {
      throw new Error(`Invalid fee percentage: ${feePercentage} (must be >= 0)`);
    }

    // Convert percentage to basis points (0.1% = 10 basis points)
    const basisPoints = Math.floor(feePercentage * 100);

    // Calculate fee using integer arithmetic: fee = (amount × basisPoints) / 10000
    const fee = (amount * BigInt(basisPoints)) / 10000n;

    return fee;
  }

  /**
   * Record packet transfers atomically in TigerBeetle (dual-leg double-entry)
   * @param packet - ILP Prepare packet being forwarded
   * @param fromPeerId - Peer ID who sent us the packet
   * @param toPeerId - Peer ID we're forwarding to
   * @param forwardedAmount - Amount forwarded after fee deduction
   * @param connectorFee - Connector fee collected
   * @param correlationId - Correlation ID for log tracing
   * @throws {Error} if settlement recording fails
   * @remarks
   * Records TWO transfers atomically in TigerBeetle:
   * 1. Incoming transfer: Debit peer's CREDIT account (peer owes us)
   * 2. Outgoing transfer: Credit peer's DEBIT account (we owe peer)
   *
   * Both transfers succeed or both fail (ACID guarantee via TigerBeetle batch API).
   * If settlement fails, packet forwarding is rejected with T00_INTERNAL_ERROR.
   *
   * Transfer IDs are deterministically generated from execution condition to enable
   * idempotent retries.
   */
  private async recordPacketTransfers(
    packet: ILPPreparePacket,
    fromPeerId: string,
    toPeerId: string,
    forwardedAmount: bigint,
    connectorFee: bigint,
    correlationId: string
  ): Promise<void> {
    if (!this.isSettlementEnabled()) {
      return;
    }

    const packetId = packet.executionCondition.toString('hex');

    this.logger.debug(
      {
        correlationId,
        packetId,
        fromPeerId,
        toPeerId,
        originalAmount: packet.amount.toString(),
        forwardedAmount: forwardedAmount.toString(),
        connectorFee: connectorFee.toString(),
      },
      'Processing settlement for packet'
    );

    try {
      // Generate deterministic transfer IDs for incoming and outgoing transfers
      const incomingTransferId = this.generateTransferId(packet.executionCondition, 'incoming');
      const outgoingTransferId = this.generateTransferId(packet.executionCondition, 'outgoing');

      // Record both transfers atomically via AccountManager
      // This posts two TigerBeetle transfers in a single batch:
      // 1. Incoming: Debit fromPeer's DEBIT account (increase "peer owes us")
      // 2. Outgoing: Credit toPeer's CREDIT account (increase "we owe peer")
      await this.accountManager!.recordPacketTransfers(
        fromPeerId,
        toPeerId,
        'ILP', // tokenId (default for MVP, future: multi-token support)
        packet.amount, // incoming amount
        forwardedAmount, // outgoing amount (after fee)
        incomingTransferId,
        outgoingTransferId,
        AccountLedgerCodes.DEFAULT_LEDGER,
        1 // transfer code (future: differentiate packet types)
      );

      // Log settlement success
      this.logger.info(
        {
          correlationId,
          packetId,
          fromPeerId,
          toPeerId,
          originalAmount: packet.amount.toString(),
          forwardedAmount: forwardedAmount.toString(),
          connectorFee: connectorFee.toString(),
        },
        'Settlement transfers recorded: incoming={originalAmount} from {fromPeerId}, outgoing={forwardedAmount} to {toPeerId}, fee={connectorFee}'
      );
    } catch (error) {
      this.logger.error(
        {
          correlationId,
          error: error instanceof Error ? error.message : String(error),
          packetId,
          fromPeerId,
          toPeerId,
        },
        'Settlement recording failed: {error}, rejecting packet with T00_INTERNAL_ERROR'
      );
      throw error;
    }
  }

  /**
   * Validate ILP Prepare packet structure and expiration
   * @param packet - ILP Prepare packet to validate
   * @returns Validation result with isValid flag and optional error details
   * @remarks
   * Validates per RFC-0027:
   * - All required fields present (amount, destination, executionCondition, expiresAt, data)
   * - Destination is valid ILP address format per RFC-0015
   * - Packet has not expired (current time < expiresAt)
   * - executionCondition is exactly 32 bytes
   */
  validatePacket(packet: ILPPreparePacket): ValidationResult {
    // Check all required fields present
    if (
      packet.amount === undefined ||
      !packet.destination ||
      !packet.executionCondition ||
      !packet.expiresAt ||
      !packet.data
    ) {
      this.logger.error(
        {
          packetType: packet.type,
          hasAmount: packet.amount !== undefined,
          hasDestination: !!packet.destination,
          hasExecutionCondition: !!packet.executionCondition,
          hasExpiresAt: !!packet.expiresAt,
          hasData: !!packet.data,
          errorCode: ILPErrorCode.F01_INVALID_PACKET,
        },
        'Packet validation failed: missing required fields'
      );
      return {
        isValid: false,
        errorCode: ILPErrorCode.F01_INVALID_PACKET,
        errorMessage: 'Missing required packet fields',
      };
    }

    // Validate destination ILP address format
    if (!isValidILPAddress(packet.destination)) {
      this.logger.error(
        {
          destination: packet.destination,
          errorCode: ILPErrorCode.F01_INVALID_PACKET,
        },
        'Packet validation failed: invalid ILP address format'
      );
      return {
        isValid: false,
        errorCode: ILPErrorCode.F01_INVALID_PACKET,
        errorMessage: `Invalid ILP address format: ${packet.destination}`,
      };
    }

    // Validate executionCondition is 32 bytes
    if (packet.executionCondition.length !== 32) {
      this.logger.error(
        {
          executionConditionLength: packet.executionCondition.length,
          errorCode: ILPErrorCode.F01_INVALID_PACKET,
        },
        'Packet validation failed: executionCondition must be 32 bytes'
      );
      return {
        isValid: false,
        errorCode: ILPErrorCode.F01_INVALID_PACKET,
        errorMessage: 'executionCondition must be exactly 32 bytes',
      };
    }

    // Check if packet has expired
    const currentTime = new Date();
    if (packet.expiresAt <= currentTime) {
      this.logger.error(
        {
          expiresAt: packet.expiresAt.toISOString(),
          currentTime: currentTime.toISOString(),
          errorCode: ILPErrorCode.R00_TRANSFER_TIMED_OUT,
        },
        'Packet validation failed: packet has expired'
      );
      return {
        isValid: false,
        errorCode: ILPErrorCode.R00_TRANSFER_TIMED_OUT,
        errorMessage: 'Packet has expired',
      };
    }

    return { isValid: true };
  }

  /**
   * Decrement packet expiry by safety margin
   * @param expiresAt - Original expiration timestamp
   * @param safetyMargin - Safety margin in milliseconds to subtract
   * @returns New expiration timestamp with safety margin applied
   * @remarks
   * Per RFC-0027, connectors must decrement expiry to prevent timeout during forwarding.
   * Returns null if decremented expiry would be in the past.
   */
  decrementExpiry(expiresAt: Date, safetyMargin: number): Date | null {
    const newExpiry = new Date(expiresAt.getTime() - safetyMargin);
    const currentTime = new Date();

    if (newExpiry <= currentTime) {
      this.logger.debug(
        {
          originalExpiry: expiresAt.toISOString(),
          decrementedExpiry: newExpiry.toISOString(),
          currentTime: currentTime.toISOString(),
          safetyMargin,
        },
        'Expiry decrement would create past timestamp'
      );
      return null;
    }

    this.logger.debug(
      {
        originalExpiry: expiresAt.toISOString(),
        newExpiry: newExpiry.toISOString(),
        safetyMargin,
      },
      'Decremented packet expiry'
    );

    return newExpiry;
  }

  /**
   * Generate ILP Reject packet
   * @param code - ILP error code per RFC-0027
   * @param message - Human-readable error description
   * @param triggeredBy - Address of connector that generated error
   * @returns ILP Reject packet
   * @remarks
   * Generates reject packet per RFC-0027 Section 3.3 with standard error codes:
   * - R00: Transfer Timed Out (packet expired)
   * - F02: Unreachable (no route to destination)
   * - F01: Invalid Packet (malformed packet)
   */
  generateReject(code: ILPErrorCode, message: string, triggeredBy: string): ILPRejectPacket {
    this.logger.info(
      {
        errorCode: code,
        message,
        triggeredBy,
      },
      'Generated reject packet'
    );

    return {
      type: PacketType.REJECT,
      code,
      triggeredBy,
      message,
      data: Buffer.alloc(0),
    };
  }

  /**
   * Convert LocalDeliveryResponse to ILP packet.
   * Handles fulfill, reject, and invalid (neither) cases.
   */
  private convertLocalDeliveryResponse(
    result: LocalDeliveryResponse
  ): ILPFulfillPacket | ILPRejectPacket {
    if (result.fulfill) {
      return {
        type: PacketType.FULFILL,
        fulfillment: Buffer.from(result.fulfill.fulfillment, 'base64'),
        data: result.fulfill.data ? Buffer.from(result.fulfill.data, 'base64') : Buffer.alloc(0),
      };
    } else if (result.reject) {
      return {
        type: PacketType.REJECT,
        code: (result.reject.code as ILPErrorCode) || ILPErrorCode.F99_APPLICATION_ERROR,
        triggeredBy: this.nodeId,
        message: result.reject.message || 'Rejected by agent',
        data: result.reject.data ? Buffer.from(result.reject.data, 'base64') : Buffer.alloc(0),
      };
    } else {
      return this.generateReject(
        ILPErrorCode.T00_INTERNAL_ERROR,
        'Invalid response from local delivery handler',
        this.nodeId
      );
    }
  }

  /**
   * Forward packet to next-hop peer via BTP
   * @param packet - ILP Prepare packet to forward
   * @param nextHop - Peer identifier to forward to
   * @param correlationId - Correlation ID for tracking packet across logs
   * @returns ILP response packet (Fulfill or Reject) from next-hop peer
   * @throws BTPConnectionError if BTP connection fails
   * @throws BTPAuthenticationError if BTP authentication fails
   * @remarks
   * Forwards packet to next-hop peer using BTPClientManager.
   * Maps BTP errors to ILP error codes:
   * - BTPConnectionError → T01 (Ledger Unreachable)
   * - BTPAuthenticationError → T01 (Ledger Unreachable)
   * - BTP timeout → T00 (Transfer Timed Out)
   */
  private async forwardToNextHop(
    packet: ILPPreparePacket,
    nextHop: string,
    correlationId: string,
    protocolData?: Array<{ protocolName: string; contentType: number; data: Buffer }>
  ): Promise<ILPFulfillPacket | ILPRejectPacket> {
    this.logger.info(
      {
        correlationId,
        event: 'btp_forward',
        destination: packet.destination,
        amount: packet.amount.toString(),
        peerId: nextHop,
      },
      'Forwarding packet to peer via BTP'
    );

    try {
      // Select transport connection upfront: prefer outbound client, fall back to server.
      // We check connectivity BEFORE sending to avoid catch-and-retry, which risks
      // duplicate packets if the first send times out but the packet was already received.
      let response: ILPFulfillPacket | ILPRejectPacket;

      const hasOutbound = this.btpClientManager.isConnected(nextHop);
      const hasInbound = this.btpServer?.hasPeer(nextHop) ?? false;

      if (hasOutbound) {
        response = await this.btpClientManager.sendToPeer(nextHop, packet, protocolData);
        this.logger.debug(
          { correlationId, peerId: nextHop },
          'Forwarded via outbound peer connection'
        );
      } else if (hasInbound) {
        this.logger.debug(
          { correlationId, peerId: nextHop },
          'No outbound connection, using incoming peer connection'
        );
        response = await this.btpServer!.sendPacketToPeer(nextHop, packet, protocolData);
        this.logger.debug(
          { correlationId, peerId: nextHop },
          'Forwarded via incoming peer connection'
        );
      } else {
        throw new BTPConnectionError(`No active BTP connection to peer ${nextHop}`);
      }

      this.logger.info(
        {
          correlationId,
          event: 'btp_forward_success',
          peerId: nextHop,
          responseType: response.type,
        },
        'Received response from peer via BTP'
      );

      return response;
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);

      // Map BTP errors to ILP error codes
      if (error instanceof BTPConnectionError) {
        this.logger.error(
          {
            correlationId,
            event: 'btp_connection_error',
            peerId: nextHop,
            error: errorMessage,
          },
          'BTP connection failed'
        );
        return this.generateReject(
          ILPErrorCode.T01_PEER_UNREACHABLE,
          `BTP connection to ${nextHop} failed: ${errorMessage}`,
          this.nodeId
        );
      }

      if (error instanceof BTPAuthenticationError) {
        this.logger.error(
          {
            correlationId,
            event: 'btp_auth_error',
            peerId: nextHop,
            error: errorMessage,
          },
          'BTP authentication failed'
        );
        return this.generateReject(
          ILPErrorCode.T01_PEER_UNREACHABLE,
          `BTP authentication to ${nextHop} failed: ${errorMessage}`,
          this.nodeId
        );
      }

      // Check if timeout error
      if (errorMessage.includes('timeout')) {
        this.logger.error(
          {
            correlationId,
            event: 'btp_timeout',
            peerId: nextHop,
            error: errorMessage,
          },
          'BTP packet send timeout'
        );
        return this.generateReject(
          ILPErrorCode.R00_TRANSFER_TIMED_OUT,
          `BTP timeout to ${nextHop}: ${errorMessage}`,
          this.nodeId
        );
      }

      // Unknown error - log and rethrow
      this.logger.error(
        {
          correlationId,
          event: 'btp_forward_error',
          peerId: nextHop,
          error: errorMessage,
        },
        'Unexpected error forwarding packet via BTP'
      );
      throw error;
    }
  }

  /**
   * Handle ILP Prepare packet - main packet processing method
   * @param packet - ILP Prepare packet to process
   * @returns Promise resolving to ILP Fulfill or Reject packet
   * @remarks
   * Complete packet handling flow per RFC-0027:
   * 1. Validate packet structure and expiration
   * 2. Look up next-hop peer using routing table
   * 3. Decrement packet expiry by safety margin
   * 4. Forward to next-hop peer (stub for Epic 1)
   * 5. Return fulfill/reject based on processing result
   *
   * Generates correlation ID for packet tracking across logs.
   */
  async handlePreparePacket(
    packet: ILPPreparePacket,
    fromPeerId?: string
  ): Promise<ILPFulfillPacket | ILPRejectPacket> {
    const correlationId = generateCorrelationId();
    const sourcePeerId = fromPeerId || 'unknown';

    this.logger.info(
      {
        correlationId,
        packetType: 'PREPARE',
        destination: packet.destination,
        amount: packet.amount.toString(),
        fromPeerId: sourcePeerId,
        timestamp: Date.now(),
      },
      'Packet received'
    );

    // Emit PACKET_RECEIVED telemetry
    if (this.telemetryEmitter) {
      this.telemetryEmitter.emitPacketReceived(packet, sourcePeerId);
    } else if (this.eventStore) {
      // Standalone mode: emit event directly to EventStore
      // Convert peer ID to full ILP address for UI display
      const fromAddress = sourcePeerId.startsWith('g.') ? sourcePeerId : `g.${sourcePeerId}`;

      const event = {
        type: 'PACKET_RECEIVED' as const,
        nodeId: this.nodeId,
        packetId: packet.executionCondition.toString('hex'),
        destination: packet.destination,
        amount: packet.amount.toString(),
        from: fromAddress,
        timestamp: Date.now(),
      };
      this.eventStore.storeEvent(event).catch((err) => {
        this.logger.warn(
          { error: err.message, packetId: event.packetId },
          'Failed to store PACKET_RECEIVED event'
        );
      });
      // Broadcast event to WebSocket clients for real-time UI updates
      this.eventBroadcaster?.broadcast(event);
    }

    // Validate packet
    const validation = this.validatePacket(packet);
    if (!validation.isValid) {
      this.logger.error(
        {
          correlationId,
          packetType: 'REJECT',
          destination: packet.destination,
          errorCode: validation.errorCode,
          reason: validation.errorMessage,
          timestamp: Date.now(),
        },
        'Packet rejected'
      );
      return this.generateReject(validation.errorCode!, validation.errorMessage!, this.nodeId);
    }

    // Look up next-hop peer
    const nextHop = this.routingTable.getNextHop(packet.destination);
    if (nextHop === null) {
      this.logger.info(
        {
          correlationId,
          destination: packet.destination,
          selectedPeer: null,
          reason: 'no route found',
        },
        'Routing decision'
      );

      // Emit ROUTE_LOOKUP telemetry for failed lookup
      if (this.telemetryEmitter) {
        this.telemetryEmitter.emitRouteLookup(packet.destination, null, 'no route found');
      }

      this.logger.error(
        {
          correlationId,
          packetType: 'REJECT',
          destination: packet.destination,
          errorCode: ILPErrorCode.F02_UNREACHABLE,
          reason: 'no route found',
          timestamp: Date.now(),
        },
        'Packet rejected'
      );
      return this.generateReject(
        ILPErrorCode.F02_UNREACHABLE,
        `No route to destination: ${packet.destination}`,
        this.nodeId
      );
    }

    this.logger.info(
      {
        correlationId,
        destination: packet.destination,
        selectedPeer: nextHop,
        reason: 'longest-prefix match',
      },
      'Routing decision'
    );

    // Emit ROUTE_LOOKUP telemetry for successful lookup
    if (this.telemetryEmitter) {
      this.telemetryEmitter.emitRouteLookup(packet.destination, nextHop, 'longest prefix match');
    }

    // Check for local delivery (destination handled by this connector)
    if (nextHop === this.nodeId || nextHop === 'local') {
      this.logger.info(
        {
          correlationId,
          destination: packet.destination,
          reason: 'local delivery',
        },
        'Delivering packet locally'
      );

      // Check for function handler first (in-process delivery, no HTTP)
      if (this.localDeliveryHandler) {
        const request: LocalDeliveryRequest = {
          destination: packet.destination,
          amount: packet.amount.toString(),
          executionCondition: packet.executionCondition.toString('base64'),
          expiresAt: packet.expiresAt.toISOString(),
          data: packet.data.toString('base64'),
          sourcePeer: sourcePeerId,
        };
        try {
          const result = await this.localDeliveryHandler(request, sourcePeerId);
          return this.convertLocalDeliveryResponse(result);
        } catch (error) {
          return this.generateReject(
            ILPErrorCode.T00_INTERNAL_ERROR,
            `Local delivery handler error: ${error instanceof Error ? error.message : String(error)}`,
            this.nodeId
          );
        }
      }

      // If local delivery client is enabled, forward to agent runtime via HTTP
      if (this.isLocalDeliveryEnabled() && this.localDeliveryClient) {
        this.logger.debug(
          { correlationId, destination: packet.destination },
          'Forwarding to agent runtime for local delivery'
        );

        const response = await this.localDeliveryClient.deliver(packet, sourcePeerId);

        this.logger.info(
          {
            correlationId,
            event: 'packet_response',
            packetType: response.type,
            destination: packet.destination,
            timestamp: Date.now(),
          },
          response.type === PacketType.FULFILL
            ? 'Packet fulfilled by agent runtime'
            : 'Packet rejected by agent runtime'
        );

        return response;
      }

      // Fallback: auto-fulfill local packets (educational/testing purposes)
      // In a real deployment, use localDelivery config to forward to agent runtime
      const fulfillPacket: ILPFulfillPacket = {
        type: PacketType.FULFILL,
        fulfillment: packet.executionCondition, // Educational implementation - using condition as fulfillment
        data: Buffer.from('Local delivery - educational implementation'),
      };

      this.logger.info(
        {
          correlationId,
          event: 'packet_response',
          packetType: PacketType.FULFILL,
          destination: packet.destination,
          timestamp: Date.now(),
        },
        'Returning local fulfillment (auto-fulfill stub)'
      );

      return fulfillPacket;
    }

    // Decrement expiry
    const newExpiry = this.decrementExpiry(packet.expiresAt, EXPIRY_SAFETY_MARGIN_MS);
    if (newExpiry === null) {
      this.logger.error(
        {
          correlationId,
          packetType: 'REJECT',
          destination: packet.destination,
          errorCode: ILPErrorCode.R00_TRANSFER_TIMED_OUT,
          expiresAt: packet.expiresAt.toISOString(),
          reason: 'Insufficient time remaining for forwarding',
          timestamp: Date.now(),
        },
        'Packet rejected'
      );
      return this.generateReject(
        ILPErrorCode.R00_TRANSFER_TIMED_OUT,
        'Insufficient time remaining for forwarding',
        this.nodeId
      );
    }

    // SETTLEMENT RECORDING (Story 6.4) - Calculate connector fee and record transfers
    let forwardingPacket: ILPPreparePacket;

    // Skip settlement and fees for local delivery
    const isLocalDelivery = nextHop === 'local';

    if (this.isSettlementEnabled() && !isLocalDelivery) {
      // Calculate connector fee
      const connectorFee = this.calculateConnectorFee(
        packet.amount,
        this.settlementConfig?.connectorFeePercentage ?? 0.1
      );
      const forwardedAmount = packet.amount - connectorFee;

      this.logger.debug(
        {
          correlationId,
          originalAmount: packet.amount.toString(),
          connectorFee: connectorFee.toString(),
          forwardedAmount: forwardedAmount.toString(),
          feePercentage: this.settlementConfig?.connectorFeePercentage,
        },
        'Calculated connector fee'
      );

      // CREDIT LIMIT CHECK (Story 6.5) - Check if incoming transfer would exceed credit limit
      // Check BEFORE settlement recording (fail-safe design)
      const fromPeerId = 'unknown'; // TODO: Pass actual incoming peer ID in future enhancement
      const tokenId = 'ILP'; // Default token for MVP (multi-token in Epic 7)

      const creditLimitViolation = await this.accountManager!.checkCreditLimit(
        fromPeerId,
        tokenId,
        packet.amount
      );

      if (creditLimitViolation) {
        // Credit limit would be exceeded - reject packet with T04_INSUFFICIENT_LIQUIDITY
        this.logger.warn(
          {
            correlationId,
            packetType: 'REJECT',
            destination: packet.destination,
            errorCode: ILPErrorCode.T04_INSUFFICIENT_LIQUIDITY,
            fromPeerId: creditLimitViolation.peerId,
            currentBalance: creditLimitViolation.currentBalance.toString(),
            requestedAmount: creditLimitViolation.requestedAmount.toString(),
            creditLimit: creditLimitViolation.creditLimit.toString(),
            wouldExceedBy: creditLimitViolation.wouldExceedBy.toString(),
            reason: 'Credit limit exceeded',
            timestamp: Date.now(),
          },
          'Packet rejected: credit limit exceeded'
        );

        return this.generateReject(
          ILPErrorCode.T04_INSUFFICIENT_LIQUIDITY,
          `Credit limit exceeded: peer ${fromPeerId} would owe ${creditLimitViolation.wouldExceedBy} units over limit of ${creditLimitViolation.creditLimit}`,
          this.nodeId
        );
      }

      // Record settlement transfers atomically BEFORE forwarding packet
      // Skip settlement for unknown/unregistered peers or zero-amount packets
      if (sourcePeerId !== 'unknown' && packet.amount > 0n && forwardedAmount > 0n) {
        try {
          await this.recordPacketTransfers(
            packet,
            sourcePeerId,
            nextHop,
            forwardedAmount,
            connectorFee,
            correlationId
          );
        } catch (error) {
          // Settlement recording failed - reject packet with T00_INTERNAL_ERROR
          this.logger.error(
            {
              correlationId,
              packetType: 'REJECT',
              destination: packet.destination,
              errorCode: ILPErrorCode.T00_INTERNAL_ERROR,
              error: error instanceof Error ? error.message : String(error),
              reason: 'Settlement recording failed',
              timestamp: Date.now(),
            },
            'Packet rejected due to settlement failure'
          );
          return this.generateReject(
            ILPErrorCode.T00_INTERNAL_ERROR,
            'Settlement recording failed',
            this.nodeId
          );
        }
      } else {
        this.logger.debug(
          {
            correlationId,
            sourcePeerId,
            reason: 'Skipping settlement for unknown peer',
          },
          'Settlement skipped for unregistered peer'
        );
      }

      // Create forwarding packet with decremented expiry AND reduced amount (after fee)
      forwardingPacket = {
        ...packet,
        expiresAt: newExpiry,
        amount: forwardedAmount,
      };
    } else {
      // Settlement disabled - forward original amount
      forwardingPacket = {
        ...packet,
        expiresAt: newExpiry,
      };
    }

    // Fire-and-forget BLS notification for transit packets (per-hop notification)
    const perHopEnabled = this.localDeliveryClient?.isPerHopNotificationEnabled() ?? false;
    if (perHopEnabled) {
      let dispatched = false;
      if (this.localDeliveryHandler) {
        // In-process handler path (takes priority over HTTP)
        dispatched = true;
        const transitRequest: LocalDeliveryRequest = {
          destination: packet.destination,
          amount: packet.amount.toString(),
          executionCondition: packet.executionCondition.toString('base64'),
          expiresAt: packet.expiresAt.toISOString(),
          data: packet.data.toString('base64'),
          sourcePeer: sourcePeerId,
          isTransit: true,
        };
        this.localDeliveryHandler(transitRequest, sourcePeerId).catch((err: unknown) => {
          this.logger.debug(
            {
              error: err instanceof Error ? err.message : String(err),
              destination: packet.destination,
            },
            'Per-hop notification failed (fire-and-forget, in-process)'
          );
        });
      } else if (this.isLocalDeliveryEnabled() && this.localDeliveryClient) {
        // HTTP client path
        dispatched = true;
        this.localDeliveryClient
          .deliver(packet, sourcePeerId, { isTransit: true })
          .catch((err: unknown) => {
            this.logger.debug(
              {
                error: err instanceof Error ? err.message : String(err),
                destination: packet.destination,
              },
              'Per-hop notification failed (fire-and-forget, HTTP)'
            );
          });
      }

      // Emit PER_HOP_NOTIFICATION telemetry only when a notification was actually dispatched
      if (dispatched) {
        try {
          const perHopEvent = {
            type: 'PER_HOP_NOTIFICATION' as const,
            nodeId: this.nodeId,
            destination: packet.destination,
            amount: packet.amount.toString(),
            nextHop,
            sourcePeer: sourcePeerId,
            correlationId,
            timestamp: Date.now(),
          };
          if (this.telemetryEmitter) {
            this.telemetryEmitter.emit(perHopEvent);
          } else if (this.eventStore) {
            this.eventStore.storeEvent(perHopEvent).catch((err) => {
              this.logger.warn(
                { error: err.message, correlationId },
                'Failed to store PER_HOP_NOTIFICATION event'
              );
            });
            this.eventBroadcaster?.broadcast(perHopEvent);
          }
        } catch {
          // Telemetry emission is non-blocking — swallow errors
        }
      }
    }

    // Generate per-packet claim before forwarding (non-blocking on failure)
    let claimProtocolData:
      | Array<{ protocolName: string; contentType: number; data: Buffer }>
      | undefined;
    if (this.perPacketClaimService && !isLocalDelivery && forwardingPacket.amount > 0n) {
      try {
        const result = await this.perPacketClaimService.generateClaimForPacket(
          nextHop,
          'ILP',
          forwardingPacket.amount
        );
        if (result) {
          claimProtocolData = [result.protocolData];
        }
      } catch (error) {
        // Claim failure MUST NOT block packet forwarding
        this.logger.warn(
          {
            correlationId,
            peerId: nextHop,
            error: error instanceof Error ? error.message : String(error),
          },
          'Claim generation failed, forwarding without claim'
        );
      }
    }

    // Forward to next hop via BTP and return response
    const response = await this.forwardToNextHop(
      forwardingPacket,
      nextHop,
      correlationId,
      claimProtocolData
    );

    // Emit PACKET_FORWARDED telemetry after successful forward
    if (this.telemetryEmitter) {
      const packetId = packet.executionCondition.toString('hex');
      this.telemetryEmitter.emitPacketSent(packetId, nextHop);
    } else if (this.eventStore) {
      // Standalone mode: emit PACKET_FORWARDED event directly to EventStore
      // Convert peer IDs to full ILP addresses for UI display
      const fromAddress = sourcePeerId.startsWith('g.') ? sourcePeerId : `g.${sourcePeerId}`;
      const toAddress = nextHop.startsWith('g.') ? nextHop : `g.${nextHop}`;

      const event = {
        type: 'PACKET_FORWARDED' as const,
        nodeId: this.nodeId,
        packetId: packet.executionCondition.toString('hex'),
        destination: packet.destination,
        amount: forwardingPacket.amount.toString(),
        from: fromAddress,
        to: toAddress,
        timestamp: Date.now(),
      };
      this.eventStore.storeEvent(event).catch((err) => {
        this.logger.warn(
          { error: err.message, packetId: event.packetId },
          'Failed to store PACKET_FORWARDED event'
        );
      });
      // Broadcast event to WebSocket clients for real-time UI updates
      this.eventBroadcaster?.broadcast(event);
    }

    this.logger.info(
      {
        correlationId,
        event: 'packet_response',
        packetType: response.type,
        destination: packet.destination,
        code: response.type === PacketType.REJECT ? response.code : undefined,
        timestamp: Date.now(),
      },
      'Returning packet response'
    );

    // Emit telemetry for packet response (FULFILL or REJECT)
    // Use telemetryEmitter.emit() for connected mode, eventStore for standalone mode
    const packetId = packet.executionCondition.toString('hex');
    const fromAddress = sourcePeerId.startsWith('g.') ? sourcePeerId : `g.${sourcePeerId}`;

    if (response.type === PacketType.FULFILL) {
      const event = {
        type: 'PACKET_FULFILLED' as const,
        nodeId: this.nodeId,
        packetId,
        destination: packet.destination,
        amount: packet.amount.toString(),
        from: fromAddress,
        fulfillment: response.fulfillment.toString('hex'),
        timestamp: Date.now(),
      };

      // Emit via telemetryEmitter if available (connected mode)
      if (this.telemetryEmitter) {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        this.telemetryEmitter.emit(event as any);
      } else if (this.eventStore) {
        // Fallback to direct eventStore in standalone mode
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        this.eventStore.storeEvent(event as any).catch((err) => {
          this.logger.warn(
            { error: err.message, packetId },
            'Failed to store PACKET_FULFILLED event'
          );
        });
        this.eventBroadcaster?.broadcast(event);
      }
    } else if (response.type === PacketType.REJECT) {
      const event = {
        type: 'PACKET_REJECTED' as const,
        nodeId: this.nodeId,
        packetId,
        destination: packet.destination,
        amount: packet.amount.toString(),
        from: fromAddress,
        code: response.code,
        message: response.message,
        timestamp: Date.now(),
      };

      // Emit via telemetryEmitter if available (connected mode)
      if (this.telemetryEmitter) {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        this.telemetryEmitter.emit(event as any);
      } else if (this.eventStore) {
        // Fallback to direct eventStore in standalone mode
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        this.eventStore.storeEvent(event as any).catch((err) => {
          this.logger.warn(
            { error: err.message, packetId },
            'Failed to store PACKET_REJECTED event'
          );
        });
        this.eventBroadcaster?.broadcast(event);
      }
    }

    return response;
  }
}
