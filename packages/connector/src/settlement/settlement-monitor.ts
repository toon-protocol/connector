/**
 * Settlement Monitor
 *
 * Event-driven settlement monitor that subscribes to ClaimReceiver events
 * and triggers settlement when cumulative claim amounts exceed configured
 * thresholds. Replaces the previous polling-based approach with immediate
 * reaction to incoming claims.
 *
 * **Threshold Detection Strategy:**
 * - Subscribes to ClaimReceiver 'CLAIM_RECEIVED' events
 * - Checks cumulative transferred amount against configured thresholds
 * - Emits SETTLEMENT_REQUIRED event on first threshold crossing
 * - Prevents duplicate triggers using state machine (IDLE → PENDING → IN_PROGRESS → IDLE)
 *
 * **Threshold Hierarchy:**
 * 1. Token-specific threshold (highest priority)
 * 2. Per-peer threshold
 * 3. Default threshold
 * 4. No threshold (monitoring disabled)
 *
 * **Integration Points:**
 * - ClaimReceiver: Subscribes to CLAIM_RECEIVED events
 * - SettlementExecutor: Listens for SETTLEMENT_REQUIRED events → calls claimFromChannel()
 *
 * @packageDocumentation
 */

import { EventEmitter } from 'events';
import type { Logger } from 'pino';
import type { SettlementThresholdConfig, SettlementTriggerEvent } from '../config/types';
import { SettlementState } from '../config/types';
import type { ClaimReceiver, ClaimReceivedEvent } from './claim-receiver';

/**
 * Settlement Monitor Configuration
 *
 * Defines peers and tokens to monitor for settlement threshold detection.
 *
 * @property thresholds - Settlement threshold configuration
 * @property peers - List of peer IDs to monitor
 * @property tokenIds - List of token IDs to monitor (default: ['M2M'] for MVP)
 */
export interface SettlementMonitorConfig {
  /**
   * Settlement threshold configuration
   * Contains default, per-peer, and token-specific thresholds
   * Optional - if undefined, monitoring is disabled
   */
  thresholds?: SettlementThresholdConfig;

  /**
   * List of peer IDs to monitor
   * Must match peer IDs from connector configuration
   * Example: ['connector-a', 'connector-b']
   */
  peers: string[];

  /**
   * List of token IDs to monitor
   * Default: ['M2M'] for MVP (single currency, resolved from on-chain symbol)
   * Future: ['M2M', 'USDC', 'BTC', 'ETH'] for multi-token support
   */
  tokenIds: string[];
}

/**
 * Settlement Monitor
 *
 * Event-driven monitor that subscribes to ClaimReceiver events and emits
 * settlement trigger events when cumulative claim amounts exceed thresholds.
 * Implements state machine to prevent duplicate triggers and coordinates
 * with SettlementExecutor for on-chain claimFromChannel() execution.
 *
 * **Events:**
 * - 'SETTLEMENT_REQUIRED': Emitted when cumulative claim amount exceeds threshold (SettlementTriggerEvent payload)
 *
 * **State Transitions:**
 * - IDLE → SETTLEMENT_PENDING: Claim amount exceeds threshold (first crossing)
 * - SETTLEMENT_PENDING → SETTLEMENT_IN_PROGRESS: SettlementExecutor starts execution
 * - SETTLEMENT_IN_PROGRESS → IDLE: Settlement completes (claimFromChannel() succeeds)
 *
 * @example
 * ```typescript
 * const monitor = new SettlementMonitor(config, logger);
 * monitor.setClaimReceiver(claimReceiver);
 *
 * // Listen for settlement triggers
 * monitor.on('SETTLEMENT_REQUIRED', async (event: SettlementTriggerEvent) => {
 *   console.log(`Settlement needed for ${event.peerId}: ${event.currentBalance} > ${event.threshold}`);
 *   await settlementExecutor.claimFromChannel(event);
 * });
 *
 * // Start monitoring (subscribes to ClaimReceiver events)
 * monitor.start();
 *
 * // Stop monitoring (unsubscribes from ClaimReceiver events)
 * monitor.stop();
 * ```
 */
export class SettlementMonitor extends EventEmitter {
  private readonly _config: SettlementMonitorConfig;
  private readonly _logger: Logger;
  private readonly _settlementStates: Map<string, SettlementState>;
  private readonly _lastSettlementTime: Map<string, number>;
  private _claimReceiver: ClaimReceiver | null;
  private readonly _boundHandleClaimReceived: (event: ClaimReceivedEvent) => void;
  private _isRunning: boolean;

