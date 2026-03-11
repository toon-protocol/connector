import { EventEmitter } from 'events';
import type { Logger } from 'pino';
import { PaymentChannelSDK } from './payment-channel-sdk';
import { SettlementExecutor } from './settlement-executor';
import type { AdminChannelStatus } from './types';

/**
 * ChannelManager configuration
 */
export interface ChannelManagerConfig {
  nodeId: string; // Our connector node ID
  defaultSettlementTimeout: number; // Default challenge period (e.g., 86400 = 24h)
  initialDepositMultiplier: number; // Channel initial deposit = threshold × multiplier (default: 10)
  idleChannelThreshold: number; // Close channel after this many seconds idle (default: 86400 = 24h)
  minDepositThreshold: number; // Add funds when deposit < threshold × multiplier × minDepositThreshold (default: 0.5)
  idleCheckInterval: number; // How often to check for idle channels (default: 3600 = 1h)
  tokenAddressMap: Map<string, string>; // tokenId → ERC20 contract address mapping
  peerIdToAddressMap: Map<string, string>; // peerId → Ethereum address mapping
  registryAddress: string; // TokenNetworkRegistry contract address
  rpcUrl: string; // Base L2 RPC URL
  privateKey: string; // Connector wallet private key
}

/**
 * Channel metadata for lifecycle tracking
 */
export interface ChannelMetadata {
  channelId: string; // bytes32 channel identifier
  peerId: string; // Peer connector ID (e.g., "connector-b")
  tokenId: string; // Token identifier (e.g., "M2M", "USDC")
  tokenAddress: string; // ERC20 token contract address
  chain: string; // Chain identifier (e.g., "evm:base:8453")
  createdAt: Date; // When channel was opened
  lastActivityAt: Date; // Last settlement or balance update
  status: AdminChannelStatus;
}

/**
 * Optional overrides for channel open operations via Admin API
 */
export interface ChannelOpenOptions {
  initialDeposit?: bigint; // Override default deposit
  settlementTimeout?: number; // Override default timeout
  chain?: string; // Chain identifier for metadata
  peerAddress?: string; // Peer's blockchain address for channel opening
}

/**
 * ChannelManager orchestrates full payment channel lifecycles:
 * - Opens channels on-demand when settlements needed
 * - Tracks channel activity and detects idle channels
 * - Automatically closes idle channels to reclaim deposits
 * - Handles cooperative and unilateral closure flows
 */
export class ChannelManager extends EventEmitter {
  private readonly config: ChannelManagerConfig;
  private readonly paymentChannelSDK: PaymentChannelSDK;
  private readonly settlementExecutor: SettlementExecutor;
  private readonly logger: Logger;
  private readonly channelMetadata: Map<string, ChannelMetadata>; // channelId → metadata
  private readonly peerChannelIndex: Map<string, Map<string, string>>; // peerId → (tokenId → channelId)
  private idleCheckTimer?: NodeJS.Timeout;

  constructor(
    config: ChannelManagerConfig,
    paymentChannelSDK: PaymentChannelSDK,
    settlementExecutor: SettlementExecutor,
    logger: Logger
  ) {
    super();
    this.config = config;
    this.paymentChannelSDK = paymentChannelSDK;
    this.settlementExecutor = settlementExecutor;
    this.channelMetadata = new Map<string, ChannelMetadata>();
    this.peerChannelIndex = new Map<string, Map<string, string>>();
    this.idleCheckTimer = undefined;

    // Create child logger
    this.logger = logger.child({ component: 'channel-manager' });
    this.logger.info({ nodeId: config.nodeId }, 'Channel manager initialized');

    // Listen for settlement activity to update channel activity timestamps
    this.settlementExecutor.on('CHANNEL_ACTIVITY', ({ channelId }: { channelId: string }) => {
      // Note: markChannelActivity is now async but we don't await here
      // to avoid blocking the event loop. Errors are handled internally.
      void this.markChannelActivity(channelId);
    });
  }

  /**
   * Start idle channel monitoring
   */
  start(): void {
    this.idleCheckTimer = setInterval(
      () => this.checkIdleChannels(),
      this.config.idleCheckInterval * 1000
    );
    this.logger.info(
      { idleCheckInterval: this.config.idleCheckInterval },
      'Channel manager started'
    );
  }

  /**
   * Stop monitoring and cleanup
   */
  stop(): void {
    if (this.idleCheckTimer) {
      clearInterval(this.idleCheckTimer);
      this.idleCheckTimer = undefined;
    }
    this.logger.info('Channel manager stopped');
  }

