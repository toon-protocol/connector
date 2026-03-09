/**
 * Settlement Executor - Automated On-Chain Settlement via Payment Channels
 *
 * This module implements the SettlementExecutor class which bridges Epic 6's
 * TigerBeetle accounting system with Epic 8's payment channel SDK.
 *
 * **Functionality:**
 * - Listens to SETTLEMENT_REQUIRED events from SettlementMonitor (Epic 6 Story 6.6)
 * - Opens new payment channels when no channel exists for a peer
 * - Signs balance proofs and executes cooperative settlements via existing channels
 * - Updates TigerBeetle accounts after successful on-chain settlement
 * - Handles settlement failures with retry logic and telemetry emission
 *
 * **Integration Points:**
 * - SettlementMonitor: Receives SETTLEMENT_REQUIRED events (Epic 6 Story 6.6)
 * - PaymentChannelSDK: Executes blockchain operations (Epic 8 Story 8.7)
 * - AccountManager: Updates TigerBeetle balances (Epic 6 Story 6.4)
 * - TelemetryEmitter: Emits settlement telemetry for dashboard (Epic 6 Story 6.8)
 *
 * Source: Epic 8 Story 8.8 - Settlement Engine Integration with Payment Channels
 *
 * @module settlement/settlement-executor
 */

import EventEmitter from 'events';
import { Logger } from 'pino';
import { SettlementTriggerEvent } from '../config/types';
import { BalanceProof } from '@crosstown/shared';
import { AccountManager } from './account-manager';
import { PaymentChannelSDK } from './payment-channel-sdk';
import { SettlementMonitor } from './settlement-monitor';
import { EventStore } from '../explorer/event-store';
import { EventBroadcaster } from '../explorer/event-broadcaster';
import type { PerPacketClaimService } from './per-packet-claim-service';

/**
 * Configuration interface for SettlementExecutor
 *
 * @interface SettlementExecutorConfig
 * @property nodeId - Our connector node ID
 * @property defaultSettlementTimeout - Default challenge period for new channels (seconds, e.g., 86400 = 24h)
 * @property initialDepositMultiplier - Channel initial deposit = threshold × multiplier (default: 10)
 * @property minDepositThreshold - Add funds when deposit < threshold × multiplier × minDepositThreshold (default: 0.5)
 * @property maxRetries - Maximum retry attempts for transient failures (default: 3)
 * @property retryDelayMs - Initial retry delay in milliseconds (default: 5000ms)
 * @property tokenAddressMap - Maps tokenId (e.g., "ILP", "USDC") to ERC20 contract address
 * @property peerIdToAddressMap - Maps peerId (e.g., "connector-b") to Ethereum address
 * @property registryAddress - TokenNetworkRegistry contract address
 * @property rpcUrl - Base L2 RPC URL (e.g., http://localhost:8545)
 * @property privateKey - Connector wallet private key
 */
export interface SettlementExecutorConfig {
  nodeId: string;
  defaultSettlementTimeout: number;
  initialDepositMultiplier: number;
  minDepositThreshold: number;
  maxRetries: number;
  retryDelayMs: number;
  tokenAddressMap: Map<string, string>;
  peerIdToAddressMap: Map<string, string>;
  registryAddress: string;
  rpcUrl: string;
  privateKey: string;
}

/**
 * TelemetryEmitter interface for settlement telemetry events
 * Source: Epic 6 Story 6.8 - Telemetry System
 */
export interface TelemetryEmitter {
  emit(event: Record<string, unknown>): void;
}

/**
 * SettlementExecutor Class
 *
 * Executes automated on-chain settlements via payment channels when
 * TigerBeetle balances exceed configured thresholds.
 *
 * **Settlement Flow:**
 * 1. Receive SETTLEMENT_REQUIRED event from SettlementMonitor
 * 2. Mark settlement as IN_PROGRESS in SettlementMonitor
 * 3. Check if payment channel exists for peer
 * 4a. If no channel: Open new channel with initial deposit
 * 4b. If channel exists: Generate balance proof and cooperative settle
 * 5. Update TigerBeetle accounts after on-chain confirmation
 * 6. Mark settlement as COMPLETED in SettlementMonitor
 * 7. Emit telemetry for settlement outcome
 *
 * **Error Handling:**
 * - Transient errors (network failures, gas spikes): Retry with exponential backoff
 * - Permanent errors (insufficient funds, channel closed): Log error, emit telemetry, halt
 * - Settlement failures leave state as IN_PROGRESS for manual intervention
 *
 * @class SettlementExecutor
 * @extends EventEmitter
 */