  /**
   * Create a new SettlementMonitor
   *
   * @param config - Settlement monitor configuration
   * @param logger - Pino logger instance
   */
  constructor(config: SettlementMonitorConfig, logger: Logger) {
    super();

    this._config = config;
    this._logger = logger.child({ component: 'settlement-monitor' });
    this._settlementStates = new Map();
    this._lastSettlementTime = new Map();
    this._claimReceiver = null;
    this._boundHandleClaimReceived = this._handleClaimReceived.bind(this);
    this._isRunning = false;

    // Initialize settlement states: All peers start in IDLE state
    for (const peerId of this._config.peers) {
      for (const tokenId of this._config.tokenIds) {
        const stateKey = `${peerId}:${tokenId}`;
        this._settlementStates.set(stateKey, SettlementState.IDLE);
      }
    }

    this._logger.info(
      {
        defaultThreshold: this._config.thresholds?.defaultThreshold?.toString(),
        peerCount: this._config.peers.length,
        tokenCount: this._config.tokenIds.length,
      },
      'Settlement monitor initialized (event-driven)'
    );
  }

  /**
   * Set ClaimReceiver for event-driven settlement monitoring
   *
   * Must be called before start() to enable claim-driven threshold checks.
   * When a verified claim is received, the monitor checks the cumulative
   * amount against the settlement threshold and triggers claimFromChannel()
   * if exceeded.
   *
   * @param claimReceiver - ClaimReceiver instance to subscribe to
   */
  setClaimReceiver(claimReceiver: ClaimReceiver): void {
    this._claimReceiver = claimReceiver;
    this._logger.info('ClaimReceiver set for event-driven settlement monitoring');
  }

  /**
   * Start settlement threshold monitoring
   *
   * Subscribes to ClaimReceiver 'CLAIM_RECEIVED' events for event-driven
   * threshold detection. No polling interval is used.
   *
   * @throws Error if monitor is already running
   */
  start(): void {
    if (this._isRunning) {
      throw new Error('Settlement monitor already running');
    }

    this._isRunning = true;

    if (this._claimReceiver) {
      this._claimReceiver.on('CLAIM_RECEIVED', this._boundHandleClaimReceived);
      this._logger.info('Settlement monitor started (subscribed to ClaimReceiver events)');
    } else {
      this._logger.warn(
        'Settlement monitor started without ClaimReceiver — no events will be processed. ' +
          'Call setClaimReceiver() and restart to enable event-driven settlement.'
      );
    }
  }

  /**
   * Stop settlement threshold monitoring
   *
   * Unsubscribes from ClaimReceiver events.
   */
  stop(): void {
    if (this._claimReceiver) {
      this._claimReceiver.off('CLAIM_RECEIVED', this._boundHandleClaimReceived);
    }

    this._isRunning = false;
    this._logger.info('Settlement monitor stopped');
  }

  /**
   * Get threshold for a specific peer-token pair
   *
   * Implements threshold hierarchy:
   * 1. Token-specific threshold (highest priority)
   * 2. Per-peer threshold
   * 3. Default threshold
   * 4. No threshold (undefined = monitoring disabled)
   *
   * @param peerId - Peer ID
   * @param tokenId - Token ID
   * @returns Settlement threshold or undefined if no threshold configured
   *
   * @private
   */
  private _getThresholdForPeer(peerId: string, tokenId: string): bigint | undefined {
    // Return undefined if no threshold config (monitoring disabled)
    if (!this._config.thresholds) {
      return undefined;
    }

    // Check token-specific threshold first (highest priority)
    const tokenSpecificThreshold = this._config.thresholds.perTokenThresholds
      ?.get(peerId)
      ?.get(tokenId);
    if (tokenSpecificThreshold !== undefined) {
      return tokenSpecificThreshold;
    }

    // Check per-peer threshold second
    const perPeerThreshold = this._config.thresholds.perPeerThresholds?.get(peerId);
    if (perPeerThreshold !== undefined) {
      return perPeerThreshold;
    }

    // Return default threshold (may be undefined)
    return this._config.thresholds.defaultThreshold;
  }

