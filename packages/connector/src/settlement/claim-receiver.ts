/**
 * Claim Receiver Module
 *
 * Receives and verifies payment channel claims from peers via BTP protocol.
 * Implements verification for EVM blockchains with signature
 * validation and monotonicity checks.
 *
 * @module claim-receiver
 * @see RFC-0023 - Bilateral Transfer Protocol (BTP)
 * @see Epic 17 - BTP Off-Chain Claim Exchange Protocol
 */

import { EventEmitter } from 'events';
import type { Database } from 'better-sqlite3';
import type { Logger } from 'pino';
import type { BTPServer } from '../btp/btp-server';
import type { BTPProtocolData, BTPMessage } from '../btp/btp-types';
import { isBTPData } from '../btp/btp-types';
import type { PaymentChannelSDK } from './payment-channel-sdk';
import type { ChannelManager } from './channel-manager';
import {
  type BTPClaimMessage,
  type EVMClaimMessage,
  type BlockchainType,
  isEVMClaim,
  validateClaimMessage,
} from '../btp/btp-claim-types';

/**
 * Event emitted after a claim is successfully validated and persisted.
 * Used by SettlementMonitor to trigger event-driven settlement checks.
 */
export interface ClaimReceivedEvent {
  /** Peer ID of the claim sender */
  peerId: string;
  /** Payment channel ID */
  channelId: string;
  /** Cumulative transferred amount from the claim (bigint) */
  cumulativeAmount: bigint;
}

/**
 * Error message constants for claim verification
 * Exported for consistent usage between implementation and tests.
 */
export const ERRORS = {
  MISSING_SELF_DESCRIBING_FIELDS:
    'Missing self-describing fields for unknown channel (chainId, tokenNetworkAddress, tokenAddress required)',
  CHANNEL_NOT_FOUND: 'Channel does not exist on-chain',
  CHANNEL_NOT_OPENED: 'Channel not in opened state',
  SIGNER_NOT_PARTICIPANT: 'Signer is not a channel participant',
  ON_CHAIN_VERIFICATION_FAILED: 'On-chain channel verification failed',
} as const;

/**
 * Result of claim verification process
 */
export interface ClaimVerificationResult {
  /** Whether the claim passed verification */
  valid: boolean;
  /** Unique message ID of the claim */
  messageId: string;
  /** Error message if verification failed */
  error?: string;
}

/**
 * ClaimReceiver - Receives and verifies payment channel claims from peers
 *
 * Responsibilities:
 * - Register BTP protocol data handler for "payment-channel-claim" protocol
 * - Parse and validate incoming claim messages
 * - Verify EVM payment channel claims with signature validation
 * - Enforce monotonicity checks (nonce/amount must increase)
 * - Persist verified claims to database for later redemption
 *
 * @example
 * ```typescript
 * const claimReceiver = new ClaimReceiver(
 *   db,
 *   evmChannelSDK,
 *   logger
 * );
 *
 * claimReceiver.registerWithBTPServer(btpServer);
 * ```
 */
export class ClaimReceiver extends EventEmitter {
  constructor(
    private readonly db: Database,
    private readonly evmChannelSDK: PaymentChannelSDK,
    private readonly logger: Logger,
    private readonly channelManager?: ChannelManager,
    private readonly peerIdToAddressMap?: Map<string, string>
  ) {
    super();
  }

  /**
   * Register claim message handler with BTP server
   *
   * Sets up callback to receive BTP messages with protocol name "payment-channel-claim"
   * and routes them to handleClaimMessage for processing.
   *
   * @param btpServer - BTP server instance to register with
   */
  registerWithBTPServer(btpServer: BTPServer): void {
    // Register message callback with BTP server
    btpServer.onMessage(async (peerId: string, message: BTPMessage) => {
      // Only process data messages (not error messages)
      if (!isBTPData(message)) {
        return;
      }

      // TypeScript now knows message.data is BTPData, not BTPErrorData
      // Iterate through protocol data array
      for (const protocolData of message.data.protocolData) {
        // Filter for claim protocol
        if (protocolData.protocolName === 'payment-channel-claim') {
          await this.handleClaimMessage(peerId, protocolData);
        }
      }
    });

    this.logger.info('ClaimReceiver registered with BTP server');
  }