export class SettlementExecutor extends EventEmitter {
  private readonly config: SettlementExecutorConfig;
  private readonly accountManager: AccountManager;
  private readonly paymentChannelSDK: PaymentChannelSDK;
  private readonly settlementMonitor: SettlementMonitor;
  private readonly logger: Logger;
  private readonly telemetryEmitter?: TelemetryEmitter;
  private readonly boundHandleSettlement: (event: SettlementTriggerEvent) => Promise<void>;
  private eventStore: EventStore | null = null;
  private eventBroadcaster: EventBroadcaster | null = null;
  private perPacketClaimService: PerPacketClaimService | null = null;

  /**
   * Constructor
   *
   * Initializes the settlement executor with required dependencies.
   * Binds event handler ONCE in constructor to enable proper cleanup.
   *
   * @param config - Settlement executor configuration
   * @param accountManager - TigerBeetle account manager
   * @param paymentChannelSDK - Payment channel blockchain SDK
   * @param settlementMonitor - Settlement threshold monitor
   * @param logger - Pino logger instance
   * @param telemetryEmitter - Optional telemetry emitter for monitoring
   */
  constructor(
    config: SettlementExecutorConfig,
    accountManager: AccountManager,
    paymentChannelSDK: PaymentChannelSDK,
    settlementMonitor: SettlementMonitor,
    logger: Logger,
    telemetryEmitter?: TelemetryEmitter
  ) {
    super();
    this.config = config;
    this.accountManager = accountManager;
    this.paymentChannelSDK = paymentChannelSDK;
    this.settlementMonitor = settlementMonitor;
    this.telemetryEmitter = telemetryEmitter;

    // Create child logger with component context
    this.logger = logger.child({ component: 'settlement-executor' });

    // CRITICAL: Bind event handler ONCE in constructor for proper cleanup
    // Source: docs/architecture/test-strategy-and-standards.md Anti-Pattern 1
    this.boundHandleSettlement = this.handleSettlement.bind(this);

    this.logger.info(
      {
        nodeId: config.nodeId,
        registryAddress: config.registryAddress,
        defaultSettlementTimeout: config.defaultSettlementTimeout,
      },
      'Settlement executor initialized'
    );
  }

  /**
   * Set EventStore reference for direct event emission in standalone mode
   * @param eventStore - EventStore instance for storing settlement events
   */
  setEventStore(eventStore: EventStore | null): void {
    this.eventStore = eventStore;
    this.logger.info(
      { hasEventStore: eventStore !== null },
      'EventStore reference set for settlement event emission'
    );
  }

  /**
   * Set EventBroadcaster reference for real-time event streaming
   * @param broadcaster - EventBroadcaster instance for live settlement events
   */
  setEventBroadcaster(broadcaster: EventBroadcaster | null): void {
    this.eventBroadcaster = broadcaster;
    this.logger.info(
      { hasBroadcaster: broadcaster !== null },
      'EventBroadcaster reference set for settlement event streaming'
    );
  }

  /**
   * Set PerPacketClaimService for using latest per-packet claims in on-chain settlement
   * @param service - PerPacketClaimService instance
   */
  setPerPacketClaimService(service: PerPacketClaimService): void {
    this.perPacketClaimService = service;
    this.logger.info('PerPacketClaimService set for on-chain settlement');
  }

  /**
   * Start listening for settlement events
   *
   * Registers event listener for SETTLEMENT_REQUIRED events from SettlementMonitor.
   * Uses stored bound handler reference for proper cleanup in stop().
   *
   * Source: docs/architecture/test-strategy-and-standards.md Anti-Pattern 1
   */
  start(): void {
    this.settlementMonitor.on('SETTLEMENT_REQUIRED', this.boundHandleSettlement);
    this.logger.info('Settlement executor started');
  }