  /**
   * Handle incoming claim event from ClaimReceiver
   *
   * Checks the cumulative transferred amount against the settlement threshold
   * for the peer. If the threshold is exceeded and the state is IDLE, emits
   * a SETTLEMENT_REQUIRED event which triggers SettlementExecutor to call
   * claimFromChannel() on-chain. The channel stays open for continued use.
   *
   * @param event - Claim received event with cumulative amount
   * @private
   */
  private _handleClaimReceived(event: ClaimReceivedEvent): void {
    try {
      const { peerId, channelId, cumulativeAmount } = event;

      this._logger.debug(
        { peerId, channelId, cumulativeAmount: cumulativeAmount.toString() },
        'Processing claim for settlement threshold check'
      );

      // Check threshold for this peer across all configured tokenIds
      for (const tokenId of this._config.tokenIds) {
        const threshold = this._getThresholdForPeer(peerId, tokenId);

        // Skip if no threshold configured
        if (!threshold) {
          continue;
        }

        const stateKey = `${peerId}:${tokenId}`;
        const currentState = this._settlementStates.get(stateKey) ?? SettlementState.IDLE;

        if (cumulativeAmount > threshold) {
          if (currentState === SettlementState.IDLE) {
            // Threshold crossed — trigger settlement via claimFromChannel()
            const exceedsBy = cumulativeAmount - threshold;

            const triggerEvent: SettlementTriggerEvent = {
              peerId,
              tokenId,
              currentBalance: cumulativeAmount,
              threshold,
              exceedsBy,
              timestamp: new Date(),
            };

            // Emit event (SettlementExecutor listens → calls claimFromChannel())
            this.emit('SETTLEMENT_REQUIRED', triggerEvent);

            // Update state to SETTLEMENT_PENDING
            this._settlementStates.set(stateKey, SettlementState.SETTLEMENT_PENDING);

            this._logger.warn(
              {
                peerId,
                tokenId,
                channelId,
                cumulativeAmount: cumulativeAmount.toString(),
                threshold: threshold.toString(),
                exceedsBy: exceedsBy.toString(),
              },
              'Settlement threshold exceeded — triggering claimFromChannel()'
            );
          } else {
            // State is SETTLEMENT_PENDING or SETTLEMENT_IN_PROGRESS
            // Skip to prevent duplicate triggers
            this._logger.debug(
              { peerId, tokenId, state: currentState },
              'Settlement already pending/in-progress, skipping duplicate trigger'
            );
          }
        }
      }
    } catch (error) {
      this._logger.error(
        {
          error: error instanceof Error ? error.message : String(error),
        },
        'Settlement threshold check failed for claim event'
      );
    }
  }

  /**
   * Get settlement state for a peer-token pair
   *
   * @param peerId - Peer ID
   * @param tokenId - Token ID
   * @returns Current settlement state (defaults to IDLE if not found)
   */
  getSettlementState(peerId: string, tokenId: string): SettlementState {
    const stateKey = `${peerId}:${tokenId}`;
    return this._settlementStates.get(stateKey) ?? SettlementState.IDLE;
  }

  /**
   * Mark settlement as in progress
   *
   * Called by SettlementExecutor when settlement execution starts.
   * Transitions state from SETTLEMENT_PENDING to SETTLEMENT_IN_PROGRESS.
   *
   * @param peerId - Peer ID
   * @param tokenId - Token ID
   */
  markSettlementInProgress(peerId: string, tokenId: string): void {
    const stateKey = `${peerId}:${tokenId}`;
    this._settlementStates.set(stateKey, SettlementState.SETTLEMENT_IN_PROGRESS);

    this._logger.info({ peerId, tokenId }, 'Settlement marked in progress');
  }

  /**
   * Mark settlement as completed
   *
   * Called by SettlementExecutor when claimFromChannel() completes and
   * on-chain settlement succeeds. Transitions state to IDLE, ready for
   * next threshold crossing.
   *
   * @param peerId - Peer ID
   * @param tokenId - Token ID
   */
  markSettlementCompleted(peerId: string, tokenId: string): void {
    const stateKey = `${peerId}:${tokenId}`;
    this._settlementStates.set(stateKey, SettlementState.IDLE);
    this._lastSettlementTime.set(stateKey, Date.now());

    this._logger.info({ peerId, tokenId }, 'Settlement completed, state reset to IDLE');
  }

  /**
   * Get all settlement states
   *
   * Returns a copy of the internal state map for debugging.
   *
   * @returns Map of state keys to settlement states
   */
  getAllSettlementStates(): Map<string, SettlementState> {
    // Return copy to prevent external mutation
    return new Map(this._settlementStates);
  }
}
