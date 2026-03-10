/**
 * Claim Redemption Service
 *
 * Automatically polls for verified unredeemed payment channel claims from the received_claims
 * database table and submits them to the blockchain for on-chain redemption.
 *
 * This service implements the final step in the BTP Claim Exchange Protocol (Epic 17):
 * - Polls SQLite database every 60 seconds (configurable) for verified claims
 * - Estimates redemption costs (gas fees)
 * - Only redeems claims where profit exceeds a configurable threshold
 * - Submits EVM balance proofs via PaymentChannelSDK.claimFromChannel()
 * - Updates database with redemption timestamp and transaction identifier
 * - Emits CLAIM_REDEEMED telemetry events
 * - Implements exponential backoff retry (3 attempts: 1s, 2s, 4s delays)
 *
 * References:
 * - Epic 17: BTP Off-Chain Claim Exchange Protocol
 * - RFC-0023: Bilateral Transfer Protocol
 * - Story 17.3: Claim Receiver (provides received_claims table)
 * - Story 17.4: UnifiedSettlementExecutor Integration (settlement flow context)
 *
 * @example
 * ```typescript
 * const service = new ClaimRedemptionService(
 *   db,
 *   evmChannelSDK,
 *   evmProvider,
 *   {
 *     minProfitThreshold: 1000n,
 *     pollingInterval: 60000,
 *     maxConcurrentRedemptions: 5,
 *     evmTokenAddress: '0x1234...'
 *   },
 *   logger,
 *   telemetryEmitter,
 *   'node-alice'
 * );
 *
 * service.start();
 * // Service polls every 60 seconds for claims to redeem
 *
 * // Stop when shutting down
 * service.stop();
 * ```
 */

import type { Database } from 'better-sqlite3';
import type { Logger } from 'pino';
import type { ethers } from 'ethers';
import type { PaymentChannelSDK } from './payment-channel-sdk';
import type { TelemetryEmitter } from '../telemetry/telemetry-emitter';
import type { BTPClaimMessage, EVMClaimMessage, BlockchainType } from '../btp/btp-claim-types';
import type { BalanceProof } from '@crosstown/shared';

/**
 * Configuration for ClaimRedemptionService
 */
export interface RedemptionConfig {
  /**
   * Minimum profit threshold in wei.
   * Claims are only redeemed if (claimAmount - gasCost) >= minProfitThreshold.
   *
   * Recommended value: 1000 wei (adjust based on token decimals)
   */
  minProfitThreshold: bigint;

  /**
   * Polling interval in milliseconds.
   * Service polls database for unredeemed claims at this interval.
   * Default: 60000 (60 seconds)
   */
  pollingInterval: number;

  /**
   * Maximum number of concurrent redemptions to process in a single poll cycle.
   * Limits parallel blockchain submissions to avoid overwhelming RPC nodes.
   * Default: 5
   */
  maxConcurrentRedemptions: number;

  /**
   * ERC20 token address for EVM channel closeChannel operations.
   * Required for EVM claim redemption (not included in BTPClaimMessage).
   * This should be the AGENT token address deployed in Epic 8.
   */
  evmTokenAddress: string;
}

/**
 * Result of a claim redemption attempt
 */
export interface ClaimRedemptionResult {
  /** Whether redemption succeeded */
  success: boolean;

  /** Message ID of the claim */
  messageId: string;

  /**
   * Transaction hash or identifier.
   * Note: Since SDK methods return void, this is set to messageId for tracking.
   * For actual on-chain tx hashes, query blockchain explorers using claim signatures/amounts
   * and the redeemed_at timestamp.
   */
  txHash?: string;

  /** Error message if redemption failed */
  error?: string;

  /** Estimated gas cost for the redemption */
  gasCost?: bigint;
}

/**
 * Database row for received claims
 */
interface ReceivedClaimRow {
  message_id: string;
  peer_id: string;
  blockchain: string;
  channel_id: string;
  claim_data: string; // JSON-encoded BTPClaimMessage
}

/**
 * ClaimRedemptionService - Automatic on-chain claim redemption
 *
 * Polls the received_claims database table for verified unredeemed claims,
 * checks profitability, and submits profitable claims to the appropriate
 * blockchain for on-chain settlement.
 */
export class ClaimRedemptionService {
  private _pollingTimer?: NodeJS.Timeout;
  private _isRunning = false;

  constructor(
    private readonly db: Database,
    private readonly evmChannelSDK: PaymentChannelSDK,
    private readonly evmProvider: ethers.Provider,
    private readonly config: RedemptionConfig,
    private readonly logger: Logger,
    private readonly telemetryEmitter?: TelemetryEmitter,
    private readonly nodeId?: string
  ) {}