  /**
   * Stop listening for settlement events
   *
   * Unregisters event listener using the same bound handler reference.
   * CRITICAL: Must use same reference for cleanup to succeed.
   *
   * Source: docs/architecture/test-strategy-and-standards.md Anti-Pattern 1
   */
  stop(): void {
    this.settlementMonitor.off('SETTLEMENT_REQUIRED', this.boundHandleSettlement);
    this.logger.info('Settlement executor stopped');
  }

  /**
   * Handle settlement event
   *
   * Main event handler that processes SETTLEMENT_REQUIRED events.
   * Wraps executeSettlement in error handling and state management.
   *
   * **State Transitions:**
   * 1. Mark settlement IN_PROGRESS immediately
   * 2. Execute settlement (open channel or cooperative settle)
   * 3. On success: Mark settlement COMPLETED
   * 4. On error: Log error, emit telemetry, leave state IN_PROGRESS
   *
   * Source: Epic 6 Story 6.6 Settlement Monitor state machine
   *
   * @param event - Settlement trigger event from SettlementMonitor
   * @private
   */
  private async handleSettlement(event: SettlementTriggerEvent): Promise<void> {
    this.logger.info(
      {
        peerId: event.peerId,
        tokenId: event.tokenId,
        currentBalance: event.currentBalance.toString(),
        threshold: event.threshold.toString(),
        exceedsBy: event.exceedsBy.toString(),
      },
      'Settlement event received'
    );

    // Mark settlement IN_PROGRESS immediately
    // Source: Epic 6 Story 6.6 SettlementMonitor.markSettlementInProgress
    this.settlementMonitor.markSettlementInProgress(event.peerId, event.tokenId);
    this.logger.info(
      { peerId: event.peerId, tokenId: event.tokenId },
      'Marked settlement in progress'
    );

    // Emit telemetry: SETTLEMENT_STARTED
    this.emitSettlementTelemetry('SETTLEMENT_STARTED', event.peerId, event.tokenId, {
      currentBalance: event.currentBalance.toString(),
      threshold: event.threshold.toString(),
      exceedsBy: event.exceedsBy.toString(),
    });

    try {
      // Execute settlement logic
      await this.executeSettlement(event);

      // Mark settlement COMPLETED after success
      this.settlementMonitor.markSettlementCompleted(event.peerId, event.tokenId);
      this.logger.info(
        { peerId: event.peerId, tokenId: event.tokenId },
        'Settlement completed, state reset to IDLE'
      );

      // Emit telemetry: SETTLEMENT_COMPLETED
      this.emitSettlementTelemetry('SETTLEMENT_COMPLETED', event.peerId, event.tokenId, {
        currentBalance: event.currentBalance.toString(),
      });
    } catch (error) {
      // Log error but do NOT call markSettlementCompleted
      // State remains IN_PROGRESS for manual intervention
      const errorMessage = error instanceof Error ? error.message : String(error);
      const errorStack = error instanceof Error ? error.stack : undefined;

      this.logger.error(
        {
          errorMessage,
          errorStack,
          errorType: error?.constructor?.name,
          peerId: event.peerId,
          tokenId: event.tokenId,
        },
        'Settlement failed'
      );

      // Emit telemetry: SETTLEMENT_FAILED
      this.emitSettlementTelemetry('SETTLEMENT_FAILED', event.peerId, event.tokenId, {
        error: errorMessage,
        currentBalance: event.currentBalance.toString(),
      });
    }
  }