  /**
   * Handle incoming claim message from BTP peer
   *
   * @param peerId - Peer ID of sender
   * @param protocolData - BTP protocol data containing claim message
   * @private
   */
  private async handleClaimMessage(peerId: string, protocolData: BTPProtocolData): Promise<void> {
    const childLogger = this.logger.child({ peerId, protocol: 'claim-receiver' });

    try {
      // Parse JSON claim message
      const claimMessage = JSON.parse(protocolData.data.toString('utf8')) as BTPClaimMessage;

      // Validate claim message structure
      validateClaimMessage(claimMessage);

      const messageId = claimMessage.messageId;
      const blockchain = claimMessage.blockchain;

      childLogger.info({ messageId, blockchain }, 'Received claim message');

      // Verify EVM claim
      if (!isEVMClaim(claimMessage)) {
        throw new Error(`Unsupported blockchain type: ${blockchain}. Only EVM is supported.`);
      }

      const verificationResult = await this.verifyEVMClaim(claimMessage, peerId);

      // Persist verified claim
      if (verificationResult.valid) {
        this._persistReceivedClaim(peerId, claimMessage, true);
        childLogger.info({ messageId }, 'Claim verified and stored');

        // Emit event for event-driven settlement monitoring
        if (isEVMClaim(claimMessage)) {
          const event: ClaimReceivedEvent = {
            peerId,
            channelId: claimMessage.channelId,
            cumulativeAmount: BigInt(claimMessage.transferredAmount),
          };
          this.emit('CLAIM_RECEIVED', event);
          childLogger.debug(
            { channelId: event.channelId, cumulativeAmount: event.cumulativeAmount.toString() },
            'CLAIM_RECEIVED event emitted'
          );
        }
      } else {
        this._persistReceivedClaim(peerId, claimMessage, false);
        childLogger.warn(
          { messageId, error: verificationResult.error },
          'Claim verification failed'
        );
      }
    } catch (error) {
      childLogger.error({ error }, 'Failed to parse claim message');
    }
  }

  /**
   * Verify EVM claim signature and nonce monotonicity
   *
   * @param claim - EVM claim message
   * @param peerId - Peer ID of sender
   * @returns Verification result
   * @private
   */
  private async verifyEVMClaim(
    claim: EVMClaimMessage,
    peerId: string
  ): Promise<ClaimVerificationResult> {
    try {
      // Create balance proof object with bigint conversion
      const balanceProof = {
        channelId: claim.channelId,
        nonce: claim.nonce,
        transferredAmount: BigInt(claim.transferredAmount),
        lockedAmount: BigInt(claim.lockedAmount),
        locksRoot: claim.locksRoot,
      };

      this.logger.debug({ channelId: claim.channelId }, 'Checking channel existence in metadata');

      // Check if channel is known (pre-registered or previously verified)
      const knownChannel = this.channelManager?.getChannelById(claim.channelId);

      if (!knownChannel && this.channelManager) {
        // Unknown channel -- attempt dynamic on-chain verification
        this.logger.info(
          { channelId: claim.channelId },
          'Unknown channel detected, starting on-chain verification'
        );

        // Require all self-describing fields
        if (claim.chainId === undefined || !claim.tokenNetworkAddress || !claim.tokenAddress) {
          this.logger.warn(
            { channelId: claim.channelId, signerAddress: claim.signerAddress },
            ERRORS.MISSING_SELF_DESCRIBING_FIELDS
          );
          return {
            valid: false,
            messageId: claim.messageId,
            error: ERRORS.MISSING_SELF_DESCRIBING_FIELDS,
          };
        }

        // Query on-chain state
        let channelState: {
          exists: boolean;
          state: number;
          participant1: string;
          participant2: string;
          settlementTimeout: number;
        };
        try {
          channelState = await this.evmChannelSDK.getChannelStateByNetwork(
            claim.channelId,
            claim.tokenNetworkAddress
          );
        } catch (error) {
          this.logger.warn(
            { channelId: claim.channelId, signerAddress: claim.signerAddress, error },
            ERRORS.ON_CHAIN_VERIFICATION_FAILED
          );
          return {
            valid: false,
            messageId: claim.messageId,
            error: ERRORS.ON_CHAIN_VERIFICATION_FAILED,
          };
        }

        // Verify channel exists (state !== 0)
        if (!channelState.exists) {
          this.logger.warn(
            { channelId: claim.channelId, signerAddress: claim.signerAddress },
            ERRORS.CHANNEL_NOT_FOUND
          );
          return {
            valid: false,
            messageId: claim.messageId,
            error: ERRORS.CHANNEL_NOT_FOUND,
          };
        }

        // Verify channel is opened (state === 1)
        if (channelState.state !== 1) {
          this.logger.warn(
            { channelId: claim.channelId, signerAddress: claim.signerAddress },
            ERRORS.CHANNEL_NOT_OPENED
          );
          return {
            valid: false,
            messageId: claim.messageId,
            error: ERRORS.CHANNEL_NOT_OPENED,
          };
        }

        // Verify signerAddress matches participant1 or participant2
        const signerLower = claim.signerAddress.toLowerCase();
        if (
          signerLower !== channelState.participant1.toLowerCase() &&
          signerLower !== channelState.participant2.toLowerCase()
        ) {
          this.logger.warn(
            { channelId: claim.channelId, signerAddress: claim.signerAddress },
            ERRORS.SIGNER_NOT_PARTICIPANT
          );
          return {
            valid: false,
            messageId: claim.messageId,
            error: ERRORS.SIGNER_NOT_PARTICIPANT,
          };
        }

        this.logger.info(
          {
            channelId: claim.channelId,
            participant1: channelState.participant1,
            participant2: channelState.participant2,
            state: channelState.state,
          },
          'On-chain channel verified successfully'
        );

        // Verify EIP-712 signature using explicit domain from claim fields
        const sigValid = await this.evmChannelSDK.verifyBalanceProofWithDomain(
          balanceProof,
          claim.signature,
          claim.signerAddress,
          claim.chainId,
          claim.tokenNetworkAddress
        );

        if (!sigValid) {
          return {
            valid: false,
            messageId: claim.messageId,
            error: 'Invalid EIP-712 signature',
          };
        }

        // Register channel in ChannelManager
        this.channelManager.registerExternalChannel({
          channelId: claim.channelId,
          peerId,
          tokenAddress: claim.tokenAddress,
          tokenNetworkAddress: claim.tokenNetworkAddress,
          chainId: claim.chainId,
          status: 'open',
        });

        this.logger.info({ channelId: claim.channelId, peerId }, 'External channel registered');

        // Register peer's EVM address for SettlementExecutor lookup
        if (this.peerIdToAddressMap && !this.peerIdToAddressMap.has(peerId)) {
          this.peerIdToAddressMap.set(peerId, claim.signerAddress);
          this.logger.info(
            { peerId, signerAddress: claim.signerAddress },
            'Peer EVM address registered from self-describing claim'
          );
        }
      } else {
        // Known channel (pre-registered or previously verified) -- use existing verification
        const isValid = await this.evmChannelSDK.verifyBalanceProof(
          balanceProof,
          claim.signature,
          claim.signerAddress
        );

        if (!isValid) {
          return {
            valid: false,
            messageId: claim.messageId,
            error: 'Invalid EIP-712 signature',
          };
        }
      }

      // Check nonce monotonicity - nonce must strictly increase
      const latestClaim = await this.getLatestVerifiedClaim(peerId, 'evm', claim.channelId);

      if (latestClaim && isEVMClaim(latestClaim)) {
        if (claim.nonce <= latestClaim.nonce) {
          return {
            valid: false,
            messageId: claim.messageId,
            error: 'Nonce not monotonically increasing',
          };
        }
      }

      return { valid: true, messageId: claim.messageId };
    } catch (error) {
      return {
        valid: false,
        messageId: claim.messageId,
        error: error instanceof Error ? error.message : 'Unknown error',
      };
    }
  }