  /**
   * Ensure channel exists for peer and token, creating if needed
   */
  async ensureChannelExists(
    peerId: string,
    tokenId: string,
    options?: ChannelOpenOptions
  ): Promise<string> {
    // Check if channel already exists
    const channelId = this.peerChannelIndex.get(peerId)?.get(tokenId);

    if (channelId) {
      // Verify channel is still active
      const metadata = this.channelMetadata.get(channelId);
      if (metadata && metadata.status !== 'closed') {
        this.logger.info({ peerId, tokenId, channelId }, 'Ensured channel exists (existing)');
        return channelId;
      }
    }

    // No active channel found, open new one
    const newChannelId = await this.openChannelForPeer(peerId, tokenId, options);
    this.logger.info({ peerId, tokenId, channelId: newChannelId }, 'Ensured channel exists (new)');
    return newChannelId;
  }

  /**
   * Get channel metadata for peer and token
   */
  getChannelForPeer(peerId: string, tokenId: string): ChannelMetadata | null {
    const channelId = this.peerChannelIndex.get(peerId)?.get(tokenId);
    if (!channelId) {
      return null;
    }
    return this.channelMetadata.get(channelId) ?? null;
  }

  /**
   * Get channel metadata by channel ID
   */
  getChannelById(channelId: string): ChannelMetadata | null {
    return this.channelMetadata.get(channelId) ?? null;
  }

  /**
   * Register a channel discovered from an incoming self-describing claim.
   * Populates both channelMetadata and peerChannelIndex without opening on-chain.
   * Idempotent: if channelId already exists, returns existing metadata.
   */
  registerExternalChannel(params: {
    channelId: string;
    peerId: string;
    tokenAddress: string;
    tokenNetworkAddress: string;
    chainId: number;
    status: AdminChannelStatus;
  }): ChannelMetadata {
    // Idempotent: return existing if already registered
    const existing = this.channelMetadata.get(params.channelId);
    if (existing) {
      this.logger.debug(
        { channelId: params.channelId },
        'External channel already registered, returning existing'
      );
      return existing;
    }

    // Resolve tokenId by reverse-lookup from tokenAddressMap
    let tokenId: string = params.tokenAddress;
    for (const [id, address] of this.config.tokenAddressMap.entries()) {
      if (address.toLowerCase() === params.tokenAddress.toLowerCase()) {
        tokenId = id;
        break;
      }
    }

    const metadata: ChannelMetadata = {
      channelId: params.channelId,
      peerId: params.peerId,
      tokenId,
      tokenAddress: params.tokenAddress,
      chain: `evm:${params.chainId}`,
      createdAt: new Date(),
      lastActivityAt: new Date(),
      status: 'open',
    };

    this.channelMetadata.set(params.channelId, metadata);

    if (!this.peerChannelIndex.has(params.peerId)) {
      this.peerChannelIndex.set(params.peerId, new Map<string, string>());
    }
    this.peerChannelIndex.get(params.peerId)!.set(tokenId, params.channelId);

    this.logger.info(
      {
        channelId: params.channelId,
        peerId: params.peerId,
        chainId: params.chainId,
        tokenNetworkAddress: params.tokenNetworkAddress,
      },
      'External channel registered'
    );

    return metadata;
  }

  /**
   * Get all channels
   */
  getAllChannels(): ChannelMetadata[] {
    return Array.from(this.channelMetadata.values());
  }

  /**
   * Mark channel as active (settlement or balance update occurred)
   */
  async markChannelActivity(channelId: string): Promise<void> {
    const metadata = this.channelMetadata.get(channelId);
    if (!metadata) {
      this.logger.warn({ channelId }, 'Cannot mark activity: channel not found');
      return;
    }
    metadata.lastActivityAt = new Date();
    this.logger.debug({ channelId }, 'Channel activity marked');
  }