  /**
   * Execute settlement logic
   *
   * Main settlement execution flow:
   * 1. Get token address from tokenId
   * 2. Find existing payment channel for peer
   * 3. If no channel: Open new channel and deposit
   * 4. If channel exists: Sign balance proof and cooperative settle
   * 5. Update TigerBeetle accounts
   *
   * @param event - Settlement trigger event
   * @private
   */
  private async executeSettlement(event: SettlementTriggerEvent): Promise<void> {
    const { peerId, tokenId, currentBalance } = event;

    this.logger.info(
      { peerId, tokenId, currentBalance: currentBalance.toString() },
      'Executing settlement'
    );

    // Get token address from configuration
    const tokenAddress = this.config.tokenAddressMap.get(tokenId);
    if (!tokenAddress) {
      this.logger.error(
        { tokenId, availableTokens: Array.from(this.config.tokenAddressMap.keys()) },
        'Token address not found'
      );
      throw new Error(`Token address not found for tokenId: ${tokenId}`);
    }

    this.logger.debug({ tokenId, tokenAddress }, 'Token address resolved');

    // Find existing channel for peer
    this.logger.debug({ peerId, tokenAddress }, 'Searching for existing channel');
    const channelId = await this.findChannelForPeer(peerId, tokenAddress);

    if (!channelId) {
      // No existing channel: Open new channel and deposit
      this.logger.info(
        { peerId, tokenId, tokenAddress },
        'No existing channel found, opening new channel'
      );
      await this.openChannelAndSettle(peerId, tokenId, tokenAddress, currentBalance);
    } else {
      // Existing channel: Sign balance proof and cooperative settle
      this.logger.info({ peerId, tokenId, channelId }, 'Using existing channel for settlement');
      await this.settleViaExistingChannel(channelId, tokenAddress, peerId, tokenId, currentBalance);
    }
  }

  /**
   * Find existing payment channel for peer
   *
   * Queries all channels for the token and filters by peer address.
   * Returns channelId if an active channel exists, null otherwise.
   *
   * @param peerId - Peer connector ID
   * @param tokenAddress - ERC20 token contract address
   * @returns channelId if found, null otherwise
   * @private
   */
  private async findChannelForPeer(peerId: string, tokenAddress: string): Promise<string | null> {
    try {
      // Get peer's Ethereum address
      const peerAddress = this.config.peerIdToAddressMap.get(peerId);
      if (!peerAddress) {
        this.logger.warn({ peerId }, 'Peer address not found in configuration');
        return null;
      }

      // Query all channels for this token
      const channelIds = await this.paymentChannelSDK.getMyChannels(tokenAddress);

      // Filter channels by peer address and status='opened'
      for (const channelId of channelIds) {
        const channelState = await this.paymentChannelSDK.getChannelState(channelId, tokenAddress);
        if (
          channelState.status === 'opened' &&
          channelState.participants.includes(peerAddress.toLowerCase())
        ) {
          return channelId;
        }
      }

      return null;
    } catch (error) {
      this.logger.error({ error, peerId, tokenAddress }, 'Failed to find channel for peer');
      return null; // Treat as no channel exists
    }
  }

  /**
   * Open new channel and deposit initial funds
   *
   * Opens a new payment channel with initial deposit based on
   * currentBalance × initialDepositMultiplier.
   *
   * After channel open, updates TigerBeetle to reduce creditBalance.
   *
   * @param peerId - Peer connector ID
   * @param tokenId - Token identifier
   * @param tokenAddress - ERC20 token contract address
   * @param amount - Amount to deposit (current balance from event)
   * @returns channelId of newly opened channel
   * @private
   */
  private async openChannelAndSettle(
    peerId: string,
    tokenId: string,
    tokenAddress: string,
    amount: bigint
  ): Promise<string> {
    // Calculate initial deposit
    const initialDeposit = amount * BigInt(this.config.initialDepositMultiplier);

    // Get peer address
    const peerAddress = this.config.peerIdToAddressMap.get(peerId);
    if (!peerAddress) {
      throw new Error(`Peer address not found for peerId: ${peerId}`);
    }

    this.logger.info(
      {
        peerId,
        tokenId,
        tokenAddress,
        peerAddress,
        initialDeposit: initialDeposit.toString(),
      },
      'Opening new payment channel'
    );

    // Open channel with retry logic
    const { channelId, txHash } = await this.retryWithBackoff(
      async () =>
        await this.paymentChannelSDK.openChannel(
          peerAddress,
          tokenAddress,
          this.config.defaultSettlementTimeout,
          initialDeposit
        ),
      'openChannel',
      this.config.maxRetries
    );

    this.logger.info(
      {
        channelId,
        peerId,
        tokenId,
        initialDeposit: initialDeposit.toString(),
        txHash,
      },
      'Channel opened for settlement'
    );

    // Update TigerBeetle: Record settlement to reduce creditBalance
    // We deposited funds into channel, so peer's debt to us decreases
    await this.accountManager.recordSettlement(peerId, tokenId, amount);

    this.logger.info(
      { peerId, tokenId, amount: amount.toString() },
      'TigerBeetle balance updated after channel deposit'
    );

    // Emit settlement completed event with transaction hash
    this.emitSettlementTelemetry('SETTLEMENT_COMPLETED', peerId, tokenId, {
      channelId,
      transactionHash: txHash,
      settledAmount: amount.toString(), // Use settledAmount to match extractIndexedFields expectations
      initialDeposit: initialDeposit.toString(),
      settlementType: 'channel_opened',
    });

    // Emit CHANNEL_ACTIVITY event for ChannelManager
    this.emit('CHANNEL_ACTIVITY', { channelId });

    return channelId;
  }