  /**
   * Start polling for claims to redeem
   */
  start(): void {
    if (this._isRunning) {
      this.logger.warn('Claim redemption service already running');
      return;
    }

    this._isRunning = true;
    this.logger.info(
      {
        pollingInterval: this.config.pollingInterval,
        minProfitThreshold: this.config.minProfitThreshold.toString(),
        maxConcurrentRedemptions: this.config.maxConcurrentRedemptions,
      },
      'Starting claim redemption service'
    );

    this._pollingTimer = setInterval(() => {
      this.processRedemptions().catch((error) => {
        this.logger.error({ error }, 'Error in processRedemptions polling cycle');
      });
    }, this.config.pollingInterval);

    // Run immediately on start
    this.processRedemptions().catch((error) => {
      this.logger.error({ error }, 'Error in initial processRedemptions call');
    });
  }

  /**
   * Stop polling for claims
   */
  stop(): void {
    if (this._pollingTimer) {
      clearInterval(this._pollingTimer);
      this._pollingTimer = undefined;
    }
    this._isRunning = false;
    this.logger.info('Claim redemption service stopped');
  }

  /**
   * Check if service is currently running
   */
  get isRunning(): boolean {
    return this._isRunning;
  }

  /**
   * Process all unredeemed verified claims (main polling cycle handler)
   */
  private async processRedemptions(): Promise<void> {
    try {
      // Query unredeemed verified claims
      const stmt = this.db.prepare(`
        SELECT message_id, peer_id, blockchain, channel_id, claim_data
        FROM received_claims
        WHERE verified = 1
          AND redeemed_at IS NULL
        ORDER BY received_at ASC
        LIMIT ?
      `);

      const rows = stmt.all(this.config.maxConcurrentRedemptions) as ReceivedClaimRow[];

      if (rows.length === 0) {
        return; // No claims to process
      }

      this.logger.info({ count: rows.length }, 'Processing claim redemptions');

      // Process claims in parallel
      const results = await Promise.allSettled(
        rows.map(async (row) => {
          try {
            const claim: BTPClaimMessage = JSON.parse(row.claim_data);

            // Estimate redemption cost
            const gasCost = await this.estimateRedemptionCost(claim.blockchain as BlockchainType);

            // Get claim amount
            const claimAmount = this._getClaimAmount(claim);

            // Check profitability
            if (!this.isProfitable(claimAmount, gasCost)) {
              this.logger.debug(
                {
                  messageId: claim.messageId,
                  blockchain: claim.blockchain,
                  claimAmount: claimAmount.toString(),
                  gasCost: gasCost.toString(),
                  minProfitThreshold: this.config.minProfitThreshold.toString(),
                },
                'Skipping unprofitable claim'
              );
              return;
            }

            // Redeem the claim
            const result = await this._redeemClaim(claim);

            // Emit telemetry
            this._emitRedemptionTelemetry(claim, result, gasCost);
          } catch (error) {
            this.logger.error(
              { error, messageId: row.message_id },
              'Error processing claim redemption'
            );
          }
        })
      );

      // Log any rejected promises (shouldn't happen due to try-catch, but defensive)
      results.forEach((result, index) => {
        if (result.status === 'rejected') {
          this.logger.error(
            { error: result.reason, messageId: rows[index]?.message_id },
            'Claim redemption promise rejected'
          );
        }
      });
    } catch (error) {
      this.logger.error({ error }, 'Error in processRedemptions');
    }
  }

  /**
   * Check if claim redemption is profitable
   *
   * @param claimAmount - Amount to be redeemed
   * @param gasCost - Estimated gas/transaction cost
   * @returns true if profit >= minProfitThreshold
   */
  private isProfitable(claimAmount: bigint, gasCost: bigint): boolean {
    const profit = claimAmount - gasCost;
    return profit >= this.config.minProfitThreshold;
  }

  /**
   * Estimate redemption cost for EVM claim (gas fees)
   *
   * @param blockchain - Blockchain type (must be 'evm')
   * @returns Estimated cost in wei
   */
  private async estimateRedemptionCost(blockchain: BlockchainType): Promise<bigint> {
    try {
      if (blockchain !== 'evm') {
        this.logger.warn({ blockchain }, 'Non-EVM blockchain type for gas estimation');
        return 0n;
      }

      // Use ethers.js provider for gas estimation
      const feeData = await this.evmProvider.getFeeData();
      const gasPrice = feeData.gasPrice ?? 0n;
      const gasLimit = 150000n; // Fixed estimate for closeChannel (~100k-150k gas)
      return gasPrice * gasLimit;
    } catch (error) {
      this.logger.error({ error, blockchain }, 'Error estimating redemption cost');
      // Return 0 (pessimistic - assume free if estimation fails)
      return 0n;
    }
  }

