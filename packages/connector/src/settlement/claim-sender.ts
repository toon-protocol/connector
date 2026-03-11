/**
 * Claim Sender - Send payment channel claims to peers via BTP
 *
 * @deprecated This module is superseded by PerPacketClaimService (Epic 31).
 * Claims are now generated per-packet and attached to BTP protocolData during
 * packet forwarding, rather than sent as separate BTP messages triggered by
 * settlement thresholds. This file is retained for reference only.
 *
 * This module implements the claim transport layer for Epic 17 (BTP Off-Chain Claim Exchange).
 * It sends signed payment channel claims over BTP WebSocket connections to enable off-chain
 * settlement without on-chain transactions for every payment.
 *
 * Key Features:
 * - Sends EVM claims via BTP protocolData
 * - Retry logic with exponential backoff (3 attempts: 1s, 2s, 4s delays)
 * - Claim persistence in SQLite for dispute resolution
 * - Idempotent message IDs for duplicate detection
 *
 * References:
 * - RFC-0023: Bilateral Transfer Protocol
 * - Epic 17: BTP Off-Chain Claim Exchange Protocol
 * - Story 17.1: BTP Claim Message Protocol Definition
 *
 * @module claim-sender
 */

import type { Database } from 'better-sqlite3';
import { Logger } from 'pino';
import { BTPClient } from '../btp/btp-client';
import {
  BTP_CLAIM_PROTOCOL,
  BTPClaimMessage,
  EVMClaimMessage,
  BlockchainType,
} from '../btp/btp-claim-types';
/**
 * Result of a claim send operation
 */
export interface ClaimSendResult {
  /** Whether the claim send was successful */
  success: boolean;
  /** Unique message ID for this claim */
  messageId: string;
  /** ISO 8601 timestamp of the send attempt */
  timestamp: string;
  /** Error message if send failed */
  error?: string;
}

/**
 * ClaimSender handles sending payment channel claims to peers via BTP.
 *
 * It integrates with BTPClient for WebSocket transmission, implements retry logic,
 * persists claims for dispute resolution.
 *
 * The caller (UnifiedSettlementExecutor) is responsible for:
 * 1. Obtaining BTPClient from BTPConnectionManager.getClientForPeer(peerId)
 * 2. Passing BTPClient to sendEVMClaim()
 *
 * This separation ensures ClaimSender remains focused on transport, while
 * connection management stays with BTPConnectionManager.
 */
export class ClaimSender {
  constructor(
    private readonly db: Database,
    private readonly logger: Logger,
    private readonly nodeId?: string
  ) {}

  /**
   * Send an EVM payment channel claim to a peer
   *
   * @param peerId - Peer identifier
   * @param btpClient - BTPClient instance for this peer connection
   * @param channelId - bytes32 hex channel ID
   * @param nonce - Balance proof nonce
   * @param transferredAmount - Cumulative transferred amount
   * @param lockedAmount - Locked amount in channel
   * @param locksRoot - Merkle root of locks
   * @param signature - EIP-712 signature
   * @param signerAddress - Ethereum address
   * @param chainId - (Optional) EVM chain ID for self-describing claims (Epic 31)
   * @param tokenNetworkAddress - (Optional) TokenNetwork contract address for self-describing claims (Epic 31)
   * @param tokenAddress - (Optional) ERC20 token contract address for self-describing claims (Epic 31)
   * @returns Promise resolving to ClaimSendResult
   *
   * @example
   * ```typescript
   * const result = await claimSender.sendEVMClaim(
   *   'peer-bob',
   *   btpClient,
   *   '0xabcd...',
   *   42,
   *   '5000000000000000000',
   *   '0',
   *   '0x0000...',
   *   '0x1234...',
   *   '0x5678...',
   *   8453, // chainId (optional)
   *   '0x1234...', // tokenNetworkAddress (optional)
   *   '0xabcd...' // tokenAddress (optional)
   * );
   * ```
   */
  async sendEVMClaim(
    peerId: string,
    btpClient: BTPClient,
    channelId: string,
    nonce: number,
    transferredAmount: string,
    lockedAmount: string,
    locksRoot: string,
    signature: string,
    signerAddress: string,
    chainId?: number,
    tokenNetworkAddress?: string,
    tokenAddress?: string
  ): Promise<ClaimSendResult> {
    const messageId = this._generateMessageId('evm', channelId, nonce);
    const timestamp = new Date().toISOString();

    const claimMessage: EVMClaimMessage = {
      version: '1.0',
      blockchain: 'evm',
      messageId,
      timestamp,
      senderId: this.nodeId ?? 'unknown',
      channelId,
      nonce,
      transferredAmount,
      lockedAmount,
      locksRoot,
      signature,
      signerAddress,
      ...(chainId !== undefined && { chainId }),
      ...(tokenNetworkAddress !== undefined && { tokenNetworkAddress }),
      ...(tokenAddress !== undefined && { tokenAddress }),
    };

    return this.sendClaim(peerId, btpClient, claimMessage);
  }