  /**
   * Settle via existing payment channel
   *
   * Signs balance proof and executes cooperative settlement.
   * Checks if channel deposit is sufficient, adds funds if needed.
   *
   * @param channelId - Payment channel ID
   * @param tokenAddress - ERC20 token address
   * @param peerId - Peer connector ID
   * @param tokenId - Token identifier
   * @param amount - Amount to settle
   * @private
   */
  private async settleViaExistingChannel(
    channelId: string,
    tokenAddress: string,
    peerId: string,
    tokenId: string,
    amount: bigint
  ): Promise<void> {
    // Query channel state
    const channelState = await this.paymentChannelSDK.getChannelState(channelId, tokenAddress);

    // Check if deposit needs to be increased
    // Calculate: amount * initialDepositMultiplier * minDepositThreshold
    const minDepositFloat =
      Number(amount) * this.config.initialDepositMultiplier * this.config.minDepositThreshold;
    const minDeposit = BigInt(Math.floor(minDepositFloat));
    if (channelState.myDeposit < minDeposit) {
      await this.depositAdditionalFunds(channelId, tokenAddress, amount);
      // Refresh channel state after deposit
      const updatedState = await this.paymentChannelSDK.getChannelState(channelId, tokenAddress);
      channelState.myDeposit = updatedState.myDeposit;
    }

    // Check if deposit is sufficient for settlement
    if (channelState.myDeposit < amount) {
      await this.depositAdditionalFunds(channelId, tokenAddress, amount);
    }

    // Use latest per-packet claim if available, otherwise generate fresh balance proof
    let myBalanceProof: BalanceProof;
    let mySignature: string;

    const latestClaim = this.perPacketClaimService?.getLatestClaim(channelId);
    if (latestClaim) {
      // Per-packet claims already accumulated the correct cumulative state
      myBalanceProof = {
        channelId,
        nonce: latestClaim.nonce,
        transferredAmount: BigInt(latestClaim.transferredAmount),
        lockedAmount: BigInt(latestClaim.lockedAmount),
        locksRoot: latestClaim.locksRoot,
      };
      mySignature = latestClaim.signature;

      this.logger.info(
        {
          channelId,
          nonce: latestClaim.nonce,
          transferred: latestClaim.transferredAmount,
        },
        'Using latest per-packet claim for on-chain settlement'
      );
    } else {
      // Fallback: calculate new balance proof (legacy path)
      const newNonce = channelState.myNonce + 1;
      const newTransferred = channelState.myTransferred + amount;

      this.logger.info(
        {
          channelId,
          newNonce,
          newTransferred: newTransferred.toString(),
          amount: amount.toString(),
        },
        'Signing balance proof for cooperative settlement'
      );

      myBalanceProof = {
        channelId,
        nonce: newNonce,
        transferredAmount: newTransferred,
        lockedAmount: 0n,
        locksRoot: '0x' + '0'.repeat(64),
      };

      mySignature = await this.retryWithBackoff(
        async () =>
          await this.paymentChannelSDK.signBalanceProof(
            channelId,
            myBalanceProof.nonce,
            myBalanceProof.transferredAmount,
            0n,
            '0x' + '0'.repeat(64)
          ),
        'signBalanceProof',
        this.config.maxRetries
      );
    }

    // In a real implementation, we would exchange balance proofs with peer off-chain
    // For this story, we simulate the peer also signing their proof
    // Their state remains unchanged (no transfers from them)
    const theirBalanceProof: BalanceProof = {
      channelId,
      nonce: channelState.theirNonce,
      transferredAmount: channelState.theirTransferred,
      lockedAmount: 0n,
      locksRoot: '0x' + '0'.repeat(64),
    };

    // Simulate peer signature (in production, peer would sign their proof)
    const theirSignature = mySignature; // Placeholder

    this.logger.info(
      {
        channelId,
        myNonce: myBalanceProof.nonce,
        myTransferred: myBalanceProof.transferredAmount.toString(),
        theirNonce: theirBalanceProof.nonce,
        theirTransferred: theirBalanceProof.transferredAmount.toString(),
      },
      'Executing cooperative settlement'
    );

    // Execute cooperative settlement
    await this.retryWithBackoff(
      async () =>
        await this.paymentChannelSDK.cooperativeSettle(
          channelId,
          tokenAddress,
          myBalanceProof,
          mySignature,
          theirBalanceProof,
          theirSignature
        ),
      'cooperativeSettle',
      this.config.maxRetries
    );

    this.logger.info(
      { channelId, amount: amount.toString() },
      'Settlement completed via existing channel'
    );

    // Update TigerBeetle after successful settlement
    await this.accountManager.recordSettlement(peerId, tokenId, amount);

    // Reset per-packet claim tracking after successful on-chain settlement
    if (this.perPacketClaimService) {
      this.perPacketClaimService.resetChannel(channelId);
    }

    this.logger.info(
      { peerId, tokenId, amount: amount.toString() },
      'TigerBeetle balance updated after settlement'
    );

    // Emit CHANNEL_ACTIVITY event for ChannelManager
    this.emit('CHANNEL_ACTIVITY', { channelId });
  }