  /**
   * Persist received claim to database
   *
   * @param peerId - Peer ID of sender
   * @param claim - Claim message
   * @param verified - Whether claim passed verification
   * @private
   */
  private _persistReceivedClaim(peerId: string, claim: BTPClaimMessage, verified: boolean): void {
    try {
      // EVM claims use channelId
      const channelId = claim.channelId;

      // Insert into database
      const stmt = this.db.prepare(`
        INSERT INTO received_claims (
          message_id,
          peer_id,
          blockchain,
          channel_id,
          claim_data,
          verified,
          received_at,
          redeemed_at,
          redemption_tx_hash
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
      `);

      stmt.run(
        claim.messageId,
        peerId,
        claim.blockchain,
        channelId,
        JSON.stringify(claim),
        verified ? 1 : 0,
        Date.now(),
        null,
        null
      );
    } catch (error) {
      // Non-blocking: Log error but don't throw
      if (error instanceof Error && error.message.includes('UNIQUE constraint failed')) {
        this.logger.warn(
          { messageId: claim.messageId },
          'Duplicate claim message ignored (idempotency)'
        );
      } else {
        this.logger.error({ error }, 'Failed to persist claim to database');
      }
    }
  }

  /**
   * Get latest verified claim for a specific peer and channel
   *
   * Used for monotonicity checks and future redemption.
   *
   * @param peerId - Peer ID
   * @param blockchain - Blockchain type
   * @param channelId - Channel or owner identifier
   * @returns Latest verified claim or null if none found
   */
  async getLatestVerifiedClaim(
    peerId: string,
    blockchain: BlockchainType,
    channelId: string
  ): Promise<BTPClaimMessage | null> {
    try {
      const stmt = this.db.prepare(`
        SELECT claim_data
        FROM received_claims
        WHERE peer_id = ?
          AND blockchain = ?
          AND channel_id = ?
          AND verified = 1
          AND redeemed_at IS NULL
        ORDER BY received_at DESC
        LIMIT 1
      `);

      const row = stmt.get(peerId, blockchain, channelId) as { claim_data: string } | undefined;

      if (!row) {
        return null;
      }

      return JSON.parse(row.claim_data) as BTPClaimMessage;
    } catch (error) {
      this.logger.error({ error }, 'Failed to query latest verified claim');
      return null;
    }
  }
}
