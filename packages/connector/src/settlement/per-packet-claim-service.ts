/**
 * Per-Packet Claim Service
 *
 * Generates signed payment channel claims for each outgoing ILP PREPARE packet.
 * Claims travel with packets via BTP protocolData, ensuring the receiving peer
 * always holds an up-to-date signed balance proof.
 *
 * On-chain settlement remains threshold-based (time or amount) via SettlementMonitor,
 * but claims flow per-packet so the counterparty can always settle with the latest proof.
 *
 * Key behaviors:
 * - Cumulative transferred amounts are tracked per channel
 * - Nonces are monotonically increasing per channel
 * - Claim failure never blocks packet forwarding
 * - Claims only generated for PREPARE direction (outgoing)
 * - Returns null gracefully if no channel exists for a peer
 *
 * @module settlement/per-packet-claim-service
 */

import type { Database } from 'better-sqlite3';
import type { Logger } from 'pino';
import type { PaymentChannelSDK } from './payment-channel-sdk';
import type { ChannelManager } from './channel-manager';
import type { TelemetryEmitter } from '../telemetry/telemetry-emitter';
import { BTP_CLAIM_PROTOCOL, EVMClaimMessage } from '../btp/btp-claim-types';

/**
 * BTP protocol data entry for claim attachment
 */
export interface BTPProtocolData {
  protocolName: string;
  contentType: number;
  data: Buffer;
}

/**
 * Cached context for a payment channel, avoiding repeated lookups
 */
interface ChannelClaimContext {
  channelId: string;
  tokenNetworkAddress: string;
  chainId: number;
  tokenAddress: string;
  signerAddress: string;
}

/**
 * Result of per-packet claim generation
 */
export interface PerPacketClaimResult {
  protocolData: BTPProtocolData;
  claimMessage: EVMClaimMessage;
}

/**
 * PerPacketClaimService generates signed claims for each outgoing ILP packet.
 *
 * Claims are attached to BTP messages via protocolData and accumulate
 * cumulative transferred amounts. The latest claim is always available
 * for on-chain settlement via getLatestClaim().
 */
export class PerPacketClaimService {
  private readonly logger: Logger;
  private readonly cumulativeTransferred: Map<string, bigint> = new Map();
  private readonly currentNonce: Map<string, number> = new Map();
  private readonly channelClaimCache: Map<string, ChannelClaimContext> = new Map();
  private readonly latestClaim: Map<string, EVMClaimMessage> = new Map();

  constructor(
    private readonly paymentChannelSDK: PaymentChannelSDK,
    private readonly channelManager: ChannelManager,
    private readonly db: Database,
    logger: Logger,
    private readonly nodeId: string,
    private readonly telemetryEmitter?: TelemetryEmitter
  ) {
    this.logger = logger.child({ component: 'per-packet-claim-service' });
    this.recoverFromDb();
  }

  /**
   * Generate a signed claim for an outgoing packet.
   *
   * Returns null if no channel exists for the peer (graceful degradation —
   * packets still flow without claims).
   *
   * @param toPeerId - Destination peer ID
   * @param tokenId - Token identifier (e.g., 'M2M')
   * @param amount - Packet amount to add to cumulative total
   * @returns PerPacketClaimResult with protocolData and claim, or null
   */
  async generateClaimForPacket(
    toPeerId: string,
    tokenId: string,
    amount: bigint
  ): Promise<PerPacketClaimResult | null> {
    // Look up channel context (cached or fresh)
    const cacheKey = `${toPeerId}:${tokenId}`;
    let ctx = this.channelClaimCache.get(cacheKey);

    if (!ctx) {
      const builtCtx = await this.buildChannelContext(toPeerId, tokenId);
      if (!builtCtx) {
        return null; // No channel for this peer — graceful degradation
      }
      ctx = builtCtx;
      this.channelClaimCache.set(cacheKey, ctx);
    }

    const { channelId } = ctx;

    // Increment cumulative transferred and nonce (synchronous — safe under Node.js single thread)
    const prevCumulative = this.cumulativeTransferred.get(channelId) ?? 0n;
    const newCumulative = prevCumulative + amount;
    this.cumulativeTransferred.set(channelId, newCumulative);

    const prevNonce = this.currentNonce.get(channelId) ?? 0;
    const newNonce = prevNonce + 1;
    this.currentNonce.set(channelId, newNonce);

    // Sign balance proof (async — but nonce/cumulative are already committed)
    const locksRoot = '0x0000000000000000000000000000000000000000000000000000000000000000';
    const signature = await this.paymentChannelSDK.signBalanceProof(
      channelId,
      newNonce,
      newCumulative,
      0n,
      locksRoot
    );

    // Construct self-describing claim message
    const messageId = `evm-${channelId.substring(0, 8)}-${newNonce}-${Date.now()}`;
    const timestamp = new Date().toISOString();

    const claimMessage: EVMClaimMessage = {
      version: '1.0',
      blockchain: 'evm',
      messageId,
      timestamp,
      senderId: this.nodeId,
      channelId,
      nonce: newNonce,
      transferredAmount: newCumulative.toString(),
      lockedAmount: '0',
      locksRoot,
      signature,
      signerAddress: ctx.signerAddress,
      chainId: ctx.chainId,
      tokenNetworkAddress: ctx.tokenNetworkAddress,
      tokenAddress: ctx.tokenAddress,
    };

    // Store as latest claim for SettlementExecutor
    this.latestClaim.set(channelId, claimMessage);

    // Persist to DB (non-blocking)
    this.persistClaim(toPeerId, claimMessage);

    // Emit telemetry (non-blocking)
    this.emitClaimTelemetry(toPeerId, claimMessage);

    // Serialize to BTP protocolData
    const data = Buffer.from(JSON.stringify(claimMessage), 'utf8');
    const protocolData: BTPProtocolData = {
      protocolName: BTP_CLAIM_PROTOCOL.NAME,
      contentType: BTP_CLAIM_PROTOCOL.CONTENT_TYPE,
      data,
    };

    this.logger.debug(
      {
        channelId,
        nonce: newNonce,
        cumulative: newCumulative.toString(),
        peerId: toPeerId,
      },
      'Generated per-packet claim'
    );

    return { protocolData, claimMessage };
  }