  /**
   * Add additional funds to channel
   *
   * Deposits additional funds when channel deposit falls below minimum threshold.
   * Target deposit = currentBalance × initialDepositMultiplier
   *
   * @param channelId - Payment channel ID
   * @param tokenAddress - ERC20 token address
   * @param requiredAmount - Amount required for settlement
   * @private
   */
  private async depositAdditionalFunds(
    channelId: string,
    tokenAddress: string,
    requiredAmount: bigint
  ): Promise<void> {
    const channelState = await this.paymentChannelSDK.getChannelState(channelId, tokenAddress);
    const targetDeposit = requiredAmount * BigInt(this.config.initialDepositMultiplier);
    const additionalDeposit = targetDeposit - channelState.myDeposit;

    if (additionalDeposit <= 0n) {
      return; // No additional deposit needed
    }

    this.logger.info(
      {
        channelId,
        currentDeposit: channelState.myDeposit.toString(),
        targetDeposit: targetDeposit.toString(),
        additionalDeposit: additionalDeposit.toString(),
      },
      'Adding funds to channel'
    );

    await this.retryWithBackoff(
      async () => await this.paymentChannelSDK.deposit(channelId, tokenAddress, additionalDeposit),
      'deposit',
      this.config.maxRetries
    );

    this.logger.info(
      { channelId, additionalDeposit: additionalDeposit.toString() },
      'Added funds to channel'
    );
  }

  /**
   * Retry operation with exponential backoff
   *
   * Retries transient failures with exponential backoff delay.
   * Throws immediately on non-retryable errors.
   *
   * Retry delays: 5s, 10s, 20s (configurable via retryDelayMs)
   *
   * @param operation - Async operation to retry
   * @param operationName - Name for logging
   * @param maxRetries - Maximum retry attempts
   * @returns Result of operation
   * @throws Error if all retries exhausted or non-retryable error
   * @private
   */
  private async retryWithBackoff<T>(
    operation: () => Promise<T>,
    operationName: string,
    maxRetries: number
  ): Promise<T> {
    let lastError: Error | undefined;

    for (let attempt = 1; attempt <= maxRetries; attempt++) {
      try {
        return await operation();
      } catch (error) {
        lastError = error instanceof Error ? error : new Error(String(error));

        // Check if error is retryable
        if (!this.isRetryableError(lastError)) {
          this.logger.error({ error: lastError, operationName }, 'Non-retryable error, aborting');
          throw lastError;
        }

        if (attempt < maxRetries) {
          const delayMs = this.config.retryDelayMs * 2 ** (attempt - 1);
          this.logger.warn(
            { attempt, maxRetries, operationName, delayMs, error: lastError.message },
            'Retrying settlement operation'
          );
          await new Promise((resolve) => setTimeout(resolve, delayMs));
        }
      }
    }

    this.logger.error({ operationName, maxRetries }, 'Max retries exhausted');
    throw lastError;
  }