  /**
   * Redeem EVM balance proof on-chain
   *
   * @param claim - EVM claim message
   * @returns Redemption result
   */
  private async redeemEVMClaim(claim: EVMClaimMessage): Promise<ClaimRedemptionResult> {
    const childLogger = this.logger.child({ messageId: claim.messageId, blockchain: 'evm' });

    childLogger.info(
      {
        channelId: claim.channelId,
        nonce: claim.nonce,
        amount: claim.transferredAmount,
      },
      'Redeeming EVM balance proof on-chain'
    );

    // Create BalanceProof object with bigint conversion
    const balanceProof: BalanceProof = {
      channelId: claim.channelId,
      nonce: claim.nonce,
      transferredAmount: BigInt(claim.transferredAmount),
      lockedAmount: BigInt(claim.lockedAmount),
      locksRoot: claim.locksRoot,
    };

    // Get tokenAddress from config
    const tokenAddress = this.config.evmTokenAddress;

    // Retry logic (3 attempts, exponential backoff 1s, 2s, 4s)
    for (let attempt = 1; attempt <= 3; attempt++) {
      try {
        // claimFromChannel: submit counterparty's signed proof to collect owed tokens
        await this.evmChannelSDK.claimFromChannel(
          claim.channelId,
          tokenAddress,
          balanceProof,
          claim.signature
        );

        // Update database with messageId as redemption identifier
        this._updateRedemptionStatus(claim.messageId, claim.messageId);

        childLogger.info('EVM balance proof redeemed successfully');

        return {
          success: true,
          messageId: claim.messageId,
          txHash: claim.messageId,
        };
      } catch (error) {
        if (attempt === 3) {
          childLogger.error({ error }, 'EVM claim redemption failed after 3 attempts');
          return {
            success: false,
            messageId: claim.messageId,
            error: error instanceof Error ? error.message : String(error),
          };
        }

        // Wait before retry (1s, 2s, 4s)
        const delayMs = Math.pow(2, attempt - 1) * 1000;
        childLogger.warn({ attempt, delayMs, error }, 'EVM claim redemption failed, retrying');
        await this._delay(delayMs);
      }
    }

    // Should never reach here due to return in loop, but TypeScript requires it
    return {
      success: false,
      messageId: claim.messageId,
      error: 'Unknown error',
    };
  }

  /**
   * Main redemption method - redeems EVM claims
   *
   * @param claim - Claim message to redeem
   * @returns Redemption result
   */
  private async _redeemClaim(claim: BTPClaimMessage): Promise<ClaimRedemptionResult> {
    if (claim.blockchain !== 'evm') {
      this.logger.error(
        { blockchain: claim.blockchain },
        'Unsupported blockchain type for claim. Only EVM is supported.'
      );
      return {
        success: false,
        messageId: claim.messageId,
        error: `Unsupported blockchain type: ${claim.blockchain}. Only EVM is supported.`,
      };
    }

    return await this.redeemEVMClaim(claim as EVMClaimMessage);
  }

  /**
   * Get claim amount from EVM claim message
   *
   * @param claim - Claim message
   * @returns Claim amount as bigint
   */
  private _getClaimAmount(claim: BTPClaimMessage): bigint {
    return BigInt((claim as EVMClaimMessage).transferredAmount);
  }

  /**
   * Get channel ID from EVM claim message
   *
   * @param claim - Claim message
   * @returns Channel ID
   */
  private _getChannelId(claim: BTPClaimMessage): string {
    return claim.channelId;
  }

  /**
   * Update database with redemption status
   *
   * @param messageId - Message ID of the claim
   * @param txHash - Transaction hash (set to messageId for tracking)
   */
  private _updateRedemptionStatus(messageId: string, txHash: string): void {
    const stmt = this.db.prepare(`
      UPDATE received_claims
      SET redeemed_at = ?, redemption_tx_hash = ?
      WHERE message_id = ?
    `);

    stmt.run(Date.now(), txHash, messageId);
  }

  /**
   * Emit telemetry event for claim redemption
   *
   * @param claim - Claim message
   * @param result - Redemption result
   * @param gasCost - Gas cost for redemption (optional)
   */
  private _emitRedemptionTelemetry(
    claim: BTPClaimMessage,
    result: ClaimRedemptionResult,
    gasCost?: bigint
  ): void {
    if (!this.telemetryEmitter) {
      return;
    }

    try {
      this.telemetryEmitter.emit({
        type: 'CLAIM_REDEEMED',
        nodeId: this.nodeId ?? 'unknown',
        peerId: claim.senderId,
        blockchain: claim.blockchain as 'evm',
        messageId: claim.messageId,
        channelId: this._getChannelId(claim),
        amount: this._getClaimAmount(claim).toString(),
        txHash: result.txHash ?? '',
        gasCost: gasCost?.toString() ?? '0',
        success: result.success,
        error: result.error,
        timestamp: new Date().toISOString(),
      });
    } catch (error) {
      // Non-blocking telemetry emission
      this.logger.error({ error }, 'Error emitting CLAIM_REDEEMED telemetry');
    }
  }

  /**
   * Delay helper for retry backoff
   *
   * @param ms - Milliseconds to delay
   * @returns Promise that resolves after delay
   */
  private _delay(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }
}