  /**
   * Core claim sending logic (private method)
   *
   * Handles serialization, retry logic, and persistence for all claim types.
   *
   * @param peerId - Peer identifier
   * @param btpClient - BTPClient instance
   * @param claimMessage - Blockchain-specific claim message
   * @returns Promise resolving to ClaimSendResult
   */
  private async sendClaim(
    peerId: string,
    btpClient: BTPClient,
    claimMessage: BTPClaimMessage
  ): Promise<ClaimSendResult> {
    const childLogger = this.logger.child({ peerId, messageId: claimMessage.messageId });

    childLogger.info({ blockchain: claimMessage.blockchain }, 'Sending claim to peer');

    try {
      // Serialize claim to JSON buffer
      const serializedClaim = this._serializeClaimMessage(claimMessage);

      // Send with retry (3 attempts, exponential backoff)
      await this._sendWithRetry(
        btpClient,
        BTP_CLAIM_PROTOCOL.NAME,
        BTP_CLAIM_PROTOCOL.CONTENT_TYPE,
        serializedClaim
      );

      // Persist claim to database
      this._persistSentClaim(peerId, claimMessage.messageId, claimMessage);

      childLogger.info('Claim sent successfully');

      return {
        success: true,
        messageId: claimMessage.messageId,
        timestamp: claimMessage.timestamp,
      };
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);

      childLogger.error({ error: errorMessage }, 'Failed to send claim');

      return {
        success: false,
        messageId: claimMessage.messageId,
        timestamp: claimMessage.timestamp,
        error: errorMessage,
      };
    }
  }

  /**
   * Generate a unique message ID for claim deduplication
   *
   * Format: `<blockchain>-<channelId-prefix>-<nonce>-<timestamp>`
   *
   * @param blockchain - Blockchain type ('evm')
   * @param channelId - Channel identifier (first 8 chars used as prefix)
   * @param nonce - Nonce number
   * @returns Unique message ID string
   *
   * @example
   * // EVM: evm-0xabcdef-42-1706889600000
   */
  private _generateMessageId(blockchain: BlockchainType, channelId: string, nonce: number): string {
    const prefix = channelId.substring(0, 8);
    const nonceStr = nonce.toString();
    const timestamp = Date.now();
    return `${blockchain}-${prefix}-${nonceStr}-${timestamp}`;
  }

  /**
   * Serialize claim message to JSON buffer for BTP transmission
   *
   * @param claimMessage - Claim message to serialize
   * @returns Buffer containing JSON-encoded claim
   */
  private _serializeClaimMessage(claimMessage: BTPClaimMessage): Buffer {
    const json = JSON.stringify(claimMessage);
    return Buffer.from(json, 'utf8');
  }

  /**
   * Send claim with retry logic and exponential backoff
   *
   * Retry strategy:
   * - Attempt 1: Immediate send
   * - Attempt 2: Wait 1s, retry
   * - Attempt 3: Wait 2s, retry
   * - Attempt 4: Wait 4s, retry (if maxAttempts=4)
   *
   * @param btpClient - BTPClient instance
   * @param protocolName - Protocol name (payment-channel-claim)
   * @param contentType - Content type (1 for JSON)
   * @param data - Serialized claim data
   * @param maxAttempts - Maximum retry attempts (default: 3)
   * @throws Error if all attempts fail
   */
  private async _sendWithRetry(
    btpClient: BTPClient,
    protocolName: string,
    contentType: number,
    data: Buffer,
    maxAttempts: number = 3
  ): Promise<void> {
    for (let attempt = 1; attempt <= maxAttempts; attempt++) {
      try {
        await btpClient.sendProtocolData(protocolName, contentType, data);
        return; // Success
      } catch (error) {
        if (attempt === maxAttempts) {
          // Final attempt failed
          throw error;
        }

        // Exponential backoff: 1s, 2s, 4s
        const delay = Math.pow(2, attempt - 1) * 1000;
        this.logger.warn({ attempt, maxAttempts, delay }, 'Retrying claim send');

        await new Promise((resolve) => setTimeout(resolve, delay));
      }
    }
  }

  /**
   * Persist sent claim to database for dispute resolution
   *
   * Stores claim in `sent_claims` table with:
   * - message_id (PRIMARY KEY)
   * - peer_id
   * - blockchain ('evm')
   * - claim_data (JSON-encoded claim)
   * - sent_at (Unix timestamp ms)
   *
   * Handles duplicate message IDs gracefully (UNIQUE constraint violation).
   *
   * @param peerId - Peer identifier
   * @param messageId - Unique message ID
   * @param claim - Claim message to persist
   */
  private _persistSentClaim(peerId: string, messageId: string, claim: BTPClaimMessage): void {
    try {
      this.db
        .prepare(
          `
        INSERT INTO sent_claims (
          message_id, peer_id, blockchain, claim_data, sent_at
        ) VALUES (?, ?, ?, ?, ?)
      `
        )
        .run(messageId, peerId, claim.blockchain, JSON.stringify(claim), Date.now());
    } catch (error) {
      // Handle duplicate message IDs (idempotency)
      if (error instanceof Error && error.message.includes('UNIQUE constraint failed')) {
        this.logger.warn({ messageId, peerId }, 'Duplicate claim message ID, skipping insert');
      } else {
        // Log other database errors but don't block send
        this.logger.error({ error, messageId, peerId }, 'Failed to persist claim to database');
      }
    }
  }
}