  /**
   * Open new channel for peer
   * @private
   */
  private async openChannelForPeer(
    peerId: string,
    tokenId: string,
    options?: ChannelOpenOptions
  ): Promise<string> {
    // Get token and peer addresses
    const tokenAddress = this.config.tokenAddressMap.get(tokenId);
    if (!tokenAddress) {
      throw new Error(`Token address not found for tokenId: ${tokenId}`);
    }

    const peerAddress = options?.peerAddress || this.config.peerIdToAddressMap.get(peerId);
    if (!peerAddress) {
      throw new Error(`Peer address not found for peerId: ${peerId}`);
    }

    this.logger.info(
      { peerId, peerAddress, source: options?.peerAddress ? 'options' : 'config' },
      'Resolved peer address for channel opening'
    );

    // Use overrides if provided, otherwise fall back to defaults
    const settlementTimeout = options?.settlementTimeout ?? this.config.defaultSettlementTimeout;
    let initialDeposit: bigint;
    if (options?.initialDeposit !== undefined) {
      initialDeposit = options.initialDeposit;
    } else {
      const defaultInitialDeposit = BigInt(1000000000000000000); // 1 ETH/token as default
      initialDeposit = defaultInitialDeposit * BigInt(this.config.initialDepositMultiplier);
    }

    // Open channel on-chain
    const { channelId, txHash } = await this.paymentChannelSDK.openChannel(
      peerAddress,
      tokenAddress,
      settlementTimeout,
      initialDeposit
    );

    this.logger.info('Channel opened with transaction', { channelId, txHash });

    // Create metadata
    const metadata: ChannelMetadata = {
      channelId,
      peerId,
      tokenId,
      tokenAddress,
      chain: options?.chain ?? '',
      createdAt: new Date(),
      lastActivityAt: new Date(),
      status: 'open',
    };

    // Store metadata
    this.channelMetadata.set(channelId, metadata);

    // Update peer channel index
    if (!this.peerChannelIndex.has(peerId)) {
      this.peerChannelIndex.set(peerId, new Map<string, string>());
    }
    this.peerChannelIndex.get(peerId)!.set(tokenId, channelId);

    this.logger.info(
      { channelId, peerId, tokenId, initialDeposit: initialDeposit.toString() },
      'Channel opened'
    );

    return channelId;
  }

  /**
   * Check all channels for idle status
   * @private
   */
  private async checkIdleChannels(): Promise<void> {
    for (const metadata of this.channelMetadata.values()) {
      // Skip if not open
      if (metadata.status !== 'open') {
        continue;
      }

      // Check if idle
      if (!this.isChannelIdle(metadata)) {
        continue;
      }

      this.logger.info(
        { channelId: metadata.channelId, peerId: metadata.peerId },
        'Idle channel detected'
      );

      // Close channel
      await this.closeIdleChannel(metadata.channelId);
    }
  }

  /**
   * Check if channel is idle
   * @private
   */
  private isChannelIdle(metadata: ChannelMetadata): boolean {
    const idleDuration = Date.now() - metadata.lastActivityAt.getTime();
    return idleDuration > this.config.idleChannelThreshold * 1000;
  }

  /**
   * Close idle channel — starts grace period for receiver to submit claims
   * @private
   */
  private async closeIdleChannel(channelId: string): Promise<void> {
    const metadata = this.channelMetadata.get(channelId);
    if (!metadata) {
      this.logger.error({ channelId }, 'Cannot close channel: metadata not found');
      return;
    }

    // Update status to closing
    metadata.status = 'closing';

    try {
      // Close channel — starts grace period for claims
      await this.paymentChannelSDK.closeChannel(channelId, metadata.tokenAddress);

      // Schedule settle after grace period
      this.scheduleChallengeSettle(channelId, this.config.defaultSettlementTimeout);

      this.logger.info(
        {
          channelId,
          peerId: metadata.peerId,
          settlementTimeout: this.config.defaultSettlementTimeout,
        },
        'Channel close initiated, grace period started'
      );
    } catch (error) {
      this.logger.error({ channelId, error }, 'Failed to close channel');
      metadata.status = 'open';
      throw error;
    }
  }

  /**
   * Schedule settlement after challenge period
   * @private
   */
  private scheduleChallengeSettle(channelId: string, settlementTimeout: number): void {
    const settleDelayMs = settlementTimeout * 1000;
    setTimeout(async () => {
      await this.settleAfterChallenge(channelId);
    }, settleDelayMs);
    this.logger.info({ channelId, settlementTimeout }, 'Scheduled settle after challenge period');
  }

  /**
   * Settle channel after challenge period expires
   * @private
   */
  private async settleAfterChallenge(channelId: string): Promise<void> {
    const metadata = this.channelMetadata.get(channelId);
    if (!metadata) {
      this.logger.warn({ channelId }, 'Cannot settle: metadata not found');
      return;
    }

    if (metadata.status !== 'closing') {
      this.logger.warn(
        { channelId, status: metadata.status },
        'Channel not in closing state, skipping settle'
      );
      return;
    }

    try {
      await this.paymentChannelSDK.settleChannel(channelId, metadata.tokenAddress);
      metadata.status = 'closed';

      this.logger.info({ channelId }, 'Channel settled after challenge period');
    } catch (error) {
      this.logger.error({ channelId, error }, 'Failed to settle channel after challenge period');
    }
  }
}