  /**
   * Check if error is retryable
   *
   * Determines if an error is transient and should be retried.
   *
   * **Retryable errors:**
   * - Network timeouts
   * - Gas price too high
   * - Nonce too low (transaction pending)
   *
   * **Non-retryable errors:**
   * - Insufficient funds
   * - Channel closed
   * - Invalid signature
   * - ChallengeNotExpiredError (settlement timing)
   *
   * @param error - Error to check
   * @returns true if retryable, false otherwise
   * @private
   */
  private isRetryableError(error: Error): boolean {
    const errorMessage = error.message.toLowerCase();

    // Retryable errors
    if (
      errorMessage.includes('timeout') ||
      errorMessage.includes('network') ||
      errorMessage.includes('gas price') ||
      errorMessage.includes('nonce too low') ||
      errorMessage.includes('replacement') || // Transaction replacement errors
      errorMessage.includes('already known') || // Transaction already in mempool
      errorMessage.includes('nonce has already been used') // Nonce conflict
    ) {
      return true;
    }

    // Non-retryable errors
    if (
      errorMessage.includes('insufficient funds') ||
      errorMessage.includes('channel closed') ||
      errorMessage.includes('invalid signature') ||
      errorMessage.includes('challenge not expired') ||
      error.constructor.name === 'ChallengeNotExpiredError'
    ) {
      return false;
    }

    // Default: Treat unknown errors as non-retryable for safety
    return false;
  }

  /**
   * Emit settlement telemetry event
   *
   * Emits telemetry events for settlement monitoring and alerting.
   * Telemetry emission is non-blocking per coding standards.
   *
   * Source: docs/architecture/coding-standards.md telemetry emission is non-blocking
   *
   * @param eventType - Event type (SETTLEMENT_STARTED | SETTLEMENT_COMPLETED | SETTLEMENT_FAILED)
   * @param peerId - Peer connector ID
   * @param tokenId - Token identifier
   * @param details - Additional event details
   * @private
   */
  private emitSettlementTelemetry(
    eventType: 'SETTLEMENT_STARTED' | 'SETTLEMENT_COMPLETED' | 'SETTLEMENT_FAILED',
    peerId: string,
    tokenId: string,
    details: Record<string, unknown>
  ): void {
    const event = {
      type: eventType,
      nodeId: this.config.nodeId || 'unknown',
      peerId: peerId || 'unknown',
      tokenId: tokenId || 'unknown',
      timestamp: new Date().toISOString(),
      // Add all details, filtering out undefined values
      ...Object.fromEntries(Object.entries(details).filter(([_, v]) => v !== undefined)),
    };

    // Emit via TelemetryEmitter if configured
    if (this.telemetryEmitter) {
      try {
        this.telemetryEmitter.emit(event);
      } catch (error) {
        this.logger.error({ error }, 'Failed to emit settlement telemetry');
      }
    }

    // Also emit to EventStore in standalone mode
    if (this.eventStore) {
      try {
        // Log the event structure for debugging
        this.logger.debug(
          { event, eventKeys: Object.keys(event) },
          'Attempting to store settlement event'
        );

        // Cast to any to bypass strict type checking - EventStore handles various event shapes
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        this.eventStore.storeEvent(event as any).catch((err: Error) => {
          this.logger.warn(
            {
              error: err.message,
              errorType: err.constructor.name,
              eventType,
              event: JSON.stringify(event),
            },
            'Failed to store settlement event'
          );
        });
        // Broadcast to WebSocket clients for real-time updates
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        this.eventBroadcaster?.broadcast(event as any);
      } catch (error) {
        this.logger.error({ error, event }, 'Failed to store settlement event in EventStore');
      }
    }
  }

  /**
   * Get settlement state for peer-token pair
   *
   * Queries the SettlementMonitor for current settlement state.
   * Useful for debugging and monitoring.
   *
   * @param peerId - Peer connector ID
   * @param tokenId - Token identifier
   * @returns Current settlement state
   */
  getSettlementState(peerId: string, tokenId: string): string {
    return this.settlementMonitor.getSettlementState(peerId, tokenId);
  }
}