  /**
   * Get the latest signed claim for a channel.
   * Used by SettlementExecutor for on-chain settlement submission.
   *
   * @param channelId - Payment channel ID
   * @returns Latest EVMClaimMessage or null if no claims generated
   */
  getLatestClaim(channelId: string): EVMClaimMessage | null {
    return this.latestClaim.get(channelId) ?? null;
  }

  /**
   * Reset tracking state for a channel after successful on-chain settlement.
   * Called by SettlementExecutor after cooperative settle completes.
   *
   * @param channelId - Payment channel ID to reset
   */
  resetChannel(channelId: string): void {
    this.cumulativeTransferred.delete(channelId);
    this.currentNonce.delete(channelId);
    this.latestClaim.delete(channelId);

    // Invalidate any cached contexts referencing this channel
    for (const [key, ctx] of this.channelClaimCache.entries()) {
      if (ctx.channelId === channelId) {
        this.channelClaimCache.delete(key);
      }
    }

    this.logger.info({ channelId }, 'Channel claim tracking reset after settlement');
  }

  /**
   * Build channel claim context by looking up channel metadata and SDK state.
   * Returns null if no channel exists for the peer.
   */
  private async buildChannelContext(
    peerId: string,
    tokenId: string
  ): Promise<ChannelClaimContext | null> {
    const metadata = this.channelManager.getChannelForPeer(peerId, tokenId);
    if (!metadata) {
      return null;
    }

    try {
      const [chainId, tokenNetworkAddress, signerAddress] = await Promise.all([
        this.paymentChannelSDK.getChainId(),
        this.paymentChannelSDK.getTokenNetworkAddress(metadata.tokenAddress),
        this.paymentChannelSDK.getSignerAddress(),
      ]);

      return {
        channelId: metadata.channelId,
        tokenNetworkAddress,
        chainId,
        tokenAddress: metadata.tokenAddress,
        signerAddress,
      };
    } catch (error) {
      this.logger.error(
        { peerId, tokenId, error: error instanceof Error ? error.message : String(error) },
        'Failed to build channel claim context'
      );
      return null;
    }
  }

  /**
   * Recover nonce and cumulative state from the sent_claims DB table on startup.
   * Ensures claim continuity across connector restarts.
   */
  private recoverFromDb(): void {
    try {
      // Query the latest claim per channel from sent_claims
      const rows = this.db
        .prepare(
          `
          SELECT claim_data FROM sent_claims
          WHERE blockchain = 'evm'
          ORDER BY sent_at DESC
        `
        )
        .all() as Array<{ claim_data: string }>;

      const recoveredChannels = new Set<string>();

      for (const row of rows) {
        try {
          const claim = JSON.parse(row.claim_data) as EVMClaimMessage;
          // Only recover the latest per channel (first seen since ordered DESC)
          if (!recoveredChannels.has(claim.channelId)) {
            recoveredChannels.add(claim.channelId);
            this.currentNonce.set(claim.channelId, claim.nonce);
            this.cumulativeTransferred.set(claim.channelId, BigInt(claim.transferredAmount));
            this.latestClaim.set(claim.channelId, claim);
          }
        } catch {
          // Skip malformed claim data
        }
      }

      if (recoveredChannels.size > 0) {
        this.logger.info(
          { channelCount: recoveredChannels.size },
          'Recovered per-packet claim state from database'
        );
      }
    } catch (error) {
      // DB recovery failure is not fatal — we start fresh
      this.logger.warn(
        { error: error instanceof Error ? error.message : String(error) },
        'Failed to recover claim state from database, starting fresh'
      );
    }
  }

  /**
   * Persist a sent claim to the database (non-blocking).
   */
  private persistClaim(peerId: string, claim: EVMClaimMessage): void {
    try {
      this.db
        .prepare(
          `
          INSERT INTO sent_claims (
            message_id, peer_id, blockchain, claim_data, sent_at
          ) VALUES (?, ?, ?, ?, ?)
        `
        )
        .run(claim.messageId, peerId, claim.blockchain, JSON.stringify(claim), Date.now());
    } catch (error) {
      if (error instanceof Error && error.message.includes('UNIQUE constraint failed')) {
        this.logger.warn({ messageId: claim.messageId }, 'Duplicate claim message ID, skipping');
      } else {
        this.logger.error(
          { error, messageId: claim.messageId },
          'Failed to persist claim to database'
        );
      }
    }
  }

  /**
   * Emit telemetry for a generated claim (non-blocking).
   */
  private emitClaimTelemetry(peerId: string, claim: EVMClaimMessage): void {
    if (!this.telemetryEmitter) {
      return;
    }

    try {
      this.telemetryEmitter.emit({
        type: 'CLAIM_SENT',
        nodeId: this.nodeId,
        peerId,
        blockchain: claim.blockchain,
        messageId: claim.messageId,
        amount: claim.transferredAmount,
        success: true,
        timestamp: new Date().toISOString(),
      });
    } catch {
      // Telemetry emission is non-blocking
    }
  }
}
